//! The mutation ledger (`MutationStore`) over SQLite.
//!
//! One row per (tenant, subject, client, operation, idempotency key), primary key. `begin`
//! grants a lease whose token is one more than the row's last token, so every grant of a
//! mutation carries a token no earlier grant carried. `complete` and `release` are
//! compare-and-set updates on (mutation, token, `leased`): a superseded attempt changes nothing.
//! A completed or released row is kept 7 days from when it finished; a row whose lease expired
//! without either stays until a later attempt reconciles it. The lease length is
//! `MUTATION_LEASE_SECONDS`.

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, MutationId};
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
    digest: String,
    state: String,
    token: i64,
    lease_expires_ms: i64,
    response: Option<String>,
}

/// How long a granted lease lasts before another attempt may take the mutation over.
pub const MUTATION_LEASE_SECONDS: i64 = 120;
/// How long a completed or released row is kept.
pub const RETENTION_SECONDS: i64 = 7 * 24 * 60 * 60;

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
    ) -> PortFuture<'a, BeginOutcome> {
        let key = key.clone();
        let digest = digest.clone();
        Box::pin(
            self.db
                .transaction(move |transaction| begin(transaction, &key, &digest)),
        )
    }

    fn complete(&self, lease: MutationLease, response: serde_json::Value) -> PortFuture<'_, ()> {
        Box::pin(self.db.transaction(move |transaction| {
            let body = serde_json::to_string(&response).map_err(|error| json(&error))?;
            let token = token_of(lease)?;
            let mutation = lease.mutation_id.0.to_string();
            let changed = transaction
                .execute(
                    "UPDATE mutations SET state = 'completed', response = ?1, finished_ms = ?2
                     WHERE mutation_id = ?3 AND token = ?4 AND state = 'leased'",
                    params![body, seam::now_ms()?, mutation, token],
                )
                .map_err(|error| sql(&error))?;
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
                Some((state, current)) if state == "completed" && current == token => Ok(()),
                Some(_) => Err(conflict(
                    "another attempt holds this mutation; this lease is no longer the current grant",
                )),
            }
        }))
    }

    fn release(&self, lease: MutationLease) -> PortFuture<'_, ()> {
        Box::pin(self.db.transaction(move |transaction| {
            transaction
                .execute(
                    "UPDATE mutations SET state = 'released', finished_ms = ?1
                     WHERE mutation_id = ?2 AND token = ?3 AND state = 'leased'",
                    params![
                        seam::now_ms()?,
                        lease.mutation_id.0.to_string(),
                        token_of(lease)?
                    ],
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
) -> Result<BeginOutcome, ApiError> {
    let now = seam::now_ms()?;
    let retention_ms = RETENTION_SECONDS.saturating_mul(1000);
    transaction
        .execute(
            "DELETE FROM mutations
             WHERE state IN ('completed', 'released') AND finished_ms <= ?1",
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
        return insert(transaction, key, &client_key, digest, expires);
    };
    resume(transaction, key, digest, row, now, expires)
}

/// The first grant of a new mutation.
fn insert(
    transaction: &Transaction<'_>,
    key: &MutationKey,
    client_key: &str,
    digest: &Digest,
    expires: i64,
) -> Result<BeginOutcome, ApiError> {
    let mutation_id: MutationId = new_id!()?;
    transaction
        .execute(
            "INSERT INTO mutations (tenant_id, subject, client_key, operation,
                 idempotency_key, mutation_id, digest, state, token, lease_expires_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'leased', 1, ?8)",
            params![
                key.tenant_id.as_str(),
                key.subject,
                client_key,
                key.operation.as_str(),
                key.key.0.to_string(),
                mutation_id.0.to_string(),
                digest.as_str(),
                expires
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
    if row.digest != digest.as_str() {
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

fn token_of(lease: MutationLease) -> Result<i64, ApiError> {
    i64::try_from(lease.token).map_err(|_| conflict("this lease token was never granted"))
}
