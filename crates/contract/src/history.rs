//! Git-backed snapshots, comparisons, attribution, and restore operations.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A content snapshot, not a complete application event log.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Commit {
    /// Commit identity.
    pub revision: crate::identity::Revision,
    /// Parent commits.
    pub parents: Vec<crate::identity::Revision>,
    /// Commit message.
    pub message: String,
    /// Git author label, not verified human identity.
    pub author: String,
    /// RFC 3339 commit timestamp.
    pub committed_at: String,
}

/// List snapshots for an item or workspace.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LogRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Limit history to an item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<crate::identity::ItemId>,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// Version history with pagination.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LogResponse {
    /// Content snapshots.
    pub commits: Vec<Commit>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Compare two explicitly resolved snapshots.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Base version.
    pub from: crate::identity::Revision,
    /// Compared version.
    pub to: crate::identity::Revision,
    /// Optional item filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<crate::identity::ItemId>,
}

/// A changed file with before and after locators.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FileChange {
    /// Previous path; absent for additions.
    pub old_path: Option<String>,
    /// New path; absent for deletions.
    pub new_path: Option<String>,
    /// Unified text diff when applicable.
    pub patch: String,
    /// The underlying bytes are binary.
    pub binary: bool,
    /// Added, modified, moved, or removed.
    pub kind: String,
}

/// A comparison tied to both source revisions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffResponse {
    /// Base commit.
    pub from: crate::identity::Revision,
    /// Compared commit.
    pub to: crate::identity::Revision,
    /// Changed content.
    pub changes: Vec<FileChange>,
    /// Limits or unsupported binary comparison details.
    pub warnings: Vec<crate::common::Warning>,
}

/// Create a named snapshot from saved drafts at a checked base revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CommitRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Human-readable snapshot name.
    pub message: String,
    /// Selected saved drafts; empty means the explicit workspace selection.
    pub item_ids: Vec<crate::identity::ItemId>,
}

/// Restore the selected state as a new commit without rewriting history.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoreRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Historical state to restore.
    pub target: crate::identity::Revision,
    /// Restore only this item when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<crate::identity::ItemId>,
    /// Reason for the restore.
    pub message: String,
}

/// Show which commit last changed lines, not who originated each fact.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BlameRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Bounded lines.
    pub lines: crate::common::TextRange,
}

/// A line and its last recorded content change.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BlameLine {
    /// One-based line.
    pub line: u32,
    /// Current text at the selected version.
    pub text: String,
    /// Last recorded change.
    pub commit: Commit,
}

/// Line attribution at an explicit revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BlameResponse {
    /// Selected source.
    pub source: crate::source::SourceReference,
    /// Attribution records.
    pub lines: Vec<BlameLine>,
}
