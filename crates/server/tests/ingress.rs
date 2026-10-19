//! The hosted sign-in ingress limit: at most 30 requests a minute from one source to
//! `/auth/login` and `/auth/callback`, so a flood cannot spend the identity provider's calls; the
//! source is the peer address, or behind a trusted proxy the right-most forwarded address that
//! is not a trusted proxy; a forwarded address from any other peer is ignored.
#![cfg(feature = "runtime")]

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use okf_jawn_server::grants::Clock;
use okf_jawn_server::ingress::{SIGN_IN_PER_MINUTE, SignInLimit, guard};
use tower::ServiceExt;

use check::TestResult;

/// A clock that moves only when a test advances it.
struct TestClock {
    start: Instant,
    elapsed_ms: AtomicU64,
}

impl TestClock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            start: Instant::now(),
            elapsed_ms: AtomicU64::new(0),
        })
    }

    fn advance(&self, by: Duration) -> Result<(), std::num::TryFromIntError> {
        self.elapsed_ms
            .fetch_add(u64::try_from(by.as_millis())?, Ordering::SeqCst);
        Ok(())
    }
}

impl Clock for TestClock {
    fn now(&self) -> Instant {
        self.start
            .checked_add(Duration::from_millis(
                self.elapsed_ms.load(Ordering::SeqCst),
            ))
            .unwrap_or(self.start)
    }
}

fn routes() -> Router {
    Router::new()
        .route("/auth/login", get(|| async { "login" }))
        .route("/auth/callback", get(|| async { "callback" }))
        .route("/auth/logout", post(|| async { "logout" }))
}

fn limited(
    trusted: &[&str],
    clock: Arc<TestClock>,
    per_window: u32,
    max_sources: usize,
) -> Result<Router, std::net::AddrParseError> {
    let trusted = trusted
        .iter()
        .map(|text| text.parse::<IpAddr>())
        .collect::<Result<Vec<_>, _>>()?;
    let limit = SignInLimit::with_bounds(trusted, clock, per_window, max_sources);
    Ok(guard(Arc::new(limit), routes()))
}

/// A request to `path` from the peer `peer`, with `forwarded` as `X-Forwarded-For` values.
fn from(
    method: &str,
    path: &str,
    peer: &str,
    forwarded: &[&str],
) -> Result<Request<Body>, Box<dyn std::error::Error>> {
    let mut builder = Request::builder().method(method).uri(path);
    for value in forwarded {
        builder = builder.header("x-forwarded-for", *value);
    }
    let mut request = builder.body(Body::empty())?;
    let peer: SocketAddr = format!("{peer}:40000").parse()?;
    request.extensions_mut().insert(ConnectInfo(peer));
    Ok(request)
}

async fn status(
    app: &Router,
    request: Request<Body>,
) -> Result<StatusCode, Box<dyn std::error::Error>> {
    Ok(app.clone().oneshot(request).await?.status())
}

#[tokio::test]
async fn a_source_gets_thirty_sign_in_requests_a_minute() -> TestResult {
    assert_eq!(SIGN_IN_PER_MINUTE, 30);
    let clock = TestClock::new();
    let app = limited(&[], clock.clone(), SIGN_IN_PER_MINUTE, 10_000)?;
    for n in 0..30 {
        let path = if n % 2 == 0 {
            "/auth/login"
        } else {
            "/auth/callback"
        };
        assert_eq!(
            status(&app, from("GET", path, "203.0.113.7", &[])?).await?,
            StatusCode::OK
        );
    }
    clock.advance(Duration::from_secs(20))?;
    let refused = app
        .clone()
        .oneshot(from("GET", "/auth/callback", "203.0.113.7", &[])?)
        .await?;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        refused
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok()),
        Some("40")
    );
    // Another source is counted apart, and logout is not limited.
    assert_eq!(
        status(&app, from("GET", "/auth/login", "198.51.100.1", &[])?).await?,
        StatusCode::OK
    );
    assert_eq!(
        status(&app, from("POST", "/auth/logout", "203.0.113.7", &[])?).await?,
        StatusCode::OK
    );
    // The next window admits it again: at exactly sixty seconds the first window is over.
    clock.advance(Duration::from_secs(40))?;
    assert_eq!(
        status(&app, from("GET", "/auth/login", "203.0.113.7", &[])?).await?,
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test]
async fn behind_a_trusted_proxy_each_forwarded_client_is_its_own_source() -> TestResult {
    let clock = TestClock::new();
    let app = limited(&["10.0.0.5", "10.0.0.6"], clock, 2, 10_000)?;
    for client in ["203.0.113.7", "203.0.113.8"] {
        for _ in 0..2 {
            assert_eq!(
                status(&app, from("GET", "/auth/login", "10.0.0.5", &[client])?).await?,
                StatusCode::OK
            );
        }
    }
    // The right-most address that is not a trusted proxy is the source, across headers too.
    assert_eq!(
        status(
            &app,
            from(
                "GET",
                "/auth/login",
                "10.0.0.5",
                &["198.51.100.9, 203.0.113.7", "10.0.0.6"]
            )?
        )
        .await?,
        StatusCode::SERVICE_UNAVAILABLE
    );
    Ok(())
}

#[tokio::test]
async fn a_forwarded_address_from_an_untrusted_peer_is_ignored() -> TestResult {
    let clock = TestClock::new();
    let app = limited(&["10.0.0.5"], clock, 2, 10_000)?;
    for forged in ["1.2.3.4", "5.6.7.8"] {
        assert_eq!(
            status(&app, from("GET", "/auth/login", "198.51.100.1", &[forged])?).await?,
            StatusCode::OK
        );
    }
    assert_eq!(
        status(
            &app,
            from("GET", "/auth/login", "198.51.100.1", &["9.9.9.9"])?
        )
        .await?,
        StatusCode::SERVICE_UNAVAILABLE
    );
    // Behind the proxy, an entry that is not an address ends the walk: the proxy is the source.
    for _ in 0..2 {
        assert_eq!(
            status(&app, from("GET", "/auth/login", "10.0.0.5", &["unknown"])?).await?,
            StatusCode::OK
        );
    }
    assert_eq!(
        status(
            &app,
            from("GET", "/auth/login", "10.0.0.5", &["not-an-ip"])?
        )
        .await?,
        StatusCode::SERVICE_UNAVAILABLE
    );
    Ok(())
}

#[tokio::test]
async fn past_its_source_bound_the_limit_shares_one_bucket_among_new_sources() -> TestResult {
    let clock = TestClock::new();
    let app = limited(&[], clock, 2, 2)?;
    assert_eq!(
        status(&app, from("GET", "/auth/login", "192.0.2.1", &[])?).await?,
        StatusCode::OK
    );
    assert_eq!(
        status(&app, from("GET", "/auth/login", "192.0.2.2", &[])?).await?,
        StatusCode::OK
    );
    // Two sources are tracked; the next two new ones share the overflow bucket of two.
    assert_eq!(
        status(&app, from("GET", "/auth/login", "192.0.2.3", &[])?).await?,
        StatusCode::OK
    );
    assert_eq!(
        status(&app, from("GET", "/auth/login", "192.0.2.4", &[])?).await?,
        StatusCode::OK
    );
    assert_eq!(
        status(&app, from("GET", "/auth/login", "192.0.2.5", &[])?).await?,
        StatusCode::SERVICE_UNAVAILABLE
    );
    // A tracked source keeps its own count.
    assert_eq!(
        status(&app, from("GET", "/auth/login", "192.0.2.1", &[])?).await?,
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test]
async fn a_request_with_no_known_peer_is_counted_in_one_bucket() -> TestResult {
    let clock = TestClock::new();
    let app = limited(&[], clock, 1, 10_000)?;
    let plain = || Request::get("/auth/login").body(Body::empty());
    assert_eq!(
        app.clone().oneshot(plain()?).await?.status(),
        StatusCode::OK
    );
    assert_eq!(
        app.clone().oneshot(plain()?).await?.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
