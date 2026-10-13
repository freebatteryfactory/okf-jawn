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

#[test]
fn any_hosted_setting_without_a_mode_is_refused_and_never_starts_local() -> TestResult {
    for present in [
        "WORKOS_CLIENT_ID",
        "WORKOS_API_KEY",
        "WORKOS_ISSUER",
        "WORKOS_MCP_RESOURCE",
        "WORKOS_REDIRECT_URI",
    ] {
        // One setting alone, with no mode.
        let settings = BTreeMap::from([(present, "anything")]);
        let error = err_of(read(&settings))?;
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("OKF_AUTH_MODE"), "{present}");
    }
    // All five, complete and valid, with no mode: still refused.
    let mut settings = complete_hosted();
    settings.remove("OKF_AUTH_MODE");
    assert_eq!(
        err_of(read(&settings))?.field.as_deref(),
        Some("OKF_AUTH_MODE")
    );
    // A blank hosted setting is no setting; an explicit local mode beside hosted settings is local.
    let blank = BTreeMap::from([("WORKOS_API_KEY", "  "), ("WORKOS_ISSUER", "")]);
    assert_eq!(read(&blank)?.kind(), AuthKind::Local);
    let mut explicit = complete_hosted();
    explicit.insert("OKF_AUTH_MODE", "local");
    assert_eq!(read(&explicit)?.kind(), AuthKind::Local);
    Ok(())
}

#[test]
fn a_mode_with_surrounding_whitespace_is_refused_not_trimmed() -> TestResult {
    for mode in [
        "local\n",
        " local",
        "local ",
        "workos\r\n",
        " workos",
        "",
        "Local",
        "WORKOS",
    ] {
        let mut settings = complete_hosted();
        settings.insert("OKF_AUTH_MODE", mode);
        let error = err_of(read(&settings))?;
        assert_eq!(error.field.as_deref(), Some("OKF_AUTH_MODE"), "{mode:?}");
        // The same without any hosted setting.
        let alone = BTreeMap::from([("OKF_AUTH_MODE", mode)]);
        assert_eq!(
            err_of(read(&alone))?.field.as_deref(),
            Some("OKF_AUTH_MODE"),
            "{mode:?} alone"
        );
    }
    Ok(())
}

#[test]
fn a_hosted_url_with_a_query_a_fragment_or_user_information_is_refused() -> TestResult {
    for (setting, base) in [
        ("WORKOS_ISSUER", "auth.example.test"),
        ("WORKOS_MCP_RESOURCE", "okf.example.test/mcp"),
        ("WORKOS_REDIRECT_URI", "okf.example.test/auth/callback"),
    ] {
        for url in [
            format!("https://{base}?next=//attacker.example"),
            format!("https://{base}?"),
            format!("https://{base}#fragment"),
            format!("https://user@{base}"),
            format!("https://user:secret@{base}"),
        ] {
            let mut settings = complete_hosted();
            settings.insert(setting, Box::leak(url.clone().into_boxed_str()));
            let error = err_of(read(&settings))?;
            assert_eq!(error.field.as_deref(), Some(setting), "{url}");
            // No refusal quotes the URL: it may carry a credential.
            assert!(!error.message.contains("secret"));
        }
        // The same URL without the extra part is accepted.
        let mut settings = complete_hosted();
        settings.insert(
            setting,
            Box::leak(format!("https://{base}").into_boxed_str()),
        );
        assert_eq!(read(&settings)?.kind(), AuthKind::Hosted);
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
