//! Real router plus a test-only Application fixture checks binding behavior, not storage.

use axum::{
    Extension,
    body::{Body, to_bytes},
    http::Request,
};
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn real_route_uses_the_injected_application() -> Result<(), Box<dyn Error>> {
    let principal = Principal {
        subject: "control".to_owned(),
        route: AccessRoute::LocalOwner,
        workspace_ids: vec![],
        permissions: vec![Permission::Read],
        client_id: None,
    };
    let fixture = support::FixtureApplication {
        responses: BTreeMap::from([("list_workspaces".to_owned(), json!({"items":[]}))]),
    };
    let app = okf_jawn_server::router(Arc::new(fixture)).layer(Extension(principal));
    let request = Request::post("/api/workspaces/list-workspaces")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"page":{"limit":10}}"#))?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), 200);
    let bytes = to_bytes(response.into_body(), 1_048_576).await?;
    let body: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(body.get("items"), Some(&json!([])));
    Ok(())
}

#[path = "../../../tests/support/application.rs"]
mod support;
