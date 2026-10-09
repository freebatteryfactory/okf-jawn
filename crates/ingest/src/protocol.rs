//! The conversion child's protocol, spelled in this one module.
//!
//! The supervisor writes `REQUEST_FILE` into a new, empty output directory and runs the child
//! with that file's path as its only argument. The child writes its files into the same
//! directory and `RESULT_FILE` last, so a child that ended without `RESULT_FILE` did not report
//! (`cap::classify`). Until `serde` with `derive` reaches ingest (request R-I2, Batch B), both
//! sides read and write `serde_json::Value` here and nowhere else, so the move to derived types
//! touches only this module.

use std::path::PathBuf;
use std::time::Duration;

use okf_jawn_contract::common::{PageRange, TextRange};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{ConversionSettings, ConverterIssue};
use okf_jawn_core::conversion::ConversionStatus;
use serde_json::{Map, Value};

/// A contract part as JSON. A macro, not a function: ingest has `serde_json` but not `serde`,
/// so it cannot name the `Serialize` bound (R-I2).
macro_rules! to_json {
    ($part:expr) => {
        serde_json::to_value($part).map_err(|error| {
            ApiError::new(
                ErrorCode::Internal,
                format!("a conversion protocol part did not serialize: {error}"),
            )
        })
    };
}

/// A contract part read from JSON; a macro for the same reason as `to_json`.
macro_rules! from_json {
    ($value:expr, $name:expr) => {
        serde_json::from_value($value.clone()).map_err(|_| shape($name))
    };
}

/// What the supervisor asks of the child.
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// One-based inclusive lines of the window's Markdown.
    pub lines: TextRange,
    /// The page the node is on, from the converter's own page markers; `None` when the format
    /// has no pages.
    pub page: Option<u32>,
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
    /// The JSON the supervisor writes.
    ///
    /// # Errors
    /// Returns `Internal` when the settings cannot be represented as JSON.
    pub fn to_json(&self) -> Result<Value, ApiError> {
        let mut object = Map::new();
        let task = match self.task {
            Task::PageCount => "page_count",
            Task::Convert => "convert",
        };
        let _previous = object.insert("task".to_owned(), Value::from(task));
        let _previous = object.insert(
            "source".to_owned(),
            Value::from(self.source.to_string_lossy().into_owned()),
        );
        let _previous = object.insert("file_name".to_owned(), Value::from(self.file_name.clone()));
        let _previous = object.insert("settings".to_owned(), to_json!(&self.settings)?);
        let window = match &self.window {
            None => Value::Null,
            Some(window) => to_json!(window)?,
        };
        let _previous = object.insert("window".to_owned(), window);
        let _previous = object.insert(
            "timeout_ms".to_owned(),
            Value::from(u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX)),
        );
        Ok(Value::Object(object))
    }

    /// Read the JSON the supervisor wrote.
    ///
    /// # Errors
    /// Returns `InvalidInput` naming the first field that is missing or of the wrong shape.
    pub fn from_json(value: &Value) -> Result<Self, ApiError> {
        let task = match text(value, "task")? {
            "page_count" => Task::PageCount,
            "convert" => Task::Convert,
            _ => return Err(shape("task")),
        };
        let window = match value.get("window") {
            None | Some(Value::Null) => None,
            Some(window) => Some(from_json!(window, "window")?),
        };
        let timeout_ms = value
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .ok_or_else(|| shape("timeout_ms"))?;
        Ok(Self {
            task,
            source: PathBuf::from(text(value, "source")?),
            file_name: text(value, "file_name")?.to_owned(),
            settings: from_json!(field(value, "settings")?, "settings")?,
            window,
            timeout: Duration::from_millis(timeout_ms),
        })
    }
}

impl Reply {
    /// The JSON the child writes.
    ///
    /// # Errors
    /// Returns `Internal` when a part cannot be represented as JSON.
    pub fn to_json(&self) -> Result<Value, ApiError> {
        let mut object = Map::new();
        let (status, reason) = match &self.status {
            ConversionStatus::Success => ("success", Value::Null),
            ConversionStatus::PartialSuccess => ("partial_success", Value::Null),
            ConversionStatus::Unsupported => ("unsupported", Value::Null),
            ConversionStatus::Failure(reason) => ("failure", to_json!(reason)?),
        };
        let _previous = object.insert("status".to_owned(), Value::from(status));
        let _previous = object.insert("reason".to_owned(), reason);
        let _previous = object.insert(
            "page_count".to_owned(),
            self.page_count.map_or(Value::Null, Value::from),
        );
        let _previous = object.insert("issues".to_owned(), to_json!(&self.issues)?);
        let blocks = self
            .blocks
            .iter()
            .map(|block| {
                let mut entry = Map::new();
                let _previous = entry.insert("lines".to_owned(), to_json!(&block.lines)?);
                let _previous = entry.insert(
                    "page".to_owned(),
                    block.page.map_or(Value::Null, Value::from),
                );
                let _previous = entry.insert("table".to_owned(), Value::from(block.table));
                Ok(Value::Object(entry))
            })
            .collect::<Result<Vec<Value>, ApiError>>()?;
        let _previous = object.insert("blocks".to_owned(), Value::Array(blocks));
        let _previous = object.insert("text_layer".to_owned(), Value::from(self.text_layer));
        Ok(Value::Object(object))
    }

    /// Read the JSON the child wrote.
    ///
    /// # Errors
    /// Returns `Internal` naming the first field that is missing or of the wrong shape: a child
    /// that reports nonsense is a worker fault.
    pub fn from_json(value: &Value) -> Result<Self, ApiError> {
        let fault = |error: ApiError| ApiError::new(ErrorCode::Internal, error.message);
        let status = match text(value, "status").map_err(fault)? {
            "success" => ConversionStatus::Success,
            "partial_success" => ConversionStatus::PartialSuccess,
            "unsupported" => ConversionStatus::Unsupported,
            "failure" => ConversionStatus::Failure(
                from_json!(field(value, "reason").map_err(fault)?, "reason").map_err(fault)?,
            ),
            _ => return Err(fault(shape("status"))),
        };
        let page_count = match value.get("page_count") {
            None | Some(Value::Null) => None,
            Some(count) => Some(
                count
                    .as_u64()
                    .and_then(|count| u32::try_from(count).ok())
                    .ok_or_else(|| fault(shape("page_count")))?,
            ),
        };
        let blocks = value
            .get("blocks")
            .and_then(Value::as_array)
            .ok_or_else(|| fault(shape("blocks")))?
            .iter()
            .map(|entry| {
                Ok(Block {
                    lines: from_json!(field(entry, "lines")?, "lines")?,
                    page: entry
                        .get("page")
                        .and_then(Value::as_u64)
                        .and_then(|page| u32::try_from(page).ok()),
                    table: entry
                        .get("table")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| shape("table"))?,
                })
            })
            .collect::<Result<Vec<Block>, ApiError>>()
            .map_err(fault)?;
        Ok(Self {
            status,
            page_count,
            issues: from_json!(field(value, "issues").map_err(fault)?, "issues").map_err(fault)?,
            blocks,
            text_layer: value
                .get("text_layer")
                .and_then(Value::as_bool)
                .ok_or_else(|| fault(shape("text_layer")))?,
        })
    }
}

/// A field that must be present.
fn field<'a>(value: &'a Value, name: &str) -> Result<&'a Value, ApiError> {
    value.get(name).ok_or_else(|| shape(name))
}

/// A string field that must be present.
fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str, ApiError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| shape(name))
}

/// A field missing or of the wrong shape.
fn shape(name: &str) -> ApiError {
    ApiError::new(
        ErrorCode::InvalidInput,
        format!("the conversion protocol field `{name}` is missing or of the wrong shape"),
    )
    .with_field(format!("/{name}"))
}
