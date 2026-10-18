//! An in-memory `RecordStore` for the runtime and handler tests (the job ledger only), and an
//! `EventLog` that records what the job writes announce.
//!
//! It encodes what the `RecordStore` docs state about jobs:
//! - `claim_job` claims a queued job under a new lease token and returns `None` otherwise, and
//!   refuses with `Conflict` a job of a workspace a purge is recorded for (`purge_workspace_of`);
//! - progress, completion and failure need the current, unexpired claim;
//! - completion and failure also need the job to be running, as storage's `complete` and
//!   `fail_job` refuse a row another writer cancelled (`lost_claim`);
//! - `seal` needs the current, unexpired claim of a running job, returns a cancelled job as it
//!   is, unsealed, and seals for the job's life (a lapsed lease queued again keeps it);
//! - `cancel_job` is unique on its mutation id (a repeated id returns the job as it stands),
//!   refuses a sealed queued or running job with `Conflict` and records nothing then, cancels a
//!   queued or running one (keeping the claim's token, so the handler learns it from its next
//!   `update_progress`), and refuses a finished one with `Conflict`;
//! - `list_jobs` lists one scope's jobs, newest first, paged from the offset its decimal cursor
//!   names (a test may make the pages small, `page_jobs`, as several pages of a long listing);
//! - `expire_leases` returns expired claims to the queue;
//! - `pending_jobs` lists unfinished jobs;
//! - `record_artifact` is unique on the mutation id.
//!
//! Time does not pass by itself: `expire_running` marks every live claim expired, as a lease
//! lapsing would. Purge records are kept as given (`get_purge`, `update_purge`, which a test
//! may make fail once, `fail_purge_update`); review, receipt and derived-object records are
//! not modelled; those methods answer `NotImplemented`, so a test that reaches one fails
//! loudly.
//!
//! `FakeEvents` encodes the `EventLog::append` doc: with a mutation identity, an identical
//! notification already appended under it in the same log is not appended again; without one,
//! every notification is appended. `list` is not modelled. `fail_on` makes one append fail
//! with `Unavailable`, appending nothing, as a store that could not write would.

use std::collections::BTreeMap;
use std::sync::Mutex;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{Event, EventKind, ListEventsResponse, Receipt};
use okf_jawn_contract::identity::{
    ArtifactId, Digest, ItemId, JobId, MutationId, PurgeId, ReceiptId, Revision, TenantId,
    Timestamp,
};
use okf_jawn_contract::import::{Job, JobState, ListJobsResponse};
use okf_jawn_contract::purge::{Purge, PurgeState, PurgeTarget};
use okf_jawn_contract::review::Review;
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
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
    /// Sealed past its point of no return (`seal`).
    pub sealed: bool,
    /// Its place among the jobs inserted: a later job is newer.
    pub order: usize,
}

/// The ledger.
#[derive(Debug, Default)]
pub struct FakeRecords {
    entries: Mutex<BTreeMap<JobId, Entry>>,
    artifacts: Mutex<BTreeMap<MutationId, ArtifactRecord>>,
    purges: Mutex<BTreeMap<PurgeId, Purge>>,
    renewals: std::sync::atomic::AtomicUsize,
    /// A job cancelled the moment it is claimed, as a cancel landing right after the claim.
    cancel_on_claim: Mutex<Option<JobId>>,
    /// A job cancelled just before its next seal is applied, as a cancel the store took first.
    cancel_on_seal: Mutex<Option<JobId>>,
    /// Jobs of a workspace a purge is recorded for: their claim is refused with `Conflict`.
    purged: Mutex<Vec<JobId>>,
    /// Each cancellation that took effect: its mutation and the job it cancelled.
    cancels: Mutex<Vec<(MutationId, JobId)>>,
    /// Every `cancel_job` call, refused or not: its mutation and job.
    cancel_calls: Mutex<Vec<(MutationId, JobId)>>,
    /// The most jobs one page of `list_jobs` holds, when a test makes listings long.
    list_page: Mutex<Option<usize>>,
    /// Jobs inserted so far, for their order.
    inserted: std::sync::atomic::AtomicUsize,
    /// The purge state whose next `update_purge` fails, writing nothing, once.
    failing_purge_update: Mutex<Option<PurgeState>>,
}

/// One notification as the fake log keeps it: its log, its mutation identity and the event.
pub type Appended = (EventScope, Option<MutationId>, NewEvent);

/// The event log.
#[derive(Debug, Default)]
pub struct FakeEvents {
    appended: Mutex<Vec<Appended>>,
    /// The `n`-th append of this kind (1-based, counting those appended) fails; once.
    fail: Mutex<Option<(EventKind, usize)>>,
}

impl FakeEvents {
    /// Every notification appended so far, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn appended(&self) -> Result<Vec<Appended>, ApiError> {
        self.appended
            .lock()
            .map(|appended| appended.clone())
            .map_err(|_| poisoned())
    }

    /// Make the `n`-th append of `kind` (1-based, counting those appended) fail, once.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_on(&self, kind: EventKind, n: usize) -> Result<(), ApiError> {
        *self.fail.lock().map_err(|_| poisoned())? = Some((kind, n));
        Ok(())
    }
}

impl EventLog for FakeEvents {
    fn append<'a>(
        &'a self,
        scope: &'a EventScope,
        mutation_id: Option<MutationId>,
        event: NewEvent,
    ) -> PortFuture<'a, Event> {
        let appended =
            self.appended
                .lock()
                .map_err(|_| poisoned())
                .and_then(|mut log| {
                    let mut fail = self.fail.lock().map_err(|_| poisoned())?;
                    let earlier = log
                        .iter()
                        .filter(|(_, _, prior)| prior.kind == event.kind)
                        .count();
                    if fail.as_ref().is_some_and(|(kind, n)| {
                        *kind == event.kind && *n == earlier.saturating_add(1)
                    }) {
                        *fail = None;
                        return Err(ApiError::new(
                            ErrorCode::Unavailable,
                            "the event could not be appended",
                        ));
                    }
                    let repeated = mutation_id.is_some()
                        && log.iter().any(|(earlier, mutation, prior)| {
                            earlier == scope && *mutation == mutation_id && *prior == event
                        });
                    if !repeated {
                        log.push((scope.clone(), mutation_id, event.clone()));
                    }
                    let at = Timestamp::try_from("2026-10-09T00:00:00.000Z".to_owned())
                        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
                    Ok(Event {
                        id: log.len().to_string(),
                        workspace_id: match scope {
                            EventScope::Workspace(workspace) => Some(workspace.workspace_id),
                            EventScope::Tenant(_) => None,
                        },
                        kind: event.kind,
                        at,
                        revision: event.revision,
                        item_id: event.item_id,
                        job_id: event.job_id,
                        connector_id: event.connector_id,
                        actor: event.actor,
                        operation: event.operation,
                        transport: None,
                        proposal_id: event.proposal_id,
                        count: event.count,
                    })
                });
        Box::pin(async move { appended })
    }

    fn list<'a>(
        &'a self,
        _scope: &'a EventScope,
        _query: EventQuery,
    ) -> PortFuture<'a, ListEventsResponse> {
        unmodelled("list")
    }
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
            backup: None,
            created_at: None,
            started_at: None,
            finished_at: None,
            retryable: None,
        };
        let mut entries = self.lock()?;
        // Inserted again, the job keeps its place: it is the same job, queued afresh.
        let order = match entries.get(&id) {
            Some(entry) => entry.order,
            None => self
                .inserted
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        };
        entries.insert(
            id,
            Entry {
                job,
                scope,
                created,
                lease: None,
                completions: Vec::new(),
                failures: Vec::new(),
                sealed: false,
                order,
            },
        );
        Ok(())
    }

    /// Make every page of `list_jobs` hold at most `size` jobs, as a long listing comes in
    /// several pages.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn page_jobs(&self, size: usize) -> Result<(), ApiError> {
        *self.list_page.lock().map_err(|_| poisoned())? = Some(size);
        Ok(())
    }

    /// Each cancellation that took effect, in order: its mutation and the job.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn cancels(&self) -> Result<Vec<(MutationId, JobId)>, ApiError> {
        Ok(self.cancels.lock().map_err(|_| poisoned())?.clone())
    }

    /// Every `cancel_job` call, refused or not, in order: its mutation and the job.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn cancel_calls(&self) -> Result<Vec<(MutationId, JobId)>, ApiError> {
        Ok(self.cancel_calls.lock().map_err(|_| poisoned())?.clone())
    }

    /// End a running job as succeeded, as its own attempt completing would.
    ///
    /// # Errors
    /// Returns `NotFound` for an unknown job.
    pub fn finish(&self, id: JobId) -> Result<(), ApiError> {
        let mut entries = self.lock()?;
        let entry = entries.get_mut(&id).ok_or_else(not_found)?;
        entry.job.state = JobState::Succeeded;
        entry.lease = None;
        Ok(())
    }

    /// Keep a purge record, as `create_purge` would.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn insert_purge(&self, purge: Purge) -> Result<(), ApiError> {
        self.purges
            .lock()
            .map_err(|_| poisoned())?
            .insert(purge.id, purge);
        Ok(())
    }

    /// A purge record as it stands.
    ///
    /// # Errors
    /// Returns `NotFound` for an unknown purge.
    pub fn purge(&self, id: PurgeId) -> Result<Purge, ApiError> {
        self.purges
            .lock()
            .map_err(|_| poisoned())?
            .get(&id)
            .cloned()
            .ok_or_else(not_found)
    }

    /// The artifacts recorded, by mutation id.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn artifacts(&self) -> Result<Vec<ArtifactRecord>, ApiError> {
        Ok(self
            .artifacts
            .lock()
            .map_err(|_| poisoned())?
            .values()
            .cloned()
            .collect())
    }

    /// Set the progress a job's record holds, as an earlier attempt would have left it.
    ///
    /// # Errors
    /// Returns `NotFound` for an unknown job.
    pub fn set_progress(&self, id: JobId, progress: u8) -> Result<(), ApiError> {
        self.lock()?
            .get_mut(&id)
            .ok_or_else(not_found)?
            .job
            .progress = progress;
        Ok(())
    }

    /// A job as it stands.
    ///
    /// # Errors
    /// Returns `NotFound` for an unknown job.
    pub fn entry(&self, id: JobId) -> Result<Entry, ApiError> {
        self.lock()?.get(&id).cloned().ok_or_else(not_found)
    }

    /// How many `update_progress` calls renewed a lease so far.
    #[must_use]
    pub fn renewals(&self) -> usize {
        self.renewals.load(std::sync::atomic::Ordering::SeqCst)
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

    /// Cancel `id` the moment it is next claimed, before its handler does anything.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn cancel_when_claimed(&self, id: JobId) -> Result<(), ApiError> {
        *self.cancel_on_claim.lock().map_err(|_| poisoned())? = Some(id);
        Ok(())
    }

    /// Cancel `id` just before its next seal is applied, as a cancellation the store took
    /// first would.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn cancel_before_seal(&self, id: JobId) -> Result<(), ApiError> {
        *self.cancel_on_seal.lock().map_err(|_| poisoned())? = Some(id);
        Ok(())
    }

    /// Record a purge of `id`'s workspace: from now on its claim is refused with `Conflict`
    /// naming the purge and the job is left as it is, as storage's `claim_job` does.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn purge_workspace_of(&self, id: JobId) -> Result<(), ApiError> {
        self.purged.lock().map_err(|_| poisoned())?.push(id);
        Ok(())
    }

    /// Cancel a running job, as `cancel_job` would: the state is `Cancelled` and the claim is
    /// left as it was, so the handler learns it from the next `update_progress`.
    ///
    /// # Errors
    /// Returns `NotFound` for an unknown job.
    pub fn cancel(&self, id: JobId) -> Result<(), ApiError> {
        self.lock()?.get_mut(&id).ok_or_else(not_found)?.job.state = JobState::Cancelled;
        Ok(())
    }

    /// Make the next `update_purge` that records `state` fail with `Unavailable`, writing
    /// nothing, once, as a store that could not write would.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_purge_update(&self, state: PurgeState) -> Result<(), ApiError> {
        *self.failing_purge_update.lock().map_err(|_| poisoned())? = Some(state);
        Ok(())
    }

    /// The failure a test asked of an `update_purge` recording `state`, taken.
    fn refused_update(&self, state: PurgeState) -> Result<(), ApiError> {
        let mut failing = self.failing_purge_update.lock().map_err(|_| poisoned())?;
        if failing.take_if(|failing| *failing == state).is_some() {
            return Err(ApiError::new(
                ErrorCode::Unavailable,
                "the purge record could not be written",
            ));
        }
        Ok(())
    }

    /// `cancel_job`, as its port doc states it (quoted there).
    fn cancel_entry(
        &self,
        scope: &JobScope,
        mutation_id: MutationId,
        job: JobId,
    ) -> Result<Job, ApiError> {
        self.cancel_calls
            .lock()
            .map_err(|_| poisoned())?
            .push((mutation_id, job));
        let mut entries = self.lock()?;
        let entry = entries
            .get_mut(&job)
            .filter(|entry| entry.scope == *scope)
            .ok_or_else(not_found)?;
        let mut cancels = self.cancels.lock().map_err(|_| poisoned())?;
        if cancels.iter().any(|(mutation, _)| *mutation == mutation_id) {
            return Ok(entry.job.clone());
        }
        match entry.job.state {
            JobState::Queued | JobState::Running if entry.sealed => Err(ApiError::new(
                ErrorCode::Conflict,
                "this job has passed its point of no return; it runs to its end",
            )),
            JobState::Queued | JobState::Running => {
                entry.job.state = JobState::Cancelled;
                cancels.push((mutation_id, job));
                Ok(entry.job.clone())
            }
            JobState::Succeeded | JobState::Failed | JobState::Cancelled => Err(ApiError::new(
                ErrorCode::Conflict,
                "a finished job cannot be cancelled",
            )),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<JobId, Entry>>, ApiError> {
        self.entries
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "fake record store lock poisoned"))
    }

    /// The entry the lease currently holds while the job runs, or `Conflict`.
    fn running_with_lease<T>(
        &self,
        lease: &JobLease,
        change: impl FnOnce(&mut Entry) -> T,
    ) -> Result<T, ApiError> {
        self.with_lease(lease, |entry| {
            (entry.job.state == JobState::Running).then(|| change(entry))
        })?
        .ok_or_else(|| ApiError::new(ErrorCode::Conflict, "the job is not running"))
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

    // "List durable jobs of one scope, newest first, with bounded pagination."
    fn list_jobs<'a>(
        &'a self,
        scope: &'a JobScope,
        page: Page,
    ) -> PortFuture<'a, ListJobsResponse> {
        let listed = self.lock().and_then(|entries| {
            let mut jobs: Vec<&Entry> = entries
                .values()
                .filter(|entry| entry.scope == *scope)
                .collect();
            jobs.sort_by_key(|entry| std::cmp::Reverse(entry.order));
            let from = match page.cursor.as_deref() {
                Some(cursor) => cursor.parse::<usize>().map_err(|_| {
                    ApiError::new(ErrorCode::InvalidInput, "the cursor is not one we issued")
                })?,
                None => 0,
            };
            let limit = usize::from(page.limit.clamp(1, 500));
            let size = self
                .list_page
                .lock()
                .map_err(|_| poisoned())?
                .map_or(limit, |small| small.min(limit));
            let items: Vec<Job> = jobs
                .iter()
                .skip(from)
                .take(size)
                .map(|entry| entry.job.clone())
                .collect();
            let next = from.saturating_add(items.len());
            Ok(ListJobsResponse {
                next_cursor: (next < jobs.len()).then(|| next.to_string()),
                items,
            })
        });
        Box::pin(async move { listed })
    }

    fn claim_job(&self, job: JobId) -> PortFuture<'_, Option<ClaimedJob>> {
        if self.purged.lock().is_ok_and(|purged| purged.contains(&job)) {
            return Box::pin(async {
                Err(ApiError::new(
                    ErrorCode::Conflict,
                    "the workspace has a recorded purge",
                ))
            });
        }
        let cancel = self
            .cancel_on_claim
            .lock()
            .is_ok_and(|mut cancel| cancel.take_if(|id| *id == job).is_some());
        let claimed = self.lock().map(|mut entries| {
            let entry = entries.get_mut(&job)?;
            if entry.job.state != JobState::Queued {
                return None;
            }
            entry.job.state = JobState::Running;
            entry.job.attempt = entry.job.attempt.saturating_add(1);
            let token = format!("lease-{}", entry.job.attempt);
            entry.lease = Some((token.clone(), false));
            let claim_job = entry.job.clone();
            if cancel {
                entry.job.state = JobState::Cancelled;
            }
            Some(ClaimedJob {
                scope: entry.scope.clone(),
                lease: JobLease {
                    job_id: job,
                    token,
                    attempt: entry.job.attempt,
                    expires_at: Timestamp::try_from("2026-10-09T00:10:00.000Z".to_owned()).ok()?,
                },
                job: claim_job,
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
        if updated.is_ok() {
            let _before = self
                .renewals
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Box::pin(async move { updated })
    }

    fn complete_job(&self, completion: JobCompletion) -> PortFuture<'_, Job> {
        let lease = completion.lease.clone();
        let done = self.running_with_lease(&lease, |entry| {
            entry.job.state = JobState::Succeeded;
            entry.job.progress = 100;
            entry.job.revision.clone_from(&completion.revision);
            entry.job.item_ids.clone_from(&completion.item_ids);
            entry.job.restore.clone_from(&completion.restore);
            entry.job.backup.clone_from(&completion.backup);
            entry.lease = None;
            entry.completions.push(completion);
            entry.job.clone()
        });
        Box::pin(async move { done })
    }

    fn fail_job(&self, lease: JobLease, error: ApiError, retryable: bool) -> PortFuture<'_, Job> {
        let failed = self.running_with_lease(&lease, |entry| {
            entry.job.state = JobState::Failed;
            entry.failures.push((error.message.clone(), retryable));
            entry.job.error = Some(error.clone());
            entry.job.retryable = Some(retryable);
            entry.lease = None;
            entry.job.clone()
        });
        Box::pin(async move { failed })
    }

    // "Refuses with `Conflict`, sealing nothing, when `lease` is not the job's current,
    // unexpired claim of a running job." "A job already cancelled is returned as it is, not
    // sealed." "Sealing again changes nothing."
    fn seal<'a>(&'a self, lease: &'a JobLease) -> PortFuture<'a, Job> {
        let cancel = self
            .cancel_on_seal
            .lock()
            .is_ok_and(|mut cancel| cancel.take_if(|id| *id == lease.job_id).is_some());
        if cancel
            && let Ok(mut entries) = self.lock()
            && let Some(entry) = entries.get_mut(&lease.job_id)
        {
            entry.job.state = JobState::Cancelled;
        }
        let sealed = self
            .with_lease(lease, |entry| match entry.job.state {
                JobState::Cancelled => Ok(entry.job.clone()),
                JobState::Running => {
                    entry.sealed = true;
                    Ok(entry.job.clone())
                }
                JobState::Queued | JobState::Succeeded | JobState::Failed => Err(ApiError::new(
                    ErrorCode::Conflict,
                    "the lease is not the current claim of a running job",
                )),
            })
            .and_then(|sealed| sealed);
        Box::pin(async move { sealed })
    }

    // "Unique on `mutation_id`: a repeated id makes no second transition and returns the job
    // row as it stands. Refuses with `Conflict` (...), writing nothing, a queued or running job
    // that sealed itself."
    fn cancel_job<'a>(
        &'a self,
        scope: &'a JobScope,
        mutation_id: MutationId,
        job: JobId,
    ) -> PortFuture<'a, Job> {
        let cancelled = self.cancel_entry(scope, mutation_id, job);
        Box::pin(async move { cancelled })
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

    fn get_purge<'a>(&'a self, _tenant: &'a TenantId, purge: PurgeId) -> PortFuture<'a, Purge> {
        let found = self.purge(purge);
        Box::pin(async move { found })
    }

    fn find_purge<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _target: &'a PurgeTarget,
    ) -> PortFuture<'a, Option<Purge>> {
        unmodelled("find_purge")
    }

    fn update_purge<'a>(&'a self, _tenant: &'a TenantId, purge: Purge) -> PortFuture<'a, Purge> {
        let stored = self.refused_update(purge.state).and_then(|()| {
            self.insert_purge(purge.clone())?;
            Ok(purge)
        });
        Box::pin(async move { stored })
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

fn poisoned() -> ApiError {
    ApiError::new(ErrorCode::Internal, "fake record store lock poisoned")
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

#[cfg(test)]
mod tests {
    use okf_jawn_contract::access::AccessRoute;
    use okf_jawn_contract::error::{ApiError, ErrorCode};
    use okf_jawn_contract::events::EventKind;
    use okf_jawn_contract::identity::{PurgeId, TenantId};
    use okf_jawn_contract::import::JobState;
    use okf_jawn_core::events::{EventLog, EventScope, NewEvent};
    use okf_jawn_core::jobs::{JobScope, JobSpec, NewJob};
    use okf_jawn_core::storage::Provenance;

    use super::{FakeEvents, FakeRecords, artifact_id, job_id};

    type Checked = Result<(), Box<dyn std::error::Error>>;

    /// Every helper of the ledger, used once, so no including target sees one as dead.
    #[test]
    fn the_ledger_helpers_keep_what_they_are_given() -> Checked {
        let records = FakeRecords::default();
        let id = job_id(7)?;
        records.insert(
            id,
            JobScope::Tenant(TenantId::try_from("tenant-a".to_owned())?),
            NewJob {
                mutation_id: serde_json::from_value(serde_json::json!(
                    "00000000-0000-0000-0000-000000000007"
                ))?,
                initiator: Provenance {
                    subject: "owner".to_owned(),
                    route: AccessRoute::LocalOwner,
                    client_id: None,
                },
                spec: JobSpec::BackupInstallation,
            },
        )?;
        records.expire_running()?;
        assert_eq!(records.entry(id)?.job.state, JobState::Queued);
        assert_eq!(records.artifacts()?.len(), 0);
        let purge: PurgeId =
            serde_json::from_value(serde_json::json!("00000000-0000-0000-0000-000000000008"))?;
        assert_eq!(
            records.purge(purge).map_err(|error| error.code).err(),
            Some(ErrorCode::NotFound)
        );
        assert_ne!(artifact_id(1)?, artifact_id(2)?);
        records.insert_purge(serde_json::from_value(serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000008",
            "target": { "kind": "workspace", "workspace_id": "00000000-0000-0000-0000-000000000009" },
            "state": "requested",
            "job_id": "00000000-0000-0000-0000-000000000007",
            "requested_by": "admin",
            "requested_at": "2026-10-09T00:00:00.000Z",
        }))?)?;
        assert!(records.purge(purge).is_ok());
        records.cancel(id)?;
        assert_eq!(records.entry(id)?.job.state, JobState::Cancelled);
        assert_eq!(records.renewals(), 0);
        records.set_progress(id, 40)?;
        records.cancel_when_claimed(id)?;
        assert_eq!(records.entry(id)?.job.progress, 40);
        records.purge_workspace_of(id)?;
        records.page_jobs(1)?;
        assert_eq!(records.cancels()?, Vec::new());
        assert_eq!(records.cancel_calls()?, Vec::new());
        records.finish(id)?;
        assert_eq!(records.entry(id)?.job.state, JobState::Succeeded);
        records.cancel_before_seal(id)?;
        records.fail_purge_update(okf_jawn_contract::purge::PurgeState::Failed)?;
        Ok(())
    }

    /// `fail_on` fails the one append it names, which appends nothing, and only once.
    #[tokio::test]
    async fn fail_on_fails_the_append_it_names_once_and_that_append_writes_nothing() -> Checked {
        let events = FakeEvents::default();
        let log = EventScope::Tenant(TenantId::try_from("tenant-a".to_owned())?);
        let event = |kind: EventKind| NewEvent {
            kind,
            revision: None,
            item_id: None,
            job_id: None,
            connector_id: None,
            actor: None,
            operation: None,
            transport: None,
            proposal_id: None,
            count: None,
        };
        // The second `imported` append; an append of another kind is not counted.
        events.fail_on(EventKind::Imported, 2)?;
        let _first = events
            .append(&log, None, event(EventKind::Imported))
            .await?;
        let _other = events.append(&log, None, event(EventKind::Changed)).await?;
        let failed = events.append(&log, None, event(EventKind::Imported)).await;
        assert_eq!(
            failed.map_err(|error| error.code).err(),
            Some(ErrorCode::Unavailable)
        );
        let kinds = |events: &FakeEvents| -> Result<Vec<EventKind>, ApiError> {
            Ok(events
                .appended()?
                .into_iter()
                .map(|(_, _, event)| event.kind)
                .collect())
        };
        assert_eq!(
            kinds(&events)?,
            [EventKind::Imported, EventKind::Changed],
            "the failed append wrote nothing"
        );
        // Once: the same append, tried again, is written.
        let _retried = events
            .append(&log, None, event(EventKind::Imported))
            .await?;
        assert_eq!(
            kinds(&events)?,
            [EventKind::Imported, EventKind::Changed, EventKind::Imported]
        );
        Ok(())
    }
}
