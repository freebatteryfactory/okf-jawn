//! Binding tests: the real router, the shared test `Application` fixtures, and the core fixture
//! `AccessControl` and `MutationStore`. Identity middleware and storage are not involved; the
//! tests insert the `Principal` and `SessionId` extensions the middleware would.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::{Extension, Router};
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::IdentityError;
use okf_jawn_core::ports::Application;
use okf_jawn_server::{BoundApplication, SessionId, router};
use serde_json::{Value, json};
use tower::ServiceExt;

use application::FixtureApplication;
use check::{TestResult, some};
use fixtures::counting::CountingApplication;
use fixtures::{FixturePorts, GrantTable, tenant, workspace};

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const OTHER_WORKSPACE: &str = "22222222-2222-4222-8222-222222222222";
const REVISION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LIST_ITEMS: &str = "/api/items/list-items";
const CREATE_ITEM: &str = "/api/items/create-item";
/// One byte more than the router's 2 MiB JSON body limit.
const OVER_LIMIT: usize = 2_097_153;

fn alice() -> Result<Principal, IdentityError> {
    Ok(Principal {
        subject: "alice".to_owned(),
        tenant_id: tenant("tenant-local")?,
        route: AccessRoute::BrowserSession,
        client_id: None,
        delegation: None,
    })
}

/// Alice may read and write one workspace and holds nothing on any other.
fn ports() -> Result<FixturePorts, serde_json::Error> {
    let mut table = GrantTable::default();
    table
        .workspaces
        .entry("alice".to_owned())
        .or_default()
        .insert(
            workspace(WORKSPACE)?,
            vec![Permission::Read, Permission::Write],
        );
    Ok(FixturePorts::new(table))
}

/// The real router over `application` and the fixture ports, with Alice signed in.
fn signed_in(
    application: Arc<dyn Application>,
    ports: &FixturePorts,
) -> Result<Router, Box<dyn std::error::Error>> {
    Ok(anonymous(application, ports).layer(Extension(alice()?)))
}

/// The real router with no identity middleware in front of it.
fn anonymous(application: Arc<dyn Application>, ports: &FixturePorts) -> Router {
    router(BoundApplication {
        application,
        access: ports.access.clone(),
        mutations: ports.mutations.clone(),
        events: ports.events.clone(),
    })
}

fn post(
    path: &str,
    content_type: &str,
    body: impl Into<Body>,
) -> Result<Request<Body>, axum::http::Error> {
    Request::post(path)
        .header("content-type", content_type)
        .body(body.into())
}

fn post_json(path: &str, input: &Value) -> Result<Request<Body>, axum::http::Error> {
    post(path, "application/json", input.to_string())
}

async fn json_body(response: Response) -> Result<Value, Box<dyn std::error::Error>> {
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if content_type.as_deref() != Some("application/json") {
        return Err(format!("expected a JSON body, got content type {content_type:?}").into());
    }
    let bytes = to_bytes(response.into_body(), 1_048_576).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn api_error(response: Response) -> Result<ApiError, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value(json_body(response).await?)?)
}

fn list_items_body(workspace_id: &str) -> Value {
    json!({
        "workspace_id": workspace_id,
        "at": { "kind": "latest" },
        "folder": "",
        "page": { "limit": 10 }
    })
}

fn listing() -> Value {
    json!({ "revision": REVISION, "items": [], "folders": [] })
}

fn create_item_body() -> Value {
    json!({
        "workspace_id": WORKSPACE,
        "base_revision": REVISION,
        "path": "notes/a.md",
        "title": "a",
        "type_name": "note",
        "kind": "note",
        "body": "hello",
        "properties": {},
        "idempotency_key": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    })
}

fn item_document() -> Value {
    json!({
        "summary": {
            "id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "path": "notes/a.md",
            "title": "a",
            "description": "",
            "type_name": "note",
            "kind": "note",
            "revision": REVISION,
            "status": "stable",
            "archived": false
        },
        "body": "hello",
        "properties": {}
    })
}

#[tokio::test]
async fn a_read_succeeds_end_to_end_and_an_ungranted_workspace_is_403() -> TestResult {
    let ports = ports()?;
    let fixture = FixtureApplication {
        responses: BTreeMap::from([("list_items".to_owned(), listing())]),
    };
    let app = signed_in(Arc::new(fixture), &ports)?;

    let request = post_json(LIST_ITEMS, &list_items_body(WORKSPACE))?;
    let response = app.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await?;
    assert_eq!(body.get("items"), Some(&json!([])));
    assert_eq!(ports.access.lookups(), 1);

    let request = post_json(LIST_ITEMS, &list_items_body(OTHER_WORKSPACE))?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(api_error(response).await?.code, ErrorCode::Forbidden);
    Ok(())
}

#[tokio::test]
async fn a_missing_principal_is_401_with_an_api_error_body() -> TestResult {
    let ports = ports()?;
    let counting = Arc::new(CountingApplication::new());
    counting.set_response("list_items", listing())?;
    let app = anonymous(counting.clone(), &ports);
    let request = post_json(LIST_ITEMS, &list_items_body(WORKSPACE))?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(api_error(response).await?.code, ErrorCode::Unauthenticated);
    assert_eq!(counting.call_count("list_items")?, 0);
    assert_eq!(ports.access.lookups(), 0);
    Ok(())
}

#[tokio::test]
async fn the_session_id_extension_reaches_the_handler_context() -> TestResult {
    let ports = ports()?;
    let counting = Arc::new(CountingApplication::new());
    counting.set_response("list_items", listing())?;
    let app = signed_in(counting.clone(), &ports)?;

    let in_session = app
        .clone()
        .layer(Extension(SessionId("session-1".to_owned())));
    let request = post_json(LIST_ITEMS, &list_items_body(WORKSPACE))?;
    assert_eq!(in_session.oneshot(request).await?.status(), StatusCode::OK);
    let request = post_json(LIST_ITEMS, &list_items_body(WORKSPACE))?;
    assert_eq!(app.oneshot(request).await?.status(), StatusCode::OK);

    let seen = counting.contexts("list_items")?;
    let with_session = some(seen.first(), "the call made inside a session")?;
    let without_session = some(seen.get(1), "the call made without a session")?;
    assert_eq!(with_session.session_id.as_deref(), Some("session-1"));
    assert_eq!(with_session.principal.subject, "alice");
    assert_eq!(without_session.session_id, None);
    Ok(())
}

#[tokio::test]
async fn a_not_implemented_handler_is_501() -> TestResult {
    let ports = ports()?;
    let counting = Arc::new(CountingApplication::new());
    counting.fail_once(
        "list_items",
        ApiError::new(ErrorCode::NotImplemented, "list_items is not built yet"),
    )?;
    let app = signed_in(counting.clone(), &ports)?;
    let request = post_json(LIST_ITEMS, &list_items_body(WORKSPACE))?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    let error = api_error(response).await?;
    assert_eq!(error.code, ErrorCode::NotImplemented);
    assert_eq!(error.message, "list_items is not built yet");
    Ok(())
}

#[tokio::test]
async fn malformed_json_is_400_invalid_input() -> TestResult {
    let ports = ports()?;
    let counting = Arc::new(CountingApplication::new());
    let app = signed_in(counting.clone(), &ports)?;
    let request = post(LIST_ITEMS, "application/json", "{not json")?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await?;
    assert_eq!(body.get("code"), Some(&json!("invalid_input")));
    assert_eq!(counting.call_count("list_items")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_wrong_content_type_is_400_invalid_input() -> TestResult {
    let ports = ports()?;
    let counting = Arc::new(CountingApplication::new());
    counting.set_response("list_items", listing())?;
    let app = signed_in(counting.clone(), &ports)?;
    let request = post(
        LIST_ITEMS,
        "text/plain",
        list_items_body(WORKSPACE).to_string(),
    )?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error = api_error(response).await?;
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert!(error.message.contains("Content-Type"));
    assert_eq!(counting.call_count("list_items")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_body_over_the_limit_is_413_too_large() -> TestResult {
    let ports = ports()?;
    let counting = Arc::new(CountingApplication::new());
    let app = signed_in(counting.clone(), &ports)?;
    let request = post(LIST_ITEMS, "application/json", vec![b' '; OVER_LIMIT])?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = json_body(response).await?;
    assert_eq!(body.get("code"), Some(&json!("too_large")));
    assert_eq!(counting.call_count("list_items")?, 0);
    Ok(())
}

#[tokio::test]
async fn in_progress_is_409_with_retry_after_from_the_detail() -> TestResult {
    let ports = ports()?;
    let counting = Arc::new(CountingApplication::new());
    counting.set_response("create_item", item_document())?;
    counting.park_next("create_item")?;
    let app = signed_in(counting.clone(), &ports)?;

    let running = app
        .clone()
        .oneshot(post_json(CREATE_ITEM, &create_item_body())?);
    tokio::pin!(running);
    tokio::select! {
        biased;
        outcome = &mut running => {
            let status = outcome.map(|response| response.status());
            return Err(format!("the first request finished while parked: {status:?}").into());
        }
        () = counting.entered() => {}
    }

    let response = app
        .oneshot(post_json(CREATE_ITEM, &create_item_body())?)
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let header = some(
        response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        "the Retry-After header",
    )?;
    let error = api_error(response).await?;
    assert_eq!(error.code, ErrorCode::InProgress);
    let detail = some(error.detail, "the in-progress detail")?;
    let ErrorDetail::InProgress { retry_after, .. } = *detail else {
        return Err("expected an in-progress detail".into());
    };
    assert!(retry_after >= 1);
    assert_eq!(header, retry_after.to_string());

    counting.resume();
    assert_eq!(running.await?.status(), StatusCode::OK);
    assert_eq!(counting.call_count("create_item")?, 1);
    Ok(())
}

#[path = "../../../tests/support/application.rs"]
mod application;
#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
