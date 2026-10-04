//! Non-JSON transport contracts complement the typed application operation table.
//!
//! These declarations document streaming and authentication bindings; they do not implement them.

use serde::Serialize;

/// A transport route whose body is not an application JSON command.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct TransportOperation {
    /// Canonical operation identifier.
    pub id: &'static str,
    /// HTTP verb.
    pub method: &'static str,
    /// Public path template.
    pub path: &'static str,
    /// Intended transport behavior.
    pub description: &'static str,
    /// Accepted request media type, absent when there is no body.
    pub request_media: Option<&'static str>,
    /// Successful response media type.
    pub response_media: &'static str,
    /// Expected response status.
    pub status: u16,
    /// Public discovery and liveness routes do not require credentials.
    pub authenticated: bool,
}

/// The additional transport bindings the server lane must implement.
pub const TRANSPORTS: &[TransportOperation] = &[
    TransportOperation {
        id: "upload_content",
        method: "put",
        path: "/api/workspaces/{workspace_id}/uploads/{upload_id}/content",
        description: "Stream authenticated original bytes into a preallocated upload, enforcing byte limits and digest verification.",
        request_media: Some("application/octet-stream"),
        response_media: "application/json",
        status: 200,
        authenticated: true,
    },
    TransportOperation {
        id: "download_object",
        method: "get",
        path: "/api/workspaces/{workspace_id}/items/{item_id}/revisions/{revision}/objects/{digest}",
        description: "Read original or derived bytes authorized through an exact item revision. Support HTTP Range without accepting hash knowledge as permission.",
        request_media: None,
        response_media: "application/octet-stream",
        status: 200,
        authenticated: true,
    },
    TransportOperation {
        id: "download_artifact",
        method: "get",
        path: "/api/workspaces/{workspace_id}/artifacts/{artifact_id}",
        description: "Download a completed authorized export or backup.",
        request_media: None,
        response_media: "application/zip",
        status: 200,
        authenticated: true,
    },
    TransportOperation {
        id: "stream_events",
        method: "get",
        path: "/api/workspaces/{workspace_id}/events",
        description: "Resume workspace notifications with Last-Event-ID; notifications are projections, not canonical content.",
        request_media: None,
        response_media: "text/event-stream",
        status: 200,
        authenticated: true,
    },
    TransportOperation {
        id: "get_form_schema",
        method: "get",
        path: "/api/schemas/{name}",
        description: "Read a generated application schema by its allowed name.",
        request_media: None,
        response_media: "application/schema+json",
        status: 200,
        authenticated: true,
    },
    TransportOperation {
        id: "get_resource_metadata",
        method: "get",
        path: "/.well-known/oauth-protected-resource",
        description: "Publish the configured MCP resource and WorkOS authorization server without leaking credentials.",
        request_media: None,
        response_media: "application/json",
        status: 200,
        authenticated: false,
    },
    TransportOperation {
        id: "begin_local_session",
        method: "get",
        path: "/auth/local",
        description: "Local mode only: exchange the single-use launch token printed at startup for an HttpOnly, SameSite=Strict local-owner session cookie, then redirect. Rejected in hosted mode.",
        request_media: None,
        response_media: "text/html",
        status: 302,
        authenticated: false,
    },
    TransportOperation {
        id: "begin_login",
        method: "get",
        path: "/auth/login",
        description: "Hosted mode only: start the WorkOS browser session flow with CSRF state and PKCE.",
        request_media: None,
        response_media: "text/html",
        status: 302,
        authenticated: false,
    },
    TransportOperation {
        id: "complete_login",
        method: "get",
        path: "/auth/callback",
        description: "Hosted mode only: validate OAuth state and exchange the authorization code through WorkOS. Never log tokens or codes.",
        request_media: None,
        response_media: "text/html",
        status: 302,
        authenticated: false,
    },
    TransportOperation {
        id: "end_session",
        method: "post",
        path: "/auth/logout",
        description: "Invalidate the local or hosted browser session after checking CSRF protection.",
        request_media: None,
        response_media: "application/json",
        status: 200,
        authenticated: true,
    },
    TransportOperation {
        id: "liveness",
        method: "get",
        path: "/healthz",
        description: "Report process liveness, not feature acceptance or dependency readiness.",
        request_media: None,
        response_media: "application/json",
        status: 200,
        authenticated: false,
    },
    TransportOperation {
        id: "mcp_request",
        method: "post",
        path: "/mcp",
        description: "MCP Streamable HTTP endpoint owned by rmcp, including tools and UI resources.",
        request_media: Some("application/json"),
        response_media: "application/json",
        status: 200,
        authenticated: true,
    },
    TransportOperation {
        id: "mcp_events",
        method: "get",
        path: "/mcp",
        description: "Optional MCP server event stream managed by the SDK.",
        request_media: None,
        response_media: "text/event-stream",
        status: 200,
        authenticated: true,
    },
    TransportOperation {
        id: "mcp_close",
        method: "delete",
        path: "/mcp",
        description: "End an authenticated MCP session through the SDK.",
        request_media: None,
        response_media: "application/json",
        status: 200,
        authenticated: true,
    },
];

/// Enumerate the additional transport bindings the server lane must implement.
#[must_use]
pub fn operations() -> Vec<TransportOperation> {
    TRANSPORTS.to_vec()
}
