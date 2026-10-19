//! The one outbound client: https only, no redirect followed, its own time limit. No test
//! reaches a network: the plain-http listeners are this test's own, on 127.0.0.1.
#![cfg(feature = "runtime")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::http::header::LOCATION;
use axum::http::{HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_server::connect::{IssuerKeys, KeySource};
use okf_jawn_server::outbound::{builder_within, client, transport};
use tokio::net::TcpListener;

use check::{TestResult, err_of};

/// A listener on 127.0.0.1 that counts the connections it accepts and answers none.
async fn counting_listener() -> std::io::Result<(String, Arc<AtomicUsize>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&accepted);
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            // Kept open and never answered.
            held.push(stream);
        }
    });
    Ok((format!("http://{address}"), accepted))
}

/// Serve `router` on 127.0.0.1 and answer its origin.
async fn serve(router: Router) -> std::io::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        let _served = axum::serve(listener, router).await;
    });
    Ok(format!("http://{address}"))
}

const KEY_SET: &str = r#"{"keys":[]}"#;

#[tokio::test]
async fn the_production_client_refuses_plain_http_before_connecting() -> TestResult {
    let (origin, accepted) = counting_listener().await?;
    let keys = IssuerKeys::new(transport(client()?), &origin);
    let error = err_of(keys.fetch().await)?;
    assert_eq!(error.code, ErrorCode::Unavailable);
    // Give a connection that was opened time to be counted.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn a_redirect_is_never_followed() -> TestResult {
    // The target serves a valid key set, so a client that followed the redirect would succeed.
    let target_hits = Arc::new(AtomicUsize::new(0));
    let hits = Arc::clone(&target_hits);
    let target = serve(Router::new().route(
        "/oauth2/jwks",
        get(move || {
            hits.fetch_add(1, Ordering::SeqCst);
            async { ([("content-type", "application/json")], KEY_SET).into_response() }
        }),
    ))
    .await?;
    let location = HeaderValue::from_str(&format!("{target}/oauth2/jwks"))?;
    let redirector = serve(Router::new().route(
        "/oauth2/jwks",
        get(move || {
            let location = location.clone();
            async move { (StatusCode::FOUND, [(LOCATION, location)]).into_response() }
        }),
    ))
    .await?;
    // The production builder without `https_only`, which these plain-http listeners need.
    let plain = builder_within(Duration::from_secs(5), Duration::from_secs(5)).build()?;
    // The same client reads the target directly: the listener and the key set are good.
    IssuerKeys::new(transport(plain.clone()), &target)
        .fetch()
        .await?;
    assert_eq!(target_hits.load(Ordering::SeqCst), 1);
    let error = err_of(IssuerKeys::new(transport(plain), &redirector).fetch().await)?;
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert_eq!(
        target_hits.load(Ordering::SeqCst),
        1,
        "the redirect was followed"
    );
    Ok(())
}

#[tokio::test]
async fn an_answer_that_never_comes_ends_at_the_client_s_own_limit() -> TestResult {
    let (origin, accepted) = counting_listener().await?;
    let plain = builder_within(Duration::from_millis(300), Duration::from_millis(300)).build()?;
    let began = Instant::now();
    let error = err_of(IssuerKeys::new(transport(plain), &origin).fetch().await)?;
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert!(began.elapsed() < Duration::from_secs(5));
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
