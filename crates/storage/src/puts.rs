//! Durable records of object puts whose referencing record has not landed (`pending_puts`).
//!
//! Objects are shared inside a tenant, and the record that references a freshly stored object
//! lands only later: a job's outputs when it completes, a card when its commit lands, an
//! upload's object when it completes. So every workspace-scoped put writes a pending-put row
//! (tenant, workspace, digest) before it retains the object, in the same transaction as its
//! purge refusal (`LocalBlobs::put_pinned`). The rows of a digest are deleted (`land`) in the
//! transaction of a record of the workspace that names the object: a job completion, an
//! artifact, a derived object, a sandbox capability, an upload completion, an import's rows.
//! What lands is what the stored record names, read back after the write: a write answered by
//! an earlier record (a replayed artifact, commit or candidate) lands only the objects that
//! record names, and an import lands only the archive objects its restored history and rows
//! name. A commit or candidate is a Git write, so its rows are deleted just after it landed; a
//! crash between the two leaves a row naming an object that a record references, which costs
//! nothing.
//!
//! A put whose record never lands (a failed job, a failed retain, a refused record write, a
//! crash, a replay of a record that names other bytes) leaves its row. The rows have no
//! lifetime:
//!
//! - a purge's objects step treats the target's rows as the target's own unreferenced bytes:
//!   it deletes each object they name unless a surviving reference names it, then clears them;
//! - another workspace's rows count as surviving references, so its objects stay, except a
//!   copy of the target's own workspace archive (`references::except_archives`);
//! - a leftover row that never gets a record is garbage of its workspace, collected by the next
//!   purge of that workspace (until then it keeps an identical object of another purge, which
//!   that purge counts as kept). No other sweep collects it.
//!
//! Cost: a put adds one immediate write transaction (the refusal and the row), so one more
//! synced commit per workspace-scoped put; a Git write that names objects adds one more for
//! its landing, and a replay reads the stored commit's appearances. Landing is one indexed
//! `DELETE` per digest inside the record's transaction. Leftover rows accumulate until their
//! workspace is purged, and every purge of the tenant reads all other workspaces' rows.

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::Digest;
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
                 WHERE tenant_id = ?1 AND workspace_id = ?2 AND digest = ?3",
                params![tenant, workspace, digest.as_str()],
            )
            .map_err(|error| sql(&error))?;
    }
    Ok(())
}

/// The objects of `workspace`'s pending puts.
pub(crate) fn of(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
) -> Result<Vec<Digest>, ApiError> {
    digests(
        connection,
        "SELECT DISTINCT digest FROM pending_puts WHERE tenant_id = ?1 AND workspace_id = ?2",
        tenant,
        workspace,
    )
}

/// The objects of every pending put of `tenant` outside `workspace`: surviving references.
pub(crate) fn others(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
) -> Result<Vec<Digest>, ApiError> {
    digests(
        connection,
        "SELECT DISTINCT digest FROM pending_puts WHERE tenant_id = ?1 AND workspace_id <> ?2",
        tenant,
        workspace,
    )
}

/// Delete every pending put of `workspace`; a purge's objects step does, after it deleted
/// the bytes they name.
pub(crate) fn clear(
    transaction: &Transaction<'_>,
    tenant: &str,
    workspace: &str,
) -> Result<(), ApiError> {
    transaction
        .execute(
            "DELETE FROM pending_puts WHERE tenant_id = ?1 AND workspace_id = ?2",
            params![tenant, workspace],
        )
        .map(|_| ())
        .map_err(|error| sql(&error))
}

fn digests(
    connection: &Connection,
    query: &str,
    tenant: &str,
    workspace: &str,
) -> Result<Vec<Digest>, ApiError> {
    let mut statement = connection.prepare(query).map_err(|error| sql(&error))?;
    let found = statement
        .query_map(params![tenant, workspace], |row| row.get::<_, String>(0))
        .map_err(|error| sql(&error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    found
        .iter()
        .map(|digest| from_text!(digest.as_str()))
        .collect()
}
