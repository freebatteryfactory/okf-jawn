//! The edge of the application origin: cross-origin protection, constrained CORS, the UI's static
//! assets, and hosted browser sign-in through a login double.
#![cfg(feature = "runtime")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::TenantId;
use okf_jawn_core::credentials::CredentialStore;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_server::cookies::CookiePolicy;
use okf_jawn_server::grants::SystemClock;
use okf_jawn_server::hosted::{HostedLogin, HostedUser, LoginStart, PendingLogins};
use okf_jawn_server::identity::session_id_of;
use okf_jawn_server::local::SecretSource;
use okf_jawn_server::sessions::{OsSecrets, SessionEntry, SessionParts, Sessions, routes};
use serde_json::json;
use tower::ServiceExt;

use check::{TestResult, some};
use support::{HOST, LocalApp, SECRET, WORKSPACE};

/// A login whose provider accepts one code.
struct OneCode;

/// A secret source with one fixed secret, so the test knows the session it opens.
struct Fixed;

/// A UI build directory holding an index page and one script, removed when dropped.
struct UiDir(PathBuf);

const LIST_ITEMS: &str = "/api/items/list-items";
const PARTNER: &str = "https://partner.example.test";
const SESSION_SECRET: &str = "hosted-session-secret";

impl HostedLogin for OneCode {
    fn begin(&self) -> Result<LoginStart, ApiError> {
        Ok(LoginStart {
            url: "https://auth.example.test/authorize?state=state-1".to_owned(),
            verifier: "verifier-1".to_owned(),
            state: "state-1".to_owned(),
        })
    }

    fn complete<'a>(&'a self, code: &'a str, verifier: &'a str) -> PortFuture<'a, HostedUser> {
        Box::pin(async move {
            if code == "code-1" && verifier == "verifier-1" {
                Ok(HostedUser {
                    subject: "user_01".to_owned(),
                    organization_id: "org_01".to_owned(),
                })
            } else {
                Err(ApiError::new(ErrorCode::Unauthenticated, "not accepted"))
            }
        })
    }
}

impl SecretSource for Fixed {
    fn secret(&self) -> Result<String, ApiError> {
        Ok(SESSION_SECRET.to_owned())
    }
}

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

#[tokio::test]
async fn a_hosted_sign_in_opens_a_browser_session_and_its_state_works_once() -> TestResult {
    let base = LocalApp::new()?;
    let sessions = Sessions::new(SessionParts {
        credentials: base.credentials.clone(),
        events: base.ports.events.clone(),
        secrets: Arc::new(Fixed),
        policy: CookiePolicy {
            secure: true,
            lifetime: Duration::from_secs(3600),
        },
        entry: SessionEntry::Hosted {
            login: Arc::new(OneCode),
            pending: Arc::new(PendingLogins::new(Arc::new(SystemClock))),
        },
    });
    let app = base.service(routes(sessions), Vec::new(), None)?;

    let begun = app.clone().oneshot(get("/auth/login")?).await?;
    assert_eq!(begun.status(), StatusCode::FOUND);
    assert_eq!(
        begun
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("https://auth.example.test/authorize?state=state-1")
    );

    // A callback with a state that was never started exchanges nothing.
    let forged = app
        .clone()
        .oneshot(get("/auth/callback?state=forged&code=code-1")?)
        .await?;
    assert_eq!(forged.status(), StatusCode::UNAUTHORIZED);

    let done = app
        .clone()
        .oneshot(get("/auth/callback?state=state-1&code=code-1")?)
        .await?;
    assert_eq!(done.status(), StatusCode::FOUND);
    let cookie = some(
        done.headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find(|value| value.starts_with("okf-session="))
            .map(str::to_owned),
        "the session cookie",
    )?;
    assert!(cookie.contains("Secure"));
    assert!(cookie.contains("HttpOnly"));
    let stored = some(
        base.credentials
            .get_session(&session_id_of(SESSION_SECRET))
            .await?,
        "the stored session",
    )?;
    assert_eq!(stored.principal.route, AccessRoute::BrowserSession);
    assert_eq!(stored.principal.subject, "user_01");
    assert_eq!(
        stored.principal.tenant_id,
        TenantId::try_from("org_01".to_owned())?
    );

    // The state was consumed: replaying the callback is refused.
    let replay = app
        .clone()
        .oneshot(get("/auth/callback?state=state-1&code=code-1")?)
        .await?;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);

    // Hosted mode has no launch-token exchange.
    let local_route = app.oneshot(get("/auth/local?token=anything")?).await?;
    assert_eq!(local_route.status(), StatusCode::NOT_FOUND);
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

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
mod support;
