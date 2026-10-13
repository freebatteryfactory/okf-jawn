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
    let first = narrow.hold("a text that has the place".to_owned()).await;
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
