//! Authenticated principals and capabilities, never inferred from document labels.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// How authenticated authority reached the application.
///
/// Local and hosted entry paths both resolve to one of these routes and share the same
/// authorization rules; no route is granted authority because the connection is loopback.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccessRoute {
    /// Hosted Explorer session established through WorkOS AuthKit.
    BrowserSession,
    /// External agent: a WorkOS Connect token when hosted, a local connector credential when local.
    McpDelegation,
    /// The installation-local owner identity, authenticated through the local browser session.
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

/// Issue a scoped credential for a local MCP client; read-only unless propose is enabled.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateConnectorRequest {
    /// Owner-chosen name identifying the client, such as the host application.
    pub label: String,
    /// Workspaces the connector may read; each must already be accessible to the owner.
    pub workspace_ids: Vec<crate::identity::WorkspaceId>,
    /// Also grant proposal creation. Review and approval are never grantable to a connector.
    pub allow_propose: bool,
}

/// A local MCP connector credential's scope, without its secret.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Connector {
    /// Stable connector identity.
    pub connector_id: crate::identity::ConnectorId,
    /// Owner-chosen name.
    pub label: String,
    /// Workspaces the connector may access.
    pub workspace_ids: Vec<crate::identity::WorkspaceId>,
    /// Granted capabilities: `read`, plus `propose` only when enabled.
    pub permissions: Vec<Permission>,
    /// RFC 3339 issue time.
    pub created_at: String,
    /// RFC 3339 revocation time; a revoked connector authenticates nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
}

/// A newly issued connector and its secret, which is returned exactly once and never stored in plain text.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IssuedConnector {
    /// The issued connector's scope.
    pub connector: Connector,
    /// Bearer secret for the MCP client's configuration.
    pub secret: String,
}

/// List the installation's connector credentials.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListConnectorsRequest {
    /// Include revoked connectors for audit.
    pub include_revoked: bool,
}

/// Connector scopes, without secrets.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListConnectorsResponse {
    /// Matching connectors.
    pub connectors: Vec<Connector>,
}

/// Revoke one connector credential immediately.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeConnectorRequest {
    /// Connector to revoke.
    pub connector_id: crate::identity::ConnectorId,
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
