//! Direct-library Docling qualification harness.
//!
//! Instantiates `docling::DocumentConverter` against real fixtures and assets.
//! Does not implement `okf_jawn_core::conversion::Converter` or any product port.
//!
//! The orchestrator (`qualification/docling/run.mjs`) runs one process per fixture,
//! named by `OKF_DOCLING_ONLY`, so peak resident memory is per fixture. With
//! `OKF_DOCLING_HOLD=1` the process prints one JSON "done" line on stdout once its
//! receipt is on disk and then blocks on stdin until EOF; the orchestrator reads this
//! process's peak memory exactly once during that wait.
//!
//! Every receipt records the stage the fixture reached: `source` (the file never
//! reached the converter), `converter_error` (`convert` returned `Err`) or
//! `converter_status` (`convert` returned `Ok`). A must-fail fixture passes only at
//! the last two, and only when the converter refused it.

use docling::{ConversionStatus, DocumentConverter, SourceDocument};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Printed once on stdout when the receipt is on disk and the process is about to wait.
#[derive(Serialize)]
struct DoneMarker<'a> {
    okf_docling: &'static str,
    only: &'a str,
    receipt: &'a Path,
}

/// Process inputs read once from the environment.
struct Environment {
    converter_version: String,
    fixtures_dir: String,
    hold: bool,
    models_dir: String,
    out_dir: String,
}

#[derive(Clone, Serialize)]
struct ErrorReceipt {
    component_type: String,
    error_message: String,
    module_name: String,
}

/// What a fixture must produce for the gate to pass.
#[derive(Clone, Copy)]
enum Expected {
    /// Supported extraction: `Success`, non-empty Markdown, optional substring.
    SuccessNonEmpty { must_contain: Option<&'static str> },
    /// The converter itself must refuse the input.
    ExplicitFailure,
}

#[derive(Clone, Serialize)]
struct FixtureReceipt {
    converter_version: String,
    elapsed_ms: u128,
    errors: Vec<ErrorReceipt>,
    expected: String,
    /// Set when a must-fail fixture was accepted; a decision for the owner.
    finding: Option<String>,
    fixture: String,
    markdown_chars: usize,
    markdown_nonempty: bool,
    must_contain_ok: Option<bool>,
    original_unchanged: bool,
    outcome: String,
    page_image_count: usize,
    page_provenance: Vec<PageProvenance>,
    path: PathBuf,
    /// Always null in-process: authored Rust forbids the OS FFI needed to sample RSS.
    /// The orchestrator attaches the peak it read for this process.
    peak_rss_bytes: Option<u64>,
    role: String,
    settings: BTreeMap<String, serde_json::Value>,
    sha256_after: String,
    sha256_before: String,
    stage: Stage,
    /// The `ConversionStatus` label; null unless `convert` returned `Ok`.
    status: Option<String>,
}

/// One fixture about to be handed to the converter.
struct FixtureRun<'a> {
    expected: Expected,
    path: &'a Path,
    role: &'a str,
    session: &'a Session,
    sha_before: String,
    started: Instant,
}

/// The gate's verdict on one observation.
struct Judgement {
    finding: Option<String>,
    must_contain_ok: Option<bool>,
    outcome: &'static str,
}

/// What came back from the converter call, with the evidence to record.
struct Observed<'a> {
    elapsed_ms: u128,
    errors: Vec<ErrorReceipt>,
    page_provenance: Vec<PageProvenance>,
    reached: Reached<'a>,
}

#[derive(Clone, Serialize)]
struct PageProvenance {
    page_no: usize,
    has_image: bool,
}

#[derive(Serialize)]
struct QualificationReport {
    converter_crate: String,
    fixtures_dir: PathBuf,
    models_dir: PathBuf,
    receipts: Vec<FixtureReceipt>,
    summary: BTreeMap<String, String>,
    timeout_case: Option<FixtureReceipt>,
}

/// How far a fixture travelled, with what the gate judges at that point.
#[derive(Clone, Copy)]
enum Reached<'a> {
    /// `SourceDocument::from_file` failed; the converter never saw the fixture.
    Source,
    /// `DocumentConverter::convert` returned `Err`.
    ConverterError,
    /// `DocumentConverter::convert` returned `Ok` with this status and Markdown.
    ConverterStatus {
        status: ConversionStatus,
        markdown: &'a str,
    },
}

/// Converter identity and settings recorded on every receipt of this process.
struct Session {
    converter_version: String,
    settings: BTreeMap<String, serde_json::Value>,
}

/// The stage name written to the receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Stage {
    Source,
    ConverterError,
    ConverterStatus,
}

const DONE: &str = "done";
const MUST_FAIL_FINDING: &str = "converter accepts truncated PDF";
const MUST_FAIL_NAME: &str = "must_fail_truncated.pdf";
const TIMEOUT_BUDGET_MS: u64 = 1;
const TIMEOUT_PROBE: &str = "timeout_probe";

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn status_label(status: ConversionStatus) -> &'static str {
    match status {
        ConversionStatus::Failure => "Failure",
        ConversionStatus::PartialSuccess => "PartialSuccess",
        ConversionStatus::Success => "Success",
    }
}

fn expected_label(expected: Expected) -> &'static str {
    match expected {
        Expected::SuccessNonEmpty { .. } => "SuccessNonEmpty",
        Expected::ExplicitFailure => "ExplicitFailure",
    }
}

fn stage_of(reached: Reached<'_>) -> Stage {
    match reached {
        Reached::Source => Stage::Source,
        Reached::ConverterError => Stage::ConverterError,
        Reached::ConverterStatus { .. } => Stage::ConverterStatus,
    }
}

fn page_provenance(result: &docling::ConversionResult) -> Vec<PageProvenance> {
    let mut pages: Vec<PageProvenance> = result
        .document
        .page_images
        .iter()
        .map(|(page_no, image)| PageProvenance {
            page_no: *page_no,
            has_image: !image.mimetype.is_empty(),
        })
        .collect();
    pages.sort_by_key(|page| page.page_no);
    pages
}

fn verdict(outcome: &'static str) -> Judgement {
    Judgement {
        finding: None,
        must_contain_ok: None,
        outcome,
    }
}

/// Judge one observation. A mutated original fails whatever else happened.
fn judge(expected: Expected, reached: Reached<'_>, original_unchanged: bool) -> Judgement {
    if !original_unchanged {
        return verdict("FAIL_original_mutated");
    }
    match expected {
        Expected::ExplicitFailure => judge_must_fail(reached),
        Expected::SuccessNonEmpty { must_contain } => judge_success(must_contain, reached),
    }
}

/// PASS only when the converter itself refused the input. A source-stage error never
/// reached the converter; an accepted input is a finding, and the fixture stays as it is.
fn judge_must_fail(reached: Reached<'_>) -> Judgement {
    match reached {
        Reached::Source => verdict("FAIL_before_converter"),
        Reached::ConverterError
        | Reached::ConverterStatus {
            status: ConversionStatus::Failure,
            ..
        } => verdict("PASS_explicit_failure"),
        Reached::ConverterStatus {
            status: ConversionStatus::Success | ConversionStatus::PartialSuccess,
            ..
        } => Judgement {
            finding: Some(MUST_FAIL_FINDING.to_owned()),
            must_contain_ok: None,
            outcome: "FAIL_expected_failure",
        },
    }
}

fn judge_success(must_contain: Option<&str>, reached: Reached<'_>) -> Judgement {
    let (status, markdown) = match reached {
        Reached::Source => return verdict("FAIL_source_error"),
        Reached::ConverterError => return verdict("FAIL_converter_error"),
        Reached::ConverterStatus { status, markdown } => (status, markdown),
    };
    let must_contain_ok = must_contain.map(|needle| markdown.contains(needle));
    let outcome = if matches!(status, ConversionStatus::Failure) {
        "FAIL_unexpected_failure"
    } else if markdown.trim().is_empty() {
        "FAIL_empty_markdown"
    } else if must_contain_ok == Some(false) {
        "FAIL_missing_expected_text"
    } else if matches!(status, ConversionStatus::Success) {
        "PASS"
    } else {
        // `PartialSuccess` with content is recorded but is not a pass for a supported fixture.
        "FAIL_partial_not_success"
    };
    Judgement {
        finding: None,
        must_contain_ok,
        outcome,
    }
}

/// A buffered `convert` reports a spent document budget as `PartialSuccess` with one
/// `pipeline` error item. An `Err` from the converter is never a honoured timeout.
fn timeout_honoured(stage: Stage, status: Option<&str>, errors: &[ErrorReceipt]) -> bool {
    stage == Stage::ConverterStatus
        && status == Some("PartialSuccess")
        && errors.iter().any(|item| {
            item.module_name == "pipeline" && item.error_message.to_lowercase().contains("timeout")
        })
}

fn build_receipt(run: &FixtureRun<'_>, observed: Observed<'_>) -> Result<FixtureReceipt, String> {
    let sha_after = sha256_file(run.path)?;
    let original_unchanged = run.sha_before == sha_after;
    let judgement = judge(run.expected, observed.reached, original_unchanged);
    let (status, markdown) = match observed.reached {
        Reached::ConverterStatus { status, markdown } => {
            (Some(status_label(status).to_owned()), markdown)
        }
        Reached::Source | Reached::ConverterError => (None, ""),
    };
    Ok(FixtureReceipt {
        converter_version: run.session.converter_version.clone(),
        elapsed_ms: observed.elapsed_ms,
        errors: observed.errors,
        expected: expected_label(run.expected).to_owned(),
        finding: judgement.finding,
        fixture: run
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        markdown_chars: markdown.chars().count(),
        markdown_nonempty: !markdown.trim().is_empty(),
        must_contain_ok: judgement.must_contain_ok,
        original_unchanged,
        outcome: judgement.outcome.to_owned(),
        page_image_count: observed.page_provenance.len(),
        page_provenance: observed.page_provenance,
        path: run.path.to_path_buf(),
        peak_rss_bytes: None,
        role: run.role.to_owned(),
        settings: run.session.settings.clone(),
        sha256_after: sha_after,
        sha256_before: run.sha_before.clone(),
        stage: stage_of(observed.reached),
        status,
    })
}

/// A receipt for a fixture that produced no `ConversionResult`.
fn refused(
    run: &FixtureRun<'_>,
    reached: Reached<'_>,
    module_name: &str,
    message: String,
) -> Result<FixtureReceipt, String> {
    let component_type = match reached {
        Reached::Source => "source",
        Reached::ConverterError | Reached::ConverterStatus { .. } => "converter",
    };
    build_receipt(
        run,
        Observed {
            elapsed_ms: run.started.elapsed().as_millis(),
            errors: vec![ErrorReceipt {
                component_type: component_type.to_owned(),
                error_message: message,
                module_name: module_name.to_owned(),
            }],
            page_provenance: Vec::new(),
            reached,
        },
    )
}

fn convert_fixture(
    converter: &DocumentConverter,
    run: &FixtureRun<'_>,
) -> Result<FixtureReceipt, String> {
    let source = match SourceDocument::from_file(run.path) {
        Ok(source) => source,
        Err(error) => {
            return refused(
                run,
                Reached::Source,
                "SourceDocument::from_file",
                error.to_string(),
            );
        }
    };
    let result = match converter.convert(source) {
        Ok(result) => result,
        Err(error) => {
            return refused(
                run,
                Reached::ConverterError,
                "DocumentConverter::convert",
                error.to_string(),
            );
        }
    };
    let elapsed_ms = run.started.elapsed().as_millis();
    let markdown = result.document.export_to_markdown();
    let errors = result
        .errors
        .iter()
        .map(|item| ErrorReceipt {
            component_type: item.component_type.clone(),
            error_message: item.error_message.clone(),
            module_name: item.module_name.clone(),
        })
        .collect();
    build_receipt(
        run,
        Observed {
            elapsed_ms,
            errors,
            page_provenance: page_provenance(&result),
            reached: Reached::ConverterStatus {
                status: result.status,
                markdown: &markdown,
            },
        },
    )
}

fn fixture_catalog() -> [(&'static str, &'static str, Expected); 12] {
    [
        (
            "sample_with_image.docx",
            "docx_with_images",
            Expected::SuccessNonEmpty {
                must_contain: Some("OKF"),
            },
        ),
        (
            "sample_sheet.xlsx",
            "xlsx",
            Expected::SuccessNonEmpty {
                must_contain: Some("Widget"),
            },
        ),
        (
            "born_digital_text.pdf",
            "born_digital_pdf",
            Expected::SuccessNonEmpty {
                must_contain: Some("Born-digital"),
            },
        ),
        (
            "scanned_image_only.pdf",
            "scanned_or_image_pdf",
            Expected::SuccessNonEmpty { must_contain: None },
        ),
        (
            "scanned_text.pdf",
            "scanned_pdf_with_text",
            Expected::SuccessNonEmpty { must_contain: None },
        ),
        (
            "table_heavy.pdf",
            "table_heavy_pdf",
            Expected::SuccessNonEmpty {
                must_contain: Some("Name"),
            },
        ),
        (
            "sample_image.png",
            "image",
            Expected::SuccessNonEmpty { must_contain: None },
        ),
        (
            "text_image.png",
            "image_with_text",
            Expected::SuccessNonEmpty { must_contain: None },
        ),
        (
            "corpus/word_sample.docx",
            "corpus_docx",
            Expected::SuccessNonEmpty {
                must_contain: Some("Summer"),
            },
        ),
        (
            "corpus/xlsx_01.xlsx",
            "corpus_xlsx",
            Expected::SuccessNonEmpty {
                must_contain: Some("col-1"),
            },
        ),
        (
            "corpus/powerpoint_sample.pptx",
            "corpus_pptx",
            Expected::SuccessNonEmpty {
                must_contain: Some("Test Table"),
            },
        ),
        (
            "corpus/redp5110_sampled.pdf",
            "corpus_pdf",
            Expected::SuccessNonEmpty {
                must_contain: Some("IBM"),
            },
        ),
    ]
}

fn lookup_only(only: &str) -> Result<(&'static str, &'static str, Expected), String> {
    if only == MUST_FAIL_NAME || only == "must_fail" {
        return Ok((MUST_FAIL_NAME, "must_fail", Expected::ExplicitFailure));
    }
    if only == TIMEOUT_PROBE {
        return Ok((
            "scanned_image_only.pdf",
            TIMEOUT_PROBE,
            Expected::SuccessNonEmpty { must_contain: None },
        ));
    }
    for (name, role, expected) in fixture_catalog() {
        if name == only {
            return Ok((name, role, expected));
        }
    }
    Err(format!(
        "OKF_DOCLING_ONLY={only} is not a known fixture, must_fail, or timeout_probe"
    ))
}

fn write_report(out_dir: &str, report: &QualificationReport) -> Result<PathBuf, String> {
    fs::create_dir_all(out_dir).map_err(|error| error.to_string())?;
    let report_path = PathBuf::from(out_dir).join("receipt.json");
    let json = serde_json::to_string_pretty(report).map_err(|error| error.to_string())?;
    fs::write(&report_path, format!("{json}\n")).map_err(|error| error.to_string())?;
    writeln!(io::stdout(), "Wrote {}", report_path.display()).map_err(|error| error.to_string())?;
    Ok(report_path)
}

/// Print the done marker, then wait until the orchestrator closes stdin.
fn hold_for_sample(only: &str, receipt: &Path) -> Result<(), String> {
    let marker = DoneMarker {
        okf_docling: DONE,
        only,
        receipt,
    };
    let line = serde_json::to_string(&marker).map_err(|error| error.to_string())?;
    {
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{line}").map_err(|error| error.to_string())?;
        stdout.flush().map_err(|error| error.to_string())?;
    }
    let _bytes = io::copy(&mut io::stdin().lock(), &mut io::sink())
        .map_err(|error| format!("wait for stdin EOF: {error}"))?;
    Ok(())
}

fn build_session(environment: &Environment, artifacts: &Path, budget_ms: Option<u64>) -> Session {
    let mut settings = BTreeMap::new();
    settings.insert(
        "artifacts_dir".to_owned(),
        serde_json::Value::String(artifacts.display().to_string()),
    );
    settings.insert(
        "models_dir".to_owned(),
        serde_json::Value::String(environment.models_dir.clone()),
    );
    settings.insert(
        "ocr_lang".to_owned(),
        serde_json::Value::String("en".to_owned()),
    );
    if let Some(ms) = budget_ms {
        settings.insert(
            "document_timeout_ms".to_owned(),
            serde_json::Value::from(ms),
        );
    }
    Session {
        converter_version: environment.converter_version.clone(),
        settings,
    }
}

fn build_converter(artifacts: &Path, budget_ms: Option<u64>) -> DocumentConverter {
    DocumentConverter::new()
        .ocr_lang("en")
        .artifacts_dir(artifacts.display().to_string())
        .document_timeout(budget_ms.map(Duration::from_millis))
}

fn run_one(only: &str, environment: &Environment) -> Result<(), String> {
    let (name, role, expected) = lookup_only(only)?;
    let is_timeout = role == TIMEOUT_PROBE;
    let budget_ms = is_timeout.then_some(TIMEOUT_BUDGET_MS);
    let artifacts = tempfile::tempdir().map_err(|error| error.to_string())?;
    let session = build_session(environment, artifacts.path(), budget_ms);
    let path = PathBuf::from(&environment.fixtures_dir).join(name);
    if !path.is_file() {
        return Err(format!("missing fixture {}", path.display()));
    }
    let converter = build_converter(artifacts.path(), budget_ms);
    let run = FixtureRun {
        expected,
        path: &path,
        role,
        session: &session,
        sha_before: sha256_file(&path)?,
        started: Instant::now(),
    };
    let mut receipt = convert_fixture(&converter, &run)?;
    if is_timeout {
        let honoured = timeout_honoured(receipt.stage, receipt.status.as_deref(), &receipt.errors);
        receipt.outcome = if honoured {
            "PASS".to_owned()
        } else {
            "FAIL_timeout_not_honoured".to_owned()
        };
    }

    let key = if is_timeout { TIMEOUT_PROBE } else { name };
    let mut summary = BTreeMap::new();
    summary.insert(key.to_owned(), receipt.outcome.clone());
    let (receipts, timeout_case) = if is_timeout {
        (Vec::new(), Some(receipt))
    } else {
        (vec![receipt], None)
    };
    let report = QualificationReport {
        converter_crate: environment.converter_version.clone(),
        fixtures_dir: PathBuf::from(&environment.fixtures_dir),
        models_dir: PathBuf::from(&environment.models_dir),
        receipts,
        summary,
        timeout_case,
    };
    let report_path = write_report(&environment.out_dir, &report)?;
    // The process exits 0 once its receipt is written; the orchestrator judges outcomes.
    if environment.hold {
        hold_for_sample(only, &report_path)?;
    }
    Ok(())
}

fn read_environment() -> Result<Environment, String> {
    let models_dir = env::var("DOCLING_RS_MODELS_DIR")
        .map_err(|_| "DOCLING_RS_MODELS_DIR must be set to the verified models cache".to_owned())?;
    let converter_version = env::var("OKF_DOCLING_CRATE_VERSION")
        .ok()
        .map(|version| version.trim().to_owned())
        .filter(|version| !version.is_empty())
        .map(|version| format!("docling {version}"))
        .ok_or_else(|| {
            "OKF_DOCLING_CRATE_VERSION must be the docling version pinned in Cargo.lock; \
             run.mjs reads it there"
                .to_owned()
        })?;
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixtures_dir = env::var("OKF_DOCLING_FIXTURES").unwrap_or_else(|_| {
        manifest_dir
            .join("../../tests/fixtures/documents")
            .to_string_lossy()
            .into_owned()
    });
    let out_dir = env::var("OKF_DOCLING_OUT").unwrap_or_else(|_| {
        manifest_dir
            .join("../../.artifacts/qualification/docling")
            .to_string_lossy()
            .into_owned()
    });
    let hold = env::var("OKF_DOCLING_HOLD").is_ok_and(|value| value == "1");
    Ok(Environment {
        converter_version,
        fixtures_dir,
        hold,
        models_dir,
        out_dir,
    })
}

fn run() -> Result<(), String> {
    let environment = read_environment()?;
    let only = env::var("OKF_DOCLING_ONLY")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "OKF_DOCLING_ONLY must name one catalog fixture, must_fail_truncated.pdf or \
             timeout_probe: one process per fixture keeps peak memory per fixture"
                .to_owned()
        })?;
    run_one(&only, &environment)
}

fn main() -> Result<(), String> {
    run().inspect_err(|error| {
        let _ = writeln!(io::stderr(), "okf-qualify-docling: {error}");
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ConversionStatus, DONE, DoneMarker, ErrorReceipt, Expected, MUST_FAIL_FINDING,
        MUST_FAIL_NAME, Reached, Stage, TIMEOUT_PROBE, fixture_catalog, judge, lookup_only,
        timeout_honoured,
    };
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The value of an option that must be present; `what` names it in the failure.
    fn some<T>(value: Option<T>, what: &str) -> Result<T, String> {
        value.ok_or_else(|| format!("missing {what}"))
    }

    #[test]
    fn must_fail_is_judged_by_the_stage_it_reached() -> TestResult {
        let never_reached = judge(Expected::ExplicitFailure, Reached::Source, true);
        assert_eq!(never_reached.outcome, "FAIL_before_converter");
        assert_eq!(never_reached.finding, None);

        let refused = judge(Expected::ExplicitFailure, Reached::ConverterError, true);
        assert_eq!(refused.outcome, "PASS_explicit_failure");
        assert_eq!(refused.finding, None);

        let failure = Reached::ConverterStatus {
            status: ConversionStatus::Failure,
            markdown: "",
        };
        assert_eq!(
            judge(Expected::ExplicitFailure, failure, true).outcome,
            "PASS_explicit_failure"
        );

        for status in [ConversionStatus::Success, ConversionStatus::PartialSuccess] {
            let reached = Reached::ConverterStatus {
                status,
                markdown: "text",
            };
            let accepted = judge(Expected::ExplicitFailure, reached, true);
            assert_eq!(accepted.outcome, "FAIL_expected_failure");
            assert_eq!(some(accepted.finding, "finding")?, MUST_FAIL_FINDING);
        }

        let mutated = judge(Expected::ExplicitFailure, Reached::ConverterError, false);
        assert_eq!(mutated.outcome, "FAIL_original_mutated");
        Ok(())
    }

    #[test]
    fn a_converter_error_never_passes_a_supported_fixture_or_the_timeout_probe() -> TestResult {
        let (_, role, expected) = lookup_only(TIMEOUT_PROBE)?;
        assert_eq!(role, TIMEOUT_PROBE);
        assert_eq!(
            judge(expected, Reached::ConverterError, true).outcome,
            "FAIL_converter_error"
        );
        assert_eq!(
            judge(expected, Reached::Source, true).outcome,
            "FAIL_source_error"
        );

        let errors = [ErrorReceipt {
            component_type: "document_backend".to_owned(),
            error_message: "document timeout of 0.001s exceeded after 0 of 1 pages".to_owned(),
            module_name: "pipeline".to_owned(),
        }];
        assert!(!timeout_honoured(Stage::ConverterError, None, &errors));
        assert!(!timeout_honoured(
            Stage::ConverterStatus,
            Some("Success"),
            &errors
        ));
        assert!(!timeout_honoured(
            Stage::ConverterStatus,
            Some("PartialSuccess"),
            &[]
        ));
        assert!(timeout_honoured(
            Stage::ConverterStatus,
            Some("PartialSuccess"),
            &errors
        ));
        Ok(())
    }

    #[test]
    fn stage_names_and_the_done_marker_match_the_orchestrator_protocol() -> TestResult {
        assert_eq!(serde_json::to_value(Stage::Source)?, "source");
        assert_eq!(
            serde_json::to_value(Stage::ConverterError)?,
            "converter_error"
        );
        assert_eq!(
            serde_json::to_value(Stage::ConverterStatus)?,
            "converter_status"
        );
        let marker = DoneMarker {
            okf_docling: DONE,
            only: "sample_sheet.xlsx",
            receipt: Path::new("out/receipt.json"),
        };
        assert_eq!(
            serde_json::to_string(&marker)?,
            r#"{"okf_docling":"done","only":"sample_sheet.xlsx","receipt":"out/receipt.json"}"#
        );
        Ok(())
    }

    #[test]
    fn catalog_and_must_fail_are_exactly_the_recorded_sources() -> TestResult {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/documents/SOURCES.json");
        let sources: serde_json::Value = serde_json::from_str(&fs::read_to_string(path)?)?;
        let files = sources.get("files").and_then(serde_json::Value::as_object);
        let recorded: BTreeSet<String> = some(files, "files object")?.keys().cloned().collect();
        let harness: BTreeSet<String> = fixture_catalog()
            .into_iter()
            .map(|(name, _, _)| name.to_owned())
            .chain(std::iter::once(MUST_FAIL_NAME.to_owned()))
            .collect();
        assert_eq!(harness, recorded);
        Ok(())
    }
}
