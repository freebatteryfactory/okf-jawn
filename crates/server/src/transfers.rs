//! The declared byte transports: upload content, object and artifact downloads.
//!
//! The routes only move bytes: they read the caller from the request extensions the identity
//! middleware set, turn the body into a `ByteReader` (`tokio_util::io::StreamReader`) or a
//! `ByteReader` into the response body (`tokio_util::io::ReaderStream`), and parse the path and an
//! optional single `Range`. Every decision (which object an item revision may open, which artifact
//! kind needs which permission on which route, an upload's byte limit) belongs to the
//! application behind core's [`Transfers`], which `ApplicationService` implements; these routes
//! are tested here with doubles of it, and no adapter of the server's stands between them.
//!
//! A download is never a page of the application origin (SPEC section 11: "Uploaded HTML is
//! hostile data until constrained by actual isolation"). The response headers are written here,
//! not by the application: every download carries `Content-Disposition: attachment`,
//! `Content-Security-Policy: sandbox; default-src 'none'` and `X-Content-Type-Options: nosniff`,
//! and its `Content-Type` is the type the transport declares for the route unless the stored type
//! is one of [`INERT_TYPES`]. A ranged answer carries the same headers.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_SECURITY_POLICY,
    CONTENT_TYPE, RANGE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Extension, Json, Router};
use futures_util::TryStreamExt as _;
use okf_jawn_contract::access::Principal;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::storage::ByteReader;
use okf_jawn_core::transfers::{ByteRange, Download, TransferCaller, Transfers};
use serde::de::DeserializeOwned;
use tokio_util::io::{ReaderStream, StreamReader};

use crate::SessionId;
use crate::errors::{Readable, error_response};

/// The upload route (`api/transports.json` `upload_content`).
pub const UPLOAD_PATH: &str = "/api/workspaces/{workspace_id}/uploads/{upload_id}/content";

/// The object route (`download_object`).
pub const OBJECT_PATH: &str =
    "/api/workspaces/{workspace_id}/items/{item_id}/revisions/{revision}/objects/{digest}";

/// The workspace artifact route (`download_artifact`).
pub const ARTIFACT_PATH: &str = "/api/workspaces/{workspace_id}/artifacts/{artifact_id}";

/// The installation archive route (`download_tenant_artifact`).
pub const TENANT_ARTIFACT_PATH: &str = "/api/artifacts/{artifact_id}";

/// The media type `api/transports.json` declares for `download_object`.
pub const OBJECT_MEDIA: &str = "application/octet-stream";

/// The media type `api/transports.json` declares for both artifact downloads.
pub const ARTIFACT_MEDIA: &str = "application/zip";

/// The stored media types a download keeps: none of them can run as a page. `image/svg+xml`,
/// `text/html` and every other type go out as the route's declared type.
pub const INERT_TYPES: [&str; 5] = [
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "application/pdf",
];

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
    Readable(Path((workspace_id, upload_id))): Readable<Path<(String, String)>>,
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
    Readable(Path((workspace_id, item_id, revision, digest))): Readable<
        Path<(String, String, String, String)>,
    >,
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
        send(download, OBJECT_MEDIA, range.is_some())
    }
    .await;
    outcome.unwrap_or_else(error_response)
}

async fn download_artifact(
    State(transfers): State<Arc<dyn Transfers>>,
    principal: Option<Extension<Principal>>,
    session: Option<Extension<SessionId>>,
    Readable(Path((workspace_id, artifact_id))): Readable<Path<(String, String)>>,
) -> Response {
    let outcome = async {
        let caller = caller_of(principal, session)?;
        let download = transfers
            .download_artifact(&caller, identity(&workspace_id)?, identity(&artifact_id)?)
            .await?;
        send(download, ARTIFACT_MEDIA, false)
    }
    .await;
    outcome.unwrap_or_else(error_response)
}

async fn download_tenant_artifact(
    State(transfers): State<Arc<dyn Transfers>>,
    principal: Option<Extension<Principal>>,
    session: Option<Extension<SessionId>>,
    Readable(Path(artifact_id)): Readable<Path<String>>,
) -> Response {
    let outcome = async {
        let caller = caller_of(principal, session)?;
        let download = transfers
            .download_tenant_artifact(&caller, identity(&artifact_id)?)
            .await?;
        send(download, ARTIFACT_MEDIA, false)
    }
    .await;
    outcome.unwrap_or_else(error_response)
}

/// A path segment as the identity it names; `not_found` for one that names none.
fn identity<T: DeserializeOwned>(segment: &str) -> Result<T, ApiError> {
    serde_json::from_value(serde_json::Value::String(segment.to_owned()))
        .map_err(|_| ApiError::new(ErrorCode::NotFound, "No such resource"))
}

/// The media type a download is sent with: the stored type when it is one of [`INERT_TYPES`],
/// compared whole and ASCII case-insensitively (a type with any parameter is not on the list),
/// and otherwise the type the transport declares for the route.
fn sent_media_type(stored: &str, declared: &'static str) -> &'static str {
    INERT_TYPES
        .iter()
        .copied()
        .find(|inert| inert.eq_ignore_ascii_case(stored))
        .unwrap_or(declared)
}

/// The response that streams `download` as an attachment: 206 with `Content-Range` for a range
/// request. It is never a page of this origin, whatever was stored (see the module header).
fn send(download: Download, declared: &'static str, ranged: bool) -> Result<Response, ApiError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static(sent_media_type(&download.media_type, declared)),
    );
    headers.insert(CONTENT_LENGTH, HeaderValue::from(download.length));
    headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(CONTENT_DISPOSITION, HeaderValue::from_static("attachment"));
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    let status = if ranged {
        let last = download
            .offset
            .saturating_add(download.length)
            .saturating_sub(1);
        let range = format!("bytes {}-{last}/{}", download.offset, download.total_size);
        let range = HeaderValue::from_str(&range)
            .map_err(|_| ApiError::new(ErrorCode::Internal, "A download range cannot be sent"))?;
        headers.insert(CONTENT_RANGE, range);
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let body = Body::from_stream(ReaderStream::new(download.body));
    Ok((status, headers, body).into_response())
}
