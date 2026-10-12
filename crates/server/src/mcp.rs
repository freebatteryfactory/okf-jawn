//! The MCP route: mcp-execution's Streamable HTTP service mounted at the declared path.
//!
//! The service reads the caller's `Principal` from the request extensions on every request
//! (`okf_jawn_mcp::McpServer`). The identity middleware, layered outside this route, admits only
//! the bearer path here (a connector secret locally, a WorkOS Connect token hosted) and inserts a
//! `Principal` with route `McpDelegation`; a browser session never reaches the service.

use axum::Router;
use okf_jawn_mcp::McpHttpService;

use crate::identity::MCP_PATH;

/// The MCP endpoint, for `ServiceParts::extra`.
pub fn routes(service: McpHttpService) -> Router {
    Router::new().route_service(MCP_PATH, service)
}
