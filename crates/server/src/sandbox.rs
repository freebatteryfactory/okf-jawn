//! The sandbox origin: hostile HTML and artifacts, served only on the separately configured
//! origin, only through a short-lived capability, and with headers that isolate them.
//!
//! SPEC section 11: "Uploaded HTML is hostile data until constrained by actual isolation. Use a
//! sandboxed separate origin, no credentials or privileged APIs, restricted navigation/messages
//! and explicit external-resource policy." So this is its own service, never merged into the
//! application origin's: it answers only the sandbox host, serves only
//! `/sandbox/{capability}`, runs no identity middleware, reads no cookie or `Authorization`
//! header and sets none. The capability is the only credential: the route applies
//! `core::sandbox::token_hash` to it and asks `SandboxCapabilityStore::resolve` for its binding,
//! then opens exactly the bound object. Every response, a refusal included, carries the
//! isolation headers: `Content-Security-Policy: sandbox` with no `allow-*` token (an opaque
//! origin with no script, form, popup or top navigation), `default-src 'none'` with inline
//! styles and `data:` images only (no external resource), `frame-ancestors` limited to the
//! application origin, `no-store`, `no-referrer`, `nosniff` and a `Permissions-Policy` that
//! grants nothing. No isolation header is ever skipped: the application origin named in the
//! policy must be an origin and nothing else, and the service does not start otherwise. The
//! token is never logged or echoed.

use std::str::FromStr as _;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HOST, REFERRER_POLICY,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::sandbox::{SandboxCapabilityStore, token_hash};
use okf_jawn_core::storage::BlobStore;
use tokio::io::AsyncReadExt as _;

use crate::errors::{error_response, with_fallbacks};

/// What the sandbox origin's service reads from.
#[derive(Clone)]
pub struct SandboxParts {
    /// Hashed capability tokens.
    pub capabilities: Arc<dyn SandboxCapabilityStore>,
    /// Retained bytes.
    pub blobs: Arc<dyn BlobStore>,
    /// The sandbox origin's `Host` (`host:port`), the only one this service answers.
    pub host: String,
    /// The application origin, the only page that may frame a sandboxed representation.
    pub app_origin: String,
}

/// The sandbox route (`api/transports.json` `serve_sandbox_representation`).
pub const SANDBOX_PATH: &str = "/sandbox/{capability}";

/// Largest representation served, in bytes (16 MiB).
pub const SANDBOX_MAX_BYTES: u64 = 16_777_216;

/// Longest capability token looked up; a longer path segment is no capability.
const MAX_TOKEN_CHARS: usize = 256;

/// The sandbox origin's service.
///
/// # Errors
/// Returns `InvalidInput` naming `app_origin` when the application origin is not an origin, so
/// the service never starts with an isolation header it could not build.
pub fn service(parts: SandboxParts) -> Result<Router, ApiError> {
    let host = parts.host.clone();
    let app_origin = parts.app_origin.clone();
    let routes = Router::new()
        .route(SANDBOX_PATH, get(serve))
        .with_state(parts);
    isolated(
        with_fallbacks(routes).layer(middleware::from_fn_with_state(host, guard_host)),
        &app_origin,
    )
}

/// `inner` with the isolation headers on every response and any `Set-Cookie` removed, whatever
/// `inner` answers.
///
/// # Errors
/// Returns `isolation_headers`'s refusal of `app_origin`.
pub fn isolated(inner: Router, app_origin: &str) -> Result<Router, ApiError> {
    let isolation = isolation_headers(app_origin)?;
    Ok(inner.layer(middleware::from_fn_with_state(isolation, isolate)))
}

/// The headers every sandbox response carries. Every value but the policy is static; the
/// policy names `app_origin`, which must be an origin and nothing else.
///
/// # Errors
/// Returns `InvalidInput` naming `app_origin` when it is not `http` or `https`, `://`, a host
/// and an optional port, in ASCII, with nothing before or after; no header is ever skipped.
pub fn isolation_headers(app_origin: &str) -> Result<HeaderMap, ApiError> {
    let invalid = || {
        ApiError::new(
            ErrorCode::InvalidInput,
            "The application origin must be an http or https origin with no path",
        )
        .with_field("app_origin")
    };
    if !is_origin(app_origin) {
        return Err(invalid());
    }
    let policy = format!(
        "sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'; \
         font-src data:; media-src data:; form-action 'none'; base-uri 'none'; \
         frame-ancestors {app_origin}"
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_str(&policy).map_err(|_| invalid())?,
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static(
            "camera=(), microphone=(), geolocation=(), payment=(), usb=(), \
             clipboard-read=(), clipboard-write=()",
        ),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-site"),
    );
    Ok(headers)
}

/// Whether `text` is an origin and nothing else: `http` or `https`, `://`, a host, an optional
/// port; no user information, path, query or fragment, and no trailing slash. The parser
/// refuses whitespace, control and non-ASCII characters, and the parsed form must spell exactly
/// the text, so nothing the parser drops gets through.
fn is_origin(text: &str) -> bool {
    let Ok(uri) = Uri::from_str(text) else {
        return false;
    };
    let (Some(scheme), Some(authority)) = (uri.scheme_str(), uri.authority()) else {
        return false;
    };
    let port_ok = authority.port().is_none_or(|port| port.as_u16() >= 1);
    matches!(scheme, "http" | "https")
        && !authority.host().is_empty()
        && !authority.as_str().contains('@')
        && port_ok
        && format!("{scheme}://{authority}") == text
}

/// Serve the representation a capability is bound to.
async fn serve(State(parts): State<SandboxParts>, Path(capability): Path<String>) -> Response {
    match representation(&parts, &capability).await {
        Ok((media_type, bytes)) => ([(CONTENT_TYPE, media_type)], bytes).into_response(),
        Err(error) => error_response(error),
    }
}

/// The bound object's media type and bytes; `NotFound` for any capability that does not
/// resolve, whatever the reason, so a refusal says nothing about why.
async fn representation(
    parts: &SandboxParts,
    capability: &str,
) -> Result<(HeaderValue, Vec<u8>), ApiError> {
    let not_found = || ApiError::new(ErrorCode::NotFound, "No such sandboxed representation");
    if capability.is_empty() || capability.chars().count() > MAX_TOKEN_CHARS {
        return Err(not_found());
    }
    let resolved = parts
        .capabilities
        .resolve(token_hash(capability))
        .await?
        .ok_or_else(not_found)?;
    let media_type = HeaderValue::from_str(&resolved.media_type).map_err(|_| {
        ApiError::new(
            ErrorCode::Internal,
            "The capability records a media type no header can carry",
        )
    })?;
    let read = parts
        .blobs
        .open(
            &resolved.scope,
            &resolved.object,
            0,
            SANDBOX_MAX_BYTES.saturating_add(1),
        )
        .await?;
    if read.object.size > SANDBOX_MAX_BYTES {
        return Err(too_large());
    }
    // The store's reported size is not trusted alone: one byte past the limit is read, and a
    // body that long is refused, never cut and served.
    let mut bytes = Vec::new();
    read.body
        .take(SANDBOX_MAX_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| {
            ApiError::new(
                ErrorCode::Unavailable,
                "The representation could not be read",
            )
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > SANDBOX_MAX_BYTES {
        return Err(too_large());
    }
    Ok((media_type, bytes))
}

fn too_large() -> ApiError {
    ApiError::new(
        ErrorCode::TooLarge,
        format!("The representation exceeds the {SANDBOX_MAX_BYTES}-byte sandbox limit"),
    )
}

/// Answer only the sandbox host.
async fn guard_host(State(host): State<String>, request: Request, next: Next) -> Response {
    let named = match request.headers().get(HOST) {
        Some(value) => value.to_str().ok().map(str::to_owned),
        None => request.uri().authority().map(ToString::to_string),
    };
    if named.is_some_and(|named| named.eq_ignore_ascii_case(&host)) {
        next.run(request).await
    } else {
        error_response(ApiError::new(
            ErrorCode::Forbidden,
            "The sandbox origin does not answer for that host",
        ))
    }
}

/// Put the isolation headers on every response, refusals included, and remove any cookie.
async fn isolate(State(headers): State<HeaderMap>, request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .remove(axum::http::header::SET_COOKIE);
    for (name, value) in &headers {
        response.headers_mut().insert(name.clone(), value.clone());
    }
    response
}
