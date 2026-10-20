//! The one request path of an operation command, and the safety rules of the address and the
//! token.
//!
//! Each test runs the real entry point, `okf_jawn_cli::run`, in this process against a
//! loopback listener answered by hand, and asserts the exact request sent, both output
//! streams and the exit code. The last tests run the built executable, for what only a
//! process shows: its exit code and its treatment of a configured proxy.

use std::process::{Command, Stdio};

use okf_jawn_cli::Endpoint;
use okf_jawn_contract::transport::MAX_REQUEST_BODY_BYTES;
use serde_json::json;

use crate::check::{TestResult, err_of, some};
use crate::support::http::{Answering, Heard, answer, closed, json as json_answer};
use crate::support::{EXECUTABLE, Outcome, TOKEN, cli, file};

/// The only request a listener received.
fn only(listener: &Answering) -> Outcome<Heard> {
    let heard = listener.heard()?;
    assert_eq!(heard.len(), 1, "{heard:?}");
    Ok(some(heard.into_iter().next(), "a request")?)
}

#[tokio::test]
async fn inline_json_is_posted_to_the_operation_and_the_body_goes_to_stdout() -> TestResult {
    let listener = Answering::with(vec![json_answer("200 OK", r#"{"items":[]}"#)]).await?;
    let ran = cli(
        &[
            "list_items",
            "--server",
            &listener.origin(),
            "--json",
            r#"{ "workspace_id": "w" }"#,
        ],
        Some(TOKEN),
        b"",
    )
    .await?;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    assert_eq!(ran.stdout, "{\"items\":[]}\n");
    let heard = only(&listener)?;
    assert_eq!(heard.line(), "POST /api/items/list-items HTTP/1.1");
    assert_eq!(
        heard.header("content-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(
        heard.header("authorization"),
        Some(format!("Bearer {TOKEN}"))
    );
    assert_eq!(
        heard.body, br#"{ "workspace_id": "w" }"#,
        "the input is sent as given"
    );
    Ok(())
}

#[tokio::test]
async fn an_alias_runs_its_operation_with_an_empty_object_by_default() -> TestResult {
    let listener = Answering::with(vec![json_answer("200 OK", "{}\n")]).await?;
    let ran = cli(&["grep", "--server", &listener.origin()], Some(TOKEN), b"").await?;
    assert_eq!(
        (ran.code, ran.stdout.as_str()),
        (0, "{}\n"),
        "{}",
        ran.stderr
    );
    let heard = only(&listener)?;
    assert_eq!(heard.line(), "POST /api/search/search-items HTTP/1.1");
    assert_eq!(heard.body, b"{}");
    Ok(())
}

#[tokio::test]
async fn the_input_is_read_from_a_file_or_from_stdin() -> TestResult {
    let listener = Answering::with(vec![
        json_answer("200 OK", "1"),
        json_answer("202 Accepted", "2"),
    ])
    .await?;
    let path = file("input.json", br#"{"from":"file"}"#)?;
    let from_file = cli(
        &[
            "get_item",
            "--server",
            &listener.origin(),
            "--json",
            &format!("@{}", path.display()),
        ],
        Some(TOKEN),
        b"never read",
    )
    .await?;
    assert_eq!(
        (from_file.code, from_file.stdout.as_str()),
        (0, "1\n"),
        "{}",
        from_file.stderr
    );
    let from_stdin = cli(
        &[
            "start_import",
            "--server",
            &listener.origin(),
            "--json",
            "-",
        ],
        Some(TOKEN),
        br#"{"from":"stdin"}"#,
    )
    .await?;
    assert_eq!(
        (from_stdin.code, from_stdin.stdout.as_str()),
        (0, "2\n"),
        "{}",
        from_stdin.stderr
    );
    let bodies: Vec<Vec<u8>> = listener
        .heard()?
        .into_iter()
        .map(|heard| heard.body)
        .collect();
    assert_eq!(
        bodies,
        [
            br#"{"from":"file"}"#.to_vec(),
            br#"{"from":"stdin"}"#.to_vec()
        ]
    );
    std::fs::remove_file(path)?;
    Ok(())
}

/// The body is written as it is; a line feed is added only when the body does not end with one.
#[tokio::test]
async fn nothing_but_the_response_body_goes_to_stdout() -> TestResult {
    for (body, expected) in [
        ("{\"a\":1}", "{\"a\":1}\n"),
        ("{\"a\":1}\n", "{\"a\":1}\n"),
        ("", ""),
    ] {
        let listener = Answering::with(vec![json_answer("200 OK", body)]).await?;
        let ran = cli(&["get_health", "--server", &listener.origin()], None, b"").await?;
        assert_eq!(
            (ran.code, ran.stdout.as_str(), ran.stderr.as_str()),
            (0, expected, "")
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_refusal_goes_to_stderr_with_its_code_and_the_exit_code_is_1() -> TestResult {
    let refusal = json!({
        "code": "conflict",
        "message": "The key was used with another request",
        "field": "mutation_id",
        "request_id": "req-17",
        "detail": {"kind": "idempotency_conflict", "operation": "create_item"}
    });
    let listener = Answering::with(vec![json_answer("409 Conflict", &refusal.to_string())]).await?;
    let ran = cli(
        &["create_item", "--server", &listener.origin()],
        Some(TOKEN),
        b"",
    )
    .await?;
    assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
    assert_eq!(
        ran.stderr,
        "okf-jawn: conflict: The key was used with another request\n  field: mutation_id\n  \
         request: req-17\n  detail: {\"kind\":\"idempotency_conflict\",\"operation\":\"create_item\"}\n"
    );
    Ok(())
}

/// The service's refusal is read member by member: a member, a detail kind or a code this
/// CLI does not know costs neither the code nor the message. The detail it cannot type is
/// printed as the service sent it, and an unknown code is named beside the message while the
/// status decides the code.
#[tokio::test]
async fn a_refusal_with_what_this_cli_does_not_know_keeps_its_code_and_message() -> TestResult {
    let newer = json!({
        "code": "conflict",
        "message": "the item moved",
        "field": "path",
        "request_id": "req-21",
        "detail": {"kind": "a_kind_of_next_year", "n": [1, 2]},
        "a_member_of_next_year": true
    });
    let unknown_code = json!({"code": "teapot", "message": "short and stout", "extra": 1});
    let no_code = json!({"message": "it says no more", "detail": null});
    let listener = Answering::with(vec![
        json_answer("409 Conflict", &newer.to_string()),
        json_answer("418 I'm a teapot", &unknown_code.to_string()),
        json_answer("404 Not Found", &no_code.to_string()),
    ])
    .await?;
    let mut said = Vec::new();
    for _ in 0..3 {
        let ran = cli(&["get_health", "--server", &listener.origin()], None, b"").await?;
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
        said.push(ran.stderr);
    }
    assert_eq!(
        said,
        [
            "okf-jawn: conflict: the item moved\n  field: path\n  request: req-21\n  \
             detail: {\"kind\":\"a_kind_of_next_year\",\"n\":[1,2]}\n",
            "okf-jawn: internal: short and stout (code: teapot)\n",
            "okf-jawn: not_found: it says no more\n",
        ]
    );
    Ok(())
}

/// A credential that holds a quote and a backslash is removed from a refusal's message and
/// from its detail, which is JSON and spells both with an escape.
/// A token the service's refusal spells with an escape is removed from its message and its
/// detail. (A bearer token is a `b64token`, so it holds no quote or backslash; the slash it may
/// hold is the character JSON lets an encoder escape.)
#[tokio::test]
async fn a_credential_that_needs_escaping_is_removed_from_a_refusal_and_its_detail() -> TestResult {
    let token = "ab/cd+ef-9912-k7m2x";
    let spelled = token.replace('/', "\\/");
    let refusal = format!(
        "{{\"code\":\"unauthenticated\",\"message\":\"Bearer {spelled} is not known\",\
         \"detail\":{{\"seen\":\"{spelled}\"}}}}"
    );
    let listener = Answering::with(vec![json_answer("401 Unauthorized", &refusal)]).await?;
    let ran = cli(
        &["get_session", "--server", &listener.origin()],
        Some(token),
        b"",
    )
    .await?;
    assert_eq!(
        only(&listener)?.header("authorization"),
        Some(format!("Bearer {token}"))
    );
    assert_eq!(
        (ran.code, ran.stderr.as_str()),
        (
            1,
            "okf-jawn: unauthenticated: Bearer [credential] is not known\n  \
             detail: {\"seen\":\"[credential]\"}\n"
        )
    );
    Ok(())
}

/// An answer that carries no `ApiError` is reported with the code its status stands for.
#[tokio::test]
async fn a_refusal_without_an_error_body_is_reported_by_its_status() -> TestResult {
    for (status, code) in [
        ("400 Bad Request", "invalid_input"),
        ("401 Unauthorized", "unauthenticated"),
        ("403 Forbidden", "forbidden"),
        ("404 Not Found", "not_found"),
        ("409 Conflict", "conflict"),
        ("413 Payload Too Large", "too_large"),
        ("422 Unprocessable Entity", "unsupported"),
        ("500 Internal Server Error", "internal"),
        ("501 Not Implemented", "not_implemented"),
        ("503 Service Unavailable", "unavailable"),
    ] {
        let listener = Answering::with(vec![answer(
            status,
            &[("content-type", "text/plain")],
            "no",
        )])
        .await?;
        let ran = cli(&["get_health", "--server", &listener.origin()], None, b"").await?;
        let number = some(status.split(' ').next(), "a status number")?;
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""), "{status}");
        assert!(
            ran.stderr.starts_with(&format!("okf-jawn: {code}: ")) && ran.stderr.contains(number),
            "{status}: {}",
            ran.stderr
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_usage_error_keeps_exit_code_2_and_help_is_success() -> TestResult {
    let cases: [&[&str]; 4] = [
        &["no_such_operation"],
        &[],
        &["list_items", "--token", "x"],
        &["mcp"],
    ];
    for arguments in cases {
        let ran = cli(arguments, Some(TOKEN), b"").await?;
        assert_eq!((ran.code, ran.stdout.as_str()), (2, ""), "{arguments:?}");
        assert!(!ran.stderr.is_empty(), "{arguments:?}");
    }
    let help = cli(&["--help"], None, b"").await?;
    assert_eq!((help.code, help.stderr.as_str()), (0, ""));
    assert!(help.stdout.contains("list_items") && help.stdout.contains("OKF_JAWN_TOKEN"));
    let version = cli(&["--version"], None, b"").await?;
    assert_eq!((version.code, version.stderr.as_str()), (0, ""));
    assert!(version.stdout.starts_with("okf-jawn "));
    Ok(())
}

#[tokio::test]
async fn input_that_is_not_json_is_refused_before_any_request() -> TestResult {
    let listener = Answering::with(Vec::new()).await?;
    let missing = file("missing.json", b"")?;
    std::fs::remove_file(&missing)?;
    for input in ["{", "", &format!("@{}", missing.display())] {
        let ran = cli(
            &[
                "list_items",
                "--server",
                &listener.origin(),
                "--json",
                input,
            ],
            Some(TOKEN),
            b"",
        )
        .await?;
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""), "{input}");
        assert!(
            ran.stderr.starts_with("okf-jawn: invalid_input: "),
            "{}",
            ran.stderr
        );
        assert!(ran.stderr.contains("field: json"), "{}", ran.stderr);
    }
    assert!(listener.heard()?.is_empty());
    Ok(())
}

/// The input is bounded by the contract's request-body limit: exactly the limit is sent, one
/// byte more is refused here.
#[tokio::test]
async fn input_over_the_request_body_limit_is_refused_and_the_limit_itself_is_sent() -> TestResult {
    let listener = Answering::with(vec![json_answer("200 OK", "{}")]).await?;
    let at_limit = format!(
        "\"{}\"",
        "a".repeat(MAX_REQUEST_BODY_BYTES.saturating_sub(2))
    );
    assert_eq!(at_limit.len(), MAX_REQUEST_BODY_BYTES);
    let sent = cli(
        &["get_health", "--server", &listener.origin(), "--json", "-"],
        None,
        at_limit.as_bytes(),
    )
    .await?;
    assert_eq!(sent.code, 0, "{}", sent.stderr);
    assert_eq!(only(&listener)?.body.len(), MAX_REQUEST_BODY_BYTES);
    let over = format!(
        "\"{}\"",
        "a".repeat(MAX_REQUEST_BODY_BYTES.saturating_sub(1))
    );
    let refused = cli(
        &["get_health", "--server", &listener.origin(), "--json", "-"],
        None,
        over.as_bytes(),
    )
    .await?;
    assert_eq!((refused.code, refused.stdout.as_str()), (1, ""));
    assert!(
        refused.stderr.starts_with("okf-jawn: too_large: "),
        "{}",
        refused.stderr
    );
    assert_eq!(
        listener.heard()?.len(),
        1,
        "the oversized input was never sent"
    );
    Ok(())
}

#[tokio::test]
async fn without_a_token_no_authorization_is_sent() -> TestResult {
    let listener = Answering::with(vec![
        json_answer("200 OK", "{}"),
        json_answer("200 OK", "{}"),
    ])
    .await?;
    for token in [None, Some("  ")] {
        let ran = cli(&["get_health", "--server", &listener.origin()], token, b"").await?;
        assert_eq!(ran.code, 0, "{}", ran.stderr);
    }
    for heard in listener.heard()? {
        assert_eq!(heard.header("authorization"), None);
    }
    Ok(())
}

/// A service, or whatever answers in its place, may echo the token; the CLI's own error
/// output never carries it.
#[tokio::test]
async fn the_token_never_appears_in_an_error() -> TestResult {
    let echoed =
        json!({"code": "unauthenticated", "message": format!("Bearer {TOKEN} is not known")});
    let listener = Answering::with(vec![
        json_answer("401 Unauthorized", &echoed.to_string()),
        answer("502 Bad Gateway", &[], TOKEN),
    ])
    .await?;
    for _ in 0..2 {
        let ran = cli(
            &["get_session", "--server", &listener.origin()],
            Some(TOKEN),
            b"",
        )
        .await?;
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
        assert!(!ran.stderr.contains(TOKEN), "{}", ran.stderr);
    }
    let unusable = cli(&["get_session"], Some("bad token\twith blanks"), b"").await?;
    assert_eq!(unusable.code, 1);
    assert!(
        unusable.stderr.starts_with("okf-jawn: invalid_input: "),
        "{}",
        unusable.stderr
    );
    assert!(
        !unusable.stderr.contains("bad token"),
        "{}",
        unusable.stderr
    );
    Ok(())
}

/// An operation's bearer token is an RFC 6750 `b64token` of at least 16 characters (the
/// integration owner's ruling on the follow-up's request 2): letters, digits and `-` `.` `_`
/// `~` `+` `/`, then any number of `=`. So `[` and `]`, from which the mark `[credential]`
/// could complete a token in a refusal the CLI prints, are refused, as are a quote, a
/// backslash, a space, a letter that is not ASCII and an `=` before the end, each in words
/// that do not repeat the token, and nothing is sent. A JWT-shaped token (a WorkOS Connect
/// access token is base64url with dots), one with padding, one with every other character of
/// the alphabet, and a connector secret are each sent as they are.
#[tokio::test]
async fn an_operation_token_is_a_b64token_or_is_refused() -> TestResult {
    let stem = "okfjawn0token1234";
    let refused = [
        format!("{stem}["),
        format!("{stem}]"),
        format!("[{stem}"),
        format!("{stem}\""),
        format!("{stem}\\x"),
        format!("{stem} x"),
        format!("{stem}\u{e9}"),
        format!("{stem}=x"),
        format!("{stem},x"),
        format!("{stem}:x"),
    ];
    let quiet = Answering::with(Vec::new()).await?;
    for token in &refused {
        let ran = cli(
            &["get_session", "--server", &quiet.origin()],
            Some(token),
            b"",
        )
        .await?;
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""), "{token}");
        assert!(
            ran.stderr.starts_with("okf-jawn: invalid_input: The ")
                && ran.stderr.contains("OKF_JAWN_TOKEN")
                && !ran.stderr.contains(stem),
            "{token}: {}",
            ran.stderr
        );
    }
    assert!(quiet.heard()?.is_empty(), "a refused token was sent");
    let jwt = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ1c2VyXzAxIn0.c2lnbmF0dXJlLV9fYmFzZTY0dXJs";
    let issued = format!("okfj_{}", "0123456789abcdef".repeat(4));
    let taken = [
        jwt.to_owned(),
        format!("{stem}=="),
        format!("{stem}-._~+/AZaz09"),
        issued,
    ];
    for token in &taken {
        let listener = Answering::with(vec![json_answer("200 OK", "{}")]).await?;
        let ran = cli(
            &["get_session", "--server", &listener.origin()],
            Some(token),
            b"",
        )
        .await?;
        assert_eq!((ran.code, ran.stderr.as_str()), (0, ""), "{token}");
        assert_eq!(
            only(&listener)?.header("authorization"),
            Some(format!("Bearer {token}"))
        );
    }
    Ok(())
}

#[tokio::test]
async fn the_refused_token_message_is_redacted_not_dropped() -> TestResult {
    let echoed =
        json!({"code": "unauthenticated", "message": format!("Bearer {TOKEN} is not known")});
    let listener =
        Answering::with(vec![json_answer("401 Unauthorized", &echoed.to_string())]).await?;
    let ran = cli(
        &["get_session", "--server", &listener.origin()],
        Some(TOKEN),
        b"",
    )
    .await?;
    assert_eq!(
        ran.stderr,
        "okf-jawn: unauthenticated: Bearer [credential] is not known\n"
    );
    Ok(())
}

/// A remote address requires HTTPS; plain HTTP is for a loopback host only. Each spelling is
/// its own case.
#[test]
fn a_remote_address_requires_https_and_loopback_may_use_http() -> TestResult {
    for refused in [
        "http://example.org",
        "http://example.org:7711",
        "HTTP://EXAMPLE.ORG:7711",
        "http://192.168.1.10:7711",
        "http://0.0.0.0:7711",
        "http://[::ffff:127.0.0.1]:7711",
        "http://[::]:7711",
        "http://localhost.example.org:7711",
        "http://127.0.0.1.example.org:7711",
        "ftp://127.0.0.1:7711",
        "ws://127.0.0.1:7711",
        "127.0.0.1:7711",
        "localhost",
    ] {
        let error = err_of(Endpoint::parse(refused))?;
        assert_eq!(
            serde_json::to_value(&error.code)?,
            json!("invalid_input"),
            "{refused}"
        );
    }
    for (accepted, loopback) in [
        ("http://127.0.0.1:7711", true),
        ("http://127.0.0.2:7711", true),
        ("http://localhost:7711", true),
        ("HTTP://LOCALHOST:7711/", true),
        ("http://[::1]:7711", true),
        ("https://127.0.0.1:7711", true),
        ("https://example.org", false),
        ("https://example.org:8443/", false),
    ] {
        let endpoint = Endpoint::parse(accepted)?;
        assert_eq!(endpoint.is_loopback(), loopback, "{accepted}");
    }
    Ok(())
}

#[tokio::test]
async fn a_remote_http_address_sends_nothing_and_exits_1() -> TestResult {
    let ran = cli(
        &["get_health", "--server", "http://okf-jawn.invalid:7711"],
        Some(TOKEN),
        b"",
    )
    .await?;
    assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
    assert!(
        ran.stderr
            .starts_with("okf-jawn: invalid_input: A remote server address requires HTTPS"),
        "{}",
        ran.stderr
    );
    Ok(())
}

/// An address with credentials is refused before any request, and the refusal repeats
/// neither the user name nor the password.
#[tokio::test]
async fn an_address_with_credentials_is_refused_and_not_repeated() -> TestResult {
    let listener = Answering::with(Vec::new()).await?;
    let port = listener.address.port();
    for address in [
        format!("http://owner:hunter2@127.0.0.1:{port}"),
        format!("http://owner@127.0.0.1:{port}"),
        format!("http://:hunter2@127.0.0.1:{port}"),
        "https://owner:hunter2@okf-jawn.invalid".to_owned(),
    ] {
        let ran = cli(&["get_health", "--server", &address], Some(TOKEN), b"").await?;
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""), "{address}");
        assert!(
            ran.stderr.starts_with("okf-jawn: invalid_input: "),
            "{}",
            ran.stderr
        );
        assert!(ran.stderr.contains("credentials"), "{}", ran.stderr);
        assert!(
            !ran.stderr.contains("hunter2") && !ran.stderr.contains("owner"),
            "{}",
            ran.stderr
        );
    }
    assert!(listener.heard()?.is_empty());
    Ok(())
}

#[test]
fn an_address_with_a_path_a_query_or_a_fragment_is_refused() -> TestResult {
    for address in [
        "http://127.0.0.1:7711/api",
        "http://127.0.0.1:7711/?workspace=w",
        "http://127.0.0.1:7711/#top",
        "https://example.org/okf/",
    ] {
        let error = err_of(Endpoint::parse(address))?;
        assert!(
            error.message.contains("origin"),
            "{address}: {}",
            error.message
        );
    }
    Ok(())
}

/// A redirect is reported and never followed: the address it names hears nothing, so the
/// token cannot travel there.
#[tokio::test]
async fn a_redirect_is_never_followed() -> TestResult {
    let elsewhere = Answering::with(vec![json_answer("200 OK", r#"{"followed":true}"#)]).await?;
    for status in [
        "301 Moved Permanently",
        "302 Found",
        "307 Temporary Redirect",
        "308 Permanent Redirect",
    ] {
        let target = format!("{}/api/health/get-health", elsewhere.origin());
        let listener = Answering::with(vec![answer(status, &[("location", &target)], "")]).await?;
        let ran = cli(
            &["get_health", "--server", &listener.origin()],
            Some(TOKEN),
            b"",
        )
        .await?;
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""), "{status}");
        assert!(
            ran.stderr.contains("redirect") && ran.stderr.contains("never followed"),
            "{}",
            ran.stderr
        );
        assert_eq!(listener.heard()?.len(), 1);
    }
    assert!(elsewhere.heard()?.is_empty(), "a redirect was followed");
    Ok(())
}

#[tokio::test]
async fn a_service_that_is_not_running_is_reported_as_unavailable() -> TestResult {
    let nowhere = closed().await?;
    let ran = cli(
        &["get_health", "--server", &format!("http://{nowhere}")],
        None,
        b"",
    )
    .await?;
    assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
    assert!(
        ran.stderr.starts_with("okf-jawn: unavailable: "),
        "{}",
        ran.stderr
    );
    Ok(())
}

/// The built executable: a usage error exits 2 and writes nothing to stdout.
#[test]
fn the_executable_exits_2_on_a_usage_error_and_0_on_help() -> TestResult {
    let usage = Command::new(EXECUTABLE)
        .arg("no_such_operation")
        .stdin(Stdio::null())
        .output()?;
    assert_eq!(usage.status.code(), Some(2));
    assert!(usage.stdout.is_empty() && !usage.stderr.is_empty());
    let help = Command::new(EXECUTABLE)
        .arg("--help")
        .stdin(Stdio::null())
        .output()?;
    assert_eq!(help.status.code(), Some(0));
    assert!(help.stderr.is_empty() && !help.stdout.is_empty());
    Ok(())
}

/// The built executable sends a loopback request straight to the service even when the
/// environment configures a proxy, so the token never reaches the proxy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_loopback_request_ignores_a_configured_proxy() -> TestResult {
    let proxy = Answering::with(vec![json_answer("200 OK", r#"{"from":"proxy"}"#)]).await?;
    let listener = Answering::with(vec![json_answer("200 OK", r#"{"from":"service"}"#)]).await?;
    let (server, through) = (listener.origin(), proxy.origin());
    let output = tokio::task::spawn_blocking(move || {
        Command::new(EXECUTABLE)
            .args(["get_health", "--server", &server])
            .env("OKF_JAWN_TOKEN", TOKEN)
            .env("HTTP_PROXY", &through)
            .env("http_proxy", &through)
            .env("ALL_PROXY", &through)
            .env("all_proxy", &through)
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .stdin(Stdio::null())
            .output()
    })
    .await??;
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)?,
        "{\"from\":\"service\"}\n"
    );
    assert!(
        proxy.heard()?.is_empty(),
        "the request went through the proxy"
    );
    assert_eq!(
        only(&listener)?.header("authorization"),
        Some(format!("Bearer {TOKEN}"))
    );
    Ok(())
}
