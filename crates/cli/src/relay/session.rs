//! The relay loop: read a line, forward it, write each answer as one line.
//!
//! Invariants:
//! - The output stream carries JSON-RPC messages and nothing else, one per line. One task
//!   owns the stream and writes each line whole; diagnostics go to the error stream.
//! - Every request the relay accepts gets a response or an error with its id: the service's
//!   own, or one the relay writes when the service gave none, `initialize` included, and when
//!   its exchange ended abnormally (`guarded`). A failed notification is
//!   logged to the error stream instead. The one exception is a request still in flight when
//!   the relay stops, which is named on the error stream (see "how it ends").
//! - Messages are forwarded concurrently, at most `Limits::in_flight` at a time. Each line
//!   read from the input takes one of that many turns and keeps it until the relay is done
//!   with the line: until its message has finished, or the relay has answered it itself. One
//!   line more is read and waits for a turn, and while it waits no further line is read.
//!   `initialize` is exchanged alone, after the messages in flight have finished, because the
//!   headers of every later message come from its answer.
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
//! - An answer is a request's answer when it names the request's id exactly: a number as the
//!   text the client wrote, never as a 64-bit float, and a string by the characters it
//!   spells (`Kind::answered_by`).
//! - The protocol version the answer to `initialize` names is put in force only when it can
//!   stand in an HTTP header as it is (`message::usable_version`, the check a version in a
//!   request's `_meta` passes). An answer that names another is passed on all the same; no
//!   version is then in force, as when the answer names none, and the error stream is told
//!   so when that answer is passed on, in words that repeat nothing of the version.
//!
//! What the relay holds of the service's answers. One room (`Room`) is the budget for every
//! answer from the moment its bytes are taken from the service until they are written, and
//! for the relay's own lines and diagnostics: 128 MiB, which is `Limits::waiting_bytes` (64
//! MiB) that all texts share and one text more, at most `Limits::answer_bytes` (64 MiB) long,
//! that did not find its room there. The shared part is the length of the longest answer, so
//! that one such answer and everything shorter beside it are read without waiting; the one
//! text more is what lets the longest answer through when the shared part is full. The
//! budget is enforced, not estimated: the transport takes room before it holds a byte, and
//! reads nothing more from the service while there is none, however many messages are in
//! flight. An answer is then read, made one line, cleaned of the credential and given the
//! request's id in the buffer it arrived in; nothing copies it and nothing is built from it,
//! so an answer of many members costs what an answer of one long string costs. While a text
//! waits for the one place beside the shared part, no further message is forwarded: each
//! waits in its own turn, of which there are sixteen, and the read loop reads on, so that it
//! sees the end of the input.
//!
//! What the relay holds besides, which no budget covers:
//! - Each line that has a turn is held, as it was read while it waits to be looked at and as
//!   the message made of it while that is in flight (at most 16 MiB; sixteen turns, 256 MiB),
//!   and one line more that waits for a turn (16 MiB). A message in flight holds its id as
//!   often as four times more (as written, as read, in the name the relay calls the message
//!   by, and for the error it owes if its exchange ends abnormally), which matters only for an
//!   id of megabytes.
//! - One request line at a time is read into a tree of values while it is prepared. A line
//!   of 16 MiB that is one string costs about 50 MiB for that moment; one that is an array of
//!   8 million numbers was measured at 403 MiB.
//! - A buffer that grows may be held twice for a moment, so a growing answer can take half
//!   as much again as its room; an answer whose length the service announced never grows.
//! - What the HTTP library holds for a connection, and a bit for each level an answer is
//!   nested, in a buffer that grows by doubling (a quarter of its length at most).
//!
//! The worst case is therefore about 0.9 GiB: 192 MiB for answers (the budget, and half as
//! much again while buffers grow), 256 MiB for sixteen request lines of 16 MiB, 16 MiB for the
//! line that waits for a turn, and about 400 MiB for the one line being prepared. Ids of 16
//! MiB would bring it to about 1.6 GiB. The service can cause the first 192 MiB of it; the
//! rest takes a client that writes lines of megabytes.
//!
//! Measured on the built relay of the test profile, Windows 11, peak private bytes of the
//! process: one answer of 60 MiB costs 65 MiB when it is one long string, 66 MiB when it is
//! 12.6 million members, and 64 MiB when it is a refusal with an array of 31 million numbers
//! (before this bound: 98, 1,414 and 1,606 MiB). Sixteen answers of 60 MiB in flight to a
//! client that reads nothing cost 127 MiB, long strings and many members alike, and 128 MiB
//! by the time all are written (before: 1,269 MiB for sixteen long strings).
//!
//! How it ends. When the input ends, the relay has `Limits::closing` (two minutes, what the
//! CLI gives one operation) to finish: for the messages in flight to be answered and for every
//! line to be written. Then it returns 0. When the time runs out it returns 1 without waiting
//! for anything, the writer included, and says on the error stream what it leaves: the
//! messages that had not finished, by name, how many lines it had read and not looked at,
//! and how many lines the output had not taken. An output that takes nothing and does not
//! fail is therefore no reason to stay. The last words go straight to the error stream and
//! get two seconds, so an error stream that takes nothing does not keep the relay either.
//! They are bounded and printable whatever the client wrote: an id or a method they quote is
//! cut to 200 characters with a mark that says so, and the names together stay within 5 KiB,
//! with a count of those left out, so the whole stays under 8 KiB. Redaction runs before the
//! cut, so a credential at the cut is not left half shown, and again on the result.
//!
//! Every diagnostic, the last words among them, is printable whoever wrote what it repeats:
//! every control character, every character that changes the direction of text and every
//! invisible one is written as an escape (`printable`, in `say`, the one place a diagnostic is
//! written). A message that was not sent or not delivered is named as the last words name it,
//! its method cut to 200 characters; what the service says is repeated up to 8192 bytes.
//! When the output stream fails, the relay stops at once and returns 1, whatever it was
//! waiting for.
//!
//! The relay learns that its input ended only by reading it, so the read loop never only
//! waits. Whenever it waits for something else (for room for a line it answers itself or for
//! what it has to say about one, for the messages in flight to finish before `initialize`,
//! for the answer to `initialize`), it reads the input ahead meanwhile (`Input::meanwhile`),
//! each line read taking a turn. The closing time starts when the end is read, wherever the
//! loop stands, and what the loop waits for is given up with everything else when the time
//! runs out: the line it was busy with is then named among the messages that had not
//! finished.
//!
//! Known limits:
//! - A forwarded message has no limit in time while the input is open: a tool call may run
//!   long, and an event stream has no fixed length. Sixteen messages that do not end therefore
//!   hold every turn, the relay reads one line more and no further, and it learns that its
//!   input ended only when one of them ends. An output that takes nothing has the same effect
//!   once sixteen lines wait for it, as messages or as lines the relay answers itself: with
//!   more lines still unread, the relay does not learn that its input ended, and stays.
//! - A response the service sends for another request's id is passed on as it is.

use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use okf_jawn_contract::transport::MAX_REQUEST_BODY_BYTES;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::task::{Id, JoinError, JoinSet};

use super::frame::{Frame, Frames};
use super::launch::{Arrival, Clock, Launcher, Progress, StartPolicy, start_or_connect};
use super::message::{
    INTERNAL_ERROR, INVALID_REQUEST, Kind, Lifecycle, Outgoing, Refusal, error_line,
    has_error_object, is_version_2, prepare, spelled, usable_version,
};
use super::room::{Held, Room};
use super::transport::{Body, Transport, TransportError};
use crate::credential::Credential;
use crate::run::Streams;

/// The channels one exchange writes to, the room they share, and the secret kept out of them.
#[derive(Clone)]
struct Desk {
    lines: mpsc::Sender<Held>,
    notes: mpsc::Sender<Held>,
    secret: Arc<Credential>,
    /// The budget for everything on its way out.
    room: Room,
    /// The longest a text of the service may be; a refusal given the request's id included.
    most: usize,
    /// How many lines were queued for the output stream and are not written yet.
    unwritten: Arc<AtomicUsize>,
}

/// The messages in flight: their tasks, and what each is called when it has to be named.
struct Flight {
    tasks: JoinSet<()>,
    left: Left,
}

/// The input as the read loop takes it up: line by line, and read ahead while the loop waits
/// for something else, which is how the end of the input is seen at such a moment.
///
/// Every line read takes a turn before it is handed on, and keeps it until the relay is done
/// with the line. So no more lines are held than there are turns, and one that waits for a
/// turn; while that one waits, nothing more is read.
struct Input<I> {
    frames: Frames<BufReader<I>>,
    /// One turn for each line the relay holds: read ahead, being looked at, or in flight.
    turns: Arc<Semaphore>,
    /// What was read ahead and has its turn, in the order it was read.
    ahead: VecDeque<(Taken, Turn)>,
    /// What was read last and has no turn yet.
    waiting: Option<Taken>,
    /// Told when the input is over; `None` once it has been told.
    ended: Option<oneshot::Sender<()>>,
    left: Left,
}

/// What the relay has not finished, kept where it can still be read when the relay gives up
/// everything else.
type Left = Arc<Mutex<Unfinished>>;

/// The relay's bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Longest input line accepted, in bytes: the contract's request-body limit.
    pub line_bytes: usize,
    /// Most bytes read of one answer: a JSON body, or one event of a stream.
    pub answer_bytes: usize,
    /// Most messages forwarded at the same time, and most lines of the input held at a time
    /// besides one that waits for its turn.
    pub in_flight: usize,
    /// The shared part of the room: most bytes of answers, lines and diagnostics held at
    /// once, from the service to the output, besides one text that found no room there.
    /// While such a text waits, nothing more is taken from the service for it and nothing
    /// more is forwarded from the input.
    pub waiting_bytes: usize,
    /// How long the relay may still take once the input has ended: for the messages in flight
    /// to finish and for every line to be written.
    pub closing: Duration,
}

/// What an exchange learned for the messages that follow it.
#[derive(Debug, Default)]
struct Outcome {
    /// The protocol version an `initialize` answer put in force.
    negotiated: Option<String>,
    /// The session the service issued.
    session: Option<String>,
}

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

/// A piece of work polled so that a panic inside it is caught where it happens and given as
/// the work's end, in the words of the panic, as a task's harness catches one; the work is not
/// polled again after it.
struct Caught<W> {
    work: Pin<Box<W>>,
}

/// What the answers relayed so far say about one exchange.
#[derive(Debug)]
struct Seen<'flag> {
    /// Set once a response or error with the request's id is queued for the output. It is
    /// shared with whoever runs the exchange, so it is still known when the exchange ends
    /// abnormally (`guarded`).
    answered: &'flag AtomicBool,
    /// The protocol version of an `initialize` result, when it can be put in force.
    negotiated: Option<String>,
}

/// One thing read from the input.
#[derive(Debug)]
enum Taken {
    /// One line without its terminator.
    Line(Vec<u8>),
    /// A line longer than the limit; its bytes were dropped.
    TooLong,
    /// The input failed; nothing is read after it.
    Failed(std::io::Error),
}

/// The turn a line of the input holds. `None` only if the turns were closed, which they never
/// are.
type Turn = Option<OwnedSemaphorePermit>;

/// What the relay has not finished.
#[derive(Debug, Default)]
struct Unfinished {
    /// What each message in flight is called.
    flying: HashMap<Id, String>,
    /// What the line is called that the read loop is busy with, while it waits for something.
    here: Option<String>,
    /// How many lines were read ahead and not looked at yet.
    ahead: usize,
}

/// What a request is told whose exchange ended abnormally.
const ABNORMAL: &str = "The relay's exchange of this request with the okf-jawn service ended \
    abnormally, so its answer is not known";
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
/// What the error stream is told when an answer is dropped.
const DROPPED: &str = "dropped an answer of the service that is not a JSON-RPC message";
/// The longest message or code of a refusal, in bytes as the service wrote it, that the relay
/// repeats: more than a person reads of a refusal, and little enough that the relay's own
/// line stays far shorter than an answer.
const MESSAGE_BYTES: usize = 8192;
/// The most characters of an id or a method that the last words quote.
const NAME_CHARS: usize = 200;
/// The most bytes of names that one sentence of the last words holds: with the rest of the
/// last words it stays under 8 KiB.
const NAMES_BYTES: usize = 5120;
/// What a request is told when the service's answer is no message.
const NOT_A_MESSAGE: &str = "The okf-jawn service's answer is not a JSON-RPC message";
/// The longest protocol version, in bytes as the service wrote it, that an `initialize`
/// answer can put in force: it travels in a header of every later message.
const VERSION_BYTES: usize = 256;
/// What the error stream is told when the answer to `initialize` names a protocol version
/// that is not put in force. The words are the relay's own and repeat nothing of the version.
const VERSION_UNUSABLE: &str = "the okf-jawn service's answer to initialize names a protocol \
    version the relay cannot use: it is not a short string of printable ASCII that can stand in \
    an HTTP header as it is. No protocol version is in force for the messages that follow, as \
    if the service had named none";
/// What the error stream is told when a line is withheld.
const WITHHELD: &str =
    "withheld a line that repeats the connector credential outside a JSON string";
/// How many events of one answer may wait for the relay to take them: the fewest a channel
/// holds, because each may be as long as a whole answer.
const AHEAD: usize = 1;
/// How long the messages in flight may still take once the input has ended: what the CLI
/// gives one operation, so a script that pipes its requests in gets its last answers, and a
/// client that only closes the input does not keep the relay for longer than that.
const CLOSING: Duration = Duration::from_secs(120);
/// How long the relay's last words may take when it gives up: the error stream may take
/// nothing either.
const LAST_WORDS: Duration = Duration::from_secs(2);
/// How many answers or diagnostics may wait for their writer.
const QUEUE: usize = 64;
/// What a line is called that the relay refused, when the relay has to name it: it could not
/// be read as a message, so it has no name of its own.
const REFUSED: &str = "a line the relay refused";
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
            Ok((id, ())) => self.forget(id),
            Err(error) => {
                self.forget(error.id());
                note(
                    desk,
                    &format!("a message's exchange ended abnormally: {error}"),
                )
                .await;
            }
        }
    }

    /// Forget the name of a message that has finished.
    fn forget(&self, id: Id) {
        // A lock that a panic poisoned is left alone: the names are only ever read to be said.
        if let Ok(mut left) = self.left.lock() {
            left.flying.remove(&id);
        }
    }

    fn new(left: Left) -> Self {
        Self {
            tasks: JoinSet::new(),
            left,
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
        let record = self.left.clone();
        let work = async move {
            work.await;
            // Finished: from here it is none of the messages the relay would leave, whether
            // or not the read loop has come round to forget it. Its name is on record by now,
            // because the record is locked while the task is started.
            if let (Some(id), Ok(mut left)) = (tokio::task::try_id(), record.lock()) {
                left.flying.remove(&id);
            }
        };
        if let Ok(mut left) = self.left.lock() {
            let id = self.tasks.spawn(work).id();
            left.flying.insert(id, name);
        } else {
            self.tasks.spawn(work);
        }
    }
}

impl<I: AsyncRead + Unpin> Input<I> {
    /// Do `work` for the line of the input called `name`, and read the input ahead for as
    /// long as that takes. While it takes, the line is on record under that name, so the
    /// relay can still name it if it has to give the work up.
    async fn busy<T>(&mut self, name: String, work: impl Future<Output = T>) -> T {
        self.record(Some(name));
        let done = self.meanwhile(work).await;
        self.record(None);
        done
    }

    /// The input is over: whoever counts the closing time is told, once.
    fn end(&mut self) {
        if let Some(ended) = self.ended.take() {
            // Nobody listens only when the relay is already being given up.
            let _ = ended.send(());
        }
    }

    /// Read one line more ahead and give it a turn; at the end of the input, say so instead.
    ///
    /// Giving this up loses nothing: a line that is half read stays with the frames, and a
    /// line that is read and waits for a turn stays here.
    async fn further(&mut self) {
        if self.over() {
            return;
        }
        if self.waiting.is_none() {
            let taken = match self.frames.next().await {
                Ok(Frame::Line(bytes)) => Taken::Line(bytes),
                Ok(Frame::TooLong) => Taken::TooLong,
                Ok(Frame::End) => {
                    self.end();
                    return;
                }
                Err(error) => {
                    self.end();
                    Taken::Failed(error)
                }
            };
            self.waiting = Some(taken);
            self.tell();
        }
        // The semaphore is never closed, so a turn is always given in the end.
        let turn = self.turns.clone().acquire_owned().await.ok();
        if let Some(taken) = self.waiting.take() {
            self.ahead.push_back((taken, turn));
        }
    }

    /// Do `work`, and read the input ahead for as long as that takes: `work` comes first
    /// whenever it can go on, and the input is read only while it waits. This is what every
    /// wait of the read loop goes through, so that the end of the input is seen, and the
    /// closing time started, whatever the loop waits for.
    async fn meanwhile<T>(&mut self, work: impl Future<Output = T>) -> T {
        let mut work = std::pin::pin!(work);
        while !self.over() {
            tokio::select! {
                biased;
                done = &mut work => return done,
                () = self.further() => {}
            }
        }
        work.await
    }

    fn new(input: I, limits: &Limits, left: Left, ended: oneshot::Sender<()>) -> Self {
        Self {
            frames: Frames::new(BufReader::new(input), limits.line_bytes),
            turns: Arc::new(Semaphore::new(limits.in_flight)),
            ahead: VecDeque::new(),
            waiting: None,
            ended: Some(ended),
            left,
        }
    }

    /// The next thing the input holds, with its turn: what was read ahead first, and
    /// otherwise what is read now. `None` when the input is over and nothing of it is left.
    async fn next(&mut self) -> Option<(Taken, Turn)> {
        while self.ahead.is_empty() && !self.over() {
            self.further().await;
        }
        let first = self.ahead.pop_front();
        self.tell();
        first
    }

    /// Whether the input is over and nothing more will be read of it.
    fn over(&self) -> bool {
        self.ended.is_none() && self.waiting.is_none()
    }

    /// Put on record what the read loop is busy with.
    fn record(&self, here: Option<String>) {
        // A lock that a panic poisoned is left alone: the record is only ever read to be said.
        if let Ok(mut left) = self.left.lock() {
            left.here = here;
        }
    }

    /// Put on record how many lines are read and not looked at yet.
    fn tell(&self) {
        let lines = self
            .ahead
            .iter()
            .map(|(taken, _)| taken)
            .chain(&self.waiting)
            .filter(|taken| !matches!(taken, Taken::Failed(_)))
            .count();
        if let Ok(mut left) = self.left.lock() {
            left.ahead = lines;
        }
    }
}

impl<E: AsyncWrite + Unpin> Progress for Saying<'_, E> {
    async fn say(&mut self, text: &str) {
        say(self.stderr, self.secret, text).await;
    }
}

impl<W: Future> Future for Caught<W> {
    type Output = Result<W::Output, String>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let work = &mut self.get_mut().work;
        // Nothing the work shares with the rest of the relay is left half changed by a panic:
        // its channels, its room and the flag of its answer are each changed in one step.
        match std::panic::catch_unwind(AssertUnwindSafe(|| work.as_mut().poll(context))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(done)) => Poll::Ready(Ok(done)),
            Err(payload) => Poll::Ready(Err(panic_words(payload.as_ref()))),
        }
    }
}

impl Seen<'_> {
    /// Note that the request has had its answer.
    fn answer(&self) {
        self.answered.store(true, Ordering::SeqCst);
    }

    /// Whether the request has had its answer.
    fn answered(&self) -> bool {
        self.answered.load(Ordering::SeqCst)
    }
}

impl Default for Limits {
    /// The contract's request-body limit for a line; four times that for an answer; sixteen
    /// messages in flight; for the shared part of the room, the length of the longest answer;
    /// and two minutes to close.
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

/// The first [`NAME_CHARS`] characters of `text`, and how many characters come after them.
fn head(text: &str) -> (&str, usize) {
    let end = text
        .char_indices()
        .nth(NAME_CHARS)
        .map_or(text.len(), |(at, _)| at);
    text.split_at_checked(end)
        .map_or((text, 0), |(head, rest)| (head, rest.chars().count()))
}

/// The mark that says how much of a name was cut off; nothing when nothing was.
fn cut(rest: usize) -> String {
    if rest == 0 {
        String::new()
    } else {
        format!(" [cut: {rest} more characters]")
    }
}

/// `text` with every character a reader cannot see as it stands written as a visible escape
/// (`\u{202e}`): the control characters (C0, DEL and C1), the characters that change the
/// direction of text, the invisible ones (zero-width characters, the byte order mark, the soft
/// hyphen, fillers, separators other than the space) and the marks that join the character
/// before them. That is every character the standard library's debug escaping writes as an
/// escape; the quote, the apostrophe and the backslash, which it escapes only to quote a
/// string, stay as they are.
fn printable(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for letter in text.chars() {
        let visible = if letter.is_ascii() {
            letter == ' ' || letter.is_ascii_graphic()
        } else {
            letter.escape_debug().len() == 1
        };
        if visible {
            shown.push(letter);
        } else {
            shown.extend(letter.escape_unicode());
        }
    }
    shown
}

/// What a message in flight is called when the relay has to name it. The id or the method is
/// the client's, so it is cleaned of the credential first, then cut, then made printable.
fn called(kind: &Kind, secret: &Credential) -> String {
    match kind {
        Kind::Request { written, .. } => {
            let clean = secret.redact(written);
            let (first, rest) = head(&clean);
            format!("request {}{}", printable(first), cut(rest))
        }
        Kind::Notification { method } => {
            let clean = secret.redact(method);
            let (first, rest) = head(&clean);
            format!("notification {first:?}{}", cut(rest))
        }
        Kind::Response => "a response to the service".to_owned(),
    }
}

/// The words a panic was raised with, when it had some.
fn panic_words(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|words| (*words).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without words".to_owned())
}

/// A message as the subject of a sentence of the error stream: `the notification "..."` or
/// `the request ...`, named by [`called`] (cleaned of the credential, cut and made
/// printable), or `a response to the service`.
fn subject(kind: &Kind, secret: &Credential) -> String {
    match kind {
        Kind::Response => called(kind, secret),
        Kind::Request { .. } | Kind::Notification { .. } => format!("the {}", called(kind, secret)),
    }
}

/// Write one answer of the service to the client, if it is a JSON-RPC message.
///
/// # Errors
/// Returns, in plain words, why it was not written. Anything that is not a message is dropped
/// and logged: the output stream carries protocol messages only.
async fn deliver(
    mut data: Held,
    kind: &Kind,
    initialize: bool,
    desk: &Desk,
    seen: &mut Seen<'_>,
) -> Result<(), &'static str> {
    // The text itself is passed on: only the line breaks between its tokens are removed.
    let Some(noted) = data
        .one_line()
        .filter(|noted| is_version_2(data.as_str(), noted))
    else {
        // Said without waiting for room: more of the answer may be coming.
        remark(desk, DROPPED).await;
        return Err(NOT_A_MESSAGE);
    };
    let answers = noted.method.is_none()
        && noted
            .id
            .and_then(|(start, end)| data.as_str().get(start..end))
            .is_some_and(|id| kind.answered_by(id));
    // Where the answer to `initialize` names a protocol version, whatever stands there.
    let named = noted.version.filter(|_| answers && initialize);
    // The version is checked here, where it is taken from the answer, by the check a version
    // in a request's `_meta` passes: it goes into a header of every later message.
    let negotiated =
        spelled(data.as_str(), named, VERSION_BYTES).filter(|version| usable_version(version));
    // The answer is passed on whatever version it names: the relay does not judge it.
    if !line(desk, data).await {
        return Err(ANSWER_WITHHELD);
    }
    if answers {
        seen.answer();
        if initialize {
            if named.is_some() && negotiated.is_none() {
                remark(desk, VERSION_UNUSABLE).await;
            }
            seen.negotiated = negotiated;
        }
    }
    Ok(())
}

/// Send one message and relay its answers; then settle what the relay still owes the client.
async fn exchange<T: Transport>(
    transport: &T,
    outgoing: Outgoing,
    desk: &Desk,
    answered: &AtomicBool,
) -> Outcome {
    let Outgoing {
        post,
        kind,
        initialize,
    } = outgoing;
    let (events, mut arriving) = mpsc::channel::<Held>(AHEAD);
    let streamed = async {
        let mut seen = Seen {
            answered,
            negotiated: None,
        };
        while let Some(data) = arriving.recv().await {
            // What is not passed on is said on the error stream by `deliver` and `line`.
            let _ = deliver(data, &kind, initialize, desk, &mut seen).await;
        }
        seen
    };
    let (reply, mut seen) = tokio::join!(transport.send(post, events, &desk.room), streamed);
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
        (Kind::Request { written, .. }, failure) if !seen.answered() => {
            let why = failure.unwrap_or_else(|| {
                "The okf-jawn service ended its answer without a response to this request"
                    .to_owned()
            });
            own(desk, error_line(Some(written), INTERNAL_ERROR, &why)).await;
            seen.answer();
        }
        (Kind::Request { .. }, Some(why)) => {
            note(desk, &format!("after a request was answered: {why}")).await;
        }
        (Kind::Notification { .. } | Kind::Response, Some(why)) => {
            let named = subject(&kind, &desk.secret);
            note(desk, &format!("{named} was not delivered: {why}")).await;
        }
        (_, None) => {}
    }
    Outcome {
        negotiated: seen.negotiated,
        session,
    }
}

/// Exchange one message so that an exchange that ends abnormally (a panic in the transport or
/// in the relay's own handling of the answer) is caught where it happens, and the client is
/// told.
///
/// A request then gets the relay's own error with its id, unless its answer was queued
/// before the exchange ended; the error stream says what happened; and the message is over,
/// with nothing learned for the messages after it. A notification or a response gets the
/// error stream's line alone, as when it fails. The exchange runs in the caller's task, as it
/// did before, so nothing about when a message is forwarded changes.
async fn guarded(transport: &impl Transport, outgoing: Outgoing, desk: &Desk) -> Outcome {
    let owed = match &outgoing.kind {
        Kind::Request { written, .. } => Some(written.clone()),
        Kind::Notification { .. } | Kind::Response => None,
    };
    let answered = AtomicBool::new(false);
    let caught = Caught {
        work: Box::pin(exchange(transport, outgoing, desk, &answered)),
    };
    match caught.await {
        Ok(outcome) => outcome,
        Err(why) => {
            if let Some(written) = owed.filter(|_| !answered.load(Ordering::SeqCst)) {
                own(desk, error_line(Some(&written), INTERNAL_ERROR, ABNORMAL)).await;
            }
            note(
                desk,
                &format!("a message's exchange ended abnormally: {why}"),
            )
            .await;
            Outcome::default()
        }
    }
}

/// Read the input to its end, forwarding each message, then let what is in flight finish;
/// returns the exit code.
///
/// Every wait of this loop that is not for the input itself goes through `Input::busy` or
/// `Input::meanwhile`, which read the input ahead meanwhile: a loop that only waited, for
/// room for one of the relay's own lines or for the answer to `initialize`, would not see
/// its input end, and the closing time would never start.
async fn forward<T: Transport, I: AsyncRead + Unpin>(
    transport: Arc<T>,
    input: I,
    limits: Limits,
    desk: &Desk,
    left: Left,
    ended: oneshot::Sender<()>,
) -> u8 {
    let mut input = Input::new(input, &limits, left.clone(), ended);
    let mut flight = Flight::new(left);
    let mut lifecycle = Lifecycle::default();
    let mut code = 0;
    // The turn of a line is given back when the relay is done with the line: at the end of
    // its pass through this loop, or when its message has finished.
    while let Some((taken, turn)) = input.next().await {
        let bytes = match taken {
            Taken::Line(bytes) => bytes,
            Taken::TooLong => {
                let why = format!("{TOO_LONG} {} bytes", limits.line_bytes);
                let answer = error_line(None, INVALID_REQUEST, &why);
                input.busy(REFUSED.to_owned(), own(desk, answer)).await;
                continue;
            }
            Taken::Failed(error) => {
                // Nothing is read after a failure, and the closing time is already counted.
                note(desk, &format!("the input stream failed: {error}")).await;
                code = 1;
                break;
            }
        };
        let outgoing = match prepare(&bytes, &mut lifecycle) {
            Ok(Some(outgoing)) => outgoing,
            Ok(None) => continue,
            Err(Refusal::Answer(answer)) => {
                input.busy(REFUSED.to_owned(), own(desk, answer)).await;
                continue;
            }
            Err(Refusal::Log(kind, why)) => {
                let said = format!("{} was not sent: {why}", subject(&kind, &desk.secret));
                input.busy(REFUSED.to_owned(), note(desk, &said)).await;
                continue;
            }
        };
        drop(bytes);
        let name = called(&outgoing.kind, &desk.secret);
        if outgoing.initialize {
            let alone = async {
                flight.drain(desk).await;
                desk.room.vacancy().await;
                guarded(transport.as_ref(), outgoing, desk).await
            };
            let outcome = input.busy(name, alone).await;
            lifecycle.version = outcome.negotiated;
            lifecycle.session = outcome.session;
        } else {
            let (transport, desk) = (transport.clone(), desk.clone());
            flight.start(name, async move {
                let _turn = turn;
                // While a text waits for room, this message is held here and not forwarded.
                // The read loop goes on meanwhile, so that it sees the end of the input.
                desk.room.vacancy().await;
                guarded(transport.as_ref(), outgoing, &desk).await;
            });
        }
        input.meanwhile(flight.reap(desk)).await;
    }
    // The input is over and the closing time is counted: what is in flight has that long.
    flight.drain(desk).await;
    code
}

/// Say on the error stream what the relay leaves when its closing time has run out: the
/// messages that had not finished, by name, how many lines it had read and not looked at, and
/// how many lines the output had not taken. It writes to the stream itself: nothing of the
/// session is left to queue it with.
async fn leave<E: AsyncWrite + Unpin>(
    stderr: &mut E,
    secret: &Credential,
    after: Duration,
    left: &Left,
    lines: usize,
) {
    let (mut names, ahead): (Vec<String>, usize) = left
        .lock()
        .map(|left| {
            let names = left.flying.values().chain(&left.here).cloned().collect();
            (names, left.ahead)
        })
        .unwrap_or_default();
    names.sort();
    if !names.is_empty() {
        let (mut quoted, mut size, mut over) = (Vec::new(), 0_usize, 0_usize);
        for name in &names {
            if size.saturating_add(name.len()) <= NAMES_BYTES {
                size = size.saturating_add(name.len());
                quoted.push(name.as_str());
            } else {
                over = over.saturating_add(1);
            }
        }
        let more = if over > 0 {
            format!(" and {over} more, not named here")
        } else {
            String::new()
        };
        let text = format!(
            "the input ended and {} of its messages had not finished {after:?} later; the relay \
             stops and leaves them unanswered: {}{more}",
            names.len(),
            quoted.join(", ")
        );
        say(stderr, secret, &text).await;
    }
    if ahead > 0 {
        let text = format!(
            "the input ended and {ahead} of its lines had not been looked at {after:?} later; \
             the relay stops and leaves them unanswered"
        );
        say(stderr, secret, &text).await;
    }
    if lines > 0 {
        let text = format!(
            "the input ended and {lines} of the lines for the output had not been written \
             {after:?} later; the relay stops without writing them"
        );
        say(stderr, secret, &text).await;
    }
}

/// Queue one protocol line for the output stream, without the credential.
///
/// Every line the relay writes passes here, the service's own and the relay's alike. The line
/// holds its room already and is cleaned where it stands, so nothing here waits for room.
/// Returns false when the line was withheld because it could not be cleaned.
async fn line(desk: &Desk, text: Held) -> bool {
    let Some(clean) = text.cleaned(&desk.secret) else {
        remark(desk, WITHHELD).await;
        return false;
    };
    desk.unwritten.fetch_add(1, Ordering::SeqCst);
    // The receiver is gone only when the output stream failed; the relay then stops.
    let _ = desk.lines.send(clean).await;
    true
}

/// Queue one diagnostic for the error stream, once it has room.
///
/// It waits for room, so it is for the moments when nothing of the message it is about is
/// still to come: the end of an exchange, and the read loop, which reads its input ahead
/// while it waits here. In the middle of an answer the relay says a [`remark`].
async fn note(desk: &Desk, text: &str) {
    let held = desk.room.hold(text.to_owned()).await;
    // The receiver lives until every sender is dropped.
    let _ = desk.notes.send(held).await;
}

/// Queue one line of the relay's own for the output stream, once it has room; returns false
/// when it was withheld. It waits for room as [`note`] does, and is for the same moments.
async fn own(desk: &Desk, text: String) -> bool {
    let held = desk.room.hold(text).await;
    line(desk, held).await
}

/// Forward the input until it ends, or until the output stream fails, whichever is first.
///
/// A failed output is noticed at any moment: while a line is awaited, while `initialize` is
/// exchanged, while a message waits for its turn or for room, and while the relay closes.
/// The messages in flight then end with the relay. `ended` is told when the end of the input
/// is read, and dropped untold when the reading is given up here.
async fn read<T: Transport, I: AsyncRead + Unpin>(
    transport: Arc<T>,
    input: I,
    limits: Limits,
    desk: Desk,
    left: Left,
    ended: oneshot::Sender<()>,
) -> u8 {
    tokio::select! {
        code = forward(transport, input, limits, &desk, left, ended) => code,
        () = desk.lines.closed() => {
            note(&desk, "the output stream failed; the relay stops").await;
            1
        }
    }
}

/// Queue one diagnostic of fixed words for the error stream. It takes no room and so never
/// waits for any, which is what lets the relay say it while an answer is still arriving; the
/// queue bounds how many wait.
async fn remark(desk: &Desk, text: &'static str) {
    // The receiver lives until every sender is dropped.
    let _ = desk.notes.send(desk.room.remark(text)).await;
}

/// Run the relay: reach the service, then forward messages until the input ends.
///
/// Returns the exit code: 0 when the input ended and everything was written, 1 when no
/// service could be reached or started, when a standard stream failed, or when the closing
/// time ran out with messages unfinished or lines unwritten.
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
    let (lines, queued_lines) = mpsc::channel::<Held>(QUEUE);
    let (notes, mut queued_notes) = mpsc::channel::<Held>(QUEUE);
    let unwritten = Arc::new(AtomicUsize::new(0));
    let left = Left::default();
    let desk = Desk {
        lines,
        notes,
        secret: secret.clone(),
        room: Room::new(limits.waiting_bytes),
        most: limits.answer_bytes,
        unwritten: unwritten.clone(),
    };
    let (ended, closing) = oneshot::channel();
    let mut session = Box::pin(async {
        let noting = async {
            while let Some(text) = queued_notes.recv().await {
                say(&mut stderr, &secret, text.as_str()).await;
                // Written: its room is free again.
                drop(text);
            }
        };
        let (code, written, ()) = tokio::join!(
            read(transport, stdin, limits, desk, left.clone(), ended),
            write(queued_lines, stdout, &unwritten),
            noting
        );
        if written.is_err() { 1 } else { code }
    });
    // While the input is open the relay stays for as long as its client keeps it.
    let early = tokio::select! {
        code = &mut session => Some(code),
        // Told, or the reading was given up because the output failed: either way it closes.
        _ = closing => None,
    };
    if let Some(code) = early {
        return code;
    }
    // From the end of the input, everything left has the closing time and no more.
    if let Ok(code) = tokio::time::timeout(limits.closing, &mut session).await {
        return code;
    }
    // Given up as it stands: nothing is waited for, the writer included.
    drop(session);
    let lines = unwritten.load(Ordering::SeqCst);
    // An error stream that takes nothing must not keep the relay either.
    let _ = tokio::time::timeout(
        LAST_WORDS,
        leave(&mut stderr, &secret, limits.closing, &left, lines),
    )
    .await;
    1
}

/// Write one diagnostic line to the error stream, without the credential, and with every
/// character a reader cannot see written as an escape ([`printable`]), whoever wrote it: the
/// relay's own words, a client's id or method, what the service says. So a diagnostic is one
/// line, and nothing in it moves the cursor, hides text or turns its direction.
async fn say<E: AsyncWrite + Unpin>(stderr: &mut E, secret: &Credential, text: &str) {
    let text = secret.redact(&format!("okf-jawn mcp: {}\n", printable(text)));
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
    seen: &mut Seen<'_>,
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
/// the client wrote it, is then put into the text where it stands (`"id":<id>,` after the
/// opening brace, or in place of the `null`) and no other byte of the text changes. Any other
/// body is a failure, described with the service's own code and message when it has them and
/// they are short enough to repeat. The body is read as an answer is: in one pass and into
/// nothing.
async fn refusal(
    status: u16,
    mut text: Held,
    kind: &Kind,
    desk: &Desk,
    seen: &mut Seen<'_>,
) -> Option<String> {
    let plain = format!("The okf-jawn service answered HTTP {status}");
    let Some(noted) = text.one_line() else {
        return Some(plain);
    };
    if let Some(opening) = noted.opening
        && is_version_2(text.as_str(), &noted)
        && has_error_object(text.as_str(), &noted)
    {
        let Kind::Request { written, .. } = kind else {
            let message = spelled(text.as_str(), noted.error_message, MESSAGE_BYTES);
            return Some(format!("{plain}: {}", message.unwrap_or_default()));
        };
        let id = noted
            .id
            .and_then(|(start, end)| Some((start..end, text.as_str().get(start..end)?)));
        let answers = match id {
            // The service named an id: the error is passed on, whoever it answers.
            Some((_, named)) if named != "null" => kind.answered_by(named),
            missing => {
                let placed = if let Some((null, _)) = missing {
                    text.splice(null, written, desk.most).await
                } else {
                    let member = format!("\"id\":{written},");
                    text.splice(opening..opening, &member, desk.most).await
                };
                if !placed {
                    return Some(format!(
                        "{plain} with an error that is too long to pass on with the request's id"
                    ));
                }
                true
            }
        };
        if !line(desk, text).await {
            return Some(ANSWER_WITHHELD.to_owned());
        }
        if answers {
            seen.answer();
        }
        return None;
    }
    let code = spelled(text.as_str(), noted.code, MESSAGE_BYTES);
    let message = spelled(text.as_str(), noted.message, MESSAGE_BYTES);
    Some(match (code, message) {
        (Some(code), Some(message)) => format!("{plain} ({code}): {message}"),
        _ => plain,
    })
}

/// Write each queued line whole, followed by a line feed, until every sender is gone.
async fn write<O: AsyncWrite + Unpin>(
    mut queued: mpsc::Receiver<Held>,
    mut output: O,
    unwritten: &AtomicUsize,
) -> std::io::Result<()> {
    while let Some(text) = queued.recv().await {
        // The line feed is written after the text, not added to it: adding a byte to a
        // buffer that is full would copy the whole line.
        output.write_all(text.as_str().as_bytes()).await?;
        output.write_all(b"\n").await?;
        output.flush().await?;
        unwritten.fetch_sub(1, Ordering::SeqCst);
        // Written: its room is free again.
        drop(text);
    }
    Ok(())
}
