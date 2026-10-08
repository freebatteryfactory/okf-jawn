//! Shared wire values, pagination, and bounded text locations.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An empty request object.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

/// A bounded page request; the cursor is opaque and scoped to the query.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageRequest {
    /// Opaque continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Maximum results requested; server may return fewer.
    #[schemars(range(min = 1, max = 200))]
    pub limit: u16,
}

/// A one-based inclusive line range.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TextRange {
    /// First line, starting at one.
    #[schemars(range(min = 1))]
    pub start: u32,
    /// Last line, at least start.
    #[schemars(range(min = 1))]
    pub end: u32,
}

/// A one-based inclusive document page range.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PageRange {
    /// First page.
    #[schemars(range(min = 1))]
    pub start: u32,
    /// Last page.
    #[schemars(range(min = 1))]
    pub end: u32,
}

/// A one-based inclusive rectangular spreadsheet selection; the XLSX source locator.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CellRange {
    /// Exact sheet name.
    pub sheet: String,
    /// First row, inclusive, starting at one.
    #[schemars(range(min = 1))]
    pub row_start: u32,
    /// Last row, inclusive.
    #[schemars(range(min = 1))]
    pub row_end: u32,
    /// First column, inclusive, starting at one.
    #[schemars(range(min = 1))]
    pub column_start: u32,
    /// Last column, inclusive.
    #[schemars(range(min = 1))]
    pub column_end: u32,
}

/// A diagnostic that does not silently discard usable input.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Warning {
    /// Stable diagnostic identifier.
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
    /// Page, cell, path, or field locator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

/// A persisted change and the resolved revision it produced.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MutationResult {
    /// New committed workspace revision.
    pub revision: crate::identity::Revision,
    /// Durable application operation receipt.
    pub receipt_id: crate::identity::ReceiptId,
    /// Non-fatal issues recorded during the operation.
    pub warnings: Vec<Warning>,
}
