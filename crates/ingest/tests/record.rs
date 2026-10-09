//! The whole-document `ConversionRecord`: windows joined in page order with their line ranges
//! shifted, coverage joined and checked, a window that hit the cap read as pages not
//! converted, and an outline whose heading entries select their sections over the joined text.
#![cfg(feature = "runtime")]

#[path = "../../../tests/support/check.rs"]
mod check;

mod outline_sections {
    use okf_jawn_contract::common::TextRange;
    use okf_jawn_contract::read::{OutlineEntryKind, Selection};
    use okf_jawn_ingest::outline::{line_count, outline_of};

    fn spans(markdown: &str) -> Vec<(String, u16, u32, u32)> {
        outline_of(markdown)
            .into_iter()
            .map(|entry| {
                assert_eq!(entry.kind, OutlineEntryKind::Heading);
                let Selection::Lines {
                    range: TextRange { start, end },
                } = entry.selection
                else {
                    return (entry.label, entry.level, 0, 0);
                };
                (entry.label, entry.level, start, end)
            })
            .collect()
    }

    #[test]
    fn a_heading_selects_lines_up_to_the_next_heading_of_the_same_or_a_higher_level() {
        let markdown = "# One\n\ntext\n\n## One.a\n\nmore\n\n### One.a.i\n\n## One.b\n\nlast\n\n# Two\n\nend\n";
        assert_eq!(line_count(markdown), 17);
        assert_eq!(
            spans(markdown),
            [
                ("One".to_owned(), 1, 1, 14),
                ("One.a".to_owned(), 2, 5, 10),
                ("One.a.i".to_owned(), 3, 9, 10),
                ("One.b".to_owned(), 2, 11, 14),
                ("Two".to_owned(), 1, 15, 17),
            ]
        );
    }

    #[test]
    fn code_and_text_that_only_looks_like_a_heading_is_not_one() {
        let markdown = "```sql\n# not a heading\n```\n#hashtag\n####### seven\n    # indented code\n## Real ##\n";
        assert_eq!(spans(markdown), [("Real".to_owned(), 2, 7, 7)]);
        assert_eq!(outline_of("").len(), 0);
    }
}

mod assembled_records {
    use okf_jawn_contract::common::{PageRange, TextRange};
    use okf_jawn_contract::error::ErrorCode;
    use okf_jawn_contract::extraction::{
        ConversionOutcome, ConversionSettings, ConverterIdentity, ExtractionWarning, FailureReason,
        PageCoverage, PageGlyphs,
    };
    use okf_jawn_contract::identity::Digest;
    use okf_jawn_contract::read::Selection;
    use okf_jawn_contract::source::{SourceLocation, SourceLocator};
    use okf_jawn_core::conversion::{ConversionStatus, LineLocation, WindowCoverage};
    use okf_jawn_ingest::record::{
        ConvertedWindow, WindowOutcome, assemble, sha256_digest, windows_of, windows_within,
    };

    use crate::check::{TestResult, err_of, some};

    fn pages(start: u32, end: u32) -> PageRange {
        PageRange { start, end }
    }

    fn identity() -> ConverterIdentity {
        ConverterIdentity {
            name: "docling".to_owned(),
            version: "2.3.0".to_owned(),
            packages: Vec::new(),
            settings: ConversionSettings::default(),
            models: Vec::new(),
            page_window: Some(2),
        }
    }

    fn digest(text: &str) -> Result<Digest, Box<dyn std::error::Error>> {
        Ok(sha256_digest(text.as_bytes())?)
    }

    fn converted(
        window: &PageRange,
        markdown: &str,
        partly: Vec<PageRange>,
    ) -> Result<WindowOutcome, Box<dyn std::error::Error>> {
        let converted: Vec<PageRange> = if partly.is_empty() {
            vec![window.clone()]
        } else {
            Vec::new()
        };
        Ok(WindowOutcome {
            window: Some(window.clone()),
            status: ConversionStatus::Success,
            coverage: Some(WindowCoverage {
                window: window.clone(),
                converted,
                partly_extracted: partly,
                not_converted: Vec::new(),
            }),
            issues: Vec::new(),
            converted: Some(ConvertedWindow {
                markdown: markdown.to_owned(),
                export: digest(markdown)?,
                locations: vec![LineLocation {
                    lines: TextRange { start: 1, end: 1 },
                    location: SourceLocation::Direct {
                        locator: SourceLocator::Page {
                            page_no: window.start,
                        },
                    },
                }],
                tables: Vec::new(),
                assets: Vec::new(),
                warnings: Vec::new(),
            }),
        })
    }

    fn capped(window: PageRange) -> WindowOutcome {
        WindowOutcome {
            window: Some(window),
            status: ConversionStatus::Failure(FailureReason::MemoryLimit {
                limit_bytes: "1048576".to_owned(),
            }),
            coverage: None,
            issues: Vec::new(),
            converted: None,
        }
    }

    #[test]
    fn a_document_is_cut_into_windows_of_the_configured_size() {
        assert_eq!(windows_of(9, 4), [pages(1, 4), pages(5, 8), pages(9, 9)]);
        assert_eq!(windows_of(4, 4), [pages(1, 4)]);
        assert_eq!(
            windows_within(&[pages(2, 3), pages(7, 12)], 4),
            [pages(2, 3), pages(7, 10), pages(11, 12)]
        );
        assert_eq!(
            windows_within(&[pages(0, 3), pages(5, 4)], 4),
            Vec::<PageRange>::new()
        );
    }

    #[test]
    fn windows_join_in_page_order_with_lines_shifted_and_the_outline_recomputed() -> TestResult {
        let first = converted(&pages(1, 2), "# Part one\n\nalpha\n", Vec::new())?;
        let second = converted(&pages(3, 4), "beta\n\n## Part one.b\n\ngamma\n", Vec::new())?;
        let assembled = assemble(identity(), Some(4), vec![first, second])?;
        assert_eq!(assembled.outcome, ConversionOutcome::Completed);
        assert_eq!(
            assembled.markdown,
            "# Part one\n\nalpha\n\nbeta\n\n## Part one.b\n\ngamma\n"
        );
        let record = some(assembled.record, "the record")?;
        assert_eq!(record.markdown, digest(&assembled.markdown)?);
        assert_eq!(record.page_count, Some(4));
        assert_eq!(
            record
                .structured
                .iter()
                .map(|export| export.window.clone())
                .collect::<Vec<_>>(),
            [Some(pages(1, 2)), Some(pages(3, 4))]
        );
        // Each window names the lines of the joined text it produced (R-I5).
        assert_eq!(
            record
                .structured
                .iter()
                .map(|export| export.lines.clone())
                .collect::<Vec<_>>(),
            [
                Some(TextRange { start: 1, end: 3 }),
                Some(TextRange { start: 5, end: 9 })
            ]
        );
        // The second window's first line is line 5 of the joined text.
        assert_eq!(
            record
                .locations
                .iter()
                .map(|location| location.lines.clone())
                .collect::<Vec<_>>(),
            [
                TextRange { start: 1, end: 1 },
                TextRange { start: 5, end: 5 }
            ]
        );
        // The first window's section runs on into the second window.
        let spans: Vec<(String, TextRange)> = record
            .outline
            .iter()
            .filter_map(|entry| match &entry.selection {
                Selection::Lines { range } => Some((entry.label.clone(), range.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            spans,
            [
                ("Part one".to_owned(), TextRange { start: 1, end: 9 }),
                ("Part one.b".to_owned(), TextRange { start: 7, end: 9 }),
            ]
        );
        Ok(())
    }

    #[test]
    fn a_window_that_hit_the_cap_is_pages_not_converted_not_a_failed_document() -> TestResult {
        let assembled = assemble(
            identity(),
            Some(6),
            vec![
                converted(&pages(1, 2), "one\n", Vec::new())?,
                capped(pages(3, 4)),
                converted(&pages(5, 6), "three\n", Vec::new())?,
            ],
        )?;
        assert_eq!(
            assembled.outcome,
            ConversionOutcome::Partial {
                coverage: Some(PageCoverage {
                    page_count: 6,
                    converted: vec![pages(1, 2), pages(5, 6)],
                    partly_extracted: Vec::new(),
                    not_converted: vec![pages(3, 4)],
                }),
                issues: Vec::new(),
            }
        );
        assert_eq!(assembled.markdown, "one\n\nthree\n");
        let record = some(assembled.record, "the record")?;
        assert_eq!(
            record
                .structured
                .iter()
                .map(|export| (export.window.clone(), export.lines.clone()))
                .collect::<Vec<_>>(),
            [
                (Some(pages(1, 2)), Some(TextRange { start: 1, end: 1 })),
                (Some(pages(5, 6)), Some(TextRange { start: 3, end: 3 }))
            ]
        );
        // A window that produced a document without text names no lines.
        let silent = assemble(
            identity(),
            Some(4),
            vec![
                converted(&pages(1, 2), "", Vec::new())?,
                converted(&pages(3, 4), "four\n", Vec::new())?,
            ],
        )?;
        let silent = some(silent.record, "the record")?;
        assert_eq!(
            silent
                .structured
                .iter()
                .map(|export| export.lines.clone())
                .collect::<Vec<_>>(),
            [None, Some(TextRange { start: 1, end: 1 })]
        );
        // When no window produced anything, the document failed with the first reason.
        let failed = assemble(
            identity(),
            Some(4),
            vec![capped(pages(1, 2)), capped(pages(3, 4))],
        )?;
        assert!(failed.record.is_none());
        assert_eq!(
            failed.outcome,
            ConversionOutcome::Failed {
                reason: FailureReason::MemoryLimit {
                    limit_bytes: "1048576".to_owned()
                },
                issues: Vec::new()
            }
        );
        Ok(())
    }

    #[test]
    fn undecodable_pages_are_partly_extracted_and_their_warnings_add_up() -> TestResult {
        let mut first = converted(&pages(1, 2), "a\n", vec![pages(1, 2)])?;
        let mut second = converted(&pages(3, 3), "b\n", vec![pages(3, 3)])?;
        for (window, page) in [(&mut first, 2), (&mut second, 3)] {
            some(window.converted.as_mut(), "a document")?
                .warnings
                .push(ExtractionWarning::UndecodableGlyphs {
                    pages: vec![PageGlyphs {
                        page_no: page,
                        glyphs: 4,
                    }],
                    unlocated_glyphs: 1,
                });
        }
        let assembled = assemble(identity(), Some(3), vec![first, second])?;
        let ConversionOutcome::Partial { coverage, .. } = &assembled.outcome else {
            return Err(format!("expected partial, got {:?}", assembled.outcome).into());
        };
        assert_eq!(
            some(coverage.as_ref(), "the coverage")?.partly_extracted,
            [pages(1, 3)]
        );
        assert_eq!(
            some(assembled.record, "the record")?.warnings,
            [ExtractionWarning::UndecodableGlyphs {
                pages: vec![
                    PageGlyphs {
                        page_no: 2,
                        glyphs: 4
                    },
                    PageGlyphs {
                        page_no: 3,
                        glyphs: 4
                    }
                ],
                unlocated_glyphs: 2
            }]
        );
        Ok(())
    }

    #[test]
    fn a_format_converted_whole_maps_its_status_and_unsupported_ends_the_document() -> TestResult {
        let mut whole = converted(&pages(1, 1), "text\n", Vec::new())?;
        whole.window = None;
        whole.coverage = None;
        whole.status = ConversionStatus::PartialSuccess;
        let assembled = assemble(identity(), None, vec![whole])?;
        assert_eq!(
            assembled.outcome,
            ConversionOutcome::Partial {
                coverage: None,
                issues: Vec::new()
            }
        );
        let unsupported = WindowOutcome {
            window: None,
            status: ConversionStatus::Unsupported,
            coverage: None,
            issues: Vec::new(),
            converted: None,
        };
        let assembled = assemble(identity(), None, vec![unsupported])?;
        assert_eq!(assembled.outcome, ConversionOutcome::Unsupported);
        assert!(assembled.record.is_none());
        Ok(())
    }

    #[test]
    fn windows_that_disagree_with_the_request_are_a_worker_fault() -> TestResult {
        // Coverage of another window.
        let mut wrong = converted(&pages(1, 2), "a\n", Vec::new())?;
        if let Some(coverage) = wrong.coverage.as_mut() {
            coverage.window = pages(3, 4);
        }
        let error = err_of(assemble(identity(), Some(2), vec![wrong]))?;
        assert_eq!(error.code, ErrorCode::Internal);
        // Windows that leave a page of the document uncovered.
        let short = converted(&pages(1, 2), "a\n", Vec::new())?;
        let error = err_of(assemble(identity(), Some(3), vec![short]))?;
        assert_eq!(error.code, ErrorCode::Internal);
        // A success with no document.
        let mut empty = converted(&pages(1, 2), "a\n", Vec::new())?;
        empty.converted = None;
        assert_eq!(
            err_of(assemble(identity(), Some(2), vec![empty]))?.code,
            ErrorCode::Internal
        );
        Ok(())
    }
}
