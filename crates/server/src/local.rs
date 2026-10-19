//! Local authentication's launch tokens: single-use, unpredictable, held only as hashes, and
//! good for a bounded time.
//!
//! SPEC section 11: "startup prints a single-use launch URL whose unpredictable token is
//! exchanged for an `HttpOnly`, `SameSite=Strict` session cookie." A token comes from a
//! [`SecretSource`] (the operating system's random source in production), is handed to the
//! caller that prints it, and is kept only as its SHA-256 and the instant it was issued.
//! Redeeming removes it under one lock, so a token works once, also when several exchanges race,
//! and a replay is refused; a token that was never issued is refused the same way. A token is
//! refused from the instant its lifetime ends ([`LAUNCH_TOKEN_LIFETIME`] in production), so
//! captured startup output is worth a bounded time; expired entries are dropped as tokens are
//! issued and redeemed. An exchange that fails after it took the token gives it back
//! ([`LaunchTokens::give_back`]) with the instant it was first issued, so a store or event-log
//! fault does not burn the launch link and never extends it. A completed sign-in is announced
//! ([`LaunchTokens::sign_in_completed`]), so the launch link can publish the next token
//! (`launch`).

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::credentials::secret_hash;
use tokio::sync::Notify;

use crate::grants::Clock;

/// Unpredictable secrets: launch tokens, session-cookie secrets and OAuth state.
///
/// Production draws at least 32 bytes from the operating system's random source (Batch C,
/// `getrandom`); the bindings are built against this trait.
pub trait SecretSource: Send + Sync {
    /// A fresh secret, URL- and cookie-safe.
    ///
    /// # Errors
    /// Returns `Unavailable` when the random source fails.
    fn secret(&self) -> Result<String, ApiError>;
}

/// The launch tokens issued and not yet redeemed, by hash, with when each was issued.
pub struct LaunchTokens {
    outstanding: Mutex<Outstanding>,
    lifetime: Duration,
    clock: Arc<dyn Clock>,
    signed_in: Notify,
}

/// A token an exchange took: given back if the exchange fails, dropped when it completes.
#[derive(Debug)]
pub struct Taken {
    hash: [u8; 32],
    issued_at: Instant,
}

/// Outstanding token hashes paired with their original issue time.
type Outstanding = Vec<([u8; 32], Instant)>;

/// How long a launch token works in production: ten minutes.
pub const LAUNCH_TOKEN_LIFETIME: Duration = Duration::from_secs(600);

impl Taken {
    /// Consume the exchange's claim when returning its token; it cannot be returned twice.
    fn give_back(self, tokens: &LaunchTokens) -> Result<(), ApiError> {
        let now = tokens.clock.now();
        if tokens.live(self.issued_at, now) {
            tokens.lock()?.push((self.hash, self.issued_at));
        }
        Ok(())
    }
}

impl LaunchTokens {
    /// No token issued yet; each one issued works for `lifetime` by `clock`.
    #[must_use]
    pub fn new(lifetime: Duration, clock: Arc<dyn Clock>) -> Self {
        Self {
            outstanding: Mutex::new(Vec::new()),
            lifetime,
            clock,
            signed_in: Notify::new(),
        }
    }

    /// How long each token works.
    #[must_use]
    pub const fn lifetime(&self) -> Duration {
        self.lifetime
    }

    /// Issue a token from `secrets`; only its hash and the instant are kept.
    ///
    /// # Errors
    /// Returns the secret source's error, or `Internal` when the lock is poisoned.
    pub fn issue(&self, secrets: &dyn SecretSource) -> Result<String, ApiError> {
        let token = secrets.secret()?;
        let now = self.clock.now();
        let mut outstanding = self.lock()?;
        outstanding.retain(|(_, issued_at)| self.live(*issued_at, now));
        outstanding.push((secret_hash(&token), now));
        Ok(token)
    }

    /// Redeem `presented`: the taken token exactly once for an issued token whose lifetime has
    /// not ended, `None` for a replay, an expired token or a token never issued.
    ///
    /// # Errors
    /// Returns `Internal` when the lock is poisoned.
    pub fn redeem(&self, presented: &str) -> Result<Option<Taken>, ApiError> {
        let hash = secret_hash(presented);
        let now = self.clock.now();
        let mut outstanding = self.lock()?;
        outstanding.retain(|(_, issued_at)| self.live(*issued_at, now));
        let Some(position) = outstanding.iter().position(|(held, _)| *held == hash) else {
            return Ok(None);
        };
        let (hash, issued_at) = outstanding.swap_remove(position);
        Ok(Some(Taken { hash, issued_at }))
    }

    /// Give back a token that was redeemed for an exchange that then failed, so the person can
    /// try the same launch link again, for what is left of its lifetime. While the exchange ran
    /// the token was absent, so a second exchange at the same moment was refused. A token whose
    /// lifetime ended meanwhile is not kept.
    ///
    /// # Errors
    /// Returns `Internal` when the lock is poisoned.
    pub fn give_back(&self, taken: Taken) -> Result<(), ApiError> {
        taken.give_back(self)
    }

    /// How many tokens are outstanding, expired ones included until the next issue or redeem.
    ///
    /// # Errors
    /// Returns `Internal` when the lock is poisoned.
    pub fn outstanding(&self) -> Result<usize, ApiError> {
        Ok(self.lock()?.len())
    }

    /// Announce that an exchange completed: a browser session was opened with a launch token.
    pub fn sign_in_completed(&self) {
        // A permit is kept when nobody waits yet, so no completion is ever missed.
        self.signed_in.notify_one();
    }

    /// Wait for the next completed sign-in.
    pub async fn signed_in(&self) {
        self.signed_in.notified().await;
    }

    /// Whether a token issued at `issued_at` still works at `now`: stale at exactly its lifetime.
    fn live(&self, issued_at: Instant, now: Instant) -> bool {
        now.saturating_duration_since(issued_at) < self.lifetime
    }

    fn lock(&self) -> Result<MutexGuard<'_, Outstanding>, ApiError> {
        self.outstanding
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "The launch token lock is poisoned"))
    }
}
