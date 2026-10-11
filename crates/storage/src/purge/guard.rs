//! The refusal of every write to a workspace a purge names (review B6, window 1).
//!
//! A purge plans what the workspace references once, at its first step, and removes the
//! repository only at its fifth: a write that landed in between would leave content outside
//! the plan. So from the moment core records the purge (`RecordStore::create_purge`, before it
//! calls the purger) every storage write scoped to that workspace is refused with `Conflict`,
//! naming the purge, the same way the catalog refuses to create a purged identity again. The
//! purger itself does not go through these writes. A completed purge keeps refusing: the
//! workspace is gone, and a stale writer must not leave a row or an object naming it.
//!
//! Where a write is one SQLite transaction the check runs inside it, so the purge record and
//! the write are ordered by the database: new jobs, job retries and claims (a refused claim
//! leaves the job as it is, for core to cancel; cancellation is never refused), drafts (save,
//! discard), upload slots,
//! upload content (which then removes the part file it appended to), upload completions (also
//! checked before anything is read, so one after the purge is refused, not answered
//! `NotFound`), artifacts, job completions, derived objects, receipts, reviews, proposals
//! (insert, update), comments, confirmations (create, consume), sandbox capabilities and the
//! catalog (update, archive, unarchive). Commits, candidates and promotions check under the
//! workspace's write lock, under which the purge's plan reads the workspace. An object put
//! checks before it receives the body and again after it pinned the object, in the
//! transaction that records its pending put (`LocalBlobs::put_pinned`, `puts`), so a put that
//! passed the first check before the purge was recorded either is waited for by the purge's
//! objects step or records and stores nothing. Written histories and workspace archives check
//! before they take the workspace's lock and again under it, and record themselves before they
//! release it, so the purge's plan either refuses them or reads their record. An import puts
//! its objects through `put_pinned`, so each is checked again in its pending-put transaction,
//! and its Git writes are refused under the lock, but it copies its rows without the check;
//! that is job work, which core drains before it calls the purger.
//!
//! Events are not refused. An event carries identities only (its kind, revision, item, job,
//! connector, actor and operation), and one appended after the rows step is tenant-level data
//! of the same kind as the revision map and the purge record, which the purge keeps.

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::storage::StorageScope;
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::{Db, conflict, scope_key, sql};

/// How every refusal's message ends (`is_refusal`).
const REFUSED: &str = "; nothing is written to it";

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
            "workspace {workspace} is purged by purge {purge} ({state}){REFUSED}"
        ))),
        None => Ok(()),
    }
}

/// Whether `error` is a refusal `refuse` made, so a write can undo what it did outside its
/// transaction before the refusal.
pub(crate) fn is_refusal(error: &ApiError) -> bool {
    error.code == ErrorCode::Conflict && error.message.ends_with(REFUSED)
}

/// `refuse` for `scope`, in its own read.
pub(crate) async fn refuse_scope(records: &Db, scope: &StorageScope) -> Result<(), ApiError> {
    let (tenant, workspace) = scope_key(scope);
    records
        .call(move |connection| refuse(connection, &tenant, &workspace))
        .await
}
