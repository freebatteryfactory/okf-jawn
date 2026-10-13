//! The streaming transports: workspace events as Server-Sent Events over the `list_events`
//! operation, and the byte routes over a `Transfers` double.
#![cfg(feature = "runtime")]

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use futures_util::StreamExt as _;
use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{ArtifactId, Digest, ItemId, Revision, UploadId, WorkspaceId};
use okf_jawn_contract::import::Upload;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::ByteReader;
use okf_jawn_server::transfers::{ByteRange, Download, TransferCaller, Transfers};
use okf_jawn_server::{events, transfers};
use serde_json::json;
use tokio::io::AsyncReadExt as _;
use tower::ServiceExt;

use check::{TestResult, some};
use support::{HOST, LocalApp, SECRET, WORKSPACE};

/// A transfers double: it keeps what was uploaded and serves one fixed object.
#[derive(Default)]
struct Bytes {
    uploaded: Mutex<Vec<u8>>,
    callers: Mutex<Vec<AccessRoute>>,
    /// The media type the store recorded for the object; empty stands for `application/pdf`.
    stored_type: &'static str,
}

const OBJECT: &[u8] = b"0123456789";
const UPLOAD: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const ITEM: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

impl Bytes {
    fn saw(&self, caller: &TransferCaller) -> Result<(), ApiError> {
        self.callers
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))?
            .push(caller.principal.route.clone());
        Ok(())
    }

    /// The object as the store holds it. Like the application, this double trusts the route to
    /// have refused a range that is not one interval: an inverted range is answered whole.
    fn object(&self, range: Option<ByteRange>) -> Result<Download, ApiError> {
        let range = range.filter(|range| range.end.is_none_or(|end| end >= range.start));
        let total = OBJECT.len();
        let start = range.map_or(0, |range| {
            usize::try_from(range.start).unwrap_or(usize::MAX)
        });
        let last = range
            .and_then(|range| range.end)
            .map_or(total.saturating_sub(1), |end| {
                usize::try_from(end)
                    .unwrap_or(usize::MAX)
                    .min(total.saturating_sub(1))
            });
        let slice = OBJECT
            .get(start..=last)
            .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput, "range outside the object"))?;
        Ok(Download {
            media_type: if self.stored_type.is_empty() {
                "application/pdf".to_owned()
            } else {
                self.stored_type.to_owned()
            },
            total_size: u64::try_from(total).unwrap_or(u64::MAX),
            offset: u64::try_from(start).unwrap_or(u64::MAX),
            length: u64::try_from(slice.len()).unwrap_or(u64::MAX),
            body: Box::pin(Cursor::new(slice.to_vec())),
        })
    }
}

impl Transfers for Bytes {
    fn upload_content<'a>(
        &'a self,
        caller: &'a TransferCaller,
        workspace: WorkspaceId,
        upload: UploadId,
        mut body: ByteReader,
    ) -> PortFuture<'a, Upload> {
        Box::pin(async move {
            self.saw(caller)?;
            let mut received = Vec::new();
            body.read_to_end(&mut received)
                .await
                .map_err(|_| ApiError::new(ErrorCode::Unavailable, "the body failed"))?;
            let count = received.len();
            *self
                .uploaded
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))? = received;
            Ok(Upload {
                id: upload,
                workspace_id: workspace,
                upload_path: String::new(),
                received_bytes: count.to_string(),
                complete: false,
            })
        })
    }

    fn download_object<'a>(
        &'a self,
        caller: &'a TransferCaller,
        _workspace: WorkspaceId,
        _item: ItemId,
        _revision: Revision,
        _digest: Digest,
        range: Option<ByteRange>,
    ) -> PortFuture<'a, Download> {
        Box::pin(async move {
            self.saw(caller)?;
            self.object(range)
        })
    }

    fn download_artifact<'a>(
        &'a self,
        caller: &'a TransferCaller,
        _workspace: WorkspaceId,
        _artifact: ArtifactId,
    ) -> PortFuture<'a, Download> {
        Box::pin(async move {
            self.saw(caller)?;
            Err(ApiError::new(
                ErrorCode::Forbidden,
                "An archive is served only in a human browser session",
            ))
        })
    }

    fn download_tenant_artifact<'a>(
        &'a self,
        caller: &'a TransferCaller,
        _artifact: ArtifactId,
    ) -> PortFuture<'a, Download> {
        Box::pin(async move {
            self.saw(caller)?;
            self.object(None)
        })
    }
}

fn storing(stored_type: &'static str) -> Arc<Bytes> {
    Arc::new(Bytes {
        stored_type,
        ..Bytes::default()
    })
}

fn header(response: &axum::response::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// The three headers every download carries, whatever was stored.
fn assert_never_a_page(response: &axum::response::Response) {
    assert_eq!(
        header(response, "content-disposition").as_deref(),
        Some("attachment")
    );
    assert_eq!(
        header(response, "content-security-policy").as_deref(),
        Some("sandbox; default-src 'none'")
    );
    assert_eq!(
        header(response, "x-content-type-options").as_deref(),
        Some("nosniff")
    );
}

fn byte_service(bytes: Arc<Bytes>) -> Result<Router, Box<dyn std::error::Error>> {
    Ok(LocalApp::new()?.service(transfers::routes(bytes), Vec::new(), None)?)
}

fn object_path() -> String {
    format!(
        "/api/workspaces/{WORKSPACE}/items/{ITEM}/revisions/{}/objects/{}",
        "a".repeat(40),
        "d".repeat(64)
    )
}

fn request(
    method: &str,
    path: &str,
    bearer: bool,
    range: Option<&str>,
    body: Body,
) -> Result<Request<Body>, axum::http::Error> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", HOST);
    if bearer {
        builder = builder.header("authorization", format!("Bearer {SECRET}"));
    }
    if let Some(range) = range {
        builder = builder.header("range", range);
    }
    builder.body(body)
}

#[tokio::test]
async fn an_upload_body_reaches_the_application_as_the_caller() -> TestResult {
    let bytes = Arc::new(Bytes::default());
    let path = format!("/api/workspaces/{WORKSPACE}/uploads/{UPLOAD}/content");
    let response = byte_service(bytes.clone())?
        .oneshot(request(
            "PUT",
            &path,
            true,
            None,
            Body::from("original bytes"),
        )?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let upload: Upload = serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await?)?;
    assert_eq!(upload.received_bytes, "14");
    assert_eq!(
        bytes.uploaded.lock().map_err(|_| "poisoned")?.as_slice(),
        b"original bytes"
    );
    assert_eq!(
        bytes.callers.lock().map_err(|_| "poisoned")?.as_slice(),
        [AccessRoute::McpDelegation]
    );
    Ok(())
}

#[tokio::test]
async fn an_object_is_streamed_whole_or_as_the_requested_range() -> TestResult {
    let bytes = Arc::new(Bytes::default());
    let whole = byte_service(bytes.clone())?
        .oneshot(request("GET", &object_path(), true, None, Body::empty())?)
        .await?;
    assert_eq!(whole.status(), StatusCode::OK);
    assert_eq!(
        whole
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/pdf")
    );
    assert_eq!(to_bytes(whole.into_body(), 65_536).await?.as_ref(), OBJECT);

    let part = byte_service(bytes.clone())?
        .oneshot(request(
            "GET",
            &object_path(),
            true,
            Some("bytes=2-5"),
            Body::empty(),
        )?)
        .await?;
    assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        part.headers()
            .get("content-range")
            .and_then(|value| value.to_str().ok()),
        Some("bytes 2-5/10")
    );
    assert_eq!(to_bytes(part.into_body(), 65_536).await?.as_ref(), b"2345");

    let bad = byte_service(bytes)?
        .oneshot(request(
            "GET",
            &object_path(),
            true,
            Some("bytes=5-2"),
            Body::empty(),
        )?)
        .await?;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn a_stored_type_that_could_run_as_a_page_is_sent_as_the_declared_type() -> TestResult {
    for stored in [
        "text/html",
        "image/svg+xml",
        "application/xhtml+xml",
        "text/html; charset=utf-8",
        "TEXT/HTML",
        "image/png; charset=utf-8",
        "text/javascript",
    ] {
        let response = byte_service(storing(stored))?
            .oneshot(request("GET", &object_path(), true, None, Body::empty())?)
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header(&response, "content-type").as_deref(),
            Some("application/octet-stream"),
            "stored {stored}"
        );
        assert_never_a_page(&response);
    }
    Ok(())
}

#[tokio::test]
async fn an_inert_stored_type_is_kept_and_is_still_an_attachment() -> TestResult {
    for (stored, sent) in [
        ("image/png", "image/png"),
        ("IMAGE/PNG", "image/png"),
        ("image/jpeg", "image/jpeg"),
        ("image/gif", "image/gif"),
        ("image/webp", "image/webp"),
        ("application/pdf", "application/pdf"),
    ] {
        let response = byte_service(storing(stored))?
            .oneshot(request("GET", &object_path(), true, None, Body::empty())?)
            .await?;
        assert_eq!(header(&response, "content-type").as_deref(), Some(sent));
        assert_never_a_page(&response);
    }
    Ok(())
}

#[tokio::test]
async fn a_ranged_download_and_an_artifact_carry_the_same_headers() -> TestResult {
    let ranged = byte_service(storing("text/html"))?
        .oneshot(request(
            "GET",
            &object_path(),
            true,
            Some("bytes=2-5"),
            Body::empty(),
        )?)
        .await?;
    assert_eq!(ranged.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        header(&ranged, "content-type").as_deref(),
        Some("application/octet-stream")
    );
    assert_never_a_page(&ranged);

    // An artifact is declared `application/zip`, whatever the store says it is.
    for stored in ["application/zip", "text/html"] {
        let artifact = byte_service(storing(stored))?
            .oneshot(request(
                "GET",
                &format!("/api/artifacts/{UPLOAD}"),
                true,
                None,
                Body::empty(),
            )?)
            .await?;
        assert_eq!(artifact.status(), StatusCode::OK);
        assert_eq!(
            header(&artifact, "content-type").as_deref(),
            Some("application/zip")
        );
        assert_never_a_page(&artifact);
    }
    Ok(())
}

#[tokio::test]
async fn an_inverted_range_is_refused_by_the_route_itself() -> TestResult {
    // The double answers an inverted range whole, so only the route's own check refuses it.
    let response = byte_service(Arc::new(Bytes::default()))?
        .oneshot(request(
            "GET",
            &object_path(),
            true,
            Some("bytes=5-2"),
            Body::empty(),
        )?)
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ApiError = serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await?)?;
    assert_eq!(error.code, ErrorCode::InvalidInput);
    Ok(())
}

#[tokio::test]
async fn a_byte_route_needs_a_principal_and_reports_the_applications_refusal() -> TestResult {
    let bytes = Arc::new(Bytes::default());
    let anonymous = byte_service(bytes.clone())?
        .oneshot(request("GET", &object_path(), false, None, Body::empty())?)
        .await?;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert!(bytes.callers.lock().map_err(|_| "poisoned")?.is_empty());

    let artifact = format!("/api/workspaces/{WORKSPACE}/artifacts/{UPLOAD}");
    let refused = byte_service(bytes.clone())?
        .oneshot(request("GET", &artifact, true, None, Body::empty())?)
        .await?;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    let error: ApiError = serde_json::from_slice(&to_bytes(refused.into_body(), 65_536).await?)?;
    assert_eq!(error.code, ErrorCode::Forbidden);

    let tenant = byte_service(bytes.clone())?
        .oneshot(request(
            "GET",
            &format!("/api/artifacts/{UPLOAD}"),
            true,
            None,
            Body::empty(),
        )?)
        .await?;
    assert_eq!(tenant.status(), StatusCode::OK);

    let no_such = byte_service(bytes)?
        .oneshot(request(
            "GET",
            "/api/artifacts/not-an-id",
            true,
            None,
            Body::empty(),
        )?)
        .await?;
    assert_eq!(no_such.status(), StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn workspace_events_stream_as_sse_with_their_cursor_as_the_id() -> TestResult {
    let base = LocalApp::new()?;
    base.counting.set_response(
        "list_events",
        json!({
            "events": [
                { "id": "41", "workspace_id": WORKSPACE, "kind": "changed",
                  "at": "2026-10-12T12:00:00.000Z" },
                { "id": "42", "workspace_id": WORKSPACE, "kind": "imported",
                  "at": "2026-10-12T12:00:01.000Z" }
            ]
        }),
    )?;
    let app = base.service(events::routes(base.bound()), Vec::new(), None)?;
    let path = format!("/api/workspaces/{WORKSPACE}/events");
    let response = app
        .clone()
        .oneshot(request("GET", &path, true, None, Body::empty())?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let mut frames = response.into_body().into_data_stream();
    let mut seen = String::new();
    // Two events are pending; a bounded read, so a stream that sent neither cannot hang the test.
    for _ in 0..4 {
        if seen.contains("\"imported\"") {
            break;
        }
        let chunk = some(frames.next().await, "another SSE frame")??;
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(seen.contains("id: 41") || seen.contains("id:41"));
    assert!(seen.contains("id: 42") || seen.contains("id:42"));
    assert!(seen.contains("\"kind\":\"changed\""));
    assert!(seen.contains("\"kind\":\"imported\""));
    // The stream ran the operation as the caller: an agent route, with no browser session.
    let contexts = base.counting.contexts("list_events")?;
    let context = some(contexts.first(), "the poll's context")?;
    assert_eq!(context.principal.route, AccessRoute::McpDelegation);

    // Without a principal the stream never opens.
    let anonymous = app
        .oneshot(request("GET", &path, false, None, Body::empty())?)
        .await?;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn a_stream_the_caller_may_not_read_is_the_operations_refusal() -> TestResult {
    let base = LocalApp::new()?;
    let app = base.service(events::routes(base.bound()), Vec::new(), None)?;
    // The connector's ceiling names one workspace; this is another.
    let path = "/api/workspaces/22222222-2222-4222-8222-222222222222/events";
    let response = app
        .oneshot(request("GET", path, true, None, Body::empty())?)
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ApiError = serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await?)?;
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(base.counting.call_count("list_events")?, 0);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
mod support;
