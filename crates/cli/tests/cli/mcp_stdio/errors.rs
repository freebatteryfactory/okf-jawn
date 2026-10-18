//! Ruling 5, the error paths: a transport failure answers the request with its id, a failed
//! notification is logged, a refused credential is explained in plain words, and when stdin
//! ends the requests in flight finish before the relay returns 0.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Notify, mpsc};

use super::{notification, request, response};
use crate::check::{TestResult, some};
use crate::support::doubles::{
    Body, Broken, FakeTransport, Setup, accepted, answered, broken, relay, relay_over, status,
};
use crate::support::{Outcome, Ran};

/// The characters ruling C6 names, one or more of each kind: C0 controls (escape, bell,
/// carriage return, tab), delete, C1 controls (CSI, next line), characters that change the
/// direction of text (right-to-left override, left-to-right isolate, pop directional isolate,
/// right-to-left mark, Arabic letter mark) and characters that are invisible (zero-width
/// space, word joiner, byte order mark, soft hyphen, Hangul filler, line separator).
const HIDDEN: [char; 18] = [
    '\u{1b}', '\u{7}', '\r', '\t', '\u{7f}', '\u{9b}', '\u{85}', '\u{202e}', '\u{2066}',
    '\u{2069}', '\u{200f}', '\u{61c}', '\u{200b}', '\u{2060}', '\u{feff}', '\u{ad}', '\u{3164}',
    '\u{2028}',
];

/// How long a test waits for something that a correct relay does at once.
const PATIENCE: Duration = Duration::from_secs(20);

/// Every character of [`HIDDEN`], each as a JSON string spells it with a `\u` escape.
fn hidden_spelled() -> String {
    HIDDEN.iter().fold(String::new(), |mut spelled, letter| {
        // Writing to a `String` cannot fail.
        let _ = write!(spelled, "\\u{:04x}", u32::from(*letter));
        spelled
    })
}

/// Asserts that `said`, what the relay wrote to its error stream, holds no character of
/// [`HIDDEN`] as it is; its own line feeds are the only line breaks in it, one after each
/// diagnostic.
fn hides_nothing(said: &str) {
    for letter in HIDDEN {
        assert!(
            !said.contains(letter),
            "U+{:04X} reached the error stream as it is: {said:?}",
            u32::from(letter)
        );
    }
    assert!(
        said.lines().all(|line| line.starts_with("okf-jawn mcp: ")),
        "{said:?}"
    );
}

/// The only line on stdout, which must be an error for request `id`.
fn only_error(ran: &Ran, id: &Value) -> Outcome<String> {
    let answers = ran.lines()?;
    assert_eq!(answers.len(), 1, "{}", ran.stdout);
    let answer = some(answers.first(), "an answer")?;
    assert_eq!(answer.get("jsonrpc"), Some(&json!("2.0")));
    assert_eq!(answer.get("id"), Some(id));
    assert_eq!(answer.get("result"), None);
    assert_eq!(answer.pointer("/error/code"), Some(&json!(-32603)));
    Ok(some(
        answer.pointer("/error/message").and_then(Value::as_str),
        "an error message",
    )?
    .to_owned())
}

/// A request that fails in transport is answered with a JSON-RPC error carrying its id,
/// whether the id is a number or a string.
#[tokio::test]
async fn a_transport_failure_answers_the_request_with_its_id() -> TestResult {
    for id in [json!(41), json!("call-7")] {
        let transport = FakeTransport::scripted(|_| broken("connection reset by peer"));
        let ran = relay(
            Setup::new(&transport),
            request(&id, "tools/call", &json!({})).as_bytes(),
        )
        .await?;
        assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
        let message = only_error(&ran, &id)?;
        assert!(message.contains("connection reset by peer"), "{message}");
    }
    Ok(())
}

#[tokio::test]
async fn a_failed_notification_is_logged_and_nothing_is_written() -> TestResult {
    let transport = FakeTransport::scripted(|message| {
        if message.get("method") == Some(&json!("notifications/cancelled")) {
            broken("connection reset by peer")
        } else if message.get("method").is_some() {
            status(503, Body::Other)
        } else {
            status(
                400,
                Body::Json(
                    json!({"jsonrpc": "2.0", "error": {"code": -32020, "message": "bad header"}})
                        .to_string(),
                ),
            )
        }
    });
    let input = format!(
        "{}{}{}\n",
        notification("notifications/cancelled"),
        notification("notifications/roots/list_changed"),
        json!({"jsonrpc": "2.0", "id": "srv-1", "result": {}})
    );
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!((ran.code, ran.stdout.as_str()), (0, ""));
    let notes: Vec<&str> = ran.stderr.lines().collect();
    assert_eq!(notes.len(), 3, "{}", ran.stderr);
    assert!(
        ran.stderr
            .contains("the notification \"notifications/cancelled\" was not delivered")
            && ran.stderr.contains("connection reset by peer"),
        "{}",
        ran.stderr
    );
    assert!(
        ran.stderr
            .contains("the notification \"notifications/roots/list_changed\" was not delivered")
            && ran.stderr.contains("HTTP 503"),
        "{}",
        ran.stderr
    );
    assert!(
        ran.stderr
            .contains("a response to the service was not delivered")
            && ran.stderr.contains("HTTP 400: bad header"),
        "{}",
        ran.stderr
    );
    Ok(())
}

/// A refused credential is a JSON-RPC error in plain words with the request's id; whatever
/// the service's body says, the credential is not in it.
#[tokio::test]
async fn a_refused_credential_is_explained_in_plain_words() -> TestResult {
    for (code, said) in [
        (401, "refused the connector credential"),
        (403, "does not grant it"),
    ] {
        let transport = FakeTransport::scripted(move |_| {
            status(
                code,
                Body::Json(json!({"jsonrpc": "2.0", "id": 5, "error": {"code": -1, "message": crate::support::TOKEN}}).to_string()),
            )
        });
        let ran = relay(
            Setup::new(&transport),
            request(&json!(5), "tools/list", &json!({})).as_bytes(),
        )
        .await?;
        assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
        let message = only_error(&ran, &json!(5))?;
        assert!(
            message.contains(said) && message.contains(&format!("HTTP {code}")),
            "{message}"
        );
        assert!(
            !ran.stdout.contains(crate::support::TOKEN),
            "{}",
            ran.stdout
        );
    }
    Ok(())
}

/// Stdin ends while a request is still in flight: the relay does not return until the answer
/// has been written, and then returns 0.
#[tokio::test]
async fn when_stdin_ends_the_requests_in_flight_finish_and_the_exit_code_is_0() -> TestResult {
    let release = Arc::new(Notify::new());
    let (arrived, mut arriving) = mpsc::unbounded_channel::<()>();
    let gate = release.clone();
    let transport = FakeTransport::new(move |_, _| {
        let (gate, arrived) = (gate.clone(), arrived.clone());
        Box::pin(async move {
            let _ = arrived.send(());
            gate.notified().await;
            answered(&response(&json!(1), &json!({"late": true}))).reply
        })
    });
    let input = request(&json!(1), "tools/call", &json!({}));
    let mut running = Box::pin(relay(Setup::new(&transport), input.as_bytes()));
    let ran = tokio::time::timeout(PATIENCE, async {
        // The request is in flight and stdin, a finished slice, is at its end.
        tokio::select! {
            biased;
            early = &mut running => return Err(format!("the relay returned early: {:?}", early.map(|ran| ran.stdout))),
            sent = arriving.recv() => assert_eq!(sent, Some(())),
        }
        for _ in 0..200 {
            tokio::select! {
                biased;
                early = &mut running => return Err(format!("the relay returned early: {:?}", early.map(|ran| ran.stdout))),
                () = tokio::task::yield_now() => {}
            }
        }
        release.notify_one();
        running.await.map_err(|error| error.to_string())
    })
    .await??;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    assert_eq!(ran.lines()?, [response(&json!(1), &json!({"late": true}))]);
    Ok(())
}

/// A stream that ends, or breaks, before the response still leaves the request answered; the
/// events that came first stay relayed.
#[tokio::test]
async fn a_stream_without_a_response_still_answers_the_request() -> TestResult {
    let progress = json!({"jsonrpc": "2.0", "method": "notifications/progress"});
    // The service asks the client something under the id the client's own request has: ids
    // are numbered by each side, so this is a request, not the answer to request 8.
    let asked = json!({"jsonrpc": "2.0", "id": 8, "method": "roots/list"});
    // An answer to some other request is not the answer to request 8 either.
    let other = json!({"jsonrpc": "2.0", "id": 9, "result": {}});
    for cut_off in [false, true] {
        let events_sent = [progress.to_string(), asked.to_string(), other.to_string()];
        let transport = FakeTransport::new(move |_, events| {
            let events_sent = events_sent.clone();
            Box::pin(async move {
                for event in events_sent {
                    let _ = events.send(event).await;
                }
                if cut_off {
                    broken("the stream was cut off").reply
                } else {
                    status(200, Body::Events).reply
                }
            })
        });
        let ran = relay(
            Setup::new(&transport),
            request(&json!(8), "tools/call", &json!({})).as_bytes(),
        )
        .await?;
        let answers = ran.lines()?;
        assert_eq!(answers.len(), 4, "{}", ran.stdout);
        assert_eq!(
            answers.iter().take(3).collect::<Vec<_>>(),
            [&progress, &asked, &other]
        );
        let last = some(answers.get(3), "the error")?;
        assert_eq!(
            (last.get("id"), last.pointer("/error/code")),
            (Some(&json!(8)), Some(&json!(-32603)))
        );
        let message = some(
            last.pointer("/error/message").and_then(Value::as_str),
            "a message",
        )?;
        assert_eq!(
            message.contains("the stream was cut off"),
            cut_off,
            "{message}"
        );
    }
    Ok(())
}

/// Each answer that is not a response leaves the request answered once, with what happened.
#[tokio::test]
async fn an_answer_that_is_no_response_is_explained_with_the_request_id() -> TestResult {
    let too_large =
        json!({"code": "too_large", "message": "Request body exceeds the 16 MiB limit"});
    let cases = [
        (
            status(202, Body::Empty),
            "without a response to this request",
        ),
        (status(200, Body::Other), "neither JSON nor an event stream"),
        (
            status(200, Body::Json("{\"ok\":true}".to_owned())),
            "not a JSON-RPC message",
        ),
        (
            status(200, Body::Json("not json".to_owned())),
            "not a JSON-RPC message",
        ),
        (status(307, Body::Empty), "HTTP 307"),
        (status(500, Body::Other), "HTTP 500"),
        (status(502, Body::Json("<html>".to_owned())), "HTTP 502"),
        (
            status(413, Body::Json(too_large.to_string())),
            "HTTP 413 (too_large): Request body exceeds the 16 MiB limit",
        ),
    ];
    for (script, said) in cases {
        let script = std::sync::Mutex::new(Some(script));
        let transport = FakeTransport::scripted(move |_| {
            script
                .lock()
                .ok()
                .and_then(|mut script| script.take())
                .unwrap_or_else(|| broken("asked twice"))
        });
        let ran = relay(
            Setup::new(&transport),
            request(&json!(3), "tools/call", &json!({})).as_bytes(),
        )
        .await?;
        assert_eq!(ran.code, 0);
        let message = only_error(&ran, &json!(3))?;
        assert!(message.contains(said), "{said}: {message}");
    }
    Ok(())
}

/// The service refuses a request with a JSON-RPC error and a refusal status, leaving the id
/// out when it refused before reading it: the error is the request's answer, relayed once
/// with the request's id and the service's own code.
#[tokio::test]
async fn a_json_rpc_error_with_a_refusal_status_is_relayed_with_the_request_id() -> TestResult {
    for (sent_id, code) in [
        (None, 400),
        (Some(json!(12)), 404),
        (Some(Value::Null), 400),
    ] {
        let transport = FakeTransport::scripted(move |_| {
            let mut error = json!({"jsonrpc": "2.0", "error": {"code": -32020, "message": "missing required Mcp-Method header"}});
            if let (Some(id), Some(object)) = (&sent_id, error.as_object_mut()) {
                object.insert("id".to_owned(), id.clone());
            }
            status(code, Body::Json(error.to_string()))
        });
        let ran = relay(
            Setup::new(&transport),
            request(&json!(12), "tools/list", &json!({})).as_bytes(),
        )
        .await?;
        assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
        assert_eq!(
            ran.lines()?,
            [
                json!({"jsonrpc": "2.0", "id": 12, "error": {"code": -32020, "message": "missing required Mcp-Method header"}})
            ]
        );
    }
    Ok(())
}

/// The client is gone: the first write fails, and the relay stops with 1 instead of reading on.
#[tokio::test]
async fn a_failed_output_stream_stops_the_relay_with_exit_code_1() -> TestResult {
    let transport = FakeTransport::echoing();
    let input = format!(
        "{}{}",
        request(&json!(1), "ping", &json!({})),
        request(&json!(2), "ping", &json!({}))
    );
    let (code, _) = tokio::time::timeout(
        PATIENCE,
        relay_over(Setup::new(&transport), input.as_bytes(), Broken),
    )
    .await??;
    assert_eq!(code, 1);
    Ok(())
}

/// The client stopped reading but keeps its end of stdin open: the relay does not wait for
/// input that will never be answered.
#[tokio::test]
async fn a_failed_output_stream_stops_the_relay_while_stdin_is_still_open() -> TestResult {
    let transport = FakeTransport::echoing();
    let (mut client, stdin) = tokio::io::duplex(4096);
    client
        .write_all(request(&json!(1), "ping", &json!({})).as_bytes())
        .await?;
    let (code, stderr) =
        tokio::time::timeout(PATIENCE, relay_over(Setup::new(&transport), stdin, Broken)).await??;
    assert_eq!(code, 1);
    assert!(stderr.contains("the output stream failed"), "{stderr}");
    drop(client);
    Ok(())
}

/// C6: the diagnostic for a notification that was not delivered quotes its method cut to 200
/// characters, with a mark that counts the rest, and with every control character, every
/// character that changes the direction of text and every invisible one written as an escape.
/// The method is the client's: of a megabyte, and beginning with each such character.
#[tokio::test]
async fn a_notification_that_was_not_delivered_is_named_in_bounded_and_visible_words() -> TestResult
{
    let long = 1_048_576_usize;
    let method = format!("{}{}", hidden_spelled(), "m".repeat(long));
    let transport = FakeTransport::scripted(|_| broken("connection reset by peer"));
    let input = format!("{{\"jsonrpc\":\"2.0\",\"method\":\"{method}\"}}\n");
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!((ran.code, ran.stdout.as_str()), (0, ""));
    hides_nothing(&ran.stderr);
    let rest = long.saturating_add(HIDDEN.len()).saturating_sub(200);
    assert!(
        ran.stderr.contains(&format!(
            "{}\" [cut: {rest} more characters] was not delivered",
            "m".repeat(200_usize.saturating_sub(HIDDEN.len()))
        )),
        "{}",
        ran.stderr.chars().take(800).collect::<String>()
    );
    assert!(ran.stderr.len() < 2048, "{} bytes", ran.stderr.len());
    Ok(())
}

/// C6: the same for a notification the relay does not send (its method cannot travel in the
/// `Mcp-Method` header that protocol `2026-07-28` asks for).
#[tokio::test]
async fn a_notification_that_was_not_sent_is_named_in_bounded_and_visible_words() -> TestResult {
    let modern = json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}});
    let transport = FakeTransport::echoing();
    let method = format!("{}{}", hidden_spelled(), "n".repeat(300));
    let input = format!(
        "{}{{\"jsonrpc\":\"2.0\",\"method\":\"{method}\"}}\n",
        request(&json!(1), "tools/list", &modern)
    );
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!(ran.code, 0);
    hides_nothing(&ran.stderr);
    let rest = 300_usize.saturating_add(HIDDEN.len()).saturating_sub(200);
    assert!(
        ran.stderr.contains(&format!(
            "{}\" [cut: {rest} more characters] was not sent",
            "n".repeat(200_usize.saturating_sub(HIDDEN.len()))
        )),
        "{}",
        ran.stderr
    );
    assert_eq!(transport.posts()?.len(), 1, "the notification was not sent");
    Ok(())
}

/// C6: what the service says is escaped too when the relay repeats it in a diagnostic: a
/// refusal of a notification whose message holds each such character, as JSON escapes that
/// the relay reads into the characters themselves.
#[tokio::test]
async fn what_the_service_says_reaches_the_error_stream_only_as_visible_text() -> TestResult {
    let message = format!("left {} right", hidden_spelled());
    let body =
        format!("{{\"jsonrpc\":\"2.0\",\"error\":{{\"code\":-32602,\"message\":\"{message}\"}}}}");
    let transport = FakeTransport::scripted(move |_| status(400, Body::Json(body.clone())));
    let ran = relay(
        Setup::new(&transport),
        notification("notifications/progress").as_bytes(),
    )
    .await?;
    assert_eq!((ran.code, ran.stdout.as_str()), (0, ""));
    hides_nothing(&ran.stderr);
    assert_eq!(ran.stderr.lines().count(), 1, "{}", ran.stderr);
    assert!(
        ran.stderr.contains("HTTP 400: left \\u{1b}\\u{7}") && ran.stderr.contains(" right\n"),
        "{}",
        ran.stderr
    );
    for letter in HIDDEN {
        assert!(
            ran.stderr.contains(&letter.escape_unicode().to_string()),
            "U+{:04X}: {}",
            u32::from(letter),
            ran.stderr
        );
    }
    Ok(())
}

/// An accepted notification after a failure shows the relay carries on: one bad exchange
/// does not end the session.
#[tokio::test]
async fn the_relay_carries_on_after_a_failed_exchange() -> TestResult {
    let transport = FakeTransport::scripted(|message| match message.get("id") {
        Some(id) if id == &json!(1) => broken("connection reset by peer"),
        Some(id) => answered(&response(id, &json!({}))),
        None => accepted(),
    });
    let input = format!(
        "{}{}{}",
        request(&json!(1), "tools/call", &json!({})),
        notification("notifications/progress"),
        request(&json!(2), "tools/call", &json!({}))
    );
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    let mut ids: Vec<String> = ran
        .lines()?
        .iter()
        .filter_map(|answer| answer.get("id").map(Value::to_string))
        .collect();
    ids.sort();
    assert_eq!(ids, ["1", "2"]);
    assert_eq!(transport.posts()?.len(), 3);
    Ok(())
}
