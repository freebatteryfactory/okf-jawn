//! Fakes of the ports the purge operations call, for the `tests/application` target's `purges`
//! module, which includes this file as `purge_fixture`: purge records with their tenant jobs, a
//! job queue that records each delivery, and a catalog of scripted workspaces. Not production
//! adapters.
//!
//! Each method a purge operation calls encodes the sentence of its port doc quoted above it.
//! Where the fake is kinder than storage: purges and jobs are kept in memory, so no write can
//! fail unless a test says so (`fail_next`); a tenant job is never claimed, run or cancelled
//! here (a test sets its state, `set_job`); and the catalog opens only the workspaces a test
//! put in it, archived or not, with no Git behind them. Every other method answers `Internal`
//! ("not used by this test"), so an operation that starts calling one fails loudly.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::Receipt;
use okf_jawn_contract::identity::{
    ArtifactId, Digest, ItemId, JobId, MutationId, PurgeId, ReceiptId, Revision, TenantId,
    Timestamp, WorkspaceId,
};
use okf_jawn_contract::import::{Job, JobState, ListJobsResponse};
use okf_jawn_contract::purge::{Purge, PurgeState, PurgeTarget};
use okf_jawn_contract::review::Review;
use okf_jawn_contract::workspace::Workspace;
use okf_jawn_core::application::{ApplicationService, Ports};
use okf_jawn_core::jobs::{
    ArtifactRecord, ClaimedJob, DerivedObject, JobCompletion, JobLease, JobQueue, JobScope,
    NewArtifact, NewJob, NewPurge, RecordStore, RevisionMapping, purge_job_spec,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    NewWorkspace, Page, Provenance, StorageScope, WorkspaceArchive, WorkspaceCatalog,
    WorkspaceUpdate,
};
use uuid::Uuid;

use crate::fixture::{Built, Unused, World, config};

/// The service over the purge fakes, and the fakes.
pub struct PurgeWorld {
    /// The service under test.
    pub service: ApplicationService,
    /// Purges and their tenant jobs.
    pub records: Arc<PurgeRecords>,
    /// Deliveries.
    pub queue: Arc<RecordingQueue>,
    /// Scripted workspaces.
    pub catalog: Arc<OpenCatalog>,
}

/// `RecordStore` of purge records and their tenant jobs, and scripted revision maps.
#[derive(Default)]
pub struct PurgeRecords {
    state: Mutex<RecordsState>,
}

#[derive(Default)]
struct RecordsState {
    /// Each purge in the order it was recorded, with its tenant and what it was recorded with.
    purges: Vec<Recorded>,
    /// Tenant jobs, with whether a failure of each may be retried.
    jobs: BTreeMap<JobId, (TenantId, Job, bool)>,
    /// The mutation of each retry that took effect.
    retries: Vec<MutationId>,
    /// Scripted revision maps.
    mappings: BTreeMap<Revision, RevisionMapping>,
    /// A purge another request records just before the next `create_purge` is applied.
    recorded_meanwhile: Option<(TenantId, Purge)>,
    /// The method whose next call fails, and how.
    fail_next: Option<(&'static str, ErrorCode)>,
    /// Every call of a method this fake models, in order.
    calls: Vec<&'static str>,
}

/// One purge as `PurgeRecords::recorded` answers it: the mutation and initiator `create_purge`
/// was given (none for one a test put or another request recorded), and the record.
pub type Kept = (Option<MutationId>, Option<Provenance>, Purge);

/// One purge as the fake keeps it.
#[derive(Clone)]
struct Recorded {
    tenant: TenantId,
    mutation: Option<MutationId>,
    initiator: Option<Provenance>,
    purge: Purge,
}

/// `JobQueue` that records every delivery.
#[derive(Default)]
pub struct RecordingQueue {
    deliveries: Mutex<Vec<(JobScope, JobId)>>,
    fail_next: Mutex<bool>,
}

/// `WorkspaceCatalog` that opens the workspaces a test put in it.
#[derive(Default)]
pub struct OpenCatalog {
    workspaces: Mutex<Vec<(TenantId, Workspace)>>,
    opens: Mutex<usize>,
}

/// The number of the first tenant job `create_purge` writes; a test numbers its own below.
const NEW_JOBS: u128 = 0x1_0000;

impl PurgeWorld {
    /// The service over fresh purge fakes and the application fixture's other fakes.
    ///
    /// # Errors
    /// Returns when the application fixture cannot be built.
    pub fn new() -> Built<Self> {
        let base = World::new()?;
        let records = Arc::new(PurgeRecords::default());
        let queue = Arc::new(RecordingQueue::default());
        let catalog = Arc::new(OpenCatalog::default());
        let unused = Arc::new(Unused);
        let ports = Ports {
            access: unused.clone(),
            mutations: unused.clone(),
            catalog: catalog.clone(),
            blobs: base.blobs.clone(),
            versions: base.versions.clone(),
            records: records.clone(),
            queue: queue.clone(),
            search: base.search.clone(),
            converter: unused.clone(),
            proposals: unused.clone(),
            uploads: unused.clone(),
            events: base.events.clone(),
            credentials: unused.clone(),
            drafts: base.drafts.clone(),
            confirmations: unused.clone(),
            sandbox: base.sandbox.clone(),
            readiness: base.readiness.clone(),
        };
        Ok(Self {
            service: ApplicationService::new(ports, config())?,
            records,
            queue,
            catalog,
        })
    }
}

impl PurgeRecords {
    /// Keep a purge as another request recorded it, with its tenant job in `job_state`, whose
    /// failure may be retried.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put(
        &self,
        tenant: &TenantId,
        purge: Purge,
        job_state: JobState,
    ) -> Result<(), ApiError> {
        let recorded = Recorded {
            tenant: tenant.clone(),
            mutation: None,
            initiator: None,
            purge,
        };
        lock(&self.state)?.keep(tenant, recorded, job_state);
        Ok(())
    }

    /// Set a tenant job's state, and whether its failure may be retried.
    ///
    /// # Errors
    /// Returns `NotFound` for an unknown job.
    pub fn set_job(
        &self,
        job: JobId,
        job_state: JobState,
        retryable: bool,
    ) -> Result<(), ApiError> {
        let mut state = lock(&self.state)?;
        let (_, stored, flag) = state.jobs.get_mut(&job).ok_or_else(not_found)?;
        stored.state = job_state;
        *flag = retryable;
        Ok(())
    }

    /// A tenant job as it stands.
    ///
    /// # Errors
    /// Returns `NotFound` for an unknown job.
    pub fn job(&self, job: JobId) -> Result<Job, ApiError> {
        lock(&self.state)?
            .jobs
            .get(&job)
            .map(|(_, stored, _)| stored.clone())
            .ok_or_else(not_found)
    }

    /// Every purge kept, in order: the mutation and initiator `create_purge` was given (none
    /// for one a test put or another request recorded), and the record.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn recorded(&self) -> Result<Vec<Kept>, ApiError> {
        Ok(lock(&self.state)?
            .purges
            .iter()
            .map(|recorded| {
                (
                    recorded.mutation,
                    recorded.initiator.clone(),
                    recorded.purge.clone(),
                )
            })
            .collect())
    }

    /// The mutation of each retry that took effect, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn retries(&self) -> Result<Vec<MutationId>, ApiError> {
        Ok(lock(&self.state)?.retries.clone())
    }

    /// Every call of a modelled method, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn calls(&self) -> Result<Vec<&'static str>, ApiError> {
        Ok(lock(&self.state)?.calls.clone())
    }

    /// Map `revision` as a purge removed or rewrote it.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn map(&self, revision: Revision, mapping: RevisionMapping) -> Result<(), ApiError> {
        let _replaced = lock(&self.state)?.mappings.insert(revision, mapping);
        Ok(())
    }

    /// Have another request record `purge` just before the next `create_purge` is applied, as
    /// one that lands between a request's lookup and its record would.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn record_meanwhile(&self, tenant: &TenantId, purge: Purge) -> Result<(), ApiError> {
        lock(&self.state)?.recorded_meanwhile = Some((tenant.clone(), purge));
        Ok(())
    }

    /// Make the next call of `method` fail with `code`, writing nothing.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_next(&self, method: &'static str, code: ErrorCode) -> Result<(), ApiError> {
        lock(&self.state)?.fail_next = Some((method, code));
        Ok(())
    }
}

impl RecordsState {
    /// Record a call of `method`, and fail it when a test said so.
    fn called(&mut self, method: &'static str) -> Result<(), ApiError> {
        self.calls.push(method);
        match self.fail_next.take() {
            Some((failing, code)) if failing == method => Err(ApiError::new(
                code,
                format!("{method} failed, as a test asked"),
            )),
            other => {
                self.fail_next = other;
                Ok(())
            }
        }
    }

    /// The purge `find_purge` and `create_purge` answer for `target` in `tenant`: the
    /// unfinished one, otherwise the first completed one.
    fn of_target(&self, tenant: &TenantId, target: &PurgeTarget) -> Option<Purge> {
        let of_target = || {
            self.purges
                .iter()
                .filter(|recorded| recorded.tenant == *tenant && recorded.purge.target == *target)
        };
        of_target()
            .find(|recorded| recorded.purge.state != PurgeState::Completed)
            .or_else(|| of_target().next())
            .map(|recorded| recorded.purge.clone())
    }

    /// Keep a purge with its tenant job in `job_state`.
    fn keep(&mut self, tenant: &TenantId, recorded: Recorded, job_state: JobState) {
        let purge = &recorded.purge;
        let job = tenant_job(purge.job_id, &purge.target, purge.id, job_state);
        let _replaced = self.jobs.insert(purge.job_id, (tenant.clone(), job, true));
        self.purges.push(recorded);
    }
}

impl RecordStore for PurgeRecords {
    fn create_job<'a>(&'a self, _scope: &'a JobScope, _job: NewJob) -> PortFuture<'a, Job> {
        unused()
    }
    fn job_mutation<'a>(&'a self, _scope: &'a JobScope, _job: JobId) -> PortFuture<'a, MutationId> {
        unused()
    }
    // "Read one durable job within its scope."
    fn get_job<'a>(&'a self, scope: &'a JobScope, job: JobId) -> PortFuture<'a, Job> {
        Box::pin(async move {
            let mut state = lock(&self.state)?;
            state.called("get_job")?;
            match (scope, state.jobs.get(&job)) {
                (JobScope::Tenant(tenant), Some((owner, stored, _))) if owner == tenant => {
                    Ok(stored.clone())
                }
                _ => Err(not_found()),
            }
        })
    }
    fn list_jobs<'a>(
        &'a self,
        _scope: &'a JobScope,
        _page: Page,
    ) -> PortFuture<'a, ListJobsResponse> {
        unused()
    }
    fn claim_job(&self, _job: JobId) -> PortFuture<'_, Option<ClaimedJob>> {
        unused()
    }
    fn update_progress<'a>(&'a self, _lease: &'a JobLease, _progress: u8) -> PortFuture<'a, Job> {
        unused()
    }
    fn complete_job(&self, _completion: JobCompletion) -> PortFuture<'_, Job> {
        unused()
    }
    fn fail_job(
        &self,
        _lease: JobLease,
        _error: ApiError,
        _retryable: bool,
    ) -> PortFuture<'_, Job> {
        unused()
    }
    fn seal<'a>(&'a self, _lease: &'a JobLease) -> PortFuture<'a, Job> {
        unused()
    }
    fn cancel_job<'a>(
        &'a self,
        _scope: &'a JobScope,
        _mutation_id: MutationId,
        _job: JobId,
    ) -> PortFuture<'a, Job> {
        unused()
    }
    // "Queue a failed job again under the same durable identity. Unique on `mutation_id`: a
    // repeated id does not queue the job a second time, even when the job has run and failed
    // again since, and returns the job row as it stands." A failure that is not retryable, and
    // a job that is not failed, are refused with `Conflict`, as storage refuses them.
    fn retry_job<'a>(
        &'a self,
        scope: &'a JobScope,
        mutation_id: MutationId,
        job: JobId,
    ) -> PortFuture<'a, Job> {
        Box::pin(async move {
            let mut state = lock(&self.state)?;
            state.called("retry_job")?;
            let repeated = state.retries.contains(&mutation_id);
            let (owner, stored, retryable) = state.jobs.get_mut(&job).ok_or_else(not_found)?;
            if !matches!(scope, JobScope::Tenant(tenant) if tenant == owner) {
                return Err(not_found());
            }
            if repeated {
                return Ok(stored.clone());
            }
            match stored.state {
                JobState::Failed if *retryable => {
                    stored.state = JobState::Queued;
                    stored.error = None;
                    let queued = stored.clone();
                    state.retries.push(mutation_id);
                    Ok(queued)
                }
                JobState::Failed => Err(ApiError::new(
                    ErrorCode::Conflict,
                    "this job's failure is not retryable",
                )),
                JobState::Queued
                | JobState::Running
                | JobState::Succeeded
                | JobState::Cancelled => Err(ApiError::new(
                    ErrorCode::Conflict,
                    "only a failed job is retried",
                )),
            }
        })
    }
    fn record_artifact<'a>(
        &'a self,
        _scope: &'a JobScope,
        _mutation_id: MutationId,
        _artifact: NewArtifact,
    ) -> PortFuture<'a, ArtifactRecord> {
        unused()
    }
    fn get_artifact<'a>(
        &'a self,
        _scope: &'a JobScope,
        _artifact: ArtifactId,
    ) -> PortFuture<'a, ArtifactRecord> {
        unused()
    }
    fn pending_jobs(&self) -> PortFuture<'_, Vec<(JobScope, JobId)>> {
        unused()
    }
    fn expire_leases(&self) -> PortFuture<'_, u32> {
        unused()
    }
    // "Record a purge and, in the same transaction, its tenant job (...) under `mutation_id`.
    // Unique on `purge.id`: a repeated id returns the prior purge. A request for a target
    // that a purge already names returns that purge and records nothing (...): the unfinished
    // one while there is one, (...) otherwise the completed one."
    fn create_purge<'a>(
        &'a self,
        tenant: &'a TenantId,
        mutation_id: MutationId,
        purge: NewPurge,
    ) -> PortFuture<'a, Purge> {
        Box::pin(async move {
            let mut state = lock(&self.state)?;
            state.called("create_purge")?;
            if let Some((other_tenant, other)) = state.recorded_meanwhile.take() {
                let recorded = Recorded {
                    tenant: other_tenant.clone(),
                    mutation: None,
                    initiator: None,
                    purge: other,
                };
                state.keep(&other_tenant, recorded, JobState::Queued);
            }
            if let Some(prior) = state
                .purges
                .iter()
                .find(|recorded| recorded.tenant == *tenant && recorded.purge.id == purge.id)
            {
                return Ok(prior.purge.clone());
            }
            if let Some(prior) = state.of_target(tenant, &purge.target) {
                return Ok(prior);
            }
            let number = u128::try_from(state.jobs.len())
                .map_err(|_| ApiError::new(ErrorCode::Internal, "too many fixture jobs"))?;
            let recorded = Purge {
                id: purge.id,
                target: purge.target.clone(),
                state: PurgeState::Requested,
                job_id: JobId(Uuid::from_u128(NEW_JOBS.saturating_add(number))),
                requested_by: purge.initiator.subject.clone(),
                requested_at: instant()?,
                completed_at: None,
                report: None,
                error: None,
            };
            state.keep(
                tenant,
                Recorded {
                    tenant: tenant.clone(),
                    mutation: Some(mutation_id),
                    initiator: Some(purge.initiator),
                    purge: recorded.clone(),
                },
                JobState::Queued,
            );
            Ok(recorded)
        })
    }
    // "Read one purge; it outlives the workspace it removed."
    fn get_purge<'a>(&'a self, tenant: &'a TenantId, purge: PurgeId) -> PortFuture<'a, Purge> {
        Box::pin(async move {
            let mut state = lock(&self.state)?;
            state.called("get_purge")?;
            state
                .purges
                .iter()
                .find(|recorded| recorded.tenant == *tenant && recorded.purge.id == purge)
                .map(|recorded| recorded.purge.clone())
                .ok_or_else(not_found)
        })
    }
    // "The purge a request for `target` is answered with: the unfinished purge of the target
    // while there is one, otherwise the first completed one; `None` when no purge names the
    // target. Nothing is recorded."
    fn find_purge<'a>(
        &'a self,
        tenant: &'a TenantId,
        target: &'a PurgeTarget,
    ) -> PortFuture<'a, Option<Purge>> {
        Box::pin(async move {
            let mut state = lock(&self.state)?;
            state.called("find_purge")?;
            Ok(state.of_target(tenant, target))
        })
    }
    fn update_purge<'a>(&'a self, _tenant: &'a TenantId, _purge: Purge) -> PortFuture<'a, Purge> {
        unused()
    }
    // "What became of a revision a purge rewrote or removed; `None` when no purge touched it."
    fn revision_mapping<'a>(
        &'a self,
        _scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Option<RevisionMapping>> {
        Box::pin(async move {
            let mut state = lock(&self.state)?;
            state.called("revision_mapping")?;
            Ok(state.mappings.get(revision).cloned())
        })
    }
    fn record_derived_object<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _object: DerivedObject,
    ) -> PortFuture<'a, DerivedObject> {
        unused()
    }
    fn derived_object<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _item: ItemId,
        _revision: &'a Revision,
        _digest: &'a Digest,
    ) -> PortFuture<'a, Option<DerivedObject>> {
        unused()
    }
    fn insert_review<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _review: Review,
    ) -> PortFuture<'a, Review> {
        unused()
    }
    fn list_reviews<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _item: ItemId,
    ) -> PortFuture<'a, Vec<Review>> {
        unused()
    }
    fn insert_receipt<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: Option<MutationId>,
        _receipt: Receipt,
    ) -> PortFuture<'a, Receipt> {
        unused()
    }
    fn get_receipt<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _receipt: ReceiptId,
    ) -> PortFuture<'a, Receipt> {
        unused()
    }
}

impl RecordingQueue {
    /// Every delivery so far, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn deliveries(&self) -> Result<Vec<(JobScope, JobId)>, ApiError> {
        Ok(lock(&self.deliveries)?.clone())
    }

    /// Make the next delivery fail with `Unavailable`, delivering nothing.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_next(&self) -> Result<(), ApiError> {
        *lock(&self.fail_next)? = true;
        Ok(())
    }
}

impl JobQueue for RecordingQueue {
    // "Deliver an existing durable job identity, accepting possible duplicate delivery."
    fn enqueue(&self, scope: JobScope, job: JobId) -> PortFuture<'_, ()> {
        Box::pin(async move {
            let mut failing = lock(&self.fail_next)?;
            if *failing {
                *failing = false;
                return Err(ApiError::new(
                    ErrorCode::Unavailable,
                    "the delivery failed, as a test asked",
                ));
            }
            lock(&self.deliveries)?.push((scope, job));
            Ok(())
        })
    }
}

impl OpenCatalog {
    /// Put `workspace` in `tenant`'s catalog, in place of the one of the same identity, as a
    /// commit that moved its head would leave it.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put(&self, tenant: &TenantId, workspace: Workspace) -> Result<(), ApiError> {
        let mut workspaces = lock(&self.workspaces)?;
        workspaces.retain(|(held, kept)| !(held == tenant && kept.id == workspace.id));
        workspaces.push((tenant.clone(), workspace));
        Ok(())
    }

    /// How many times a workspace was opened.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn opens(&self) -> Result<usize, ApiError> {
        Ok(*lock(&self.opens)?)
    }
}

impl WorkspaceCatalog for OpenCatalog {
    fn list<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _include_archived: bool,
    ) -> PortFuture<'a, Vec<Workspace>> {
        unused()
    }
    fn create<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _mutation_id: MutationId,
        _workspace: NewWorkspace,
    ) -> PortFuture<'a, Workspace> {
        unused()
    }
    // "archived or not; `NotFound` when the tenant has no such workspace"
    fn open<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Workspace> {
        Box::pin(async move {
            let mut opens = lock(&self.opens)?;
            *opens = opens.saturating_add(1);
            drop(opens);
            lock(&self.workspaces)?
                .iter()
                .find(|(tenant, workspace)| {
                    *tenant == scope.tenant_id && workspace.id == scope.workspace_id
                })
                .map(|(_, workspace)| workspace.clone())
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such workspace"))
        })
    }
    fn update<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _update: WorkspaceUpdate,
    ) -> PortFuture<'a, Workspace> {
        unused()
    }
    fn archive<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _archive: WorkspaceArchive,
    ) -> PortFuture<'a, Workspace> {
        unused()
    }
    fn unarchive<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _author: Provenance,
    ) -> PortFuture<'a, Workspace> {
        unused()
    }
}

/// The tenant job that carries out a purge, in `job_state`.
fn tenant_job(id: JobId, target: &PurgeTarget, purge: PurgeId, job_state: JobState) -> Job {
    Job {
        id,
        workspace_id: None,
        kind: purge_job_spec(target, purge).kind(),
        state: job_state,
        progress: 0,
        attempt: 0,
        warnings: Vec::new(),
        error: None,
        revision: None,
        item_ids: Vec::new(),
        artifact: None,
        restore: None,
        backup: None,
        created_at: None,
        started_at: None,
        finished_at: None,
        retryable: None,
    }
}

/// The instant every purge of this fixture is requested at.
fn instant() -> Result<Timestamp, ApiError> {
    Timestamp::try_from("2026-10-15T00:00:00.000Z".to_owned())
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, ApiError> {
    mutex
        .lock()
        .map_err(|_| ApiError::new(ErrorCode::Internal, "a fixture lock is poisoned"))
}

fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "not found in the purge fixture")
}

/// A future that answers that the method is not used.
fn unused<'a, T: Send + 'a>() -> PortFuture<'a, T> {
    Box::pin(async { Err(ApiError::new(ErrorCode::Internal, "not used by this test")) })
}

/// A workspace identity from a number.
#[must_use]
pub const fn workspace_id(number: u128) -> WorkspaceId {
    WorkspaceId(Uuid::from_u128(number))
}
