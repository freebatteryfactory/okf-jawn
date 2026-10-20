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
//! - completed and released mutation rows of the workspace become terminally purged:
//!   response and request digest are removed, while retry identity, token and clocks stay.
//!   Purge-operation rows and leased rows remain untouched. A late completion/release
//!   checks the purge in its own transaction (mutations.rs); a retry is refused there;
//! - older copies cannot represent the purged state: their finished rows are released
//!   with NULL response and an empty digest. Without a workspace column, every such
//!   non-purge row of the tenant is scrubbed. Already-empty rows are not selected again;
//! - a receipt or review of another workspace of the tenant that cites the workspace is marked
//!   invalidated (a receipt's `invalidated_by`, a review's coverage), and each citing source
//!   becomes the invalidated citation (`scrub_source`): its path `purged/<item id>`, its
//!   selection `all`, without a digest or locations.
//!
//! "Holds" means holds something a purge still removes or scrubs, so a database or archive
//! already scrubbed of the workspace no longer holds it.
//!
//! No installation predates records schema 2 (owner lifecycle ruling 9). The schema-5
//! upgrade sanitizes the one pre-upgrade copy. Required ledger columns are checked before
//! inspecting or changing any table: a malformed copy stops the purge instead of being skipped.
//! Retained caller-supplied idempotency keys are retry identities; no request hash remains.
use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::PurgeId;
use okf_jawn_contract::metadata::OperationName;
use rusqlite::{Connection, OptionalExtension, ToSql, params};
use serde_json::Value;

use crate::db::{internal, json, sql};

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
const LEDGER_ROWS: &str =
    "tenant_id = ?1 AND workspace_id = ?2 AND state IN ('completed', 'released')
     AND operation NOT IN (?3, ?4)";

/// The same rows in a ledger that records no workspace (a copy older than records schema 2):
/// none can be attributed, so every completed row of the tenant but a purge's. `?2` is bound
/// like `LEDGER_ROWS`' and compared with nothing.
const UNATTRIBUTED_ROWS: &str =
    "tenant_id = ?1 AND ?2 IS NOT NULL AND state IN ('completed', 'released')
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
    fn ledger_rows(&self) -> Option<String> {
        if self.name != LEDGER {
            return None;
        }
        let rows = if self.has("workspace_id") {
            LEDGER_ROWS
        } else {
            UNATTRIBUTED_ROWS
        };
        Some(if self.has("purge_id") {
            rows.to_owned()
        } else {
            // Old schemas cannot represent purged/NULL digest. An emptied released row
            // is no longer content, so a rewritten copy is not selected again.
            format!("{rows} AND (response IS NOT NULL OR digest <> '')")
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
    if let Some(table) = tables.iter().find(|table| table.name == LEDGER)
        && let Some(rows) = table.ledger_rows()
    {
        let [purge_workspace, purge_item] = PURGES.map(OperationName::as_str);
        let purge = purge.0.to_string();
        let mut values: Vec<&dyn ToSql> = vec![&tenant, &workspace, &purge_workspace, &purge_item];
        let assignment = if table.has("purge_id") {
            values.push(&purge);
            "state = 'purged', digest = NULL, response = NULL, purge_id = ?5"
        } else {
            "state = 'released', digest = '', response = NULL"
        };
        // Identity, token, lease and the original retention clock remain exact.
        connection
            .execute(
                &format!("UPDATE {LEDGER} SET {assignment} WHERE {rows}"),
                values.as_slice(),
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
            let table = Table { name, columns };
            if table.name == LEDGER {
                for column in ["tenant_id", "operation", "state", "response", "digest"] {
                    if !table.has(column) {
                        return Err(internal(format!(
                            "the records copy holds a ledger without {column}"
                        )));
                    }
                }
            }
            Ok(table)
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
            .to_version(&mut connection, 1)
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

    /// One V5 ledger with literal target, purge-operation, leased and other-scope rows.
    fn v5_ledger() -> Result<Connection, ApiError> {
        let mut connection = Connection::open_in_memory().map_err(|error| fault(&error))?;
        crate::schema::RECORDS
            .to_latest(&mut connection)
            .map_err(|error| fault(&error))?;
        for (key, tenant, workspace, operation, state, response, finished) in [
            (
                "1",
                "local",
                TARGET,
                "update_workspace",
                "completed",
                Some("completed"),
                Some(11),
            ),
            (
                "2",
                "local",
                TARGET,
                "save_draft",
                "released",
                None,
                Some(12),
            ),
            ("3", "local", TARGET, "save_draft", "leased", None, None),
            (
                "4",
                "local",
                TARGET,
                "purge_workspace",
                "completed",
                Some("purge"),
                Some(14),
            ),
            (
                "5",
                "local",
                TARGET,
                "purge_item",
                "completed",
                Some("item purge"),
                Some(15),
            ),
            (
                "6",
                "local",
                "other-workspace",
                "save_draft",
                "completed",
                Some("other workspace"),
                Some(16),
            ),
            (
                "7",
                "other-tenant",
                TARGET,
                "save_draft",
                "completed",
                Some("other tenant"),
                Some(17),
            ),
        ] {
            connection.execute(
                "INSERT INTO mutations (tenant_id,subject,client_key,operation,idempotency_key,
                     mutation_id,digest,state,token,lease_expires_ms,response,finished_ms,workspace_id)
                 VALUES (?1,'ana','-',?2,?3,?3,'request-digest',?4,7,99,?5,?6,?7)",
                params![tenant,operation,key,state,response,finished,workspace],
            ).map_err(|error| fault(&error))?;
        }
        Ok(connection)
    }

    /// Deliberately corrupt schema, not a claimed released fixture. Closed literal column
    /// names alone build the SQL; no caller-provided identifiers enter this helper.
    fn malformed_ledger(missing: &str) -> Result<Connection, ApiError> {
        let connection = Connection::open_in_memory().map_err(|error| fault(&error))?;
        let columns = ["tenant_id", "operation", "state", "response", "digest"]
            .into_iter()
            .filter(|column| *column != missing)
            .map(|column| format!("{column} TEXT"))
            .collect::<Vec<_>>()
            .join(",");
        connection
            .execute_batch(&format!(
                "CREATE TABLE mutations ({columns});
             CREATE TABLE a_scoped (tenant_id TEXT,workspace_id TEXT,body TEXT);"
            ))
            .map_err(|error| fault(&error))?;
        connection
            .execute(
                "INSERT INTO a_scoped VALUES ('local',?1,'preserved')",
                [TARGET],
            )
            .map_err(|error| fault(&error))?;
        Ok(connection)
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
        let digest: String = connection
            .query_row(
                "SELECT digest FROM mutations WHERE tenant_id='local' AND operation='update_workspace'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| fault(&error))?;
        assert_eq!(
            digest, "",
            "section5.2: no request digest survives in the legacy copy"
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

    /// Section5.2: live rows retain identity, clocks and leases while losing only the target's
    /// completed/released payloads. Purge operations, leased rows and other scopes stay exact.
    #[test]
    fn ledger_rows_v5_scrubs_both_finished_states_and_preserves_every_other_row()
    -> Result<(), ApiError> {
        let connection = v5_ledger()?;
        let purge = PurgeId(uuid::Uuid::from_u128(60));
        remove(&connection, "local", TARGET, purge)?;
        let mut statement = connection.prepare(
            "SELECT json_array(idempotency_key,state,digest,response,purge_id,token,lease_expires_ms,finished_ms)
             FROM mutations ORDER BY idempotency_key",
        ).map_err(|error| fault(&error))?;
        let actual = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| fault(&error))?
            .map(|row| {
                row.map_err(|error| fault(&error)).and_then(|text| {
                    serde_json::from_str::<serde_json::Value>(&text).map_err(|error| fault(&error))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            serde_json::Value::Array(actual),
            serde_json::json!([
                ["1", "purged", null, null, purge.0.to_string(), 7, 99, 11],
                ["2", "purged", null, null, purge.0.to_string(), 7, 99, 12],
                ["3", "leased", "request-digest", null, null, 7, 99, null],
                ["4", "completed", "request-digest", "purge", null, 7, 99, 14],
                [
                    "5",
                    "completed",
                    "request-digest",
                    "item purge",
                    null,
                    7,
                    99,
                    15
                ],
                [
                    "6",
                    "completed",
                    "request-digest",
                    "other workspace",
                    null,
                    7,
                    99,
                    16
                ],
                [
                    "7",
                    "completed",
                    "request-digest",
                    "other tenant",
                    null,
                    7,
                    99,
                    17
                ]
            ])
        );
        assert!(holds(&connection, "local", "other-workspace")?);
        assert!(!holds(&connection, "local", TARGET)?);
        remove(&connection, "local", TARGET, purge)?;
        Ok(())
    }

    /// Section5.2: a released old-copy row can still hold a request digest without a response.
    /// Once scrubbed it is not held again; its original finish clock never moves.
    #[test]
    fn ledger_rows_legacy_released_digest_is_held_until_emptied() -> Result<(), ApiError> {
        let connection = ledger_without_workspaces(&[("local", "save_draft")])?;
        connection
            .execute(
                "UPDATE mutations SET state='released',response=NULL,finished_ms=42",
                [],
            )
            .map_err(|error| fault(&error))?;
        assert!(
            holds(&connection, "local", TARGET)?,
            "the request digest is still content"
        );
        remove(
            &connection,
            "local",
            TARGET,
            PurgeId(uuid::Uuid::from_u128(60)),
        )?;
        let actual: (String, String, bool, i64) = connection
            .query_row(
                "SELECT state,digest,response IS NULL,finished_ms FROM mutations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|error| fault(&error))?;
        assert_eq!(actual, ("released".to_owned(), String::new(), true, 42));
        assert!(!holds(&connection, "local", TARGET)?);
        Ok(())
    }

    /// Section5.3 limit6: even a different table holding the target cannot hide a malformed
    /// ledger. These names are precisely the nonoptional columns the scrub reads/writes.
    #[test]
    fn ledger_rows_holds_refuses_each_missing_required_ledger_column() -> Result<(), ApiError> {
        for missing in ["tenant_id", "operation", "state", "response", "digest"] {
            let connection = malformed_ledger(missing)?;
            let refused = holds(&connection, "local", TARGET)
                .err()
                .ok_or_else(|| fault(&format!("holds accepted a ledger without {missing}")))?;
            assert_eq!(refused.code, ErrorCode::Internal);
            assert!(refused.message.contains(missing), "{}", refused.message);
        }
        Ok(())
    }

    /// The same schema refusal must happen before remove can alter any other scoped table.
    #[test]
    fn ledger_rows_remove_refuses_each_missing_required_ledger_column() -> Result<(), ApiError> {
        for missing in ["tenant_id", "operation", "state", "response", "digest"] {
            let connection = malformed_ledger(missing)?;
            let refused = remove(
                &connection,
                "local",
                TARGET,
                PurgeId(uuid::Uuid::from_u128(60)),
            )
            .err()
            .ok_or_else(|| fault(&format!("remove accepted a ledger without {missing}")))?;
            assert_eq!(refused.code, ErrorCode::Internal);
            assert!(refused.message.contains(missing), "{}", refused.message);
            let remaining: i64 = connection
                .query_row("SELECT count(*) FROM a_scoped", [], |row| row.get(0))
                .map_err(|error| fault(&error))?;
            assert_eq!(remaining, 1, "a schema refusal writes nothing");
        }
        Ok(())
    }
}
