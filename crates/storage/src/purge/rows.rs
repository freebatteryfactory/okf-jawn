//! A workspace's rows in a records database, removed or found by reading the schema itself.
//!
//! The same functions run on the live records database and on every pre-migration copy of it,
//! which may be at an older schema version, so they find the tables from `sqlite_schema` and
//! their columns from `pragma_table_info` instead of naming the current schema:
//!
//! - every table with `tenant_id` and `workspace_id` loses the workspace's rows, except the
//!   purge records (`purges`, `purge_progress`) and the revision map, which hold identities only
//!   and outlive the workspace;
//! - a table keyed by `job_id` without a workspace column loses the rows of the workspace's
//!   jobs, before the jobs go;
//! - every connector's `workspace_ids` loses the workspace;
//! - a mutation-ledger row whose stored response names the workspace is removed (the response
//!   may hold its name or an item's title; a retry then finds no response and is refused by the
//!   purged workspace);
//! - a receipt or review of another workspace of the tenant that cites the workspace is marked
//!   invalidated (a receipt's `invalidated_by`, a review's coverage), and each citing source
//!   becomes the invalidated citation (`scrub_source`): its path `purged/<item id>`, its
//!   selection `all`, without a digest or locations.
//!
//! "Holds" means holds something a purge still removes or scrubs, so a database or archive
//! already scrubbed of the workspace no longer holds it.

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::PurgeId;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::db::{json, sql};

/// One table and its column names.
struct Table {
    name: String,
    columns: Vec<String>,
}

/// Tables that outlive a purged workspace: they hold identities, never content. The pending
/// puts of the workspace stay until the objects step, which deletes the bytes they name and
/// then clears them (`puts`).
const KEPT: &[&str] = &["pending_puts", "purges", "purge_progress", "revision_map"];

/// Tables whose records of one workspace can cite another, with their key column: a receipt's
/// `sources` and a review's `source`.
const CITING: &[(&str, &str)] = &[("receipts", "receipt_id"), ("reviews", "review_id")];

impl Table {
    fn has(&self, column: &str) -> bool {
        self.columns.iter().any(|name| name == column)
    }

    /// A workspace-scoped table whose rows a purge removes.
    fn scoped(&self) -> bool {
        self.has("tenant_id") && self.has("workspace_id") && !KEPT.contains(&self.name.as_str())
    }
}

/// Whether the database holds anything of the workspace a purge removes or scrubs.
pub(crate) fn holds(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
) -> Result<bool, ApiError> {
    for table in tables(connection)? {
        let query = if table.scoped() {
            format!(
                "SELECT 1 FROM \"{}\" WHERE tenant_id = ?1 AND workspace_id = ?2 LIMIT 1",
                table.name
            )
        } else if table.name == "mutations" && table.has("response") {
            "SELECT 1 FROM mutations WHERE tenant_id = ?1 AND instr(response, ?2) > 0 LIMIT 1"
                .to_owned()
        } else if table.name == "connectors" && table.has("record") {
            "SELECT 1 FROM connectors WHERE ?1 = ?1 AND instr(record, ?2) > 0 LIMIT 1".to_owned()
        } else {
            continue;
        };
        let found: Option<i64> = connection
            .query_row(&query, params![tenant, workspace], |row| row.get(0))
            .optional()
            .map_err(|error| sql(&error))?;
        if found.is_some() {
            return Ok(true);
        }
    }
    Ok(citations(connection, tenant, workspace, None)? > 0)
}

/// Remove the workspace's rows and scrub what cites it; see the module header. Run it inside
/// one transaction. Repeating it changes nothing more.
pub(crate) fn remove(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
    purge: PurgeId,
) -> Result<(), ApiError> {
    let tables = tables(connection)?;
    let has_jobs = tables.iter().any(|table| table.name == "jobs");
    for table in &tables {
        if has_jobs && table.name != "jobs" && table.has("job_id") && !table.has("workspace_id") {
            execute(
                connection,
                &format!(
                    "DELETE FROM \"{}\" WHERE job_id IN
                         (SELECT job_id FROM jobs WHERE tenant_id = ?1 AND workspace_id = ?2)",
                    table.name
                ),
                tenant,
                workspace,
            )?;
        }
    }
    for table in &tables {
        if table.scoped() {
            execute(
                connection,
                &format!(
                    "DELETE FROM \"{}\" WHERE tenant_id = ?1 AND workspace_id = ?2",
                    table.name
                ),
                tenant,
                workspace,
            )?;
        }
    }
    if tables
        .iter()
        .any(|table| table.name == "mutations" && table.has("response"))
    {
        execute(
            connection,
            "DELETE FROM mutations WHERE tenant_id = ?1 AND instr(response, ?2) > 0",
            tenant,
            workspace,
        )?;
    }
    if tables
        .iter()
        .any(|table| table.name == "connectors" && table.has("record"))
    {
        unlist_from_connectors(connection, workspace)?;
    }
    citations(connection, tenant, workspace, Some(purge))?;
    Ok(())
}

/// The stored citations of other workspaces' receipts and reviews that cite the workspace and
/// are not yet scrubbed. With `scrub`, each is scrubbed (`scrub_source`) and its record marked
/// invalidated: a receipt gets `invalidated_by` that purge, a review the coverage `invalidated`.
pub(crate) fn citations(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
    scrub: Option<PurgeId>,
) -> Result<u32, ApiError> {
    let tables = tables(connection)?;
    let mut count: u32 = 0;
    for (table, key) in CITING {
        if !tables
            .iter()
            .any(|found| found.name == *table && found.has("record"))
        {
            continue;
        }
        let rows: Vec<(String, String)> = {
            let mut statement = connection
                .prepare(&format!(
                    "SELECT {key}, record FROM {table}
                     WHERE tenant_id = ?1 AND workspace_id <> ?2 AND instr(record, ?2) > 0"
                ))
                .map_err(|error| sql(&error))?;
            statement
                .query_map(params![tenant, workspace], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(|error| sql(&error))?
                .collect::<Result<_, _>>()
                .map_err(|error| sql(&error))?
        };
        for (id, record) in rows {
            let mut record: Value = serde_json::from_str(&record).map_err(|error| json(&error))?;
            let cited = match *table {
                "receipts" => record
                    .get_mut("sources")
                    .and_then(Value::as_array_mut)
                    .map_or(0, |sources| {
                        sources.iter_mut().fold(0_u32, |cited, source| {
                            cited.saturating_add(u32::from(scrub_source(source, workspace)))
                        })
                    }),
                _ => record
                    .get_mut("source")
                    .map_or(0, |source| u32::from(scrub_source(source, workspace))),
            };
            count = count.saturating_add(cited);
            let (Some(purge), Some(fields)) = (scrub, record.as_object_mut()) else {
                continue;
            };
            if cited == 0 {
                continue;
            }
            if *table == "receipts" {
                fields
                    .entry("invalidated_by")
                    .or_insert_with(|| Value::String(purge.0.to_string()));
            } else {
                fields.insert(
                    "coverage".to_owned(),
                    Value::String("invalidated".to_owned()),
                );
            }
            connection
                .execute(
                    &format!("UPDATE {table} SET record = ?1 WHERE {key} = ?2"),
                    params![record.to_string(), id],
                )
                .map_err(|error| sql(&error))?;
        }
    }
    Ok(count)
}

/// Replace a stored citation of the purged workspace with the invalidated citation (Stage 1b
/// design section 6): the identities stay (workspace, item, revision, which the revision map
/// resolves to the purge), the path becomes `purged/<item id>`, which names nothing, and every
/// field that can carry the target's content is dropped: the selection becomes `all` (a section
/// heading or a sheet name is the target's text), and the derived digest and the locations (a
/// sheet name again) go. Returns whether the citation was not yet in that form.
fn scrub_source(source: &mut Value, workspace: &str) -> bool {
    if source.get("workspace_id").and_then(Value::as_str) != Some(workspace) {
        return false;
    }
    let field = |name: &str| source.get(name).cloned().unwrap_or(Value::Null);
    let item = field("item_id");
    let scrubbed = serde_json::json!({
        "workspace_id": workspace,
        "item_id": item,
        "path": format!("purged/{}", item.as_str().unwrap_or_default()),
        "revision": field("revision"),
        "selection": { "kind": "all" },
    });
    if *source == scrubbed {
        return false;
    }
    *source = scrubbed;
    true
}

/// Remove the workspace from every connector's `workspace_ids`.
fn unlist_from_connectors(connection: &Connection, workspace: &str) -> Result<(), ApiError> {
    let rows: Vec<(String, String)> = {
        let mut statement = connection
            .prepare("SELECT connector_id, record FROM connectors WHERE instr(record, ?1) > 0")
            .map_err(|error| sql(&error))?;
        statement
            .query_map([workspace], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|error| sql(&error))?
            .collect::<Result<_, _>>()
            .map_err(|error| sql(&error))?
    };
    for (id, record) in rows {
        let mut record: Value = serde_json::from_str(&record).map_err(|error| json(&error))?;
        if let Some(listed) = record
            .get_mut("workspace_ids")
            .and_then(Value::as_array_mut)
        {
            listed.retain(|value| value.as_str() != Some(workspace));
        }
        connection
            .execute(
                "UPDATE connectors SET record = ?1 WHERE connector_id = ?2",
                params![record.to_string(), id],
            )
            .map_err(|error| sql(&error))?;
    }
    Ok(())
}

fn execute(
    connection: &Connection,
    query: &str,
    tenant: &str,
    workspace: &str,
) -> Result<(), ApiError> {
    connection
        .execute(query, params![tenant, workspace])
        .map(|_| ())
        .map_err(|error| sql(&error))
}

/// Every ordinary table of the database with its columns.
fn tables(connection: &Connection) -> Result<Vec<Table>, ApiError> {
    let names: Vec<String> = {
        let mut statement = connection
            .prepare(
                "SELECT name FROM sqlite_schema
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .map_err(|error| sql(&error))?;
        statement
            .query_map([], |row| row.get(0))
            .map_err(|error| sql(&error))?
            .collect::<Result<_, _>>()
            .map_err(|error| sql(&error))?
    };
    names
        .into_iter()
        .map(|name| {
            let mut statement = connection
                .prepare("SELECT name FROM pragma_table_info(?1)")
                .map_err(|error| sql(&error))?;
            let columns = statement
                .query_map([&name], |row| row.get(0))
                .map_err(|error| sql(&error))?
                .collect::<Result<_, _>>()
                .map_err(|error| sql(&error))?;
            Ok(Table { name, columns })
        })
        .collect()
}
