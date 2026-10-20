//! The production transport over real HTTP: the exact requests the relay sends to a loopback
//! listener answered by hand, and what it makes of the listener's raw answers.
//!
//! This is the proof of what is on the wire: the headers of every `POST`, the liveness check
//! that carries no credential, the redirect that is not followed, and the event-stream and
//! answer bounds. The doubles in the other modules cannot show any of it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use okf_jawn_cli::relay::{
    EventParser, HttpTransport, Post, Probe, Room, Transport, TransportError,
};
use okf_jawn_cli::{Credential, Endpoint};
use okf_jawn_contract::transport::MAX_REQUEST_BODY_BYTES;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{notification, request};
use crate::check::{TestResult, err_of, some};
use crate::support::doubles::{Body, Reply};
use crate::support::http::{Answering, Heard, answer, closed, events, json as json_answer};
use crate::support::{Outcome, TOKEN, Untouched, cli, cli_reading};

/// The request among `heard` whose body mentions `text`.
fn mentioning<'heard>(heard: &'heard [Heard], text: &str) -> Outcome<&'heard Heard> {
    Ok(some(
        heard
            .iter()
            .find(|request| String::from_utf8_lossy(&request.body).contains(text)),
        text,
    )?)
}

/// Feed `bytes` to `parser` and add the data of each event they complete to `events`.
async fn fed(
    parser: &mut EventParser,
    mut bytes: &[u8],
    events: &mut Vec<String>,
) -> Result<(), TransportError> {
    while let Some(event) = parser.next(&mut bytes).await? {
        events.push(event.as_str().to_owned());
    }
    Ok(())
}

/// The data of the events a fresh parser of `limit` bytes reads from a stream that arrives in
/// `parts`.
async fn events_of(
    parts: &[&[u8]],
    limit: usize,
    room: &Room,
) -> Result<Vec<String>, TransportError> {
    let mut parser = EventParser::new(limit, room);
    let mut events = Vec::new();
    for part in parts {
        fed(&mut parser, part, &mut events).await?;
    }
    Ok(events)
}

/// The data of the events of `stream`, read whole. The same stream is also read a byte at a
/// time and cut in two at every place, and each way must give the same events as the whole.
async fn however_cut(stream: &[u8], limit: usize, room: &Room) -> Outcome<Vec<String>> {
    let whole = events_of(&[stream], limit, room)
        .await
        .map_err(|error| error.0)?;
    let bytes: Vec<&[u8]> = stream.chunks(1).collect();
    let single = events_of(&bytes, limit, room)
        .await
        .map_err(|error| error.0)?;
    assert_eq!(single, whole, "a byte at a time");
    for at in 0..=stream.len() {
        let (first, second) = some(stream.split_at_checked(at), "a place in the stream")?;
        let cut = events_of(&[first, second], limit, room)
            .await
            .map_err(|error| error.0)?;
        assert_eq!(cut, whole, "cut after {at} bytes");
    }
    Ok(whole)
}

/// Send `post` through `transport` and collect the reply and the events passed on.
async fn send(
    transport: &HttpTransport,
    post: Post,
) -> (Result<Reply, TransportError>, Vec<String>) {
    send_within(transport, post, &Room::new(1_048_576)).await
}

/// [`send`], with the room the transport takes for what it reads. The reply and the events
/// come back as plain texts, and the room they held is free again.
async fn send_within(
    transport: &HttpTransport,
    post: Post,
    room: &Room,
) -> (Result<Reply, TransportError>, Vec<String>) {
    let (sender, mut receiver) = mpsc::channel(64);
    let reply = transport.send(post, sender, room).await.map(|reply| Reply {
        status: reply.status,
        session: reply.session,
        body: match reply.body {
            okf_jawn_cli::relay::Body::Empty => Body::Empty,
            okf_jawn_cli::relay::Body::Json(text) => Body::Json(text.as_str().to_owned()),
            okf_jawn_cli::relay::Body::Events => Body::Events,
            okf_jawn_cli::relay::Body::Other => Body::Other,
        },
    });
    let mut passed = Vec::new();
    while let Some(event) = receiver.recv().await {
        passed.push(event.as_str().to_owned());
    }
    (reply, passed)
}

/// A production transport to `listener` that reads at most `answer_bytes` of one answer.
fn transport(origin: &str, answer_bytes: usize) -> Outcome<HttpTransport> {
    let credential = some(Credential::parse(TOKEN, "the test")?, "a credential")?;
    Ok(HttpTransport::new(
        &Endpoint::parse(origin)?,
        &credential,
        answer_bytes,
    )?)
}

/// Every `POST` goes to the MCP endpoint with the credential and both content negotiations.
fn common_headers(heard: &[Heard]) -> TestResult {
    for post in heard.iter().skip(1) {
        assert_eq!(post.line(), "POST /mcp HTTP/1.1");
        assert_eq!(
            post.header("authorization"),
            Some(format!("Bearer {TOKEN}"))
        );
        assert_eq!(
            post.header("content-type").as_deref(),
            Some("application/json")
        );
        let accept = some(post.header("accept"), "an Accept header")?;
        assert!(
            accept.contains("application/json") && accept.contains("text/event-stream"),
            "{accept}"
        );
    }
    Ok(())
}

/// The relay as shipped, against a service that answers a handshake, a notification and a
/// call whose answer is an event stream.
#[tokio::test]
async fn the_relay_sends_the_headers_streamable_http_requires() -> TestResult {
    let stream = ": keep-alive\n\nid: 0\nretry: 3000\ndata:\n\n\
                  data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n\
                  event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\r\ndata: \"result\":{\"ok\":true}}\r\n\r\n";
    let listener = Answering::routed(move |heard| {
        let body = String::from_utf8_lossy(&heard.body).into_owned();
        if heard.line().starts_with("GET /healthz ") {
            json_answer("200 OK", "{\"status\":\"ok\"}")
        } else if body.contains("\"initialize\"") {
            json_answer(
                "200 OK",
                "{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{}}}",
            )
        } else if body.contains("notifications/initialized") {
            answer("202 Accepted", &[], "")
        } else {
            events(stream)
        }
    })
    .await?;
    let call = request(
        &json!(1),
        "tools/call",
        &json!({"name": "grep", "arguments": {}, "_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}}),
    );
    let initialize = request(
        &json!(0),
        "initialize",
        &json!({"protocolVersion": "2025-11-25"}),
    );
    let input = format!(
        "{initialize}{}{call}",
        notification("notifications/initialized")
    );
    let ran = cli(
        &["mcp", "--stdio", "--server", &listener.origin()],
        Some(TOKEN),
        input.as_bytes(),
    )
    .await?;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    let mut answers = ran.lines()?;
    assert_eq!(
        answers
            .first()
            .and_then(|answer| answer.pointer("/result/protocolVersion")),
        Some(&json!("2025-06-18"))
    );
    assert_eq!(
        answers.split_off(1),
        [
            json!({"jsonrpc": "2.0", "method": "notifications/progress"}),
            json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": true}}),
        ]
    );

    let heard = listener.heard()?;
    assert_eq!(heard.len(), 4);
    let liveness = some(heard.first(), "the liveness check")?;
    assert_eq!(liveness.line(), "GET /healthz HTTP/1.1");
    assert_eq!(
        liveness.header("authorization"),
        None,
        "the liveness check carries no credential"
    );
    common_headers(&heard)?;
    let first = mentioning(&heard, "\"initialize\"")?;
    assert_eq!(first.body, initialize.trim_end().as_bytes());
    for name in [
        "mcp-protocol-version",
        "mcp-method",
        "mcp-name",
        "mcp-session-id",
    ] {
        assert_eq!(first.header(name), None, "{name} on initialize");
    }
    let second = mentioning(&heard, "notifications/initialized")?;
    assert_eq!(
        second.header("mcp-protocol-version").as_deref(),
        Some("2025-06-18")
    );
    assert_eq!(second.header("mcp-method"), None);
    let third = mentioning(&heard, "tools/call")?;
    assert_eq!(
        third.header("mcp-protocol-version").as_deref(),
        Some("2026-07-28")
    );
    assert_eq!(third.header("mcp-method").as_deref(), Some("tools/call"));
    assert_eq!(third.header("mcp-name").as_deref(), Some("grep"));
    assert_eq!(third.body, call.trim_end().as_bytes());
    Ok(())
}

/// A redirect is an answer, never a new destination: the address it names hears nothing.
#[tokio::test]
async fn the_relay_never_follows_a_redirect() -> TestResult {
    let elsewhere = Answering::with(vec![
        json_answer("200 OK", "{}"),
        json_answer("200 OK", "{}"),
    ])
    .await?;
    let target = format!("{}/mcp", elsewhere.origin());
    let listener = Answering::routed(move |heard| {
        if heard.line().starts_with("GET ") {
            json_answer("200 OK", "{}")
        } else {
            answer("307 Temporary Redirect", &[("location", &target)], "")
        }
    })
    .await?;
    let ran = cli(
        &["mcp", "--stdio", "--server", &listener.origin()],
        Some(TOKEN),
        request(&json!(4), "tools/list", &json!({})).as_bytes(),
    )
    .await?;
    assert_eq!(ran.code, 0);
    let answers = ran.lines()?;
    let refused = some(answers.first(), "an answer")?;
    assert_eq!(refused.get("id"), Some(&json!(4)));
    let message = some(
        refused.pointer("/error/message").and_then(Value::as_str),
        "a message",
    )?;
    assert!(message.contains("HTTP 307"), "{message}");
    assert!(elsewhere.heard()?.is_empty(), "a redirect was followed");
    Ok(())
}

/// A line over the contract's request-body limit is refused by the relay as shipped and
/// never reaches the service.
#[tokio::test]
async fn the_relay_as_shipped_refuses_a_line_over_the_request_body_limit() -> TestResult {
    let listener = Answering::routed(|heard| {
        if heard.line().starts_with("GET ") {
            json_answer("200 OK", "{}")
        } else {
            answer("202 Accepted", &[], "")
        }
    })
    .await?;
    let mut input = vec![b'a'; MAX_REQUEST_BODY_BYTES.saturating_add(1)];
    input.push(b'\n');
    input.extend_from_slice(notification("notifications/initialized").as_bytes());
    let ran = cli(
        &["mcp", "--stdio", "--server", &listener.origin()],
        Some(TOKEN),
        &input,
    )
    .await?;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    let answers = ran.lines()?;
    assert_eq!(answers.len(), 1);
    assert_eq!(
        answers
            .first()
            .and_then(|answer| answer.pointer("/error/code")),
        Some(&json!(-32600))
    );
    let heard = listener.heard()?;
    assert_eq!(
        heard.len(),
        2,
        "the liveness check and the notification, not the long line"
    );
    assert!(heard.iter().all(|request| request.body.len() < 1024));
    Ok(())
}

#[tokio::test]
async fn the_liveness_check_tells_refused_from_answering() -> TestResult {
    let nowhere = closed().await?;
    assert_eq!(
        transport(&format!("http://{nowhere}"), 1024)?.probe().await,
        Probe::Refused
    );
    for (status, expected) in [
        ("200 OK", Probe::Up),
        ("204 No Content", Probe::Up),
        ("503 Service Unavailable", Probe::Unhealthy(503)),
        ("404 Not Found", Probe::Unhealthy(404)),
        ("302 Found", Probe::Unhealthy(302)),
    ] {
        let listener = Answering::with(vec![answer(
            status,
            &[("location", "http://127.0.0.1:9/")],
            "",
        )])
        .await?;
        assert_eq!(
            transport(&listener.origin(), 1024)?.probe().await,
            expected,
            "{status}"
        );
        let heard = listener.heard()?;
        let check = some(heard.first(), "the check")?;
        assert_eq!(check.line(), "GET /healthz HTTP/1.1");
        assert_eq!(check.header("authorization"), None);
    }
    Ok(())
}

/// One answer is read up to the limit and no further: a JSON body or one event of exactly the
/// limit is taken, and one byte more is a failure.
#[tokio::test]
async fn an_answer_is_read_up_to_the_limit_and_no_further() -> TestResult {
    let at_limit = format!("\"{}\"", "a".repeat(62));
    let over = format!("\"{}\"", "a".repeat(63));
    let listener = Answering::with(vec![
        answer(
            "200 OK",
            &[
                ("content-type", "application/json; charset=utf-8"),
                ("mcp-session-id", "s-1"),
            ],
            &at_limit,
        ),
        json_answer("200 OK", &over),
        events(&format!("data: {}\n\n", "b".repeat(58))),
        events(&format!(
            "data: {}\n\ndata: {}\n\n",
            "c".repeat(8),
            "d".repeat(59)
        )),
    ])
    .await?;
    let transport = transport(&listener.origin(), 64)?;
    let post = || Post {
        body: b"{}".to_vec(),
        headers: Vec::new(),
    };
    let (taken, _) = send(&transport, post()).await;
    let taken = taken.map_err(|error| error.0)?;
    assert_eq!((taken.status, taken.session.as_deref()), (200, Some("s-1")));
    assert_eq!(taken.body, Body::Json(at_limit));
    let (refused, _) = send(&transport, post()).await;
    assert!(err_of(refused)?.0.contains("64 bytes"));
    let (stream, passed) = send(&transport, post()).await;
    assert_eq!(stream.map_err(|error| error.0)?.body, Body::Events);
    assert_eq!(passed, ["b".repeat(58)]);
    let (cut, passed) = send(&transport, post()).await;
    assert!(err_of(cut)?.0.contains("64 bytes"));
    assert_eq!(
        passed,
        ["c".repeat(8)],
        "the event before the oversized one stays delivered"
    );
    Ok(())
}

#[tokio::test]
async fn an_answer_without_a_body_or_of_another_kind_is_told_apart() -> TestResult {
    let listener = Answering::with(vec![
        answer("202 Accepted", &[], ""),
        answer("200 OK", &[("content-type", "application/json")], ""),
        answer(
            "500 Internal Server Error",
            &[("content-type", "text/plain")],
            "boom",
        ),
        answer(
            "400 Bad Request",
            &[("content-type", "text/event-stream")],
            "data: {}\n\n",
        ),
    ])
    .await?;
    let transport = transport(&listener.origin(), 1024)?;
    let mut seen = Vec::new();
    for _ in 0..4 {
        let (reply, passed) = send(
            &transport,
            Post {
                body: b"{}".to_vec(),
                headers: vec![("Mcp-Method", "ping".to_owned())],
            },
        )
        .await;
        let reply = reply.map_err(|error| error.0)?;
        assert_eq!(passed, Vec::<String>::new());
        seen.push((reply.status, reply.body));
    }
    assert_eq!(
        seen,
        [
            (202, Body::Empty),
            (200, Body::Empty),
            (500, Body::Other),
            (400, Body::Other)
        ]
    );
    let heard = listener.heard()?;
    assert_eq!(
        heard
            .first()
            .and_then(|request| request.header("mcp-method"))
            .as_deref(),
        Some("ping")
    );
    Ok(())
}

/// The event-stream framing: however the bytes are split, the same events come out.
#[tokio::test]
async fn mcp_stdio_event_stream_framing() -> TestResult {
    let room = Room::new(1_048_576);
    let stream = "\u{feff}: a comment\n\
                  data: one\n\n\
                  event: message\ndata:two\ndata: lines\n\n\
                  id: 7\nretry: 100\ndata:\n\n\
                  event: endpoint\ndata: not a message\n\n\
                  data\n\n\
                  data: three\r\n\r\n\
                  unknown: field\ndata:  four\n\n\
                  data: never ended";
    let expected = ["one", "two\nlines", "three", " four"];
    let mut whole = EventParser::new(1024, &room);
    let mut events = Vec::new();
    fed(&mut whole, stream.as_bytes(), &mut events)
        .await
        .map_err(|error| error.0)?;
    assert_eq!(events, expected);
    let mut split = EventParser::new(1024, &room);
    let mut events = Vec::new();
    for byte in stream.as_bytes() {
        fed(&mut split, std::slice::from_ref(byte), &mut events)
            .await
            .map_err(|error| error.0)?;
    }
    assert_eq!(events, expected);

    let mut bounded = EventParser::new(16, &room);
    let mut events = Vec::new();
    fed(
        &mut bounded,
        b"data: 0123456789\n\ndata: 0123456789\n\n",
        &mut events,
    )
    .await
    .map_err(|error| error.0)?;
    assert_eq!(events, ["0123456789", "0123456789"]);
    let refused = err_of(
        fed(
            &mut bounded,
            b"data: kept\n\ndata: 0123456789a",
            &mut events,
        )
        .await,
    )?;
    assert!(refused.0.contains("16 bytes"), "{}", refused.0);
    assert_eq!(
        events.len(),
        3,
        "the event before the oversized one is kept"
    );
    let mut invalid = EventParser::new(64, &room);
    assert!(
        err_of(fed(&mut invalid, b"data: \xff\xfe\n\n", &mut events).await)?
            .0
            .contains("UTF-8")
    );
    Ok(())
}

/// The event-stream format ends a line with a line feed, with a carriage return, or with a
/// carriage return and the line feed after it (the pull request's finding 1). The same events,
/// written with each of the three, give the same messages, however the bytes arrive. The
/// letters backslash and `r` inside a value are two letters and end nothing.
#[tokio::test]
async fn an_event_stream_is_split_at_each_of_its_three_line_ends() -> TestResult {
    let room = Room::new(1_048_576);
    let lines = [
        "\u{feff}: a comment",
        "data: one",
        "",
        "event: message",
        "data:two",
        "data: lines",
        "",
        "id: 7",
        "retry: 100",
        "data:",
        "",
        "event: endpoint",
        "data: not a message",
        "",
        "data",
        "",
        "data: {\"text\":\"a\\rb\\nc\"}",
        "",
        "unknown: field",
        "data:  four",
        "",
        "data: never ended",
    ];
    let expected = ["one", "two\nlines", "{\"text\":\"a\\rb\\nc\"}", " four"];
    for end in ["\n", "\r\n", "\r"] {
        let stream = lines.join(end);
        assert_eq!(
            however_cut(stream.as_bytes(), 1024, &room).await?,
            expected,
            "lines that end with {end:?}"
        );
    }
    // One stream with all three: each pair of line ends that can stand side by side.
    let mixed = "data: one\r\n\r\
                 event: message\rdata:two\ndata: lines\r\n\n\
                 event: endpoint\r\ndata: not a message\r\r\
                 data: three\n\r\
                 data: four\r\r\n\
                 data: five\n\n\
                 data: never ended\r";
    assert_eq!(
        however_cut(mixed.as_bytes(), 1024, &room).await?,
        ["one", "two\nlines", "three", "four", "five"]
    );
    Ok(())
}

/// A carriage return that is the last byte of one chunk and the line feed that is the first
/// byte of the next are one line end, not two: a second one would be the blank line that ends
/// the event, so the event would be handed over early, in two halves, or under the type of
/// the event before it. A carriage return at the end of a chunk ends its line at once,
/// whatever comes after it, and so does one at the very end of the stream.
#[tokio::test]
async fn a_carriage_return_and_the_line_feed_after_it_are_one_line_end_across_chunks() -> TestResult
{
    let room = Room::new(1_048_576);
    let cases: [(&[&[u8]], &[&str]); 7] = [
        // Every carriage return ends a chunk and its line feed starts the next.
        (&[b"data: a\r", b"\ndata: b\r", b"\n\r", b"\n"], &["a\nb"]),
        // A chunk that is nothing but the line feed of a line end.
        (&[b"data: a\r", b"\n", b"data: b\n\n"], &["a\nb"]),
        // A lone carriage return at the end of a chunk, and other data after it.
        (&[b"data: a\r", b"data: b\r\r"], &["a\nb"]),
        // A lone carriage return, a line without an end, then a chunk that starts with the
        // line feed that ends that line: it belongs to no carriage return.
        (&[b"data: a\r", b"data: b", b"\n\n"], &["a\nb"]),
        // The carriage return that ends the event is the last byte of the stream.
        (&[b"data: last\r\r"], &["last"]),
        (&[b"data: a\r\n", b"\r\n", b"data: b\r", b"\r"], &["a", "b"]),
        // The type of an event lasts until the blank line, and no longer.
        (
            &[b"event: other\r", b"\ndata: x\r", b"\n\r", b"\ndata: y\r\r"],
            &["y"],
        ),
    ];
    for (parts, expected) in cases {
        let shown: Vec<String> = parts
            .iter()
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect();
        let events = events_of(parts, 1024, &room)
            .await
            .map_err(|error| error.0)?;
        assert_eq!(events, expected, "{shown:?}");
    }
    Ok(())
}

/// The bound of one event is the same whichever way its lines end: an event of exactly the
/// limit is taken, and one byte more is refused, the event before it kept. A line end is not
/// part of what is buffered.
#[tokio::test]
async fn an_event_is_bounded_alike_whichever_way_its_lines_end() -> TestResult {
    let room = Room::new(1_048_576);
    for end in ["\n", "\r\n", "\r"] {
        let at_limit = format!("data: 0123456789{end}{end}data: 0123456789{end}{end}");
        assert_eq!(
            however_cut(at_limit.as_bytes(), 16, &room).await?,
            ["0123456789", "0123456789"],
            "{end:?}"
        );
        let over = format!("data: kept{end}{end}data: 0123456789a{end}{end}");
        let mut bounded = EventParser::new(16, &room);
        let mut events = Vec::new();
        let refused = err_of(fed(&mut bounded, over.as_bytes(), &mut events).await)?;
        assert!(refused.0.contains("16 bytes"), "{end:?}: {}", refused.0);
        assert_eq!(events, ["kept"], "{end:?}");
    }
    Ok(())
}

/// The relay as shipped, against a service whose event stream ends its lines with carriage
/// returns alone: the request gets the service's answer, and nothing is said.
#[tokio::test]
async fn the_relay_as_shipped_reads_an_event_stream_of_carriage_returns() -> TestResult {
    let stream = ": keep-alive\r\revent: message\rdata: {\"jsonrpc\":\"2.0\",\"id\":1,\r\
                  data: \"result\":{\"ok\":true}}\r\r";
    let listener = Answering::routed(move |heard| {
        if heard.line().starts_with("GET ") {
            json_answer("200 OK", "{}")
        } else {
            events(stream)
        }
    })
    .await?;
    let ran = cli(
        &["mcp", "--stdio", "--server", &listener.origin()],
        Some(TOKEN),
        request(&json!(1), "tools/call", &json!({})).as_bytes(),
    )
    .await?;
    assert_eq!(
        (ran.code, ran.stdout.as_str(), ran.stderr.as_str()),
        (
            0,
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n",
            ""
        )
    );
    Ok(())
}

/// The relay as shipped, with nothing listening at a loopback address and a server executable
/// that is not there: stderr says why and names the executable it was told to start, the exit
/// code is 1, stdin is not read and stdout stays empty.
#[tokio::test]
async fn the_relay_as_shipped_explains_a_server_it_cannot_start() -> TestResult {
    let nowhere = closed().await?;
    let missing = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("no-okf-jawn-server-here")
        .display()
        .to_string();
    let read = Arc::new(AtomicBool::new(false));
    let ran = cli_reading(
        &[
            "mcp",
            "--stdio",
            "--server",
            &format!("http://{nowhere}"),
            "--server-bin",
            &missing,
        ],
        Some(TOKEN),
        Untouched(read.clone()),
    )
    .await?;
    assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
    assert!(
        ran.stderr.starts_with(
            "okf-jawn mcp: No service is running, and the server could not be started: "
        ),
        "{}",
        ran.stderr
    );
    assert!(ran.stderr.contains(&missing), "{}", ran.stderr);
    assert!(!read.load(Ordering::SeqCst), "stdin was read");
    Ok(())
}

/// The production transport holds the bytes of an answer only in room it took for them (the
/// re-review's F1). A JSON answer whose length the service announced holds exactly that
/// length; the data of an event holds room too; each gives it back when it is dropped. And
/// while the room has no place, the transport waits: the answer lies ready on the wire and
/// is not taken until the text that holds the place is gone.
#[tokio::test]
async fn the_transport_holds_an_answer_only_in_room_it_took() -> TestResult {
    let body = format!("\"{}\"", "a".repeat(298));
    let listener = Answering::with(vec![
        json_answer("200 OK", &body),
        events(&format!("data: {}\n\n", "b".repeat(300))),
        json_answer("200 OK", &body),
    ])
    .await?;
    let transport = transport(&listener.origin(), 1024)?;
    let post = || Post {
        body: b"{}".to_vec(),
        headers: Vec::new(),
    };
    let room = Room::new(1000);
    let (sender, _receiver) = mpsc::channel(4);
    let reply = transport
        .send(post(), sender, &room)
        .await
        .map_err(|error| error.0)?;
    assert_eq!(
        room.free(),
        700,
        "a body of 300 bytes holds 300 bytes of room"
    );
    drop(reply);
    assert_eq!(room.free(), 1000);
    let (sender, mut receiver) = mpsc::channel(4);
    transport
        .send(post(), sender, &room)
        .await
        .map_err(|error| error.0)?;
    let event = some(receiver.recv().await, "an event")?;
    assert_eq!(event.len(), 300);
    assert!(room.free() <= 700, "the event holds {} bytes", room.free());
    drop(event);
    assert_eq!(room.free(), 1000);
    // No shared part, and the one place beside it is taken.
    let narrow = Room::new(0);
    let first = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        narrow.hold("a text that has the place".to_owned()),
    )
    .await?;
    let (sender, _receiver) = mpsc::channel(4);
    let mut sending = Box::pin(transport.send(post(), sender, &narrow));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(500), &mut sending)
            .await
            .is_err(),
        "the transport took an answer it had no room for"
    );
    drop(first);
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), sending)
        .await?
        .map_err(|error| error.0)?;
    assert_eq!(reply.status, 200);
    Ok(())
}

/// The most memory the process `pid` has held so far, in bytes: on Linux its peak resident set
/// (`VmHWM` in `/proc/<pid>/status`).
#[cfg(target_os = "linux")]
fn peak_bytes(pid: u32) -> Outcome<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
    let line = some(
        status.lines().find(|line| line.starts_with("VmHWM:")),
        "the VmHWM line",
    )?;
    let kib: u64 = some(line.split_whitespace().nth(1), "a number of KiB")?.parse()?;
    Ok(kib.saturating_mul(1024))
}

/// The most memory the process `pid` has held so far, in bytes: on Windows its peak private
/// bytes, which Windows PowerShell's `Get-Process` calls `PeakPagedMemorySize64` (no API of the
/// standard library reads another process's memory).
#[cfg(windows)]
fn peak_bytes(pid: u32) -> Outcome<u64> {
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("(Get-Process -Id {pid}).PeakPagedMemorySize64"),
        ])
        .output()?;
    Ok(String::from_utf8(output.stdout)?.trim().parse()?)
}

/// Start the built relay against `origin`, give it `line` and a line feed, and read its first
/// output line; returns its peak memory then, what it wrote to its error stream once its input
/// was closed and it had ended (with exit code 0), and that output line.
#[cfg(any(target_os = "linux", windows))]
fn relay_one_line(origin: &str, line: String) -> Outcome<(u64, String, String)> {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    let mut child = Command::new(crate::support::EXECUTABLE)
        .args(["mcp", "--stdio", "--server", origin])
        .env("OKF_JAWN_TOKEN", TOKEN)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = some(child.stdin.take(), "stdin")?;
    let feeding = std::thread::spawn(move || -> std::io::Result<std::process::ChildStdin> {
        stdin.write_all(line.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(stdin)
    });
    let mut written = String::new();
    BufReader::new(some(child.stdout.take(), "stdout")?).read_line(&mut written)?;
    let peak = peak_bytes(child.id());
    let stdin = feeding.join().map_err(|_| "the feeding thread failed")??;
    drop(stdin);
    let output = child.wait_with_output()?;
    assert_eq!(output.status.code(), Some(0));
    Ok((peak?, String::from_utf8(output.stderr)?, written))
}

/// C2: a request line is never read into a tree of values. The built relay is given one line
/// of exactly the request limit, 16 MiB, that holds 8 million numbers, the shape that cost
/// about 400 MiB while it was parsed. It forwards the line byte for byte, passes the answer
/// on, and its peak memory stays within four times the line (64 MiB): the line itself, its
/// buffer while it grows, and what the process holds anyway. Measured on the built executable
/// as the operating system counts it (`peak_bytes`), Windows and Linux; not built elsewhere.
#[cfg(any(target_os = "linux", windows))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_stdio_a_request_line_of_the_full_limit_costs_a_bounded_memory() -> TestResult {
    const BOUND: u64 = 67_108_864;
    let answer = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}";
    let listener = Answering::routed(move |heard| {
        if heard.line().starts_with("GET ") {
            json_answer("200 OK", "{}")
        } else {
            json_answer("200 OK", answer)
        }
    })
    .await?;
    let opening = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"x\",\
                \"arguments\":{\"n\":[0";
    let tail = "]}}}";
    let room = MAX_REQUEST_BODY_BYTES.saturating_sub(opening.len().saturating_add(tail.len()));
    let numbers = room / 2;
    let mut line = String::with_capacity(MAX_REQUEST_BODY_BYTES.saturating_add(1));
    line.push_str(opening);
    line.push_str(&",0".repeat(numbers));
    line.push_str(&" ".repeat(room.saturating_sub(numbers.saturating_mul(2))));
    line.push_str(tail);
    assert_eq!(line.len(), MAX_REQUEST_BODY_BYTES);
    assert!(numbers > 8_000_000);
    let origin = listener.origin();
    let sent = line.clone();
    let measuring = tokio::task::spawn_blocking(move || {
        relay_one_line(&origin, sent).map_err(|error| error.to_string())
    });
    let (peak, said, written) =
        tokio::time::timeout(std::time::Duration::from_secs(300), measuring)
            .await
            .map_err(|_| "the relay did not answer one long line within five minutes")???;
    // Seen with `--nocapture`: the figure the report quotes.
    eprintln!("the relay's peak memory for one line of 16 MiB: {peak} bytes");
    assert_eq!(
        (written.as_str(), said.as_str()),
        (format!("{answer}\n").as_str(), "")
    );
    let heard = listener.heard()?;
    let post = some(heard.get(1), "the POST")?;
    assert!(
        post.body == line.as_bytes(),
        "the line was not forwarded as it came"
    );
    assert!(
        peak < BOUND,
        "the relay's peak memory was {} MiB for one line of 16 MiB",
        peak / 1_048_576
    );
    Ok(())
}

/// The answer bound of the relay as shipped is the stated one, all the way to its transport
/// (the re-review's F4). Through the entry point: an answer of exactly 64 MiB is passed on
/// whole, and one of a byte more is refused for its request in words that name the bound.
#[tokio::test]
async fn the_relay_as_shipped_reads_an_answer_up_to_the_stated_bound() -> TestResult {
    let bound = okf_jawn_cli::relay::Limits::default().answer_bytes;
    assert_eq!(bound, 67_108_864);
    let frame = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"s\":\"\"}}".len();
    for (extra, passes) in [(0_usize, true), (1, false)] {
        let body = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"s\":\"{}\"}}}}",
            "a".repeat(bound.saturating_sub(frame).saturating_add(extra))
        );
        assert_eq!(body.len(), bound.saturating_add(extra));
        let served = Arc::new(json_answer("200 OK", &body));
        drop(body);
        let listener = Answering::routed(move |heard| {
            if heard.line().starts_with("GET ") {
                json_answer("200 OK", "{}")
            } else {
                served.as_ref().clone()
            }
        })
        .await?;
        let ran = cli(
            &["mcp", "--stdio", "--server", &listener.origin()],
            Some(TOKEN),
            request(&json!(1), "tools/call", &json!({})).as_bytes(),
        )
        .await?;
        if passes {
            assert_eq!(
                (ran.stdout.len(), ran.stderr.as_str()),
                (bound.saturating_add(1), "")
            );
        } else {
            let lines = ran.lines()?;
            let only = some(lines.first(), "an answer")?;
            assert_eq!(
                (lines.len(), only.get("id"), only.pointer("/error/code")),
                (1, Some(&json!(1)), Some(&json!(-32603)))
            );
            assert!(
                ran.stdout
                    .contains("the service's answer exceeds 67108864 bytes"),
                "{}",
                ran.stdout
            );
        }
    }
    Ok(())
}
