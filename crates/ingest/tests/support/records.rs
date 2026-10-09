//! An in-memory `RecordStore` for the runtime and handler tests: the job ledger only.
//!
//! It encodes what the `RecordStore` docs state about jobs:
//! - `claim_job` claims a queued job under a new lease token and returns `None` otherwise;
//! - progress, completion and failure need the current, unexpired claim;
//! - `expire_leases` returns expired claims to the queue;
//! - `pending_jobs` lists unfinished jobs;
//! - `record_artifact` is unique on the mutation id.
//!
//! Time does not pass by itself: `expire_running` marks every live claim expired, as a lease
//! lapsing would. Purge, review, receipt and derived-object records are not modelled; those
//! methods answer `NotImplemented`, so a test that reaches one fails loudly.

use std::collections::BTreeMap;
use std::sync::Mutex;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::Receipt;
use okf_jawn_contract::identity::{
    ArtifactId, Digest, ItemId, JobId, MutationId, PurgeId, ReceiptId, Revision, TenantId,
    Timestamp,
};
use okf_jawn_contract::import::{Job, JobState, ListJobsResponse};
use okf_jawn_contract::purge::Purge;
use okf_jawn_contract::review::Review;
use okf_jawn_core::jobs::{
    ArtifactRecord, ClaimedJob, DerivedObject, JobCompletion, JobLease, JobScope, NewArtifact,
    NewJob, NewPurge, RecordStore, RevisionMapping,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{Page, StorageScope};

/// One durable job as the fake keeps it.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The record as `get_job` would return it.
    pub job: Job,
    /// Scope of the job.
    pub scope: JobScope,
    /// What it was created with.
    pub created: NewJob,
    /// The live claim: its token, and whether it has expired.
    pub lease: Option<(String, bool)>,
    /// Completions written, in order.
    pub completions: Vec<JobCompletion>,
    /// Failures written: message and retry eligibility.
    pub failures: Vec<(String, bool)>,
}

/// The ledger.
#[derive(Debug, Default)]
pub struct FakeRecords {
    entries: Mutex<BTreeMap<JobId, Entry>>,
    artifacts: Mutex<BTreeMap<MutationId, ArtifactRecord>>,
}

impl FakeRecords {
    /// Register a queued job directly, as `create_job` would.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn insert(&self, id: JobId, scope: JobScope, created: NewJob) -> Result<(), ApiError> {
        let job = Job {
            id,
            workspace_id: scope.workspace().copied(),
            kind: created.spec.kind(),
            state: JobState::Queued,
            progress: 0,
            attempt: 0,
            warnings: Vec::new(),
            error: None,
            revision: None,
            item_ids: Vec::new(),
            artifact: None,
            restore: None,
        };
        self.lock()?.insert(
            id,
            Entry {
                job,
                scope,
                created,
                lease: None,
                completions: Vec::new(),
                failures: Vec::new(),
            },
        );
        Ok(())
    }

    /// A job as it stands.
    ///
    /// # Errors
    /// Returns `NotFound` for an unknown job.
    pub fn entry(&self, id: JobId) -> Result<Entry, ApiError> {
        self.lock()?.get(&id).cloned().ok_or_else(not_found)
    }

    /// Mark every live claim expired, as if its lease had lapsed.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn expire_running(&self) -> Result<(), ApiError> {
        for entry in self.lock()?.values_mut() {
            if let Some((_, expired)) = entry.lease.as_mut() {
                *expired = true;
            }
        }
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<JobId, Entry>>, ApiError> {
        self.entries
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "fake record store lock poisoned"))
    }

    /// The entry the lease currently holds, or `Conflict`.
    fn with_lease<T>(
        &self,
        lease: &JobLease,
        change: impl FnOnce(&mut Entry) -> T,
    ) -> Result<T, ApiError> {
        let mut entries = self.lock()?;
        let entry = entries.get_mut(&lease.job_id).ok_or_else(not_found)?;
        match &entry.lease {
            Some((token, false)) if *token == lease.token => Ok(change(entry)),
            Some(_) | None => Err(ApiError::new(
                ErrorCode::Conflict,
                "the lease is not the current unexpired claim",
            )),
        }
    }
}

impl RecordStore for FakeRecords {
    fn create_job<'a>(&'a self, _scope: &'a JobScope, _job: NewJob) -> PortFuture<'a, Job> {
        unmodelled("create_job")
    }

    fn get_job<'a>(&'a self, _scope: &'a JobScope, job: JobId) -> PortFuture<'a, Job> {
        let found = self.entry(job).map(|entry| entry.job);
        Box::pin(async move { found })
    }

    fn list_jobs<'a>(
        &'a self,
        _scope: &'a JobScope,
        _page: Page,
    ) -> PortFuture<'a, ListJobsResponse> {
        unmodelled("list_jobs")
    }

    fn claim_job(&self, job: JobId) -> PortFuture<'_, Option<ClaimedJob>> {
        let claimed = self.lock().map(|mut entries| {
            let entry = entries.get_mut(&job)?;
            if entry.job.state != JobState::Queued {
                return None;
            }
            entry.job.state = JobState::Running;
            entry.job.attempt = entry.job.attempt.saturating_add(1);
            let token = format!("lease-{}", entry.job.attempt);
            entry.lease = Some((token.clone(), false));
            Some(ClaimedJob {
                scope: entry.scope.clone(),
                lease: JobLease {
                    job_id: job,
                    token,
                    attempt: entry.job.attempt,
                    expires_at: Timestamp::try_from("2026-10-09T00:10:00.000Z".to_owned()).ok()?,
                },
                job: entry.job.clone(),
                mutation_id: entry.created.mutation_id,
                initiator: entry.created.initiator.clone(),
                spec: entry.created.spec.clone(),
            })
        });
        Box::pin(async move { claimed })
    }

    fn update_progress<'a>(&'a self, lease: &'a JobLease, progress: u8) -> PortFuture<'a, Job> {
        let updated = self.with_lease(lease, |entry| {
            entry.job.progress = progress;
            entry.job.clone()
        });
        Box::pin(async move { updated })
    }

    fn complete_job(&self, completion: JobCompletion) -> PortFuture<'_, Job> {
        let lease = completion.lease.clone();
        let done = self.with_lease(&lease, |entry| {
            entry.job.state = JobState::Succeeded;
            entry.job.progress = 100;
            entry.job.revision.clone_from(&completion.revision);
            entry.job.item_ids.clone_from(&completion.item_ids);
            entry.job.restore.clone_from(&completion.restore);
            entry.lease = None;
            entry.completions.push(completion);
            entry.job.clone()
        });
        Box::pin(async move { done })
    }

    fn fail_job(&self, lease: JobLease, message: String, retryable: bool) -> PortFuture<'_, Job> {
        let failed = self.with_lease(&lease, |entry| {
            entry.job.state = JobState::Failed;
            entry.job.error = Some(ApiError::new(ErrorCode::Internal, message.clone()));
            entry.lease = None;
            entry.failures.push((message, retryable));
            entry.job.clone()
        });
        Box::pin(async move { failed })
    }

    fn cancel_job<'a>(
        &'a self,
        _scope: &'a JobScope,
        _mutation_id: MutationId,
        _job: JobId,
    ) -> PortFuture<'a, Job> {
        unmodelled("cancel_job")
    }

    fn retry_job<'a>(
        &'a self,
        _scope: &'a JobScope,
        _mutation_id: MutationId,
        _job: JobId,
    ) -> PortFuture<'a, Job> {
        unmodelled("retry_job")
    }

    fn record_artifact<'a>(
        &'a self,
        scope: &'a JobScope,
        mutation_id: MutationId,
        artifact: NewArtifact,
    ) -> PortFuture<'a, ArtifactRecord> {
        let recorded = self
            .artifacts
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "fake artifact lock poisoned"))
            .and_then(|mut artifacts| {
                if let Some(prior) = artifacts.get(&mutation_id) {
                    return Ok(prior.clone());
                }
                let next = u128::try_from(artifacts.len())
                    .unwrap_or(u128::MAX)
                    .saturating_add(1);
                let record = ArtifactRecord {
                    id: artifact_id(next)
                        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?,
                    scope: scope.clone(),
                    kind: artifact.kind,
                    object: artifact.object,
                    media_type: artifact.media_type,
                    created_by_job: artifact.created_by_job,
                };
                artifacts.insert(mutation_id, record.clone());
                Ok(record)
            });
        Box::pin(async move { recorded })
    }

    fn get_artifact<'a>(
        &'a self,
        _scope: &'a JobScope,
        _artifact: ArtifactId,
    ) -> PortFuture<'a, ArtifactRecord> {
        unmodelled("get_artifact")
    }

    fn pending_jobs(&self) -> PortFuture<'_, Vec<(JobScope, JobId)>> {
        let pending = self.lock().map(|entries| {
            entries
                .iter()
                .filter(|(_, entry)| {
                    matches!(entry.job.state, JobState::Queued | JobState::Running)
                })
                .map(|(id, entry)| (entry.scope.clone(), *id))
                .collect()
        });
        Box::pin(async move { pending })
    }

    fn expire_leases(&self) -> PortFuture<'_, u32> {
        let released = self.lock().map(|mut entries| {
            let mut released = 0_u32;
            for entry in entries.values_mut() {
                if matches!(entry.lease, Some((_, true))) {
                    entry.lease = None;
                    entry.job.state = JobState::Queued;
                    released = released.saturating_add(1);
                }
            }
            released
        });
        Box::pin(async move { released })
    }

    fn create_purge<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _mutation_id: MutationId,
        _purge: NewPurge,
    ) -> PortFuture<'a, Purge> {
        unmodelled("create_purge")
    }

    fn get_purge<'a>(&'a self, _tenant: &'a TenantId, _purge: PurgeId) -> PortFuture<'a, Purge> {
        unmodelled("get_purge")
    }

    fn update_purge<'a>(&'a self, _tenant: &'a TenantId, _purge: Purge) -> PortFuture<'a, Purge> {
        unmodelled("update_purge")
    }

    fn revision_mapping<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
    ) -> PortFuture<'a, Option<RevisionMapping>> {
        unmodelled("revision_mapping")
    }

    fn record_derived_object<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _object: DerivedObject,
    ) -> PortFuture<'a, DerivedObject> {
        unmodelled("record_derived_object")
    }

    fn derived_object<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _item: ItemId,
        _revision: &'a Revision,
        _digest: &'a Digest,
    ) -> PortFuture<'a, Option<DerivedObject>> {
        unmodelled("derived_object")
    }

    fn insert_review<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _review: Review,
    ) -> PortFuture<'a, Review> {
        unmodelled("insert_review")
    }

    fn list_reviews<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _item: ItemId,
    ) -> PortFuture<'a, Vec<Review>> {
        unmodelled("list_reviews")
    }

    fn insert_receipt<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: Option<MutationId>,
        _receipt: Receipt,
    ) -> PortFuture<'a, Receipt> {
        unmodelled("insert_receipt")
    }

    fn get_receipt<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _receipt: ReceiptId,
    ) -> PortFuture<'a, Receipt> {
        unmodelled("get_receipt")
    }
}

/// A job identity built from a number.
///
/// # Errors
/// Never for a number: every one is a UUID.
pub fn job_id(value: u128) -> Result<JobId, serde_json::Error> {
    serde_json::from_value(serde_json::Value::String(uuid_text(value)))
}

/// An artifact identity built from a number.
///
/// # Errors
/// Never for a number: every one is a UUID.
pub fn artifact_id(value: u128) -> Result<ArtifactId, serde_json::Error> {
    serde_json::from_value(serde_json::Value::String(uuid_text(value)))
}

/// A UUID spelled from a number.
fn uuid_text(value: u128) -> String {
    let hex = format!("{value:032x}");
    let part = |from: usize, to: usize| hex.get(from..to).unwrap_or_default().to_owned();
    format!(
        "{}-{}-{}-{}-{}",
        part(0, 8),
        part(8, 12),
        part(12, 16),
        part(16, 20),
        part(20, 32)
    )
}

fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "no such job in the fake record store")
}

fn unmodelled<'a, T: Send + 'a>(method: &'static str) -> PortFuture<'a, T> {
    Box::pin(async move {
        Err(ApiError::new(
            ErrorCode::NotImplemented,
            format!("the fake record store does not model {method}"),
        ))
    })
}
