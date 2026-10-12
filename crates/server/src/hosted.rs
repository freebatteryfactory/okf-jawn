//! Hosted browser sign-in through WorkOS AuthKit: PKCE, single-use state, and the code exchange.
//!
//! `/auth/login` asks [`HostedLogin::begin`] for an AuthKit authorization URL with a fresh PKCE
//! verifier and state, keeps the pair in [`PendingLogins`] (state as its SHA-256, for at most
//! [`LOGIN_TTL`]) and redirects. `/auth/callback` takes the state back once: an unknown,
//! replayed or expired state is refused before any code is exchanged. The exchange
//! ([`HostedLogin::complete`]) yields the WorkOS user and organization, which become the
//! subject and tenant of a server-side browser session with route `BrowserSession`. Codes and
//! tokens are never logged or echoed. The production login is WorkOS (`WorkosLogin`, runtime);
//! the routes are bound against the trait.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::credentials::secret_hash;
use okf_jawn_core::ports::PortFuture;

use crate::grants::Clock;

/// The identity provider's half of a browser sign-in.
pub trait HostedLogin: Send + Sync {
    /// An authorization URL carrying a fresh PKCE challenge and state.
    ///
    /// # Errors
    /// Returns `Unavailable` when the URL cannot be built.
    fn begin(&self) -> Result<LoginStart, ApiError>;

    /// Exchange an authorization code with its PKCE verifier for the signed-in user.
    fn complete<'a>(&'a self, code: &'a str, verifier: &'a str) -> PortFuture<'a, HostedUser>;
}

/// A started sign-in: where to send the browser, and what the callback must present.
#[derive(Clone)]
pub struct LoginStart {
    /// The AuthKit authorization URL.
    pub url: String,
    /// The PKCE verifier; kept server-side, never sent to the browser.
    pub verifier: String,
    /// The state the callback must return.
    pub state: String,
}

/// The user a completed sign-in names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedUser {
    /// The WorkOS user id, the session's subject.
    pub subject: String,
    /// The WorkOS organization, the session's tenant.
    pub organization_id: String,
}

/// Sign-ins started and not yet completed, by state hash.
pub struct PendingLogins {
    clock: Arc<dyn Clock>,
    pending: Mutex<Vec<Pending>>,
}

struct Pending {
    state: [u8; 32],
    verifier: String,
    at: Instant,
}

/// How long a started sign-in may wait for its callback.
pub const LOGIN_TTL: Duration = Duration::from_secs(600);

impl PendingLogins {
    /// No sign-in pending; time read from `clock`.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            pending: Mutex::new(Vec::new()),
        }
    }

    /// Keep `start`'s verifier under its state.
    ///
    /// # Errors
    /// Returns `Internal` when the lock is poisoned.
    pub fn remember(&self, start: &LoginStart) -> Result<(), ApiError> {
        let now = self.clock.now();
        let mut pending = self.lock()?;
        pending.retain(|entry| live(entry.at, now));
        pending.push(Pending {
            state: secret_hash(&start.state),
            verifier: start.verifier.clone(),
            at: now,
        });
        Ok(())
    }

    /// The verifier of the sign-in `state` started, taken once; `None` for an unknown, replayed
    /// or expired state.
    ///
    /// # Errors
    /// Returns `Internal` when the lock is poisoned.
    pub fn take(&self, state: &str) -> Result<Option<String>, ApiError> {
        let now = self.clock.now();
        let hash = secret_hash(state);
        let mut pending = self.lock()?;
        pending.retain(|entry| live(entry.at, now));
        let found = pending.iter().position(|entry| entry.state == hash);
        Ok(found.map(|index| pending.swap_remove(index).verifier))
    }

    fn lock(&self) -> Result<MutexGuard<'_, Vec<Pending>>, ApiError> {
        self.pending
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "The pending sign-in lock is poisoned"))
    }
}

/// Whether a sign-in started at `at` may still complete at `now`.
fn live(at: Instant, now: Instant) -> bool {
    now.checked_duration_since(at)
        .is_some_and(|age| age < LOGIN_TTL)
}
