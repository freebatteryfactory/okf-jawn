//! The `JobHandler` for every `JobSpec` kind (decision I3).
//!
//! A claim is all a handler gets and all it needs. Every write is idempotent on the claim's
//! `MutationId`, so a second run of the same job repeats no effect. The ports are storage's and
//! the rules are core's; this module only sequences them and writes the completion through the
//! claim's lease. A job found cancelled while it runs stops and writes no completion. A job
//! refused for good (an import colliding with what the workspace holds now, R-I8) writes its
//! failure through the lease as not retryable; any other error is the runtime's to record.
//!
//! A long storage call runs under the same heartbeat as a conversion, so its lease stays live:
//! the index rebuild, both archive writes, the restore and both purges each renew the lease
//! every `HandlerLimits::heartbeat` while they run (`renewing`), and so do an import's two long
//! reads of a replayed commit (the tree its check runs over again, and the outputs its card
//! references). Unlike a conversion, such a call
//! is never dropped when the job is found cancelled: storage's write is one transaction that
//! must not be abandoned mid-call, so the cancellation is observed after it returns, and the job
//! then stops without a completion. A purge's own record still states what the purge removed.
//! A restore is the exception: it seals itself before it calls `restore_import`
//! (`RecordStore::seal`), from which point the store refuses to cancel it, and it goes on to
//! its completion (`restore`).
//!
//! A workspace purge job is the `Purger`'s caller: before it calls the purger it drains the
//! workspace's jobs, cancelling each queued or running one and waiting, at most
//! `HandlerLimits::drain`, until none runs (`drain`). A purge job that fails records its
//! purge `Failed`, with the error, before the runtime records the job's failure; asking for
//! the purge again resumes it (contract ruling 24).
//!
//! What a job commits is announced as core's `announce` does for a request (core
//! `application::shared::commit`): every revision an ingest job lands (each import commit, the
//! redigest commit, the revision a restore produces), written now or replayed, is first indexed
//! (`SearchIndex::index_revision`) and then announced as a `changed` event under that commit's
//! own mutation identity. An import that completes having produced items appends `imported`
//! once, under the job's mutation. Both are idempotent, so an attempt that runs again announces
//! again with no second effect; an index or event failure is a worker error, and the job is
//! retried. Each durable job write ingest makes (a progress value that changed, completion,
//! failure) is followed by a `job_updated` event with no mutation identity; a renewal that only
//! keeps the lease, changing neither progress nor state, appends nothing (`Attempt`). A tenant
//! job's `job_updated` goes to the tenant log: it has no workspace.
//!
//! Built: import and redigest (`import`), index rebuild, both backups, restore, both purges.
//! Construction limitations, stated rather than hidden: a portable export and a View export
//! fail as `NotImplemented`, which the runtime records as a non-retryable failure (requests
//! R-I6 and R-I7 in the lane report). Nothing is reported as done that was not done.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::{
    ArtifactId, Digest, ItemId, JobId, MutationId, PurgeId, Revision, TenantId, UploadId,
    WorkspaceId,
};
use okf_jawn_contract::import::{Job, JobState};
use okf_jawn_contract::purge::{Purge, PurgeReport, PurgeState};
use okf_jawn_contract::workspace::{BackupReport, RestoreReport};
use okf_jawn_core::conversion::Converter;
use okf_jawn_core::events::{EventLog, EventScope, NewEvent};
use okf_jawn_core::jobs::{
    ArtifactKind, ClaimedJob, JobCompletion, JobHandler, JobScope, JobSpec, NewArtifact,
    RecordStore,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::search::SearchIndex;
use okf_jawn_core::storage::{
    Backups, BlobStore, CandidateCheck, ObjectInfo, Page, Purger, StorageScope, VersionStore,
    derive_drain_mutation_id,
};
use okf_jawn_core::uploads::UploadStore;

pub use crate::import::CARD_NOT_WRITTEN;
use crate::import::{ImportRequest, import, redigest};

/// The ports the handler calls: storage's, the converter and the injected check.
#[derive(Clone)]
pub struct HandlerPorts {
    /// Durable job, artifact and purge records.
    pub records: Arc<dyn RecordStore>,
    /// Versioned workspace content.
    pub versions: Arc<dyn VersionStore>,
    /// The derived search and link index.
    pub search: Arc<dyn SearchIndex>,
    /// The live notifications a job's commits and job writes are announced on.
    pub events: Arc<dyn EventLog>,
    /// Workspace and installation archives.
    pub backups: Arc<dyn Backups>,
    /// Purge execution.
    pub purger: Arc<dyn Purger>,
    /// Upload slots an import or a restore consumes.
    pub uploads: Arc<dyn UploadStore>,
    /// Retained originals, exports, assets and records.
    pub blobs: Arc<dyn BlobStore>,
    /// The bounded converter.
    pub converter: Arc<dyn Converter>,
    /// The conformance check composition injects (core-cli's production check).
    pub check: Arc<dyn CandidateCheck>,
    /// Timing.
    pub limits: HandlerLimits,
}

/// How the handler paces long work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandlerLimits {
    /// How often a converting job renews its lease; at most a third of the lease.
    pub heartbeat: Duration,
    /// The time budget of one window (`ConversionInput::timeout`).
    pub window_timeout: Duration,
    /// The longest a workspace purge waits for the workspace's jobs to stop running
    /// (`DRAIN_BOUND` in production).
    pub drain: Duration,
}

/// What a finished job produced, before it is written as the claim's completion.
#[derive(Debug, Clone, Default)]
pub(crate) struct Done {
    /// The committed revision, when the job changed the workspace.
    pub(crate) revision: Option<Revision>,
    /// Items the job produced.
    pub(crate) item_ids: Vec<ItemId>,
    /// Retained output objects.
    pub(crate) outputs: Vec<Digest>,
    /// The recorded artifact.
    pub(crate) artifact: Option<ArtifactId>,
    /// What a restore did.
    pub(crate) restore: Option<RestoreReport>,
    /// What a workspace backup left out of its archive.
    pub(crate) backup: Option<BackupReport>,
    /// Findings of the checks the commits ran.
    pub(crate) warnings: Vec<Warning>,
    /// The job was found cancelled: no completion is written.
    pub(crate) cancelled: bool,
    /// The job was refused for good (an import that collides with what the workspace holds
    /// now): its failure is written as not retryable, and no completion.
    pub(crate) refused: Option<ApiError>,
}

/// Runs claimed jobs of every kind.
#[derive(Clone)]
pub struct IngestHandler {
    ports: HandlerPorts,
}

/// One attempt at a claimed job: the claim it runs under, and the progress the job's record
/// holds as this attempt last wrote it, so a write that changes the progress is announced and
/// a renewal that does not change it is not.
pub(crate) struct Attempt<'a> {
    /// The claim.
    pub(crate) claimed: &'a ClaimedJob,
    /// The progress the job's record holds.
    progress: AtomicU8,
}

/// How a long storage call under the heartbeat ended.
pub(crate) enum Renewed<T> {
    /// It returned, and the job is still running.
    Done(T),
    /// It returned, and the job was found cancelled while it ran.
    Cancelled(T),
}

/// The media type both archives are served with (`download_artifact`,
/// `download_tenant_artifact`).
pub const ARCHIVE_MEDIA_TYPE: &str = "application/zip";

/// The longest a workspace purge waits for the workspace's jobs to stop running (the drain)
/// before its attempt fails, to be resumed when the purge is asked for again: ten minutes,
/// long enough for a running conversion's lease to expire. A job whose attempt died stays
/// `running` until its lease lapses (five minutes, storage's job lease) and the runtime's next
/// sweep, a third of a lease later, queues it again, where the purge's record keeps any worker
/// from claiming it; a restore past its point of no return renews its lease and is waited for
/// while it finishes. Storage's purge waits as long for the target's writes in flight.
pub const DRAIN_BOUND: Duration = Duration::from_secs(600);

/// How many of the workspace's jobs one page of the drain's listing asks for.
const DRAIN_PAGE: u16 = 500;

impl IngestHandler {
    /// A handler over storage's ports.
    #[must_use]
    pub const fn new(ports: HandlerPorts) -> Self {
        Self { ports }
    }

    /// Run one claimed job to its completion, and announce the completion or failure it wrote.
    async fn run(&self, claimed: &ClaimedJob) -> Result<(), ApiError> {
        let attempt = Attempt::new(claimed);
        let done = self.dispatch(&attempt).await?;
        if done.cancelled {
            return Ok(());
        }
        if let Some(refusal) = done.refused {
            let _job = self
                .ports
                .records
                .fail_job(claimed.lease.clone(), refusal, false)
                .await?;
            return announce_job(self.ports.events.as_ref(), claimed).await;
        }
        let _job = self
            .ports
            .records
            .complete_job(JobCompletion {
                lease: claimed.lease.clone(),
                revision: done.revision,
                item_ids: done.item_ids,
                artifact: done.artifact,
                restore: done.restore,
                backup: done.backup,
                outputs: done.outputs,
                warnings: done.warnings,
            })
            .await?;
        announce_job(self.ports.events.as_ref(), claimed).await
    }

    /// Do the work the claim's specification names.
    async fn dispatch(&self, attempt: &Attempt<'_>) -> Result<Done, ApiError> {
        let ports = &self.ports;
        let claimed = attempt.claimed;
        match (&claimed.spec, &claimed.scope) {
            (
                JobSpec::Import {
                    base_revision,
                    upload_ids,
                    destination,
                    apply_naming_rules,
                    settings,
                },
                JobScope::Workspace(scope),
            ) => {
                let request = ImportRequest {
                    base_revision,
                    upload_ids,
                    destination: destination.as_ref(),
                    apply_naming_rules: *apply_naming_rules,
                    settings,
                };
                import(ports, attempt, scope, request).await
            }
            (
                JobSpec::Redigest {
                    item_id,
                    base_revision,
                    settings,
                    pages,
                },
                JobScope::Workspace(scope),
            ) => {
                redigest(
                    ports,
                    attempt,
                    scope,
                    *item_id,
                    base_revision,
                    settings,
                    pages.as_ref(),
                )
                .await
            }
            (JobSpec::RebuildIndex, JobScope::Workspace(scope)) => {
                self.rebuild(attempt, scope).await
            }
            (JobSpec::BackupWorkspace, JobScope::Workspace(scope)) => {
                self.workspace_backup(attempt, scope).await
            }
            (
                JobSpec::RestoreWorkspace {
                    upload_id,
                    archive,
                    editors,
                },
                JobScope::Workspace(scope),
            ) => {
                self.restore(attempt, scope, *upload_id, archive, editors)
                    .await
            }
            (JobSpec::BackupInstallation, JobScope::Tenant(tenant)) => {
                self.installation_backup(attempt, tenant).await
            }
            (
                JobSpec::PurgeWorkspace { .. } | JobSpec::PurgeItem { .. },
                JobScope::Tenant(tenant),
            ) => self.purge_job(attempt, tenant).await,
            (JobSpec::ExportWorkspace { .. } | JobSpec::ExportView { .. }, _) => {
                Err(ApiError::new(
                    ErrorCode::NotImplemented,
                    format!(
                        "the ingest handler does not run {:?} jobs yet",
                        claimed.spec.kind()
                    ),
                ))
            }
            (
                JobSpec::Import { .. }
                | JobSpec::Redigest { .. }
                | JobSpec::RebuildIndex
                | JobSpec::BackupWorkspace
                | JobSpec::RestoreWorkspace { .. }
                | JobSpec::BackupInstallation
                | JobSpec::PurgeWorkspace { .. }
                | JobSpec::PurgeItem { .. },
                _,
            ) => Err(ApiError::new(
                ErrorCode::Internal,
                "the claimed job's specification does not fit its scope",
            )),
        }
    }

    /// Back up the job's workspace: its archive, recorded as the artifact, and what the archive
    /// left out, carried to the job (`Job::backup`).
    async fn workspace_backup(
        &self,
        attempt: &Attempt<'_>,
        scope: &StorageScope,
    ) -> Result<Done, ApiError> {
        let written = async {
            let written = self
                .ports
                .backups
                .write_workspace_archive(scope, attempt.claimed.mutation_id)
                .await?;
            let report = BackupReport {
                rows_left_out: written.rows_left_out,
            };
            Ok((written.object, Some(report)))
        };
        self.backup(attempt, ArtifactKind::WorkspaceBackup, written)
            .await
    }

    /// Back up the installation of the job's tenant, recorded as the artifact.
    async fn installation_backup(
        &self,
        attempt: &Attempt<'_>,
        tenant: &TenantId,
    ) -> Result<Done, ApiError> {
        let written = async {
            let object = self
                .ports
                .backups
                .write_installation_archive(tenant, attempt.claimed.mutation_id)
                .await?;
            Ok((object, None))
        };
        self.backup(attempt, ArtifactKind::InstallationBackup, written)
            .await
    }

    /// Write an archive under the heartbeat and record it as the job's artifact, with what a
    /// workspace archive left out; a job found cancelled while it was written records none.
    async fn backup(
        &self,
        attempt: &Attempt<'_>,
        kind: ArtifactKind,
        write: impl Future<Output = Result<(ObjectInfo, Option<BackupReport>), ApiError>>,
    ) -> Result<Done, ApiError> {
        match renewing(&self.ports, attempt, write).await? {
            Renewed::Done((object, report)) => {
                let done = self.artifact(attempt.claimed, kind, object).await?;
                Ok(Done {
                    backup: report,
                    ..done
                })
            }
            Renewed::Cancelled(_written) => Ok(Done::stopped()),
        }
    }

    /// Rebuild the derived index at the current head.
    async fn rebuild(&self, attempt: &Attempt<'_>, scope: &StorageScope) -> Result<Done, ApiError> {
        let head = self.ports.versions.head(scope).await?;
        Ok(
            match renewing(&self.ports, attempt, self.ports.search.rebuild(scope, head)).await? {
                Renewed::Done(()) => Done::default(),
                Renewed::Cancelled(()) => Done::stopped(),
            },
        )
    }

    /// Import a workspace archive into the job's workspace.
    ///
    /// The editors were read from `AccessControl` when the request was accepted; the handler
    /// passes them on and never consults `AccessControl` itself.
    ///
    /// The order is fixed (fix-5 ruling 1 and contract ruling 23: the upload is consumed when
    /// the restore starts): the job checks for a cancellation, then consumes the archive's
    /// upload under this job, then seals itself past its point of no return
    /// (`RecordStore::seal`, request R-I11), then calls `restore_import`. A cancellation seen
    /// before the consume stops the job, consuming and restoring nothing. An upload another job
    /// consumed refuses the job for good, and so does a consumption the store refuses because a
    /// purge names the workspace (`UploadStore::consume`): both are `Conflict`, and a retry
    /// would meet the same refusal. A cancellation the store took between the consume and the
    /// seal stops the job before `restore_import`, its upload consumed by it. From the seal on
    /// the store refuses to cancel the job, so nothing can mark it cancelled while
    /// `restore_import` runs: the job indexes and announces the revision and writes its
    /// completion with the restore report. A sealed attempt that dies is resumed by the next
    /// claim (its lease lapses and the job is queued again, still sealed): the consume is
    /// already this job's and changes nothing, the seal is already set, and `restore_import`
    /// and the commit it writes are idempotent on the job's mutation.
    ///
    /// The completion records the revision the restore wrote (review N9): the commit carrying
    /// the job's mutation, which `restore_import` is idempotent on, and not whatever head stands
    /// after it, which on a resumed attempt can hold later, unrelated commits. That revision is
    /// indexed and announced, written now or replayed.
    async fn restore(
        &self,
        attempt: &Attempt<'_>,
        scope: &StorageScope,
        upload_id: UploadId,
        archive: &Digest,
        editors: &[String],
    ) -> Result<Done, ApiError> {
        let ports = &self.ports;
        let claimed = attempt.claimed;
        if attempt.renew(ports).await?.state == JobState::Cancelled {
            return Ok(Done::stopped());
        }
        match ports
            .uploads
            .consume(scope, upload_id, claimed.lease.job_id)
            .await
        {
            Ok(_consumed) => {}
            Err(error) if error.code == ErrorCode::Conflict => {
                return Ok(Done {
                    refused: Some(error),
                    ..Done::default()
                });
            }
            Err(error) => return Err(error),
        }
        // The point of no return: from the seal on the store refuses to cancel the job. A
        // cancellation that landed first leaves the job unsealed, and it stops here, restoring
        // nothing (its upload stays consumed by this job, as one that fails after the consume).
        if ports.records.seal(&claimed.lease).await?.state == JobState::Cancelled {
            return Ok(Done::stopped());
        }
        // Under the heartbeat and never dropped; sealed, so no cancellation can land while it
        // runs.
        let report = match renewing(
            ports,
            attempt,
            ports.backups.restore_import(
                scope,
                archive.clone(),
                claimed.mutation_id,
                editors.to_vec(),
            ),
        )
        .await?
        {
            Renewed::Done(report) | Renewed::Cancelled(report) => report,
        };
        // `None` walks the whole accepted line, so a commit an earlier attempt wrote is found.
        // Only its revision is read: no check ran for a restore's commit, so the store kept no
        // warnings for it and answers that they are not available, which is not a finding.
        let revision = ports
            .versions
            .find_commit(scope, claimed.mutation_id, None)
            .await?
            .map(|found| found.revision);
        if let Some(revision) = &revision {
            announce_commit(ports, claimed, scope, revision, claimed.mutation_id, None).await?;
        }
        Ok(Done {
            revision,
            restore: Some(report),
            ..Done::default()
        })
    }

    /// Record a retained archive as the job's artifact.
    async fn artifact(
        &self,
        claimed: &ClaimedJob,
        kind: ArtifactKind,
        object: ObjectInfo,
    ) -> Result<Done, ApiError> {
        let digest = object.digest.clone();
        let record = self
            .ports
            .records
            .record_artifact(
                &claimed.scope,
                claimed.mutation_id,
                NewArtifact {
                    kind,
                    object,
                    media_type: ARCHIVE_MEDIA_TYPE.to_owned(),
                    created_by_job: claimed.lease.job_id,
                },
            )
            .await?;
        Ok(Done {
            outputs: vec![digest],
            artifact: Some(record.id),
            ..Done::default()
        })
    }

    /// Run a purge job of the tenant: a workspace purge, which drains the workspace's jobs
    /// first, or an item purge, which storage has not built (its purger answers
    /// `NotImplemented`) and which drains nothing until it is built.
    async fn purge_job(&self, attempt: &Attempt<'_>, tenant: &TenantId) -> Result<Done, ApiError> {
        let claimed = attempt.claimed;
        match &claimed.spec {
            JobSpec::PurgeWorkspace {
                purge_id,
                workspace_id,
            } => {
                let drained = item_scope(tenant, *workspace_id);
                self.purge(attempt, tenant, *purge_id, Some(drained), |purger| {
                    purger.purge_workspace(tenant, claimed.mutation_id, *purge_id, *workspace_id)
                })
                .await
            }
            JobSpec::PurgeItem {
                purge_id,
                workspace_id,
                item_id,
            } => {
                let scope = item_scope(tenant, *workspace_id);
                self.purge(attempt, tenant, *purge_id, None, |purger| async move {
                    purger
                        .purge_item(&scope, claimed.mutation_id, *purge_id, *item_id)
                        .await
                })
                .await
            }
            JobSpec::Import { .. }
            | JobSpec::Redigest { .. }
            | JobSpec::ExportWorkspace { .. }
            | JobSpec::BackupWorkspace
            | JobSpec::RestoreWorkspace { .. }
            | JobSpec::RebuildIndex
            | JobSpec::ExportView { .. }
            | JobSpec::BackupInstallation => Err(ApiError::new(
                ErrorCode::Internal,
                "a purge job was expected",
            )),
        }
    }

    /// Carry out a purge and keep its record current: running, then its report, or failed.
    ///
    /// A workspace purge drains the workspace's jobs first (`drain`, given the workspace). The
    /// removal then runs under the heartbeat and is never dropped; its report is recorded even
    /// when the job was cancelled meanwhile, since the purge record states what was removed, and
    /// the job then stops without a completion. Any error once the record was read is the purge
    /// job's failure, which the runtime records and which no attempt repeats by itself: the
    /// purge is first recorded `Failed` with that error (contract ruling 24; repeating the
    /// request resumes it). An attempt that had lost its lease records the same; the attempt
    /// that runs the job next records `Running` over it. When the failed state cannot be
    /// recorded either, the error returned says so and the purge stays as it was.
    async fn purge<'a, F, Fut>(
        &'a self,
        attempt: &'a Attempt<'a>,
        tenant: &'a TenantId,
        purge_id: PurgeId,
        drained: Option<StorageScope>,
        remove: F,
    ) -> Result<Done, ApiError>
    where
        F: FnOnce(&'a dyn Purger) -> Fut,
        Fut: Future<Output = Result<PurgeReport, ApiError>> + 'a,
    {
        let records = &self.ports.records;
        let mut purge: Purge = records.get_purge(tenant, purge_id).await?;
        if purge.state == PurgeState::Completed {
            return Ok(Done::default());
        }
        let error = match self
            .remove(attempt, tenant, purge.clone(), drained, remove)
            .await
        {
            Ok(done) => return Ok(done),
            Err(error) => error,
        };
        purge.state = PurgeState::Failed;
        purge.error = Some(error.clone());
        purge.report = None;
        purge.completed_at = None;
        match records.update_purge(tenant, purge).await {
            Ok(_failed) => Err(error),
            Err(unrecorded) => Err(ApiError {
                message: format!(
                    "{}; recording the purge as failed failed too: {}",
                    error.message, unrecorded.message
                ),
                ..error
            }),
        }
    }

    /// Record the purge running, drain the workspace when there is one, run the removal under
    /// the heartbeat and record its report as completed.
    async fn remove<'a, F, Fut>(
        &'a self,
        attempt: &'a Attempt<'a>,
        tenant: &'a TenantId,
        mut purge: Purge,
        drained: Option<StorageScope>,
        remove: F,
    ) -> Result<Done, ApiError>
    where
        F: FnOnce(&'a dyn Purger) -> Fut,
        Fut: Future<Output = Result<PurgeReport, ApiError>> + 'a,
    {
        let records = &self.ports.records;
        purge.state = PurgeState::Running;
        purge.error = None;
        purge.completed_at = None;
        let mut purge = records.update_purge(tenant, purge).await?;
        if let Some(scope) = &drained {
            self.drain(attempt, scope).await?;
        }
        let mut done = Done::default();
        let report =
            match renewing(&self.ports, attempt, remove(self.ports.purger.as_ref())).await? {
                Renewed::Done(report) => report,
                Renewed::Cancelled(report) => {
                    done = Done::stopped();
                    report
                }
            };
        purge.report = Some(report);
        purge.state = PurgeState::Completed;
        // The store stamps `completed_at` in the write that first records completion
        // (`RecordStore::update_purge`); the handler has no clock and passes none.
        purge.completed_at = None;
        let _stored = records.update_purge(tenant, purge).await?;
        Ok(done)
    }

    /// The `Purger` caller's drain: cancel every queued or running job of the purged workspace
    /// and wait, at most `HandlerLimits::drain`, until none is running.
    ///
    /// Each pass lists the workspace's jobs again, every page, and cancels each queued or
    /// running one under `derive_drain_mutation_id` of the purge job's mutation and that job, so
    /// a resumed purge cancels the same way and the store makes no second transition. Each
    /// cancellation that took effect is announced (`job_updated`, in the workspace's log, under
    /// that identity, so a resumed purge appends no second one). A job past its point of no
    /// return (a sealed restore) refuses its cancellation: while it runs the drain waits for it;
    /// once its attempt died and its lease lapsed it is queued and no worker can claim it (the
    /// purge's record refuses the claim), so it is not waited for, and the purge removes its
    /// row. Between passes, every `HandlerLimits::heartbeat`, the purge job renews its lease.
    ///
    /// "Running" is the store's state. A job cancelled while its attempt is inside one storage
    /// call is `cancelled` at once, though that attempt stops only when the call returns; every
    /// write it would then make to the workspace storage refuses from the moment the purge was
    /// recorded (`Purger`). The one write storage does not refuse, the import of a restore, runs
    /// only after the restore sealed itself, so it is never cancelled and always waited for.
    ///
    /// # Errors
    /// Returns `Unavailable`, naming the jobs still running, when one still runs once the bound
    /// has passed: the purge job fails, retryable, and the run that repeating the request
    /// starts drains again. Returns any port error.
    async fn drain(&self, attempt: &Attempt<'_>, scope: &StorageScope) -> Result<(), ApiError> {
        let limits = self.ports.limits;
        let started = tokio::time::Instant::now();
        loop {
            let running = self.cancel_unfinished(attempt.claimed, scope).await?;
            if running.is_empty() {
                return Ok(());
            }
            let waited = started.elapsed();
            if waited >= limits.drain {
                let named: Vec<String> = running.iter().map(|job| job.0.to_string()).collect();
                return Err(ApiError::new(
                    ErrorCode::Unavailable,
                    format!(
                        "the purge waited {:?} for the workspace's jobs to stop, and {} still \
                         run past their point of no return; asking for the purge again resumes it",
                        limits.drain,
                        named.join(", ")
                    ),
                ));
            }
            let _renewed = attempt.renew(&self.ports).await?;
            let rest = limits.drain.saturating_sub(waited);
            tokio::time::sleep(limits.heartbeat.min(rest).max(Duration::from_millis(1))).await;
        }
    }

    /// One pass of the drain over every page of the workspace's jobs: cancel each queued or
    /// running one, and answer the jobs still running after it.
    async fn cancel_unfinished(
        &self,
        claimed: &ClaimedJob,
        scope: &StorageScope,
    ) -> Result<Vec<JobId>, ApiError> {
        let records = &self.ports.records;
        let jobs = JobScope::Workspace(scope.clone());
        let mut running = Vec::new();
        let mut cursor = None;
        loop {
            let page = records
                .list_jobs(
                    &jobs,
                    Page {
                        cursor,
                        limit: DRAIN_PAGE,
                    },
                )
                .await?;
            for job in page.items {
                if !matches!(job.state, JobState::Queued | JobState::Running) {
                    continue;
                }
                let cancel = derive_drain_mutation_id(claimed.mutation_id, job.id);
                let stands = match records.cancel_job(&jobs, cancel, job.id).await {
                    Ok(cancelled) => {
                        if cancelled.state == JobState::Cancelled {
                            announce_cancelled(self.ports.events.as_ref(), scope, cancel, job.id)
                                .await?;
                        }
                        cancelled
                    }
                    // Refused: past its point of no return, or finished since it was listed.
                    Err(error) if error.code == ErrorCode::Conflict => {
                        records.get_job(&jobs, job.id).await?
                    }
                    Err(error) => return Err(error),
                };
                if stands.state == JobState::Running {
                    running.push(job.id);
                }
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => return Ok(running),
            }
        }
    }
}

impl<'a> Attempt<'a> {
    /// An attempt at `claimed`, whose record holds the progress the claim returned.
    fn new(claimed: &'a ClaimedJob) -> Self {
        Self {
            claimed,
            progress: AtomicU8::new(claimed.job.progress),
        }
    }

    /// Write `progress` through the lease, which also renews it, and announce it
    /// (`job_updated`) when it changed what the record held; return the job as stored. A
    /// cancelled job's progress is not written, so nothing is announced.
    pub(crate) async fn report(&self, ports: &HandlerPorts, progress: u8) -> Result<Job, ApiError> {
        let job = ports
            .records
            .update_progress(&self.claimed.lease, progress)
            .await?;
        if job.state != JobState::Cancelled
            && self.progress.swap(progress, Ordering::SeqCst) != progress
        {
            announce_job(ports.events.as_ref(), self.claimed).await?;
        }
        Ok(job)
    }

    /// Renew the lease, leaving the progress as the record holds it: nothing is announced.
    pub(crate) async fn renew(&self, ports: &HandlerPorts) -> Result<Job, ApiError> {
        self.report(ports, self.progress.load(Ordering::SeqCst))
            .await
    }
}

impl Done {
    /// A job found cancelled: no completion is written.
    fn stopped() -> Self {
        Self {
            cancelled: true,
            ..Self::default()
        }
    }
}

impl JobHandler for IngestHandler {
    fn handle<'a>(&'a self, claimed: &'a ClaimedJob) -> PortFuture<'a, ()> {
        Box::pin(self.run(claimed))
    }
}

/// Run a long storage call to its end while renewing the claim's lease every
/// `HandlerLimits::heartbeat`, and say whether the job was found cancelled meanwhile.
///
/// The call is never dropped: a storage write is one transaction that must not be abandoned
/// mid-call, so a cancellation is only observed, and acted on once the call has returned. A
/// renewal that fails (the lease was lost) stops the renewals; the call still runs to its end,
/// and then that error is returned.
pub(crate) async fn renewing<T>(
    ports: &HandlerPorts,
    attempt: &Attempt<'_>,
    work: impl Future<Output = Result<T, ApiError>>,
) -> Result<Renewed<T>, ApiError> {
    let mut work = std::pin::pin!(work);
    let period = ports.limits.heartbeat.max(Duration::from_millis(1));
    let now = tokio::time::Instant::now();
    let mut ticks = tokio::time::interval_at(now.checked_add(period).unwrap_or(now), period);
    let mut cancelled = false;
    let mut renewal: Result<(), ApiError> = Ok(());
    loop {
        tokio::select! {
            result = &mut work => {
                let value = result?;
                renewal?;
                return Ok(if cancelled {
                    Renewed::Cancelled(value)
                } else {
                    Renewed::Done(value)
                });
            }
            _instant = ticks.tick(), if renewal.is_ok() => {
                match attempt.renew(ports).await {
                    Ok(job) => cancelled |= job.state == JobState::Cancelled,
                    Err(error) => renewal = Err(error),
                }
            }
        }
    }
}

/// Index `revision`, which the job landed (written now or replayed) under `mutation_id`, and
/// announce it as a `changed` event of the job, naming `item` when the commit wrote one item.
/// Both steps are idempotent, so announcing a replayed commit again has no second effect.
///
/// # Errors
/// Returns the index's or the event log's error: a worker error, after which the job is
/// retried and announces again.
pub(crate) async fn announce_commit(
    ports: &HandlerPorts,
    claimed: &ClaimedJob,
    scope: &StorageScope,
    revision: &Revision,
    mutation_id: MutationId,
    item: Option<ItemId>,
) -> Result<(), ApiError> {
    ports.search.index_revision(scope, revision.clone()).await?;
    let _event = ports
        .events
        .append(
            &EventScope::Workspace(scope.clone()),
            Some(mutation_id),
            job_event(EventKind::Changed, claimed, Some(revision.clone()), item),
        )
        .await?;
    Ok(())
}

/// Announce that the job's durable record changed (`job_updated`), with no mutation identity:
/// every such write is a new notification.
///
/// # Errors
/// Returns the event log's error.
pub(crate) async fn announce_job(
    events: &dyn EventLog,
    claimed: &ClaimedJob,
) -> Result<(), ApiError> {
    let scope = match &claimed.scope {
        JobScope::Workspace(scope) => EventScope::Workspace(scope.clone()),
        JobScope::Tenant(tenant) => EventScope::Tenant(tenant.clone()),
    };
    let _event = events
        .append(
            &scope,
            None,
            job_event(EventKind::JobUpdated, claimed, None, None),
        )
        .await?;
    Ok(())
}

/// Announce that a purge's drain cancelled `job` of the purged workspace (`job_updated`, in the
/// workspace's log) under the cancellation's identity, so a resumed purge that finds the job
/// cancelled already appends no second notification.
///
/// # Errors
/// Returns the event log's error.
async fn announce_cancelled(
    events: &dyn EventLog,
    scope: &StorageScope,
    cancel: MutationId,
    job: JobId,
) -> Result<(), ApiError> {
    let _event = events
        .append(
            &EventScope::Workspace(scope.clone()),
            Some(cancel),
            NewEvent {
                kind: EventKind::JobUpdated,
                revision: None,
                item_id: None,
                job_id: Some(job),
                connector_id: None,
                actor: None,
                operation: None,
                proposal_id: None,
                count: None,
            },
        )
        .await?;
    Ok(())
}

/// A notification of `kind` about the claimed job.
pub(crate) fn job_event(
    kind: EventKind,
    claimed: &ClaimedJob,
    revision: Option<Revision>,
    item_id: Option<ItemId>,
) -> NewEvent {
    NewEvent {
        kind,
        revision,
        item_id,
        job_id: Some(claimed.lease.job_id),
        connector_id: None,
        actor: None,
        operation: None,
        proposal_id: None,
        count: None,
    }
}

/// The storage scope of an item purge's workspace.
fn item_scope(tenant: &TenantId, workspace_id: WorkspaceId) -> StorageScope {
    StorageScope {
        tenant_id: tenant.clone(),
        workspace_id,
    }
}
