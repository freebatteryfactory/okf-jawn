//! The bearer token of an operation and the connector credential of the relay.
//!
//! A credential comes from the environment or, for the relay, from the first line of a file
//! (UTF-8 or ASCII, with or without a byte order mark; at most 8192 bytes, and a longer line
//! is refused, not cut). From either place it is printable ASCII without spaces and at least
//! 16 characters long; anything else is refused before the CLI does anything with it. The
//! relay's connector credential is also refused unless it is spelled in the alphabet the
//! service issues a connector secret in, lowercase letters, digits and the underscore
//! ([`Credential::connector`]): so it never holds `[` or `]`, and the mark that replaces it in
//! what the relay writes cannot complete it. An operation's bearer token is refused unless it
//! is an RFC 6750 `b64token` ([`Credential::operation`]): letters, digits and `-` `.` `_` `~`
//! `+` `/`, then any number of `=`. That alphabet holds a hosted service's WorkOS Connect
//! access token (base64url and dots) and every connector secret, and no `[` or `]`, so the
//! mark cannot complete a token in a refusal the CLI prints either.
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
//!   does, cannot be both clean and still JSON: it is refused whole. The line is cleaned in
//!   the buffer it stands in, in one pass that keeps no record of what it found: the line
//!   only becomes shorter, so cleaning needs no memory beyond the line itself.
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

/// Walks a JSON line unit by unit, decoding the escapes inside its strings. It holds its
/// place and not the line, so the line can be rewritten behind it.
#[derive(Debug, Default)]
struct Units {
    at: usize,
    inside: bool,
}

/// The UTF-8 byte order mark a credential file may begin with.
const BYTE_ORDER_MARK: &[u8] = b"\xEF\xBB\xBF";

/// The longest first line of a credential file that is taken as a credential, in bytes.
const MAX_CREDENTIAL_BYTES: u64 = 8192;

/// Most bytes ever read of a credential file: the longest line, a byte order mark, a carriage
/// return and a line feed, and one byte more, which tells a longer line from the longest.
const MAX_FIRST_LINE_BYTES: u64 = MAX_CREDENTIAL_BYTES + 6;

/// The shortest text that is taken as a credential, in characters. A credential the service
/// issues is far longer. A shorter one would be a wrong setting, and keeping it out of what
/// the CLI writes would then rewrite ordinary text, or withhold every line: a credential of
/// one letter is in every answer.
const MIN_CREDENTIAL_BYTES: usize = 16;

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

    /// This credential as the relay's connector credential: refused unless it is spelled in the
    /// alphabet the service issues a connector secret in (`in_connector_alphabet`). `source`
    /// names where it came from, for the refusal.
    ///
    /// # Errors
    /// Returns `InvalidInput` when a character is outside that alphabet; the credential itself
    /// is never repeated.
    pub fn connector(self, source: &str) -> Result<Self, ApiError> {
        if self.0.bytes().all(in_connector_alphabet) {
            return Ok(self);
        }
        Err(ApiError::new(
            ErrorCode::InvalidInput,
            format!(
                "The connector credential in {source} is not usable: a connector credential the \
                 okf-jawn service issues holds only lowercase letters, digits and the underscore. \
                 Copy the whole credential as the service gave it"
            ),
        ))
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
    /// Returns `InvalidInput` when the file cannot be read, and for each reason
    /// [`Credential::from_reader`] gives. The line itself is never repeated.
    pub fn from_file(path: &Path) -> Result<Self, ApiError> {
        let file = std::fs::File::open(path).map_err(|error| unreadable(path, &error))?;
        Self::from_reader(file, path)
    }

    /// Read the credential on the first line of `text`; `path` names the text in a refusal.
    ///
    /// The line ends at the first line feed, and a carriage return before it is not part of
    /// it. A UTF-8 byte order mark at the start is skipped. A line longer than 8192 bytes is
    /// refused, not cut. No more than that, a mark and a line ending are ever read, however
    /// long the text is.
    ///
    /// # Errors
    /// Returns `InvalidInput` when the text cannot be read, is UTF-16, has a first line that
    /// is too long, empty or not UTF-8, or when that line is not a usable credential. The
    /// line itself is never repeated.
    pub fn from_reader<R: Read>(text: R, path: &Path) -> Result<Self, ApiError> {
        let refused = |what: String| ApiError::new(ErrorCode::InvalidInput, what);
        let file = format!("the credential file {}", path.display());
        let mut first = Vec::new();
        BufReader::new(text.take(MAX_FIRST_LINE_BYTES))
            .read_until(b'\n', &mut first)
            .map_err(|error| unreadable(path, &error))?;
        // UTF-16 carries a mark of its own, or shows as a NUL beside its first character.
        if first.starts_with(&[0xFF, 0xFE])
            || first.starts_with(&[0xFE, 0xFF])
            || first.iter().take(2).any(|byte| *byte == 0)
        {
            return Err(refused(format!(
                "The text of {file} is UTF-16, which Windows PowerShell writes with > and with \
                 Out-File. Write the file as UTF-8 or ASCII, for example in PowerShell with \
                 Set-Content -Encoding ascii"
            )));
        }
        let line = first.strip_prefix(BYTE_ORDER_MARK).unwrap_or(&first);
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if u64::try_from(line.len()).unwrap_or(u64::MAX) > MAX_CREDENTIAL_BYTES {
            return Err(refused(format!(
                "The first line of {file} is longer than {MAX_CREDENTIAL_BYTES} bytes, which no \
                 connector credential is"
            )));
        }
        let line = std::str::from_utf8(line)
            .map_err(|_| refused(format!("The first line of {file} is not text")))?;
        Self::parse(line, "--token-file")?
            .ok_or_else(|| refused(format!("The first line of {file} is empty")))
    }

    /// This credential as an operation's bearer token: refused unless it is an RFC 6750
    /// `b64token` (`is_b64token`). `source` names where it came from, for the refusal.
    ///
    /// # Errors
    /// Returns `InvalidInput` when it is not; the token itself is never repeated.
    pub fn operation(self, source: &str) -> Result<Self, ApiError> {
        if is_b64token(&self.0) {
            return Ok(self);
        }
        Err(ApiError::new(
            ErrorCode::InvalidInput,
            format!(
                "The bearer token in {source} is not usable: a bearer token holds only letters, \
                 digits and - . _ ~ + /, and may end in =. Copy the whole token as it was issued"
            ),
        ))
    }

    /// Take a credential from raw text: surrounding whitespace is dropped, and blank is none.
    ///
    /// `source` names where the text came from, for the refusal.
    ///
    /// # Errors
    /// Returns `InvalidInput` when the text holds a byte that an HTTP header value cannot
    /// carry or a space, and when it is shorter than 16 characters; the text itself is never
    /// repeated.
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
        if secret.len() < MIN_CREDENTIAL_BYTES {
            return Err(ApiError::new(
                ErrorCode::InvalidInput,
                format!(
                    "The credential in {source} is not usable: it is shorter than \
                     {MIN_CREDENTIAL_BYTES} characters, and a credential the okf-jawn service \
                     issues is far longer. Copy the whole credential"
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
    /// The line is cleaned in the buffer it came in, which is returned: each unit is moved
    /// back over what was removed before it, and a spelling of the secret is overwritten by
    /// the mark. The mark always has its place: it is 12 bytes, and a secret is at least 16
    /// characters, each spelled with a byte or more.
    ///
    /// Returns `None` when the line spells the secret outside a string or across two of them:
    /// replacing it there would leave no JSON, so the line must not be written at all.
    #[must_use]
    pub fn redact_line(&self, line: String) -> Option<String> {
        let secret = self.0.as_bytes();
        let fallback = fallbacks(secret);
        let mut matched = 0_usize;
        let mut run = 0_usize;
        // Where, in the clean text, each of the last units starts: as many as the secret is long.
        let mut starts = VecDeque::with_capacity(secret.len().saturating_add(1));
        let mut bytes = line.into_bytes();
        let mut units = Units::default();
        // Where the clean text ends so far; never past the unit being read.
        let mut clean = 0_usize;
        while let Some(unit) = units.next(&bytes) {
            run = if unit.inside {
                run.saturating_add(1)
            } else {
                0
            };
            starts.push_back(clean);
            if starts.len() > secret.len() {
                starts.pop_front();
            }
            if clean != unit.start {
                bytes.copy_within(unit.start..unit.end, clean);
            }
            clean = clean.saturating_add(unit.end.saturating_sub(unit.start));
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
                let from = starts.front().copied()?;
                bytes
                    .get_mut(from..clean)?
                    .get_mut(..REDACTED.len())?
                    .copy_from_slice(REDACTED.as_bytes());
                clean = from.saturating_add(REDACTED.len());
                starts.clear();
                matched = 0;
            }
        }
        bytes.truncate(clean);
        // Whole units were moved or replaced by ASCII, so the text is UTF-8 as it was.
        String::from_utf8(bytes).ok()
    }
}

impl Debug for Credential {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Credential([credential])")
    }
}

impl Units {
    /// The next unit of `bytes`, which is the line from the place this has reached on.
    fn next(&mut self, bytes: &[u8]) -> Option<Unit> {
        let start = self.at;
        let first = *bytes.get(start)?;
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
                let (byte, length) = escape(bytes.get(start.saturating_add(1)..));
                (byte, length, true)
            }
            (true, _) => (first, 1, true),
        };
        // An escape the line cuts short ends where the line does.
        self.at = start.saturating_add(length).min(bytes.len());
        Some(Unit {
            start,
            end: self.at,
            byte,
            inside,
        })
    }
}

/// The refusal for a credential file that cannot be read.
fn unreadable(path: &Path, error: &std::io::Error) -> ApiError {
    ApiError::new(
        ErrorCode::InvalidInput,
        format!(
            "The credential file {} cannot be read: {error}",
            path.display()
        ),
    )
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

/// Whether `byte` is in the alphabet a connector secret is spelled in. The service issues
/// `okfj_` and 64 lowercase hexadecimal digits (32 random bytes); the alphabet is the
/// lowercase letters, the digits and the underscore. The mark that replaces a credential,
/// `[credential]`, begins and ends with a bracket, so it can never complete one.
fn in_connector_alphabet(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
}

/// Whether `text` is a `b64token` of RFC 6750 section 2.1, the syntax of a bearer token:
/// `1*( ALPHA / DIGIT / "-" / "." / "_" / "~" / "+" / "/" ) *"="`. A WorkOS Connect access
/// token (base64url and dots) and a connector secret are; `[` and `]`, from which the mark
/// `[credential]` could complete a token, are not.
fn is_b64token(text: &str) -> bool {
    let body = text.trim_end_matches('=');
    !body.is_empty()
        && body.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'+' | b'/')
        })
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
