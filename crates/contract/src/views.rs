//! Persistent visual artifacts bind presentation to identifiable source data.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    /// Retained dataset bytes for reproducibility.
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentResponse {
    /// Validated presentation.
    pub view: ViewDocument,
    /// Exact data supplied for this render.
    pub resolved_bindings: Vec<ViewBinding>,
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

impl ViewDocument {
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
