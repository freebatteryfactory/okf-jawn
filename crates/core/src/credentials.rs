//! Connector credentials, browser sessions, and installation identity are never inferred from content.
//!
//! `create_connector` takes `MutationId` and enforces uniqueness: inserting with a reused id
//! returns the prior connector without issuing a second secret.

use okf_jawn_contract::access::{
    Connector, CreateConnectorRequest, IssuedConnector, ListConnectorsResponse, Principal,
};
use okf_jawn_contract::identity::{ConnectorId, MutationId};

use crate::ports::PortFuture;

/// Persistent local installation owner identity (local auth mode only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationIdentity {
    /// Stable subject for the local owner principal.
    pub subject: String,
    /// RFC 3339 creation time.
    pub created_at: String,
}

/// Authenticated browser session record without exposing the cookie secret.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    /// Opaque session identity.
    pub session_id: String,
    /// Authenticated principal bound to the session.
    pub principal: Principal,
    /// RFC 3339 expiry.
    pub expires_at: String,
}

/// Connectors, sessions, and installation identity; storage owns the implementation.
pub trait CredentialStore: Send + Sync {
    /// Load or create the persistent local installation identity.
    fn installation_identity(&self) -> PortFuture<'_, InstallationIdentity>;
    /// Issue a local MCP connector credential; the secret is returned once.
    ///
    /// Unique on `mutation_id`. An abandoned retry returns the existing connector without
    /// issuing a second secret.
    fn create_connector<'a>(
        &'a self,
        mutation_id: MutationId,
        request: CreateConnectorRequest,
    ) -> PortFuture<'a, IssuedConnector>;
    /// Look up a connector created under `mutation_id`, for abandoned-lease reconciliation.
    fn find_connector_by_mutation(
        &self,
        mutation_id: MutationId,
    ) -> PortFuture<'_, Option<Connector>>;
    /// List connector metadata without secrets.
    fn list_connectors(&self) -> PortFuture<'_, ListConnectorsResponse>;
    /// Revoke a connector immediately.
    fn revoke_connector(&self, connector: ConnectorId) -> PortFuture<'_, Connector>;
    /// Resolve a connector secret to its non-secret record when still valid.
    fn lookup_connector<'a>(&'a self, secret: &'a str) -> PortFuture<'a, Option<Connector>>;
    /// Persist a browser session after authentication.
    fn insert_session(&self, session: SessionRecord) -> PortFuture<'_, SessionRecord>;
    /// Resolve a session by opaque identity when unexpired.
    fn get_session<'a>(&'a self, session_id: &'a str) -> PortFuture<'a, Option<SessionRecord>>;
    /// Invalidate a browser session.
    fn revoke_session<'a>(&'a self, session_id: &'a str) -> PortFuture<'a, ()>;
}
