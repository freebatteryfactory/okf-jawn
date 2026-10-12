//! Authentication mode selection: a missing or invalid hosted configuration fails startup and
//! never falls back to local.

use std::collections::BTreeMap;

use okf_jawn_contract::error::ErrorCode;
use okf_jawn_server::config::{AuthConfig, AuthKind};

use check::{TestResult, err_of};

fn complete_hosted() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("OKF_AUTH_MODE", "workos"),
        ("WORKOS_CLIENT_ID", "client_01"),
        ("WORKOS_API_KEY", "not-a-real-api-key"),
        ("WORKOS_ISSUER", "https://auth.example.test"),
        ("WORKOS_MCP_RESOURCE", "https://okf.example.test/mcp"),
        (
            "WORKOS_REDIRECT_URI",
            "https://okf.example.test/auth/callback",
        ),
    ])
}

fn read(
    settings: &BTreeMap<&'static str, &'static str>,
) -> Result<AuthConfig, okf_jawn_contract::error::ApiError> {
    AuthConfig::from_lookup(|name| settings.get(name).map(|value| (*value).to_owned()))
}

#[test]
fn an_unset_mode_is_local() -> TestResult {
    assert_eq!(read(&BTreeMap::new())?.kind(), AuthKind::Local);
    Ok(())
}

#[test]
fn a_complete_hosted_configuration_is_hosted() -> TestResult {
    assert_eq!(read(&complete_hosted())?.kind(), AuthKind::Hosted);
    Ok(())
}

#[test]
fn every_missing_hosted_setting_fails_and_never_answers_local() -> TestResult {
    for missing in [
        "WORKOS_CLIENT_ID",
        "WORKOS_API_KEY",
        "WORKOS_ISSUER",
        "WORKOS_MCP_RESOURCE",
        "WORKOS_REDIRECT_URI",
    ] {
        let mut settings = complete_hosted();
        settings.remove(missing);
        let error = err_of(read(&settings))?;
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(error.field.as_deref(), Some(missing));
        settings.insert(missing, "   ");
        assert_eq!(err_of(read(&settings))?.field.as_deref(), Some(missing));
    }
    Ok(())
}

#[test]
fn hosted_urls_must_be_https() -> TestResult {
    for (setting, value) in [
        ("WORKOS_ISSUER", "http://auth.example.test"),
        ("WORKOS_MCP_RESOURCE", "okf.example.test/mcp"),
        (
            "WORKOS_REDIRECT_URI",
            "http://okf.example.test/auth/callback",
        ),
        ("WORKOS_REDIRECT_URI", "https://okf.example.test/elsewhere"),
    ] {
        let mut settings = complete_hosted();
        settings.insert(setting, value);
        assert_eq!(err_of(read(&settings))?.field.as_deref(), Some(setting));
    }
    Ok(())
}

#[test]
fn an_unknown_mode_is_refused() -> TestResult {
    let settings = BTreeMap::from([("OKF_AUTH_MODE", "guest")]);
    assert_eq!(
        err_of(read(&settings))?.field.as_deref(),
        Some("OKF_AUTH_MODE")
    );
    Ok(())
}

#[test]
fn the_api_key_never_appears_in_debug_output() -> TestResult {
    let AuthConfig::Hosted(hosted) = read(&complete_hosted())? else {
        return Err("expected a hosted configuration".into());
    };
    assert!(!format!("{hosted:?}").contains("not-a-real-api-key"));
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
