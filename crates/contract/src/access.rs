//! Authenticated principals and capabilities, never inferred from document labels.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How authenticated authority reached the application.
///
/// Local and hosted entry paths both resolve to one of these routes and share the same
/// authorization rules; no route is granted authority because the connection is loopback.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
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
#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
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

/// Upper bound on what a delegated client may do; it only narrows grants, never adds to them.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DelegationCeiling {
    /// Capabilities the delegation may exercise; effective permission is the intersection with grants.
    pub permissions: Vec<Permission>,
    /// Workspaces the delegation is limited to; absent means no restriction beyond the delegator's grants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_ids: Option<Vec<crate::identity::WorkspaceId>>,
}

/// Server-established identity; never accepted from a request body.
///
/// Identity is not authorization: effective permissions come from the access-control adapter
/// as workspace and tenant grants, intersected with any delegation ceiling.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Principal {
    /// Validated identity-provider subject.
    pub subject: String,
    /// Tenant boundary every storage scope for this principal carries.
    pub tenant_id: crate::identity::TenantId,
    /// Verified authentication route.
    pub route: AccessRoute,
    /// OAuth client when delegation is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Ceiling applied to delegated routes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegation: Option<DelegationCeiling>,
}

/// Current identity and capability summary without credentials.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionResponse {
    /// Authenticated caller.
    pub principal: Principal,
    /// Browser writes require a same-origin anti-CSRF token.
    pub csrf_required: bool,
}

/// Issue a scoped credential for a local MCP client; read-only unless propose is enabled.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateConnectorRequest {
    /// Owner-chosen name identifying the client, such as the host application.
    pub label: String,
    /// Workspaces the connector may read; each must already be accessible to the owner.
    pub workspace_ids: Vec<crate::identity::WorkspaceId>,
    /// Also grant proposal creation. Review and approval are never grantable to a connector.
    pub allow_propose: bool,
    /// Retry identity; a replay returns `already_issued`, never the secret again.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// A local MCP connector credential's scope, without its secret.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    /// Issue time.
    pub created_at: crate::identity::Timestamp,
    /// Revocation time; a revoked connector authenticates nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<crate::identity::Timestamp>,
}

/// A newly issued connector and its secret, which is returned exactly once and never stored in plain text.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssuedConnector {
    /// The issued connector's scope.
    pub connector: Connector,
    /// Bearer secret for the MCP client's configuration.
    pub secret: String,
}

/// List the installation's connector credentials.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListConnectorsRequest {
    /// Include revoked connectors for audit.
    pub include_revoked: bool,
}

/// Connector scopes, without secrets.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListConnectorsResponse {
    /// Matching connectors.
    pub connectors: Vec<Connector>,
}

/// Revoke one connector credential immediately.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeConnectorRequest {
    /// Connector to revoke.
    pub connector_id: crate::identity::ConnectorId,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// OAuth protected-resource metadata for external MCP clients.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
