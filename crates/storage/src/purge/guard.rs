//! The refusal of every write to a workspace a purge names (review B6, window 1).
//!
//! A purge plans what the workspace references once, at its first step, and removes the
//! repository only at its fifth: a write that landed in between (a commit, an object, a draft,
//! an upload, an artifact or a job completion) would leave content outside the plan. So from
//! the moment core records the purge (`RecordStore::create_purge`, before it calls the purger)
//! every storage write scoped to that workspace is refused with `Conflict`, naming the purge,
//! the same way the catalog refuses to create a purged identity again. The purger itself does
//! not go through these writes. A completed purge keeps refusing: the workspace is gone, and a
//! stale writer must not leave a row or an object naming it.
//!
//! The check runs in the write's own transaction where the write is one SQLite transaction, so
//! the purge record and the write are ordered by the database; a Git write checks before it
//! takes the workspace's lock, and core drains the workspace's jobs before calling the purger
//! (the `Purger` contract), so no write that passed the check is still running when the plan
//! is read.

use okf_jawn_contract::error::ApiError;
use okf_jawn_core::storage::StorageScope;
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::{Db, conflict, scope_key, sql};

/// Refuse a write to `workspace` when a purge names it.
pub(crate) fn refuse(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
) -> Result<(), ApiError> {
    let found: Option<(String, String)> = connection
        .query_row(
            "SELECT purge_id, state FROM purges WHERE tenant_id = ?1 AND target_key = ?2
             ORDER BY state = 'completed' LIMIT 1",
            params![tenant, format!("workspace/{workspace}")],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    match found {
        Some((purge, state)) => Err(conflict(format!(
            "workspace {workspace} is purged by purge {purge} ({state}); nothing is written to it"
        ))),
        None => Ok(()),
    }
}

/// `refuse` for `scope`, in its own read.
pub(crate) async fn refuse_scope(records: &Db, scope: &StorageScope) -> Result<(), ApiError> {
    let (tenant, workspace) = scope_key(scope);
    records
        .call(move |connection| refuse(connection, &tenant, &workspace))
        .await
}
