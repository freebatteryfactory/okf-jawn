//! The memory cap of the conversion child (`converter-worker-memory-ceiling`, part a): a real
//! child allocates past a real cap on the platform the test runs on, and its window is read as
//! `Failure(MemoryLimit)` with its pages not converted. Then how a child's end is read: a
//! reported result, the cap, the time bound or a crash (integration-owner ruling on C2).
//!
//! The child is this test binary run again (`--exact`, with `CHILD_ENV` set). It loads no
//! model, so CI runs it. Select it with `-- memory_cap_mechanism`.
//!
//! `child_supervision` drops the supervision of a running child and checks the child stops:
//! `kill_on_drop` kills it. On Windows closing the job object kills it as well, so there the
//! test passes by either mechanism; Linux and macOS rely on `kill_on_drop` alone.
#![cfg(feature = "runtime")]

#[path = "../../../tests/support/check.rs"]
mod check;

mod memory_cap_mechanism {
    use std::time::Duration;

    use okf_jawn_contract::common::PageRange;
    use okf_jawn_contract::extraction::{
        ConversionOutcome, ConversionSettings, ConverterIdentity, FailureReason,
    };
    use okf_jawn_core::conversion::{ConversionStatus, WindowCoverage};
    use okf_jawn_ingest::cap::{
        ChildVerdict, cap_margin, classify, limit_self, spawn_capped, supervise,
    };
    use okf_jawn_ingest::record::{ConvertedWindow, WindowOutcome, assemble, sha256_digest};

    use crate::check::{TestResult, some};

    /// Set in the child, so the test body allocates instead of supervising.
    const CHILD_ENV: &str = "OKF_JAWN_INGEST_CAP_TEST_CHILD";
    /// This test's name, as `--exact` selects it.
    const THIS_TEST: &str =
        "memory_cap_mechanism::a_child_past_the_cap_is_stopped_and_its_window_is_not_converted";
    /// The cap the child runs under: 256 MiB.
    const CAP: u64 = 256 * 1024 * 1024;
    /// What the child tries to hold, far past the cap: 4 GiB.
    const ATTEMPT: usize = 4 * 1024 * 1024 * 1024;
    /// Each allocation, written so its pages are committed: 16 MiB.
    const CHUNK: usize = 16 * 1024 * 1024;
    /// The time bound; the cap must stop the child long before it.
    const TIME_LIMIT_SECONDS: u32 = 120;

    /// In the child: take the cap as the conversion child does, then allocate past it. Reaching
    /// the end means the cap did not hold; the supervisor then reads a crash, not the cap.
    fn allocate_past_the_cap() -> TestResult {
        let _cap = limit_self()?;
        let mut held: Vec<Vec<u8>> = Vec::new();
        let mut total = 0_usize;
        while total < ATTEMPT {
            held.push(vec![1_u8; CHUNK]);
            total = total.saturating_add(CHUNK);
        }
        Ok(())
    }

    fn pages(start: u32, end: u32) -> PageRange {
        PageRange { start, end }
    }

    #[test]
    fn a_child_past_the_cap_is_stopped_and_its_window_is_not_converted() -> TestResult {
        if std::env::var_os(CHILD_ENV).is_some() {
            return allocate_past_the_cap();
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let end = runtime.block_on(async {
            let mut command = tokio::process::Command::new(std::env::current_exe()?);
            let _configured = command
                .args(["--exact", THIS_TEST, "--nocapture", "--test-threads=1"])
                .env(CHILD_ENV, "1");
            let child = spawn_capped(command, CAP)?;
            let end = supervise(
                child,
                CAP,
                Duration::from_secs(u64::from(TIME_LIMIT_SECONDS)),
            )
            .await?;
            Ok::<_, Box<dyn std::error::Error>>(end)
        })?;
        assert!(
            !end.killed_at_time_limit,
            "the cap, not the time bound, stops the child"
        );
        // Stopped at the cap: where the platform reports the peak, it stayed within the
        // classifier's margin of the cap, far below what the child asked for, and reached the
        // cap by that margin, which is the evidence the classification reads there. (Observed
        // on Windows 11: the job's peak commit stood 1.4 MiB above a 256 MiB limit when the
        // allocation was refused, so the bound is the margin, not the exact cap.)
        if let Some(peak) = end.peak_committed_bytes {
            assert!(
                peak <= CAP.saturating_add(cap_margin(CAP)),
                "peak commit {peak} passed the cap {CAP} by more than the margin"
            );
            assert!(
                peak.saturating_add(cap_margin(CAP)) >= CAP,
                "peak commit {peak} never came near the cap {CAP}"
            );
        }
        let verdict = classify(&end, CAP, TIME_LIMIT_SECONDS);
        let memory = FailureReason::MemoryLimit {
            limit_bytes: CAP.to_string(),
        };
        assert_eq!(verdict, ChildVerdict::Failed(memory), "child end: {end:?}");

        // The capped window is pages not converted; the window before it stays converted.
        let ChildVerdict::Failed(reason) = verdict else {
            return Err("the child reported a result".into());
        };
        let first = WindowOutcome {
            window: Some(pages(1, 4)),
            status: ConversionStatus::Success,
            coverage: Some(WindowCoverage {
                window: pages(1, 4),
                converted: vec![pages(1, 4)],
                partly_extracted: Vec::new(),
                not_converted: Vec::new(),
            }),
            issues: Vec::new(),
            converted: Some(ConvertedWindow {
                markdown: "# One\n".to_owned(),
                export: sha256_digest(b"window one")?,
                locations: Vec::new(),
                tables: Vec::new(),
                assets: Vec::new(),
                warnings: Vec::new(),
            }),
        };
        let capped = WindowOutcome {
            window: Some(pages(5, 8)),
            status: ConversionStatus::Failure(reason),
            coverage: None,
            issues: Vec::new(),
            converted: None,
        };
        let identity = ConverterIdentity {
            name: "docling".to_owned(),
            version: "2.3.0".to_owned(),
            packages: Vec::new(),
            settings: ConversionSettings::default(),
            models: Vec::new(),
            page_window: Some(4),
        };
        let assembled = assemble(identity, Some(8), vec![first, capped])?;
        let ConversionOutcome::Partial { coverage, .. } = assembled.outcome else {
            return Err(format!("expected a partial outcome, got {:?}", assembled.outcome).into());
        };
        let coverage = some(coverage, "the joined coverage")?;
        assert_eq!(coverage.converted, vec![pages(1, 4)]);
        assert_eq!(coverage.not_converted, vec![pages(5, 8)]);
        Ok(())
    }
}

mod child_supervision {
    use std::io::Write as _;
    use std::time::Duration;

    use okf_jawn_ingest::cap::{spawn_capped, supervise};

    use crate::check::TestResult;

    /// Set in the child, naming the file it keeps writing to.
    const CHILD_ENV: &str = "OKF_JAWN_INGEST_SUPERVISION_TEST_CHILD";
    /// This test's name, as `--exact` selects it.
    const THIS_TEST: &str = "child_supervision::dropping_the_supervision_kills_the_child";
    /// A cap the child never comes near: 1 GiB.
    const CAP: u64 = 1024 * 1024 * 1024;

    /// In the child: append a byte to the file every 20 ms for a minute, far longer than the
    /// test waits, so a child left running keeps the file growing.
    fn keep_writing(path: &std::ffi::OsStr) -> TestResult {
        for _tick in 0..3000 {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            file.write_all(b".")?;
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    fn size(path: &std::path::Path) -> u64 {
        std::fs::metadata(path).map_or(0, |metadata| metadata.len())
    }

    #[test]
    fn dropping_the_supervision_kills_the_child() -> TestResult {
        if let Some(path) = std::env::var_os(CHILD_ENV) {
            return keep_writing(&path);
        }
        let directory = tempfile::tempdir()?;
        let beat = directory.path().join("beat");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let mut command = tokio::process::Command::new(std::env::current_exe()?);
            let _configured = command
                .args(["--exact", THIS_TEST, "--nocapture", "--test-threads=1"])
                .env(CHILD_ENV, &beat);
            let child = spawn_capped(command, CAP)?;
            // The supervision is dropped long before its own time bound, as a cancelled job
            // drops its conversion.
            let supervised = tokio::time::timeout(
                Duration::from_secs(3),
                supervise(child, CAP, Duration::from_secs(600)),
            )
            .await;
            assert!(supervised.is_err(), "the child ended by itself");
            Ok::<_, Box<dyn std::error::Error>>(())
        })?;
        assert!(size(&beat) > 0, "the child never ran");
        std::thread::sleep(Duration::from_millis(500));
        let settled = size(&beat);
        std::thread::sleep(Duration::from_millis(1000));
        assert_eq!(
            size(&beat),
            settled,
            "the child kept running after the drop"
        );
        Ok(())
    }
}

mod memory_cap_classification {
    use okf_jawn_contract::extraction::FailureReason;
    use okf_jawn_ingest::cap::{
        ALLOCATION_FAILURE_MARKER, CAP_MARGIN_MIN_BYTES, ChildEnd, ChildVerdict,
        WATCHDOG_SAMPLE_PERIOD_MS, WORST_ALLOCATION_RATE_BYTES_PER_SECOND, cap_margin, classify,
        watchdog_threshold,
    };

    use crate::check::{TestResult, err_of};

    const CAP: u64 = 2 * 1024 * 1024 * 1024;

    fn memory() -> ChildVerdict {
        ChildVerdict::Failed(FailureReason::MemoryLimit {
            limit_bytes: CAP.to_string(),
        })
    }

    /// The reason a verdict fails the window for; an error when the child reported.
    fn reason_of(verdict: ChildVerdict) -> Result<FailureReason, String> {
        match verdict {
            ChildVerdict::Failed(reason) => Ok(reason),
            ChildVerdict::Reported => Err("the child reported".to_owned()),
        }
    }

    #[test]
    fn a_child_that_reported_is_read_whatever_else_happened() -> TestResult {
        let end = ChildEnd {
            reported: true,
            stderr_tail: "memory allocation of 8 bytes failed".to_owned(),
            ..ChildEnd::default()
        };
        assert_eq!(
            err_of(reason_of(classify(&end, CAP, 600)))?,
            "the child reported"
        );
        Ok(())
    }

    #[test]
    fn the_watchdog_kill_and_an_allocation_failure_are_the_cap() -> TestResult {
        let killed = ChildEnd {
            killed_at_cap: true,
            ..ChildEnd::default()
        };
        assert_eq!(
            ChildVerdict::Failed(reason_of(classify(&killed, CAP, 600))?),
            memory()
        );
        for words in [
            "memory allocation of 1073741824 bytes failed\n".to_owned(),
            format!(
                "okf-jawn-convert: the retained original could not be read: out of memory\n{ALLOCATION_FAILURE_MARKER}\n"
            ),
        ] {
            let aborted = ChildEnd {
                stderr_tail: words.clone(),
                ..ChildEnd::default()
            };
            assert_eq!(classify(&aborted, CAP, 600), memory(), "{words}");
        }
        Ok(())
    }

    #[test]
    fn free_text_about_memory_is_never_the_cap() -> TestResult {
        // Only the child's own marker or Rust's exact message, as the last line, is evidence.
        // Words about memory anywhere else (a library log, a panic quoting document text) are
        // a crash.
        for words in [
            "okf-jawn-convert: the retained original could not be read: out of memory\n",
            "terminate called after throwing an instance of 'std::bad_alloc'",
            "onnxruntime: Failed to allocate memory for requested buffer",
            "thread 'main' panicked: byte index 3 is out of bounds of `Out of memory`",
            "memory allocation of 8 bytes failed\nthen something else went wrong\n",
            "note: memory allocation of 8 bytes failed here",
            "memory allocation of many bytes failed",
        ] {
            let crashed = ChildEnd {
                stderr_tail: words.to_owned(),
                ..ChildEnd::default()
            };
            assert_eq!(
                reason_of(classify(&crashed, CAP, 600))?,
                FailureReason::ConverterCrashed,
                "{words}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_non_memory_failure_under_a_small_cap_is_a_crash() -> TestResult {
        // A test-sized cap: the margin scales down with it, so a child that failed for another
        // reason, far below the cap, is not read as having reached it.
        const SMALL: u64 = 16 * 1024 * 1024;
        let margin = cap_margin(SMALL);
        assert!(margin < SMALL, "margin {margin} is not below the cap");
        let failed = ChildEnd {
            stderr_tail: "okf-jawn-convert: the retained original could not be read: not found\n"
                .to_owned(),
            peak_committed_bytes: Some(6 * 1024 * 1024),
            ..ChildEnd::default()
        };
        assert_eq!(
            classify(&failed, SMALL, 30),
            ChildVerdict::Failed(FailureReason::ConverterCrashed)
        );
        let at = ChildEnd {
            peak_committed_bytes: Some(SMALL.checked_sub(margin / 2).ok_or("the cap")?),
            ..ChildEnd::default()
        };
        assert_eq!(
            classify(&at, SMALL, 30),
            ChildVerdict::Failed(FailureReason::MemoryLimit {
                limit_bytes: SMALL.to_string()
            })
        );
        Ok(())
    }

    #[test]
    fn a_peak_commit_at_the_cap_is_the_cap_and_one_well_below_is_a_crash() -> TestResult {
        assert_eq!(cap_margin(CAP), CAP / 16);
        assert!(cap_margin(CAP) >= CAP_MARGIN_MIN_BYTES);
        let at = ChildEnd {
            peak_committed_bytes: Some(CAP.saturating_sub(cap_margin(CAP) / 2)),
            ..ChildEnd::default()
        };
        assert_eq!(classify(&at, CAP, 600), memory());
        let below = ChildEnd {
            peak_committed_bytes: Some(CAP / 2),
            ..ChildEnd::default()
        };
        assert_eq!(
            reason_of(classify(&below, CAP, 600))?,
            FailureReason::ConverterCrashed
        );
        Ok(())
    }

    #[test]
    fn the_watchdog_kills_at_the_cap_minus_the_greater_headroom() -> TestResult {
        const MIB: u64 = 1024 * 1024;
        // Two 75 ms samples at 512 MiB/s is 76.8 MiB; 5% of 2 GiB is 102.4 MiB, the greater.
        let two_gib = MIB.checked_mul(2048).ok_or("2 GiB in bytes")?;
        assert_eq!(
            watchdog_threshold(two_gib, 75, 512 * MIB),
            two_gib.checked_sub(two_gib / 20).ok_or("the threshold")?
        );
        // At 2 GiB/s growth dominates: 2 x 75 ms x 2 GiB/s = 307.2 MiB.
        assert_eq!(
            watchdog_threshold(2048 * MIB, 75, 2048 * MIB),
            2048 * MIB - 2048 * MIB * 150 / 1000
        );
        // Headroom never takes the threshold below zero.
        assert_eq!(watchdog_threshold(MIB, 100, 1024 * MIB), 0);
        assert_eq!(
            watchdog_threshold(
                2048 * MIB,
                WATCHDOG_SAMPLE_PERIOD_MS,
                WORST_ALLOCATION_RATE_BYTES_PER_SECOND
            ),
            2048 * MIB - 2048 * MIB / 20
        );
        Ok(())
    }

    #[test]
    fn the_time_bound_and_a_silent_death_are_told_apart_from_the_cap() -> TestResult {
        let timed_out = ChildEnd {
            killed_at_time_limit: true,
            ..ChildEnd::default()
        };
        assert_eq!(
            reason_of(classify(&timed_out, CAP, 600))?,
            FailureReason::TimeLimit { limit_seconds: 600 }
        );
        let panicked = ChildEnd {
            stderr_tail: "thread 'main' panicked at src/lib.rs:1:1".to_owned(),
            ..ChildEnd::default()
        };
        assert_eq!(
            classify(&panicked, CAP, 600),
            ChildVerdict::Failed(FailureReason::ConverterCrashed)
        );
        Ok(())
    }
}
