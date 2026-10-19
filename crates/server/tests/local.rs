//! Local launch tokens: a token works once, a replay is refused, a token never issued is
//! refused, and a token is refused from the instant its lifetime ends, a given-back one included.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_server::grants::{Clock, SystemClock};
use okf_jawn_server::local::{LAUNCH_TOKEN_LIFETIME, LaunchTokens, SecretSource};

use check::{TestResult, err_of, some};

/// Secrets handed out in order; a test knows each one.
struct Sequence(Mutex<Vec<&'static str>>);

/// A clock that moves only when a test advances it.
struct TestClock {
    start: Instant,
    elapsed_ms: AtomicU64,
}

impl SecretSource for Sequence {
    fn secret(&self) -> Result<String, ApiError> {
        self.0
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))?
            .pop()
            .map(str::to_owned)
            .ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "no secret left"))
    }
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

fn tokens() -> LaunchTokens {
    LaunchTokens::new(LAUNCH_TOKEN_LIFETIME, Arc::new(SystemClock))
}

#[test]
fn a_launch_token_works_once_and_its_replay_is_refused() -> TestResult {
    let tokens = tokens();
    let secrets = Sequence(Mutex::new(vec!["second-token", "first-token"]));
    let first = tokens.issue(&secrets)?;
    assert_eq!(first, "first-token");
    assert!(tokens.redeem(&first)?.is_some());
    assert!(tokens.redeem(&first)?.is_none());
    Ok(())
}

#[test]
fn a_token_never_issued_is_refused_and_leaves_the_issued_one_usable() -> TestResult {
    let tokens = tokens();
    let secrets = Sequence(Mutex::new(vec!["second-token", "first-token"]));
    let first = tokens.issue(&secrets)?;
    let second = tokens.issue(&secrets)?;
    assert!(tokens.redeem("guessed-token")?.is_none());
    assert!(tokens.redeem("")?.is_none());
    assert!(tokens.redeem(&second)?.is_some());
    assert!(tokens.redeem(&first)?.is_some());
    Ok(())
}

#[test]
fn a_failing_random_source_issues_nothing() -> TestResult {
    let tokens = tokens();
    let secrets = Sequence(Mutex::new(Vec::new()));
    let refused = err_of(tokens.issue(&secrets))?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(tokens.outstanding()?, 0);
    Ok(())
}

#[test]
fn eight_threads_redeeming_one_token_at_once_get_it_once() -> TestResult {
    for round in 0..300 {
        let tokens = Arc::new(self::tokens());
        let secrets = Sequence(Mutex::new(vec!["the-token"]));
        let token = tokens.issue(&secrets)?;
        let start = Arc::new(Barrier::new(8));
        let redeemers: Vec<_> = (0..8)
            .map(|_| {
                let tokens = tokens.clone();
                let token = token.clone();
                let start = start.clone();
                std::thread::spawn(move || {
                    start.wait();
                    tokens
                        .redeem(&token)
                        .map(|taken| taken.is_some())
                        .map_err(|error| error.to_string())
                })
            })
            .collect();
        let mut redeemed = 0_u32;
        for redeemer in redeemers {
            let outcome = redeemer.join().map_err(|_| "a redeemer thread failed")?;
            if outcome? {
                redeemed = redeemed.saturating_add(1);
            }
        }
        assert_eq!(redeemed, 1, "round {round}");
    }
    Ok(())
}

#[test]
fn a_token_given_back_works_once_more_and_only_once() -> TestResult {
    let tokens = tokens();
    let secrets = Sequence(Mutex::new(vec!["the-token"]));
    let token = tokens.issue(&secrets)?;
    let taken = some(tokens.redeem(&token)?, "the taken token")?;
    // While the exchange that took it runs, a second exchange is refused.
    assert!(tokens.redeem(&token)?.is_none());
    tokens.give_back(taken)?;
    assert!(tokens.redeem(&token)?.is_some());
    assert!(tokens.redeem(&token)?.is_none());
    Ok(())
}

#[test]
fn a_token_is_refused_from_the_instant_its_lifetime_ends() -> TestResult {
    assert_eq!(LAUNCH_TOKEN_LIFETIME, Duration::from_secs(600));
    let clock = TestClock::new();
    let tokens = LaunchTokens::new(Duration::from_secs(600), clock.clone());
    let secrets = Sequence(Mutex::new(vec!["the-token"]));
    let token = tokens.issue(&secrets)?;
    clock.advance(Duration::from_millis(599_999))?;
    let taken = some(tokens.redeem(&token)?, "the token one millisecond short")?;
    // Given back, it keeps the instant it was issued at: no extension.
    tokens.give_back(taken)?;
    clock.advance(Duration::from_millis(1))?;
    assert!(tokens.redeem(&token)?.is_none());
    Ok(())
}

#[test]
fn a_token_given_back_after_its_lifetime_is_not_kept() -> TestResult {
    let clock = TestClock::new();
    let tokens = LaunchTokens::new(Duration::from_secs(600), clock.clone());
    let secrets = Sequence(Mutex::new(vec!["the-token"]));
    let token = tokens.issue(&secrets)?;
    let taken = some(tokens.redeem(&token)?, "the taken token")?;
    clock.advance(Duration::from_secs(600))?;
    tokens.give_back(taken)?;
    assert_eq!(tokens.outstanding()?, 0);
    assert!(tokens.redeem(&token)?.is_none());
    Ok(())
}

#[test]
fn expired_tokens_are_dropped_as_new_ones_are_issued() -> TestResult {
    let clock = TestClock::new();
    let tokens = LaunchTokens::new(Duration::from_secs(600), clock.clone());
    let secrets = Sequence(Mutex::new(vec!["third", "second", "first"]));
    tokens.issue(&secrets)?;
    tokens.issue(&secrets)?;
    clock.advance(Duration::from_secs(600))?;
    let third = tokens.issue(&secrets)?;
    assert_eq!(tokens.outstanding()?, 1);
    assert!(tokens.redeem(&third)?.is_some());
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
