//! The `JobHandler` for every `JobSpec` kind (decision I3).
//!
//! A claim is all a handler gets and all it needs. Every write is idempotent on the claim's
//! `MutationId`, so a second run of the same job repeats no effect. The ports are storage's and
//! the rules are core's; this module only sequences them and writes the completion through the
//! claim's lease.
//!
//! Built so far: index rebuild, both backups, both purges. Construction limitation, stated
//! rather than hidden: import and redigest wait for the conversion child and the docling
//! adapter. Export, View export and restore wait for core-cli M0's export, import and restore
//! inputs. A claim of one of those kinds fails with `NotImplemented`, which the runtime records
//! as a non-retryable failure. Nothing is reported as done that was not done.

use std::sync::Arc;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{ArtifactId, Digest, PurgeId, TenantId, WorkspaceId};
use okf_jawn_contract::purge::{Purge, PurgeReport, PurgeState};
use okf_jawn_core::jobs::{
    ArtifactKind, ClaimedJob, JobCompletion, JobHandler, JobScope, JobSpec, NewArtifact,
    RecordStore,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::search::SearchIndex;
use okf_jawn_core::storage::{Backups, ObjectInfo, Purger, StorageScope, VersionStore};

/// The ports the handler calls, all implemented by storage.
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
}

/// What a finished job produced, before it is written as the claim's completion.
#[derive(Debug, Clone, Default)]
struct Done {
    /// Retained output objects.
    outputs: Vec<Digest>,
    /// The recorded artifact.
    artifact: Option<ArtifactId>,
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
        let done = match (&claimed.spec, &claimed.scope) {
            (JobSpec::RebuildIndex, JobScope::Workspace(scope)) => self.rebuild(scope).await?,
            (JobSpec::BackupWorkspace, JobScope::Workspace(scope)) => {
                let object = self
                    .ports
                    .backups
                    .write_workspace_archive(scope, claimed.mutation_id)
                    .await?;
                self.artifact(claimed, ArtifactKind::WorkspaceBackup, object)
                    .await?
            }
            (JobSpec::BackupInstallation, JobScope::Tenant(tenant)) => {
                let object = self
                    .ports
                    .backups
                    .write_installation_archive(tenant, claimed.mutation_id)
                    .await?;
                self.artifact(claimed, ArtifactKind::InstallationBackup, object)
                    .await?
            }
            (
                JobSpec::PurgeWorkspace {
                    purge_id,
                    workspace_id,
                },
                JobScope::Tenant(tenant),
            ) => {
                self.purge(tenant, *purge_id, |purger| {
                    purger.purge_workspace(tenant, claimed.mutation_id, *purge_id, *workspace_id)
                })
                .await?
            }
            (
                JobSpec::PurgeItem {
                    purge_id,
                    workspace_id,
                    item_id,
                },
                JobScope::Tenant(tenant),
            ) => {
                let scope = item_scope(tenant, *workspace_id);
                self.purge(tenant, *purge_id, |purger| async move {
                    purger
                        .purge_item(&scope, claimed.mutation_id, *purge_id, *item_id)
                        .await
                })
                .await?
            }
            (
                JobSpec::Import { .. }
                | JobSpec::Redigest { .. }
                | JobSpec::ExportWorkspace { .. }
                | JobSpec::ExportView { .. }
                | JobSpec::RestoreWorkspace { .. },
                _,
            ) => {
                return Err(ApiError::new(
                    ErrorCode::NotImplemented,
                    format!(
                        "the ingest handler does not run {:?} jobs yet",
                        claimed.spec.kind()
                    ),
                ));
            }
            (
                JobSpec::RebuildIndex
                | JobSpec::BackupWorkspace
                | JobSpec::BackupInstallation
                | JobSpec::PurgeWorkspace { .. }
                | JobSpec::PurgeItem { .. },
                _,
            ) => {
                return Err(ApiError::new(
                    ErrorCode::Internal,
                    "the claimed job's specification does not fit its scope",
                ));
            }
        };
        let _job = self
            .ports
            .records
            .complete_job(JobCompletion {
                lease: claimed.lease.clone(),
                revision: None,
                item_ids: Vec::new(),
                artifact: done.artifact,
                restore: None,
                outputs: done.outputs,
                warnings: Vec::new(),
            })
            .await?;
        Ok(())
    }

    /// Rebuild the derived index at the current head.
    async fn rebuild(&self, scope: &StorageScope) -> Result<Done, ApiError> {
        let head = self.ports.versions.head(scope).await?;
        self.ports.search.rebuild(scope, head).await?;
        Ok(Done::default())
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
        })
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
            purge = records.update_purge(tenant, purge).await?;
            // Limitation: `completed_at` is left as the store has it. Ingest has no clock
            // dependency, and the `update_purge` doc does not say whether the store stamps it
            // (request in the lane report).
            purge.report = Some(remove(self.ports.purger.as_ref()).await?);
            purge.state = PurgeState::Completed;
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
