//! Fallback errors and bounded body refusals: an unknown path, a wrong method and an unreadable
//! body are each an `ApiError` JSON body, and no refusal quotes the client's bytes.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::{Extension, Router};
use okf_jawn_contract::access::{AccessRoute, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::echo::ECHO_LIMIT;
use okf_jawn_server::{BoundApplication, router};
use tower::ServiceExt;

use check::TestResult;
use fixtures::counting::CountingApplication;
use fixtures::{FixturePorts, GrantTable, tenant};

const LIST_ITEMS: &str = "/api/items/list-items";

fn app(counting: Arc<CountingApplication>) -> Result<Router, Box<dyn std::error::Error>> {
    let ports = FixturePorts::new(GrantTable::default());
    let principal = Principal {
        subject: "alice".to_owned(),
        tenant_id: tenant("tenant-local")?,
        route: AccessRoute::LocalOwner,
        client_id: None,
        delegation: None,
    };
    Ok(router(BoundApplication {
        application: counting,
        access: ports.access.clone(),
        mutations: ports.mutations.clone(),
        events: ports.events.clone(),
    })
    .layer(Extension(principal)))
}

/// The refusal in `response`, which must be JSON.
async fn api_error(response: Response) -> Result<ApiError, Box<dyn std::error::Error>> {
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

#[tokio::test]
async fn a_huge_malformed_body_is_refused_without_quoting_it() -> TestResult {
    let counting = Arc::new(CountingApplication::new());
    // Just under the 2 MiB limit: a string that never closes, full of a marker.
    let mut body = String::from("{\"workspace_id\": \"");
    body.push_str(&"MARKER".repeat(300_000));
    let request = Request::post(LIST_ITEMS)
        .header("content-type", "application/json")
        .body(Body::from(body))?;
    let response = app(counting.clone())?.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error = api_error(response).await?;
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert!(!error.message.contains("MARKER"));
    assert!(error.message.len() <= ECHO_LIMIT);
    assert_eq!(counting.call_count("list_items")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_wrong_content_type_refusal_names_the_header_and_not_the_body() -> TestResult {
    let counting = Arc::new(CountingApplication::new());
    let request = Request::post(LIST_ITEMS)
        .header("content-type", "text/plain")
        .body(Body::from("MARKER".repeat(1000)))?;
    let response = app(counting)?.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error = api_error(response).await?;
    assert!(error.message.contains("Content-Type"));
    assert!(!error.message.contains("MARKER"));
    Ok(())
}

mod http_fallback_errors {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use okf_jawn_contract::error::ErrorCode;
    use tower::ServiceExt;

    use super::{LIST_ITEMS, api_error, app};
    use crate::check::TestResult;
    use crate::fixtures::counting::CountingApplication;

    #[tokio::test]
    async fn an_unknown_path_is_404_api_error_json() -> TestResult {
        let counting = Arc::new(CountingApplication::new());
        let request = Request::post("/api/items/no-such-operation")
            .header("content-type", "application/json")
            .body(Body::from("{}"))?;
        let response = app(counting)?.oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(api_error(response).await?.code, ErrorCode::NotFound);
        Ok(())
    }

    #[tokio::test]
    async fn a_get_of_an_unrouted_path_is_404_api_error_json() -> TestResult {
        let counting = Arc::new(CountingApplication::new());
        let request = Request::get("/nothing/here").body(Body::empty())?;
        let response = app(counting)?.oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(api_error(response).await?.code, ErrorCode::NotFound);
        Ok(())
    }

    #[tokio::test]
    async fn a_wrong_method_is_api_error_json_and_runs_nothing() -> TestResult {
        let counting = Arc::new(CountingApplication::new());
        let request = Request::get(LIST_ITEMS).body(Body::empty())?;
        let response = app(counting.clone())?.oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let error = api_error(response).await?;
        assert_eq!(error.code, ErrorCode::NotFound);
        // The refusal names no method: a method is the client's text.
        assert!(!error.message.contains("GET"));
        assert_eq!(counting.call_count("list_items")?, 0);
        Ok(())
    }

    #[tokio::test]
    async fn a_delete_of_an_operation_path_is_api_error_json() -> TestResult {
        let counting = Arc::new(CountingApplication::new());
        let request = Request::delete(LIST_ITEMS).body(Body::empty())?;
        let response = app(counting)?.oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(api_error(response).await?.code, ErrorCode::NotFound);
        Ok(())
    }
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
