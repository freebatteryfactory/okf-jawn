//! History: Timeline (`log_items`), Changes (`diff_items`), Who (`blame_item`), Snapshot
//! (`commit_items`) and Rewind (`restore_items`), SPEC section 8.
//!
//! Every revision a request names is checked against the revision map a purge wrote before
//! anything is read at it (`shared::resolve`, `shared::check_named`): `at`, both ends of a
//! comparison, a Rewind's base and target, and the base of every draft a Snapshot commits. A
//! Timeline's later pages read at the tip its first page resolved, which its cursor carries
//! (`shared::resolve_page`), and that tip is checked again on every page.
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
//! `since`), and completes it instead of committing again, answering the warnings the store
//! kept when that commit was written (or the store's one warning that it kept none), which the
//! store gives with the commit it finds.
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
//! It tries at most `HEAD_ATTEMPTS` heads, then answers `Conflict`. Only the store's refusal of
//! the commit is retried: an error after the store committed (indexing, the event), a
//! `Conflict` included, fails the request and never leads to another commit attempt.
//!
//! A Rewind restores the target's state in a new commit on its base and never rewrites history:
//! one item with every companion file it owns (`TreeEdit::RestoreItem`, so a source card's
//! appearance, supplied text and corrections come back with its card), or the whole tree
//! (`TreeEdit::RestoreWorkspace`). It is refused when the item's comparison from the base to
//! the target, which follows its moves, lists nothing. The store restores the item by identity,
//! so another item's file is never brought back, and restoring over another item's path fails
//! the commit with `Conflict`. The files that comparison shows at the target count as touched.
//! The store moves an item the base still holds back with its links. An item deleted since has
//! no path at the base for its links to follow from, so its Rewind first puts it back at its
//! last path before the delete and moves it from there (`item_restore`, R-I14): the links that
//! pointed at that last path end pointing at the restored path. In both cases the store tells
//! the check which files it rewrote for those links, and they count as touched too.
//!
//! Both writes commit through `shared::commit` with the production edit check: OKF conformance
//! refuses, and the validator's warnings and OKF lint about the files the write touches come
//! back as warnings.

use std::collections::BTreeSet;

use okf_jawn_contract::common::{MutationResult, TextRange};
use okf_jawn_contract::error::{ApiError, DraftConflictItem, ErrorCode, ErrorDetail};
use okf_jawn_contract::history::{
    BlameRequest, BlameResponse, CommitRequest, DiffRequest, DiffResponse, FileChange, LogRequest,
    LogResponse, RestoreRequest,
};
use okf_jawn_contract::identity::{Digest, ItemId, MutationId, Revision, WorkspacePath};
use okf_jawn_contract::item::{Draft, DraftContent};
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::source::SourceReference;

use super::ApplicationService;
use super::drafts::refuse_unsupported_draft;
use super::shared::{
    Touched, Write, announce, check_named, commit_with_receipt, invalid, mutation_id, next_page,
    record_receipt, resolve, resolve_page, store_commit, workspace_scope,
};
use super::sources::{citation, record_of};
use crate::context::{Attempt, OperationContext};
use crate::portable::item_content_digest;
use crate::storage::{BlameQuery, Committed, DiffQuery, LogQuery, Page, StorageScope, TreeEdit};

/// Which side of a comparison a citation is of.
#[derive(Clone, Copy)]
enum Side {
    From,
    To,
}

/// What one side of a unified hunk header covers.
enum Hunk {
    /// The lines the side holds.
    Lines(TextRange),
    /// A count of 0: the side holds no line of this hunk.
    Empty,
    /// A header that does not parse.
    Unparsed,
}

/// How many heads a Snapshot tries while unrelated commits keep landing before its commit.
const HEAD_ATTEMPTS: usize = 3;
/// How many entries of a folder are read at a time when a Rewind asks whether a path is taken.
const FOLDER_PAGE: u16 = 500;

/// Content snapshots of the workspace or of one item (following its moves), newest first,
/// walking back from the resolved revision. The commits carry metadata and no item content, so
/// the receipt it records cites no source.
///
/// A continuation walks back from the tip its first page resolved (`shared::resolve_page`), so
/// a Snapshot that lands between two pages of a Timeline at `latest` repeats no commit.
///
/// # Errors
/// Returns `InvalidInput` on `/page/cursor` for a cursor that is not this operation's (see `shared::resolve_page`) or one
/// that continues a read of another revision than `at` names, the typed `NotFound` of an
/// invalidated revision, or any port error.
pub(super) async fn log_items(
    service: &ApplicationService,
    context: &OperationContext,
    request: LogRequest,
) -> Result<LogResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let (tip, page, _) = resolve_page(
        service,
        &scope,
        context.operation,
        &request.at,
        request.page,
    )
    .await?;
    let response = service
        .ports()
        .versions
        .log(
            &scope,
            LogQuery {
                tip: tip.clone(),
                item_id: request.item_id,
                page,
            },
        )
        .await?;
    // Commit metadata only: the receipt names no item content.
    record_receipt(service, context, &scope, None, Vec::new()).await?;
    Ok(LogResponse {
        next_cursor: next_page(context.operation, &tip, response.next_cursor, None)?,
        commits: response.commits,
    })
}

/// Compare two named revisions, optionally only one item (following its moves).
///
/// The comparison returns content, so a receipt records what it returned: for each side of each
/// change that holds an item, one citation of body lines per hunk of that side, or the side
/// whole when a hunk shows its frontmatter (`diff_citations`).
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
    let response = service
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
        .await?;
    let sources = diff_citations(service, &scope, &response).await?;
    record_receipt(service, context, &scope, None, sources).await?;
    Ok(response)
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
    let source = citation(
        &scope,
        &revision,
        &document,
        record.as_ref(),
        Selection::Lines { range },
    );
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
        && let Some(found) = ports.versions.find_commit(&scope, mutation, None).await?
    {
        let revision = found.revision;
        announce(service, scope.clone(), mutation, &revision, item).await?;
        // Each draft is removed only if it is still what the found commit holds.
        let mut committed = Vec::with_capacity(request.item_ids.len());
        for selected in &request.item_ids {
            let document = ports.versions.show(&scope, &revision, *selected).await?;
            committed.push((*selected, item_content_digest(&document)?));
        }
        // The warnings are the ones the store kept when the first attempt committed.
        return finish(
            service,
            context,
            &scope,
            mutation,
            committed,
            revision,
            found.warnings,
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
    // A draft that predates the rules of `save_draft` is refused before anything is committed.
    for drafted in &drafts {
        refuse_unsupported_draft(service, &scope, drafted).await?;
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
    // What the restore leaves in the tree: every file present at the target. Restoring one
    // item moves it back from its path at the base, as `MoveItem` moves it, so that path counts
    // too, as a move's old path does. The referrers whose links the move back rewrites (or
    // the `MoveItem` of a deleted item's Rewind, `item_restore`) are the store's to name: it
    // reports them to the check, which counts them as touched.
    let restored: Vec<WorkspacePath> = changes
        .iter()
        .flat_map(|change| {
            let moved_from = request.item_id.and(change.old_path.clone());
            [change.new_path.clone(), moved_from]
        })
        .flatten()
        .filter_map(|path| WorkspacePath::try_from(path).ok())
        .collect();
    let edits = match request.item_id {
        None => vec![TreeEdit::RestoreWorkspace {
            from: request.target.clone(),
        }],
        Some(item) => {
            item_restore(
                service,
                &scope,
                item,
                &request.base_revision,
                &request.target,
            )
            .await?
        }
    };
    commit_with_receipt(
        service,
        context,
        Write {
            base: request.base_revision,
            message: request.message,
            edits,
            item: request.item_id,
            touched: Touched {
                paths: restored,
                ..Touched::default()
            },
        },
    )
    .await
}

/// The edits that restore `item` to its state at `target` on top of `base`.
///
/// One `RestoreItem` is the whole restore for an item the base still holds: the store moves it
/// back from its path there with its links. An item the base does not hold (deleted since) has
/// no path there to be moved from, so the links that pointed at it still name its last path
/// before the delete. When that path is not the one `target` holds it at, and nothing holds it
/// now, the Rewind is three edits (R-I14):
///
/// 1. `RestoreItem` from the last revision that held the item (found as said below), which
///    puts it back where the links still point;
/// 2. `MoveItem` to its path at `target`, which rewrites every link that points at it, as a
///    move does;
/// 3. `RestoreItem` from `target`, which sets the item and its companions to their state there.
///
/// The move separates the two restores, so the store applies them in order and not as one
/// group. In every other case the links have nothing to follow, and the Rewind is the one
/// `RestoreItem`: the item's last path is its path at `target`, or another spelling of it (the
/// two have one collision key, `WorkspacePath::collision_key`, so the store holds one file for
/// both and refuses a move between them; the links then keep the spelling they have, which may
/// be the last path's and not the restored one's); another item, or a folder, took that last
/// path since (its links are that one's now); or the history of `base` never held the item.
///
/// The last holder is the first parent of the newest commit, at or before `base`, that changed
/// the item: `VersionStore::log` of the item from `base`, one commit. Its cost is the store's
/// walk, which does not end at that commit: it runs on to the item's second-newest change, or
/// to the root, and storage reads every item file of two trees at each commit it walks. The
/// review measured 34 s for 200 items and 301 commits in a debug build; it grows with items
/// times commits walked. Only a Rewind of an item `base` does not hold pays it.
///
/// # Errors
/// Returns any port error of reading the item, its history or the folder of its last path.
async fn item_restore(
    service: &ApplicationService,
    scope: &StorageScope,
    item: ItemId,
    base: &Revision,
    target: &Revision,
) -> Result<Vec<TreeEdit>, ApiError> {
    let restore = TreeEdit::RestoreItem {
        item_id: item,
        from: target.clone(),
    };
    let versions = &service.ports().versions;
    if path_at(service, scope, base, item).await?.is_some() {
        return Ok(vec![restore]);
    }
    let Some(destination) = path_at(service, scope, target, item).await? else {
        return Ok(vec![restore]);
    };
    // The newest commit at or before the base that changed the item removed it, since the base
    // does not hold it; that commit's first parent is the last revision that held it.
    let newest = versions
        .log(
            scope,
            LogQuery {
                tip: base.clone(),
                item_id: Some(item),
                page: Page {
                    cursor: None,
                    limit: 1,
                },
            },
        )
        .await?
        .commits
        .into_iter()
        .next();
    let Some(last) = newest.and_then(|commit| commit.parents.into_iter().next()) else {
        return Ok(vec![restore]);
    };
    let Some(last_path) = path_at(service, scope, &last, item).await? else {
        return Ok(vec![restore]);
    };
    // Compared as the store compares them: a move between two spellings of one path is refused.
    let same_path = last_path.collision_key() == destination.collision_key();
    if same_path || taken(service, scope, base, &last_path).await? {
        return Ok(vec![restore]);
    }
    Ok(vec![
        TreeEdit::RestoreItem {
            item_id: item,
            from: last,
        },
        TreeEdit::MoveItem {
            item_id: item,
            destination,
        },
        restore,
    ])
}

/// The path `revision` holds `item` at; `None` when it does not hold the item.
async fn path_at(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
    item: ItemId,
) -> Result<Option<WorkspacePath>, ApiError> {
    match service.ports().versions.show(scope, revision, item).await {
        Ok(document) => Ok(Some(document.summary.path)),
        Err(error) if error.code == ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Whether the folder `path` lies in, listed at `revision` by the spelling `path` gives it,
/// holds an item or a child folder whose path has `path`'s collision key
/// (`WorkspacePath::collision_key`), so a path differing only in letter case counts as taken.
/// Every page of the listing is read; the store lists a folder the revision does not hold as
/// empty. This is narrower than the store's own check of a destination, which compares every
/// path of the tree by key and also counts a file standing where a folder of the path is
/// needed: a taker this does not see makes the store refuse the three edits with `Conflict`.
///
/// # Errors
/// Returns any error of listing the folder.
async fn taken(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
    path: &WorkspacePath,
) -> Result<bool, ApiError> {
    let key = path.collision_key();
    let folder = match path.as_str().rsplit_once('/') {
        Some((folder, _)) => Some(
            WorkspacePath::try_from(folder.to_owned())
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.0))?,
        ),
        None => None,
    };
    let versions = &service.ports().versions;
    let mut cursor = None;
    loop {
        let page = Page {
            cursor: cursor.take(),
            limit: FOLDER_PAGE,
        };
        // Every error is the Rewind's: the store lists a folder the revision does not hold as
        // empty, so `NotFound` here would be an unknown revision, never "nothing there".
        let listing = versions
            .list(scope, revision, folder.as_ref(), page)
            .await?;
        let collides = listing
            .items
            .iter()
            .map(|item| &item.path)
            .chain(&listing.folders)
            .any(|held| held.collision_key() == key);
        if collides {
            return Ok(true);
        }
        match listing.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(false),
        }
    }
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
        // Only the store's refusal is retried: once it has committed, indexing and the event
        // follow once, and their errors (a `Conflict` included) fail the request, which a resumed
        // attempt completes through `find_commit` without a second commit.
        let written = store_commit(
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
            Err(error) => return Err(error),
            Ok(committed) => {
                announce(
                    service,
                    scope.clone(),
                    mutation_id(context)?,
                    &committed.revision,
                    item,
                )
                .await?;
                return Ok(committed);
            }
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
/// `None` when the item's comparison lists no change. The conflict is decided by that
/// comparison, `deleted` by the item's identity at the head.
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
    // Deleted means the item no longer exists at the head, whatever the comparison shows at its
    // path: a different item may have been created there.
    let deleted = match service
        .ports()
        .versions
        .show(scope, head, draft.item_id)
        .await
    {
        Ok(_) => false,
        Err(error) if error.code == ErrorCode::NotFound => true,
        Err(error) => return Err(error),
    };
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

/// The citations of what a comparison returned: for each change, the item at `old_path` at
/// `from` and the item at `new_path` at `to`, each cited once per hunk of its side by the body
/// lines the hunk covers (a patch numbers lines of the whole file, frontmatter included), or
/// once whole for a binary change, a side without a hunk, a hunk header that does not parse,
/// or a side with a hunk reaching into the frontmatter. A path that holds no item (a companion
/// file) cites nothing.
async fn diff_citations(
    service: &ApplicationService,
    scope: &StorageScope,
    response: &DiffResponse,
) -> Result<Vec<SourceReference>, ApiError> {
    let versions = &service.ports().versions;
    let mut sources = Vec::new();
    for change in &response.changes {
        for (side, revision, path) in [
            (Side::From, &response.from, change.old_path.as_ref()),
            (Side::To, &response.to, change.new_path.as_ref()),
        ] {
            let Some(path) = path.and_then(|path| WorkspacePath::try_from(path.clone()).ok())
            else {
                continue;
            };
            let Some(found) = versions.item_at_path(scope, revision, &path).await? else {
                continue;
            };
            let document = versions.show(scope, revision, found.id).await?;
            let start = versions.body_start(scope, revision, found.id).await?;
            let record = record_of(service, scope, document.source.as_ref()).await?;
            for selection in side_selections(change, side, start) {
                sources.push(citation(
                    scope,
                    revision,
                    &document,
                    record.as_ref(),
                    selection,
                ));
            }
        }
    }
    Ok(sources)
}

/// What one side of a change returned: the body lines of each hunk of that side, or
/// everything. `start` is the file line the side's body starts on (`VersionStore::body_start`):
/// a hunk's file lines from it on are body lines `line - start + 1`.
fn side_selections(change: &FileChange, side: Side, start: u32) -> Vec<Selection> {
    if change.binary {
        return vec![Selection::All];
    }
    let headers: Vec<&str> = change
        .patch
        .lines()
        .filter(|line| line.starts_with("@@"))
        .collect();
    if headers.is_empty() {
        return vec![Selection::All];
    }
    let mut selections = Vec::new();
    for header in headers {
        match hunk_side(header, side) {
            Hunk::Empty => {}
            // A hunk reaching before the body returned frontmatter, which no line selection
            // names: the side is cited whole. Context lines count, because they are returned.
            Hunk::Lines(range) => match body_lines(&range, start) {
                Some(range) => selections.push(Selection::Lines { range }),
                None => return vec![Selection::All],
            },
            Hunk::Unparsed => return vec![Selection::All],
        }
    }
    selections
}

/// The body lines of a hunk side's file lines `range`, the body starting on file line `start`;
/// `None` when the range reaches a line before the body.
fn body_lines(range: &TextRange, start: u32) -> Option<TextRange> {
    let body = |line: u32| line.checked_sub(start)?.checked_add(1);
    Some(TextRange {
        start: body(range.start)?,
        end: body(range.end)?,
    })
}

/// The lines one side of a unified hunk header `@@ -a,b +c,d @@` covers.
fn hunk_side(header: &str, side: Side) -> Hunk {
    let parsed = parse_side(header, side);
    match parsed {
        Some((_, 0)) => Hunk::Empty,
        Some((start, count)) => match start
            .checked_add(count)
            .and_then(|next| next.checked_sub(1))
        {
            Some(end) => Hunk::Lines(TextRange { start, end }),
            None => Hunk::Unparsed,
        },
        None => Hunk::Unparsed,
    }
}

/// `start` and `count` of one side of a header.
fn parse_side(header: &str, side: Side) -> Option<(u32, u32)> {
    let mut parts = header.split_whitespace();
    if parts.next()? != "@@" {
        return None;
    }
    let from = parts.next()?.strip_prefix('-')?;
    let to = parts.next()?.strip_prefix('+')?;
    span(match side {
        Side::From => from,
        Side::To => to,
    })
}

/// `start,count` of one side of a hunk header; a missing count is 1.
fn span(text: &str) -> Option<(u32, u32)> {
    match text.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((text.parse().ok()?, 1)),
    }
}
