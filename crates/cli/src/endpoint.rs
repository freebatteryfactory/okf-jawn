//! The service address, and the rules it passes before any request is built.
//!
//! A remote address requires HTTPS; plain HTTP is accepted for a loopback host only. An address
//! that carries credentials is refused, and so is one with a path, a query or a fragment: the
//! address is an origin, and each request sets its own path. No refusal repeats the address,
//! because a refused address may hold a password.
//!
//! Loopback means this computer. A loopback IP literal is one by its value. The name
//! `localhost` is trusted only as a name: over plain HTTP it is replaced by `127.0.0.1` when
//! the address is read, so every request goes to the loopback address and no resolver is
//! asked. The pin is that rewriting of the host and nothing subtler, with two consequences.
//! The service hears `Host: 127.0.0.1:<port>`, not `localhost`, and must accept that host.
//! And only the IPv4 loopback address is reached: a service bound to `[::1]` alone is not
//! reached through `http://localhost`, and has to be named `http://[::1]:<port>`. Over HTTPS
//! the name is kept and resolved as any other, and the relay never starts a server for it
//! ([`Endpoint::may_start`]).
//!
//! [`Endpoint::client`] is the one place an HTTP client is built, so both the operation path
//! and the relay never follow a redirect and never send a loopback request through a proxy.

use std::net::IpAddr;
use std::time::Duration;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use reqwest::Url;

/// A checked service origin.
#[derive(Debug, Clone)]
pub struct Endpoint {
    origin: Url,
    loopback: bool,
}

/// How long a connection attempt may take before it counts as failed.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The address a plain-HTTP request to `localhost` is sent to.
const LOOPBACK_ADDRESS: &str = "127.0.0.1";

impl Endpoint {
    /// Check a service address.
    ///
    /// # Errors
    /// Returns `InvalidInput` when the text is not a URL, carries credentials, is plain HTTP to
    /// a host that is not loopback, uses another scheme, or has a path, query or fragment.
    pub fn parse(text: &str) -> Result<Self, ApiError> {
        let origin = Url::parse(text)
            .map_err(|error| refuse(format!("The server address is not a URL: {error}")))?;
        if !origin.username().is_empty() || origin.password().is_some() {
            return Err(refuse(
                "The server address carries credentials; a URL with a user name or a password \
                 is refused",
            ));
        }
        let loopback = origin.host_str().is_some_and(is_loopback_host);
        match origin.scheme() {
            "https" => {}
            "http" if loopback => {}
            "http" => {
                return Err(refuse(
                    "A remote server address requires HTTPS; plain HTTP is accepted for a \
                     loopback address only",
                ));
            }
            _ => {
                return Err(refuse(
                    "The server address must use HTTPS, or HTTP for a loopback address",
                ));
            }
        }
        if !matches!(origin.path(), "" | "/")
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err(refuse(
                "The server address is an origin: a scheme, a host and a port, with no path, \
                 query or fragment",
            ));
        }
        let mut origin = origin;
        if origin.scheme() == "http" && origin.host_str() == Some("localhost") {
            // Plain HTTP carries the credential in clear, so the name is not left to a
            // resolver, which a hosts file or a DNS answer can point at another computer.
            origin.set_host(Some(LOOPBACK_ADDRESS)).map_err(|error| {
                refuse(format!(
                    "The server address cannot be read as the loopback address: {error}"
                ))
            })?;
        }
        Ok(Self { origin, loopback })
    }

    /// The address a locally started server binds so that this endpoint reaches it: an IP
    /// address and a port. `None` for an address the relay never starts a server for.
    #[must_use]
    pub fn bind_address(&self) -> Option<String> {
        if !self.may_start() {
            return None;
        }
        let host = self.origin.host_str()?;
        let port = self.origin.port_or_known_default()?;
        Some(format!("{host}:{port}"))
    }

    /// Build the HTTP client for this endpoint.
    ///
    /// The client never follows a redirect, so a credential is never carried to another
    /// address. For a loopback endpoint it also ignores every configured proxy. `total` bounds
    /// a whole exchange; the relay passes `None`, because an event stream has no fixed length.
    ///
    /// # Errors
    /// Returns `Internal` when the HTTP client cannot be built.
    pub fn client(&self, total: Option<Duration>) -> Result<reqwest::Client, ApiError> {
        self.client_within(total, CONNECT_TIMEOUT)
    }

    /// [`Endpoint::client`], with the time one connection attempt may take.
    fn client_within(
        &self,
        total: Option<Duration>,
        connect: Duration,
    ) -> Result<reqwest::Client, ApiError> {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(connect);
        if self.loopback {
            builder = builder.no_proxy();
        }
        if let Some(total) = total {
            builder = builder.timeout(total);
        }
        builder.build().map_err(|error| {
            ApiError::new(
                ErrorCode::Internal,
                format!("The HTTP client could not be built: {error}"),
            )
        })
    }

    /// Whether the host is a loopback address or `localhost`.
    #[must_use]
    pub fn is_loopback(&self) -> bool {
        self.loopback
    }

    /// Whether the relay may start a local server for this address: plain HTTP to a loopback
    /// address, and nothing else. An HTTPS address is only ever connected to, `localhost`
    /// included: the server the relay starts speaks plain HTTP.
    #[must_use]
    pub fn may_start(&self) -> bool {
        self.loopback && self.origin.scheme() == "http"
    }

    /// The URL of `path` on this origin.
    #[must_use]
    pub fn url(&self, path: &str) -> Url {
        let mut url = self.origin.clone();
        url.set_path(path);
        url
    }
}

/// Whether a URL host, as `Url::host_str` spells it, is `localhost` or a loopback IP address.
fn is_loopback_host(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    host.strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host)
        .parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

fn refuse(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::InvalidInput, message).with_field("server")
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{CONNECT_TIMEOUT, Endpoint};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn err_of<T: std::fmt::Debug, E>(result: Result<T, E>) -> Result<E, String> {
        match result {
            Ok(value) => Err(format!("expected an error, got Ok({value:?})")),
            Err(error) => Ok(error),
        }
    }

    /// A connection attempt has a limit in time (the review's R12). The address is in
    /// TEST-NET-1 (RFC 5737), where nothing answers: an attempt gets neither an acceptance
    /// nor a refusal, and without a limit it waits as long as the operating system does
    /// (about 21 seconds on Windows, longer on Linux). The limit of the shipped CLI is 10
    /// seconds; the test passes a short one through the same builder.
    ///
    /// Known weakness: on a computer with no route to that address the attempt fails at once
    /// and this test passes whatever the limit is.
    #[tokio::test]
    async fn a_connection_attempt_ends_at_its_time_limit() -> TestResult {
        assert_eq!(CONNECT_TIMEOUT, Duration::from_secs(10));
        let endpoint = Endpoint::parse("https://192.0.2.1:9")?;
        let client = endpoint.client_within(None, Duration::from_millis(300))?;
        let started = Instant::now();
        let sent = tokio::time::timeout(
            Duration::from_secs(12),
            client.get(endpoint.url("/healthz")).send(),
        )
        .await;
        let failed = err_of(sent.map_err(|_| "the attempt was still waiting after 12 s")?)?;
        assert!(failed.is_connect() || failed.is_timeout(), "{failed}");
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "the attempt took {:?}",
            started.elapsed()
        );
        Ok(())
    }
}
