//! The live launch link of a local installation: the first link is returned to be printed and
//! written to `<data dir>/launch-url`; after each sign-in and each expiry the next is written;
//! the file is owner-only on Unix and removed at shutdown; a file that cannot be written leaves
//! the printed link working.
#![cfg(feature = "runtime")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_server::grants::SystemClock;
use okf_jawn_server::launch::{LAUNCH_FILE, LaunchLink};
use okf_jawn_server::local::{LaunchTokens, SecretSource};
use tokio_util::sync::CancellationToken;

use check::{TestResult, some};

/// Secrets handed out in order.
struct Sequence(Mutex<Vec<String>>);

/// A temporary data directory, removed when dropped (best effort).
struct Scratch(PathBuf);

const ORIGIN: &str = "http://127.0.0.1:7711";

impl SecretSource for Sequence {
    fn secret(&self) -> Result<String, ApiError> {
        self.0
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))?
            .pop()
            .ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "no secret left"))
    }
}

impl Scratch {
    fn new(name: &str) -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "okf-jawn-launch-{name}-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a leftover directory under the temporary directory harms nothing.
        let _removed = std::fs::remove_dir_all(&self.0);
    }
}

/// A distinct suffix per call within this process.
fn uuid_like() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

fn secrets(count: usize) -> Arc<Sequence> {
    let mut all: Vec<String> = (0..count).map(|n| format!("{n:064x}")).collect();
    all.reverse();
    Arc::new(Sequence(Mutex::new(all)))
}

fn token_of(url: &str) -> Result<String, String> {
    url.strip_prefix(&format!("{ORIGIN}/auth/local?token="))
        .map(str::to_owned)
        .ok_or_else(|| format!("not a launch link of {ORIGIN}: {url}"))
}

/// The link the file holds, without its line end.
fn published(file: &Path) -> std::io::Result<String> {
    Ok(std::fs::read_to_string(file)?
        .trim_end_matches('\n')
        .to_owned())
}

/// Wait until the file holds another link than `old`, at most ten seconds.
async fn next_after(file: &Path, old: &str) -> Result<String, Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(link) = published(file)
            && link != old
            && !link.is_empty()
        {
            return Ok(link);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("the launch file still holds {old}").into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn the_first_link_is_returned_and_written_where_a_launcher_reads_it() -> TestResult {
    let scratch = Scratch::new("first")?;
    let tokens = Arc::new(LaunchTokens::new(
        Duration::from_secs(600),
        Arc::new(SystemClock),
    ));
    let link = LaunchLink::new(tokens.clone(), secrets(4), ORIGIN, &scratch.0);
    let url = link.publish_first()?;
    let file = scratch.0.join(LAUNCH_FILE);
    assert_eq!(LAUNCH_FILE, "launch-url");
    assert_eq!(std::fs::read_to_string(&file)?, format!("{url}\n"));
    let token = token_of(&url)?;
    assert_eq!(token.len(), 64);
    assert!(tokens.redeem(&token)?.is_some());
    Ok(())
}

#[tokio::test]
async fn a_sign_in_publishes_the_next_link() -> TestResult {
    let scratch = Scratch::new("sign-in")?;
    let tokens = Arc::new(LaunchTokens::new(
        Duration::from_secs(600),
        Arc::new(SystemClock),
    ));
    let link = LaunchLink::new(tokens.clone(), secrets(4), ORIGIN, &scratch.0);
    let first = link.publish_first()?;
    let stop = CancellationToken::new();
    let keeping = tokio::spawn(link.keep(stop.clone()));
    let file = scratch.0.join(LAUNCH_FILE);
    // The exchange took the token and completed.
    let _taken = some(tokens.redeem(&token_of(&first)?)?, "the first token")?;
    tokens.sign_in_completed();
    let second = next_after(&file, &first).await?;
    assert!(tokens.redeem(&token_of(&first)?)?.is_none());
    assert!(tokens.redeem(&token_of(&second)?)?.is_some());
    stop.cancel();
    keeping.await?;
    Ok(())
}

#[tokio::test]
async fn an_unused_link_is_replaced_when_its_lifetime_ends() -> TestResult {
    let scratch = Scratch::new("expiry")?;
    // Long enough that the link read below is still live when it is redeemed, on a loaded
    // machine too; the first is past it by then.
    let tokens = Arc::new(LaunchTokens::new(
        Duration::from_secs(2),
        Arc::new(SystemClock),
    ));
    let link = LaunchLink::new(tokens.clone(), secrets(8), ORIGIN, &scratch.0);
    let first = link.publish_first()?;
    let stop = CancellationToken::new();
    let keeping = tokio::spawn(link.keep(stop.clone()));
    let file = scratch.0.join(LAUNCH_FILE);
    let second = next_after(&file, &first).await?;
    // The printed one has expired; the published one works.
    assert!(tokens.redeem(&token_of(&first)?)?.is_none());
    assert!(tokens.redeem(&token_of(&second)?)?.is_some());
    stop.cancel();
    keeping.await?;
    Ok(())
}

#[tokio::test]
async fn shutdown_removes_the_launch_file() -> TestResult {
    let scratch = Scratch::new("shutdown")?;
    let tokens = Arc::new(LaunchTokens::new(
        Duration::from_secs(600),
        Arc::new(SystemClock),
    ));
    let link = LaunchLink::new(tokens, secrets(4), ORIGIN, &scratch.0);
    link.publish_first()?;
    let file = scratch.0.join(LAUNCH_FILE);
    assert!(file.is_file());
    let stop = CancellationToken::new();
    let keeping = tokio::spawn(link.keep(stop.clone()));
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(10), keeping).await??;
    assert!(!file.exists());
    Ok(())
}

#[tokio::test]
async fn a_file_that_cannot_be_written_leaves_the_printed_link_working() -> TestResult {
    let scratch = Scratch::new("unwritable")?;
    // The data directory is a file: nothing can be written under it.
    let blocked = scratch.0.join("not-a-directory");
    std::fs::write(&blocked, "")?;
    let tokens = Arc::new(LaunchTokens::new(
        Duration::from_secs(600),
        Arc::new(SystemClock),
    ));
    let link = LaunchLink::new(tokens.clone(), secrets(4), ORIGIN, &blocked);
    let url = link.publish_first()?;
    assert!(tokens.redeem(&token_of(&url)?)?.is_some());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn the_launch_file_is_readable_by_its_owner_only() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    let scratch = Scratch::new("mode")?;
    let tokens = Arc::new(LaunchTokens::new(
        Duration::from_secs(600),
        Arc::new(SystemClock),
    ));
    let link = LaunchLink::new(tokens, secrets(4), ORIGIN, &scratch.0);
    link.publish_first()?;
    let mode = std::fs::metadata(scratch.0.join(LAUNCH_FILE))?
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
