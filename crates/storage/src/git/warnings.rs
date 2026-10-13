//! The warnings a commit's candidate check gave, kept so a replay of the commit answers them
//! (`VersionStore::commit`, records schema 3, table `commit_warnings`).
//!
//! The rule a reader can rely on: a replay answers the warnings that commit was given, or says
//! they are unavailable; never another attempt's.
//!
//! - `keep` writes the row of (workspace, mutation) before the commit lands: the warnings and
//!   the identity of the commit they were kept for, which is known before the reference moves.
//!   It replaces what an earlier attempt of the mutation kept, commit and warnings together. It
//!   runs under the workspace's write lock, after the write's purge refusal
//!   (`git::refuse_locked`), like the commit it belongs to.
//! - `replayed` answers a commit that already exists: its row's warnings when the row names
//!   that commit, and otherwise the one warning `replay_warnings_unavailable`, with the reason:
//!   there is no row, the row was kept for another commit, or the row does not read as a list
//!   of warnings. A commit that landed is never failed for its warnings; only a fault of the
//!   records database itself fails the call, and a retry meets the same commit again.
//!
//! A row can name another commit than the one found because the key is the mutation, not the
//! commit: a later attempt of a mutation whose commit landed can miss that commit (its expected
//! head is at or after it), pass its check, keep its own warnings and then fail to land. The
//! row is then that attempt's, and the landed commit is answered as having none available.
//! The same replacement is what clears a row an attempt left without a commit, so it stays.
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
/// Why a replay has no warnings to answer: the row of the mutation was kept for another commit,
/// one a later attempt wrote and could not land.
const OTHER_ATTEMPT: &str =
    "the ones the store holds under this mutation were kept for another attempt's commit";
/// Why a replay has no warnings to answer: the row is not a list of warnings this build reads.
const NOT_READABLE: &str = "the ones the store kept for this commit could not be read";

/// Keep `warnings` as the answer of `commit`, the commit `mutation` is about to land in
/// `scope`, replacing what an earlier attempt kept. Blocking: call it from the write's blocking
/// task.
pub(super) fn keep(
    records: &Db,
    scope: &StorageScope,
    mutation: MutationId,
    commit: Oid,
    warnings: &[Warning],
) -> Result<(), ApiError> {
    let (tenant, workspace) = scope_key(scope);
    let warnings = serde_json::to_string(warnings).map_err(|error| json(&error))?;
    let commit = commit.to_string();
    tokio::runtime::Handle::current().block_on(records.transaction(move |transaction| {
        transaction
            .execute(
                "INSERT INTO commit_warnings
                     (tenant_id, workspace_id, mutation_id, commit_id, warnings)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (tenant_id, workspace_id, mutation_id)
                 DO UPDATE SET commit_id = excluded.commit_id, warnings = excluded.warnings",
                params![tenant, workspace, mutation.0.to_string(), commit, warnings],
            )
            .map(|_| ())
            .map_err(|error| sql(&error))
    }))
}

/// The answer of a commit that already exists: `found`, as a replay, with the warnings kept
/// for it under `mutation` in `scope` (module header). Blocking.
pub(super) fn replayed(
    records: &Db,
    scope: &StorageScope,
    mutation: MutationId,
    found: Oid,
) -> Result<Committed, ApiError> {
    let (tenant, workspace) = scope_key(scope);
    let kept: Option<(String, String)> =
        tokio::runtime::Handle::current().block_on(records.call(move |connection| {
            connection
                .query_row(
                    "SELECT commit_id, warnings FROM commit_warnings
                     WHERE tenant_id = ?1 AND workspace_id = ?2 AND mutation_id = ?3",
                    params![tenant, workspace, mutation.0.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| sql(&error))
        }))?;
    let warnings = match kept {
        None => vec![replay_warnings_unavailable(NOT_KEPT)],
        // The row is another attempt's: its warnings are about another tree.
        Some((commit, _)) if commit != found.to_string() => {
            vec![replay_warnings_unavailable(OTHER_ATTEMPT)]
        }
        // A row another build wrote in a shape this one does not read is not a reason to fail
        // a commit that landed: the caller is told the warnings are not available.
        Some((_, text)) => serde_json::from_str::<Vec<Warning>>(&text)
            .unwrap_or_else(|_| vec![replay_warnings_unavailable(NOT_READABLE)]),
    };
    Ok(Committed {
        revision: revision_of(found)?,
        replayed: true,
        warnings,
    })
}
