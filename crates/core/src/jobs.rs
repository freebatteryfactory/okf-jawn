//! Durable application work records are independent of queue delivery and telemetry.
//!
//! `RecordStore` is the durable source of job truth. `JobQueue` delivers wake-ups.
//! `JobHandler` executes claimed work; ingest owns the runtime adapter implementation.
//! Creating a job takes `MutationId` and enforces uniqueness — one idempotency mechanism.

use crate::ports::PortFuture;
use crate::storage::StorageScope;
use okf_jawn_contract::{
    events::Receipt,
    identity::{JobId, MutationId, ReceiptId, WorkspaceId},
    import::{Job, ListJobsRequest, ListJobsResponse},
    review::{ListReviewsRequest, ListReviewsResponse, Review},
};

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

/// A claimed job ready for `JobHandler::handle`, including the storage scope.
#[derive(Debug, Clone)]
pub struct ClaimedJob {
    /// Tenant and workspace scope for the work.
    pub scope: StorageScope,
    /// Compare-and-set claim identity.
    pub lease: JobLease,
    /// Durable job record.
    pub job: Job,
}

/// A durable completion result, never inferred solely from a queue acknowledgment.
#[derive(Debug, Clone)]
pub struct JobCompletion {
    /// Compare-and-set claim identity.
    pub lease: JobLease,
    /// Retained output object references.
    pub outputs: Vec<okf_jawn_contract::identity::Digest>,
    /// A committed content revision when the job changes a workspace.
    pub revision: Option<okf_jawn_contract::identity::Revision>,
}

/// SQLite-backed non-rebuildable application records.
pub trait RecordStore: Send + Sync {
    /// Insert work idempotently under `mutation_id`; a reused id returns the prior row.
    ///
    /// Inserting with a reused `MutationId` is a no-op that returns the prior row, never a
    /// duplicate effect.
    fn create_job<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        job: Job,
    ) -> PortFuture<'a, Job>;
    /// Read one durable job within workspace scope.
    fn get_job<'a>(&'a self, scope: &'a StorageScope, job: JobId) -> PortFuture<'a, Job>;
    /// List durable jobs for a workspace with bounded pagination.
    fn list_jobs<'a>(
        &'a self,
        scope: &'a StorageScope,
        request: ListJobsRequest,
    ) -> PortFuture<'a, ListJobsResponse>;
    /// Claim runnable work under a compare-and-set lease.
    fn claim_job(&self, job: JobId) -> PortFuture<'_, Option<ClaimedJob>>;
    /// Commit completion only for the current unexpired claim.
    fn complete_job(&self, completion: JobCompletion) -> PortFuture<'_, Job>;
    /// Keep the failure and retry eligibility for the current claim.
    fn fail_job(&self, lease: JobLease, message: String, retryable: bool) -> PortFuture<'_, Job>;
    /// Cancel pending or running work while retaining recorded state.
    fn cancel_job<'a>(&'a self, scope: &'a StorageScope, job: JobId) -> PortFuture<'a, Job>;
    /// Retry the same durable work identity without duplicating completed effects.
    fn retry_job<'a>(&'a self, scope: &'a StorageScope, job: JobId) -> PortFuture<'a, Job>;
    /// Enumerate unfinished records across tenants for queue reconciliation on restart.
    fn pending_jobs(&self) -> PortFuture<'_, Vec<(StorageScope, JobId)>>;
    /// Release claims whose leases have expired so work can be reclaimed.
    fn expire_leases(&self) -> PortFuture<'_, u32>;
    /// Record exact reviewed content after application-level confirmation.
    fn insert_review<'a>(
        &'a self,
        scope: &'a StorageScope,
        review: Review,
    ) -> PortFuture<'a, Review>;
    /// List review evidence for an authorized item selection.
    fn list_reviews<'a>(
        &'a self,
        scope: &'a StorageScope,
        request: ListReviewsRequest,
    ) -> PortFuture<'a, ListReviewsResponse>;
    /// Persist what was returned, not merely a trace identifier.
    fn insert_receipt<'a>(
        &'a self,
        scope: &'a StorageScope,
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
