//! The one outbound HTTP client: every call this service makes to another service (the WorkOS
//! API, AuthKit's code exchange, the Connect key set) goes through it.
//!
//! It follows no redirect, speaks only `https`, and has its own time limits: a whole exchange
//! [`REQUEST_TIME_LIMIT`], a connection [`CONNECT_TIME_LIMIT`]. A redirect answer is returned to
//! the caller as it is, so a key fetch or an API call that is redirected fails instead of
//! carrying its request (an API key, a code) to another address (request R7 of server M1). The
//! client is built fallibly, so a TLS backend that cannot start refuses startup instead of
//! panicking (the `workos` crate's own `ReqwestTransport::with_timeout` panics,
//! `transport/reqwest_impl.rs:31-37`).
//!
//! The WorkOS client built over it retries a safe request at most [`WORKOS_RETRIES`] time: the
//! crate's default of three, with backoff and any `Retry-After` the API sends, could hold a request
//! for minutes (`client.rs:422-470`). Its callers bound each call in time on top of that.

use std::sync::Arc;
use std::time::Duration;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use reqwest::redirect::Policy;
use workos::Client;
use workos::transport::{ReqwestTransport, SharedTransport};

use crate::config::HostedConfig;

/// The longest one outbound exchange may take, connection included.
pub const REQUEST_TIME_LIMIT: Duration = Duration::from_secs(10);

/// The longest a connection may take to open.
pub const CONNECT_TIME_LIMIT: Duration = Duration::from_secs(5);

/// How many times the WorkOS client repeats a safe request that failed for a reason a retry can
/// change (a 429, a 5xx, a refused or timed-out connection).
pub const WORKOS_RETRIES: u32 = 1;

/// The client builder every outbound client starts from: no redirect, and the two time limits
/// given. The production client adds `https_only` ([`client`]).
pub fn builder_within(total: Duration, connect: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(total)
        .connect_timeout(connect)
}

/// The production outbound client: [`builder_within`] the production limits, `https` only.
///
/// # Errors
/// Returns `Internal` when the HTTP client cannot be built (its TLS backend did not start).
pub fn client() -> Result<reqwest::Client, ApiError> {
    builder_within(REQUEST_TIME_LIMIT, CONNECT_TIME_LIMIT)
        .https_only(true)
        .build()
        .map_err(|_| {
            ApiError::new(
                ErrorCode::Internal,
                "The outbound HTTP client could not be built",
            )
        })
}

/// `client` as the `workos` crate's transport.
#[must_use]
pub fn transport(client: reqwest::Client) -> SharedTransport {
    Arc::new(ReqwestTransport::from_client(client))
}

/// The one WorkOS client of a hosted deployment, over `transport`, retrying at most
/// [`WORKOS_RETRIES`] time.
///
/// # Errors
/// Returns `InvalidInput` naming `WORKOS_API_KEY` when the key cannot be sent as an HTTP header.
pub fn workos_client(
    config: &HostedConfig,
    transport: SharedTransport,
) -> Result<Arc<Client>, ApiError> {
    Client::builder()
        .api_key(config.api_key.clone())
        .client_id(config.client_id.clone())
        .transport(transport)
        .max_retries(WORKOS_RETRIES)
        .try_build()
        .map(Arc::new)
        .map_err(|_| {
            ApiError::new(
                ErrorCode::InvalidInput,
                "WORKOS_API_KEY cannot be sent as an HTTP header",
            )
            .with_field("WORKOS_API_KEY")
        })
}
