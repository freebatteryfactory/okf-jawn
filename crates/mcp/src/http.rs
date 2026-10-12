//! The Streamable HTTP service the server mounts, built on rmcp's server transport.
//!
//! The service runs stateless: rmcp's legacy session mode is off, so no `Mcp-Session-Id` is
//! issued and every POST is served by its own handler clone with that request's identity. A
//! session would bind state to whoever opened it and need its own check that a later request
//! comes from the same principal; without sessions there is nothing to hijack, and protocol
//! `2026-07-28` removes sessions anyway. Responses are plain JSON where rmcp allows it. This crate
//! serves no stdio; a local process client reaches it through the CLI's `mcp --stdio` relay.

use std::path::PathBuf;
use std::sync::Arc;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use crate::server::{McpPorts, McpServer};

/// The tower service to mount on the MCP endpoint path, behind the identity middleware.
pub type McpHttpService = StreamableHttpService<McpServer, NeverSessionManager>;

/// What the MCP endpoint needs beyond its ports.
#[derive(Debug, Clone)]
pub struct McpConfig {
    /// The built MCP App HTML served as `ui://okf-jawn/app.html`; read on each resource read.
    pub app_bundle: PathBuf,
    /// `Host` values rmcp accepts (`host` for any port, or `host:port`), its DNS-rebinding
    /// guard. Never empty: rmcp reads an empty list as "accept every host".
    pub allowed_hosts: Vec<String>,
    /// The largest POST body rmcp reads; a larger request is refused with HTTP 413 before the
    /// handler runs. [`DEFAULT_MAX_REQUEST_BODY_BYTES`] is provisional (see there).
    pub max_request_body_bytes: usize,
}

/// rmcp's own default POST body limit (4 MiB), which rmcp does not export.
///
/// Provisional: the contract bounds neither agent-supplied proposal text nor the number of
/// changes, so a contract-valid `propose` can exceed it and be refused by the transport (HTTP
/// 413) before dispatch. The contract bound, and one body limit shared by HTTP and MCP, are
/// integration-owner work carried to server M1; this default changes with them.
pub const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Build the Streamable HTTP service over the application.
///
/// The server mounts it (for example `Router::nest_service("/mcp", service)`) behind the
/// middleware that authenticates the caller and inserts its `Principal` into the request's
/// extensions; the handler reads it from there on every call.
///
/// # Errors
/// Returns `InvalidInput` for an empty host list, and `Internal` when the committed catalogs
/// disagree with the operation table.
pub fn streamable_http_service(
    ports: McpPorts,
    config: McpConfig,
) -> Result<McpHttpService, ApiError> {
    if config.allowed_hosts.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidInput,
            "The MCP endpoint needs at least one allowed host",
        ));
    }
    let server = McpServer::new(ports, config.app_bundle)?;
    let transport = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(config.allowed_hosts)
        .with_max_request_body_bytes(config.max_request_body_bytes);
    Ok(StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(NeverSessionManager::default()),
        transport,
    ))
}
