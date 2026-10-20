//! The public routes: liveness and the OAuth protected-resource metadata.
//!
//! Liveness is not readiness (SPEC section 12). `/healthz` answers that the process responds and
//! asks no dependency, so a store that is down never makes the process look dead; readiness is
//! the `get_readiness` operation, which reports the configured `ReadinessProbe` through core.
//! The protected-resource metadata (RFC 9728) exists only in hosted mode, where MCP clients
//! discover WorkOS through it; it names the configured resource and issuer and no credential.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use okf_jawn_contract::access::ResourceMetadata;
use okf_jawn_contract::health::HealthResponse;

use crate::config::HostedConfig;

/// What `/healthz` reports for a process that can respond.
const ALIVE: &str = "alive";

/// The liveness path (`api/transports.json` `liveness`).
pub const LIVENESS_PATH: &str = "/healthz";

/// The protected-resource metadata path (`api/transports.json` `get_resource_metadata`).
pub const RESOURCE_METADATA_PATH: &str = "/.well-known/oauth-protected-resource";

/// The liveness route, and in hosted mode the protected-resource metadata route.
pub fn routes(hosted: Option<&HostedConfig>) -> Router {
    let router = Router::new().route(LIVENESS_PATH, get(liveness));
    match hosted {
        Some(config) => router.route(
            RESOURCE_METADATA_PATH,
            get(resource_metadata).with_state(metadata_of(config)),
        ),
        None => router,
    }
}

/// The metadata a hosted deployment publishes: its MCP resource, the WorkOS issuer, and bearer
/// tokens in the `Authorization` header. No scope is invented: Connect tokens carry none the
/// application reads.
#[must_use]
pub fn metadata_of(config: &HostedConfig) -> ResourceMetadata {
    ResourceMetadata {
        resource: config.resource.clone(),
        authorization_servers: vec![config.issuer.clone()],
        scopes_supported: Vec::new(),
        bearer_methods_supported: vec!["header".to_owned()],
    }
}

async fn liveness() -> Response {
    Json(HealthResponse {
        status: ALIVE.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    })
    .into_response()
}

async fn resource_metadata(State(metadata): State<ResourceMetadata>) -> Response {
    Json(metadata).into_response()
}
