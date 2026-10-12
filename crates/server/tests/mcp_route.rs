//! The MCP route: mcp-execution's service mounted behind the identity middleware. A connector
//! bearer reaches it as `McpDelegation`; a request without a bearer, a browser cookie included,
//! never does.
#![cfg(feature = "runtime")]

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use okf_jawn_contract::access::Permission;
use okf_jawn_mcp::{DEFAULT_MAX_REQUEST_BODY_BYTES, McpConfig, McpPorts, streamable_http_service};
use okf_jawn_server::BoundApplication;
use okf_jawn_server::app::{ServiceParts, service};
use okf_jawn_server::cookies::CrateCookies;
use okf_jawn_server::identity::{BearerAuth, Identity, IdentityPorts};
use serde_json::{Value, json};
use tower::ServiceExt;

use check::{TestResult, some};
use fixtures::counting::CountingApplication;
use fixtures::{FixturePorts, GrantTable, workspace};
use support::FakeCredentials;

const HOST: &str = "127.0.0.1:7711";
const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const CONNECTOR: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const SECRET: &str = "connector-secret-of-the-test";

fn local_service(credentials: Arc<FakeCredentials>) -> Result<Router, Box<dyn std::error::Error>> {
    let ports = FixturePorts::new(GrantTable::default());
    let application = Arc::new(CountingApplication::new());
    let mcp = streamable_http_service(
        McpPorts {
            application: application.clone(),
            access: ports.access.clone(),
            mutations: ports.mutations.clone(),
            events: ports.events.clone(),
        },
        McpConfig {
            app_bundle: PathBuf::from("no-app-bundle-in-these-tests.html"),
            allowed_hosts: vec![HOST.to_owned()],
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
        },
    )?;
    let identity = Identity::new(
        BearerAuth::Local,
        IdentityPorts {
            credentials,
            events: ports.events.clone(),
            cookies: Arc::new(CrateCookies),
        },
        vec![format!("http://{HOST}")],
    );
    Ok(service(ServiceParts {
        bound: BoundApplication {
            application,
            access: ports.access.clone(),
            mutations: ports.mutations.clone(),
            events: ports.events.clone(),
        },
        identity,
        hosts: vec![HOST.to_owned()],
        hosted: None,

        cors_origins: Vec::new(),
        ui_dir: None,
        extra: okf_jawn_server::mcp::routes(mcp),
    })?)
}

fn initialize(
    bearer: Option<&str>,
    cookie: Option<&str>,
) -> Result<Request<Body>, axum::http::Error> {
    let mut builder = Request::post("/mcp")
        .header("host", HOST)
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json");
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "okf-jawn-server-tests", "version": "0" }
        }
    });
    builder.body(Body::from(body.to_string()))
}

#[tokio::test]
async fn a_connector_bearer_initializes_the_mounted_mcp_service() -> TestResult {
    let credentials = Arc::new(FakeCredentials::default());
    credentials.add_connector(
        SECRET,
        CONNECTOR,
        vec![workspace(WORKSPACE)?],
        vec![Permission::Read],
        false,
    )?;
    let response = local_service(credentials)?
        .oneshot(initialize(Some(SECRET), None)?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1_048_576).await?;
    let reply: Value = serde_json::from_slice(&bytes)?;
    // The service admits only `McpDelegation`; any other route would be refused here.
    assert!(reply.get("error").is_none(), "refused: {reply}");
    some(
        reply.pointer("/result/serverInfo"),
        "the server's information",
    )?;
    Ok(())
}

#[tokio::test]
async fn a_browser_cookie_never_reaches_the_mcp_service() -> TestResult {
    let credentials = Arc::new(FakeCredentials::default());
    let response = local_service(credentials)?
        .oneshot(initialize(None, Some("okf-session=any-session-secret"))?)
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().contains_key("www-authenticate"));
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
mod support;
