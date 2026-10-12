//! The WorkOS AuthKit sign-in adapter of hosted mode, through the `workos` crate.
//!
//! Errors from WorkOS are reported in fixed words: a WorkOS error can quote the request, and the
//! request carries an authorization code and the API key's client secret, which never leave the
//! server. A sign-in without an organization is refused: the organization is the tenant.

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

impl WorkosLogin {
    /// A login over a WorkOS client built from `config`.
    ///
    /// # Errors
    /// Returns `InvalidInput` naming `WORKOS_API_KEY` when the key cannot be sent as a header.
    pub fn new(config: &HostedConfig) -> Result<Self, ApiError> {
        let client = Client::builder()
            .api_key(config.api_key.clone())
            .client_id(config.client_id.clone())
            .try_build()
            .map_err(|_| {
                ApiError::new(
                    ErrorCode::InvalidInput,
                    "WORKOS_API_KEY cannot be sent as an HTTP header",
                )
                .with_field("WORKOS_API_KEY")
            })?;
        Ok(Self {
            client: Arc::new(client),
            redirect_uri: config.redirect_uri.clone(),
        })
    }
}

impl HostedLogin for WorkosLogin {
    fn begin(&self) -> Result<LoginStart, ApiError> {
        let started = self
            .client
            .authkit()
            .pkce_authorization_url(AuthKitAuthorizationUrlParams {
                redirect_uri: self.redirect_uri.clone(),
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
