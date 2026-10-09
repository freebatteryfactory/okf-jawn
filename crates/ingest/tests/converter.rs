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

    use crate::check::{TestResult, err_of};

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
        assert_eq!(Request::from_json(&request.to_json()?)?, request);
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
                blocks: vec![Block {
                    lines: TextRange { start: 1, end: 3 },
                    page: Some(5),
                    table: true,
                }],
                text_layer: true,
            };
            assert_eq!(Reply::from_json(&reply.to_json()?)?, reply);
        }
        Ok(())
    }

    #[test]
    fn a_reply_of_the_wrong_shape_is_a_worker_fault() -> TestResult {
        let error = err_of(Reply::from_json(&json!({ "status": "converted" })))?;
        assert_eq!(error.code, ErrorCode::Internal);
        let error = err_of(Request::from_json(&json!({ "task": "convert" })))?;
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert!(error.field.is_some());
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
                    table: false,
                },
                Block {
                    lines: TextRange { start: 3, end: 5 },
                    page: Some(1),
                    table: true,
                },
                Block {
                    lines: TextRange { start: 7, end: 7 },
                    page: Some(2),
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
        // Page 1 has a placeholder in a table cell, page 2 in a paragraph; page 3 has none.
        assert_eq!(coverage.converted, vec![PageRange { start: 3, end: 3 }]);
        assert_eq!(
            coverage.partly_extracted,
            vec![PageRange { start: 1, end: 2 }]
        );
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

mod conversion_child {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use okf_jawn_contract::extraction::{ConversionSettings, FailureReason};
    use okf_jawn_contract::read::Selection;
    use okf_jawn_core::conversion::{
        ConversionInput, ConversionStatus, Converter, ConverterLimits,
    };
    use okf_jawn_core::storage::{LocalSource, ObjectInfo};
    use okf_jawn_ingest::converter::{ConverterConfig, DoclingConverter};
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
