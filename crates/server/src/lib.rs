//! HTTP binding for the declared application port.
//!
//! Supply real identity middleware and an application implementation before serving.

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use okf_jawn_contract::access::Principal;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::dispatch::{DispatchPorts, dispatch};
use okf_jawn_core::mutations::{AbandonedEffects, MutationStore};
use okf_jawn_core::ports::Application;
use serde_json::Value;

/// Application handlers plus the ports dispatch needs for authorization and idempotency.
#[derive(Clone)]
pub struct BoundApplication {
    /// Typed operation handlers.
    pub application: Arc<dyn Application>,
    /// Workspace and tenant grant resolution.
    pub access: Arc<dyn AccessControl>,
    /// Idempotency ledger.
    pub mutations: Arc<dyn MutationStore>,
    /// Abandoned-lease effect lookup across creating stores.
    pub effects: Arc<dyn AbandonedEffects>,
}

macro_rules! bind_routes {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal, $visibility:literal,
        $permission:ident, $ui:literal, $status:literal, $description:literal)),* $(,)?) => {
        /// Bind every canonical operation to its shared domain implementation.
        ///
        /// Missing authenticated `Principal` extensions reject requests; there is no anonymous owner fallback.
        pub fn router(bound: BoundApplication) -> Router {
            let router = Router::new();
            $(let router = router.route($path, post(|State(bound): State<BoundApplication>,
                Extension(principal): Extension<Principal>, Json(input): Json<Value>| async move {
                let ports = DispatchPorts {
                    access: bound.access.as_ref(),
                    mutations: bound.mutations.as_ref(),
                    effects: bound.effects.as_ref(),
                };
                match dispatch(bound.application.as_ref(), &ports, &principal, stringify!($id), input).await {
                    Ok(output) => match StatusCode::from_u16($status) {
                        Ok(status) => (status, Json(output)).into_response(),
                        Err(_) => error_response(ApiError::new(ErrorCode::Internal, "Invalid declared response status")),
                    },
                    Err(error) => error_response(error),
                }
            }));)*
            router.layer(DefaultBodyLimit::max(2 * 1024 * 1024)).with_state(bound)
        }
    };
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
        ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::NotImplemented => StatusCode::NOT_IMPLEMENTED,
    };
    (status, Json(error)).into_response()
}

okf_jawn_contract::for_each_operation!(bind_routes);
