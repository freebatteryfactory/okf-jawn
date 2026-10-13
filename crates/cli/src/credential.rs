//! The bearer token of an operation and the connector credential of the relay.
//!
//! A credential comes from the environment or, for the relay, from the first line of a file.
//! It is never an argument. The type has no `Display` and its `Debug` prints no secret.
//!
//! Two functions keep the secret out of what the CLI writes:
//! - [`Credential::redact`] is for plain text the CLI composes, such as a failure on the error
//!   stream. It replaces the secret as it stands and as a JSON or Rust string literal spells it
//!   (a quote and a backslash escaped), which is how the CLI itself embeds a value in a line.
//! - [`Credential::redact_line`] is for one JSON line, whoever wrote it. It reads the strings of
//!   the line as a JSON parser would, so the secret is found under the spellings JSON has for
//!   its characters (an escaped quote, backslash or slash, and a `\u` escape), and replaces
//!   those bytes and no others. A line that spells the secret outside a string, as a number
//!   does, cannot be both clean and still JSON: it is refused whole.
//!
//! Neither finds a secret that was transformed (Base64, a hash, split over two strings).

use std::collections::VecDeque;
use std::fmt::{Debug, Formatter};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use reqwest::header::HeaderValue;

/// A secret the service accepts as `Authorization: Bearer`.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential(String);

/// One character of a JSON line as a parser reads it.
#[derive(Debug, Clone, Copy)]
struct Unit {
    /// Where its spelling starts in the line.
    start: usize,
    /// Where its spelling ends.
    end: usize,
    /// The ASCII byte it stands for; [`NO_MATCH`] for an escape that stands for anything else.
    byte: u8,
    /// Whether it is part of a string's content.
    inside: bool,
}

/// Walks a JSON line unit by unit, decoding the escapes inside its strings.
#[derive(Debug)]
struct Units<'line> {
    bytes: &'line [u8],
    at: usize,
    inside: bool,
}

/// Most bytes of a credential file's first line that are read.
const MAX_CREDENTIAL_BYTES: u64 = 8192;

/// A byte no credential holds and no UTF-8 text contains: what an escape decodes to when it
/// stands for a character a credential cannot hold.
const NO_MATCH: u8 = 0xFF;

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

    /// The secret as a JSON or Rust string literal spells it, without the quotes.
    fn quoted(&self) -> Option<String> {
        let literal = serde_json::to_string(&self.0).ok()?;
        Some(literal.strip_prefix('"')?.strip_suffix('"')?.to_owned())
    }

    /// Plain `text` with every occurrence of the secret replaced, as it stands and as a string
    /// literal spells it.
    #[must_use]
    pub fn redact(&self, text: &str) -> String {
        let plain = text.replace(&self.0, REDACTED);
        match self.quoted() {
            Some(quoted) if quoted != self.0 => plain.replace(&quoted, REDACTED),
            _ => plain,
        }
    }

    /// One JSON `line` with the secret replaced in every string that spells it, whatever
    /// escapes spell it; a line without the secret comes back as it is.
    ///
    /// Returns `None` when the line spells the secret outside a string or across two of them:
    /// replacing it there would leave no JSON, so the line must not be written at all.
    #[must_use]
    pub fn redact_line(&self, line: String) -> Option<String> {
        let secret = self.0.as_bytes();
        let fallback = fallbacks(secret);
        let mut matched = 0_usize;
        let mut run = 0_usize;
        let mut starts = VecDeque::with_capacity(secret.len().saturating_add(1));
        let mut found = Vec::new();
        for unit in Units::new(&line) {
            run = if unit.inside {
                run.saturating_add(1)
            } else {
                0
            };
            starts.push_back(unit.start);
            if starts.len() > secret.len() {
                starts.pop_front();
            }
            while matched > 0 && secret.get(matched) != Some(&unit.byte) {
                matched = fallback
                    .get(matched.saturating_sub(1))
                    .copied()
                    .unwrap_or(0);
            }
            if secret.get(matched) == Some(&unit.byte) {
                matched = matched.saturating_add(1);
            }
            if matched == secret.len() {
                if run < secret.len() {
                    return None;
                }
                found.push((starts.front().copied()?, unit.end));
                matched = 0;
            }
        }
        if found.is_empty() {
            return Some(line);
        }
        let mut clean = String::with_capacity(line.len());
        let mut copied = 0;
        for (start, end) in found {
            clean.push_str(line.get(copied..start)?);
            clean.push_str(REDACTED);
            copied = end;
        }
        clean.push_str(line.get(copied..)?);
        Some(clean)
    }
}

impl Debug for Credential {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Credential([credential])")
    }
}

impl Iterator for Units<'_> {
    type Item = Unit;

    fn next(&mut self) -> Option<Unit> {
        let start = self.at;
        let first = *self.bytes.get(start)?;
        let (byte, length, inside) = match (self.inside, first) {
            (false, _) => {
                self.inside = first == b'"';
                (first, 1, false)
            }
            (true, b'"') => {
                self.inside = false;
                (first, 1, false)
            }
            (true, b'\\') => {
                let (byte, length) = escape(self.bytes.get(start.saturating_add(1)..));
                (byte, length, true)
            }
            (true, _) => (first, 1, true),
        };
        self.at = start.saturating_add(length);
        Some(Unit {
            start,
            end: self.at,
            byte,
            inside,
        })
    }
}

impl<'line> Units<'line> {
    fn new(line: &'line str) -> Self {
        Self {
            bytes: line.as_bytes(),
            at: 0,
            inside: false,
        }
    }
}

/// What the escape after a backslash stands for, and how many bytes spell it, the backslash
/// included. `rest` is what follows the backslash.
fn escape(rest: Option<&[u8]>) -> (u8, usize) {
    match rest.and_then(<[u8]>::first) {
        Some(plain @ (b'"' | b'\\' | b'/')) => (*plain, 2),
        Some(b'u') => {
            let code = rest
                .and_then(|rest| rest.get(1..5))
                .filter(|digits| digits.iter().all(u8::is_ascii_hexdigit))
                .and_then(|digits| std::str::from_utf8(digits).ok())
                .and_then(|digits| u8::from_str_radix(digits, 16).ok())
                .filter(u8::is_ascii);
            (code.unwrap_or(NO_MATCH), 6)
        }
        _ => (NO_MATCH, 2),
    }
}

/// For each prefix of `secret`, the length of its longest proper prefix that is also its
/// suffix: where the search resumes after a mismatch, so that no byte of a line is read twice.
fn fallbacks(secret: &[u8]) -> Vec<usize> {
    let mut table = vec![0_usize; secret.len()];
    let mut length = 0_usize;
    for at in 1..secret.len() {
        while length > 0 && secret.get(at) != secret.get(length) {
            length = table.get(length.saturating_sub(1)).copied().unwrap_or(0);
        }
        if secret.get(at) == secret.get(length) {
            length = length.saturating_add(1);
        }
        if let Some(slot) = table.get_mut(at) {
            *slot = length;
        }
    }
    table
}
