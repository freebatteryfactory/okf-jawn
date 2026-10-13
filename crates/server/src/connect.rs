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
//! `client_id` must be present and not empty. A token that is false is `Unauthenticated`, in the
//! same words whatever the reason, and no token or claim is echoed.
//!
//! The key set is cached here, and its timing is this module's, read from an injected clock:
//! - the issuer is asked at most once per [`REFRESH_INTERVAL`] (60 seconds), whatever arrives: an
//!   unknown `kid` inside that interval is answered from the cached set with no fetch, so a flood
//!   of forged tokens costs the issuer one request a minute;
//! - a set older than [`KEY_SET_MAX_AGE`] (10 minutes) is refreshed at its next use, so a key the
//!   issuer withdrew stops being trusted;
//! - when a refresh fails the cached set stays in use, for at most [`KEY_SET_KEPT`] (one hour)
//!   after the last successful fetch; after that, and when no set was ever fetched, the answer is
//!   `Unavailable`.
//!
//! A fetch holds up one request only. The cache's lock is held to decide and to store, never
//! while the issuer is asked. The clock is read under that lock, and an instant later than the
//! reading counts as no time at all, so of any number of requests at the same moment exactly
//! one takes the minute's attempt. A set is stamped with the instant its fetch began, and an
//! answer never replaces a set whose fetch began later. The request that takes the minute's attempt (its `kid` is unknown,
//! or the set is stale) waits for that fetch; every other request is answered at once from the
//! cached set as it stands, so a token whose key is cached is verified however slow the issuer
//! is, and an unknown `kid` that arrives during a fetch, or inside the minute after one, causes
//! no second fetch. The fetch is bounded by [`FETCH_TIME_LIMIT`] (10 seconds), this module's own
//! limit whatever the transport's is; a fetch that exceeds it is a failed fetch. It runs in a
//! task of its own, so a caller that goes away after taking the minute's attempt does not take
//! the refresh with it.
//!
//! A token the server cannot judge yet is not called false. It is refused as `Unavailable`
//! with a wait (`Unverified::retry_after`, sent as `Retry-After`), in two cases the words tell
//! apart:
//! - the token names a key this server does not have yet: its `kid` is not in the cached set
//!   and the issuer may not be asked. While a fetch is under way the wait is
//!   [`FETCH_RETRY_SECONDS`]: an ordinary fetch answers in well under a second, so the retry
//!   finds its answer, and a fetch lasts ten seconds at most, so a client is told this at most
//!   five times. Inside the minute after a fetch the wait is the whole seconds, rounded up,
//!   until the issuer may be asked again;
//! - the issuer's keys could not be fetched: there is no usable set at all, with the same wait.
//!
//! After a key rotation a good token would otherwise be refused as bad for up to a minute and
//! send its client back to sign in for nothing. Only the request whose own fetch just succeeded
//! without bringing the `kid` is told the token is false; a fetch that failed, ran out of time
//! or did not finish judged nothing. A token that fails for any other reason (algorithm,
//! signature, claims) is `Unauthenticated` as before.
//!
//! What this costs, by ruling: under a flood of forged tokens each minute's attempt is taken by
//! the flood, so a false token that names an unknown key is told "cannot judge yet" for as long
//! as the flood lasts, never "false". It is never accepted, and a good token is never refused;
//! the cost is a client that keeps retrying. The answer carries no secret (a key id is or is not
//! in a set the issuer publishes to everyone) and causes no fetch. The cure is a rate limit at
//! the ingress, carried to the next server package.
//!
//! Also by ruling: the request that takes the attempt on a set ten minutes old waits for that
//! fetch, at most ten seconds, though its own key is cached, so that a key the issuer withdrew
//! is refused at the first request after ten minutes. And one known wart: if a key source
//! panics after the caller that asked went away, the short wait is told until the next fetch
//! ends.
//!
//! Known limit: the fetch follows whatever redirect policy the transport it is given has.
//! [`IssuerKeys`] cannot see or refuse a redirect through the `workos` transport interface; the
//! composition must hand it a transport whose client follows none
//! (`ReqwestTransport::from_client`), which needs `reqwest` in this crate's manifest (reported as
//! a request).
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
use tokio::sync::Mutex;
use tokio::time::timeout;
use workos::transport::{HttpRequest, SharedTransport};

use crate::config::HostedConfig;
use crate::grants::Clock;
use crate::identity::{ConnectIdentity, ConnectVerifier, Unverified, Verifying};

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
    /// Locked to decide and to store, never while the issuer is asked.
    cache: Arc<Mutex<KeyCache>>,
}

/// The cached key set and when the issuer was last asked.
#[derive(Default)]
struct KeyCache {
    set: Option<JwkSet>,
    /// The last fetch that succeeded.
    fetched_at: Option<Instant>,
    /// The last fetch that was tried, whether or not it succeeded.
    attempted_at: Option<Instant>,
    /// A fetch is under way: set when an attempt is taken, cleared when its fetch ends.
    fetching: bool,
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

/// The longest one fetch of the key set may take, whatever the transport's own limit; a fetch
/// that exceeds it is a failed fetch.
pub const FETCH_TIME_LIMIT: Duration = Duration::from_secs(10);

/// The `Retry-After`, in seconds, of a token that cannot be judged while a fetch is under way.
/// An ordinary fetch answers in well under a second, so two seconds is after it; a fetch lasts
/// [`FETCH_TIME_LIMIT`] at most, so a client is told this at most five times.
pub const FETCH_RETRY_SECONDS: u32 = 2;

impl KeyCache {
    /// Whether a request naming `kid` at `now` is the one that asks the issuer: the minute's
    /// attempt is still free, and the set is stale or does not hold the key.
    fn needs_fetch(&self, kid: &str, now: Instant) -> bool {
        let known = self.set.as_ref().is_some_and(|set| set.find(kid).is_some());
        let stale = since(self.fetched_at, now).is_none_or(|age| age >= KEY_SET_MAX_AGE);
        let may_refresh =
            since(self.attempted_at, now).is_none_or(|waited| waited >= REFRESH_INTERVAL);
        may_refresh && (stale || !known)
    }

    /// The verification key `kid` names in the cached set as it stands at `now`. `fetched` says
    /// the set was fetched for this very request: only then is a `kid` it lacks known to be
    /// false. Any other unknown `kid`, and any request with no usable set, cannot be judged yet.
    fn key(&self, kid: &str, now: Instant, fetched: bool) -> Result<DecodingKey, Unverified> {
        let kept = since(self.fetched_at, now).is_some_and(|age| age < KEY_SET_KEPT);
        let Some(set) = self.set.as_ref().filter(|_| kept) else {
            return Err(self.not_yet(now, unavailable()));
        };
        match set.find(kid) {
            Some(jwk) => DecodingKey::from_jwk(jwk).map_err(|_| refused().into()),
            None if fetched => Err(refused().into()),
            None => Err(self.not_yet(now, unknown_key())),
        }
    }

    /// The refusal `error` of a token that cannot be judged at `now`, with its wait:
    /// [`FETCH_RETRY_SECONDS`] while a fetch is under way, else the whole seconds, rounded up,
    /// until the issuer may be asked again.
    fn not_yet(&self, now: Instant, error: ApiError) -> Unverified {
        let waited = since(self.attempted_at, now).unwrap_or_default();
        let left = REFRESH_INTERVAL.saturating_sub(waited);
        let seconds = u32::try_from(left.as_nanos().div_ceil(1_000_000_000)).unwrap_or(u32::MAX);
        Unverified {
            error,
            retry_after: Some(if self.fetching {
                FETCH_RETRY_SECONDS
            } else {
                seconds
            }),
        }
    }
}

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
            cache: Arc::new(Mutex::new(KeyCache::default())),
        })
    }

    /// The verification key `kid` names, by the timing rules of the module header.
    async fn key(&self, kid: &str) -> Result<DecodingKey, Unverified> {
        let began = {
            let mut cache = self.cache.lock().await;
            // Read under the lock that decides who takes the attempt: no request decides on a
            // reading older than an attempt another request marked in the meantime.
            let now = self.clock.now();
            if !cache.needs_fetch(kid, now) {
                return cache.key(kid, now, false);
            }
            // Taken under the lock, before the issuer is asked: every other request of this
            // minute, also one that arrives while the fetch runs, answers from the cache.
            cache.attempted_at = Some(now);
            cache.fetching = true;
            now
        };
        // The lock is released. The fetch runs in a task of its own, so it and what it brings
        // outlive this request if its caller goes away.
        let fetching = tokio::spawn(refresh(self.keys.clone(), self.cache.clone(), began));
        let finished = fetching.await;
        let mut cache = self.cache.lock().await;
        // A task that did not finish is a failed fetch, tolerated like any other; it could not
        // say itself that the fetch is over.
        let fetched = finished.unwrap_or_else(|_| {
            cache.fetching = false;
            false
        });
        cache.key(kid, self.clock.now(), fetched)
    }

    async fn identity(&self, token: &str) -> Result<ConnectIdentity, Unverified> {
        let header = decode_header(token).map_err(|_| refused())?;
        // Before any key lookup: a token of another algorithm never causes a fetch.
        if header.alg != Algorithm::RS256 {
            return Err(refused().into());
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
            return Err(refused().into());
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
    fn verify<'a>(&'a self, token: &'a str) -> Verifying<'a> {
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

/// Ask the issuer once, within [`FETCH_TIME_LIMIT`], and keep what it answers, stamped with
/// `began`, the instant the attempt was taken. Whether a set was fetched and kept.
async fn refresh(keys: Arc<dyn KeySource>, cache: Arc<Mutex<KeyCache>>, began: Instant) -> bool {
    // The one error tolerated here: a fetch that fails, or that exceeds the time limit, leaves
    // the cached set in use, for as long as `KEY_SET_KEPT` allows.
    let answer = timeout(FETCH_TIME_LIMIT, keys.fetch()).await;
    let mut cache = cache.lock().await;
    cache.fetching = false;
    let Ok(Ok(set)) = answer else {
        return false;
    };
    // An answer never replaces a set whose fetch began later.
    if cache.fetched_at.is_none_or(|held| held <= began) {
        cache.set = Some(set);
        cache.fetched_at = Some(began);
    }
    true
}

/// How long before `now` the instant `at` was, when there is one. An instant later than
/// `now` counts as no time at all: it was stored a moment ago, never long ago, so it can make
/// neither an attempt look free nor a set look old.
fn since(at: Option<Instant>, now: Instant) -> Option<Duration> {
    at.map(|at| now.saturating_duration_since(at))
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::Unauthenticated,
        "The bearer token does not authenticate",
    )
}

/// There is no usable key set: none was ever fetched, or the last is older than the hour.
fn unavailable() -> ApiError {
    ApiError::new(
        ErrorCode::Unavailable,
        "The token issuer's keys could not be fetched",
    )
}

/// There is a key set, the token's key is not in it, and the issuer may not be asked yet.
fn unknown_key() -> ApiError {
    ApiError::new(
        ErrorCode::Unavailable,
        "The token names a key this server does not have yet",
    )
}
