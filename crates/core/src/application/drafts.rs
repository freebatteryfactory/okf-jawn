//! The caller's own drafts: saving, listing and discarding them (SPEC section 8).
//!
//! A draft belongs to the authenticated subject; no request names its editor, so a caller only
//! ever saves, lists or discards its own. Saving never creates a revision and always succeeds
//! when the head has moved: the first save records the request's `base_revision`, the revision
//! the editor saw, as the draft's base, so a Snapshot is blocked when the item changed between
//! that revision and the head (SPEC section 8); a later save keeps the draft's base, and a save
//! whose `base_revision` is the current head rebases it (resolving a Snapshot conflict as
//! "keep mine"). A first save is refused when the item does not exist at its base. The base is
//! not checked to lie on the head's history: a revision of this workspace that holds the item
//! is recorded as given. A draft may leave the application header out
//! or echo it unchanged; any other value is refused before the draft is written.
//!
//! A draft never changes the text of a source card: its body is compared with the body the item
//! has at the draft's base as a read shows it (storage's reader folds `\r\n` to `\n` and drops
//! one final newline, see `as_a_read_shows`), so the body a client sends back from `get_item` is
//! never refused for those bytes alone. Whatever the request's body was, the draft of a source
//! card holds the item's own body at the draft's base, byte for byte, so a Snapshot never commits
//! the client's spelling. A source's text is changed by `correct_digest`. A View has no
//! draft until its layout and validation land (construction plan M1.6). `commit_items` applies
//! the same two rules (`refuse_unsupported_draft`) to drafts that predate them.

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{ItemId, Revision};
use okf_jawn_contract::item::{
    DiscardDraftRequest, Draft, DraftContent, ItemDocument, ItemKind, ListDraftsRequest,
    ListDraftsResponse, SaveDraftRequest,
};

use super::ApplicationService;
use super::shared::{check_named, invalid, mutation_id, workspace_scope};
use crate::context::OperationContext;
use crate::drafts::DraftWrite;
use crate::items::refuse_header_change;
use crate::portable::item_content_digest;
use crate::storage::StorageScope;

/// Save or replace the caller's draft of one item.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated base revision, `NotFound` when the item does
/// not exist at the head, `NotFound` on `/base_revision` when a first save names a revision at
/// which the item does not exist, `InvalidInput` on `/properties/okf_jawn` for a changed
/// application header, `NotImplemented` on `/item_id` for a View, `InvalidInput` on `/body` for a
/// source card whose body differs from the one at the draft's base, or any port error.
pub(super) async fn save_draft(
    service: &ApplicationService,
    context: &OperationContext,
    request: SaveDraftRequest,
) -> Result<Draft, ApiError> {
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    check_named(service, &scope, &request.base_revision).await?;
    let ports = service.ports();
    let editor = &context.principal.subject;
    let head = ports.versions.head(&scope).await?;
    let current = ports.versions.show(&scope, &head, request.item_id).await?;
    refuse_header_change(&current.properties, &request.properties, "/properties")?;
    // A View has no draft: refused before anything is read for it or written.
    if current.summary.kind == ItemKind::View {
        return Err(view_has_no_draft(request.item_id));
    }
    let existing = ports.drafts.get(&scope, request.item_id, editor).await?;
    let base_revision = match existing {
        // A later save keeps the draft's base; naming the head is the explicit rebase.
        Some(saved) if request.base_revision != head => saved.draft.base_revision,
        Some(_) => head.clone(),
        // A first save records the revision the editor saw, which may be behind the head.
        None => {
            if request.base_revision != head {
                require_item_at(service, &scope, &request.base_revision, request.item_id).await?;
            }
            request.base_revision
        }
    };
    let mut body = request.body;
    if current.summary.kind == ItemKind::Source {
        // The text the draft is measured against is the item's at the draft's base.
        let at_base = if base_revision == head {
            current.body.clone()
        } else {
            // The kept base is checked as the request's revision is: a purge may have
            // invalidated it since the draft was saved.
            check_named(service, &scope, &base_revision).await?;
            ports
                .versions
                .show(&scope, &base_revision, request.item_id)
                .await?
                .body
        };
        refuse_source_text_change(request.item_id, &at_base, &body)?;
        // The draft holds the item's own text, not the request's spelling of it.
        body = at_base;
    }
    // The digest of the drafted body and properties, computed as the item they would commit
    // is, so a draft and that item compare.
    let drafted = ItemDocument {
        body,
        properties: request.properties,
        ..current
    };
    let content_digest = item_content_digest(&drafted)?;
    ports
        .drafts
        .save(
            &scope,
            mutation,
            DraftWrite {
                item_id: request.item_id,
                editor: editor.clone(),
                base_revision,
                body: drafted.body,
                properties: drafted.properties,
                content_digest,
            },
        )
        .await
}

/// The refusal of a draft of a View.
///
/// `NotImplemented`, as `create_item` answers for a View: the request is well formed and the
/// operation exists, but this build has no validation for a View document yet (the contract's
/// rule: "declared but this build does not implement it"), so the caller's input is not what
/// is wrong and `InvalidInput` would tell it to change something it cannot. Not a success.
fn view_has_no_draft(item: ItemId) -> ApiError {
    ApiError::new(
        ErrorCode::NotImplemented,
        format!(
            "item {} is a View; a draft of a View is not implemented in this build, because \
             a saved View is validated before it is written",
            item.0
        ),
    )
    .with_field("/item_id")
}

/// Refuse a draft body that differs from the body a source card has at the draft's base.
///
/// Both texts are compared as a read shows them (`as_a_read_shows`). A card's text is generated
/// extraction or a recorded correction, and only `correct_digest` records the second (SPEC
/// section 5); a draft would replace the shown text with no record and the next re-extraction
/// would erase it. The refusal has two causes, so its message names both: the draft changed the
/// text, or the card's text changed since the draft's base.
fn refuse_source_text_change(item: ItemId, at_base: &str, drafted: &str) -> Result<(), ApiError> {
    if as_a_read_shows(at_base) == as_a_read_shows(drafted) {
        return Ok(());
    }
    Err(invalid(
        format!(
            "item {} is a source card; its text is changed by a correction \
             (correct_digest), not by a draft. If the draft did not change the text, the \
             card's text changed since the draft's base: take the current text",
            item.0
        ),
        "/body",
    ))
}

/// A body spelled the way storage's reader gives it back: line ends folded to `\n` and one final
/// newline dropped.
///
/// okf-core's `Document::parse` (0.2.7, `document.rs`) rebuilds a body from `text.lines()` joined
/// with `\n`, and `serialize` puts one final newline back, so two bodies that differ only in
/// those bytes are the same text to every read of the item. A lone `\r` is kept, as `lines`
/// keeps it.
fn as_a_read_shows(body: &str) -> String {
    let folded = body.replace("\r\n", "\n");
    match folded.strip_suffix('\n') {
        Some(without) => without.to_owned(),
        None => folded,
    }
}

/// The two draft rules for a draft that already exists, as a Snapshot is about to commit it:
/// a View has no draft, and a source card's draft keeps the body the card has at the draft's
/// base. Drafts saved before the rules (restored from an archive, for instance) meet them here.
///
/// # Errors
/// The refusals of `save_draft` (same code and field), or any port error reading the item at
/// the draft's base.
pub(super) async fn refuse_unsupported_draft(
    service: &ApplicationService,
    scope: &StorageScope,
    held: &DraftContent,
) -> Result<(), ApiError> {
    let item = held.draft.item_id;
    let at_base = service
        .ports()
        .versions
        .show(scope, &held.draft.base_revision, item)
        .await?;
    match at_base.summary.kind {
        ItemKind::Note => Ok(()),
        ItemKind::View => Err(view_has_no_draft(item)),
        ItemKind::Source => refuse_source_text_change(item, &at_base.body, &held.body),
    }
}

/// Refuse a first save whose base does not hold the item: the resource the draft is based on
/// is absent, which the contract's error rules name `NotFound` (the store's own answer for an
/// item a revision does not hold, and for a revision it does not hold).
async fn require_item_at(
    service: &ApplicationService,
    scope: &StorageScope,
    base: &Revision,
    item: ItemId,
) -> Result<(), ApiError> {
    match service.ports().versions.show(scope, base, item).await {
        Ok(_) => Ok(()),
        Err(error) if error.code == ErrorCode::NotFound => Err(ApiError::new(
            ErrorCode::NotFound,
            "the item does not exist at the revision this draft is based on",
        )
        .with_field("/base_revision")),
        Err(error) => Err(error),
    }
}

/// One page of the caller's drafts in the workspace, most recently saved first.
///
/// The store lists every draft of the editor; the page is cut here, and its cursor is the
/// decimal position of the next draft in that order.
///
/// # Errors
/// Returns `InvalidInput` on `/page/cursor` for a cursor this operation did not issue, or any
/// port error.
pub(super) async fn list_drafts(
    service: &ApplicationService,
    context: &OperationContext,
    request: ListDraftsRequest,
) -> Result<ListDraftsResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let offset = match request.page.cursor.as_deref() {
        None => 0,
        Some(cursor) => cursor
            .parse::<usize>()
            .map_err(|_| invalid("the cursor was not issued by list_drafts", "/page/cursor"))?,
    };
    let limit = usize::from(request.page.limit.max(1));
    let drafts = service
        .ports()
        .drafts
        .list(&scope, &context.principal.subject)
        .await?;
    let after = offset.saturating_add(limit);
    let next_cursor = (after < drafts.len()).then(|| after.to_string());
    Ok(ListDraftsResponse {
        items: drafts.into_iter().skip(offset).take(limit).collect(),
        next_cursor,
    })
}

/// Remove the caller's own draft of one item, without changing committed content.
///
/// # Errors
/// Returns `NotFound` when the caller has no draft of the item, or any port error.
pub(super) async fn discard_draft(
    service: &ApplicationService,
    context: &OperationContext,
    request: DiscardDraftRequest,
) -> Result<Draft, ApiError> {
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    service
        .ports()
        .drafts
        .discard(
            &scope,
            mutation,
            request.item_id,
            &context.principal.subject,
            None,
        )
        .await
}
