//! The WorkOS AuthKit sign-in adapter of hosted mode, through the `workos` crate.
//!
//! The authorize URL names the AuthKit provider (`provider=authkit`): WorkOS's reference for
//! `GET /user_management/authorize` says "If you are using AuthKit, set the provider parameter to
//! "authkit""; without it the request is the SSO path, which needs a connection selector. The
//! crate adds only the parameters it is given (`helpers/authkit.rs`).
//!
//! The login runs over the deployment's one WorkOS client (`outbound::workos_client`), whose
//! transport follows no redirect and has its own time limits. Errors from WorkOS are reported in
//! fixed words: a WorkOS error can quote the request, and the request carries an authorization
//! code and the API key's client secret, which never leave the server. A sign-in without an
//! organization is refused: the organization is the tenant.

use std::sync::Arc;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::ports::PortFuture;
use workos::{AuthKitAuthorizationUrlParams, AuthKitPkceCodeExchangeParams, Client};

use crate::config::HostedConfig;
use crate::hosted::{HostedLogin, HostedUser, LoginStart};

/// AuthKit sign-in with PKCE.
pub struct WorkosLogin {
    client: Arc<Client>,
    redirect_uri: String,
}

/// The provider an AuthKit authorize URL names.
pub const AUTHKIT_PROVIDER: &str = "authkit";

impl WorkosLogin {
    /// A login over the deployment's one WorkOS `client` (`outbound::workos_client`), returning
    /// the browser to `config`'s redirect URI.
    #[must_use]
    pub fn new(client: Arc<Client>, config: &HostedConfig) -> Self {
        Self {
            client,
            redirect_uri: config.redirect_uri.clone(),
        }
    }
}

impl HostedLogin for WorkosLogin {
    fn begin(&self) -> Result<LoginStart, ApiError> {
        let started = self
            .client
            .authkit()
            .pkce_authorization_url(AuthKitAuthorizationUrlParams {
                redirect_uri: self.redirect_uri.clone(),
                provider: Some(AUTHKIT_PROVIDER.to_owned()),
                ..AuthKitAuthorizationUrlParams::default()
            })
            .map_err(|_| {
                ApiError::new(ErrorCode::Unavailable, "The sign-in could not be started")
            })?;
        Ok(LoginStart {
            url: started.url,
            verifier: started.code_verifier,
            state: started.state,
        })
    }

    fn complete<'a>(&'a self, code: &'a str, verifier: &'a str) -> PortFuture<'a, HostedUser> {
        Box::pin(async move {
            let response = self
                .client
                .authkit()
                .pkce_code_exchange(AuthKitPkceCodeExchangeParams {
                    code: code.to_owned(),
                    code_verifier: verifier.to_owned(),
                })
                .await
                .map_err(|_| {
                    ApiError::new(ErrorCode::Unauthenticated, "The sign-in was not accepted")
                })?;
            let organization_id = response.organization_id.ok_or_else(|| {
                ApiError::new(
                    ErrorCode::Forbidden,
                    "Sign in through an organization; it is the tenant of every workspace",
                )
            })?;
            Ok(HostedUser {
                subject: response.user.id,
                organization_id,
            })
        })
    }
}
