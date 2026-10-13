//! The identity middleware: the only place a `Principal` or a `SessionId` enters a request.
//!
//! It is layered outside every router, and it first removes any such extension already on the
//! request, so no inner layer or route can supply one. Beside every `Principal` it inserts the
//! `Credential` the request authenticated with (a route reads one only with the other), and `Identity::still_authenticates` answers,
//! through the same functions the middleware authenticates with, whether that credential would
//! authenticate a new request; an event stream asks it before every poll. Each request takes exactly one entry path:
//!
//! - **Bearer** (`Authorization: Bearer ...`): locally a connector secret, resolved through
//!   `CredentialStore::lookup_connector` by `secret_hash`; hosted a WorkOS Connect access token
//!   (`ConnectVerifier`). Either becomes route `McpDelegation`, with the subject and the client
//!   kept separately. A bearer that does not authenticate is 401: it never falls back to the
//!   cookie. A hosted token the verifier cannot judge yet (`Unverified::retry_after`) is 503
//!   `Unavailable` with `Retry-After`, never a 401 that would send a good client back to sign
//!   in. The CLI, a connector and every other non-browser client arrive this way.
//! - **Browser session cookie** (`okf-session`): resolved through `CredentialStore::get_session`
//!   by the SHA-256 of its secret. Only this path may carry `BrowserSession` or `LocalOwner`
//!   (core lets those two routes reach drafts and decides by route alone, SPEC section 8), and a
//!   stored session with any other route, or of the other mode's route, is refused. A cookie
//!   request that comes from another origin or site (`Origin`, `Sec-Fetch-Site`) is not
//!   authenticated by its cookie; one that would change state is refused, as is a mutation whose
//!   `x-okf-csrf` header is not the session's anti-CSRF token, and the refusal is recorded as
//!   `permission_denied`.
//! - **None**: no `Principal`; every route that needs one answers 401.
//!
//! The MCP route accepts the bearer path only: a browser session never reaches it, and a request
//! without a bearer is answered 401 here, with the `WWW-Authenticate` challenge MCP clients read.
//! Loopback is never trust and there is no guest owner.

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, ORIGIN, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, HeaderValue, Method};
use axum::middleware::Next;
use axum::response::Response;
use okf_jawn_contract::access::{
    AccessRoute, CSRF_HEADER, Connector, DelegationCeiling, Principal,
};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::TenantId;
use okf_jawn_contract::metadata::{OperationName, operations};
use okf_jawn_core::credentials::{CredentialStore, secret_hash};
use okf_jawn_core::events::{EventLog, EventScope, NewEvent};

use crate::SessionId;
use crate::errors::{error_response, retry_response};

/// Verifies a WorkOS Connect access token: JWKS signature, issuer, audience and expiry.
///
/// The production verifier is the `jsonwebtoken` adapter over WorkOS's key set (Batch C); the
/// middleware is bound against this trait.
pub trait ConnectVerifier: Send + Sync {
    /// The identity a valid token carries. A token that is false is refused as
    /// `Unauthenticated`; a token that cannot be judged now is refused as `Unavailable`, with
    /// the seconds after which it can be.
    fn verify<'a>(&'a self, token: &'a str) -> Verifying<'a>;
}

/// A token verification under way.
pub type Verifying<'a> =
    Pin<Box<dyn Future<Output = Result<ConnectIdentity, Unverified>> + Send + 'a>>;

/// Why a bearer token was not accepted.
#[derive(Debug)]
pub struct Unverified {
    /// The refusal: `Unauthenticated` for a token that is false, `Unavailable` for one that
    /// cannot be judged now, or the fault that stopped the check.
    pub error: ApiError,
    /// Whole seconds after which the same token can be judged, when it could not be judged
    /// now; the middleware sends it as `Retry-After`.
    pub retry_after: Option<u32>,
}

/// Reads one cookie's value from request headers.
///
/// The production reader is the `cookie` crate (Batch C); the middleware is bound against this
/// trait, and no cookie header is parsed by hand.
pub trait CookieReader: Send + Sync {
    /// The value of the cookie `name`, when the request carries exactly that cookie.
    fn read(&self, headers: &HeaderMap, name: &str) -> Option<String>;
}

/// Where the middleware reads the wall-clock time a session's expiry is compared with.
pub trait WallClock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn unix_millis(&self) -> u128;
}

/// The operating system's wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemWallClock;

/// What a valid Connect access token identifies; it carries no workspace permission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectIdentity {
    /// The user the client acts for (`sub`).
    pub subject: String,
    /// The WorkOS organization (`org_id`), the tenant boundary.
    pub organization_id: String,
    /// The OAuth client (`client_id`), kept separately from the subject.
    pub client_id: String,
    /// When the token expires (`exp`), in seconds since the Unix epoch.
    pub expires_at: u64,
}

/// How a request authenticated. The middleware inserts it beside the `Principal`, so a response
/// that outlives its request (an event stream) can ask [`Identity::still_authenticates`] whether
/// the same credential would authenticate a new request. It holds no secret: a session by its
/// stored id, a connector by the SHA-256 of its secret, a Connect token by its expiry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// A browser session.
    Session {
        /// The stored session id (the SHA-256 of the cookie secret, in hex).
        session_id: String,
    },
    /// A local connector secret.
    Connector {
        /// The SHA-256 of the presented secret, as the store looks it up.
        secret_hash: [u8; 32],
        /// The connector the secret resolved to.
        connector_id: String,
    },
    /// A WorkOS Connect access token.
    Connect {
        /// The token's `exp`, in seconds since the Unix epoch.
        expires_at: u64,
    },
}

/// How a bearer token is resolved: by the deployment's entry path.
#[derive(Clone)]
pub enum BearerAuth {
    /// A connector secret of this installation, whose tenant is the fixed local one
    /// (`TenantId::local`).
    Local,
    /// A WorkOS Connect access token.
    Hosted {
        /// The token verifier.
        connect: Arc<dyn ConnectVerifier>,
        /// The protected-resource metadata URL named in a 401 challenge.
        resource_metadata: String,
    },
}

/// The identity middleware's state.
#[derive(Clone)]
pub struct Identity {
    inner: Arc<Inner>,
}

/// What the middleware reads identities from.
pub struct IdentityPorts {
    /// Connectors, sessions and the installation identity.
    pub credentials: Arc<dyn CredentialStore>,
    /// Where a refused cookie request is recorded.
    pub events: Arc<dyn EventLog>,
    /// How the session cookie is read.
    pub cookies: Arc<dyn CookieReader>,
}

struct Inner {
    bearer: BearerAuth,
    ports: IdentityPorts,
    /// Origins a cookie request may come from: the service's own public origin.
    origins: Vec<String>,
    clock: Arc<dyn WallClock>,
}

/// The browser session cookie (`api/openapi.json` `browserSession`).
pub const SESSION_COOKIE: &str = "okf-session";

/// The MCP endpoint path (`api/transports.json` `mcp_request`).
pub const MCP_PATH: &str = "/mcp";

impl From<ApiError> for Unverified {
    fn from(error: ApiError) -> Self {
        Self {
            error,
            retry_after: None,
        }
    }
}

impl fmt::Display for Unverified {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, formatter)
    }
}

impl std::error::Error for Unverified {}

impl WallClock for SystemWallClock {
    fn unix_millis(&self) -> u128 {
        // A clock before 1970 reads as the epoch: every session is then unexpired by this
        // check, and the store's own check still stands.
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis())
    }
}

impl Identity {
    /// The middleware state: bearer tokens resolved by `bearer`, sessions and connectors read
    /// through `ports`, and cookie requests accepted only from `origins`. Session expiry is
    /// compared with the operating system's clock.
    #[must_use]
    pub fn new(bearer: BearerAuth, ports: IdentityPorts, origins: Vec<String>) -> Self {
        Self::with_clock(bearer, ports, origins, Arc::new(SystemWallClock))
    }

    /// [`Identity::new`], reading the wall-clock time from `clock`.
    #[must_use]
    pub fn with_clock(
        bearer: BearerAuth,
        ports: IdentityPorts,
        origins: Vec<String>,
        clock: Arc<dyn WallClock>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                bearer,
                ports,
                origins,
                clock,
            }),
        }
    }

    /// Whether `credential` would authenticate a new request now: the session still stored, of
    /// this deployment's browser route and not expired; the connector still resolving, unrevoked,
    /// to the same connector; the Connect token not past its `exp`. The session and connector
    /// answers come from the very functions the middleware authenticates with
    /// (`live_session`, `live_connector`). A store fault answers `false`: a response that
    /// outlives its request ends when its credential cannot be confirmed.
    pub async fn still_authenticates(&self, credential: &Credential) -> bool {
        match credential {
            Credential::Session { session_id } => {
                matches!(self.live_session(session_id).await, Ok(Some(_)))
            }
            Credential::Connector {
                secret_hash,
                connector_id,
            } => matches!(
                self.live_connector(*secret_hash).await,
                Ok(Some(connector)) if connector.connector_id.0.to_string() == *connector_id
            ),
            Credential::Connect { expires_at } => {
                u128::from(*expires_at).saturating_mul(1000) > self.inner.clock.unix_millis()
            }
        }
    }

    /// The connector a secret's hash resolves to, when it is not revoked.
    async fn live_connector(&self, hash: [u8; 32]) -> Result<Option<Connector>, ApiError> {
        Ok(self
            .inner
            .ports
            .credentials
            .lookup_connector(hash)
            .await?
            .filter(|connector| connector.revoked_at.is_none()))
    }

    /// The principal of the stored session `session_id`, when that session is live: stored
    /// under that id, of the route this deployment's browser sessions take, and not expired.
    ///
    /// The port says the store answers only unexpired sessions; the server compares
    /// `expires_at` with its own clock as well, and a session is dead from that instant on.
    async fn live_session(&self, session_id: &str) -> Result<Option<Principal>, ApiError> {
        let Some(record) = self.inner.ports.credentials.get_session(session_id).await? else {
            return Ok(None);
        };
        let expected = match self.inner.bearer {
            BearerAuth::Local => AccessRoute::LocalOwner,
            BearerAuth::Hosted { .. } => AccessRoute::BrowserSession,
        };
        if record.principal.route != expected || record.session_id != session_id {
            return Ok(None);
        }
        let expires = record.expires_at.instant().map_err(|_| {
            ApiError::new(
                ErrorCode::Internal,
                "A stored session has no readable expiry",
            )
        })?;
        let expires_ms = expires
            .unix_timestamp_nanos()
            .checked_div(1_000_000)
            .unwrap_or(i128::MIN);
        let now_ms = i128::try_from(self.inner.clock.unix_millis()).unwrap_or(i128::MAX);
        if expires_ms <= now_ms {
            return Ok(None);
        }
        Ok(Some(record.principal))
    }

    /// The `Principal` a bearer token authenticates, and the credential it authenticated with.
    async fn bearer_principal(&self, token: &str) -> Result<(Principal, Credential), Unverified> {
        match &self.inner.bearer {
            BearerAuth::Local => {
                let hash = secret_hash(token);
                let connector = self.live_connector(hash).await?.ok_or_else(invalid_token)?;
                let owner = self.inner.ports.credentials.installation_identity().await?;
                let connector_id = connector.connector_id.0.to_string();
                let principal = Principal {
                    subject: owner.subject,
                    tenant_id: TenantId::local(),
                    route: AccessRoute::McpDelegation,
                    client_id: Some(connector_id.clone()),
                    delegation: Some(DelegationCeiling {
                        permissions: connector.permissions,
                        workspace_ids: Some(connector.workspace_ids),
                    }),
                };
                let credential = Credential::Connector {
                    secret_hash: hash,
                    connector_id,
                };
                Ok((principal, credential))
            }
            BearerAuth::Hosted { connect, .. } => {
                let identity = connect.verify(token).await?;
                let tenant_id =
                    TenantId::try_from(identity.organization_id).map_err(|_| invalid_token())?;
                let principal = Principal {
                    subject: identity.subject,
                    tenant_id,
                    route: AccessRoute::McpDelegation,
                    client_id: Some(identity.client_id),
                    delegation: None,
                };
                let credential = Credential::Connect {
                    expires_at: identity.expires_at,
                };
                Ok((principal, credential))
            }
        }
    }

    /// The session a cookie request carries, when its cookie names a live session whose route
    /// this deployment's browser sessions take.
    async fn session(&self, secret: &str) -> Result<Option<(Principal, SessionId)>, ApiError> {
        let session_id = session_id_of(secret);
        Ok(self
            .live_session(&session_id)
            .await?
            .map(|principal| (principal, SessionId(session_id))))
    }

    /// Whether a cookie request comes from this service's own origin and site.
    fn same_origin(&self, headers: &HeaderMap) -> bool {
        let origin_ok = headers.get(ORIGIN).is_none_or(|origin| {
            origin
                .to_str()
                .is_ok_and(|origin| self.inner.origins.iter().any(|allowed| allowed == origin))
        });
        let site_ok = headers
            .get("sec-fetch-site")
            .is_none_or(|site| matches!(site.to_str(), Ok("same-origin" | "none")));
        origin_ok && site_ok
    }

    /// The 401 a request without a usable bearer receives on the MCP route.
    fn challenge(&self, error: ApiError) -> Response {
        let value = match &self.inner.bearer {
            BearerAuth::Local => "Bearer".to_owned(),
            BearerAuth::Hosted {
                resource_metadata, ..
            } => format!("Bearer resource_metadata=\"{resource_metadata}\""),
        };
        // A challenge that cannot be written is this service's fault, never a 401 without one.
        let Ok(value) = HeaderValue::from_str(&value) else {
            return error_response(ApiError::new(
                ErrorCode::Internal,
                "The authentication challenge cannot be sent",
            ));
        };
        let mut response = error_response(error);
        response.headers_mut().insert(WWW_AUTHENTICATE, value);
        response
    }

    /// Record the refusal of a cookie-authenticated request in the tenant's log.
    async fn record_refusal(&self, principal: &Principal, path: &str) {
        let Some(operation) = operation_at(path) else {
            return;
        };
        let scope = EventScope::Tenant(principal.tenant_id.clone());
        let _recorded = self
            .inner
            .ports
            .events
            .append(
                &scope,
                None,
                NewEvent::permission_denied(principal, operation),
            )
            .await;
    }
}

/// The identity middleware: resolve the request's one entry path and insert what it proves.
pub async fn identify(
    State(identity): State<Identity>,
    mut request: Request,
    next: Next,
) -> Response {
    request.extensions_mut().remove::<Principal>();
    request.extensions_mut().remove::<SessionId>();
    let on_mcp = request.uri().path() == MCP_PATH;
    let headers = request.headers().clone();
    let headers = &headers;

    if let Some(authorization) = headers.get(AUTHORIZATION) {
        let Some(token) = bearer_token(authorization) else {
            return identity.challenge(ApiError::new(
                ErrorCode::Unauthenticated,
                "The Authorization header must be a Bearer token",
            ));
        };
        return match identity.bearer_principal(&token).await {
            Ok((principal, credential)) => {
                request.extensions_mut().insert(principal);
                request.extensions_mut().insert(credential);
                next.run(request).await
            }
            Err(refusal) if refusal.error.code == ErrorCode::Unauthenticated => {
                identity.challenge(refusal.error)
            }
            // A token that cannot be judged yet: try again, not sign in again.
            Err(Unverified {
                error,
                retry_after: Some(seconds),
            }) => retry_response(error, seconds),
            Err(refusal) => error_response(refusal.error),
        };
    }
    if on_mcp {
        return identity.challenge(ApiError::new(
            ErrorCode::Unauthenticated,
            "The MCP endpoint needs a bearer token",
        ));
    }
    let Some(secret) = identity.inner.ports.cookies.read(headers, SESSION_COOKIE) else {
        return next.run(request).await;
    };
    let session = match identity.session(&secret).await {
        Ok(session) => session,
        Err(error) => return error_response(error),
    };
    let Some((principal, session_id)) = session else {
        return next.run(request).await;
    };
    let mutation = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    let same_origin = identity.same_origin(headers);
    if !mutation && !same_origin {
        // A cross-origin or cross-site read is served as if no cookie had been sent.
        return next.run(request).await;
    }
    if mutation && !(same_origin && csrf_matches(headers, &secret)) {
        identity
            .record_refusal(&principal, request.uri().path())
            .await;
        return error_response(ApiError::new(
            ErrorCode::Forbidden,
            "A browser write needs its same-origin anti-CSRF token",
        ));
    }
    request.extensions_mut().insert(principal);
    request.extensions_mut().insert(Credential::Session {
        session_id: session_id.0.clone(),
    });
    request.extensions_mut().insert(session_id);
    next.run(request).await
}

/// The opaque session identity a cookie secret is stored and looked up under: the lowercase
/// hex SHA-256 of the secret, so the store never holds a usable cookie value.
#[must_use]
pub fn session_id_of(secret: &str) -> String {
    hex(&secret_hash(secret))
}

/// The anti-CSRF token of the session whose cookie secret is `secret`: derived from the secret,
/// so it needs no storage, and useless without the session cookie it belongs to.
#[must_use]
pub fn csrf_token_of(secret: &str) -> String {
    hex(&secret_hash(&format!("okf-csrf\n{secret}")))
}

/// Whether the request's `x-okf-csrf` header is the session's anti-CSRF token. Both sides are
/// compared as SHA-256 digests, so the comparison's timing says nothing about the token.
fn csrf_matches(headers: &HeaderMap, secret: &str) -> bool {
    headers
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|presented| secret_hash(presented) == secret_hash(&csrf_token_of(secret)))
}

/// The token of an `Authorization: Bearer <token>` header; `None` for any other form.
fn bearer_token(value: &HeaderValue) -> Option<String> {
    let text = value.to_str().ok()?;
    let (scheme, token) = text.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() && !token.contains(' '))
        .then(|| token.to_owned())
}

/// The canonical operation whose JSON route is `path`.
fn operation_at(path: &str) -> Option<OperationName> {
    let info = operations().into_iter().find(|info| info.path == path)?;
    OperationName::ALL
        .iter()
        .copied()
        .find(|name| name.as_str() == info.id)
}

fn invalid_token() -> ApiError {
    ApiError::new(
        ErrorCode::Unauthenticated,
        "The bearer token does not authenticate",
    )
}

/// Lowercase hexadecimal of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        for nibble in [byte >> 4, byte & 0x0f] {
            if let Some(digit) = DIGITS.get(usize::from(nibble)) {
                text.push(char::from(*digit));
            }
        }
    }
    text
}
