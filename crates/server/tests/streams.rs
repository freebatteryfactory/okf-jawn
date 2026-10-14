//! The streaming transports: workspace events as Server-Sent Events over the `list_events`
//! operation, and the byte routes over a `Transfers` double.
#![cfg(feature = "runtime")]

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use futures_util::StreamExt as _;
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{
    ArtifactId, Digest, ItemId, Revision, TenantId, Timestamp, UploadId, WorkspaceId,
};
use okf_jawn_contract::import::Upload;
use okf_jawn_core::context::OperationContext;
use okf_jawn_core::credentials::{CredentialStore, SessionRecord};
use okf_jawn_core::ports::{Application, PortFuture};
use okf_jawn_core::storage::ByteReader;
use okf_jawn_server::app::{ServiceParts, service};
use okf_jawn_server::cookies::CrateCookies;
use okf_jawn_server::events::EventStreams;
use okf_jawn_server::identity::{
    BearerAuth, ConnectIdentity, Identity, IdentityPorts, WallClock, session_id_of,
};
use okf_jawn_server::transfers::{ByteRange, Download, TransferCaller, Transfers};
use okf_jawn_server::{BoundApplication, events, transfers};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt as _;
use tokio::time::timeout;
use tower::ServiceExt;

use check::{TestResult, some};
use fixtures::workspace;
use support::{CONNECTOR, HOST, LocalApp, ORIGIN, OWNER, OneToken, SECRET, WORKSPACE};

macro_rules! scripted_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        impl Application for Scripted {
            $(fn $id<'a>(&'a self, _context: &'a OperationContext, request: $request)
                -> PortFuture<'a, $response> {
                Box::pin(async move {
                    let asked = serde_json::to_value(&request)
                        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
                    let value = self.answer(stringify!($id), asked)?;
                    serde_json::from_value(value)
                        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
                })
            })*
        }
    };
}

/// A transfers double: it keeps what was uploaded and serves one fixed object.
#[derive(Default)]
struct Bytes {
    uploaded: Mutex<Vec<u8>>,
    callers: Mutex<Vec<AccessRoute>>,
    /// The media type the store recorded for the object; empty stands for `application/pdf`.
    stored_type: &'static str,
}

/// An application whose `list_events` plays scripted pages and keeps every request it was
/// asked; every other operation is refused. The shared counting double records contexts only.
#[derive(Default)]
struct Scripted {
    pages: Mutex<VecDeque<Value>>,
    asked: Mutex<Vec<Value>>,
    /// When set, every `list_events` after this many is refused as `Forbidden`: the caller's
    /// grant was revoked.
    allowed: Mutex<Option<usize>>,
}

/// One open stream and what it was built over.
struct Stream {
    base: LocalApp,
    scripted: Arc<Scripted>,
    app: Router,
}

/// A wall clock a test sets, in milliseconds since the Unix epoch.
struct WallAt(AtomicU64);

/// The one Connect token the hosted stream service accepts.
const CONNECT_TOKEN: &str = "connect-token";

const FAR_FUTURE: &str = "2999-01-01T00:00:00.000Z";
const LONG_PAST: &str = "2000-01-01T00:00:00.000Z";
const SHORT: Duration = Duration::from_millis(20);
const NEVER: Duration = Duration::from_secs(3600);

const OBJECT: &[u8] = b"0123456789";
const UPLOAD: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const ITEM: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

impl Scripted {
    fn answer(&self, operation: &str, asked: Value) -> Result<Value, ApiError> {
        if operation != "list_events" {
            return Err(ApiError::new(ErrorCode::NotImplemented, "not scripted"));
        }
        let polls = {
            let mut all = self.asked.lock().map_err(|_| poisoned())?;
            all.push(asked);
            all.len()
        };
        let allowed = *self.allowed.lock().map_err(|_| poisoned())?;
        if allowed.is_some_and(|allowed| polls > allowed) {
            return Err(ApiError::new(ErrorCode::Forbidden, "the grant was revoked"));
        }
        Ok(self
            .pages
            .lock()
            .map_err(|_| poisoned())?
            .pop_front()
            .unwrap_or_else(|| json!({ "events": [] })))
    }

    fn asked(&self) -> Result<Vec<Value>, ApiError> {
        Ok(self.asked.lock().map_err(|_| poisoned())?.clone())
    }
}

impl WallClock for WallAt {
    fn unix_millis(&self) -> u128 {
        u128::from(self.0.load(Ordering::SeqCst))
    }
}

impl Stream {
    /// A local stream service polling every `interval` over `pages`.
    fn new(interval: Duration, pages: Vec<Value>) -> Result<Self, Box<dyn std::error::Error>> {
        let base = LocalApp::new()?;
        let identity = base.identity();
        Self::over(base, identity, interval, pages)
    }

    /// A hosted stream service: its verifier accepts [`CONNECT_TOKEN`] and says it expires at
    /// `expires_at` (seconds since the Unix epoch), and the service reads the time from `clock`.
    fn hosted(
        interval: Duration,
        pages: Vec<Value>,
        expires_at: u64,
        clock: Arc<WallAt>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let base = LocalApp::new()?;
        let identity = Identity::with_clock(
            BearerAuth::Hosted {
                connect: Arc::new(OneToken {
                    token: CONNECT_TOKEN.to_owned(),
                    identity: ConnectIdentity {
                        subject: OWNER.to_owned(),
                        organization_id: "org_01".to_owned(),
                        client_id: "client_mcp".to_owned(),
                        expires_at,
                    },
                }),
                resource_metadata: "https://okf.example.test/.well-known/oauth-protected-resource"
                    .to_owned(),
            },
            IdentityPorts {
                credentials: base.credentials.clone(),
                events: base.ports.events.clone(),
                cookies: Arc::new(CrateCookies),
            },
            vec![ORIGIN.to_owned()],
            clock,
        );
        Self::over(base, identity, interval, pages)
    }

    /// The stream service over `base`, authenticating and re-checking with `identity`.
    fn over(
        base: LocalApp,
        identity: Identity,
        interval: Duration,
        pages: Vec<Value>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let scripted = Arc::new(Scripted {
            pages: Mutex::new(pages.into()),
            asked: Mutex::new(Vec::new()),
            allowed: Mutex::new(None),
        });
        let streams = EventStreams::new(
            BoundApplication {
                application: scripted.clone(),
                ..base.bound()
            },
            identity.clone(),
            interval,
        )?;
        let app = service(ServiceParts {
            bound: base.bound(),
            identity,
            hosts: vec![HOST.to_owned()],
            hosted: None,
            cors_origins: Vec::new(),
            ui_dir: None,
            extra: events::routes(streams),
        })?;
        Ok(Self {
            base,
            scripted,
            app,
        })
    }

    /// Refuse every `list_events` after the first `polls`, as a revoked grant would.
    fn refuse_after(&self, polls: usize) -> Result<(), ApiError> {
        *self.scripted.allowed.lock().map_err(|_| poisoned())? = Some(polls);
        Ok(())
    }

    /// Open the stream with `headers`; the response of the first poll.
    async fn open(
        &self,
        headers: &[(&str, &str)],
    ) -> Result<axum::response::Response, Box<dyn std::error::Error>> {
        let mut builder =
            Request::get(format!("/api/workspaces/{WORKSPACE}/events")).header("host", HOST);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        Ok(self
            .app
            .clone()
            .oneshot(builder.body(Body::empty())?)
            .await?)
    }

    /// Store a live owner session and answer its `Cookie` header.
    fn session(
        &self,
        secret: &str,
        expires_at: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        self.base.credentials.add_session(SessionRecord {
            session_id: session_id_of(secret),
            principal: Principal {
                subject: OWNER.to_owned(),
                tenant_id: TenantId::local(),
                route: AccessRoute::LocalOwner,
                client_id: None,
                delegation: None,
            },
            expires_at: Timestamp::try_from(expires_at.to_owned())?,
        })?;
        Ok(format!("okf-session={secret}"))
    }
}

okf_jawn_contract::for_each_operation!(scripted_operations);

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
                filename: None,
                relative_path: None,
                size: None,
                created_at: None,
                consumed_by: None,
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

fn poisoned() -> ApiError {
    ApiError::new(ErrorCode::Internal, "poisoned")
}

/// A page of `count` events whose ids start at `first`.
fn page(first: usize, count: usize) -> Value {
    let events: Vec<Value> = (first..first.saturating_add(count))
        .map(|id| {
            json!({
                "id": id.to_string(),
                "workspace_id": WORKSPACE,
                "kind": "changed",
                "at": "2026-10-12T12:00:00.000Z"
            })
        })
        .collect();
    json!({ "events": events })
}

/// Read the stream until it ends or `events` events were seen, giving up after ten seconds; the
/// text received, and whether the stream ended.
async fn read(
    response: axum::response::Response,
    events: usize,
) -> Result<(String, bool), Box<dyn std::error::Error>> {
    let mut frames = response.into_body().into_data_stream();
    let mut seen = String::new();
    let reading = async {
        loop {
            if seen.matches("\"kind\"").count() >= events {
                return Ok::<bool, axum::Error>(false);
            }
            match frames.next().await {
                Some(chunk) => seen.push_str(&String::from_utf8_lossy(&chunk?)),
                None => return Ok(true),
            }
        }
    };
    let ended = tokio::time::timeout(Duration::from_secs(10), reading).await;
    match ended {
        Ok(ended) => Ok((seen, ended?)),
        Err(_) => Err(format!(
            "the stream neither ended nor delivered {events} events in ten seconds; got {} bytes",
            seen.len()
        )
        .into()),
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
    let stream = Stream::new(NEVER, vec![page(41, 2)])?;
    let response = stream
        .open(&[("authorization", &format!("Bearer {SECRET}"))])
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header(&response, "content-type").as_deref(),
        Some("text/event-stream")
    );
    let (seen, ended) = read(response, 2).await?;
    assert!(!ended);
    assert!(seen.contains("id: 41") || seen.contains("id:41"));
    assert!(seen.contains("id: 42") || seen.contains("id:42"));

    // Without a principal the stream never opens.
    let anonymous = stream.open(&[]).await?;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn last_event_id_is_forwarded_as_after_and_each_poll_resumes_after_the_last_event()
-> TestResult {
    let stream = Stream::new(SHORT, vec![page(42, 2), page(44, 1)])?;
    let response = stream
        .open(&[
            ("authorization", &format!("Bearer {SECRET}")),
            ("last-event-id", "41"),
        ])
        .await?;
    let (_, ended) = read(response, 3).await?;
    assert!(!ended);
    let asked = stream.scripted.asked()?;
    let first = some(asked.first(), "the first poll")?;
    assert_eq!(first.get("after"), Some(&json!("41")));
    assert_eq!(first.get("workspace_id"), Some(&json!(WORKSPACE)));
    let second = some(asked.get(1), "the second poll")?;
    assert_eq!(second.get("after"), Some(&json!("43")));

    // Without the header the first poll names no cursor.
    let fresh = Stream::new(NEVER, vec![page(1, 1)])?;
    let response = fresh
        .open(&[("authorization", &format!("Bearer {SECRET}"))])
        .await?;
    read(response, 1).await?;
    let asked = fresh.scripted.asked()?;
    assert_eq!(some(asked.first(), "the first poll")?.get("after"), None);
    Ok(())
}

#[tokio::test]
async fn a_last_event_id_over_the_bound_is_refused_not_dropped() -> TestResult {
    let stream = Stream::new(NEVER, vec![page(1, 1), page(1, 1)])?;
    let bearer = format!("Bearer {SECRET}");
    let at_bound = "c".repeat(256);
    let accepted = stream
        .open(&[("authorization", &bearer), ("last-event-id", &at_bound)])
        .await?;
    assert_eq!(accepted.status(), StatusCode::OK);
    let over = "c".repeat(257);
    let refused = stream
        .open(&[("authorization", &bearer), ("last-event-id", &over)])
        .await?;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    let error: ApiError = serde_json::from_slice(&to_bytes(refused.into_body(), 65_536).await?)?;
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("Last-Event-ID"));
    // Only the accepted request reached the operation.
    assert_eq!(stream.scripted.asked()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_backlog_of_three_pages_arrives_without_waiting_between_full_pages() -> TestResult {
    // The interval is an hour: only polls that follow a full page at once can deliver page two
    // and page three inside the read's ten seconds.
    let stream = Stream::new(NEVER, vec![page(1, 100), page(101, 100), page(201, 1)])?;
    let response = stream
        .open(&[("authorization", &format!("Bearer {SECRET}"))])
        .await?;
    let (seen, ended) = read(response, 201).await?;
    assert!(!ended);
    assert!(seen.contains("id: 201") || seen.contains("id:201"));
    assert_eq!(stream.scripted.asked()?.len(), 3);
    Ok(())
}

#[tokio::test]
async fn a_page_that_is_not_full_is_followed_by_a_wait() -> TestResult {
    let stream = Stream::new(NEVER, vec![page(1, 99), page(100, 1)])?;
    let response = stream
        .open(&[("authorization", &format!("Bearer {SECRET}"))])
        .await?;
    let mut frames = response.into_body().into_data_stream();
    let mut seen = String::new();
    while seen.matches("\"kind\"").count() < 99 {
        let chunk = some(frames.next().await, "another SSE frame")??;
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }
    // The second page is an hour away: nothing more arrives, and the operation ran once.
    let more = tokio::time::timeout(Duration::from_millis(300), frames.next()).await;
    assert!(more.is_err(), "an event arrived without the wait");
    assert_eq!(stream.scripted.asked()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_stream_ends_when_its_session_is_logged_out() -> TestResult {
    let stream = Stream::new(SHORT, vec![page(1, 1), page(99, 1)])?;
    let cookie = stream.session("stream-session-secret", FAR_FUTURE)?;
    let response = stream.open(&[("cookie", &cookie)]).await?;
    assert_eq!(response.status(), StatusCode::OK);
    // Logged out before the second poll: the session is gone from the store.
    stream
        .base
        .credentials
        .revoke_session(&session_id_of("stream-session-secret"))
        .await?;
    let (seen, ended) = read(response, 2).await?;
    assert!(ended, "the stream outlived its session");
    assert!(!seen.contains("id: 99") && !seen.contains("id:99"));
    assert_eq!(stream.scripted.asked()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_stream_ends_when_its_session_expires() -> TestResult {
    let stream = Stream::new(SHORT, vec![page(1, 1), page(99, 1)])?;
    let cookie = stream.session("stream-session-secret", FAR_FUTURE)?;
    let response = stream.open(&[("cookie", &cookie)]).await?;
    assert_eq!(response.status(), StatusCode::OK);
    // The store double keeps answering the record; only its expiry is now in the past.
    stream.session("stream-session-secret", LONG_PAST)?;
    let (seen, ended) = read(response, 2).await?;
    assert!(ended, "the stream outlived its session's expiry");
    assert!(!seen.contains("id: 99") && !seen.contains("id:99"));
    Ok(())
}

#[tokio::test]
async fn a_stream_ends_when_its_connector_is_revoked() -> TestResult {
    let stream = Stream::new(SHORT, vec![page(1, 1), page(99, 1)])?;
    let response = stream
        .open(&[("authorization", &format!("Bearer {SECRET}"))])
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    stream.base.credentials.add_connector(
        SECRET,
        CONNECTOR,
        vec![workspace(WORKSPACE)?],
        vec![Permission::Read],
        true,
    )?;
    let (seen, ended) = read(response, 2).await?;
    assert!(ended, "the stream outlived its connector");
    assert!(!seen.contains("id: 99") && !seen.contains("id:99"));
    Ok(())
}

#[tokio::test]
async fn a_hosted_stream_ends_when_its_connect_token_expires() -> TestResult {
    // The verifier says the token expires at second 1000; the service's clock stands one
    // millisecond before that.
    let clock = Arc::new(WallAt(AtomicU64::new(999_999)));
    let pages = vec![page(1, 1), page(99, 1), page(100, 1), page(101, 1)];
    let stream = Stream::hosted(SHORT, pages, 1_000, clock.clone())?;
    let bearer = format!("Bearer {CONNECT_TOKEN}");
    // While the token lives the stream delivers poll after poll.
    let response = stream.open(&[("authorization", &bearer)]).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let (seen, ended) = read(response, 2).await?;
    assert!(!ended);
    assert!(seen.contains("id: 99") || seen.contains("id:99"));

    // A second stream opens inside the token's life; the token expires before its second poll.
    let response = stream.open(&[("authorization", &bearer)]).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let polls = stream.scripted.asked()?.len();
    clock.0.store(1_000_000, Ordering::SeqCst);
    let (seen, ended) = read(response, 2).await?;
    assert!(ended, "the stream outlived its token's expiry");
    assert!(seen.contains("id: 100") || seen.contains("id:100"));
    assert!(!seen.contains("id: 101") && !seen.contains("id:101"));
    assert_eq!(stream.scripted.asked()?.len(), polls);
    Ok(())
}

#[tokio::test]
async fn a_credential_that_dies_ends_the_stream_before_the_poll_that_follows_a_full_page()
-> TestResult {
    // The interval is an hour: page two could only arrive by the poll that follows a full page
    // at once, and that poll must ask about the credential first.
    let stream = Stream::new(NEVER, vec![page(1, 100), page(101, 1)])?;
    let cookie = stream.session("stream-session-secret", FAR_FUTURE)?;
    let response = stream.open(&[("cookie", &cookie)]).await?;
    assert_eq!(response.status(), StatusCode::OK);
    stream
        .base
        .credentials
        .revoke_session(&session_id_of("stream-session-secret"))
        .await?;
    // The page fetched while the session lived is delivered; nothing is fetched after it.
    let (seen, ended) = read(response, 101).await?;
    assert!(ended, "the stream outlived its session");
    assert_eq!(seen.matches("\"kind\"").count(), 100);
    assert_eq!(stream.scripted.asked()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_credential_that_dies_during_the_wait_ends_the_stream_before_the_next_poll() -> TestResult
{
    let stream = Stream::new(SHORT, vec![page(1, 1), page(99, 1)])?;
    let cookie = stream.session("stream-session-secret", FAR_FUTURE)?;
    let response = stream.open(&[("cookie", &cookie)]).await?;
    let mut frames = response.into_body().into_data_stream();
    let first = some(frames.next().await, "the first event")??;
    assert!(String::from_utf8_lossy(&first).contains("\"kind\""));
    // One poll of the stream, and no more: it has nothing pending, so it begins its wait.
    let waiting = timeout(Duration::ZERO, frames.next()).await;
    assert!(
        waiting.is_err(),
        "nothing can arrive before the wait is over"
    );
    // The session ends while the stream waits.
    stream
        .base
        .credentials
        .revoke_session(&session_id_of("stream-session-secret"))
        .await?;
    let rest = async {
        let mut rest = String::new();
        while let Some(chunk) = frames.next().await {
            rest.push_str(&String::from_utf8_lossy(&chunk?));
        }
        Ok::<String, axum::Error>(rest)
    };
    // The credential is asked about after the wait, so the stream ends without polling again.
    let rest = timeout(Duration::from_secs(10), rest).await??;
    assert!(
        !rest.contains("\"kind\""),
        "delivered after the session ended: {rest}"
    );
    assert_eq!(stream.scripted.asked()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_poll_that_is_refused_ends_the_stream() -> TestResult {
    let stream = Stream::new(SHORT, vec![page(1, 1)])?;
    // The connector stays valid; the caller's grant is revoked after the first poll.
    stream.refuse_after(1)?;
    let response = stream
        .open(&[("authorization", &format!("Bearer {SECRET}"))])
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let (seen, ended) = read(response, 2).await?;
    assert!(ended, "the stream went on after its poll was refused");
    assert_eq!(seen.matches("\"kind\"").count(), 1);
    // The refused poll was the last: the stream does not ask again.
    assert_eq!(stream.scripted.asked()?.len(), 2);
    Ok(())
}

#[test]
fn an_event_stream_interval_of_zero_is_refused_where_it_is_built() -> TestResult {
    let base = LocalApp::new()?;
    let refused = some(
        EventStreams::new(base.bound(), base.identity(), Duration::ZERO).err(),
        "the refusal of a zero interval",
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("interval"));
    // The shortest interval that is not zero is accepted.
    assert!(EventStreams::new(base.bound(), base.identity(), Duration::from_nanos(1)).is_ok());
    Ok(())
}

#[tokio::test]
async fn a_stream_whose_credential_lives_keeps_delivering() -> TestResult {
    let stream = Stream::new(SHORT, vec![page(1, 1), page(99, 1)])?;
    let cookie = stream.session("stream-session-secret", FAR_FUTURE)?;
    let response = stream.open(&[("cookie", &cookie)]).await?;
    let (seen, ended) = read(response, 2).await?;
    assert!(!ended);
    assert!(seen.contains("id: 99") || seen.contains("id:99"));
    Ok(())
}

#[tokio::test]
async fn a_stream_the_caller_may_not_read_is_the_operations_refusal() -> TestResult {
    let stream = Stream::new(NEVER, vec![page(1, 1)])?;
    // The connector's ceiling names one workspace; this is another.
    let request = Request::get("/api/workspaces/22222222-2222-4222-8222-222222222222/events")
        .header("host", HOST)
        .header("authorization", format!("Bearer {SECRET}"))
        .body(Body::empty())?;
    let response = stream.app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ApiError = serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await?)?;
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(stream.scripted.asked()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn a_path_that_cannot_be_read_is_an_api_error_on_every_byte_and_event_route() -> TestResult {
    let stream = Stream::new(NEVER, Vec::new())?;
    let bytes = byte_service(Arc::new(Bytes::default()))?;
    let unreadable: [(&Router, &str, &str); 5] = [
        (&stream.app, "GET", "/api/workspaces/%FF/events"),
        (&bytes, "GET", "/api/artifacts/%FF"),
        (&bytes, "GET", "/api/workspaces/%FF/artifacts/%FF"),
        (&bytes, "PUT", "/api/workspaces/%FF/uploads/%FF/content"),
        (
            &bytes,
            "GET",
            "/api/workspaces/%FF/items/%FF/revisions/%FF/objects/%FF",
        ),
    ];
    for (app, method, path) in unreadable {
        let response = app
            .clone()
            .oneshot(request(method, path, true, None, Body::empty())?)
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{method} {path}"
        );
        assert_eq!(
            header(&response, "content-type").as_deref(),
            Some("application/json"),
            "{method} {path}"
        );
        let error: ApiError =
            serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await?)?;
        assert_eq!(error.code, ErrorCode::InvalidInput);
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
mod support;
