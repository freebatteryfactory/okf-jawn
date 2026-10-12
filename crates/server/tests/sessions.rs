//! Local browser sessions end to end over the real service: the launch-token exchange, the
//! cookie path's `LocalOwner` route, CSRF and origin refusals, and logout.
#![cfg(feature = "runtime")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use okf_jawn_contract::access::{AccessRoute, CSRF_HEADER, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::{TenantId, Timestamp};
use okf_jawn_core::credentials::{CredentialStore, SessionRecord};
use okf_jawn_core::events::EventScope;
use okf_jawn_server::cookies::CookiePolicy;
use okf_jawn_server::identity::{csrf_token_of, session_id_of};
use okf_jawn_server::local::{LaunchTokens, SecretSource};
use okf_jawn_server::sessions::{SessionEntry, SessionParts, Sessions, routes};
use serde_json::json;
use tower::ServiceExt;

use check::{TestResult, some};
use support::{HOST, INSTANT, LocalApp, ORIGIN, OWNER, WORKSPACE};

/// Secrets handed out in order.
struct Sequence(Mutex<Vec<&'static str>>);

/// The service, its stores and a launch token.
struct Local {
    app: Router,
    base: LocalApp,
    token: String,
}

const SESSION_SECRET: &str = "session-secret-of-the-test";
const LIST_DRAFTS: &str = "/api/items/list-drafts";

impl SecretSource for Sequence {
    fn secret(&self) -> Result<String, ApiError> {
        self.0
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))?
            .pop()
            .map(str::to_owned)
            .ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "no secret left"))
    }
}

impl Local {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let base = LocalApp::new()?;
        base.counting
            .set_response("list_drafts", json!({ "items": [] }))?;
        let tokens = Arc::new(LaunchTokens::new());
        let secrets = Arc::new(Sequence(Mutex::new(vec![SESSION_SECRET, "launch-token"])));
        let token = tokens.issue(secrets.as_ref())?;
        let sessions = Sessions::new(SessionParts {
            credentials: base.credentials.clone(),
            events: base.ports.events.clone(),
            secrets,
            policy: CookiePolicy {
                secure: false,
                lifetime: Duration::from_secs(3600),
            },
            entry: SessionEntry::Local { tokens },
        });
        let extra = routes(sessions).merge(okf_jawn_server::schemas::routes());
        let app = base.service(extra, Vec::new(), None)?;
        Ok(Self { app, base, token })
    }

    async fn launch(&self) -> Result<Response, Box<dyn std::error::Error>> {
        let request = Request::get(format!("/auth/local?token={}", self.token))
            .header("host", HOST)
            .body(Body::empty())?;
        Ok(self.app.clone().oneshot(request).await?)
    }

    async fn list_drafts(
        &self,
        cookie: Option<&str>,
        csrf: Option<&str>,
        origin: Option<&str>,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let mut builder = Request::post(LIST_DRAFTS)
            .header("host", HOST)
            .header("content-type", "application/json");
        if let Some(cookie) = cookie {
            builder = builder.header("cookie", cookie);
        }
        if let Some(csrf) = csrf {
            builder = builder.header(CSRF_HEADER, csrf);
        }
        if let Some(origin) = origin {
            builder = builder.header("origin", origin);
        }
        let body = json!({ "workspace_id": WORKSPACE, "page": { "limit": 10 } });
        Ok(self
            .app
            .clone()
            .oneshot(builder.body(Body::from(body.to_string()))?)
            .await?)
    }
}

fn set_cookies(response: &Response) -> Vec<String> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok().map(str::to_owned))
        .collect()
}

fn cookie() -> String {
    format!("okf-session={SESSION_SECRET}")
}

#[tokio::test]
async fn the_launch_token_opens_a_strict_http_only_session_and_records_the_sign_in() -> TestResult {
    let local = Local::new()?;
    let response = local.launch().await?;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok()),
        Some("/")
    );
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let cookies = set_cookies(&response);
    let session = some(
        cookies.iter().find(|c| c.starts_with("okf-session=")),
        "the session cookie",
    )?;
    assert!(session.starts_with(&format!("okf-session={SESSION_SECRET};")));
    assert!(session.contains("HttpOnly"));
    assert!(session.contains("SameSite=Strict"));
    assert!(session.contains("Path=/"));
    let csrf = some(
        cookies.iter().find(|c| c.starts_with("okf-csrf=")),
        "the anti-CSRF cookie",
    )?;
    assert!(csrf.starts_with(&format!("okf-csrf={};", csrf_token_of(SESSION_SECRET))));
    assert!(!csrf.contains("HttpOnly"));
    assert!(csrf.contains("SameSite=Strict"));

    let stored = some(
        local
            .base
            .credentials
            .get_session(&session_id_of(SESSION_SECRET))
            .await?,
        "the stored session",
    )?;
    assert_eq!(stored.principal.route, AccessRoute::LocalOwner);
    assert_eq!(stored.principal.subject, OWNER);
    let appended = local.base.ports.events.appended()?;
    let (scope, event) = some(appended.first(), "the sign-in event")?;
    assert_eq!(scope, &EventScope::Tenant(TenantId::local()));
    assert_eq!(event.kind, EventKind::SignedIn);
    assert_eq!(
        event.actor.as_ref().map(|actor| actor.route.clone()),
        Some(AccessRoute::LocalOwner)
    );
    Ok(())
}

#[tokio::test]
async fn a_replayed_launch_token_is_refused() -> TestResult {
    let local = Local::new()?;
    assert_eq!(local.launch().await?.status(), StatusCode::FOUND);
    let replay = local.launch().await?;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(set_cookies(&replay), Vec::<String>::new());
    Ok(())
}

#[tokio::test]
async fn the_session_cookie_is_the_local_owner_and_reaches_drafts() -> TestResult {
    let local = Local::new()?;
    local.launch().await?;
    let csrf = csrf_token_of(SESSION_SECRET);
    let response = local
        .list_drafts(Some(&cookie()), Some(&csrf), Some(ORIGIN))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let contexts = local.base.counting.contexts("list_drafts")?;
    let context = some(contexts.first(), "the call's context")?;
    assert_eq!(context.principal.route, AccessRoute::LocalOwner);
    assert_eq!(
        context.session_id.as_deref(),
        Some(session_id_of(SESSION_SECRET).as_str())
    );
    Ok(())
}

#[tokio::test]
async fn a_cookie_request_without_the_csrf_token_is_refused_and_recorded() -> TestResult {
    let local = Local::new()?;
    local.launch().await?;
    for csrf in [None, Some("not-the-token")] {
        let response = local
            .list_drafts(Some(&cookie()), csrf, Some(ORIGIN))
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    assert_eq!(local.base.counting.call_count("list_drafts")?, 0);
    let refusals = local
        .base
        .ports
        .events
        .appended()?
        .into_iter()
        .filter(|(_, event)| event.kind == EventKind::PermissionDenied)
        .count();
    assert_eq!(refusals, 2);
    Ok(())
}

#[tokio::test]
async fn a_cookie_request_from_another_origin_is_refused() -> TestResult {
    let local = Local::new()?;
    local.launch().await?;
    let csrf = csrf_token_of(SESSION_SECRET);
    let response = local
        .list_drafts(Some(&cookie()), Some(&csrf), Some("http://127.0.0.1:7712"))
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(local.base.counting.call_count("list_drafts")?, 0);
    // The identity middleware refused it, as the session's owner, and recorded the refusal.
    let recorded = local
        .base
        .ports
        .events
        .appended()?
        .into_iter()
        .filter(|(_, event)| event.kind == EventKind::PermissionDenied)
        .count();
    assert_eq!(recorded, 1);
    Ok(())
}

#[tokio::test]
async fn a_cookie_read_from_another_origin_or_site_is_not_authenticated() -> TestResult {
    let local = Local::new()?;
    local.launch().await?;
    let read = |header: Option<(&'static str, &'static str)>| {
        let builder = Request::get("/api/schemas/view")
            .header("host", HOST)
            .header("cookie", cookie());
        let builder = match header {
            Some((name, value)) => builder.header(name, value),
            None => builder,
        };
        builder.body(Body::empty())
    };
    let same = local.app.clone().oneshot(read(None)?).await?;
    assert_eq!(same.status(), StatusCode::OK);
    for header in [
        ("origin", "http://127.0.0.1:7712"),
        ("sec-fetch-site", "cross-site"),
        ("sec-fetch-site", "same-site"),
    ] {
        let response = local.app.clone().oneshot(read(Some(header))?).await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

#[tokio::test]
async fn logout_ends_the_session_at_once() -> TestResult {
    let local = Local::new()?;
    local.launch().await?;
    let csrf = csrf_token_of(SESSION_SECRET);
    let request = Request::post("/auth/logout")
        .header("host", HOST)
        .header("cookie", cookie())
        .header(CSRF_HEADER, csrf.as_str())
        .header("origin", ORIGIN)
        .body(Body::empty())?;
    let response = local.app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        set_cookies(&response)
            .iter()
            .any(|c| c.starts_with("okf-session=;"))
    );
    assert_eq!(
        local.base.credentials.revoked_sessions()?,
        vec![session_id_of(SESSION_SECRET)]
    );
    let after = local
        .list_drafts(Some(&cookie()), Some(&csrf), Some(ORIGIN))
        .await?;
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn a_stored_session_of_another_route_authenticates_nothing() -> TestResult {
    let local = Local::new()?;
    local.base.credentials.add_session(SessionRecord {
        session_id: session_id_of(SESSION_SECRET),
        principal: Principal {
            subject: OWNER.to_owned(),
            tenant_id: TenantId::local(),
            route: AccessRoute::McpDelegation,
            client_id: None,
            delegation: None,
        },
        expires_at: Timestamp::try_from(INSTANT.to_owned())?,
    })?;
    let csrf = csrf_token_of(SESSION_SECRET);
    let response = local
        .list_drafts(Some(&cookie()), Some(&csrf), Some(ORIGIN))
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn two_session_cookies_of_one_name_are_read_as_none() -> TestResult {
    let local = Local::new()?;
    local.launch().await?;
    let csrf = csrf_token_of(SESSION_SECRET);
    let doubled = format!("{}; okf-session=tossed-by-a-sibling-site", cookie());
    let response = local
        .list_drafts(Some(&doubled), Some(&csrf), Some(ORIGIN))
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
mod support;
