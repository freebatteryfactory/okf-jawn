//! Detect the library's placeholders for glyphs a PDF font gives no Unicode for.
//!
//! When a simple font has no `ToUnicode` map and its `/Encoding /Differences` names a code
//! with a synthetic glyph name, docling-pdf prints `/` + that name instead of a character
//! (docling-pdf 1.93.6 `textparse.rs`, `Font::decode_code`, lines 118-119:
//! `if let Some(name) = self.fallback_names.get(&(code as u8)) { return (Some(format!("/{name}")), w); }`).
//! A name is kept as such a fallback only when `is_gid_name` accepts it (lines 354-376):
//! `g`, `G`, `cid`, `CID`, `glyph` or `index` followed by digits, or one to three ASCII letters
//! followed by at least three digits (`SM590000`), and never a name starting `afii` or `uni`.
//!
//! The words those glyphs draw are not in the file's text layer at all, so nothing decodes
//! them; they can only be detected. This module restates the library's rule and counts the
//! placeholders in a converted document, by page. It must be read again on every library
//! version change.
//!
//! What the detector cannot do: tell a placeholder from real text of the same shape standing
//! alone (`/A380`), or see a placeholder glued to a preceding decoded character (`x/g12`).

use crate::locate::{ItemLocation, body_items};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// One item of the converted document that holds placeholders.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct GlyphItem {
    /// The `self_ref` of the text item, or of the table whose cells hold the placeholders.
    item: String,
    /// `text` or `table`.
    kind: &'static str,
    /// The page the item is located on; null when it has no location.
    page_no: Option<u64>,
    /// How many placeholders the item holds.
    tokens: usize,
}

/// The placeholders found in one converted PDF.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct UndecodedGlyphs {
    /// Every item that holds a placeholder, in document order.
    items: Vec<GlyphItem>,
    /// Placeholders per page, for the pages that have any.
    pages: BTreeMap<u64, usize>,
    /// The rule in words.
    rule: &'static str,
    /// Placeholders in the whole document.
    tokens: usize,
    /// Placeholders in items that have no page, counted in `tokens` and in no page.
    unlocated_tokens: usize,
}

/// The prefixes after which any run of digits is a placeholder name.
const GID_PREFIXES: [&str; 6] = ["g", "G", "cid", "CID", "glyph", "index"];

/// The rule, as the receipt states it.
pub(crate) const GLYPH_RULE: &str = "a placeholder is `/` followed by a glyph name the library prints because the font has no Unicode for it (docling-pdf textparse.rs is_gid_name: g, G, cid, CID, glyph or index and digits, or 1 to 3 letters and at least 3 digits; never afii... or uni...), where the slash starts the text, follows whitespace or follows another placeholder; counted in the text of every text item and of every table cell, on the page the item is located on";

/// True for a glyph name the library prints verbatim because it carries no Unicode meaning:
/// docling-pdf 1.93.6 `textparse.rs` `is_gid_name`, restated.
pub(crate) fn is_placeholder_glyph_name(name: &str) -> bool {
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
pub(crate) fn placeholder_glyph_tokens(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    let mut may_start = true;
    while let Some(first) = rest.chars().next() {
        if may_start && let Some((token, tail)) = leading_placeholder(rest) {
            found.push(token);
            rest = tail;
            continue;
        }
        may_start = first.is_whitespace();
        let mut after = rest.chars();
        let _skipped = after.next();
        rest = after.as_str();
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

/// Count the placeholders of a converted document by page.
///
/// `export` is the document's JSON export; `locations` says which page each item is on,
/// whichever source located it. An item without a page still counts, under `unlocated_tokens`.
pub(crate) fn undecoded_glyphs(export: &Value, locations: &[ItemLocation]) -> UndecodedGlyphs {
    let page_of = |item: &str| {
        locations
            .iter()
            .find(|location| location.item == item)
            .and_then(|location| location.page_no)
    };
    let mut items = Vec::new();
    let mut pages: BTreeMap<u64, usize> = BTreeMap::new();
    let mut total = 0_usize;
    let mut unlocated = 0_usize;
    for (kind, item) in body_items(export) {
        let tokens = match kind {
            "text" => item
                .get("text")
                .and_then(Value::as_str)
                .map_or(0, |text| placeholder_glyph_tokens(text).len()),
            "table" => table_tokens(item),
            _ => 0,
        };
        if tokens == 0 {
            continue;
        }
        let reference = item
            .get("self_ref")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let page_no = page_of(&reference);
        total = total.saturating_add(tokens);
        match page_no {
            Some(page) => {
                let count = pages.entry(page).or_insert(0);
                *count = count.saturating_add(tokens);
            }
            None => unlocated = unlocated.saturating_add(tokens),
        }
        items.push(GlyphItem {
            item: reference,
            kind,
            page_no,
            tokens,
        });
    }
    UndecodedGlyphs {
        items,
        pages,
        rule: GLYPH_RULE,
        tokens: total,
        unlocated_tokens: unlocated,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_placeholder_glyph_name, placeholder_glyph_tokens, undecoded_glyphs};
    use crate::locate::locate_items;
    use serde_json::json;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The value of an option that must be present; `what` names it in the failure.
    fn some<T>(value: Option<T>, what: &str) -> Result<T, String> {
        value.ok_or_else(|| format!("missing {what}"))
    }

    #[test]
    fn a_placeholder_name_is_what_the_library_rule_accepts() {
        for name in [
            "g115", "g3", "G12", "cid42", "CID7", "glyph7", "index9", "SM590000", "a123", "abc1234",
        ] {
            assert!(is_placeholder_glyph_name(name), "{name}");
        }
        for name in [
            "",
            "g",
            "glyph",
            "space",
            "lambda",
            "A",
            "a12",
            "abcd123",
            "afii10017",
            "uni0041",
            "uni041",
            "SM59000x",
            "12345",
            "g12a",
            "bullet",
            "f_i",
            "a.sc",
        ] {
            assert!(!is_placeholder_glyph_name(name), "{name}");
        }
    }

    #[test]
    fn the_two_shapes_the_library_prints_are_found() {
        assert_eq!(
            placeholder_glyph_tokens("/SM590000 Work Function Usage ( WRKFCNUSG )"),
            ["/SM590000"]
        );
        // One placeholder per glyph, with nothing between them; a space glyph is one too.
        assert_eq!(
            placeholder_glyph_tokens("/g115/g3 /g40/g81/g75"),
            ["/g115", "/g3", "/g40", "/g81", "/g75"]
        );
        assert_eq!(
            placeholder_glyph_tokens("Usage setting: /SM590000 ALLOWED: yes /SM590000 DENIED: no"),
            ["/SM590000", "/SM590000"]
        );
        // A decoded character directly after a placeholder ends it; the placeholder still counts.
        assert_eq!(placeholder_glyph_tokens("/g115A"), ["/g115"]);
        assert_eq!(
            placeholder_glyph_tokens("\t/cid42\n/glyph7"),
            ["/cid42", "/glyph7"]
        );
        assert_eq!(placeholder_glyph_tokens(""), [""; 0]);
    }

    #[test]
    fn ordinary_text_with_slashes_is_not_flagged() {
        for text in [
            "/usr/bin",
            "and/or",
            "see https://example.com/g12/v100/index9 for more",
            "1/2",
            "3/4 of 5/6",
            "c:/g12/x",
            "TCP/IP",
            "/ g115",
            "/gamma",
            "/uni0041",
            "/afii10017",
            "x/g12",
            "//g12",
            "(/SM590000)",
            "/a12",
            "a / b",
        ] {
            assert_eq!(placeholder_glyph_tokens(text), [""; 0], "{text}");
        }
    }

    #[test]
    fn placeholders_are_counted_by_page_in_text_items_and_table_cells() -> TestResult {
        let prov = |page: u64| json!([{ "page_no": page, "bbox": { "l": 10.0, "t": 50.0, "r": 90.0, "b": 40.0, "coord_origin": "BOTTOMLEFT" } }]);
        let export = json!({
            "pages": { "3": { "page_no": 3, "size": { "width": 612.0, "height": 792.0 } }, "8": { "page_no": 8, "size": { "width": 612.0, "height": 792.0 } } },
            "body": { "self_ref": "#/body", "children": [] },
            "texts": [
                { "self_ref": "#/texts/0", "label": "list_item", "text": "/g115/g3 /g40/g81", "prov": prov(3) },
                { "self_ref": "#/texts/1", "label": "text", "text": "plain words and/or a path /usr/bin", "prov": prov(3) },
                { "self_ref": "#/texts/2", "label": "list_item", "text": "/SM590000 A bullet", "prov": prov(8) },
                { "self_ref": "#/texts/3", "label": "caption", "text": "/SM590000 without a page", "prov": [], "parent": { "$ref": "#/body" } },
            ],
            "tables": [{
                "self_ref": "#/tables/0", "label": "table", "prov": prov(8),
                "data": {
                    "table_cells": [
                        { "text": "Usage setting: /SM590000 ALLOWED /SM590000 DENIED" },
                        { "text": "no placeholder here" },
                        { "text": "/SM590000 USER" },
                    ],
                    // The grid repeats a spanning cell; it is not what is counted.
                    "grid": [[{ "text": "/SM590000 USER" }, { "text": "/SM590000 USER" }]],
                },
            }],
            "pictures": [],
        });
        let locations = locate_items(&export, Err("no text layer was read"));
        let report = undecoded_glyphs(&export, &locations.items);
        assert_eq!(report.tokens, 9);
        assert_eq!(report.unlocated_tokens, 1);
        assert_eq!(
            report
                .pages
                .iter()
                .map(|(page, count)| (*page, *count))
                .collect::<Vec<_>>(),
            [(3, 4), (8, 4)]
        );
        // A placeholder inside a table cell is counted, on the table's page.
        let table = some(
            report.items.iter().find(|item| item.kind == "table"),
            "the table",
        )?;
        assert_eq!(
            (table.item.as_str(), table.page_no, table.tokens),
            ("#/tables/0", Some(8), 3)
        );
        assert_eq!(
            report
                .items
                .iter()
                .map(|item| item.item.as_str())
                .collect::<Vec<_>>(),
            ["#/texts/0", "#/texts/2", "#/texts/3", "#/tables/0"]
        );
        // A document without placeholders reports none, on no page.
        let clean = json!({ "texts": [{ "self_ref": "#/texts/0", "label": "text", "text": "and/or 1/2", "prov": prov(3) }] });
        let none = undecoded_glyphs(&clean, &locate_items(&clean, Err("unread")).items);
        assert_eq!(
            (
                none.tokens,
                none.unlocated_tokens,
                none.pages.len(),
                none.items.len()
            ),
            (0, 0, 0, 0)
        );
        Ok(())
    }
}
