//! The public and declared non-JSON routes of the application origin: liveness asks no
//! dependency, the protected-resource metadata exists only hosted, and form schemas need a
//! signed-in caller.

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use okf_jawn_contract::access::{Permission, ResourceMetadata};
use okf_jawn_contract::health::HealthResponse;
use okf_jawn_server::BoundApplication;
use okf_jawn_server::app::{ServiceParts, service};
use okf_jawn_server::config::HostedConfig;
use okf_jawn_server::identity::{BearerAuth, ConnectIdentity, Identity, IdentityPorts};
use serde_json::Value;
use tower::ServiceExt;

use check::TestResult;
use fixtures::counting::CountingApplication;
use fixtures::{FixturePorts, GrantTable, tenant, workspace};
use support::{FakeCredentials, NoCookies, OWNER, OneToken};

const HOST: &str = "127.0.0.1:7711";
const SECRET: &str = "connector-secret-of-the-test";

fn hosted_config() -> HostedConfig {
    HostedConfig {
        client_id: "client_01".to_owned(),
        api_key: "not-a-real-api-key".to_owned(),
        issuer: "https://auth.example.test".to_owned(),
        resource: "https://okf.example.test/mcp".to_owned(),
        redirect_uri: "https://okf.example.test/auth/callback".to_owned(),
    }
}

fn app(hosted: bool) -> Result<Router, Box<dyn std::error::Error>> {
    let ports = FixturePorts::new(GrantTable::default());
    let credentials = Arc::new(FakeCredentials::default());
    credentials.add_connector(
        SECRET,
        "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
        vec![workspace("11111111-1111-4111-8111-111111111111")?],
        vec![Permission::Read],
        false,
    )?;
    let bearer = if hosted {
        BearerAuth::Hosted {
            connect: Arc::new(OneToken {
                token: SECRET.to_owned(),
                identity: ConnectIdentity {
                    subject: OWNER.to_owned(),
                    organization_id: "org_01".to_owned(),
                    client_id: "client_mcp".to_owned(),
                },
            }),
            resource_metadata: "https://okf.example.test/.well-known/oauth-protected-resource"
                .to_owned(),
        }
    } else {
        BearerAuth::Local {
            tenant_id: tenant("local")?,
        }
    };
    let identity = Identity::new(
        bearer,
        IdentityPorts {
            credentials,
            events: ports.events.clone(),
            cookies: Arc::new(NoCookies),
        },
        vec![format!("http://{HOST}")],
    );
    Ok(service(ServiceParts {
        bound: BoundApplication {
            application: Arc::new(CountingApplication::new()),
            access: ports.access.clone(),
            mutations: ports.mutations.clone(),
            events: ports.events.clone(),
        },
        identity,
        hosts: vec![HOST.to_owned()],
        hosted: hosted.then(hosted_config),
        extra: okf_jawn_server::schemas::routes(),
    })?)
}

fn get(path: &str, bearer: Option<&str>) -> Result<Request<Body>, axum::http::Error> {
    let builder = Request::get(path).header("host", HOST);
    let builder = match bearer {
        Some(token) => builder.header("authorization", format!("Bearer {token}")),
        None => builder,
    };
    builder.body(Body::empty())
}

#[tokio::test]
async fn liveness_is_public_and_reports_only_that_the_process_responds() -> TestResult {
    let response = app(false)?.oneshot(get("/healthz", None)?).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 65_536).await?;
    let health: HealthResponse = serde_json::from_slice(&bytes)?;
    assert_eq!(health.status, "alive");
    Ok(())
}

#[tokio::test]
async fn hosted_mode_publishes_its_resource_and_issuer() -> TestResult {
    let response = app(true)?
        .oneshot(get("/.well-known/oauth-protected-resource", None)?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 65_536).await?;
    let metadata: ResourceMetadata = serde_json::from_slice(&bytes)?;
    assert_eq!(metadata.resource, "https://okf.example.test/mcp");
    assert_eq!(
        metadata.authorization_servers,
        vec!["https://auth.example.test".to_owned()]
    );
    assert_eq!(metadata.bearer_methods_supported, vec!["header".to_owned()]);
    assert!(!String::from_utf8_lossy(&bytes).contains("not-a-real-api-key"));
    Ok(())
}

#[tokio::test]
async fn local_mode_publishes_no_resource_metadata() -> TestResult {
    let response = app(false)?
        .oneshot(get("/.well-known/oauth-protected-resource", None)?)
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn a_form_schema_is_served_to_a_signed_in_caller_only() -> TestResult {
    let response = app(false)?.oneshot(get("/api/schemas/view", None)?).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = app(false)?
        .oneshot(get("/api/schemas/view", Some(SECRET))?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/schema+json")
    );
    let bytes = to_bytes(response.into_body(), 4_194_304).await?;
    let schema: Value = serde_json::from_slice(&bytes)?;
    assert!(schema.is_object());

    let response = app(false)?
        .oneshot(get("/api/schemas/..%2F..%2FCargo.toml", Some(SECRET))?)
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
mod support;
