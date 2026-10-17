//! Proposals and their discussion (`ProposalStore`) over SQLite.
//!
//! A proposal and a comment are each unique on their `MutationId`; listings page by row order
//! with an opaque cursor (the row sequence), proposals newest first, comments oldest first.
//! A status change is a compare-and-set inside its transaction (`UPDATE ... WHERE status =
//! expected`): of two changes racing from one status, one is written and the other refused,
//! even when both write the same record. The same transaction keeps the mutation that wrote the
//! change beside the proposal, as a `workspace_mutations` row (action `STATUS_WRITTEN`, its
//! `result` the proposal's identity): the records schema's row of what a mutation did in a
//! workspace, a retry record that a workspace archive does not carry and a workspace purge
//! removes, so no schema step is needed. Writing the stored record again is a no-op only under
//! the mutation that wrote it (a retry). Every write is refused inside its transaction while a
//! purge names the workspace (`purge::guard`).

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::{MutationId, ProposalId};
use okf_jawn_contract::proposal::{Comment, ListProposalsResponse, Proposal, ProposalStatus};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::proposals::{CommentPage, ProposalFilter, ProposalStore};
use okf_jawn_core::storage::{Page, StorageScope};
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::{
    Db, conflict, cursor, internal, json, not_found, page_limit, paged, scope_key, sql,
};
use crate::purge::guard;

/// `ProposalStore` over the records database.
#[derive(Debug, Clone)]
pub struct SqliteProposals {
    db: Db,
}

/// The mutation that writes one proposal's status, as `update` keeps it beside the proposal.
struct Writer<'a> {
    mutation: String,
    tenant: &'a str,
    workspace: &'a str,
    proposal: &'a str,
}

/// The `workspace_mutations` action of the row that names the mutation which wrote a proposal's
/// status; the row's `result` is the proposal's identity.
const STATUS_WRITTEN: &str = "proposal_status";

impl SqliteProposals {
    pub(crate) const fn new(db: Db) -> Self {
        Self { db }
    }
}

impl ProposalStore for SqliteProposals {
    fn insert<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        proposal: Proposal,
    ) -> PortFuture<'a, Proposal> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.transaction(move |transaction| {
            guard::refuse(transaction, &tenant, &workspace)?;
            transaction
                .execute(
                    "INSERT INTO proposals (proposal_id, tenant_id, workspace_id, mutation_id,
                         status, record)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (mutation_id) DO NOTHING",
                    params![
                        proposal.id.0.to_string(),
                        tenant,
                        workspace,
                        mutation_id.0.to_string(),
                        variant_text!(&proposal.status)?,
                        serde_json::to_string(&proposal).map_err(|error| json(&error))?
                    ],
                )
                .map_err(|error| sql(&error))?;
            let stored: String = transaction
                .query_row(
                    "SELECT record FROM proposals WHERE mutation_id = ?1",
                    [mutation_id.0.to_string()],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            serde_json::from_str(&stored).map_err(|error| json(&error))
        }))
    }

    fn get<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal: ProposalId,
    ) -> PortFuture<'a, Proposal> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.call(move |connection| {
            let record = read(connection, &tenant, &workspace, proposal)?
                .ok_or_else(|| not_found("proposal"))?;
            serde_json::from_str(&record).map_err(|error| json(&error))
        }))
    }

    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        filter: ProposalFilter,
    ) -> PortFuture<'a, ListProposalsResponse> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.call(move |connection| {
            let status = filter
                .status
                .as_ref()
                .map(|status| variant_text!(status))
                .transpose()?;
            let after = cursor(filter.page.cursor.as_deref())?;
            let limit = page_limit(filter.page.limit);
            let mut statement = connection
                .prepare(
                    "SELECT rowid, record FROM proposals
                     WHERE tenant_id = ?1 AND workspace_id = ?2 AND (?3 IS NULL OR status = ?3)
                       AND (?4 IS NULL OR rowid < ?4)
                     ORDER BY rowid DESC LIMIT ?5",
                )
                .map_err(|error| sql(&error))?;
            let rows = statement
                .query_map(
                    params![tenant, workspace, status, after, limit.saturating_add(1)],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(|error| sql(&error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| sql(&error))?;
            let (records, next_cursor) = paged(rows, limit);
            let items = records
                .iter()
                .map(|record| serde_json::from_str(record).map_err(|error| json(&error)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ListProposalsResponse { items, next_cursor })
        }))
    }

    fn update<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        proposal: Proposal,
        expected: ProposalStatus,
    ) -> PortFuture<'a, Proposal> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.transaction(move |transaction| {
            guard::refuse(transaction, &tenant, &workspace)?;
            let id = proposal.id.0.to_string();
            let writer = Writer {
                mutation: mutation_id.0.to_string(),
                tenant: &tenant,
                workspace: &workspace,
                proposal: &id,
            };
            let status = variant_text!(&proposal.status)?;
            let record = serde_json::to_string(&proposal).map_err(|error| json(&error))?;
            let stored: (String, String) = transaction
                .query_row(
                    "SELECT status, record FROM proposals
                     WHERE tenant_id = ?1 AND workspace_id = ?2 AND proposal_id = ?3",
                    params![tenant, workspace, id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| sql(&error))?
                .ok_or_else(|| not_found("proposal"))?;
            // A retry of a change this mutation wrote: the same record again changes nothing.
            // Another mutation's identical change goes on to the compare-and-set.
            if stored == (status.clone(), record.clone()) && writer.wrote(transaction)? {
                return Ok(proposal);
            }
            // The compare-and-set: written only while the stored status is the expected one,
            // in this transaction, so of two changes racing from one status one is refused.
            let changed = transaction
                .execute(
                    "UPDATE proposals SET status = ?1, record = ?2
                     WHERE tenant_id = ?3 AND workspace_id = ?4 AND proposal_id = ?5
                       AND status = ?6",
                    params![
                        status,
                        record,
                        tenant,
                        workspace,
                        id,
                        variant_text!(&expected)?
                    ],
                )
                .map_err(|error| sql(&error))?;
            if changed == 0 {
                return Err(conflict(format!(
                    "the proposal is {}, not {}",
                    stored.0,
                    variant_text!(&expected)?
                )));
            }
            writer.keep(transaction)?;
            Ok(proposal)
        }))
    }

    fn add_comment<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        proposal: ProposalId,
        comment: Comment,
    ) -> PortFuture<'a, Comment> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.transaction(move |transaction| {
            guard::refuse(transaction, &tenant, &workspace)?;
            read(transaction, &tenant, &workspace, proposal)?
                .ok_or_else(|| not_found("proposal"))?;
            transaction
                .execute(
                    "INSERT INTO comments (comment_id, tenant_id, workspace_id, proposal_id,
                         mutation_id, record)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (mutation_id) DO NOTHING",
                    params![
                        comment.id,
                        tenant,
                        workspace,
                        proposal.0.to_string(),
                        mutation_id.0.to_string(),
                        serde_json::to_string(&comment).map_err(|error| json(&error))?
                    ],
                )
                .map_err(|error| sql(&error))?;
            let stored: String = transaction
                .query_row(
                    "SELECT record FROM comments WHERE mutation_id = ?1",
                    [mutation_id.0.to_string()],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            serde_json::from_str(&stored).map_err(|error| json(&error))
        }))
    }

    fn list_comments<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal: ProposalId,
        page: Page,
    ) -> PortFuture<'a, CommentPage> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.call(move |connection| {
            read(connection, &tenant, &workspace, proposal)?
                .ok_or_else(|| not_found("proposal"))?;
            let after = cursor(page.cursor.as_deref())?;
            let limit = page_limit(page.limit);
            let mut statement = connection
                .prepare(
                    "SELECT rowid, record FROM comments
                     WHERE tenant_id = ?1 AND workspace_id = ?2 AND proposal_id = ?3
                       AND (?4 IS NULL OR rowid > ?4)
                     ORDER BY rowid LIMIT ?5",
                )
                .map_err(|error| sql(&error))?;
            let rows = statement
                .query_map(
                    params![
                        tenant,
                        workspace,
                        proposal.0.to_string(),
                        after,
                        limit.saturating_add(1)
                    ],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(|error| sql(&error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| sql(&error))?;
            let (records, next_cursor) = paged(rows, limit);
            let items = records
                .iter()
                .map(|record| serde_json::from_str(record).map_err(|error| json(&error)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CommentPage { items, next_cursor })
        }))
    }
}

impl Writer<'_> {
    /// Whether this mutation wrote the proposal's stored status.
    fn wrote(&self, connection: &Connection) -> Result<bool, ApiError> {
        connection
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM workspace_mutations
                     WHERE mutation_id = ?1 AND tenant_id = ?2 AND workspace_id = ?3
                       AND action = ?4 AND result = ?5)",
                params![
                    self.mutation,
                    self.tenant,
                    self.workspace,
                    STATUS_WRITTEN,
                    self.proposal
                ],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))
    }

    /// Keep this mutation as the writer of the proposal's status, in the transaction that wrote
    /// it. A mutation decides one proposal (no operation writes the status of two), so a row this
    /// mutation already holds for anything else is a fault, and the write rolls back with it.
    fn keep(&self, connection: &Connection) -> Result<(), ApiError> {
        connection
            .execute(
                "INSERT INTO workspace_mutations (mutation_id, tenant_id, workspace_id, action,
                     result)
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (mutation_id) DO NOTHING",
                params![
                    self.mutation,
                    self.tenant,
                    self.workspace,
                    STATUS_WRITTEN,
                    self.proposal
                ],
            )
            .map_err(|error| sql(&error))?;
        if self.wrote(connection)? {
            Ok(())
        } else {
            Err(internal(
                "this mutation already wrote another record, so it cannot decide this proposal",
            ))
        }
    }
}

fn read(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
    proposal: ProposalId,
) -> Result<Option<String>, ApiError> {
    connection
        .query_row(
            "SELECT record FROM proposals
             WHERE tenant_id = ?1 AND workspace_id = ?2 AND proposal_id = ?3",
            params![tenant, workspace, proposal.0.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))
}
