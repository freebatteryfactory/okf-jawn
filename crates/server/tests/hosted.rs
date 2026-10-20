//! The sign-in cookie's value: a bounded state, `:`, and a bounded PKCE verifier, each of its own
//! characters. Nothing else is ever handed to a browser or read back from one.

use okf_jawn_contract::error::ErrorCode;
use okf_jawn_server::hosted::{LoginStart, PendingLogin};

use check::{TestResult, err_of, some};

fn start(state: &str, verifier: &str) -> LoginStart {
    LoginStart {
        url: "https://auth.example.test/authorize".to_owned(),
        verifier: verifier.to_owned(),
        state: state.to_owned(),
    }
}

fn of(count: usize) -> String {
    "a".repeat(count)
}

#[test]
fn a_started_sign_in_round_trips_through_its_cookie_value() -> TestResult {
    let state = "State-of_one-sign-in";
    let verifier = "Verifier.of_one-sign~in.0123456789abcdefghij";
    let value = PendingLogin::encode(&start(state, verifier))?;
    assert_eq!(value, format!("{state}:{verifier}"));
    let read = some(PendingLogin::parse(&value), "the parsed cookie")?;
    assert_eq!(read.state, state);
    assert_eq!(read.verifier, verifier);
    Ok(())
}

#[test]
fn the_state_is_sixteen_to_128_characters() {
    let verifier = of(43);
    for (length, accepted) in [(15, false), (16, true), (128, true), (129, false)] {
        let value = format!("{}:{verifier}", of(length));
        assert_eq!(
            PendingLogin::parse(&value).is_some(),
            accepted,
            "state of {length}"
        );
        let encoded = PendingLogin::encode(&start(&of(length), &verifier));
        assert_eq!(encoded.is_ok(), accepted, "encoding a state of {length}");
    }
}

#[test]
fn the_verifier_is_43_to_128_characters() {
    let state = of(16);
    for (length, accepted) in [(42, false), (43, true), (128, true), (129, false)] {
        let value = format!("{state}:{}", of(length));
        assert_eq!(
            PendingLogin::parse(&value).is_some(),
            accepted,
            "verifier of {length}"
        );
        let encoded = PendingLogin::encode(&start(&state, &of(length)));
        assert_eq!(encoded.is_ok(), accepted, "encoding a verifier of {length}");
    }
}

#[test]
fn each_part_has_only_its_own_characters() -> TestResult {
    let state = of(16);
    let verifier = of(43);
    for bad_state in [
        format!("{}.", of(16)),
        format!("{}~", of(16)),
        format!("{} ", of(16)),
        format!("{}=", of(16)),
        format!("{}\u{e9}", of(16)),
    ] {
        assert!(PendingLogin::parse(&format!("{bad_state}:{verifier}")).is_none());
        let refused = err_of(PendingLogin::encode(&start(&bad_state, &verifier)))?;
        assert_eq!(refused.code, ErrorCode::Internal);
    }
    for bad_verifier in [
        format!("{}:", of(43)),
        format!("{};", of(43)),
        format!("{} ", of(43)),
        format!("{}/", of(43)),
        format!("{}\u{e9}", of(43)),
    ] {
        assert!(PendingLogin::parse(&format!("{state}:{bad_verifier}")).is_none());
        let refused = err_of(PendingLogin::encode(&start(&state, &bad_verifier)))?;
        assert_eq!(refused.code, ErrorCode::Internal);
    }
    // No separator, and nothing at all.
    assert!(PendingLogin::parse(&format!("{state}{verifier}")).is_none());
    assert!(PendingLogin::parse("").is_none());
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
