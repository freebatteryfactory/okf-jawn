//! Create, open, list, and export user-organized workspaces.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A user-owned OKF workspace with a resolved head revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    /// Stable application identity.
    pub id: crate::identity::WorkspaceId,
    /// Human-selected workspace name.
    pub name: String,
    /// One-line context for people and agents.
    pub description: String,
    /// Current resolved Git commit.
    pub head: crate::identity::Revision,
    /// RFC 3339 creation time.
    pub created_at: String,
    /// Whether this principal may only read it.
    pub read_only: bool,
}

/// List only workspaces visible to the authenticated principal.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListWorkspacesRequest {
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// A bounded workspace listing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListWorkspacesResponse {
    /// Authorized workspaces.
    pub items: Vec<Workspace>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Create a blank workspace without baked-in example content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceRequest {
    /// Display name.
    pub name: String,
    /// Purpose of this workspace.
    pub description: String,
    /// Caller-chosen retry identity.
    pub idempotency_key: String,
}

/// Open an existing workspace by application identity.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenWorkspaceRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
}

/// Update workspace presentation metadata at a known revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateWorkspaceRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// New display name.
    pub name: String,
    /// Updated purpose.
    pub description: String,
}

/// Archive the workspace without deleting retained content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveWorkspaceRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
}

/// Export portable OKF content and all selected referenced assets.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportWorkspaceRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Include retained Git history in addition to the selected state.
    pub include_history: bool,
    /// Retry identity for the export job.
    pub idempotency_key: String,
}

/// Back up content plus application records; distinct from portable export.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BackupWorkspaceRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Retry identity for the backup.
    pub idempotency_key: String,
}

/// Restore a workspace from a retained backup artifact (full restore of content and app records).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoreWorkspaceRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Retained backup artifact to restore from.
    pub artifact_id: crate::identity::ArtifactId,
    /// Optional integrity check against the artifact digest before restore begins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<crate::identity::Digest>,
    /// Retry identity for the restore job.
    pub idempotency_key: String,
}

/// An authorized export or backup artifact with integrity metadata.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DownloadArtifact {
    /// Stable downloadable artifact.
    pub artifact_id: crate::identity::ArtifactId,
    /// Digest of the archive.
    pub sha256: crate::identity::Digest,
    /// Byte count as a decimal string to preserve integer precision.
    pub size: String,
    /// Application-relative authorized download path.
    pub download_path: String,
}
