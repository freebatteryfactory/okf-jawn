//! Durable application work records are independent of queue delivery and telemetry.
//!
//! `RecordStore` is the durable source of job truth. `JobQueue` delivers wake-ups.
//! `JobHandler` executes claimed work; ingest owns the runtime adapter implementation.
//! A job is created from a core-owned `JobSpec`, never from the wire `Job`: the specification
//! carries the inputs, the initiator and the `MutationId` a handler needs to do the work again
//! after a crash. Every insert is unique on `MutationId` and a repeated id returns the prior row.

use okf_jawn_contract::{
    common::Warning,
    events::Receipt,
    identity::{
        ArtifactId, Digest, ItemId, JobId, MutationId, ReceiptId, Revision, UploadId, WorkspaceId,
        WorkspacePath,
    },
    import::{Job, JobKind, ListJobsResponse},
    review::Review,
    workspace::DownloadArtifact,
};
use serde::{Deserialize, Serialize};

use crate::conversion::ConversionSettings;
use crate::ports::PortFuture;
use crate::storage::{Page, Provenance, StorageScope};

/// What a job must do: the inputs of the request that started it, with selectors resolved.
///
/// The record store keeps the specification as written and hands it back on every claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JobSpec {
    /// Convert finalized uploads into source cards.
    Import {
        /// Revision the import is based on.
        base_revision: Revision,
        /// Completed upload slots to import.
        upload_ids: Vec<UploadId>,
        /// Destination folder; `None` is the workspace root.
        destination: Option<WorkspacePath>,
        /// Apply the workspace naming rules to the new cards.
        apply_naming_rules: bool,
    },
    /// Convert one source again with explicit settings, keeping human corrections.
    Redigest {
        /// Source item to convert again.
        item_id: ItemId,
        /// Revision the new digest is based on.
        base_revision: Revision,
        /// Settings that identify the new digest.
        settings: ConversionSettings,
    },
    /// Build a portable export of the workspace.
    ExportWorkspace {
        /// Resolved revision to export.
        revision: Revision,
        /// Include retained history in addition to the selected state.
        include_history: bool,
    },
    /// Back up content, retained objects and application records.
    BackupWorkspace,
    /// Restore content and application records from a retained backup artifact.
    RestoreWorkspace {
        /// Backup artifact to restore from.
        artifact_id: ArtifactId,
        /// Digest the artifact must have before the restore begins, when supplied.
        sha256: Option<Digest>,
    },
    /// Rebuild the derived search and link index at the current head.
    RebuildIndex,
    /// Export one View with its specification, sources and data table.
    ExportView {
        /// View item to export.
        item_id: ItemId,
        /// Resolved revision to export.
        revision: Revision,
    },
}

/// A job to register durably before it is enqueued or reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    /// Durable write identity of the request that starts the job; every commit and row the
    /// handler creates is written under it.
    pub mutation_id: MutationId,
    /// Who the job acts for.
    pub initiator: Provenance,
    /// What the job must do.
    pub spec: JobSpec,
}

/// Lease identity prevents a late worker from completing a newer attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobLease {
    /// Stable application work id.
    pub job_id: JobId,
    /// Unique claim token changed on every claim.
    pub token: String,
    /// Monotonic attempt number.
    pub attempt: u32,
}

/// A claimed job: everything `JobHandler::handle` needs, with nothing to look up elsewhere.
#[derive(Debug, Clone)]
pub struct ClaimedJob {
    /// Tenant and workspace scope for the work.
    pub scope: StorageScope,
    /// Compare-and-set claim identity.
    pub lease: JobLease,
    /// Durable job record as it stands at the claim.
    pub job: Job,
    /// Durable write identity the job was created under; the same on every attempt.
    pub mutation_id: MutationId,
    /// Who the job acts for.
    pub initiator: Provenance,
    /// What the job must do.
    pub spec: JobSpec,
}

/// A durable completion result, never inferred solely from a queue acknowledgment.
#[derive(Debug, Clone)]
pub struct JobCompletion {
    /// Compare-and-set claim identity.
    pub lease: JobLease,
    /// Committed content revision, when the job changed the workspace.
    pub revision: Option<Revision>,
    /// Items the job produced.
    pub item_ids: Vec<ItemId>,
    /// Export or backup artifact the job produced.
    pub artifact: Option<DownloadArtifact>,
    /// Retained output objects, kept as garbage-collection roots.
    pub outputs: Vec<Digest>,
    /// Non-fatal issues recorded during the work.
    pub warnings: Vec<Warning>,
}

/// SQLite-backed non-rebuildable application records.
pub trait RecordStore: Send + Sync {
    /// Register a queued job and allocate its identity.
    ///
    /// Unique on `job.mutation_id`: a repeated id inserts nothing and returns the prior job,
    /// whatever state it has reached. The returned `Job::kind` is `job.spec.kind()`.
    fn create_job<'a>(&'a self, scope: &'a StorageScope, job: NewJob) -> PortFuture<'a, Job>;
    /// Read one durable job within workspace scope.
    fn get_job<'a>(&'a self, scope: &'a StorageScope, job: JobId) -> PortFuture<'a, Job>;
    /// List durable jobs for a workspace, newest first, with bounded pagination.
    fn list_jobs<'a>(
        &'a self,
        scope: &'a StorageScope,
        page: Page,
    ) -> PortFuture<'a, ListJobsResponse>;
    /// Claim runnable work under a compare-and-set lease; `None` when it is not runnable.
    fn claim_job(&self, job: JobId) -> PortFuture<'_, Option<ClaimedJob>>;
    /// Record progress, 0 to 100, for the current claim and return the job as it now stands.
    ///
    /// A handler reads the returned state: a cancelled job stops working.
    fn update_progress<'a>(&'a self, lease: &'a JobLease, progress: u8) -> PortFuture<'a, Job>;
    /// Commit completion only for the current unexpired claim.
    fn complete_job(&self, completion: JobCompletion) -> PortFuture<'_, Job>;
    /// Keep the failure and retry eligibility for the current claim.
    fn fail_job(&self, lease: JobLease, message: String, retryable: bool) -> PortFuture<'_, Job>;
    /// Cancel pending or running work while retaining recorded state.
    ///
    /// Unique on `mutation_id`: a repeated id makes no second transition and returns the job
    /// row as it stands.
    fn cancel_job<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        job: JobId,
    ) -> PortFuture<'a, Job>;
    /// Queue a failed job again under the same durable identity.
    ///
    /// Unique on `mutation_id`: a repeated id does not queue the job a second time, even when
    /// the job has run and failed again since, and returns the job row as it stands.
    fn retry_job<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        job: JobId,
    ) -> PortFuture<'a, Job>;
    /// Enumerate unfinished records across tenants for queue reconciliation on restart.
    fn pending_jobs(&self) -> PortFuture<'_, Vec<(StorageScope, JobId)>>;
    /// Release claims whose leases have expired so work can be reclaimed.
    fn expire_leases(&self) -> PortFuture<'_, u32>;
    /// Record exact reviewed content after application-level confirmation.
    ///
    /// Unique on `mutation_id`: a repeated id inserts nothing and returns the prior review.
    fn insert_review<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        review: Review,
    ) -> PortFuture<'a, Review>;
    /// Every review recorded for an item, oldest first, with coverage as it was recorded.
    ///
    /// The application recomputes coverage against the revision it resolved.
    fn list_reviews<'a>(
        &'a self,
        scope: &'a StorageScope,
        item: ItemId,
    ) -> PortFuture<'a, Vec<Review>>;
    /// Persist what was returned, not merely a trace identifier.
    ///
    /// A write passes its `MutationId`: the receipt is then unique on it and a repeated id
    /// returns the prior receipt. A read passes `None` and always inserts.
    fn insert_receipt<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: Option<MutationId>,
        receipt: Receipt,
    ) -> PortFuture<'a, Receipt>;
    /// Read one durable receipt within workspace scope.
    fn get_receipt<'a>(
        &'a self,
        scope: &'a StorageScope,
        receipt: ReceiptId,
    ) -> PortFuture<'a, Receipt>;
}

/// Delivery adapter; job truth remains in `RecordStore`.
pub trait JobQueue: Send + Sync {
    /// Deliver an existing durable job identity, accepting possible duplicate delivery.
    fn enqueue(&self, workspace: WorkspaceId, job: JobId) -> PortFuture<'_, ()>;
}

/// Executes claimed durable work; ingest owns the Tokio + `RecordStore` runtime adapter.
pub trait JobHandler: Send + Sync {
    /// Run one claimed attempt; completion and failure are written through `RecordStore`.
    fn handle<'a>(&'a self, claimed: &'a ClaimedJob) -> PortFuture<'a, ()>;
}

impl JobSpec {
    /// The wire kind shown on the job.
    #[must_use]
    pub const fn kind(&self) -> JobKind {
        match self {
            Self::Import { .. } => JobKind::Import,
            Self::Redigest { .. } => JobKind::Redigest,
            Self::ExportWorkspace { .. } => JobKind::ExportWorkspace,
            Self::BackupWorkspace => JobKind::BackupWorkspace,
            Self::RestoreWorkspace { .. } => JobKind::RestoreWorkspace,
            Self::RebuildIndex => JobKind::RebuildIndex,
            Self::ExportView { .. } => JobKind::ExportView,
        }
    }
}
