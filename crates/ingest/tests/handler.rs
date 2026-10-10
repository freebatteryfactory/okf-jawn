//! The handler for every job kind: import and redigest over a fake converter, index rebuild,
//! both backups, restore and both purges. Each runs under its claim, writes its completion
//! through the lease and repeats no effect when it runs again. A kind not built yet fails as
//! `NotImplemented` and is never reported as done.
#![cfg(feature = "runtime")]

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "support/ports.rs"]
mod ports;
#[path = "support/records.rs"]
mod records;

mod job_handler {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use okf_jawn_contract::access::AccessRoute;
    use okf_jawn_contract::common::PageRange;
    use okf_jawn_contract::extraction::{ConversionOutcome, ConversionSettings, TextOrigin};
    use okf_jawn_contract::identity::{
        Digest, ItemId, MutationId, PurgeId, Revision, TenantId, Timestamp, UploadId, WorkspaceId,
        WorkspacePath,
    };
    use okf_jawn_contract::import::JobState;
    use okf_jawn_contract::purge::{Purge, PurgeReport, PurgeState};
    use okf_jawn_contract::workspace::ArtifactKind;
    use okf_jawn_core::conversion::{ConversionRecord, ConverterLimits};
    use okf_jawn_core::jobs::{JobHandler, JobScope, JobSpec, NewJob, RecordStore};
    use okf_jawn_core::storage::{
        CandidateCheck, ObjectInfo, Provenance, SourceCard, StorageScope, TreeEdit,
        derive_commit_mutation_id, derive_item_id,
    };
    use okf_jawn_core::uploads::UploadRecord;
    use okf_jawn_ingest::handler::{
        ARCHIVE_MEDIA_TYPE, HandlerLimits, HandlerPorts, IngestHandler,
    };
    use okf_jawn_ingest::runtime::run_one;
    use serde_json::json;

    use crate::check::{TestResult, some};
    use crate::ports::{
        AcceptAll, FakeBackups, FakeBlobs, FakeConverter, FakePurger, FakeSearch, FakeUploads,
        FakeVersions, RefuseAll,
    };
    use crate::records::{FakeRecords, job_id};

    type Built<T> = Result<T, Box<dyn std::error::Error>>;

    struct World {
        records: Arc<FakeRecords>,
        versions: Arc<FakeVersions>,
        search: Arc<FakeSearch>,
        backups: Arc<FakeBackups>,
        purger: Arc<FakePurger>,
        uploads: Arc<FakeUploads>,
        blobs: Arc<FakeBlobs>,
        converter: Arc<FakeConverter>,
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
        world_with(Some(6), Vec::new())
    }

    /// A world whose converter reports `pages` and hits the cap on the windows starting at
    /// `capped`, with windows of 4 pages.
    fn world_with(pages: Option<u32>, capped: Vec<u32>) -> Built<World> {
        world_full(pages, capped, Arc::new(AcceptAll), Duration::from_secs(60))
    }

    /// A world as `world_with`, with the conformance check composition injects and the
    /// heartbeat of a converting job.
    fn world_full(
        pages: Option<u32>,
        capped: Vec<u32>,
        check: Arc<dyn CandidateCheck>,
        heartbeat: Duration,
    ) -> Built<World> {
        let records = Arc::new(FakeRecords::default());
        let versions = Arc::new(FakeVersions::at(base()?));
        let search = Arc::new(FakeSearch::default());
        let backups = Arc::new(FakeBackups {
            archive: ObjectInfo {
                digest: Digest::try_from("ab".repeat(32))?,
                size: 2048,
            },
            writes: AtomicUsize::new(0),
            restored: Mutex::new(Vec::new()),
        });
        let purger = Arc::new(FakePurger {
            report: report()?,
            calls: AtomicUsize::new(0),
        });
        let uploads = Arc::new(FakeUploads::default());
        let blobs = Arc::new(FakeBlobs::new()?);
        let converter = Arc::new(FakeConverter {
            pages,
            capped,
            limits: ConverterLimits {
                window_pages: 4,
                memory_limit_bytes: 1 << 30,
            },
            windows: Mutex::new(Vec::new()),
            on_window: Mutex::new(None),
            dropped: Arc::new(AtomicUsize::new(0)),
        });
        let handler = IngestHandler::new(HandlerPorts {
            records: Arc::clone(&records) as Arc<dyn RecordStore>,
            versions: Arc::clone(&versions) as Arc<dyn okf_jawn_core::storage::VersionStore>,
            search: Arc::clone(&search) as Arc<dyn okf_jawn_core::search::SearchIndex>,
            backups: Arc::clone(&backups) as Arc<dyn okf_jawn_core::storage::Backups>,
            purger: Arc::clone(&purger) as Arc<dyn okf_jawn_core::storage::Purger>,
            uploads: Arc::clone(&uploads) as Arc<dyn okf_jawn_core::uploads::UploadStore>,
            blobs: Arc::clone(&blobs) as Arc<dyn okf_jawn_core::storage::BlobStore>,
            converter: Arc::clone(&converter) as Arc<dyn okf_jawn_core::conversion::Converter>,
            check,
            limits: HandlerLimits {
                heartbeat,
                window_timeout: Duration::from_secs(600),
            },
        });
        Ok(World {
            records,
            versions,
            search,
            backups,
            purger,
            uploads,
            blobs,
            converter,
            handler,
        })
    }

    fn base() -> Built<Revision> {
        Ok(Revision::try_from("1".repeat(40))?)
    }

    /// A complete upload of `bytes` named `filename`.
    fn upload(world: &World, number: u128, filename: &str, bytes: &[u8]) -> Built<UploadId> {
        let object = world.blobs.insert(bytes)?;
        let id = serde_json::from_value::<UploadId>(json!(format!(
            "00000000-0000-0000-0000-{number:012}"
        )))?;
        world
            .uploads
            .records
            .lock()
            .map_err(|_| "poisoned")?
            .push(UploadRecord {
                id,
                filename: filename.to_owned(),
                relative_path: String::new(),
                expected_size: object.size,
                expected_sha256: None,
                supplied_by: initiator(),
                created_at: Timestamp::try_from("2026-10-09T00:00:00.000Z".to_owned())?,
                received_bytes: object.size,
                object: Some(object),
                consumed_by: None,
            });
        Ok(id)
    }

    fn initiator() -> Provenance {
        Provenance {
            subject: "owner".to_owned(),
            route: AccessRoute::LocalOwner,
            client_id: None,
        }
    }

    fn import_spec(uploads: Vec<UploadId>, apply_naming_rules: bool) -> Built<JobSpec> {
        Ok(JobSpec::Import {
            base_revision: base()?,
            upload_ids: uploads,
            destination: Some(WorkspacePath::try_from("inbox".to_owned())?),
            apply_naming_rules,
            settings: ConversionSettings::default(),
        })
    }

    /// The source card each commit wrote, in order.
    fn cards(world: &World) -> Built<Vec<SourceCard>> {
        Ok(world
            .versions
            .written()?
            .into_iter()
            .flat_map(|(changes, _)| changes.edits)
            .filter_map(|edit| match edit {
                TreeEdit::WriteSourceCard(card) => Some(*card),
                _ => None,
            })
            .collect())
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
                initiator: initiator(),
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
        assert_eq!(world.search.rebuilt()?, [base()?]);
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
        let kinds = [
            (
                5,
                JobSpec::ExportView {
                    item_id: serde_json::from_value::<ItemId>(json!(
                        "00000000-0000-0000-0000-0000000000ef"
                    ))?,
                    revision: base()?,
                },
            ),
            (
                21,
                JobSpec::ExportWorkspace {
                    revision: base()?,
                    include_history: true,
                },
            ),
        ];
        for (number, spec) in kinds {
            let kind = spec.kind();
            let id = job(&world, number, workspace_scope()?, spec)?;
            assert!(run(&world, id).await?);
            let entry = world.records.entry(id)?;
            assert_eq!(entry.job.state, JobState::Failed, "{kind:?}");
            assert!(entry.completions.is_empty(), "{kind:?}");
            let (message, retryable) = some(entry.failures.first(), "the failure")?;
            assert!(message.contains("does not run"), "{kind:?}: {message}");
            assert!(!retryable, "{kind:?}");
        }
        assert!(world.versions.written()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn a_restore_passes_the_accepted_editors_and_consumes_its_upload() -> TestResult {
        let world = world()?;
        let upload_id = upload(&world, 70, "backup.zip", b"archive bytes")?;
        let editors = vec!["alice".to_owned(), "bob".to_owned()];
        let id = job(
            &world,
            7,
            workspace_scope()?,
            JobSpec::RestoreWorkspace {
                upload_id,
                archive: Digest::try_from("cd".repeat(32))?,
                editors: editors.clone(),
            },
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded);
        let completion = some(entry.completions.first(), "the completion")?;
        assert_eq!(
            completion
                .restore
                .as_ref()
                .map(|report| report.drafts_unassigned),
            Some(1)
        );
        assert_eq!(
            world
                .backups
                .restored
                .lock()
                .map_err(|_| "poisoned")?
                .clone(),
            [editors]
        );
        assert_eq!(world.uploads.consumed()?, [(upload_id, id)]);
        Ok(())
    }

    #[tokio::test]
    async fn an_import_commits_pending_cards_then_each_converted_card() -> TestResult {
        // Six pages in windows of four; the second window hits the cap.
        let world = world_with(Some(6), vec![5])?;
        let report = upload(&world, 80, "report.pdf", b"%PDF report")?;
        let notes = upload(&world, 81, "report.pdf", b"%PDF other report")?;
        let id = job(
            &world,
            8,
            workspace_scope()?,
            import_spec(vec![report, notes], false)?,
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        assert_eq!(world.uploads.consumed()?, [(report, id), (notes, id)]);

        // One commit of both pending cards, then one per converted card, each expecting the
        // revision before it, and each under its own identity derived from the job's.
        let commits = world.versions.written()?;
        assert_eq!(commits.len(), 3);
        let mutation = entry.created.mutation_id;
        assert_eq!(
            commits
                .iter()
                .map(|(changes, _)| changes.mutation_id)
                .collect::<Vec<_>>(),
            [0, 1, 2].map(|ordinal| derive_commit_mutation_id(mutation, ordinal))
        );
        let first = some(commits.first(), "the pending commit")?;
        assert_eq!(first.0.expected_head, base()?);
        assert_eq!(some(commits.get(1), "the second")?.0.expected_head, first.1);

        let cards = cards(&world)?;
        let paths: Vec<&str> = cards.iter().map(|card| card.path.as_str()).collect();
        // Named by source_card_name, the second numbered.
        assert_eq!(
            paths,
            [
                "inbox/report-pdf.md",
                "inbox/report-pdf-2.md",
                "inbox/report-pdf.md",
                "inbox/report-pdf-2.md"
            ]
        );
        let pending = some(cards.first(), "the pending card")?;
        assert_eq!(
            pending.appearance.extraction.outcome,
            ConversionOutcome::Pending
        );
        assert_eq!(pending.item_id, derive_item_id(mutation, 0));
        let converted = some(cards.get(2), "the converted card")?;
        assert_eq!(converted.item_id, pending.item_id);
        // The capped window is pages not converted, never a failed document.
        let ConversionOutcome::Partial { coverage, .. } = &converted.appearance.extraction.outcome
        else {
            return Err(format!(
                "expected a partial outcome, got {:?}",
                converted.appearance.extraction.outcome
            )
            .into());
        };
        let coverage = some(coverage.as_ref(), "the coverage")?;
        assert_eq!(coverage.not_converted, vec![PageRange { start: 5, end: 6 }]);
        assert_eq!(converted.appearance.extraction.page_count, Some(6));
        assert_eq!(
            converted.appearance.extraction.text_origin,
            TextOrigin::Converter
        );
        assert!(
            converted.body.contains("# Pages from 1"),
            "{}",
            converted.body
        );

        // The record is retained under the digest the card shows.
        let digest = some(
            converted.appearance.extraction.digest.as_ref(),
            "the record digest",
        )?;
        let record: ConversionRecord = serde_json::from_slice(&world.blobs.bytes(digest)?)?;
        assert_eq!(record.page_count, Some(6));
        assert_eq!(record.structured.len(), 1);
        let completion = some(entry.completions.first(), "the completion")?;
        assert_eq!(
            completion.revision.as_ref(),
            commits.last().map(|(_, revision)| revision)
        );
        assert_eq!(
            completion.item_ids,
            [derive_item_id(mutation, 0), derive_item_id(mutation, 1)]
        );
        assert!(completion.outputs.contains(digest));
        Ok(())
    }

    #[tokio::test]
    async fn an_import_run_again_writes_no_second_commit() -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let report = upload(&world, 90, "report.pdf", b"%PDF report")?;
        let id = job(
            &world,
            9,
            workspace_scope()?,
            import_spec(vec![report], false)?,
        )?;
        assert!(run(&world, id).await?);
        let before = world.versions.written()?.len();
        let entry = world.records.entry(id)?;
        // The completion was lost: the same claim runs again.
        world
            .records
            .insert(id, entry.scope.clone(), entry.created.clone())?;
        assert!(run(&world, id).await?);
        assert_eq!(world.versions.written()?.len(), before);
        let again = world.records.entry(id)?;
        assert_eq!(again.job.state, JobState::Succeeded);
        assert_eq!(
            some(again.completions.first(), "the completion")?.item_ids,
            some(entry.completions.first(), "the first completion")?.item_ids
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_import_that_died_after_its_second_of_three_commits_completes_without_a_duplicate()
    -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let first = upload(&world, 91, "first.pdf", b"%PDF first")?;
        let second = upload(&world, 92, "second.pdf", b"%PDF second")?;
        let id = job(
            &world,
            13,
            workspace_scope()?,
            import_spec(vec![first, second], false)?,
        )?;
        // The pending cards and the first card are written; the process dies with the second.
        world.versions.die_after(2)?;
        assert!(run(&world, id).await?);
        let died = world.records.entry(id)?;
        assert_eq!(died.job.state, JobState::Failed);
        assert_eq!(world.versions.written()?.len(), 2);
        // The next attempt replays the two written commits and writes only the third.
        world
            .records
            .insert(id, died.scope.clone(), died.created.clone())?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let mutation = entry.created.mutation_id;
        let commits = world.versions.written()?;
        assert_eq!(
            commits
                .iter()
                .map(|(changes, _)| changes.mutation_id)
                .collect::<Vec<_>>(),
            [0, 1, 2].map(|ordinal| derive_commit_mutation_id(mutation, ordinal))
        );
        let written: Vec<(String, ConversionOutcome)> = cards(&world)?
            .into_iter()
            .map(|card| {
                (
                    card.path.as_str().to_owned(),
                    card.appearance.extraction.outcome,
                )
            })
            .collect();
        assert_eq!(
            written,
            [
                ("inbox/first-pdf.md".to_owned(), ConversionOutcome::Pending),
                ("inbox/second-pdf.md".to_owned(), ConversionOutcome::Pending),
                (
                    "inbox/first-pdf.md".to_owned(),
                    ConversionOutcome::Completed
                ),
                (
                    "inbox/second-pdf.md".to_owned(),
                    ConversionOutcome::Completed
                ),
            ]
        );
        let completion = some(entry.completions.first(), "the completion")?;
        assert_eq!(
            completion.revision.as_ref(),
            commits.last().map(|(_, revision)| revision)
        );
        assert_eq!(
            completion.item_ids,
            [derive_item_id(mutation, 0), derive_item_id(mutation, 1)]
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_import_completes_over_edits_made_elsewhere_after_it_was_accepted() -> TestResult {
        let world = world_with(Some(6), Vec::new())?;
        let report = upload(&world, 93, "report.pdf", b"%PDF report")?;
        let id = job(
            &world,
            14,
            workspace_scope()?,
            import_spec(vec![report], false)?,
        )?;
        // Someone edits another item between acceptance and the first commit, and again just
        // as the card commit is written.
        world.versions.edit_elsewhere(None)?;
        world.versions.edit_elsewhere_before_call(2)?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let mutation = entry.created.mutation_id;
        let commits = world.versions.written()?;
        let ours: Vec<_> = commits
            .iter()
            .filter(|(changes, _)| changes.author.subject == "owner")
            .collect();
        assert_eq!(
            ours.iter()
                .map(|(changes, _)| changes.mutation_id)
                .collect::<Vec<_>>(),
            [0, 1].map(|ordinal| derive_commit_mutation_id(mutation, ordinal))
        );
        // Each was written on the head as it stood, not on the accepted base revision.
        let pending = some(ours.first(), "the pending commit")?;
        assert_eq!(
            Some(&pending.0.expected_head),
            commits.first().map(|(_, revision)| revision)
        );
        assert_eq!(commits.len(), 4);
        assert_eq!(
            some(entry.completions.first(), "the completion")?
                .revision
                .as_ref(),
            commits.last().map(|(_, revision)| revision)
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_import_whose_card_path_was_taken_after_acceptance_fails_once_and_for_good()
    -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let report = upload(&world, 94, "report.pdf", b"%PDF report")?;
        let id = job(
            &world,
            15,
            workspace_scope()?,
            import_spec(vec![report], false)?,
        )?;
        let theirs =
            serde_json::from_value::<ItemId>(json!("00000000-0000-0000-0000-0000000000fe"))?;
        world.versions.edit_elsewhere(Some((
            &WorkspacePath::try_from("inbox/REPORT-pdf.md".to_owned())?,
            theirs,
        )))?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        assert!(entry.completions.is_empty());
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(message.contains("collides"), "{message}");
        assert!(!retryable);
        assert_eq!(entry.failures.len(), 1);
        // Nothing of the import was written, and the job is not delivered again.
        assert_eq!(world.versions.written()?.len(), 1);
        assert!(
            !world
                .records
                .pending_jobs()
                .await?
                .iter()
                .any(|(_, pending)| *pending == id)
        );
        assert!(!run(&world, id).await?);
        Ok(())
    }

    #[tokio::test]
    async fn an_import_commits_through_the_import_check_lint_warns_and_a_refusal_refuses()
    -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let report = upload(&world, 105, "report.pdf", b"%PDF report")?;
        let id = job(
            &world,
            19,
            workspace_scope()?,
            import_spec(vec![report], false)?,
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        // The pending card's body opens with no top-level heading: OKF lint of the imported
        // file (portable::ImportCheck) reaches the completion as a warning, and refuses nothing.
        let completion = some(entry.completions.first(), "the completion")?;
        assert!(
            completion.warnings.iter().any(|warning| {
                warning.code.starts_with("okf_lint")
                    && warning.location.as_deref() == Some("inbox/report-pdf.md")
            }),
            "{:?}",
            completion.warnings
        );

        // The injected conformance check still refuses, and nothing is written.
        let refusing = world_full(
            Some(3),
            Vec::new(),
            Arc::new(RefuseAll),
            Duration::from_secs(60),
        )?;
        let report = upload(&refusing, 106, "report.pdf", b"%PDF report")?;
        let id = job(
            &refusing,
            20,
            workspace_scope()?,
            import_spec(vec![report], false)?,
        )?;
        assert!(run(&refusing, id).await?);
        let entry = refusing.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(message.contains("not a conformant"), "{message}");
        assert!(!retryable);
        assert!(refusing.versions.written()?.is_empty());
        Ok(())
    }

    /// The job's state, what it wrote and the windows converted, after a cancellation.
    fn assert_stopped(world: &World, id: okf_jawn_contract::identity::JobId) -> TestResult {
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Cancelled);
        assert!(entry.completions.is_empty(), "{:?}", entry.completions);
        assert!(entry.failures.is_empty(), "{:?}", entry.failures);
        Ok(())
    }

    fn windows(world: &World) -> Built<Vec<Option<PageRange>>> {
        Ok(world
            .converter
            .windows
            .lock()
            .map_err(|_| "poisoned")?
            .clone())
    }

    #[tokio::test]
    async fn a_job_cancelled_during_a_window_stops_before_the_next_window() -> TestResult {
        let world = world_with(Some(6), Vec::new())?;
        let report = upload(&world, 101, "report.pdf", b"%PDF report")?;
        let id = job(
            &world,
            16,
            workspace_scope()?,
            import_spec(vec![report], false)?,
        )?;
        let records = Arc::clone(&world.records);
        *world.converter.on_window.lock().map_err(|_| "poisoned")? =
            Some(Box::new(move |window: Option<&PageRange>| {
                if window.is_some_and(|window| window.start == 1) {
                    let _cancelled = records.cancel(id);
                }
                false
            }));
        assert!(run(&world, id).await?);
        assert_stopped(&world, id)?;
        // The second window is never converted, and no card is replaced.
        assert_eq!(windows(&world)?, [Some(PageRange { start: 1, end: 4 })]);
        assert_eq!(world.versions.written()?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn a_job_cancelled_while_a_window_converts_drops_the_conversion() -> TestResult {
        let world = world_full(
            Some(6),
            Vec::new(),
            Arc::new(AcceptAll),
            Duration::from_millis(10),
        )?;
        let report = upload(&world, 102, "report.pdf", b"%PDF report")?;
        let id = job(
            &world,
            17,
            workspace_scope()?,
            import_spec(vec![report], false)?,
        )?;
        let records = Arc::clone(&world.records);
        // The first window never ends by itself; the heartbeat must find the job cancelled.
        *world.converter.on_window.lock().map_err(|_| "poisoned")? =
            Some(Box::new(move |_window: Option<&PageRange>| {
                let _cancelled = records.cancel(id);
                true
            }));
        // An `Elapsed` error here means the heartbeat never stopped the cancelled job.
        assert!(tokio::time::timeout(Duration::from_secs(20), run(&world, id)).await??);
        assert_stopped(&world, id)?;
        assert_eq!(world.converter.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(windows(&world)?, [Some(PageRange { start: 1, end: 4 })]);
        assert_eq!(world.versions.written()?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn a_job_cancelled_between_card_commits_converts_no_further_source() -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let first = upload(&world, 103, "first.pdf", b"%PDF first")?;
        let second = upload(&world, 104, "second.pdf", b"%PDF second")?;
        let id = job(
            &world,
            18,
            workspace_scope()?,
            import_spec(vec![first, second], false)?,
        )?;
        let records = Arc::clone(&world.records);
        // Cancelled as the first card's commit is written (the second commit call).
        *world.versions.on_commit.lock().map_err(|_| "poisoned")? =
            Some(Box::new(move |call: usize| {
                if call == 2 {
                    let _cancelled = records.cancel(id);
                }
            }));
        assert!(run(&world, id).await?);
        assert_stopped(&world, id)?;
        assert_eq!(windows(&world)?, [Some(PageRange { start: 1, end: 3 })]);
        assert_eq!(world.versions.written()?.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn an_original_converted_whole_has_one_window() -> TestResult {
        let world = world_with(None, Vec::new())?;
        let notes = upload(&world, 95, "notes.md", b"# Notes\n")?;
        let id = job(
            &world,
            10,
            workspace_scope()?,
            import_spec(vec![notes], false)?,
        )?;
        assert!(run(&world, id).await?);
        assert_eq!(
            world
                .converter
                .windows
                .lock()
                .map_err(|_| "poisoned")?
                .clone(),
            [None]
        );
        let cards = cards(&world)?;
        let card = some(cards.last(), "the converted card")?;
        assert_eq!(
            card.appearance.extraction.outcome,
            ConversionOutcome::Completed
        );
        assert_eq!(card.appearance.media_type, "text/markdown");
        Ok(())
    }

    #[tokio::test]
    async fn naming_rules_and_a_partial_redigest_are_refused_until_core_has_their_inputs()
    -> TestResult {
        let world = world()?;
        let report = upload(&world, 96, "report.pdf", b"%PDF")?;
        let id = job(
            &world,
            11,
            workspace_scope()?,
            import_spec(vec![report], true)?,
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        assert!(world.versions.written()?.is_empty());
        let redigest = job(
            &world,
            12,
            workspace_scope()?,
            JobSpec::Redigest {
                item_id: serde_json::from_value::<ItemId>(json!(
                    "00000000-0000-0000-0000-0000000000ab"
                ))?,
                base_revision: base()?,
                settings: ConversionSettings::default(),
                pages: Some(vec![PageRange { start: 5, end: 6 }]),
            },
        )?;
        assert!(run(&world, redigest).await?);
        let entry = world.records.entry(redigest)?;
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(message.contains("R-I5"), "{message}");
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
