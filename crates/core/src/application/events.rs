//! Receipts and the two event logs: what this server returned, and the resumable notifications
//! of a workspace and of the tenant.
//!
//! `get_receipt` reads one receipt of the workspace; a receipt of another workspace is
//! `NotFound`, whatever the store answered. Its stored citations are resolved through the
//! revision map a purge wrote, as every stored citation is when it is read (the gate
//! `application-operations`): a cited revision a purge rewrote is shown at its replacement (an
//! item purge keeps every other item's content, so a citation of another item follows the
//! rewritten revision); a cited revision a purge removed is shown as the invalidated citation
//! (its path `purged/<item id>`, its selection `all`, no digest or locations, the form storage's
//! purge scrubs a citation to) and the receipt names the purge in `invalidated_by`. A citation
//! the purge already scrubbed is shown as it is. Nothing is written back: the stored receipt
//! keeps what was returned then.
//!
//! `list_events` reads the workspace's log after a cursor, oldest first, a page at a time. The
//! read starts after the request's `page.cursor` when it has one (the continuation of an earlier
//! page) and after its `after` otherwise. A workspace's security events (`EventKind::is_security`)
//! are returned only to a caller holding `admin` on it (contract ruling 21), looked up from
//! `AccessControl` on every call whose page holds one, never kept between calls (SPEC section
//! 11: `admin` is always checked fresh). So a page may hold fewer events than asked, or none,
//! and still carries a cursor: the store's continuation, or, when the page's last event was
//! hidden from the caller, that event's cursor, so a reader resumes after what it was not shown
//! instead of reading it again. `stream_events` polls this operation.
//!
//! `list_tenant_events` reads the tenant's log the same way; dispatch has required the tenant's
//! `admin`, so nothing in it is hidden.

use std::collections::BTreeMap;

use okf_jawn_contract::access::Permission;
use okf_jawn_contract::common::PageRequest;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{
    GetReceiptRequest, ListEventsRequest, ListEventsResponse, ListTenantEventsRequest, Receipt,
};
use okf_jawn_contract::identity::{PurgeId, Revision, WorkspaceId, WorkspacePath};
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::source::SourceReference;

use super::ApplicationService;
use super::shared::{tenant_scope, workspace_scope};
use crate::access::authorize_workspace;
use crate::context::OperationContext;
use crate::events::{EventQuery, EventScope};
use crate::jobs::RevisionMapping;
use crate::storage::{Page, StorageScope};

/// The request field the store names when it refuses a cursor.
const PAGE_CURSOR: &str = "/page/cursor";

/// One receipt of the workspace, its citations resolved through the revision map.
///
/// # Errors
/// Returns `NotFound` for a receipt the workspace does not hold, or any port error.
pub(super) async fn get_receipt(
    service: &ApplicationService,
    context: &OperationContext,
    request: GetReceiptRequest,
) -> Result<Receipt, ApiError> {
    let scope = workspace_scope(context)?;
    let mut receipt = service
        .ports()
        .records
        .get_receipt(&scope, request.receipt_id)
        .await?;
    if receipt.id != request.receipt_id || receipt.workspace_id != scope.workspace_id {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "No receipt with that identity in this workspace",
        ));
    }
    resolve_citations(service, &scope, &mut receipt).await?;
    Ok(receipt)
}

/// The workspace's notifications after a cursor; security events only for its administrator.
///
/// # Errors
/// Returns `InvalidInput` on `/after` or `/page/cursor` for a cursor the store did not issue,
/// `Internal` when the access adapter answers a grant of another workspace or tenant, or any
/// port error.
pub(super) async fn list_events(
    service: &ApplicationService,
    context: &OperationContext,
    request: ListEventsRequest,
) -> Result<ListEventsResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let page = read_log(
        service,
        &EventScope::Workspace(scope.clone()),
        request.after,
        request.page,
    )
    .await?;
    if !page.events.iter().any(|event| event.kind.is_security()) {
        return Ok(page);
    }
    if is_admin(service, context, &scope).await? {
        return Ok(page);
    }
    let last = page.events.last().map(|event| event.id.clone());
    let events: Vec<_> = page
        .events
        .into_iter()
        .filter(|event| !event.kind.is_security())
        .collect();
    let last_shown = events.last().map(|event| event.id.clone());
    let next_cursor = page
        .next_cursor
        .or(if last_shown == last { None } else { last });
    Ok(ListEventsResponse {
        events,
        next_cursor,
    })
}

/// The tenant's notifications after a cursor.
///
/// # Errors
/// Returns `InvalidInput` on `/after` or `/page/cursor` for a cursor the store did not issue,
/// `Internal` when the request was authorized for no tenant, or any port error.
pub(super) async fn list_tenant_events(
    service: &ApplicationService,
    context: &OperationContext,
    request: ListTenantEventsRequest,
) -> Result<ListEventsResponse, ApiError> {
    let tenant = tenant_scope(context)?;

    read_log(
        service,
        &EventScope::Tenant(tenant),
        request.after,
        request.page,
    )
    .await
}

/// One page of `log` after the page's cursor, or after `after` without one.
///
/// # Errors
/// Returns the store's error; its refusal of a cursor names the request field that carried it.
async fn read_log(
    service: &ApplicationService,
    log: &EventScope,
    after: Option<String>,
    page: PageRequest,
) -> Result<ListEventsResponse, ApiError> {
    let continued = page.cursor.is_some();
    let query = EventQuery {
        after: page.cursor.clone().or(after),
        page: Page::from(page),
    };
    service
        .ports()
        .events
        .list(log, query)
        .await
        .map_err(|error| {
            // The store reads one cursor and names `/page/cursor`; without a page cursor the
            // one it read was the request's `after`.
            if !continued && error.field.as_deref() == Some(PAGE_CURSOR) {
                error.with_field("/after")
            } else {
                error
            }
        })
}

/// Whether the caller holds `admin` on the workspace now: its grant fetched from
/// `AccessControl` (which never answers `admin` from a cache), with its route rule and
/// delegation ceiling applied as dispatch applies them.
///
/// # Errors
/// Returns `Internal` when the adapter answers a grant of another workspace or tenant, or any
/// error of the adapter but its `Forbidden`, which means no grant.
async fn is_admin(
    service: &ApplicationService,
    context: &OperationContext,
    scope: &StorageScope,
) -> Result<bool, ApiError> {
    let principal = &context.principal;
    let raw = match service
        .ports()
        .access
        .authorize(principal, scope.workspace_id, Permission::Admin)
        .await
    {
        Ok(raw) => raw,
        Err(error) if error.code == ErrorCode::Forbidden => return Ok(false),
        Err(error) => return Err(error),
    };
    if raw.workspace_id() != scope.workspace_id || raw.scope.tenant_id != scope.tenant_id {
        return Err(ApiError::new(
            ErrorCode::Internal,
            "Access adapter returned a grant for a different scope",
        ));
    }
    Ok(authorize_workspace(principal, raw, Permission::Admin).is_ok())
}

/// Resolve every stored citation of `receipt` through the revision map of its workspace, each
/// (workspace, revision) looked up once.
///
/// # Errors
/// Returns any error of the record store.
async fn resolve_citations(
    service: &ApplicationService,
    scope: &StorageScope,
    receipt: &mut Receipt,
) -> Result<(), ApiError> {
    let mut mappings: BTreeMap<(WorkspaceId, Revision), Option<RevisionMapping>> = BTreeMap::new();
    let mut invalidated_by: Option<PurgeId> = None;
    for source in &mut receipt.sources {
        if is_scrubbed(source) {
            continue;
        }
        let key = (source.workspace_id, source.revision.clone());
        let mapping = if let Some(known) = mappings.get(&key) {
            known.clone()
        } else {
            let cited = StorageScope {
                tenant_id: scope.tenant_id.clone(),
                workspace_id: source.workspace_id,
            };
            let found = service
                .ports()
                .records
                .revision_mapping(&cited, &source.revision)
                .await?;
            mappings.insert(key, found.clone());
            found
        };
        match mapping {
            None => {}
            Some(RevisionMapping {
                replacement: Some(replacement),
                ..
            }) => source.revision = replacement,
            Some(RevisionMapping {
                purge_id,
                replacement: None,
            }) => {
                scrub(source)?;
                invalidated_by.get_or_insert(purge_id);
            }
        }
    }
    if let Some(purge) = invalidated_by {
        receipt.invalidated_by.get_or_insert(purge);
    }
    Ok(())
}

/// The path an invalidated citation of `source`'s item carries: `purged/<item id>`.
///
/// # Errors
/// Returns `Internal` when that path is not a workspace path, which an item id never makes.
fn purged_path(source: &SourceReference) -> Result<WorkspacePath, ApiError> {
    WorkspacePath::try_from(format!("purged/{}", source.item_id.0))
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.0))
}

/// Whether `source` is already the invalidated citation storage's purge scrubs it to.
fn is_scrubbed(source: &SourceReference) -> bool {
    purged_path(source).is_ok_and(|path| path == source.path)
}

/// Turn `source` into the invalidated citation (Stage 1b design section 6): its identities
/// stay, its path becomes `purged/<item id>`, which names nothing, its selection `all`, and the
/// digest and locations, which could carry the removed content's text, go.
///
/// # Errors
/// Returns `Internal` when the purged path does not parse.
fn scrub(source: &mut SourceReference) -> Result<(), ApiError> {
    source.path = purged_path(source)?;
    source.selection = Selection::All;
    source.digest = None;
    source.locations = Vec::new();
    Ok(())
}
