//! WorkOS Connect access-token verification: JWKS signature, issuer, audience and expiry.
//!
//! SPEC section 11: "validate JWKS signatures/issuer/audience/expiry ... Connect access tokens
//! identify the caller (subject, org, client) and do not carry workspace permissions". A token is
//! verified with `jsonwebtoken` against the key its `kid` names in the issuer's key set
//! (`{issuer}/oauth2/jwks`, fetched through the `workos` crate's transport); an unknown `kid`
//! refreshes the set once, for a rotated key. Only RS256 is accepted, so a token cannot choose a
//! weaker algorithm. The issuer must be the configured AuthKit issuer and the audience the
//! configured MCP resource; `exp`, `iss`, `aud` and `sub` are required, and `org_id` and
//! `client_id` must be present. Every refusal is the same `Unauthenticated`, and no token or claim
//! is echoed.
//!
//! Only public-key signature verification happens here: no private-key operation is ever
//! performed in production, so the `rsa` crate's private-key decryption timing advisory
//! (RUSTSEC-2023-0071) does not reach this code. `install_crypto` selects jsonwebtoken's
//! `rust_crypto` provider explicitly before any verification, so a crate that later enables
//! another provider cannot leave verification without one.

use std::sync::{Arc, OnceLock};

use axum::http::{HeaderMap, Method};
use jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::ports::PortFuture;
use serde::Deserialize;
use workos::transport::{HttpRequest, SharedTransport};

use crate::config::HostedConfig;
use crate::identity::{ConnectIdentity, ConnectVerifier};

/// Where verification keys come from.
pub trait KeySource: Send + Sync {
    /// The issuer's key set; `refresh` asks for a fresh copy instead of a cached one.
    fn keys(&self, refresh: bool) -> PortFuture<'_, JwkSet>;
}

/// The issuer's published key set, fetched with the `workos` crate's HTTP transport.
pub struct IssuerKeys {
    transport: SharedTransport,
    url: String,
    cached: tokio::sync::RwLock<Option<JwkSet>>,
}

/// Connect token verification against a key source.
pub struct ConnectTokens {
    keys: Arc<dyn KeySource>,
    issuer: String,
    audience: String,
}

/// The claims a Connect access token must carry for this service.
#[derive(Debug, Deserialize)]
struct ConnectClaims {
    sub: String,
    org_id: String,
    client_id: String,
}

/// Seconds of clock difference tolerated on `exp`.
const LEEWAY_SECONDS: u64 = 30;

impl IssuerKeys {
    /// The key set at `{issuer}/oauth2/jwks`, fetched through `transport`.
    #[must_use]
    pub fn new(transport: SharedTransport, issuer: &str) -> Self {
        Self {
            transport,
            url: format!("{}/oauth2/jwks", issuer.trim_end_matches('/')),
            cached: tokio::sync::RwLock::new(None),
        }
    }
}

impl KeySource for IssuerKeys {
    fn keys(&self, refresh: bool) -> PortFuture<'_, JwkSet> {
        Box::pin(async move {
            if !refresh && let Some(set) = self.cached.read().await.as_ref() {
                return Ok(set.clone());
            }
            let unavailable = || {
                ApiError::new(
                    ErrorCode::Unavailable,
                    "The token issuer's keys are unavailable",
                )
            };
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
            let set: JwkSet = serde_json::from_slice(&response.body).map_err(|_| unavailable())?;
            *self.cached.write().await = Some(set.clone());
            Ok(set)
        })
    }
}

impl ConnectTokens {
    /// A verifier for tokens `config.issuer` issued for `config.resource`, keyed by `keys`.
    ///
    /// # Errors
    /// Returns `install_crypto`'s error.
    pub fn new(keys: Arc<dyn KeySource>, config: &HostedConfig) -> Result<Self, ApiError> {
        install_crypto()?;
        Ok(Self {
            keys,
            issuer: config.issuer.clone(),
            audience: config.resource.clone(),
        })
    }

    async fn identity(&self, token: &str) -> Result<ConnectIdentity, ApiError> {
        let header = decode_header(token).map_err(|_| refused())?;
        if header.alg != Algorithm::RS256 {
            return Err(refused());
        }
        let kid = header.kid.ok_or_else(refused)?;
        let known = self.keys.keys(false).await?;
        let key = if let Some(jwk) = known.find(&kid) {
            DecodingKey::from_jwk(jwk).map_err(|_| refused())?
        } else {
            // An unknown key id: the issuer may have rotated its keys since the set was cached.
            let fresh = self.keys.keys(true).await?;
            let jwk = fresh.find(&kid).ok_or_else(refused)?;
            DecodingKey::from_jwk(jwk).map_err(|_| refused())?
        };
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.audience.as_str()]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.validate_exp = true;
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
