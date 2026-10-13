//! Ruling 1, framing: one JSON-RPC message per line in each direction, a bounded line, a
//! parse error for a line that is not JSON, and nothing but protocol messages on stdout.

use okf_jawn_cli::relay::{Body, Frame, Frames, Limits, Probe};
use okf_jawn_contract::transport::MAX_REQUEST_BODY_BYTES;
use serde_json::{Value, json};

use super::{notification, request};
use crate::check::{TestResult, some};
use crate::support::doubles::{FakeTransport, Setup, accepted, broken, relay, status};

/// Read every frame of `input` at `limit`.
async fn frames(input: &[u8], limit: usize) -> std::io::Result<Vec<Frame>> {
    let mut frames = Frames::new(input, limit);
    let mut all = Vec::new();
    loop {
        let frame = frames.next().await?;
        let ended = frame == Frame::End;
        all.push(frame);
        if ended {
            return Ok(all);
        }
    }
}

fn line(text: &str) -> Frame {
    Frame::Line(text.as_bytes().to_vec())
}

#[tokio::test]
async fn each_line_in_is_one_message_out_and_each_answer_is_one_line() -> TestResult {
    let transport = FakeTransport::echoing();
    let first = "{\"jsonrpc\": \"2.0\",  \"method\": \"tools/list\", \"id\": 1}";
    let second = "{\"id\":\"b\",\"jsonrpc\":\"2.0\",\"method\":\"ping\"}";
    let ran = relay(
        Setup::new(&transport),
        format!("{first}\n{second}\n").as_bytes(),
    )
    .await?;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    let mut answers = ran.lines()?;
    answers.sort_by_key(ToString::to_string);
    assert_eq!(
        answers,
        [
            json!({"jsonrpc": "2.0", "id": "b", "result": {"method": "ping"}}),
            json!({"jsonrpc": "2.0", "id": 1, "result": {"method": "tools/list"}}),
        ]
    );
    let mut bodies: Vec<String> = Vec::new();
    for post in transport.posts()? {
        bodies.push(String::from_utf8(post.body)?);
    }
    bodies.sort();
    assert_eq!(
        bodies,
        [second, first],
        "each message is sent as the client wrote it"
    );
    Ok(())
}

/// The limit counts the bytes before the line feed, without a carriage return: a line of
/// exactly the limit is a line, one byte more is too long, and the next line is unharmed.
#[tokio::test]
async fn a_line_at_the_limit_is_read_and_one_byte_more_is_too_long() -> TestResult {
    assert_eq!(
        frames(
            b"12345678\n123456789\n1234567\r\n12345678\r\n123456789\r\nlast",
            8
        )
        .await?,
        [
            line("12345678"),
            Frame::TooLong,
            line("1234567"),
            line("12345678"),
            Frame::TooLong,
            line("last"),
            Frame::End,
        ]
    );
    assert_eq!(
        frames(b"\n\r\n", 8).await?,
        [line(""), line(""), Frame::End]
    );
    assert_eq!(frames(b"123456789", 8).await?, [Frame::TooLong, Frame::End]);
    assert_eq!(frames(b"", 8).await?, [Frame::End]);
    Ok(())
}

/// A line far over the limit, arriving in the reader's small pieces, is one `TooLong` frame.
#[tokio::test]
async fn a_very_long_line_is_dropped_and_the_next_line_is_read() -> TestResult {
    let mut input = vec![b'a'; 1_048_576];
    input.extend_from_slice(b"\nnext\n");
    assert_eq!(
        frames(&input, 16).await?,
        [Frame::TooLong, line("next"), Frame::End]
    );
    Ok(())
}

#[tokio::test]
async fn a_line_over_the_limit_gets_an_error_and_is_never_forwarded() -> TestResult {
    let transport = FakeTransport::echoing();
    let at_limit = request(
        &json!(1),
        "ping",
        &json!({"pad": "aaaaaaaaaaaaaaaaaaaaaaaaaa"}),
    );
    let limit = at_limit.len().saturating_sub(1);
    let over = request(
        &json!(2),
        "ping",
        &json!({"pad": "aaaaaaaaaaaaaaaaaaaaaaaaaaa"}),
    );
    assert_eq!(over.len(), at_limit.len().saturating_add(1));
    let mut setup = Setup::new(&transport);
    setup.limits = Limits {
        line_bytes: limit,
        ..Limits::default()
    };
    let ran = relay(setup, format!("{over}{at_limit}").as_bytes()).await?;
    assert_eq!(ran.code, 0);
    let answers = ran.lines()?;
    let refused = some(answers.first(), "the refusal")?;
    assert_eq!(
        refused.get("id"),
        None,
        "the id of a line that was dropped is not known"
    );
    assert_eq!(refused.pointer("/error/code"), Some(&json!(-32600)));
    let message = some(
        refused.pointer("/error/message").and_then(Value::as_str),
        "a message",
    )?;
    assert!(message.contains(&format!("{limit} bytes")), "{message}");
    assert_eq!(
        answers.get(1).and_then(|answer| answer.get("id")),
        Some(&json!(1)),
        "the line at the limit is answered"
    );
    let posts = transport.posts()?;
    assert_eq!(posts.len(), 1, "only the line at the limit was forwarded");
    assert_eq!(posts.first().map(|post| post.body.len()), Some(limit));
    Ok(())
}

#[test]
fn mcp_stdio_default_line_limit_is_the_contract_request_body_limit() {
    assert_eq!(Limits::default().line_bytes, MAX_REQUEST_BODY_BYTES);
    assert_eq!(MAX_REQUEST_BODY_BYTES, 16_777_216);
}

#[tokio::test]
async fn a_line_that_is_not_json_gets_a_parse_error_without_an_id() -> TestResult {
    let transport = FakeTransport::echoing();
    let ran = relay(Setup::new(&transport), b"this is not json\n{\"jsonrpc\":\n").await?;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    let answers = ran.lines()?;
    assert_eq!(answers.len(), 2);
    for answer in answers {
        assert_eq!(answer.get("jsonrpc"), Some(&json!("2.0")));
        assert_eq!(answer.get("id"), None);
        assert_eq!(answer.pointer("/error/code"), Some(&json!(-32700)));
    }
    assert_eq!(transport.posts()?, Vec::new(), "nothing was forwarded");
    Ok(())
}

/// JSON that is not one JSON-RPC message is an invalid request, with the id when one can be
/// read, and is never forwarded.
#[tokio::test]
async fn json_that_is_not_a_message_gets_invalid_request() -> TestResult {
    let transport = FakeTransport::echoing();
    let cases = [
        (json!(42), None),
        (json!("text"), None),
        (json!([{"jsonrpc": "2.0", "id": 1, "method": "ping"}]), None),
        (
            json!({"jsonrpc": "1.0", "id": 3, "method": "ping"}),
            Some(json!(3)),
        ),
        (json!({"id": "x", "method": "ping"}), Some(json!("x"))),
        (
            json!({"jsonrpc": "2.0", "id": null, "method": "ping"}),
            None,
        ),
        (
            json!({"jsonrpc": "2.0", "id": {"a": 1}, "method": "ping"}),
            None,
        ),
        (
            json!({"jsonrpc": "2.0", "id": 4, "method": 5}),
            Some(json!(4)),
        ),
        (json!({"jsonrpc": "2.0", "id": 6}), Some(json!(6))),
        (json!({"jsonrpc": "2.0", "result": {}}), None),
        (json!({"jsonrpc": "2.0"}), None),
    ];
    let mut input = String::new();
    for (value, _) in &cases {
        input.push_str(&value.to_string());
        input.push('\n');
    }
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    let answers = ran.lines()?;
    assert_eq!(answers.len(), cases.len());
    for ((sent, id), answer) in cases.iter().zip(&answers) {
        assert_eq!(
            answer.pointer("/error/code"),
            Some(&json!(-32600)),
            "{sent}"
        );
        assert_eq!(answer.get("id"), id.as_ref(), "{sent}");
    }
    assert_eq!(transport.posts()?, Vec::new(), "nothing was forwarded");
    Ok(())
}

#[tokio::test]
async fn blank_lines_carriage_returns_and_a_byte_order_mark_are_tolerated() -> TestResult {
    let transport = FakeTransport::echoing();
    let message = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}";
    let input = format!("\n   \r\n\u{feff}{message}\r\n\n");
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    assert_eq!(ran.lines()?.len(), 1);
    let posts = transport.posts()?;
    assert_eq!(posts.len(), 1);
    assert_eq!(
        posts.first().map(|post| post.body.as_slice()),
        Some(message.as_bytes())
    );
    Ok(())
}

#[tokio::test]
async fn a_last_line_without_a_line_feed_is_still_forwarded() -> TestResult {
    let transport = FakeTransport::echoing();
    let ran = relay(
        Setup::new(&transport),
        b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}",
    )
    .await?;
    assert_eq!(
        ran.lines()?.first().and_then(|answer| answer.get("id")),
        Some(&json!(9))
    );
    Ok(())
}

/// An answer the service wrote over several lines reaches the client as one line.
#[tokio::test]
async fn an_answer_with_line_breaks_is_written_as_one_line() -> TestResult {
    let transport = FakeTransport::scripted(|message| {
        let pretty = serde_json::to_string_pretty(&json!({
            "jsonrpc": "2.0",
            "id": message.get("id"),
            "result": {"text": "two\nlines"}
        }));
        pretty.map_or_else(
            |_| broken("the answer could not be written"),
            |text| status(200, Body::Json(text)),
        )
    });
    let ran = relay(
        Setup::new(&transport),
        request(&json!(1), "ping", &json!({})).as_bytes(),
    )
    .await?;
    assert_eq!(ran.stdout.matches('\n').count(), 1, "{}", ran.stdout);
    assert_eq!(
        ran.lines()?
            .first()
            .and_then(|answer| answer.pointer("/result/text")),
        Some(&json!("two\nlines"))
    );
    Ok(())
}

/// Whatever goes wrong, stdout carries JSON-RPC messages only; every diagnostic is on stderr.
#[tokio::test]
async fn nothing_but_protocol_messages_reaches_stdout() -> TestResult {
    let transport = FakeTransport::probing(
        |post, events| {
            let text = String::from_utf8_lossy(&post.body).into_owned();
            Box::pin(async move {
                if text.contains("notifications/lost") {
                    return broken("the connection was reset").reply;
                }
                if text.contains("notifications/kept") {
                    return accepted().reply;
                }
                // Not a JSON-RPC message, then not JSON at all, then the real answer.
                for event in [
                    "{\"stray\":true}",
                    "plain text",
                    "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}",
                ] {
                    let _ = events.send(event.to_owned()).await;
                }
                status(200, Body::Events).reply
            })
        },
        vec![Probe::Refused],
        Probe::Up,
    );
    let input = format!(
        "{}{}{}",
        notification("notifications/lost"),
        notification("notifications/kept"),
        request(&json!(1), "tools/list", &json!({}))
    );
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!(ran.code, 0);
    let answers = ran.lines()?;
    assert_eq!(answers, [json!({"jsonrpc": "2.0", "id": 1, "result": {}})]);
    for answer in &answers {
        assert_eq!(answer.get("jsonrpc"), Some(&json!("2.0")));
    }
    let notes: Vec<&str> = ran.stderr.lines().collect();
    assert_eq!(notes.len(), 4, "{}", ran.stderr);
    for note in &notes {
        assert!(note.starts_with("okf-jawn mcp: "), "{note}");
    }
    assert!(
        ran.stderr.contains("started the okf-jawn service"),
        "{}",
        ran.stderr
    );
    assert!(ran.stderr.contains("notifications/lost"), "{}", ran.stderr);
    assert_eq!(
        ran.stderr.matches("not a JSON-RPC message").count(),
        2,
        "{}",
        ran.stderr
    );
    Ok(())
}
