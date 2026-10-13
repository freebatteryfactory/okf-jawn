//! The identity middleware over the real service: each entry path gets its route, a bearer never
//! carries `BrowserSession` or `LocalOwner`, a connector never reaches drafts, and a `Principal`
//! from anywhere but the middleware is removed.
#![cfg(feature = "runtime")]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::{Extension, Router};
use okf_jawn_contract::access::{AccessRoute, CSRF_HEADER, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{TenantId, Timestamp};
use okf_jawn_core::credentials::{CredentialStore, SessionRecord, secret_hash};
use okf_jawn_server::BoundApplication;
use okf_jawn_server::app::{ServiceParts, service};
use okf_jawn_server::cookies::CrateCookies;
use okf_jawn_server::identity::{
    BearerAuth, ConnectIdentity, ConnectVerifier, Credential, Identity, IdentityPorts, Unverified,
    Verifying, WallClock, csrf_token_of, session_id_of,
};
use serde_json::{Value, json};
use tower::ServiceExt;

use check::{TestResult, some};
use fixtures::counting::CountingApplication;
use fixtures::{FixturePorts, GrantTable, tenant, workspace};
use support::{FakeCredentials, INSTANT, OWNER, OneToken};

/// A wall clock a test sets, in milliseconds since the Unix epoch.
struct WallAt(AtomicU64);

/// A Connect verifier that can judge no token now; it may say after how many seconds it can.
struct NotYet(Option<u32>);

/// What one test service is built over.
struct Harness {
    counting: Arc<CountingApplication>,
    credentials: Arc<FakeCredentials>,
    ports: FixturePorts,
}

const HOST: &str = "127.0.0.1:7711";
const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const CONNECTOR: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const SECRET: &str = "connector-secret-of-the-test";
const LIST_ITEMS: &str = "/api/items/list-items";
const LIST_DRAFTS: &str = "/api/items/list-drafts";
const FAR_FUTURE: &str = "2999-01-01T00:00:00.000Z";

impl ConnectVerifier for NotYet {
    fn verify<'a>(&'a self, _token: &'a str) -> Verifying<'a> {
        Box::pin(async move {
            Err(Unverified {
                error: ApiError::new(ErrorCode::Unavailable, "the issuer's keys are unavailable"),
                retry_after: self.0,
            })
        })
    }
}

impl WallClock for WallAt {
    fn unix_millis(&self) -> u128 {
        u128::from(self.0.load(Ordering::SeqCst))
    }
}

impl Harness {
    /// The owner holds every permission on the workspace, so only the route can refuse drafts.
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mut table = GrantTable::default();
        table
            .workspaces
            .entry(OWNER.to_owned())
            .or_default()
            .insert(workspace(WORKSPACE)?, fixtures::all_permissions());
        let counting = Arc::new(CountingApplication::new());
        counting.set_response("list_items", listing())?;
        counting.set_response("list_drafts", json!({ "items": [] }))?;
        Ok(Self {
            counting,
            credentials: Arc::new(FakeCredentials::default()),
            ports: FixturePorts::new(table),
        })
    }

    fn ports(&self) -> IdentityPorts {
        IdentityPorts {
            credentials: self.credentials.clone(),
            events: self.ports.events.clone(),
            cookies: Arc::new(CrateCookies),
        }
    }

    /// The identity state reading the wall-clock time from `clock`.
    fn identity_at(&self, bearer: BearerAuth, clock: Arc<WallAt>) -> Identity {
        Identity::with_clock(bearer, self.ports(), vec![format!("http://{HOST}")], clock)
    }

    fn service(&self, bearer: BearerAuth) -> Result<Router, Box<dyn std::error::Error>> {
        self.serve(
            Identity::new(bearer, self.ports(), vec![format!("http://{HOST}")]),
            Router::new(),
        )
    }

    /// The service whose session expiry is compared with `clock`.
    fn service_at(
        &self,
        bearer: BearerAuth,
        clock: Arc<WallAt>,
    ) -> Result<Router, Box<dyn std::error::Error>> {
        self.serve(self.identity_at(bearer, clock), Router::new())
    }

    /// The service with `extra` routes beside the operations.
    fn service_with(
        &self,
        bearer: BearerAuth,
        extra: Router,
    ) -> Result<Router, Box<dyn std::error::Error>> {
        self.serve(
            Identity::new(bearer, self.ports(), vec![format!("http://{HOST}")]),
            extra,
        )
    }

    /// Store a session of the installation owner under `secret`'s id, expiring at `expires_at`.
    fn owner_session(
        &self,
        secret: &str,
        expires_at: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.credentials.add_session(SessionRecord {
            session_id: session_id_of(secret),
            principal: owner(AccessRoute::LocalOwner),
            expires_at: Timestamp::try_from(expires_at.to_owned())?,
        })?;
        Ok(())
    }

    fn serve(
        &self,
        identity: Identity,
        extra: Router,
    ) -> Result<Router, Box<dyn std::error::Error>> {
        Ok(service(ServiceParts {
            bound: BoundApplication {
                application: self.counting.clone(),
                access: self.ports.access.clone(),
                mutations: self.ports.mutations.clone(),
                events: self.ports.events.clone(),
            },
            identity,
            hosts: vec![HOST.to_owned()],
            hosted: None,

            cors_origins: Vec::new(),
            ui_dir: None,
            extra,
        })?)
    }

    fn local(&self) -> Result<Router, Box<dyn std::error::Error>> {
        self.service(BearerAuth::Local)
    }

    fn hosted(&self) -> Result<Router, Box<dyn std::error::Error>> {
        self.hosted_naming("https://okf.example.test/.well-known/oauth-protected-resource")
    }

    /// The hosted service whose 401 challenge names `resource_metadata`.
    fn hosted_naming(&self, resource_metadata: &str) -> Result<Router, Box<dyn std::error::Error>> {
        self.service(BearerAuth::Hosted {
            connect: Arc::new(OneToken {
                token: "connect-token".to_owned(),
                identity: ConnectIdentity {
                    subject: OWNER.to_owned(),
                    organization_id: "org_01".to_owned(),
                    client_id: "client_mcp".to_owned(),
                    expires_at: u64::MAX,
                },
            }),
            resource_metadata: resource_metadata.to_owned(),
        })
    }

    fn connector(&self, revoked: bool) -> Result<(), Box<dyn std::error::Error>> {
        self.credentials.add_connector(
            SECRET,
            CONNECTOR,
            vec![workspace(WORKSPACE)?],
            vec![Permission::Read],
            revoked,
        )?;
        Ok(())
    }

    /// The principal the application saw on its only call of `operation`.
    fn seen(&self, operation: &str) -> Result<Principal, Box<dyn std::error::Error>> {
        let contexts = self.counting.contexts(operation)?;
        assert_eq!(contexts.len(), 1);
        let context = some(contexts.first(), "the call's context")?;
        assert_eq!(context.session_id, None);
        Ok(context.principal.clone())
    }
}

/// The installation owner, arriving by `route`.
fn owner(route: AccessRoute) -> Principal {
    Principal {
        subject: OWNER.to_owned(),
        tenant_id: TenantId::local(),
        route,
        client_id: None,
        delegation: None,
    }
}

fn listing() -> Value {
    json!({ "revision": "a".repeat(40), "items": [], "folders": [] })
}

fn list_items_body() -> Value {
    json!({
        "workspace_id": WORKSPACE,
        "at": { "kind": "latest" },
        "folder": "",
        "page": { "limit": 10 }
    })
}

fn list_drafts_body() -> Value {
    json!({ "workspace_id": WORKSPACE, "page": { "limit": 10 } })
}

fn post(
    path: &str,
    body: &Value,
    bearer: Option<&str>,
) -> Result<Request<Body>, axum::http::Error> {
    let builder = Request::post(path)
        .header("host", HOST)
        .header("content-type", "application/json");
    let builder = match bearer {
        Some(token) => builder.header("authorization", format!("Bearer {token}")),
        None => builder,
    };
    builder.body(Body::from(body.to_string()))
}

async fn api_error(response: Response) -> Result<ApiError, Box<dyn std::error::Error>> {
    let bytes = to_bytes(response.into_body(), 1_048_576).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[tokio::test]
async fn a_connector_bearer_is_mcp_delegation_for_the_owner_through_that_client() -> TestResult {
    let harness = Harness::new()?;
    harness.connector(false)?;
    let request = post(LIST_ITEMS, &list_items_body(), Some(SECRET))?;
    let response = harness.local()?.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let principal = harness.seen("list_items")?;
    assert_eq!(principal.route, AccessRoute::McpDelegation);
    assert_eq!(principal.subject, OWNER);
    assert_eq!(principal.client_id.as_deref(), Some(CONNECTOR));
    assert_eq!(principal.tenant_id, TenantId::local());
    let ceiling = some(principal.delegation, "the connector's ceiling")?;
    assert_eq!(ceiling.permissions, vec![Permission::Read]);
    assert_eq!(ceiling.workspace_ids, Some(vec![workspace(WORKSPACE)?]));
    Ok(())
}

#[tokio::test]
async fn a_connector_bearer_never_reaches_drafts() -> TestResult {
    let harness = Harness::new()?;
    harness.connector(false)?;
    let request = post(LIST_DRAFTS, &list_drafts_body(), Some(SECRET))?;
    let response = harness.local()?.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(api_error(response).await?.code, ErrorCode::Forbidden);
    assert_eq!(harness.counting.call_count("list_drafts")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_connect_token_is_mcp_delegation_with_subject_tenant_and_client_apart() -> TestResult {
    let harness = Harness::new()?;
    let request = post(LIST_DRAFTS, &list_drafts_body(), Some("connect-token"))?;
    let response = harness.hosted()?.oneshot(request).await?;
    // A Connect token is an agent route too: drafts are refused whatever the grants.
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(harness.counting.call_count("list_drafts")?, 0);

    let request = post(LIST_ITEMS, &list_items_body(), Some("connect-token"))?;
    harness.hosted()?.oneshot(request).await?;
    let principal = harness.seen("list_items")?;
    assert_eq!(principal.route, AccessRoute::McpDelegation);
    assert_eq!(principal.subject, OWNER);
    assert_eq!(principal.tenant_id, tenant("org_01")?);
    assert_eq!(principal.client_id.as_deref(), Some("client_mcp"));
    Ok(())
}

#[tokio::test]
async fn an_unknown_or_revoked_bearer_is_401_and_runs_nothing() -> TestResult {
    let harness = Harness::new()?;
    harness.connector(true)?;
    for token in [SECRET, "never-issued"] {
        let request = post(LIST_ITEMS, &list_items_body(), Some(token))?;
        let response = harness.local()?.oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().contains_key("www-authenticate"));
        assert_eq!(api_error(response).await?.code, ErrorCode::Unauthenticated);
    }
    let request = post(
        LIST_ITEMS,
        &list_items_body(),
        Some("not-the-connect-token"),
    )?;
    let response = harness.hosted()?.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(harness.counting.call_count("list_items")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_token_that_cannot_be_judged_yet_is_a_503_with_retry_after_and_no_challenge() -> TestResult
{
    let harness = Harness::new()?;
    let hosted = |seconds: Option<u32>| BearerAuth::Hosted {
        connect: Arc::new(NotYet(seconds)),
        resource_metadata: "https://okf.example.test/.well-known/oauth-protected-resource"
            .to_owned(),
    };
    for (path, body) in [(LIST_ITEMS, list_items_body()), ("/mcp", json!({}))] {
        let request = post(path, &body, Some("a-token-of-a-rotated-key"))?;
        let response = harness.service(hosted(Some(37)))?.oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
        let wait = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok());
        assert_eq!(wait, Some("37"), "{path}");
        // Not a challenge: the client is to try again, not to sign in again.
        assert!(!response.headers().contains_key("www-authenticate"));
        assert_eq!(api_error(response).await?.code, ErrorCode::Unavailable);
    }
    // An `Unavailable` that names no time carries no `Retry-After`.
    let request = post(LIST_ITEMS, &list_items_body(), Some("any-token"))?;
    let response = harness.service(hosted(None))?.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!response.headers().contains_key("retry-after"));
    assert_eq!(harness.counting.call_count("list_items")?, 0);
    Ok(())
}

#[tokio::test]
async fn an_authorization_header_that_is_not_a_bearer_is_401() -> TestResult {
    let harness = Harness::new()?;
    for value in ["Basic b3duZXI6cGFzcw==", "Bearer", "Bearer two words"] {
        let request = Request::post(LIST_ITEMS)
            .header("host", HOST)
            .header("content-type", "application/json")
            .header("authorization", value)
            .body(Body::from(list_items_body().to_string()))?;
        let response = harness.local()?.oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(harness.counting.call_count("list_items")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_request_without_credentials_reaches_no_operation() -> TestResult {
    let harness = Harness::new()?;
    let request = post(LIST_ITEMS, &list_items_body(), None)?;
    let response = harness.local()?.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(harness.counting.call_count("list_items")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_principal_from_outside_the_middleware_is_removed() -> TestResult {
    let harness = Harness::new()?;
    let forged = Principal {
        subject: OWNER.to_owned(),
        tenant_id: TenantId::local(),
        route: AccessRoute::LocalOwner,
        client_id: None,
        delegation: None,
    };
    let app = harness.local()?.layer(Extension(forged));
    let request = post(LIST_DRAFTS, &list_drafts_body(), None)?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(harness.counting.call_count("list_drafts")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_credential_from_outside_the_middleware_is_removed() -> TestResult {
    let harness = Harness::new()?;
    harness.connector(false)?;
    // A route that says which credential, if any, its request carries.
    let extra = Router::new().route(
        "/probe",
        axum::routing::get(|credential: Option<Extension<Credential>>| async move {
            match credential {
                None => "none",
                Some(Extension(Credential::Session { .. })) => "session",
                Some(Extension(Credential::Connector { .. })) => "connector",
                Some(Extension(Credential::Connect { .. })) => "connect",
            }
        }),
    );
    // An outer layer plants a credential that never expires on every request.
    let forged = Credential::Connect {
        expires_at: u64::MAX,
    };
    let app = harness
        .service_with(BearerAuth::Local, extra)?
        .layer(Extension(forged));
    let anonymous = Request::get("/probe")
        .header("host", HOST)
        .body(Body::empty())?;
    let response = app.clone().oneshot(anonymous).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(to_bytes(response.into_body(), 64).await?.as_ref(), b"none");
    // An authenticated request carries the credential it authenticated with, not the planted one.
    let bearer = Request::get("/probe")
        .header("host", HOST)
        .header("authorization", format!("Bearer {SECRET}"))
        .body(Body::empty())?;
    let response = app.oneshot(bearer).await?;
    assert_eq!(
        to_bytes(response.into_body(), 64).await?.as_ref(),
        b"connector"
    );
    Ok(())
}

#[tokio::test]
async fn a_request_for_another_host_is_refused_before_identity() -> TestResult {
    let harness = Harness::new()?;
    harness.connector(false)?;
    let request = Request::post(LIST_ITEMS)
        .header("host", "attacker.example:7711")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {SECRET}"))
        .body(Body::from(list_items_body().to_string()))?;
    let response = harness.local()?.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(harness.counting.call_count("list_items")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_challenge_that_cannot_be_written_is_a_500_never_a_401_without_one() -> TestResult {
    let harness = Harness::new()?;
    // A metadata URL no header can carry (a line break from a badly read setting).
    let app = harness.hosted_naming("https://okf.example.test/metadata\r\n")?;
    let request = Request::post("/mcp")
        .header("host", HOST)
        .header("content-type", "application/json")
        .body(Body::from("{}"))?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(api_error(response).await?.code, ErrorCode::Internal);
    Ok(())
}

#[tokio::test]
async fn the_mcp_path_without_a_bearer_is_a_401_challenge() -> TestResult {
    let harness = Harness::new()?;
    let request = Request::post("/mcp")
        .header("host", HOST)
        .header("content-type", "application/json")
        .body(Body::from("{}"))?;
    let response = harness.hosted()?.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let challenge = some(
        response
            .headers()
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        "the challenge",
    )?;
    assert!(challenge.contains("resource_metadata="));
    Ok(())
}

/// The instant the test sessions expire, in milliseconds since the Unix epoch.
fn expiry_ms() -> Result<u64, Box<dyn std::error::Error>> {
    let nanos = Timestamp::try_from(INSTANT.to_owned())?
        .instant()?
        .unix_timestamp_nanos();
    Ok(u64::try_from(
        nanos
            .checked_div(1_000_000)
            .ok_or("a division by a constant")?,
    )?)
}

/// A draft listing sent with the owner's session cookie and its token.
fn cookie_request(secret: &str) -> Result<Request<Body>, axum::http::Error> {
    Request::post(LIST_DRAFTS)
        .header("host", HOST)
        .header("content-type", "application/json")
        .header("cookie", format!("okf-session={secret}"))
        .header(CSRF_HEADER, csrf_token_of(secret))
        .body(Body::from(list_drafts_body().to_string()))
}

#[tokio::test]
async fn a_session_is_dead_from_its_expiry_on_whatever_the_store_answers() -> TestResult {
    let harness = Harness::new()?;
    // The store double answers this session whatever the time: only the server's check ends it.
    harness.owner_session("session-secret", INSTANT)?;
    let clock = Arc::new(WallAt(AtomicU64::new(0)));
    let app = harness.service_at(BearerAuth::Local, clock.clone())?;
    let expiry = expiry_ms()?;
    for (now, status) in [
        (expiry.saturating_sub(1), StatusCode::OK),
        (expiry, StatusCode::UNAUTHORIZED),
        (expiry.saturating_add(1), StatusCode::UNAUTHORIZED),
    ] {
        clock.0.store(now, Ordering::SeqCst);
        let response = app
            .clone()
            .oneshot(cookie_request("session-secret")?)
            .await?;
        assert_eq!(response.status(), status, "at {now}, expiry {expiry}");
    }
    assert_eq!(harness.counting.call_count("list_drafts")?, 1);
    Ok(())
}

#[tokio::test]
async fn a_stored_session_under_another_id_authenticates_nothing() -> TestResult {
    let harness = Harness::new()?;
    // The store answers the cookie's id with a record that names another session.
    harness.credentials.add_session_under(
        &session_id_of("session-secret"),
        SessionRecord {
            session_id: session_id_of("another-secret"),
            principal: owner(AccessRoute::LocalOwner),
            expires_at: Timestamp::try_from(FAR_FUTURE.to_owned())?,
        },
    )?;
    let response = harness
        .local()?
        .oneshot(cookie_request("session-secret")?)
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(harness.counting.call_count("list_drafts")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_valid_owner_session_with_its_token_never_reaches_the_mcp_path() -> TestResult {
    let harness = Harness::new()?;
    harness.owner_session("session-secret", FAR_FUTURE)?;
    let reached = Arc::new(AtomicU64::new(0));
    let counter = reached.clone();
    // A handler mounted where the MCP service goes: it must never run for a cookie.
    let extra = Router::new().route(
        "/mcp",
        axum::routing::any(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                "reached"
            }
        }),
    );
    let app = harness.service_with(BearerAuth::Local, extra)?;
    // The session is live: the same cookie lists drafts.
    let response = app
        .clone()
        .oneshot(cookie_request("session-secret")?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    for path in ["/mcp", "/mcp?x=1"] {
        for method in ["POST", "GET", "DELETE"] {
            let request = Request::builder()
                .method(method)
                .uri(path)
                .header("host", HOST)
                .header("cookie", "okf-session=session-secret")
                .header(CSRF_HEADER, csrf_token_of("session-secret"))
                .body(Body::empty())?;
            let response = app.clone().oneshot(request).await?;
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path}"
            );
        }
    }
    assert_eq!(reached.load(Ordering::SeqCst), 0);
    // A connector bearer does reach it, cookie or not.
    harness.connector(false)?;
    let request = Request::post("/mcp")
        .header("host", HOST)
        .header("cookie", "okf-session=session-secret")
        .header("authorization", format!("Bearer {SECRET}"))
        .body(Body::empty())?;
    assert_eq!(app.oneshot(request).await?.status(), StatusCode::OK);
    assert_eq!(reached.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_credential_authenticates_only_while_it_would_authenticate_a_new_request() -> TestResult {
    let harness = Harness::new()?;
    let clock = Arc::new(WallAt(AtomicU64::new(0)));
    let identity = harness.identity_at(BearerAuth::Local, clock.clone());
    // A Connect token, by its expiry in seconds.
    let token = Credential::Connect { expires_at: 1_000 };
    clock.0.store(999_999, Ordering::SeqCst);
    assert!(identity.still_authenticates(&token).await);
    clock.0.store(1_000_000, Ordering::SeqCst);
    assert!(!identity.still_authenticates(&token).await);

    // A session, by its id: while stored and unexpired.
    harness.owner_session("session-secret", INSTANT)?;
    let session = Credential::Session {
        session_id: session_id_of("session-secret"),
    };
    let expiry = expiry_ms()?;
    clock.0.store(expiry.saturating_sub(1), Ordering::SeqCst);
    assert!(identity.still_authenticates(&session).await);
    clock.0.store(expiry, Ordering::SeqCst);
    assert!(!identity.still_authenticates(&session).await);
    clock.0.store(expiry.saturating_sub(1), Ordering::SeqCst);
    harness
        .credentials
        .revoke_session(&session_id_of("session-secret"))
        .await?;
    assert!(!identity.still_authenticates(&session).await);

    // A connector, by its secret's hash and its id: while unrevoked and the same connector.
    harness.connector(false)?;
    let connector = Credential::Connector {
        secret_hash: secret_hash(SECRET),
        connector_id: CONNECTOR.to_owned(),
    };
    assert!(identity.still_authenticates(&connector).await);
    let other = Credential::Connector {
        secret_hash: secret_hash(SECRET),
        connector_id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd".to_owned(),
    };
    assert!(!identity.still_authenticates(&other).await);
    harness.connector(true)?;
    assert!(!identity.still_authenticates(&connector).await);
    Ok(())
}

#[tokio::test]
async fn a_credential_the_store_cannot_confirm_does_not_authenticate() -> TestResult {
    let harness = Harness::new()?;
    let clock = Arc::new(WallAt(AtomicU64::new(0)));
    let identity = harness.identity_at(BearerAuth::Local, clock);
    harness.owner_session("session-secret", FAR_FUTURE)?;
    harness.connector(false)?;
    let session = Credential::Session {
        session_id: session_id_of("session-secret"),
    };
    let connector = Credential::Connector {
        secret_hash: secret_hash(SECRET),
        connector_id: CONNECTOR.to_owned(),
    };
    for credential in [&session, &connector] {
        assert!(identity.still_authenticates(credential).await);
        // The store is down: the credential cannot be confirmed, so the answer is no.
        harness.credentials.fail_lookups(true);
        assert!(!identity.still_authenticates(credential).await);
        harness.credentials.fail_lookups(false);
        assert!(identity.still_authenticates(credential).await);
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
mod support;
