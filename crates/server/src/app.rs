//! The application origin's service: every route, the fallbacks and the guarding layers, in one
//! place, so the order of the layers is reviewed once.
//!
//! From the outside in:
//! 1. the `Host` guard: a request naming a host this service does not answer for (in its `Host`
//!    header or in its request target), naming none, or carrying two `Host` headers is refused
//!    before anything else, which defeats DNS rebinding; loopback grants nothing;
//! 2. constrained CORS (`tower_http::cors`): only the configured origins, never credentials, so a
//!    page of another origin can at most present a bearer token it already holds, and by default
//!    no other origin is allowed at all;
//! 3. the identity middleware, which also refuses a cookie request from another origin and a
//!    cookie mutation without its double-submit token, and records the refusal;
//! 4. cross-origin protection (`tower_http::csrf`, on `Sec-Fetch-Site` and `Origin`) for every
//!    request that is not `GET`, `HEAD` or `OPTIONS`, trusting only the configured CORS origins;
//! 5. the routes, then the UI's pages for a path no route claims, else the JSON fallbacks.
//!
//! Every layer wraps the merged router with its fallbacks, so it also runs for an unknown path.
//! The sandbox origin is a separate service (`sandbox`), never merged into this one.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, HOST, RANGE};
use axum::http::{HeaderName, HeaderValue, Method};
use axum::middleware::{self, Next};
use axum::response::Response;
use okf_jawn_contract::access::CSRF_HEADER;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use tokio::net::TcpListener;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::csrf::{CsrfLayer, ProtectionError};

use crate::config::HostedConfig;
use crate::errors::{error_response, with_fallbacks, wrong_method};
use crate::identity::{Identity, identify};
use crate::{BoundApplication, assets, operation_routes, public};

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
    /// Origins, other than the service's own, whose pages may call it with a bearer token;
    /// usually empty. A cookie request is never accepted from them.
    pub cors_origins: Vec<String>,
    /// The UI's build output, served for every path no route claims; `None` serves no page.
    pub ui_dir: Option<PathBuf>,
    /// Extra routes mounted beside the operations, already bound to their own state: the MCP
    /// endpoint, the session routes and the declared non-JSON transports.
    pub extra: Router,
}

/// The `Host` values a service answers.
#[derive(Clone)]
struct Hosts(Arc<Vec<String>>);

/// How a refused cross-origin request is answered.
type Refusal = fn(ProtectionError) -> Response;

/// Build the application origin's service.
///
/// # Errors
/// Returns `InvalidInput` naming `hosts` when the host list is empty (an empty list would answer
/// any `Host`), and naming `cors_origins` for an entry that is not an `http` or `https` origin.
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
    let routes = match parts.ui_dir {
        Some(directory) => routes
            .fallback_service(assets::pages(&directory))
            .method_not_allowed_fallback(wrong_method),
        None => with_fallbacks(routes),
    };
    Ok(routes
        .layer(cross_origin_protection(&parts.cors_origins)?)
        .layer(middleware::from_fn_with_state(parts.identity, identify))
        .layer(constrained_cors(&parts.cors_origins)?)
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

/// `tower_http`'s cross-origin protection, trusting `origins` and answering in `ApiError` JSON.
fn cross_origin_protection(origins: &[String]) -> Result<CsrfLayer<Refusal>, ApiError> {
    let mut layer = CsrfLayer::new();
    for origin in origins {
        layer = layer
            .add_trusted_origin(origin)
            .map_err(|_| invalid_origin())?;
    }
    Ok(layer.with_rejection_response(cross_origin_refusal as Refusal))
}

/// CORS for exactly `origins`: no wildcard and no credentials.
fn constrained_cors(origins: &[String]) -> Result<CorsLayer, ApiError> {
    let allowed = origins
        .iter()
        .map(|origin| HeaderValue::from_str(origin).map_err(|_| invalid_origin()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            RANGE,
            HeaderName::from_static(CSRF_HEADER),
        ]))
}

fn cross_origin_refusal(_error: ProtectionError) -> Response {
    error_response(ApiError::new(
        ErrorCode::Forbidden,
        "A request from another origin may not change state",
    ))
}

fn invalid_origin() -> ApiError {
    ApiError::new(
        ErrorCode::InvalidInput,
        "A CORS origin must be an http or https origin with no path",
    )
    .with_field("cors_origins")
}

/// Whether every host `request` names is one this service answers for, and it names at least one.
///
/// A request names a host in its `Host` header and, for an absolute-form target or over HTTP/2,
/// in the authority of its request target. Each one present must be on the list, compared whole
/// and ASCII case-insensitively, so this guard and any proxy that routes on the other value can
/// never disagree. More than one `Host` header is refused outright, and so is a request that
/// names no host at all.
fn names_only_known_hosts(hosts: &Hosts, request: &Request) -> bool {
    let known = |host: &str| hosts.0.iter().any(|known| known.eq_ignore_ascii_case(host));
    let mut headers = request.headers().get_all(HOST).iter();
    let header = headers.next();
    if headers.next().is_some() {
        return false;
    }
    let authority = request.uri().authority();
    let header_ok = header.is_none_or(|host| host.to_str().is_ok_and(known));
    let authority_ok = authority.is_none_or(|authority| known(authority.as_str()));
    (header.is_some() || authority.is_some()) && header_ok && authority_ok
}

/// Refuse a request whose `Host` is not one this service answers for.
async fn guard_host(State(hosts): State<Hosts>, request: Request, next: Next) -> Response {
    if names_only_known_hosts(&hosts, &request) {
        next.run(request).await
    } else {
        error_response(ApiError::new(
            ErrorCode::Forbidden,
            "This service does not answer for that host",
        ))
    }
}
