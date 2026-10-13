//! The edge of the application origin: cross-origin protection, constrained CORS, the UI's static
//! assets, and the operating system's secret source.
#![cfg(feature = "runtime")]

use std::path::PathBuf;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_server::local::SecretSource;
use okf_jawn_server::sessions::OsSecrets;
use serde_json::json;
use tower::ServiceExt;

use check::TestResult;
use support::{HOST, LocalApp, SECRET, WORKSPACE};

/// A UI build directory holding an index page and one script, removed when dropped.
struct UiDir(PathBuf);

const LIST_ITEMS: &str = "/api/items/list-items";
const PARTNER: &str = "https://partner.example.test";

impl UiDir {
    fn new(name: &str) -> std::io::Result<Self> {
        let directory = std::env::temp_dir().join(format!(
            "okf-jawn-server-test-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join("index.html"),
            "<!doctype html><title>workspace</title>",
        )?;
        std::fs::write(directory.join("app.js"), "console.log('workspace');")?;
        Ok(Self(directory))
    }
}

impl Drop for UiDir {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.0);
    }
}

fn local(cors: Vec<String>, ui_dir: Option<PathBuf>) -> Result<Router, Box<dyn std::error::Error>> {
    let base = LocalApp::new()?;
    base.counting.set_response(
        "list_items",
        json!({ "revision": "a".repeat(40), "items": [], "folders": [] }),
    )?;
    Ok(base.service(Router::new(), cors, ui_dir)?)
}

fn list_items(headers: &[(&str, &str)]) -> Result<Request<Body>, axum::http::Error> {
    let mut builder = Request::post(LIST_ITEMS)
        .header("host", HOST)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {SECRET}"));
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let body = json!({
        "workspace_id": WORKSPACE,
        "at": { "kind": "latest" },
        "folder": "",
        "page": { "limit": 10 }
    });
    builder.body(Body::from(body.to_string()))
}

fn get(path: &str) -> Result<Request<Body>, axum::http::Error> {
    Request::get(path).header("host", HOST).body(Body::empty())
}

async fn text(response: Response) -> Result<String, Box<dyn std::error::Error>> {
    Ok(String::from_utf8(
        to_bytes(response.into_body(), 1_048_576).await?.to_vec(),
    )?)
}
#[tokio::test]
async fn a_cross_site_write_is_refused_in_api_error_json() -> TestResult {
    let app = local(Vec::new(), None)?;
    let same = app.clone().oneshot(list_items(&[])?).await?;
    assert_eq!(same.status(), StatusCode::OK);

    for headers in [
        [("sec-fetch-site", "cross-site")],
        [("origin", "https://attacker.example.test")],
    ] {
        let response = app.clone().oneshot(list_items(&headers)?).await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let error: ApiError = serde_json::from_str(&text(response).await?)?;
        assert_eq!(error.code, ErrorCode::Forbidden);
    }
    Ok(())
}

#[tokio::test]
async fn only_a_configured_origin_gets_cors_and_never_credentials() -> TestResult {
    let app = local(vec![PARTNER.to_owned()], None)?;
    let preflight = |origin: &'static str| {
        Request::builder()
            .method("OPTIONS")
            .uri(LIST_ITEMS)
            .header("host", HOST)
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "authorization,content-type",
            )
            .body(Body::empty())
    };
    let allowed = app.clone().oneshot(preflight(PARTNER)?).await?;
    assert_eq!(
        allowed
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some(PARTNER)
    );
    assert!(
        !allowed
            .headers()
            .contains_key("access-control-allow-credentials")
    );

    let refused = app
        .clone()
        .oneshot(preflight("https://attacker.example.test")?)
        .await?;
    assert!(
        !refused
            .headers()
            .contains_key("access-control-allow-origin")
    );

    // The configured origin may write with its bearer; no cookie is involved.
    let write = app
        .oneshot(list_items(&[
            ("origin", PARTNER),
            ("sec-fetch-site", "cross-site"),
        ])?)
        .await?;
    assert_eq!(write.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn the_ui_build_is_served_and_api_paths_stay_json() -> TestResult {
    let ui = UiDir::new("assets")?;
    let app = local(Vec::new(), Some(ui.0.clone()))?;

    let index = app.clone().oneshot(get("/")?).await?;
    assert_eq!(index.status(), StatusCode::OK);
    assert!(text(index).await?.contains("<title>workspace</title>"));

    let script = app.clone().oneshot(get("/app.js")?).await?;
    assert_eq!(script.status(), StatusCode::OK);
    assert!(text(script).await?.contains("console.log"));

    // A client-side route opens the workspace page.
    let deep = app.clone().oneshot(get("/w/some/client/route")?).await?;
    assert_eq!(deep.status(), StatusCode::OK);
    assert!(text(deep).await?.contains("<title>workspace</title>"));

    // An unknown API path, and a write to a page path, are JSON refusals, never a page.
    for request in [
        get("/api/no-such-thing")?,
        get("/auth/no-such-thing")?,
        Request::post("/w/some/client/route")
            .header("host", HOST)
            .body(Body::empty())?,
    ] {
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let error: ApiError = serde_json::from_str(&text(response).await?)?;
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    // Nothing outside the directory is served.
    let outside = app.oneshot(get("/..%2F..%2FCargo.toml")?).await?;
    assert!(!text(outside).await?.contains("[workspace]"));
    Ok(())
}

#[test]
fn the_operating_system_secret_source_yields_fresh_256_bit_secrets() -> TestResult {
    let first = OsSecrets.secret()?;
    let second = OsSecrets.secret()?;
    assert_eq!(first.len(), 64);
    assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_ne!(first, second);
    Ok(())
}

/// A read of the liveness route naming `host` in its `Host` header (none when `None`), with the
/// request target `target`.
fn health(target: &str, hosts: &[&str]) -> Result<Request<Body>, axum::http::Error> {
    let mut builder = Request::get(target);
    for host in hosts {
        builder = builder.header("host", *host);
    }
    builder.body(Body::empty())
}

#[tokio::test]
async fn a_request_that_names_no_host_is_refused() -> TestResult {
    let app = local(Vec::new(), None)?;
    let response = app.oneshot(health("/healthz", &[])?).await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn a_host_that_only_begins_or_ends_like_an_allowed_one_is_refused() -> TestResult {
    let app = local(Vec::new(), None)?;
    for host in [
        "127.0.0.1:7711.attacker.example",
        "127.0.0.1:77110",
        "127.0.0.1:771",
        "127.0.0.1",
        "attacker.example.127.0.0.1:7711",
        "",
    ] {
        let response = app.clone().oneshot(health("/healthz", &[host])?).await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{host:?}");
    }
    let exact = app.oneshot(health("/healthz", &[HOST])?).await?;
    assert_eq!(exact.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn two_host_headers_are_refused_whatever_they_name() -> TestResult {
    let app = local(Vec::new(), None)?;
    for hosts in [
        [HOST, "attacker.example:7711"],
        ["attacker.example:7711", HOST],
        [HOST, HOST],
    ] {
        let response = app.clone().oneshot(health("/healthz", &hosts)?).await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{hosts:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_request_targets_authority_must_be_an_allowed_host_too() -> TestResult {
    let app = local(Vec::new(), None)?;
    // An absolute-form target: a proxy may route on it, so it is held to the same list.
    let evil_target = health("http://attacker.example:7711/healthz", &[HOST])?;
    assert_eq!(
        app.clone().oneshot(evil_target).await?.status(),
        StatusCode::FORBIDDEN
    );
    let evil_host = health(
        &format!("http://{HOST}/healthz"),
        &["attacker.example:7711"],
    )?;
    assert_eq!(
        app.clone().oneshot(evil_host).await?.status(),
        StatusCode::FORBIDDEN
    );
    let both = health(&format!("http://{HOST}/healthz"), &[HOST])?;
    assert_eq!(app.clone().oneshot(both).await?.status(), StatusCode::OK);
    // HTTP/2 names the host only in the target's authority.
    let authority_only = health(&format!("http://{HOST}/healthz"), &[])?;
    assert_eq!(app.oneshot(authority_only).await?.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn no_refusal_quotes_the_method_and_every_refusal_is_under_one_kibibyte() -> TestResult {
    let app = local(Vec::new(), None)?;
    let method = "M".repeat(100_000);
    for path in ["/healthz", "/api/items/list-items", "/nothing/here"] {
        let request = Request::builder()
            .method(method.as_str())
            .uri(path)
            .header("host", HOST)
            .body(Body::empty())?;
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        let body = text(response).await?;
        assert!(body.len() < 1024, "{path}: {} bytes", body.len());
        assert!(!body.contains("MMMM"), "{path}");
        let error: ApiError = serde_json::from_str(&body)?;
        assert_eq!(error.code, ErrorCode::NotFound);
    }
    Ok(())
}

#[tokio::test]
async fn a_path_or_query_that_cannot_be_read_is_an_api_error_not_plain_text() -> TestResult {
    let base = LocalApp::new()?;
    let app = base.service(okf_jawn_server::schemas::routes(), Vec::new(), None)?;
    for target in ["/api/schemas/%FF", "/api/schemas/%C3%28"] {
        let request = Request::get(target)
            .header("host", HOST)
            .header("authorization", format!("Bearer {SECRET}"))
            .body(Body::empty())?;
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{target}");
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/json"),
            "{target}"
        );
        let error: ApiError = serde_json::from_str(&text(response).await?)?;
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert!(!error.message.contains("%FF"));
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
mod support;
