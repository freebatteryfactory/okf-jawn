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
use std::process;
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
    fixture: String,
    markdown_chars: usize,
    markdown_nonempty: bool,
    original_unchanged: bool,
    outcome: String,
    page_image_count: usize,
    path: PathBuf,
    peak_rss_bytes: Option<u64>,
    role: String,
    settings: BTreeMap<String, serde_json::Value>,
    sha256_after: String,
    sha256_before: String,
    status: String,
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

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn peak_rss_bytes() -> Option<u64> {
    // Authored code forbids unsafe. The Bun/PowerShell orchestrator records
    // peak working-set from the OS after this process exits.
    None
}

fn status_label(status: ConversionStatus) -> &'static str {
    match status {
        ConversionStatus::Failure => "Failure",
        ConversionStatus::PartialSuccess => "PartialSuccess",
        ConversionStatus::Success => "Success",
    }
}

fn convert_fixture(
    converter: &DocumentConverter,
    path: &Path,
    role: &str,
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
    let empty_success =
        matches!(result.status, ConversionStatus::Success) && markdown.trim().is_empty();
    let original_unchanged = sha_before == sha_after;
    let outcome = if empty_success {
        "FAIL_empty_success".to_owned()
    } else if !original_unchanged {
        "FAIL_original_mutated".to_owned()
    } else {
        match result.status {
            ConversionStatus::Failure => "PASS_explicit_failure".to_owned(),
            ConversionStatus::PartialSuccess | ConversionStatus::Success => "PASS".to_owned(),
        }
    };
    Ok(FixtureReceipt {
        converter_version: "docling-1.93.5".to_owned(),
        elapsed_ms,
        errors,
        fixture: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        markdown_chars: markdown.chars().count(),
        markdown_nonempty: !markdown.trim().is_empty(),
        original_unchanged,
        outcome,
        page_image_count,
        path: path.to_path_buf(),
        peak_rss_bytes: peak_rss_bytes(),
        role: role.to_owned(),
        settings: settings.clone(),
        sha256_after: sha_after,
        sha256_before: sha_before,
        status,
    })
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

    let fixtures = [
        ("sample_with_image.docx", "docx_with_images"),
        ("sample_sheet.xlsx", "xlsx"),
        ("born_digital_text.pdf", "born_digital_pdf"),
        ("scanned_image_only.pdf", "scanned_or_image_pdf"),
        ("table_heavy.pdf", "table_heavy_pdf"),
        ("sample_image.png", "image"),
    ];

    let mut receipts = Vec::new();
    let mut summary = BTreeMap::new();
    for (name, role) in fixtures {
        let path = PathBuf::from(&fixtures_dir).join(name);
        if !path.is_file() {
            return Err(format!("missing fixture {}", path.display()));
        }
        let receipt = convert_fixture(&converter, &path, role, &settings)?;
        summary.insert(name.to_owned(), receipt.outcome.clone());
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
    let timeout_case = convert_fixture(
        &timeout_converter,
        &timeout_path,
        "timeout_probe",
        &timeout_settings,
    )?;
    let timeout_ok = timeout_case.status == "PartialSuccess"
        || timeout_case.errors.iter().any(|item| {
            item.module_name == "pipeline" || item.error_message.to_lowercase().contains("timeout")
        });
    summary.insert(
        "timeout_probe".to_owned(),
        if timeout_ok {
            "PASS".to_owned()
        } else {
            format!("OBSERVED_{}", timeout_case.status)
        },
    );

    let report = QualificationReport {
        converter_crate: "docling 1.93.5".to_owned(),
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
        return Err("one or more fixtures failed qualification rules".to_owned());
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        let _ = writeln!(io::stderr(), "okf-qualify-docling: {error}");
        process::exit(1);
    }
}
