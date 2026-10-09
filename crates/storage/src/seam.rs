//! Identities, the clock and randomness, each behind one private function.
//!
//! TODO(Batch A): the storage manifest gains `uuid`, `time` and `getrandom` in Batch A. Until
//! then these functions draw on the bundled SQLite: `strftime('%Y-%m-%dT%H:%M:%fZ', 'now')` is
//! the canonical `Timestamp` spelling (UTC, three fraction digits), and `randomblob` is SQLite's
//! randomness. When Batch A lands, `now` becomes `Timestamp::from_utc(OffsetDateTime::now_utc())`,
//! `new_uuid` becomes `Uuid::new_v4()`, and `random_bytes` becomes `getrandom::fill`, which is the
//! operating system's random source that `CredentialStore::create_connector` requires. Every
//! caller goes through these functions, so the swap changes this file only.

use std::time::{SystemTime, UNIX_EPOCH};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::Timestamp;
use rusqlite::Connection;

/// The current instant in the canonical spelling.
pub(crate) fn now() -> Result<Timestamp, ApiError> {
    later(0)
}

/// The instant `seconds` from now in the canonical spelling.
pub(crate) fn later(seconds: i64) -> Result<Timestamp, ApiError> {
    let connection = Connection::open_in_memory().map_err(|error| clock_error(&error))?;
    let spelled: String = connection
        .query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1)",
            [format!("{seconds:+} seconds")],
            |row| row.get(0),
        )
        .map_err(|error| clock_error(&error))?;
    Timestamp::try_from(spelled).map_err(|error| {
        ApiError::new(
            ErrorCode::Internal,
            format!("the clock did not give a canonical instant: {error}"),
        )
    })
}

/// The canonical spelling of a whole-second Unix time, such as a Git commit time.
pub(crate) fn unix_instant(seconds: i64) -> Result<Timestamp, ApiError> {
    let connection = Connection::open_in_memory().map_err(|error| clock_error(&error))?;
    let spelled: String = connection
        .query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', ?1, 'unixepoch')",
            [seconds],
            |row| row.get(0),
        )
        .map_err(|error| clock_error(&error))?;
    Timestamp::try_from(spelled).map_err(|error| {
        ApiError::new(
            ErrorCode::Internal,
            format!("a commit time has no canonical spelling: {error}"),
        )
    })
}

/// Milliseconds since the Unix epoch, for lease and retention arithmetic inside one store.
pub(crate) fn now_ms() -> Result<i64, ApiError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ApiError::new(ErrorCode::Internal, "the system clock is before 1970"))?;
    i64::try_from(elapsed.as_millis())
        .map_err(|_| ApiError::new(ErrorCode::Internal, "the system clock is out of range"))
}

/// A fresh random (version 4) UUID as a JSON string, for `serde_json::from_value` into the
/// identity type the caller names.
pub(crate) fn new_uuid() -> Result<serde_json::Value, ApiError> {
    let mut bytes = random_bytes::<16>()?;
    if let Some(version) = bytes.get_mut(6) {
        *version = (*version & 0x0f) | 0x40;
    }
    if let Some(variant) = bytes.get_mut(8) {
        *variant = (*variant & 0x3f) | 0x80;
    }
    let hex = hex(&bytes);
    let mut spelled = String::with_capacity(36);
    for (index, character) in hex.chars().enumerate() {
        if matches!(index, 8 | 12 | 16 | 20) {
            spelled.push('-');
        }
        spelled.push(character);
    }
    Ok(serde_json::Value::String(spelled))
}

/// `N` random bytes.
pub(crate) fn random_bytes<const N: usize>() -> Result<[u8; N], ApiError> {
    let connection = Connection::open_in_memory().map_err(|error| random_error(&error))?;
    let blob: Vec<u8> = connection
        .query_row(
            "SELECT randomblob(?1)",
            [i64::try_from(N).unwrap_or(0)],
            |row| row.get(0),
        )
        .map_err(|error| random_error(&error))?;
    blob.try_into().map_err(|_| {
        ApiError::new(
            ErrorCode::Internal,
            "the random source returned a short read",
        )
    })
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

fn clock_error(error: &rusqlite::Error) -> ApiError {
    ApiError::new(
        ErrorCode::Internal,
        format!("the clock is unavailable: {error}"),
    )
}

fn random_error(error: &rusqlite::Error) -> ApiError {
    ApiError::new(
        ErrorCode::Internal,
        format!("the random source is unavailable: {error}"),
    )
}
