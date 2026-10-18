//! Persistent Views (SPEC section 10; core M1.6 and M1b): reading a saved View (`get_view`) and
//! the approved component catalog (`get_catalog`).
//!
//! A View is an item of kind `view` whose file `views::layout` reads; it names no workspace, so
//! its bindings bind to the workspace that holds it. `get_view` reads it at one resolved
//! revision and fills each binding's locations (`views::resolve::cite`); it does not run
//! `views::validate`, so a person can open a View that no longer validates (a later catalog, an
//! archive restored from an older build) and fix it. A stored file this build cannot read is
//! `unsupported` on `/item_id`, naming where (`stored_fault`); an item that is no View is
//! `invalid_input` there. `get_catalog` answers the generated catalog as it is; it grants nothing.

use std::collections::BTreeMap;
use std::future::Ready;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{ItemId, Revision};
use okf_jawn_contract::item::{ItemDocument, ItemKind};
use okf_jawn_contract::views::{CatalogResponse, GetCatalogRequest, GetViewRequest, ViewDocument};
use serde_json::Value;

use super::ApplicationService;
use super::shared::{invalid, resolve, workspace_scope};
use crate::context::OperationContext;
use crate::reading::check_citation;
use crate::storage::StorageScope;
use crate::views::layout::{view_from, view_of};
use crate::views::resolve::{ViewPorts, cite};
use crate::views::schemas;
use crate::views::validate::validate;

/// Where a saved-View write reports a fault (`check_view_write`).
#[derive(Debug, Clone, Copy)]
pub(super) struct ViewFields<'a> {
    /// The request field of the View's title (`/title`, `/properties/title`).
    pub(super) title: &'a str,
    /// The request field of the View's body (`/body`, `/changes/0/body`).
    pub(super) body: &'a str,
    /// The item a Snapshot names the draft by, named in the message.
    pub(super) item: Option<ItemId>,
}

/// Read a saved View at one resolved revision, each binding's locations filled.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated revision, `NotFound` for an item the revision
/// does not hold, `InvalidInput` on `/item_id` for an item that is no View, `Unsupported` there
/// for a file this build does not read as a View, or any port error.
pub(super) async fn get_view(
    service: &ApplicationService,
    context: &OperationContext,
    request: GetViewRequest,
) -> Result<ViewDocument, ApiError> {
    let scope = workspace_scope(context)?;
    let revision = resolve(service, &scope, &request.at).await?;
    let (mut view, _) = stored_view(service, &scope, &revision, request.item_id).await?;
    cite(view_ports(service), &scope, &mut view).await?;
    Ok(view)
}

/// The approved component and action catalog, as the generator wrote it. It reads no port, so
/// its answer is ready at once.
///
/// # Errors
/// Returns `Internal` when the embedded catalog does not decode.
pub(super) fn get_catalog(
    _service: &ApplicationService,
    _context: &OperationContext,
    _request: GetCatalogRequest,
) -> Ready<Result<CatalogResponse, ApiError>> {
    std::future::ready(schemas::catalog().cloned())
}

/// The View an item holds at `revision`, in the workspace of `scope`, with its document.
///
/// # Errors
/// As `get_view`.
pub(super) async fn stored_view(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
    item: ItemId,
) -> Result<(ViewDocument, ItemDocument), ApiError> {
    let document = service.ports().versions.show(scope, revision, item).await?;
    let kind = match document.summary.kind {
        ItemKind::View => {
            let view = view_of(&document, scope.workspace_id).map_err(stored_fault)?;
            return Ok((view, document));
        }
        ItemKind::Note => "a note",
        ItemKind::Source => "a source card",
    };
    Err(invalid(
        format!("item {} is {kind}, not a View", item.0),
        "/item_id",
    ))
}

/// A stored View this build cannot read or present: `Unsupported` on `/item_id`, the message
/// naming where in the View document the fault lies.
pub(super) fn stored_fault(error: ApiError) -> ApiError {
    let ApiError { message, field, .. } = error;
    let at = field.map_or_else(String::new, |field| format!("{field}: "));
    ApiError::new(
        ErrorCode::Unsupported,
        format!("the stored View is not a View document this build reads: {at}{message}"),
    )
    .with_field("/item_id")
}

/// Check the View a write would store, before anything is written (core M1.6 design section
/// 3.6): the file reads as a View (`views::layout`), `views::validate` passes with every chart
/// drawable (ruling E7), and every binding's citation checks as a comment's does
/// (`reading::check_citation`). `properties` are the ones the write stores, `type` and `title`
/// included as the store writes them.
///
/// # Errors
/// Returns the refusal at its request field (`written_fault`): `InvalidInput` for a file that
/// is not a valid View or a citation whose path or locations are wrong, the typed `NotFound` of
/// an invalidated cited revision, `NotFound` for a cited item or digest the revision does not
/// hold; `Internal` when an embedded schema does not compile; or any port error.
pub(super) async fn check_view_write(
    service: &ApplicationService,
    scope: &StorageScope,
    properties: &BTreeMap<String, Value>,
    body: &str,
    fields: ViewFields<'_>,
) -> Result<(), ApiError> {
    let into_field = |error: ApiError| written_fault(error, fields);
    let view = view_from(properties, body, scope.workspace_id).map_err(into_field)?;
    validate(&view, scope.workspace_id)
        .and_then(|checked| checked.require_drawable())
        .map_err(into_field)?;
    let ports = service.ports();
    for (index, binding) in view.bindings.into_iter().enumerate() {
        let at = format!("/bindings/{index}/source");
        check_citation(
            ports.versions.as_ref(),
            ports.blobs.as_ref(),
            ports.records.as_ref(),
            scope,
            binding.source,
            &at,
        )
        .await
        .map_err(|error| {
            // A refusal of the cited revision or item names no field of its own.
            let located = if error.field.is_some() {
                error
            } else {
                error.with_field(at.as_str())
            };
            into_field(located)
        })?;
    }
    Ok(())
}

/// A fault of a View a write would store, at its request field: the title's, or the body's
/// with the pointer into the View document leading the message; for a Snapshot, which names
/// the draft by its item, the item leads it. Its code and detail are kept, so an invalidated
/// citation is still the typed `NotFound`.
fn written_fault(error: ApiError, fields: ViewFields<'_>) -> ApiError {
    let title = error.field.as_deref() == Some("/title");
    let at = match error.field.as_deref() {
        Some(pointer) if !title => format!("{pointer}: "),
        _ => String::new(),
    };
    let about = fields.item.map_or_else(String::new, |item| {
        format!("the draft of item {} is not a valid View: ", item.0)
    });
    ApiError {
        message: format!("{about}{at}{}", error.message),
        field: Some(if title { fields.title } else { fields.body }.to_owned()),
        ..error
    }
}

/// The properties a created item is stored with: the request's, with `type` and, when the
/// request names one, `title` set as the store sets them (`TreeEdit::CreateItem`).
pub(super) fn created_properties(
    properties: &BTreeMap<String, Value>,
    type_name: &str,
    title: Option<&str>,
) -> BTreeMap<String, Value> {
    let mut stored = properties.clone();
    stored.insert("type".to_owned(), Value::String(type_name.to_owned()));
    if let Some(title) = title {
        stored.insert("title".to_owned(), Value::String(title.to_owned()));
    }
    stored
}

/// The ports a View is read and resolved through.
pub(super) fn view_ports(service: &ApplicationService) -> ViewPorts<'_> {
    let ports = service.ports();
    ViewPorts {
        versions: ports.versions.as_ref(),
        blobs: ports.blobs.as_ref(),
        records: ports.records.as_ref(),
    }
}
