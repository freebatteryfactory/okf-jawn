//! Start or connect: the relay connects to a running service, and starts one only when nothing
//! accepts connections at a loopback address.
//!
//! The relay never becomes a second writer. It launches the server executable at most once,
//! only for a plain-HTTP loopback address (never for a remote one, never for HTTPS) and
//! never when anything answers at the address. When the
//! process it launched ends, as it does when another process already owns the data directory,
//! the relay keeps asking and connects to whichever service answers. It never stops a
//! service: the one it started is left running, because another client or the browser may be
//! using it.
//!
//! The wait is bounded by [`StartPolicy`]: its whole time is counted on a [`Clock`] from the
//! launch, whatever one liveness check costs, and the relay says through [`Progress`] that it
//! is waiting.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::transport::{Probe, Transport};

/// How the relay reached the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrival {
    /// A service was already running.
    Connected,
    /// The relay launched the server, and a service then answered.
    Started,
}

/// What became of the process a [`Launcher`] started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildState {
    /// Nothing was launched.
    NotStarted,
    /// The process is still running.
    Running,
    /// The process ended, with this exit code when it had one.
    Exited(Option<i32>),
    /// The process's state could not be read, for this reason.
    Unknown(String),
}

/// How the started server is cut loose from the terminal the relay runs in: what this
/// platform's `Command` was told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detachment {
    /// Windows process creation flags.
    CreationFlags(u32),
    /// Unix: the process group the child joins; 0 is a new group of its own.
    ProcessGroup(i32),
    /// A platform on which the relay knows no way to do it.
    Unsupported,
}

/// The production launcher: the server executable as a child process the relay does not stop.
///
/// The child's three standard streams are closed to it, so nothing it prints, its single-use
/// launch URL included, reaches the MCP client. It is given `OKF_BIND`, the service's bind
/// setting, so that it listens where the relay will connect, and `OKF_JAWN_TOKEN` is removed
/// from its environment: the connector credential is the relay's own. It is cut loose from
/// the relay's terminal ([`Detachment`]), so an interrupt typed there does not reach it.
/// Dropping the launcher does not stop it.
///
/// Known limit, Windows only: the child is not cut loose from the relay's own streams. Windows
/// hands a child every inheritable handle of its parent, and the pipes the MCP client gave
/// the relay as standard input, output and error are inheritable. So the started server
/// holds an end of each beside its own three closed streams, and a client that waits for the
/// end of the relay's output does not see it while that server runs. Nothing the server
/// prints goes there. Keeping the handles back takes `SetHandleInformation`, which is not
/// safe Rust, or `CommandExt::inherit_handles`, which is not stable; on Unix the child holds
/// none of them.
#[derive(Debug)]
pub struct ProcessLauncher {
    program: Option<PathBuf>,
    bind: Option<String>,
    child: Option<Child>,
}

/// How long the relay waits for a service it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartPolicy {
    /// The whole wait, counted on the clock from the launch.
    pub wait: Duration,
    /// The pause before each liveness check.
    pub pause: Duration,
    /// How often, at least, the relay says that it is still waiting.
    pub report_every: Duration,
}

/// The process's own clock.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    started: Instant,
}

/// The wait for a started server: its clock, where it reports, and its times.
struct Watch<'wait, C, P> {
    clock: &'wait C,
    progress: &'wait mut P,
    /// When the server was launched.
    started: Duration,
    /// When the wait is over.
    deadline: Duration,
    /// When the relay is next due to say that it still waits.
    report_at: Duration,
    policy: StartPolicy,
}

/// The clock the wait for a started server runs on. Tests pass one that they move themselves.
pub trait Clock {
    /// The time since a fixed moment.
    fn now(&self) -> Duration;

    /// Complete once `duration` has passed.
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()>;
}

/// Starts the service process and reports whether it is still running.
pub trait Launcher {
    /// Start the server process.
    ///
    /// # Errors
    /// Returns why the process could not be started, in plain words.
    fn launch(&mut self) -> Result<(), String>;

    /// What became of the process this launcher started.
    fn state(&mut self) -> ChildState;
}

/// Where the relay says what it is doing while it waits for a server it started.
pub trait Progress {
    /// Say one line.
    fn say(&mut self, text: &str) -> impl Future<Output = ()>;
}
/// The service's bind setting, as `deploy/.env.example` names it.
const BIND_VARIABLE: &str = "OKF_BIND";

/// Windows: the new process leads a process group of its own, and Ctrl-C is disabled for it.
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Windows: the new process gets a console of its own that has no window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Unix: the process group argument that makes the child the leader of a new group.
#[cfg(unix)]
const OWN_GROUP: i32 = 0;

/// The server executable's file name, without the platform's suffix.
const SERVER_PROGRAM: &str = "okf-jawn-server";

impl Default for StartPolicy {
    /// Forty-five seconds in all, under the sixty that MCP hosts commonly give `initialize`;
    /// a check every half second; and a word at least every ten seconds.
    fn default() -> Self {
        Self {
            wait: Duration::from_secs(45),
            pause: Duration::from_millis(500),
            report_every: Duration::from_secs(10),
        }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> {
        tokio::time::sleep(duration)
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl<C: Clock, P: Progress> Watch<'_, C, P> {
    /// Wait for `work`, saying on the way that the relay still waits.
    ///
    /// Returns `None` when the wait's time is up first: `work` is then dropped, however far
    /// it had come, and nothing is started once the time is up. Work that ends exactly when
    /// the time is up still counts.
    async fn until<F: Future>(&mut self, work: F) -> Option<F::Output> {
        let mut work = std::pin::pin!(work);
        loop {
            let now = self.clock.now();
            if now >= self.deadline {
                return None;
            }
            if now >= self.report_at {
                self.report_at = self.report_at.saturating_add(self.policy.report_every);
                self.progress
                    .say(&format!(
                        "still waiting for the okf-jawn server to answer ({} of {} seconds)",
                        now.saturating_sub(self.started).as_secs(),
                        self.policy.wait.as_secs()
                    ))
                    .await;
            }
            let next = self.report_at.min(self.deadline);
            tokio::select! {
                biased;
                output = &mut work => return Some(output),
                () = self.clock.sleep(next.saturating_sub(now)) => {}
            }
        }
    }
}
impl ProcessLauncher {
    /// The command that starts the server and how it is cut loose from the relay's terminal,
    /// or `None` when no executable is known.
    #[must_use]
    pub fn command(&self) -> Option<(Command, Detachment)> {
        let mut command = Command::new(self.program.as_ref()?);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // The connector credential is the relay's own; the server has no use for it.
        command.env_remove(crate::TOKEN_VARIABLE);
        if let Some(bind) = &self.bind {
            command.env(BIND_VARIABLE, bind);
        }
        let detachment = detach(&mut command);
        Some((command, detachment))
    }

    /// A launcher for the executable at `program`, binding `bind` when one is given.
    #[must_use]
    pub fn new(program: Option<PathBuf>, bind: Option<String>) -> Self {
        Self {
            program,
            bind,
            child: None,
        }
    }
}

impl Launcher for ProcessLauncher {
    fn launch(&mut self) -> Result<(), String> {
        let (Some(program), Some((mut command, _))) = (self.program.as_ref(), self.command())
        else {
            return Err("the server executable's location is not known".to_owned());
        };
        let child = command
            .spawn()
            .map_err(|error| format!("{} could not be started: {error}", program.display()))?;
        self.child = Some(child);
        Ok(())
    }

    fn state(&mut self) -> ChildState {
        match self.child.as_mut().map(Child::try_wait) {
            None => ChildState::NotStarted,
            Some(Ok(None)) => ChildState::Running,
            Some(Ok(Some(status))) => ChildState::Exited(status.code()),
            Some(Err(error)) => ChildState::Unknown(error.to_string()),
        }
    }
}

/// Cut the process `command` starts loose from the relay's terminal, and say what was asked.
///
/// On Windows it gets a console of its own without a window, and its own process group: a
/// Ctrl-C or a Ctrl-Break in the client's console is not delivered to it, and no console
/// window appears when the client has none.
#[cfg(windows)]
fn detach(command: &mut Command) -> Detachment {
    use std::os::windows::process::CommandExt as _;
    command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    Detachment::CreationFlags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
}

/// Cut the process `command` starts loose from the relay's terminal, and say what was asked.
///
/// On Unix it leads a process group of its own, so the interrupt a terminal sends to the
/// client's foreground group does not reach it.
#[cfg(unix)]
fn detach(command: &mut Command) -> Detachment {
    use std::os::unix::process::CommandExt as _;
    command.process_group(OWN_GROUP);
    Detachment::ProcessGroup(OWN_GROUP)
}

/// No way is known on this platform to cut the process loose; it is started as it is.
#[cfg(not(any(windows, unix)))]
fn detach(_command: &mut Command) -> Detachment {
    Detachment::Unsupported
}

/// The server executable shipped beside the CLI executable `cli`: the same directory, and the
/// platform's executable suffix.
///
/// `None` when `cli` is not an absolute path: a bare or relative name would leave it to the
/// working directory or to `PATH` which program is started, so there is then no default.
#[must_use]
pub fn server_beside(cli: &Path) -> Option<PathBuf> {
    cli.is_absolute()
        .then(|| cli.with_file_name(format!("{SERVER_PROGRAM}{}", std::env::consts::EXE_SUFFIX)))
}

/// Connect to the running service, or start the local one and wait for it.
///
/// The wait for a started server is `policy.wait` on `clock`, counted from the launch,
/// whatever one liveness check costs: a check still running when the time is up is given up.
/// (The one check before the launch is bounded by the transport alone.) While it waits the
/// relay says so through `progress`: once at the launch, and then every
/// `policy.report_every` at the latest.
///
/// # Errors
/// Returns, in plain words, why no service is running and none could be started.
pub async fn start_or_connect<T: Transport, L: Launcher, C: Clock, P: Progress>(
    transport: &T,
    launcher: &mut L,
    loopback: bool,
    policy: StartPolicy,
    clock: &C,
    progress: &mut P,
) -> Result<Arrival, String> {
    match transport.probe().await {
        Probe::Up => return Ok(Arrival::Connected),
        Probe::Refused if loopback => {}
        Probe::Refused => {
            return Err(
                "No service accepts connections at the server address, and the relay never \
                 starts a remote service or one behind HTTPS: it starts a server only for a \
                 plain-HTTP loopback address"
                    .to_owned(),
            );
        }
        Probe::Unhealthy(status) => {
            return Err(format!(
                "The server address answered its liveness check with HTTP {status}; no second \
                 service is started beside whatever is listening there"
            ));
        }
        Probe::Failed(why) => {
            return Err(format!(
                "The liveness check of the server address failed: {why}"
            ));
        }
    }
    launcher.launch().map_err(|why| {
        format!(
            "No service is running, and the server could not be started: {why}. Name the \
             okf-jawn-server executable with --server-bin or OKF_JAWN_SERVER_BIN"
        )
    })?;
    let seconds = policy.wait.as_secs();
    let started = clock.now();
    progress
        .say(&format!(
            "no service answers at the server address; started the okf-jawn server and waiting \
             up to {seconds} seconds for it to answer"
        ))
        .await;
    let mut watch = Watch {
        clock,
        progress,
        started,
        deadline: started.saturating_add(policy.wait),
        report_at: started.saturating_add(policy.report_every),
        policy,
    };
    while watch.until(clock.sleep(policy.pause)).await.is_some() {
        match watch.until(transport.probe()).await {
            Some(Probe::Up) => return Ok(Arrival::Started),
            Some(_) => {}
            None => break,
        }
    }
    Err(match launcher.state() {
        ChildState::Exited(code) => format!(
            "The server process the relay started ended{} and no service answered within \
             {seconds} seconds. If another process owns the data directory, it is not answering \
             at the server address",
            code.map(|code| format!(" with exit code {code}"))
                .unwrap_or_default()
        ),
        ChildState::Unknown(why) => format!(
            "The relay started the server, but no service answered within {seconds} seconds, \
             and the server process's state could not be read: {why}"
        ),
        ChildState::Running | ChildState::NotStarted => format!(
            "The relay started the server, but no service answered within {seconds} seconds; \
             the server process is left running"
        ),
    })
}
