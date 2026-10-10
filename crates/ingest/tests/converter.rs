//! The docling adapter: the child protocol, a window read back from the child's files, and the
//! production `Converter` running the real conversion child (`okf-jawn-convert`).
//!
//! Every test here but the last needs no model: a Markdown original converts through the real
//! child with docling's pure-Rust Markdown backend. The last converts a PDF with the pinned
//! models and pdfium, so it is ignored by default and runs locally only:
//! `DOCLING_RS_MODELS_DIR=<pinned models> cargo test -p okf-jawn-ingest --features runtime
//! --test converter -- --ignored`.
#![cfg(feature = "runtime")]

#[path = "../../../tests/support/check.rs"]
mod check;

mod conversion_protocol {
    use std::path::PathBuf;
    use std::time::Duration;

    use okf_jawn_contract::common::{PageRange, TextRange};
    use okf_jawn_contract::error::ErrorCode;
    use okf_jawn_contract::extraction::{ConversionSettings, ConverterIssue, FailureReason};
    use okf_jawn_core::conversion::ConversionStatus;
    use okf_jawn_ingest::protocol::{Block, Reply, Request, Task};
    use serde_json::json;

    use crate::check::{TestResult, err_of, some};

    #[test]
    fn a_request_and_a_reply_read_back_as_written() -> TestResult {
        let request = Request {
            task: Task::Convert,
            source: PathBuf::from("objects/ab/cdef"),
            file_name: "report.pdf".to_owned(),
            settings: ConversionSettings::default(),
            window: Some(PageRange { start: 5, end: 8 }),
            timeout: Duration::from_millis(90_500),
        };
        assert_eq!(Request::decode(&request.encode()?)?, request);
        for status in [
            ConversionStatus::Success,
            ConversionStatus::PartialSuccess,
            ConversionStatus::Unsupported,
            ConversionStatus::Failure(FailureReason::TimeLimit { limit_seconds: 90 }),
        ] {
            let reply = Reply {
                status,
                page_count: Some(18),
                issues: vec![ConverterIssue {
                    component_type: "model".to_owned(),
                    module_name: "layout".to_owned(),
                    error_message: "slow".to_owned(),
                }],
                blocks: vec![
                    Block {
                        lines: TextRange { start: 1, end: 3 },
                        page: Some(5),
                        unlocated: None,
                        table: true,
                    },
                    Block {
                        lines: TextRange { start: 5, end: 5 },
                        page: None,
                        unlocated: Some("Figure 1 \"quoted\"".to_owned()),
                        table: false,
                    },
                ],
                text_layer: true,
            };
            assert_eq!(Reply::decode(&reply.encode()?)?, reply);
        }
        Ok(())
    }

    #[test]
    fn a_reply_of_the_wrong_shape_is_a_worker_fault() -> TestResult {
        let error = err_of(Reply::decode(
            json!({ "status": "converted" }).to_string().as_bytes(),
        ))?;
        assert_eq!(error.code, ErrorCode::Internal);
        let error = err_of(Request::decode(
            json!({ "task": "convert" }).to_string().as_bytes(),
        ))?;
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert!(error.message.contains("source"), "{}", error.message);
        // A failure must say why, and nothing else may.
        let reply = json!({ "status": "failure", "reason": null, "page_count": null, "issues": [], "blocks": [], "text_layer": false });
        assert_eq!(
            err_of(Reply::decode(reply.to_string().as_bytes()))?.code,
            ErrorCode::Internal
        );
        Ok(())
    }

    #[test]
    fn a_field_the_protocol_does_not_know_is_refused() -> TestResult {
        let request = Request {
            task: Task::PageCount,
            source: PathBuf::from("objects/ab/cdef"),
            file_name: "report.pdf".to_owned(),
            settings: ConversionSettings::default(),
            window: None,
            timeout: Duration::from_secs(120),
        };
        let mut written: serde_json::Value = serde_json::from_slice(&request.encode()?)?;
        let object = some(written.as_object_mut(), "the request object")?;
        let _previous = object.insert("renderer".to_owned(), json!("docling-parse"));
        let error = err_of(Request::decode(written.to_string().as_bytes()))?;
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert!(error.message.contains("renderer"), "{}", error.message);

        let reply = Reply {
            status: ConversionStatus::Success,
            page_count: Some(3),
            issues: Vec::new(),
            blocks: vec![Block {
                lines: TextRange { start: 1, end: 1 },
                page: Some(1),
                unlocated: None,
                table: false,
            }],
            text_layer: false,
        };
        let mut written: serde_json::Value = serde_json::from_slice(&reply.encode()?)?;
        let object = some(written.as_object_mut(), "the reply object")?;
        let _previous = object.insert("pages_seen".to_owned(), json!(3));
        let error = err_of(Reply::decode(written.to_string().as_bytes()))?;
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(error.message.contains("pages_seen"), "{}", error.message);
        // An unknown field inside a line run is refused as well.
        let mut written: serde_json::Value = serde_json::from_slice(&reply.encode()?)?;
        let run = some(
            written
                .get_mut("blocks")
                .and_then(|blocks| blocks.get_mut(0))
                .and_then(serde_json::Value::as_object_mut),
            "the first line run",
        )?;
        let _previous = run.insert("bbox".to_owned(), json!([0, 0, 1, 1]));
        let error = err_of(Reply::decode(written.to_string().as_bytes()))?;
        assert!(error.message.contains("bbox"), "{}", error.message);
        Ok(())
    }
}

mod flags_undecodable_text_in_a_window {
    use std::path::Path;

    use okf_jawn_contract::common::{PageRange, TextRange};
    use okf_jawn_contract::extraction::{ExtractionWarning, PageGlyphs};
    use okf_jawn_contract::read::{OutlineEntryKind, Selection};
    use okf_jawn_contract::source::{SourceLocation, SourceLocator};
    use okf_jawn_core::conversion::ConversionStatus;
    use okf_jawn_ingest::converter::read_window;
    use okf_jawn_ingest::glyphs::UNDECODED;
    use okf_jawn_ingest::protocol::{Block, EXPORT_FILE, MARKDOWN_FILE, Reply, TEXT_LAYER_FILE};
    use serde_json::{Value, json};

    use crate::check::{TestResult, some};

    fn prov(page: u64) -> Value {
        json!([{ "page_no": page, "bbox": { "l": 72.0, "t": 700.0, "r": 300.0, "b": 680.0, "coord_origin": "BOTTOMLEFT" } }])
    }

    /// A two-page window: a heading and a table on page 1, a paragraph with placeholders on
    /// page 2, and a caption the converter left unlocated whose text the text layer has once.
    fn write_window(directory: &Path) -> TestResult {
        let size = json!({ "width": 612.0, "height": 792.0 });
        let export = json!({
            "pages": { "1": { "page_no": 1, "size": size }, "2": { "page_no": 2, "size": size } },
            "body": { "self_ref": "#/body", "children": [] },
            "texts": [
                { "self_ref": "#/texts/0", "label": "section_header", "text": "Usage", "prov": prov(1), "parent": { "$ref": "#/body" } },
                { "self_ref": "#/texts/1", "label": "text", "text": "/SM590000 Work /g115 Function", "prov": prov(2), "parent": { "$ref": "#/body" } },
                { "self_ref": "#/texts/2", "label": "caption", "text": "Table 1 Settings", "prov": [], "parent": { "$ref": "#/tables/0" } },
            ],
            "tables": [{
                "self_ref": "#/tables/0", "label": "table", "prov": prov(1), "parent": { "$ref": "#/body" },
                "data": {
                    "num_rows": 2, "num_cols": 2,
                    "table_cells": [
                        { "text": "Name", "start_row_offset_idx": 0, "end_row_offset_idx": 1, "start_col_offset_idx": 0, "end_col_offset_idx": 1, "column_header": true, "row_header": false },
                        { "text": "Value", "start_row_offset_idx": 0, "end_row_offset_idx": 1, "start_col_offset_idx": 1, "end_col_offset_idx": 2, "column_header": true, "row_header": false },
                        { "text": "limit", "start_row_offset_idx": 1, "end_row_offset_idx": 2, "start_col_offset_idx": 0, "end_col_offset_idx": 1, "column_header": false, "row_header": false },
                        { "text": "/SM590000 4", "start_row_offset_idx": 1, "end_row_offset_idx": 2, "start_col_offset_idx": 1, "end_col_offset_idx": 2, "column_header": false, "row_header": false },
                    ],
                },
            }],
            "pictures": [],
        });
        let layer = json!({
            "pages": { "1": { "page_no": 1, "size": size } },
            "texts": [
                { "self_ref": "#/texts/0", "label": "text", "text": "Table 1 Settings", "prov": prov(1) },
            ],
        });
        let markdown = "## Usage\n\n| Name | Value |\n| - | - |\n| limit | /SM590000 4 |\n\n/SM590000 Work /g115 Function\n";
        std::fs::write(directory.join(MARKDOWN_FILE), markdown)?;
        std::fs::write(directory.join(EXPORT_FILE), export.to_string())?;
        std::fs::write(directory.join(TEXT_LAYER_FILE), layer.to_string())?;
        Ok(())
    }

    fn reply() -> Reply {
        Reply {
            status: ConversionStatus::Success,
            page_count: None,
            issues: Vec::new(),
            blocks: vec![
                Block {
                    lines: TextRange { start: 1, end: 1 },
                    page: Some(1),
                    unlocated: None,
                    table: false,
                },
                Block {
                    lines: TextRange { start: 3, end: 5 },
                    page: Some(1),
                    unlocated: None,
                    table: true,
                },
                Block {
                    lines: TextRange { start: 7, end: 7 },
                    page: Some(2),
                    unlocated: None,
                    table: false,
                },
            ],
            text_layer: true,
        }
    }

    #[test]
    fn a_window_shows_no_placeholder_and_reports_its_page_as_partly_extracted() -> TestResult {
        let directory = tempfile::tempdir()?;
        write_window(directory.path())?;
        let window = PageRange { start: 1, end: 3 };
        let (document, coverage) = read_window(directory.path(), Some(&window), &reply())?;
        // Never shown or indexed as words.
        assert!(
            !document.markdown.contains("/SM590000"),
            "{}",
            document.markdown
        );
        assert!(
            !document.markdown.contains("/g115"),
            "{}",
            document.markdown
        );
        assert!(
            document
                .markdown
                .contains(&format!("{UNDECODED} Work {UNDECODED} Function"))
        );
        let coverage = some(coverage, "the window's coverage")?;
        // Page 1 has a placeholder in a table cell, page 2 in a paragraph. Page 3 is not in the
        // export at all, so it was not converted.
        assert_eq!(coverage.converted, Vec::new());
        assert_eq!(
            coverage.partly_extracted,
            vec![PageRange { start: 1, end: 2 }]
        );
        assert_eq!(coverage.not_converted, vec![PageRange { start: 3, end: 3 }]);
        coverage.check()?;
        assert!(
            document
                .warnings
                .contains(&ExtractionWarning::UndecodableGlyphs {
                    pages: vec![
                        PageGlyphs {
                            page_no: 1,
                            glyphs: 1
                        },
                        PageGlyphs {
                            page_no: 2,
                            glyphs: 2
                        },
                    ],
                    unlocated_glyphs: 0,
                }),
            "{:?}",
            document.warnings
        );
        // The table cell is scrubbed too.
        let table = some(document.tables.first(), "the table")?;
        assert!(
            table
                .cells
                .iter()
                .all(|cell| !cell.text.contains("/SM590000"))
        );
        Ok(())
    }

    #[test]
    fn a_window_locates_lines_tables_and_the_unlocated_caption() -> TestResult {
        let directory = tempfile::tempdir()?;
        write_window(directory.path())?;
        let window = PageRange { start: 1, end: 2 };
        let (document, _coverage) = read_window(directory.path(), Some(&window), &reply())?;
        // Each line run is on its page, from the converter's own page markers.
        let pages: Vec<(u32, Option<u32>)> = document
            .locations
            .iter()
            .map(|location| match &location.location {
                SourceLocation::Direct {
                    locator: SourceLocator::Page { page_no },
                } => (location.lines.start, Some(*page_no)),
                SourceLocation::Direct { .. }
                | SourceLocation::Inferred { .. }
                | SourceLocation::Unresolved { .. } => (location.lines.start, None),
            })
            .collect();
        assert_eq!(pages, [(1, Some(1)), (3, Some(1)), (7, Some(2))]);
        // The table is on its run's lines with its grid and its box.
        let table = some(document.tables.first(), "the table")?;
        assert_eq!(table.lines, TextRange { start: 3, end: 5 });
        assert_eq!(
            (table.num_rows, table.num_cols, table.cells.len()),
            (2, 2, 4)
        );
        assert!(matches!(
            table.location,
            SourceLocation::Direct {
                locator: SourceLocator::Region { .. }
            }
        ));
        // The caption the converter left unlocated was found through the text layer.
        assert!(
            document
                .warnings
                .contains(&ExtractionWarning::InferredLocations { items: 1 }),
            "{:?}",
            document.warnings
        );
        // The outline's heading selects its section to the end of the text.
        let heading = some(document.outline.first(), "the heading")?;
        assert_eq!(heading.kind, OutlineEntryKind::Heading);
        assert_eq!(
            heading.selection,
            Selection::Lines {
                range: TextRange { start: 1, end: 7 }
            }
        );
        Ok(())
    }

    #[test]
    fn tables_are_left_out_when_the_runs_and_the_export_disagree() -> TestResult {
        let directory = tempfile::tempdir()?;
        write_window(directory.path())?;
        let mut reply = reply();
        for block in &mut reply.blocks {
            block.table = false;
        }
        let window = PageRange { start: 1, end: 2 };
        let (document, _coverage) = read_window(directory.path(), Some(&window), &reply)?;
        assert_eq!(document.tables, Vec::new());
        Ok(())
    }
}

mod locates_unlocated_items_in_line_runs {
    use docling::{DoclingDocument, Node};
    use okf_jawn_contract::common::{PageRange, TextRange};
    use okf_jawn_contract::source::{SourceLocation, SourceLocator, UnresolvedReason};
    use okf_jawn_core::conversion::ConversionStatus;
    use okf_jawn_ingest::child::line_blocks;
    use okf_jawn_ingest::converter::read_window;
    use okf_jawn_ingest::protocol::{Block, EXPORT_FILE, MARKDOWN_FILE, Reply, TEXT_LAYER_FILE};
    use serde_json::{Value, json};

    use crate::check::{TestResult, some};

    fn page(page_no: usize) -> Node {
        Node::PageInfo {
            page_no,
            width: 612.0,
            height: 792.0,
        }
    }

    fn paragraph(text: &str) -> Node {
        Node::Paragraph {
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_line_run_is_on_a_page_only_by_its_own_provenance() -> TestResult {
        let mut document = DoclingDocument::new("window");
        document.push(page(1));
        document.push(Node::Located {
            location: [40, 40, 300, 60],
            inner: Box::new(paragraph("Located on one.")),
        });
        // No provenance of its own: the page marker before it does not locate it.
        document.push(paragraph("Left unlocated."));
        document.push(page(2));
        document.push(Node::Prov {
            page_no: 2,
            bbox: [72.0, 80.0, 300.0, 100.0],
            charspan: [0, 14],
            seq: None,
            inner: Box::new(paragraph("Placed on two.")),
        });
        let markdown = document.export_to_markdown();
        let blocks = line_blocks(&document, &markdown);
        let seen: Vec<(Option<u32>, Option<&str>)> = blocks
            .iter()
            .map(|block| (block.page, block.unlocated.as_deref()))
            .collect();
        assert_eq!(
            seen,
            [
                (Some(1), None),
                (None, Some("Left unlocated.")),
                (Some(2), None)
            ],
            "{markdown}"
        );
        assert!(!some(blocks.get(1), "the unlocated run")?.table);
        // The JSON export agrees: only the unwrapped paragraph has no provenance.
        let export = document.export_to_json_value();
        let unlocated: Vec<&str> = export
            .get("texts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| {
                item.get("prov")
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)
            })
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect();
        assert_eq!(unlocated, ["Left unlocated."]);
        Ok(())
    }

    #[test]
    fn a_run_without_provenance_takes_the_location_rule_result_never_a_page_marker() -> TestResult {
        let directory = tempfile::tempdir()?;
        let size = json!({ "width": 612.0, "height": 792.0 });
        let located = json!([{ "page_no": 1, "bbox": { "l": 72.0, "t": 700.0, "r": 300.0, "b": 680.0, "coord_origin": "BOTTOMLEFT" } }]);
        let found = json!([{ "page_no": 1, "bbox": { "l": 72.0, "t": 600.0, "r": 300.0, "b": 580.0, "coord_origin": "BOTTOMLEFT" } }]);
        let export = json!({
            "pages": { "1": { "page_no": 1, "size": size } },
            "body": { "self_ref": "#/body", "children": [
                { "$ref": "#/texts/0" }, { "$ref": "#/texts/1" }, { "$ref": "#/texts/2" }
            ] },
            "texts": [
                { "self_ref": "#/texts/0", "label": "text", "text": "Located.", "prov": located, "parent": { "$ref": "#/body" } },
                { "self_ref": "#/texts/1", "label": "text", "text": "Found by text.", "prov": [], "parent": { "$ref": "#/body" } },
                { "self_ref": "#/texts/2", "label": "text", "text": "Nowhere.", "prov": [], "parent": { "$ref": "#/body" } },
            ],
            "tables": [],
            "pictures": [],
        });
        let layer = json!({
            "pages": { "1": { "page_no": 1, "size": size } },
            "texts": [
                { "self_ref": "#/texts/0", "label": "text", "text": "Found by text.", "prov": found },
            ],
        });
        std::fs::write(
            directory.path().join(MARKDOWN_FILE),
            "Located.\n\nFound by text.\n\nNowhere.\n\nNot exported.\n",
        )?;
        std::fs::write(directory.path().join(EXPORT_FILE), export.to_string())?;
        std::fs::write(directory.path().join(TEXT_LAYER_FILE), layer.to_string())?;
        let run = |line: u32, page: Option<u32>, unlocated: Option<&str>| Block {
            lines: TextRange {
                start: line,
                end: line,
            },
            page,
            unlocated: unlocated.map(str::to_owned),
            table: false,
        };
        let reply = Reply {
            status: ConversionStatus::Success,
            page_count: None,
            issues: Vec::new(),
            blocks: vec![
                run(1, Some(1), None),
                run(3, None, Some("Found by text.")),
                run(5, None, Some("Nowhere.")),
                run(7, None, Some("Not exported.")),
            ],
            text_layer: true,
        };
        let window = PageRange { start: 1, end: 1 };
        let (document, _coverage) = read_window(directory.path(), Some(&window), &reply)?;
        let locations: Vec<(u32, &SourceLocation)> = document
            .locations
            .iter()
            .map(|location| (location.lines.start, &location.location))
            .collect();
        assert_eq!(locations.len(), 4, "{locations:?}");
        assert_eq!(
            some(locations.first(), "line 1")?.1,
            &SourceLocation::Direct {
                locator: SourceLocator::Page { page_no: 1 }
            }
        );
        // The rule found it through the text layer: inferred, with the box it found.
        assert!(
            matches!(
                some(locations.get(1), "line 3")?.1,
                SourceLocation::Inferred {
                    locator: SourceLocator::Region { region }
                } if region.page_no == 1
            ),
            "{locations:?}"
        );
        // The rule did not find it: unresolved with the rule's reason.
        assert_eq!(
            some(locations.get(2), "line 5")?.1,
            &SourceLocation::Unresolved {
                reason: UnresolvedReason::NoMatch
            }
        );
        // No export item to pair with: not located by the converter.
        assert_eq!(
            some(locations.get(3), "line 7")?.1,
            &SourceLocation::Unresolved {
                reason: UnresolvedReason::NotLocatedByConverter
            }
        );
        Ok(())
    }
}

mod partial_windows {
    use okf_jawn_contract::common::PageRange;
    use okf_jawn_core::conversion::ConversionStatus;
    use okf_jawn_ingest::converter::read_window;
    use okf_jawn_ingest::protocol::{EXPORT_FILE, MARKDOWN_FILE, Reply};
    use serde_json::{Value, json};

    use crate::check::{TestResult, some};

    fn prov(page: u64) -> Value {
        json!([{ "page_no": page, "bbox": { "l": 72.0, "t": 700.0, "r": 300.0, "b": 680.0, "coord_origin": "BOTTOMLEFT" } }])
    }

    #[test]
    fn a_window_that_ran_out_of_time_lists_only_the_pages_it_converted() -> TestResult {
        // Docling 2.3.0 `PartialSuccess`: the budget ran out after two of four pages, and only
        // the finished pages became the document.
        let directory = tempfile::tempdir()?;
        let size = json!({ "width": 612.0, "height": 792.0 });
        let export = json!({
            "pages": { "1": { "page_no": 1, "size": size }, "2": { "page_no": 2, "size": size } },
            "body": { "self_ref": "#/body", "children": [] },
            "texts": [
                { "self_ref": "#/texts/0", "label": "text", "text": "One", "prov": prov(1), "parent": { "$ref": "#/body" } },
                { "self_ref": "#/texts/1", "label": "text", "text": "Two", "prov": prov(2), "parent": { "$ref": "#/body" } },
            ],
            "tables": [],
            "pictures": [],
        });
        std::fs::write(directory.path().join(MARKDOWN_FILE), "One\n\nTwo\n")?;
        std::fs::write(directory.path().join(EXPORT_FILE), export.to_string())?;
        let reply = Reply {
            status: ConversionStatus::PartialSuccess,
            page_count: None,
            issues: Vec::new(),
            blocks: Vec::new(),
            text_layer: false,
        };
        let window = PageRange { start: 1, end: 4 };
        let (_document, coverage) = read_window(directory.path(), Some(&window), &reply)?;
        let coverage = some(coverage, "the window's coverage")?;
        assert_eq!(coverage.converted, vec![PageRange { start: 1, end: 2 }]);
        assert_eq!(coverage.partly_extracted, Vec::new());
        assert_eq!(coverage.not_converted, vec![PageRange { start: 3, end: 4 }]);
        coverage.check()?;
        Ok(())
    }
}

mod conversion_child {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use okf_jawn_contract::extraction::{ConversionSettings, FailureReason};
    use okf_jawn_contract::read::Selection;
    use okf_jawn_core::conversion::{
        ConversionInput, ConversionStatus, Converter, ConverterLimits,
    };
    use okf_jawn_core::storage::{LocalSource, ObjectInfo};
    use okf_jawn_ingest::cap::MEMORY_LIMIT_ENV;
    use okf_jawn_ingest::converter::{ConverterConfig, DoclingConverter};
    use okf_jawn_ingest::protocol::{REQUEST_FILE, RESULT_FILE, Request, Task};
    use okf_jawn_ingest::record::sha256_digest;

    use crate::check::{TestResult, some};

    const LIMITS: ConverterLimits = ConverterLimits {
        window_pages: 4,
        memory_limit_bytes: 2 * 1024 * 1024 * 1024,
    };

    fn converter(child: PathBuf) -> DoclingConverter {
        DoclingConverter::new(ConverterConfig {
            child,
            limits: LIMITS,
        })
    }

    /// A retained original: bytes at a path with no extension.
    fn retained(directory: &Path, bytes: &[u8]) -> Result<LocalSource, Box<dyn std::error::Error>> {
        let path = directory.join("object");
        std::fs::write(&path, bytes)?;
        Ok(LocalSource {
            path,
            object: ObjectInfo {
                digest: sha256_digest(bytes)?,
                size: u64::try_from(bytes.len())?,
            },
        })
    }

    fn input(source: LocalSource, file_name: &str, output: &Path) -> ConversionInput {
        ConversionInput {
            source,
            file_name: file_name.to_owned(),
            settings: ConversionSettings::default(),
            window: None,
            timeout: Duration::from_secs(60),
            output_directory: output.to_path_buf(),
        }
    }

    #[tokio::test]
    async fn a_markdown_original_converts_through_the_real_child() -> TestResult {
        let store = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let bytes = b"# Guide\n\nIntro.\n\n## Setup\n\nSteps.\n\n# Appendix\n\nMore.\n";
        let source = retained(store.path(), bytes)?;
        let converter = converter(PathBuf::from(env!("CARGO_BIN_EXE_okf-jawn-convert")));
        assert_eq!(converter.page_count(&source, "guide.md").await?, None);
        let conversion = converter
            .convert(input(source.clone(), "guide.md", output.path()))
            .await?;
        assert_eq!(
            conversion.status,
            ConversionStatus::Success,
            "{:?}",
            conversion.issues
        );
        assert_eq!(conversion.source_digest, source.object.digest);
        assert!(conversion.coverage.is_none());
        assert_eq!(conversion.converter.name, "docling");
        assert_eq!(conversion.converter.models, Vec::new());
        assert_ne!(conversion.converter.packages, Vec::new());
        let document = some(conversion.document, "the converted document")?;
        assert!(
            document.markdown.contains("## Setup"),
            "{}",
            document.markdown
        );
        assert!(document.structured.is_file());
        // "Guide" runs to the line before "Appendix"; "Setup" is inside it.
        let spans: Vec<(String, u32, u32)> = document
            .outline
            .iter()
            .filter_map(|entry| match &entry.selection {
                Selection::Lines { range } => Some((entry.label.clone(), range.start, range.end)),
                Selection::All
                | Selection::Pages { .. }
                | Selection::Cells { .. }
                | Selection::Section { .. }
                | Selection::Region { .. } => None,
            })
            .collect();
        let appendix = some(
            spans.iter().find(|(label, _, _)| label == "Appendix"),
            "the Appendix heading",
        )?;
        let guide = some(spans.first(), "the Guide heading")?;
        assert_eq!(
            (guide.0.as_str(), guide.1, guide.2),
            ("Guide", 1, appendix.1 - 1)
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_child_that_ends_without_a_reply_is_a_failed_window() -> TestResult {
        // This test binary as the child: it takes the request path as a test filter, runs no
        // test, and exits 0 without writing a reply.
        let store = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let source = retained(store.path(), b"# Note\n")?;
        let converter = converter(std::env::current_exe()?);
        let conversion = converter
            .convert(input(source, "note.md", output.path()))
            .await?;
        assert_eq!(
            conversion.status,
            ConversionStatus::Failure(FailureReason::ConverterCrashed)
        );
        assert!(conversion.document.is_none());
        assert_ne!(conversion.issues, Vec::new());
        Ok(())
    }

    /// A request for the real child to convert `source` as `file_name`, written into `output`.
    fn write_request(
        source: &LocalSource,
        file_name: &str,
        output: &Path,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let request = Request {
            task: Task::Convert,
            source: source.path.clone(),
            file_name: file_name.to_owned(),
            settings: ConversionSettings::default(),
            window: None,
            timeout: Duration::from_secs(60),
        };
        let path = output.join(REQUEST_FILE);
        std::fs::write(&path, request.encode()?)?;
        Ok(path)
    }

    #[tokio::test]
    async fn the_real_child_takes_its_cap_before_it_reads_its_request() -> TestResult {
        // The real `okf-jawn-convert`, started without a usable cap, stops at its first
        // statement (`cap::limit_self`): it never reads the request, so it writes no result.
        let store = tempfile::tempdir()?;
        let source = retained(store.path(), b"# Note\n\nText.\n")?;
        for cap in [None, Some("0"), Some("lots")] {
            let output = tempfile::tempdir()?;
            let request = write_request(&source, "note.md", output.path())?;
            let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_okf-jawn-convert"));
            let _configured = command.arg(&request).env_remove(MEMORY_LIMIT_ENV);
            if let Some(cap) = cap {
                let _configured = command.env(MEMORY_LIMIT_ENV, cap);
            }
            let ended = command.output().await?;
            let stderr = String::from_utf8_lossy(&ended.stderr);
            assert!(!ended.status.success(), "{cap:?}: {stderr}");
            assert!(stderr.contains(MEMORY_LIMIT_ENV), "{cap:?}: {stderr}");
            assert!(!output.path().join(RESULT_FILE).exists(), "{cap:?}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn the_real_child_under_a_small_cap_fails_its_window_at_the_cap() -> TestResult {
        // The real child under a 16 MiB cap must hold a 24 MiB original to convert it. On
        // Linux the child's own RLIMIT_DATA is the cap (set as the first statement of its
        // main); on Windows the job object holds it.
        const CAP: u64 = 16 * 1024 * 1024;
        let store = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let line = "Words of a large original that does not fit the cap.\n\n";
        let bytes = line.repeat(24 * 1024 * 1024 / line.len());
        let source = retained(store.path(), bytes.as_bytes())?;
        let converter = DoclingConverter::new(ConverterConfig {
            child: PathBuf::from(env!("CARGO_BIN_EXE_okf-jawn-convert")),
            limits: ConverterLimits {
                window_pages: 4,
                memory_limit_bytes: CAP,
            },
        });
        let mut request = input(source, "large.md", output.path());
        request.timeout = Duration::from_secs(30);
        let conversion = converter.convert(request).await?;
        assert_eq!(
            conversion.status,
            ConversionStatus::Failure(FailureReason::MemoryLimit {
                limit_bytes: CAP.to_string()
            }),
            "{:?}",
            conversion.issues
        );
        assert!(conversion.document.is_none());
        Ok(())
    }

    #[tokio::test]
    #[ignore = "needs the pinned docling models and pdfium; run locally with --ignored"]
    async fn a_pdf_window_converts_with_the_pinned_models() -> TestResult {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/documents");
        let bytes = std::fs::read(root.join("born_digital_text.pdf"))?;
        let store = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let source = retained(store.path(), &bytes)?;
        let converter = converter(PathBuf::from(env!("CARGO_BIN_EXE_okf-jawn-convert")));
        let pages = some(
            converter
                .page_count(&source, "born_digital_text.pdf")
                .await?,
            "the page count",
        )?;
        let window = okf_jawn_contract::common::PageRange {
            start: 1,
            end: pages.min(LIMITS.window_pages),
        };
        let mut request = input(source, "born_digital_text.pdf", output.path());
        request.window = Some(window.clone());
        let conversion = converter.convert(request).await?;
        assert!(
            matches!(
                conversion.status,
                ConversionStatus::Success | ConversionStatus::PartialSuccess
            ),
            "{:?} {:?}",
            conversion.status,
            conversion.issues
        );
        assert_ne!(conversion.converter.models, Vec::new());
        let coverage = some(conversion.coverage, "the window's coverage")?;
        assert_eq!(coverage.window, window);
        coverage.check()?;
        let document = some(conversion.document, "the converted document")?;
        assert!(!document.locations.is_empty(), "no line was located");
        Ok(())
    }
}
