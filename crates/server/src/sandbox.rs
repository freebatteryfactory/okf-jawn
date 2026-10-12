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
//! grants nothing. The token is never logged or echoed.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HOST, REFERRER_POLICY,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
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
pub fn service(parts: SandboxParts) -> Router {
    let isolation = isolation_headers(&parts.app_origin);
    let host = parts.host.clone();
    let routes = Router::new()
        .route(SANDBOX_PATH, get(serve))
        .with_state(parts);
    with_fallbacks(routes)
        .layer(middleware::from_fn_with_state(host, guard_host))
        .layer(middleware::from_fn_with_state(isolation, isolate))
}

/// The headers every sandbox response carries.
#[must_use]
pub fn isolation_headers(app_origin: &str) -> HeaderMap {
    let policy = format!(
        "sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'; \
         font-src data:; media-src data:; form-action 'none'; base-uri 'none'; \
         frame-ancestors {app_origin}"
    );
    let mut headers = HeaderMap::new();
    let entries = [
        (CONTENT_SECURITY_POLICY, policy),
        (CACHE_CONTROL, "no-store".to_owned()),
        (REFERRER_POLICY, "no-referrer".to_owned()),
        (X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        (
            HeaderName::from_static("permissions-policy"),
            "camera=(), microphone=(), geolocation=(), payment=(), usb=(), \
             clipboard-read=(), clipboard-write=()"
                .to_owned(),
        ),
        (
            HeaderName::from_static("cross-origin-resource-policy"),
            "same-site".to_owned(),
        ),
    ];
    for (name, value) in entries {
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers.insert(name, value);
        }
    }
    headers
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
        return Err(ApiError::new(
            ErrorCode::TooLarge,
            format!("The representation exceeds the {SANDBOX_MAX_BYTES}-byte sandbox limit"),
        ));
    }
    let mut bytes = Vec::new();
    read.body
        .take(SANDBOX_MAX_BYTES)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| {
            ApiError::new(
                ErrorCode::Unavailable,
                "The representation could not be read",
            )
        })?;
    Ok((media_type, bytes))
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
