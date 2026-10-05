//! Resumable import identities, conversion jobs, and durable progress.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

/// Durable work status; acknowledgement is not a claim of completion.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Job {
    /// Job identity.
    pub id: crate::identity::JobId,
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
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
    /// Converter options that participate in the digest cache key.
    pub settings: std::collections::BTreeMap<String, serde_json::Value>,
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
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}
