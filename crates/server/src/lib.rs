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
use okf_jawn_core::dispatch::dispatch;
use okf_jawn_core::ports::Application;
use serde_json::Value;

macro_rules! bind_routes {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal, $visibility:literal,
        $permission:ident, $ui:literal, $status:literal, $description:literal)),* $(,)?) => {
        /// Bind every canonical operation to its shared domain implementation.
        ///
        /// Missing authenticated `Principal` extensions reject requests; there is no anonymous owner fallback.
        pub fn router(service: Arc<dyn Application>) -> Router {
            let router = Router::new();
            $(let router = router.route($path, post(|State(service): State<Arc<dyn Application>>,
                Extension(principal): Extension<Principal>, Json(input): Json<Value>| async move {
                match dispatch(service.as_ref(), &principal, stringify!($id), input).await {
                    Ok(output) => match StatusCode::from_u16($status) {
                        Ok(status) => (status, Json(output)).into_response(),
                        Err(_) => error_response(ApiError::new(ErrorCode::Internal, "Invalid declared response status")),
                    },
                    Err(error) => error_response(error),
                }
            }));)*
            router.layer(DefaultBodyLimit::max(2 * 1024 * 1024)).with_state(service)
        }
    };
}

fn error_response(error: ApiError) -> Response {
    let status = match error.code {
        ErrorCode::InvalidInput => StatusCode::BAD_REQUEST,
        ErrorCode::Unauthenticated => StatusCode::UNAUTHORIZED,
        ErrorCode::Forbidden => StatusCode::FORBIDDEN,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict | ErrorCode::Cancelled => StatusCode::CONFLICT,
        ErrorCode::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::Unsupported => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(error)).into_response()
}

okf_jawn_contract::for_each_operation!(bind_routes);
