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
//! This process observes and the orchestrator judges. Beside its receipt it writes what
//! the converter returned: `document.md` (`export_to_markdown`), `document.json`
//! (`export_to_json_value`, the docling wire schema with per-item `prov`) and one file per
//! page image. The receipt carries the hash of each, so content, structure, provenance and
//! page renders are judged from the files, against expectations this binary never sees.
//!
//! How the process ends is part of the protocol. It exits 0 once its receipt is written,
//! whatever the converter returned, and `EXIT_HARNESS` when it could not make the run itself.
//! Nothing here panics or exits otherwise, so any other exit is the process dying under the
//! library, and the orchestrator judges that as the library failing on the fixture.
//!
//! Every receipt records the stage the fixture reached: `source` (the file never
//! reached the converter), `converter_error` (`convert` returned `Err`) or
//! `converter_status` (`convert` returned `Ok`). A must-fail fixture passes only at
//! the last two, and only when the converter refused it. The one judgement made here is
//! that conversion-level rule, named in `conversion_rule`.
//!
//! For a PDF that converted, two more observations are recorded, each by a rule that is a
//! pure function over the document export: `locations` (module `locate`: every item with the
//! source that located it, the export or the library's own text-layer document) and
//! `undecoded_glyphs` (module `glyphs`: the library's placeholders for glyphs a font gives no
//! Unicode for, by page). The orchestrator judges both; these two modules are the reference
//! behaviour for the ingest lane.

use docling::{
    ConversionResult, ConversionStatus, DocumentConverter, PictureImage, SourceDocument,
};
use glyphs::UndecodedGlyphs;
use locate::Locations;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// The builder calls this harness makes. One value drives the converter and the receipt,
/// so a setting cannot be recorded without being applied.
#[derive(Clone, Serialize)]
struct Applied {
    artifacts_dir: String,
    document_timeout_ms: Option<u64>,
    generate_page_images: bool,
    ocr_lang: &'static str,
}

/// Converter identity, with where each fact was read.
#[derive(Clone, Serialize)]
struct ConverterIdentity {
    /// `docling::PDF_ML_COMPILED`: a compile-time fact of the linked crate.
    pdf_ml_compiled: bool,
    version: String,
    version_source: &'static str,
}

/// What a successful `convert` returned, written beside the receipt.
#[derive(Clone, Serialize)]
struct DocumentEvidence {
    json_bytes: usize,
    json_file: &'static str,
    json_sha256: String,
    markdown_chars: usize,
    markdown_file: &'static str,
    markdown_nonempty: bool,
    markdown_sha256: String,
    page_images: Vec<PageImageRecord>,
}

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
    /// Every `DOCLING_*` and `PDFIUM_*` variable this process sees: the library reads them.
    docling_env: BTreeMap<String, String>,
    fixtures_dir: String,
    hold: bool,
    out_dir: String,
}

#[derive(Clone, Serialize)]
struct ErrorReceipt {
    component_type: String,
    error_message: String,
    module_name: String,
}

#[derive(Clone, Serialize)]
struct FixtureReceipt {
    /// The rule `outcome` was judged by; see [`rule_label`].
    conversion_rule: &'static str,
    converter: ConverterIdentity,
    /// Null unless `convert` returned `Ok`.
    document: Option<DocumentEvidence>,
    elapsed_ms: u128,
    errors: Vec<ErrorReceipt>,
    /// Set when a must-fail fixture was accepted; a decision for the owner.
    finding: Option<String>,
    fixture: String,
    /// The format the library detected (`InputFormat::as_str`); null when detection failed.
    input_format: Option<&'static str>,
    /// `docling::pdf_page_count` on the fixture bytes; null for a fixture that is not a PDF.
    library_page_count: Option<PageCount>,
    /// Where each item of a converted PDF is and which source located it; null for another
    /// format and when `convert` did not return `Ok`.
    locations: Option<Locations>,
    original_unchanged: bool,
    outcome: String,
    path: PathBuf,
    settings: Settings,
    sha256_after: String,
    sha256_before: String,
    stage: Stage,
    /// The `ConversionStatus` label; null unless `convert` returned `Ok`.
    status: Option<String>,
    /// The library's placeholders for glyphs without Unicode in a converted PDF, by page;
    /// null for another format and when `convert` did not return `Ok`.
    undecoded_glyphs: Option<UndecodedGlyphs>,
}

/// One fixture about to be handed to the converter.
struct FixtureRun<'a> {
    out_dir: &'a Path,
    path: &'a Path,
    rule: Rule,
    session: &'a Session,
    sha_before: String,
    started: Instant,
}

/// The gate's verdict on one observation.
struct Judgement {
    finding: Option<String>,
    outcome: &'static str,
}

/// One entry of `docling::model_inventory()`: the file a pipeline stage would load.
#[derive(Serialize)]
struct ModelRecord {
    bytes: u64,
    found: bool,
    path: String,
    stage: &'static str,
}

/// What came back from the converter call, with the evidence to record.
struct Observed<'a> {
    document: Option<DocumentEvidence>,
    elapsed_ms: u128,
    errors: Vec<ErrorReceipt>,
    input_format: Option<&'static str>,
    library_page_count: Option<PageCount>,
    pdf: Option<PdfFacts>,
    reached: Reached<'a>,
}

/// The library's own page count for a PDF, or the error it gave instead.
#[derive(Clone, Serialize)]
struct PageCount {
    error: Option<String>,
    value: Option<usize>,
}

/// What is observed of a converted PDF beyond its export.
struct PdfFacts {
    locations: Locations,
    undecoded_glyphs: UndecodedGlyphs,
}

/// One page image as the library returned it; `file` holds its bytes.
#[derive(Clone, Serialize)]
struct PageImageRecord {
    bytes: usize,
    dpi: u32,
    file: String,
    height: u32,
    mimetype: String,
    page_no: usize,
    sha256: String,
    width: u32,
}

#[derive(Serialize)]
struct QualificationReport {
    fixtures_dir: PathBuf,
    /// `docling::model_inventory()` under this process's environment.
    model_inventory: Vec<ModelRecord>,
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

/// The conversion-level rule a fixture is judged by.
#[derive(Clone, Copy)]
enum Rule {
    /// A supported fixture: `Success` with Markdown that is not blank.
    Converted,
    /// The must-fail fixture: the converter itself must refuse the input.
    ExplicitFailure,
    /// The timeout probe: a spent document budget reported as the library documents it.
    TimeoutHonoured,
}

/// Converter identity and settings recorded on every receipt of this process.
struct Session {
    converter: ConverterIdentity,
    settings: Settings,
}

/// The converter settings of this process, from three independent readings.
#[derive(Clone, Serialize)]
struct Settings {
    applied: Applied,
    /// `Debug` of the built `DocumentConverter`: every option as the library holds it,
    /// including the ones this harness leaves at the library default.
    converter_debug: String,
    environment: BTreeMap<String, String>,
}

/// The stage name written to the receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Stage {
    Source,
    ConverterError,
    ConverterStatus,
}

const DOCUMENT_JSON: &str = "document.json";
const DOCUMENT_MD: &str = "document.md";
const DONE: &str = "done";
/// The exit code of a run this binary itself could not make: a variable that is not set, a
/// fixture file that is missing, an output file that cannot be written. The orchestrator
/// (`lib/receipt.mjs` `HARNESS_EXIT`) reads it as a harness error. Every other non-zero exit
/// (a panic exits 101) is the process dying on the fixture, which the orchestrator records as
/// a failure of the library on that fixture.
const EXIT_HARNESS: u8 = 64;
const MUST_FAIL_FINDING: &str = "converter accepts truncated PDF";
const MUST_FAIL_NAME: &str = "must_fail_truncated.pdf";
const OCR_LANG: &str = "en";
const TIMEOUT_BUDGET_MS: u64 = 1;
const TIMEOUT_PROBE: &str = "timeout_probe";
const TIMEOUT_PROBE_FIXTURE: &str = "scanned_image_only.pdf";
const VERSION_SOURCE: &str = "environment variable OKF_DOCLING_CRATE_VERSION, which run.mjs sets from the docling entry of Cargo.lock";

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

fn status_label(status: ConversionStatus) -> &'static str {
    match status {
        ConversionStatus::Failure => "Failure",
        ConversionStatus::PartialSuccess => "PartialSuccess",
        ConversionStatus::Success => "Success",
    }
}

/// The name of the rule, as the receipt states it.
fn rule_label(rule: Rule) -> &'static str {
    match rule {
        Rule::Converted => "success_status_and_nonblank_markdown",
        Rule::ExplicitFailure => "converter_refuses_the_input",
        Rule::TimeoutHonoured => "partial_success_with_pipeline_timeout_error",
    }
}

fn stage_of(reached: Reached<'_>) -> Stage {
    match reached {
        Reached::Source => Stage::Source,
        Reached::ConverterError => Stage::ConverterError,
        Reached::ConverterStatus { .. } => Stage::ConverterStatus,
    }
}

fn verdict(outcome: &'static str) -> Judgement {
    Judgement {
        finding: None,
        outcome,
    }
}

/// Judge one observation. A mutated original fails whatever else happened.
fn judge(
    rule: Rule,
    reached: Reached<'_>,
    errors: &[ErrorReceipt],
    original_unchanged: bool,
) -> Judgement {
    if !original_unchanged {
        return verdict("FAIL_original_mutated");
    }
    match rule {
        Rule::ExplicitFailure => judge_must_fail(reached),
        Rule::Converted => judge_converted(reached),
        Rule::TimeoutHonoured => {
            let status = match reached {
                Reached::ConverterStatus { status, .. } => Some(status_label(status)),
                Reached::Source | Reached::ConverterError => None,
            };
            if timeout_honoured(stage_of(reached), status, errors) {
                verdict("PASS")
            } else {
                verdict("FAIL_timeout_not_honoured")
            }
        }
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
            outcome: "FAIL_expected_failure",
        },
    }
}

/// `Success` with Markdown that is not blank. What the Markdown says is not judged here.
fn judge_converted(reached: Reached<'_>) -> Judgement {
    let (status, markdown) = match reached {
        Reached::Source => return verdict("FAIL_source_error"),
        Reached::ConverterError => return verdict("FAIL_converter_error"),
        Reached::ConverterStatus { status, markdown } => (status, markdown),
    };
    verdict(match status {
        ConversionStatus::Failure => "FAIL_unexpected_failure",
        _ if markdown.trim().is_empty() => "FAIL_empty_markdown",
        ConversionStatus::Success => "PASS",
        // `PartialSuccess` with content is recorded but is not a pass for a supported fixture.
        ConversionStatus::PartialSuccess => "FAIL_partial_not_success",
    })
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
    let judgement = judge(
        run.rule,
        observed.reached,
        &observed.errors,
        original_unchanged,
    );
    let status = match observed.reached {
        Reached::ConverterStatus { status, .. } => Some(status_label(status).to_owned()),
        Reached::Source | Reached::ConverterError => None,
    };
    let (locations, undecoded_glyphs) = match observed.pdf {
        Some(pdf) => (Some(pdf.locations), Some(pdf.undecoded_glyphs)),
        None => (None, None),
    };
    Ok(FixtureReceipt {
        conversion_rule: rule_label(run.rule),
        converter: run.session.converter.clone(),
        document: observed.document,
        elapsed_ms: observed.elapsed_ms,
        errors: observed.errors,
        finding: judgement.finding,
        fixture: run
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        input_format: observed.input_format,
        library_page_count: observed.library_page_count,
        locations,
        original_unchanged,
        outcome: judgement.outcome.to_owned(),
        path: run.path.to_path_buf(),
        settings: run.session.settings.clone(),
        sha256_after: sha_after,
        sha256_before: run.sha_before.clone(),
        stage: stage_of(observed.reached),
        status,
        undecoded_glyphs,
    })
}

/// A receipt for a fixture that produced no `ConversionResult`.
fn refused(
    run: &FixtureRun<'_>,
    reached: Reached<'_>,
    module_name: &str,
    message: String,
    source: Option<(&'static str, Option<PageCount>)>,
) -> Result<FixtureReceipt, String> {
    let component_type = match reached {
        Reached::Source => "source",
        Reached::ConverterError | Reached::ConverterStatus { .. } => "converter",
    };
    let (input_format, library_page_count) = match source {
        Some((format, pages)) => (Some(format), pages),
        None => (None, None),
    };
    build_receipt(
        run,
        Observed {
            document: None,
            elapsed_ms: run.started.elapsed().as_millis(),
            errors: vec![ErrorReceipt {
                component_type: component_type.to_owned(),
                error_message: message,
                module_name: module_name.to_owned(),
            }],
            input_format,
            library_page_count,
            pdf: None,
            reached,
        },
    )
}

/// File extension for a page image of this mimetype; bytes are written as returned.
fn image_extension(mimetype: &str) -> &'static str {
    match mimetype {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        _ => "bin",
    }
}

/// Write each page image the library returned and record what it is.
fn write_page_images(
    out_dir: &Path,
    images: &BTreeMap<usize, PictureImage>,
) -> Result<Vec<PageImageRecord>, String> {
    let mut records = Vec::with_capacity(images.len());
    for (page_no, image) in images {
        let file = format!("page-{page_no}.{}", image_extension(&image.mimetype));
        let target = out_dir.join(&file);
        fs::write(&target, &image.data)
            .map_err(|error| format!("write {}: {error}", target.display()))?;
        records.push(PageImageRecord {
            bytes: image.data.len(),
            dpi: image.dpi,
            file,
            height: image.height,
            mimetype: image.mimetype.clone(),
            page_no: *page_no,
            sha256: sha256_hex(&image.data),
            width: image.width,
        });
    }
    Ok(records)
}

/// Write the Markdown, the document export and the page images of one conversion.
///
/// The page images leave the document first: the JSON export would otherwise inline each
/// one as a base64 `pages[n].image` (docling-core json.rs:329), and they are recorded here
/// from the bytes the library returned.
fn write_evidence(
    out_dir: &Path,
    result: &mut ConversionResult,
) -> Result<(DocumentEvidence, String), String> {
    let images = std::mem::take(&mut result.document.page_images);
    let page_images = write_page_images(out_dir, &images)?;
    let markdown = result.document.export_to_markdown();
    let json = serde_json::to_vec(&result.document.export_to_json_value())
        .map_err(|error| format!("serialize document export: {error}"))?;
    for (file, bytes) in [
        (DOCUMENT_MD, markdown.as_bytes()),
        (DOCUMENT_JSON, json.as_slice()),
    ] {
        let target = out_dir.join(file);
        fs::write(&target, bytes)
            .map_err(|error| format!("write {}: {error}", target.display()))?;
    }
    let evidence = DocumentEvidence {
        json_bytes: json.len(),
        json_file: DOCUMENT_JSON,
        json_sha256: sha256_hex(&json),
        markdown_chars: markdown.chars().count(),
        markdown_file: DOCUMENT_MD,
        markdown_nonempty: !markdown.trim().is_empty(),
        markdown_sha256: sha256_hex(markdown.as_bytes()),
        page_images,
    };
    Ok((evidence, markdown))
}

/// The library's page count for a PDF source; `None` for every other format.
fn library_page_count(source: &SourceDocument) -> Option<PageCount> {
    (source.format == docling::InputFormat::Pdf).then(|| {
        match docling::pdf_page_count(&source.bytes, None) {
            Ok(value) => PageCount {
                error: None,
                value: Some(value),
            },
            Err(error) => PageCount {
                error: Some(error.to_string()),
                value: None,
            },
        }
    })
}

/// The observations a converted PDF gets beyond its export.
///
/// The text layer is read with the library's own entry point, from the same bytes. When the
/// library cannot read it, every item the export left unlocated stays unlocated and the
/// error is recorded.
fn pdf_facts(export: &serde_json::Value, bytes: &[u8], name: &str) -> PdfFacts {
    let text_layer = docling::pdf_text_layer_pages(bytes, name, None)
        .map(|document| document.export_to_json_value())
        .map_err(|error| error.to_string());
    let locations = locate::locate_items(export, text_layer.as_ref().map_err(String::as_str));
    let undecoded_glyphs = glyphs::undecoded_glyphs(export, &locations.items);
    PdfFacts {
        locations,
        undecoded_glyphs,
    }
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
                None,
            );
        }
    };
    let input_format = source.format.as_str();
    let pages = library_page_count(&source);
    // The converter takes the source; a PDF's bytes are kept to read its text layer afterwards.
    let pdf_source = (source.format == docling::InputFormat::Pdf)
        .then(|| (source.bytes.clone(), source.name.clone()));
    let mut result = match converter.convert(source) {
        Ok(result) => result,
        Err(error) => {
            return refused(
                run,
                Reached::ConverterError,
                "DocumentConverter::convert",
                error.to_string(),
                Some((input_format, pages)),
            );
        }
    };
    let elapsed_ms = run.started.elapsed().as_millis();
    let errors = result
        .errors
        .iter()
        .map(|item| ErrorReceipt {
            component_type: item.component_type.clone(),
            error_message: item.error_message.clone(),
            module_name: item.module_name.clone(),
        })
        .collect();
    let (document, markdown) = write_evidence(run.out_dir, &mut result)?;
    // The export read here is the one just written: the page images have left the document.
    let pdf = pdf_source
        .map(|(bytes, name)| pdf_facts(&result.document.export_to_json_value(), &bytes, &name));
    build_receipt(
        run,
        Observed {
            document: Some(document),
            elapsed_ms,
            errors,
            input_format: Some(result.format.as_str()),
            library_page_count: pages,
            pdf,
            reached: Reached::ConverterStatus {
                status: result.status,
                markdown: &markdown,
            },
        },
    )
}

/// The supported fixtures. What each must contain is declared in `SOURCES.json` and judged
/// by the orchestrator; this list only says which files this binary will open.
fn fixture_catalog() -> [&'static str; 12] {
    [
        "sample_with_image.docx",
        "sample_sheet.xlsx",
        "born_digital_text.pdf",
        "scanned_image_only.pdf",
        "scanned_text.pdf",
        "table_heavy.pdf",
        "sample_image.png",
        "text_image.png",
        "corpus/word_sample.docx",
        "corpus/xlsx_01.xlsx",
        "corpus/powerpoint_sample.pptx",
        "corpus/redp5110_sampled.pdf",
    ]
}

/// The fixture file and the rule for one `OKF_DOCLING_ONLY` value.
fn lookup_only(only: &str) -> Result<(&'static str, Rule), String> {
    if only == MUST_FAIL_NAME || only == "must_fail" {
        return Ok((MUST_FAIL_NAME, Rule::ExplicitFailure));
    }
    if only == TIMEOUT_PROBE {
        return Ok((TIMEOUT_PROBE_FIXTURE, Rule::TimeoutHonoured));
    }
    fixture_catalog()
        .into_iter()
        .find(|name| *name == only)
        .map(|name| (name, Rule::Converted))
        .ok_or_else(|| {
            format!("OKF_DOCLING_ONLY={only} is not a known fixture, must_fail, or timeout_probe")
        })
}

fn write_report(out_dir: &Path, report: &QualificationReport) -> Result<PathBuf, String> {
    let report_path = out_dir.join("receipt.json");
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

/// The converter for one process. Page images are requested (`generate_page_images`,
/// docling converter.rs:756); `images_scale` is left unset, so each page image is the
/// pipeline's own page render, not a resample (docling-pdf assemble.rs:2419).
fn build_converter(applied: &Applied) -> DocumentConverter {
    DocumentConverter::new()
        .ocr_lang(applied.ocr_lang)
        .artifacts_dir(applied.artifacts_dir.clone())
        .generate_page_images(applied.generate_page_images)
        .document_timeout(applied.document_timeout_ms.map(Duration::from_millis))
}

fn build_session(
    environment: &Environment,
    artifacts: &Path,
    budget_ms: Option<u64>,
) -> (DocumentConverter, Session) {
    let applied = Applied {
        artifacts_dir: artifacts.display().to_string(),
        document_timeout_ms: budget_ms,
        generate_page_images: true,
        ocr_lang: OCR_LANG,
    };
    let converter = build_converter(&applied);
    let session = Session {
        converter: ConverterIdentity {
            pdf_ml_compiled: docling::PDF_ML_COMPILED,
            version: environment.converter_version.clone(),
            version_source: VERSION_SOURCE,
        },
        settings: Settings {
            applied,
            converter_debug: format!("{converter:?}"),
            environment: environment.docling_env.clone(),
        },
    };
    (converter, session)
}

fn model_inventory() -> Vec<ModelRecord> {
    docling::model_inventory()
        .into_iter()
        .map(|entry| ModelRecord {
            bytes: entry.bytes,
            found: entry.found,
            path: entry.path,
            stage: entry.stage,
        })
        .collect()
}

fn run_one(only: &str, environment: &Environment) -> Result<(), String> {
    let (name, rule) = lookup_only(only)?;
    let is_timeout = matches!(rule, Rule::TimeoutHonoured);
    let budget_ms = is_timeout.then_some(TIMEOUT_BUDGET_MS);
    let artifacts = tempfile::tempdir().map_err(|error| error.to_string())?;
    let (converter, session) = build_session(environment, artifacts.path(), budget_ms);
    let path = PathBuf::from(&environment.fixtures_dir).join(name);
    if !path.is_file() {
        return Err(format!("missing fixture {}", path.display()));
    }
    let out_dir = PathBuf::from(&environment.out_dir);
    fs::create_dir_all(&out_dir).map_err(|error| error.to_string())?;
    let inventory = model_inventory();
    let run = FixtureRun {
        out_dir: &out_dir,
        path: &path,
        rule,
        session: &session,
        sha_before: sha256_file(&path)?,
        started: Instant::now(),
    };
    let receipt = convert_fixture(&converter, &run)?;

    let key = if is_timeout { TIMEOUT_PROBE } else { name };
    let mut summary = BTreeMap::new();
    summary.insert(key.to_owned(), receipt.outcome.clone());
    let (receipts, timeout_case) = if is_timeout {
        (Vec::new(), Some(receipt))
    } else {
        (vec![receipt], None)
    };
    let report = QualificationReport {
        fixtures_dir: PathBuf::from(&environment.fixtures_dir),
        model_inventory: inventory,
        receipts,
        summary,
        timeout_case,
    };
    let report_path = write_report(&out_dir, &report)?;
    // The process exits 0 once its receipt is written; the orchestrator judges outcomes.
    if environment.hold {
        hold_for_sample(only, &report_path)?;
    }
    Ok(())
}

fn read_environment() -> Result<Environment, String> {
    if env::var("DOCLING_RS_MODELS_DIR").is_err() {
        return Err("DOCLING_RS_MODELS_DIR must be set to the verified models cache".to_owned());
    }
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
    let docling_env = env::vars()
        .filter(|(name, _)| name.starts_with("DOCLING_") || name.starts_with("PDFIUM_"))
        .collect();
    Ok(Environment {
        converter_version,
        docling_env,
        fixtures_dir,
        hold,
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

/// Ends with `EXIT_HARNESS` when this binary could not make the run, and never panics or
/// exits anywhere else, so any other non-zero exit is the process dying under the library.
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "okf-qualify-docling: {error}");
            ExitCode::from(EXIT_HARNESS)
        }
    }
}

mod glyphs;
mod locate;

#[cfg(test)]
mod tests {
    use super::{
        Applied, ConversionResult, ConversionStatus, DOCUMENT_JSON, DOCUMENT_MD, DONE, DoneMarker,
        ErrorReceipt, MUST_FAIL_FINDING, MUST_FAIL_NAME, PictureImage, Reached, Rule, Stage,
        TIMEOUT_PROBE, TIMEOUT_PROBE_FIXTURE, build_converter, fixture_catalog, judge, lookup_only,
        rule_label, sha256_hex, timeout_honoured, write_evidence,
    };
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The value of an option that must be present; `what` names it in the failure.
    fn some<T>(value: Option<T>, what: &str) -> Result<T, String> {
        value.ok_or_else(|| format!("missing {what}"))
    }

    fn timeout_error() -> [ErrorReceipt; 1] {
        [ErrorReceipt {
            component_type: "document_backend".to_owned(),
            error_message: "document timeout of 0.001s exceeded after 0 of 1 pages".to_owned(),
            module_name: "pipeline".to_owned(),
        }]
    }

    #[test]
    fn must_fail_is_judged_by_the_stage_it_reached() -> TestResult {
        let never_reached = judge(Rule::ExplicitFailure, Reached::Source, &[], true);
        assert_eq!(never_reached.outcome, "FAIL_before_converter");
        assert_eq!(never_reached.finding, None);

        let refused = judge(Rule::ExplicitFailure, Reached::ConverterError, &[], true);
        assert_eq!(refused.outcome, "PASS_explicit_failure");
        assert_eq!(refused.finding, None);

        let failure = Reached::ConverterStatus {
            status: ConversionStatus::Failure,
            markdown: "",
        };
        assert_eq!(
            judge(Rule::ExplicitFailure, failure, &[], true).outcome,
            "PASS_explicit_failure"
        );

        for status in [ConversionStatus::Success, ConversionStatus::PartialSuccess] {
            let reached = Reached::ConverterStatus {
                status,
                markdown: "text",
            };
            let accepted = judge(Rule::ExplicitFailure, reached, &[], true);
            assert_eq!(accepted.outcome, "FAIL_expected_failure");
            assert_eq!(some(accepted.finding, "finding")?, MUST_FAIL_FINDING);
        }

        let mutated = judge(Rule::ExplicitFailure, Reached::ConverterError, &[], false);
        assert_eq!(mutated.outcome, "FAIL_original_mutated");
        Ok(())
    }

    #[test]
    fn a_supported_fixture_passes_only_on_success_with_nonblank_markdown() -> TestResult {
        let (_, rule) = lookup_only("born_digital_text.pdf")?;
        assert_eq!(rule_label(rule), "success_status_and_nonblank_markdown");
        let reached = |status, markdown| Reached::ConverterStatus { status, markdown };
        let outcome = |reached| judge(rule, reached, &[], true).outcome;
        assert_eq!(outcome(reached(ConversionStatus::Success, "text")), "PASS");
        assert_eq!(
            outcome(reached(ConversionStatus::Success, " \n")),
            "FAIL_empty_markdown"
        );
        assert_eq!(
            outcome(reached(ConversionStatus::PartialSuccess, "text")),
            "FAIL_partial_not_success"
        );
        assert_eq!(
            outcome(reached(ConversionStatus::Failure, "text")),
            "FAIL_unexpected_failure"
        );
        assert_eq!(outcome(Reached::ConverterError), "FAIL_converter_error");
        assert_eq!(outcome(Reached::Source), "FAIL_source_error");
        Ok(())
    }

    #[test]
    fn the_timeout_probe_is_judged_by_its_own_rule_and_says_so() -> TestResult {
        let (name, rule) = lookup_only(TIMEOUT_PROBE)?;
        assert_eq!(name, TIMEOUT_PROBE_FIXTURE);
        assert_eq!(
            rule_label(rule),
            "partial_success_with_pipeline_timeout_error"
        );
        let errors = timeout_error();
        let partial = Reached::ConverterStatus {
            status: ConversionStatus::PartialSuccess,
            markdown: "",
        };
        assert_eq!(judge(rule, partial, &errors, true).outcome, "PASS");
        assert_eq!(
            judge(rule, partial, &[], true).outcome,
            "FAIL_timeout_not_honoured"
        );
        // A conversion that simply succeeded did not honour the budget.
        let success = Reached::ConverterStatus {
            status: ConversionStatus::Success,
            markdown: "text",
        };
        assert_eq!(
            judge(rule, success, &errors, true).outcome,
            "FAIL_timeout_not_honoured"
        );
        assert_eq!(
            judge(rule, Reached::ConverterError, &errors, true).outcome,
            "FAIL_timeout_not_honoured"
        );

        assert!(!timeout_honoured(Stage::ConverterError, None, &errors));
        assert!(!timeout_honoured(
            Stage::ConverterStatus,
            Some("Success"),
            &errors
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
            .map(str::to_owned)
            .chain(std::iter::once(MUST_FAIL_NAME.to_owned()))
            .collect();
        assert_eq!(harness, recorded);
        Ok(())
    }

    #[test]
    fn the_recorded_settings_are_the_ones_the_converter_holds() -> TestResult {
        let applied = Applied {
            artifacts_dir: "out/artifacts".to_owned(),
            document_timeout_ms: Some(1),
            generate_page_images: true,
            ocr_lang: "en",
        };
        let debug = format!("{:?}", build_converter(&applied));
        for held in [
            "generate_page_images: true",
            "images_scale: None",
            "ocr_lang: Some(\"en\")",
            "document_timeout: Some(1ms)",
            "artifacts_dir: \"out/artifacts\"",
        ] {
            let _at = some(debug.find(held), held)?;
        }
        let without = Applied {
            document_timeout_ms: None,
            generate_page_images: false,
            ..applied
        };
        let debug = format!("{:?}", build_converter(&without));
        assert!(debug.contains("generate_page_images: false"), "{debug}");
        assert!(debug.contains("document_timeout: None"), "{debug}");
        Ok(())
    }

    #[test]
    fn evidence_files_hold_what_the_receipt_hashes_and_page_images_leave_the_export() -> TestResult
    {
        let mut document = docling::DoclingDocument::new("probe");
        document.add_heading(1, "Probe heading");
        document.add_paragraph("Probe body.");
        let pixels = vec![7_u8, 8, 9];
        document.page_images.insert(
            2,
            PictureImage {
                mimetype: "image/png".to_owned(),
                width: 40,
                height: 30,
                data: pixels.clone(),
                dpi: 144,
            },
        );
        let mut result = ConversionResult {
            document,
            status: ConversionStatus::Success,
            input_name: "probe".to_owned(),
            format: docling::InputFormat::Md,
            errors: Vec::new(),
        };
        let out = tempfile::tempdir()?;
        let (evidence, markdown) = write_evidence(out.path(), &mut result)?;

        assert!(markdown.contains("Probe heading"), "{markdown}");
        assert!(evidence.markdown_nonempty);
        assert_eq!(evidence.markdown_chars, markdown.chars().count());
        let written_md = fs::read(out.path().join(DOCUMENT_MD))?;
        assert_eq!(written_md, markdown.as_bytes());
        assert_eq!(evidence.markdown_sha256, sha256_hex(&written_md));

        let written_json = fs::read(out.path().join(DOCUMENT_JSON))?;
        assert_eq!(evidence.json_sha256, sha256_hex(&written_json));
        assert_eq!(evidence.json_bytes, written_json.len());
        let export: serde_json::Value = serde_json::from_slice(&written_json)?;
        let texts = some(
            export.get("texts").and_then(serde_json::Value::as_array),
            "texts",
        )?;
        assert_eq!(texts.len(), 2);
        assert!(!String::from_utf8_lossy(&written_json).contains("base64"));

        let image = some(evidence.page_images.first(), "page image record")?;
        assert_eq!(evidence.page_images.len(), 1);
        assert_eq!(
            (image.page_no, image.width, image.height, image.dpi),
            (2, 40, 30, 144)
        );
        assert_eq!(image.file, "page-2.png");
        assert_eq!(image.bytes, pixels.len());
        assert_eq!(image.sha256, sha256_hex(&pixels));
        assert_eq!(fs::read(out.path().join(&image.file))?, pixels);
        assert!(result.document.page_images.is_empty());
        Ok(())
    }
}
