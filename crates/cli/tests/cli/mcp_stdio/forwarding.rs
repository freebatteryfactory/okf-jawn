//! Ruling 2, forwarding: one `POST` per message with the headers Streamable HTTP requires, a
//! JSON body or an event stream relayed one message per line, nothing for an accepted
//! notification, and concurrent forwarding.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use okf_jawn_cli::relay::Limits;
use serde_json::{Value, json};
use tokio::sync::{Notify, Semaphore, mpsc};

use super::{notification, request, response};
use crate::check::{TestResult, some};
use crate::support::Outcome;
use crate::support::doubles::{
    Body, FakeTransport, Piped, Reply, Setup, accepted, answered, header, relay, relay_over,
    status, stream,
};

/// How long a test waits for something that a correct relay does at once.
const PATIENCE: Duration = Duration::from_secs(20);

/// The request `_meta` that names a per-request protocol version.
fn meta(version: &str) -> Value {
    json!({"io.modelcontextprotocol/protocolVersion": version})
}

/// The id of a sent message, as text.
fn id_of(body: &[u8]) -> Outcome<String> {
    let message = serde_json::from_slice::<Value>(body)?;
    Ok(some(message.get("id"), "an id")?.to_string())
}

#[tokio::test]
async fn an_event_stream_is_relayed_one_message_per_line_in_order() -> TestResult {
    let progress =
        json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progress": 1}});
    let asked = json!({"jsonrpc": "2.0", "id": "srv-1", "method": "roots/list"});
    let done = response(&json!(1), &json!({"content": []}));
    let events = [progress.clone(), asked.clone(), done.clone()];
    let transport = FakeTransport::scripted(move |_| stream(&events));
    let ran = relay(
        Setup::new(&transport),
        request(&json!(1), "tools/call", &json!({})).as_bytes(),
    )
    .await?;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    assert_eq!(ran.lines()?, [progress, asked, done]);
    Ok(())
}

/// An event reaches the client while the stream is still open: the service's final answer is
/// held back until the first event has been seen on stdout.
#[tokio::test]
async fn an_event_reaches_the_client_before_the_stream_ends() -> TestResult {
    let seen = Arc::new(Notify::new());
    let gate = seen.clone();
    let transport = FakeTransport::new(move |_, events| {
        let gate = gate.clone();
        Box::pin(async move {
            let first = json!({"jsonrpc": "2.0", "method": "notifications/progress"});
            let _ = events.send(first.to_string()).await;
            gate.notified().await;
            let _ = events
                .send(response(&json!(1), &json!({})).to_string())
                .await;
            status(200, Body::Events).reply
        })
    });
    let (written, mut reading) = mpsc::unbounded_channel();
    let input = request(&json!(1), "tools/call", &json!({}));
    let running = relay_over(Setup::new(&transport), input.as_bytes(), Piped(written));
    let watching = async {
        let first = some(reading.recv().await, "a first line")?;
        assert!(String::from_utf8(first)?.contains("notifications/progress"));
        // The line feed that ends a line is written after it.
        assert_eq!(reading.recv().await.as_deref(), Some(b"\n".as_slice()));
        seen.notify_one();
        let last = some(reading.recv().await, "a last line")?;
        assert!(String::from_utf8(last)?.contains("\"result\""));
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let (ran, watched) =
        tokio::time::timeout(PATIENCE, async { tokio::join!(running, watching) }).await?;
    watched?;
    assert_eq!(ran?.0, 0);
    Ok(())
}

#[tokio::test]
async fn an_accepted_notification_and_a_client_response_write_nothing() -> TestResult {
    let transport = FakeTransport::scripted(|_| accepted());
    let reply = json!({"jsonrpc": "2.0", "id": "srv-1", "result": {"roots": []}});
    let failed =
        json!({"jsonrpc": "2.0", "id": "srv-2", "error": {"code": -32601, "message": "no"}});
    let input = format!(
        "{}{reply}\n{failed}\n",
        notification("notifications/initialized")
    );
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!(
        (ran.code, ran.stdout.as_str(), ran.stderr.as_str()),
        (0, "", "")
    );
    assert_eq!(transport.posts()?.len(), 3, "each of them is one POST");
    Ok(())
}

/// Request `a` is answered only once request `b` has reached the service, so both must be in
/// flight together; a relay that forwards one message at a time never finishes.
#[tokio::test]
async fn requests_are_forwarded_concurrently() -> TestResult {
    let arrived = Arc::new(Notify::new());
    let transport = FakeTransport::new(move |post, _| {
        let arrived = arrived.clone();
        Box::pin(async move {
            let id = serde_json::from_slice::<Value>(&post.body)
                .ok()
                .and_then(|message| message.get("id").cloned())
                .unwrap_or(Value::Null);
            if id == json!("a") {
                arrived.notified().await;
            } else {
                arrived.notify_one();
            }
            answered(&response(&id, &json!({}))).reply
        })
    });
    let input = format!(
        "{}{}",
        request(&json!("a"), "tools/call", &json!({})),
        request(&json!("b"), "tools/call", &json!({}))
    );
    let ran =
        tokio::time::timeout(PATIENCE, relay(Setup::new(&transport), input.as_bytes())).await??;
    let ids: Vec<Value> = ran
        .lines()?
        .iter()
        .filter_map(|answer| answer.get("id").cloned())
        .collect();
    assert_eq!(
        ids,
        [json!("b"), json!("a")],
        "b was answered while a was still in flight"
    );
    Ok(())
}

/// With a limit of two, a third message is not sent while two are in flight.
#[tokio::test]
async fn no_more_than_the_limit_is_in_flight_at_once() -> TestResult {
    let active = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(Semaphore::new(0));
    let (started, mut starting) = mpsc::unbounded_channel::<()>();
    let (counting, peak, held) = (active.clone(), most.clone(), gate.clone());
    let transport = FakeTransport::new(move |post, _| {
        let (counting, peak, held, started) = (
            counting.clone(),
            peak.clone(),
            held.clone(),
            started.clone(),
        );
        Box::pin(async move {
            let now = counting.fetch_add(1, Ordering::SeqCst).saturating_add(1);
            peak.fetch_max(now, Ordering::SeqCst);
            let _ = started.send(());
            let permit = held.acquire().await;
            drop(permit);
            counting.fetch_sub(1, Ordering::SeqCst);
            let id = serde_json::from_slice::<Value>(&post.body)
                .ok()
                .and_then(|message| message.get("id").cloned())
                .unwrap_or(Value::Null);
            answered(&response(&id, &json!({}))).reply
        })
    });
    let mut setup = Setup::new(&transport);
    setup.limits = Limits {
        in_flight: 2,
        ..Limits::default()
    };
    let mut input = String::new();
    for id in 1..=5 {
        input.push_str(&request(&json!(id), "tools/call", &json!({})));
    }
    let running = relay(setup, input.as_bytes());
    let releasing = async {
        // Two messages are in flight; give the relay every chance to send a third.
        starting.recv().await;
        starting.recv().await;
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        gate.add_permits(5);
    };
    let (ran, ()) =
        tokio::time::timeout(PATIENCE, async { tokio::join!(running, releasing) }).await?;
    assert_eq!(ran?.lines()?.len(), 5);
    assert_eq!(most.load(Ordering::SeqCst), 2);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    Ok(())
}

/// `initialize` carries no protocol version and is answered before the next message is sent;
/// every later message carries the version its answer named.
#[tokio::test]
async fn initialize_is_exchanged_alone_and_its_answer_sets_the_protocol_version() -> TestResult {
    let transport =
        FakeTransport::scripted(
            |message| match message.get("method").and_then(Value::as_str) {
                Some("initialize") => answered(&response(
                    &json!(0),
                    &json!({"protocolVersion": "2025-06-18", "capabilities": {}}),
                )),
                Some("tools/list") => answered(&response(&json!(1), &json!({"tools": []}))),
                _ => accepted(),
            },
        );
    let input = format!(
        "{}{}{}{}\n",
        request(
            &json!(0),
            "initialize",
            &json!({"protocolVersion": "2025-11-25"})
        ),
        notification("notifications/initialized"),
        request(&json!(1), "tools/list", &json!({})),
        json!({"jsonrpc": "2.0", "id": "srv-1", "result": {}})
    );
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    let posts = transport.posts()?;
    assert_eq!(posts.len(), 4);
    let first = some(posts.first(), "the initialize POST")?;
    assert!(first.headers.is_empty(), "{:?}", first.headers);
    for later in posts.iter().skip(1) {
        assert_eq!(
            later.headers,
            [("MCP-Protocol-Version", "2025-06-18".to_owned())],
            "below 2026-07-28 the version is the only MCP header"
        );
    }
    Ok(())
}

/// Before any version is known, a message carries no MCP header at all.
#[tokio::test]
async fn a_message_before_any_version_is_known_carries_no_mcp_header() -> TestResult {
    let transport = FakeTransport::echoing();
    let input = format!(
        "{}{}",
        request(&json!(1), "ping", &json!({})),
        notification("notifications/x")
    );
    relay(Setup::new(&transport), input.as_bytes()).await?;
    for post in transport.posts()? {
        assert!(post.headers.is_empty(), "{:?}", post.headers);
    }
    Ok(())
}

/// From protocol `2026-07-28` a request names its version in `_meta`, and the `POST` repeats
/// the version, the method and the name in headers; the version then applies to what follows.
#[tokio::test]
async fn a_request_meta_version_sets_the_standard_headers() -> TestResult {
    let transport = FakeTransport::echoing();
    let modern = meta("2026-07-28");
    let input = format!(
        "{}{}{}{}{}{}{}\n{}",
        request(
            &json!(1),
            "tools/call",
            &json!({"name": "grep", "_meta": modern})
        ),
        request(
            &json!(2),
            "resources/read",
            &json!({"uri": "ui://okf-jawn/app.html", "_meta": modern})
        ),
        request(
            &json!(3),
            "prompts/get",
            &json!({"name": "a prompt", "_meta": modern})
        ),
        request(
            &json!(4),
            "tasks/get",
            &json!({"taskId": "t-1", "_meta": modern})
        ),
        request(&json!(5), "tools/list", &json!({"_meta": modern})),
        notification("notifications/cancelled"),
        json!({"jsonrpc": "2.0", "id": "srv-1", "result": {}}),
        request(
            &json!(6),
            "tools/call",
            &json!({"name": "ls", "_meta": meta("2025-11-25")})
        ),
    );
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    let mut by_body = std::collections::BTreeMap::new();
    for post in transport.posts()? {
        let message = serde_json::from_slice::<Value>(&post.body)?;
        let key = message
            .get("id")
            .map_or_else(|| "notification".to_owned(), Value::to_string);
        by_body.insert(key, post);
    }
    let expect =
        |key: &str, version: &str, method: Option<&str>, name: Option<&str>| -> TestResult {
            let post = some(by_body.get(key), key)?;
            assert_eq!(header(post, "MCP-Protocol-Version"), Some(version), "{key}");
            assert_eq!(header(post, "Mcp-Method"), method, "{key}");
            assert_eq!(header(post, "Mcp-Name"), name, "{key}");
            Ok(())
        };
    expect("1", "2026-07-28", Some("tools/call"), Some("grep"))?;
    expect(
        "2",
        "2026-07-28",
        Some("resources/read"),
        Some("ui://okf-jawn/app.html"),
    )?;
    expect("3", "2026-07-28", Some("prompts/get"), Some("a prompt"))?;
    expect("4", "2026-07-28", Some("tasks/get"), Some("t-1"))?;
    expect("5", "2026-07-28", Some("tools/list"), None)?;
    expect(
        "notification",
        "2026-07-28",
        Some("notifications/cancelled"),
        None,
    )?;
    expect("\"srv-1\"", "2026-07-28", None, None)?;
    expect("6", "2025-11-25", None, None)?;
    Ok(())
}

/// The Base64 header form is not built: a name that needs it is refused with the request's
/// id and never sent, and a notification that cannot be sent is logged.
#[tokio::test]
async fn a_name_or_method_that_cannot_travel_in_a_header_is_refused() -> TestResult {
    let transport = FakeTransport::echoing();
    let modern = meta("2026-07-28");
    let input = format!(
        "{}{}{}{}{}",
        request(
            &json!(1),
            "tools/call",
            &json!({"name": "caf\u{e9}", "_meta": modern})
        ),
        request(
            &json!(2),
            "tools/call",
            &json!({"name": " padded", "_meta": modern})
        ),
        request(
            &json!(3),
            "tools/call",
            &json!({"name": "=?base64?eA==?=", "_meta": modern})
        ),
        request(
            &json!(4),
            "tools/list",
            &json!({"_meta": meta("2026-07-28\r\nX-Injected: 1")})
        ),
        notification("notifications/line\nbreak"),
    );
    let ran = relay(Setup::new(&transport), input.as_bytes()).await?;
    assert_eq!(ran.code, 0);
    let answers = ran.lines()?;
    assert_eq!(answers.len(), 4);
    for (answer, id) in answers.iter().zip(1..=4) {
        assert_eq!(answer.get("id"), Some(&json!(id)));
        assert_eq!(answer.pointer("/error/code"), Some(&json!(-32600)));
    }
    assert!(ran.stderr.contains("was not sent"), "{}", ran.stderr);
    assert_eq!(transport.posts()?, Vec::new(), "nothing was forwarded");
    Ok(())
}

/// A session the service issues at `initialize` is carried on every later message, and a new
/// `initialize` replaces the session and the version with what its own answer carries.
#[tokio::test]
async fn a_session_the_service_issues_is_carried_on_later_messages() -> TestResult {
    let transport = FakeTransport::scripted(|message| {
        if message.get("method") != Some(&json!("initialize")) {
            return answered(&response(&json!(1), &json!({})));
        }
        if message.pointer("/params/refused") == Some(&json!(true)) {
            return answered(&json!({
                "jsonrpc": "2.0",
                "id": 0,
                "error": {"code": -32602, "message": "Unsupported protocol version"}
            }));
        }
        if message.pointer("/params/first") != Some(&json!(true)) {
            return answered(&response(
                &json!(0),
                &json!({"protocolVersion": "2025-03-26"}),
            ));
        }
        let mut script = answered(&response(
            &json!(0),
            &json!({"protocolVersion": "2025-06-18"}),
        ));
        script.reply = script.reply.map(|reply| Reply {
            session: Some("session-7".to_owned()),
            ..reply
        });
        script
    });
    let input = format!(
        "{}{}{}{}{}{}",
        request(&json!(0), "initialize", &json!({"first": true})),
        request(&json!(1), "tools/list", &json!({})),
        request(&json!(0), "initialize", &json!({})),
        request(&json!(1), "tools/list", &json!({})),
        request(&json!(0), "initialize", &json!({"refused": true})),
        request(&json!(1), "tools/list", &json!({})),
    );
    relay(Setup::new(&transport), input.as_bytes()).await?;
    let posts = transport.posts()?;
    let sessions: Vec<Option<&str>> = posts
        .iter()
        .map(|post| header(post, "Mcp-Session-Id"))
        .collect();
    assert_eq!(sessions, [None, Some("session-7"), None, None, None, None]);
    let versions: Vec<Option<&str>> = posts
        .iter()
        .map(|post| header(post, "MCP-Protocol-Version"))
        .collect();
    assert_eq!(
        versions,
        [
            None,
            Some("2025-06-18"),
            None,
            Some("2025-03-26"),
            None,
            None
        ],
        "after a handshake the service refused, no version is in force"
    );
    assert_eq!(id_of(&some(posts.get(1), "the second POST")?.body)?, "1");
    Ok(())
}

/// The relay sends no `Mcp-Param-*` header. That is right only while no tool of the service
/// declares one; this fails the day the committed catalog gains an `x-mcp-header`.
#[test]
fn mcp_stdio_tool_catalog_declares_no_parameter_headers() -> TestResult {
    let catalog = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/mcp-tools.json"),
    )?;
    let tools = serde_json::from_str::<Value>(&catalog)?;
    assert!(some(tools.get("tools").and_then(Value::as_array), "the tools")?.len() > 5);
    assert!(!catalog.contains("x-mcp-header"));
    Ok(())
}
