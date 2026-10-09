//! Text a PDF font cannot map to Unicode is flagged, never indexed as words.
//!
//! When a simple font has no `ToUnicode` map and its `/Encoding /Differences` names a code
//! with a synthetic glyph name, docling-pdf prints `/` and that name instead of a character.
//! The rule restated here is `qualification/docling/src/glyphs.rs`, itself a restatement of
//! docling-pdf `textparse.rs` `is_gid_name`. It must be read again on every Docling version
//! change.
//!
//! The words those glyphs draw are not in the file. So each placeholder is counted on the page
//! of the item that holds it, and that page is reported as partly extracted. In the Markdown
//! the card shows and search indexes, each placeholder is replaced by U+FFFD REPLACEMENT
//! CHARACTER. U+FFFD is the standard mark for a character that could not be decoded. It is
//! not a letter or a digit, so no tokenizer reads it as part of a word. The retained docling
//! export keeps the library's own text unchanged.
//!
//! The detector has two known limits, and the tests hold them as they are:
//!
//! - It flags real text of the placeholder's shape that stands after a space or starts the
//!   text: `/B747`, `/v100`, `/tmp123`.
//! - It misses a placeholder glued to a preceding character: `x/g12`.
//!
//! A signal from the library that a glyph had no Unicode is preferred to this detector as soon
//! as the library gives one.

use std::collections::BTreeMap;

use okf_jawn_contract::extraction::{ExtractionWarning, PageGlyphs};
use serde_json::Value;

use crate::export::body_items;

/// The placeholders found in one converted document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UndecodedGlyphs {
    /// Placeholders per one-based page, for the pages that have any.
    pub pages: BTreeMap<u32, u32>,
    /// Placeholders in the whole document.
    pub tokens: u32,
    /// Placeholders in items that have no page, counted in `tokens` and in no page.
    pub unlocated_tokens: u32,
}

/// The rule, in words.
pub const GLYPH_RULE: &str = "a placeholder is `/` followed by a glyph name the library prints because the font has no Unicode for it (docling-pdf textparse.rs is_gid_name: g, G, cid, CID, glyph or index and digits, or 1 to 3 letters and at least 3 digits; never afii... or uni...), where the slash starts the text, follows whitespace or follows another placeholder; counted in the text of every text item and of every table cell, on the page the item is located on. Two known limits: real text of that shape standing after a space (/B747) is counted too, and a placeholder glued to a preceding character (x/g12) is not";

/// What stands in the shown Markdown in place of each placeholder.
pub const UNDECODED: char = '\u{FFFD}';

/// The prefixes after which any run of digits is a placeholder name.
const GID_PREFIXES: [&str; 6] = ["g", "G", "cid", "CID", "glyph", "index"];

impl UndecodedGlyphs {
    /// The warning a conversion records, or `None` when the document has no placeholder.
    #[must_use]
    pub fn warning(&self) -> Option<ExtractionWarning> {
        (self.tokens > 0).then(|| ExtractionWarning::UndecodableGlyphs {
            pages: self
                .pages
                .iter()
                .map(|(page_no, glyphs)| PageGlyphs {
                    page_no: *page_no,
                    glyphs: *glyphs,
                })
                .collect(),
            unlocated_glyphs: self.unlocated_tokens,
        })
    }
}

/// True for a glyph name the library prints as it is because it carries no Unicode meaning:
/// docling-pdf `textparse.rs` `is_gid_name`, restated.
#[must_use]
pub fn is_placeholder_glyph_name(name: &str) -> bool {
    if name.starts_with("afii") || name.starts_with("uni") {
        return false;
    }
    let digits_only =
        |rest: &str| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit());
    if GID_PREFIXES
        .iter()
        .any(|prefix| name.strip_prefix(prefix).is_some_and(digits_only))
    {
        return true;
    }
    let letters = name.bytes().take_while(u8::is_ascii_alphabetic).count();
    let digits = name.len().saturating_sub(letters);
    (1..=3).contains(&letters)
        && digits >= 3
        && name.bytes().skip(letters).all(|byte| byte.is_ascii_digit())
}

/// The placeholders in `text`, in order.
///
/// A placeholder may start where the text starts, after whitespace, or directly after another
/// placeholder (the library prints one per glyph with nothing between: `/g40/g81`). Its name is
/// the run of ASCII letters then digits after the slash, and must satisfy
/// [`is_placeholder_glyph_name`]. A slash anywhere else (`and/or`, `/usr/bin`, a URL, `1/2`)
/// starts none.
#[must_use]
pub fn placeholder_glyph_tokens(text: &str) -> Vec<&str> {
    split_placeholders(text)
        .into_iter()
        .filter_map(|(piece, placeholder)| placeholder.then_some(piece))
        .collect()
}

/// `text` with every placeholder replaced by [`UNDECODED`], and how many were replaced.
#[must_use]
pub fn scrub_placeholders(text: &str) -> (String, u32) {
    let mut scrubbed = String::with_capacity(text.len());
    let mut replaced = 0_u32;
    for (piece, placeholder) in split_placeholders(text) {
        if placeholder {
            scrubbed.push(UNDECODED);
            replaced = replaced.saturating_add(1);
        } else {
            scrubbed.push_str(piece);
        }
    }
    (scrubbed, replaced)
}

/// Count the placeholders of a converted document by page.
///
/// `export` is the document's docling JSON export. `page_of` gives the page each item
/// (`self_ref`) is located on, whichever source located it. An item without a page still
/// counts, under `unlocated_tokens`.
pub fn undecoded_glyphs(export: &Value, page_of: impl Fn(&str) -> Option<u32>) -> UndecodedGlyphs {
    let mut found = UndecodedGlyphs::default();
    for (kind, item) in body_items(export) {
        let tokens = match kind {
            crate::export::ItemKind::Text => item
                .get("text")
                .and_then(Value::as_str)
                .map_or(0, |text| placeholder_glyph_tokens(text).len()),
            crate::export::ItemKind::Table => table_tokens(item),
            crate::export::ItemKind::Picture => 0,
        };
        let tokens = u32::try_from(tokens).unwrap_or(u32::MAX);
        if tokens == 0 {
            continue;
        }
        let reference = item
            .get("self_ref")
            .and_then(Value::as_str)
            .unwrap_or_default();
        found.tokens = found.tokens.saturating_add(tokens);
        match page_of(reference) {
            Some(page) => {
                let count = found.pages.entry(page).or_insert(0);
                *count = count.saturating_add(tokens);
            }
            None => found.unlocated_tokens = found.unlocated_tokens.saturating_add(tokens),
        }
    }
    found
}

/// The placeholder `text` starts with and what follows it, when it starts with one.
fn leading_placeholder(text: &str) -> Option<(&str, &str)> {
    let name_and_tail = text.strip_prefix('/')?;
    let letters = name_and_tail
        .bytes()
        .take_while(u8::is_ascii_alphabetic)
        .count();
    let digits = name_and_tail
        .bytes()
        .skip(letters)
        .take_while(u8::is_ascii_digit)
        .count();
    let length = letters.saturating_add(digits);
    let (name, _) = name_and_tail.split_at_checked(length)?;
    if !is_placeholder_glyph_name(name) {
        return None;
    }
    text.split_at_checked(length.saturating_add(1))
}

/// `text` cut into pieces, each marked `true` when it is one placeholder; the pieces joined
/// are `text`.
fn split_placeholders(text: &str) -> Vec<(&str, bool)> {
    let mut pieces = Vec::new();
    let mut rest = text;
    let mut plain_start = text;
    let mut plain_len = 0_usize;
    let mut may_start = true;
    while let Some(first) = rest.chars().next() {
        if may_start && let Some((token, tail)) = leading_placeholder(rest) {
            if let Some((plain, _)) = plain_start.split_at_checked(plain_len)
                && !plain.is_empty()
            {
                pieces.push((plain, false));
            }
            pieces.push((token, true));
            rest = tail;
            plain_start = tail;
            plain_len = 0;
            continue;
        }
        may_start = first.is_whitespace();
        plain_len = plain_len.saturating_add(first.len_utf8());
        let mut after = rest.chars();
        let _skipped = after.next();
        rest = after.as_str();
    }
    if let Some((plain, _)) = plain_start.split_at_checked(plain_len)
        && !plain.is_empty()
    {
        pieces.push((plain, false));
    }
    pieces
}

/// The placeholders in the texts of a table's cells, each cell counted once.
fn table_tokens(table: &Value) -> usize {
    table
        .get("data")
        .and_then(|data| data.get("table_cells"))
        .and_then(Value::as_array)
        .map_or(0, |cells| {
            cells
                .iter()
                .filter_map(|cell| cell.get("text").and_then(Value::as_str))
                .map(|text| placeholder_glyph_tokens(text).len())
                .sum()
        })
}
