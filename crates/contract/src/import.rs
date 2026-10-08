//! Resumable import identities, conversion jobs, and durable progress.
//!
//! A job runs in a workspace or, for an installation backup and a purge, at the tenant: a
//! tenant job's wire `Job` has no `workspace_id` and is followed through `get_tenant_job`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::metadata::OperationName;

/// Job progress retained independently of diagnostic traces.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Durably registered for execution.
    Queued,
    /// Execution attempt in progress.
    Running,
    /// Committed output is available.
    Succeeded,
    /// Failure retained with retry information.
    Failed,
    /// Explicit cancellation completed.
    Cancelled,
}

/// What a durable job does; fixed when the job is accepted.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// Convert finalized uploads into source cards.
    Import,
    /// Re-run extraction for one item.
    Redigest,
    /// Build a portable export.
    ExportWorkspace,
    /// Back up content and durable application records.
    BackupWorkspace,
    /// Restore content and durable application records from a backup.
    RestoreWorkspace,
    /// Rebuild derived search and link data.
    RebuildIndex,
    /// Export one View with its sources, data table and rendering.
    ExportView,
    /// Write an installation archive (tenant-level).
    BackupInstallation,
    /// Remove a workspace permanently (tenant-level).
    PurgeWorkspace,
    /// Remove one item permanently (tenant-level).
    PurgeItem,
}

/// Durable work status; acknowledgement is not a claim of completion.
///
/// `workspace_id` is present exactly for a workspace job kind and absent exactly for a tenant
/// job kind (`JobKind::is_tenant`); `check` enforces it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Job {
    /// Job identity.
    pub id: crate::identity::JobId,
    /// Workspace whose permissions and storage scope apply; absent for a tenant job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<crate::identity::WorkspaceId>,
    /// What this job does.
    pub kind: JobKind,
    /// Current durable state.
    pub state: JobState,
    /// Approximate completion percentage.
    pub progress: u8,
    /// Execution attempt number.
    pub attempt: u32,
    /// Recorded warnings.
    pub warnings: Vec<crate::common::Warning>,
    /// Last failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<crate::error::ApiError>,
    /// Committed result revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<crate::identity::Revision>,
    /// Produced items.
    pub item_ids: Vec<crate::identity::ItemId>,
    /// Export or backup artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<crate::workspace::DownloadArtifact>,
    /// What a completed restore did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<crate::workspace::RestoreReport>,
}

/// Register immutable upload intent before streaming bytes.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateUploadRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Original filename, preserved.
    pub filename: String,
    /// Supplied folder context.
    pub relative_path: String,
    /// Expected decimal byte count.
    pub size: String,
    /// Expected content hash if caller knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<crate::identity::Digest>,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// An authenticated upload slot for a specific source occurrence.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Upload {
    /// Upload identity.
    pub id: crate::identity::UploadId,
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Same-origin binary PUT target.
    pub upload_path: String,
    /// Durably retained byte count.
    pub received_bytes: String,
    /// Whether expected bytes and digest have been verified.
    pub complete: bool,
}

/// Verify upload length and hash without silently creating duplicate occurrences.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompleteUploadRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Upload to finalize.
    pub upload_id: crate::identity::UploadId,
    /// Expected full-content hash.
    pub sha256: crate::identity::Digest,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Convert finalized uploads into source cards in the user-selected folder.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartImportRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Completed upload slots.
    pub upload_ids: Vec<crate::identity::UploadId>,
    /// Relative destination folder; empty for root.
    pub destination: String,
    /// Durable retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
    /// Apply the explicit workspace conventions after preview acceptance.
    pub apply_naming_rules: bool,
    /// Converter settings; absent means the deployment's defaults, resolved into the job at
    /// acceptance so a retry converts with the same settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<crate::extraction::ConversionSettings>,
}

/// Read durable job state.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetJobRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Job identity.
    pub job_id: crate::identity::JobId,
}

/// List workspace background work.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListJobsRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// A bounded list of jobs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListJobsResponse {
    /// Job records.
    pub items: Vec<Job>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Read one installation-level job: an installation backup or a purge.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetTenantJobRequest {
    /// Job identity.
    pub job_id: crate::identity::JobId,
}

/// List installation-level jobs, newest first.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTenantJobsRequest {
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// Retry a recoverable job with the same durable work identity.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetryJobRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Existing failed job.
    pub job_id: crate::identity::JobId,
    /// Retry identity of this retry request, distinct from the job's durable identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Cancel pending or running work without discarding the uploaded original.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelJobRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Job to cancel.
    pub job_id: crate::identity::JobId,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Reconvert an original with explicitly selected settings, preserving human corrections.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RedigestRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Converter settings; they are part of the new digest's identity. With
    /// `unconverted_only` they must equal the current digest's settings, or the request is
    /// `invalid_input` on `/settings`.
    pub settings: crate::extraction::ConversionSettings,
    /// Convert exactly the `not_converted` pages of the current extraction and merge them into
    /// a new digest, instead of converting the whole source again.
    pub unconverted_only: bool,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Record human corrections separately from the generated extraction.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CorrectDigestRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Digest being corrected.
    pub digest: crate::identity::Digest,
    /// Corrected text kept separate from generated text.
    pub corrected_markdown: String,
    /// Keep the accepted agent-supplied text of this proposal over a later complete
    /// conversion: `corrected_markdown` must equal that text (compared by digest), and the
    /// result records who kept it instead of a correction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopts: Option<crate::identity::ProposalId>,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

impl JobKind {
    /// Whether a job of this kind runs at the tenant level; its `Job` has no `workspace_id`,
    /// and `retry_job` and `cancel_job`, which take a workspace, never reach it.
    #[must_use]
    pub const fn is_tenant(self) -> bool {
        matches!(
            self,
            Self::BackupInstallation | Self::PurgeWorkspace | Self::PurgeItem
        )
    }

    /// The operation that creates a job of this kind. `retry_job` and `cancel_job` require its
    /// table permission, and a human route when it is in `HUMAN_SESSION_ONLY`.
    #[must_use]
    pub const fn started_by(self) -> OperationName {
        match self {
            Self::Import => OperationName::StartImport,
            Self::Redigest => OperationName::RedigestItem,
            Self::ExportWorkspace => OperationName::ExportWorkspace,
            Self::BackupWorkspace => OperationName::BackupWorkspace,
            Self::RestoreWorkspace => OperationName::RestoreWorkspace,
            Self::RebuildIndex => OperationName::RebuildIndex,
            Self::ExportView => OperationName::ExportView,
            Self::BackupInstallation => OperationName::BackupInstallation,
            Self::PurgeWorkspace => OperationName::PurgeWorkspace,
            Self::PurgeItem => OperationName::PurgeItem,
        }
    }
}

impl Job {
    /// Refuse a job whose `workspace_id` disagrees with its kind's scope.
    ///
    /// # Errors
    /// Returns `InvalidInput` on `/workspace_id` when a workspace job names no workspace or a
    /// tenant job names one.
    pub fn check(&self) -> Result<(), crate::error::ApiError> {
        if self.workspace_id.is_some() == self.kind.is_tenant() {
            Err(crate::error::ApiError::new(
                crate::error::ErrorCode::InvalidInput,
                "a workspace job names its workspace and a tenant job names none",
            )
            .with_field("/workspace_id"))
        } else {
            Ok(())
        }
    }
}
