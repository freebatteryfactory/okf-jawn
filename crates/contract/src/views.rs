//! Persistent visual artifacts bind presentation to identifiable source data.
//!
//! A chart reads only a `Dataset` the server resolved from cited application data; a chart
//! that cannot be drawn fails alone (`ChartStatus::Failed`) while the View's text and other
//! charts still render.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ErrorCode};

/// The data kind of one column.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ColumnKind {
    /// Text.
    String,
    /// A real number.
    Number,
    /// A whole number.
    Integer,
    /// True or false.
    Boolean,
    /// A UTC instant in the canonical `Timestamp` spelling; an ambiguous document date stays a
    /// `string` (SPEC section 3).
    DateTime,
}

/// One column of a dataset.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DatasetColumn {
    /// Field name a chart encodes.
    pub name: String,
    /// Data kind.
    pub kind: ColumnKind,
    /// Unit, such as `EUR` or `kg`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// One cell: a JSON scalar of its column's kind, or null. Untagged because a tag per cell would
/// multiply the dataset's size; the only untagged enum on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum DatasetValue {
    /// Missing.
    Null,
    /// A boolean.
    Boolean(bool),
    /// A number, kept exact.
    Number(serde_json::Number),
    /// Text, or a `Timestamp` for a `date_time` column.
    Text(String),
}

/// A typed table resolved from cited application data; its canonical JSON bytes are the blob
/// `ViewBinding::materialized` names.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    /// Format version, currently 1.
    pub schema_version: u32,
    /// The exact citation the rows were read from.
    pub source: crate::source::SourceReference,
    /// Whose text the values were read from.
    pub text_origin: crate::extraction::TextOrigin,
    /// Columns, in order.
    #[schemars(length(min = 1, max = 1024))]
    pub columns: Vec<DatasetColumn>,
    /// Rows; each has exactly one value per column.
    #[schemars(length(max = 100_000))]
    pub rows: Vec<Vec<DatasetValue>>,
    /// Extraction warnings of the source that bear on these values, such as
    /// `cell_values_only` for a dataset read from spreadsheet cells.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<crate::extraction::ExtractionWarning>,
}

/// Which chart of a View a result is about.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChartRef {
    /// The View's own `spec`, for `vega_lite` grammar.
    Spec,
    /// A named entry of `ViewDocument::charts`, for `json_render` grammar.
    Named {
        /// The key in `charts`.
        name: String,
    },
}

/// Why one chart cannot be drawn.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChartFailure {
    /// The Vega-Lite specification fails server validation.
    InvalidSpec,
    /// The chart reads a dataset name no binding declares.
    UnknownBinding,
    /// A binding's data could not be resolved or materialized.
    DatasetUnavailable,
    /// A binding cites purged content.
    Invalidated,
    /// The dataset exceeds a row, column, cell or column-name size bound.
    TooLarge,
}

/// The state of one chart in a present or resolve result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChartStatus {
    /// Every dataset it reads is materialized.
    Ready {
        /// Binding names it reads.
        bindings: Vec<String>,
    },
    /// It cannot be drawn; the View's text and other charts are unaffected.
    Failed {
        /// Typed cause.
        reason: ChartFailure,
        /// The binding at fault, when one is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        binding: Option<String>,
        /// Safe explanation for the chart's own alert.
        message: String,
    },
}

/// One chart and its state.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChartResult {
    /// Which chart.
    pub chart: ChartRef,
    /// Its state.
    pub status: ChartStatus,
}

/// Pinned historical views and explicit live refresh have different semantics.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    /// All bindings reference retained immutable source revisions.
    Pinned,
    /// Bindings request current sources and disclose each resolved revision.
    Live,
}

/// Supported declarative presentation formats.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RenderGrammar {
    /// Charts over application-resolved named datasets.
    VegaLite,
    /// Compositions of the approved component catalog.
    JsonRender,
}

/// A named source selection used by a chart or layout.
///
/// A saved View binds only to sources in its own workspace: `source.workspace_id` must equal
/// the workspace the View is saved or presented in (`ViewDocument::bindings_outside`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewBinding {
    /// Dataset or component binding name.
    pub name: String,
    /// Resolved cited input.
    pub source: crate::source::SourceReference,
    /// Column or field units.
    pub units: std::collections::BTreeMap<String, String>,
    /// Declared allowed data transformations; validated against the selected grammar.
    pub transforms: Vec<serde_json::Value>,
    /// Retained `Dataset` bytes for reproducibility; in a present or resolve result, absent
    /// for a binding whose dataset could not be produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialized: Option<crate::identity::Digest>,
}

/// An editable visual artifact with prose and explicit source bindings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewDocument {
    /// App-owned format version, currently 1.
    pub schema_version: u32,
    /// Human-readable visual title.
    pub title: String,
    /// Readable explanation independent of the renderer.
    pub description: String,
    /// Pinned or explicitly live.
    pub mode: ViewMode,
    /// Declarative renderer grammar.
    pub grammar: RenderGrammar,
    /// Cited input datasets and passages.
    pub bindings: Vec<ViewBinding>,
    /// Validated renderer specification with no arbitrary code or external fetching.
    pub spec: serde_json::Value,
    /// Named Vega-Lite specs referenced by `json_render` Chart components; empty for `vega_lite`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub charts: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Render a candidate from already resolved bindings without saving or approving it.
///
/// Every binding must name `workspace_id`. Dispatch refuses a view for which
/// `view.bindings_outside(workspace_id)` is not empty as `invalid_input`, before authorization
/// and before the handler runs, whatever the caller may read: reading another workspace is not
/// permission to republish its data here. `workspace_id` is the only authorization target.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Candidate visual composition.
    pub view: ViewDocument,
}

/// A candidate presentation and the exact sources available to its UI.
///
/// A View-level fault (schema, a binding outside the workspace) refuses the whole request; a
/// chart-level fault never does and is reported in `charts`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentResponse {
    /// Validated presentation.
    pub view: ViewDocument,
    /// Exact data supplied for this render; each binding's `source.revision` is the resolved
    /// revision.
    pub resolved_bindings: Vec<ViewBinding>,
    /// Exactly one per entry of `view.chart_refs()`, in that order.
    pub charts: Vec<ChartResult>,
    /// When the bindings were resolved; for a pinned View, when the pinned inputs were read.
    pub as_of: crate::identity::Timestamp,
    /// Limits or freshness information.
    pub warnings: Vec<crate::common::Warning>,
    /// Tool response record.
    pub receipt_id: crate::identity::ReceiptId,
}

/// Read a stored visual artifact at its selected revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetViewRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
}

/// Resolve and materialize a View without silently changing its saved specification.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolveViewRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Only live views may request current input resolution.
    pub refresh_live: bool,
}

/// Export the visual specification, sources, data table, and rendering assets.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportViewRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Read the approved component and action catalog for visual proposals.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetCatalogRequest {}

/// A renderer component with typed props and named supported actions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogComponent {
    /// Canonical component name.
    pub name: String,
    /// When to use it.
    pub description: String,
    /// JSON Schema for props.
    pub properties_schema: serde_json::Value,
    /// Allowed existing operations.
    pub actions: Vec<String>,
}

/// The presentation vocabulary; it grants no permissions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogResponse {
    /// Catalog version.
    pub schema_version: u32,
    /// Approved components.
    pub components: Vec<CatalogComponent>,
    /// Schema for persisted View documents.
    pub view_schema: serde_json::Value,
}

impl Dataset {
    /// Refuse a dataset whose rows do not fit its columns.
    ///
    /// # Errors
    /// Returns `InvalidInput` on `/rows/<row>` for a row of the wrong width and on
    /// `/rows/<row>/<column>` for a value that is neither null nor of its column's kind,
    /// including a `date_time` value that is not a canonical `Timestamp`.
    pub fn check(&self) -> Result<(), ApiError> {
        let width = self.columns.len();
        for (row_index, row) in self.rows.iter().enumerate() {
            if row.len() != width {
                return Err(ApiError::new(
                    ErrorCode::InvalidInput,
                    format!("a dataset row has {} values for {width} columns", row.len()),
                )
                .with_field(format!("/rows/{row_index}")));
            }
            for (column_index, (value, column)) in row.iter().zip(&self.columns).enumerate() {
                if !value.fits(column.kind) {
                    return Err(ApiError::new(
                        ErrorCode::InvalidInput,
                        format!("a value of column {} is not of its kind", column.name),
                    )
                    .with_field(format!("/rows/{row_index}/{column_index}")));
                }
            }
        }
        Ok(())
    }
}

impl DatasetValue {
    /// Whether this value is null or of `kind`.
    #[must_use]
    pub fn fits(&self, kind: ColumnKind) -> bool {
        match (self, kind) {
            (Self::Null, _)
            | (Self::Boolean(_), ColumnKind::Boolean)
            | (Self::Number(_), ColumnKind::Number)
            | (Self::Text(_), ColumnKind::String) => true,
            (Self::Number(number), ColumnKind::Integer) => number.is_i64() || number.is_u64(),
            (Self::Text(text), ColumnKind::DateTime) => {
                crate::identity::Timestamp::try_from(text.clone()).is_ok()
            }
            _ => false,
        }
    }
}

impl ViewDocument {
    /// The charts this View draws: `[Spec]` for `vega_lite`, one `Named` per `charts` key for
    /// `json_render`, in key order.
    #[must_use]
    pub fn chart_refs(&self) -> Vec<ChartRef> {
        match self.grammar {
            RenderGrammar::VegaLite => vec![ChartRef::Spec],
            RenderGrammar::JsonRender => self
                .charts
                .keys()
                .map(|name| ChartRef::Named { name: name.clone() })
                .collect(),
        }
    }

    /// Bindings whose source lives outside `workspace`; a saved or presented View must have none.
    #[must_use]
    pub fn bindings_outside(&self, workspace: crate::identity::WorkspaceId) -> Vec<&ViewBinding> {
        self.bindings
            .iter()
            .filter(|binding| binding.source.workspace_id != workspace)
            .collect()
    }

    /// Refuse this View when any binding lies outside `workspace` (SPEC section 10).
    ///
    /// `pointer` is the JSON Pointer of this View inside its request; the refusal names the
    /// `bindings` field under it. The refusal does not depend on grants: no permission on the
    /// other workspace makes the binding valid.
    ///
    /// # Errors
    /// Returns `InvalidInput` when `bindings_outside(workspace)` is not empty.
    pub fn require_own_workspace(
        &self,
        workspace: crate::identity::WorkspaceId,
        pointer: &str,
    ) -> Result<(), crate::error::ApiError> {
        let foreign = self.bindings_outside(workspace).len();
        if foreign == 0 {
            return Ok(());
        }
        Err(crate::error::ApiError::new(
            crate::error::ErrorCode::InvalidInput,
            format!(
                "A View binds only to sources in its own workspace; {foreign} binding(s) name another workspace"
            ),
        )
        .with_field(format!("{pointer}/bindings")))
    }
}
