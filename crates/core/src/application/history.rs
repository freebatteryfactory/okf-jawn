//! History: Timeline (`log_items`), Changes (`diff_items`), Who (`blame_item`), Snapshot
//! (`commit_items`) and Rewind (`restore_items`), SPEC section 8.
//!
//! Every revision a request names is checked against the revision map a purge wrote before
//! anything is read at it (`shared::resolve`, `shared::check_named`): `at`, both ends of a
//! comparison, a Rewind's base and target, and the base of every draft a Snapshot commits.
//!
//! A Snapshot commits the caller's own drafts of the selected items in one commit, on the head
//! it reads. It is blocked only when an item being snapshotted was itself changed or deleted
//! since its own draft's base: the item's comparison from that base to the head
//! (`VersionStore::diff` limited to the item) lists a change. Head movement that did not touch a
//! selected item never blocks. When blocked, nothing is committed and the `draft_conflict` detail
//! lists every conflicting item with its draft base, the head and that comparison. On success
//! the snapshotted drafts are removed. A Snapshot reads the current head, so its expected head is
//! not stable across attempts: a resumed attempt first asks the store for the commit its earlier
//! attempt may have written (`VersionStore::find_commit`, from a draft's base), and completes it
//! instead of committing again. Drafts are removed only after the commit, and the draft store
//! remembers what it removed under the mutation, so a resumed attempt also recovers a draft its
//! earlier attempt already removed.
//!
//! A Rewind restores the target's state in a new commit on its base and never rewrites history:
//! one item's files (`TreeEdit::RestorePaths`) or the whole tree (`TreeEdit::RestoreWorkspace`).
//! The files it restores are those the comparison from the base to the target lists, which
//! follows an item's moves; a Rewind of one item never writes over, or brings back, another
//! item's file.
//!
//! Both writes commit through `shared::commit` with the production edit check: OKF conformance
//! refuses, and the validator's warnings and OKF lint about the files the write touches come
//! back as warnings.

use std::collections::BTreeSet;

use okf_jawn_contract::common::{MutationResult, TextRange};
use okf_jawn_contract::error::{ApiError, DraftConflictItem, ErrorCode, ErrorDetail};
use okf_jawn_contract::history::{
    BlameRequest, BlameResponse, CommitRequest, DiffRequest, DiffResponse, FileChange,
    FileChangeKind, LogRequest, LogResponse, RestoreRequest,
};
use okf_jawn_contract::identity::{ItemId, MutationId, Revision, WorkspacePath};
use okf_jawn_contract::item::{Draft, DraftContent, ItemDocument};
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::source::SourceReference;

use super::ApplicationService;
use super::shared::{
    Touched, Write, announce, check_named, commit, commit_with_receipt, invalid, mutation_id,
    record_receipt, resolve, workspace_scope,
};
use super::sources::{computed_locations, record_of};
use crate::context::{Attempt, OperationContext};
use crate::conversion::ConversionRecord;
use crate::storage::{BlameQuery, DiffQuery, LogQuery, StorageScope, TreeEdit};

/// One draft a Snapshot selected.
enum Selected {
    /// The caller's draft, still held.
    Held(DraftContent),
    /// The caller's draft an earlier attempt of this Snapshot already removed, as the draft
    /// store remembers it under the mutation.
    Snapshotted(Draft),
}

impl Selected {
    /// The draft's metadata.
    const fn draft(&self) -> &Draft {
        match self {
            Self::Held(content) => &content.draft,
            Self::Snapshotted(draft) => draft,
        }
    }
}

/// Content snapshots of the workspace or of one item (following its moves), newest first,
/// walking back from the resolved revision.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated revision, or any port error.
pub(super) async fn log_items(
    service: &ApplicationService,
    context: &OperationContext,
    request: LogRequest,
) -> Result<LogResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let tip = resolve(service, &scope, &request.at).await?;
    service
        .ports()
        .versions
        .log(
            &scope,
            LogQuery {
                tip,
                item_id: request.item_id,
                page: request.page.into(),
            },
        )
        .await
}

/// Compare two named revisions, optionally only one item (following its moves).
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated revision at either end, or any port error.
pub(super) async fn diff_items(
    service: &ApplicationService,
    context: &OperationContext,
    request: DiffRequest,
) -> Result<DiffResponse, ApiError> {
    let scope = workspace_scope(context)?;
    check_named(service, &scope, &request.from).await?;
    check_named(service, &scope, &request.to).await?;
    service
        .ports()
        .versions
        .diff(
            &scope,
            DiffQuery {
                from: request.from,
                to: request.to,
                item_id: request.item_id,
            },
        )
        .await
}

/// The commit that last changed each selected line of an item at the resolved revision.
///
/// The citation is the server's: the lines attributed, of the item's shown text, located in the
/// original as a read of those lines locates them (`sources::computed_locations`). The lines
/// returned are recorded in a receipt, as every read of content is.
///
/// # Errors
/// Returns `InvalidInput` on `/lines/end` for a range that ends before it starts, the typed
/// `NotFound` of an invalidated revision, the store's refusal of lines outside the body, or any
/// port error.
pub(super) async fn blame_item(
    service: &ApplicationService,
    context: &OperationContext,
    request: BlameRequest,
) -> Result<BlameResponse, ApiError> {
    if request.lines.end < request.lines.start {
        return Err(invalid(
            "the last line comes before the first",
            "/lines/end",
        ));
    }
    let scope = workspace_scope(context)?;
    let revision = resolve(service, &scope, &request.at).await?;
    let ports = service.ports();
    let attributed = ports
        .versions
        .blame(
            &scope,
            BlameQuery {
                revision: revision.clone(),
                item_id: request.item_id,
                lines: request.lines.clone(),
            },
        )
        .await?;
    let document = ports
        .versions
        .show(&scope, &revision, request.item_id)
        .await?;
    let record = record_of(service, &scope, document.source.as_ref()).await?;
    let first = attributed.lines.iter().map(|line| line.line).min();
    let last = attributed.lines.iter().map(|line| line.line).max();
    let range = match (first, last) {
        (Some(start), Some(end)) => TextRange { start, end },
        _ => request.lines,
    };
    let source = lines_citation(&scope, &revision, &document, record.as_ref(), range);
    record_receipt(service, context, &scope, None, vec![source.clone()]).await?;
    Ok(BlameResponse {
        source,
        lines: attributed.lines,
    })
}

/// Snapshot the caller's drafts of the selected items in one commit, as the module header says.
///
/// # Errors
/// Returns `InvalidInput` on `/item_ids` for an empty selection or on `/item_ids/<n>` for an
/// item listed twice; `NotFound` on `/item_ids/<n>` when the caller has no draft of that item;
/// the typed `NotFound` of an invalidated draft base; `Conflict` with the `draft_conflict` detail
/// listing every selected item changed or deleted since its draft's base; or any error of the
/// commit.
pub(super) async fn commit_items(
    service: &ApplicationService,
    context: &OperationContext,
    request: CommitRequest,
) -> Result<MutationResult, ApiError> {
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    refuse_repeats(&request.item_ids)?;
    let mut selected = Vec::with_capacity(request.item_ids.len());
    for (index, item) in request.item_ids.iter().enumerate() {
        selected.push(select_draft(service, context, &scope, mutation, *item, index).await?);
    }
    // Every draft base is a revision this Snapshot reads at.
    for one in &selected {
        check_named(service, &scope, &one.draft().base_revision).await?;
    }
    let item = match request.item_ids.as_slice() {
        [only] => Some(*only),
        _ => None,
    };
    let ports = service.ports();
    if context.attempt == Attempt::Resumed
        && let Some(first) = selected.first()
        && let Some(revision) = ports
            .versions
            .find_commit(&scope, mutation, &first.draft().base_revision)
            .await?
    {
        announce(service, scope.clone(), mutation, &revision, item).await?;
        return finish(
            service,
            context,
            &scope,
            mutation,
            &request.item_ids,
            revision,
            Vec::new(),
        )
        .await;
    }
    let mut drafts = Vec::with_capacity(selected.len());
    for one in selected {
        match one {
            Selected::Held(drafted) => drafts.push(drafted),
            Selected::Snapshotted(_) => {
                return Err(ApiError::new(
                    ErrorCode::Internal,
                    "an earlier attempt removed a draft of this snapshot but wrote no commit",
                ));
            }
        }
    }
    let head = ports.versions.head(&scope).await?;
    let mut conflicts = Vec::new();
    for drafted in &drafts {
        if let Some(conflict) = conflict_of(service, &scope, &drafted.draft, &head).await? {
            conflicts.push(conflict);
        }
    }
    if !conflicts.is_empty() {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            format!(
                "{} of the selected items changed since their drafts were based; nothing was \
                 committed",
                conflicts.len()
            ),
        )
        .with_detail(ErrorDetail::DraftConflict { items: conflicts }));
    }
    let committed = commit(
        service,
        context,
        Write {
            base: head,
            message: request.message,
            edits: drafts
                .into_iter()
                .map(|drafted| TreeEdit::EditItem {
                    item_id: drafted.draft.item_id,
                    body: drafted.body,
                    properties: drafted.properties,
                })
                .collect(),
            item,
            touched: Touched {
                items: request.item_ids.clone(),
                ..Touched::default()
            },
        },
    )
    .await?;
    finish(
        service,
        context,
        &scope,
        mutation,
        &request.item_ids,
        committed.revision,
        committed.warnings,
    )
    .await
}

/// Restore the target's state of one item or of the whole workspace in a new commit on the
/// base, as the module header says.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated base or target; `InvalidInput` on `/target`
/// when nothing differs between the base and the target; `Conflict` on `/item_id` when
/// restoring the item would write over, or bring back, another item's file; or any error of the
/// commit.
pub(super) async fn restore_items(
    service: &ApplicationService,
    context: &OperationContext,
    request: RestoreRequest,
) -> Result<MutationResult, ApiError> {
    let scope = workspace_scope(context)?;
    check_named(service, &scope, &request.base_revision).await?;
    check_named(service, &scope, &request.target).await?;
    let changes = service
        .ports()
        .versions
        .diff(
            &scope,
            DiffQuery {
                from: request.base_revision.clone(),
                to: request.target.clone(),
                item_id: request.item_id,
            },
        )
        .await?
        .changes;
    if changes.is_empty() {
        return Err(invalid(
            "nothing differs between the base and the target; there is nothing to restore",
            "/target",
        ));
    }
    // What the restore leaves in the tree: every file present at the target.
    let restored: Vec<WorkspacePath> = changes
        .iter()
        .filter_map(|change| change.new_path.clone())
        .filter_map(|path| WorkspacePath::try_from(path).ok())
        .collect();
    let edit = match request.item_id {
        None => TreeEdit::RestoreWorkspace {
            from: request.target.clone(),
        },
        Some(item) => {
            let paths = item_paths(&changes)?;
            refuse_other_items(service, &scope, &request, item, &paths).await?;
            TreeEdit::RestorePaths {
                from: request.target.clone(),
                paths,
            }
        }
    };
    commit_with_receipt(
        service,
        context,
        Write {
            base: request.base_revision,
            message: request.message,
            edits: vec![edit],
            item: request.item_id,
            touched: Touched {
                paths: restored,
                ..Touched::default()
            },
        },
    )
    .await
}

/// Refuse an empty selection and an item listed twice.
fn refuse_repeats(items: &[ItemId]) -> Result<(), ApiError> {
    if items.is_empty() {
        return Err(invalid("select at least one item to snapshot", "/item_ids"));
    }
    let mut seen = BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        if !seen.insert(item.0) {
            return Err(invalid(
                "an item is selected twice",
                &format!("/item_ids/{index}"),
            ));
        }
    }
    Ok(())
}

/// The caller's own draft of `item`; on a resumed attempt, the one its earlier attempt
/// removed, as the draft store remembers it under the mutation (`DraftStore::discard` removes
/// nothing then: the editor holds no draft of the item).
async fn select_draft(
    service: &ApplicationService,
    context: &OperationContext,
    scope: &StorageScope,
    mutation: MutationId,
    item: ItemId,
    index: usize,
) -> Result<Selected, ApiError> {
    let drafts = &service.ports().drafts;
    let editor = context.principal.subject.as_str();
    if let Some(content) = drafts.get(scope, item, editor).await? {
        return Ok(Selected::Held(content));
    }
    let missing = || {
        ApiError::new(
            ErrorCode::NotFound,
            "you hold no draft of this item to snapshot",
        )
        .with_field(format!("/item_ids/{index}"))
    };
    if context.attempt != Attempt::Resumed {
        return Err(missing());
    }
    match drafts.discard(scope, mutation, item, editor).await {
        Ok(draft) => Ok(Selected::Snapshotted(draft)),
        Err(error) if error.code == ErrorCode::NotFound => Err(missing()),
        Err(error) => Err(error),
    }
}

/// The conflict of one draft whose item changed or was deleted between its base and the head;
/// `None` when the item's comparison lists no change.
async fn conflict_of(
    service: &ApplicationService,
    scope: &StorageScope,
    draft: &Draft,
    head: &Revision,
) -> Result<Option<DraftConflictItem>, ApiError> {
    if &draft.base_revision == head {
        return Ok(None);
    }
    let changes = service
        .ports()
        .versions
        .diff(
            scope,
            DiffQuery {
                from: draft.base_revision.clone(),
                to: head.clone(),
                item_id: Some(draft.item_id),
            },
        )
        .await?
        .changes;
    if changes.is_empty() {
        return Ok(None);
    }
    // Nothing of the item is present at the head (`FileChangeKind::Removed`).
    let deleted = changes
        .iter()
        .all(|change| change.kind == FileChangeKind::Removed);
    Ok(Some(DraftConflictItem {
        item_id: draft.item_id,
        draft_base: draft.base_revision.clone(),
        current_revision: head.clone(),
        deleted,
        changes,
    }))
}

/// Remove the snapshotted drafts and record the Snapshot's receipt; both are idempotent on the
/// mutation.
async fn finish(
    service: &ApplicationService,
    context: &OperationContext,
    scope: &StorageScope,
    mutation: MutationId,
    items: &[ItemId],
    revision: Revision,
    warnings: Vec<okf_jawn_contract::common::Warning>,
) -> Result<MutationResult, ApiError> {
    for item in items {
        service
            .ports()
            .drafts
            .discard(scope, mutation, *item, &context.principal.subject)
            .await?;
    }
    let receipt_id = record_receipt(service, context, scope, Some(mutation), Vec::new()).await?;
    Ok(MutationResult {
        revision,
        receipt_id,
        warnings,
    })
}

/// Every file of one item the comparison names, at the base or at the target, once each.
fn item_paths(changes: &[FileChange]) -> Result<Vec<WorkspacePath>, ApiError> {
    let mut paths: Vec<WorkspacePath> = Vec::new();
    for path in changes
        .iter()
        .flat_map(|change| [change.old_path.as_ref(), change.new_path.as_ref()])
        .flatten()
    {
        let path = WorkspacePath::try_from(path.clone()).map_err(|error| {
            ApiError::new(
                ErrorCode::Internal,
                format!(
                    "the store compared a file that is not a workspace path: {}",
                    error.0
                ),
            )
        })?;
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    Ok(paths)
}

/// Refuse a Rewind of one item that would write over another item's file at the base, or bring
/// one back from the target.
async fn refuse_other_items(
    service: &ApplicationService,
    scope: &StorageScope,
    request: &RestoreRequest,
    item: ItemId,
    paths: &[WorkspacePath],
) -> Result<(), ApiError> {
    let versions = &service.ports().versions;
    for path in paths {
        for revision in [&request.base_revision, &request.target] {
            if let Some(other) = versions.item_at_path(scope, revision, path).await?
                && other.id != item
            {
                return Err(ApiError::new(
                    ErrorCode::Conflict,
                    format!(
                        "restoring this item would also restore {}, which holds another item",
                        path.as_str()
                    ),
                )
                .with_field("/item_id"));
            }
        }
    }
    Ok(())
}

/// The server's citation of `range` of an item's shown text at one revision: a source card's
/// lines are cited by its conversion record's digest and located in the original.
fn lines_citation(
    scope: &StorageScope,
    revision: &Revision,
    document: &ItemDocument,
    record: Option<&ConversionRecord>,
    range: TextRange,
) -> SourceReference {
    let digest = document
        .source
        .as_ref()
        .and_then(|source| source.extraction.digest.clone());
    let selection = Selection::Lines { range };
    let locations = computed_locations(
        document.source.as_ref(),
        record,
        digest.as_ref(),
        &selection,
    );
    SourceReference {
        workspace_id: scope.workspace_id,
        item_id: document.summary.id,
        path: document.summary.path.clone(),
        revision: revision.clone(),
        digest,
        selection,
        locations,
    }
}
