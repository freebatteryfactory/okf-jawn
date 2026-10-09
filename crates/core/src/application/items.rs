//! Folders, items and type definitions: listing and reading committed content, and the
//! immediate commits SPEC section 8 names (create, move, archive or status, delete, a folder,
//! a type).
//!
//! Reads resolve their selector once (`shared::resolve`) and never see a draft, except that
//! `get_item` adds the caller's own draft, read for the authenticated subject and nobody else
//! (dispatch has already refused it on every route that is not a human session). Every
//! returned `ItemDocument` carries the server's content digest. Writes refuse a caller's
//! application header before any port is touched (`items::refuse_supplied_header`,
//! `items::refuse_header_in_type`) and commit through `shared::commit` with the edit check:
//! OKF conformance refuses, and OKF lint comes back as warnings.

use okf_jawn_contract::common::MutationResult;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::ExtractionFilter;
use okf_jawn_contract::identity::{At, WorkspacePath};
use okf_jawn_contract::item::{
    CreateFolderRequest, CreateItemRequest, DeleteItemRequest, GetItemRequest, ItemDocument,
    ItemKind, ItemSummary, ListItemsRequest, ListItemsResponse, ListTypesRequest,
    ListTypesResponse, MoveItemRequest, SetLifecycleRequest, SetTypeRequest,
};

use super::ApplicationService;
use super::shared::{
    Write, commit, commit_with_receipt, invalid, mutation_id, resolve, with_content_digest,
    workspace_scope,
};
use crate::context::OperationContext;
use crate::echo::bounded;
use crate::items::{refuse_header_in_type, refuse_supplied_header};
use crate::storage::{TreeEdit, derive_item_id};

/// List one folder at one resolved revision; with the `unprocessed` filter, only the sources
/// among its items whose conversion is partial, failed or unsupported.
///
/// The filter applies to the page the store returned, so a filtered page may hold fewer items
/// than asked; its continuation is the store's. Child folders are listed either way.
///
/// # Errors
/// Returns `InvalidInput` on `/folder` for a folder that is not a workspace path, the typed
/// `NotFound` of an invalidated revision, or any port error.
pub(super) async fn list_items(
    service: &ApplicationService,
    context: &OperationContext,
    request: ListItemsRequest,
) -> Result<ListItemsResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let folder = if request.folder.is_empty() {
        None
    } else {
        Some(WorkspacePath::try_from(request.folder).map_err(|error| invalid(error.0, "/folder"))?)
    };
    let revision = resolve(service, &scope, &request.at).await?;
    let listing = service
        .ports()
        .versions
        .list(&scope, &revision, folder.as_ref(), request.page.into())
        .await?;
    let items = match request.extraction {
        None => listing.items,
        Some(ExtractionFilter::Unprocessed) => {
            listing.items.into_iter().filter(is_unprocessed).collect()
        }
    };
    Ok(ListItemsResponse {
        revision,
        items,
        folders: listing
            .folders
            .iter()
            .map(|folder| folder.as_str().to_owned())
            .collect(),
        next_cursor: listing.next_cursor,
    })
}

/// Read an item's committed content at one resolved revision, with the caller's own draft.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated revision, or any port error.
pub(super) async fn get_item(
    service: &ApplicationService,
    context: &OperationContext,
    request: GetItemRequest,
) -> Result<ItemDocument, ApiError> {
    let scope = workspace_scope(context)?;
    let revision = resolve(service, &scope, &request.at).await?;
    let ports = service.ports();
    let mut document = with_content_digest(
        ports
            .versions
            .show(&scope, &revision, request.item_id)
            .await?,
    )?;
    document.draft = ports
        .drafts
        .get(&scope, request.item_id, &context.principal.subject)
        .await?;
    Ok(document)
}

/// Create a note at a new path in one commit and return it as committed.
///
/// A source card comes only from an upload or an import, so `kind: source` is refused. A View
/// is refused as not implemented until its file layout lands with `get_view` (construction plan
/// M1.6), because a saved View is validated before it is written.
///
/// # Errors
/// Returns `InvalidInput` on `/kind` for a source, `NotImplemented` for a View, `InvalidInput`
/// on `/properties/okf_jawn` for a supplied application header, or any error of the commit.
pub(super) async fn create_item(
    service: &ApplicationService,
    context: &OperationContext,
    request: CreateItemRequest,
) -> Result<ItemDocument, ApiError> {
    match request.kind {
        ItemKind::Note => {}
        ItemKind::Source => {
            return Err(invalid(
                "a source card is made only by an upload or an import",
                "/kind",
            ));
        }
        ItemKind::View => {
            return Err(ApiError::new(
                ErrorCode::NotImplemented,
                "creating a View is not implemented in this build",
            )
            .with_field("/kind"));
        }
    }
    refuse_supplied_header(&request.properties, "/properties")?;
    let item_id = derive_item_id(mutation_id(context)?, 0);
    let committed = commit(
        service,
        context,
        Write {
            base: request.base_revision,
            message: format!("Create {}", request.path.as_str()),
            edits: vec![TreeEdit::CreateItem {
                item_id,
                path: request.path,
                title: Some(request.title),
                type_name: request.type_name,
                kind: request.kind,
                body: request.body,
                properties: request.properties,
            }],
            item: Some(item_id),
            check: service.edit_check(),
        },
    )
    .await?;
    let scope = workspace_scope(context)?;
    with_content_digest(
        service
            .ports()
            .versions
            .show(&scope, &committed.revision, item_id)
            .await?,
    )
}

/// Move an item and rewrite the links that point at it, in one commit.
///
/// # Errors
/// Returns any error of the commit.
pub(super) async fn move_item(
    service: &ApplicationService,
    context: &OperationContext,
    request: MoveItemRequest,
) -> Result<MutationResult, ApiError> {
    commit_with_receipt(
        service,
        context,
        Write {
            base: request.base_revision,
            message: format!("Move to {}", request.destination.as_str()),
            edits: vec![TreeEdit::MoveItem {
                item_id: request.item_id,
                destination: request.destination,
            }],
            item: Some(request.item_id),
            check: service.edit_check(),
        },
    )
    .await
}

/// Change an item's OKF status word, its archived flag, or both, in one commit; neither is a
/// claim about the content (`check_rules` already refused an empty change and `other`).
///
/// # Errors
/// Returns any error of the commit.
pub(super) async fn set_lifecycle(
    service: &ApplicationService,
    context: &OperationContext,
    request: SetLifecycleRequest,
) -> Result<MutationResult, ApiError> {
    let message = match request.archived {
        Some(true) => "Archive",
        Some(false) => "Unarchive",
        None => "Change status",
    };
    commit_with_receipt(
        service,
        context,
        Write {
            base: request.base_revision,
            message: message.to_owned(),
            edits: vec![TreeEdit::SetStatus {
                item_id: request.item_id,
                status: request.status,
                archived: request.archived,
            }],
            item: Some(request.item_id),
            check: service.edit_check(),
        },
    )
    .await
}

/// Remove an item from the tree in one commit; its history and retained objects stay.
///
/// # Errors
/// Returns any error of the commit.
pub(super) async fn delete_item(
    service: &ApplicationService,
    context: &OperationContext,
    request: DeleteItemRequest,
) -> Result<MutationResult, ApiError> {
    commit_with_receipt(
        service,
        context,
        Write {
            base: request.base_revision,
            message: "Remove".to_owned(),
            edits: vec![TreeEdit::DeleteItem {
                item_id: request.item_id,
            }],
            item: Some(request.item_id),
            check: service.edit_check(),
        },
    )
    .await
}

/// Create a folder with its maintained index, in one commit.
///
/// # Errors
/// Returns any error of the commit.
pub(super) async fn create_folder(
    service: &ApplicationService,
    context: &OperationContext,
    request: CreateFolderRequest,
) -> Result<MutationResult, ApiError> {
    commit_with_receipt(
        service,
        context,
        Write {
            base: request.base_revision,
            message: format!("New folder {}", request.folder.as_str()),
            edits: vec![TreeEdit::CreateFolder {
                folder: request.folder,
            }],
            item: None,
            check: service.edit_check(),
        },
    )
    .await
}

/// The workspace's type definitions at the current head.
///
/// The application defines no type of its own: OKF leaves `type` to the producer, so every
/// definition listed is one the workspace saved with `set_type`.
///
/// # Errors
/// Returns any port error.
pub(super) async fn list_types(
    service: &ApplicationService,
    context: &OperationContext,
    _request: ListTypesRequest,
) -> Result<ListTypesResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let revision = resolve(service, &scope, &At::Latest).await?;
    Ok(ListTypesResponse {
        items: service.ports().versions.types(&scope, &revision).await?,
    })
}

/// Save a type definition in one commit, after refusing a schema that names the application
/// header or reaches outside itself, and a properties schema that is not a JSON Schema.
///
/// # Errors
/// Returns `InvalidInput` on the offending field of `/definition/properties_schema`, or any
/// error of the commit.
pub(super) async fn set_type(
    service: &ApplicationService,
    context: &OperationContext,
    request: SetTypeRequest,
) -> Result<MutationResult, ApiError> {
    refuse_header_in_type(&request.definition)?;
    jsonschema::validator_for(&request.definition.properties_schema).map_err(|error| {
        invalid(
            bounded(format!(
                "the properties schema is not a JSON Schema: {error}"
            )),
            "/definition/properties_schema",
        )
    })?;
    commit_with_receipt(
        service,
        context,
        Write {
            base: request.base_revision,
            message: format!("Save type {}", request.definition.name),
            edits: vec![TreeEdit::SetType {
                definition: request.definition,
            }],
            item: None,
            check: service.edit_check(),
        },
    )
    .await
}

/// Whether a listed item is a source whose conversion left part or all of it unextracted.
fn is_unprocessed(item: &ItemSummary) -> bool {
    item.extraction
        .as_ref()
        .is_some_and(|extraction| extraction.status.is_unprocessed())
}
