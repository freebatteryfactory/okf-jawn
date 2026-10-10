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
//! attempt may have written anywhere on the accepted line (`VersionStore::find_commit` with no
//! `since`), and completes it instead of committing again.
//!
//! A draft is removed only when it is still the content that was committed
//! (`DraftStore::discard` with the expected digest): on a first attempt the digest of the draft
//! read, and on a resumed attempt the digest of the item as the found commit holds it
//! (`portable::item_content_digest`). A draft saved again in between, before or after a crash,
//! stays in place. The draft store remembers what it removed under the mutation, so a resumed
//! attempt also completes the removals its earlier attempt made.
//!
//! An unrelated commit that lands between the Snapshot's check and its commit makes the store
//! answer `Conflict` for the moved head. The Snapshot then reads the head again and evaluates
//! the item-level rule against it; when no selected item changed, it commits on the new head.
//! It tries at most `HEAD_ATTEMPTS` heads, then answers `Conflict`.
//!
//! A Rewind restores the target's state in a new commit on its base and never rewrites history:
//! one item with every companion file it owns (`TreeEdit::RestoreItem`, so a source card's
//! appearance, supplied text and corrections come back with its card), or the whole tree
//! (`TreeEdit::RestoreWorkspace`). It is refused when the item's comparison from the base to
//! the target, which follows its moves, lists nothing. The store restores the item by identity,
//! so another item's file is never brought back, and restoring over another item's path fails
//! the commit with `Conflict`. The files that comparison shows at the target count as touched.
//!
//! Both writes commit through `shared::commit` with the production edit check: OKF conformance
//! refuses, and the validator's warnings and OKF lint about the files the write touches come
//! back as warnings.

use std::collections::BTreeSet;

use okf_jawn_contract::common::{MutationResult, TextRange};
use okf_jawn_contract::error::{ApiError, DraftConflictItem, ErrorCode, ErrorDetail};
use okf_jawn_contract::history::{
    BlameRequest, BlameResponse, CommitRequest, DiffRequest, DiffResponse, FileChangeKind,
    LogRequest, LogResponse, RestoreRequest,
};
use okf_jawn_contract::identity::{Digest, ItemId, MutationId, Revision, WorkspacePath};
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
use crate::portable::item_content_digest;
use crate::storage::{BlameQuery, Committed, DiffQuery, LogQuery, StorageScope, TreeEdit};

/// How many heads a Snapshot tries while unrelated commits keep landing before its commit.
const HEAD_ATTEMPTS: usize = 3;

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
/// listing every selected item changed or deleted since its draft's base; `Conflict` when the
/// head moved under every one of `HEAD_ATTEMPTS` attempts; or any error of the commit.
pub(super) async fn commit_items(
    service: &ApplicationService,
    context: &OperationContext,
    request: CommitRequest,
) -> Result<MutationResult, ApiError> {
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    refuse_repeats(&request.item_ids)?;
    let item = match request.item_ids.as_slice() {
        [only] => Some(*only),
        _ => None,
    };
    let ports = service.ports();
    if context.attempt == Attempt::Resumed
        && let Some(revision) = ports.versions.find_commit(&scope, mutation, None).await?
    {
        announce(service, scope.clone(), mutation, &revision, item).await?;
        // Each draft is removed only if it is still what the found commit holds.
        let mut committed = Vec::with_capacity(request.item_ids.len());
        for selected in &request.item_ids {
            let document = ports.versions.show(&scope, &revision, *selected).await?;
            committed.push((*selected, item_content_digest(&document)?));
        }
        return finish(
            service,
            context,
            &scope,
            mutation,
            committed,
            revision,
            Vec::new(),
        )
        .await;
    }
    let mut drafts = Vec::with_capacity(request.item_ids.len());
    for (index, selected) in request.item_ids.iter().enumerate() {
        drafts.push(held_draft(service, context, &scope, *selected, index).await?);
    }
    // Every draft base is a revision this Snapshot reads at.
    for drafted in &drafts {
        check_named(service, &scope, &drafted.draft.base_revision).await?;
    }
    let committed_drafts: Vec<(ItemId, Digest)> = drafts
        .iter()
        .map(|drafted| (drafted.draft.item_id, drafted.draft.content_digest.clone()))
        .collect();
    let committed = commit_on_a_head(
        service,
        context,
        &scope,
        &drafts,
        &request.message,
        item,
        &request.item_ids,
    )
    .await?;
    finish(
        service,
        context,
        &scope,
        mutation,
        committed_drafts,
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
/// when nothing differs between the base and the target; or any error of the commit, among
/// them the store's `Conflict` when restoring the item would write over another item's path.
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
        Some(item) => TreeEdit::RestoreItem {
            item_id: item,
            from: request.target.clone(),
        },
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

/// Commit the drafts on the head when no selected item changed since its draft's base.
///
/// When the store answers `Conflict` and the head has moved, an unrelated commit landed after
/// the check: the item-level rule is evaluated again against the new head, and the drafts are
/// committed there when it still allows them. At most `HEAD_ATTEMPTS` heads are tried.
async fn commit_on_a_head(
    service: &ApplicationService,
    context: &OperationContext,
    scope: &StorageScope,
    drafts: &[DraftContent],
    message: &str,
    item: Option<ItemId>,
    items: &[ItemId],
) -> Result<Committed, ApiError> {
    let versions = &service.ports().versions;
    let mut head = versions.head(scope).await?;
    for _ in 0..HEAD_ATTEMPTS {
        let mut conflicts = Vec::new();
        for drafted in drafts {
            if let Some(conflict) = conflict_of(service, scope, &drafted.draft, &head).await? {
                conflicts.push(conflict);
            }
        }
        if !conflicts.is_empty() {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                format!(
                    "{} of the selected items changed since their drafts were based; nothing \
                     was committed",
                    conflicts.len()
                ),
            )
            .with_detail(ErrorDetail::DraftConflict { items: conflicts }));
        }
        let written = commit(
            service,
            context,
            Write {
                base: head.clone(),
                message: message.to_owned(),
                edits: drafts
                    .iter()
                    .map(|drafted| TreeEdit::EditItem {
                        item_id: drafted.draft.item_id,
                        body: drafted.body.clone(),
                        properties: drafted.properties.clone(),
                    })
                    .collect(),
                item,
                touched: Touched {
                    items: items.to_vec(),
                    ..Touched::default()
                },
            },
        )
        .await;
        match written {
            Err(error) if error.code == ErrorCode::Conflict => {
                let now = versions.head(scope).await?;
                if now == head {
                    return Err(error);
                }
                head = now;
            }
            answer => return answer,
        }
    }
    Err(ApiError::new(
        ErrorCode::Conflict,
        format!(
            "the head moved under {HEAD_ATTEMPTS} attempts to commit this snapshot; nothing was \
             committed"
        ),
    ))
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

/// The caller's own draft of `item`, the `index`-th selected.
async fn held_draft(
    service: &ApplicationService,
    context: &OperationContext,
    scope: &StorageScope,
    item: ItemId,
    index: usize,
) -> Result<DraftContent, ApiError> {
    service
        .ports()
        .drafts
        .get(scope, item, &context.principal.subject)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::NotFound,
                "you hold no draft of this item to snapshot",
            )
            .with_field(format!("/item_ids/{index}"))
        })
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

/// Remove each snapshotted draft that is still the committed content (the item with the digest
/// of what was committed), and record the Snapshot's receipt; both are idempotent on the
/// mutation.
///
/// A draft saved again since that content was read (`Conflict`), or one the editor discarded
/// meanwhile (`NotFound`), is left as the editor left it: neither was committed by this
/// Snapshot.
async fn finish(
    service: &ApplicationService,
    context: &OperationContext,
    scope: &StorageScope,
    mutation: MutationId,
    committed: Vec<(ItemId, Digest)>,
    revision: Revision,
    warnings: Vec<okf_jawn_contract::common::Warning>,
) -> Result<MutationResult, ApiError> {
    for (item, digest) in committed {
        let removed = service
            .ports()
            .drafts
            .discard(
                scope,
                mutation,
                item,
                &context.principal.subject,
                Some(digest),
            )
            .await;
        match removed {
            Err(error) if !matches!(error.code, ErrorCode::Conflict | ErrorCode::NotFound) => {
                return Err(error);
            }
            _ => {}
        }
    }
    let receipt_id = record_receipt(service, context, scope, Some(mutation), Vec::new()).await?;
    Ok(MutationResult {
        revision,
        receipt_id,
        warnings,
    })
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
