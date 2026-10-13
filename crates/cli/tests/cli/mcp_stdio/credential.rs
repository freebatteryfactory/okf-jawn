//! Ruling 3, the connector credential: read from the environment or the first line of a
//! file, never an argument, never logged, never on stdout; with none, the relay says so and
//! exits 1 before it reads stdin or asks the service anything.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use okf_jawn_cli::Credential;
use okf_jawn_cli::relay::Body;
use serde_json::{Value, json};

use super::{notification, request, response};
use crate::check::{TestResult, err_of, some};
use crate::support::doubles::{
    FakeTransport, Script, Setup, accepted, answered, broken, relay, status,
};
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

/// A line the service wrote is cleaned like the relay's own: a refusal, a result and an event
/// that repeat the credential reach the client without it, and nothing else of them changes.
#[tokio::test]
async fn a_line_the_service_wrote_never_carries_the_credential() -> TestResult {
    let refused = |text: &str| json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32602, "message": format!("saw Bearer {text}")}});
    let progress = |text: &str| json!({"jsonrpc": "2.0", "method": "notifications/message", "params": {"data": text}});
    let transport = FakeTransport::scripted(move |message| match message.get("id") {
        Some(id) if id == &json!(1) => status(400, Body::Json(refused(TOKEN).to_string())),
        Some(id) => Script {
            events: vec![
                progress(TOKEN).to_string(),
                response(id, &json!({"echo": TOKEN, TOKEN: [TOKEN]})).to_string(),
            ],
            ..status(200, Body::Events)
        },
        None => accepted(),
    });
    let input = format!(
        "{}{}",
        request(&json!(1), "tools/list", &json!({})),
        request(&json!(2), "tools/call", &json!({}))
    );
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    assert!(!ran.stdout.contains(TOKEN), "{}", ran.stdout);
    let clean = "[credential]";
    let mut lines = ran.lines()?;
    lines.sort_by_key(Value::to_string);
    let mut expected = vec![
        refused(clean),
        progress(clean),
        response(&json!(2), &json!({"echo": clean, clean: [clean]})),
    ];
    expected.sort_by_key(Value::to_string);
    assert_eq!(lines, expected);
    Ok(())
}

/// A credential may hold a quote or a backslash, which a JSON line spells with an escape, and
/// JSON has more than one spelling for a character: each is found, in a value and in a key,
/// and only the bytes that spell the credential change.
#[test]
fn mcp_stdio_credential_is_found_under_every_json_spelling() -> TestResult {
    let awkward = "ab\"cd\\ef/9912";
    let credential = some(Credential::parse(awkward, "the test")?, "a credential")?;
    // A `\u` escape of one UTF-16 code unit, with its hexadecimal digits in either case.
    let unit = |code: u32, upper: bool| {
        if upper {
            format!("{}u{code:04X}", '\\')
        } else {
            format!("{}u{code:04x}", '\\')
        }
    };
    let all_escaped: String = awkward
        .chars()
        .enumerate()
        .map(|(at, character)| unit(u32::from(character), at.is_multiple_of(2)))
        .collect();
    // A surrogate pair and a Latin-1 letter: escapes that stand for no character of a credential.
    let (pair, letter) = (
        format!("{}{}", unit(0xD83D, false), unit(0xDE00, false)),
        unit(0xE9, false),
    );
    let cases = [
        // As serde writes it.
        (
            r#"{"a":"x ab\"cd\\ef/9912 y"}"#.to_owned(),
            r#"{"a":"x [credential] y"}"#.to_owned(),
        ),
        // The slash escaped, as other encoders write it.
        (
            r#"{"a":"ab\"cd\\ef\/9912"}"#.to_owned(),
            r#"{"a":"[credential]"}"#.to_owned(),
        ),
        // Every character as a `\u` escape.
        (
            format!("[\"{all_escaped}\"]"),
            r#"["[credential]"]"#.to_owned(),
        ),
        // A key, twice in one string, and beside escapes that are not part of it.
        (
            format!(r#"{{"ab\"cd\\ef/9912":"\n{pair}ab\"cd\\ef/9912{letter}ab\"cd\\ef\/9912\t"}}"#),
            format!(r#"{{"[credential]":"\n{pair}[credential]{letter}[credential]\t"}}"#),
        ),
        // A start that is taken back: the search resumes inside what it had matched.
        (
            r#"{"a":"abab\"cd\\ef/9912"}"#.to_owned(),
            r#"{"a":"ab[credential]"}"#.to_owned(),
        ),
        // Nothing to find: the line comes back as it went in, white space included.
        (
            r#"{ "a" : "ab\"cd\\ef/991" , "b":1e400}"#.to_owned(),
            r#"{ "a" : "ab\"cd\\ef/991" , "b":1e400}"#.to_owned(),
        ),
    ];
    assert!(all_escaped.len() == awkward.len().saturating_mul(6) && !all_escaped.contains("9912"));
    for (line, clean) in cases {
        assert_eq!(credential.redact_line(line.clone()), Some(clean), "{line}");
    }
    // Plain text the CLI composes: the credential as it stands, and as a string literal spells it.
    assert_eq!(
        credential.redact(&format!("saw {awkward} and {awkward:?}")),
        "saw [credential] and \"[credential]\""
    );
    Ok(())
}

/// A credential spelled outside a JSON string (a number, or text that runs across two strings)
/// cannot be replaced and leave JSON: the line is refused whole.
#[test]
fn mcp_stdio_credential_outside_a_string_refuses_the_line() -> TestResult {
    let digits = some(Credential::parse("8675309112", "the test")?, "a credential")?;
    assert_eq!(digits.redact_line(r#"{"n":8675309112}"#.to_owned()), None);
    assert_eq!(digits.redact_line(r#"{"n":18675309112}"#.to_owned()), None);
    assert_eq!(
        digits
            .redact_line(r#"{"n":"8675309112"}"#.to_owned())
            .as_deref(),
        Some(r#"{"n":"[credential]"}"#)
    );
    let across = some(Credential::parse("a\",\"b", "the test")?, "a credential")?;
    assert_eq!(across.redact_line(r#"["a","b"]"#.to_owned()), None);
    assert_eq!(
        across.redact_line(r#"["a\",\"b"]"#.to_owned()).as_deref(),
        Some(r#"["[credential]"]"#)
    );
    Ok(())
}

/// The relay holds a credential of digits and the service repeats it as a number: the answer
/// is withheld, stderr says so, and the request still gets one error with its id. An event
/// that does the same is withheld too, and the events after it are still passed on.
#[tokio::test]
async fn a_line_that_cannot_be_cleaned_is_withheld_and_the_request_is_still_answered() -> TestResult
{
    const DIGITS: &str = "8675309112";
    let harmless =
        json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"n": 1}});
    for (streamed, refused) in [(false, false), (true, false), (false, true)] {
        let passed = harmless.clone();
        let transport = FakeTransport::scripted(move |message| {
            let leaking = json!({"jsonrpc": "2.0", "id": message.get("id"), "result": {"n": 8_675_309_112_u64}});
            let leaking_error =
                json!({"jsonrpc": "2.0", "error": {"code": 8_675_309_112_u64, "message": "no"}});
            if streamed {
                Script {
                    events: vec![
                        json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"n": 8_675_309_112_u64}}).to_string(),
                        passed.to_string(),
                        leaking.to_string(),
                    ],
                    ..status(200, Body::Events)
                }
            } else if refused {
                status(400, Body::Json(leaking_error.to_string()))
            } else {
                answered(&leaking)
            }
        });
        let setup = Setup {
            token: DIGITS,
            ..Setup::new(&transport)
        };
        let ran = relay(
            setup,
            request(&json!(7), "tools/call", &json!({})).as_bytes(),
        )
        .await?;
        assert_eq!(ran.code, 0);
        assert!(!ran.stdout.contains(DIGITS), "{}", ran.stdout);
        assert!(!ran.stderr.contains(DIGITS), "{}", ran.stderr);
        let lines = ran.lines()?;
        let last = some(lines.last(), "an answer")?;
        assert_eq!(
            (last.get("id"), last.pointer("/error/code")),
            (Some(&json!(7)), Some(&json!(-32603)))
        );
        let expected = if streamed {
            vec![&harmless, last]
        } else {
            vec![last]
        };
        assert_eq!(lines.iter().collect::<Vec<_>>(), expected, "{}", ran.stdout);
        assert_eq!(
            ran.stderr
                .matches("withheld a line that repeats the connector credential")
                .count(),
            if streamed { 2 } else { 1 },
            "{}",
            ran.stderr
        );
    }
    Ok(())
}

/// A failure the relay words itself is cleaned in the same place, also when the credential
/// holds a quote and a backslash, which the relay's own JSON encoder escapes.
#[tokio::test]
async fn the_relay_s_own_lines_are_cleaned_when_the_credential_needs_escaping() -> TestResult {
    const AWKWARD: &str = "ab\"cd\\ef-9912";
    let transport = FakeTransport::scripted(|message| match message.get("id") {
        Some(_) => broken(&format!(
            "header Authorization: Bearer {AWKWARD} was rejected"
        )),
        None => broken(&format!("a notification with {AWKWARD} was lost")),
    });
    let setup = Setup {
        token: AWKWARD,
        ..Setup::new(&transport)
    };
    let input = format!(
        "{}{}",
        request(&json!(1), "tools/call", &json!({})),
        notification("notifications/progress")
    );
    let ran = relay(setup, input.as_bytes()).await?;
    assert_eq!(ran.code, 0);
    for written in [&ran.stdout, &ran.stderr] {
        assert!(!written.contains("ef-9912"), "{written}");
        assert_eq!(written.matches("[credential]").count(), 1, "{written}");
    }
    Ok(())
}
