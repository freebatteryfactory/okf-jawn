//! The byte transfers (`transfers::Transfers`): an upload slot's bytes, an object of an exact
//! item revision, a workspace artifact and an installation archive.
//!
//! A transfer is not an operation and does not pass through dispatch, so it authorizes itself as
//! its caller by dispatch's rules for a target: the route (`access::check_route`), then the
//! adapter's raw grant, used only when it is for the workspace and tenant asked (`internal`
//! otherwise), then the delegation ceiling and the permission (`access::authorize_workspace`). A
//! route the rule refuses is refused before any grant is looked up.
//!
//! - `upload_content` needs `write` on the workspace, so a connector's route is refused. After
//!   authorization it asks the catalog whether the workspace is archived, as dispatch asks for
//!   `create_upload` and `complete_upload`, and refuses an archived one as they are refused:
//!   `conflict` with `ErrorDetail::WorkspaceArchived`. The body goes to
//!   `UploadStore::put_content` with the deployment's `ApplicationConfig::max_upload_bytes` as its
//!   limit; the store cuts a body past that limit or the slot's announced size while it streams
//!   (`too_large`, nothing appended) and refuses a complete slot (`conflict`). The digest is
//!   verified by `complete_upload`, not here. The answer is the slot's wire form.
//! - `download_object` takes `get_object`'s steps in its order: the cited revision against the
//!   revision map a purge wrote (`check_named`), then the object among that revision's objects
//!   and its media type (`served_media_type`, through `reading::authorize_object`: a digest
//!   authorizes nothing, and a backup artifact's object belongs to no revision), then
//!   `BlobStore::open`. It needs `read`, and `mcp_delegation` is refused before any grant is
//!   looked up: originals are not sent to a connected AI (Stage 1b design, the rule that refuses
//!   exports there), and no setting turns that on. A range that starts at or past the object's
//!   end, or ends before it starts, is `invalid_input` (the contract's statuses are a closed set:
//!   no 416); an end past the last byte is cut to it; without a range the whole object is sent.
//!   No receipt is written.
//! - The two artifact downloads are not built yet: each answers `NotImplemented`, naming it.
//!
//! Every `Forbidden` a transfer answers, its own or a port's, is recorded as a `permission_denied`
//! event naming the transport and no operation (`NewEvent::transport_denied`), in the asked
//! workspace's log. The refusal is the answer, so a failed append does not change it (dispatch's
//! rule); any other error is returned without an event. A download's body yields at most its
//! `length` bytes: the stream the store opened is cut to it.

use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{ArtifactId, Digest, ItemId, Revision, UploadId, WorkspaceId};
use okf_jawn_contract::import::Upload;
use okf_jawn_contract::transport::TransportName;
use tokio::io::AsyncReadExt as _;

use super::ApplicationService;
use super::shared::check_named;
use super::sources::served_media_type;
use super::workspaces::archived_at;
use crate::access::{authorize_workspace, check_route};
use crate::context::WorkspaceGrant;
use crate::events::{EventScope, NewEvent};
use crate::ports::PortFuture;
use crate::storage::{ByteReader, ObjectRead, StorageScope};
use crate::transfers::{ByteRange, Download, TransferCaller, Transfers};

impl Transfers for ApplicationService {
    fn upload_content<'a>(
        &'a self,
        caller: &'a TransferCaller,
        workspace: WorkspaceId,
        upload: UploadId,
        body: ByteReader,
    ) -> PortFuture<'a, Upload> {
        Box::pin(async move {
            let principal = &caller.principal;
            let outcome = upload_content(self, principal, workspace, upload, body).await;
            let log = workspace_log(principal, workspace);
            recorded(self, &log, principal, TransportName::UploadContent, outcome).await
        })
    }

    fn download_object<'a>(
        &'a self,
        caller: &'a TransferCaller,
        workspace: WorkspaceId,
        item: ItemId,
        revision: Revision,
        digest: Digest,
        range: Option<ByteRange>,
    ) -> PortFuture<'a, Download> {
        Box::pin(async move {
            let principal = &caller.principal;
            let outcome =
                download_object(self, principal, workspace, item, &revision, &digest, range).await;
            let log = workspace_log(principal, workspace);
            recorded(
                self,
                &log,
                principal,
                TransportName::DownloadObject,
                outcome,
            )
            .await
        })
    }

    fn download_artifact<'a>(
        &'a self,
        _caller: &'a TransferCaller,
        _workspace: WorkspaceId,
        _artifact: ArtifactId,
    ) -> PortFuture<'a, Download> {
        Box::pin(async { Err(pending(TransportName::DownloadArtifact)) })
    }

    fn download_tenant_artifact<'a>(
        &'a self,
        _caller: &'a TransferCaller,
        _artifact: ArtifactId,
    ) -> PortFuture<'a, Download> {
        Box::pin(async { Err(pending(TransportName::DownloadTenantArtifact)) })
    }
}

/// Append the body to the slot, as `Transfers::upload_content`.
///
/// # Errors
/// Returns `Forbidden` without `write` or on a route that cannot write, `Internal` for a grant
/// of another scope, `Conflict` with `ErrorDetail::WorkspaceArchived` in an archived workspace,
/// the store's refusals (`TooLarge`, `Conflict` for a complete slot, `NotFound`), or any port
/// error.
async fn upload_content(
    service: &ApplicationService,
    principal: &Principal,
    workspace: WorkspaceId,
    upload: UploadId,
    body: ByteReader,
) -> Result<Upload, ApiError> {
    let scope = workspace_grant(service, principal, workspace, Permission::Write)
        .await?
        .scope;
    if let Some(archived_at) = archived_at(service, &scope).await? {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            format!(
                "Workspace {} is archived, and {} does not run on an archived workspace; \
                 unarchive it first",
                workspace.0,
                TransportName::UploadContent.as_str()
            ),
        )
        .with_detail(ErrorDetail::WorkspaceArchived { archived_at }));
    }
    let record = service
        .ports()
        .uploads
        .put_content(&scope, upload, body, service.config().max_upload_bytes)
        .await?;
    Ok(record.to_wire(workspace))
}

/// The object `digest` of `item` at `revision`, whole or as `range`, as
/// `Transfers::download_object`.
///
/// # Errors
/// Returns `Forbidden` on `mcp_delegation` or without `read`, `Internal` for a grant of another
/// scope, the typed `NotFound` of an invalidated revision, `NotFound` for an object that is none
/// of the revision's, `InvalidInput` for a range that is no interval of the object, or any port
/// error.
async fn download_object(
    service: &ApplicationService,
    principal: &Principal,
    workspace: WorkspaceId,
    item: ItemId,
    revision: &Revision,
    digest: &Digest,
    range: Option<ByteRange>,
) -> Result<Download, ApiError> {
    if principal.route == AccessRoute::McpDelegation {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "An object's bytes are not sent to a connected AI",
        ));
    }
    let scope = workspace_grant(service, principal, workspace, Permission::Read)
        .await?
        .scope;
    check_named(service, &scope, revision).await?;
    let media_type = served_media_type(service, &scope, item, revision, digest).await?;
    let (start, asked) = match range {
        None => (0, u64::MAX),
        Some(ByteRange {
            start,
            end: Some(end),
        }) => {
            if end < start {
                return Err(not_an_interval("the range ends before it starts"));
            }
            (start, end.saturating_sub(start).saturating_add(1))
        }
        Some(ByteRange { start, end: None }) => (start, u64::MAX),
    };
    let read = service
        .ports()
        .blobs
        .open(&scope, digest, start, asked)
        .await?;
    let size = read.object.size;
    if range.is_some() && start >= size {
        return Err(not_an_interval(
            "the range starts at or past the end of the object",
        ));
    }
    let length = asked.min(size.saturating_sub(start));
    Ok(cut(media_type, start, length, read))
}

/// The caller's grant on `workspace` with `permission`, by dispatch's rules for a workspace
/// target: the route first, before any lookup; then the adapter's raw grant, used only when it
/// is for the workspace and the tenant asked; then the delegation ceiling and the permission.
///
/// # Errors
/// Returns `Forbidden` for the route or a missing permission, `Internal` for a grant of another
/// scope, or the adapter's error.
async fn workspace_grant(
    service: &ApplicationService,
    principal: &Principal,
    workspace: WorkspaceId,
    permission: Permission,
) -> Result<WorkspaceGrant, ApiError> {
    check_route(principal, permission)?;
    let raw = service
        .ports()
        .access
        .authorize(principal, workspace, permission)
        .await?;
    if raw.workspace_id() != workspace || raw.scope.tenant_id != principal.tenant_id {
        return Err(foreign_grant());
    }
    authorize_workspace(principal, raw, permission)
}

/// Record `outcome`'s `Forbidden` in `log` as a refusal of `transport` to `principal`, and return
/// `outcome` unchanged, whether or not the append succeeded.
async fn recorded<T>(
    service: &ApplicationService,
    log: &EventScope,
    principal: &Principal,
    transport: TransportName,
    outcome: Result<T, ApiError>,
) -> Result<T, ApiError> {
    if let Err(error) = &outcome
        && error.code == ErrorCode::Forbidden
    {
        let _recorded = service
            .ports()
            .events
            .append(log, None, NewEvent::transport_denied(principal, transport))
            .await;
    }
    outcome
}

/// The log of the asked workspace, in the caller's tenant.
fn workspace_log(principal: &Principal, workspace: WorkspaceId) -> EventScope {
    EventScope::Workspace(StorageScope {
        tenant_id: principal.tenant_id.clone(),
        workspace_id: workspace,
    })
}

/// The download of `read` from `offset`, its body cut to `length` bytes.
fn cut(media_type: String, offset: u64, length: u64, read: ObjectRead) -> Download {
    Download {
        media_type,
        total_size: read.object.size,
        offset,
        length,
        body: Box::pin(read.body.take(length)),
    }
}

/// A range that is no interval of the object.
fn not_an_interval(message: &str) -> ApiError {
    ApiError::new(ErrorCode::InvalidInput, message)
}

/// An adapter's grant for a workspace or tenant that was not asked.
fn foreign_grant() -> ApiError {
    ApiError::new(
        ErrorCode::Internal,
        "Access adapter returned a grant for a different scope",
    )
}

/// The answer of a transfer whose construction part has not landed.
fn pending(transport: TransportName) -> ApiError {
    ApiError::new(
        ErrorCode::NotImplemented,
        format!("{} is not implemented in this build", transport.as_str()),
    )
}
