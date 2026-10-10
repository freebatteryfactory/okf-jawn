//! The event logs (`EventLog`) over SQLite: one per workspace and one per tenant.
//!
//! The row sequence is the event cursor. An append with a `MutationId` carries a replay key
//! (the mutation and the event's own content) in a unique column, so an identical notification
//! under one mutation is appended once. An append to a workspace the tenant does not hold, one
//! never created or one whose purge completed, is recorded in the tenant's log: it never creates
//! or revives a row for that workspace.

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::events::{Event, ListEventsResponse};
use okf_jawn_contract::identity::MutationId;
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
use okf_jawn_core::ports::PortFuture;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::db::{Db, cursor, internal, json, page_limit, paged, sql};
use crate::seam;

/// `EventLog` over the records database.
#[derive(Debug, Clone)]
pub struct SqliteEvents {
    db: Db,
}

impl SqliteEvents {
    pub(crate) const fn new(db: Db) -> Self {
        Self { db }
    }
}

impl EventLog for SqliteEvents {
    fn append<'a>(
        &'a self,
        scope: &'a EventScope,
        mutation_id: Option<MutationId>,
        event: NewEvent,
    ) -> PortFuture<'a, Event> {
        let scope = scope.clone();
        Box::pin(self.db.transaction(move |transaction| {
            let (tenant, workspace) = log_of(transaction, &scope)?;
            let record = Event {
                id: String::new(),
                workspace_id: workspace.as_deref().map(|id| from_text!(id)).transpose()?,
                kind: event.kind,
                at: seam::now()?,
                revision: event.revision,
                item_id: event.item_id,
                job_id: event.job_id,
                connector_id: event.connector_id,
                actor: event.actor,
                operation: event.operation,
            };
            let replay_key = mutation_id
                .map(|mutation| {
                    let content = serde_json::json!({
                        "workspace_id": record.workspace_id,
                        "kind": record.kind,
                        "revision": record.revision,
                        "item_id": record.item_id,
                        "job_id": record.job_id,
                        "connector_id": record.connector_id,
                        "actor": record.actor,
                        "operation": record.operation,
                    });
                    format!("{}/{content}", mutation.0)
                })
                .map(|key| format!("{tenant}/{key}"));
            let inserted = transaction
                .execute(
                    "INSERT INTO events (tenant_id, workspace_id, replay_key, record)
                     VALUES (?1, ?2, ?3, ?4) ON CONFLICT (replay_key) DO NOTHING",
                    params![
                        tenant,
                        workspace,
                        replay_key,
                        serde_json::to_string(&record).map_err(|error| json(&error))?
                    ],
                )
                .map_err(|error| sql(&error))?;
            let sequence = if inserted == 1 {
                transaction.last_insert_rowid()
            } else {
                transaction
                    .query_row(
                        "SELECT seq FROM events WHERE replay_key = ?1",
                        [&replay_key],
                        |row| row.get(0),
                    )
                    .map_err(|error| sql(&error))?
            };
            read(transaction, sequence)
        }))
    }

    fn list<'a>(
        &'a self,
        scope: &'a EventScope,
        query: EventQuery,
    ) -> PortFuture<'a, ListEventsResponse> {
        let (tenant, workspace) = match scope {
            EventScope::Tenant(tenant) => (tenant.as_str().to_owned(), None),
            EventScope::Workspace(scope) => (
                scope.tenant_id.as_str().to_owned(),
                Some(scope.workspace_id.0.to_string()),
            ),
        };
        Box::pin(self.db.call(move |connection| {
            let after = cursor(query.after.as_deref())?;
            let limit = page_limit(query.page.limit);
            let mut statement = connection
                .prepare(
                    "SELECT seq, record FROM events
                     WHERE tenant_id = ?1 AND workspace_id IS ?2 AND (?3 IS NULL OR seq > ?3)
                     ORDER BY seq LIMIT ?4",
                )
                .map_err(|error| sql(&error))?;
            let rows = statement
                .query_map(
                    params![tenant, workspace, after, limit.saturating_add(1)],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(|error| sql(&error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| sql(&error))?;
            let sequences: Vec<i64> = rows.iter().map(|(sequence, _)| *sequence).collect();
            let (records, next_cursor) = paged(rows, limit);
            let events = records
                .iter()
                .zip(sequences)
                .map(|(record, sequence)| decode(record, sequence))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ListEventsResponse {
                events,
                next_cursor,
            })
        }))
    }
}

/// The log an append writes to: the workspace's when the tenant holds it, the tenant's
/// otherwise.
fn log_of(
    transaction: &Transaction<'_>,
    scope: &EventScope,
) -> Result<(String, Option<String>), ApiError> {
    match scope {
        EventScope::Tenant(tenant) => Ok((tenant.as_str().to_owned(), None)),
        EventScope::Workspace(scope) => {
            let tenant = scope.tenant_id.as_str().to_owned();
            let workspace = scope.workspace_id.0.to_string();
            let held = holds(transaction, &tenant, &workspace)?;
            Ok((tenant, held.then_some(workspace)))
        }
    }
}

/// Whether the tenant holds the workspace: it was created and no purge of it completed.
pub(crate) fn holds(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
) -> Result<bool, ApiError> {
    let created: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM workspaces WHERE tenant_id = ?1 AND workspace_id = ?2",
            params![tenant, workspace],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let purged: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM purges WHERE tenant_id = ?1 AND target_key = ?2 AND state = 'completed'",
            params![tenant, format!("workspace/{workspace}")],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    Ok(created.is_some() && purged.is_none())
}

fn read(connection: &Connection, sequence: i64) -> Result<Event, ApiError> {
    let record: String = connection
        .query_row(
            "SELECT record FROM events WHERE seq = ?1",
            [sequence],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?
        .ok_or_else(|| internal("an event row vanished after its insert"))?;
    decode(&record, sequence)
}

fn decode(record: &str, sequence: i64) -> Result<Event, ApiError> {
    let mut event: Event = serde_json::from_str(record).map_err(|error| json(&error))?;
    event.id = sequence.to_string();
    Ok(event)
}
