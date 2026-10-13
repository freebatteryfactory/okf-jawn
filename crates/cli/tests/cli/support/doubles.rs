//! Doubles of the relay's two seams, and a runner of the relay over in-memory streams.
//!
//! The transport double records every message it is asked to send and answers through a
//! handler the test supplies. It encodes only what the `Transport` doc states: events are
//! passed on as they arrive, and a failure leaves the delivered events delivered. The launcher
//! double counts launches and reports the child state it was given.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use okf_jawn_cli::relay::{
    Body, ChildState, Clock, Launcher, Limits, Post, Probe, Progress, Relay, Reply, StartPolicy,
    Transport, TransportError, serve,
};
use okf_jawn_cli::{Credential, Streams};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use super::{Outcome, Ran, TOKEN};
use crate::check::some;

/// An output stream that fails every write.
pub struct Broken;

/// A clock that tests move: time passes only when something waits on it.
///
/// A sleep moves the clock straight to its end, unless work that costs time ([`FakeClock::spend`])
/// ends sooner: then the clock moves to that end first, so the work is seen to finish before
/// the sleep does, and a tie goes to the work. Nothing here waits in real time.
#[derive(Clone, Default)]
pub struct FakeClock {
    state: Arc<Mutex<Times>>,
}

/// What a relay said while it waited for a server, with the clock's time at each word.
pub struct Reports {
    clock: FakeClock,
    pub said: Vec<(Duration, String)>,
}

/// A wait on a [`FakeClock`]: a sleep, or work that costs time.
pub struct Passing {
    clock: FakeClock,
    length: Duration,
    /// Whether this is work, which never moves the clock itself.
    work: bool,
    /// The registration of the work's end with the clock, once polled.
    key: Option<u64>,
    until: Option<Duration>,
}

/// The state of a [`FakeClock`]: its time, and when each piece of work in progress ends.
#[derive(Default)]
struct Times {
    now: Duration,
    ends: Vec<(u64, Duration)>,
    keys: u64,
    /// How many waits of no time were asked for in a row.
    idle: u32,
}

/// A launcher that counts its launches.
#[derive(Clone)]
pub struct FakeLauncher {
    pub launches: Arc<AtomicUsize>,
    pub outcome: Result<(), String>,
    pub state: ChildState,
}

/// A transport whose answers a test scripts.
pub struct FakeTransport {
    handler: Handler,
    posts: Mutex<Vec<Post>>,
    probes: Mutex<VecDeque<Probe>>,
    otherwise: Probe,
    probed: AtomicUsize,
    /// What each liveness check costs on the clock, in order; then `cost`.
    costs: Mutex<VecDeque<Duration>>,
    cost: Duration,
    clock: Option<FakeClock>,
}

/// An output stream that hands each write to a channel, so a test can watch it mid-run.
pub struct Piped(pub mpsc::UnboundedSender<Vec<u8>>);

/// One scripted answer: the events of a stream, then the reply or the failure.
pub struct Script {
    pub events: Vec<String>,
    pub reply: Result<Reply, TransportError>,
}

/// Everything a relay run is given, with defaults a test overrides.
pub struct Setup {
    pub transport: Arc<FakeTransport>,
    pub launcher: FakeLauncher,
    pub loopback: bool,
    pub start: StartPolicy,
    /// The clock the wait for a started server runs on; its sleeps take no real time.
    pub clock: FakeClock,
    pub limits: Limits,
    /// The connector credential the relay holds.
    pub token: &'static str,
}

type Handler = Box<dyn Fn(Post, mpsc::Sender<String>) -> Sending + Send + Sync>;

/// The future of one scripted send.
pub type Sending = Pin<Box<dyn Future<Output = Result<Reply, TransportError>> + Send>>;

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        self.state.lock().map_or(Duration::ZERO, |times| times.now)
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> {
        self.passing(duration, false)
    }
}

impl FakeClock {
    fn passing(&self, length: Duration, work: bool) -> Passing {
        Passing {
            clock: self.clone(),
            length,
            work,
            key: None,
            until: None,
        }
    }

    /// Work that takes `cost` on this clock, such as a liveness check that is slow to answer.
    pub fn spend(&self, cost: Duration) -> Passing {
        self.passing(cost, true)
    }
}

impl Drop for Passing {
    fn drop(&mut self) {
        if let (Some(key), Ok(mut times)) = (self.key, self.clock.state.lock()) {
            times.ends.retain(|(other, _)| *other != key);
        }
    }
}

impl Future for Passing {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let Ok(mut times) = this.clock.state.lock() else {
            return Poll::Ready(());
        };
        let until = *this
            .until
            .get_or_insert_with(|| times.now.saturating_add(this.length));
        if this.work && this.key.is_none() {
            times.keys = times.keys.saturating_add(1);
            let key = times.keys;
            times.ends.push((key, until));
            this.key = Some(key);
        }
        if this.length.is_zero() && !this.work {
            // A wait of no time moves nothing. A relay that asks for it without end, while
            // the clock stands still, would spin without ever yielding: that is a failed
            // test, not one that never ends.
            times.idle = times.idle.saturating_add(1);
            assert!(
                times.idle < 100_000,
                "the clock was asked 100,000 times to wait for no time while it stood still"
            );
        }
        if times.now >= until {
            return Poll::Ready(());
        }
        if !this.work {
            // A sleep moves the clock: to the end of the work that ends first, or to its own.
            let first = times.ends.iter().map(|(_, end)| *end).min();
            let (to, over) = match first {
                Some(end) if end <= until => (times.now.max(end), false),
                _ => (until, true),
            };
            if to > times.now {
                times.idle = 0;
            }
            times.now = to;
            // No wait a test asks for comes near a day. A relay that waits without end would
            // move the clock for ever without yielding: a failed test, not an endless one.
            assert!(
                to < Duration::from_hours(24),
                "the clock was moved past a day: the wait has no end"
            );
            if over {
                return Poll::Ready(());
            }
        }
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

impl Progress for Reports {
    async fn say(&mut self, text: &str) {
        self.said.push((self.clock.now(), text.to_owned()));
    }
}

impl Reports {
    pub fn on(clock: &FakeClock) -> Self {
        Self {
            clock: clock.clone(),
            said: Vec::new(),
        }
    }
}

impl AsyncWrite for Broken {
    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
    }
}

impl Default for FakeLauncher {
    /// A launcher whose launch succeeds and whose process keeps running.
    fn default() -> Self {
        Self {
            launches: Arc::new(AtomicUsize::new(0)),
            outcome: Ok(()),
            state: ChildState::Running,
        }
    }
}

impl FakeLauncher {
    pub fn launched(&self) -> usize {
        self.launches.load(Ordering::SeqCst)
    }
}

impl Launcher for FakeLauncher {
    fn launch(&mut self) -> Result<(), String> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        self.outcome.clone()
    }

    fn state(&mut self) -> ChildState {
        self.state.clone()
    }
}

impl FakeTransport {
    /// A service that answers every request with its method and accepts everything else.
    pub fn echoing() -> Arc<Self> {
        Self::scripted(|message| match (message.get("id"), message.get("method")) {
            (Some(id), Some(method)) => {
                answered(&json!({"jsonrpc": "2.0", "id": id, "result": {"method": method}}))
            }
            _ => accepted(),
        })
    }

    /// A transport that answers each send through `handler`; the service is up.
    pub fn new(
        handler: impl Fn(Post, mpsc::Sender<String>) -> Sending + Send + Sync + 'static,
    ) -> Arc<Self> {
        Self::probing(handler, Vec::new(), Probe::Up)
    }

    /// The messages sent so far, in the order they were sent.
    pub fn posts(&self) -> Outcome<Vec<Post>> {
        Ok(self
            .posts
            .lock()
            .map_err(|_| "the transport's record is poisoned")?
            .clone())
    }

    /// How many times the relay asked whether the service is up.
    pub fn probed(&self) -> usize {
        self.probed.load(Ordering::SeqCst)
    }

    /// A transport whose liveness checks answer `probes` in order and then `otherwise`, and
    /// take time on `clock`: `costs` in order, then `cost` each.
    pub fn costing(
        probes: Vec<Probe>,
        otherwise: Probe,
        clock: &FakeClock,
        costs: Vec<Duration>,
        cost: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            handler: Box::new(|_, _| Box::pin(async { accepted().reply })),
            posts: Mutex::new(Vec::new()),
            probes: Mutex::new(probes.into()),
            otherwise,
            probed: AtomicUsize::new(0),
            costs: Mutex::new(costs.into()),
            cost,
            clock: Some(clock.clone()),
        })
    }

    /// A transport whose liveness checks answer `probes` in order and then `otherwise`.
    pub fn probing(
        handler: impl Fn(Post, mpsc::Sender<String>) -> Sending + Send + Sync + 'static,
        probes: Vec<Probe>,
        otherwise: Probe,
    ) -> Arc<Self> {
        Arc::new(Self {
            handler: Box::new(handler),
            posts: Mutex::new(Vec::new()),
            probes: Mutex::new(probes.into()),
            otherwise,
            probed: AtomicUsize::new(0),
            costs: Mutex::new(VecDeque::new()),
            cost: Duration::ZERO,
            clock: None,
        })
    }

    /// A transport that answers each message with the script `script` gives for its JSON.
    pub fn scripted(script: impl Fn(&Value) -> Script + Send + Sync + 'static) -> Arc<Self> {
        Self::new(move |post, events| {
            let script = serde_json::from_slice::<Value>(&post.body).map_or_else(
                |_| broken("the double was sent a body that is not JSON"),
                |message| script(&message),
            );
            Box::pin(async move {
                for event in script.events {
                    // The relay stops listening only when its output failed.
                    let _ = events.send(event).await;
                }
                script.reply
            })
        })
    }
}

impl Transport for FakeTransport {
    fn probe(&self) -> impl Future<Output = Probe> + Send {
        self.probed.fetch_add(1, Ordering::SeqCst);
        let answer = self
            .probes
            .lock()
            .ok()
            .and_then(|mut probes| probes.pop_front())
            .unwrap_or_else(|| self.otherwise.clone());
        let cost = self
            .costs
            .lock()
            .ok()
            .and_then(|mut costs| costs.pop_front())
            .unwrap_or(self.cost);
        let spent = self.clock.as_ref().map(|clock| clock.spend(cost));
        async move {
            if let Some(spent) = spent {
                spent.await;
            }
            answer
        }
    }

    async fn send(
        &self,
        post: Post,
        events: mpsc::Sender<String>,
    ) -> Result<Reply, TransportError> {
        if let Ok(mut posts) = self.posts.lock() {
            posts.push(post.clone());
        }
        (self.handler)(post, events).await
    }
}

impl AsyncWrite for Piped {
    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(
            self.0
                .send(bytes.to_vec())
                .map(|()| bytes.len())
                .map_err(|_| std::io::ErrorKind::BrokenPipe.into()),
        )
    }
}

impl Setup {
    /// A relay over `transport`: a loopback address, the production wait for a started
    /// server on a clock whose sleeps take no real time, and the production limits.
    pub fn new(transport: &Arc<FakeTransport>) -> Self {
        Self {
            transport: transport.clone(),
            launcher: FakeLauncher::default(),
            loopback: true,
            start: StartPolicy::default(),
            clock: FakeClock::default(),
            limits: Limits::default(),
            token: TOKEN,
        }
    }
}

/// The service accepted a message and sent no body.
pub fn accepted() -> Script {
    status(202, Body::Empty)
}

/// The service answered with one JSON message.
pub fn answered(message: &Value) -> Script {
    status(200, Body::Json(message.to_string()))
}

/// The message could not be exchanged.
pub fn broken(why: &str) -> Script {
    Script {
        events: Vec::new(),
        reply: Err(TransportError(why.to_owned())),
    }
}

/// The header `name` of a sent message.
pub fn header<'post>(post: &'post Post, name: &str) -> Option<&'post str> {
    post.headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Run the relay over `input` and collect what it wrote.
pub async fn relay(setup: Setup, input: &[u8]) -> Outcome<Ran> {
    let mut stdout = Vec::new();
    let (code, stderr) = relay_over(setup, input, &mut stdout).await?;
    Ok(Ran {
        code,
        stdout: String::from_utf8(stdout)?,
        stderr,
    })
}

/// Run the relay over the given input and output streams; returns its exit code and what it
/// wrote to the error stream.
pub async fn relay_over<I: AsyncRead + Unpin, O: AsyncWrite + Unpin>(
    setup: Setup,
    stdin: I,
    stdout: O,
) -> Outcome<(u8, String)> {
    let mut stderr = Vec::new();
    let code = relay_on(setup, stdin, stdout, &mut stderr).await?;
    Ok((code, String::from_utf8(stderr)?))
}

/// Run the relay over the three given streams; returns its exit code.
pub async fn relay_on<I: AsyncRead + Unpin, O: AsyncWrite + Unpin, E: AsyncWrite + Unpin>(
    setup: Setup,
    stdin: I,
    stdout: O,
    stderr: E,
) -> Outcome<u8> {
    let secret = some(Credential::parse(setup.token, "the test")?, "a credential")?;
    Ok(serve(
        Relay {
            transport: setup.transport,
            launcher: setup.launcher,
            loopback: setup.loopback,
            start: setup.start,
            clock: setup.clock,
            limits: setup.limits,
            secret,
        },
        Streams {
            stdin,
            stdout,
            stderr,
        },
    )
    .await)
}

/// The service answered with `status` and `body`.
pub fn status(status: u16, body: Body) -> Script {
    Script {
        events: Vec::new(),
        reply: Ok(Reply {
            status,
            session: None,
            body,
        }),
    }
}

/// The service answered with an event stream of `messages`.
pub fn stream(messages: &[Value]) -> Script {
    Script {
        events: messages.iter().map(Value::to_string).collect(),
        ..status(200, Body::Events)
    }
}
