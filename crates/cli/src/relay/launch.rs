//! Start or connect: the relay connects to a running service, and starts one only when nothing
//! accepts connections at a loopback address.
//!
//! The relay never becomes a second writer. It launches the server executable at most once,
//! never for a remote address and never when anything answers at the address. When the
//! process it launched ends, as it does when another process already owns the data directory,
//! the relay keeps asking and connects to whichever service answers. It never stops a
//! service: the one it started is left running, because another client or the browser may be
//! using it.
//!
//! The wait is bounded by [`StartPolicy`]: `checks` liveness checks, `pause` apart, each of
//! which the transport bounds itself.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

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

/// The production launcher: the server executable as a detached child process.
///
/// The child's standard streams are closed to it, so nothing it prints, its single-use launch
/// URL included, reaches the MCP client. It is given `OKF_BIND`, the service's bind setting,
/// so that it listens where the relay will connect, and `OKF_JAWN_TOKEN` is removed from its
/// environment: the connector credential is the relay's own. Dropping the launcher does not
/// stop it.
#[derive(Debug)]
pub struct ProcessLauncher {
    program: Option<PathBuf>,
    bind: Option<String>,
    child: Option<Child>,
}

/// How long the relay waits for a service it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartPolicy {
    /// How many liveness checks follow the launch.
    pub checks: u32,
    /// The pause before each of them.
    pub pause: Duration,
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

/// The service's bind setting, as `deploy/.env.example` names it.
const BIND_VARIABLE: &str = "OKF_BIND";

/// The server executable's file name, without the platform's suffix.
const SERVER_PROGRAM: &str = "okf-jawn-server";

impl Default for StartPolicy {
    /// Sixty checks, half a second apart: about thirty seconds where a refused connection is
    /// reported at once, and about two minutes on Windows, which takes about two seconds to
    /// report one.
    fn default() -> Self {
        Self {
            checks: 60,
            pause: Duration::from_millis(500),
        }
    }
}

impl ProcessLauncher {
    /// The command that starts the server, or `None` when no executable is known.
    #[must_use]
    pub fn command(&self) -> Option<Command> {
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
        Some(command)
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
        let (Some(program), Some(mut command)) = (self.program.as_ref(), self.command()) else {
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

/// The server executable shipped beside the CLI executable `cli`: the same directory, and the
/// platform's executable suffix.
#[must_use]
pub fn server_beside(cli: &Path) -> PathBuf {
    cli.with_file_name(format!("{SERVER_PROGRAM}{}", std::env::consts::EXE_SUFFIX))
}

/// Connect to the running service, or start the local one and wait for it.
///
/// # Errors
/// Returns, in plain words, why no service is running and none could be started.
pub async fn start_or_connect<T: Transport, L: Launcher>(
    transport: &T,
    launcher: &mut L,
    loopback: bool,
    policy: StartPolicy,
) -> Result<Arrival, String> {
    match transport.probe().await {
        Probe::Up => return Ok(Arrival::Connected),
        Probe::Refused if loopback => {}
        Probe::Refused => {
            return Err(
                "No service accepts connections at the server address, and the relay never \
                 starts a remote service"
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
    for _ in 0..policy.checks {
        tokio::time::sleep(policy.pause).await;
        if transport.probe().await == Probe::Up {
            return Ok(Arrival::Started);
        }
    }
    Err(match launcher.state() {
        ChildState::Exited(code) => format!(
            "The server process the relay started ended{} and no service answered after {} \
             checks. If another process owns the data directory, it is not answering at the \
             server address",
            code.map(|code| format!(" with exit code {code}"))
                .unwrap_or_default(),
            policy.checks
        ),
        ChildState::Unknown(why) => format!(
            "The relay started the server, but no service answered after {} checks, and the \
             server process's state could not be read: {why}",
            policy.checks
        ),
        ChildState::Running | ChildState::NotStarted => format!(
            "The relay started the server, but no service answered after {} checks; the \
             server process is left running",
            policy.checks
        ),
    })
}
