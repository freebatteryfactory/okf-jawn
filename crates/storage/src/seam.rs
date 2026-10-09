//! Identities, the clock and randomness, each behind one private function.
//!
//! Identities are random (version 4) UUIDs from `uuid`; instants come from `time` and are
//! written through `Timestamp::from_utc`; random bytes come from `getrandom::fill`, the
//! operating system's random source that `CredentialStore::create_connector` requires for a
//! connector secret. Every caller goes through these functions, so no store draws an identity,
//! an instant or a secret from anywhere else.

use std::time::{SystemTime, UNIX_EPOCH};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::Timestamp;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// The current instant in the canonical spelling.
pub(crate) fn now() -> Result<Timestamp, ApiError> {
    canonical(OffsetDateTime::now_utc())
}

/// The instant `seconds` from now in the canonical spelling.
pub(crate) fn later(seconds: i64) -> Result<Timestamp, ApiError> {
    OffsetDateTime::now_utc()
        .checked_add(Duration::seconds(seconds))
        .ok_or_else(|| clock_error("an instant lies outside the representable range"))
        .and_then(canonical)
}

/// The canonical spelling of a whole-second Unix time, such as a Git commit time.
pub(crate) fn unix_instant(seconds: i64) -> Result<Timestamp, ApiError> {
    OffsetDateTime::from_unix_timestamp(seconds)
        .map_err(|error| clock_error(&format!("a commit time is out of range: {error}")))
        .and_then(canonical)
}

/// Milliseconds since the Unix epoch, for lease and retention arithmetic inside one store.
pub(crate) fn now_ms() -> Result<i64, ApiError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| clock_error("the system clock is before 1970"))?;
    i64::try_from(elapsed.as_millis()).map_err(|_| clock_error("the system clock is out of range"))
}

/// A fresh random (version 4) UUID as a JSON string, for `serde_json::from_value` into the
/// identity type the caller names.
pub(crate) fn new_uuid() -> serde_json::Value {
    serde_json::Value::String(Uuid::new_v4().to_string())
}

/// `N` bytes from the operating system's random source.
pub(crate) fn random_bytes<const N: usize>() -> Result<[u8; N], ApiError> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(|error| {
        ApiError::new(
            ErrorCode::Unavailable,
            format!("the operating system's random source is unavailable: {error}"),
        )
    })?;
    #[cfg(test)]
    draws::count();
    Ok(bytes)
}

/// Lower-case hexadecimal spelling of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(
        String::with_capacity(bytes.len().saturating_mul(2)),
        |mut text, byte| {
            // Writing to a String cannot fail.
            let _ = write!(text, "{byte:02x}");
            text
        },
    )
}

fn canonical(instant: OffsetDateTime) -> Result<Timestamp, ApiError> {
    Timestamp::from_utc(instant)
        .map_err(|error| clock_error(&format!("the clock gave no canonical instant: {error}")))
}

fn clock_error(message: &str) -> ApiError {
    ApiError::new(
        ErrorCode::Internal,
        format!("the clock is unavailable: {message}"),
    )
}

/// How many draws this thread made from the random source, so a unit test can tell that a
/// value came through `random_bytes` and nowhere else.
#[cfg(test)]
pub(crate) mod draws {
    use std::cell::Cell;

    thread_local! {
        static DRAWS: Cell<usize> = const { Cell::new(0) };
    }

    /// Record one draw.
    pub(crate) fn count() {
        DRAWS.with(|draws| draws.set(draws.get().saturating_add(1)));
    }

    /// The draws so far on this thread.
    pub(crate) fn so_far() -> usize {
        DRAWS.with(Cell::get)
    }
}
