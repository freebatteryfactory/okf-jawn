//! Local launch tokens: a token works once, a replay is refused, and a token never issued is
//! refused.

use std::sync::{Arc, Barrier, Mutex};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_server::local::{LaunchTokens, SecretSource};

use check::{TestResult, err_of};

/// Secrets handed out in order; a test knows each one.
struct Sequence(Mutex<Vec<&'static str>>);

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

#[test]
fn a_launch_token_works_once_and_its_replay_is_refused() -> TestResult {
    let tokens = LaunchTokens::new();
    let secrets = Sequence(Mutex::new(vec!["second-token", "first-token"]));
    let first = tokens.issue(&secrets)?;
    assert_eq!(first, "first-token");
    assert!(tokens.redeem(&first)?);
    assert!(!tokens.redeem(&first)?);
    Ok(())
}

#[test]
fn a_token_never_issued_is_refused_and_leaves_the_issued_one_usable() -> TestResult {
    let tokens = LaunchTokens::new();
    let secrets = Sequence(Mutex::new(vec!["second-token", "first-token"]));
    let first = tokens.issue(&secrets)?;
    let second = tokens.issue(&secrets)?;
    assert!(!tokens.redeem("guessed-token")?);
    assert!(!tokens.redeem("")?);
    assert!(tokens.redeem(&second)?);
    assert!(tokens.redeem(&first)?);
    Ok(())
}

#[test]
fn a_failing_random_source_issues_nothing() -> TestResult {
    let tokens = LaunchTokens::new();
    let secrets = Sequence(Mutex::new(Vec::new()));
    let refused = err_of(tokens.issue(&secrets))?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    Ok(())
}

#[test]
fn eight_threads_redeeming_one_token_at_once_get_it_once() -> TestResult {
    for round in 0..300 {
        let tokens = Arc::new(LaunchTokens::new());
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
                    tokens.redeem(&token).map_err(|error| error.to_string())
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
    let tokens = LaunchTokens::new();
    let secrets = Sequence(Mutex::new(vec!["the-token"]));
    let token = tokens.issue(&secrets)?;
    assert!(tokens.redeem(&token)?);
    // While the exchange that took it runs, a second exchange is refused.
    assert!(!tokens.redeem(&token)?);
    tokens.give_back(&token)?;
    tokens.give_back(&token)?;
    assert!(tokens.redeem(&token)?);
    assert!(!tokens.redeem(&token)?);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
