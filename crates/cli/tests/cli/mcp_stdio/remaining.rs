//! The remaining tests the review asked for (its notes N11 and N12): rules whose removal
//! every earlier test survived, each with a smaller outcome than the blocking findings.
//!
//! The relay command needs `--stdio`; nothing printable holds the credential; a refusal for
//! another request's id does not count as this request's answer; a failed input stream is an
//! exit code of 1; a notification's `_meta` sets no protocol version; a name with a trailing
//! blank and an empty `_meta` version are refused before anything is sent; and an operation's
//! input from standard input is read up to the limit and one byte, not whole.

use std::pin::Pin;
use std::task::{Context, Poll};

use okf_jawn_cli::relay::{Body, HttpTransport};
use okf_jawn_cli::{Credential, Endpoint, RELAY_COMMAND, command};
use okf_jawn_contract::transport::MAX_REQUEST_BODY_BYTES;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

use super::request;
use crate::check::{TestResult, err_of, some};
use crate::support::doubles::{FakeTransport, Setup, relay, relay_over, status};
use crate::support::{TOKEN, cli_reading};

/// An input stream whose every read fails.
struct Failing;

impl AsyncRead for Failing {
    fn poll_read(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::ErrorKind::ConnectionReset.into()))
    }
}

/// The request `_meta` that names a per-request protocol version.
fn meta(version: &str) -> Value {
    json!({"io.modelcontextprotocol/protocolVersion": version})
}

/// N11: the relay speaks over exactly one transport, and it must be named.
#[test]
fn mcp_stdio_relay_command_requires_the_stdio_flag() -> TestResult {
    let refused = err_of(command().try_get_matches_from(["okf-jawn", RELAY_COMMAND]))?;
    assert_eq!(refused.exit_code(), 2, "{refused}");
    let matches = command().try_get_matches_from(["okf-jawn", RELAY_COMMAND, "--stdio"])?;
    assert_eq!(matches.subcommand_name(), Some(RELAY_COMMAND));
    Ok(())
}

/// R9: the production transport holds the credential as a header value marked sensitive, so
/// neither the transport nor the header prints it.
#[test]
fn mcp_stdio_transport_and_its_header_print_no_credential() -> TestResult {
    let credential = some(Credential::parse(TOKEN, "the test")?, "a credential")?;
    let transport =
        HttpTransport::new(&Endpoint::parse("http://127.0.0.1:7711")?, &credential, 64)?;
    let header = credential.bearer()?;
    assert!(header.is_sensitive());
    let printed = format!("{transport:?} {header:?} {credential:?}");
    assert!(!printed.contains(TOKEN), "{printed}");
    Ok(())
}

/// R13: a refusal the service sends for another request's id is passed on, and it is not this
/// request's answer: the request still gets one error with its own id.
#[tokio::test]
async fn a_refusal_for_another_id_still_leaves_the_request_answered() -> TestResult {
    let other = json!({"jsonrpc": "2.0", "id": 99, "error": {"code": -32602, "message": "other"}});
    let sent = other.clone();
    let transport = FakeTransport::scripted(move |_| status(400, Body::Json(sent.to_string())));
    let ran = relay(
        Setup::new(&transport),
        request(&json!(12), "tools/list", &json!({})).as_bytes(),
    )
    .await?;
    let lines = ran.lines()?;
    assert_eq!(lines.len(), 2, "{}", ran.stdout);
    assert_eq!(lines.first(), Some(&other));
    let own = some(lines.get(1), "the request's own answer")?;
    assert_eq!(
        (own.get("id"), own.pointer("/error/code")),
        (Some(&json!(12)), Some(&json!(-32603)))
    );
    Ok(())
}

/// R14: an input stream that fails is not an input that ended: the relay says so and
/// returns 1.
#[tokio::test]
async fn a_failed_input_stream_is_exit_code_1() -> TestResult {
    let transport = FakeTransport::echoing();
    let mut stdout = Vec::new();
    let (code, stderr) = relay_over(Setup::new(&transport), Failing, &mut stdout).await?;
    assert_eq!((code, stdout.len()), (1, 0));
    assert!(stderr.contains("the input stream failed"), "{stderr}");
    assert_eq!(transport.posts()?, Vec::new());
    Ok(())
}

/// R16: only a request's `_meta` names a protocol version. One in a notification sets
/// nothing, for the notification itself and for the messages after it.
#[tokio::test]
async fn a_notification_meta_version_sets_nothing() -> TestResult {
    let transport = FakeTransport::echoing();
    let input = format!(
        "{}\n{}",
        json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"_meta": meta("2026-07-28")}}),
        request(&json!(1), "ping", &json!({}))
    );
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!((ran.code, ran.lines()?.len()), (0, 1));
    let posts = transport.posts()?;
    assert_eq!(posts.len(), 2);
    for post in posts {
        assert_eq!(post.headers, Vec::new());
    }
    Ok(())
}

/// R17 and R18: a name that ends in a blank cannot travel in a header as it is, and an empty
/// `_meta` protocol version is no version. Each request is refused with its id and never
/// sent; the same name without the blank, and a real version, are sent.
#[tokio::test]
async fn a_name_with_a_trailing_blank_and_an_empty_meta_version_are_refused() -> TestResult {
    let refused = [
        request(
            &json!(1),
            "tools/call",
            &json!({"name": "padded ", "_meta": meta("2026-07-28")}),
        ),
        request(
            &json!(1),
            "tools/call",
            &json!({"name": "\tpadded", "_meta": meta("2026-07-28")}),
        ),
        request(&json!(1), "tools/list", &json!({"_meta": meta("")})),
    ];
    for line in refused {
        let transport = FakeTransport::echoing();
        let ran = relay(Setup::new(&transport), line.as_bytes()).await?;
        assert_eq!(transport.posts()?, Vec::new(), "{line}");
        let lines = ran.lines()?;
        let only = some(lines.first(), "an answer")?;
        assert_eq!(
            (lines.len(), only.get("id"), only.pointer("/error/code")),
            (1, Some(&json!(1)), Some(&json!(-32600))),
            "{line}"
        );
    }
    let transport = FakeTransport::echoing();
    let sent = request(
        &json!(1),
        "tools/call",
        &json!({"name": "padded", "_meta": meta("2026-07-28")}),
    );
    relay(Setup::new(&transport), sent.as_bytes()).await?;
    assert_eq!(transport.posts()?.len(), 1);
    Ok(())
}

/// R20: an operation's input from standard input is read up to the request-body limit and
/// one byte more, which is what tells it is too long; the rest is never read. Of 64 MiB
/// offered, the limit and one byte are taken.
#[tokio::test]
async fn input_from_stdin_is_read_up_to_the_limit_and_one_byte() -> TestResult {
    const OFFERED: u64 = 67_108_864;
    let mut stdin = tokio::io::repeat(b' ').take(OFFERED);
    let ran = cli_reading(
        &[
            "create_item",
            "--server",
            "http://127.0.0.1:1",
            "--json",
            "-",
        ],
        None,
        &mut stdin,
    )
    .await?;
    assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
    assert!(
        ran.stderr.starts_with("okf-jawn: too_large: "),
        "{}",
        ran.stderr
    );
    assert_eq!(
        OFFERED.saturating_sub(stdin.limit()),
        u64::try_from(MAX_REQUEST_BODY_BYTES)?.saturating_add(1)
    );
    Ok(())
}
