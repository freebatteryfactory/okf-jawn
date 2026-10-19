//! Local browser sessions end to end over the real service: the launch-token exchange, the
//! cookie path's `LocalOwner` route, CSRF and origin refusals, and logout.
#![cfg(feature = "runtime")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use okf_jawn_contract::access::{AccessRoute, CSRF_HEADER, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{Event, EventActor, EventKind, ListEventsResponse};
use okf_jawn_contract::identity::{MutationId, TenantId, Timestamp};
use okf_jawn_core::credentials::{CredentialStore, SessionRecord};
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_server::grants::SystemClock;
use okf_jawn_server::identity::{csrf_token_of, session_id_of};
use okf_jawn_server::local::{LAUNCH_TOKEN_LIFETIME, LaunchTokens, SecretSource};
use okf_jawn_server::sessions::{SessionEntry, SessionParts, Sessions, routes};
use serde_json::json;
use tower::ServiceExt;

use check::{TestResult, some};
use fixtures::{FixturePorts, GrantTable};
use support::{HOST, INSTANT, LocalApp, ORIGIN, OWNER, WORKSPACE};

/// Secrets handed out in order.
struct Sequence(Mutex<Vec<&'static str>>);

/// An event log that refuses to append while `failing` is set.
struct Flaky {
    failing: AtomicBool,
    inner: Arc<fixtures::FixtureEvents>,
}

/// The service, its stores and a launch token.
struct Local {
    app: Router,
    base: LocalApp,
    token: String,
    tokens: Arc<LaunchTokens>,
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

impl EventLog for Flaky {
    fn append<'a>(
        &'a self,
        scope: &'a EventScope,
        mutation_id: Option<MutationId>,
        event: NewEvent,
    ) -> PortFuture<'a, Event> {
        Box::pin(async move {
            if self.failing.load(Ordering::SeqCst) {
                return Err(ApiError::new(
                    ErrorCode::Unavailable,
                    "the event log is down",
                ));
            }
            self.inner.append(scope, mutation_id, event).await
        })
    }

    fn list<'a>(
        &'a self,
        scope: &'a EventScope,
        query: EventQuery,
    ) -> PortFuture<'a, ListEventsResponse> {
        self.inner.list(scope, query)
    }
}

impl Local {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_events(None)
    }

    /// The service whose sign-ins are recorded in `events` (the fixture log when `None`).
    fn with_events(events: Option<Arc<dyn EventLog>>) -> Result<Self, Box<dyn std::error::Error>> {
        let base = LocalApp::new()?;
        base.counting.set_response(
            "list_drafts",
            json!({ "revision": "a".repeat(40), "items": [] }),
        )?;
        let tokens = Arc::new(LaunchTokens::new(
            LAUNCH_TOKEN_LIFETIME,
            Arc::new(SystemClock),
        ));
        // One secret more than one sign-in needs, so a second sign-in would show.
        let secrets = Arc::new(Sequence(Mutex::new(vec![
            "second-session-secret",
            SESSION_SECRET,
            "launch-token",
        ])));
        let token = tokens.issue(secrets.as_ref())?;
        // One identity middleware: the session routes write the cookie names it reads.
        let identity = base.identity();
        let sessions = Sessions::new(SessionParts {
            credentials: base.credentials.clone(),
            events: events.unwrap_or_else(|| base.ports.events.clone()),
            secrets,
            policy: identity.cookie_policy(Duration::from_secs(3600)),
            entry: SessionEntry::Local {
                tokens: tokens.clone(),
            },
        });
        let extra = routes(sessions).merge(okf_jawn_server::schemas::routes());
        let app = base.service_behind(identity, extra, Vec::new(), None)?;
        Ok(Self {
            app,
            base,
            token,
            tokens,
        })
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
async fn only_a_completed_sign_in_tells_the_launch_link_to_publish_the_next() -> TestResult {
    let local = Local::new()?;
    let refused = Request::get("/auth/local?token=not-the-token")
        .header("host", HOST)
        .body(Body::empty())?;
    let response = local.app.clone().oneshot(refused).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), local.tokens.signed_in())
            .await
            .is_err(),
        "a refused exchange asked for a new launch link"
    );
    let response = local.launch().await?;
    assert_eq!(response.status(), StatusCode::FOUND);
    tokio::time::timeout(Duration::from_secs(10), local.tokens.signed_in()).await?;
    Ok(())
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
    // On loopback http the session cookie is `okf-session` (ruling 17): no `__Host-` cookie, which
    // a browser would refuse without `Secure`.
    assert!(
        !cookies.iter().any(|c| c.starts_with("__Host-okf-session=")),
        "{cookies:?}"
    );
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
    // Every field, written out: the session's principal as the actor, and nothing else.
    let expected = NewEvent {
        kind: EventKind::SignedIn,
        revision: None,
        item_id: None,
        job_id: None,
        connector_id: None,
        actor: Some(EventActor {
            subject: OWNER.to_owned(),
            route: AccessRoute::LocalOwner,
            client_id: None,
        }),
        operation: None,
        transport: None,
        proposal_id: None,
        count: None,
    };
    assert_eq!(event, &expected);
    assert_eq!(appended.len(), 1, "one event for one sign-in");
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

/// The attributes of a `Set-Cookie` value, sorted, without its `name=value`.
fn attributes(cookie: &str) -> Vec<&str> {
    let mut attributes: Vec<&str> = cookie.split("; ").skip(1).collect();
    attributes.sort_unstable();
    attributes
}

#[tokio::test]
async fn the_session_cookies_carry_exactly_their_attributes() -> TestResult {
    let local = Local::new()?;
    let response = local.launch().await?;
    let cookies = set_cookies(&response);
    let session = some(
        cookies.iter().find(|c| c.starts_with("okf-session=")),
        "the session cookie",
    )?;
    // `Path=/` exactly, the lifetime as `Max-Age`, and no `Domain`; not `Secure` over http.
    assert_eq!(
        attributes(session),
        vec!["HttpOnly", "Max-Age=3600", "Path=/", "SameSite=Strict"]
    );
    let csrf = some(
        cookies.iter().find(|c| c.starts_with("okf-csrf=")),
        "the anti-CSRF cookie",
    )?;
    assert_eq!(
        attributes(csrf),
        vec!["Max-Age=3600", "Path=/", "SameSite=Strict"]
    );
    Ok(())
}

#[tokio::test]
async fn logout_clears_both_cookies_on_the_path_they_were_set_on() -> TestResult {
    let local = Local::new()?;
    local.launch().await?;
    let csrf = csrf_token_of(SESSION_SECRET);
    let request = Request::post("/auth/logout")
        .header("host", HOST)
        .header("cookie", cookie())
        .header(CSRF_HEADER, csrf.as_str())
        .body(Body::empty())?;
    let response = local.app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let cookies = set_cookies(&response);
    for (name, http_only) in [("okf-session", true), ("okf-csrf", false)] {
        let cleared = some(
            cookies.iter().find(|c| c.starts_with(&format!("{name}=;"))),
            "a clearing cookie",
        )?;
        let attributes = attributes(cleared);
        assert!(attributes.contains(&"Path=/"), "{cleared}");
        assert!(attributes.contains(&"Max-Age=0"), "{cleared}");
        assert!(attributes.contains(&"SameSite=Strict"), "{cleared}");
        assert_eq!(attributes.contains(&"HttpOnly"), http_only, "{cleared}");
        assert!(!attributes.contains(&"Secure"), "not over http: {cleared}");
    }
    // On loopback http only the plain names are cleared (review N10).
    assert!(
        !cookies.iter().any(|c| c.starts_with("__Host-")),
        "{cookies:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_origin_that_only_begins_like_the_services_own_is_refused() -> TestResult {
    let local = Local::new()?;
    local.launch().await?;
    let csrf = csrf_token_of(SESSION_SECRET);
    for origin in [
        "http://127.0.0.1:7711.attacker.example",
        "http://127.0.0.1:77110",
        "http://127.0.0.1:7711/",
        "http://127.0.0.1:771",
        "HTTP://127.0.0.1:7711",
        "null",
    ] {
        let response = local
            .list_drafts(Some(&cookie()), Some(&csrf), Some(origin))
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin}");
    }
    assert_eq!(local.base.counting.call_count("list_drafts")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_cookie_put_patch_or_delete_without_the_token_is_refused() -> TestResult {
    let local = Local::new()?;
    local.launch().await?;
    for method in ["PUT", "PATCH", "DELETE", "POST"] {
        let send = |token: Option<String>| {
            let mut builder = Request::builder()
                .method(method)
                .uri("/api/schemas/view")
                .header("host", HOST)
                .header("cookie", cookie());
            if let Some(token) = token {
                builder = builder.header(CSRF_HEADER, token);
            }
            builder.body(Body::empty())
        };
        let refused = local.app.clone().oneshot(send(None)?).await?;
        assert_eq!(
            refused.status(),
            StatusCode::FORBIDDEN,
            "{method} without the token"
        );
        // With the token the request passes the middleware: no route takes the method.
        let passed = local
            .app
            .clone()
            .oneshot(send(Some(csrf_token_of(SESSION_SECRET)))?)
            .await?;
        assert_eq!(
            passed.status(),
            StatusCode::NOT_FOUND,
            "{method} with the token"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_sign_in_that_cannot_be_recorded_is_undone_and_the_launch_link_still_works() -> TestResult
{
    let events = Arc::new(Flaky {
        failing: AtomicBool::new(true),
        inner: FixturePorts::new(GrantTable::default()).events,
    });
    let log: Arc<dyn EventLog> = events.clone();
    let local = Local::with_events(Some(log))?;
    let failed = local.launch().await?;
    assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(set_cookies(&failed), Vec::<String>::new());
    assert_eq!(
        failed
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    // The session that was stored is revoked: nothing authenticates with its secret.
    assert_eq!(
        local.base.credentials.revoked_sessions()?,
        vec![session_id_of(SESSION_SECRET)]
    );
    assert!(
        local
            .base
            .credentials
            .get_session(&session_id_of(SESSION_SECRET))
            .await?
            .is_none()
    );
    // The token was given back: once the log is up, the same link signs in.
    events.failing.store(false, Ordering::SeqCst);
    assert_eq!(local.launch().await?.status(), StatusCode::FOUND);
    // And is then spent.
    assert_eq!(local.launch().await?.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn a_head_of_the_launch_link_does_not_take_the_token() -> TestResult {
    let local = Local::new()?;
    let head = Request::head(format!("/auth/local?token={}", local.token))
        .header("host", HOST)
        .body(Body::empty())?;
    let response = local.app.clone().oneshot(head).await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(set_cookies(&response), Vec::<String>::new());
    assert_eq!(local.launch().await?.status(), StatusCode::FOUND);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eight_exchanges_of_one_launch_token_at_once_open_one_session() -> TestResult {
    for round in 0..40 {
        let local = Arc::new(Local::new()?);
        let barrier = Arc::new(tokio::sync::Barrier::new(8));
        let mut exchanges = Vec::new();
        for _ in 0..8 {
            let local = local.clone();
            let barrier = barrier.clone();
            exchanges.push(tokio::spawn(async move {
                barrier.wait().await;
                let request = Request::get(format!("/auth/local?token={}", local.token))
                    .header("host", HOST)
                    .body(Body::empty())
                    .map_err(|error| error.to_string())?;
                local
                    .app
                    .clone()
                    .oneshot(request)
                    .await
                    .map(|response| response.status())
                    .map_err(|error| error.to_string())
            }));
        }
        let mut opened = 0_u32;
        for exchange in exchanges {
            let status = exchange.await??;
            if status == StatusCode::FOUND {
                opened = opened.saturating_add(1);
            } else {
                assert_eq!(status, StatusCode::UNAUTHORIZED);
            }
        }
        assert_eq!(opened, 1, "round {round}");
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
mod support;
