//! The mutation ledger (`MutationStore`) over SQLite.
//!
//! One row per (tenant, subject, client, operation, idempotency key), primary key. `begin`
//! grants a lease whose token is one more than the row's last token, so every grant of a
//! mutation carries a token no earlier grant carried. `complete` and `release` are
//! compare-and-set updates on (mutation, token, `leased`): a superseded attempt changes nothing.
//! A completed, released or purged row is kept 7 days from when it finished; a row whose lease expired
//! without either stays until a later attempt reconciles it. The lease length is
//! `MUTATION_LEASE_SECONDS`.
//!
//! `workspace_id` is written by the `begin` that inserts the row and, while it is still `NULL`,
//! by `complete`; a recorded workspace purge makes a finished attempt terminal without
//! retaining its digest or response. A refused retry preserves that row's retention clock.

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, MutationId, WorkspaceId};
use okf_jawn_core::mutations::{
    BeginOutcome, MutationKey, MutationLease, MutationStore, StoredResponse,
};
use okf_jawn_core::ports::PortFuture;
use rusqlite::{OptionalExtension, Transaction, params};

use crate::db::{Db, conflict, internal, json, sql};
use crate::seam;

/// `MutationStore` over the records database.
#[derive(Debug, Clone)]
pub struct SqliteMutations {
    db: Db,
}

/// What `begin` found under the key.
struct LedgerRow {
    mutation_id: String,
    digest: Option<String>,
    state: String,
    token: i64,
    lease_expires_ms: i64,
    response: Option<String>,
}

/// The recorded workspace purge whose terminal ledger row must keep its identity.
struct PurgedWorkspace {
    workspace: String,
    purge_id: String,
    state: String,
}

/// How long a granted lease lasts before another attempt may take the mutation over.
pub const MUTATION_LEASE_SECONDS: i64 = 120;
/// How long a completed, released or purged row is kept.
pub const RETENTION_SECONDS: i64 = 7 * 24 * 60 * 60;

impl PurgedWorkspace {
    fn refusal(&self) -> ApiError {
        conflict(format!(
            "workspace {} is purged by purge {} ({}); nothing is written to it",
            self.workspace, self.purge_id, self.state
        ))
    }
}

impl SqliteMutations {
    pub(crate) const fn new(db: Db) -> Self {
        Self { db }
    }
}

impl MutationStore for SqliteMutations {
    fn begin<'a>(
        &'a self,
        key: &'a MutationKey,
        digest: &'a Digest,
        workspace: Option<WorkspaceId>,
    ) -> PortFuture<'a, BeginOutcome> {
        let key = key.clone();
        let digest = digest.clone();
        Box::pin(async move {
            // An attributable old attempt is terminalized before its refusal reaches core.
            // The inner error is returned only after Db commits that deliberate change.
            self.db
                .transaction(move |transaction| begin(transaction, &key, &digest, workspace))
                .await?
        })
    }

    fn complete(
        &self,
        lease: MutationLease,
        response: serde_json::Value,
        workspace: Option<WorkspaceId>,
    ) -> PortFuture<'_, ()> {
        Box::pin(self.db.transaction(move |transaction| {
            let body = serde_json::to_string(&response).map_err(|error| json(&error))?;
            let token = token_of(lease)?;
            let mutation = lease.mutation_id.0.to_string();
            let named = workspace.map(|workspace| workspace.0.to_string());
            let now = seam::now_ms()?;
            let changed = if let Some(purge) = purge_for_mutation(transaction, &mutation, named.as_deref())? {
                terminalize(transaction, &mutation, token, &purge, now, "leased")?
            } else {
                transaction.execute(
                    "UPDATE mutations SET state = 'completed', response = ?1, finished_ms = ?2,
                         workspace_id = coalesce(workspace_id, ?5)
                     WHERE mutation_id = ?3 AND token = ?4 AND state = 'leased'",
                    params![body, now, mutation, token, named],
                )
                .map_err(|error| sql(&error))?
            };
            if changed == 1 {
                return Ok(());
            }
            let found: Option<(String, i64)> = transaction
                .query_row(
                    "SELECT state, token FROM mutations WHERE mutation_id = ?1",
                    [&mutation],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| sql(&error))?;
            match found {
                None => Err(internal("the mutation ledger has no row for this lease")),
                // This grant already completed: completing again keeps the first response.
                Some((state, current))
                    if matches!(state.as_str(), "completed" | "purged") && current == token => Ok(()),
                Some(_) => Err(conflict(
                    "another attempt holds this mutation; this lease is no longer the current grant",
                )),
            }
        }))
    }

    fn release(&self, lease: MutationLease) -> PortFuture<'_, ()> {
        Box::pin(self.db.transaction(move |transaction| {
            let mutation = lease.mutation_id.0.to_string();
            let token = token_of(lease)?;
            let now = seam::now_ms()?;
            if let Some(purge) = purge_for_mutation(transaction, &mutation, None)? {
                terminalize(transaction, &mutation, token, &purge, now, "leased")?;
                return Ok(());
            }
            transaction
                .execute(
                    "UPDATE mutations SET state = 'released', finished_ms = ?1
                     WHERE mutation_id = ?2 AND token = ?3 AND state = 'leased'",
                    params![now, mutation, token],
                )
                .map_err(|error| sql(&error))?;
            Ok(())
        }))
    }
}

fn begin(
    transaction: &Transaction<'_>,
    key: &MutationKey,
    digest: &Digest,
    workspace: Option<WorkspaceId>,
) -> Result<Result<BeginOutcome, ApiError>, ApiError> {
    let now = seam::now_ms()?;
    let retention_ms = RETENTION_SECONDS.saturating_mul(1000);
    transaction
        .execute(
            "DELETE FROM mutations
             WHERE state IN ('completed', 'released', 'purged') AND finished_ms <= ?1",
            [now.saturating_sub(retention_ms)],
        )
        .map_err(|error| sql(&error))?;
    let client_key = key
        .client_id
        .as_ref()
        .map_or_else(|| "-".to_owned(), |client| format!("client:{client}"));
    let key_params = params![
        key.tenant_id.as_str(),
        key.subject,
        client_key,
        key.operation.as_str(),
        key.key.0.to_string()
    ];
    let row = transaction
        .query_row(
            "SELECT mutation_id, digest, state, token, lease_expires_ms, response FROM mutations
             WHERE tenant_id = ?1 AND subject = ?2 AND client_key = ?3 AND operation = ?4
               AND idempotency_key = ?5",
            key_params,
            |row| {
                Ok(LedgerRow {
                    mutation_id: row.get(0)?,
                    digest: row.get(1)?,
                    state: row.get(2)?,
                    token: row.get(3)?,
                    lease_expires_ms: row.get(4)?,
                    response: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let expires = now.saturating_add(MUTATION_LEASE_SECONDS.saturating_mul(1000));
    let Some(row) = row else {
        return insert(transaction, key, &client_key, digest, workspace, expires).map(Ok);
    };
    if row.state == "purged"
        || row.state == "released"
        || (row.state == "leased" && row.lease_expires_ms <= now)
    {
        if let Some(purge) = purge_for_mutation(transaction, &row.mutation_id, None)? {
            if row.state != "purged" {
                terminalize(
                    transaction,
                    &row.mutation_id,
                    row.token,
                    &purge,
                    now,
                    &row.state,
                )?;
            }
            return Ok(Err(purge.refusal()));
        }
        if row.state == "purged" {
            return Err(internal(
                "a purged mutation has no recorded workspace purge",
            ));
        }
    }
    resume(transaction, key, digest, row, now, expires).map(Ok)
}

/// The first grant of a new mutation, recording the workspace it is about.
fn insert(
    transaction: &Transaction<'_>,
    key: &MutationKey,
    client_key: &str,
    digest: &Digest,
    workspace: Option<WorkspaceId>,
    expires: i64,
) -> Result<BeginOutcome, ApiError> {
    let mutation_id: MutationId = new_id!()?;
    transaction
        .execute(
            "INSERT INTO mutations (tenant_id, subject, client_key, operation,
                 idempotency_key, mutation_id, digest, state, token, lease_expires_ms,
                 workspace_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'leased', 1, ?8, ?9)",
            params![
                key.tenant_id.as_str(),
                key.subject,
                client_key,
                key.operation.as_str(),
                key.key.0.to_string(),
                mutation_id.0.to_string(),
                digest.as_str(),
                expires,
                workspace.map(|workspace| workspace.0.to_string())
            ],
        )
        .map_err(|error| sql(&error))?;
    Ok(BeginOutcome::New(MutationLease {
        mutation_id,
        token: 1,
    }))
}

/// What an existing row means for this attempt: a conflict, a replay, a live lease, or a
/// takeover under a new token.
fn resume(
    transaction: &Transaction<'_>,
    key: &MutationKey,
    digest: &Digest,
    row: LedgerRow,
    now: i64,
    expires: i64,
) -> Result<BeginOutcome, ApiError> {
    let mutation_id: MutationId = from_text!(row.mutation_id.as_str())?;
    if row.digest.as_deref() != Some(digest.as_str()) {
        return Ok(BeginOutcome::Conflict {
            operation: key.operation,
        });
    }
    match row.state.as_str() {
        "completed" => {
            let text = row
                .response
                .ok_or_else(|| internal("a completed mutation has no stored response"))?;
            let body = serde_json::from_str(&text).map_err(|error| json(&error))?;
            Ok(BeginOutcome::Replay(StoredResponse { mutation_id, body }))
        }
        "leased" if row.lease_expires_ms > now => {
            let remaining_ms = row.lease_expires_ms.saturating_sub(now);
            let seconds = remaining_ms.saturating_add(999) / 1000;
            Ok(BeginOutcome::InProgress {
                mutation_id,
                retry_after: u32::try_from(seconds.max(1)).unwrap_or(u32::MAX),
            })
        }
        _ => {
            let token = row.token.checked_add(1).ok_or_else(|| {
                ApiError::new(ErrorCode::Internal, "a mutation's lease token overflowed")
            })?;
            transaction
                .execute(
                    "UPDATE mutations SET state = 'leased', token = ?1, lease_expires_ms = ?2,
                         finished_ms = NULL
                     WHERE mutation_id = ?3",
                    params![token, expires, row.mutation_id],
                )
                .map_err(|error| sql(&error))?;
            Ok(BeginOutcome::Abandoned {
                lease: MutationLease {
                    mutation_id,
                    token: u64::try_from(token)
                        .map_err(|_| internal("a mutation's lease token is negative"))?,
                },
            })
        }
    }
}

/// Resolve the original purge for a terminal row, or a recorded purge of its effective
/// workspace for a current attempt. Purge operations keep their normal replay semantics.
fn purge_for_mutation(
    transaction: &Transaction<'_>,
    mutation: &str,
    workspace: Option<&str>,
) -> Result<Option<PurgedWorkspace>, ApiError> {
    transaction
        .query_row(
            "SELECT coalesce(m.workspace_id, ?2), p.purge_id, p.state
             FROM mutations AS m JOIN purges AS p
               ON p.tenant_id = m.tenant_id
              AND p.target_key = 'workspace/' || coalesce(m.workspace_id, ?2)
             WHERE m.mutation_id = ?1 AND m.operation NOT IN ('purge_workspace', 'purge_item')
               AND (m.purge_id IS NULL OR p.purge_id = m.purge_id)
             ORDER BY p.state = 'completed', p.rowid LIMIT 1",
            params![mutation, workspace],
            |row| {
                Ok(PurgedWorkspace {
                    workspace: row.get(0)?,
                    purge_id: row.get(1)?,
                    state: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(|error| sql(&error))
}

/// A compare-and-set that erases content but preserves the retry identity and any existing
/// finished clock. The old state and token prevent a stale completion/release from writing.
fn terminalize(
    transaction: &Transaction<'_>,
    mutation: &str,
    token: i64,
    purge: &PurgedWorkspace,
    now: i64,
    previous: &str,
) -> Result<usize, ApiError> {
    transaction
        .execute(
            "UPDATE mutations SET state = 'purged', digest = NULL, response = NULL,
                 purge_id = ?1, workspace_id = ?2, finished_ms = coalesce(finished_ms, ?3)
             WHERE mutation_id = ?4 AND token = ?5 AND state = ?6",
            params![
                purge.purge_id,
                purge.workspace,
                now,
                mutation,
                token,
                previous
            ],
        )
        .map_err(|error| sql(&error))
}

fn token_of(lease: MutationLease) -> Result<i64, ApiError> {
    i64::try_from(lease.token).map_err(|_| conflict("this lease token was never granted"))
}
