//! Durable application work records are independent of queue delivery and telemetry.

use crate::ports::PortFuture;
use okf_jawn_contract::{
    events::Receipt,
    identity::{Digest, JobId, WorkspaceId},
    import::Job,
    review::Review,
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

/// A durable completion result, never inferred solely from a queue acknowledgment.
#[derive(Debug, Clone)]
pub struct JobCompletion {
    /// Compare-and-set claim identity.
    pub lease: JobLease,
    /// Retained output object references.
    pub outputs: Vec<Digest>,
    /// A committed content revision when the job changes a workspace.
    pub revision: Option<okf_jawn_contract::identity::Revision>,
}

/// SQLite-backed non-rebuildable application records.
pub trait RecordStore: Send + Sync {
    /// Insert work idempotently, conflicting if a key is reused with a different payload.
    fn create_job(&self, job: Job, key: String, request_digest: Digest) -> PortFuture<'_, Job>;
    /// Claim runnable work under a compare-and-set lease.
    fn claim_job(&self, workspace: WorkspaceId, job: JobId) -> PortFuture<'_, Option<JobLease>>;
    /// Commit completion only for the current unexpired claim.
    fn complete_job(&self, completion: JobCompletion) -> PortFuture<'_, Job>;
    /// Keep the failure and retry eligibility for the current claim.
    fn fail_job(&self, lease: JobLease, message: String, retryable: bool) -> PortFuture<'_, Job>;
    /// Enumerate unfinished records for queue reconciliation on restart.
    fn pending_jobs(&self) -> PortFuture<'_, Vec<Job>>;
    /// Record exact reviewed content after application-level confirmation.
    fn insert_review(&self, review: Review) -> PortFuture<'_, Review>;
    /// Persist what was returned, not merely a trace identifier.
    fn insert_receipt(&self, receipt: Receipt) -> PortFuture<'_, Receipt>;
}

/// Delivery adapter; job truth remains in `RecordStore`.
pub trait JobQueue: Send + Sync {
    /// Deliver an existing durable job identity, accepting possible duplicate delivery.
    fn enqueue(&self, workspace: WorkspaceId, job: JobId) -> PortFuture<'_, ()>;
}
