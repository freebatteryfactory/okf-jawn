//! The session routes: the local launch-token exchange, hosted AuthKit sign-in, and logout.
//!
//! A session is opened in one place (`open`): a fresh secret from the operating system's random
//! source becomes the cookie, its SHA-256 the stored session id (`CredentialStore::insert_session`),
//! and a `signed_in` event naming the subject and route is appended to the tenant's log before the
//! cookies are sent, by core's one rule for it (`okf_jawn_core::events::record_sign_in`, which
//! `ApplicationService::record_sign_in` applies too); a sign-in that cannot be recorded is undone
//! and refused. A completed local sign-in tells the launch link to publish the next token
//! (`LaunchTokens::sign_in_completed`). Locally the session's
//! principal is the persistent installation identity with route `LocalOwner`; hosted it is the
//! AuthKit user in their organization with route `BrowserSession`. No other route ever gets a
//! session. A hosted sign-in in progress lives in its browser's cookie, never on the server
//! (`hosted`). Logout revokes the request's own session at once and clears both cookies. Every
//! response of these routes, a refusal included, is `no-store` and `no-referrer`, and no token,
//! code or secret is echoed or logged.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::header::{CACHE_CONTROL, LOCATION, REFERRER_POLICY, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use getrandom::fill;
use okf_jawn_contract::access::{AccessRoute, Principal};
use okf_jawn_contract::common::Empty;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{TenantId, Timestamp};
use okf_jawn_core::credentials::{CredentialStore, SessionRecord, secret_hash};
use okf_jawn_core::events::{EventLog, record_sign_in};
use time::OffsetDateTime;

use crate::SessionId;
use crate::cookies::{
    CookiePolicy, CrateCookies, cleared_cookies, cleared_login_cookie, login_cookie,
    session_cookies,
};
use crate::errors::{error_response, wrong_method};
use crate::hosted::{HostedLogin, LOGIN_COOKIE, MAX_CALLBACK_PARAMETER_CHARS, PendingLogin};
use crate::identity::{CookieReader as _, hex, session_id_of};
use crate::local::{LaunchTokens, SecretSource};

/// Secrets from the operating system's random source: 32 bytes, lowercase hex.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsSecrets;

/// The session routes' state.
#[derive(Clone)]
pub struct Sessions {
    inner: Arc<Inner>,
}

/// What the session routes are built from.
pub struct SessionParts {
    /// Where sessions are stored and the installation identity is read.
    pub credentials: Arc<dyn CredentialStore>,
    /// Where sign-ins are recorded.
    pub events: Arc<dyn EventLog>,
    /// Where cookie secrets come from.
    pub secrets: Arc<dyn SecretSource>,
    /// How the cookies are written: `Identity::cookie_policy` of the identity middleware the
    /// routes are served behind, so they write the cookie names it reads.
    pub policy: CookiePolicy,
    /// The deployment's entry path.
    pub entry: SessionEntry,
}

/// How a browser session begins.
#[derive(Clone)]
pub enum SessionEntry {
    /// The local launch-token exchange at `/auth/local`.
    Local {
        /// The outstanding launch tokens.
        tokens: Arc<LaunchTokens>,
    },
    /// Hosted AuthKit sign-in at `/auth/login` and `/auth/callback`. A sign-in in progress lives
    /// in its browser's cookie (`hosted`), not here.
    Hosted {
        /// The identity provider.
        login: Arc<dyn HostedLogin>,
    },
}

struct Inner {
    parts: SessionParts,
}

/// Where a signed-in browser lands.
const HOME: &str = "/";

/// The launch-token exchange (`api/transports.json` `begin_local_session`).
pub const LOCAL_PATH: &str = "/auth/local";

/// The start of a hosted sign-in (`begin_login`).
pub const LOGIN_PATH: &str = "/auth/login";

/// AuthKit's return (`complete_login`).
pub const CALLBACK_PATH: &str = "/auth/callback";

/// Logout (`end_session`).
pub const LOGOUT_PATH: &str = "/auth/logout";

impl SecretSource for OsSecrets {
    fn secret(&self) -> Result<String, ApiError> {
        let mut bytes = [0_u8; 32];
        fill(&mut bytes).map_err(|_| {
            ApiError::new(ErrorCode::Unavailable, "The random source is unavailable")
        })?;
        Ok(hex(&bytes))
    }
}

impl Sessions {
    /// The session routes' state over `parts`.
    #[must_use]
    pub fn new(parts: SessionParts) -> Self {
        Self {
            inner: Arc::new(Inner { parts }),
        }
    }

    /// Open a browser session for `principal` and answer with the redirect that sets its cookies.
    async fn open(&self, principal: Principal) -> Result<Response, ApiError> {
        let parts = &self.inner.parts;
        let secret = parts.secrets.secret()?;
        let session_id = session_id_of(&secret);
        let lifetime = time::Duration::try_from(parts.policy.lifetime())
            .map_err(|_| ApiError::new(ErrorCode::Internal, "The session lifetime is too long"))?;
        let expires = OffsetDateTime::now_utc()
            .checked_add(lifetime)
            .ok_or_else(|| ApiError::new(ErrorCode::Internal, "The session would never expire"))?;
        let expires_at = Timestamp::from_utc(expires)
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        parts
            .credentials
            .insert_session(SessionRecord {
                session_id: session_id.clone(),
                principal: principal.clone(),
                expires_at,
            })
            .await?;
        if let Err(error) = record_sign_in(parts.events.as_ref(), &principal).await {
            let _revoked = parts.credentials.revoke_session(&session_id).await;
            return Err(error);
        }
        let mut headers = private_headers();
        for cookie in session_cookies(&secret, parts.policy)? {
            headers.append(SET_COOKIE, cookie);
        }
        headers.insert(LOCATION, HeaderValue::from_static(HOME));
        Ok((StatusCode::FOUND, headers).into_response())
    }
}

/// The session routes of the deployment's entry path, and logout.
pub fn routes(sessions: Sessions) -> Router {
    let router = Router::new().route(LOGOUT_PATH, post(logout));
    let router = match sessions.inner.parts.entry {
        SessionEntry::Local { .. } => router.route(LOCAL_PATH, get(begin_local)),
        SessionEntry::Hosted { .. } => router
            .route(LOGIN_PATH, get(begin_login))
            .route(CALLBACK_PATH, get(complete_login)),
    };
    router.with_state(sessions)
}

/// Exchange the launch token for the local owner's session. Only a `GET` takes the token: a
/// `HEAD` (a link preview, a prefetch) is refused without touching it. An exchange that fails
/// after the token was taken gives it back.
async fn begin_local(
    State(sessions): State<Sessions>,
    method: Method,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    let SessionEntry::Local { tokens } = &sessions.inner.parts.entry else {
        return private(error_response(not_here()));
    };
    if method != Method::GET {
        return private(wrong_method().await);
    }
    let presented = query.get("token").map(String::as_str).unwrap_or_default();
    let taken = match tokens.redeem(presented) {
        Ok(Some(taken)) => taken,
        Ok(None) => {
            return private(error_response(ApiError::new(
                ErrorCode::Unauthenticated,
                "The launch link is not valid or was already used",
            )));
        }
        Err(error) => return private(error_response(error)),
    };
    let outcome = async {
        let owner = sessions
            .inner
            .parts
            .credentials
            .installation_identity()
            .await?;
        sessions
            .open(Principal {
                subject: owner.subject,
                tenant_id: TenantId::local(),
                route: AccessRoute::LocalOwner,
                client_id: None,
                delegation: None,
            })
            .await
    }
    .await;
    match outcome {
        Ok(response) => {
            // The launch link publishes the next token.
            tokens.sign_in_completed();
            private(response)
        }
        Err(error) => {
            // The first error is the answer; a failed give-back leaves the token spent.
            let _given_back = tokens.give_back(taken);
            private(error_response(error))
        }
    }
}

/// Start a hosted sign-in: redirect to the provider, and hand the browser the state and the
/// PKCE verifier in its sign-in cookie. Nothing is kept on the server.
async fn begin_login(State(sessions): State<Sessions>) -> Response {
    let SessionEntry::Hosted { login } = &sessions.inner.parts.entry else {
        return private(error_response(not_here()));
    };
    let outcome = login.begin().and_then(|start| {
        let pending = login_cookie(&PendingLogin::encode(&start)?)?;
        let location = HeaderValue::from_str(&start.url)
            .map_err(|_| ApiError::new(ErrorCode::Internal, "The sign-in URL cannot be sent"))?;
        let mut headers = private_headers();
        headers.insert(LOCATION, location);
        headers.append(SET_COOKIE, pending);
        Ok((StatusCode::FOUND, headers).into_response())
    });
    private(outcome.unwrap_or_else(error_response))
}

/// Complete a hosted sign-in for the browser that began it. Nothing reaches the provider unless
/// the request carries the sign-in cookie, well formed, with the state of the `state` parameter.
///
/// A callback that is not the one this browser's sign-in is waiting for (no cookie, a cookie
/// that is not a sign-in, or another state) is refused and leaves the cookie alone, so a sign-in
/// begun in another tab can still finish. Once the state matches, the cookie is cleared on
/// every outcome, success or refusal.
async fn complete_login(
    State(sessions): State<Sessions>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let SessionEntry::Hosted { login } = &sessions.inner.parts.entry else {
        return private(error_response(not_here()));
    };
    let refused = || {
        ApiError::new(
            ErrorCode::Unauthenticated,
            "The sign-in is not valid or has expired; start it again",
        )
    };
    let bounded = |name: &str| {
        query.get(name).filter(|value| {
            !value.is_empty() && value.chars().count() <= MAX_CALLBACK_PARAMETER_CHARS
        })
    };
    // Digests are compared, so the comparison's timing says nothing about the state.
    let pending = CrateCookies
        .read(&headers, LOGIN_COOKIE)
        .and_then(|value| PendingLogin::parse(&value))
        .filter(|pending| {
            bounded("state").is_some_and(|state| secret_hash(&pending.state) == secret_hash(state))
        });
    let Some(pending) = pending else {
        return private(error_response(refused()));
    };
    let cleared = match cleared_login_cookie() {
        Ok(cleared) => cleared,
        Err(error) => return private(error_response(error)),
    };
    let outcome = async {
        let code = bounded("code").ok_or_else(refused)?;
        let user = login.complete(code, &pending.verifier).await?;
        let tenant_id = TenantId::try_from(user.organization_id).map_err(|_| refused())?;
        sessions
            .open(Principal {
                subject: user.subject,
                tenant_id,
                route: AccessRoute::BrowserSession,
                client_id: None,
                delegation: None,
            })
            .await
    }
    .await;
    let mut response = private(outcome.unwrap_or_else(error_response));
    response.headers_mut().append(SET_COOKIE, cleared);
    response
}

/// End the request's own browser session; CSRF was checked by the identity middleware.
async fn logout(
    State(sessions): State<Sessions>,
    session: Option<Extension<SessionId>>,
) -> Response {
    let Some(Extension(SessionId(session_id))) = session else {
        return private(error_response(ApiError::new(
            ErrorCode::Unauthenticated,
            "There is no browser session to end",
        )));
    };
    let parts = &sessions.inner.parts;
    let outcome = async {
        parts.credentials.revoke_session(&session_id).await?;
        let mut headers = private_headers();
        for cookie in cleared_cookies(parts.policy.scheme())? {
            headers.append(SET_COOKIE, cookie);
        }
        Ok::<_, ApiError>((StatusCode::OK, headers, Json(Empty {})).into_response())
    }
    .await;
    private(outcome.unwrap_or_else(error_response))
}

/// `no-store` and `no-referrer`.
fn private_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers
}

/// `response` with `no-store` and `no-referrer`.
fn private(mut response: Response) -> Response {
    for (name, value) in &private_headers() {
        response.headers_mut().insert(name.clone(), value.clone());
    }
    response
}

fn not_here() -> ApiError {
    ApiError::new(
        ErrorCode::NotFound,
        "This deployment does not sign in this way",
    )
}
