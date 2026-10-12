//! HTTP binding for the declared application port, and the authenticated service around it.
//!
//! Every canonical operation is one JSON `POST` route that calls `core::dispatch`. The identity
//! middleware (`identity`) is the only place a `Principal` or a `SessionId` is inserted as a
//! request extension, and it is layered outside every router. A request that reaches a route
//! without a `Principal` is answered 401; there is no anonymous owner fallback and loopback is
//! never trust. Every failure, including a malformed, mistyped or oversized body, an unknown path
//! and a method no route accepts, is an `ApiError` JSON body whose status follows its
//! `ErrorCode` (`errors`).
//!
//! Startup wires dependencies and holds no operation behavior: authorization, validation and
//! the mutation ledger are core's dispatch, grants come from an `AccessControl` adapter
//! (`grants` caches the hosted one), and the MCP route mounts mcp-execution's service.

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
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
use okf_jawn_core::events::EventLog;
use okf_jawn_core::mutations::MutationStore;
use okf_jawn_core::ports::Application;
use serde_json::Value;

use crate::errors::{body_rejection, error_response};

macro_rules! bind_routes {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Every canonical operation bound to its shared domain implementation, without
        /// fallbacks: the service composition adds the fallbacks once, after every router is
        /// merged (`errors::with_fallbacks`).
        ///
        /// A request without an authenticated `Principal` extension is answered 401.
        pub fn operation_routes(bound: BoundApplication) -> Router {
            let router = Router::new();
            $(let router = router.route($path, post(
                |State(bound): State<BoundApplication>,
                 authenticated: Authenticated,
                 body: Result<Json<Value>, JsonRejection>| async move {
                    respond(&bound, &authenticated, body, stringify!($id), $status).await
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
    /// Where dispatch records a refusal of an authenticated caller (`permission_denied`).
    pub events: Arc<dyn EventLog>,
}

/// Browser session identity. The identity middleware inserts it as a request extension for a
/// request authenticated by the session cookie; bearer and connector callers have none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(pub String);

/// The authenticated caller taken from request extensions.
struct Authenticated {
    principal: Principal,
    session_id: Option<String>,
}

/// Largest JSON request body accepted, in bytes: the contract's one request-body limit.
const JSON_BODY_LIMIT: usize = okf_jawn_contract::transport::MAX_REQUEST_BODY_BYTES;

impl<S: Send + Sync> FromRequestParts<S> for Authenticated {
    type Rejection = Response;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> {
        let Some(principal) = parts.extensions.get::<Principal>().cloned() else {
            return std::future::ready(Err(error_response(ApiError::new(
                ErrorCode::Unauthenticated,
                "Authentication is required",
            ))));
        };
        let session_id = parts
            .extensions
            .get::<SessionId>()
            .map(|session| session.0.clone());
        std::future::ready(Ok(Self {
            principal,
            session_id,
        }))
    }
}

/// Every canonical operation with the fallbacks, for a service that serves nothing else.
pub fn router(bound: BoundApplication) -> Router {
    errors::with_fallbacks(operation_routes(bound))
}

/// Run one operation for an authenticated caller and render its outcome.
async fn respond(
    bound: &BoundApplication,
    authenticated: &Authenticated,
    body: Result<Json<Value>, JsonRejection>,
    operation_id: &str,
    success_status: u16,
) -> Response {
    let input = match body {
        Ok(Json(input)) => input,
        Err(rejection) => return error_response(body_rejection(&rejection, JSON_BODY_LIMIT)),
    };
    let ports = DispatchPorts {
        access: bound.access.as_ref(),
        mutations: bound.mutations.as_ref(),
        events: bound.events.as_ref(),
    };
    let caller = Caller {
        principal: &authenticated.principal,
        session_id: authenticated.session_id.as_deref(),
    };
    match dispatch(
        bound.application.as_ref(),
        &ports,
        &caller,
        operation_id,
        input,
    )
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

okf_jawn_contract::for_each_operation!(bind_routes);

#[cfg(feature = "runtime")]
pub mod app;
#[cfg(feature = "runtime")]
pub mod assets;
#[cfg(feature = "runtime")]
pub mod authkit;
pub mod config;
#[cfg(feature = "runtime")]
pub mod connect;
#[cfg(feature = "runtime")]
pub mod cookies;
pub mod errors;
#[cfg(feature = "runtime")]
pub mod events;
pub mod grants;
pub mod hosted;
pub mod identity;
pub mod local;
#[cfg(feature = "runtime")]
pub mod mcp;
pub mod public;
pub mod sandbox;
pub mod schemas;
#[cfg(feature = "runtime")]
pub mod sessions;
#[cfg(feature = "runtime")]
pub mod transfers;
