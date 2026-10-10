//! The caller's own drafts: saving, listing and discarding them (SPEC section 8).
//!
//! A draft belongs to the authenticated subject; no request names its editor, so a caller only
//! ever saves, lists or discards its own. Saving never creates a revision and always succeeds
//! when the head has moved: the first save takes the current head as its base, a later save
//! keeps the draft's base, and a save whose `base_revision` is the current head rebases it
//! (resolving a Snapshot conflict as "keep mine"). A draft may leave the application header out
//! or echo it unchanged; any other value is refused before the draft is written.

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::item::{
    DiscardDraftRequest, Draft, ItemDocument, ListDraftsRequest, ListDraftsResponse,
    SaveDraftRequest,
};

use super::ApplicationService;
use super::shared::{check_named, invalid, mutation_id, workspace_scope};
use crate::context::OperationContext;
use crate::drafts::DraftWrite;
use crate::items::refuse_header_change;
use crate::portable::item_content_digest;

/// Save or replace the caller's draft of one item.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated base revision, `NotFound` when the item does
/// not exist at the head, `InvalidInput` on `/properties/okf_jawn` for a changed application
/// header, or any port error.
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
    let existing = ports.drafts.get(&scope, request.item_id, editor).await?;
    let base_revision = match existing {
        Some(saved) if request.base_revision != head => saved.draft.base_revision,
        _ => head,
    };
    // The digest of the drafted body and properties, computed as the item they would commit
    // is, so a draft and that item compare.
    let drafted = ItemDocument {
        body: request.body,
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
        )
        .await
}
