//! Test doubles of the server's identity ports: a credential store, a Connect verifier and a
//! cookie reader. Not production adapters; each answers only what a test put in it.

use std::collections::BTreeMap;
use std::sync::Mutex;

use axum::http::HeaderMap;
use okf_jawn_contract::access::{Connector, IssuedConnector, Permission};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{ConnectorId, MutationId, Timestamp, WorkspaceId};
use okf_jawn_core::credentials::{
    ConnectorIssue, CredentialStore, InstallationIdentity, NewConnector, SessionRecord, secret_hash,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_server::identity::{ConnectIdentity, ConnectVerifier, CookieReader};

/// Connectors by secret hash, sessions by id, and one installation identity.
#[derive(Default)]
pub struct FakeCredentials {
    connectors: Mutex<BTreeMap<[u8; 32], Connector>>,
    sessions: Mutex<BTreeMap<String, SessionRecord>>,
    revoked_sessions: Mutex<Vec<String>>,
}

/// A Connect verifier that accepts exactly one token.
pub struct OneToken {
    /// The token it accepts.
    pub token: String,
    /// What that token identifies.
    pub identity: ConnectIdentity,
}

/// A cookie reader that finds no cookie: the bearer and anonymous paths need none.
pub struct NoCookies;

/// The installation owner's subject.
pub const OWNER: &str = "owner-1";

/// A valid instant for records.
pub const INSTANT: &str = "2026-10-12T12:00:00.000Z";

impl FakeCredentials {
    /// Register a connector reachable with `secret`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned or a fixed value does not parse.
    pub fn add_connector(
        &self,
        secret: &str,
        id: &str,
        workspaces: Vec<WorkspaceId>,
        permissions: Vec<Permission>,
        revoked: bool,
    ) -> Result<Connector, Box<dyn std::error::Error>> {
        let connector = Connector {
            connector_id: serde_json::from_value(serde_json::Value::String(id.to_owned()))?,
            label: "test agent".to_owned(),
            workspace_ids: workspaces,
            permissions,
            created_at: Timestamp::try_from(INSTANT.to_owned())?,
            revoked_at: revoked
                .then(|| Timestamp::try_from(INSTANT.to_owned()))
                .transpose()?,
        };
        self.connectors
            .lock()
            .map_err(|_| poisoned())?
            .insert(secret_hash(secret), connector.clone());
        Ok(connector)
    }

    /// Store a session record as storage would.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn add_session(&self, record: SessionRecord) -> Result<(), ApiError> {
        self.sessions
            .lock()
            .map_err(|_| poisoned())?
            .insert(record.session_id.clone(), record);
        Ok(())
    }

    /// Every session id revoked so far.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn revoked_sessions(&self) -> Result<Vec<String>, ApiError> {
        Ok(self
            .revoked_sessions
            .lock()
            .map_err(|_| poisoned())?
            .clone())
    }
}

impl CredentialStore for FakeCredentials {
    fn installation_identity(&self) -> PortFuture<'_, InstallationIdentity> {
        Box::pin(async {
            Ok(InstallationIdentity {
                subject: OWNER.to_owned(),
                created_at: Timestamp::try_from(INSTANT.to_owned())
                    .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?,
            })
        })
    }

    fn create_connector(
        &self,
        _mutation_id: MutationId,
        _connector: NewConnector,
    ) -> PortFuture<'_, ConnectorIssue> {
        Box::pin(async { Err(unused("create_connector")) })
    }

    fn rotate_connector_secret(&self, _connector: ConnectorId) -> PortFuture<'_, IssuedConnector> {
        Box::pin(async { Err(unused("rotate_connector_secret")) })
    }

    fn list_connectors(&self, _include_revoked: bool) -> PortFuture<'_, Vec<Connector>> {
        Box::pin(async { Err(unused("list_connectors")) })
    }

    fn revoke_connector(&self, _connector: ConnectorId) -> PortFuture<'_, Connector> {
        Box::pin(async { Err(unused("revoke_connector")) })
    }

    fn lookup_connector(&self, hash: [u8; 32]) -> PortFuture<'_, Option<Connector>> {
        Box::pin(async move {
            Ok(self
                .connectors
                .lock()
                .map_err(|_| poisoned())?
                .get(&hash)
                .cloned())
        })
    }

    fn insert_session(&self, session: SessionRecord) -> PortFuture<'_, SessionRecord> {
        Box::pin(async move {
            self.add_session(session.clone())?;
            Ok(session)
        })
    }

    fn get_session<'a>(&'a self, session_id: &'a str) -> PortFuture<'a, Option<SessionRecord>> {
        Box::pin(async move {
            Ok(self
                .sessions
                .lock()
                .map_err(|_| poisoned())?
                .get(session_id)
                .cloned())
        })
    }

    fn revoke_session<'a>(&'a self, session_id: &'a str) -> PortFuture<'a, ()> {
        Box::pin(async move {
            self.sessions
                .lock()
                .map_err(|_| poisoned())?
                .remove(session_id);
            self.revoked_sessions
                .lock()
                .map_err(|_| poisoned())?
                .push(session_id.to_owned());
            Ok(())
        })
    }
}

impl ConnectVerifier for OneToken {
    fn verify<'a>(&'a self, token: &'a str) -> PortFuture<'a, ConnectIdentity> {
        Box::pin(async move {
            if token == self.token {
                Ok(self.identity.clone())
            } else {
                Err(ApiError::new(ErrorCode::Unauthenticated, "invalid token"))
            }
        })
    }
}

impl CookieReader for NoCookies {
    fn read(&self, _headers: &HeaderMap, _name: &str) -> Option<String> {
        None
    }
}

fn poisoned() -> ApiError {
    ApiError::new(ErrorCode::Internal, "fake credentials lock poisoned")
}

fn unused(what: &str) -> ApiError {
    ApiError::new(
        ErrorCode::Internal,
        format!("{what} is not used by these tests"),
    )
}

#[cfg(test)]
mod tests {
    use okf_jawn_contract::access::{AccessRoute, Principal};
    use okf_jawn_contract::identity::{TenantId, Timestamp};
    use okf_jawn_core::credentials::{CredentialStore, SessionRecord};

    use super::{FakeCredentials, INSTANT, NoCookies, OWNER};
    use crate::check::TestResult;

    #[tokio::test]
    async fn one_token_accepts_only_its_token() -> TestResult {
        let verifier = super::OneToken {
            token: "t".to_owned(),
            identity: super::ConnectIdentity {
                subject: OWNER.to_owned(),
                organization_id: "org".to_owned(),
                client_id: "client".to_owned(),
            },
        };
        assert_eq!(
            super::ConnectVerifier::verify(&verifier, "t")
                .await?
                .subject,
            OWNER
        );
        assert!(
            super::ConnectVerifier::verify(&verifier, "u")
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_stored_session_is_found_until_it_is_revoked() -> TestResult {
        let credentials = FakeCredentials::default();
        credentials.add_session(SessionRecord {
            session_id: "s".to_owned(),
            principal: Principal {
                subject: OWNER.to_owned(),
                tenant_id: TenantId::try_from("local".to_owned())?,
                route: AccessRoute::LocalOwner,
                client_id: None,
                delegation: None,
            },
            expires_at: Timestamp::try_from(INSTANT.to_owned())?,
        })?;
        assert!(credentials.get_session("s").await?.is_some());
        credentials.revoke_session("s").await?;
        assert!(credentials.get_session("s").await?.is_none());
        assert_eq!(credentials.revoked_sessions()?, vec!["s".to_owned()]);
        assert!(
            super::CookieReader::read(&NoCookies, &axum::http::HeaderMap::new(), "x").is_none()
        );
        Ok(())
    }
}
