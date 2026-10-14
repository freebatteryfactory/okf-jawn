//! Diagnostics are actionable observations, never a blanket trust score.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Known maintenance conditions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// A reference does not resolve.
    BrokenLink,
    /// `stale_after` requires checking.
    Stale,
    /// Agent-authored content has no covering review.
    Unreviewed,
    /// Identical bytes have multiple visible appearances.
    Duplicate,
    /// Navigation summary is absent.
    MissingDescription,
    /// A source whose conversion is partial, failed or unsupported; action `redigest_item`.
    Unprocessed,
    /// A completed source whose conversion recorded warnings; action `read_item`.
    ExtractionWarnings,
    /// A pinned View has newer source data available.
    NewerSource,
    /// Possible instruction-like source text; advisory only.
    InstructionLike,
    /// A saved View or a citation refers to purged content; action `get_view`.
    Invalidated,
    /// An imported file's header does not parse; its bytes are kept and nothing is blocked.
    /// Action `get_item`.
    UnparseableHeader,
    /// Uploads that are complete and not imported, with their count and total size in the
    /// message; one row a workspace when any exist. Action `list_uploads`.
    UploadsWaiting,
}

/// An observation with a useful next action.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttentionItem {
    /// Diagnostic class.
    pub kind: AttentionKind,
    /// Relevant item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<crate::identity::ItemId>,
    /// The item at the response's `revision`; absent when the observation is about no one
    /// item or the item was purged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<crate::item::ItemSummary>,
    /// The place inside the item the observation is about, in the form `read_item` takes;
    /// absent when it is about the whole item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<crate::read::Selection>,
    /// Concrete explanation.
    pub message: String,
    /// Suggested existing operation, never executable code.
    pub action: crate::metadata::OperationName,
}

/// Read workspace diagnostics at an explicit revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetAttentionRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// Bounded diagnostics; an empty list does not establish safety.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetAttentionResponse {
    /// Inspected revision.
    pub revision: crate::identity::Revision,
    /// Visible observations.
    pub items: Vec<AttentionItem>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Rebuild derived search and link indexes without deleting application records.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RebuildIndexRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}
