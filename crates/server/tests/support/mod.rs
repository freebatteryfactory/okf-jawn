//! Test doubles of the server's identity ports (a credential store and a Connect verifier) and a
//! local-mode service over fixture ports. Not production adapters; each answers only what a
//! test put in it. The including target declares `mod fixtures;` (core's test support) at its root.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use okf_jawn_contract::access::{Connector, IssuedConnector, Permission};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{ConnectorId, MutationId, Timestamp, WorkspaceId};
use okf_jawn_core::credentials::{
    ConnectorIssue, CredentialStore, InstallationIdentity, NewConnector, SessionRecord, secret_hash,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_server::BoundApplication;
use okf_jawn_server::app::{ServiceParts, service};
use okf_jawn_server::cookies::CrateCookies;
use okf_jawn_server::identity::{
    BearerAuth, ConnectIdentity, ConnectVerifier, Identity, IdentityPorts, Verifying,
};

use crate::fixtures::counting::CountingApplication;
use crate::fixtures::{FixturePorts, GrantTable, all_permissions, workspace};

/// Connectors by secret hash, sessions by id, and one installation identity.
#[derive(Default)]
pub struct FakeCredentials {
    connectors: Mutex<BTreeMap<[u8; 32], Connector>>,
    sessions: Mutex<BTreeMap<String, SessionRecord>>,
    revoked_sessions: Mutex<Vec<String>>,
    /// While set, every session and connector lookup and every session insert fails, as a
    /// store that is down.
    down: AtomicBool,
}

/// A Connect verifier that accepts exactly one token.
pub struct OneToken {
    /// The token it accepts.
    pub token: String,
    /// What that token identifies.
    pub identity: ConnectIdentity,
}

/// A local-mode service over fixture ports: the owner holds every permission on [`WORKSPACE`],
/// and one read-only connector is reachable with [`SECRET`].
pub struct LocalApp {
    /// The application double.
    pub counting: Arc<CountingApplication>,
    /// The credential store double.
    pub credentials: Arc<FakeCredentials>,
    /// Dispatch's fixture ports.
    pub ports: FixturePorts,
}

/// The installation owner's subject.
pub const OWNER: &str = "owner-1";

/// A valid instant for records.
pub const INSTANT: &str = "2026-10-12T12:00:00.000Z";

/// The `Host` the test service answers.
pub const HOST: &str = "127.0.0.1:7711";

/// The test service's own origin.
pub const ORIGIN: &str = "http://127.0.0.1:7711";

/// The sandbox origin the test service's pages may frame.
pub const SANDBOX_ORIGIN: &str = "http://127.0.0.1:8788";

/// The workspace the owner and the connector may read.
pub const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";

/// The connector's identity.
pub const CONNECTOR: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

/// The connector's bearer secret.
pub const SECRET: &str = "connector-secret-of-the-test";

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

    /// Store `record` under `key`, whatever session the record itself names: a store that
    /// answers the wrong record.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn add_session_under(&self, key: &str, record: SessionRecord) -> Result<(), ApiError> {
        self.sessions
            .lock()
            .map_err(|_| poisoned())?
            .insert(key.to_owned(), record);
        Ok(())
    }

    /// Take the store down (`true`), so every lookup and every session insert fails, or bring
    /// it back (`false`).
    pub fn set_down(&self, down: bool) {
        self.down.store(down, Ordering::SeqCst);
    }

    /// The fault a lookup or an insert answers while the store is down.
    fn fault(&self) -> Result<(), ApiError> {
        if self.down.load(Ordering::SeqCst) {
            return Err(ApiError::new(ErrorCode::Unavailable, "the store is down"));
        }
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
            self.fault()?;
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
            self.fault()?;
            self.add_session(session.clone())?;
            Ok(session)
        })
    }

    fn get_session<'a>(&'a self, session_id: &'a str) -> PortFuture<'a, Option<SessionRecord>> {
        Box::pin(async move {
            self.fault()?;
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
    fn verify<'a>(&'a self, token: &'a str) -> Verifying<'a> {
        Box::pin(async move {
            if token == self.token {
                Ok(self.identity.clone())
            } else {
                Err(ApiError::new(ErrorCode::Unauthenticated, "invalid token").into())
            }
        })
    }
}

impl LocalApp {
    /// The fixtures, with the connector registered.
    ///
    /// # Errors
    /// Returns when a fixed value does not parse.
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mut table = GrantTable::default();
        table
            .workspaces
            .entry(OWNER.to_owned())
            .or_default()
            .insert(workspace(WORKSPACE)?, all_permissions());
        let credentials = Arc::new(FakeCredentials::default());
        credentials.add_connector(
            SECRET,
            CONNECTOR,
            vec![workspace(WORKSPACE)?],
            vec![Permission::Read],
            false,
        )?;
        Ok(Self {
            counting: Arc::new(CountingApplication::new()),
            credentials,
            ports: FixturePorts::new(table),
        })
    }

    /// The application and dispatch ports the routes are bound to.
    #[must_use]
    pub fn bound(&self) -> BoundApplication {
        BoundApplication {
            application: self.counting.clone(),
            access: self.ports.access.clone(),
            mutations: self.ports.mutations.clone(),
            events: self.ports.events.clone(),
        }
    }

    /// The local identity middleware state over these fixtures, reading real cookies.
    #[must_use]
    pub fn identity(&self) -> Identity {
        Identity::new(
            BearerAuth::Local,
            IdentityPorts {
                credentials: self.credentials.clone(),
                events: self.ports.events.clone(),
                cookies: Arc::new(CrateCookies),
            },
            vec![ORIGIN.to_owned()],
        )
    }

    /// The local service with `extra` routes, the given CORS origins and UI directory.
    ///
    /// # Errors
    /// Returns the composition's refusal.
    pub fn service(
        &self,
        extra: Router,
        cors_origins: Vec<String>,
        ui_dir: Option<PathBuf>,
    ) -> Result<Router, ApiError> {
        self.service_behind(self.identity(), extra, cors_origins, ui_dir)
    }

    /// The service with `extra` routes behind `identity`, the one middleware state its session
    /// routes, if any, were given their cookie policy by (`Identity::cookie_policy`).
    ///
    /// # Errors
    /// Returns the composition's refusal.
    pub fn service_behind(
        &self,
        identity: Identity,
        extra: Router,
        cors_origins: Vec<String>,
        ui_dir: Option<PathBuf>,
    ) -> Result<Router, ApiError> {
        service(ServiceParts {
            identity,
            ..self.parts(extra, cors_origins, ui_dir)
        })
    }

    /// The parts of the local service: `extra` routes, the given CORS origins and UI
    /// directory, its pages framing the sandbox origin `SANDBOX_ORIGIN`.
    pub fn parts(
        &self,
        extra: Router,
        cors_origins: Vec<String>,
        ui_dir: Option<PathBuf>,
    ) -> ServiceParts {
        ServiceParts {
            bound: self.bound(),
            identity: self.identity(),
            hosts: vec![HOST.to_owned()],
            hosted: None,
            cors_origins,
            ui_dir,
            sandbox_origin: Some(SANDBOX_ORIGIN.to_owned()),
            extra,
        }
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

    use super::{FakeCredentials, HOST, INSTANT, LocalApp, OWNER};
    use crate::check::TestResult;

    #[tokio::test]
    async fn the_local_app_serves_liveness() -> TestResult {
        use tower::ServiceExt as _;
        let local = LocalApp::new()?;
        let app = local.service(axum::Router::new(), Vec::new(), None)?;
        let request = axum::http::Request::get("/healthz")
            .header("host", HOST)
            .body(axum::body::Body::empty())?;
        assert_eq!(
            app.oneshot(request).await?.status(),
            axum::http::StatusCode::OK
        );
        assert_eq!(local.counting.call_count("get_health")?, 0);
        Ok(())
    }

    #[tokio::test]
    async fn one_token_accepts_only_its_token() -> TestResult {
        let verifier = super::OneToken {
            token: "t".to_owned(),
            identity: super::ConnectIdentity {
                subject: OWNER.to_owned(),
                organization_id: "org".to_owned(),
                client_id: "client".to_owned(),
                expires_at: u64::MAX,
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
        let other = credentials.get_session("s").await?.ok_or("the session")?;
        credentials.add_session_under("t", other)?;
        assert!(credentials.get_session("t").await?.is_some());
        // A store that is down answers no lookup, and answers again when it is back.
        credentials.set_down(true);
        assert!(credentials.get_session("s").await.is_err());
        assert!(credentials.lookup_connector([0; 32]).await.is_err());
        let record = credentials.get_session("t").await;
        assert!(record.is_err());
        credentials.set_down(false);
        assert!(credentials.lookup_connector([0; 32]).await?.is_none());
        credentials.revoke_session("s").await?;
        assert!(credentials.get_session("s").await?.is_none());
        assert_eq!(credentials.revoked_sessions()?, vec!["s".to_owned()]);
        Ok(())
    }
}
