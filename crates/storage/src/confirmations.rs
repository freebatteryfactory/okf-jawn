//! Session-bound confirmation challenges (`ConfirmationStore`) over SQLite.
//!
//! `consume` checks the subject, session, action, target, revision, digest and expiry inside one
//! transaction and then sets `consumed_by` only where it is empty or already this mutation, so a
//! confirmation is used by exactly one `MutationId`; the same mutation may consume it again after
//! a crash, any other is refused as already used.

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{MutationId, Timestamp};
use okf_jawn_contract::review::{Confirmation, ConfirmationTarget};
use okf_jawn_core::confirmations::{ConfirmationConsume, ConfirmationCreate, ConfirmationStore};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::StorageScope;
use rusqlite::{OptionalExtension, Transaction, params};

use crate::db::{Db, conflict, internal, json, not_found, scope_key, sql};
use crate::seam;

/// `ConfirmationStore` over the records database.
#[derive(Debug, Clone)]
pub struct SqliteConfirmations {
    db: Db,
}

/// A stored challenge.
struct Challenge {
    id: String,
    action: String,
    target: String,
    revision: String,
    content_digest: String,
    session_id: String,
    subject: String,
    expires_at: String,
    consumed_by: Option<String>,
}

impl SqliteConfirmations {
    pub(crate) const fn new(db: Db) -> Self {
        Self { db }
    }
}

impl ConfirmationStore for SqliteConfirmations {
    fn create<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        create: ConfirmationCreate,
    ) -> PortFuture<'a, Confirmation> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.transaction(move |transaction| {
            let expires_at = canonical_expiry(&create.expires_at)?;
            let id: okf_jawn_contract::identity::ConfirmationId = new_id!()?;
            transaction
                .execute(
                    "INSERT INTO confirmations (confirmation_id, tenant_id, workspace_id,
                         mutation_id, action, target, revision, content_digest, session_id,
                         subject, expires_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT (mutation_id) DO NOTHING",
                    params![
                        id.0.to_string(),
                        tenant,
                        workspace,
                        mutation_id.0.to_string(),
                        variant_text!(&create.action)?,
                        target_text(&create.target)?,
                        create.revision.as_str(),
                        create.content_digest.as_str(),
                        create.session_id,
                        create.subject,
                        expires_at.as_str()
                    ],
                )
                .map_err(|error| sql(&error))?;
            let challenge = challenge(transaction, "mutation_id", &mutation_id.0.to_string())?
                .ok_or_else(|| internal("a confirmation row vanished after its insert"))?;
            confirmation(&challenge)
        }))
    }

    fn consume<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        consume: ConfirmationConsume,
    ) -> PortFuture<'a, Confirmation> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.transaction(move |transaction| {
            let id = consume.confirmation_id.0.to_string();
            let found: Option<(String, String)> = transaction
                .query_row(
                    "SELECT tenant_id, workspace_id FROM confirmations WHERE confirmation_id = ?1",
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| sql(&error))?;
            if found != Some((tenant, workspace)) {
                return Err(not_found("confirmation"));
            }
            let challenge = challenge(transaction, "confirmation_id", &id)?
                .ok_or_else(|| not_found("confirmation"))?;
            check(&challenge, &consume, mutation_id)?;
            transaction
                .execute(
                    "UPDATE confirmations SET consumed_by = ?1
                     WHERE confirmation_id = ?2 AND (consumed_by IS NULL OR consumed_by = ?1)",
                    params![mutation_id.0.to_string(), id],
                )
                .map_err(|error| sql(&error))?;
            confirmation(&challenge)
        }))
    }
}

/// Every check `consume` makes before it records the consumer.
fn check(
    challenge: &Challenge,
    consume: &ConfirmationConsume,
    mutation_id: MutationId,
) -> Result<(), ApiError> {
    if challenge.subject != consume.subject || challenge.session_id != consume.session_id {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "this confirmation belongs to another session",
        ));
    }
    if challenge
        .consumed_by
        .as_deref()
        .is_some_and(|consumer| consumer != mutation_id.0.to_string())
    {
        return Err(conflict("this confirmation was already used"));
    }
    if challenge.action != variant_text!(&consume.action)?
        || challenge.target != target_text(&consume.target)?
    {
        return Err(conflict(
            "this confirmation was issued for another action or target",
        ));
    }
    if challenge.revision != consume.revision.as_str()
        || challenge.content_digest != consume.content_digest.as_str()
    {
        return Err(conflict(
            "this confirmation was issued for another revision or content",
        ));
    }
    if challenge.expires_at.as_str() <= seam::now()?.as_str() {
        return Err(conflict("this confirmation has expired"));
    }
    Ok(())
}

fn challenge(
    transaction: &Transaction<'_>,
    column: &str,
    value: &str,
) -> Result<Option<Challenge>, ApiError> {
    transaction
        .query_row(
            &format!(
                "SELECT confirmation_id, action, target, revision, content_digest, session_id,
                     subject, expires_at, consumed_by
                 FROM confirmations WHERE {column} = ?1"
            ),
            [value],
            |row| {
                Ok(Challenge {
                    id: row.get(0)?,
                    action: row.get(1)?,
                    target: row.get(2)?,
                    revision: row.get(3)?,
                    content_digest: row.get(4)?,
                    session_id: row.get(5)?,
                    subject: row.get(6)?,
                    expires_at: row.get(7)?,
                    consumed_by: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(|error| sql(&error))
}

fn confirmation(challenge: &Challenge) -> Result<Confirmation, ApiError> {
    Ok(Confirmation {
        id: from_text!(challenge.id.as_str())?,
        expires_at: from_text!(challenge.expires_at.as_str())?,
        revision: from_text!(challenge.revision.as_str())?,
    })
}

/// The canonical JSON of a target, compared as text.
fn target_text(target: &ConfirmationTarget) -> Result<String, ApiError> {
    serde_json::to_string(target).map_err(|error| json(&error))
}

/// The expiry as a canonical `Timestamp`, so it compares as text against the clock.
pub(crate) fn canonical_expiry(expires_at: &str) -> Result<Timestamp, ApiError> {
    Timestamp::try_from(expires_at.to_owned()).map_err(|error| {
        ApiError::new(
            ErrorCode::InvalidInput,
            format!("an expiry must be a canonical UTC instant: {error}"),
        )
        .with_field("/expires_at")
    })
}
