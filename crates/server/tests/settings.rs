//! Deployment settings: every `OKF_*` setting is read once, gets its default, or is refused by
//! name before anything is touched (server-m2-design.md section 1.2).
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_server::config::AuthKind;
use okf_jawn_server::settings::{
    DEFAULT_CONVERTER_MEMORY_LIMIT_BYTES, Places, Resolved, Settings, offline_settings,
};

use check::{TestResult, err_of, some};

type Values = BTreeMap<&'static str, String>;

/// Places of a machine whose executable and per-user data directory are known.
fn places() -> Places {
    Places {
        exe_dir: Some(absolute("app")),
        user_data: Some(absolute("user-data")),
    }
}

/// An absolute path under the system's temporary directory.
fn absolute(name: &str) -> PathBuf {
    std::env::temp_dir().join("okf-jawn-settings").join(name)
}

fn values(pairs: &[(&'static str, &str)]) -> Values {
    pairs
        .iter()
        .map(|(name, value)| (*name, (*value).to_owned()))
        .collect()
}

fn read(values: &Values) -> Result<Settings, ApiError> {
    Settings::from_lookup(|name| values.get(name).cloned(), &places())
}

/// Read, then resolve against the configured addresses as if they were bound as given.
fn resolve(values: &Values) -> Result<Resolved, ApiError> {
    let settings = read(values)?;
    settings.resolve(settings.bind, settings.sandbox_bind)
}

/// The setting a refusal names.
fn refused(result: Result<impl std::fmt::Debug, ApiError>) -> Result<String, String> {
    let error = err_of(result)?;
    if error.code != ErrorCode::InvalidInput {
        return Err(format!("expected invalid_input, got {:?}", error.code));
    }
    error
        .field
        .ok_or_else(|| format!("the refusal names no setting: {}", error.message))
}

fn hosted(public: &str) -> Values {
    values(&[
        ("OKF_AUTH_MODE", "workos"),
        ("WORKOS_CLIENT_ID", "client_01"),
        ("WORKOS_API_KEY", "not-a-real-api-key"),
        ("WORKOS_ISSUER", "https://auth.example.test"),
        ("WORKOS_MCP_RESOURCE", &format!("{public}/mcp")),
        ("WORKOS_REDIRECT_URI", &format!("{public}/auth/callback")),
        ("OKF_PUBLIC_ORIGIN", public),
        ("OKF_SANDBOX_ORIGIN", "https://sandbox.example.test"),
        ("OKF_BIND", "0.0.0.0:7711"),
    ])
}

fn address(text: &str) -> Result<SocketAddr, std::net::AddrParseError> {
    text.parse()
}

#[test]
fn nothing_set_is_a_loopback_installation_with_every_default() -> TestResult {
    let settings = read(&Values::new())?;
    assert_eq!(settings.auth.kind(), AuthKind::Local);
    assert_eq!(settings.bind, address("127.0.0.1:7711")?);
    assert_eq!(settings.sandbox_bind, address("127.0.0.1:7712")?);
    assert_eq!(settings.data_dir, absolute("user-data"));
    assert_eq!(settings.ui_dir, absolute("app").join("ui"));
    assert_eq!(
        settings.mcp_app,
        absolute("app").join("mcp-apps").join("app.html")
    );
    assert_eq!(
        settings.converter,
        absolute("app").join(format!("okf-jawn-convert{}", std::env::consts::EXE_SUFFIX))
    );
    assert_eq!(
        settings.converter_memory_limit_bytes,
        DEFAULT_CONVERTER_MEMORY_LIMIT_BYTES
    );
    assert_eq!(DEFAULT_CONVERTER_MEMORY_LIMIT_BYTES, 4_294_967_296);
    assert_eq!(settings.trusted_proxies, [] as [IpAddr; 0]);
    let resolved = settings.resolve(settings.bind, settings.sandbox_bind)?;
    assert_eq!(resolved.public_origin, "http://127.0.0.1:7711");
    assert_eq!(resolved.hosts, vec!["127.0.0.1:7711".to_owned()]);
    assert_eq!(resolved.sandbox_origin, "http://127.0.0.1:7712");
    assert_eq!(resolved.sandbox_host, "127.0.0.1:7712");
    Ok(())
}

#[test]
fn an_ipv6_loopback_bind_derives_bracketed_origins() -> TestResult {
    let resolved = resolve(&values(&[("OKF_BIND", "[::1]:7711")]))?;
    assert_eq!(resolved.public_origin, "http://[::1]:7711");
    assert_eq!(resolved.sandbox_origin, "http://[::1]:7712");
    Ok(())
}

#[test]
fn a_port_zero_bind_takes_its_origins_from_the_bound_addresses() -> TestResult {
    let settings = read(&values(&[("OKF_BIND", "127.0.0.1:0")]))?;
    assert_eq!(settings.sandbox_bind, address("127.0.0.1:0")?);
    let resolved = settings.resolve(address("127.0.0.1:50001")?, address("127.0.0.1:50002")?)?;
    assert_eq!(resolved.public_origin, "http://127.0.0.1:50001");
    assert_eq!(resolved.hosts, vec!["127.0.0.1:50001".to_owned()]);
    assert_eq!(resolved.sandbox_origin, "http://127.0.0.1:50002");
    Ok(())
}

#[test]
fn a_bind_must_be_an_ip_and_a_port() -> TestResult {
    for bad in [
        "localhost:7711",
        "127.0.0.1",
        ":7711",
        "127.0.0.1:70000",
        " 127.0.0.1:7711",
        "127.0.0.1:7711 ",
        "",
    ] {
        assert_eq!(
            refused(read(&values(&[("OKF_BIND", bad)])))?,
            "OKF_BIND",
            "{bad:?}"
        );
        assert_eq!(
            refused(read(&values(&[("OKF_SANDBOX_BIND", bad)])))?,
            "OKF_SANDBOX_BIND",
            "{bad:?}"
        );
    }
    Ok(())
}

#[test]
fn a_bind_that_is_not_loopback_needs_a_public_origin() -> TestResult {
    assert_eq!(
        refused(read(&values(&[("OKF_BIND", "0.0.0.0:7711")])))?,
        "OKF_PUBLIC_ORIGIN"
    );
    // With a public origin it is the sandbox origin that is then required.
    assert_eq!(
        refused(read(&values(&[
            ("OKF_BIND", "0.0.0.0:7711"),
            ("OKF_PUBLIC_ORIGIN", "https://okf.example.test"),
        ])))?,
        "OKF_SANDBOX_ORIGIN"
    );
    let resolved = resolve(&values(&[
        ("OKF_BIND", "0.0.0.0:7711"),
        ("OKF_PUBLIC_ORIGIN", "https://okf.example.test"),
        ("OKF_SANDBOX_ORIGIN", "https://sandbox.example.test"),
    ]))?;
    assert_eq!(resolved.public_origin, "https://okf.example.test");
    assert_eq!(resolved.hosts, vec!["okf.example.test".to_owned()]);
    assert_eq!(resolved.sandbox_host, "sandbox.example.test");
    Ok(())
}

#[test]
fn an_origin_must_be_written_as_a_browser_writes_it() -> TestResult {
    for bad in [
        "https://okf.example.test/",
        "https://okf.example.test/app",
        "https://OKF.example.test",
        "https://okf.example.test:443",
        "http://127.0.0.1:80",
        "https://u:p@okf.example.test",
        "https://okf.example.test?x",
        "https://okf.example.test#x",
        "ftp://okf.example.test",
        "okf.example.test",
        "",
    ] {
        let setting = refused(read(&values(&[
            ("OKF_PUBLIC_ORIGIN", bad),
            ("OKF_SANDBOX_ORIGIN", "https://sandbox.example.test"),
        ])))?;
        assert_eq!(setting, "OKF_PUBLIC_ORIGIN", "{bad:?}");
        let setting = refused(read(&values(&[("OKF_SANDBOX_ORIGIN", bad)])))?;
        assert_eq!(setting, "OKF_SANDBOX_ORIGIN", "{bad:?}");
    }
    Ok(())
}

#[test]
fn plain_http_is_for_loopback_hosts_only() -> TestResult {
    assert_eq!(
        refused(read(&values(&[(
            "OKF_PUBLIC_ORIGIN",
            "http://okf.example.test"
        )])))?,
        "OKF_PUBLIC_ORIGIN"
    );
    assert_eq!(
        refused(read(&values(&[(
            "OKF_SANDBOX_ORIGIN",
            "http://sandbox.example.test"
        )])))?,
        "OKF_SANDBOX_ORIGIN"
    );
    for loopback in [
        "http://localhost:7711",
        "http://127.0.0.2:7711",
        "http://[::1]:7711",
    ] {
        let settings = read(&values(&[("OKF_PUBLIC_ORIGIN", loopback)]))?;
        assert_eq!(settings.public_origin.as_deref(), Some(loopback));
    }
    Ok(())
}

#[test]
fn a_hosted_deployment_is_https_with_its_urls_under_the_public_origin() -> TestResult {
    let resolved = resolve(&hosted("https://okf.example.test"))?;
    assert_eq!(resolved.public_origin, "https://okf.example.test");
    let mut plain = hosted("https://okf.example.test");
    plain.insert("OKF_PUBLIC_ORIGIN", "http://127.0.0.1:7711".to_owned());
    assert_eq!(refused(read(&plain))?, "OKF_PUBLIC_ORIGIN");
    let mut missing = hosted("https://okf.example.test");
    missing.remove("OKF_PUBLIC_ORIGIN");
    missing.insert("OKF_BIND", "127.0.0.1:7711".to_owned());
    assert_eq!(refused(read(&missing))?, "OKF_PUBLIC_ORIGIN");
    let mut no_sandbox = hosted("https://okf.example.test");
    no_sandbox.remove("OKF_SANDBOX_ORIGIN");
    no_sandbox.insert("OKF_BIND", "127.0.0.1:7711".to_owned());
    assert_eq!(refused(read(&no_sandbox))?, "OKF_SANDBOX_ORIGIN");
    let mut sandbox_http = hosted("https://okf.example.test");
    sandbox_http.insert("OKF_SANDBOX_ORIGIN", "http://127.0.0.1:7712".to_owned());
    assert_eq!(refused(read(&sandbox_http))?, "OKF_SANDBOX_ORIGIN");
    for (setting, value) in [
        (
            "WORKOS_REDIRECT_URI",
            "https://other.example.test/auth/callback",
        ),
        ("WORKOS_MCP_RESOURCE", "https://okf.example.test/mcp/"),
        ("WORKOS_MCP_RESOURCE", "https://okf.example.test/"),
        ("WORKOS_MCP_RESOURCE", "https://other.example.test/mcp"),
    ] {
        let mut settings = hosted("https://okf.example.test");
        settings.insert(setting, value.to_owned());
        assert_eq!(refused(resolve(&settings))?, setting, "{value}");
    }
    Ok(())
}

#[test]
fn allowed_hosts_are_an_exact_list_without_empty_entries() -> TestResult {
    let resolved = resolve(&values(&[(
        "OKF_ALLOWED_HOSTS",
        "okf.example.test,okf.internal:7711",
    )]))?;
    assert_eq!(
        resolved.hosts,
        vec![
            "okf.example.test".to_owned(),
            "okf.internal:7711".to_owned()
        ]
    );
    for bad in ["", "a,,b", "a,", ",a", "a, b", "a@b", "*", "a/b", "a b"] {
        assert_eq!(
            refused(read(&values(&[("OKF_ALLOWED_HOSTS", bad)])))?,
            "OKF_ALLOWED_HOSTS",
            "{bad:?}"
        );
    }
    Ok(())
}

#[test]
fn the_sandbox_listens_beside_the_application_and_never_on_it() -> TestResult {
    assert_eq!(
        refused(read(&values(&[("OKF_SANDBOX_BIND", "127.0.0.1:7711")])))?,
        "OKF_SANDBOX_BIND"
    );
    assert_eq!(
        refused(read(&values(&[("OKF_BIND", "127.0.0.1:65535")])))?,
        "OKF_SANDBOX_BIND"
    );
    let settings = read(&values(&[
        ("OKF_BIND", "127.0.0.1:65535"),
        ("OKF_SANDBOX_BIND", "127.0.0.1:7712"),
    ]))?;
    assert_eq!(settings.sandbox_bind, address("127.0.0.1:7712")?);
    Ok(())
}

#[test]
fn the_sandbox_origin_is_never_the_application_s() -> TestResult {
    assert_eq!(
        refused(resolve(&values(&[(
            "OKF_SANDBOX_ORIGIN",
            "http://127.0.0.1:7711"
        )])))?,
        "OKF_SANDBOX_ORIGIN"
    );
    // Not the same origin, but one of the hosts the application answers.
    assert_eq!(
        refused(resolve(&values(&[
            ("OKF_PUBLIC_ORIGIN", "http://localhost:7711"),
            ("OKF_ALLOWED_HOSTS", "localhost:7711,127.0.0.1:7712"),
            ("OKF_SANDBOX_ORIGIN", "http://127.0.0.1:7712"),
        ])))?,
        "OKF_SANDBOX_ORIGIN"
    );
    // The schemes must agree: an https page cannot frame an http sandbox.
    assert_eq!(
        refused(resolve(&values(&[
            ("OKF_PUBLIC_ORIGIN", "https://okf.example.test"),
            ("OKF_SANDBOX_ORIGIN", "http://127.0.0.1:7712"),
        ])))?,
        "OKF_SANDBOX_ORIGIN"
    );
    Ok(())
}

#[test]
fn every_path_setting_is_absolute() -> TestResult {
    for setting in ["OKF_DATA_DIR", "OKF_UI_DIR", "OKF_MCP_APP", "OKF_CONVERTER"] {
        for bad in ["./.okf-jawn-data", "okf-data", ""] {
            assert_eq!(
                refused(read(&values(&[(setting, bad)])))?,
                setting,
                "{bad:?}"
            );
        }
        let chosen = absolute("chosen");
        let text = some(chosen.to_str(), "a UTF-8 path")?;
        let settings = read(&values(&[(setting, text)]))?;
        let read_back = match setting {
            "OKF_DATA_DIR" => settings.data_dir,
            "OKF_UI_DIR" => settings.ui_dir,
            "OKF_MCP_APP" => settings.mcp_app,
            _ => settings.converter,
        };
        assert_eq!(read_back, chosen);
    }
    Ok(())
}

#[test]
fn a_default_path_needs_its_place() -> TestResult {
    let nowhere = Places {
        exe_dir: None,
        user_data: None,
    };
    let lookup = |_: &str| None;
    let error = err_of(Settings::from_lookup(lookup, &nowhere))?;
    assert_eq!(error.field.as_deref(), Some("OKF_DATA_DIR"));
    let data = absolute("data");
    let data = some(data.to_str(), "a UTF-8 path")?.to_owned();
    let with_data = |name: &str| (name == "OKF_DATA_DIR").then(|| data.clone());
    let error = err_of(Settings::from_lookup(with_data, &nowhere))?;
    assert_eq!(error.field.as_deref(), Some("OKF_UI_DIR"));
    Ok(())
}

#[test]
fn the_memory_cap_is_a_byte_count_of_at_least_256_mib() -> TestResult {
    let settings = read(&values(&[(
        "OKF_CONVERTER_MEMORY_LIMIT_BYTES",
        "268435456",
    )]))?;
    assert_eq!(settings.converter_memory_limit_bytes, 268_435_456);
    for bad in ["268435455", "4GiB", "-1", "", " 268435456", "0"] {
        assert_eq!(
            refused(read(&values(&[("OKF_CONVERTER_MEMORY_LIMIT_BYTES", bad)])))?,
            "OKF_CONVERTER_MEMORY_LIMIT_BYTES",
            "{bad:?}"
        );
    }
    Ok(())
}

#[test]
fn trusted_proxies_are_ip_addresses() -> TestResult {
    let settings = read(&values(&[("OKF_TRUSTED_PROXIES", "10.0.0.5,fd00::1")]))?;
    assert_eq!(
        settings.trusted_proxies,
        vec!["10.0.0.5".parse::<std::net::IpAddr>()?, "fd00::1".parse()?]
    );
    for bad in [
        "10.0.0.0/8",
        "10.0.0.5,",
        "",
        "proxy.internal",
        "10.0.0.5, 10.0.0.6",
    ] {
        assert_eq!(
            refused(read(&values(&[("OKF_TRUSTED_PROXIES", bad)])))?,
            "OKF_TRUSTED_PROXIES",
            "{bad:?}"
        );
    }
    Ok(())
}

#[test]
fn a_hosted_mode_without_its_settings_is_refused_as_before() -> TestResult {
    assert_eq!(
        refused(read(&values(&[("OKF_AUTH_MODE", "workos")])))?,
        "WORKOS_CLIENT_ID"
    );
    assert_eq!(
        refused(read(&values(&[("WORKOS_CLIENT_ID", "client_01")])))?,
        "OKF_AUTH_MODE"
    );
    Ok(())
}

#[test]
fn the_offline_commands_read_only_the_mode_and_the_data_directory() -> TestResult {
    let lookup = |name: &str| (name == "OKF_AUTH_MODE").then(|| "workos".to_owned());
    let (kind, data_dir) = offline_settings(lookup, &places())?;
    assert_eq!(kind, AuthKind::Hosted);
    assert_eq!(data_dir, absolute("user-data"));
    // A setting the service would refuse does not stop an offline command.
    let lookup = |name: &str| (name == "OKF_BIND").then(|| "not an address".to_owned());
    assert_eq!(offline_settings(lookup, &places())?.0, AuthKind::Local);
    // The mode rule still holds: hosted settings without a mode are refused.
    let lookup = |name: &str| (name == "WORKOS_API_KEY").then(|| "key".to_owned());
    assert_eq!(
        refused(offline_settings(lookup, &places()))?,
        "OKF_AUTH_MODE"
    );
    let lookup = |name: &str| (name == "OKF_DATA_DIR").then(|| "relative".to_owned());
    assert_eq!(
        refused(offline_settings(lookup, &places()))?,
        "OKF_DATA_DIR"
    );
    Ok(())
}

#[test]
fn the_three_files_are_checked_before_anything_is_touched() -> TestResult {
    let root = std::env::temp_dir().join(format!("okf-jawn-settings-files-{}", std::process::id()));
    let ui = root.join("ui");
    std::fs::create_dir_all(&ui)?;
    let app = root.join("app.html");
    let child = root.join("convert");
    let text = |path: &std::path::Path| path.to_str().map(str::to_owned);
    let (ui_text, app_text, child_text) = (
        some(text(&ui), "a UTF-8 path")?,
        some(text(&app), "a UTF-8 path")?,
        some(text(&child), "a UTF-8 path")?,
    );
    let all = values(&[
        ("OKF_UI_DIR", &ui_text),
        ("OKF_MCP_APP", &app_text),
        ("OKF_CONVERTER", &child_text),
    ]);
    let settings = read(&all)?;
    assert_eq!(refused(settings.check_files())?, "OKF_UI_DIR");
    std::fs::write(ui.join("index.html"), "<!doctype html>")?;
    assert_eq!(refused(settings.check_files())?, "OKF_MCP_APP");
    std::fs::write(&app, "<!doctype html>")?;
    assert_eq!(refused(settings.check_files())?, "OKF_CONVERTER");
    std::fs::create_dir_all(&child)?;
    // A directory is not the conversion child.
    assert_eq!(refused(settings.check_files())?, "OKF_CONVERTER");
    std::fs::remove_dir(&child)?;
    std::fs::write(&child, "")?;
    settings.check_files()?;
    // Best effort: a leftover directory under the temporary directory harms nothing.
    let _removed = std::fs::remove_dir_all(&root);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
