//! The warnings a commit's candidate check gave, kept so a replay of the commit answers them
//! (`VersionStore::commit`, records schema 3, table `commit_warnings`).
//!
//! - `keep` writes the row of (workspace, mutation) before the commit lands, replacing a row an
//!   earlier attempt left without a commit. It runs under the workspace's write lock, after the
//!   write's purge refusal (`git::refuse_locked`), like the commit it belongs to.
//! - `replayed` answers a commit that already exists: its row's warnings, or the one warning
//!   `replay_warnings_unavailable` when there is no row or the row does not read as a list of
//!   warnings. A commit that landed is never failed for its warnings; only a fault of the
//!   records database itself fails the call, and a retry meets the same commit again.
//!
//! The rows are removed only by the purge of their workspace (`purge::rows`).

use git2::Oid;
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::MutationId;
use okf_jawn_core::storage::{Committed, StorageScope, replay_warnings_unavailable};
use rusqlite::{OptionalExtension, params};

use super::repo::revision_of;
use crate::db::{Db, json, scope_key, sql};

/// Why a replay has no warnings to answer: the store holds no row for the commit.
const NOT_KEPT: &str = "the store kept none for this commit";
/// Why a replay has no warnings to answer: the row is not a list of warnings this build reads.
const NOT_READABLE: &str = "the ones the store kept for this commit could not be read";

/// Keep `warnings` as the answer of the commit `mutation` is about to land in `scope`,
/// replacing what an earlier attempt kept. Blocking: call it from the write's blocking task.
pub(super) fn keep(
    records: &Db,
    scope: &StorageScope,
    mutation: MutationId,
    warnings: &[Warning],
) -> Result<(), ApiError> {
    let (tenant, workspace) = scope_key(scope);
    let warnings = serde_json::to_string(warnings).map_err(|error| json(&error))?;
    tokio::runtime::Handle::current().block_on(records.transaction(move |transaction| {
        transaction
            .execute(
                "INSERT INTO commit_warnings (tenant_id, workspace_id, mutation_id, warnings)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (tenant_id, workspace_id, mutation_id)
                 DO UPDATE SET warnings = excluded.warnings",
                params![tenant, workspace, mutation.0.to_string(), warnings],
            )
            .map(|_| ())
            .map_err(|error| sql(&error))
    }))
}

/// The answer of a commit that already exists: `found`, as a replay, with the warnings kept
/// for `mutation` in `scope` (module header). Blocking.
pub(super) fn replayed(
    records: &Db,
    scope: &StorageScope,
    mutation: MutationId,
    found: Oid,
) -> Result<Committed, ApiError> {
    let (tenant, workspace) = scope_key(scope);
    let kept: Option<String> =
        tokio::runtime::Handle::current().block_on(records.call(move |connection| {
            connection
                .query_row(
                    "SELECT warnings FROM commit_warnings
                     WHERE tenant_id = ?1 AND workspace_id = ?2 AND mutation_id = ?3",
                    params![tenant, workspace, mutation.0.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| sql(&error))
        }))?;
    let warnings = match kept {
        None => vec![replay_warnings_unavailable(NOT_KEPT)],
        // A row another build wrote in a shape this one does not read is not a reason to fail
        // a commit that landed: the caller is told the warnings are not available.
        Some(text) => serde_json::from_str::<Vec<Warning>>(&text)
            .unwrap_or_else(|_| vec![replay_warnings_unavailable(NOT_READABLE)]),
    };
    Ok(Committed {
        revision: revision_of(found)?,
        replayed: true,
        warnings,
    })
}
