//! How a conversion child's end is read: a reported result, the cap, the time bound or a crash
//! (integration-owner ruling on concern C2). The cap mechanism itself, a child that allocates
//! past a real cap on each platform, is added once the Batch A facilities are on `main`.
#![cfg(feature = "runtime")]

#[path = "../../../tests/support/check.rs"]
mod check;

mod memory_cap_classification {
    use okf_jawn_contract::extraction::FailureReason;
    use okf_jawn_ingest::cap::{CAP_MARGIN_MIN_BYTES, ChildEnd, ChildVerdict, classify};

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
