//! Local launch tokens: a token works once, a replay is refused, and a token never issued is
//! refused.

use std::sync::Mutex;

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

#[path = "../../../tests/support/check.rs"]
mod check;
