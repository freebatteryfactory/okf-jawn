//! User-defined OKF types, editable notes, and source appearances.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Built-in presentation roles; user-defined OKF type names remain unrestricted.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// Human or agent-authored Markdown knowledge.
    Note,
    /// An appearance of immutable source bytes.
    Source,
    /// A source-bound declarative visual artifact.
    View,
}

/// Application lifecycle distinct from business approval.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    /// Current and usable content.
    Active,
    /// Retained historical content.
    Deprecated,
    /// Hidden from ordinary current listings; still retrievable.
    Archived,
}

/// A navigable item description bound to a resolved workspace revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ItemSummary {
    /// Stable identity.
    pub id: crate::identity::ItemId,
    /// Current relative OKF path.
    pub path: crate::identity::WorkspacePath,
    /// Human-readable title.
    pub title: String,
    /// One-line navigation summary.
    pub description: String,
    /// User-selected OKF type; not a closed enum.
    pub type_name: String,
    /// Built-in rendering role.
    pub kind: ItemKind,
    /// Resolved revision for this summary.
    pub revision: crate::identity::Revision,
    /// Presentation lifecycle.
    pub lifecycle: Lifecycle,
    /// Original MIME type if there is a source object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

/// Editable Markdown and preserved frontmatter extension properties.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ItemDocument {
    /// Identity and navigation metadata.
    pub summary: ItemSummary,
    /// Markdown body, preserving source locators.
    pub body: String,
    /// All user and OKF properties; unknown extension values are retained.
    pub properties: std::collections::BTreeMap<String, serde_json::Value>,
    /// Source occurrence metadata when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::source::SourceAppearance>,
}

/// List one folder with descriptions instead of loading all document bodies.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListItemsRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Empty string for the root; otherwise a validated relative folder.
    pub folder: String,
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// A folder listing at one resolved revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListItemsResponse {
    /// One revision resolved for the whole listing.
    pub revision: crate::identity::Revision,
    /// Visible child items.
    pub items: Vec<ItemSummary>,
    /// Child folder paths.
    pub folders: Vec<String>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Read the editable representation of an item at a selected revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GetItemRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
}

/// Create a note or View without altering source bytes.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateItemRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Destination relative path.
    pub path: crate::identity::WorkspacePath,
    /// Display title.
    pub title: String,
    /// User-selected OKF type.
    pub type_name: String,
    /// Built-in role.
    pub kind: ItemKind,
    /// Markdown content.
    pub body: String,
    /// Preserved extension properties.
    pub properties: std::collections::BTreeMap<String, serde_json::Value>,
    /// Retry identity.
    pub idempotency_key: String,
}

/// Edit content against its base revision, invalidating prior review coverage.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateItemRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Replacement Markdown.
    pub body: String,
    /// Complete preserved property map.
    pub properties: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Move an item and rewrite internal references in one versioned operation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveItemRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// New relative path.
    pub destination: crate::identity::WorkspacePath,
}

/// Retire or reactivate an item without claiming a business decision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetLifecycleRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Requested lifecycle.
    pub lifecycle: Lifecycle,
}

/// Remove the active item reference while retaining historical objects.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteItemRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
}

/// Create a user-selected folder and its navigable index.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateFolderRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// New relative folder path.
    pub folder: crate::identity::WorkspacePath,
}

/// A user-defined type schema and UI hints.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TypeDefinition {
    /// Exact user-facing OKF type name.
    pub name: String,
    /// Application format version.
    pub schema_version: u32,
    /// JSON Schema for type-specific properties.
    pub properties_schema: serde_json::Value,
    /// Presentation hints; never authorization rules.
    pub ui_schema: serde_json::Value,
}

/// List type definitions available in a workspace.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTypesRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
}

/// User and built-in type definitions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTypesResponse {
    /// Available property schemas.
    pub items: Vec<TypeDefinition>,
}

/// Save a type definition without discarding existing extension properties.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetTypeRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Validated property schema and UI hints.
    pub definition: TypeDefinition,
}
