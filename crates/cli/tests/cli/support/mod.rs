//! Shared test support: in-memory runs of the command line and the hand-answered listener.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use okf_jawn_cli::{Streams, run};

/// The result of a helper that can fail.
pub type Outcome<T> = Result<T, Box<dyn std::error::Error>>;

/// What one run of the command line wrote and returned.
#[derive(Debug)]
pub struct Ran {
    pub code: u8,
    pub stdout: String,
    pub stderr: String,
}

/// The built command-line executable. Cargo names it after the package; the command it
/// implements is `okf-jawn`.
pub const EXECUTABLE: &str = env!("CARGO_BIN_EXE_okf-jawn-cli");

/// The token every test that needs one uses; no test expects to see it in any output.
pub const TOKEN: &str = "tok-5f1c2a9e";

static FILES: AtomicUsize = AtomicUsize::new(0);

/// Run the command line in this process with `token` as the environment's token.
pub async fn cli(arguments: &[&str], token: Option<&str>, stdin: &[u8]) -> Outcome<Ran> {
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

pub mod http;
