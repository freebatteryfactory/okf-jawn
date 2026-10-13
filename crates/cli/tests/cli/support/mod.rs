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

/// A standard input that records whether anything tried to read it, and is otherwise empty.
pub struct Untouched(pub Arc<AtomicBool>);

/// The built command-line executable. Cargo names it after the package; the command it
/// implements is `okf-jawn`.
pub const EXECUTABLE: &str = env!("CARGO_BIN_EXE_okf-jawn-cli");

/// The token every test that needs one uses; no test expects to see it in any output.
pub const TOKEN: &str = "tok-5f1c2a9e";

static FILES: AtomicUsize = AtomicUsize::new(0);

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
