//! What the relay holds and how it ends (the review's notes N5 and N6).
//!
//! Everything waiting to be written shares one room counted in bytes; while it is full the
//! relay takes nothing more from the service and forwards nothing more from its input. When
//! the input ends, the messages in flight get a bounded time; when the output fails, the
//! relay stops whatever it was waiting for.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use okf_jawn_cli::relay::Limits;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{notification, request, response};
use crate::check::TestResult;
use crate::support::Outcome;
use crate::support::doubles::{
    Broken, FakeTransport, Setup, answered, broken, relay, relay_on, relay_over,
};

/// The length of every line the service's double answers with in the tests of the room.
const LINE: usize = 200;

/// How long a test waits for a relay that must end by itself.
const PATIENCE: Duration = Duration::from_secs(30);

/// How often a test lets every other task run before it looks at what they did: far more
/// often than the few steps anything here needs before it has to wait.
const TURNS: usize = 500;

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

/// The room is counted in bytes, and from both sides: with room for exactly two lines, two
/// wait to be written, a third waits in the exchange's hands and a fourth for the exchange,
/// so the service hands over four events and no fifth; one byte less room and it is one line
/// fewer. Once the output is read again, every line comes out.
#[tokio::test]
async fn what_waits_to_be_written_is_bounded_in_bytes() -> TestResult {
    let two_lines = LINE.saturating_mul(2);
    assert_eq!(
        handed_over_while_the_output_stands_still(two_lines).await?,
        (4, 9)
    );
    assert_eq!(
        handed_over_while_the_output_stands_still(two_lines.saturating_sub(1)).await?,
        (3, 9)
    );
    assert_eq!(
        handed_over_while_the_output_stands_still(LINE.saturating_mul(3)).await?,
        (5, 9)
    );
    Ok(())
}

/// While the room is full the relay forwards nothing more from its input, from both sides:
/// with room for exactly one answer, the first request is forwarded and the second is held;
/// with one byte more, the second is forwarded too, and its answer then waits for room.
#[tokio::test]
async fn nothing_more_is_forwarded_while_the_room_is_full() -> TestResult {
    assert_eq!(forwarded_while_the_output_stands_still(LINE).await?, 1);
    assert_eq!(
        forwarded_while_the_output_stands_still(LINE.saturating_add(1)).await?,
        2
    );
    Ok(())
}

/// Diagnostics share the room with the lines. The error stream stands still and every
/// notification fails, which is said there: the first diagnostic fills a room smaller than
/// itself, and the relay forwards nothing more until the stream is read again.
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
    let (ran, read) = tokio::join!(running, client_err.read_to_string(&mut said));
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
