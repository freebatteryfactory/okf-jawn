//! Direct-library Docling qualification harness.
//!
//! Instantiates `docling::DocumentConverter` against real fixtures and assets.
//! Does not implement `okf_jawn_core::conversion::Converter` or any product port.

use docling::{ConversionStatus, DocumentConverter, SourceDocument};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Serialize)]
struct ErrorReceipt {
    component_type: String,
    error_message: String,
    module_name: String,
}

#[derive(Serialize)]
struct FixtureReceipt {
    converter_version: String,
    elapsed_ms: u128,
    errors: Vec<ErrorReceipt>,
    expected: String,
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
    /// The orchestrator may attach a process-level peak separately.
    peak_rss_bytes: Option<u64>,
    role: String,
    settings: BTreeMap<String, serde_json::Value>,
    sha256_after: String,
    sha256_before: String,
    status: String,
}

#[derive(Serialize)]
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

/// What a fixture must produce for the gate to pass.
#[derive(Clone, Copy)]
enum Expected {
    /// Supported extraction: Success, non-empty markdown, optional substring.
    SuccessNonEmpty { must_contain: Option<&'static str> },
    /// Must surface ConversionStatus::Failure (never empty Success).
    ExplicitFailure,
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn converter_version() -> String {
    format!(
        "{} {}",
        env!("CARGO_PKG_NAME"),
        option_env!("DOCLING_CRATE_VERSION").unwrap_or("1.93.5")
    )
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

fn judge_outcome(
    expected: Expected,
    status: ConversionStatus,
    markdown: &str,
    original_unchanged: bool,
) -> (String, Option<bool>) {
    if !original_unchanged {
        return ("FAIL_original_mutated".to_owned(), None);
    }
    let nonempty = !markdown.trim().is_empty();
    match expected {
        Expected::SuccessNonEmpty { must_contain } => {
            let contain_ok = must_contain.map(|needle| markdown.contains(needle));
            if matches!(status, ConversionStatus::Failure) {
                return ("FAIL_unexpected_failure".to_owned(), contain_ok);
            }
            if !nonempty {
                return ("FAIL_empty_markdown".to_owned(), contain_ok);
            }
            if let Some(false) = contain_ok {
                return ("FAIL_missing_expected_text".to_owned(), contain_ok);
            }
            if matches!(status, ConversionStatus::Success) {
                ("PASS".to_owned(), contain_ok)
            } else {
                // PartialSuccess with content is recorded but not a full pass for text fixtures.
                ("FAIL_partial_not_success".to_owned(), contain_ok)
            }
        }
        Expected::ExplicitFailure => {
            if matches!(status, ConversionStatus::Failure) {
                ("PASS_explicit_failure".to_owned(), None)
            } else if matches!(status, ConversionStatus::Success) && !nonempty {
                ("FAIL_empty_success".to_owned(), None)
            } else {
                ("FAIL_expected_failure".to_owned(), None)
            }
        }
    }
}

fn convert_fixture(
    converter: &DocumentConverter,
    path: &Path,
    role: &str,
    expected: Expected,
    settings: &BTreeMap<String, serde_json::Value>,
) -> Result<FixtureReceipt, String> {
    let sha_before = sha256_file(path)?;
    let started = Instant::now();
    let source = SourceDocument::from_file(path).map_err(|error| error.to_string())?;
    let result = converter
        .convert(source)
        .map_err(|error| error.to_string())?;
    let elapsed_ms = started.elapsed().as_millis();
    let sha_after = sha256_file(path)?;
    let markdown = result.document.export_to_markdown();
    let page_image_count = result.document.page_images.len();
    let provenance = page_provenance(&result);
    let errors: Vec<ErrorReceipt> = result
        .errors
        .iter()
        .map(|item| ErrorReceipt {
            component_type: item.component_type.clone(),
            error_message: item.error_message.clone(),
            module_name: item.module_name.clone(),
        })
        .collect();
    let status = status_label(result.status).to_owned();
    let original_unchanged = sha_before == sha_after;
    let (outcome, must_contain_ok) =
        judge_outcome(expected, result.status, &markdown, original_unchanged);
    Ok(FixtureReceipt {
        converter_version: converter_version(),
        elapsed_ms,
        errors,
        expected: expected_label(expected).to_owned(),
        fixture: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        markdown_chars: markdown.chars().count(),
        markdown_nonempty: !markdown.trim().is_empty(),
        must_contain_ok,
        original_unchanged,
        outcome,
        page_image_count,
        page_provenance: provenance,
        path: path.to_path_buf(),
        peak_rss_bytes: None,
        role: role.to_owned(),
        settings: settings.clone(),
        sha256_after: sha_after,
        sha256_before: sha_before,
        status,
    })
}

fn timeout_honoured(timeout_case: &FixtureReceipt) -> bool {
    timeout_case.errors.iter().any(|item| {
        item.module_name == "pipeline" && item.error_message.to_lowercase().contains("timeout")
    }) || (timeout_case.status == "PartialSuccess"
        && timeout_case
            .errors
            .iter()
            .any(|item| item.error_message.to_lowercase().contains("timeout")))
}

fn run() -> Result<(), String> {
    let models_dir = env::var("DOCLING_RS_MODELS_DIR")
        .map_err(|_| "DOCLING_RS_MODELS_DIR must be set to the verified models cache".to_owned())?;
    let fixtures_dir = env::var("OKF_DOCLING_FIXTURES").unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/documents")
            .to_string_lossy()
            .into_owned()
    });
    let out_dir = env::var("OKF_DOCLING_OUT").unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.artifacts/qualification/docling")
            .to_string_lossy()
            .into_owned()
    });
    fs::create_dir_all(&out_dir).map_err(|error| error.to_string())?;
    let artifacts = tempfile::tempdir().map_err(|error| error.to_string())?;

    let mut settings = BTreeMap::new();
    settings.insert(
        "artifacts_dir".to_owned(),
        serde_json::Value::String(artifacts.path().display().to_string()),
    );
    settings.insert(
        "models_dir".to_owned(),
        serde_json::Value::String(models_dir.clone()),
    );
    settings.insert(
        "ocr_lang".to_owned(),
        serde_json::Value::String("en".to_owned()),
    );

    let converter = DocumentConverter::new()
        .ocr_lang("en")
        .artifacts_dir(artifacts.path().display().to_string());

    // must_contain needles are stable tokens from the MIT fixtures / corpus notes.
    // Corpus fixtures added in the docling-rerun step keep the same needles where possible.
    let fixtures: [(&str, &str, Expected); 6] = [
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
    ];

    let mut receipts = Vec::new();
    let mut summary = BTreeMap::new();
    for (name, role, expected) in fixtures {
        let path = PathBuf::from(&fixtures_dir).join(name);
        if !path.is_file() {
            return Err(format!("missing fixture {}", path.display()));
        }
        let receipt = convert_fixture(&converter, &path, role, expected, &settings)?;
        summary.insert(name.to_owned(), receipt.outcome.clone());
        receipts.push(receipt);
    }

    // Optional must-fail fixture (added in docling-rerun). Absent is OK until that step.
    let fail_name = "must_fail_corrupt.bin";
    let fail_path = PathBuf::from(&fixtures_dir).join(fail_name);
    if fail_path.is_file() {
        let receipt = convert_fixture(
            &converter,
            &fail_path,
            "must_fail",
            Expected::ExplicitFailure,
            &settings,
        )?;
        summary.insert(fail_name.to_owned(), receipt.outcome.clone());
        receipts.push(receipt);
    }

    let timeout_converter = DocumentConverter::new()
        .ocr_lang("en")
        .artifacts_dir(artifacts.path().display().to_string())
        .document_timeout(Some(Duration::from_millis(1)));
    let mut timeout_settings = settings.clone();
    timeout_settings.insert(
        "document_timeout_ms".to_owned(),
        serde_json::Value::Number(1.into()),
    );
    let timeout_path = PathBuf::from(&fixtures_dir).join("scanned_image_only.pdf");
    let mut timeout_case = convert_fixture(
        &timeout_converter,
        &timeout_path,
        "timeout_probe",
        Expected::SuccessNonEmpty { must_contain: None },
        &timeout_settings,
    )?;
    let timeout_ok = timeout_honoured(&timeout_case);
    let timeout_outcome = if timeout_ok {
        "PASS".to_owned()
    } else {
        "FAIL_timeout_not_honoured".to_owned()
    };
    timeout_case.outcome = timeout_outcome.clone();
    summary.insert("timeout_probe".to_owned(), timeout_outcome);

    let report = QualificationReport {
        converter_crate: converter_version(),
        fixtures_dir: PathBuf::from(fixtures_dir),
        models_dir: PathBuf::from(models_dir),
        receipts,
        summary,
        timeout_case: Some(timeout_case),
    };
    let report_path = PathBuf::from(&out_dir).join("receipt.json");
    let json = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    fs::write(&report_path, json).map_err(|error| error.to_string())?;
    writeln!(io::stdout(), "Wrote {}", report_path.display()).map_err(|error| error.to_string())?;
    let failed = report
        .summary
        .values()
        .any(|value| value.starts_with("FAIL"));
    if failed {
        return Err(format!(
            "one or more fixtures failed qualification rules: {:?}",
            report.summary
        ));
    }
    Ok(())
}

fn main() -> Result<(), String> {
    run().inspect_err(|error| {
        let _ = writeln!(io::stderr(), "okf-qualify-docling: {error}");
    })
}
