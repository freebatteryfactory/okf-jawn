//! Static assets: the UI's build output, served from a configured directory.
//!
//! `tower_http`'s `ServeDir` serves the files and refuses any path outside the directory; a path
//! that names no file gets `index.html`, so a client-side route opens the workspace. Pages are
//! served only for `GET` and `HEAD`, and never under a prefix the API owns (`/api`, `/auth`,
//! `/mcp`, `/sandbox`, `/.well-known`, `/healthz`): there an unknown path stays an `ApiError`
//! JSON `not_found`, never a page. The directory is deployment configuration, never a
//! client-chosen path.

use std::path::Path;

use axum::Router;
use axum::extract::Request;
use axum::http::Method;
use axum::middleware::{self, Next};
use axum::response::Response;
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

/// The service of the UI build in `directory`, for the application router's fallback.
pub fn pages(directory: &Path) -> Router {
    let files = ServeDir::new(directory)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(directory.join("index.html")));
    Router::new()
        .fallback_service(files)
        .layer(middleware::from_fn(pages_only))
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
