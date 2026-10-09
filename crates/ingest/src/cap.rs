//! The memory cap of the conversion child, and how a child's end is read.
//!
//! Conversion runs in a child process with a hard memory cap (`converter-worker-memory-ceiling`,
//! O3). Each platform enforces it with the strongest facility it has, behind `MemoryCap`:
//!
//! - Windows: a job object with a committed-memory limit, assigned before the child runs.
//! - Linux: the child sets its own `RLIMIT_DATA` as the first thing it does.
//! - macOS: a supervisor watchdog samples the child's `phys_footprint` (what jetsam uses) and
//!   kills it with SIGKILL at `watchdog_threshold`, the cap minus a headroom. The metric is read
//!   in one private function, so the crate behind it can change (sysinfo, then libproc).
//!   The owner has not yet confirmed that this counts as the hard cap there.
//!
//! The facilities come from Batch A (`processkit`, `rlimit`, `sysinfo`). Until it is on `main`
//! this module holds the interface and the classification only, and no platform is capped.
//!
//! Only the watchdog kills. Under the other two facilities an allocation fails inside the
//! child, which then aborts or reports an error. So whether the cap fired is classified, not
//! signalled (integration-owner ruling on concern C2). The classification:
//!
//! 1. A child that wrote its result reported for itself, whatever its exit.
//! 2. A child the watchdog killed hit the cap.
//! 3. A child whose last words name an allocation failure hit the cap.
//! 4. On Windows, a child whose peak committed memory reached the cap hit it. "Reached" means
//!    within `CAP_MARGIN_DIVISOR`'s share of the cap, at least `CAP_MARGIN_MIN_BYTES`, because
//!    the allocation that failed was never committed.
//! 5. Any other end without a result is a crash.

use okf_jawn_contract::extraction::FailureReason;

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

/// What a child's end means for its window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildVerdict {
    /// The child reported its own result; read it.
    Reported,
    /// No result: the window failed for this reason.
    Failed(FailureReason),
}

/// The share of the cap, as a divisor, within which a peak counts as having reached it.
pub const CAP_MARGIN_DIVISOR: u64 = 16;

/// The least margin below the cap that a peak counts as having reached it: 64 MiB.
pub const CAP_MARGIN_MIN_BYTES: u64 = 64 * 1024 * 1024;

/// What a process writes to stderr when an allocation fails: Rust's default handler, the C++
/// runtime ONNX Runtime throws through, and ONNX Runtime's own allocator.
pub const ALLOCATION_FAILURE_MARKERS: [&str; 4] = [
    "memory allocation of",
    "std::bad_alloc",
    "bad allocation",
    "Failed to allocate memory",
];

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

/// Read a child's end against its cap and time bound.
#[must_use]
pub fn classify(end: &ChildEnd, limit_bytes: u64, limit_seconds: u32) -> ChildVerdict {
    if end.reported {
        return ChildVerdict::Reported;
    }
    let memory = || FailureReason::MemoryLimit {
        limit_bytes: limit_bytes.to_string(),
    };
    if end.killed_at_cap {
        return ChildVerdict::Failed(memory());
    }
    if end.killed_at_time_limit {
        return ChildVerdict::Failed(FailureReason::TimeLimit { limit_seconds });
    }
    if ALLOCATION_FAILURE_MARKERS
        .iter()
        .any(|marker| end.stderr_tail.contains(marker))
    {
        return ChildVerdict::Failed(memory());
    }
    let margin = (limit_bytes / CAP_MARGIN_DIVISOR).max(CAP_MARGIN_MIN_BYTES);
    if end
        .peak_committed_bytes
        .is_some_and(|peak| peak.saturating_add(margin) >= limit_bytes)
    {
        return ChildVerdict::Failed(memory());
    }
    ChildVerdict::Failed(FailureReason::ConverterCrashed)
}
