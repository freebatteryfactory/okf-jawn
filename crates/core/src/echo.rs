//! The bound on caller or stored text that an error message quotes.
//!
//! Validator and decoder texts quote the offending value. Unbounded, a request or a stored
//! value of megabytes comes back whole in its own refusal, and from there into logs.

/// Most bytes of validator or decoder text an error message carries.
pub const ECHO_LIMIT: usize = 512;

/// Keep at most [`ECHO_LIMIT`] bytes of `text`, cut on a character boundary and marked with `…`.
#[must_use]
pub fn bounded(mut text: String) -> String {
    if text.len() <= ECHO_LIMIT {
        return text;
    }
    text.truncate(text.floor_char_boundary(ECHO_LIMIT));
    text.push('…');
    text
}
