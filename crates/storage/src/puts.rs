//! Durable publication roots (`pending_puts`) written before CAS retention.
//!
//! Workspace puts record (tenant, workspace, digest) with their purge refusal. Only the
//! transaction publishing a referencing record lands the digests that record actually names;
//! Git commits/candidates land just after their reference moves, so a stop can leave a harmless
//! extra pending root. A replay never lands unrelated attempted bytes.
//!
//! Tenant archives instead record (tenant, NULL, digest, producing mutation). Their publication
//! transaction lands every tenant-level pending put of that mutation, and no other mutation's.
//! A retry can adopt its first retained pending object before creating a second archive.
//!
//! The shared sweep snapshots pending puts together with other durable roots. Recorded-purged
//! workspaces contribute no roots; only upload/pending copies of a removed workspace's archives
//! are excluded. Pins cover publication after that snapshot. Dead writes' pending puts have no
//! lifetime and are not reclaimed by age; this is an explicit lifecycle limit.
use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::{Digest, MutationId};
use rusqlite::{Connection, Transaction, params};

use crate::db::sql;
use crate::seam;

/// Record a put of `digest` in `workspace` of `tenant`, before the object is retained.
pub(crate) fn record(
    transaction: &Transaction<'_>,
    tenant: &str,
    workspace: &str,
    digest: &Digest,
) -> Result<(), ApiError> {
    transaction
        .execute(
            "INSERT INTO pending_puts (tenant_id, workspace_id, digest, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![tenant, workspace, digest.as_str(), seam::now()?.as_str()],
        )
        .map(|_| ())
        .map_err(|error| sql(&error))
}

/// A tenant archive is pending under its durable producing mutation before retention.
pub(crate) fn record_tenant(
    transaction: &Transaction<'_>,
    tenant: &str,
    mutation: MutationId,
    digest: &Digest,
) -> Result<(), ApiError> {
    transaction
        .execute(
            "INSERT INTO pending_puts (tenant_id,workspace_id,digest,created_at,mutation_id)
         VALUES (?1,NULL,?2,?3,?4)",
            params![
                tenant,
                digest.as_str(),
                seam::now()?.as_str(),
                mutation.0.to_string()
            ],
        )
        .map(|_| ())
        .map_err(|error| sql(&error))
}

/// Landing a tenant record clears only that mutation's puts, including stopped earlier bytes.
pub(crate) fn land_tenant(
    transaction: &Transaction<'_>,
    tenant: &str,
    mutation: MutationId,
) -> Result<(), ApiError> {
    transaction.execute(
        "DELETE FROM pending_puts WHERE tenant_id=?1 AND workspace_id IS NULL AND mutation_id=?2",
        params![tenant,mutation.0.to_string()],
    ).map(|_| ()).map_err(|error|sql(&error))
}

/// Retry candidates in original put order; the caller must pin before checking retention.
pub(crate) fn tenant_pending(
    connection: &Connection,
    tenant: &str,
    mutation: MutationId,
) -> Result<Vec<Digest>, ApiError> {
    let mut statement = connection
        .prepare(
            "SELECT digest FROM pending_puts WHERE tenant_id=?1 AND workspace_id IS NULL
         AND mutation_id=?2 ORDER BY put_id",
        )
        .map_err(|error| sql(&error))?;
    let values = statement
        .query_map(params![tenant, mutation.0.to_string()], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| sql(&error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    values
        .iter()
        .map(|text| from_text!(text.as_str()))
        .collect()
}

/// The records naming `digests` in `workspace` landed: delete their pending puts.
pub(crate) fn land<'a>(
    transaction: &Transaction<'_>,
    tenant: &str,
    workspace: &str,
    digests: impl IntoIterator<Item = &'a Digest>,
) -> Result<(), ApiError> {
    for digest in digests {
        transaction
            .execute(
                "DELETE FROM pending_puts
                 WHERE tenant_id = ?1 AND workspace_id IS ?2 AND digest = ?3",
                params![tenant, workspace, digest.as_str()],
            )
            .map_err(|error| sql(&error))?;
    }
    Ok(())
}
