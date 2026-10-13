//! WorkOS Connect access-token verification: JWKS signature, issuer, audience, expiry and
//! not-before.
//!
//! SPEC section 11: "validate JWKS signatures/issuer/audience/expiry ... Connect access tokens
//! identify the caller (subject, org, client) and do not carry workspace permissions". A token is
//! verified with `jsonwebtoken` against the key its `kid` names in the issuer's key set
//! (`{issuer}/oauth2/jwks`, fetched through the `workos` crate's transport). Only RS256 is
//! accepted, and a token of another algorithm is refused before any key is looked up, so it can
//! neither choose a weaker algorithm nor cause a fetch. The issuer must be the configured AuthKit
//! issuer and the audience the configured MCP resource; `exp`, `iss`, `aud` and `sub` are
//! required, `exp` and `nbf` are checked with 30 seconds of leeway, and `sub`, `org_id` and
//! `client_id` must be present and not empty. A refused token is `Unauthenticated`, in the same
//! words whatever the reason, and no token or claim is echoed.
//!
//! The key set is cached here, and its timing is this module's, read from an injected clock:
//! - the issuer is asked at most once per [`REFRESH_INTERVAL`] (60 seconds), whatever arrives: an
//!   unknown `kid` inside that interval is refused from the cached set with no fetch, so a flood
//!   of forged tokens costs the issuer one request a minute;
//! - a set older than [`KEY_SET_MAX_AGE`] (10 minutes) is refreshed at its next use, so a key the
//!   issuer withdrew stops being trusted;
//! - when a refresh fails the cached set stays in use, for at most [`KEY_SET_KEPT`] (one hour)
//!   after the last successful fetch; after that, and when no set was ever fetched, the answer is
//!   `Unavailable`.
//!
//! One fetch runs at a time and verification waits for it. Known limit: the fetch follows
//! whatever redirect policy the transport it is given has. [`IssuerKeys`] cannot see or refuse a
//! redirect through the `workos` transport interface; the composition must hand it a transport
//! whose client follows none (`ReqwestTransport::from_client`), which needs `reqwest` in this
//! crate's manifest (reported as a request).
//!
//! Only public-key signature verification happens here: no private-key operation is ever
//! performed in production, so the `rsa` crate's private-key decryption timing advisory
//! (RUSTSEC-2023-0071) does not reach this code. `install_crypto` selects jsonwebtoken's
//! `rust_crypto` provider explicitly before any verification, so a crate that later enables
//! another provider cannot leave verification without one.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, Method};
use jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::ports::PortFuture;
use serde::Deserialize;
use workos::transport::{HttpRequest, SharedTransport};

use crate::config::HostedConfig;
use crate::grants::Clock;
use crate::identity::{ConnectIdentity, ConnectVerifier};

/// Where verification keys come from; caching and its timing are [`ConnectTokens`]'s.
pub trait KeySource: Send + Sync {
    /// Fetch the issuer's current key set.
    fn fetch(&self) -> PortFuture<'_, JwkSet>;
}

/// The issuer's published key set, fetched with the `workos` crate's HTTP transport.
pub struct IssuerKeys {
    transport: SharedTransport,
    url: String,
}

/// Connect token verification against a key source.
pub struct ConnectTokens {
    keys: Arc<dyn KeySource>,
    clock: Arc<dyn Clock>,
    issuer: String,
    audience: String,
    cache: tokio::sync::Mutex<KeyCache>,
}

/// The cached key set and when the issuer was last asked.
#[derive(Default)]
struct KeyCache {
    set: Option<JwkSet>,
    /// The last fetch that succeeded.
    fetched_at: Option<Instant>,
    /// The last fetch that was tried, whether or not it succeeded.
    attempted_at: Option<Instant>,
}

/// The claims a Connect access token must carry for this service.
#[derive(Debug, Deserialize)]
struct ConnectClaims {
    sub: String,
    org_id: String,
    client_id: String,
    exp: u64,
}

/// Seconds of clock difference tolerated on `exp` and `nbf`.
pub const LEEWAY_SECONDS: u64 = 30;

/// The issuer is asked for its keys at most once in this interval.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// A cached key set this old is refreshed at its next use.
pub const KEY_SET_MAX_AGE: Duration = Duration::from_secs(600);

/// How long after its last successful fetch a key set stays in use while refreshes fail.
pub const KEY_SET_KEPT: Duration = Duration::from_secs(3600);

impl IssuerKeys {
    /// The key set at `{issuer}/oauth2/jwks`, fetched through `transport`.
    #[must_use]
    pub fn new(transport: SharedTransport, issuer: &str) -> Self {
        Self {
            transport,
            url: format!("{}/oauth2/jwks", issuer.trim_end_matches('/')),
        }
    }
}

impl KeySource for IssuerKeys {
    fn fetch(&self) -> PortFuture<'_, JwkSet> {
        Box::pin(async move {
            let response = self
                .transport
                .execute(HttpRequest {
                    method: Method::GET,
                    url: self.url.clone(),
                    headers: HeaderMap::new(),
                    body: None,
                })
                .await
                .map_err(|_| unavailable())?;
            if !response.status.is_success() {
                return Err(unavailable());
            }
            serde_json::from_slice(&response.body).map_err(|_| unavailable())
        })
    }
}

impl ConnectTokens {
    /// A verifier for tokens `config.issuer` issued for `config.resource`, keyed by `keys`, with
    /// the key set's timing read from `clock`.
    ///
    /// # Errors
    /// Returns `install_crypto`'s error.
    pub fn new(
        keys: Arc<dyn KeySource>,
        config: &HostedConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ApiError> {
        install_crypto()?;
        Ok(Self {
            keys,
            clock,
            issuer: config.issuer.clone(),
            audience: config.resource.clone(),
            cache: tokio::sync::Mutex::new(KeyCache::default()),
        })
    }

    /// The verification key `kid` names, by the timing rules of the module header.
    async fn key(&self, kid: &str) -> Result<DecodingKey, ApiError> {
        let now = self.clock.now();
        let since = |at: Option<Instant>| at.and_then(|at| now.checked_duration_since(at));
        // Held across the fetch: one fetch at a time, and no second decision on a half-updated
        // cache.
        let mut cache = self.cache.lock().await;
        let known = cache
            .set
            .as_ref()
            .is_some_and(|set| set.find(kid).is_some());
        let stale = since(cache.fetched_at).is_none_or(|age| age >= KEY_SET_MAX_AGE);
        let may_refresh = since(cache.attempted_at).is_none_or(|waited| waited >= REFRESH_INTERVAL);
        if may_refresh && (stale || !known) {
            cache.attempted_at = Some(now);
            // The one error tolerated here: a failed refresh leaves the cached set in use, for
            // as long as `KEY_SET_KEPT` allows below.
            if let Ok(set) = self.keys.fetch().await {
                cache.set = Some(set);
                cache.fetched_at = Some(now);
            }
        }
        let kept = since(cache.fetched_at).is_some_and(|age| age < KEY_SET_KEPT);
        let set = cache
            .set
            .as_ref()
            .filter(|_| kept)
            .ok_or_else(unavailable)?;
        let jwk = set.find(kid).ok_or_else(refused)?;
        DecodingKey::from_jwk(jwk).map_err(|_| refused())
    }

    async fn identity(&self, token: &str) -> Result<ConnectIdentity, ApiError> {
        let header = decode_header(token).map_err(|_| refused())?;
        // Before any key lookup: a token of another algorithm never causes a fetch.
        if header.alg != Algorithm::RS256 {
            return Err(refused());
        }
        let kid = header.kid.ok_or_else(refused)?;
        let key = self.key(&kid).await?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.audience.as_str()]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.leeway = LEEWAY_SECONDS;
        // Public-key signature verification only; no private-key operation happens here.
        let claims = decode::<ConnectClaims>(token, &key, &validation)
            .map_err(|_| refused())?
            .claims;
        if claims.sub.is_empty() || claims.org_id.is_empty() || claims.client_id.is_empty() {
            return Err(refused());
        }
        Ok(ConnectIdentity {
            subject: claims.sub,
            organization_id: claims.org_id,
            client_id: claims.client_id,
            expires_at: claims.exp,
        })
    }
}

impl ConnectVerifier for ConnectTokens {
    fn verify<'a>(&'a self, token: &'a str) -> PortFuture<'a, ConnectIdentity> {
        Box::pin(self.identity(token))
    }
}

/// Select jsonwebtoken's `rust_crypto` provider for this process, once.
///
/// Calling it again is harmless. It fails when another provider was selected first, by any code
/// in the process, before this service's first call.
///
/// # Errors
/// Returns `Internal` when another JWT crypto provider is already installed.
pub fn install_crypto() -> Result<(), ApiError> {
    static INSTALLED: OnceLock<bool> = OnceLock::new();
    if *INSTALLED.get_or_init(|| DEFAULT_PROVIDER.install_default().is_ok()) {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::Internal,
            "Another JWT crypto provider was installed before this service's",
        ))
    }
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::Unauthenticated,
        "The bearer token does not authenticate",
    )
}

fn unavailable() -> ApiError {
    ApiError::new(
        ErrorCode::Unavailable,
        "The token issuer's keys are unavailable",
    )
}
