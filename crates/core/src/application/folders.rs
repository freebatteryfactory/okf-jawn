//! A folder as a whole (contract ruling 9): `move_folder` and `delete_folder`, each one commit
//! of one edit (`TreeEdit::MoveFolder`, `TreeEdit::DeleteFolder`) that the store applies to
//! every item and file under the folder.
//!
//! What core decides around the store's edit:
//!
//! - The paths alone are refused before any port is read. A reserved folder (`.okf`,
//!   `index.md` or `log.md` as any segment, in any letter case: `conventions::reserved`) is
//!   `InvalidInput` on `/folder`, so `.okf/` and everything below it can be neither moved nor
//!   deleted. The root cannot be named at all: a `WorkspacePath` is never empty. For a move, a
//!   reserved destination, the folder itself, or a path inside it (compared by collision key)
//!   is `InvalidInput` on `/destination`; a destination that differs from the folder only in
//!   letter case or normalization renames it, which the store does in two hops. What needs the
//!   tree (a folder the tree does not hold, a destination already taken) is the store's to
//!   refuse.
//! - The base is checked against the revision map, then the folder's items are read at it
//!   (`VersionStore::tree`, archived items and items below a folder whose name starts with `.`
//!   included) and counted for `FolderResult::items`. Their paths at the base, and for a move
//!   under the destination, are the files the write touches, so the edit check reports their
//!   lint; the store tells the check which other files its moves rewrote
//!   (`CandidateCheck::check`).
//! - "A folder changed since `base_revision` conflicts" (`folder_commit`). The store refuses
//!   any moved head, so the edit is committed on the base first. When the store answers that
//!   the head moved (`ErrorDetail::StaleBase`), the store's comparison of the base with that
//!   head (`VersionStore::diff`) is read; when no file at or under the folder, nor at or under
//!   the destination of a move, nor a companion of a contained item changed between them (at
//!   either end of a change, paths by collision key), the same edit is committed on that head.
//!   Companion ownership is the same rule item-filtered history uses. Otherwise the answer is
//!   `Conflict` with
//!   `StaleBase` naming that head, on `/base_revision`, and so is a comparison the store cannot
//!   make because it no longer holds the base. A change elsewhere in the workspace never blocks
//!   a folder operation. The comparison is read as listing every changed file, which the
//!   store's does (its warnings name no file it left out). The items counted at the base are
//!   the items under the folder at that head too, since nothing under it changed between them.
//! - The comparison and the commit on the head are not atomic: another write can land between
//!   them. The store's own head check closes that window: it refuses the commit (`StaleBase`
//!   naming the newer head, answered on `/base_revision`), so nothing lands on a head that was
//!   not compared.
//! - A retry repeats its first attempt: the commit that attempt landed, on the base or on the
//!   head it compared, lies after the base, so the store answers the commit on the base as a
//!   replay of it, with the warnings it kept, and nothing is compared again. The count is read
//!   at the base again, which has not changed.
//!
//! Drafts are keyed by item (`drafts`), so a moved item's draft stays with it, and a deleted
//! item's draft is kept as `delete_item` keeps one: neither operation touches the draft store.
//! A delete removes the folder from the tree only; history and retained objects stay, so a
//! Rewind of an item brings it back. Neither is offered to proposals (contract ruling 9).

use std::collections::BTreeSet;

use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::history::FileChange;
use okf_jawn_contract::identity::{ItemId, Revision, WorkspacePath, collision_key_of};
use okf_jawn_contract::item::{DeleteFolderRequest, FolderResult, MoveFolderRequest};

use super::ApplicationService;
use super::shared::{
    Touched, Write, announce, check_named, invalid, mutation_id, on_the_base, record_receipt,
    store_commit, workspace_scope,
};
use crate::context::OperationContext;
use crate::conventions::reserved;
use crate::storage::{
    Committed, DiffQuery, StorageScope, TreeEdit, TreeItem, item_companion_owner,
};

/// The edit, the message and the touched files of one folder write, committed on a base.
struct FolderWrite<'a> {
    /// The folder the write changes, and for a move its destination: a change at or under any
    /// of these since the base conflicts.
    under: Vec<&'a WorkspacePath>,
    /// The contained items whose versioned companions change with them, even outside `under`.
    items: BTreeSet<ItemId>,
    /// Commit message, without the trailer.
    message: String,
    /// The one edit.
    edit: TreeEdit,
    /// The files the write touches, whose lint it reports.
    touched: Vec<WorkspacePath>,
}

/// Move a folder with everything under it to `destination` in one commit, and count its items.
///
/// # Errors
/// Returns `InvalidInput` on `/folder` for a reserved folder; `InvalidInput` on `/destination`
/// for a reserved destination, the folder itself, a path inside it, or one under which an
/// item's path would not be a workspace path; the purge check's refusal of the base; `Conflict`
/// with `StaleBase` on `/base_revision` when the folder or the destination changed since the
/// base; the store's refusal of the edit; or any port error.
pub(super) async fn move_folder(
    service: &ApplicationService,
    context: &OperationContext,
    request: MoveFolderRequest,
) -> Result<FolderResult, ApiError> {
    let MoveFolderRequest {
        base_revision,
        folder,
        destination,
        ..
    } = request;
    refuse_reserved(&folder, "/folder")?;
    refuse_reserved(&destination, "/destination")?;
    if destination == folder {
        return Err(invalid(
            format!("{} is the folder itself", destination.as_str()),
            "/destination",
        ));
    }
    if under(destination.as_str(), &folder) {
        return Err(invalid(
            format!(
                "{} is inside {}, which cannot move into itself",
                destination.as_str(),
                folder.as_str()
            ),
            "/destination",
        ));
    }
    let scope = workspace_scope(context)?;
    check_named(service, &scope, &base_revision).await?;
    let held = items_under(service, &scope, &base_revision, &folder).await?;
    let mut touched: Vec<WorkspacePath> =
        held.iter().map(|item| item.summary.path.clone()).collect();
    for item in &held {
        touched.push(moved_to(&item.summary.path, &folder, &destination)?);
    }
    let write = FolderWrite {
        under: vec![&folder, &destination],
        items: held.iter().map(|item| item.summary.id).collect(),
        message: format!(
            "Move folder {} to {}",
            folder.as_str(),
            destination.as_str()
        ),
        edit: TreeEdit::MoveFolder {
            folder: folder.clone(),
            destination: destination.clone(),
        },
        touched,
    };
    let committed = folder_commit(service, context, &base_revision, &write).await?;
    finish(service, context, &scope, committed, held.len()).await
}

/// Remove a folder with everything under it in one commit, and count its items; history and
/// retained objects stay.
///
/// # Errors
/// Returns `InvalidInput` on `/folder` for a reserved folder; the purge check's refusal of the
/// base; `Conflict` with `StaleBase` on `/base_revision` when the folder changed since the
/// base; the store's refusal of the edit (`NotFound` on `/folder` for a folder it does not
/// hold); or any port error.
pub(super) async fn delete_folder(
    service: &ApplicationService,
    context: &OperationContext,
    request: DeleteFolderRequest,
) -> Result<FolderResult, ApiError> {
    let DeleteFolderRequest {
        base_revision,
        folder,
        ..
    } = request;
    refuse_reserved(&folder, "/folder")?;
    let scope = workspace_scope(context)?;
    check_named(service, &scope, &base_revision).await?;
    let held = items_under(service, &scope, &base_revision, &folder).await?;
    let items = held.len();
    let write = FolderWrite {
        under: vec![&folder],
        items: held.iter().map(|item| item.summary.id).collect(),
        message: format!("Delete folder {}", folder.as_str()),
        edit: TreeEdit::DeleteFolder {
            folder: folder.clone(),
        },
        touched: held.into_iter().map(|item| item.summary.path).collect(),
    };
    let committed = folder_commit(service, context, &base_revision, &write).await?;
    finish(service, context, &scope, committed, items).await
}

/// Commit one folder write as the module header says: on the base, or, when the head moved and
/// nothing under the folder (or the destination) changed since the base, on that head; then
/// index and announce the revision.
///
/// # Errors
/// Returns `Conflict` with `StaleBase` on `/base_revision` when something under the folder or
/// the destination changed, when the store cannot compare the base with the head, or when the
/// head moved again before the commit on it; the store's other refusals; or any port error.
async fn folder_commit(
    service: &ApplicationService,
    context: &OperationContext,
    base: &Revision,
    write: &FolderWrite<'_>,
) -> Result<Committed, ApiError> {
    let scope = workspace_scope(context)?;
    let on = |at: &Revision| Write {
        base: at.clone(),
        message: write.message.clone(),
        edits: vec![write.edit.clone()],
        item: None,
        touched: Touched {
            paths: write.touched.clone(),
            ..Touched::default()
        },
    };
    let committed = match store_commit(service, context, on(base)).await {
        Ok(committed) => committed,
        Err(refused) => {
            let Some(ErrorDetail::StaleBase { head }) = refused.detail.as_deref() else {
                return Err(refused);
            };
            let head = head.clone();
            if let Some(why) =
                changed_between(service, &scope, base, &head, &write.under, &write.items).await?
            {
                return Err(ApiError::new(ErrorCode::Conflict, why)
                    .with_detail(ErrorDetail::StaleBase { head })
                    .with_field("/base_revision"));
            }
            store_commit(service, context, on(&head))
                .await
                .map_err(on_the_base)?
        }
    };
    announce(
        service,
        scope,
        mutation_id(context)?,
        &committed.revision,
        None,
    )
    .await?;
    Ok(committed)
}

/// Why the write cannot be committed on `head`: what changed at or under one of `folders`
/// between `base` and `head`, by the store's comparison, or that the store no longer holds
/// `base` (`NotFound`), so nothing can show that nothing changed. `None` when nothing there
/// changed.
///
/// # Errors
/// Returns any port error of the comparison but `NotFound`.
async fn changed_between(
    service: &ApplicationService,
    scope: &StorageScope,
    base: &Revision,
    head: &Revision,
    folders: &[&WorkspacePath],
    items: &BTreeSet<ItemId>,
) -> Result<Option<String>, ApiError> {
    let compared = service
        .ports()
        .versions
        .diff(
            scope,
            DiffQuery {
                from: base.clone(),
                to: head.clone(),
                item_id: None,
            },
        )
        .await;
    let diff = match compared {
        Ok(diff) => diff,
        Err(error) if error.code == ErrorCode::NotFound => {
            return Ok(Some(format!(
                "the workspace head moved and the base revision can no longer be compared with \
                 it, so the folder may have changed: {}",
                error.message
            )));
        }
        Err(error) => return Err(error),
    };
    Ok(diff
        .changes
        .iter()
        .find_map(|change| changed_at(change, folders, items))
        .map(|path| format!("{path} changed since the base revision")))
}

/// The end of `change` (its old or new path) under `folders` or owned as a companion by one of
/// their contained `items`, if any.
fn changed_at(
    change: &FileChange,
    folders: &[&WorkspacePath],
    items: &BTreeSet<ItemId>,
) -> Option<String> {
    [change.old_path.as_deref(), change.new_path.as_deref()]
        .into_iter()
        .flatten()
        .find(|path| {
            folders
                .iter()
                .any(|folder| path_is(path, folder) || under(path, folder))
                || item_companion_owner(path).is_some_and(|item| items.contains(&item))
        })
        .map(str::to_owned)
}

/// Whether `path` is `folder`, compared by collision key.
fn path_is(path: &str, folder: &WorkspacePath) -> bool {
    collision_key_of(path) == folder.collision_key()
}

/// Whether `path` lies below `folder`, compared by collision key, so `Notes/a.md` is below
/// `notes` and `notes-old/a.md` is not.
fn under(path: &str, folder: &WorkspacePath) -> bool {
    collision_key_of(path)
        .strip_prefix(&folder.collision_key())
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Every item under `folder` at `revision`, in path order, archived items and items
/// below a folder whose name starts with `.` included. `folder` is matched as it is spelled,
/// as the store matches it (a folder the tree spells another way is the store's to refuse).
///
/// # Errors
/// Returns any port error.
async fn items_under(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
    folder: &WorkspacePath,
) -> Result<Vec<TreeItem>, ApiError> {
    let prefix = format!("{}/", folder.as_str());
    Ok(service
        .ports()
        .versions
        .tree(scope, revision)
        .await?
        .items
        .into_iter()
        .filter(|item| item.summary.path.as_str().starts_with(&prefix))
        .collect())
}

/// Where `path`, a path under `folder`, is after the folder moves to `destination`.
///
/// # Errors
/// Returns `InvalidInput` on `/destination` when that is not a workspace path (longer than a
/// path may be).
fn moved_to(
    path: &WorkspacePath,
    folder: &WorkspacePath,
    destination: &WorkspacePath,
) -> Result<WorkspacePath, ApiError> {
    let below = path
        .as_str()
        .get(folder.as_str().len()..)
        .unwrap_or_default();
    let moved = format!("{}{below}", destination.as_str());
    WorkspacePath::try_from(moved.clone()).map_err(|error| {
        invalid(
            format!(
                "{} would move to a path that is not a workspace path ({error}): {moved}",
                path.as_str()
            ),
            "/destination",
        )
    })
}

/// Refuse a reserved path on `field`.
fn refuse_reserved(path: &WorkspacePath, field: &str) -> Result<(), ApiError> {
    if reserved(path.as_str()) {
        return Err(invalid(
            format!(
                "{} is reserved: .okf, index.md and log.md (in any letter case, as any path \
                 segment) are kept by the server",
                path.as_str()
            ),
            field,
        ));
    }
    Ok(())
}

/// Record the write's receipt and answer it with the commit and the count of its items.
///
/// # Errors
/// Returns `Internal` for a count past `u32`, or any error of recording the receipt.
async fn finish(
    service: &ApplicationService,
    context: &OperationContext,
    scope: &StorageScope,
    committed: Committed,
    items: usize,
) -> Result<FolderResult, ApiError> {
    let items = u32::try_from(items).map_err(|_| {
        ApiError::new(
            ErrorCode::Internal,
            "the folder holds more items than a count holds",
        )
    })?;
    let receipt_id = record_receipt(
        service,
        context,
        scope,
        Some(mutation_id(context)?),
        Vec::new(),
    )
    .await?;
    Ok(FolderResult {
        revision: committed.revision,
        receipt_id,
        warnings: committed.warnings,
        items,
    })
}
