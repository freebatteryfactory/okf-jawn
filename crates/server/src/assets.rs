//! Static assets: the UI's build output, served from a configured directory, with the security
//! headers every page carries.
//!
//! `tower_http`'s `ServeDir` serves the files and refuses any path outside the directory; a path
//! that names no file gets `index.html`, so a client-side route opens the workspace. Pages are
//! served only for `GET` and `HEAD`, and never under a prefix the API owns (`/api`, `/auth`,
//! `/mcp`, `/sandbox`, `/.well-known`, `/healthz`): there an unknown path stays an `ApiError`
//! JSON `not_found`, never a page. The directory is deployment configuration, never a
//! client-chosen path.
//!
//! Every page and file served carries (server-m2 rulings, Q7):
//! - `Content-Security-Policy`: everything from the service's own origin and nothing else;
//!   scripts strict (`script-src 'self'`: no inline script, no `eval`); styles may be inline,
//!   because `CodeMirror` and vega-embed insert `<style>` elements; images and fonts also from
//!   `data:`, images from `blob:`; the YAML module worker (`worker-src 'self'`); frames only of
//!   the sandbox origin (`frame-src`, `'none'` without one); the page itself framed by nobody
//!   (`frame-ancestors 'none'`); no `<base>`, no plugin, forms only to the service;
//! - `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`
//!   and `Cross-Origin-Opener-Policy: same-origin`.
//!
//! A JSON refusal under a reserved prefix is not a page and carries none of them.

use std::path::Path;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header::{
    CONTENT_SECURITY_POLICY, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method};
use axum::middleware::{self, Next};
use axum::response::Response;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use tower_http::services::{ServeDir, ServeFile};

use crate::errors::unknown_path;

/// Path prefixes the API owns; no page is ever served under them.
const RESERVED: [&str; 6] = [
    "/api",
    "/auth",
    "/mcp",
    "/sandbox",
    "/.well-known",
    "/healthz",
];

/// The service of the UI build in `directory`, for the application router's fallback, its
/// pages framing only `sandbox_origin`.
///
/// # Errors
/// Returns `InvalidInput` naming `sandbox_origin` when it cannot be written into the policy.
pub fn pages(directory: &Path, sandbox_origin: Option<&str>) -> Result<Router, ApiError> {
    let headers = page_headers(sandbox_origin)?;
    let files = ServeDir::new(directory)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(directory.join("index.html")));
    Ok(Router::new()
        .fallback_service(files)
        .layer(middleware::from_fn_with_state(headers, with_headers))
        .layer(middleware::from_fn(pages_only)))
}

/// The headers of every page, the policy framing only `sandbox_origin`.
///
/// # Errors
/// Returns `InvalidInput` naming `sandbox_origin` when it is not an origin.
pub fn page_headers(sandbox_origin: Option<&str>) -> Result<HeaderMap, ApiError> {
    let invalid = || {
        ApiError::new(
            ErrorCode::InvalidInput,
            "The sandbox origin must be an http or https origin with no path",
        )
        .with_field("sandbox_origin")
    };
    let frames = match sandbox_origin {
        Some(origin) if crate::sandbox::is_origin(origin) => origin.to_owned(),
        Some(_) => return Err(invalid()),
        None => "'none'".to_owned(),
    };
    let policy = format!(
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; \
         worker-src 'self'; frame-src {frames}; frame-ancestors 'none'; base-uri 'none'; \
         form-action 'self'; object-src 'none'"
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_str(&policy).map_err(|_| invalid())?,
    );
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    Ok(headers)
}

/// Serve a page only for a read of a path the API does not own.
async fn pages_only(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let reserved = RESERVED.iter().any(|prefix| {
        path.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    });
    if reserved || !matches!(*request.method(), Method::GET | Method::HEAD) {
        return unknown_path().await;
    }
    next.run(request).await
}

/// Put the page headers on a served page or file.
async fn with_headers(State(headers): State<HeaderMap>, request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    for (name, value) in &headers {
        response.headers_mut().insert(name.clone(), value.clone());
    }
    response
}
