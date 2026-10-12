//! The identity middleware over the real service: each entry path gets its route, a bearer never
//! carries `BrowserSession` or `LocalOwner`, a connector never reaches drafts, and a `Principal`
//! from anywhere but the middleware is removed.
#![cfg(feature = "runtime")]

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::{Extension, Router};
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::TenantId;
use okf_jawn_server::BoundApplication;
use okf_jawn_server::app::{ServiceParts, service};
use okf_jawn_server::cookies::CrateCookies;
use okf_jawn_server::identity::{BearerAuth, ConnectIdentity, Identity, IdentityPorts};
use serde_json::{Value, json};
use tower::ServiceExt;

use check::{TestResult, some};
use fixtures::counting::CountingApplication;
use fixtures::{FixturePorts, GrantTable, tenant, workspace};
use support::{FakeCredentials, OWNER, OneToken};

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

    fn service(&self, bearer: BearerAuth) -> Result<Router, Box<dyn std::error::Error>> {
        let identity = Identity::new(
            bearer,
            IdentityPorts {
                credentials: self.credentials.clone(),
                events: self.ports.events.clone(),
                cookies: Arc::new(CrateCookies),
            },
            vec![format!("http://{HOST}")],
        );
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
            extra: Router::new(),
        })?)
    }

    fn local(&self) -> Result<Router, Box<dyn std::error::Error>> {
        self.service(BearerAuth::Local)
    }

    fn hosted(&self) -> Result<Router, Box<dyn std::error::Error>> {
        self.service(BearerAuth::Hosted {
            connect: Arc::new(OneToken {
                token: "connect-token".to_owned(),
                identity: ConnectIdentity {
                    subject: OWNER.to_owned(),
                    organization_id: "org_01".to_owned(),
                    client_id: "client_mcp".to_owned(),
                },
            }),
            resource_metadata: "https://okf.example.test/.well-known/oauth-protected-resource"
                .to_owned(),
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

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
mod support;
