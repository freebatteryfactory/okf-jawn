//! Shared test support: in-memory runs of the command line, the hand-answered listener and
//! the relay's doubles.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};

use okf_jawn_cli::{Streams, run};
use serde_json::Value;
use tokio::io::{AsyncRead, ReadBuf};

/// The result of a helper that can fail.
pub type Outcome<T> = Result<T, Box<dyn std::error::Error>>;

/// What one run of the command line wrote and returned.
#[derive(Debug)]
pub struct Ran {
    pub code: u8,
    pub stdout: String,
    pub stderr: String,
}

/// A stand-in for the server executable: a script the relay can start.
///
/// It says [`STAND_IN_SAYS`] on both its standard streams, records what it was given, and
/// then lives until the test stops it, or about thirty seconds, whichever comes first.
/// Dropping it stops it.
pub struct StandIn {
    /// The script, as a `--server-bin` value.
    pub program: PathBuf,
    record: PathBuf,
    stop: PathBuf,
}

/// A standard input that records whether anything tried to read it, and is otherwise empty.
pub struct Untouched(pub Arc<AtomicBool>);

/// The built command-line executable: the `okf-jawn` bin target of `crates/cli/Cargo.toml`.
pub const EXECUTABLE: &str = env!("CARGO_BIN_EXE_okf-jawn");

/// What a [`StandIn`] writes to its standard output and to its standard error.
pub const STAND_IN_SAYS: &str = "stand-in-output";

/// The token every test that needs one uses; no test expects to see it in any output.
pub const TOKEN: &str = "tok-5f1c2a9e-7d3b-41c6";

static FILES: AtomicUsize = AtomicUsize::new(0);

impl Drop for StandIn {
    fn drop(&mut self) {
        self.stop();
        // The record is the test's own; the script may still be running for a moment, and
        // the stop file must outlive it, so both stay in the target's temporary directory.
        let _ = std::fs::remove_file(&self.record);
    }
}

impl StandIn {
    /// Write the script for this platform.
    pub fn new() -> Outcome<Self> {
        let record = file("stand-in-record.txt", b"")?;
        std::fs::remove_file(&record)?;
        let stop = file("stand-in-stop", b"")?;
        std::fs::remove_file(&stop)?;
        let (suffix, script) = if cfg!(windows) {
            (
                "stand-in.cmd",
                format!(
                    "@echo off\r\necho {STAND_IN_SAYS}\r\necho {STAND_IN_SAYS} 1>&2\r\n\
                     > \"{record}\" echo token=%OKF_JAWN_TOKEN%\r\n\
                     >> \"{record}\" echo bind=%OKF_BIND%\r\n\
                     >> \"{record}\" echo ready\r\n\
                     set /a n=0\r\n:wait\r\nif exist \"{stop}\" exit /b 0\r\nset /a n+=1\r\n\
                     if %n% geq 30 exit /b 0\r\nping -n 2 127.0.0.1 > nul\r\ngoto wait\r\n",
                    record = record.display(),
                    stop = stop.display()
                ),
            )
        } else {
            (
                "stand-in.sh",
                format!(
                    "#!/bin/sh\necho {STAND_IN_SAYS}\necho {STAND_IN_SAYS} >&2\n\
                     {{ echo \"token=$OKF_JAWN_TOKEN\"; echo \"bind=$OKF_BIND\"; echo \"pid=$$\"; \
                     echo \"group=$(ps -o pgid= -p $$ | tr -d ' ')\"; echo ready; }} > \"{record}\"\n\
                     n=0\nwhile [ ! -e \"{stop}\" ] && [ \"$n\" -lt 30 ]; do sleep 1; n=$((n+1)); done\n",
                    record = record.display(),
                    stop = stop.display()
                ),
            )
        };
        let program = file(suffix, script.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(Self {
            program,
            record,
            stop,
        })
    }

    /// What the script recorded once it had run, one `name=value` per line; waits up to
    /// `patience` for it.
    pub fn recorded(&self, patience: std::time::Duration) -> Outcome<String> {
        let started = std::time::Instant::now();
        loop {
            let text = std::fs::read_to_string(&self.record).unwrap_or_default();
            if text.lines().any(|line| line.trim() == "ready") {
                return Ok(text);
            }
            if started.elapsed() > patience {
                return Err(
                    format!("the stand-in did not run within {patience:?}: {text:?}").into(),
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// Ask the script to end; it does within about a second.
    pub fn stop(&self) {
        let _ = std::fs::write(&self.stop, b"stop");
    }

    /// The value the script recorded under `name`.
    pub fn value(recorded: &str, name: &str) -> Option<String> {
        recorded.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key == name).then(|| value.trim().to_owned())
        })
    }
}

impl Ran {
    /// The output as JSON values, one per line; fails when a line is not JSON or the last
    /// line has no line feed.
    pub fn lines(&self) -> Outcome<Vec<Value>> {
        if !self.stdout.is_empty() && !self.stdout.ends_with('\n') {
            return Err(format!("the output's last line is not whole: {}", self.stdout).into());
        }
        self.stdout
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line)
                    .map_err(|error| format!("an output line is not JSON ({error}): {line}").into())
            })
            .collect()
    }
}

impl AsyncRead for Untouched {
    fn poll_read(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.0.store(true, Ordering::SeqCst);
        Poll::Ready(Ok(()))
    }
}

/// Run the command line in this process with `token` as the environment's token.
pub async fn cli(arguments: &[&str], token: Option<&str>, stdin: &[u8]) -> Outcome<Ran> {
    cli_reading(arguments, token, stdin).await
}

/// Run the command line in this process, reading standard input from `stdin`.
pub async fn cli_reading<I: AsyncRead + Unpin>(
    arguments: &[&str],
    token: Option<&str>,
    stdin: I,
) -> Outcome<Ran> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let environment = |name: &str| {
        (name == "OKF_JAWN_TOKEN")
            .then(|| token.map(str::to_owned))
            .flatten()
    };
    let mut line = vec!["okf-jawn"];
    line.extend_from_slice(arguments);
    let code = run(
        line,
        &environment,
        Streams {
            stdin,
            stdout: &mut stdout,
            stderr: &mut stderr,
        },
    )
    .await;
    Ok(Ran {
        code,
        stdout: String::from_utf8(stdout)?,
        stderr: String::from_utf8(stderr)?,
    })
}

/// Write `content` to a fresh file under the test target's temporary directory.
pub fn file(name: &str, content: &[u8]) -> Outcome<PathBuf> {
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("okf-jawn-cli");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!(
        "{}-{}-{name}",
        std::process::id(),
        FILES.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::write(&path, content)?;
    Ok(path)
}

pub mod doubles;
pub mod http;
