//! Ruling 3, the connector credential: read from the environment or the first line of a
//! file, never an argument, never logged, never on stdout; with none, the relay says so and
//! exits 1 before it reads stdin or asks the service anything.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use okf_jawn_cli::Credential;
use okf_jawn_cli::relay::Body;
use serde_json::{Value, json};

use super::{notification, request};
use crate::check::{TestResult, err_of, some};
use crate::support::doubles::{FakeTransport, Setup, broken, relay, status};
use crate::support::http::{Answering, json as json_answer};
use crate::support::{TOKEN, Untouched, cli, cli_reading, file};

/// A liveness answer, then one accepted message.
fn service() -> Vec<String> {
    vec![json_answer("200 OK", "{}"), json_answer("202 Accepted", "")]
}

#[tokio::test]
async fn without_a_credential_the_relay_exits_1_before_reading_stdin() -> TestResult {
    let listener = Answering::with(service()).await?;
    for token in [None, Some(""), Some(" \t ")] {
        let read = Arc::new(AtomicBool::new(false));
        let ran = cli_reading(
            &["mcp", "--stdio", "--server", &listener.origin()],
            token,
            Untouched(read.clone()),
        )
        .await?;
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
        assert_eq!(
            ran.stderr,
            "okf-jawn: unauthenticated: No connector credential: set OKF_JAWN_TOKEN, or name a \
             file with --token-file\n"
        );
        assert!(!read.load(Ordering::SeqCst), "stdin was read");
    }
    assert!(
        listener.heard()?.is_empty(),
        "the service was asked before the credential was known"
    );
    Ok(())
}

#[tokio::test]
async fn the_credential_comes_from_the_environment() -> TestResult {
    let listener = Answering::with(service()).await?;
    let ran = cli(
        &["mcp", "--stdio", "--server", &listener.origin()],
        Some(TOKEN),
        notification("notifications/initialized").as_bytes(),
    )
    .await?;
    assert_eq!(
        (ran.code, ran.stdout.as_str(), ran.stderr.as_str()),
        (0, "", "")
    );
    let heard = listener.heard()?;
    let sent = some(heard.get(1), "the POST")?;
    assert_eq!(
        sent.header("authorization"),
        Some(format!("Bearer {TOKEN}"))
    );
    Ok(())
}

/// The first line of the file is the credential, and a named file is preferred to the
/// environment.
#[tokio::test]
async fn the_credential_comes_from_the_first_line_of_the_token_file() -> TestResult {
    let listener = Answering::with(service()).await?;
    let path = file("token", b"  file-token-81c4\r\nnot-the-credential\n")?;
    let ran = cli(
        &[
            "mcp",
            "--stdio",
            "--server",
            &listener.origin(),
            "--token-file",
            &path.display().to_string(),
        ],
        Some(TOKEN),
        notification("notifications/initialized").as_bytes(),
    )
    .await?;
    assert_eq!(
        (ran.code, ran.stdout.as_str(), ran.stderr.as_str()),
        (0, "", "")
    );
    let heard = listener.heard()?;
    let sent = some(heard.get(1), "the POST")?;
    assert_eq!(
        sent.header("authorization").as_deref(),
        Some("Bearer file-token-81c4")
    );
    std::fs::remove_file(path)?;
    Ok(())
}

/// A credential that cannot be used is refused without being repeated, and nothing is sent.
#[tokio::test]
async fn an_unusable_credential_is_refused_and_not_repeated() -> TestResult {
    let listener = Answering::with(service()).await?;
    let empty = file("empty-token", b"\nsecond-line-secret\n")?;
    let spaced = file("spaced-token", b"two words\n")?;
    let missing = file("missing-token", b"")?;
    std::fs::remove_file(&missing)?;
    for path in [&empty, &spaced, &missing] {
        let ran = cli(
            &[
                "mcp",
                "--stdio",
                "--server",
                &listener.origin(),
                "--token-file",
                &path.display().to_string(),
            ],
            None,
            b"",
        )
        .await?;
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
        assert!(
            ran.stderr.starts_with("okf-jawn: invalid_input: "),
            "{}",
            ran.stderr
        );
        assert!(!ran.stderr.contains("two words") && !ran.stderr.contains("second-line-secret"));
    }
    let from_environment = cli(
        &["mcp", "--stdio", "--server", &listener.origin()],
        Some("caf\u{e9}-token"),
        b"",
    )
    .await?;
    assert_eq!(from_environment.code, 1);
    assert!(
        !from_environment.stderr.contains("caf"),
        "{}",
        from_environment.stderr
    );
    assert!(listener.heard()?.is_empty());
    std::fs::remove_file(empty)?;
    std::fs::remove_file(spaced)?;
    Ok(())
}

#[tokio::test]
async fn the_credential_is_not_an_argument() -> TestResult {
    for arguments in [
        ["mcp", "--stdio", "--token", TOKEN],
        ["mcp", "--stdio", "--credential", TOKEN],
        ["mcp", "--stdio", "--bearer", TOKEN],
    ] {
        let ran = cli(&arguments, None, b"").await?;
        assert_eq!((ran.code, ran.stdout.as_str()), (2, ""), "{arguments:?}");
    }
    Ok(())
}

#[test]
fn mcp_stdio_credential_has_no_printable_form() -> TestResult {
    let credential = some(Credential::parse(TOKEN, "the test")?, "a credential")?;
    assert_eq!(format!("{credential:?}"), "Credential([credential])");
    assert_eq!(
        credential.redact(&format!("Bearer {TOKEN}, again {TOKEN}")),
        "Bearer [credential], again [credential]"
    );
    let refused = err_of(Credential::parse("bad\u{7f}secret", "OKF_JAWN_TOKEN"))?;
    assert!(!refused.message.contains("secret"), "{}", refused.message);
    assert_eq!(Credential::parse(" \n", "OKF_JAWN_TOKEN")?, None);
    Ok(())
}

/// The service, or whatever answers in its place, may repeat the credential in a refusal or a
/// failure; neither stdout nor stderr ever carries it.
#[tokio::test]
async fn the_credential_never_reaches_stdout_or_stderr() -> TestResult {
    let transport = FakeTransport::scripted(|message| {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match method {
            "tools/list" => status(
                500,
                Body::Json(
                    json!({"code": "internal", "message": format!("saw Bearer {TOKEN}")})
                        .to_string(),
                ),
            ),
            "tools/call" => broken(&format!(
                "header Authorization: Bearer {TOKEN} was rejected"
            )),
            _ => broken(&format!("a notification with {TOKEN} was lost")),
        }
    });
    let input = format!(
        "{}{}{}",
        request(&json!(1), "tools/list", &json!({})),
        request(&json!(2), "tools/call", &json!({})),
        notification("notifications/progress")
    );
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!(ran.code, 0);
    assert!(!ran.stdout.contains(TOKEN), "{}", ran.stdout);
    assert!(!ran.stderr.contains(TOKEN), "{}", ran.stderr);
    assert_eq!(
        ran.stdout.matches("[credential]").count(),
        2,
        "{}",
        ran.stdout
    );
    assert_eq!(
        ran.stderr.matches("[credential]").count(),
        1,
        "{}",
        ran.stderr
    );
    Ok(())
}
