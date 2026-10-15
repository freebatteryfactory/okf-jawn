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
    /// Failure retained; `retryable` says whether `retry_job` accepts it.
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
/// job kind (`JobKind::is_tenant`); `check` enforces it. The store writes the job's instants
/// and `retryable` as its state changes; a job accepted before a field existed reads it as
/// absent.
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
    /// The refusal as the job's handler gave it; `field` is a JSON Pointer into the request
    /// that started the job.
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
    /// What a completed workspace backup left out of its archive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<crate::workspace::BackupReport>,
    /// When the job was accepted; absent on a job accepted before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<crate::identity::Timestamp>,
    /// When the current attempt was claimed; absent while the job waits, and cleared when a
    /// retry queues it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<crate::identity::Timestamp>,
    /// When the job reached a terminal state; cleared when a retry queues it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<crate::identity::Timestamp>,
    /// Whether `retry_job` will accept the job: present when it failed under a build that
    /// records this field, and absent in every other state. A job that failed before this field
    /// existed has none; `retry_job` still decides for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
}

/// Register immutable upload intent before streaming bytes.
///
/// `filename` and `relative_path` are bounded in UTF-8 bytes (`MAX_UPLOAD_FILENAME_BYTES`,
/// `MAX_UPLOAD_RELATIVE_PATH_BYTES`): the schema states each bound as `x-max-utf8-bytes`, never as
/// a `maxLength` in characters, and `check_rules` counts the bytes and refuses a longer name as
/// `too_large` on its field. A `filename` that gives no workspace path for its source card
/// (`conventions::source_card_name`, then `WorkspacePath::from_supplied`) is refused as
/// `invalid_input` on `/filename` when the slot is opened; an import that later places the
/// card in a folder where the path cannot be written still refuses the job.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateUploadRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Original filename, preserved. At most 1024 bytes in UTF-8: a character takes one to four.
    #[schemars(extend("x-max-utf8-bytes" = MAX_UPLOAD_FILENAME_BYTES))]
    pub filename: String,
    /// Supplied folder context, preserved.
    /// At most 4096 bytes in UTF-8: a character takes one to four.
    #[schemars(extend("x-max-utf8-bytes" = MAX_UPLOAD_RELATIVE_PATH_BYTES))]
    pub relative_path: String,
    /// Expected decimal byte count. A size over `limits.upload_bytes` is refused as
    /// `too_large` on `/size` when the slot is opened.
    pub size: String,
    /// Expected content hash if caller knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<crate::identity::Digest>,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// An authenticated upload slot for a specific source occurrence.
///
/// A slot stays until it is discarded, imported or its workspace is purged; nothing expires,
/// and a completed upload is never removed by the service on its own.
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
    /// Original filename, as supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// Supplied folder context, as supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    /// The announced byte count, as a decimal string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// When the slot was opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<crate::identity::Timestamp>,
    /// The job that took this upload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_by: Option<crate::identity::JobId>,
}

/// Which uploads of a workspace to list.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UploadListState {
    /// Not yet complete: bytes are still to come, or the slot is not finished.
    Incomplete,
    /// Complete and not consumed: waiting to be imported or discarded.
    Waiting,
    /// Taken by a job as its input.
    Consumed,
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

/// Read one upload slot as it now stands.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetUploadRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Upload identity.
    pub upload_id: crate::identity::UploadId,
}

/// List a workspace's upload slots, newest first.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListUploadsRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Only the uploads in this state; absent lists every upload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<UploadListState>,
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// A bounded list of upload slots, newest first.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListUploadsResponse {
    /// Upload slots.
    pub items: Vec<Upload>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Remove an upload slot and its bytes.
///
/// Refused with `conflict` when a job has consumed the upload or a queued or running job names
/// it. A repeat under the same key answers the response first given.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiscardUploadRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Upload to remove.
    pub upload_id: crate::identity::UploadId,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Convert finalized uploads into source cards in the user-selected folder.
///
/// `upload_ids` holds between one and `MAX_IMPORT_UPLOADS` uploads (`check_rules`): a longer
/// list is `too_large` on `/upload_ids`, an empty one `invalid_input` on `/upload_ids`, and an
/// upload listed twice `invalid_input` on `/upload_ids/<index of the second>`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartImportRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// The revision the import was prepared against; card names are chosen there. It must be
    /// a revision of the workspace. A head that moved since does not refuse the import. A card
    /// path taken since fails the job, which is not retried.
    pub base_revision: crate::identity::Revision,
    /// Completed upload slots, each listed once.
    #[schemars(length(min = 1, max = MAX_IMPORT_UPLOADS))]
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
    /// At most 8388608 bytes in UTF-8: a character takes one to four.
    #[schemars(extend("x-max-utf8-bytes" = crate::item::MAX_ITEM_TEXT_BYTES))]
    pub corrected_markdown: String,
    /// Keep the accepted agent-supplied text of this proposal over a later complete
    /// conversion: `corrected_markdown` must equal that text (compared by digest), and the
    /// result records who kept it instead of a correction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopts: Option<crate::identity::ProposalId>,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// The upload size a deployment allows unless it configures another: 512 MiB.
///
/// A deployment may set its own (core's `ApplicationConfig::max_upload_bytes`); a client reads
/// the limit in force from `get_session` (`limits.upload_bytes`), never from this constant.
pub const DEFAULT_MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// The most UTF-8 bytes of an upload's `filename`: 1024 (a 255-unit NTFS name of three-byte
/// characters is 765).
pub const MAX_UPLOAD_FILENAME_BYTES: usize = 1024;

/// The most UTF-8 bytes of an upload's `relative_path`: the bound of a `WorkspacePath`.
pub const MAX_UPLOAD_RELATIVE_PATH_BYTES: usize = crate::identity::MAX_WORKSPACE_PATH_BYTES;

/// The most uploads one `start_import` takes: 5000, the bound of one drop in the workspace UI.
pub const MAX_IMPORT_UPLOADS: usize = 5000;

impl JobKind {
    /// Every job kind, in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::Import,
        Self::Redigest,
        Self::ExportWorkspace,
        Self::BackupWorkspace,
        Self::RestoreWorkspace,
        Self::RebuildIndex,
        Self::ExportView,
        Self::BackupInstallation,
        Self::PurgeWorkspace,
        Self::PurgeItem,
    ];

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

impl CreateUploadRequest {
    /// Refuse a name over its byte bound, and a `filename` that gives no workspace path for
    /// its source card.
    ///
    /// # Errors
    /// Returns `TooLarge` on `/filename` or `/relative_path` for a name longer than its bound,
    /// and `InvalidInput` on `/filename` for a name `WorkspacePath::from_supplied` refuses once
    /// `conventions::source_card_name` has made it a card's file name.
    pub fn check_names(&self) -> Result<(), crate::error::ApiError> {
        check_bytes(&self.filename, MAX_UPLOAD_FILENAME_BYTES, "/filename")?;
        check_bytes(
            &self.relative_path,
            MAX_UPLOAD_RELATIVE_PATH_BYTES,
            "/relative_path",
        )?;
        let card = crate::conventions::source_card_name(&self.filename);
        crate::identity::WorkspacePath::from_supplied(&card).map_err(|error| {
            crate::error::ApiError::new(
                crate::error::ErrorCode::InvalidInput,
                format!("the file name gives no workspace path for its source card: {error}"),
            )
            .with_field("/filename")
        })?;
        Ok(())
    }
}

impl StartImportRequest {
    /// Refuse an empty list of uploads, a list longer than `MAX_IMPORT_UPLOADS`, and an upload
    /// listed twice.
    ///
    /// # Errors
    /// Returns `TooLarge` on `/upload_ids` for more than `MAX_IMPORT_UPLOADS`, `InvalidInput` on
    /// `/upload_ids` for none, and `InvalidInput` on `/upload_ids/<index>` naming the second
    /// place an upload is listed.
    pub fn check_uploads(&self) -> Result<(), crate::error::ApiError> {
        if self.upload_ids.len() > MAX_IMPORT_UPLOADS {
            return Err(crate::error::ApiError::new(
                crate::error::ErrorCode::TooLarge,
                format!(
                    "an import takes at most {MAX_IMPORT_UPLOADS} uploads; this one lists {}",
                    self.upload_ids.len()
                ),
            )
            .with_field("/upload_ids"));
        }
        if self.upload_ids.is_empty() {
            return Err(crate::error::ApiError::new(
                crate::error::ErrorCode::InvalidInput,
                "an import takes at least one upload",
            )
            .with_field("/upload_ids"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for (index, upload) in self.upload_ids.iter().enumerate() {
            if !seen.insert(upload.0) {
                return Err(crate::error::ApiError::new(
                    crate::error::ErrorCode::InvalidInput,
                    "an upload is listed more than once",
                )
                .with_field(format!("/upload_ids/{index}")));
            }
        }
        Ok(())
    }
}

/// Refuse `text` longer than `bound` UTF-8 bytes as `too_large` on `field`.
fn check_bytes(text: &str, bound: usize, field: &str) -> Result<(), crate::error::ApiError> {
    if text.len() <= bound {
        return Ok(());
    }
    Err(crate::error::ApiError::new(
        crate::error::ErrorCode::TooLarge,
        format!(
            "The name is {} bytes; it is at most {bound} bytes",
            text.len()
        ),
    )
    .with_field(field))
}
