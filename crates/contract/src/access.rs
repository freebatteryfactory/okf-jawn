//! Authenticated principals and capabilities, never inferred from document labels.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// How authenticated authority reached the application.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccessRoute {
    /// Authenticated Explorer session.
    BrowserSession,
    /// Delegated external-agent connection.
    McpDelegation,
    /// Explicit trusted local-owner session.
    LocalOwner,
    /// Configured internal service identity.
    Service,
}

/// Application capabilities checked on every operation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Read authorized workspace data.
    Read,
    /// Edit authorized workspace content.
    Write,
    /// Create or update proposals without merging.
    Propose,
    /// Accept proposals at exact preconditions.
    Approve,
    /// Record an explicit content review.
    Review,
    /// Manage deployment or workspace access.
    Admin,
}

/// Server-established identity; never accepted from a request body.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Principal {
    /// Validated identity-provider subject.
    pub subject: String,
    /// Verified authentication route.
    pub route: AccessRoute,
    /// Explicitly accessible workspaces.
    pub workspace_ids: Vec<crate::identity::WorkspaceId>,
    /// Resolved application capabilities.
    pub permissions: Vec<Permission>,
    /// OAuth client when delegation is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

/// Current identity and capability summary without credentials.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionResponse {
    /// Authenticated caller.
    pub principal: Principal,
    /// Browser writes require a same-origin anti-CSRF token.
    pub csrf_required: bool,
}

/// OAuth protected-resource metadata for external MCP clients.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceMetadata {
    /// Canonical externally reachable MCP resource URI.
    pub resource: String,
    /// Configured WorkOS issuer.
    pub authorization_servers: Vec<String>,
    /// Scopes actually configured; not invented permissions.
    pub scopes_supported: Vec<String>,
    /// Bearer methods accepted by the protected resource.
    pub bearer_methods_supported: Vec<String>,
}
