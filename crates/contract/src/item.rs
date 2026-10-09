//! User-defined OKF types, editable notes, and source appearances.
//!
//! Every item file carries one application-owned header mapping under `APP_HEADER_KEY`: the
//! item id, the archived flag when set, and on a source card the original name and digest and
//! the extraction. The server owns it; a write that changes it is refused.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Built-in presentation roles; user-defined OKF type names remain unrestricted.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// Human or agent-authored Markdown knowledge.
    Note,
    /// An appearance of immutable source bytes.
    Source,
    /// A source-bound declarative visual artifact.
    View,
}

/// OKF's `status` word of an item, distinct from business approval and from archiving.
///
/// An absent OKF `status` reads `stable`. `other` is a producer's value outside the three,
/// kept exactly as written in the file's `status` property; it cannot be set through
/// `set_lifecycle`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// Work in progress.
    Draft,
    /// Current and usable content.
    Stable,
    /// Retained, superseded content; still searchable.
    Deprecated,
    /// A status word outside the three, preserved in the file.
    Other,
}

/// A navigable item description bound to a resolved workspace revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    /// OKF status word.
    pub status: ItemStatus,
    /// Hidden from ordinary current listings; reversible and keeps history.
    pub archived: bool,
    /// Original MIME type if there is a source object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The converter's verdict and whose text is shown; present exactly when `kind` is `source`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction: Option<crate::extraction::ExtractionSummary>,
}

/// Editable Markdown and preserved frontmatter extension properties.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ItemDocument {
    /// Identity and navigation metadata.
    pub summary: ItemSummary,
    /// Markdown body, preserving source locators.
    pub body: String,
    /// All user and OKF properties; unknown extension values are retained. The server-owned
    /// application header appears under `APP_HEADER_KEY`.
    pub properties: std::collections::BTreeMap<String, serde_json::Value>,
    /// SHA-256 of the item's body and properties without the server-owned header, keys sorted.
    /// It is the value a confirmation and a review cite, so a client never computes it; the
    /// server computes it with `okf_jawn_core::portable::item_content_digest`.
    pub content_digest: crate::identity::Digest,
    /// Source occurrence metadata when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::source::SourceAppearance>,
    /// The caller's own uncommitted draft; never another editor's, and never on agent routes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftContent>,
}

/// Saved draft metadata; one per (item, editor), and never a revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    /// Drafted item.
    pub item_id: crate::identity::ItemId,
    /// Server-established subject who owns this draft.
    pub editor: String,
    /// Head revision the draft is based on; a Snapshot conflicts if the item changed since.
    pub base_revision: crate::identity::Revision,
    /// Digest of the drafted body and properties.
    pub content_digest: crate::identity::Digest,
    /// Time of the latest save.
    pub saved_at: crate::identity::Timestamp,
}

/// The caller's own draft content returned beside committed content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftContent {
    /// Draft metadata.
    pub draft: Draft,
    /// Drafted Markdown body.
    pub body: String,
    /// Drafted complete property map.
    pub properties: std::collections::BTreeMap<String, serde_json::Value>,
}

/// List one folder with descriptions instead of loading all document bodies.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListItemsRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Empty string for the root; otherwise a validated relative folder.
    pub folder: String,
    /// List only the sources that match among the folder's items; child folders are listed
    /// as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction: Option<crate::extraction::ExtractionFilter>,
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// A folder listing at one resolved revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Save the caller's draft; always succeeds even when the head has moved.
///
/// The first save takes the item's head revision as its base. Later saves keep the draft's
/// base unless `base_revision` equals the current head, which is the explicit rebase.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveDraftRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Revision the editor saw; equal to the current head to rebase after resolving a conflict.
    pub base_revision: crate::identity::Revision,
    /// Drafted Markdown body.
    pub body: String,
    /// Complete preserved property map.
    pub properties: std::collections::BTreeMap<String, serde_json::Value>,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// List the caller's own drafts in a workspace.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListDraftsRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// The caller's drafts, without other editors' drafts.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListDraftsResponse {
    /// Draft metadata.
    pub items: Vec<Draft>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Remove the caller's own draft of one item.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiscardDraftRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Move an item and rewrite internal references in one versioned operation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Change an item's status word or archive flag without claiming a business decision.
///
/// At least one of `status` and `archived` is present, and `status` is never `other`
/// (`check_rules`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetLifecycleRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// New OKF status word; absent leaves it unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ItemStatus>,
    /// Archive (`true`) or unarchive (`false`); absent leaves it unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Remove the active item reference while retaining historical objects.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteItemRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Create a user-selected folder and its navigable index.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateFolderRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// New relative folder path.
    pub folder: crate::identity::WorkspacePath,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// A user-defined type schema and UI hints.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTypesRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
}

/// User and built-in type definitions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTypesResponse {
    /// Available property schemas.
    pub items: Vec<TypeDefinition>,
}

/// Save a type definition without discarding existing extension properties.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetTypeRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Validated property schema and UI hints.
    pub definition: TypeDefinition,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// The frontmatter key of the application-owned header every item file carries.
pub const APP_HEADER_KEY: &str = "okf_jawn";
