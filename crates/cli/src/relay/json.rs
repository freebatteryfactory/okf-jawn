//! Reading a JSON text without writing it again.
//!
//! An answer of the service is passed on as the service wrote it, so nothing here builds a
//! value and encodes it. [`is_json`] asks `serde_json` whether a text is one JSON value
//! without reading its numbers or strings into anything, so a number of any size, a
//! duplicate key and any escape pass, as they do in the grammar. [`members`] then finds the
//! members of a text already known to be a JSON object, and [`one_line`] removes its line
//! breaks.
//!
//! Invariants:
//! - [`members`] and [`one_line`] are only for a text [`is_json`] accepted. In such a text a
//!   raw line feed or carriage return stands between tokens and never inside a string (a
//!   string holds them only as escapes), which is why removing them changes no value.
//! - When a key occurs twice the last member is the one that counts, as it is for the JSON
//!   parsers a client is likely to use; [`last`] finds it.

use serde::de::IgnoredAny;

/// One member of a JSON object, as it stands in the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Member<'text> {
    /// The key, decoded; `None` when it holds an escape that stands for no character.
    pub(super) key: Option<String>,
    /// The value as written.
    pub(super) value: &'text str,
    /// Where the value starts in the text.
    pub(super) start: usize,
    /// Where the value ends in the text.
    pub(super) end: usize,
}

/// Whether `text` is exactly one JSON value.
pub(super) fn is_json(text: &str) -> bool {
    serde_json::from_str::<IgnoredAny>(text).is_ok()
}

/// The member that counts for `key`: the last one with that key.
pub(super) fn last<'members, 'text>(
    members: &'members [Member<'text>],
    key: &str,
) -> Option<&'members Member<'text>> {
    members
        .iter()
        .rev()
        .find(|member| member.key.as_deref() == Some(key))
}

/// The members of the JSON object `text`, in order; `None` when `text` is no object.
pub(super) fn members(text: &str) -> Option<Vec<Member<'_>>> {
    let bytes = text.as_bytes();
    let opening = skip_space(bytes, 0);
    if bytes.get(opening) != Some(&b'{') {
        return None;
    }
    let mut at = skip_space(bytes, opening.saturating_add(1));
    let mut found = Vec::new();
    if bytes.get(at) == Some(&b'}') {
        return Some(found);
    }
    loop {
        let key_end = string_end(bytes, at)?;
        let key = serde_json::from_str::<String>(text.get(at..key_end)?).ok();
        let colon = skip_space(bytes, key_end);
        if bytes.get(colon) != Some(&b':') {
            return None;
        }
        let start = skip_space(bytes, colon.saturating_add(1));
        let end = value_end(bytes, start)?;
        found.push(Member {
            key,
            value: text.get(start..end)?,
            start,
            end,
        });
        let next = skip_space(bytes, end);
        match bytes.get(next) {
            Some(b',') => at = skip_space(bytes, next.saturating_add(1)),
            Some(b'}') => return Some(found),
            _ => return None,
        }
    }
}

/// `text` without its line feeds and carriage returns, so that it is one line.
pub(super) fn one_line(mut text: String) -> String {
    text.retain(|character| character != '\n' && character != '\r');
    text
}

/// The first position at or after `at` that holds no JSON white space.
fn skip_space(bytes: &[u8], mut at: usize) -> usize {
    while matches!(bytes.get(at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        at = at.saturating_add(1);
    }
    at
}

/// Where the string that opens at `at` ends: the position after its closing quote.
fn string_end(bytes: &[u8], at: usize) -> Option<usize> {
    if bytes.get(at) != Some(&b'"') {
        return None;
    }
    let mut next = at.saturating_add(1);
    loop {
        match bytes.get(next)? {
            b'"' => return Some(next.saturating_add(1)),
            // An escape: whatever follows the backslash is part of it.
            b'\\' => next = next.saturating_add(2),
            _ => next = next.saturating_add(1),
        }
    }
}

/// Where the value that starts at `start` ends.
fn value_end(bytes: &[u8], start: usize) -> Option<usize> {
    match bytes.get(start)? {
        b'"' => string_end(bytes, start),
        b'{' | b'[' => {
            let mut depth = 0_usize;
            let mut at = start;
            loop {
                match bytes.get(at)? {
                    b'"' => {
                        at = string_end(bytes, at)?;
                        continue;
                    }
                    b'{' | b'[' => depth = depth.saturating_add(1),
                    b'}' | b']' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            return Some(at.saturating_add(1));
                        }
                    }
                    _ => {}
                }
                at = at.saturating_add(1);
            }
        }
        _ => {
            let mut at = start;
            while bytes.get(at).is_some_and(|byte| {
                !matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
            }) {
                at = at.saturating_add(1);
            }
            (at > start).then_some(at)
        }
    }
}

#[cfg(test)]
mod mcp_stdio {
    use super::{is_json, last, members, one_line};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn some<T>(value: Option<T>, what: &str) -> Result<T, String> {
        value.ok_or_else(|| format!("expected {what} to be present"))
    }

    /// The members are found as written, whatever stands inside their values: a string that
    /// holds a brace, a quote or a backslash, nested objects and arrays, numbers of any size.
    #[test]
    fn members_are_found_as_written() -> TestResult {
        let text = concat!(
            " {\n \"a\" : \"}\\\"]\\\\\" ,\"b\":{\"x\":[1,{\"y\":\"]\"}],\"z\":\"\\\\\"},",
            "\"c\":[\"[\",\"\\\"\"],\"n\":-1.5e400,\"t\":true,\"b\":null}\r\n"
        );
        assert!(is_json(text));
        let found = some(members(text), "an object")?;
        let written: Vec<(Option<&str>, &str)> = found
            .iter()
            .map(|member| (member.key.as_deref(), member.value))
            .collect();
        assert_eq!(
            written,
            [
                (Some("a"), "\"}\\\"]\\\\\""),
                (Some("b"), "{\"x\":[1,{\"y\":\"]\"}],\"z\":\"\\\\\"}"),
                (Some("c"), "[\"[\",\"\\\"\"]"),
                (Some("n"), "-1.5e400"),
                (Some("t"), "true"),
                (Some("b"), "null"),
            ]
        );
        for member in &found {
            assert_eq!(text.get(member.start..member.end), Some(member.value));
        }
        // A key that occurs twice: the last one counts.
        assert_eq!(last(&found, "b").map(|member| member.value), Some("null"));
        assert_eq!(last(&found, "missing"), None);
        Ok(())
    }

    #[test]
    fn only_an_object_has_members() {
        assert_eq!(members("{}").map(|found| found.len()), Some(0));
        assert_eq!(members(" { } ").map(|found| found.len()), Some(0));
        for text in ["[1,2]", "\"{}\"", "12", "null", ""] {
            assert_eq!(members(text), None, "{text}");
        }
    }

    /// One JSON value and nothing else; the grammar decides, not what a number type can hold.
    #[test]
    fn one_json_value_is_told_from_everything_else() {
        for text in [
            "{\"n\":12345678901234567890123}",
            "{\"n\":1e400,\"m\":-0,\"k\":1E2}",
            "{\"a\":1,\"a\":2}",
            " [1, 2]\n",
            "\"a\\nb\"",
        ] {
            assert!(is_json(text), "{text}");
        }
        for text in [
            "",
            "{} {}",
            "{\"a\":1}x",
            "{\"a\":01}",
            "{'a':1}",
            "{\"a\":\"raw\nline feed\"}",
            "{\"a\":\"bad \\x escape\"}",
            "not json",
        ] {
            assert!(!is_json(text), "{text}");
        }
    }

    /// Line breaks go; every other byte stays, an escaped line feed inside a string included.
    #[test]
    fn one_line_removes_only_the_line_breaks() {
        let pretty = "{\r\n  \"a\": \"x\\ny\",\n\t\"b\": [1,\n 2]\n}\n";
        assert_eq!(
            one_line(pretty.to_owned()),
            "{  \"a\": \"x\\ny\",\t\"b\": [1, 2]}"
        );
        assert_eq!(one_line("{\"a\":1}".to_owned()), "{\"a\":1}");
    }
}
