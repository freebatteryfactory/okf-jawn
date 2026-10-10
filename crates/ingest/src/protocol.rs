//! The conversion child's protocol, spelled in this one module.
//!
//! The supervisor writes `REQUEST_FILE` into a new, empty output directory and runs the child
//! with that file's path as its only argument. The child writes its files into the same
//! directory and `RESULT_FILE` last, so a child that ended without `RESULT_FILE` did not report
//! (`cap::classify`).
//!
//! Both files are JSON of derived serde types that refuse a field they do not know
//! (`deny_unknown_fields`), so a request or reply of another shape is refused rather than read
//! with a part silently dropped.

use std::path::PathBuf;
use std::time::Duration;

use okf_jawn_contract::common::{PageRange, TextRange};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{ConversionSettings, ConverterIssue, FailureReason};
use okf_jawn_core::conversion::ConversionStatus;
use serde::{Deserialize, Serialize};

/// What the supervisor asks of the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    /// Count the pages of a paginated original.
    PageCount,
    /// Convert one window.
    Convert,
}

/// One request to the child.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// What to do.
    pub task: Task,
    /// The retained original; its path has no extension.
    pub source: PathBuf,
    /// The occurrence's file name, whose extension selects the format.
    pub file_name: String,
    /// Contract settings.
    pub settings: ConversionSettings,
    /// One-based inclusive page window; `None` converts the whole document.
    pub window: Option<PageRange>,
    /// The window's time budget (docling `document_timeout`).
    pub timeout: Duration,
}

/// A run of Markdown lines that one top-level document node rendered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    /// One-based inclusive lines of the window's Markdown.
    pub lines: TextRange,
    /// The page of the node's own provenance, which the converter gave it: a provenance
    /// wrapper's page, or a location (a location wrapper or the node's own location field) on
    /// the page of the converter's current page marker, exactly when docling's JSON export gives
    /// the node's item a `prov`. `None` when the node has no provenance of its own, or the
    /// format has no pages; a preceding page marker alone never locates a node.
    pub page: Option<u32>,
    /// For a node on a page that has no provenance of its own: the text of its first item in
    /// docling's JSON export, so the supervisor can pair the run with that item's result from
    /// the location rule. `None` otherwise.
    pub unlocated: Option<String>,
    /// The node is a table.
    pub table: bool,
}

/// What the child reports in `RESULT_FILE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// How the task ended. `Success` for a page count that was read.
    pub status: ConversionStatus,
    /// The page count, for `Task::PageCount`; `None` for a format converted whole.
    pub page_count: Option<u32>,
    /// What the converter recorded.
    pub issues: Vec<ConverterIssue>,
    /// The line runs of the Markdown, in order; empty when no document was produced or the
    /// streamed Markdown did not match the exported Markdown byte for byte.
    pub blocks: Vec<Block>,
    /// `TEXT_LAYER_FILE` was written (a PDF window).
    pub text_layer: bool,
}

/// The request as it is written: the time budget in whole milliseconds.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestFile {
    task: Task,
    source: PathBuf,
    file_name: String,
    settings: ConversionSettings,
    window: Option<PageRange>,
    timeout_ms: u64,
}

/// The reply as it is written: the status as a word, with the failure's reason beside it.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyFile {
    status: StatusWord,
    reason: Option<FailureReason>,
    page_count: Option<u32>,
    issues: Vec<ConverterIssue>,
    blocks: Vec<Block>,
    text_layer: bool,
}

/// `ConversionStatus` without its reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StatusWord {
    Success,
    PartialSuccess,
    Unsupported,
    Failure,
}

/// The request the supervisor writes.
pub const REQUEST_FILE: &str = "request.json";
/// The reply the child writes last.
pub const RESULT_FILE: &str = "result.json";
/// The window's Markdown, as docling exports it.
pub const MARKDOWN_FILE: &str = "document.md";
/// The window's docling JSON export.
pub const EXPORT_FILE: &str = "document.json";
/// The docling JSON export of the window's PDF text layer, for the location rule.
pub const TEXT_LAYER_FILE: &str = "text_layer.json";

impl Request {
    /// The bytes the supervisor writes.
    ///
    /// # Errors
    /// Returns `Internal` when the request cannot be written as JSON (a source path that is not
    /// Unicode).
    pub fn encode(&self) -> Result<Vec<u8>, ApiError> {
        serde_json::to_vec(&RequestFile {
            task: self.task,
            source: self.source.clone(),
            file_name: self.file_name.clone(),
            settings: self.settings.clone(),
            window: self.window.clone(),
            timeout_ms: u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX),
        })
        .map_err(|error| fault(&format!("a conversion request did not serialize: {error}")))
    }

    /// Read the bytes the supervisor wrote.
    ///
    /// # Errors
    /// Returns `InvalidInput` naming what is missing, unknown or of the wrong shape.
    pub fn decode(bytes: &[u8]) -> Result<Self, ApiError> {
        let file: RequestFile = serde_json::from_slice(bytes).map_err(|error| {
            ApiError::new(
                ErrorCode::InvalidInput,
                format!("the conversion request is not of the protocol's shape: {error}"),
            )
        })?;
        Ok(Self {
            task: file.task,
            source: file.source,
            file_name: file.file_name,
            settings: file.settings,
            window: file.window,
            timeout: Duration::from_millis(file.timeout_ms),
        })
    }
}

impl Reply {
    /// The bytes the child writes.
    ///
    /// # Errors
    /// Returns `Internal` when a part cannot be written as JSON.
    pub fn encode(&self) -> Result<Vec<u8>, ApiError> {
        let (status, reason) = match &self.status {
            ConversionStatus::Success => (StatusWord::Success, None),
            ConversionStatus::PartialSuccess => (StatusWord::PartialSuccess, None),
            ConversionStatus::Unsupported => (StatusWord::Unsupported, None),
            ConversionStatus::Failure(reason) => (StatusWord::Failure, Some(reason.clone())),
        };
        serde_json::to_vec(&ReplyFile {
            status,
            reason,
            page_count: self.page_count,
            issues: self.issues.clone(),
            blocks: self.blocks.clone(),
            text_layer: self.text_layer,
        })
        .map_err(|error| fault(&format!("a conversion reply did not serialize: {error}")))
    }

    /// Read the bytes the child wrote.
    ///
    /// # Errors
    /// Returns `Internal` naming what is missing, unknown or of the wrong shape, and when a
    /// failure carries no reason or another status carries one: a child that reports nonsense
    /// is a worker fault.
    pub fn decode(bytes: &[u8]) -> Result<Self, ApiError> {
        let file: ReplyFile = serde_json::from_slice(bytes).map_err(|error| {
            fault(&format!(
                "the conversion reply is not of the protocol's shape: {error}"
            ))
        })?;
        let status = match (file.status, file.reason) {
            (StatusWord::Success, None) => ConversionStatus::Success,
            (StatusWord::PartialSuccess, None) => ConversionStatus::PartialSuccess,
            (StatusWord::Unsupported, None) => ConversionStatus::Unsupported,
            (StatusWord::Failure, Some(reason)) => ConversionStatus::Failure(reason),
            (StatusWord::Failure, None) => {
                return Err(fault("a failed conversion reply names no reason"));
            }
            (StatusWord::Success | StatusWord::PartialSuccess | StatusWord::Unsupported, _) => {
                return Err(fault("only a failed conversion reply names a reason"));
            }
        };
        Ok(Self {
            status,
            page_count: file.page_count,
            issues: file.issues,
            blocks: file.blocks,
            text_layer: file.text_layer,
        })
    }
}

/// A worker fault.
fn fault(message: &str) -> ApiError {
    ApiError::new(ErrorCode::Internal, message)
}
