//! Local authentication's launch tokens: single-use, unpredictable, held only as hashes.
//!
//! SPEC section 11: "startup prints a single-use launch URL whose unpredictable token is
//! exchanged for an `HttpOnly`, `SameSite=Strict` session cookie." A token comes from a
//! [`SecretSource`] (the operating system's random source in production), is handed to the
//! caller that prints it, and is kept only as its SHA-256. Redeeming removes it, so a token works
//! once and a replay is refused; a token that was never issued is refused the same way.

use std::sync::Mutex;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::credentials::secret_hash;

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

/// The launch tokens issued and not yet redeemed, by hash.
#[derive(Debug, Default)]
pub struct LaunchTokens {
    outstanding: Mutex<Vec<[u8; 32]>>,
}

impl LaunchTokens {
    /// No token issued yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Issue a token from `secrets`; only its hash is kept.
    ///
    /// # Errors
    /// Returns the secret source's error, or `Internal` when the lock is poisoned.
    pub fn issue(&self, secrets: &dyn SecretSource) -> Result<String, ApiError> {
        let token = secrets.secret()?;
        self.lock()?.push(secret_hash(&token));
        Ok(token)
    }

    /// Redeem `presented`: true exactly once for an issued token, false for a replay or a
    /// token never issued.
    ///
    /// # Errors
    /// Returns `Internal` when the lock is poisoned.
    pub fn redeem(&self, presented: &str) -> Result<bool, ApiError> {
        let hash = secret_hash(presented);
        let mut outstanding = self.lock()?;
        let before = outstanding.len();
        outstanding.retain(|held| *held != hash);
        Ok(outstanding.len() != before)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Vec<[u8; 32]>>, ApiError> {
        self.outstanding
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "The launch token lock is poisoned"))
    }
}
