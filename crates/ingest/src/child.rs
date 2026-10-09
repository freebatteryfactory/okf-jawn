//! The body of the conversion child (`bin/okf-jawn-convert`): it runs one `protocol::Request`
//! inside the capped child process and writes its files and `protocol::RESULT_FILE`.
//!
//! - `Task::PageCount` reads a PDF's page count with pdfium (`docling::pdf_page_count`); an
//!   image is one page; any other format is converted whole and has none.
//! - `Task::Convert` builds `DocumentConverter` from `ConversionSettings`, the window
//!   (`page_range`) and the time budget (`document_timeout`), and opens the source with
//!   `SourceDocument::from_bytes` in the format of the occurrence's file name. It writes the
//!   Markdown export, the JSON export and, for a PDF, the JSON export of the window's text layer
//!   (`docling::pdf_text_layer_pages`, read by the location rule).
//! - The line runs (`protocol::Block`): each top-level node is rendered on its own with
//!   `MarkdownStreamer::push`, whose chunks concatenate byte for byte to the Markdown export, so
//!   each node's lines and the page of the converter's last page marker are known. When the
//!   streamed text differs from the export the runs are left out rather than guessed.
//!
//! A document the converter cannot read is a reply (`Unsupported`, `Failure`), never a failing
//! exit. A failing exit without a reply is the child at fault, which the supervisor classifies.

use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

use docling::{
    ConversionError, DoclingDocument, DocumentConverter, ImageMode, InputFormat, MarkdownStreamer,
    Node, SourceDocument,
};
use okf_jawn_contract::common::TextRange;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{ConverterIssue, FailureReason, OcrPolicy};
use okf_jawn_core::conversion::ConversionStatus;

use crate::protocol::{
    Block, EXPORT_FILE, MARKDOWN_FILE, RESULT_FILE, Reply, Request, TEXT_LAYER_FILE, Task,
};

/// Run the request whose path is the first argument. Call after `cap::limit_self`.
#[must_use]
pub fn main() -> ExitCode {
    let Some(request) = std::env::args_os().nth(1) else {
        return refuse("okf-jawn-convert: the request file is the only argument");
    };
    match run(Path::new(&request)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => refuse(&format!("okf-jawn-convert: {}", error.message)),
    }
}

/// Read the request, carry it out, and write the reply last.
///
/// # Errors
/// Returns `Internal` when the request cannot be read or an output cannot be written.
pub fn run(request_path: &Path) -> Result<(), ApiError> {
    let directory = request_path
        .parent()
        .ok_or_else(|| fault("the request file has no directory"))?;
    let bytes = std::fs::read(request_path)
        .map_err(|error| fault(&format!("the request could not be read: {error}")))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| fault(&format!("the request is not JSON: {error}")))?;
    let request = Request::from_json(&value)?;
    let reply = match request.task {
        Task::PageCount => page_count(&request)?,
        Task::Convert => convert(&request, directory)?,
    };
    write_json(&directory.join(RESULT_FILE), &reply.to_json()?)
}

/// The input format the occurrence's file name selects.
#[must_use]
pub fn format_of(file_name: &str) -> Option<InputFormat> {
    Path::new(file_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .and_then(InputFormat::from_extension)
}

/// The line runs of `markdown`, one per top-level node of `document`, or none when the
/// streamed text is not exactly `markdown`.
#[must_use]
pub fn line_blocks(document: &DoclingDocument, markdown: &str) -> Vec<Block> {
    let mut renderer = MarkdownStreamer::new(
        document.strict_markdown,
        ImageMode::Placeholder,
        document.compact_tables,
    )
    .with_page_break_placeholder(document.page_break_placeholder.clone());
    let mut streamed = String::new();
    let mut blocks = Vec::new();
    let mut page: Option<u32> = None;
    for node in &document.nodes {
        if let Some(marked) = page_marker(node) {
            page = Some(marked);
        }
        let chunk = renderer.push(std::slice::from_ref(node), &[]);
        if chunk.is_empty() {
            continue;
        }
        let body = chunk.strip_prefix("\n\n").unwrap_or(&chunk);
        let lead = chunk.len().saturating_sub(body.len());
        let before = streamed
            .matches('\n')
            .count()
            .saturating_add(chunk.get(..lead).unwrap_or("").matches('\n').count());
        let start = u32::try_from(before.saturating_add(1)).unwrap_or(u32::MAX);
        let inner =
            u32::try_from(body.trim_end_matches('\n').matches('\n').count()).unwrap_or(u32::MAX);
        blocks.push(Block {
            lines: TextRange {
                start,
                end: start.saturating_add(inner),
            },
            page: node_page(node).or(page),
            table: is_table(node),
        });
        streamed.push_str(&chunk);
    }
    streamed.push_str(&renderer.finish());
    if streamed == markdown {
        blocks
    } else {
        Vec::new()
    }
}

/// Count the pages of a paginated original.
fn page_count(request: &Request) -> Result<Reply, ApiError> {
    let reply = |status, page_count, issues| Reply {
        status,
        page_count,
        issues,
        blocks: Vec::new(),
        text_layer: false,
    };
    match format_of(&request.file_name) {
        Some(InputFormat::Pdf) => {
            let bytes = read_source(request)?;
            Ok(match docling::pdf_page_count(&bytes, None) {
                Ok(count) => reply(
                    ConversionStatus::Success,
                    Some(u32::try_from(count).unwrap_or(u32::MAX)),
                    Vec::new(),
                ),
                // A PDF whose pages cannot be counted is converted whole, where the converter
                // records why it cannot be read.
                Err(error) => reply(
                    ConversionStatus::Failure(FailureReason::Damaged),
                    None,
                    vec![issue("document_backend", "page_count", &error.to_string())],
                ),
            })
        }
        Some(InputFormat::Image) => Ok(reply(ConversionStatus::Success, Some(1), Vec::new())),
        Some(_) | None => Ok(reply(ConversionStatus::Success, None, Vec::new())),
    }
}

/// Convert one window and write its outputs.
fn convert(request: &Request, directory: &Path) -> Result<Reply, ApiError> {
    let unconverted = |status, issues| Reply {
        status,
        page_count: None,
        issues,
        blocks: Vec::new(),
        text_layer: false,
    };
    let Some(format) = format_of(&request.file_name) else {
        return Ok(unconverted(ConversionStatus::Unsupported, Vec::new()));
    };
    let bytes = read_source(request)?;
    let source = SourceDocument::from_bytes(request.file_name.clone(), format, bytes.clone());
    let result = match converter(request).convert(source) {
        Ok(result) => result,
        Err(error) => {
            let status = failure_status(&error, request);
            let issues = vec![issue("document_backend", "convert", &error.to_string())];
            return Ok(unconverted(status, issues));
        }
    };
    let issues: Vec<ConverterIssue> = result
        .errors
        .iter()
        .map(|error| {
            issue(
                &error.component_type,
                &error.module_name,
                &error.error_message,
            )
        })
        .collect();
    let status = match result.status {
        docling::ConversionStatus::Success => ConversionStatus::Success,
        docling::ConversionStatus::PartialSuccess => ConversionStatus::PartialSuccess,
        docling::ConversionStatus::Failure => {
            return Ok(unconverted(
                ConversionStatus::Failure(FailureReason::ConverterError),
                issues,
            ));
        }
    };
    let markdown = result.document.export_to_markdown();
    write_text(&directory.join(MARKDOWN_FILE), &markdown)?;
    write_text(
        &directory.join(EXPORT_FILE),
        &result.document.export_to_json(),
    )?;
    let blocks = line_blocks(&result.document, &markdown);
    let text_layer = format == InputFormat::Pdf
        && match docling::pdf_text_layer_pages(
            &bytes,
            &request.file_name,
            request.window.as_ref().map(|window| {
                (
                    usize::try_from(window.start).unwrap_or(usize::MAX),
                    usize::try_from(window.end).unwrap_or(usize::MAX),
                )
            }),
        ) {
            Ok(layer) => {
                write_text(&directory.join(TEXT_LAYER_FILE), &layer.export_to_json())?;
                true
            }
            // Without a text layer every unlocated item stays unlocated (the rule's own
            // answer when the layer cannot be read).
            Err(_) => false,
        };
    Ok(Reply {
        status,
        page_count: None,
        issues,
        blocks,
        text_layer,
    })
}

/// The converter the settings, window and time budget describe.
fn converter(request: &Request) -> DocumentConverter {
    let settings = &request.settings;
    let mut converter = DocumentConverter::new()
        .document_timeout(Some(request.timeout))
        .no_table_former(!settings.table_structure)
        .generate_page_images(settings.page_images)
        .images_scale(f32::from(settings.page_image_dpi) / 72.0);
    converter = match settings.ocr {
        OcrPolicy::Auto => converter,
        OcrPolicy::Skip => converter.skip_ocr(true),
        OcrPolicy::ForceFullPage => converter.force_full_page_ocr(true),
    };
    if let Some(language) = &settings.ocr_language {
        converter = converter.ocr_lang(language.clone());
    }
    if let Some(window) = &request.window {
        converter = converter.page_range(
            usize::try_from(window.start).unwrap_or(usize::MAX),
            usize::try_from(window.end).unwrap_or(usize::MAX),
        );
    }
    converter
}

/// The status of a conversion the library refused with an error.
fn failure_status(error: &ConversionError, request: &Request) -> ConversionStatus {
    match error {
        ConversionError::UnknownFormat { .. } | ConversionError::UnsupportedFormat(_) => {
            ConversionStatus::Unsupported
        }
        ConversionError::Timeout(_) => ConversionStatus::Failure(FailureReason::TimeLimit {
            limit_seconds: u32::try_from(request.timeout.as_secs()).unwrap_or(u32::MAX),
        }),
        ConversionError::Parse(_) => ConversionStatus::Failure(FailureReason::Damaged),
        _ => ConversionStatus::Failure(FailureReason::ConverterError),
    }
}

/// The page a page marker or a located node names.
fn page_marker(node: &Node) -> Option<u32> {
    match node {
        Node::PageInfo { page_no, .. } if *page_no > 0 => u32::try_from(*page_no).ok(),
        _ => None,
    }
}

/// The page a node's own provenance names, looking through location wrappers.
fn node_page(node: &Node) -> Option<u32> {
    match node {
        Node::Prov { page_no, .. } if *page_no > 0 => u32::try_from(*page_no).ok(),
        Node::Located { inner, .. } => node_page(inner),
        _ => None,
    }
}

/// Whether a node is a table, looking through location wrappers.
fn is_table(node: &Node) -> bool {
    match node {
        Node::Table(_) => true,
        Node::Located { inner, .. } | Node::Prov { inner, .. } => is_table(inner),
        _ => false,
    }
}

/// The retained original's bytes.
fn read_source(request: &Request) -> Result<Vec<u8>, ApiError> {
    std::fs::read(&request.source)
        .map_err(|error| fault(&format!("the retained original could not be read: {error}")))
}

/// One converter issue.
fn issue(component_type: &str, module_name: &str, message: &str) -> ConverterIssue {
    ConverterIssue {
        component_type: component_type.to_owned(),
        module_name: module_name.to_owned(),
        error_message: message.to_owned(),
    }
}

/// Write a text output.
fn write_text(path: &Path, text: &str) -> Result<(), ApiError> {
    std::fs::write(path, text)
        .map_err(|error| fault(&format!("{} could not be written: {error}", path.display())))
}

/// Write a JSON output.
fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), ApiError> {
    let text = serde_json::to_string(value)
        .map_err(|error| fault(&format!("a reply did not serialize: {error}")))?;
    write_text(path, &text)
}

/// Say why the child stops, and fail.
fn refuse(message: &str) -> ExitCode {
    let _written = writeln!(std::io::stderr(), "{message}");
    ExitCode::FAILURE
}

/// A worker fault.
fn fault(message: &str) -> ApiError {
    ApiError::new(ErrorCode::Internal, message)
}
