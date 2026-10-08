//! Create, open, list, archive, export, back up and restore user-organized workspaces.
//!
//! The download boundary decides by artifact kind and route: a backup archive is served only to
//! an administrator in a human browser session or to the local owner, never to an agent or a
//! service route, and never through an export or object read.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::{AccessRoute, Permission};

/// A user-owned OKF workspace with a resolved head revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    /// Creation time.
    pub created_at: crate::identity::Timestamp,
    /// When it was archived; absent while it is not archived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<crate::identity::Timestamp>,
    /// The caller's effective grant on this workspace after any delegation ceiling.
    pub permissions: Vec<crate::access::Permission>,
}

/// List only workspaces visible to the authenticated principal.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListWorkspacesRequest {
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
    /// Also list archived workspaces; absent means false. Optional, so the `workspaces` model
    /// tool gains no required argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_archived: Option<bool>,
}

/// A bounded workspace listing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListWorkspacesResponse {
    /// Authorized workspaces.
    pub items: Vec<Workspace>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Create a blank workspace without baked-in example content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceRequest {
    /// Display name.
    pub name: String,
    /// Purpose of this workspace.
    pub description: String,
    /// Caller-chosen retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Open an existing workspace by application identity.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenWorkspaceRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
}

/// Update workspace presentation metadata at a known revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Archive the workspace without deleting retained content.
///
/// While a workspace is archived, an operation that would change its content or open work on
/// it (a commit, a draft, a proposal, an upload, an import or redigest) is refused as
/// `conflict`; reads, export, backup, `unarchive_workspace` and purge are allowed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveWorkspaceRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Return an archived workspace to ordinary listings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnarchiveWorkspaceRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Export portable OKF content and all selected referenced assets.
///
/// Committed content at one revision with the originals and retained derivatives it
/// references, and its history when asked. A current app review is written into its item's
/// header as an OKF `verified` entry. Drafts, sessions and credentials are never included.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportWorkspaceRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Include retained Git history in addition to the selected state.
    pub include_history: bool,
    /// Retry identity for the export job.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Back up one workspace into one self-contained archive; distinct from portable export.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BackupWorkspaceRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Retry identity for the backup.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// What a retained artifact is.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// A portable export of a workspace.
    Export,
    /// A workspace archive.
    WorkspaceBackup,
    /// An installation archive (hosted: one tenant's).
    InstallationBackup,
    /// An export of one View.
    ViewExport,
}

/// Where an artifact's records live, and so which transport serves it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactScope {
    /// A workspace's artifact, served by `download_artifact`.
    Workspace,
    /// A tenant's artifact, served by `download_tenant_artifact`.
    Tenant,
}

/// Start an installation archive: identities, tenant grants, connector records without
/// secrets, purge records and tenant events; no workspace content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BackupInstallationRequest {
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Fill a blank workspace of this installation from an uploaded workspace archive.
///
/// The restore keeps item ids, rewrites View bindings to this workspace, gives each draft
/// whose editor is a validated subject with `write` here back to that editor, and keeps the
/// others in the archive, unassigned and counted. An archive of this tenant whose workspace
/// still exists, or has a purge record, is refused: purged content never comes back through
/// an import. The upload is consumed when the restore completes.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoreWorkspaceRequest {
    /// Blank target workspace.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Completed upload of the archive into this workspace.
    pub upload_id: crate::identity::UploadId,
    /// SHA-256 the archive must have; checked before anything is written.
    pub sha256: crate::identity::Digest,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// What a restore did.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RestoreReport {
    /// Items restored.
    pub items: u32,
    /// Drafts given back to their editors.
    pub drafts_restored: u32,
    /// Drafts whose editor is not a validated identity here; kept in the archive.
    pub drafts_unassigned: u32,
}

/// An authorized export or backup artifact with integrity metadata.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DownloadArtifact {
    /// Stable downloadable artifact.
    pub artifact_id: crate::identity::ArtifactId,
    /// What the artifact is; it decides the download permission and routes.
    pub kind: ArtifactKind,
    /// Digest of the archive.
    pub sha256: crate::identity::Digest,
    /// Byte count as a decimal string to preserve integer precision.
    pub size: String,
    /// Application-relative authorized download path:
    /// `/api/workspaces/{workspace_id}/artifacts/{artifact_id}` for a workspace artifact,
    /// `/api/artifacts/{artifact_id}` for a tenant artifact.
    pub download_path: String,
}

impl ArtifactKind {
    /// Where the artifact lives: `InstallationBackup` is tenant-scoped, the rest workspace-scoped.
    #[must_use]
    pub const fn scope(self) -> ArtifactScope {
        match self {
            Self::InstallationBackup => ArtifactScope::Tenant,
            Self::Export | Self::WorkspaceBackup | Self::ViewExport => ArtifactScope::Workspace,
        }
    }

    /// Permission the download transport requires, on the workspace or on the tenant: `admin`
    /// for both backups, `read` for both exports.
    #[must_use]
    pub const fn download_permission(self) -> Permission {
        match self {
            Self::WorkspaceBackup | Self::InstallationBackup => Permission::Admin,
            Self::Export | Self::ViewExport => Permission::Read,
        }
    }

    /// Routes that may download it; every other route is refused whatever its grants. No
    /// export reaches `mcp_delegation`: it holds originals.
    #[must_use]
    pub const fn download_routes(self) -> &'static [AccessRoute] {
        match self {
            Self::WorkspaceBackup | Self::InstallationBackup => {
                &[AccessRoute::BrowserSession, AccessRoute::LocalOwner]
            }
            Self::Export | Self::ViewExport => &[
                AccessRoute::BrowserSession,
                AccessRoute::LocalOwner,
                AccessRoute::Service,
            ],
        }
    }
}
