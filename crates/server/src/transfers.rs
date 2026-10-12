//! The declared byte transports: upload content, object and artifact downloads.
//!
//! The routes only move bytes: they read the caller from the request extensions the identity
//! middleware set, turn the body into a `ByteReader` (`tokio_util::io::StreamReader`) or a
//! `ByteReader` into the response body (`tokio_util::io::ReaderStream`), and parse the path and an
//! optional single `Range`. Every decision (which object an item revision may open, which artifact
//! kind needs which permission on which route, an upload's byte limit) belongs to the
//! application behind [`Transfers`]. Core has no such port yet (report request S3), so these
//! routes are bound against this trait and tested with doubles; no production adapter exists in
//! the server.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Extension, Json, Router};
use futures_util::TryStreamExt as _;
use okf_jawn_contract::access::Principal;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{ArtifactId, Digest, ItemId, Revision, UploadId, WorkspaceId};
use okf_jawn_contract::import::Upload;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::ByteReader;
use serde::de::DeserializeOwned;
use tokio_util::io::{ReaderStream, StreamReader};

use crate::SessionId;
use crate::errors::error_response;

/// The application side of the byte transports; each call authorizes as `caller`.
pub trait Transfers: Send + Sync {
    /// Append the request body to an upload slot (`upload_content`).
    fn upload_content<'a>(
        &'a self,
        caller: &'a TransferCaller,
        workspace: WorkspaceId,
        upload: UploadId,
        body: ByteReader,
    ) -> PortFuture<'a, Upload>;
    /// An object authorized through an exact item revision (`download_object`).
    fn download_object<'a>(
        &'a self,
        caller: &'a TransferCaller,
        workspace: WorkspaceId,
        item: ItemId,
        revision: Revision,
        digest: Digest,
        range: Option<ByteRange>,
    ) -> PortFuture<'a, Download>;
    /// A workspace artifact (`download_artifact`).
    fn download_artifact<'a>(
        &'a self,
        caller: &'a TransferCaller,
        workspace: WorkspaceId,
        artifact: ArtifactId,
    ) -> PortFuture<'a, Download>;
    /// An installation archive (`download_tenant_artifact`).
    fn download_tenant_artifact<'a>(
        &'a self,
        caller: &'a TransferCaller,
        artifact: ArtifactId,
    ) -> PortFuture<'a, Download>;
}

/// Who a transfer acts for, as the identity middleware established it.
#[derive(Debug, Clone)]
pub struct TransferCaller {
    /// The authenticated principal.
    pub principal: Principal,
    /// The browser session, when the request came with the session cookie.
    pub session_id: Option<String>,
}

/// One requested byte interval: `bytes=start-` or `bytes=start-end` (inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    /// First byte.
    pub start: u64,
    /// Last byte, inclusive; `None` to the end.
    pub end: Option<u64>,
}

/// Bytes to send and what they are.
pub struct Download {
    /// Media type.
    pub media_type: String,
    /// Whole object size.
    pub total_size: u64,
    /// Offset of the first byte sent.
    pub offset: u64,
    /// Bytes sent.
    pub length: u64,
    /// The bytes.
    pub body: ByteReader,
}

/// The upload route (`api/transports.json` `upload_content`).
pub const UPLOAD_PATH: &str = "/api/workspaces/{workspace_id}/uploads/{upload_id}/content";

/// The object route (`download_object`).
pub const OBJECT_PATH: &str =
    "/api/workspaces/{workspace_id}/items/{item_id}/revisions/{revision}/objects/{digest}";

/// The workspace artifact route (`download_artifact`).
pub const ARTIFACT_PATH: &str = "/api/workspaces/{workspace_id}/artifacts/{artifact_id}";

/// The installation archive route (`download_tenant_artifact`).
pub const TENANT_ARTIFACT_PATH: &str = "/api/artifacts/{artifact_id}";

/// The byte routes over `transfers`, for `ServiceParts::extra`.
pub fn routes(transfers: Arc<dyn Transfers>) -> Router {
    Router::new()
        .route(UPLOAD_PATH, put(upload_content))
        .route(OBJECT_PATH, get(download_object))
        .route(ARTIFACT_PATH, get(download_artifact))
        .route(TENANT_ARTIFACT_PATH, get(download_tenant_artifact))
        .with_state(transfers)
}

/// The single `bytes=` range of `headers`; `Ok(None)` without one.
///
/// # Errors
/// Returns `InvalidInput` for a range header that is not one `bytes=start-[end]` interval with
/// `start <= end`.
pub fn requested_range(headers: &HeaderMap) -> Result<Option<ByteRange>, ApiError> {
    let Some(value) = headers.get(RANGE) else {
        return Ok(None);
    };
    let invalid = || {
        ApiError::new(
            ErrorCode::InvalidInput,
            "Range must be one bytes=start-[end]",
        )
    };
    let text = value.to_str().map_err(|_| invalid())?;
    let spec = text.strip_prefix("bytes=").ok_or_else(invalid)?;
    let (start, end) = spec.split_once('-').ok_or_else(invalid)?;
    let start: u64 = start.parse().map_err(|_| invalid())?;
    let end = if end.is_empty() {
        None
    } else {
        let end: u64 = end.parse().map_err(|_| invalid())?;
        if end < start {
            return Err(invalid());
        }
        Some(end)
    };
    Ok(Some(ByteRange { start, end }))
}

fn caller_of(
    principal: Option<Extension<Principal>>,
    session: Option<Extension<SessionId>>,
) -> Result<TransferCaller, ApiError> {
    let Some(Extension(principal)) = principal else {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Authentication is required",
        ));
    };
    Ok(TransferCaller {
        principal,
        session_id: session.map(|Extension(SessionId(id))| id),
    })
}

async fn upload_content(
    State(transfers): State<Arc<dyn Transfers>>,
    principal: Option<Extension<Principal>>,
    session: Option<Extension<SessionId>>,
    Path((workspace_id, upload_id)): Path<(String, String)>,
    body: Body,
) -> Response {
    let outcome = async {
        let caller = caller_of(principal, session)?;
        let workspace = identity(&workspace_id)?;
        let upload = identity(&upload_id)?;
        let stream = body.into_data_stream().map_err(std::io::Error::other);
        let reader: ByteReader = Box::pin(StreamReader::new(stream));
        let upload = transfers
            .upload_content(&caller, workspace, upload, reader)
            .await?;
        Ok::<_, ApiError>(Json(upload).into_response())
    }
    .await;
    outcome.unwrap_or_else(error_response)
}

async fn download_object(
    State(transfers): State<Arc<dyn Transfers>>,
    principal: Option<Extension<Principal>>,
    session: Option<Extension<SessionId>>,
    Path((workspace_id, item_id, revision, digest)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let outcome = async {
        let caller = caller_of(principal, session)?;
        let range = requested_range(&headers)?;
        let download = transfers
            .download_object(
                &caller,
                identity(&workspace_id)?,
                identity(&item_id)?,
                identity(&revision)?,
                identity(&digest)?,
                range,
            )
            .await?;
        send(download, range.is_some())
    }
    .await;
    outcome.unwrap_or_else(error_response)
}

async fn download_artifact(
    State(transfers): State<Arc<dyn Transfers>>,
    principal: Option<Extension<Principal>>,
    session: Option<Extension<SessionId>>,
    Path((workspace_id, artifact_id)): Path<(String, String)>,
) -> Response {
    let outcome = async {
        let caller = caller_of(principal, session)?;
        let download = transfers
            .download_artifact(&caller, identity(&workspace_id)?, identity(&artifact_id)?)
            .await?;
        send(download, false)
    }
    .await;
    outcome.unwrap_or_else(error_response)
}

async fn download_tenant_artifact(
    State(transfers): State<Arc<dyn Transfers>>,
    principal: Option<Extension<Principal>>,
    session: Option<Extension<SessionId>>,
    Path(artifact_id): Path<String>,
) -> Response {
    let outcome = async {
        let caller = caller_of(principal, session)?;
        let download = transfers
            .download_tenant_artifact(&caller, identity(&artifact_id)?)
            .await?;
        send(download, false)
    }
    .await;
    outcome.unwrap_or_else(error_response)
}

/// A path segment as the identity it names; `not_found` for one that names none.
fn identity<T: DeserializeOwned>(segment: &str) -> Result<T, ApiError> {
    serde_json::from_value(serde_json::Value::String(segment.to_owned()))
        .map_err(|_| ApiError::new(ErrorCode::NotFound, "No such resource"))
}

/// The response that streams `download`: 206 with `Content-Range` for a range request.
fn send(download: Download, ranged: bool) -> Result<Response, ApiError> {
    let header = |text: String| {
        HeaderValue::from_str(&text)
            .map_err(|_| ApiError::new(ErrorCode::Internal, "A download header cannot be sent"))
    };
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, header(download.media_type.clone())?);
    headers.insert(CONTENT_LENGTH, header(download.length.to_string())?);
    headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    let status = if ranged {
        let last = download
            .offset
            .saturating_add(download.length)
            .saturating_sub(1);
        headers.insert(
            CONTENT_RANGE,
            header(format!(
                "bytes {}-{last}/{}",
                download.offset, download.total_size
            ))?,
        );
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let body = Body::from_stream(ReaderStream::new(download.body));
    Ok((status, headers, body).into_response())
}
