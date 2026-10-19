//! The hosted sign-in ingress limit: at most [`SIGN_IN_PER_MINUTE`] requests a minute from one
//! source to `/auth/login` and `/auth/callback`.
//!
//! A callback whose cookie a client made itself makes this server ask the identity provider for
//! a code exchange (server M1 re-review, N1), and `/auth/login` builds a sign-in each time; so a
//! flood from one source could spend the provider's request budget, which every sign-in shares.
//! A source past its allowance is answered `Unavailable` with `Retry-After`, the seconds left in
//! its window, and the provider is not asked. Other routes are not limited here; logout needs a
//! session and its anti-CSRF token.
//!
//! **The source** is the address of the connection's peer, unless that peer is one of the
//! trusted proxies (`OKF_TRUSTED_PROXIES`); then it is the right-most address of the
//! `X-Forwarded-For` values, in order, that is not a trusted proxy. An entry that is not an IP
//! address ends the walk, and the source is then the last proxy seen. A forwarded address from a
//! peer that is not a trusted proxy is never read, so a client cannot choose its own source. With
//! no trusted proxy configured, every client behind a proxy shares the proxy's allowance: the
//! limit then holds for all of them together, which fails safe. A request whose peer is unknown
//! (served without connection information) is counted in one shared source.
//!
//! A window opens at a source's first request and is over at exactly sixty seconds. At most
//! [`MAX_SOURCES`] sources are tracked; past that, new sources share one overflow allowance
//! until windows end. The contract has no rate-limit code yet (carried to the next contract
//! batch), so the refusal is `Unavailable`.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::HeaderMap;
use axum::middleware::{self, Next};
use axum::response::Response;
use okf_jawn_contract::error::{ApiError, ErrorCode};

use crate::errors::{error_response, retry_response};
use crate::grants::Clock;

/// The per-source limit of the hosted sign-in routes.
pub struct SignInLimit {
    trusted: Vec<IpAddr>,
    clock: Arc<dyn Clock>,
    per_window: u32,
    max_sources: usize,
    windows: Mutex<BTreeMap<Source, (Instant, u32)>>,
}

/// Who a request is counted for.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    /// A client address.
    Address(IpAddr),
    /// A request whose peer is not known.
    Unknown,
    /// Every new source past the bound.
    Overflow,
}

/// Requests one source may make to the sign-in routes in a window.
pub const SIGN_IN_PER_MINUTE: u32 = 30;

/// The most sources tracked at once.
pub const MAX_SOURCES: usize = 10_000;

/// How long one source's window lasts.
const WINDOW: Duration = Duration::from_secs(60);

/// The routes the limit applies to.
const LIMITED: [&str; 2] = ["/auth/login", "/auth/callback"];

impl SignInLimit {
    /// The production limit: [`SIGN_IN_PER_MINUTE`] a source, [`MAX_SOURCES`] sources,
    /// `X-Forwarded-For` believed only from `trusted`.
    #[must_use]
    pub fn new(trusted: Vec<IpAddr>, clock: Arc<dyn Clock>) -> Self {
        Self::with_bounds(trusted, clock, SIGN_IN_PER_MINUTE, MAX_SOURCES)
    }

    /// [`Self::new`] with `per_window` requests a source and at most `max_sources` sources.
    #[must_use]
    pub fn with_bounds(
        trusted: Vec<IpAddr>,
        clock: Arc<dyn Clock>,
        per_window: u32,
        max_sources: usize,
    ) -> Self {
        Self {
            trusted,
            clock,
            per_window,
            max_sources,
            windows: Mutex::new(BTreeMap::new()),
        }
    }

    /// The source of a request from `peer` carrying `headers`.
    fn source(&self, peer: Option<IpAddr>, headers: &HeaderMap) -> Source {
        let Some(mut source) = peer else {
            return Source::Unknown;
        };
        if !self.trusted.contains(&source) {
            return Source::Address(source);
        }
        let forwarded: Vec<&str> = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .collect();
        for entry in forwarded.iter().rev() {
            let Ok(address) = IpAddr::from_str(entry) else {
                break;
            };
            source = address;
            if !self.trusted.contains(&address) {
                break;
            }
        }
        Source::Address(source)
    }

    /// Admit one request of `source`: `None`, or the whole seconds until it may try again.
    fn admit(&self, source: Source) -> Result<Option<u32>, ApiError> {
        let now = self.clock.now();
        let mut windows = self.windows.lock().map_err(|_| {
            ApiError::new(ErrorCode::Internal, "the sign-in limit lock is poisoned")
        })?;
        if let Some((began, _)) = windows.get(&source)
            && now.saturating_duration_since(*began) >= WINDOW
        {
            windows.remove(&source);
        }
        let source = if windows.contains_key(&source) || windows.len() < self.max_sources {
            source
        } else {
            windows.retain(|_, (began, _)| now.saturating_duration_since(*began) < WINDOW);
            if windows.len() < self.max_sources {
                source
            } else {
                Source::Overflow
            }
        };
        let (began, used) = windows.entry(source).or_insert((now, 0));
        if *used < self.per_window {
            *used = used.saturating_add(1);
            return Ok(None);
        }
        let left = WINDOW.saturating_sub(now.saturating_duration_since(*began));
        Ok(Some(
            u32::try_from(left.as_nanos().div_ceil(1_000_000_000)).unwrap_or(u32::MAX),
        ))
    }
}

/// `routes` with the sign-in limit on `/auth/login` and `/auth/callback`.
pub fn guard(limit: Arc<SignInLimit>, routes: Router) -> Router {
    routes.layer(middleware::from_fn_with_state(limit, limited))
}

/// Count a request to a limited route, and refuse it past its source's allowance.
async fn limited(State(limit): State<Arc<SignInLimit>>, request: Request, next: Next) -> Response {
    if !LIMITED.contains(&request.uri().path()) {
        return next.run(request).await;
    }
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| address.ip());
    let source = limit.source(peer, request.headers());
    match limit.admit(source) {
        Ok(None) => next.run(request).await,
        Ok(Some(seconds)) => retry_response(
            ApiError::new(
                ErrorCode::Unavailable,
                "Too many sign-in requests from this address; try again later",
            ),
            seconds,
        ),
        Err(error) => error_response(error),
    }
}
