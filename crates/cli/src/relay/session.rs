//! The relay loop: read a line, forward it, write each answer as one line.
//!
//! Invariants:
//! - The output stream carries JSON-RPC messages and nothing else, one per line. One task
//!   owns the stream and writes each line whole; diagnostics go to the error stream.
//! - Every request the relay accepts gets a response or an error with its id: the service's
//!   own, or one the relay writes when the service gave none. A failed notification is
//!   logged to the error stream instead. The one exception is a request still in flight when
//!   the relay stops, which is named on the error stream (see "how it ends").
//! - Messages are forwarded concurrently, at most `Limits::in_flight` at a time; while that
//!   many are in flight no further line is read. `initialize` is exchanged alone, after the
//!   messages in flight have finished, because the headers of every later message come from
//!   its answer.
//! - The connector credential is never written, whoever wrote the line: every line for the
//!   output stream, the service's own included, passes `Credential::redact_line` in `line`,
//!   the one place a line is queued, and every diagnostic passes `Credential::redact`. A line
//!   that spells the credential outside a JSON string cannot be cleaned and stay JSON: it is
//!   not written, the error stream says so, and a request it would have answered gets the
//!   relay's own error instead.
//! - An answer is passed on as the service wrote it. The relay checks that it is one JSON
//!   value and a JSON-RPC message, removes the line feeds and carriage returns that stand
//!   between its tokens so that it is one line, and changes no other byte, the credential
//!   apart: a number of any size, a key that occurs twice and every escape come out as they
//!   went in. A refusal that lacks the request's id gets it put into its text. An id is
//!   written back as the client wrote it. A message to the service is sent as the client
//!   wrote it.
//!
//! What the relay holds. Everything waiting to be written, lines and diagnostics together,
//! shares one room of `Limits::waiting_bytes` (64 MiB: the length of the longest answer, the
//! least that still lets every answer through). A text takes its length of that room before
//! it is queued and gives it back once it is written. While the room is full, an exchange
//! waits with the answer it has in hand and takes no further event, so the transport stops
//! reading from the service; and the read loop holds the one message it has read and neither
//! forwards it nor reads another. Outside that room, each message in flight can hold its
//! request line (16 MiB), the answer or event being read (64 MiB), one event waiting to be
//! taken (64 MiB), and the one in hand, with a second copy while the credential is removed
//! from it (2 x 64 MiB). With sixteen in flight the worst case is about 4.3 GiB. It takes
//! sixteen event streams that each send events of the full 64 MiB repeating the credential;
//! an ordinary answer costs once or twice its own length.
//!
//! How it ends. When the input ends, the messages in flight get `Limits::closing` (two
//! minutes, what the CLI gives one operation) to finish; the relay then returns 0, or names
//! on the error stream the messages that had not finished and returns 1. When the output
//! stream fails, the relay stops at once and returns 1, whatever it was waiting for.
//!
//! Known limits:
//! - A forwarded message has no limit in time: a tool call may run long, and an event stream
//!   has no fixed length. Sixteen messages that do not end therefore hold every turn, the
//!   relay reads no further line, and it learns that its input ended only when one of them
//!   ends; `initialize` likewise waits for the messages in flight.
//! - A response the service sends for another request's id is passed on as it is.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use okf_jawn_contract::transport::MAX_REQUEST_BODY_BYTES;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::{Id, JoinError, JoinSet};

use super::frame::{Frame, Frames};
use super::json::one_line;
use super::launch::{Arrival, Clock, Launcher, Progress, StartPolicy, start_or_connect};
use super::message::{
    Envelope, INTERNAL_ERROR, INVALID_REQUEST, Kind, Lifecycle, Outgoing, Refusal, error_line,
    prepare,
};
use super::transport::{Body, Transport, TransportError};
use crate::credential::Credential;
use crate::run::Streams;

/// The channels one exchange writes to, the room they share, and the secret kept out of them.
#[derive(Clone)]
struct Desk {
    lines: mpsc::Sender<Queued>,
    notes: mpsc::Sender<Queued>,
    secret: Arc<Credential>,
    /// One permit for each byte that may still wait to be written.
    room: Arc<Semaphore>,
    /// The whole room: no text takes more than this, so the longest one still gets through.
    budget: u32,
}

/// The messages in flight: their tasks, and what each is called when it has to be named.
struct Flight {
    tasks: JoinSet<()>,
    names: HashMap<Id, String>,
}

/// The relay's bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Longest input line accepted, in bytes: the contract's request-body limit.
    pub line_bytes: usize,
    /// Most bytes read of one answer: a JSON body, or one event of a stream.
    pub answer_bytes: usize,
    /// Most messages forwarded at the same time.
    pub in_flight: usize,
    /// Most bytes of lines and diagnostics that may wait to be written. While they fill it,
    /// nothing more is taken from the service and nothing more is forwarded from the input.
    pub waiting_bytes: usize,
    /// How long the messages in flight may still take once the input has ended.
    pub closing: Duration,
}

/// What an exchange learned for the messages that follow it.
#[derive(Debug, Default)]
struct Outcome {
    /// The protocol version an `initialize` answer carried.
    negotiated: Option<String>,
    /// The session the service issued.
    session: Option<String>,
}

/// A text waiting for its writer, with the room it holds until it is written.
type Queued = (String, Option<OwnedSemaphorePermit>);
/// Everything one relay run needs.
#[derive(Debug)]
pub struct Relay<T, L, C> {
    /// Sends messages and asks whether the service is up.
    pub transport: Arc<T>,
    /// Starts the local server.
    pub launcher: L,
    /// Whether the service address is a plain-HTTP loopback address (`Endpoint::may_start`);
    /// only then may the server be started.
    pub loopback: bool,
    /// How long to wait for a server the relay started.
    pub start: StartPolicy,
    /// The clock that wait runs on.
    pub clock: C,
    /// The relay's bounds.
    pub limits: Limits,
    /// The connector credential, kept here only to keep it out of everything written.
    pub secret: Credential,
}

/// The error stream as the place where the wait for a started server is reported.
struct Saying<'run, E> {
    stderr: &'run mut E,
    secret: &'run Credential,
}

/// What the answers relayed so far say about one exchange.
#[derive(Debug, Default)]
struct Seen {
    /// A response or error with the request's id was written.
    answered: bool,
    /// The protocol version of an `initialize` result.
    negotiated: Option<String>,
}

/// The service refused the credential.
const CREDENTIAL_REFUSED: &str = "The okf-jawn service refused the connector credential (HTTP \
    401). Ask the workspace owner for a current connector credential, then set OKF_JAWN_TOKEN \
    or --token-file";
/// The service refused what the credential asked for.
const ACCESS_REFUSED: &str = "The okf-jawn service refused this request (HTTP 403): the \
    connector credential does not grant it";
/// What a request is told when its answer was withheld.
const ANSWER_WITHHELD: &str = "The okf-jawn service's answer repeats the connector credential \
    outside a JSON string, so it was not passed on";
/// What a request is told when the service's answer is no message.
const NOT_A_MESSAGE: &str = "The okf-jawn service's answer is not a JSON-RPC message";
/// How many events of one answer may wait for the relay to take them: the fewest a channel
/// holds, because each may be as long as a whole answer.
const AHEAD: usize = 1;
/// How long the messages in flight may still take once the input has ended: what the CLI
/// gives one operation, so a script that pipes its requests in gets its last answers, and a
/// client that only closes the input does not keep the relay for longer than that.
const CLOSING: Duration = Duration::from_secs(120);
/// How many answers or diagnostics may wait for their writer.
const QUEUE: usize = 64;
/// The line the relay writes for a line over the limit.
const TOO_LONG: &str = "Invalid request: the line is longer than the request limit of";

impl Flight {
    /// Wait for every message in flight.
    async fn drain(&mut self, desk: &Desk) {
        while let Some(joined) = self.tasks.join_next_with_id().await {
            self.landed(joined, desk).await;
        }
    }

    /// Forget one finished message; an exchange that ended abnormally is said.
    async fn landed(&mut self, joined: Result<(Id, ()), JoinError>, desk: &Desk) {
        match joined {
            Ok((id, ())) => {
                self.names.remove(&id);
            }
            Err(error) => {
                self.names.remove(&error.id());
                note(
                    desk,
                    &format!("a message's exchange ended abnormally: {error}"),
                )
                .await;
            }
        }
    }

    fn new() -> Self {
        Self {
            tasks: JoinSet::new(),
            names: HashMap::new(),
        }
    }

    /// Forget the messages that have finished, so a long session holds only those in flight.
    async fn reap(&mut self, desk: &Desk) {
        while let Some(joined) = self.tasks.try_join_next_with_id() {
            self.landed(joined, desk).await;
        }
    }

    /// Start `work` as the message called `name`.
    fn start(&mut self, name: String, work: impl Future<Output = ()> + Send + 'static) {
        let id = self.tasks.spawn(work).id();
        self.names.insert(id, name);
    }
}

impl<E: AsyncWrite + Unpin> Progress for Saying<'_, E> {
    async fn say(&mut self, text: &str) {
        say(self.stderr, self.secret, text).await;
    }
}

impl Default for Limits {
    /// The contract's request-body limit for a line; four times that for an answer; sixteen
    /// messages in flight; for what waits to be written, the length of the longest answer,
    /// which is the least that still lets every answer through; and two minutes to close.
    fn default() -> Self {
        Self {
            line_bytes: MAX_REQUEST_BODY_BYTES,
            answer_bytes: MAX_REQUEST_BODY_BYTES.saturating_mul(4),
            in_flight: 16,
            waiting_bytes: MAX_REQUEST_BODY_BYTES.saturating_mul(4),
            closing: CLOSING,
        }
    }
}

/// What a message in flight is called when the relay has to name it.
fn called(kind: &Kind) -> String {
    match kind {
        Kind::Request { written, .. } => format!("request {written}"),
        Kind::Notification { method } => format!("notification {method:?}"),
        Kind::Response => "a response to the service".to_owned(),
    }
}

/// Let the messages in flight finish, for at most `limit` after the input has ended.
///
/// Returns the exit code: 0 when all finished, 1 when some had not; those are named on the
/// error stream, and they end with the relay.
async fn close(mut flight: Flight, limit: Duration, desk: &Desk) -> u8 {
    if tokio::time::timeout(limit, flight.drain(desk))
        .await
        .is_ok()
    {
        return 0;
    }
    let mut left: Vec<String> = flight.names.into_values().collect();
    left.sort();
    note(
        desk,
        &format!(
            "the input ended and {} of its messages had not finished {limit:?} later; the relay \
             stops and leaves them unanswered: {}",
            left.len(),
            left.join(", ")
        ),
    )
    .await;
    1
}

/// Write one answer of the service to the client, if it is a JSON-RPC message.
///
/// # Errors
/// Returns, in plain words, why it was not written. Anything that is not a message is dropped
/// and logged: the output stream carries protocol messages only.
async fn deliver(
    data: String,
    kind: &Kind,
    initialize: bool,
    desk: &Desk,
    seen: &mut Seen,
) -> Result<(), &'static str> {
    let Some(envelope) = Envelope::read(&data) else {
        note(
            desk,
            "dropped an answer of the service that is not a JSON-RPC message",
        )
        .await;
        return Err(NOT_A_MESSAGE);
    };
    // The text itself is passed on: only the line breaks between its tokens are removed.
    if !line(desk, one_line(data)).await {
        return Err(ANSWER_WITHHELD);
    }
    if let Kind::Request { id, .. } = kind
        && !envelope.method
        && envelope.id.as_ref() == Some(id)
    {
        seen.answered = true;
        if initialize {
            seen.negotiated = envelope.negotiated;
        }
    }
    Ok(())
}

/// Send one message and relay its answers; then settle what the relay still owes the client.
async fn exchange<T: Transport>(transport: &T, outgoing: Outgoing, desk: &Desk) -> Outcome {
    let Outgoing {
        post,
        kind,
        initialize,
    } = outgoing;
    let (events, mut arriving) = mpsc::channel::<String>(AHEAD);
    let streamed = async {
        let mut seen = Seen::default();
        while let Some(data) = arriving.recv().await {
            // What is not passed on is said on the error stream by `deliver` and `line`.
            let _ = deliver(data, &kind, initialize, desk, &mut seen).await;
        }
        seen
    };
    let (reply, mut seen) = tokio::join!(transport.send(post, events), streamed);
    let mut session = None;
    let failure = match reply {
        Err(TransportError(why)) => Some(format!(
            "The request did not reach the okf-jawn service, or its answer was cut off: {why}"
        )),
        Ok(reply) => {
            session = reply.session;
            settle(reply.status, reply.body, &kind, initialize, desk, &mut seen).await
        }
    };
    match (&kind, failure) {
        (Kind::Request { written, .. }, failure) if !seen.answered => {
            let why = failure.unwrap_or_else(|| {
                "The okf-jawn service ended its answer without a response to this request"
                    .to_owned()
            });
            line(desk, error_line(Some(written), INTERNAL_ERROR, &why)).await;
        }
        (Kind::Request { .. }, Some(why)) => {
            note(desk, &format!("after a request was answered: {why}")).await;
        }
        (Kind::Notification { method }, Some(why)) => {
            note(
                desk,
                &format!("the notification {method:?} was not delivered: {why}"),
            )
            .await;
        }
        (Kind::Response, Some(why)) => {
            note(
                desk,
                &format!("a response to the service was not delivered: {why}"),
            )
            .await;
        }
        (_, None) => {}
    }
    Outcome {
        negotiated: seen.negotiated,
        session,
    }
}

/// Read the input to its end, forwarding each message, then let what is in flight finish;
/// returns the exit code.
async fn forward<T: Transport, I: AsyncRead + Unpin>(
    transport: Arc<T>,
    input: I,
    limits: Limits,
    desk: &Desk,
) -> u8 {
    let mut frames = Frames::new(BufReader::new(input), limits.line_bytes);
    let mut flight = Flight::new();
    let mut lifecycle = Lifecycle::default();
    let permits = Arc::new(Semaphore::new(limits.in_flight));
    let mut code = 0;
    loop {
        let bytes = match frames.next().await {
            Ok(Frame::Line(bytes)) => bytes,
            Ok(Frame::End) => break,
            Ok(Frame::TooLong) => {
                let why = format!("{TOO_LONG} {} bytes", limits.line_bytes);
                line(desk, error_line(None, INVALID_REQUEST, &why)).await;
                continue;
            }
            Err(error) => {
                note(desk, &format!("the input stream failed: {error}")).await;
                code = 1;
                break;
            }
        };
        let outgoing = match prepare(&bytes, &mut lifecycle) {
            Ok(Some(outgoing)) => outgoing,
            Ok(None) => continue,
            Err(Refusal::Answer(answer)) => {
                line(desk, answer).await;
                continue;
            }
            Err(Refusal::Log(why)) => {
                note(desk, &why).await;
                continue;
            }
        };
        drop(bytes);
        // While what waits to be written fills its room, this one message is held, and
        // nothing more is read or forwarded.
        vacancy(desk).await;
        if outgoing.initialize {
            flight.drain(desk).await;
            let outcome = exchange(transport.as_ref(), outgoing, desk).await;
            lifecycle.version = outcome.negotiated;
            lifecycle.session = outcome.session;
        } else {
            // The semaphore is never closed, so a permit is always granted.
            let permit = permits.clone().acquire_owned().await.ok();
            let name = called(&outgoing.kind);
            let (transport, desk) = (transport.clone(), desk.clone());
            flight.start(name, async move {
                let _permit = permit;
                exchange(transport.as_ref(), outgoing, &desk).await;
            });
        }
        flight.reap(desk).await;
    }
    code.max(close(flight, limits.closing, desk).await)
}

/// Queue one protocol line for the output stream, without the credential.
///
/// Every line the relay writes passes here, the service's own and the relay's alike. It waits
/// until the line has room among what waits to be written. Returns false when the line was
/// withheld because it could not be cleaned.
async fn line(desk: &Desk, text: String) -> bool {
    let Some(clean) = desk.secret.redact_line(text) else {
        note(
            desk,
            "withheld a line that repeats the connector credential outside a JSON string",
        )
        .await;
        return false;
    };
    let held = reserve(desk, clean.len()).await;
    // The receiver is gone only when the output stream failed; the relay then stops.
    let _ = desk.lines.send((clean, held)).await;
    true
}

/// Queue one diagnostic for the error stream, once it has room.
async fn note(desk: &Desk, text: &str) {
    let held = reserve(desk, text.len()).await;
    // The receiver lives until every sender is dropped.
    let _ = desk.notes.send((text.to_owned(), held)).await;
}

/// Forward the input until it ends, or until the output stream fails, whichever is first.
///
/// A failed output is noticed at any moment: while a line is awaited, while `initialize` is
/// exchanged, while a message waits for its turn or for room, and while the relay closes.
/// The messages in flight then end with the relay.
async fn read<T: Transport, I: AsyncRead + Unpin>(
    transport: Arc<T>,
    input: I,
    limits: Limits,
    desk: Desk,
) -> u8 {
    tokio::select! {
        code = forward(transport, input, limits, &desk) => code,
        () = desk.lines.closed() => {
            note(&desk, "the output stream failed; the relay stops").await;
            1
        }
    }
}

/// Take the room `length` bytes need among what waits to be written, waiting until it is free.
///
/// The room is given back when the permit is dropped, which the writer does once the text is
/// written. A text longer than the whole room takes all of it.
async fn reserve(desk: &Desk, length: usize) -> Option<OwnedSemaphorePermit> {
    let wanted = u32::try_from(length)
        .unwrap_or(u32::MAX)
        .min(desk.budget)
        .max(1);
    // The semaphore is never closed, so the room is always granted in the end.
    desk.room.clone().acquire_many_owned(wanted).await.ok()
}

/// Wait until what waits to be written leaves room: one byte free, and no text that asked
/// for room before this call still waiting for it.
async fn vacancy(desk: &Desk) {
    // The semaphore is never closed. The permit is given back at once: this only waits.
    drop(desk.room.acquire().await);
}

/// Run the relay: reach the service, then forward messages until the input ends.
///
/// Returns the exit code: 0 when the input ended, 1 when no service could be reached or
/// started, or when a standard stream failed.
pub async fn serve<T, L, C, I, O, E>(relay: Relay<T, L, C>, streams: Streams<I, O, E>) -> u8
where
    T: Transport,
    L: Launcher,
    C: Clock,
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let Relay {
        transport,
        mut launcher,
        loopback,
        start,
        clock,
        limits,
        secret,
    } = relay;
    let Streams {
        stdin,
        stdout,
        mut stderr,
    } = streams;
    let secret = Arc::new(secret);
    let mut progress = Saying {
        stderr: &mut stderr,
        secret: &secret,
    };
    let arrival = start_or_connect(
        transport.as_ref(),
        &mut launcher,
        loopback,
        start,
        &clock,
        &mut progress,
    )
    .await;
    match arrival {
        Ok(Arrival::Connected) => {}
        Ok(Arrival::Started) => {
            say(
                &mut stderr,
                &secret,
                "the okf-jawn server the relay started answers; it is left running when the \
                 relay ends",
            )
            .await;
        }
        Err(why) => {
            say(&mut stderr, &secret, &why).await;
            return 1;
        }
    }
    // A semaphore holds no more permits than this, and one text asks for no more than a `u32`.
    let most = Semaphore::MAX_PERMITS.min(usize::try_from(u32::MAX).unwrap_or(usize::MAX));
    let room = limits.waiting_bytes.min(most).max(1);
    let (lines, queued_lines) = mpsc::channel::<Queued>(QUEUE);
    let (notes, mut queued_notes) = mpsc::channel::<Queued>(QUEUE);
    let desk = Desk {
        lines,
        notes,
        secret: secret.clone(),
        room: Arc::new(Semaphore::new(room)),
        budget: u32::try_from(room).unwrap_or(u32::MAX),
    };
    let noting = async {
        while let Some((text, held)) = queued_notes.recv().await {
            say(&mut stderr, &secret, &text).await;
            // Written: its room is free again.
            drop(held);
        }
    };
    let (code, written, ()) = tokio::join!(
        read(transport, stdin, limits, desk),
        write(queued_lines, stdout),
        noting
    );
    if written.is_err() { 1 } else { code }
}

/// Write one diagnostic line to the error stream, without the credential.
async fn say<E: AsyncWrite + Unpin>(stderr: &mut E, secret: &Credential, text: &str) {
    let text = secret.redact(&format!("okf-jawn mcp: {text}\n"));
    // Nothing can report a failure of the error stream itself.
    let _ = stderr.write_all(text.as_bytes()).await;
    let _ = stderr.flush().await;
}

/// Decide what a finished HTTP answer means; returns the failure, in plain words, if it is one.
async fn settle(
    status: u16,
    body: Body,
    kind: &Kind,
    initialize: bool,
    desk: &Desk,
    seen: &mut Seen,
) -> Option<String> {
    match (status, body) {
        (401, _) => Some(CREDENTIAL_REFUSED.to_owned()),
        (403, _) => Some(ACCESS_REFUSED.to_owned()),
        (200..=299, Body::Empty | Body::Events) => None,
        (200..=299, Body::Json(text)) => deliver(text, kind, initialize, desk, seen)
            .await
            .err()
            .map(str::to_owned),
        (200..=299, Body::Other) => Some(format!(
            "The okf-jawn service answered HTTP {status} with a body that is neither JSON nor \
             an event stream"
        )),
        (_, Body::Json(text)) => refusal(status, text, kind, desk, seen).await,
        (_, Body::Empty | Body::Events | Body::Other) => {
            Some(format!("The okf-jawn service answered HTTP {status}"))
        }
    }
}

/// A JSON body the service sent with a status that is not success.
///
/// A JSON-RPC error answering a request is the request's answer. The service leaves the id
/// out, or writes `null`, when it refused the request before reading it; the request's id, as
/// the client wrote it, is then put into the text (`"id":<id>,` after the opening brace, or in
/// place of the `null`) and no other byte of the text changes. Any other body is a failure,
/// described with the service's own code and message when it has them.
async fn refusal(
    status: u16,
    text: String,
    kind: &Kind,
    desk: &Desk,
    seen: &mut Seen,
) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(&text).ok();
    let message = |pointer: &str| {
        parsed
            .as_ref()
            .and_then(|value| value.pointer(pointer))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    if let Some(envelope) = Envelope::read(&text)
        && envelope.error
    {
        let Kind::Request { id, written } = kind else {
            return Some(format!(
                "The okf-jawn service answered HTTP {status}: {}",
                message("/error/message").unwrap_or_default()
            ));
        };
        let (answer, answers) = match (&envelope.id, envelope.id_at) {
            (_, None) => (with_id(&text, written), true),
            (Some(Value::Null), Some((start, end))) => (
                text.get(..start)
                    .zip(text.get(end..))
                    .map(|(before, after)| format!("{before}{written}{after}")),
                true,
            ),
            (other, Some(_)) => (Some(text), other.as_ref() == Some(id)),
        };
        let Some(answer) = answer else {
            return Some(NOT_A_MESSAGE.to_owned());
        };
        if !line(desk, one_line(answer)).await {
            return Some(ANSWER_WITHHELD.to_owned());
        }
        if answers {
            seen.answered = true;
        }
        return None;
    }
    Some(match (message("/code"), message("/message")) {
        (Some(code), Some(message)) => {
            format!("The okf-jawn service answered HTTP {status} ({code}): {message}")
        }
        _ => format!("The okf-jawn service answered HTTP {status}"),
    })
}

/// The JSON object `text` with the member `"id":<id>` put first, and nothing else changed.
fn with_id(text: &str, id: &str) -> Option<String> {
    let (before, after) = text.split_once('{')?;
    Some(format!("{before}{{\"id\":{id},{after}"))
}

/// Write each queued line whole, followed by a line feed, until every sender is gone.
async fn write<O: AsyncWrite + Unpin>(
    mut queued: mpsc::Receiver<Queued>,
    mut output: O,
) -> std::io::Result<()> {
    while let Some((mut text, held)) = queued.recv().await {
        text.push('\n');
        output.write_all(text.as_bytes()).await?;
        output.flush().await?;
        // Written: its room is free again.
        drop(held);
    }
    Ok(())
}
