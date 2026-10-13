//! The bearer token of an operation and the connector credential of the relay.
//!
//! A credential comes from the environment or, for the relay, from the first line of a file.
//! It is never an argument. The type has no `Display` and its `Debug` prints no secret, and
//! [`Credential::redact`] removes the secret from any text the CLI itself is about to write.

use std::fmt::{Debug, Formatter};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use reqwest::header::HeaderValue;

/// A secret the service accepts as `Authorization: Bearer`.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential(String);

/// Most bytes of a credential file's first line that are read.
const MAX_CREDENTIAL_BYTES: u64 = 8192;

/// What replaces the secret in text the CLI writes.
const REDACTED: &str = "[credential]";

impl Credential {
    /// The `Authorization` header value, marked sensitive so the HTTP stack never logs it.
    ///
    /// # Errors
    /// Returns `Internal` when the header value cannot be built; `parse` admits only bytes a
    /// header value may hold, so this is not expected.
    pub fn bearer(&self) -> Result<HeaderValue, ApiError> {
        let mut value = HeaderValue::from_str(&format!("Bearer {}", self.0)).map_err(|_| {
            ApiError::new(
                ErrorCode::Internal,
                "The credential cannot be sent as a header",
            )
        })?;
        value.set_sensitive(true);
        Ok(value)
    }

    /// Read the credential the environment variable `name` holds, if it is set and not blank.
    ///
    /// # Errors
    /// Returns `InvalidInput` when the value is not a usable credential.
    pub fn from_environment(
        environment: &dyn Fn(&str) -> Option<String>,
        name: &str,
    ) -> Result<Option<Self>, ApiError> {
        match environment(name) {
            Some(raw) => Self::parse(&raw, name),
            None => Ok(None),
        }
    }

    /// Read the credential on the first line of `path`.
    ///
    /// # Errors
    /// Returns `InvalidInput` when the file cannot be read, when its first line is blank, or
    /// when that line is not a usable credential. The line itself is never repeated.
    pub fn from_file(path: &Path) -> Result<Self, ApiError> {
        let unreadable = |error: std::io::Error| {
            ApiError::new(
                ErrorCode::InvalidInput,
                format!(
                    "The credential file {} cannot be read: {error}",
                    path.display()
                ),
            )
        };
        let file = std::fs::File::open(path).map_err(unreadable)?;
        let mut first = Vec::new();
        BufReader::new(file.take(MAX_CREDENTIAL_BYTES))
            .read_until(b'\n', &mut first)
            .map_err(unreadable)?;
        let line = String::from_utf8(first).map_err(|_| {
            ApiError::new(
                ErrorCode::InvalidInput,
                format!(
                    "The first line of the credential file {} is not text",
                    path.display()
                ),
            )
        })?;
        Self::parse(&line, "--token-file")?.ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidInput,
                format!(
                    "The first line of the credential file {} is empty",
                    path.display()
                ),
            )
        })
    }

    /// Take a credential from raw text: surrounding whitespace is dropped, and blank is none.
    ///
    /// `source` names where the text came from, for the refusal.
    ///
    /// # Errors
    /// Returns `InvalidInput` when the text holds a byte that an HTTP header value cannot
    /// carry or a space; the text itself is never repeated.
    pub fn parse(raw: &str, source: &str) -> Result<Option<Self>, ApiError> {
        let secret = raw.trim();
        if secret.is_empty() {
            return Ok(None);
        }
        if !secret.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(ApiError::new(
                ErrorCode::InvalidInput,
                format!(
                    "The credential in {source} is not usable: it must be printable ASCII \
                     without spaces"
                ),
            ));
        }
        Ok(Some(Self(secret.to_owned())))
    }

    /// `text` with every occurrence of the secret replaced.
    #[must_use]
    pub fn redact(&self, text: &str) -> String {
        text.replace(&self.0, REDACTED)
    }
}

impl Debug for Credential {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Credential([credential])")
    }
}
