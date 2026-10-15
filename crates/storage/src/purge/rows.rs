//! A workspace's rows in a records database, removed or found by reading the schema itself.
//!
//! The same functions run on the live records database and on every pre-migration copy of it,
//! which may be at an older schema version, so they find the tables from `sqlite_schema` and
//! their columns from `pragma_table_info` instead of naming the current schema:
//!
//! - every table with `tenant_id` and `workspace_id` loses the workspace's rows, except the
//!   purge records (`purges`, `purge_progress`) and the revision map, which hold identities only
//!   and outlive the workspace, and the mutation ledger (next). The warnings kept for each
//!   commit's replay (`commit_warnings`, records schema 3) are among the rows removed: a
//!   warning can quote a path or a title of the workspace, so the table must never join `KEPT`;
//! - a table keyed by `job_id` without a workspace column loses the rows of the workspace's
//!   jobs, before the jobs go;
//! - every connector's `workspace_ids` loses the workspace;
//! - every completed mutation-ledger row of the workspace (`mutations.workspace_id`, the
//!   workspace its request writes or its response names at `RequestScope::RESPONSE_WORKSPACE`)
//!   is emptied: its stored response (which may hold the workspace's name or an item's title)
//!   is dropped and the row released, its identity kept. A retry under its key then re-runs
//!   under the same `MutationId` and is refused by the purge (`guard`, and the catalog for a
//!   `create_workspace`); a deleted row would let the retry start a new mutation, and a
//!   `create_workspace` retry would create a second workspace. The rows of purges
//!   (`purge_workspace`, `purge_item`) are kept: their response is the purge record, identities
//!   and counts only, which outlives the workspace as `purges` does, so a retry of the purge
//!   replays it. The response text is never searched, so another workspace's row that quotes
//!   the workspace's identity is not touched;
//! - in a ledger without the workspace column (a copy older than records schema 2) no row can
//!   be attributed, so every completed row of the tenant but a purge's is emptied the same way,
//!   whichever workspace it belonged to. Such a copy is a backup: its ledger is never consulted
//!   for a replay, and a purge does not complete while a managed backup that holds the
//!   workspace can be restored (SPEC section 8);
//! - a receipt or review of another workspace of the tenant that cites the workspace is marked
//!   invalidated (a receipt's `invalidated_by`, a review's coverage), and each citing source
//!   becomes the invalidated citation (`scrub_source`): its path `purged/<item id>`, its
//!   selection `all`, without a digest or locations.
//!
//! "Holds" means holds something a purge still removes or scrubs, so a database or archive
//! already scrubbed of the workspace no longer holds it.
//!
//! Known limits of the ledger step (the first two concern the live database only):
//!
//! - A row completed before records schema 2 has no `workspace_id`, so the purge leaves its
//!   response in place until the ledger drops the row: at the first `begin` that comes seven
//!   days or more after the row completed (`mutations::RETENTION_SECONDS`).
//! - A row whose `complete` lands after the purge's last pass over the rows (its step 5) keeps
//!   its response until the ledger drops it the same way.
//! - An emptied row keeps the digest of the request and its idempotency key. The digest is a
//!   SHA-256 of the canonical request (a workspace's name and description, a draft's body, a
//!   comment's text), so a guessed text could be confirmed against it.
//! - A retry of an emptied row restarts that row's seven days: the refused retry resumes it
//!   (`finished_ms` cleared) and releases it again (`finished_ms` now).
//! - A `mutations` table that lacks one of the four columns the purge reads (`tenant_id`,
//!   `operation`, `state`, `response`) is not looked at, by `holds` or by `remove`.

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::PurgeId;
use okf_jawn_contract::metadata::OperationName;
use rusqlite::{Connection, OptionalExtension, ToSql, params};
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
/// `sources`, a review's `source` and a comment's `source`.
const CITING: &[(&str, &str)] = &[
    ("receipts", "receipt_id"),
    ("reviews", "review_id"),
    ("comments", "comment_id"),
];

/// The mutation ledger, whose rows of the workspace are emptied, not removed (module header).
const LEDGER: &str = "mutations";

/// The operations whose ledger rows a purge keeps: their response is a purge record.
const PURGES: [OperationName; 2] = [OperationName::PurgeWorkspace, OperationName::PurgeItem];

/// The completed ledger rows of the workspace (`?1` tenant, `?2` workspace) whose stored
/// response a purge drops: every one but a purge's (`?3`, `?4`, `PURGES`).
const LEDGER_ROWS: &str = "tenant_id = ?1 AND workspace_id = ?2 AND state = 'completed'
     AND operation NOT IN (?3, ?4)";

/// The same rows in a ledger that records no workspace (a copy older than records schema 2):
/// none can be attributed, so every completed row of the tenant but a purge's. `?2` is bound
/// like `LEDGER_ROWS`' and compared with nothing.
const UNATTRIBUTED_ROWS: &str = "tenant_id = ?1 AND ?2 IS NOT NULL AND state = 'completed'
     AND operation NOT IN (?3, ?4)";

impl Table {
    fn has(&self, column: &str) -> bool {
        self.columns.iter().any(|name| name == column)
    }

    /// A workspace-scoped table whose rows a purge removes.
    fn scoped(&self) -> bool {
        self.has("tenant_id")
            && self.has("workspace_id")
            && !KEPT.contains(&self.name.as_str())
            && self.name != LEDGER
    }

    /// For the mutation ledger, the predicate of the rows a purge empties: by the workspace
    /// each row records, or, where the ledger has no such column, every row of the tenant.
    /// `None` for any other table.
    fn ledger_rows(&self) -> Option<&'static str> {
        let ledger = self.name == LEDGER
            && ["tenant_id", "operation", "state", "response"]
                .iter()
                .all(|column| self.has(column));
        if !ledger {
            return None;
        }
        Some(if self.has("workspace_id") {
            LEDGER_ROWS
        } else {
            UNATTRIBUTED_ROWS
        })
    }
}

/// Whether the database holds anything of the workspace a purge removes or scrubs.
pub(crate) fn holds(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
) -> Result<bool, ApiError> {
    let [purge_workspace, purge_item] = PURGES.map(OperationName::as_str);
    let scope: [&dyn ToSql; 2] = [&tenant, &workspace];
    let ledger: [&dyn ToSql; 4] = [&tenant, &workspace, &purge_workspace, &purge_item];
    for table in tables(connection)? {
        let (query, values): (String, &[&dyn ToSql]) = if table.scoped() {
            (
                format!(
                    "SELECT 1 FROM \"{}\" WHERE tenant_id = ?1 AND workspace_id = ?2 LIMIT 1",
                    table.name
                ),
                &scope,
            )
        } else if let Some(rows) = table.ledger_rows() {
            (
                format!("SELECT 1 FROM {LEDGER} WHERE {rows} LIMIT 1"),
                &ledger,
            )
        } else if table.name == "connectors" && table.has("record") {
            (
                "SELECT 1 FROM connectors WHERE ?1 = ?1 AND instr(record, ?2) > 0 LIMIT 1"
                    .to_owned(),
                &scope,
            )
        } else {
            continue;
        };
        let found: Option<i64> = connection
            .query_row(&query, values, |row| row.get(0))
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
    if let Some(rows) = tables.iter().find_map(Table::ledger_rows) {
        let [purge_workspace, purge_item] = PURGES.map(OperationName::as_str);
        // Keep `finished_ms`: the emptied row expires when the completed one would have.
        connection
            .execute(
                &format!(
                    "UPDATE {LEDGER} SET state = 'released', response = NULL
                     WHERE {rows}"
                ),
                params![tenant, workspace, purge_workspace, purge_item],
            )
            .map_err(|error| sql(&error))?;
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

/// The stored citations of other workspaces' receipts, reviews and comments that cite the
/// workspace and are not yet scrubbed. With `scrub`, each is scrubbed (`scrub_source`) and its
/// record marked invalidated: a receipt and a comment get `invalidated_by` that purge, a review
/// the coverage `invalidated`.
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
            if *table == "receipts" || *table == "comments" {
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

#[cfg(test)]
mod tests {
    use okf_jawn_contract::error::{ApiError, ErrorCode};
    use okf_jawn_contract::identity::PurgeId;
    use rusqlite::{Connection, params};

    use super::{holds, remove};

    /// One ledger row as a test reads it: tenant, operation, state, and whether it holds a
    /// response.
    type Row = (String, String, String, bool);

    const TARGET: &str = "11111111-1111-4111-8111-111111111111";

    fn fault(error: &dyn std::fmt::Display) -> ApiError {
        ApiError::new(ErrorCode::Internal, error.to_string())
    }

    /// A records database whose ledger is as records schema 1 left it, without the workspace
    /// column, holding one completed row for each (tenant, operation) of `rows`. Each response
    /// holds a workspace's name and no identity.
    fn ledger_without_workspaces(rows: &[(&str, &str)]) -> Result<Connection, ApiError> {
        let mut connection = Connection::open_in_memory().map_err(|error| fault(&error))?;
        crate::schema::RECORDS
            .to_latest(&mut connection)
            .map_err(|error| fault(&error))?;
        connection
            .execute_batch(
                "DROP INDEX mutations_by_workspace;
                 ALTER TABLE mutations DROP COLUMN workspace_id;",
            )
            .map_err(|error| fault(&error))?;
        for (number, (tenant, operation)) in rows.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO mutations (tenant_id, subject, client_key, operation,
                         idempotency_key, mutation_id, digest, state, token, lease_expires_ms,
                         response, finished_ms)
                     VALUES (?1, 'ana', '-', ?2, ?3, ?3, 'digest', 'completed', 1, 0,
                         '{\"name\":\"zanzibar trip\"}', 0)",
                    params![tenant, operation, number.to_string()],
                )
                .map_err(|error| fault(&error))?;
        }
        Ok(connection)
    }

    /// The ledger rows of `connection`, in the order they were inserted.
    fn ledger(connection: &Connection) -> Result<Vec<Row>, ApiError> {
        let mut statement = connection
            .prepare(
                "SELECT tenant_id, operation, state, response IS NOT NULL FROM mutations
                 ORDER BY idempotency_key",
            )
            .map_err(|error| fault(&error))?;
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|error| fault(&error))?
            .collect::<Result<_, _>>()
            .map_err(|error| fault(&error))
    }

    fn row(tenant: &str, operation: &str, state: &str, response: bool) -> Row {
        (
            tenant.to_owned(),
            operation.to_owned(),
            state.to_owned(),
            response,
        )
    }

    /// A ledger that records no workspace cannot attribute a row, so a purge empties every
    /// completed row of the tenant but a purge's, and after that one pass the database no
    /// longer holds the workspace: the purge's second look at its backups finds nothing to do.
    #[test]
    fn a_ledger_without_workspaces_is_held_until_its_tenant_s_rows_are_emptied()
    -> Result<(), ApiError> {
        let connection = ledger_without_workspaces(&[
            ("local", "update_workspace"),
            ("local", "purge_workspace"),
            ("local", "purge_item"),
            ("elsewhere", "update_workspace"),
        ])?;
        assert!(
            holds(&connection, "local", TARGET)?,
            "a response no purge can attribute is held"
        );
        remove(
            &connection,
            "local",
            TARGET,
            PurgeId(uuid::Uuid::from_u128(60)),
        )?;
        assert_eq!(
            ledger(&connection)?,
            [
                row("local", "update_workspace", "released", false),
                row("local", "purge_workspace", "completed", true),
                row("local", "purge_item", "completed", true),
                row("elsewhere", "update_workspace", "completed", true),
            ]
        );
        assert!(
            !holds(&connection, "local", TARGET)?,
            "one pass, and the copy no longer holds the workspace"
        );
        Ok(())
    }

    /// A ledger that records the workspace holds it through one completed response and nothing
    /// else: `holds` is true until the row is emptied, so a copy whose only trace of the
    /// workspace is a ledger response is found, and rewritten, by the purge.
    #[test]
    fn a_ledger_response_alone_makes_a_database_hold_the_workspace() -> Result<(), ApiError> {
        let mut connection = Connection::open_in_memory().map_err(|error| fault(&error))?;
        crate::schema::RECORDS
            .to_latest(&mut connection)
            .map_err(|error| fault(&error))?;
        assert!(
            !holds(&connection, "local", TARGET)?,
            "an empty database holds nothing"
        );
        connection
            .execute(
                "INSERT INTO mutations (tenant_id, subject, client_key, operation,
                     idempotency_key, mutation_id, digest, state, token, lease_expires_ms,
                     response, finished_ms, workspace_id)
                 VALUES ('local', 'ana', '-', 'update_workspace', '1', '1', 'digest',
                     'completed', 1, 0, '{\"name\":\"zanzibar trip\"}', 0, ?1)",
                [TARGET],
            )
            .map_err(|error| fault(&error))?;
        assert!(
            holds(&connection, "local", TARGET)?,
            "a completed response of the workspace is held"
        );
        remove(
            &connection,
            "local",
            TARGET,
            PurgeId(uuid::Uuid::from_u128(60)),
        )?;
        assert!(!holds(&connection, "local", TARGET)?);
        Ok(())
    }

    #[test]
    fn a_ledger_without_workspaces_that_holds_only_purges_and_other_tenants_is_not_held()
    -> Result<(), ApiError> {
        let connection = ledger_without_workspaces(&[
            ("local", "purge_workspace"),
            ("elsewhere", "update_workspace"),
        ])?;
        assert!(!holds(&connection, "local", TARGET)?);
        Ok(())
    }
}
