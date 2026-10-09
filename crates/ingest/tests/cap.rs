//! The memory cap of the conversion child (`converter-worker-memory-ceiling`, part a): a real
//! child allocates past a real cap on the platform the test runs on, and its window is read as
//! `Failure(MemoryLimit)` with its pages not converted. Then how a child's end is read: a
//! reported result, the cap, the time bound or a crash (integration-owner ruling on C2).
//!
//! The child is this test binary run again (`--exact`, with `CHILD_ENV` set). It loads no
//! model, so CI runs it. Select it with `-- memory_cap_mechanism`.
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
        CAP_MARGIN_MIN_BYTES, ChildVerdict, classify, limit_self, spawn_capped, supervise,
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
        // classifier's margin of the cap, far below what the child asked for. (Observed on
        // Windows 11: the job's peak commit stood 1.4 MiB above a 256 MiB limit when the
        // allocation was refused, so the bound is the margin, not the exact cap.)
        if let Some(peak) = end.peak_committed_bytes {
            assert!(
                peak <= CAP.saturating_add(CAP_MARGIN_MIN_BYTES),
                "peak commit {peak} passed the cap {CAP} by more than the margin"
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

mod memory_cap_classification {
    use okf_jawn_contract::extraction::FailureReason;
    use okf_jawn_ingest::cap::{
        CAP_MARGIN_MIN_BYTES, ChildEnd, ChildVerdict, WATCHDOG_SAMPLE_PERIOD_MS,
        WORST_ALLOCATION_RATE_BYTES_PER_SECOND, classify, watchdog_threshold,
    };

    const CAP: u64 = 2 * 1024 * 1024 * 1024;

    fn memory() -> ChildVerdict {
        ChildVerdict::Failed(FailureReason::MemoryLimit {
            limit_bytes: CAP.to_string(),
        })
    }

    #[test]
    fn a_child_that_reported_is_read_whatever_else_happened() {
        let end = ChildEnd {
            reported: true,
            stderr_tail: "memory allocation of 8 bytes failed".to_owned(),
            ..ChildEnd::default()
        };
        assert_eq!(classify(&end, CAP, 600), ChildVerdict::Reported);
    }

    #[test]
    fn the_watchdog_kill_and_an_allocation_failure_are_the_cap() {
        let killed = ChildEnd {
            killed_at_cap: true,
            ..ChildEnd::default()
        };
        assert_eq!(classify(&killed, CAP, 600), memory());
        for words in [
            "memory allocation of 1073741824 bytes failed\n",
            "terminate called after throwing an instance of 'std::bad_alloc'",
            "onnxruntime: Failed to allocate memory for requested buffer",
        ] {
            let aborted = ChildEnd {
                stderr_tail: words.to_owned(),
                ..ChildEnd::default()
            };
            assert_eq!(classify(&aborted, CAP, 600), memory(), "{words}");
        }
    }

    #[test]
    fn a_peak_commit_at_the_cap_is_the_cap_and_one_well_below_is_a_crash() {
        let at = ChildEnd {
            peak_committed_bytes: Some(CAP - CAP_MARGIN_MIN_BYTES / 2),
            ..ChildEnd::default()
        };
        assert_eq!(classify(&at, CAP, 600), memory());
        let below = ChildEnd {
            peak_committed_bytes: Some(CAP / 2),
            ..ChildEnd::default()
        };
        assert_eq!(
            classify(&below, CAP, 600),
            ChildVerdict::Failed(FailureReason::ConverterCrashed)
        );
    }

    #[test]
    fn the_watchdog_kills_at_the_cap_minus_the_greater_headroom() {
        const MIB: u64 = 1024 * 1024;
        // Two 75 ms samples at 512 MiB/s is 76.8 MiB; 5% of 2 GiB is 102.4 MiB, the greater.
        assert_eq!(
            watchdog_threshold(2048 * MIB, 75, 512 * MIB),
            2048 * MIB - 2048 * MIB / 20
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
    }

    #[test]
    fn the_time_bound_and_a_silent_death_are_told_apart_from_the_cap() {
        let timed_out = ChildEnd {
            killed_at_time_limit: true,
            ..ChildEnd::default()
        };
        assert_eq!(
            classify(&timed_out, CAP, 600),
            ChildVerdict::Failed(FailureReason::TimeLimit { limit_seconds: 600 })
        );
        let panicked = ChildEnd {
            stderr_tail: "thread 'main' panicked at src/lib.rs:1:1".to_owned(),
            ..ChildEnd::default()
        };
        assert_eq!(
            classify(&panicked, CAP, 600),
            ChildVerdict::Failed(FailureReason::ConverterCrashed)
        );
    }
}
