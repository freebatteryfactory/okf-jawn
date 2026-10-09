//! The handler for the jobs that are not conversions: index rebuild, both backups and both
//! purges. Each runs under its claim, writes its completion through the lease and repeats no
//! effect when it runs again. A kind not built yet fails as `NotImplemented` and is never
//! reported as done.
#![cfg(feature = "runtime")]

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "support/ports.rs"]
mod ports;
#[path = "support/records.rs"]
mod records;

mod job_handler {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use okf_jawn_contract::access::AccessRoute;
    use okf_jawn_contract::identity::{
        Digest, MutationId, PurgeId, Revision, TenantId, UploadId, WorkspaceId,
    };
    use okf_jawn_contract::import::JobState;
    use okf_jawn_contract::purge::{Purge, PurgeReport, PurgeState};
    use okf_jawn_contract::workspace::ArtifactKind;
    use okf_jawn_core::jobs::{JobHandler, JobScope, JobSpec, NewJob, RecordStore};
    use okf_jawn_core::storage::{ObjectInfo, Provenance, StorageScope};
    use okf_jawn_ingest::handler::{ARCHIVE_MEDIA_TYPE, HandlerPorts, IngestHandler};
    use okf_jawn_ingest::runtime::run_one;
    use serde_json::json;

    use crate::check::{TestResult, some};
    use crate::ports::{FakeBackups, FakePurger, FakeSearch, FakeVersions};
    use crate::records::{FakeRecords, job_id};

    type Built<T> = Result<T, Box<dyn std::error::Error>>;

    struct World {
        records: Arc<FakeRecords>,
        search: Arc<FakeSearch>,
        backups: Arc<FakeBackups>,
        purger: Arc<FakePurger>,
        handler: IngestHandler,
    }

    const PURGE: &str = "00000000-0000-0000-0000-0000000000cc";
    const WORKSPACE: &str = "00000000-0000-0000-0000-0000000000aa";

    fn tenant() -> Built<TenantId> {
        Ok(TenantId::try_from("tenant-a".to_owned())?)
    }

    fn workspace_scope() -> Built<JobScope> {
        Ok(JobScope::Workspace(StorageScope {
            tenant_id: tenant()?,
            workspace_id: serde_json::from_value::<WorkspaceId>(json!(WORKSPACE))?,
        }))
    }

    fn report() -> Built<PurgeReport> {
        Ok(serde_json::from_value(json!({
            "objects_removed": "3", "objects_kept_shared": "1", "backups_removed": 0,
            "backups_rewritten": 1, "exports_removed": 0, "proposals_removed": 0,
            "drafts_removed": 0, "revisions_invalidated": 2, "citations_remapped": 0,
            "citations_invalidated": 0, "views_invalidated": 0,
        }))?)
    }

    fn world() -> Built<World> {
        let records = Arc::new(FakeRecords::default());
        let search = Arc::new(FakeSearch::default());
        let backups = Arc::new(FakeBackups {
            archive: ObjectInfo {
                digest: Digest::try_from("ab".repeat(32))?,
                size: 2048,
            },
            writes: AtomicUsize::new(0),
        });
        let purger = Arc::new(FakePurger {
            report: report()?,
            calls: AtomicUsize::new(0),
        });
        let handler = IngestHandler::new(HandlerPorts {
            records: Arc::clone(&records) as Arc<dyn RecordStore>,
            versions: Arc::new(FakeVersions {
                head: Revision::try_from("1".repeat(40))?,
            }),
            search: Arc::clone(&search) as Arc<dyn okf_jawn_core::search::SearchIndex>,
            backups: Arc::clone(&backups) as Arc<dyn okf_jawn_core::storage::Backups>,
            purger: Arc::clone(&purger) as Arc<dyn okf_jawn_core::storage::Purger>,
        });
        Ok(World {
            records,
            search,
            backups,
            purger,
            handler,
        })
    }

    fn job(
        world: &World,
        number: u128,
        scope: JobScope,
        spec: JobSpec,
    ) -> Built<okf_jawn_contract::identity::JobId> {
        let id = job_id(number)?;
        world.records.insert(
            id,
            scope,
            NewJob {
                mutation_id: serde_json::from_value::<MutationId>(json!(format!(
                    "00000000-0000-0000-0000-{number:012}"
                )))?,
                initiator: Provenance {
                    subject: "owner".to_owned(),
                    route: AccessRoute::LocalOwner,
                    client_id: None,
                },
                spec,
            },
        )?;
        Ok(id)
    }

    async fn run(world: &World, id: okf_jawn_contract::identity::JobId) -> Built<bool> {
        Ok(run_one(
            world.records.as_ref(),
            &world.handler as &dyn JobHandler,
            id,
        )
        .await?)
    }

    #[tokio::test]
    async fn rebuild_index_rebuilds_at_the_current_head() -> TestResult {
        let world = world()?;
        let id = job(&world, 1, workspace_scope()?, JobSpec::RebuildIndex)?;
        assert!(run(&world, id).await?);
        assert_eq!(
            world.search.rebuilt()?,
            [Revision::try_from("1".repeat(40))?]
        );
        assert_eq!(world.records.entry(id)?.job.state, JobState::Succeeded);
        Ok(())
    }

    #[tokio::test]
    async fn a_backup_records_its_archive_once_even_when_the_job_runs_again() -> TestResult {
        let world = world()?;
        let id = job(&world, 2, workspace_scope()?, JobSpec::BackupWorkspace)?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        let completion = some(entry.completions.first(), "the completion")?;
        let artifacts = world.records.artifacts()?;
        let artifact = some(artifacts.first(), "the artifact")?;
        assert_eq!(
            (
                artifact.kind,
                artifact.media_type.as_str(),
                artifact.created_by_job
            ),
            (ArtifactKind::WorkspaceBackup, ARCHIVE_MEDIA_TYPE, id)
        );
        assert_eq!(completion.artifact, Some(artifact.id));
        assert_eq!(
            completion.outputs,
            std::slice::from_ref(&artifact.object.digest)
        );

        // The attempt is repeated (its completion was lost): the same artifact, no second one.
        world
            .records
            .insert(id, entry.scope.clone(), entry.created.clone())?;
        assert!(run(&world, id).await?);
        assert_eq!(world.records.artifacts()?.len(), 1);
        assert_eq!(
            some(
                world.records.entry(id)?.completions.first(),
                "the completion"
            )?
            .artifact,
            Some(artifact.id)
        );
        assert_eq!(world.backups.writes.load(Ordering::SeqCst), 2);

        // An installation backup is the tenant's.
        let tenant_job = job(
            &world,
            3,
            JobScope::Tenant(tenant()?),
            JobSpec::BackupInstallation,
        )?;
        assert!(run(&world, tenant_job).await?);
        assert!(
            world
                .records
                .artifacts()?
                .iter()
                .any(|artifact| artifact.kind == ArtifactKind::InstallationBackup)
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_purge_keeps_its_record_and_a_completed_purge_is_not_run_again() -> TestResult {
        let world = world()?;
        let purge_id = serde_json::from_value::<PurgeId>(json!(PURGE))?;
        let workspace_id = serde_json::from_value::<WorkspaceId>(json!(WORKSPACE))?;
        let spec = JobSpec::PurgeItem {
            purge_id,
            workspace_id,
            item_id: serde_json::from_value(json!("00000000-0000-0000-0000-0000000000dd"))?,
        };
        let id = job(&world, 4, JobScope::Tenant(tenant()?), spec)?;
        world.records.insert_purge(serde_json::from_value::<Purge>(json!({
            "id": PURGE,
            "target": { "kind": "item", "workspace_id": WORKSPACE, "item_id": "00000000-0000-0000-0000-0000000000dd" },
            "state": "requested",
            "job_id": serde_json::to_value(id)?,
            "requested_by": "admin",
            "requested_at": "2026-10-09T00:00:00.000Z",
        }))?)?;
        assert!(run(&world, id).await?);
        let purge = world.records.purge(purge_id)?;
        assert_eq!(purge.state, PurgeState::Completed);
        assert_eq!(purge.report, Some(report()?));
        assert_eq!(world.records.entry(id)?.job.state, JobState::Succeeded);

        // Run again: the purge is already complete, so nothing is removed a second time.
        let entry = world.records.entry(id)?;
        world.records.insert(id, entry.scope, entry.created)?;
        assert!(run(&world, id).await?);
        assert_eq!(world.purger.calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn a_kind_not_built_yet_fails_and_is_never_reported_done() -> TestResult {
        let world = world()?;
        let id = job(
            &world,
            5,
            workspace_scope()?,
            JobSpec::RestoreWorkspace {
                upload_id: serde_json::from_value::<UploadId>(json!(
                    "00000000-0000-0000-0000-0000000000ee"
                ))?,
                archive: Digest::try_from("cd".repeat(32))?,
                editors: vec!["owner".to_owned()],
            },
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        assert!(entry.completions.is_empty());
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(message.contains("does not run"), "{message}");
        assert!(!retryable);
        Ok(())
    }

    #[tokio::test]
    async fn a_specification_that_does_not_fit_its_scope_is_a_fault() -> TestResult {
        let world = world()?;
        let id = job(&world, 6, workspace_scope()?, JobSpec::BackupInstallation)?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        assert_eq!(world.backups.writes.load(Ordering::SeqCst), 0);
        Ok(())
    }
}
