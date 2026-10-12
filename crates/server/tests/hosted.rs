//! Hosted sign-in state: a started sign-in's state works once, a replayed, unknown or expired
//! state is refused before any code is exchanged.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use okf_jawn_server::grants::Clock;
use okf_jawn_server::hosted::{LOGIN_TTL, LoginStart, PendingLogins};

use check::TestResult;

/// A clock that moves only when a test advances it.
struct TestClock {
    start: Instant,
    elapsed_ms: AtomicU64,
}

impl Clock for TestClock {
    fn now(&self) -> Instant {
        self.start
            .checked_add(Duration::from_millis(
                self.elapsed_ms.load(Ordering::SeqCst),
            ))
            .unwrap_or(self.start)
    }
}

fn start(state: &str, verifier: &str) -> LoginStart {
    LoginStart {
        url: "https://auth.example.test/authorize".to_owned(),
        verifier: verifier.to_owned(),
        state: state.to_owned(),
    }
}

#[test]
fn a_state_returns_its_verifier_once() -> TestResult {
    let clock = Arc::new(TestClock {
        start: Instant::now(),
        elapsed_ms: AtomicU64::new(0),
    });
    let pending = PendingLogins::new(clock);
    pending.remember(&start("state-a", "verifier-a"))?;
    pending.remember(&start("state-b", "verifier-b"))?;
    assert_eq!(pending.take("state-a")?.as_deref(), Some("verifier-a"));
    assert_eq!(pending.take("state-a")?, None);
    assert_eq!(pending.take("never-started")?, None);
    assert_eq!(pending.take("state-b")?.as_deref(), Some("verifier-b"));
    Ok(())
}

#[test]
fn an_expired_state_is_refused() -> TestResult {
    let clock = Arc::new(TestClock {
        start: Instant::now(),
        elapsed_ms: AtomicU64::new(0),
    });
    let pending = PendingLogins::new(clock.clone());
    pending.remember(&start("state-a", "verifier-a"))?;
    clock
        .elapsed_ms
        .store(u64::try_from(LOGIN_TTL.as_millis())?, Ordering::SeqCst);
    assert_eq!(pending.take("state-a")?, None);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
