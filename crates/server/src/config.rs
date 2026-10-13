//! Deployment settings the service is bound with, read once at startup and checked there.
//!
//! SPEC section 11: two explicit authentication entry paths selected by deployment
//! configuration, and "a missing or invalid hosted configuration never falls back to local
//! authentication". `OKF_AUTH_MODE` selects the path: `local`, or `workos` for hosted, spelled
//! exactly (a value with surrounding whitespace is refused, not trimmed into a mode). An unset
//! mode is local only when no `WORKOS_*` setting is present: hosted settings are never ignored.
//! Any other value is refused, and `workos` with a missing or invalid setting is refused, so a
//! hosted deployment either starts hosted or does not start. A hosted URL with a query, a
//! fragment or user information is refused, naming its setting.
//!
//! Remote endpoints require HTTPS (SPEC section 2): every hosted URL is `https`. Locally the
//! public origin may be `http` on loopback; that origin is only where the service answers, never
//! a grant of trust. Secrets never appear in `Debug` output.

use std::fmt::{Debug, Formatter};
use std::str::FromStr as _;

use axum::http::Uri;
use okf_jawn_contract::error::{ApiError, ErrorCode};

/// Which entry path a deployment authenticates through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    /// One persistent local identity, a launch-token browser session and connector secrets.
    Local,
    /// WorkOS AuthKit browser sessions and WorkOS Connect delegation.
    Hosted,
}

/// The authentication settings of one deployment.
#[derive(Debug, Clone)]
pub enum AuthConfig {
    /// Local installation authentication.
    Local,
    /// Hosted WorkOS authentication, checked complete.
    Hosted(HostedConfig),
}

/// The WorkOS settings a hosted deployment needs; every field is required.
#[derive(Clone)]
pub struct HostedConfig {
    /// WorkOS client id (`WORKOS_CLIENT_ID`).
    pub client_id: String,
    /// WorkOS API key (`WORKOS_API_KEY`); never printed.
    pub api_key: String,
    /// The issuer Connect access tokens must name, an `https` URL (`WORKOS_ISSUER`).
    pub issuer: String,
    /// The externally reachable MCP endpoint, the audience Connect tokens must carry
    /// (`WORKOS_MCP_RESOURCE`).
    pub resource: String,
    /// Where AuthKit returns the browser, `https` and ending `/auth/callback`
    /// (`WORKOS_REDIRECT_URI`).
    pub redirect_uri: String,
}

/// The WorkOS settings; any of them present without a mode is refused.
const HOSTED_SETTINGS: [&str; 5] = [
    "WORKOS_CLIENT_ID",
    "WORKOS_API_KEY",
    "WORKOS_ISSUER",
    "WORKOS_MCP_RESOURCE",
    "WORKOS_REDIRECT_URI",
];

impl Debug for HostedConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedConfig")
            .field("client_id", &self.client_id)
            .field("api_key", &"[redacted]")
            .field("issuer", &self.issuer)
            .field("resource", &self.resource)
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

impl AuthConfig {
    /// The entry path these settings select.
    #[must_use]
    pub const fn kind(&self) -> AuthKind {
        match self {
            Self::Local => AuthKind::Local,
            Self::Hosted(_) => AuthKind::Hosted,
        }
    }

    /// Read the authentication settings through `lookup` (the process environment at startup).
    ///
    /// # Errors
    /// Returns `InvalidInput` naming the setting for an unknown `OKF_AUTH_MODE`, for an unset
    /// one beside any `WORKOS_*` setting, and for `workos` with any WorkOS setting missing or
    /// invalid. Never answers `Local` for a hosted mode or beside a hosted setting.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ApiError> {
        // The mode is read exactly as set: a value with surrounding whitespace is no mode.
        let mode = lookup("OKF_AUTH_MODE");
        match mode.as_deref() {
            None => {
                // Hosted settings are never ignored: with any of them present and no mode, the
                // deployment was meant to be hosted, and starting it local would hand the
                // installation to whoever reads the launch link.
                let hosted_setting = HOSTED_SETTINGS
                    .iter()
                    .any(|name| lookup(name).is_some_and(|value| !value.trim().is_empty()));
                if hosted_setting {
                    return Err(invalid(
                        "OKF_AUTH_MODE",
                        "must be set to `workos` when a WORKOS_* setting is present",
                    ));
                }
                Ok(Self::Local)
            }
            Some("local") => Ok(Self::Local),
            Some("workos") => {
                let required = |name: &str| {
                    lookup(name)
                        .map(|value| value.trim().to_owned())
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| invalid(name, "is required in workos mode"))
                };
                let hosted = HostedConfig {
                    client_id: required("WORKOS_CLIENT_ID")?,
                    api_key: required("WORKOS_API_KEY")?,
                    issuer: required("WORKOS_ISSUER")?,
                    resource: required("WORKOS_MCP_RESOURCE")?,
                    redirect_uri: required("WORKOS_REDIRECT_URI")?,
                };
                hosted.check()?;
                Ok(Self::Hosted(hosted))
            }
            Some(_) => Err(invalid("OKF_AUTH_MODE", "must be `local` or `workos`")),
        }
    }
}

impl HostedConfig {
    /// Refuse settings no hosted deployment may run with.
    ///
    /// # Errors
    /// Returns `InvalidInput` naming the setting: an empty client id or API key, an issuer or
    /// MCP resource that is not an absolute `https` URL, or a redirect URI that is not an
    /// absolute `https` URL whose path is `/auth/callback`.
    pub fn check(&self) -> Result<(), ApiError> {
        if self.client_id.trim().is_empty() {
            return Err(invalid("WORKOS_CLIENT_ID", "is required in workos mode"));
        }
        if self.api_key.trim().is_empty() {
            return Err(invalid("WORKOS_API_KEY", "is required in workos mode"));
        }
        if https_url(&self.issuer).is_none() {
            return Err(invalid(
                "WORKOS_ISSUER",
                "must be an absolute https URL with no query, fragment or user information",
            ));
        }
        if https_url(&self.resource).is_none() {
            return Err(invalid(
                "WORKOS_MCP_RESOURCE",
                "must be an absolute https URL with no query, fragment or user information",
            ));
        }
        if https_url(&self.redirect_uri).is_none_or(|uri| uri.path() != "/auth/callback") {
            return Err(invalid(
                "WORKOS_REDIRECT_URI",
                "must be an absolute https URL ending /auth/callback, with no query, fragment or user information",
            ));
        }
        Ok(())
    }
}

/// `text` as an absolute `https` URL with a host and no query, fragment or user information.
fn https_url(text: &str) -> Option<Uri> {
    // The parser drops a fragment and keeps a query; neither belongs in a configured URL.
    if text.contains(['?', '#']) {
        return None;
    }
    let uri = Uri::from_str(text).ok()?;
    let authority = uri.authority()?;
    if uri.scheme_str() != Some("https")
        || authority.host().is_empty()
        || authority.as_str().contains('@')
    {
        return None;
    }
    Some(uri)
}

fn invalid(setting: &str, rule: &str) -> ApiError {
    ApiError::new(ErrorCode::InvalidInput, format!("{setting} {rule}")).with_field(setting)
}
