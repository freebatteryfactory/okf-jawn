//! The memory cap of the conversion child, and how a child's end is read.
//!
//! Conversion runs in a child process with a hard memory cap (`converter-worker-memory-ceiling`,
//! O3). Each platform enforces it with the strongest facility it has, behind `MemoryCap`:
//!
//! - Windows: a job object with a committed-memory limit, assigned before the child runs.
//! - Linux: the child sets its own `RLIMIT_DATA` as the first thing it does.
//! - macOS: a supervisor watchdog samples the child's memory every `WATCHDOG_SAMPLE_PERIOD_MS`
//!   and kills it (Tokio's `start_kill`, SIGKILL) at `watchdog_threshold`, the cap minus a
//!   headroom. The metric is read in one private function, `footprint`: libproc's
//!   `pidrusage::<RUsageInfoV2>` and its `ri_phys_footprint`, the physical footprint macOS
//!   itself uses for memory pressure (jetsam), which unlike the resident set still counts
//!   compressed pages. The owner has not yet confirmed that the watchdog counts as the hard cap
//!   there. That function compiles only on macOS; no macOS target or runner builds it here.
//!
//! `spawn_capped` starts a child under the cap and `supervise` waits for it under the time
//! bound, keeping the tail of its stderr. The child learns its cap from `MEMORY_LIMIT_ENV`,
//! and calls `limit_self` first thing; only Linux needs that call to do anything.
//!
//! Only the watchdog kills. Under the other two facilities an allocation fails inside the
//! child, which then aborts or reports an error. So whether the cap fired is classified, not
//! signalled (integration-owner ruling on concern C2), and only from evidence, never from free
//! text such as "out of memory" anywhere in stderr (fix-2 ruling 4). The classification:
//!
//! 1. A child that wrote its result reported for itself, whatever its exit.
//! 2. A child the supervisor killed at the time bound ran out of time.
//! 3. On Windows, a child whose job's peak committed memory reached the cap hit it. "Reached"
//!    means within `cap_margin` of the cap, because the allocation that failed was never
//!    committed; the margin scales with the cap, so a small cap is not reached by any peak.
//! 4. A child whose last stderr line is the explicit marker it writes when an allocation fails
//!    (`ALLOCATION_FAILURE_MARKER`; Rust has no stable allocation-error hook, so the child
//!    writes it where an allocation failure reaches it as a typed error), or exactly the
//!    message Rust's runtime prints before it aborts on one (`memory allocation of N bytes
//!    failed`), hit the cap.
//! 5. A child the macOS watchdog killed hit the cap.
//! 6. Any other end without a result is a crash.

use std::process::Stdio;
use std::time::Duration;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::FailureReason;
use tokio::io::AsyncReadExt as _;
use tokio::process::{Child, ChildStderr, Command};

/// How a child process ended, as the supervisor observed it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChildEnd {
    /// The child wrote its result file before it ended.
    pub reported: bool,
    /// The watchdog killed the child at the cap.
    pub killed_at_cap: bool,
    /// The supervisor killed the child at the hard time bound.
    pub killed_at_time_limit: bool,
    /// The last bytes the child wrote to stderr, lossily decoded.
    pub stderr_tail: String,
    /// Peak committed memory of the child's job object, where the platform keeps one.
    pub peak_committed_bytes: Option<u64>,
}

/// How the wait for a child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// The child exited by itself.
    Exited,
    /// The supervisor killed it at the time bound.
    TimeLimit,
    /// The watchdog killed it at the cap.
    Cap,
}

/// What a child's end means for its window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildVerdict {
    /// The child reported its own result; read it.
    Reported,
    /// No result: the window failed for this reason.
    Failed(FailureReason),
}

/// A child started under the memory cap. Dropping it kills the child (and, on Windows, closes
/// its job object, which kills it too).
#[derive(Debug)]
pub struct CappedChild {
    child: Child,
    /// The job object that holds the child and enforces its committed-memory limit.
    #[cfg(windows)]
    group: processkit::ProcessGroup,
}

/// The environment variable that tells a child its cap, in bytes.
pub const MEMORY_LIMIT_ENV: &str = "OKF_JAWN_MEMORY_LIMIT_BYTES";

/// How many of the last bytes a child wrote to stderr are kept for the classification.
pub const STDERR_TAIL_BYTES: usize = 8 * 1024;

/// The share of the cap, as a divisor, within which a peak counts as having reached it.
pub const CAP_MARGIN_DIVISOR: u64 = 16;

/// The margin a large cap is given at least: 64 MiB.
pub const CAP_MARGIN_MIN_BYTES: u64 = 64 * 1024 * 1024;

/// The floor never exceeds this share of the cap, as a divisor: an eighth, so a small cap
/// (a test's 16 MiB) is not reached by every peak.
pub const CAP_MARGIN_FLOOR_DIVISOR: u64 = 8;

/// The last line the conversion child writes to stderr when an allocation fails and reaches it
/// as a typed error (`std::io::ErrorKind::OutOfMemory`: `std::fs::read` reserves its buffer
/// fallibly, so under `RLIMIT_DATA` or a job memory limit it returns that error instead of
/// aborting).
pub const ALLOCATION_FAILURE_MARKER: &str =
    "okf-jawn-convert: an allocation failed at the memory cap";

/// What Rust's runtime prints, as a whole line, before it aborts on a failed allocation:
/// `memory allocation of N bytes failed` (std's default allocation-error handler).
const RUST_ALLOCATION_FAILURE: (&str, &str) = ("memory allocation of ", " bytes failed");

/// How often the macOS watchdog samples the child, within the 50 to 100 ms the plan allows.
pub const WATCHDOG_SAMPLE_PERIOD_MS: u64 = 75;

/// The fastest the converter child was seen to grow, in bytes per second.
///
/// Placeholder until part (b) of `converter-worker-memory-ceiling` measures it: 512 MiB/s is an
/// assumption, not a measurement. The watchdog kills at the cap minus a headroom large enough
/// for two samples of growth at this rate, so the child cannot pass the cap between samples.
pub const WORST_ALLOCATION_RATE_BYTES_PER_SECOND: u64 = 512 * 1024 * 1024;

/// The least headroom, as a share of the cap: one twentieth, 5%.
pub const WATCHDOG_MIN_HEADROOM_DIVISOR: u64 = 20;

/// The footprint at which the macOS watchdog kills the child with SIGKILL: the cap minus the
/// headroom. The headroom is the greater of two sample periods of growth at the worst observed
/// rate and 5% of the cap.
#[must_use]
pub fn watchdog_threshold(
    limit_bytes: u64,
    sample_period_ms: u64,
    worst_rate_per_second: u64,
) -> u64 {
    let growth = worst_rate_per_second
        .saturating_mul(sample_period_ms.saturating_mul(2))
        .checked_div(1000)
        .unwrap_or(0);
    let share = limit_bytes
        .checked_div(WATCHDOG_MIN_HEADROOM_DIVISOR)
        .unwrap_or(0);
    limit_bytes.saturating_sub(growth.max(share))
}

/// How close to the cap a job's peak commit counts as having reached it: a sixteenth of the
/// cap, or `CAP_MARGIN_MIN_BYTES` when that is more, but never more than an eighth of the cap.
#[must_use]
pub fn cap_margin(limit_bytes: u64) -> u64 {
    let share = limit_bytes.checked_div(CAP_MARGIN_DIVISOR).unwrap_or(0);
    let floor = CAP_MARGIN_MIN_BYTES.min(
        limit_bytes
            .checked_div(CAP_MARGIN_FLOOR_DIVISOR)
            .unwrap_or(0),
    );
    share.max(floor)
}

/// Read a child's end against its cap and time bound, from evidence only.
#[must_use]
pub fn classify(end: &ChildEnd, limit_bytes: u64, limit_seconds: u32) -> ChildVerdict {
    if end.reported {
        return ChildVerdict::Reported;
    }
    if end.killed_at_time_limit {
        return ChildVerdict::Failed(FailureReason::TimeLimit { limit_seconds });
    }
    let margin = cap_margin(limit_bytes);
    let peak_at_cap = end
        .peak_committed_bytes
        .is_some_and(|peak| peak.saturating_add(margin) >= limit_bytes);
    let last = end
        .stderr_tail
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty());
    let allocation_failed = last
        .is_some_and(|line| line == ALLOCATION_FAILURE_MARKER || is_rust_allocation_failure(line));
    if peak_at_cap || allocation_failed || end.killed_at_cap {
        return ChildVerdict::Failed(FailureReason::MemoryLimit {
            limit_bytes: limit_bytes.to_string(),
        });
    }
    ChildVerdict::Failed(FailureReason::ConverterCrashed)
}

/// Start `command` as a child under a memory cap of `limit_bytes`.
///
/// Its stdin and stdout are closed, its stderr is piped for `supervise`, and it is killed when
/// the returned value is dropped. On Windows it is created suspended, assigned to a job object
/// with a committed-memory limit and only then resumed (processkit), so no instruction of the
/// child runs outside the cap. On Linux the child sets its own `RLIMIT_DATA` (`limit_self`). On
/// macOS `supervise` runs the watchdog.
///
/// # Errors
/// Returns `Internal` when the job object cannot be created with the limit or the child cannot
/// be started: a worker fault, never an uncapped child.
pub fn spawn_capped(mut command: Command, limit_bytes: u64) -> Result<CappedChild, ApiError> {
    let _configured = command
        .env(MEMORY_LIMIT_ENV, limit_bytes.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    spawn_platform(command, limit_bytes)
}

/// Wait for a capped child to end, at most `time_limit`, and say how it ended.
///
/// A child still running at `time_limit` is killed and marked `killed_at_time_limit`. On macOS
/// a child whose memory reaches `watchdog_threshold` is killed and marked `killed_at_cap`. The
/// returned `reported` is `false`: whether the child wrote its result is the caller's to read.
/// Dropping the future drops the child, which kills it.
///
/// # Errors
/// Returns `Internal` when the child's end cannot be waited for.
pub async fn supervise(
    capped: CappedChild,
    limit_bytes: u64,
    time_limit: Duration,
) -> Result<ChildEnd, ApiError> {
    let CappedChild {
        mut child,
        #[cfg(windows)]
        group,
    } = capped;
    let tail = tokio::spawn(stderr_tail(child.stderr.take()));
    let pid = child.id();
    let ended = tokio::select! {
        status = child.wait() => {
            let _status = status.map_err(|error| {
                fault(&format!("the conversion child could not be waited for: {error}"))
            })?;
            Ended::Exited
        }
        () = tokio::time::sleep(time_limit) => Ended::TimeLimit,
        () = watchdog(pid, limit_bytes) => Ended::Cap,
    };
    if ended != Ended::Exited {
        // The kill fails only when the child has already exited; the wait reaps it either way.
        let _killed = child.start_kill();
        let _status = child.wait().await.map_err(|error| {
            fault(&format!(
                "the killed conversion child could not be waited for: {error}"
            ))
        })?;
    }
    let stderr_tail = tail.await.unwrap_or_default();
    #[cfg(windows)]
    let peak_committed_bytes = group.stats().ok().and_then(|stats| stats.peak_memory_bytes);
    #[cfg(not(windows))]
    let peak_committed_bytes = None;
    Ok(ChildEnd {
        reported: false,
        killed_at_cap: ended == Ended::Cap,
        killed_at_time_limit: ended == Ended::TimeLimit,
        stderr_tail,
        peak_committed_bytes,
    })
}

/// In the child: put the cap `MEMORY_LIMIT_ENV` names on this process, first thing in `main`.
///
/// On Linux it sets `RLIMIT_DATA` (soft and hard) to the cap, or to the present hard limit when
/// that is lower: a process may always lower its own limit, so no `pre_exec` and no `unsafe`
/// is needed. Elsewhere the supervisor enforces the cap and this only reads it. Returns the cap.
///
/// # Errors
/// Returns `Internal` when the variable is missing or not a byte count, or the limit cannot be
/// set: the child must not convert uncapped.
pub fn limit_self() -> Result<u64, ApiError> {
    let limit = std::env::var(MEMORY_LIMIT_ENV)
        .ok()
        .and_then(|text| text.parse::<u64>().ok())
        .filter(|limit| *limit > 0)
        .ok_or_else(|| {
            fault(&format!(
                "the conversion child needs its cap in {MEMORY_LIMIT_ENV}"
            ))
        })?;
    // Windows and macOS: the supervisor enforces the cap, so there is nothing to set here.
    #[cfg(target_os = "linux")]
    limit_platform(limit)?;
    Ok(limit)
}

/// Windows: spawn suspended into a job object with a committed-memory limit, then resume.
#[cfg(windows)]
fn spawn_platform(command: Command, limit_bytes: u64) -> Result<CappedChild, ApiError> {
    let group = processkit::ProcessGroup::with_options(
        processkit::ProcessGroupOptions::default().max_memory(limit_bytes),
    )
    .map_err(|error| {
        fault(&format!(
            "the conversion child's job object could not hold the memory cap: {error}"
        ))
    })?;
    let child = group.spawn(command).map_err(|error| {
        fault(&format!(
            "the conversion child could not be started: {error}"
        ))
    })?;
    Ok(CappedChild { child, group })
}

/// Linux and macOS: an ordinary spawn; the child (Linux) or the watchdog (macOS) holds the cap.
#[cfg(not(windows))]
fn spawn_platform(mut command: Command, _limit_bytes: u64) -> Result<CappedChild, ApiError> {
    let child = command.spawn().map_err(|error| {
        fault(&format!(
            "the conversion child could not be started: {error}"
        ))
    })?;
    Ok(CappedChild { child })
}

/// Linux: `RLIMIT_DATA` counts private writable mappings (heap and anonymous `mmap`) since
/// Linux 4.7, and not the address space ONNX Runtime only reserves.
#[cfg(target_os = "linux")]
fn limit_platform(limit: u64) -> Result<(), ApiError> {
    let (_soft, hard) = rlimit::Resource::DATA
        .get()
        .map_err(|error| fault(&format!("RLIMIT_DATA could not be read: {error}")))?;
    let cap = limit.min(hard);
    rlimit::Resource::DATA
        .set(cap, cap)
        .map_err(|error| fault(&format!("RLIMIT_DATA could not be set: {error}")))
}

/// macOS: return once the child's memory reaches the kill threshold; never return otherwise.
#[cfg(target_os = "macos")]
async fn watchdog(pid: Option<u32>, limit_bytes: u64) {
    let Some(pid) = pid else {
        return std::future::pending().await;
    };
    let threshold = watchdog_threshold(
        limit_bytes,
        WATCHDOG_SAMPLE_PERIOD_MS,
        WORST_ALLOCATION_RATE_BYTES_PER_SECOND,
    );
    let mut ticks = tokio::time::interval(Duration::from_millis(WATCHDOG_SAMPLE_PERIOD_MS));
    loop {
        let _instant = ticks.tick().await;
        if footprint(pid).is_some_and(|bytes| bytes >= threshold) {
            return;
        }
    }
}

/// The one function that reads the child's memory for the watchdog: its physical footprint,
/// from libproc 0.14.11's `pidrusage::<RUsageInfoV2>` (`proc_pid_rusage`, flavor V2), field
/// `ri_phys_footprint` (`src/libproc/pid_rusage.rs`). `None` when the child cannot be read
/// (it has already exited).
#[cfg(target_os = "macos")]
fn footprint(pid: u32) -> Option<u64> {
    use libproc::libproc::pid_rusage::{RUsageInfoV2, pidrusage};
    let pid = i32::try_from(pid).ok()?;
    pidrusage::<RUsageInfoV2>(pid)
        .ok()
        .map(|usage| usage.ri_phys_footprint)
}

/// Windows and Linux: no watchdog; the operating system enforces the cap.
#[cfg(not(target_os = "macos"))]
async fn watchdog(_pid: Option<u32>, _limit_bytes: u64) {
    std::future::pending::<()>().await;
}

/// Whether a whole line is exactly Rust's message for a failed allocation.
fn is_rust_allocation_failure(line: &str) -> bool {
    let (before, after) = RUST_ALLOCATION_FAILURE;
    line.strip_prefix(before)
        .and_then(|rest| rest.strip_suffix(after))
        .is_some_and(|size| !size.is_empty() && size.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Read a child's stderr to its end, keeping the last `STDERR_TAIL_BYTES`.
async fn stderr_tail(stderr: Option<ChildStderr>) -> String {
    let Some(mut stderr) = stderr else {
        return String::new();
    };
    let mut kept: Vec<u8> = Vec::new();
    let mut buffer = vec![0_u8; 4096];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                kept.extend(buffer.iter().take(read));
                let excess = kept.len().saturating_sub(STDERR_TAIL_BYTES);
                if excess > 0 {
                    let _dropped = kept.drain(..excess);
                }
            }
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// A worker fault.
fn fault(message: &str) -> ApiError {
    ApiError::new(ErrorCode::Internal, message)
}
