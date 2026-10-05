//! HTTP binding for the declared application port.
//!
//! Every canonical operation is one JSON `POST` route that calls `core::dispatch`. Identity
//! middleware, written by the server lane, inserts the authenticated `Principal` and, for
//! browser sessions, a `SessionId` as request extensions before this router runs. A request
//! that reaches a route without a `Principal` is answered 401; there is no anonymous owner
//! fallback. A dispatch failure is an `ApiError` JSON body whose status follows its `ErrorCode`.

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, FromRequestParts, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use okf_jawn_contract::access::Principal;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::mutations::MutationStore;
use okf_jawn_core::ports::Application;
use serde_json::Value;

macro_rules! bind_routes {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Bind every canonical operation to its shared domain implementation.
        ///
        /// A request without an authenticated `Principal` extension is answered 401.
        pub fn router(bound: BoundApplication) -> Router {
            let router = Router::new();
            $(let router = router.route($path, post(
                |State(bound): State<BoundApplication>,
                 authenticated: Authenticated,
                 Json(input): Json<Value>| async move {
                    respond(&bound, &authenticated, input, stringify!($id), $status).await
                },
            ));)*
            router.layer(DefaultBodyLimit::max(JSON_BODY_LIMIT)).with_state(bound)
        }
    };
}

/// Application handlers plus the ports dispatch needs for authorization and idempotency.
#[derive(Clone)]
pub struct BoundApplication {
    /// Typed operation handlers.
    pub application: Arc<dyn Application>,
    /// Workspace and tenant grant resolution.
    pub access: Arc<dyn AccessControl>,
    /// Idempotency ledger.
    pub mutations: Arc<dyn MutationStore>,
}

/// Browser session identity. Session middleware inserts it as a request extension; bearer and
/// connector callers have none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(pub String);

/// The authenticated caller taken from request extensions.
struct Authenticated {
    principal: Principal,
    session_id: Option<String>,
}

/// Largest JSON request body accepted, in bytes (2 MiB).
const JSON_BODY_LIMIT: usize = 2_097_152;

impl<S: Send + Sync> FromRequestParts<S> for Authenticated {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Some(principal) = parts.extensions.get::<Principal>().cloned() else {
            return Err(error_response(ApiError::new(
                ErrorCode::Unauthenticated,
                "Authentication is required",
            )));
        };
        let session_id = parts
            .extensions
            .get::<SessionId>()
            .map(|session| session.0.clone());
        Ok(Self {
            principal,
            session_id,
        })
    }
}

/// Run one operation for an authenticated caller and render its outcome.
async fn respond(
    bound: &BoundApplication,
    authenticated: &Authenticated,
    input: Value,
    operation_id: &str,
    success_status: u16,
) -> Response {
    let ports = DispatchPorts {
        access: bound.access.as_ref(),
        mutations: bound.mutations.as_ref(),
    };
    let caller = Caller {
        principal: &authenticated.principal,
        session_id: authenticated.session_id.as_deref(),
    };
    match Box::pin(dispatch(
        bound.application.as_ref(),
        &ports,
        &caller,
        operation_id,
        input,
    ))
    .await
    {
        Ok(output) => match StatusCode::from_u16(success_status) {
            Ok(status) => (status, Json(output)).into_response(),
            Err(_) => error_response(ApiError::new(
                ErrorCode::Internal,
                "Invalid declared response status",
            )),
        },
        Err(error) => error_response(error),
    }
}

fn error_response(error: ApiError) -> Response {
    let status = match error.code {
        ErrorCode::InvalidInput => StatusCode::BAD_REQUEST,
        ErrorCode::Unauthenticated => StatusCode::UNAUTHORIZED,
        ErrorCode::Forbidden => StatusCode::FORBIDDEN,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict
        | ErrorCode::AlreadyIssued
        | ErrorCode::InProgress
        | ErrorCode::Cancelled => StatusCode::CONFLICT,
        ErrorCode::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::Unsupported => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::NotImplemented => StatusCode::NOT_IMPLEMENTED,
        ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(error)).into_response()
}

okf_jawn_contract::for_each_operation!(bind_routes);
