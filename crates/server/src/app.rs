//! The application origin's service: every route, the fallbacks and the guarding layers, in one
//! place, so the order of the layers is reviewed once.
//!
//! From the outside in: the `Host` guard (a request naming a host this service does not answer
//! for is refused before anything else, which defeats DNS rebinding; loopback grants nothing),
//! then the identity middleware, then the routes. Both layers wrap the merged router with its
//! fallbacks, so they also run for an unknown path. The sandbox origin is a separate service
//! (`sandbox`), never merged into this one.

use std::future::Future;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header::HOST;
use axum::middleware::{self, Next};
use axum::response::Response;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use tokio::net::TcpListener;

use crate::config::HostedConfig;
use crate::errors::{error_response, with_fallbacks};
use crate::identity::{Identity, identify};
use crate::{BoundApplication, operation_routes, public};

/// Everything the application origin's service is built from.
pub struct ServiceParts {
    /// The application and dispatch's ports.
    pub bound: BoundApplication,
    /// The identity middleware's state.
    pub identity: Identity,
    /// The `Host` values this service answers (`host:port`, compared exactly, ASCII case
    /// insensitive); never empty.
    pub hosts: Vec<String>,
    /// The hosted settings, when hosted; they publish the protected-resource metadata.
    pub hosted: Option<HostedConfig>,
    /// Extra routes mounted beside the operations, already bound to their own state: the MCP
    /// endpoint and the declared non-JSON transports.
    pub extra: Router,
}

/// The `Host` values a service answers.
#[derive(Clone)]
struct Hosts(Arc<Vec<String>>);

/// Build the application origin's service.
///
/// # Errors
/// Returns `InvalidInput` naming `hosts` when the host list is empty: an empty list would
/// answer any `Host`.
pub fn service(parts: ServiceParts) -> Result<Router, ApiError> {
    if parts.hosts.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidInput,
            "The service needs at least one host it answers for",
        )
        .with_field("hosts"));
    }
    let routes = operation_routes(parts.bound)
        .merge(public::routes(parts.hosted.as_ref()))
        .merge(parts.extra);
    Ok(with_fallbacks(routes)
        .layer(middleware::from_fn_with_state(parts.identity, identify))
        .layer(middleware::from_fn_with_state(
            Hosts(Arc::new(parts.hosts)),
            guard_host,
        )))
}

/// Serve `router` on `listener` until `shutdown` resolves, then stop accepting and let the
/// requests in flight finish.
///
/// # Errors
/// Returns the listener's I/O error.
pub async fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
}

/// Refuse a request whose `Host` is not one this service answers for.
async fn guard_host(State(hosts): State<Hosts>, request: Request, next: Next) -> Response {
    // HTTP/2 names the host in the request target's authority instead of a `Host` header.
    let named = match request.headers().get(HOST) {
        Some(host) => host.to_str().ok().map(str::to_owned),
        None => request.uri().authority().map(ToString::to_string),
    };
    let allowed = named.is_some_and(|host| {
        hosts
            .0
            .iter()
            .any(|known| known.eq_ignore_ascii_case(&host))
    });
    if allowed {
        next.run(request).await
    } else {
        error_response(ApiError::new(
            ErrorCode::Forbidden,
            "This service does not answer for that host",
        ))
    }
}
