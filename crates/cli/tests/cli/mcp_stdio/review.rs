//! The rules the independent review found without a test (its findings B1 and B3 to B7), each
//! through the shipped entry point or the built executable where only that shows the rule.
//!
//! A remote address is never started; what a started server prints reaches no stream of the
//! relay; one liveness check is bounded; the process ends when its output fails while its
//! input stays open; an answer that was written and then cut off is not answered twice; and
//! the bounds of the shipped relay and CLI are the stated ones, each from both sides.

use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, UdpSocket};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use okf_jawn_cli::relay::Limits;
use okf_jawn_cli::{Credential, Endpoint};
use okf_jawn_contract::transport::MAX_REQUEST_BODY_BYTES;
use serde_json::json;

use super::{notification, request, response};
use crate::check::{TestResult, err_of, some};
use crate::support::doubles::{FakeTransport, Setup, broken, relay};
use crate::support::http::{Answering, json as json_answer};
use crate::support::{
    EXECUTABLE, Outcome, STAND_IN_SAYS, StandIn, TOKEN, Untouched, cli, cli_reading, file,
};

/// How long a test waits for what a correct relay does within a few seconds.
const PATIENCE: Duration = Duration::from_secs(30);
/// The test of a remote address, as the test run names it.
const REMOTE_TEST: &str =
    "mcp_stdio::review::a_remote_address_is_never_started_by_the_shipped_entry_point";

/// A loopback port on which nothing listens: one that was free a moment ago.
fn closed_port() -> Outcome<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// An address of this machine that is not loopback: the one it would send from. No packet is
/// sent to find it, and a connection to a closed port there is refused by this machine.
///
/// `None` when the operating system will not connect a socket toward a remote address, which
/// is what it does on a machine that has no address but loopback (a build sandbox, a runner
/// without a network): it has no route to send by. Whatever its reason, a machine that
/// refuses here has no address this test could use.
fn own_address() -> Outcome<Option<IpAddr>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    if socket.connect("192.0.2.1:9").is_err() {
        return Ok(None);
    }
    Ok(Some(socket.local_addr()?.ip()))
}

/// B1 for the address `address` of this machine. With no address there is nothing to run:
/// that is said on `said`, in one line that names the test, and it is no failure.
async fn remote_case(address: Option<IpAddr>, said: &mut impl Write) -> TestResult {
    let Some(address) = address else {
        writeln!(
            said,
            "{REMOTE_TEST} did not run its remote-address case: this machine has no address \
             but loopback, so the relay's refusal to start a server for a remote address was \
             not tested here"
        )?;
        return Ok(());
    };
    assert!(
        !address.is_loopback() && !address.is_unspecified(),
        "{address}"
    );
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-such-okf-jawn-server");
    let read = Arc::new(AtomicBool::new(false));
    let ran = cli_reading(
        &[
            "mcp",
            "--stdio",
            "--server",
            &format!("https://{address}:{}", closed_port()?),
            "--server-bin",
            &missing.display().to_string(),
        ],
        Some(TOKEN),
        Untouched(read.clone()),
    )
    .await?;
    assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
    assert!(
        ran.stderr.contains("never starts a remote service"),
        "{}",
        ran.stderr
    );
    assert!(!read.load(Ordering::SeqCst), "stdin was read");
    Ok(())
}

/// Start the built relay as a process of its own, its three standard streams piped.
fn relay_process(server: &str, server_bin: Option<&Path>) -> Outcome<Child> {
    let mut command = Command::new(EXECUTABLE);
    command.args(["mcp", "--stdio", "--server", server]);
    if let Some(program) = server_bin {
        command.arg("--server-bin").arg(program);
    }
    Ok(command
        .env("OKF_JAWN_TOKEN", TOKEN)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?)
}

/// Stop a relay process this test started and collect what it wrote.
fn stopped(mut child: Child) -> Outcome<(String, String)> {
    // The process may have ended already; then there is nothing to stop.
    let _ = child.kill();
    child.wait()?;
    let (mut out, mut err) = (String::new(), String::new());
    some(child.stdout.take(), "stdout")?.read_to_string(&mut out)?;
    some(child.stderr.take(), "stderr")?.read_to_string(&mut err)?;
    Ok((out, err))
}

/// B1: the shipped entry point never starts a server for a remote address. The address is
/// this machine's own, on a closed port, so the connection is refused without a network; the
/// server executable named does not exist, so a start would be reported as such.
///
/// A machine with no address but loopback cannot run this: the test then says so and passes.
/// It writes to the error stream itself, which the test runner does not capture as it does
/// what a test prints, so the line shows in a run that passes.
#[tokio::test]
async fn a_remote_address_is_never_started_by_the_shipped_entry_point() -> TestResult {
    remote_case(own_address()?, &mut std::io::stderr()).await
}

/// On a machine with no address but loopback, the test above says in one line, by its name,
/// that it did not run its remote-address case, and does not fail. It never passes in
/// silence: this is the same code, given no address.
#[tokio::test]
async fn a_machine_with_no_remote_address_is_told_that_the_remote_case_did_not_run() -> TestResult {
    let mut said = Vec::new();
    remote_case(None, &mut said).await?;
    let said = String::from_utf8(said)?;
    assert_eq!(said.lines().count(), 1, "{said}");
    assert!(
        said.starts_with(
            "mcp_stdio::review::a_remote_address_is_never_started_by_the_shipped_entry_point \
             did not run its remote-address case: "
        ) && said.ends_with('\n'),
        "{said}"
    );
    Ok(())
}

/// N3: loopback means this computer. A plain-HTTP request to `localhost`, however the name
/// is cased, is sent to the loopback address itself, so no resolver decides where the
/// credential goes: the listener hears `127.0.0.1` as the host on the relay's liveness check,
/// on its message and on an operation. Over HTTPS the name is kept.
#[tokio::test]
async fn a_plain_http_request_to_localhost_goes_to_the_loopback_address() -> TestResult {
    let listener = Answering::routed(|heard| {
        if heard.line().starts_with("GET ") {
            json_answer("200 OK", "{}")
        } else {
            json_answer("202 Accepted", "")
        }
    })
    .await?;
    let port = listener.address.port();
    let named = format!("http://LocalHost:{port}");
    let relayed = cli(
        &["mcp", "--stdio", "--server", &named],
        Some(TOKEN),
        notification("notifications/initialized").as_bytes(),
    )
    .await?;
    assert_eq!((relayed.code, relayed.stderr.as_str()), (0, ""));
    let operated = cli(&["get_health", "--server", &named], Some(TOKEN), b"").await?;
    assert_eq!((operated.code, operated.stderr.as_str()), (0, ""));
    let heard = listener.heard()?;
    assert_eq!(heard.len(), 3);
    for request in &heard {
        assert_eq!(
            request.header("host"),
            Some(format!("127.0.0.1:{port}")),
            "{}",
            request.line()
        );
    }
    assert_eq!(
        Endpoint::parse("http://localhost:7711")?
            .url("/mcp")
            .as_str(),
        "http://127.0.0.1:7711/mcp"
    );
    assert_eq!(
        Endpoint::parse("https://localhost:7711")?
            .url("/mcp")
            .as_str(),
        "https://localhost:7711/mcp"
    );
    Ok(())
}

/// N4: a server is started only for a plain-HTTP loopback address. `https://localhost` and an
/// HTTPS loopback literal are connected to and never started: nothing answers, the relay says
/// so and names no executable, though one is given.
#[tokio::test]
async fn an_https_loopback_address_is_never_started_by_the_shipped_entry_point() -> TestResult {
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-such-okf-jawn-server");
    for host in ["localhost", "127.0.0.1"] {
        let read = Arc::new(AtomicBool::new(false));
        let ran = cli_reading(
            &[
                "mcp",
                "--stdio",
                "--server",
                &format!("https://{host}:{}", closed_port()?),
                "--server-bin",
                &missing.display().to_string(),
            ],
            Some(TOKEN),
            Untouched(read.clone()),
        )
        .await?;
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""), "{host}");
        assert!(
            ran.stderr
                .contains("starts a server only for a plain-HTTP loopback address")
                && !ran.stderr.contains("could not be started"),
            "{host}: {}",
            ran.stderr
        );
        assert!(!read.load(Ordering::SeqCst), "stdin was read");
    }
    Ok(())
}

/// B3: the server the relay starts has no stream of the relay's, so nothing it prints, its
/// single-use launch URL included, reaches the MCP client. The stand-in prints on both its
/// streams before it records that it ran.
#[test]
fn mcp_stdio_what_a_started_server_prints_reaches_no_stream_of_the_relay() -> TestResult {
    let stand_in = StandIn::new()?;
    let port = closed_port()?;
    let child = relay_process(&format!("http://127.0.0.1:{port}"), Some(&stand_in.program))?;
    let recorded = stand_in.recorded(PATIENCE);
    stand_in.stop();
    let (out, err) = stopped(child)?;
    let recorded = recorded?;
    // What the started process was given: where to listen, and not the relay's credential.
    assert_eq!(
        (
            StandIn::value(&recorded, "bind"),
            StandIn::value(&recorded, "token")
        ),
        (Some(format!("127.0.0.1:{port}")), Some(String::new())),
        "{recorded}"
    );
    if cfg!(unix) {
        // An interrupt typed in the client's terminal goes to the client's process group;
        // the started process leads a group of its own.
        let (pid, group) = (
            StandIn::value(&recorded, "pid"),
            StandIn::value(&recorded, "group"),
        );
        assert!(pid.is_some() && pid == group, "{recorded}");
    }
    assert_eq!(out, "", "the relay's stdout");
    assert!(!err.contains(STAND_IN_SAYS), "{err}");
    assert!(
        err.lines().all(|line| line.starts_with("okf-jawn mcp: ")),
        "{err}"
    );
    Ok(())
}

/// B4: one liveness check is bounded. Something accepts the connection and never answers; the
/// relay gives up on the check and says so, and starts nothing.
#[tokio::test]
async fn a_service_that_accepts_and_never_answers_ends_the_liveness_check() -> TestResult {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let holding = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    let started = Instant::now();
    let ran = tokio::time::timeout(
        PATIENCE,
        cli(&["mcp", "--stdio", "--server", &origin], Some(TOKEN), b""),
    )
    .await;
    holding.abort();
    let ran = ran.map_err(|_| "the relay was still waiting for one liveness check")??;
    assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
    assert!(
        ran.stderr
            .contains("The liveness check of the server address failed"),
        "{}",
        ran.stderr
    );
    assert!(
        started.elapsed() >= Duration::from_secs(4),
        "it gave up early"
    );
    Ok(())
}

/// B5: the process ends when its output fails while its input stays open. Only the built
/// executable shows it: a read of standard input that is still pending must not hold the
/// process open after the run has ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_process_ends_when_stdout_fails_while_stdin_stays_open() -> TestResult {
    let listener = Answering::routed(|heard| {
        if heard.line().starts_with("GET ") {
            json_answer("200 OK", "{}")
        } else {
            json_answer("200 OK", "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}")
        }
    })
    .await?;
    let origin = listener.origin();
    let outcome = tokio::task::spawn_blocking(move || -> Result<Option<i32>, String> {
        let mut child = relay_process(&origin, None).map_err(|error| error.to_string())?;
        drop(child.stdout.take());
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let written = stdin
            .write_all(request(&json!(1), "ping", &json!({})).as_bytes())
            .and_then(|()| stdin.flush());
        let started = Instant::now();
        let mut ended = None;
        while ended.is_none() && started.elapsed() < PATIENCE {
            ended = child.try_wait().map_err(|error| error.to_string())?;
            std::thread::sleep(Duration::from_millis(100));
        }
        if ended.is_none() {
            // The stand-in for a hung relay is stopped by the test that started it.
            let _ = child.kill();
            let _ = child.wait();
        }
        // Stdin stayed open until here.
        drop(stdin);
        written.map_err(|error| error.to_string())?;
        Ok(ended.and_then(|status| status.code()))
    })
    .await??;
    assert_eq!(outcome, Some(1), "None: the process never ended");
    Ok(())
}

/// B6: an answer that was written and then cut off is not answered a second time. The failure
/// after the answer goes to stderr.
#[tokio::test]
async fn a_failure_after_the_answer_writes_no_second_answer() -> TestResult {
    let transport = FakeTransport::new(|_, events| {
        Box::pin(async move {
            let _ = events
                .send(response(&json!(1), &json!({})).to_string())
                .await;
            broken("the stream was cut off").reply
        })
    });
    let ran = relay(
        Setup::new(&transport),
        request(&json!(1), "tools/call", &json!({})).as_bytes(),
    )
    .await?;
    assert_eq!(ran.lines()?, [response(&json!(1), &json!({}))]);
    assert!(
        ran.stderr.contains("after a request was answered")
            && ran.stderr.contains("the stream was cut off"),
        "{}",
        ran.stderr
    );
    Ok(())
}

/// B7: the bounds the shipped relay runs with are the stated ones, no larger and no smaller.
/// What each bound does at its edge is tested where the bound is applied: the line and the
/// answer in `framing` and `wire`, the messages in flight in `forwarding`.
#[test]
fn mcp_stdio_default_bounds_are_the_stated_ones() {
    let limits = Limits::default();
    assert_eq!(limits.line_bytes, 16_777_216);
    assert_eq!(limits.line_bytes, MAX_REQUEST_BODY_BYTES);
    assert_eq!(limits.answer_bytes, 67_108_864);
    assert_eq!(limits.in_flight, 16);
    assert_eq!(limits.waiting_bytes, 67_108_864);
    assert_eq!(limits.closing, Duration::from_secs(120));
}

/// What `Credential::from_file` makes of a file with `content`: the length of the
/// `Authorization` value, or the refusal's words.
fn read_token(content: &[u8]) -> Outcome<Result<usize, String>> {
    let path = file("token-case", content)?;
    let read = Credential::from_file(&path);
    std::fs::remove_file(path)?;
    Ok(match read {
        Ok(credential) => Ok(credential.bearer()?.len()),
        Err(error) => Err(error.message),
    })
}

/// B7 and N1: the token file's first line is bounded, from both sides, and a longer line is
/// refused, not cut. A line of exactly 8192 bytes is the credential, with a line feed, with a
/// carriage return and a line feed, with a byte order mark before it, or with nothing after
/// it; one byte more and the file is refused in words that say so.
#[test]
fn mcp_stdio_token_file_line_is_bounded_and_a_longer_line_is_refused() -> TestResult {
    const BOUND: usize = 8192;
    let header = "Bearer ".len().saturating_add(BOUND);
    let longest = vec![b'b'; BOUND];
    for (before, after) in [
        (&b""[..], &b"\n"[..]),
        (b"", b"\r\nsecond line\n"),
        (b"", b""),
        (b"\xEF\xBB\xBF", b"\r\n"),
        (b"\xEF\xBB\xBF", b""),
    ] {
        let content = [before, &longest, after].concat();
        assert_eq!(read_token(&content)?, Ok(header), "{before:?} {after:?}");
    }
    let too_long = vec![b'a'; BOUND.saturating_add(1)];
    for (before, after) in [
        (&b""[..], &b"\n"[..]),
        (b"", b"\r\n"),
        (b"", b""),
        (b"\xEF\xBB\xBF", b"\n"),
    ] {
        let content = [before, &too_long, after].concat();
        let why = err_of(read_token(&content)?)?;
        assert!(
            why.contains("is longer than 8192 bytes") && why.contains("token-case"),
            "{why}"
        );
        assert!(!why.contains("aaaa"), "{why}");
    }
    for length in [10_000, 1_000_000] {
        let why = err_of(read_token(&vec![b'a'; length])?)?;
        assert!(why.contains("is longer than 8192 bytes"), "{why}");
    }
    Ok(())
}

/// B7: no more of a token file is read than the longest line, a mark and a line ending need,
/// however long the file is: of 64 MiB without a line feed, 8198 bytes are read.
#[test]
fn mcp_stdio_token_file_is_read_within_its_bound() -> TestResult {
    const LENGTH: u64 = 67_108_864;
    let mut text = std::io::repeat(b'a').take(LENGTH);
    let why = err_of(Credential::from_reader(
        &mut text,
        Path::new("the-token-file"),
    ))?;
    assert!(
        why.message.contains("the-token-file") && why.message.contains("longer than 8192 bytes"),
        "{}",
        why.message
    );
    assert_eq!(LENGTH.saturating_sub(text.limit()), 8198);
    Ok(())
}

/// N2: a UTF-8 byte order mark is not part of the credential, and UTF-16, which Windows
/// PowerShell 5.1 writes with `>` and `Out-File`, is refused in words that say what the file
/// is and how to write it. No refusal repeats what the file holds.
#[test]
fn mcp_stdio_token_file_encodings() -> TestResult {
    assert_eq!(
        read_token(b"\xEF\xBB\xBFzq7-with-mark-5d1c\r\n")?,
        Ok("Bearer zq7-with-mark-5d1c".len())
    );
    assert_eq!(
        read_token(b"zq7-plain-8f3a2b6c\n")?,
        Ok("Bearer zq7-plain-8f3a2b6c".len())
    );
    let little: Vec<u8> = "zq7-wide\r\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let big: Vec<u8> = "zq7-wide\r\n"
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect();
    for content in [
        little.clone(),
        big.clone(),
        [&b"\xFF\xFE"[..], &little].concat(),
        [&b"\xFE\xFF"[..], &big].concat(),
    ] {
        let why = err_of(read_token(&content)?)?;
        assert!(
            why.contains("is UTF-16")
                && why.contains("Write the file as UTF-8 or ASCII")
                && why.contains("Set-Content -Encoding ascii")
                && why.contains("token-case"),
            "{why}"
        );
    }
    for (content, said) in [
        (&b"\nzq7-on-the-second-line\n"[..], "is empty"),
        (b"", "is empty"),
        (b"zq7\xff\xfe\n", "is not text"),
        (b"zq7\0nul\n", "not usable"),
        (b"zq7 two words\n", "not usable"),
    ] {
        let why = err_of(read_token(content)?)?;
        assert!(why.contains(said), "{said}: {why}");
        assert!(!why.contains("zq7"), "{why}");
    }
    Ok(())
}

/// B7: a refusal's body is read up to the request-body limit and explained; one byte more and
/// it is not held: the refusal is then reported by its status alone.
#[tokio::test]
async fn a_refusal_body_is_read_up_to_the_request_body_limit_and_no_further() -> TestResult {
    let refusal = |length: usize| {
        let empty =
            json!({"code": "conflict", "message": "the item moved", "request_id": ""}).to_string();
        let pad = "x".repeat(length.saturating_sub(empty.len()));
        json!({"code": "conflict", "message": "the item moved", "request_id": pad}).to_string()
    };
    let at_limit = refusal(MAX_REQUEST_BODY_BYTES);
    let over = refusal(MAX_REQUEST_BODY_BYTES.saturating_add(1));
    assert_eq!(
        (at_limit.len(), over.len()),
        (
            MAX_REQUEST_BODY_BYTES,
            MAX_REQUEST_BODY_BYTES.saturating_add(1)
        )
    );
    let listener = Answering::with(vec![
        json_answer("409 Conflict", &at_limit),
        json_answer("409 Conflict", &over),
    ])
    .await?;
    let explained = cli(&["get_health", "--server", &listener.origin()], None, b"").await?;
    assert_eq!(explained.code, 1);
    assert!(
        explained
            .stderr
            .starts_with("okf-jawn: conflict: the item moved\n  request: xxx"),
        "{}",
        explained.stderr.chars().take(200).collect::<String>()
    );
    let unexplained = cli(&["get_health", "--server", &listener.origin()], None, b"").await?;
    assert_eq!(
        (unexplained.code, unexplained.stderr.as_str()),
        (
            1,
            "okf-jawn: conflict: The service answered HTTP 409 without an error it could explain\n"
        )
    );
    Ok(())
}

/// The process's exit code for a failure is 1: tested on the executable, where the code is
/// made, beside the 0 and 2 that `requests` tests there.
#[test]
fn mcp_stdio_executable_exits_1_when_no_service_can_be_reached() -> TestResult {
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-such-okf-jawn-server");
    let output = Command::new(EXECUTABLE)
        .args([
            "mcp",
            "--stdio",
            "--server",
            &format!("http://127.0.0.1:{}", closed_port()?),
            "--server-bin",
        ])
        .arg(&missing)
        .env("OKF_JAWN_TOKEN", TOKEN)
        .stdin(Stdio::null())
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"");
    let said = String::from_utf8(output.stderr)?;
    assert!(said.starts_with("okf-jawn mcp: "), "{said}");
    assert!(!said.contains(TOKEN), "{said}");
    Ok(())
}

/// B2, on Unix: a client that waits for the end of the relay's stdout sees it when the relay
/// ends, although the server the relay started lives on: the started process holds no end of
/// the relay's own streams. The measure is the review's: the end of the stream within two
/// seconds of the relay's end, while the stand-in is still running.
///
/// On Windows this does not hold and the test is not built there: the started process
/// inherits every inheritable handle of the relay, its three standard streams among them
/// (`relay/launch.rs` says so). The same test, run on Windows, fails after about 30 seconds.
#[cfg(unix)]
#[test]
fn mcp_stdio_the_started_server_does_not_hold_the_relay_s_stdout_open() -> TestResult {
    let stand_in = StandIn::new()?;
    let mut child = relay_process(
        &format!("http://127.0.0.1:{}", closed_port()?),
        Some(&stand_in.program),
    )?;
    let recorded = stand_in.recorded(PATIENCE);
    // The process may have ended already; then there is nothing to stop.
    let _ = child.kill();
    child.wait()?;
    let ended = Instant::now();
    let mut out = String::new();
    // This read returns only when every holder of the stream's other end has let go of it.
    let read = some(child.stdout.take(), "stdout")?.read_to_string(&mut out);
    let waited = ended.elapsed();
    stand_in.stop();
    recorded?;
    read?;
    assert!(
        waited < Duration::from_secs(2),
        "the end of the relay's stdout came {waited:?} after the relay ended"
    );
    Ok(())
}
