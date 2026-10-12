//! `get_form_schema`: the generated form schemas, by their allowed names, to signed-in callers.
//!
//! The schemas are the committed generator output under `api/forms/`, embedded at build time,
//! so the route serves exactly what `gen` wrote and never reads a client-chosen path. Any other
//! name is `not_found`; a request without a `Principal` is 401.

use axum::extract::Path;
use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};
use okf_jawn_contract::access::Principal;
use okf_jawn_contract::error::{ApiError, ErrorCode};

use crate::errors::error_response;

/// The form schemas, by allowed name.
const FORMS: [(&str, &str); 4] = [
    (
        "naming-rules",
        include_str!("../../../api/forms/naming-rules.schema.json"),
    ),
    ("view", include_str!("../../../api/forms/view.schema.json")),
    ("type", include_str!("../../../api/forms/type.schema.json")),
    (
        "dataset",
        include_str!("../../../api/forms/dataset.schema.json"),
    ),
];

/// The route (`api/transports.json` `get_form_schema`).
pub const FORM_SCHEMA_PATH: &str = "/api/schemas/{name}";

/// The form schema route, for `ServiceParts::extra`.
pub fn routes() -> Router {
    Router::new().route(FORM_SCHEMA_PATH, get(form_schema))
}

async fn form_schema(
    principal: Option<Extension<Principal>>,
    Path(name): Path<String>,
) -> Response {
    if principal.is_none() {
        return error_response(ApiError::new(
            ErrorCode::Unauthenticated,
            "Authentication is required",
        ));
    }
    match FORMS.iter().find(|(allowed, _)| *allowed == name) {
        Some((_, schema)) => (
            [(
                CONTENT_TYPE,
                HeaderValue::from_static("application/schema+json"),
            )],
            *schema,
        )
            .into_response(),
        None => error_response(ApiError::new(
            ErrorCode::NotFound,
            "No form schema has that name",
        )),
    }
}
