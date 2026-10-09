//! `ingest-flags-undecodable-text`: the library's placeholders for glyphs a font gives no
//! Unicode for are detected by the rule of `qualification/docling/src/glyphs.rs`, counted on
//! the page of the item that holds them, and replaced in the shown Markdown so no tokenizer
//! reads them as words.
//!
//! The last test reads the corpus PDF's text layer with `docling::pdf_text_layer_pages`, the
//! library's pure-Rust text path. It loads no model and no native library, so CI runs it.
#![cfg(feature = "runtime")]

#[path = "../../../tests/support/check.rs"]
mod check;

mod flags_undecodable_text {
    use std::collections::BTreeMap;

    use okf_jawn_contract::extraction::{ExtractionWarning, PageGlyphs};
    use okf_jawn_ingest::glyphs::{
        GLYPH_RULE, UNDECODED, is_placeholder_glyph_name, placeholder_glyph_tokens,
        scrub_placeholders, undecoded_glyphs,
    };
    use okf_jawn_ingest::locate::locate_items;
    use serde_json::{Value, json};

    use crate::check::{TestResult, some};

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
        assert_eq!(
            placeholder_glyph_tokens("/g115/g3 /g40/g81/g75"),
            ["/g115", "/g3", "/g40", "/g81", "/g75"]
        );
        assert_eq!(placeholder_glyph_tokens("/g115A"), ["/g115"]);
        assert_eq!(
            placeholder_glyph_tokens("\t/cid42\n/glyph7"),
            ["/cid42", "/glyph7"]
        );
        assert_eq!(placeholder_glyph_tokens(""), Vec::<&str>::new());
    }

    #[test]
    fn the_two_known_limits_of_the_detector_are_as_stated() {
        for (text, flagged) in [
            ("/A380", vec!["/A380"]),
            ("Boeing 747/A380 and /B747", vec!["/B747"]),
            ("see RFC /v100 and /G20 summit", vec!["/v100", "/G20"]),
            ("path /tmp123/file", vec!["/tmp123"]),
        ] {
            assert_eq!(placeholder_glyph_tokens(text), flagged, "{text}");
        }
        for text in ["x/g12", "(/SM590000)", "Usage/SM590000 next"] {
            assert!(placeholder_glyph_tokens(text).is_empty(), "{text}");
        }
        assert!(GLYPH_RULE.contains("Two known limits"));
    }

    #[test]
    fn ordinary_text_with_slashes_is_not_flagged() {
        for text in [
            "/usr/bin",
            "and/or",
            "see https://example.com/g12/v100/index9 for more",
            "1/2",
            "c:/g12/x",
            "TCP/IP",
            "/ g115",
            "/gamma",
            "/uni0041",
            "//g12",
            "/a12",
            "a / b",
        ] {
            assert!(placeholder_glyph_tokens(text).is_empty(), "{text}");
            assert_eq!(scrub_placeholders(text), (text.to_owned(), 0), "{text}");
        }
    }

    #[test]
    fn a_placeholder_is_never_shown_or_indexed_as_a_word() {
        let (shown, replaced) =
            scrub_placeholders("- /SM590000 A bullet\n| /g115/g3 /g40 | and/or é /B747 |\n");
        assert_eq!(replaced, 5);
        assert_eq!(
            shown,
            format!(
                "- {UNDECODED} A bullet\n| {UNDECODED}{UNDECODED} {UNDECODED} | and/or é {UNDECODED} |\n"
            )
        );
        // What a word tokenizer would read: no placeholder name survives as a word.
        let words: Vec<&str> = shown
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect();
        assert_eq!(words, ["A", "bullet", "and", "or", "é"]);
        assert!(!UNDECODED.is_alphanumeric());
    }

    #[test]
    fn placeholders_are_counted_by_page_in_text_items_and_table_cells() {
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
                    "grid": [[{ "text": "/SM590000 USER" }, { "text": "/SM590000 USER" }]],
                },
            }],
            "pictures": [],
        });
        let locations = locate_items(&export, Err("no text layer was read"));
        let found = undecoded_glyphs(&export, |item| locations.page_of(item));
        assert_eq!((found.tokens, found.unlocated_tokens), (9, 1));
        assert_eq!(found.pages, BTreeMap::from([(3, 4), (8, 4)]));
        assert_eq!(
            found.warning(),
            Some(ExtractionWarning::UndecodableGlyphs {
                pages: vec![
                    PageGlyphs {
                        page_no: 3,
                        glyphs: 4
                    },
                    PageGlyphs {
                        page_no: 8,
                        glyphs: 4
                    }
                ],
                unlocated_glyphs: 1
            })
        );
        // A document without placeholders reports none, and records no warning.
        let clean = json!({ "texts": [{ "self_ref": "#/texts/0", "label": "text", "text": "and/or 1/2", "prov": prov(3) }] });
        let none = undecoded_glyphs(&clean, |_| Some(3));
        assert_eq!(
            (none.tokens, none.pages.len(), none.warning()),
            (0, 0, None)
        );
    }

    #[test]
    fn the_corpus_pdf_shows_its_declared_placeholders_on_its_declared_pages() -> TestResult {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/documents");
        let bytes = std::fs::read(root.join("corpus/redp5110_sampled.pdf"))?;
        let layer = docling::pdf_text_layer_pages(&bytes, "redp5110_sampled.pdf", None)?
            .export_to_json_value();
        // The expectation SOURCES.json declares, counted from the file's font dictionaries.
        let sources: Value = serde_json::from_slice(&std::fs::read(root.join("SOURCES.json"))?)?;
        let fixture = some(
            sources
                .get("files")
                .and_then(|files| files.get("corpus/redp5110_sampled.pdf")),
            "the corpus PDF in SOURCES.json",
        )?;
        let declared = some(
            fixture.pointer("/expect/undecoded_glyphs/pages"),
            "the declared placeholder pages",
        )?;
        let declared: BTreeMap<u32, u32> = some(declared.as_object(), "a page map")?
            .iter()
            .map(|(page, count)| {
                Ok((
                    page.parse::<u32>()?,
                    u32::try_from(some(count.as_u64(), "a count")?)?,
                ))
            })
            .collect::<Result<_, Box<dyn std::error::Error>>>()?;
        let locations = locate_items(&layer, Ok(&layer));
        let found = undecoded_glyphs(&layer, |item| locations.page_of(item));
        assert_eq!(found.pages, declared);
        assert_eq!(found.unlocated_tokens, 0);
        Ok(())
    }
}
