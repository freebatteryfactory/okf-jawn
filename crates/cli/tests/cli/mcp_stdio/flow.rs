//! What the relay holds and how it ends (the review's notes N5 and N6, the re-review's F1).
//!
//! Everything on its way out holds room in one budget counted in bytes, from the moment the
//! service's double hands it over, as the transport takes room before it reads: a shared part,
//! and one text more that did not find its room there. While that place is taken the service
//! can hand over nothing more that needs it, and the relay forwards nothing more from its
//! input. When the input ends, the messages in flight get a bounded time; when the output
//! fails, the relay stops whatever it was waiting for.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use okf_jawn_cli::relay::Limits;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
use tokio::sync::watch;

use super::{notification, request, response};
use crate::check::{TestResult, some};
use crate::support::doubles::{
    Body, Broken, FakeTransport, Script, Setup, Stuck, accepted, answered, broken, relay, relay_on,
    relay_over, status,
};
use crate::support::{Outcome, TOKEN};

/// An input that hands out its bytes, counts how many were taken, and then stays open.
struct Counted {
    bytes: Vec<u8>,
    taken: Arc<AtomicUsize>,
}

/// An input that fails every read.
struct Failing;

/// The length of every line the service's double answers with in the tests of the room.
const LINE: usize = 200;

/// How long a test waits for a relay that must end by itself.
const PATIENCE: Duration = Duration::from_secs(30);

/// How often a test lets every other task run before it looks at what they did: far more
/// often than the few steps anything here needs before it has to wait.
const TURNS: usize = 500;

impl AsyncRead for Counted {
    fn poll_read(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let from = this.taken.load(Ordering::SeqCst);
        let rest = this.bytes.get(from..).unwrap_or_default();
        let part = rest.get(..buffer.remaining()).unwrap_or(rest);
        if part.is_empty() {
            // Open and silent, as a client is that writes nothing more: nobody is woken.
            return Poll::Pending;
        }
        buffer.put_slice(part);
        this.taken.fetch_add(part.len(), Ordering::SeqCst);
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for Failing {
    fn poll_read(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::ErrorKind::ConnectionReset.into()))
    }
}

/// A response to `id`, or a notification without one, that is exactly `LINE` bytes as a line.
fn sized(id: Option<&Value>) -> Value {
    let with = |pad: &str| match id {
        Some(id) => response(id, &json!({"pad": pad})),
        None => {
            json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"pad": pad}})
        }
    };
    let message = with(&"x".repeat(LINE.saturating_sub(with("").to_string().len())));
    assert_eq!(message.to_string().len(), LINE);
    message
}

/// One request whose answer is a stream of eight events of `LINE` bytes each, relayed into an
/// output that takes one byte and then nothing. Returns how many events the service could
/// hand over while the output stood still, and how many lines came out once it was read.
async fn handed_over_while_the_output_stands_still(room: usize) -> Outcome<(usize, usize)> {
    const EVENTS: usize = 8;
    let handed = Arc::new(AtomicUsize::new(0));
    let counted = handed.clone();
    let transport = FakeTransport::new(move |_, events| {
        let counted = counted.clone();
        Box::pin(async move {
            for _ in 0..EVENTS {
                if events.send(sized(None).to_string()).await.is_err() {
                    return broken("the relay stopped taking events").reply;
                }
                counted.fetch_add(1, Ordering::SeqCst);
            }
            answered(&response(&json!(1), &json!({}))).reply
        })
    });
    let setup = Setup {
        limits: Limits {
            waiting_bytes: room,
            ..Limits::default()
        },
        ..Setup::new(&transport)
    };
    let (stdout, mut client) = tokio::io::duplex(1);
    let input = request(&json!(1), "tools/call", &json!({}));
    let mut running = Box::pin(relay_over(setup, input.as_bytes(), stdout));
    for _ in 0..TURNS {
        tokio::select! {
            biased;
            early = &mut running => {
                return Err(format!("the relay returned while its output stood still: {early:?}").into());
            }
            () = tokio::task::yield_now() => {}
        }
    }
    let stood = handed.load(Ordering::SeqCst);
    let mut written = Vec::new();
    let (ran, read) = tokio::join!(running, client.read_to_end(&mut written));
    ran?;
    read?;
    Ok((stood, String::from_utf8(written)?.lines().count()))
}

/// Requests arrive one by one while the output stands still, each answered at once with a
/// line of `LINE` bytes. Returns how many the relay forwarded before it stopped.
async fn forwarded_while_the_output_stands_still(room: usize) -> Outcome<usize> {
    let transport = FakeTransport::scripted(|message| answered(&sized(message.get("id"))));
    let setup = Setup {
        limits: Limits {
            waiting_bytes: room,
            ..Limits::default()
        },
        ..Setup::new(&transport)
    };
    let (stdout, mut client_out) = tokio::io::duplex(1);
    let (mut client_in, stdin) = tokio::io::duplex(65_536);
    let mut running = Box::pin(relay_over(setup, stdin, stdout));
    let mut forwarded = 0;
    for id in 1..=4 {
        client_in
            .write_all(request(&json!(id), "tools/call", &json!({})).as_bytes())
            .await?;
        for _ in 0..TURNS {
            tokio::select! {
                biased;
                early = &mut running => {
                    return Err(format!("the relay returned while its output stood still: {early:?}").into());
                }
                () = tokio::task::yield_now() => {}
            }
        }
        forwarded = transport.posts()?.len();
    }
    drop(client_in);
    let mut written = Vec::new();
    let (ran, read) = tokio::join!(running, client_out.read_to_end(&mut written));
    ran?;
    read?;
    assert_eq!(String::from_utf8(written)?.lines().count(), 4);
    assert_eq!(transport.posts()?.len(), 4);
    Ok(forwarded)
}

/// What the relay says of the lines the output had not taken when it gives up.
fn unwritten(lines: usize) -> String {
    format!(
        "okf-jawn mcp: the input ended and {lines} of the lines for the output had not been \
         written 300ms later; the relay stops without writing them\n"
    )
}

/// What the relay says of the messages it leaves unanswered when it gives up.
fn unfinished(names: &[&str]) -> String {
    format!(
        "okf-jawn mcp: the input ended and {} of its messages had not finished 300ms later; the \
         relay stops and leaves them unanswered: {}\n",
        names.len(),
        names.join(", ")
    )
}

/// A relay whose output takes nothing is fed one request for each of `answers`, one at a
/// time, each only after the answer before it had its room; then `last`; and then its input
/// ends, or fails when `fails`. The closing time is 300 ms and a line may be 1024 bytes.
/// Returns the exit code, what the relay said, and how many messages it forwarded.
async fn stuck_with_its_room_full(
    room: usize,
    answers: Vec<String>,
    last: &str,
    fails: bool,
) -> Outcome<(u8, String, usize)> {
    let fed = answers.len();
    let answers = Arc::new(answers);
    let transport = FakeTransport::scripted(move |message| {
        let nth = message
            .get("id")
            .and_then(Value::as_u64)
            .and_then(|id| usize::try_from(id).ok());
        match nth.and_then(|nth| answers.get(nth)) {
            Some(answer) => status(200, Body::Json(answer.clone())),
            None => broken("the test scripted no answer for this message"),
        }
    });
    let setup = Setup {
        limits: Limits {
            line_bytes: 1024,
            waiting_bytes: room,
            closing: Duration::from_millis(300),
            ..Limits::default()
        },
        ..Setup::new(&transport)
    };
    let (mut client, stdin) = tokio::io::duplex(65_536);
    let stdin: Box<dyn AsyncRead + Unpin> = if fails {
        Box::new(stdin.chain(Failing))
    } else {
        Box::new(stdin)
    };
    let mut stderr = Vec::new();
    let mut running = Box::pin(relay_on(setup, stdin, Stuck, &mut stderr));
    // The version makes a notification carry its method in a header, which is how a
    // notification can be one the relay does not send.
    let params = json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}});
    for id in 0..fed {
        client
            .write_all(request(&json!(id), "tools/call", &params).as_bytes())
            .await?;
        for _ in 0..TURNS {
            tokio::select! {
                biased;
                early = &mut running => {
                    return Err(format!("the relay returned while its input was open: {early:?}").into());
                }
                () = tokio::task::yield_now() => {}
            }
        }
        assert_eq!(
            transport.handed(),
            id.saturating_add(1),
            "every answer had its room"
        );
    }
    client.write_all(last.as_bytes()).await?;
    drop(client);
    let code = tokio::time::timeout(PATIENCE, running)
        .await
        .map_err(|_| "the relay had not ended long after its closing time of 300 ms")??;
    Ok((code, String::from_utf8(stderr)?, transport.posts()?.len()))
}

/// The room is counted in bytes, and from both sides: with a shared part for exactly two
/// lines, the service hands over two events and one more, which takes the place beside the
/// shared part, and no fourth: it would have to be held before it has room. One byte less and
/// it is one line fewer; one line more and it is one more. Once the output is read again,
/// every line comes out.
#[tokio::test]
async fn what_is_held_of_the_service_s_answers_is_bounded_in_bytes() -> TestResult {
    let two_lines = LINE.saturating_mul(2);
    assert_eq!(
        handed_over_while_the_output_stands_still(two_lines).await?,
        (3, 9)
    );
    assert_eq!(
        handed_over_while_the_output_stands_still(two_lines.saturating_sub(1)).await?,
        (2, 9)
    );
    assert_eq!(
        handed_over_while_the_output_stands_still(LINE.saturating_mul(3)).await?,
        (4, 9)
    );
    Ok(())
}

/// While a text holds the place beside the shared part the relay forwards nothing more from
/// its input, from both sides: with a shared part for exactly one answer, the second request
/// is forwarded too, its answer takes that place, and the third is held; with one byte less,
/// the first answer already takes it, and the second request is held.
#[tokio::test]
async fn nothing_more_is_forwarded_while_the_room_is_full() -> TestResult {
    assert_eq!(forwarded_while_the_output_stands_still(LINE).await?, 2);
    assert_eq!(
        forwarded_while_the_output_stands_still(LINE.saturating_sub(1)).await?,
        1
    );
    assert_eq!(
        forwarded_while_the_output_stands_still(LINE.saturating_mul(2)).await?,
        3
    );
    Ok(())
}

/// The budget holds however many messages are in flight (the re-review's F1). Sixteen
/// requests are in flight, the service answers all of them at the same moment, and the
/// output takes nothing. The answers the shared part has room for are handed over, and one
/// more; the others are not held anywhere in the relay. One byte less of room and it is one
/// answer fewer. Once the output is read, all sixteen come out.
#[tokio::test]
async fn the_room_holds_with_sixteen_messages_in_flight() -> TestResult {
    const FLYING: usize = 16;
    for (room, held) in [
        (LINE.saturating_mul(4), 5),
        (LINE.saturating_mul(4).saturating_sub(1), 4),
    ] {
        let arrived = Arc::new(AtomicUsize::new(0));
        let (open, gate) = watch::channel(false);
        let transport = FakeTransport::new(move |post, _| {
            let (arrived, open, mut gate) = (arrived.clone(), open.clone(), gate.clone());
            Box::pin(async move {
                let message = serde_json::from_slice::<Value>(&post.body).unwrap_or_default();
                if arrived.fetch_add(1, Ordering::SeqCst).saturating_add(1) == FLYING {
                    // The last one is in flight: every answer is ready at once.
                    let _ = open.send(true);
                }
                // The sender lives in every answer, so this ends only when the gate opens.
                let _ = gate.wait_for(|open| *open).await;
                answered(&sized(message.get("id"))).reply
            })
        });
        let setup = Setup {
            limits: Limits {
                waiting_bytes: room,
                ..Limits::default()
            },
            ..Setup::new(&transport)
        };
        let (stdout, mut client) = tokio::io::duplex(1);
        let input: String = (0..FLYING)
            .map(|id| request(&json!(id), "tools/call", &json!({})))
            .collect();
        let mut running = Box::pin(relay_over(setup, input.as_bytes(), stdout));
        for _ in 0..TURNS {
            tokio::select! {
                biased;
                early = &mut running => {
                    return Err(format!("the relay returned while its output stood still: {early:?}").into());
                }
                () = tokio::task::yield_now() => {}
            }
        }
        assert_eq!(
            (transport.posts()?.len(), transport.handed()),
            (FLYING, held),
            "a shared part of {room} bytes"
        );
        let mut written = Vec::new();
        let (ran, read) = tokio::join!(running, client.read_to_end(&mut written));
        assert_eq!(ran?.0, 0);
        read?;
        assert_eq!(String::from_utf8(written)?.lines().count(), FLYING);
        assert_eq!(transport.handed(), FLYING);
    }
    Ok(())
}

/// What the relay has to say in the middle of an answer waits for no room. The room has no
/// shared part here, so every text needs the one place beside it. The first event of a stream
/// is no message and is dropped, which the relay says; the second event already waits for the
/// place. A diagnostic that waited for room at that moment would wait for an event that only
/// the waiting relay can take: the relay would stand still.
#[tokio::test]
async fn what_the_relay_says_in_the_middle_of_an_answer_waits_for_no_room() -> TestResult {
    let transport = FakeTransport::scripted(|message| Script {
        events: vec![
            "no message at all".to_owned(),
            response(message.get("id").unwrap_or(&Value::Null), &json!({})).to_string(),
        ],
        ..status(200, Body::Events)
    });
    let setup = Setup {
        limits: Limits {
            waiting_bytes: 0,
            ..Limits::default()
        },
        ..Setup::new(&transport)
    };
    let ran = tokio::time::timeout(
        PATIENCE,
        relay(
            setup,
            request(&json!(1), "tools/call", &json!({})).as_bytes(),
        ),
    )
    .await
    .map_err(
        |_| "the relay stood still: a diagnostic waited for room in the middle of an answer",
    )??;
    assert_eq!(ran.lines()?, [response(&json!(1), &json!({}))]);
    assert_eq!(
        (ran.code, ran.stderr.as_str()),
        (
            0,
            "okf-jawn mcp: dropped an answer of the service that is not a JSON-RPC message\n"
        )
    );
    Ok(())
}

/// The relay's own lines hold room as the service's answers do. Every request fails before
/// the service answers, so each gets an error the relay writes itself, and the output takes
/// nothing. With a shared part of exactly one such line, the second takes the place beside
/// it and the third request is not forwarded; with one byte less, the first line takes that
/// place and the second request is not forwarded.
#[tokio::test]
async fn the_relay_s_own_lines_hold_room_too() -> TestResult {
    let failing = || FakeTransport::scripted(|_| broken("connection reset by peer"));
    let one = relay(
        Setup::new(&failing()),
        request(&json!(1), "tools/call", &json!({})).as_bytes(),
    )
    .await?;
    let line = one.stdout.len().saturating_sub(1);
    assert!(
        one.stdout.contains("-32603") && line > 100,
        "{}",
        one.stdout
    );
    for (room, expected) in [(line, 2_usize), (line.saturating_sub(1), 1)] {
        let transport = failing();
        let setup = Setup {
            limits: Limits {
                waiting_bytes: room,
                ..Limits::default()
            },
            ..Setup::new(&transport)
        };
        let (stdout, mut client_out) = tokio::io::duplex(1);
        let (mut client_in, stdin) = tokio::io::duplex(65_536);
        let mut running = Box::pin(relay_over(setup, stdin, stdout));
        for id in 1..=4 {
            client_in
                .write_all(request(&json!(id), "tools/call", &json!({})).as_bytes())
                .await?;
            for _ in 0..TURNS {
                tokio::select! {
                    biased;
                    early = &mut running => {
                        return Err(format!("the relay returned while its output stood still: {early:?}").into());
                    }
                    () = tokio::task::yield_now() => {}
                }
            }
        }
        assert_eq!(
            transport.posts()?.len(),
            expected,
            "a shared part of {room} bytes"
        );
        drop(client_in);
        let mut written = Vec::new();
        let (ran, read) = tokio::join!(running, client_out.read_to_end(&mut written));
        assert_eq!(ran?.0, 0);
        read?;
        assert_eq!(String::from_utf8(written)?.lines().count(), 4);
    }
    Ok(())
}

/// A text longer than the whole shared part still gets through (the re-review's note N7):
/// it takes the place beside the shared part. With a shared part of ten bytes, one request
/// is answered, where a text that waited for room it can never have would wait for ever.
#[tokio::test]
async fn a_line_longer_than_the_room_still_gets_through() -> TestResult {
    let transport = FakeTransport::echoing();
    let setup = Setup {
        limits: Limits {
            waiting_bytes: 10,
            ..Limits::default()
        },
        ..Setup::new(&transport)
    };
    let ran = tokio::time::timeout(
        Duration::from_secs(10),
        relay(setup, request(&json!(1), "ping", &json!({})).as_bytes()),
    )
    .await
    .map_err(|_| "a line longer than the room never got through")??;
    assert_eq!((ran.lines()?.len(), ran.code), (1, 0));
    Ok(())
}

/// Diagnostics hold room as lines do. The error stream stands still and every notification
/// fails, which is said there: the first diagnostic is longer than the shared part and takes
/// the place beside it, and the relay forwards nothing more until the stream is read again.
/// Every wait here is bounded, so a relay that stands still fails the test.
#[tokio::test]
async fn diagnostics_wait_in_the_same_room() -> TestResult {
    let transport = FakeTransport::scripted(|_| broken("connection reset by peer"));
    let setup = Setup {
        limits: Limits {
            waiting_bytes: 10,
            ..Limits::default()
        },
        ..Setup::new(&transport)
    };
    let (stderr, mut client_err) = tokio::io::duplex(1);
    let (mut client_in, stdin) = tokio::io::duplex(65_536);
    let mut stdout = Vec::new();
    let mut running = Box::pin(relay_on(setup, stdin, &mut stdout, stderr));
    for _ in 0..3 {
        client_in
            .write_all(notification("notifications/progress").as_bytes())
            .await?;
        for _ in 0..TURNS {
            tokio::select! {
                biased;
                early = &mut running => {
                    return Err(format!("the relay returned while its error stream stood still: {early:?}").into());
                }
                () = tokio::task::yield_now() => {}
            }
        }
    }
    assert_eq!(transport.posts()?.len(), 1);
    drop(client_in);
    let mut said = String::new();
    let (ran, read) = tokio::time::timeout(PATIENCE, async {
        tokio::join!(running, client_err.read_to_string(&mut said))
    })
    .await
    .map_err(|_| "the relay stood still with its diagnostics unread")?;
    assert_eq!(ran?, 0);
    read?;
    assert_eq!(said.matches("was not delivered").count(), 3, "{said}");
    assert_eq!(transport.posts()?.len(), 3);
    Ok(())
}

/// The input has ended and three messages do not finish: the relay waits its closing time and
/// no longer, writes what did finish, names the rest on stderr, and returns 1.
#[tokio::test]
async fn after_the_input_ends_what_is_in_flight_gets_a_bounded_time() -> TestResult {
    let transport = FakeTransport::new(|post, _| {
        Box::pin(async move {
            let message = serde_json::from_slice::<Value>(&post.body).unwrap_or_default();
            match message.get("id") {
                Some(id) if id == &json!(2) => answered(&response(id, &json!({}))).reply,
                _ => std::future::pending().await,
            }
        })
    });
    let setup = Setup {
        limits: Limits {
            closing: Duration::from_millis(300),
            ..Limits::default()
        },
        ..Setup::new(&transport)
    };
    let input = format!(
        "{}{}{}{}",
        request(&json!(1), "tools/call", &json!({})),
        request(&json!(2), "tools/call", &json!({})),
        notification("notifications/progress"),
        request(&json!("three"), "tools/list", &json!({}))
    );
    let ran = tokio::time::timeout(PATIENCE, relay(setup, input.as_bytes())).await??;
    assert_eq!(ran.code, 1, "{}", ran.stderr);
    assert_eq!(ran.lines()?, [response(&json!(2), &json!({}))]);
    assert_eq!(
        ran.stderr,
        "okf-jawn mcp: the input ended and 3 of its messages had not finished 300ms later; the \
         relay stops and leaves them unanswered: notification \"notifications/progress\", \
         request \"three\", request 1\n"
    );
    Ok(())
}

/// The closing time also covers an output that takes nothing and does not fail (the
/// re-review's F2). Four requests are answered at once, the input ends, and the output never
/// takes a byte: when the closing time is over the relay returns 1 without waiting for its
/// writer, and says what it leaves. With the shared room of the shipped relay every answer
/// waits to be written, and their number is said. With a shared part of exactly one answer,
/// one answer waits there and one in the place beside it; the other two requests wait for
/// that place, and are named.
#[tokio::test]
async fn the_relay_ends_when_its_output_takes_nothing_after_the_input_ended() -> TestResult {
    let one = relay(
        Setup::new(&FakeTransport::echoing()),
        request(&json!(0), "tools/call", &json!({})).as_bytes(),
    )
    .await?;
    let line = one.stdout.len().saturating_sub(1);
    assert!(line > 20, "{}", one.stdout);
    let unwritten = |lines: usize| {
        format!(
            "okf-jawn mcp: the input ended and {lines} of the lines for the output had not been \
             written 300ms later; the relay stops without writing them\n"
        )
    };
    let unfinished = "okf-jawn mcp: the input ended and 2 of its messages had not finished \
                      300ms later; the relay stops and leaves them unanswered: request 2, \
                      request 3\n";
    for (room, said, forwarded) in [
        (Limits::default().waiting_bytes, unwritten(4), 4),
        (line, format!("{unfinished}{}", unwritten(2)), 2),
    ] {
        let transport = FakeTransport::echoing();
        let setup = Setup {
            limits: Limits {
                waiting_bytes: room,
                closing: Duration::from_millis(300),
                ..Limits::default()
            },
            ..Setup::new(&transport)
        };
        let input: String = (0..4)
            .map(|id| request(&json!(id), "tools/call", &json!({})))
            .collect();
        let mut stderr = Vec::new();
        let code = tokio::time::timeout(
            PATIENCE,
            relay_on(setup, input.as_bytes(), Stuck, &mut stderr),
        )
        .await
        .map_err(|_| "the relay had not ended long after its closing time of 300 ms")??;
        assert_eq!(
            (code, String::from_utf8(stderr)?),
            (1, said),
            "a shared part of {room} bytes"
        );
        // The two that waited for the place were read, which is how the end of the input
        // was seen, and were not forwarded.
        assert_eq!(transport.posts()?.len(), forwarded);
    }
    Ok(())
}

/// The read loop reads on while messages wait for room, which is how it sees the end of the
/// input (the re-review's F2). Requests arrive one at a time, each after the relay has done
/// what it can with the one before, and the output takes nothing. The shared part holds one
/// answer: the first answer waits there, the second in the place beside it, and the third
/// and fourth requests wait for that place and are not forwarded. They are read all the
/// same, so when the input ends the closing time starts, and the relay ends and names them.
/// A relay that held the third request and read no further would never see the end.
#[tokio::test]
async fn the_end_of_the_input_is_seen_while_messages_wait_for_room() -> TestResult {
    let one = relay(
        Setup::new(&FakeTransport::echoing()),
        request(&json!(0), "tools/call", &json!({})).as_bytes(),
    )
    .await?;
    let transport = FakeTransport::echoing();
    let setup = Setup {
        limits: Limits {
            waiting_bytes: one.stdout.len().saturating_sub(1),
            closing: Duration::from_millis(300),
            ..Limits::default()
        },
        ..Setup::new(&transport)
    };
    let (mut client_in, stdin) = tokio::io::duplex(65_536);
    let mut stderr = Vec::new();
    let mut running = Box::pin(relay_on(setup, stdin, Stuck, &mut stderr));
    for id in 0..4 {
        client_in
            .write_all(request(&json!(id), "tools/call", &json!({})).as_bytes())
            .await?;
        for _ in 0..TURNS {
            tokio::select! {
                biased;
                early = &mut running => {
                    return Err(format!("the relay returned while its input was open: {early:?}").into());
                }
                () = tokio::task::yield_now() => {}
            }
        }
    }
    assert_eq!(transport.posts()?.len(), 2);
    drop(client_in);
    let code = tokio::time::timeout(PATIENCE, running)
        .await
        .map_err(|_| "the relay never saw that its input had ended")??;
    assert_eq!(
        (code, String::from_utf8(stderr)?.as_str()),
        (
            1,
            "okf-jawn mcp: the input ended and 2 of its messages had not finished 300ms later; \
             the relay stops and leaves them unanswered: request 2, request 3\n\
             okf-jawn mcp: the input ended and 2 of the lines for the output had not been \
             written 300ms later; the relay stops without writing them\n"
        )
    );
    Ok(())
}

/// An error stream that takes nothing does not keep the relay either: its last words have
/// a bounded time of their own, and then it returns 1 having said nothing.
#[tokio::test]
async fn the_relay_ends_when_neither_output_stream_takes_anything() -> TestResult {
    let transport = FakeTransport::echoing();
    let setup = Setup {
        limits: Limits {
            closing: Duration::from_millis(300),
            ..Limits::default()
        },
        ..Setup::new(&transport)
    };
    let code = tokio::time::timeout(
        PATIENCE,
        relay_on(
            setup,
            request(&json!(1), "tools/call", &json!({})).as_bytes(),
            Stuck,
            Stuck,
        ),
    )
    .await
    .map_err(|_| "the relay waited for an error stream that takes nothing")??;
    assert_eq!(code, 1);
    Ok(())
}

/// An answer that comes after the input ended, inside the closing time, is waited for and
/// written, and the relay returns 0.
#[tokio::test]
async fn an_answer_inside_the_closing_time_is_waited_for() -> TestResult {
    let transport = FakeTransport::new(|_, _| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            answered(&response(&json!(1), &json!({"late": true}))).reply
        })
    });
    let ran = tokio::time::timeout(
        PATIENCE,
        relay(
            Setup::new(&transport),
            request(&json!(1), "tools/call", &json!({})).as_bytes(),
        ),
    )
    .await??;
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    assert_eq!(ran.lines()?, [response(&json!(1), &json!({"late": true}))]);
    Ok(())
}

/// The output fails while the relay is not reading its input: while `initialize` is being
/// exchanged, and while a message waits for a turn that no message in flight gives up. The
/// input stays open and the service never answers; the relay stops all the same.
#[tokio::test]
async fn a_failed_output_is_noticed_whatever_the_relay_waits_for() -> TestResult {
    let waiting_for_initialize = request(&json!(0), "initialize", &json!({}));
    let waiting_for_a_turn = format!(
        "{}{}",
        request(&json!(1), "tools/call", &json!({})),
        request(&json!(2), "tools/call", &json!({}))
    );
    for held in [waiting_for_initialize, waiting_for_a_turn] {
        let transport = FakeTransport::new(|_, _| Box::pin(std::future::pending()));
        let setup = Setup {
            limits: Limits {
                in_flight: 1,
                ..Limits::default()
            },
            ..Setup::new(&transport)
        };
        let (mut client, stdin) = tokio::io::duplex(4096);
        // The line that is not JSON makes the relay write, which is how a failed output shows.
        client
            .write_all(format!("not json\n{held}").as_bytes())
            .await?;
        let (code, stderr) =
            tokio::time::timeout(PATIENCE, relay_over(setup, stdin, Broken)).await??;
        assert_eq!(code, 1);
        assert!(stderr.contains("the output stream failed"), "{stderr}");
        assert_eq!(transport.posts()?.len(), 1, "one message was in flight");
        drop(client);
    }
    Ok(())
}

/// The relay ends when its output is stuck, whatever lines came before (the second
/// re-review's B1). The output takes nothing and the room is full: one answer waits in the
/// shared part and one in the place beside it. Then comes a line that the relay does not
/// forward at once but answers or reports itself, which needs room and finds none, and the
/// input ends. The relay sees the end all the same, because its read loop reads on while it
/// waits for room: when the closing time is over it returns 1 and names the line among what
/// it leaves. A read loop that only waited for room would wait for ever.
///
/// One case for each such line: one that is not JSON, a request the relay refuses, a
/// notification it cannot send, a line over the limit, and an `initialize`, which waits for
/// the same place before it is forwarded, and is not forwarded. Then an input that fails
/// where the others end, and the control, which has no such line.
#[tokio::test]
async fn the_relay_ends_when_its_output_is_stuck_whatever_lines_came_before() -> TestResult {
    let one = relay(
        Setup::new(&FakeTransport::echoing()),
        request(&json!(0), "tools/call", &json!({})).as_bytes(),
    )
    .await?;
    let answer = |id: usize| response(&json!(id), &json!({"method": "tools/call"})).to_string();
    assert_eq!(format!("{}\n", answer(0)), one.stdout);
    let room = answer(0).len();
    let too_long = format!("{}\n", "x".repeat(1025));
    let initialize = request(&json!(9), "initialize", &json!({}));
    let refused = Some("a line the relay refused");
    for (last, fails, left) in [
        ("this line is not JSON\n", false, refused),
        (
            "{\"jsonrpc\":\"1.0\",\"id\":5,\"method\":\"x\"}\n",
            false,
            refused,
        ),
        (
            "{\"jsonrpc\":\"2.0\",\"method\":\"n\u{e9}\"}\n",
            false,
            refused,
        ),
        (too_long.as_str(), false, refused),
        (initialize.as_str(), false, Some("request 9")),
        ("", true, None),
        ("", false, None),
    ] {
        let (code, said, forwarded) =
            stuck_with_its_room_full(room, vec![answer(0), answer(1)], last, fails).await?;
        let names = left.map(|name| unfinished(&[name])).unwrap_or_default();
        assert_eq!(
            (code, said, forwarded),
            (1, format!("{names}{}", unwritten(2)), 2),
            "after {last:?}, the input failing: {fails}"
        );
    }
    Ok(())
}

/// The same with the room of the shipped relay (the reviewer's probe `rr2_q04b`). The first
/// answer is as long as the shared part less one short answer, the second fills the shared
/// part to the byte, and the third takes the place beside it. The line that is not JSON then
/// finds no room, and the relay ends all the same.
#[tokio::test]
async fn the_shipped_relay_ends_when_its_output_is_stuck_and_its_room_is_full() -> TestResult {
    let answer = |id: usize| response(&json!(id), &json!({"method": "tools/call"})).to_string();
    let short = answer(1).len();
    let frame = "{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":\"\"}".len();
    let long = Limits::default().waiting_bytes.saturating_sub(short);
    let first = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":\"{}\"}}",
        "a".repeat(long.saturating_sub(frame))
    );
    assert_eq!(first.len(), long);
    let (code, said, forwarded) = tokio::time::timeout(
        Duration::from_secs(240),
        stuck_with_its_room_full(
            Limits::default().waiting_bytes,
            vec![first, answer(1), answer(2)],
            "this line is not JSON\n",
            false,
        ),
    )
    .await
    .map_err(|_| "the answers of the test had not been taken after four minutes")??;
    assert_eq!(
        (code, said, forwarded),
        (
            1,
            format!(
                "{}{}",
                unfinished(&["a line the relay refused"]),
                unwritten(3)
            ),
            3
        )
    );
    Ok(())
}

/// The end of the input is seen while `initialize` is exchanged, and the lines read meanwhile
/// are counted in what the relay leaves. The service never answers `initialize`, two more
/// requests follow, and the input ends: the relay reads on while it waits, so the closing time
/// starts, and when it is over the relay names the request it waited for and says how many
/// lines it had read and not looked at. Neither of them was forwarded.
#[tokio::test]
async fn the_end_of_the_input_is_seen_while_initialize_is_exchanged() -> TestResult {
    let transport = FakeTransport::new(|_, _| Box::pin(std::future::pending()));
    let setup = Setup {
        limits: Limits {
            closing: Duration::from_millis(300),
            ..Limits::default()
        },
        ..Setup::new(&transport)
    };
    let input = format!(
        "{}{}{}",
        request(&json!(0), "initialize", &json!({})),
        request(&json!(1), "tools/list", &json!({})),
        request(&json!(2), "tools/call", &json!({}))
    );
    let ran = tokio::time::timeout(PATIENCE, relay(setup, input.as_bytes()))
        .await
        .map_err(|_| "the relay never saw that its input had ended")??;
    assert_eq!(
        (ran.code, ran.stdout.as_str(), ran.stderr),
        (
            1,
            "",
            format!(
                "{}okf-jawn mcp: the input ended and 2 of its lines had not been looked at \
                 300ms later; the relay stops and leaves them unanswered\n",
                unfinished(&["request 0"])
            )
        )
    );
    assert_eq!(transport.posts()?.len(), 1);
    Ok(())
}

/// A line the relay answered itself is done with: when the relay gives up later, it is not
/// named among what is left. The first line is not JSON and is answered at once; the request
/// after it is never answered.
#[tokio::test]
async fn a_line_the_relay_answered_is_not_named_when_it_gives_up() -> TestResult {
    let transport = FakeTransport::new(|_, _| Box::pin(std::future::pending()));
    let setup = Setup {
        limits: Limits {
            closing: Duration::from_millis(300),
            ..Limits::default()
        },
        ..Setup::new(&transport)
    };
    let input = format!("not json\n{}", request(&json!(1), "tools/call", &json!({})));
    let ran = tokio::time::timeout(PATIENCE, relay(setup, input.as_bytes())).await??;
    assert_eq!(
        (ran.code, ran.stderr.as_str()),
        (1, unfinished(&["request 1"]).as_str()),
        "{}",
        ran.stdout
    );
    assert_eq!(
        some(ran.lines()?.first(), "the relay's answer")?.pointer("/error/code"),
        Some(&json!(-32700))
    );
    Ok(())
}

/// Reading ahead is bounded by the turns: a line read ahead holds one, and one line more is
/// read and waits for a turn. With two turns and an `initialize` that is never answered, the
/// relay takes `initialize` and two of the six long lines after it from its input, and no
/// more than its reader's buffer of the third. A relay that read ahead without a turn would
/// take them all.
#[tokio::test]
async fn no_more_lines_are_read_ahead_than_there_are_turns() -> TestResult {
    const LONG: usize = 20_000;
    let transport = FakeTransport::new(|_, _| Box::pin(std::future::pending()));
    let setup = Setup {
        limits: Limits {
            in_flight: 2,
            ..Limits::default()
        },
        ..Setup::new(&transport)
    };
    let first = request(&json!(0), "initialize", &json!({}));
    let mut input = first.clone();
    for id in 1..=6 {
        let line = request(&json!(id), "tools/call", &json!({"pad": "x".repeat(LONG)}));
        assert!(line.len() > LONG && line.len() < LONG.saturating_add(200));
        input.push_str(&line);
    }
    let taken = Arc::new(AtomicUsize::new(0));
    let stdin = Counted {
        bytes: input.into_bytes(),
        taken: taken.clone(),
    };
    let mut stdout = Vec::new();
    let mut running = Box::pin(relay_over(setup, stdin, &mut stdout));
    for _ in 0..TURNS {
        tokio::select! {
            biased;
            early = &mut running => {
                return Err(format!("the relay returned while its input was open: {early:?}").into());
            }
            () = tokio::task::yield_now() => {}
        }
    }
    let taken = taken.load(Ordering::SeqCst);
    let two = first.len().saturating_add(LONG.saturating_mul(2));
    let three = first.len().saturating_add(LONG.saturating_mul(3));
    assert!(
        taken >= two && taken < three,
        "{taken} bytes were taken from the input; two long lines end after {two}, three after {three}"
    );
    assert_eq!(transport.posts()?.len(), 1);
    Ok(())
}

/// The relay's last words are cleaned of the credential like every other line it writes (the
/// second re-review's B3). They go straight to the error stream, past the queue every other
/// diagnostic takes. A request whose id holds the credential is still unanswered when the
/// closing time runs out, and so is an `initialize` with such an id, which the read loop
/// itself waits for: each is named with the mark where the credential stood.
#[tokio::test]
async fn the_relay_s_last_words_never_carry_the_credential() -> TestResult {
    let secret_id = json!(format!("id-{TOKEN}"));
    for (input, names) in [
        (
            format!(
                "{}{}",
                request(&json!(7), "tools/call", &json!({})),
                request(&secret_id, "tools/call", &json!({}))
            ),
            ["request \"id-[credential]\"", "request 7"].as_slice(),
        ),
        (
            request(&secret_id, "initialize", &json!({})),
            ["request \"id-[credential]\""].as_slice(),
        ),
    ] {
        let transport = FakeTransport::new(|_, _| Box::pin(std::future::pending()));
        let setup = Setup {
            limits: Limits {
                closing: Duration::from_millis(300),
                ..Limits::default()
            },
            ..Setup::new(&transport)
        };
        let ran = tokio::time::timeout(PATIENCE, relay(setup, input.as_bytes())).await??;
        assert_eq!(
            (ran.code, ran.stderr.as_str()),
            (1, unfinished(names).as_str())
        );
        assert!(!ran.stderr.contains(TOKEN), "{}", ran.stderr);
    }
    Ok(())
}

/// The last words get two seconds and no more (the second re-review's note N3), from both
/// sides. Neither output stream takes anything and the closing time is 300 ms, so the relay
/// ends 2.3 seconds after its input: not sooner, which would be an error stream that was not
/// given its two seconds, and not much later, which would keep a client waiting for a relay
/// that has nothing more to do. Ten seconds are left for a machine that is busy.
#[tokio::test]
async fn the_relay_s_last_words_get_two_seconds_and_no_more() -> TestResult {
    let transport = FakeTransport::echoing();
    let setup = Setup {
        limits: Limits {
            closing: Duration::from_millis(300),
            ..Limits::default()
        },
        ..Setup::new(&transport)
    };
    let started = std::time::Instant::now();
    let code = tokio::time::timeout(
        PATIENCE,
        relay_on(
            setup,
            request(&json!(1), "tools/call", &json!({})).as_bytes(),
            Stuck,
            Stuck,
        ),
    )
    .await
    .map_err(|_| "the relay waited for an error stream that takes nothing")??;
    let took = started.elapsed();
    assert_eq!(code, 1);
    assert!(
        took >= Duration::from_millis(2300) && took < Duration::from_millis(12_300),
        "the relay ended {took:?} after its input"
    );
    Ok(())
}

/// An input that hands out all of its bytes at once and then fails.
struct Breaking {
    bytes: Vec<u8>,
    given: bool,
}

impl AsyncRead for Breaking {
    fn poll_read(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.given {
            return Poll::Ready(Err(std::io::Error::other("the input breaks")));
        }
        this.given = true;
        buffer.put_slice(&std::mem::take(&mut this.bytes));
        Poll::Ready(Ok(()))
    }
}

/// An input that fails while the loop is busy with `initialize` and every turn is taken: the
/// failure is read ahead and waits for a turn. The loop must still be handed it (exit code 1,
/// said on the error stream, never 0 in silence), and when the relay gives up meanwhile the
/// failure is not counted among the lines that were not looked at.
#[tokio::test]
async fn an_input_failure_that_is_read_ahead_is_handed_to_the_loop_and_is_no_line() -> TestResult {
    let lines = format!(
        "{}{}\n",
        request(&json!(1), "initialize", &json!({})),
        json!({"jsonrpc": "2.0", "method": "notifications/x"})
    );
    // The answer to `initialize` comes late, after the input was read ahead.
    let late = FakeTransport::new(|post, _events| {
        Box::pin(async move {
            let message: Value = serde_json::from_slice(&post.body).unwrap_or(Value::Null);
            for _ in 0..TURNS {
                tokio::task::yield_now().await;
            }
            match message.get("id") {
                Some(id) => answered(&json!({"jsonrpc": "2.0", "id": id, "result": {}})).reply,
                None => accepted().reply,
            }
        })
    });
    let mut setup = Setup::new(&late);
    setup.limits.in_flight = 2;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let input = Breaking {
        bytes: lines.clone().into_bytes(),
        given: false,
    };
    let code = tokio::time::timeout(PATIENCE, relay_on(setup, input, &mut stdout, &mut stderr))
        .await
        .map_err(|_| "the relay did not end")??;
    let said = String::from_utf8_lossy(&stderr).into_owned();
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("the input stream failed"), "{said}");
    assert!(said.contains("the input breaks"), "{said}");

    // `initialize` is never answered: the relay gives up, and the line that waited is one line.
    let never = FakeTransport::new(|_, _| Box::pin(std::future::pending()));
    let mut setup = Setup::new(&never);
    setup.limits.in_flight = 2;
    setup.limits.closing = Duration::from_millis(300);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let input = Breaking {
        bytes: lines.into_bytes(),
        given: false,
    };
    let code = tokio::time::timeout(PATIENCE, relay_on(setup, input, &mut stdout, &mut stderr))
        .await
        .map_err(|_| "the relay did not end")??;
    let said = String::from_utf8_lossy(&stderr).into_owned();
    assert_eq!(code, 1, "{said}");
    assert!(
        said.contains("1 of its lines had not been looked at"),
        "a failure is not a line: {said}"
    );
    Ok(())
}
