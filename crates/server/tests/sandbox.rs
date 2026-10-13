//! The sandbox origin: a capability serves exactly its bound object, only on the sandbox host,
//! with isolation headers on every response and never a cookie.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, Revision};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::sandbox::{SandboxCapabilityStore, SandboxMint, SandboxResolved, token_hash};
use okf_jawn_core::storage::{
    BlobStore, ByteReader, LocalSource, ObjectInfo, ObjectRead, StorageScope,
};
use okf_jawn_server::sandbox::{
    SANDBOX_MAX_BYTES, SandboxParts, isolated, isolation_headers, service,
};
use tower::ServiceExt;

use check::{TestResult, err_of, some};
use fixtures::{tenant, workspace};

/// Capabilities by token hash.
#[derive(Default)]
struct Capabilities(Mutex<BTreeMap<[u8; 32], SandboxResolved>>);

/// Objects by digest, each with the size `open` reports.
#[derive(Default)]
struct Blobs(Mutex<BTreeMap<String, (Vec<u8>, u64)>>);

const SANDBOX_HOST: &str = "127.0.0.1:7712";
const APP_ORIGIN: &str = "http://127.0.0.1:7711";
const TOKEN: &str = "capability-token-of-the-test";
const HTML: &str = "<html><body><script>alert(1)</script>hostile</body></html>";

impl SandboxCapabilityStore for Capabilities {
    fn mint<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _hash: [u8; 32],
        _mint: SandboxMint,
    ) -> PortFuture<'a, ()> {
        Box::pin(async { Err(ApiError::new(ErrorCode::Internal, "not used")) })
    }

    fn resolve(&self, hash: [u8; 32]) -> PortFuture<'_, Option<SandboxResolved>> {
        Box::pin(async move {
            Ok(self
                .0
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))?
                .get(&hash)
                .cloned())
        })
    }
}

impl BlobStore for Blobs {
    fn put<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _body: ByteReader,
        _limit: u64,
        _expected: Option<Digest>,
    ) -> PortFuture<'a, ObjectInfo> {
        Box::pin(async { Err(ApiError::new(ErrorCode::Internal, "not used")) })
    }

    fn open<'a>(
        &'a self,
        _scope: &'a StorageScope,
        digest: &'a Digest,
        offset: u64,
        length: u64,
    ) -> PortFuture<'a, ObjectRead> {
        Box::pin(async move {
            let (bytes, size) = self
                .0
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))?
                .get(digest.as_str())
                .cloned()
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no object"))?;
            Ok(ObjectRead {
                object: ObjectInfo {
                    digest: digest.clone(),
                    size,
                },
                offset,
                length,
                body: Box::pin(Cursor::new(bytes)),
            })
        })
    }

    fn materialize<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _digest: &'a Digest,
    ) -> PortFuture<'a, LocalSource> {
        Box::pin(async { Err(ApiError::new(ErrorCode::Internal, "not used")) })
    }
}

fn digest(fill: char) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::try_from(fill.to_string().repeat(64))?)
}

/// The sandbox service with one capability bound to an HTML object of `size` reported bytes.
fn sandbox(size: u64) -> Result<Router, Box<dyn std::error::Error>> {
    sandbox_of(HTML.as_bytes().to_vec(), size, APP_ORIGIN)
}

/// The sandbox service, framed by `app_origin`, with one capability bound to `body`, whose size
/// the store reports as `size` (which a test may make wrong).
fn sandbox_of(
    body: Vec<u8>,
    size: u64,
    app_origin: &str,
) -> Result<Router, Box<dyn std::error::Error>> {
    let object = digest('d')?;
    let capabilities = Capabilities::default();
    capabilities.0.lock().map_err(|_| "poisoned")?.insert(
        token_hash(TOKEN),
        SandboxResolved {
            scope: StorageScope {
                tenant_id: tenant("local")?,
                workspace_id: workspace("11111111-1111-4111-8111-111111111111")?,
            },
            item_id: serde_json::from_value(serde_json::json!(
                "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
            ))?,
            revision: Revision::try_from("a".repeat(40))?,
            object: object.clone(),
            media_type: "text/html".to_owned(),
        },
    );
    let blobs = Blobs::default();
    blobs
        .0
        .lock()
        .map_err(|_| "poisoned")?
        .insert(object.as_str().to_owned(), (body, size));
    Ok(service(SandboxParts {
        capabilities: Arc::new(capabilities),
        blobs: Arc::new(blobs),
        host: SANDBOX_HOST.to_owned(),
        app_origin: app_origin.to_owned(),
    })?)
}

fn get(path: &str, host: &str) -> Result<Request<Body>, axum::http::Error> {
    Request::get(path)
        .header("host", host)
        .header("cookie", "okf-session=ambient-session-secret")
        .body(Body::empty())
}

/// Every isolation header is present on `response`, and no cookie is set.
fn assert_isolated(response: &Response) -> TestResult {
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    // The whole policy, directive by directive: nothing may be added, widened or dropped.
    let policy = some(header("content-security-policy"), "the CSP")?;
    let directives: Vec<&str> = policy.split("; ").collect();
    assert_eq!(
        directives,
        vec![
            "sandbox",
            "default-src 'none'",
            "img-src data:",
            "style-src 'unsafe-inline'",
            "font-src data:",
            "media-src data:",
            "form-action 'none'",
            "base-uri 'none'",
            "frame-ancestors http://127.0.0.1:7711",
        ]
    );
    assert_eq!(header("cache-control").as_deref(), Some("no-store"));
    assert_eq!(header("referrer-policy").as_deref(), Some("no-referrer"));
    assert_eq!(header("x-content-type-options").as_deref(), Some("nosniff"));
    assert_eq!(
        header("permissions-policy").as_deref(),
        Some(
            "camera=(), microphone=(), geolocation=(), payment=(), usb=(), clipboard-read=(), clipboard-write=()"
        )
    );
    assert_eq!(
        header("cross-origin-resource-policy").as_deref(),
        Some("same-site")
    );
    assert!(header("set-cookie").is_none());
    Ok(())
}

#[tokio::test]
async fn a_capability_serves_its_bound_html_with_isolation_headers() -> TestResult {
    let response = sandbox(u64::try_from(HTML.len())?)?
        .oneshot(get(&format!("/sandbox/{TOKEN}"), SANDBOX_HOST)?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_isolated(&response)?;
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/html")
    );
    let bytes = to_bytes(response.into_body(), 1_048_576).await?;
    assert_eq!(bytes.as_ref(), HTML.as_bytes());
    Ok(())
}

#[tokio::test]
async fn an_unknown_capability_is_404_and_still_isolated() -> TestResult {
    let response = sandbox(u64::try_from(HTML.len())?)?
        .oneshot(get("/sandbox/guessed-token", SANDBOX_HOST)?)
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_isolated(&response)?;
    Ok(())
}

#[tokio::test]
async fn the_sandbox_answers_only_its_own_host() -> TestResult {
    let response = sandbox(u64::try_from(HTML.len())?)?
        .oneshot(get(&format!("/sandbox/{TOKEN}"), "127.0.0.1:7711")?)
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_isolated(&response)?;
    Ok(())
}

#[tokio::test]
async fn the_sandbox_serves_no_application_route() -> TestResult {
    let response = sandbox(u64::try_from(HTML.len())?)?
        .oneshot(get("/api/items/list-items", SANDBOX_HOST)?)
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_isolated(&response)?;
    Ok(())
}

#[tokio::test]
async fn a_representation_over_the_limit_is_refused() -> TestResult {
    let response = sandbox(SANDBOX_MAX_BYTES.saturating_add(1))?
        .oneshot(get(&format!("/sandbox/{TOKEN}"), SANDBOX_HOST)?)
        .await?;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_isolated(&response)?;
    Ok(())
}

#[tokio::test]
async fn a_cookie_set_inside_the_sandbox_never_leaves_it() -> TestResult {
    // A route double that sets a cookie: only the isolation layer can remove it.
    let inner = Router::new().route(
        "/sets-a-cookie",
        axum::routing::get(|| async { ([("set-cookie", "tracker=1; Path=/")], "page") }),
    );
    let response = isolated(inner, APP_ORIGIN)?
        .oneshot(get("/sets-a-cookie", SANDBOX_HOST)?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_isolated(&response)?;
    Ok(())
}

#[tokio::test]
async fn a_body_longer_than_the_store_reported_is_refused_at_the_limit() -> TestResult {
    let limit = usize::try_from(SANDBOX_MAX_BYTES)?;
    // Exactly the limit, honestly reported, is served whole.
    let response = sandbox_of(vec![b'a'; limit], SANDBOX_MAX_BYTES, APP_ORIGIN)?
        .oneshot(get(&format!("/sandbox/{TOKEN}"), SANDBOX_HOST)?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await?.len(),
        limit
    );
    // One byte more, while the store reports ten bytes: refused, not cut and served.
    let response = sandbox_of(vec![b'a'; limit.saturating_add(1)], 10, APP_ORIGIN)?
        .oneshot(get(&format!("/sandbox/{TOKEN}"), SANDBOX_HOST)?)
        .await?;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_isolated(&response)?;
    Ok(())
}

#[test]
fn the_service_does_not_start_with_an_application_origin_that_is_not_an_origin() -> TestResult {
    for origin in [
        "http://127.0.0.1:7711\r",
        "http://127.0.0.1:7711\n",
        "http://127.0.0.1:7711 ",
        "http://b\u{fc}cher.example.test",
        "http://127.0.0.1:7711/",
        "http://127.0.0.1:7711/path",
        "http://127.0.0.1:7711?q=1",
        "http://user@127.0.0.1:7711",
        "http://127.0.0.1:0",
        "http://:7711",
        "http://127.0.0.1:7711; script-src *",
        "ftp://127.0.0.1:7711",
        "HTTP://127.0.0.1:7711",
        "127.0.0.1:7711",
        "*",
        "",
        // A host is letters, digits, hyphens and dots, or a bracketed IPv6 literal.
        "http://a;b",
        "http://h;sandbox",
        "http://a,b",
        "http://*",
        "http://*.example.test",
        "http://a_b",
        "http://a'b",
        // A host is labels joined by dots: none empty, none beginning or ending with a hyphen.
        "http://.",
        "http://-",
        "http://---",
        "http://a..b",
        "http://.a",
        "http://a.",
        "http://example.test.",
        "http://-a.b",
        "http://a-.b",
        "http://a.-b",
        "http://a.b-",
        "http://",
        "http://[::1",
        "http://::1",
        "http://[zz]",
        "http://[]",
        "http://[::1]x",
        "http://[::1]]",
        "http://[::1%25eth0]",
        // A port is one to five digits and a real port.
        "http://h:",
        "http://[::1]:",
        "http://h:8a",
        "http://h:+80",
        "http://h:123456",
        "http://h:000080",
        "http://h:65536",
        "http://h:00000",
        "http://h:80:80",
        "https://",
        "http:/h",
        "http//h",
    ] {
        let refused = err_of(isolation_headers(origin))?;
        assert_eq!(refused.code, ErrorCode::InvalidInput, "{origin:?}");
        assert_eq!(refused.field.as_deref(), Some("app_origin"));
        assert!(sandbox_of(Vec::new(), 0, origin).is_err(), "{origin:?}");
    }
    for origin in [
        "http://127.0.0.1:7711",
        "https://okf.example.test",
        "http://[::1]:7711",
        "http://[2001:db8::1]",
        "https://a-b.example.test:443",
        "http://h:1",
        "http://h:65535",
        "http://a-b.c-d",
        "http://xn--bcher-kva.example",
        "http://1.2.3.4",
        "http://LOCALHOST:8080",
    ] {
        let headers = isolation_headers(origin)?;
        assert_eq!(headers.len(), 6, "{origin}");
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
