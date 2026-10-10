//! The `JobHandler` for every `JobSpec` kind (decision I3).
//!
//! A claim is all a handler gets and all it needs. Every write is idempotent on the claim's
//! `MutationId`, so a second run of the same job repeats no effect. The ports are storage's and
//! the rules are core's; this module only sequences them and writes the completion through the
//! claim's lease. A job found cancelled while it runs stops and writes no completion. A job
//! refused for good (an import colliding with what the workspace holds now, R-I8) writes its
//! failure through the lease as not retryable; any other error is the runtime's to record.
//!
//! Built: import and redigest (`import`), index rebuild, both backups, restore, both purges.
//! Construction limitations, stated rather than hidden: a portable export and a View export
//! fail as `NotImplemented`, which the runtime records as a non-retryable failure (requests
//! R-I6 and R-I7 in the lane report). Nothing is reported as done that was not done.

use std::sync::Arc;
use std::time::Duration;

use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{
    ArtifactId, Digest, ItemId, PurgeId, Revision, TenantId, UploadId, WorkspaceId,
};
use okf_jawn_contract::purge::{Purge, PurgeReport, PurgeState};
use okf_jawn_contract::workspace::RestoreReport;
use okf_jawn_core::conversion::Converter;
use okf_jawn_core::jobs::{
    ArtifactKind, ClaimedJob, JobCompletion, JobHandler, JobScope, JobSpec, NewArtifact,
    RecordStore,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::search::SearchIndex;
use okf_jawn_core::storage::{
    Backups, BlobStore, CandidateCheck, ObjectInfo, Purger, StorageScope, VersionStore,
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

/// The media type both archives are served with (`download_artifact`,
/// `download_tenant_artifact`).
pub const ARCHIVE_MEDIA_TYPE: &str = "application/zip";

impl IngestHandler {
    /// A handler over storage's ports.
    #[must_use]
    pub const fn new(ports: HandlerPorts) -> Self {
        Self { ports }
    }

    /// Run one claimed job to its completion.
    async fn run(&self, claimed: &ClaimedJob) -> Result<(), ApiError> {
        let done = self.dispatch(claimed).await?;
        if done.cancelled {
            return Ok(());
        }
        if let Some(refusal) = done.refused {
            let _job = self
                .ports
                .records
                .fail_job(claimed.lease.clone(), refusal.message, false)
                .await?;
            return Ok(());
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
                outputs: done.outputs,
                warnings: done.warnings,
            })
            .await?;
        Ok(())
    }

    /// Do the work the claim's specification names.
    async fn dispatch(&self, claimed: &ClaimedJob) -> Result<Done, ApiError> {
        let ports = &self.ports;
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
                import(ports, claimed, scope, request).await
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
                    claimed,
                    scope,
                    *item_id,
                    base_revision,
                    settings,
                    pages.as_ref(),
                )
                .await
            }
            (JobSpec::RebuildIndex, JobScope::Workspace(scope)) => self.rebuild(scope).await,
            (JobSpec::BackupWorkspace, JobScope::Workspace(scope)) => {
                let object = ports
                    .backups
                    .write_workspace_archive(scope, claimed.mutation_id)
                    .await?;
                self.artifact(claimed, ArtifactKind::WorkspaceBackup, object)
                    .await
            }
            (
                JobSpec::RestoreWorkspace {
                    upload_id,
                    archive,
                    editors,
                },
                JobScope::Workspace(scope),
            ) => {
                self.restore(claimed, scope, *upload_id, archive, editors)
                    .await
            }
            (JobSpec::BackupInstallation, JobScope::Tenant(tenant)) => {
                let object = ports
                    .backups
                    .write_installation_archive(tenant, claimed.mutation_id)
                    .await?;
                self.artifact(claimed, ArtifactKind::InstallationBackup, object)
                    .await
            }
            (
                JobSpec::PurgeWorkspace { .. } | JobSpec::PurgeItem { .. },
                JobScope::Tenant(tenant),
            ) => self.purge_job(claimed, tenant).await,
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

    /// Rebuild the derived index at the current head.
    async fn rebuild(&self, scope: &StorageScope) -> Result<Done, ApiError> {
        let head = self.ports.versions.head(scope).await?;
        self.ports.search.rebuild(scope, head).await?;
        Ok(Done::default())
    }

    /// Import a workspace archive into the job's workspace.
    ///
    /// The editors were read from `AccessControl` when the request was accepted; the handler
    /// passes them on and never consults `AccessControl` itself. The upload is consumed when
    /// the restore completes (Stage 1b design section 8).
    async fn restore(
        &self,
        claimed: &ClaimedJob,
        scope: &StorageScope,
        upload_id: UploadId,
        archive: &Digest,
        editors: &[String],
    ) -> Result<Done, ApiError> {
        let ports = &self.ports;
        let report = ports
            .backups
            .restore_import(
                scope,
                archive.clone(),
                claimed.mutation_id,
                editors.to_vec(),
            )
            .await?;
        let _consumed = ports
            .uploads
            .consume(scope, upload_id, claimed.lease.job_id)
            .await?;
        Ok(Done {
            revision: Some(ports.versions.head(scope).await?),
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

    /// Run a purge job of the tenant: a workspace purge or an item purge.
    async fn purge_job(&self, claimed: &ClaimedJob, tenant: &TenantId) -> Result<Done, ApiError> {
        match &claimed.spec {
            JobSpec::PurgeWorkspace {
                purge_id,
                workspace_id,
            } => {
                self.purge(tenant, *purge_id, |purger| {
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
                self.purge(tenant, *purge_id, |purger| async move {
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

    /// Carry out a purge and keep its record current: running, then its report.
    async fn purge<'a, F, Fut>(
        &'a self,
        tenant: &'a TenantId,
        purge_id: PurgeId,
        remove: F,
    ) -> Result<Done, ApiError>
    where
        F: FnOnce(&'a dyn Purger) -> Fut,
        Fut: Future<Output = Result<PurgeReport, ApiError>> + 'a,
    {
        let records = &self.ports.records;
        let mut purge: Purge = records.get_purge(tenant, purge_id).await?;
        if purge.state != PurgeState::Completed {
            purge.state = PurgeState::Running;
            purge.error = None;
            purge.completed_at = None;
            purge = records.update_purge(tenant, purge).await?;
            purge.report = Some(remove(self.ports.purger.as_ref()).await?);
            purge.state = PurgeState::Completed;
            // The store stamps `completed_at` in the write that first records completion
            // (`RecordStore::update_purge`); the handler has no clock and passes none.
            purge.completed_at = None;
            let _stored = records.update_purge(tenant, purge).await?;
        }
        Ok(Done::default())
    }
}

impl JobHandler for IngestHandler {
    fn handle<'a>(&'a self, claimed: &'a ClaimedJob) -> PortFuture<'a, ()> {
        Box::pin(self.run(claimed))
    }
}

/// The storage scope of an item purge's workspace.
fn item_scope(tenant: &TenantId, workspace_id: WorkspaceId) -> StorageScope {
    StorageScope {
        tenant_id: tenant.clone(),
        workspace_id,
    }
}
