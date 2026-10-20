//! The handler for every job kind: import and redigest over a fake converter, index rebuild,
//! both backups, restore and both purges. Each runs under its claim, writes its completion
//! through the lease and repeats no effect when it runs again. A kind not built yet fails as
//! `NotImplemented` and is never reported as done.
#![cfg(feature = "runtime")]

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "support/ports.rs"]
pub mod ports;
#[path = "support/records.rs"]
pub mod records;

mod job_handler {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use okf_jawn_contract::access::AccessRoute;
    use okf_jawn_contract::common::{PageRange, Warning};
    use okf_jawn_contract::conventions::{
        CollisionPolicy, DateOrder, ExtensionPolicy, LetterCase, NamingRule, NamingRules,
    };
    use okf_jawn_contract::error::{ApiError, ErrorCode};
    use okf_jawn_contract::events::EventKind;
    use okf_jawn_contract::extraction::{
        ConversionOutcome, ConversionSettings, FailureReason, TextOrigin,
    };
    use okf_jawn_contract::identity::{
        Digest, ItemId, MutationId, PurgeId, Revision, TenantId, Timestamp, UploadId, WorkspaceId,
        WorkspacePath,
    };
    use okf_jawn_contract::import::JobState;
    use okf_jawn_contract::purge::{Purge, PurgeReport, PurgeState};
    use okf_jawn_contract::workspace::{ArtifactKind, BackupReport};
    use okf_jawn_core::conventions::{COLLISION_KEPT, COLLISION_SUFFIXED, PATH_NOT_VALID};
    use okf_jawn_core::conversion::{ConversionRecord, ConverterLimits, PixelSize, RetainedAsset};
    use okf_jawn_core::events::EventScope;
    use okf_jawn_core::jobs::{JobHandler, JobScope, JobSpec, NewJob, RecordStore};
    use okf_jawn_core::storage::{
        CandidateCheck, ObjectInfo, Provenance, RulesFile, RulesParseError, SourceCard,
        StorageScope, TreeEdit, VersionStore, derive_commit_mutation_id, derive_drain_mutation_id,
        derive_item_id,
    };
    use okf_jawn_core::uploads::{UploadRecord, UploadStore};
    use okf_jawn_ingest::handler::{
        ARCHIVE_MEDIA_TYPE, CARD_NOT_WRITTEN, DRAIN_BOUND, HandlerLimits, HandlerPorts,
        IngestHandler, NAMING_RULES_ABSENT,
    };

    use okf_jawn_ingest::runtime::run_one;
    use serde_json::json;

    use crate::check::{TestResult, err_of, some};
    use crate::ports::{
        ARCHIVE_ROWS_LEFT_OUT, AcceptAll, FakeBackups, FakeBlobs, FakeConverter, FakePurger,
        FakeSearch, FakeUploads, FakeVersions, Hold, RefuseAll,
    };
    use crate::records::{Appended, FakeEvents, FakeRecords, job_id};

    type Built<T> = Result<T, Box<dyn std::error::Error>>;

    /// A retained picture of a record, with its retained bytes.
    type Kept = (RetainedAsset, Vec<u8>);

    /// A picture as a test compares it: media type, caption, pixel size and bytes.
    type Seen = (String, Option<String>, Option<PixelSize>, Vec<u8>);

    /// A `changed` notification as a test compares it: log, mutation, revision and item.
    type Announced = (
        EventScope,
        Option<MutationId>,
        Option<Revision>,
        Option<ItemId>,
    );

    struct World {
        records: Arc<FakeRecords>,
        events: Arc<FakeEvents>,
        versions: Arc<FakeVersions>,
        search: Arc<FakeSearch>,
        backups: Arc<FakeBackups>,
        purger: Arc<FakePurger>,
        uploads: Arc<FakeUploads>,
        blobs: Arc<FakeBlobs>,
        converter: Arc<FakeConverter>,
        handler: IngestHandler,
    }

    /// A conformance check that answers for the whole candidate whatever the commit wrote, as
    /// the production `conformance::OkfConformance` does with the validator's findings: one
    /// warning for each file under the staged root, located at the file's workspace path, in
    /// path order.
    struct WholeCandidate;

    const PURGE: &str = "00000000-0000-0000-0000-0000000000cc";
    const WORKSPACE: &str = "00000000-0000-0000-0000-0000000000aa";
    const SOURCE_ITEM: &str = "00000000-0000-0000-0000-0000000000ac";
    /// Another item of the workspace, which a redigest of `SOURCE_ITEM` does not write.
    const OTHER_ITEM: &str = "00000000-0000-0000-0000-0000000000ad";
    /// The code of the warnings `WholeCandidate` gives.
    const WHOLE_CANDIDATE: &str = "whole_candidate";
    /// How many sources the many-card import of review N2 takes.
    const MANY_CARDS: u32 = 12;

    impl CandidateCheck for WholeCandidate {
        fn check(
            &self,
            root: &Path,
            _rewritten: &[WorkspacePath],
        ) -> Result<Vec<Warning>, ApiError> {
            let mut files = Vec::new();
            files_under(root, "", &mut files)
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
            files.sort_unstable();
            Ok(files
                .into_iter()
                .map(|path| Warning {
                    code: WHOLE_CANDIDATE.to_owned(),
                    message: "a finding of the whole candidate".to_owned(),
                    location: Some(path),
                })
                .collect())
        }
    }

    /// Every file under `directory`, as a `/`-separated path below it, after `prefix`.
    fn files_under(directory: &Path, prefix: &str, found: &mut Vec<String>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if entry.file_type()?.is_dir() {
                files_under(&entry.path(), &path, found)?;
            } else {
                found.push(path);
            }
        }
        Ok(())
    }

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
        world_drained(pages, capped, check, heartbeat, DRAIN_BOUND)
    }

    /// A world as `world_full`, whose workspace purges wait at most `drain` for the
    /// workspace's jobs to stop.
    fn world_drained(
        pages: Option<u32>,
        capped: Vec<u32>,
        check: Arc<dyn CandidateCheck>,
        heartbeat: Duration,
        drain: Duration,
    ) -> Built<World> {
        let records = Arc::new(FakeRecords::default());
        let events = Arc::new(FakeEvents::default());
        let versions = Arc::new(FakeVersions::at(base()?));
        let search = Arc::new(FakeSearch::default());
        let backups = Arc::new(FakeBackups {
            archive: ObjectInfo {
                digest: Digest::try_from("ab".repeat(32))?,
                size: 2048,
            },
            writes: AtomicUsize::new(0),
            restored: Mutex::new(Vec::new()),
            managed: Mutex::new(Vec::new()),
            versions: Arc::clone(&versions),
            restore_hold: Mutex::new(None),
            write_hold: Mutex::new(None),
        });
        let purger = Arc::new(FakePurger {
            clean_up_calls: Mutex::new(Vec::new()),
            report: report()?,
            calls: AtomicUsize::new(0),
            hold: Mutex::new(None),
            failure: Mutex::new(None),
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
            on_page_count: Mutex::new(None),
            dropped: Arc::new(AtomicUsize::new(0)),
            count_failed: Mutex::new(None),
            runs: AtomicUsize::new(0),
            pictures: std::sync::atomic::AtomicBool::new(false),
        });
        let handler = IngestHandler::new(HandlerPorts {
            records: Arc::clone(&records) as Arc<dyn RecordStore>,
            versions: Arc::clone(&versions) as Arc<dyn okf_jawn_core::storage::VersionStore>,
            search: Arc::clone(&search) as Arc<dyn okf_jawn_core::search::SearchIndex>,
            events: Arc::clone(&events) as Arc<dyn okf_jawn_core::events::EventLog>,
            backups: Arc::clone(&backups) as Arc<dyn okf_jawn_core::storage::Backups>,
            purger: Arc::clone(&purger) as Arc<dyn okf_jawn_core::storage::Purger>,
            uploads: Arc::clone(&uploads) as Arc<dyn okf_jawn_core::uploads::UploadStore>,
            blobs: Arc::clone(&blobs) as Arc<dyn okf_jawn_core::storage::BlobStore>,
            converter: Arc::clone(&converter) as Arc<dyn okf_jawn_core::conversion::Converter>,
            check,
            limits: HandlerLimits {
                heartbeat,
                window_timeout: Duration::from_secs(600),
                drain,
            },
        });
        Ok(World {
            records,
            events,
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

    /// An upload named `filename` whose bytes have not all arrived: not complete.
    fn incomplete_upload(world: &World, number: u128, filename: &str) -> Built<UploadId> {
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
                expected_size: 100,
                expected_sha256: None,
                supplied_by: initiator(),
                created_at: Timestamp::try_from("2026-10-09T00:00:00.000Z".to_owned())?,
                received_bytes: 40,
                object: None,
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
            world.events.as_ref(),
            &world.handler as &dyn JobHandler,
            id,
        )
        .await?)
    }

    /// The notifications of `kind` appended so far, in order.
    fn events_of(world: &World, kind: &EventKind) -> Built<Vec<Appended>> {
        Ok(world
            .events
            .appended()?
            .into_iter()
            .filter(|(_, _, event)| event.kind == *kind)
            .collect())
    }

    /// The storage scope of the test workspace.
    fn storage_scope() -> Built<StorageScope> {
        match workspace_scope()? {
            JobScope::Workspace(scope) => Ok(scope),
            JobScope::Tenant(_) => Err("a workspace scope was expected".into()),
        }
    }

    #[tokio::test]
    async fn an_import_indexes_and_announces_each_commit_and_a_replay_appends_no_duplicate()
    -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let first = upload(&world, 160, "first.pdf", b"%PDF first")?;
        let second = upload(&world, 161, "second.pdf", b"%PDF second")?;
        let id = job(
            &world,
            160,
            workspace_scope()?,
            import_spec(vec![first, second], false)?,
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let mutation = entry.created.mutation_id;
        let revisions: Vec<Revision> = world
            .versions
            .written()?
            .into_iter()
            .map(|(_, revision)| revision)
            .collect();
        assert_eq!(revisions.len(), 3);
        // Every commit was indexed, in order.
        assert_eq!(world.search.indexed()?, revisions);
        // And announced as `changed` in the workspace's log, under its own derived mutation,
        // naming the job and the card it wrote (the pending commit wrote two: none).
        let scope = EventScope::Workspace(storage_scope()?);
        let changed: Vec<Announced> = events_of(&world, &EventKind::Changed)?
            .into_iter()
            .map(|(log, mutation, event)| {
                assert_eq!(event.job_id, Some(id));
                (log, mutation, event.revision, event.item_id)
            })
            .collect();
        let expected: Vec<_> = revisions
            .iter()
            .zip([
                None,
                Some(derive_item_id(mutation, 0)),
                Some(derive_item_id(mutation, 1)),
            ])
            .zip(0_u32..)
            .map(|((revision, item), ordinal)| {
                (
                    scope.clone(),
                    Some(derive_commit_mutation_id(mutation, ordinal)),
                    Some(revision.clone()),
                    item,
                )
            })
            .collect();
        assert_eq!(changed, expected);
        // `imported` once, under the job's own mutation, with the last revision.
        let imported = events_of(&world, &EventKind::Imported)?;
        assert_eq!(imported.len(), 1);
        let (log, imported_mutation, event) = some(imported.first(), "the imported event")?;
        assert_eq!(
            (log, *imported_mutation, &event.revision, event.job_id),
            (&scope, Some(mutation), &revisions.last().cloned(), Some(id))
        );
        // `job_updated` for the progress that changed and for the completion, the last event,
        // never under a mutation.
        let updated = events_of(&world, &EventKind::JobUpdated)?;
        assert!(updated.len() >= 2, "{updated:?}");
        assert!(
            updated
                .iter()
                .all(|(_, mutation, event)| mutation.is_none() && event.job_id == Some(id))
        );
        let last = some(world.events.appended()?.last().cloned(), "the last event")?;
        assert_eq!(last.2.kind, EventKind::JobUpdated);

        // The completion was lost: the attempt that runs again indexes and announces every
        // replayed commit again, and appends no second `changed` or `imported`.
        world
            .records
            .insert(id, entry.scope.clone(), entry.created.clone())?;
        assert!(run(&world, id).await?);
        assert_eq!(world.records.entry(id)?.job.state, JobState::Succeeded);
        assert_eq!(world.versions.written()?.len(), 3);
        assert_eq!(world.search.indexed()?.len(), 6);
        assert_eq!(events_of(&world, &EventKind::Changed)?.len(), 3);
        assert_eq!(events_of(&world, &EventKind::Imported)?.len(), 1);
        Ok(())
    }

    /// Every commit of the import `id` of two sources is indexed and announced as `changed`
    /// exactly once, under its own derived mutation and naming its card (the pending commit
    /// wrote two: none), and `imported` was appended once with the last revision.
    fn assert_import_announced_once(
        world: &World,
        id: okf_jawn_contract::identity::JobId,
    ) -> TestResult {
        let mutation = world.records.entry(id)?.created.mutation_id;
        let revisions: Vec<Revision> = world
            .versions
            .written()?
            .into_iter()
            .map(|(_, revision)| revision)
            .collect();
        assert_eq!(revisions.len(), 3);
        let indexed = world.search.indexed()?;
        for revision in &revisions {
            assert!(indexed.contains(revision), "{revision:?} was not indexed");
        }
        let scope = EventScope::Workspace(storage_scope()?);
        let changed: Vec<Announced> = events_of(world, &EventKind::Changed)?
            .into_iter()
            .map(|(log, mutation, event)| (log, mutation, event.revision, event.item_id))
            .collect();
        let items = [
            None,
            Some(derive_item_id(mutation, 0)),
            Some(derive_item_id(mutation, 1)),
        ];
        let expected: Vec<Announced> = revisions
            .iter()
            .zip(items)
            .zip(0_u32..)
            .map(|((revision, item), ordinal)| {
                (
                    scope.clone(),
                    Some(derive_commit_mutation_id(mutation, ordinal)),
                    Some(revision.clone()),
                    item,
                )
            })
            .collect();
        assert_eq!(changed, expected);
        let imported = events_of(world, &EventKind::Imported)?;
        assert_eq!(imported.len(), 1);
        let (_, imported_mutation, event) = some(imported.first(), "the imported event")?;
        assert_eq!(
            (*imported_mutation, &event.revision),
            (Some(mutation), &revisions.last().cloned())
        );
        Ok(())
    }

    /// An import of two sources whose first attempt `breaks` makes fail as a worker fault
    /// (retryable, no completion), run again: it ends with every revision announced once.
    async fn import_announces_after(
        number: u128,
        breaks: impl FnOnce(&World) -> Built<()>,
    ) -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let first = upload(&world, number, "first.pdf", b"%PDF first")?;
        let second = upload(
            &world,
            number.saturating_add(1),
            "second.pdf",
            b"%PDF second",
        )?;
        let id = job(
            &world,
            number,
            workspace_scope()?,
            import_spec(vec![first, second], false)?,
        )?;
        breaks(&world)?;
        assert!(run(&world, id).await?);
        let failed = world.records.entry(id)?;
        assert_eq!(failed.job.state, JobState::Failed);
        assert!(failed.completions.is_empty());
        let (message, retryable) = some(failed.failures.first(), "the failure")?;
        assert!(retryable, "{message}");
        world.records.insert(id, failed.scope, failed.created)?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        assert_import_announced_once(&world, id)
    }

    #[tokio::test]
    async fn a_commit_whose_attempt_died_before_announcing_it_is_announced_by_the_replay()
    -> TestResult {
        // The first card's commit is written and the process dies before announcing it.
        import_announces_after(180, |world| Ok(world.versions.die_after(2)?)).await
    }

    #[tokio::test]
    async fn an_index_failure_fails_the_attempt_and_the_replay_announces() -> TestResult {
        // The first card's commit is the second revision indexed.
        import_announces_after(182, |world| {
            *world.search.fail_index.lock().map_err(|_| "poisoned")? = Some(2);
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn a_changed_append_failure_fails_the_attempt_and_the_replay_announces() -> TestResult {
        import_announces_after(184, |world| {
            Ok(world.events.fail_on(EventKind::Changed, 2)?)
        })
        .await
    }

    #[tokio::test]
    async fn an_imported_append_failure_fails_the_attempt_and_the_replay_appends_it_once()
    -> TestResult {
        import_announces_after(186, |world| {
            Ok(world.events.fail_on(EventKind::Imported, 1)?)
        })
        .await
    }

    /// The redigest `id` of `item` was indexed and announced exactly once, under its derived
    /// mutation and naming the item; a redigest appends no `imported`.
    fn assert_redigest_announced_once(
        world: &World,
        id: okf_jawn_contract::identity::JobId,
        item: ItemId,
    ) -> TestResult {
        let mutation = world.records.entry(id)?.created.mutation_id;
        let ours =
            some(
                world.versions.written()?.into_iter().find(|(changes, _)| {
                    changes.mutation_id == derive_commit_mutation_id(mutation, 0)
                }),
                "the redigest commit",
            )?
            .1;
        assert!(world.search.indexed()?.contains(&ours));
        let changed: Vec<Announced> = events_of(world, &EventKind::Changed)?
            .into_iter()
            .map(|(log, mutation, event)| (log, mutation, event.revision, event.item_id))
            .collect();
        assert_eq!(
            changed,
            [(
                EventScope::Workspace(storage_scope()?),
                Some(derive_commit_mutation_id(mutation, 0)),
                Some(ours),
                Some(item)
            )]
        );
        assert_eq!(events_of(world, &EventKind::Imported)?, Vec::new());
        Ok(())
    }

    #[tokio::test]
    async fn a_redigest_run_again_is_announced_once() -> TestResult {
        let (world, id, item) = redigest_world(36)?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        // The completion was lost: the rerun replays through the redigest's early replay.
        world.records.insert(id, entry.scope, entry.created)?;
        assert!(run(&world, id).await?);
        assert_eq!(world.records.entry(id)?.job.state, JobState::Succeeded);
        assert_redigest_announced_once(&world, id, item)
    }

    #[tokio::test]
    async fn a_redigest_whose_attempt_died_before_announcing_is_announced_once_by_the_replay()
    -> TestResult {
        let (world, id, item) = redigest_world(37)?;
        world.versions.die_after(1)?;
        assert!(run(&world, id).await?);
        let died = world.records.entry(id)?;
        assert_eq!(died.job.state, JobState::Failed);
        assert_eq!(events_of(&world, &EventKind::Changed)?, Vec::new());
        world.records.insert(id, died.scope, died.created)?;
        assert!(run(&world, id).await?);
        assert_eq!(world.records.entry(id)?.job.state, JobState::Succeeded);
        assert_redigest_announced_once(&world, id, item)
    }

    #[tokio::test]
    async fn a_resumed_attempts_renewals_keep_the_progress_its_record_holds() -> TestResult {
        let world = heartbeat_world()?;
        let id = restore_job(&world, 188)?;
        // An earlier attempt left the record at 37%.
        world.records.set_progress(id, 37)?;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let records = Arc::clone(&world.records);
        let observed = Arc::clone(&seen);
        *world.backups.restore_hold.lock().map_err(|_| "poisoned")? = Some(Arc::new(move || {
            let records = Arc::clone(&records);
            let observed = Arc::clone(&observed);
            Box::pin(async move {
                for _interval in 0..5 {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    let progress = records.entry(id).map(|entry| entry.job.progress);
                    if let (Ok(progress), Ok(mut seen)) = (progress, observed.lock()) {
                        seen.push(progress);
                    }
                }
            })
        }));
        let before = world.records.renewals();
        assert!(run(&world, id).await?);
        assert_completed_under(&world, id, 1)?;
        assert!(world.records.renewals() > before.saturating_add(2));
        // Every renewal wrote back the 37% the record held, and none was announced.
        let seen = seen.lock().map_err(|_| "poisoned")?.clone();
        assert_eq!(seen, [37; 5]);
        assert_eq!(events_of(&world, &EventKind::JobUpdated)?.len(), 1);
        Ok(())
    }

    /// The pictures the record of `card` names, each with its retained bytes.
    fn retained_pictures(world: &World, card: &SourceCard) -> Built<Vec<Kept>> {
        let digest = some(
            card.appearance.extraction.digest.as_ref(),
            "the record digest",
        )?;
        let record: ConversionRecord = serde_json::from_slice(&world.blobs.bytes(digest)?)?;
        Ok(record
            .assets
            .iter()
            .map(|asset| Ok((asset.clone(), world.blobs.bytes(&asset.digest)?)))
            .collect::<Result<Vec<_>, okf_jawn_contract::error::ApiError>>()?)
    }

    #[tokio::test]
    async fn an_import_retains_each_windows_pictures_in_its_record_and_its_outputs() -> TestResult {
        // Six pages in windows of four: two windows, one picture each.
        let world = world_with(Some(6), Vec::new())?;
        world.converter.pictures.store(true, Ordering::SeqCst);
        let report = upload(&world, 190, "report.pdf", b"%PDF report")?;
        let id = job(
            &world,
            190,
            workspace_scope()?,
            import_spec(vec![report], false)?,
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let cards = cards(&world)?;
        let card = some(cards.last(), "the converted card")?;
        let pictures = retained_pictures(&world, card)?;
        // Both windows' pictures are merged into the record, in page order, each retained.
        let seen: Vec<Seen> = pictures
            .iter()
            .map(|(asset, bytes)| {
                (
                    asset.media_type.clone(),
                    asset.caption.clone(),
                    asset.pixel_size,
                    bytes.clone(),
                )
            })
            .collect();
        let size = Some(PixelSize {
            width: 20,
            height: 10,
        });
        assert_eq!(
            seen,
            [
                (
                    "image/png".to_owned(),
                    Some("Figure 1".to_owned()),
                    size,
                    b"picture of window 1, run 0".to_vec()
                ),
                (
                    "image/png".to_owned(),
                    Some("Figure 5".to_owned()),
                    size,
                    b"picture of window 5, run 1".to_vec()
                ),
            ]
        );
        // And the job's outputs list each picture's object.
        let completion = some(entry.completions.first(), "the completion")?;
        for (asset, _bytes) in &pictures {
            assert!(completion.outputs.contains(&asset.digest), "{asset:?}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_redigest_of_a_source_imported_before_pictures_were_kept_retains_them() -> TestResult
    {
        // The card was imported before assets were retained: its extraction names no record.
        let (world, id, item_id) = redigest_world(38)?;
        world.converter.pictures.store(true, Ordering::SeqCst);
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let completion = some(entry.completions.first(), "the completion")?;
        let head = some(completion.revision.as_ref(), "the redigest's revision")?;
        let card = some(
            world.versions.item_at(head, item_id)?,
            "the redigested card",
        )?;
        let pictures = retained_pictures(&world, &card)?;
        assert_eq!(pictures.len(), 1);
        let (asset, bytes) = some(pictures.first(), "the picture")?;
        assert_eq!(bytes, b"picture of window 1, run 0");
        assert_eq!(asset.caption.as_deref(), Some("Figure 1"));
        assert!(completion.outputs.contains(&asset.digest));
        Ok(())
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
        // The job carries what the archive left out.
        assert_eq!(
            completion.backup,
            Some(BackupReport {
                rows_left_out: ARCHIVE_ROWS_LEFT_OUT
            })
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
        let tenant_completion = world.records.entry(tenant_job)?;
        assert_eq!(
            some(tenant_completion.completions.first(), "the completion")?.backup,
            None,
            "an installation archive has no rows of a workspace to leave out"
        );
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
        let kinds = [(
            5,
            JobSpec::ExportView {
                item_id: serde_json::from_value::<ItemId>(json!(
                    "00000000-0000-0000-0000-0000000000ef"
                ))?,
                revision: base()?,
            },
        )];
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

    /// R3 builds tenant execution; the invalid workspace adapter input remains an Internal fault.
    #[tokio::test]
    async fn clean_up_dispatch_refuses_workspace_scope() -> TestResult {
        let world = world()?;
        let spec = JobSpec::from_stored(json!({ "kind": "clean_up" }))?;
        let id = job(&world, 790, workspace_scope()?, spec)?;
        let claimed = some(world.records.claim_job(id).await?, "the test claim")?;
        let refusal = err_of(world.handler.handle(&claimed).await)?;
        assert_eq!(refusal.code, ErrorCode::Internal);
        assert!(world.records.entry(id)?.completions.is_empty());
        assert!(world.versions.written()?.is_empty());
        Ok(())
    }

    /// Lifecycle design3.2/R3: one tenant pass forwards its literal count-only report,
    /// called with the actual tenant/job identities, with no workspace effect.
    #[tokio::test]
    async fn clean_up_forwards_the_tenant_job_and_literal_report() -> TestResult {
        let world = world()?;
        let tenant_b = TenantId::try_from("tenant-b".to_owned())?;
        let id = job(
            &world,
            791,
            JobScope::Tenant(tenant_b.clone()),
            JobSpec::CleanUp,
        )?;
        assert!(run(&world, id).await?);
        assert_completed_under(&world, id, 1)?;
        let entry = world.records.entry(id)?;
        let expected = okf_jawn_contract::import::CleanUpReport {
            objects_removed: "18446744073709551616".to_owned(),
            objects_kept: "7".to_owned(),
            upload_files_removed: 2,
        };
        assert_eq!(
            some(entry.completions.first(), "the completion")?.clean_up,
            Some(expected.clone())
        );
        assert_eq!(entry.job.clean_up, Some(expected));
        assert_eq!(
            *world.purger.clean_up_calls.lock().map_err(|_| "poisoned")?,
            [(tenant_b.clone(), id)]
        );
        assert!(world.versions.written()?.is_empty());
        assert_eq!(
            world.records.artifacts()?,
            [] as [okf_jawn_core::jobs::ArtifactRecord; 0]
        );
        assert_eq!(
            world.purger.calls.load(Ordering::SeqCst),
            0,
            "no purge invoked"
        );
        let updates = events_of(&world, &EventKind::JobUpdated)?;
        assert_eq!(updates.len(), 1);
        assert_eq!(
            some(updates.first(), "the tenant update")?.0,
            EventScope::Tenant(tenant_b)
        );
        Ok(())
    }

    /// A damaged survivor is the runtime's Internal failure, not a successful empty sweep
    /// and not the purge handler's separate resumable-record path (design3.2).
    #[tokio::test]
    async fn clean_up_failure_is_recorded_by_the_runtime_without_completion() -> TestResult {
        let world = world()?;
        let id = job(&world, 792, JobScope::Tenant(tenant()?), JobSpec::CleanUp)?;
        world.purger.fail_next(ApiError::new(
            ErrorCode::Internal,
            "survivor workspace 00000000-0000-0000-0000-0000000000aa is unreadable",
        ))?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        assert_eq!(
            some(entry.job.error.as_ref(), "the runtime's error")?.code,
            ErrorCode::Internal
        );
        assert_eq!(
            entry.failures,
            [(
                "survivor workspace 00000000-0000-0000-0000-0000000000aa is unreadable".to_owned(),
                true
            )]
        );
        assert!(entry.completions.is_empty());
        assert_eq!(entry.job.clean_up, None);
        assert_eq!(
            *world.purger.clean_up_calls.lock().map_err(|_| "poisoned")?,
            [(tenant()?, id)]
        );
        assert!(world.versions.written()?.is_empty());
        Ok(())
    }

    /// Reuse the same six50ms lease windows as existing rebuild/backup/purge controls.
    /// This proves only that this new storage arm uses renewing, not storage lease fencing.
    #[tokio::test]
    async fn clean_up_outlasting_lease_intervals_renews_the_first_claim() -> TestResult {
        let world = heartbeat_world()?;
        *world.purger.hold.lock().map_err(|_| "poisoned")? = Some(lapsing(&world.records));
        let id = job(&world, 793, JobScope::Tenant(tenant()?), JobSpec::CleanUp)?;
        assert!(run(&world, id).await?);
        assert_completed_under(&world, id, 1)?;
        assert!(
            world.records.renewals() >= 6,
            "{} renewals",
            world.records.renewals()
        );
        assert_eq!(events_of(&world, &EventKind::JobUpdated)?.len(), 1);
        assert_eq!(
            *world.purger.clean_up_calls.lock().map_err(|_| "poisoned")?,
            [(tenant()?, id)]
        );
        Ok(())
    }

    /// Tenant jobs have no public cancel route. This adversarial fake-ledger cancellation
    /// verifies renewing's shared rule: finish the in-flight storage call, then no completion.
    #[tokio::test]
    async fn clean_up_observes_cancellation_after_the_storage_call_returns() -> TestResult {
        let world = heartbeat_world()?;
        let id = job(&world, 794, JobScope::Tenant(tenant()?), JobSpec::CleanUp)?;
        let records = Arc::clone(&world.records);
        let finished = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&finished);
        *world.purger.hold.lock().map_err(|_| "poisoned")? = Some(Arc::new(move || {
            let records = Arc::clone(&records);
            let seen = Arc::clone(&seen);
            Box::pin(async move {
                let _cancelled = records.cancel(id);
                tokio::time::sleep(Duration::from_millis(50)).await;
                let _previous = seen.fetch_add(1, Ordering::SeqCst);
            })
        }));
        assert!(run(&world, id).await?);
        assert_eq!(
            finished.load(Ordering::SeqCst),
            1,
            "storage work returned, not dropped"
        );
        assert_eq!(world.records.entry(id)?.job.state, JobState::Cancelled);
        assert!(world.records.entry(id)?.completions.is_empty());
        assert_eq!(
            world.records.entry(id)?.failures,
            [] as [(std::string::String, bool); 0]
        );
        assert_eq!(
            *world.purger.clean_up_calls.lock().map_err(|_| "poisoned")?,
            [(tenant()?, id)]
        );
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
        // The completion names the commit the restore wrote under the job's mutation.
        let restored = some(
            world
                .versions
                .written()?
                .into_iter()
                .find(|(changes, _)| changes.mutation_id == entry.created.mutation_id),
            "the restore's commit",
        )?
        .1;
        assert_eq!(completion.revision.as_ref(), Some(&restored));
        // The restore's revision is indexed and announced under the job's mutation.
        assert_eq!(world.search.indexed()?, std::slice::from_ref(&restored));
        let changed = events_of(&world, &EventKind::Changed)?;
        assert_eq!(changed.len(), 1);
        let (log, mutation, event) = some(changed.first(), "the changed event")?;
        assert_eq!(
            (log, *mutation, &event.revision, event.item_id, event.job_id),
            (
                &EventScope::Workspace(storage_scope()?),
                Some(entry.created.mutation_id),
                &Some(restored.clone()),
                None,
                Some(id)
            )
        );

        // Someone edits after the restore, and its completion is lost: the attempt that runs
        // again records the restore's commit, not the later head.
        world.versions.edit_elsewhere(None)?;
        world
            .records
            .insert(id, entry.scope.clone(), entry.created.clone())?;
        assert!(run(&world, id).await?);
        let again = world.records.entry(id)?;
        assert_eq!(again.job.state, JobState::Succeeded, "{:?}", again.failures);
        assert_eq!(world.versions.written()?.len(), 2);
        assert_eq!(
            some(again.completions.first(), "the completion")?
                .revision
                .as_ref(),
            Some(&restored)
        );
        // Announced again, with no second event.
        assert_eq!(events_of(&world, &EventKind::Changed)?.len(), 1);
        Ok(())
    }

    /// Review B1: a restore's completion carries no warning. No check runs for a restore's
    /// commit, so the store keeps no warnings for it and `find_commit` answers it with the one
    /// `replay_warnings_unavailable` warning, as real storage does and as this fake does (it is
    /// asserted here, so the handler is shown to be given one). The handler reads the revision
    /// only: on a first attempt, and on a resumed attempt that finds the commit its first
    /// attempt wrote.
    #[tokio::test]
    async fn a_restores_completion_carries_no_warning_on_a_first_or_a_resumed_attempt() -> TestResult
    {
        let world = world()?;
        let id = restore_job(&world, 125)?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let completion = some(entry.completions.first(), "the completion")?;
        let restored = some(completion.revision.clone(), "the restore's revision")?;
        // The store answers the restore's commit with the unavailable warning.
        let found = world
            .versions
            .find_commit(&storage_scope()?, entry.created.mutation_id, None)
            .await?;
        let found = some(found, "the restore's commit")?;
        assert_eq!(found.revision, restored);
        assert_eq!(
            found
                .warnings
                .iter()
                .map(|warning| warning.code.as_str())
                .collect::<Vec<_>>(),
            ["replay_warnings_unavailable"]
        );
        assert!(completion.warnings.is_empty(), "{:?}", completion.warnings);

        // The completion is lost: the attempt that runs again finds the commit, and completes
        // with its revision and still no warning.
        world
            .records
            .insert(id, entry.scope.clone(), entry.created.clone())?;
        assert!(run(&world, id).await?);
        let again = world.records.entry(id)?;
        assert_eq!(again.job.state, JobState::Succeeded, "{:?}", again.failures);
        assert_eq!(world.versions.written()?.len(), 1, "no second commit");
        let resumed = some(again.completions.first(), "the resumed completion")?;
        assert_eq!(resumed.revision.as_ref(), Some(&restored));
        assert!(resumed.warnings.is_empty(), "{:?}", resumed.warnings);
        Ok(())
    }

    /// A restore job of `world` under the job number `number`.
    fn restore_job(world: &World, number: u128) -> Built<okf_jawn_contract::identity::JobId> {
        let upload_id = upload(world, number, "backup.zip", b"archive bytes")?;
        job(
            world,
            number,
            workspace_scope()?,
            JobSpec::RestoreWorkspace {
                upload_id,
                archive: Digest::try_from("cd".repeat(32))?,
                editors: vec!["alice".to_owned()],
            },
        )
    }

    #[tokio::test]
    async fn a_restore_that_outlasts_several_lease_intervals_keeps_its_lease() -> TestResult {
        // A heartbeat of 10 ms; a lease that lapses when 50 ms pass without a renewal; a
        // restore that takes six of those intervals.
        let world = world_full(
            Some(6),
            Vec::new(),
            Arc::new(AcceptAll),
            Duration::from_millis(10),
        )?;
        let id = restore_job(&world, 120)?;
        let records = Arc::clone(&world.records);
        *world.backups.restore_hold.lock().map_err(|_| "poisoned")? = Some(Arc::new(move || {
            let records = Arc::clone(&records);
            Box::pin(async move {
                for _interval in 0..6 {
                    let before = records.renewals();
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    if records.renewals() == before {
                        let _lapsed = records.expire_running();
                    }
                }
            })
        }));
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        // Completed under its own lease, the first attempt's.
        let completion = some(entry.completions.first(), "the completion")?;
        assert_eq!(
            (completion.lease.attempt, completion.lease.token.as_str()),
            (1, "lease-1")
        );
        assert!(
            world.records.renewals() >= 6,
            "{} renewals",
            world.records.renewals()
        );
        // Renewals that changed neither progress nor state announced nothing: the only
        // `job_updated` is the completion's.
        assert_eq!(events_of(&world, &EventKind::JobUpdated)?.len(), 1);
        Ok(())
    }

    /// A hold that outlasts six lease intervals of 50 ms, in each of which the lease lapses
    /// unless the job renewed it.
    fn lapsing(records: &Arc<FakeRecords>) -> Hold {
        let records = Arc::clone(records);
        Arc::new(move || {
            let records = Arc::clone(&records);
            Box::pin(async move {
                for _interval in 0..6 {
                    let before = records.renewals();
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    if records.renewals() == before {
                        let _lapsed = records.expire_running();
                    }
                }
            })
        })
    }

    /// The job succeeded, completed under the lease of its `attempt`-th claim.
    fn assert_completed_under(
        world: &World,
        id: okf_jawn_contract::identity::JobId,
        attempt: u32,
    ) -> TestResult {
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let completion = some(entry.completions.first(), "the completion")?;
        assert_eq!(
            (completion.lease.attempt, completion.lease.token.clone()),
            (attempt, format!("lease-{attempt}"))
        );
        Ok(())
    }

    /// A world with a 10 ms heartbeat, whose lease lapses after 50 ms without a renewal.
    fn heartbeat_world() -> Built<World> {
        world_full(
            Some(3),
            Vec::new(),
            Arc::new(AcceptAll),
            Duration::from_millis(10),
        )
    }

    #[tokio::test]
    async fn a_rebuild_that_outlasts_several_lease_intervals_keeps_its_lease() -> TestResult {
        let world = heartbeat_world()?;
        *world.search.rebuild_hold.lock().map_err(|_| "poisoned")? = Some(lapsing(&world.records));
        let id = job(&world, 170, workspace_scope()?, JobSpec::RebuildIndex)?;
        assert!(run(&world, id).await?);
        assert_completed_under(&world, id, 1)?;
        assert_eq!(world.search.rebuilt()?, [base()?]);
        Ok(())
    }

    #[tokio::test]
    async fn both_backups_that_outlast_several_lease_intervals_keep_their_leases() -> TestResult {
        let world = heartbeat_world()?;
        *world.backups.write_hold.lock().map_err(|_| "poisoned")? = Some(lapsing(&world.records));
        let workspace = job(&world, 171, workspace_scope()?, JobSpec::BackupWorkspace)?;
        assert!(run(&world, workspace).await?);
        assert_completed_under(&world, workspace, 1)?;
        let installation = job(
            &world,
            172,
            JobScope::Tenant(tenant()?),
            JobSpec::BackupInstallation,
        )?;
        assert!(run(&world, installation).await?);
        assert_completed_under(&world, installation, 1)?;
        assert_eq!(world.records.artifacts()?.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn both_purges_that_outlast_several_lease_intervals_keep_their_leases() -> TestResult {
        let world = heartbeat_world()?;
        *world.purger.hold.lock().map_err(|_| "poisoned")? = Some(lapsing(&world.records));
        let workspace_id = serde_json::from_value::<WorkspaceId>(json!(WORKSPACE))?;
        let item = "00000000-0000-0000-0000-0000000000dd";
        let purges = [
            (
                173,
                "00000000-0000-0000-0000-0000000000c1",
                json!({ "kind": "workspace", "workspace_id": WORKSPACE }),
            ),
            (
                174,
                "00000000-0000-0000-0000-0000000000c2",
                json!({ "kind": "item", "workspace_id": WORKSPACE, "item_id": item }),
            ),
        ];
        for (number, purge, target) in purges {
            let purge_id = serde_json::from_value::<PurgeId>(json!(purge))?;
            let spec = if number == 173 {
                JobSpec::PurgeWorkspace {
                    purge_id,
                    workspace_id,
                }
            } else {
                JobSpec::PurgeItem {
                    purge_id,
                    workspace_id,
                    item_id: serde_json::from_value(json!(item))?,
                }
            };
            let id = job(&world, number, JobScope::Tenant(tenant()?), spec)?;
            world
                .records
                .insert_purge(serde_json::from_value::<Purge>(json!({
                    "id": purge,
                    "target": target,
                    "state": "requested",
                    "job_id": serde_json::to_value(id)?,
                    "requested_by": "admin",
                    "requested_at": "2026-10-09T00:00:00.000Z",
                }))?)?;
            assert!(run(&world, id).await?);
            assert_completed_under(&world, id, 1)?;
            assert_eq!(world.records.purge(purge_id)?.state, PurgeState::Completed);
        }
        assert_eq!(world.purger.calls.load(Ordering::SeqCst), 2);
        Ok(())
    }

    /// An import of two sources in a world with a 10 ms heartbeat whose first attempt died as
    /// its first card's commit returned, queued again to resume (the fake ledger queues it
    /// afresh, so the resumed claim is its first again).
    async fn resumed_import(number: u128) -> Built<(World, okf_jawn_contract::identity::JobId)> {
        let world = heartbeat_world()?;
        let first = upload(&world, number, "first.pdf", b"%PDF first")?;
        let second = upload(
            &world,
            number.saturating_add(1),
            "second.pdf",
            b"%PDF second",
        )?;
        let id = job(
            &world,
            number,
            workspace_scope()?,
            import_spec(vec![first, second], false)?,
        )?;
        world.versions.die_after(2)?;
        assert!(run(&world, id).await?);
        let died = world.records.entry(id)?;
        assert_eq!(died.job.state, JobState::Failed);
        world.records.insert(id, died.scope, died.created)?;
        Ok((world, id))
    }

    #[tokio::test]
    async fn a_replayed_cards_outputs_read_that_outlasts_several_lease_intervals_keeps_its_lease()
    -> TestResult {
        let (world, id) = resumed_import(177).await?;
        // Reading back the record the replayed first card references is long.
        let cards = cards(&world)?;
        let committed = some(cards.get(2), "the first converted card")?;
        let record = some(
            committed.appearance.extraction.digest.clone(),
            "the record digest",
        )?;
        *world
            .blobs
            .materialize_hold
            .lock()
            .map_err(|_| "poisoned")? = Some((record, lapsing(&world.records)));
        assert!(run(&world, id).await?);
        assert_completed_under(&world, id, 1)?;
        assert!(
            world
                .blobs
                .materialize_hold
                .lock()
                .map_err(|_| "poisoned")?
                .is_none()
        );
        Ok(())
    }

    /// R-I11: a cancellation asked for once a restore sealed itself, while `restore_import`
    /// runs, is refused with `Conflict` (the store's seal), so nothing marks the job cancelled
    /// and it completes with its report. Before the seal, this cancellation took effect and the
    /// store then refused the completion of a restore that had been applied.
    #[tokio::test]
    async fn a_restore_cannot_be_cancelled_once_it_sealed_and_completes() -> TestResult {
        let world = heartbeat_world()?;
        let id = restore_job(&world, 121)?;
        let upload_id = match &world.records.entry(id)?.created.spec {
            JobSpec::RestoreWorkspace { upload_id, .. } => *upload_id,
            _ => return Err("a restore was expected".into()),
        };
        let records = Arc::clone(&world.records);
        let answers = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&answers);
        let scope = workspace_scope()?;
        let cancel =
            serde_json::from_value::<MutationId>(json!("00000000-0000-0000-0000-000000000921"))?;
        *world.backups.restore_hold.lock().map_err(|_| "poisoned")? = Some(Arc::new(move || {
            let records = Arc::clone(&records);
            let kept = Arc::clone(&kept);
            let scope = scope.clone();
            Box::pin(async move {
                let answer = records.cancel_job(&scope, cancel, id).await;
                if let Ok(mut kept) = kept.lock() {
                    kept.push(answer.map(|job| job.state).map_err(|error| error.code));
                }
                // Long enough for several renewals.
                tokio::time::sleep(Duration::from_millis(50)).await;
            })
        }));
        assert!(run(&world, id).await?);
        assert_eq!(
            answers.lock().map_err(|_| "poisoned")?.clone(),
            [Err(ErrorCode::Conflict)],
            "the cancellation was refused"
        );
        assert!(world.records.entry(id)?.sealed);
        assert_completed_under(&world, id, 1)?;
        assert_eq!(world.uploads.consumed()?, [(upload_id, id)]);
        assert_eq!(
            world.backups.restored.lock().map_err(|_| "poisoned")?.len(),
            1
        );
        let changed = events_of(&world, &EventKind::Changed)?;
        assert_eq!(changed.len(), 1);
        Ok(())
    }

    /// R-I11 and ruling F2 (core M1.4b fix round 1): the restore seals before it consumes its
    /// upload, so a cancellation the store takes just before the seal wins: the seal answers the
    /// job cancelled, and the restore stops before it consumes anything, restoring nothing. Its
    /// upload is not asked for and stays usable (contract ruling 23's first case). Under the old
    /// order (consume, then seal) the upload stayed consumed by the cancelled job, which no
    /// retry could use.
    #[tokio::test]
    async fn a_restore_cancelled_just_before_its_seal_restores_nothing() -> TestResult {
        let world = world()?;
        let id = restore_job(&world, 127)?;
        world.records.cancel_before_seal(id)?;
        assert!(run(&world, id).await?);
        assert_stopped(&world, id)?;
        assert!(!world.records.entry(id)?.sealed);
        assert_eq!(
            world.backups.restored.lock().map_err(|_| "poisoned")?.len(),
            0,
            "stopped before restore_import"
        );
        assert!(world.versions.written()?.is_empty());
        assert_eq!(world.uploads.singles()?, Vec::new(), "not asked for");
        assert_eq!(
            world.uploads.consumed()?,
            Vec::new(),
            "the upload stays usable"
        );
        Ok(())
    }

    /// Ruling F2: a consume that fails after the seal fails the job, which stays sealed (a
    /// sealed job may fail; only its cancellation is refused). Here a purge recorded meanwhile
    /// refuses the consumption: the sealed job is refused for good, restores nothing, and its
    /// upload stays unmarked.
    #[tokio::test]
    async fn a_restore_whose_consume_is_refused_after_its_seal_fails_sealed() -> TestResult {
        let world = world()?;
        let id = restore_job(&world, 129)?;
        purge_names_the_workspace(&world)?;
        assert!(run(&world, id).await?);
        assert_refused_by_the_purge(&world, id)?;
        assert!(
            world.records.entry(id)?.sealed,
            "sealed before the consume, and failed sealed"
        );
        assert_eq!(world.uploads.consumed()?, Vec::new());
        assert_eq!(
            world.backups.restored.lock().map_err(|_| "poisoned")?.len(),
            0
        );
        assert!(world.versions.written()?.is_empty());
        Ok(())
    }

    /// R-I11: a sealed restore whose attempt dies (its lease lapses while `restore_import`
    /// runs, so neither its completion nor its failure can be written) is queued again, still
    /// sealed, so its cancellation stays refused; the next attempt resumes it and completes,
    /// its upload consumed by it once and the restore's commit written once.
    #[tokio::test]
    async fn a_sealed_restore_whose_lease_lapsed_resumes_and_consumes_its_upload_once() -> TestResult
    {
        let world = heartbeat_world()?;
        let id = restore_job(&world, 128)?;
        let upload_id = match &world.records.entry(id)?.created.spec {
            JobSpec::RestoreWorkspace { upload_id, .. } => *upload_id,
            _ => return Err("a restore was expected".into()),
        };
        let records = Arc::clone(&world.records);
        *world.backups.restore_hold.lock().map_err(|_| "poisoned")? = Some(Arc::new(move || {
            let records = Arc::clone(&records);
            Box::pin(async move {
                let _lapsed = records.expire_running();
                tokio::time::sleep(Duration::from_millis(50)).await;
            })
        }));
        let died = err_of(run(&world, id).await)?;
        assert!(died.to_string().contains("lease"), "{died}");
        *world.backups.restore_hold.lock().map_err(|_| "poisoned")? = None;
        assert_eq!(world.records.expire_leases().await?, 1);
        let queued = world.records.entry(id)?;
        assert_eq!(queued.job.state, JobState::Queued);
        assert!(queued.sealed, "the seal stays with the job");
        let refused = err_of(
            world
                .records
                .cancel_job(&workspace_scope()?, queued.created.mutation_id, id)
                .await,
        )?;
        assert_eq!(refused.code, ErrorCode::Conflict);
        assert!(run(&world, id).await?);
        assert_completed_under(&world, id, 2)?;
        assert_eq!(
            world.uploads.consumed()?,
            [(upload_id, id)],
            "consumed once"
        );
        assert_eq!(world.uploads.singles()?.len(), 2, "asked by each attempt");
        let mutation = queued.created.mutation_id;
        assert_eq!(
            world
                .versions
                .written()?
                .iter()
                .filter(|(changes, _)| changes.mutation_id == mutation)
                .count(),
            1,
            "the restore's commit is written once"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_restore_cancelled_before_it_consumes_its_upload_restores_nothing() -> TestResult {
        let world = world()?;
        let id = restore_job(&world, 122)?;
        world.records.cancel_when_claimed(id)?;
        assert!(run(&world, id).await?);
        assert_stopped(&world, id)?;
        assert_eq!(world.uploads.consumed()?, Vec::new());
        assert_eq!(
            world.backups.restored.lock().map_err(|_| "poisoned")?.len(),
            0
        );
        assert!(world.versions.written()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn a_second_restore_of_the_same_upload_is_refused_and_restores_nothing() -> TestResult {
        let world = world()?;
        let first = restore_job(&world, 123)?;
        assert!(run(&world, first).await?);
        assert_completed_under(&world, first, 1)?;
        let created = world.records.entry(first)?.created;
        // Another job restoring the same upload.
        let second = job_id(124)?;
        world.records.insert(
            second,
            workspace_scope()?,
            NewJob {
                mutation_id: serde_json::from_value::<MutationId>(json!(
                    "00000000-0000-0000-0000-000000000124"
                ))?,
                ..created
            },
        )?;
        assert!(run(&world, second).await?);
        let entry = world.records.entry(second)?;
        assert_eq!(entry.job.state, JobState::Failed);
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(message.contains("another job"), "{message}");
        assert!(!retryable);
        // The store's refusal is the job's failure as it was given, its code kept.
        assert_eq!(
            entry.job.error.as_ref().map(|error| &error.code),
            Some(&ErrorCode::Conflict)
        );
        // Refused at the consume, before restore_import: one restore applied, by the first job.
        assert_eq!(
            world.backups.restored.lock().map_err(|_| "poisoned")?.len(),
            1
        );
        Ok(())
    }

    /// A purge now names the test workspace: the upload store refuses every consumption.
    fn purge_names_the_workspace(world: &World) -> TestResult {
        *world.uploads.purged.lock().map_err(|_| "poisoned")? =
            Some(serde_json::from_value::<PurgeId>(json!(PURGE))?);
        Ok(())
    }

    /// Assert that job `id` was refused for good by the purge's `Conflict`: failed once, not
    /// retryable, its failure naming the purge.
    fn assert_refused_by_the_purge(
        world: &World,
        id: okf_jawn_contract::identity::JobId,
    ) -> TestResult {
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        assert!(entry.completions.is_empty(), "{:?}", entry.completions);
        assert_eq!(entry.failures.len(), 1, "{:?}", entry.failures);
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(
            message.contains("is purged by purge") && message.contains(PURGE),
            "the failure names the purge: {message}"
        );
        assert!(!retryable, "a retry would meet the same refusal");
        Ok(())
    }

    /// Review N9: a restore whose `consume` meets the purge's `Conflict` (the store refuses a
    /// consumption in a workspace a purge names) is refused for good, like a restore of an
    /// upload another job took: nothing is restored, the upload stays unmarked, and no commit
    /// is written or announced.
    #[tokio::test]
    async fn a_restore_whose_upload_is_refused_by_a_purge_is_refused_for_good() -> TestResult {
        let world = world()?;
        let id = restore_job(&world, 126)?;
        let upload_id = match &world.records.entry(id)?.created.spec {
            JobSpec::RestoreWorkspace { upload_id, .. } => *upload_id,
            _ => return Err("a restore was expected".into()),
        };
        purge_names_the_workspace(&world)?;
        assert!(run(&world, id).await?);
        assert_refused_by_the_purge(&world, id)?;
        // It asked for its one upload, once, and the store marked nothing.
        assert_eq!(world.uploads.singles()?, [(upload_id, id)]);
        assert_eq!(world.uploads.consumed()?, Vec::new());
        assert_eq!(
            world.backups.restored.lock().map_err(|_| "poisoned")?.len(),
            0,
            "refused before restore_import"
        );
        assert!(world.versions.written()?.is_empty());
        assert_eq!(events_of(&world, &EventKind::Changed)?, Vec::new());
        Ok(())
    }

    /// Review N9: an import whose batch meets the purge's `Conflict` is refused for good, like
    /// an import of an upload another job took: no upload is marked and nothing is written.
    #[tokio::test]
    async fn an_import_whose_batch_is_refused_by_a_purge_is_refused_for_good() -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let first = upload(&world, 170, "first.pdf", b"%PDF first")?;
        let second = upload(&world, 171, "second.pdf", b"%PDF second")?;
        let id = job(
            &world,
            170,
            workspace_scope()?,
            import_spec(vec![first, second], false)?,
        )?;
        purge_names_the_workspace(&world)?;
        assert!(run(&world, id).await?);
        assert_refused_by_the_purge(&world, id)?;
        // One batch was asked for, whole, and the store marked neither upload.
        assert_eq!(world.uploads.batches()?, [(vec![first, second], id)]);
        assert_eq!(world.uploads.singles()?, Vec::new());
        assert_eq!(world.uploads.consumed()?, Vec::new());
        assert!(world.versions.written()?.is_empty());
        assert!(
            world
                .converter
                .windows
                .lock()
                .map_err(|_| "poisoned")?
                .is_empty()
        );
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
        // Taken in one batch, in the request's order, and never one upload at a time (R-I13).
        assert_eq!(world.uploads.batches()?, [(vec![report, notes], id)]);
        assert_eq!(world.uploads.singles()?, Vec::new());

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

    /// Lifecycle design V7: the fake's literal tenant/digest classification is not derived
    /// from the import. No native converter or archive parser is claimed by these tests.
    async fn mark_managed(
        world: &World,
        upload: UploadId,
        owner: TenantId,
        kind: okf_jawn_core::storage::ManagedKind,
    ) -> TestResult {
        let record = world.uploads.get(&storage_scope()?, upload).await?;
        let object = some(record.object, "a complete managed upload")?;
        world
            .backups
            .managed
            .lock()
            .map_err(|_| "poisoned")?
            .push((owner, object.digest, kind));
        Ok(())
    }

    /// V7.1/V7.2: every managed kind is refused on its upload before conversion or a commit.
    #[tokio::test]
    async fn managed_backup_import_refuses_each_kind_without_consumption() -> TestResult {
        use okf_jawn_core::storage::ManagedKind;
        for (kind, label) in [
            (ManagedKind::WorkspaceArchive, "workspace archive"),
            (ManagedKind::InstallationArchive, "installation archive"),
            (ManagedKind::WrittenHistory, "written history"),
        ] {
            let world = world_with(None, Vec::new())?;
            let managed = upload(&world, 700, "renamed.pdf", b"known managed bytes")?;
            mark_managed(&world, managed, tenant()?, kind).await?;
            let id = job(
                &world,
                700,
                workspace_scope()?,
                import_spec(vec![managed], false)?,
            )?;
            assert!(run(&world, id).await?);
            let entry = world.records.entry(id)?;
            assert_eq!(entry.job.state, JobState::Failed, "{kind:?}");
            let error = some(entry.job.error.as_ref(), "the managed import refusal")?;
            assert_eq!(error.code, ErrorCode::InvalidInput);
            assert_eq!(error.field.as_deref(), Some("/upload_ids/0"));
            assert_eq!(
                error.message,
                format!(
                    "upload 0 is a {label} this installation wrote; restore it instead of importing it"
                )
            );
            assert_eq!(entry.failures.len(), 1);
            assert!(!some(entry.failures.first(), "the recorded refusal")?.1);
            assert!(world.uploads.consumed()?.is_empty());
            assert!(world.uploads.batches()?.is_empty());
            assert!(world.versions.written()?.is_empty());
            assert_eq!(world.converter.runs.load(Ordering::SeqCst), 0);
        }
        Ok(())
    }

    /// V7.3: a normal first upload remains available when the second is managed.
    #[tokio::test]
    async fn managed_backup_import_refuses_second_upload_without_taking_first() -> TestResult {
        let world = world_with(None, Vec::new())?;
        let first = upload(&world, 701, "ordinary.pdf", b"ordinary source")?;
        let second = upload(&world, 702, "backup.zip", b"known backup bytes")?;
        mark_managed(
            &world,
            second,
            tenant()?,
            okf_jawn_core::storage::ManagedKind::WorkspaceArchive,
        )
        .await?;
        let id = job(
            &world,
            701,
            workspace_scope()?,
            import_spec(vec![first, second], false)?,
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        let error = some(entry.job.error.as_ref(), "the second-upload refusal")?;
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("/upload_ids/1"));
        assert_eq!(
            error.message,
            "upload 1 is a workspace archive this installation wrote; restore it instead of importing it"
        );
        assert!(!some(entry.failures.first(), "the recorded refusal")?.1);
        assert!(world.uploads.consumed()?.is_empty());
        assert!(world.uploads.batches()?.is_empty());
        assert!(world.versions.written()?.is_empty());
        assert_eq!(world.converter.runs.load(Ordering::SeqCst), 0);
        Ok(())
    }

    /// V7.5 tenant boundary: identical bytes managed by another tenant are an ordinary source.
    #[tokio::test]
    async fn managed_backup_import_does_not_classify_another_tenants_digest() -> TestResult {
        let world = world_with(None, Vec::new())?;
        let input = upload(&world, 703, "ordinary.pdf", b"foreign tenant backup bytes")?;
        mark_managed(
            &world,
            input,
            TenantId::try_from("another-tenant".to_owned())?,
            okf_jawn_core::storage::ManagedKind::WorkspaceArchive,
        )
        .await?;
        let id = job(
            &world,
            703,
            workspace_scope()?,
            import_spec(vec![input], false)?,
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        assert_eq!(world.uploads.consumed()?, [(input, id)]);
        assert_eq!(world.converter.runs.load(Ordering::SeqCst), 1);
        Ok(())
    }

    /// V7.4: restore takes the very same managed upload through its separate consume path.
    #[tokio::test]
    async fn managed_backup_import_does_not_block_restoring_the_upload() -> TestResult {
        let world = world()?;
        let input = upload(&world, 704, "backup.zip", b"known workspace archive")?;
        mark_managed(
            &world,
            input,
            tenant()?,
            okf_jawn_core::storage::ManagedKind::WorkspaceArchive,
        )
        .await?;
        let record = world.uploads.get(&storage_scope()?, input).await?;
        let archive = some(record.object, "the archive bytes")?.digest;
        let id = job(
            &world,
            704,
            workspace_scope()?,
            JobSpec::RestoreWorkspace {
                upload_id: input,
                archive,
                editors: vec!["ana".to_owned()],
            },
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        assert_eq!(world.uploads.consumed()?, [(input, id)]);
        assert_eq!(
            *world.backups.restored.lock().map_err(|_| "poisoned")?,
            [vec!["ana".to_owned()]]
        );
        assert_eq!(world.converter.runs.load(Ordering::SeqCst), 0);
        Ok(())
    }
    #[tokio::test]
    async fn an_import_refused_for_one_upload_consumes_none_of_its_batch() -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let complete = upload(&world, 150, "complete.pdf", b"%PDF complete")?;
        let partial = incomplete_upload(&world, 151, "partial.pdf")?;
        let refused = job(
            &world,
            150,
            workspace_scope()?,
            import_spec(vec![complete, partial], false)?,
        )?;
        assert!(run(&world, refused).await?);
        let entry = world.records.entry(refused)?;
        assert_eq!(entry.job.state, JobState::Failed);
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(message.contains("not complete"), "{message}");
        assert_eq!(
            entry
                .job
                .error
                .as_ref()
                .and_then(|error| error.field.as_deref()),
            Some("/upload_ids/1"),
            "the incomplete upload is the second of the request"
        );
        // InvalidInput: not retried.
        assert!(!retryable);
        // The complete upload was not consumed, and nothing was written.
        assert_eq!(world.uploads.consumed()?, Vec::new());
        assert!(world.versions.written()?.is_empty());

        // So another job can still import it.
        let taker = job(
            &world,
            151,
            workspace_scope()?,
            import_spec(vec![complete], false)?,
        )?;
        assert!(run(&world, taker).await?);
        let entry = world.records.entry(taker)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        assert_eq!(world.uploads.consumed()?, [(complete, taker)]);

        // A batch holding an upload another job consumed is refused before any consume.
        let fresh = upload(&world, 152, "fresh.pdf", b"%PDF fresh")?;
        let late = job(
            &world,
            152,
            workspace_scope()?,
            import_spec(vec![fresh, complete], false)?,
        )?;
        assert!(run(&world, late).await?);
        let entry = world.records.entry(late)?;
        assert_eq!(entry.job.state, JobState::Failed);
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(message.contains("another job"), "{message}");
        // Refused for good, like a collision: a retry would meet the same refusal.
        assert_eq!(
            entry
                .job
                .error
                .as_ref()
                .and_then(|error| error.field.as_deref()),
            Some("/upload_ids/1"),
            "the taken upload is the second of the request"
        );
        assert!(!retryable);
        assert_eq!(entry.failures.len(), 1);
        assert_eq!(world.uploads.consumed()?, [(complete, taker)]);
        // Both refusals were the handler's own check: neither refused batch reached the store.
        assert_eq!(world.uploads.batches()?, [(vec![complete], taker)]);
        Ok(())
    }

    /// R-I13: another job takes one upload of the batch after the import checked it and before
    /// the import consumes. The store refuses the whole batch, so the import holds none of its
    /// uploads (the other job can still take the rest), and the import is refused for good.
    #[tokio::test]
    async fn an_import_whose_upload_another_job_takes_after_its_check_takes_none() -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let first = upload(&world, 160, "first.pdf", b"%PDF first")?;
        let shared = upload(&world, 161, "shared.pdf", b"%PDF shared")?;
        let other = job_id(161)?;
        *world
            .uploads
            .taken_before_batch
            .lock()
            .map_err(|_| "poisoned")? = Some((shared, other));
        let id = job(
            &world,
            160,
            workspace_scope()?,
            import_spec(vec![first, shared], false)?,
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(message.contains("another job"), "{message}");
        assert!(!retryable, "a retry would meet the same refusal");
        assert_eq!(entry.failures.len(), 1);
        // One batch was asked for, whole; the import marked neither upload.
        assert_eq!(world.uploads.batches()?, [(vec![first, shared], id)]);
        assert_eq!(world.uploads.singles()?, Vec::new());
        assert_eq!(world.uploads.consumed()?, [(shared, other)]);
        assert!(world.versions.written()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn an_import_whose_file_name_gives_no_card_path_consumes_none_of_its_batch() -> TestResult
    {
        let world = world_with(Some(3), Vec::new())?;
        let valid = upload(&world, 153, "valid.pdf", b"%PDF valid")?;
        let escaping = upload(&world, 154, "../report.pdf", b"%PDF escaping")?;
        let id = job(
            &world,
            153,
            workspace_scope()?,
            import_spec(vec![valid, escaping], false)?,
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(message.contains("gives no workspace path"), "{message}");
        assert!(!retryable);
        // The valid upload was not consumed, and nothing was written.
        assert_eq!(world.uploads.consumed()?, Vec::new());
        assert!(world.versions.written()?.is_empty());
        Ok(())
    }

    /// The last segment of each card path the import wrote.
    fn card_names(world: &World) -> Built<Vec<String>> {
        Ok(cards(world)?
            .iter()
            .filter_map(|card| card.path.as_str().rsplit('/').next().map(str::to_owned))
            .collect())
    }

    #[tokio::test]
    async fn an_import_of_a_file_with_a_long_name_succeeds_with_a_card_name_of_one_segment()
    -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let long = format!("{}.pdf", "q".repeat(296));
        assert_eq!(long.chars().count(), 300);
        let only = upload(&world, 157, &long, b"%PDF long")?;
        let id = job(
            &world,
            155,
            workspace_scope()?,
            import_spec(vec![only], false)?,
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let names = card_names(&world)?;
        assert_ne!(names, Vec::<String>::new(), "the import wrote its card");
        for name in &names {
            assert!(name.encode_utf16().count() <= 255, "{name}");
            assert!(name.ends_with("-pdf.md"), "{name}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_second_file_of_the_same_long_name_is_numbered_within_one_segment() -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let long = format!("{}.pdf", "q".repeat(296));
        let first = upload(&world, 158, &long, b"%PDF first")?;
        let second = upload(&world, 159, &long, b"%PDF second")?;
        let id = job(
            &world,
            156,
            workspace_scope()?,
            import_spec(vec![first, second], false)?,
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let names = card_names(&world)?;
        let pending: Vec<&String> = names.iter().take(2).collect();
        assert_eq!(
            pending,
            [
                &format!("{}-pdf.md", "q".repeat(248)),
                // The numbered name keeps the source-card tail `-pdf-2` whole and cuts the stem
                // before it to fit (review N8).
                &format!("{}-pdf-2.md", "q".repeat(246))
            ]
        );
        for name in &names {
            assert!(name.encode_utf16().count() <= 255, "{name}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_second_file_of_a_long_name_with_no_extension_is_numbered_within_one_segment()
    -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let long = "q".repeat(300);
        let first = upload(&world, 160, &long, b"%PDF first")?;
        let second = upload(&world, 161, &long, b"%PDF second")?;
        let id = job(
            &world,
            157,
            workspace_scope()?,
            import_spec(vec![first, second], false)?,
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let names = card_names(&world)?;
        let pending: Vec<&String> = names.iter().take(2).collect();
        assert_eq!(
            pending,
            [
                &format!("{}.md", "q".repeat(252)),
                &format!("{}-2.md", "q".repeat(250))
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_import_refused_for_a_file_name_names_the_upload_ids_field() -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let valid = upload(&world, 155, "valid.pdf", b"%PDF valid")?;
        let escaping = upload(&world, 156, "../report.pdf", b"%PDF escaping")?;
        let id = job(
            &world,
            154,
            workspace_scope()?,
            import_spec(vec![valid, escaping], false)?,
        )?;
        // The refusal names the one upload, the second of the request, and the job's failure
        // keeps it as given (`RecordStore::fail_job`).
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        assert_eq!(entry.job.retryable, Some(false));
        let refused = some(entry.job.error, "the failure")?;
        assert_eq!(refused.code, ErrorCode::InvalidInput);
        assert_eq!(refused.field.as_deref(), Some("/upload_ids/1"));
        assert!(
            refused.message.contains("gives no workspace path"),
            "{}",
            refused.message
        );
        assert_eq!(world.uploads.consumed()?, Vec::new());
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

    /// The warnings an import of two sources completes with, after its first attempt died right
    /// after writing `died` commits, when it did.
    async fn completed_warnings(
        died: Option<usize>,
        number: u128,
    ) -> Built<Vec<(String, String, Option<String>)>> {
        let world = world_with(Some(3), Vec::new())?;
        let first = upload(&world, number, "first.pdf", b"%PDF first")?;
        let second = upload(
            &world,
            number.saturating_add(1),
            "second.pdf",
            b"%PDF second",
        )?;
        let id = job(
            &world,
            number,
            workspace_scope()?,
            import_spec(vec![first, second], false)?,
        )?;
        if let Some(count) = died {
            world.versions.die_after(count)?;
            assert!(run(&world, id).await?);
            let entry = world.records.entry(id)?;
            assert_eq!(entry.job.state, JobState::Failed);
            world.records.insert(id, entry.scope, entry.created)?;
        }
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        assert_eq!(world.versions.written()?.len(), 3);
        Ok(some(entry.completions.first(), "the completion")?
            .warnings
            .iter()
            .map(|warning| {
                (
                    warning.code.clone(),
                    warning.message.clone(),
                    warning.location.clone(),
                )
            })
            .collect())
    }

    #[tokio::test]
    async fn a_resumed_import_completes_with_the_warnings_of_the_commits_it_replays() -> TestResult
    {
        let whole = completed_warnings(None, 130).await?;
        // OKF lint of the pending cards (no top-level heading) reached the completion.
        assert!(
            whole.iter().any(|(code, _, location)| {
                code.starts_with("okf_lint") && location.as_deref() == Some("inbox/second-pdf.md")
            }),
            "{whole:?}"
        );
        // Died after the pending commit, and after the first card's commit: the same warnings.
        assert_eq!(completed_warnings(Some(1), 132).await?, whole);
        assert_eq!(completed_warnings(Some(2), 134).await?, whole);
        Ok(())
    }

    /// A world whose conformance check answers for the whole candidate (`WholeCandidate`), with
    /// an import of `MANY_CARDS` sources (`doc-00.pdf` onward) as job 300.
    fn many_cards_import() -> Built<(World, okf_jawn_contract::identity::JobId)> {
        let world = world_full(
            Some(3),
            Vec::new(),
            Arc::new(WholeCandidate),
            Duration::from_secs(60),
        )?;
        let mut uploads = Vec::new();
        for card in 0..MANY_CARDS {
            uploads.push(upload(
                &world,
                300_u128.saturating_add(u128::from(card)),
                &format!("doc-{card:02}.pdf"),
                format!("%PDF {card}").as_bytes(),
            )?);
        }
        let id = job(
            &world,
            300,
            workspace_scope()?,
            import_spec(uploads, false)?,
        )?;
        Ok((world, id))
    }

    /// The locations of the warnings of `warnings` whose code starts with `code`, in order.
    fn located(warnings: &[Warning], code: &str) -> Vec<String> {
        warnings
            .iter()
            .filter(|warning| warning.code.starts_with(code))
            .filter_map(|warning| warning.location.clone())
            .collect()
    }

    /// `warnings` as a test compares them: code, message and location.
    fn compared(warnings: &[Warning]) -> Vec<(String, String, Option<String>)> {
        warnings
            .iter()
            .map(|warning| {
                (
                    warning.code.clone(),
                    warning.message.clone(),
                    warning.location.clone(),
                )
            })
            .collect()
    }

    /// Review N2: a commit's check reports what that commit wrote. An import of twelve cards
    /// runs thirteen commits over a candidate that holds all twelve, each with a conformance
    /// finding (`WholeCandidate`) and OKF lint findings. The pending commit, which writes all
    /// twelve, keeps and reports all twelve cards' findings; each card's own commit keeps and
    /// reports that card's only. So the completion holds each commit's findings once (two
    /// conformance findings a card, not thirteen), and no commit keeps more than the pending
    /// commit. A replayed attempt completes with the same warnings wherever it died.
    #[tokio::test]
    async fn an_import_keeps_and_reports_each_commits_own_warnings_not_every_cards_on_every_commit()
    -> TestResult {
        let (world, id) = many_cards_import()?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let mutation = entry.created.mutation_id;
        let cards = usize::try_from(MANY_CARDS)?;
        assert_eq!(world.versions.written()?.len(), cards.saturating_add(1));
        let paths: Vec<String> = (0..MANY_CARDS)
            .map(|card| format!("inbox/doc-{card:02}-pdf.md"))
            .collect();
        // What the store kept for each commit is what that commit's check answered.
        let kept = world
            .versions
            .warnings
            .lock()
            .map_err(|_| "poisoned")?
            .clone();
        let kept_for = |ordinal: u32| -> Built<Vec<Warning>> {
            let commit = derive_commit_mutation_id(mutation, ordinal);
            let found = kept.iter().find(|(of, _)| *of == commit);
            Ok(some(found, "the warnings kept for a commit")?.1.clone())
        };
        // The pending commit writes every card: every card's findings, each once.
        let pending = kept_for(0)?;
        assert_eq!(located(&pending, WHOLE_CANDIDATE), paths);
        let pending_lint = located(&pending, "okf_lint");
        for path in &paths {
            assert!(pending_lint.contains(path), "{path}: {pending:?}");
        }
        assert_eq!(pending.len(), 36, "no commit keeps more than this one");
        // Each card's own commit writes that card: that card's findings and no other card's,
        // though the candidate it checks holds all twelve.
        let mut card_lint = 0_usize;
        for (ordinal, path) in (1_u32..).zip(&paths) {
            let own = kept_for(ordinal)?;
            assert_eq!(
                located(&own, WHOLE_CANDIDATE),
                std::slice::from_ref(path),
                "{own:?}"
            );
            assert!(
                own.iter()
                    .all(|warning| warning.location.as_deref() == Some(path.as_str())),
                "the commit of {path} keeps a warning about another file: {own:?}"
            );
            let lint = located(&own, "okf_lint").len();
            assert_eq!(lint, 2, "{path} has two lint findings of its own: {own:?}");
            assert_eq!(own.len(), 3, "kept for the commit of {path}: {own:?}");
            card_lint = card_lint.saturating_add(lint);
        }
        // The completion is the sum: each commit's findings once.
        let completion = some(entry.completions.first(), "the completion")?;
        assert_eq!(
            located(&completion.warnings, WHOLE_CANDIDATE).len(),
            cards.saturating_mul(2),
            "twelve from the pending commit and one from each card's commit"
        );
        assert_eq!(
            located(&completion.warnings, "okf_lint").len(),
            pending_lint.len().saturating_add(card_lint)
        );
        // In numbers, for twelve cards with two lint findings each as pending and as converted:
        // the pending commit's 24 lint findings, the card commits' 24, and with the 24
        // conformance findings 72 warnings in all. A check over every card on every commit
        // would complete with 468 (36 for each of thirteen commits).
        assert_eq!(
            (pending_lint.len(), card_lint, completion.warnings.len()),
            (24, 24, 72)
        );

        // A resumed attempt replays the commits its first attempt wrote and completes with the
        // same warnings: died after the pending commit, after six cards, and after the last.
        for died in [1, 7, cards.saturating_add(1)] {
            let (resumed, id) = many_cards_import()?;
            resumed.versions.die_after(died)?;
            assert!(run(&resumed, id).await?);
            let stopped = resumed.records.entry(id)?;
            assert_eq!(stopped.job.state, JobState::Failed, "died after {died}");
            resumed.records.insert(id, stopped.scope, stopped.created)?;
            assert!(run(&resumed, id).await?);
            let again = resumed.records.entry(id)?;
            assert_eq!(again.job.state, JobState::Succeeded, "{:?}", again.failures);
            assert_eq!(resumed.versions.written()?.len(), cards.saturating_add(1));
            assert_eq!(
                compared(&some(again.completions.first(), "the completion")?.warnings),
                compared(&completion.warnings),
                "died after {died}"
            );
        }
        Ok(())
    }

    /// R-I10, the commit with no kept warnings: an import resumes over two commits the store
    /// holds no warnings for (as for commits written before it kept them). It completes, and
    /// each replayed commit contributes the store's one warning in place of its own.
    #[tokio::test]
    async fn a_resumed_import_over_commits_without_kept_warnings_says_so_and_completes()
    -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let first = upload(&world, 136, "first.pdf", b"%PDF first")?;
        let second = upload(&world, 137, "second.pdf", b"%PDF second")?;
        let id = job(
            &world,
            136,
            workspace_scope()?,
            import_spec(vec![first, second], false)?,
        )?;
        world.versions.die_after(2)?;
        assert!(run(&world, id).await?);
        let died = world.records.entry(id)?;
        assert_eq!(died.job.state, JobState::Failed);
        world.versions.forget_warnings()?;
        world.records.insert(id, died.scope, died.created)?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        assert_eq!(
            world.versions.written()?.len(),
            3,
            "no commit written twice"
        );
        let completion = some(entry.completions.first(), "the completion")?;
        let unavailable = completion
            .warnings
            .iter()
            .filter(|warning| warning.code == "replay_warnings_unavailable")
            .count();
        assert_eq!(unavailable, 2, "{:?}", completion.warnings);
        Ok(())
    }

    /// The retained outputs the cards at `revision` reference: each card's record, its window
    /// exports and assets.
    fn referenced_outputs(world: &World, revision: &Revision) -> Built<Vec<Digest>> {
        let mut outputs = Vec::new();
        for card in world.versions.items_at(revision)? {
            let Some(digest) = card.appearance.extraction.digest else {
                continue;
            };
            let record: ConversionRecord = serde_json::from_slice(&world.blobs.bytes(&digest)?)?;
            outputs.extend(record.structured.into_iter().map(|export| export.digest));
            outputs.extend(record.assets.into_iter().map(|asset| asset.digest));
            outputs.push(digest);
        }
        outputs.sort();
        Ok(outputs)
    }

    #[tokio::test]
    async fn a_resumed_import_converts_no_committed_card_and_lists_its_committed_outputs()
    -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let first = upload(&world, 140, "first.pdf", b"%PDF first")?;
        let second = upload(&world, 141, "second.pdf", b"%PDF second")?;
        let id = job(
            &world,
            140,
            workspace_scope()?,
            import_spec(vec![first, second], false)?,
        )?;
        // The first card is converted and committed; the process dies as that commit returns.
        world.versions.die_after(2)?;
        assert!(run(&world, id).await?);
        let died = world.records.entry(id)?;
        assert_eq!(died.job.state, JobState::Failed);
        assert_eq!(windows(&world)?.len(), 1);
        world
            .records
            .insert(id, died.scope.clone(), died.created.clone())?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        // Only the second source was converted again.
        assert_eq!(windows(&world)?.len(), 2);
        // The completion lists exactly what the committed cards reference, the first card's
        // outputs being those of the first attempt's conversion.
        let completion = some(entry.completions.first(), "the completion")?;
        let head = some(completion.revision.as_ref(), "the completion's revision")?;
        let mut listed = completion.outputs.clone();
        listed.sort();
        assert_eq!(listed, referenced_outputs(&world, head)?);
        assert_eq!(listed.len(), 4);
        Ok(())
    }

    #[tokio::test]
    async fn a_resumed_redigest_converts_nothing_and_lists_its_committed_outputs() -> TestResult {
        let (world, id, _item_id) = redigest_world(33)?;
        world.versions.die_after(1)?;
        assert!(run(&world, id).await?);
        let died = world.records.entry(id)?;
        assert_eq!(died.job.state, JobState::Failed);
        assert_eq!(windows(&world)?.len(), 1);
        world
            .records
            .insert(id, died.scope.clone(), died.created.clone())?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        assert_eq!(windows(&world)?.len(), 1);
        let completion = some(entry.completions.first(), "the completion")?;
        let head = some(completion.revision.as_ref(), "the completion's revision")?;
        let mut listed = completion.outputs.clone();
        listed.sort();
        assert_eq!(listed, referenced_outputs(&world, head)?);
        assert_eq!(listed.len(), 2);
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
    async fn an_import_never_overwrites_a_pending_card_a_person_changed_or_deleted() -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let edited = upload(&world, 110, "edited.pdf", b"%PDF edited")?;
        let deleted = upload(&world, 111, "deleted.pdf", b"%PDF deleted")?;
        let untouched = upload(&world, 112, "untouched.pdf", b"%PDF untouched")?;
        let id = job(
            &world,
            23,
            workspace_scope()?,
            import_spec(vec![edited, deleted, untouched], false)?,
        )?;
        let mutation = world.records.entry(id)?.created.mutation_id;
        let edited_id = derive_item_id(mutation, 0);
        let deleted_id = derive_item_id(mutation, 1);
        let mut theirs = std::collections::BTreeMap::new();
        let _previous = theirs.insert("reviewed".to_owned(), json!(true));
        // While each source converts, a person acts on that source's pending card: the first
        // gets a property, the second is deleted. An unrelated item is edited too.
        let versions = Arc::clone(&world.versions);
        let windows_seen = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&windows_seen);
        *world.converter.on_window.lock().map_err(|_| "poisoned")? =
            Some(Box::new(move |_window: Option<&PageRange>| {
                let edit = match seen.fetch_add(1, Ordering::SeqCst) {
                    0 => vec![TreeEdit::EditItem {
                        item_id: edited_id,
                        body: String::new(),
                        properties: theirs.clone(),
                    }],
                    1 => vec![TreeEdit::DeleteItem {
                        item_id: deleted_id,
                    }],
                    _ => Vec::new(),
                };
                let _landed = versions.edit_elsewhere_with(edit);
                false
            }));
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        // The job completes; the two cards are reported, not written, and not retried.
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let completion = some(entry.completions.first(), "the completion")?;
        let not_written: Vec<&str> = completion
            .warnings
            .iter()
            .filter(|warning| warning.code == CARD_NOT_WRITTEN)
            .filter_map(|warning| warning.location.as_deref())
            .collect();
        assert_eq!(
            not_written,
            ["inbox/edited-pdf.md", "inbox/deleted-pdf.md"],
            "{:?}",
            completion.warnings
        );
        // Ours: the pending commit and the one card nobody touched.
        let ours: Vec<_> = world
            .versions
            .written()?
            .into_iter()
            .filter(|(changes, _)| changes.author.subject == "owner")
            .map(|(changes, _)| changes.mutation_id)
            .collect();
        assert_eq!(
            ours,
            [0, 3].map(|ordinal| derive_commit_mutation_id(mutation, ordinal))
        );
        // The person's edit and deletion stand at the head.
        let head = some(
            world
                .versions
                .written()?
                .last()
                .map(|(_, revision)| revision.clone()),
            "the head",
        )?;
        let kept = some(world.versions.item_at(&head, edited_id)?, "the edited card")?;
        assert_eq!(kept.properties.get("reviewed"), Some(&json!(true)));
        assert_eq!(
            kept.appearance.extraction.outcome,
            ConversionOutcome::Pending
        );
        assert!(world.versions.item_at(&head, deleted_id)?.is_none());
        let converted = some(
            world.versions.item_at(&head, derive_item_id(mutation, 2))?,
            "the untouched card",
        )?;
        assert_eq!(
            converted.appearance.extraction.outcome,
            ConversionOutcome::Completed
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
        // The failure it wrote is announced.
        let last = some(world.events.appended()?.last().cloned(), "the last event")?;
        assert_eq!(
            (last.2.kind, last.2.job_id),
            (EventKind::JobUpdated, Some(id))
        );
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
    async fn a_job_cancelled_while_its_pages_are_counted_stops_and_renews_its_lease() -> TestResult
    {
        let world = world_full(
            Some(6),
            Vec::new(),
            Arc::new(AcceptAll),
            Duration::from_millis(10),
        )?;
        let report = upload(&world, 107, "report.pdf", b"%PDF report")?;
        let id = job(
            &world,
            22,
            workspace_scope()?,
            import_spec(vec![report], false)?,
        )?;
        let records = Arc::clone(&world.records);
        let counting = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&counting);
        // The page count never ends by itself; the heartbeat must renew the lease while it
        // runs, and find the job cancelled.
        *world
            .converter
            .on_page_count
            .lock()
            .map_err(|_| "poisoned")? = Some(Box::new(move || {
            let _previous = seen.fetch_add(1, Ordering::SeqCst);
            let _cancelled = records.cancel(id);
            true
        }));
        // An `Elapsed` error here means nothing renewed the lease during the count.
        assert!(tokio::time::timeout(Duration::from_secs(20), run(&world, id)).await??);
        assert_stopped(&world, id)?;
        assert_eq!(counting.load(Ordering::SeqCst), 1);
        assert_eq!(world.converter.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(windows(&world)?, Vec::<Option<PageRange>>::new());
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
    async fn a_page_count_that_reached_its_time_bound_fails_the_document_not_the_job() -> TestResult
    {
        let world = world_with(Some(6), Vec::new())?;
        *world
            .converter
            .count_failed
            .lock()
            .map_err(|_| "poisoned")? = Some(FailureReason::TimeLimit { limit_seconds: 120 });
        let report = upload(&world, 113, "report.pdf", b"%PDF slow report")?;
        let id = job(
            &world,
            24,
            workspace_scope()?,
            import_spec(vec![report], false)?,
        )?;
        assert!(run(&world, id).await?);
        // The job completes; it is not a worker fault to retry.
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        assert_eq!(entry.failures, Vec::new());
        // The pending card is replaced with the document's failure, and nothing is converted.
        let cards = cards(&world)?;
        assert_eq!(cards.len(), 2);
        let card = some(cards.last(), "the converted card")?;
        let ConversionOutcome::Failed { reason, issues } = &card.appearance.extraction.outcome
        else {
            return Err(format!(
                "expected a failed outcome, got {:?}",
                card.appearance.extraction.outcome
            )
            .into());
        };
        assert_eq!(*reason, FailureReason::TimeLimit { limit_seconds: 120 });
        assert_ne!(*issues, Vec::new());
        assert_eq!(card.appearance.extraction.page_count, None);
        assert_eq!(card.appearance.extraction.text_origin, TextOrigin::None);
        // The count's failure is the card's conversion: no second child converts anything.
        assert_eq!(windows(&world)?, Vec::<Option<PageRange>>::new());
        Ok(())
    }

    #[tokio::test]
    async fn a_partial_redigest_is_refused_until_core_has_its_inputs() -> TestResult {
        let world = world()?;
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

    /// The baseline naming rule R0 of the design (kebab case joined by `-`) with `collision`.
    fn naming_rules(collision: CollisionPolicy) -> NamingRules {
        NamingRules {
            schema_version: 1,
            rules: vec![NamingRule {
                glob: "**".to_owned(),
                type_name: None,
                case: LetterCase::Kebab,
                separator: "-".to_owned(),
                prefix: String::new(),
                suffix: None,
                date_order: DateOrder::Ymd,
                date_format: String::new(),
                strip_version_suffix: false,
                extension_policy: ExtensionPolicy::Preserve,
                alias_preservation: false,
                destination: None,
                collision,
            }],
        }
    }

    /// Save `parsed` as the rules file at every revision of the world's store.
    fn save_rules(world: &World, parsed: Result<NamingRules, RulesParseError>) -> Built<()> {
        world.versions.put_rules(RulesFile {
            text: "schema_version: 1\n".to_owned(),
            parsed,
        })?;
        Ok(())
    }

    /// A source card numbered `number` at `at` in the base, of an original named `original`.
    fn card_at_base(world: &World, number: u128, at: &str, original: &str) -> Built<()> {
        let object = world.blobs.insert(original.as_bytes())?;
        world
            .versions
            .at_base
            .lock()
            .map_err(|_| "poisoned")?
            .push(SourceCard {
                item_id: serde_json::from_value::<ItemId>(json!(format!(
                    "00000000-0000-0000-0000-{number:012}"
                )))?,
                path: WorkspacePath::try_from(at.to_owned())?,
                title: original.to_owned(),
                type_name: "source".to_owned(),
                body: String::new(),
                properties: std::collections::BTreeMap::new(),
                appearance: serde_json::from_value(json!({
                    "object": object.digest.as_str(),
                    "names": [{ "filename": original, "folder": "", "observed_at": "2026-10-09T00:00:00.000Z", "supplied_by": "owner" }],
                    "media_type": "application/pdf",
                    "size": object.size.to_string(),
                    "metadata": {},
                    "extraction": {
                        "outcome": { "status": "unsupported" },
                        "text_origin": "none",
                        "corrected": false,
                        "warnings": []
                    }
                }))?,
            });
        Ok(())
    }

    /// Run an import of `files` with the naming rules applied, and answer the paths of its
    /// pending cards and the codes and locations of its completion's warnings.
    async fn import_named(
        world: &World,
        number: u128,
        files: &[&str],
    ) -> Built<(Vec<String>, Vec<(String, Option<String>)>)> {
        let mut uploads = Vec::new();
        for (offset, name) in (0_u128..).zip(files) {
            uploads.push(upload(
                world,
                number.saturating_mul(10).saturating_add(offset),
                name,
                b"%PDF named",
            )?);
        }
        let id = job(
            world,
            number,
            workspace_scope()?,
            import_spec(uploads, true)?,
        )?;
        assert!(run(world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let paths = cards(world)?
            .iter()
            .take(files.len())
            .map(|card| card.path.as_str().to_owned())
            .collect();
        let warnings = some(entry.completions.first(), "the completion")?
            .warnings
            .iter()
            .filter(|warning| !warning.code.starts_with("okf"))
            .map(|warning| (warning.code.clone(), warning.location.clone()))
            .collect();
        Ok((paths, warnings))
    }

    #[tokio::test]
    async fn conventions_import_im1_with_no_rules_saved_names_without_them_and_says_so()
    -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        let (paths, warnings) = import_named(&world, 201, &["Q3 Report.PDF"]).await?;
        assert_eq!(paths, ["inbox/Q3 Report-PDF.md"]);
        assert_eq!(warnings, [(NAMING_RULES_ABSENT.to_owned(), None)]);
        Ok(())
    }

    /// SPEC section 6: selecting the fallback must not consume uploads before discovering
    /// that the destination is a file. Both absent rules and disabled rules use this path.
    async fn refuses_unruled_item_destination(apply_naming_rules: bool) -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        card_at_base(&world, 90, "archive.md", "archive.pdf")?;
        let report = upload(&world, 2071, "report.pdf", b"%PDF")?;
        let id = job(
            &world,
            207,
            workspace_scope()?,
            JobSpec::Import {
                base_revision: base()?,
                upload_ids: vec![report],
                destination: Some(WorkspacePath::try_from("archive.md".to_owned())?),
                apply_naming_rules,
                settings: ConversionSettings::default(),
            },
        )?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        let error = some(entry.job.error.as_ref(), "the destination refusal")?;
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("/destination"));
        assert_eq!(world.uploads.consumed()?, []);
        assert!(world.versions.written()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn unruled_item_destination_is_refused_when_rules_are_absent() -> TestResult {
        refuses_unruled_item_destination(true).await
    }

    #[tokio::test]
    async fn unruled_item_destination_is_refused_when_rules_are_disabled() -> TestResult {
        refuses_unruled_item_destination(false).await
    }

    #[tokio::test]
    async fn conventions_import_im2_im3_cards_are_named_by_the_rules_and_by_each_other()
    -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        save_rules(&world, Ok(naming_rules(CollisionPolicy::Suffix)))?;
        let (paths, warnings) =
            import_named(&world, 202, &["Q3 Report.PDF", "a.pdf", "A.pdf"]).await?;
        assert_eq!(
            paths,
            [
                "inbox/q3-report-PDF.md",
                "inbox/a-pdf.md",
                "inbox/a-pdf-2.md"
            ]
        );
        assert_eq!(
            warnings,
            [(
                COLLISION_SUFFIXED.to_owned(),
                Some("inbox/a-pdf-2.md".to_owned())
            )]
        );
        Ok(())
    }

    #[tokio::test]
    async fn conventions_import_im4_im7_names_taken_at_the_base_are_numbered_or_kept() -> TestResult
    {
        // `suffix` numbers the ruled name; `keep_existing` falls back to the name without rules.
        for (collision, named, warned) in [
            (
                CollisionPolicy::Suffix,
                "inbox/q3-report-pdf-2.md",
                COLLISION_SUFFIXED,
            ),
            (
                CollisionPolicy::KeepExisting,
                "inbox/Q3 Report-pdf.md",
                COLLISION_KEPT,
            ),
        ] {
            let world = world_with(Some(3), Vec::new())?;
            card_at_base(&world, 90, "inbox/q3-report-pdf.md", "q3 report.pdf")?;
            save_rules(&world, Ok(naming_rules(collision)))?;
            let (paths, warnings) = import_named(&world, 203, &["Q3 Report.pdf"]).await?;
            assert_eq!(paths, [named]);
            assert_eq!(warnings, [(warned.to_owned(), Some(named.to_owned()))]);
        }
        Ok(())
    }

    /// SPEC section 6: an unusable ruled folder falls back before consumption, so an item
    /// called archive.md never becomes a directory. These literal paths are the import's
    /// ordinary unruled card names, not answers computed by the naming engine under test.
    #[tokio::test]
    async fn conventions_import_an_item_ancestor_falls_back_before_the_upload_is_consumed()
    -> TestResult {
        for collision in [
            CollisionPolicy::Suffix,
            CollisionPolicy::KeepExisting,
            CollisionPolicy::Reject,
        ] {
            let world = world_with(Some(3), Vec::new())?;
            card_at_base(&world, 90, "archive.md", "archive.pdf")?;
            let mut rules = naming_rules(collision);
            let rule = some(rules.rules.first_mut(), "the naming rule")?;
            rule.destination = Some("ARCHIVE.md/nested".to_owned());
            save_rules(&world, Ok(rules))?;
            let (paths, warnings) = import_named(&world, 205, &["report.pdf"]).await?;
            assert_eq!(paths, ["inbox/report-pdf.md"]);
            assert_eq!(
                warnings,
                [(
                    PATH_NOT_VALID.to_owned(),
                    Some("inbox/report-pdf.md".to_owned())
                )]
            );
            assert_eq!(world.uploads.consumed()?.len(), 1);
            assert!(
                cards(&world)?
                    .iter()
                    .all(|card| { card.path.as_str() == "inbox/report-pdf.md" })
            );
        }
        Ok(())
    }

    /// A folder may itself end in .md. SPEC section 3 requires its stored spelling; the item
    /// ancestor refusal must distinguish this real folder from an item of the same name.
    #[tokio::test]
    async fn conventions_import_an_existing_folder_is_allowed_with_its_stored_spelling()
    -> TestResult {
        let world = world_with(Some(3), Vec::new())?;
        card_at_base(&world, 90, "Archive.md/kept.md", "kept.pdf")?;
        let mut rules = naming_rules(CollisionPolicy::Suffix);
        let rule = some(rules.rules.first_mut(), "the naming rule")?;
        rule.destination = Some("archive.MD".to_owned());
        save_rules(&world, Ok(rules))?;
        let (paths, warnings) = import_named(&world, 206, &["report.pdf"]).await?;
        assert_eq!(paths, ["Archive.md/report-pdf.md"]);
        assert_eq!(warnings, Vec::new());
        assert_eq!(world.uploads.consumed()?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn conventions_import_im6_rules_that_cannot_be_applied_fail_the_job_for_good()
    -> TestResult {
        let unreadable = Err(RulesParseError {
            message: "unknown field `colour`".to_owned(),
            line: Some(2),
            column: Some(1),
        });
        let mut refused = naming_rules(CollisionPolicy::Suffix);
        if let Some(rule) = refused.rules.first_mut() {
            rule.glob = "notes/**x".to_owned();
        }
        for parsed in [unreadable, Ok(refused)] {
            let world = world_with(Some(3), Vec::new())?;
            save_rules(&world, parsed)?;
            let report = upload(&world, 2041, "report.pdf", b"%PDF")?;
            let id = job(
                &world,
                204,
                workspace_scope()?,
                import_spec(vec![report], true)?,
            )?;
            assert!(run(&world, id).await?);
            let entry = world.records.entry(id)?;
            assert_eq!(entry.job.state, JobState::Failed);
            let (message, retryable) = some(entry.failures.first(), "the failure")?;
            assert!(message.contains("naming rules"), "{message}");
            assert!(!retryable);
            let error = some(entry.job.error.as_ref(), "the job's error")?;
            assert_eq!(error.code, ErrorCode::InvalidInput);
            assert_eq!(error.field.as_deref(), Some("/apply_naming_rules"));
            assert!(world.versions.written()?.is_empty());
            assert!(
                world
                    .uploads
                    .consumed
                    .lock()
                    .map_err(|_| "poisoned")?
                    .is_empty()
            );
        }
        Ok(())
    }

    /// A world whose base holds one converted source card at `inbox/report-pdf.md`, with a
    /// property of its own; and a redigest of it against the base.
    fn redigest_world(number: u128) -> Built<(World, okf_jawn_contract::identity::JobId, ItemId)> {
        redigest_world_with(number, Arc::new(AcceptAll))
    }

    /// `redigest_world`, with `check` as the conformance check composition injects.
    fn redigest_world_with(
        number: u128,
        check: Arc<dyn CandidateCheck>,
    ) -> Built<(World, okf_jawn_contract::identity::JobId, ItemId)> {
        let world = world_full(Some(3), Vec::new(), check, Duration::from_secs(60))?;
        let object = world.blobs.insert(b"%PDF report")?;
        let item_id = serde_json::from_value::<ItemId>(json!(SOURCE_ITEM))?;
        let mut properties = std::collections::BTreeMap::new();
        let _previous = properties.insert("topic".to_owned(), json!("budget"));
        let card = SourceCard {
            item_id,
            path: WorkspacePath::try_from("inbox/report-pdf.md".to_owned())?,
            title: "Report".to_owned(),
            type_name: "source".to_owned(),
            body: "old text".to_owned(),
            properties,
            appearance: serde_json::from_value(json!({
                "object": object.digest.as_str(),
                "names": [{ "filename": "report.pdf", "folder": "", "observed_at": "2026-10-09T00:00:00.000Z", "supplied_by": "owner" }],
                "media_type": "application/pdf",
                "size": "11",
                "metadata": {},
                "extraction": {
                    "outcome": { "status": "pending" },
                    "page_count": null,
                    "text_origin": "none",
                    "corrected": false,
                    "warnings": [],
                },
            }))?,
        };
        world
            .versions
            .at_base
            .lock()
            .map_err(|_| "poisoned")?
            .push(card);
        let id = job(
            &world,
            number,
            workspace_scope()?,
            JobSpec::Redigest {
                item_id,
                base_revision: base()?,
                settings: ConversionSettings::default(),
                pages: None,
            },
        )?;
        Ok((world, id, item_id))
    }

    /// Another item at the base, at `notes/other.md`: a file of the workspace a redigest of the
    /// report does not write.
    fn other_file_at_base(world: &World) -> TestResult {
        let mut other = some(
            world.versions.item_at(
                &base()?,
                serde_json::from_value::<ItemId>(json!(SOURCE_ITEM))?,
            )?,
            "the card",
        )?;
        other.item_id = serde_json::from_value::<ItemId>(json!(OTHER_ITEM))?;
        other.path = WorkspacePath::try_from("notes/other.md".to_owned())?;
        "Other".clone_into(&mut other.title);
        world
            .versions
            .at_base
            .lock()
            .map_err(|_| "poisoned")?
            .push(other);
        Ok(())
    }

    /// The warnings a redigest completed with, and the ones the store kept for its commit.
    fn redigest_warnings(
        world: &World,
        id: okf_jawn_contract::identity::JobId,
    ) -> Built<(Vec<Warning>, Vec<Warning>)> {
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let completed = some(entry.completions.first(), "the completion")?
            .warnings
            .clone();
        let commit = derive_commit_mutation_id(entry.created.mutation_id, 0);
        let kept = world
            .versions
            .warnings
            .lock()
            .map_err(|_| "poisoned")?
            .clone();
        let row = some(kept.iter().find(|(of, _)| *of == commit), "the kept row")?;
        Ok((completed, row.1.clone()))
    }

    /// A redigest's one commit writes the card (and the card's appearance record, the folder
    /// indexes and the log, which are no concept), so its check reports that card: in a workspace
    /// whose other files hold findings, the redigest completes with the card's findings only, and
    /// the store keeps the same for its commit.
    #[tokio::test]
    async fn a_redigest_completes_and_keeps_only_the_findings_of_the_card_it_wrote() -> TestResult {
        let (world, id, _item_id) = redigest_world_with(40, Arc::new(WholeCandidate))?;
        other_file_at_base(&world)?;
        assert!(run(&world, id).await?);
        let (completed, kept) = redigest_warnings(&world, id)?;
        assert_eq!(
            located(&completed, WHOLE_CANDIDATE),
            ["inbox/report-pdf.md"],
            "{completed:?}"
        );
        assert!(
            completed
                .iter()
                .all(|warning| warning.location.as_deref() == Some("inbox/report-pdf.md")),
            "{completed:?}"
        );
        assert_eq!(compared(&kept), compared(&completed));
        Ok(())
    }

    /// The card is written at the head's path, so a redigest of a card a person moved reports the
    /// findings of the card at its new path, and of no other file.
    #[tokio::test]
    async fn a_redigest_of_a_moved_card_reports_the_findings_at_its_new_path() -> TestResult {
        let (world, id, item_id) = redigest_world_with(41, Arc::new(WholeCandidate))?;
        other_file_at_base(&world)?;
        world
            .versions
            .edit_elsewhere_with(vec![TreeEdit::MoveItem {
                item_id,
                destination: WorkspacePath::try_from("archive/report.md".to_owned())?,
                keep_alias: false,
            }])?;
        assert!(run(&world, id).await?);
        let (completed, kept) = redigest_warnings(&world, id)?;
        assert_eq!(
            located(&completed, WHOLE_CANDIDATE),
            ["archive/report.md"],
            "{completed:?}"
        );
        assert!(
            completed
                .iter()
                .all(|warning| warning.location.as_deref() == Some("archive/report.md")),
            "{completed:?}"
        );
        assert_eq!(compared(&kept), compared(&completed));
        Ok(())
    }

    /// A refusal of the injected conformance check still refuses a redigest for good, and nothing
    /// of it is written.
    #[tokio::test]
    async fn a_redigest_the_conformance_check_refuses_is_refused_for_good() -> TestResult {
        let (world, id, _item_id) = redigest_world_with(42, Arc::new(RefuseAll))?;
        other_file_at_base(&world)?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        assert!(entry.completions.is_empty());
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(message.contains("not a conformant"), "{message}");
        assert!(!retryable);
        assert!(world.versions.written()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn a_redigest_lands_over_an_unrelated_edit_and_runs_again_without_a_second_commit()
    -> TestResult {
        let (world, id, item_id) = redigest_world(30)?;
        world.versions.edit_elsewhere(None)?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let commits = world.versions.written()?;
        let (ours, made) = some(commits.last(), "the redigest commit")?;
        assert_eq!(
            ours.mutation_id,
            derive_commit_mutation_id(entry.created.mutation_id, 0)
        );
        // Written on the head, after the unrelated edit, not on the base.
        assert_eq!(
            Some(&ours.expected_head),
            commits.first().map(|(_, revision)| revision)
        );
        let card = some(world.versions.item_at(made, item_id)?, "the card")?;
        assert_eq!(
            card.appearance.extraction.outcome,
            ConversionOutcome::Completed
        );
        assert_eq!(card.properties.get("topic"), Some(&json!("budget")));
        // The completion was lost: the same claim runs again and replays its commit.
        let before = commits.len();
        world
            .records
            .insert(id, entry.scope.clone(), entry.created.clone())?;
        assert!(run(&world, id).await?);
        assert_eq!(world.versions.written()?.len(), before);
        assert_eq!(world.records.entry(id)?.job.state, JobState::Succeeded);
        Ok(())
    }

    #[tokio::test]
    async fn a_redigest_of_a_moved_card_lands_at_its_new_path() -> TestResult {
        let (world, id, item_id) = redigest_world(31)?;
        let mut renamed = std::collections::BTreeMap::new();
        let _previous = renamed.insert("topic".to_owned(), json!("audit"));
        world.versions.edit_elsewhere_with(vec![
            TreeEdit::MoveItem {
                item_id,
                destination: WorkspacePath::try_from("archive/report.md".to_owned())?,
                keep_alias: false,
            },
            TreeEdit::EditItem {
                item_id,
                body: "a person's text".to_owned(),
                properties: renamed,
            },
        ])?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded, "{:?}", entry.failures);
        let commits = world.versions.written()?;
        let (ours, made) = some(commits.last(), "the redigest commit")?;
        let written = cards(&world)?;
        let card = some(written.last(), "the written card")?;
        // The head's path, title and properties are kept; the new extraction is written.
        assert_eq!(card.path.as_str(), "archive/report.md");
        assert_eq!(card.title, "Report");
        assert_eq!(card.properties.get("topic"), Some(&json!("audit")));
        assert_eq!(
            card.appearance.extraction.outcome,
            ConversionOutcome::Completed
        );
        assert_eq!(ours.author.subject, "owner");
        assert!(world.versions.item_at(made, item_id)?.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn a_redigest_whose_original_changed_since_its_base_is_refused_for_good() -> TestResult {
        let (world, id, item_id) = redigest_world(39)?;
        // Someone replaces the card's original after the redigest was accepted.
        let replacement = world.blobs.insert(b"%PDF a different report")?;
        let mut card = some(world.versions.item_at(&base()?, item_id)?, "the card")?;
        card.appearance.object = replacement.digest;
        world
            .versions
            .edit_elsewhere_with(vec![TreeEdit::WriteSourceCard(Box::new(card))])?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        assert!(entry.completions.is_empty());
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(message.contains("original"), "{message}");
        assert!(message.contains("changed"), "{message}");
        assert!(!retryable);
        // Nothing of the redigest was written: the new original stands.
        assert!(
            world
                .versions
                .written()?
                .iter()
                .all(|(changes, _)| changes.author.subject != "owner")
        );
        Ok(())
    }
    #[tokio::test]
    async fn a_redigest_of_a_deleted_card_is_refused_for_good() -> TestResult {
        let (world, id, item_id) = redigest_world(32)?;
        world
            .versions
            .edit_elsewhere_with(vec![TreeEdit::DeleteItem { item_id }])?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        assert!(entry.completions.is_empty());
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(message.contains("deleted"), "{message}");
        assert!(!retryable);
        // Nothing of the redigest was written: the deletion stands.
        assert!(
            world
                .versions
                .written()?
                .iter()
                .all(|(changes, _)| changes.author.subject != "owner")
        );
        let head = some(
            world
                .versions
                .written()?
                .last()
                .map(|(_, revision)| revision.clone()),
            "the head",
        )?;
        assert!(world.versions.item_at(&head, item_id)?.is_none());
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

    /// A world with a 10 ms heartbeat whose workspace purges wait at most `drain` for the
    /// workspace's jobs to stop.
    fn world_with_drain(drain: Duration) -> Built<World> {
        world_drained(
            Some(3),
            Vec::new(),
            Arc::new(AcceptAll),
            Duration::from_millis(10),
            drain,
        )
    }

    /// The workspace purge job of `world` numbered `number`, with its purge recorded as
    /// `requested`: the job's identity, its mutation and the purge's identity.
    fn workspace_purge(
        world: &World,
        number: u128,
    ) -> Built<(okf_jawn_contract::identity::JobId, MutationId, PurgeId)> {
        let purge_id = serde_json::from_value::<PurgeId>(json!(format!(
            "00000000-0000-0000-0000-{:012}",
            number.saturating_add(1000)
        )))?;
        let workspace_id = serde_json::from_value::<WorkspaceId>(json!(WORKSPACE))?;
        let id = job(
            world,
            number,
            JobScope::Tenant(tenant()?),
            JobSpec::PurgeWorkspace {
                purge_id,
                workspace_id,
            },
        )?;
        world
            .records
            .insert_purge(serde_json::from_value::<Purge>(json!({
                "id": purge_id,
                "target": { "kind": "workspace", "workspace_id": WORKSPACE },
                "state": "requested",
                "job_id": serde_json::to_value(id)?,
                "requested_by": "admin",
                "requested_at": "2026-10-09T00:00:00.000Z",
            }))?)?;
        Ok((id, world.records.entry(id)?.created.mutation_id, purge_id))
    }

    /// A job of the purged workspace, claimed now so that it runs.
    async fn running_job(world: &World, number: u128) -> Built<okf_jawn_contract::identity::JobId> {
        let id = job(world, number, workspace_scope()?, JobSpec::RebuildIndex)?;
        let _claimed = some(world.records.claim_job(id).await?, "the claim")?;
        Ok(id)
    }

    /// A restore of the purged workspace that runs and sealed itself past its point of no return.
    async fn sealed_restore(
        world: &World,
        number: u128,
    ) -> Built<okf_jawn_contract::identity::JobId> {
        let id = restore_job(world, number)?;
        let claimed = some(world.records.claim_job(id).await?, "the claim")?;
        assert_eq!(
            world.records.seal(&claimed.lease).await?.state,
            JobState::Running
        );
        Ok(id)
    }

    /// The state of each of `jobs` as the purger saw it when it was called, kept by its hold.
    fn record_states_at_removal(
        world: &World,
        jobs: Vec<okf_jawn_contract::identity::JobId>,
    ) -> Built<Arc<Mutex<Vec<JobState>>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let records = Arc::clone(&world.records);
        let kept = Arc::clone(&seen);
        *world.purger.hold.lock().map_err(|_| "poisoned")? = Some(Arc::new(move || {
            let records = Arc::clone(&records);
            let kept = Arc::clone(&kept);
            let jobs = jobs.clone();
            Box::pin(async move {
                for job in jobs {
                    if let (Ok(entry), Ok(mut kept)) = (records.entry(job), kept.lock()) {
                        kept.push(entry.job.state);
                    }
                }
            })
        }));
        Ok(seen)
    }

    /// P3: before the purger is called, the purge job cancels every queued or running job of
    /// the workspace, each under the identity derived from its own mutation and the job, and
    /// announces each cancellation; a finished job and another workspace's job are left alone.
    #[tokio::test]
    async fn a_workspace_purge_cancels_the_workspaces_unfinished_jobs_before_it_removes()
    -> TestResult {
        let world = heartbeat_world()?;
        let queued = job(&world, 300, workspace_scope()?, JobSpec::RebuildIndex)?;
        let running = running_job(&world, 301).await?;
        let finished = job(&world, 302, workspace_scope()?, JobSpec::RebuildIndex)?;
        world.records.finish(finished)?;
        let elsewhere = job(
            &world,
            303,
            JobScope::Workspace(StorageScope {
                tenant_id: tenant()?,
                workspace_id: serde_json::from_value(json!(
                    "00000000-0000-0000-0000-0000000000bb"
                ))?,
            }),
            JobSpec::RebuildIndex,
        )?;
        let seen = record_states_at_removal(&world, vec![queued, running])?;
        let (id, mutation, purge_id) = workspace_purge(&world, 304)?;
        assert!(run(&world, id).await?);
        assert_completed_under(&world, id, 1)?;
        assert_eq!(world.records.purge(purge_id)?.state, PurgeState::Completed);
        assert_eq!(
            seen.lock().map_err(|_| "poisoned")?.clone(),
            [JobState::Cancelled, JobState::Cancelled],
            "both were cancelled before the purger ran"
        );
        // Newest first, as the store lists them, each under its own derived identity.
        assert_eq!(
            world.records.cancels()?,
            [
                (derive_drain_mutation_id(mutation, running), running),
                (derive_drain_mutation_id(mutation, queued), queued),
            ]
        );
        assert_eq!(
            world.records.entry(finished)?.job.state,
            JobState::Succeeded
        );
        assert_eq!(world.records.entry(elsewhere)?.job.state, JobState::Queued);
        let workspace_log = EventScope::Workspace(storage_scope()?);
        let cancelled: Vec<_> = events_of(&world, &EventKind::JobUpdated)?
            .into_iter()
            .filter(|(log, _, _)| *log == workspace_log)
            .map(|(_, mutation, event)| (mutation, event.job_id))
            .collect();
        assert_eq!(
            cancelled,
            [
                (
                    Some(derive_drain_mutation_id(mutation, running)),
                    Some(running)
                ),
                (
                    Some(derive_drain_mutation_id(mutation, queued)),
                    Some(queued)
                ),
            ]
        );
        Ok(())
    }

    /// P3: the drain reads every page of the workspace's jobs.
    #[tokio::test]
    async fn a_workspace_purge_cancels_unfinished_jobs_on_every_page() -> TestResult {
        let world = world()?;
        world.records.page_jobs(1)?;
        let jobs = [
            job(&world, 310, workspace_scope()?, JobSpec::RebuildIndex)?,
            job(&world, 311, workspace_scope()?, JobSpec::RebuildIndex)?,
            job(&world, 312, workspace_scope()?, JobSpec::RebuildIndex)?,
        ];
        let (id, _, _) = workspace_purge(&world, 313)?;
        assert!(run(&world, id).await?);
        for job in jobs {
            assert_eq!(world.records.entry(job)?.job.state, JobState::Cancelled);
        }
        Ok(())
    }

    /// P3 and P4: a restore past its point of no return refuses its cancellation, and the drain
    /// waits for it to finish before the purger is called.
    #[tokio::test]
    async fn a_workspace_purge_waits_for_a_sealed_restore_to_finish() -> TestResult {
        let world = heartbeat_world()?;
        let restore = sealed_restore(&world, 320).await?;
        let seen = record_states_at_removal(&world, vec![restore])?;
        let (id, mutation, purge_id) = workspace_purge(&world, 321)?;
        let records = Arc::clone(&world.records);
        let finishing = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            records.finish(restore)
        });
        assert!(run(&world, id).await?);
        finishing.await??;
        assert_eq!(world.records.purge(purge_id)?.state, PurgeState::Completed);
        assert_eq!(
            seen.lock().map_err(|_| "poisoned")?.clone(),
            [JobState::Succeeded],
            "the purger ran once the restore had finished"
        );
        let cancel = derive_drain_mutation_id(mutation, restore);
        assert!(world.records.cancel_calls()?.contains(&(cancel, restore)));
        assert_eq!(
            world.records.cancels()?,
            Vec::new(),
            "its cancellation was refused"
        );
        assert!(
            world.records.renewals() >= 2,
            "the purge job kept its lease while it waited"
        );
        Ok(())
    }

    /// P3: a sealed restore whose attempt died and whose lease lapsed is queued again, and no
    /// worker can claim it while the purge is recorded: it is not running, so the drain does
    /// not wait for it.
    #[tokio::test]
    async fn a_workspace_purge_does_not_wait_for_a_sealed_restore_that_no_longer_runs() -> TestResult
    {
        // A bound short enough that waiting for the queued restore would fail the purge.
        let world = world_with_drain(Duration::from_secs(2))?;
        let restore = sealed_restore(&world, 330).await?;
        world.records.expire_running()?;
        assert_eq!(world.records.expire_leases().await?, 1);
        world.records.purge_workspace_of(restore)?;
        let (id, _, purge_id) = workspace_purge(&world, 331)?;
        assert!(run(&world, id).await?);
        assert_eq!(world.records.purge(purge_id)?.state, PurgeState::Completed);
        assert_eq!(world.records.entry(restore)?.job.state, JobState::Queued);
        Ok(())
    }

    /// P3 and P5: past the drain's bound the purge job fails, retryable, with the purge recorded
    /// failed and the purger never called; the run that the repeated request starts drains
    /// again, and completes once the restore has finished.
    #[tokio::test]
    async fn a_drain_past_its_bound_fails_the_purge_and_the_next_run_drains_again() -> TestResult {
        let world = world_with_drain(Duration::from_millis(60))?;
        let restore = sealed_restore(&world, 340).await?;
        let (id, _, purge_id) = workspace_purge(&world, 341)?;
        let started = tokio::time::Instant::now();
        assert!(run(&world, id).await?);
        // The bound from above: the attempt gives up soon after it, not long after.
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(*retryable, "a drain past its bound is retried");
        assert!(message.contains(&restore.0.to_string()), "{message}");
        let failed = world.records.purge(purge_id)?;
        assert_eq!(failed.state, PurgeState::Failed);
        assert_eq!(
            failed.error.as_ref().map(|error| &error.code),
            Some(&ErrorCode::Unavailable)
        );
        assert_eq!(world.purger.calls.load(Ordering::SeqCst), 0);
        // The restore finishes; the purge is asked for again and its job runs again.
        world.records.finish(restore)?;
        let entry = world.records.entry(id)?;
        world.records.insert(id, entry.scope, entry.created)?;
        assert!(run(&world, id).await?);
        let completed = world.records.purge(purge_id)?;
        assert_eq!(completed.state, PurgeState::Completed);
        assert!(completed.error.is_none());
        assert_eq!(world.purger.calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    /// P3: a resumed purge drains again and finds nothing to do: every job it cancelled stays
    /// cancelled, no cancellation is asked for twice and no notification is appended twice.
    #[tokio::test]
    async fn a_resumed_purge_drains_again_and_finds_nothing_to_do() -> TestResult {
        let world = world()?;
        let queued = job(&world, 350, workspace_scope()?, JobSpec::RebuildIndex)?;
        world.purger.fail_next(ApiError::new(
            ErrorCode::Internal,
            "a survivor's record does not read",
        ))?;
        let (id, _, purge_id) = workspace_purge(&world, 351)?;
        assert!(run(&world, id).await?);
        assert_eq!(world.records.purge(purge_id)?.state, PurgeState::Failed);
        let calls = world.records.cancel_calls()?;
        let workspace_log = EventScope::Workspace(storage_scope()?);
        let announced = events_of(&world, &EventKind::JobUpdated)?
            .into_iter()
            .filter(|(log, _, _)| *log == workspace_log)
            .count();
        let entry = world.records.entry(id)?;
        world.records.insert(id, entry.scope, entry.created)?;
        assert!(run(&world, id).await?);
        assert_eq!(world.records.purge(purge_id)?.state, PurgeState::Completed);
        assert_eq!(world.records.entry(queued)?.job.state, JobState::Cancelled);
        assert_eq!(
            world.records.cancel_calls()?,
            calls,
            "nothing left to cancel"
        );
        assert_eq!(
            events_of(&world, &EventKind::JobUpdated)?
                .into_iter()
                .filter(|(log, _, _)| *log == workspace_log)
                .count(),
            announced
        );
        Ok(())
    }

    /// P5: a purge whose removal fails is recorded failed with the purger's error before the
    /// job's failure is written; the job's failure holds the same error.
    #[tokio::test]
    async fn a_purge_whose_removal_fails_is_recorded_failed_with_its_error() -> TestResult {
        let world = world()?;
        let refusal = ApiError::new(ErrorCode::Internal, "a survivor's record does not read");
        world.purger.fail_next(refusal.clone())?;
        let (id, _, purge_id) = workspace_purge(&world, 360)?;
        assert!(run(&world, id).await?);
        let failed = world.records.purge(purge_id)?;
        assert_eq!(failed.state, PurgeState::Failed);
        assert_eq!(
            failed
                .error
                .as_ref()
                .map(|error| (&error.code, error.message.as_str())),
            Some((&refusal.code, refusal.message.as_str()))
        );
        assert_eq!(failed.report, None);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        assert_eq!(
            entry.job.error.as_ref().map(|error| error.message.clone()),
            Some(refusal.message)
        );
        Ok(())
    }

    /// P5: a purge whose removal ran but whose completion could not be recorded is recorded
    /// failed with that error and no report: a report is present exactly when the purge
    /// completed.
    #[tokio::test]
    async fn a_purge_whose_completion_cannot_be_recorded_is_recorded_failed_without_a_report()
    -> TestResult {
        let world = world()?;
        world.records.fail_purge_update(PurgeState::Completed)?;
        let (id, _, purge_id) = workspace_purge(&world, 375)?;
        assert!(run(&world, id).await?);
        let failed = world.records.purge(purge_id)?;
        assert_eq!(failed.state, PurgeState::Failed);
        assert_eq!(failed.report, None);
        assert_eq!(
            failed.error.as_ref().map(|error| &error.code),
            Some(&ErrorCode::Unavailable)
        );
        Ok(())
    }

    /// P5: when the failed state cannot be recorded either, the job still fails with the
    /// purger's error, and its message says the purge was not recorded failed.
    #[tokio::test]
    async fn a_purge_whose_failure_cannot_be_recorded_still_fails_with_its_error() -> TestResult {
        let world = world()?;
        world
            .purger
            .fail_next(ApiError::new(ErrorCode::Internal, "the removal broke"))?;
        world.records.fail_purge_update(PurgeState::Failed)?;
        let (id, _, purge_id) = workspace_purge(&world, 370)?;
        assert!(run(&world, id).await?);
        assert_eq!(world.records.purge(purge_id)?.state, PurgeState::Running);
        let entry = world.records.entry(id)?;
        let error = some(entry.job.error.as_ref(), "the job's failure")?;
        assert_eq!(error.code, ErrorCode::Internal, "the purger's code");
        assert!(
            error.message.starts_with("the removal broke")
                && error
                    .message
                    .contains("recording the purge as failed failed too"),
            "{}",
            error.message
        );
        Ok(())
    }

    /// Core M1.4b fix round 1, ruling F1 (contract rulings 14 and 24): whatever stops a
    /// workspace purge is a state to repair, so the handler writes the job's failure as
    /// retryable whatever its code, each code the runtime alone holds not retryable included,
    /// keeping the code and the message that names what failed. The purge is recorded `Failed`
    /// with the same error, and the failure is announced once.
    #[tokio::test]
    async fn a_workspace_purge_that_fails_is_retryable_whatever_its_code() -> TestResult {
        let codes = [
            ErrorCode::InvalidInput,
            ErrorCode::Forbidden,
            ErrorCode::Unsupported,
            ErrorCode::NotImplemented,
        ];
        let kept = |error: Option<&ApiError>| {
            error.map(|error| (error.code.clone(), error.message.clone()))
        };
        for (number, code) in (390_u128..).zip(codes) {
            let world = world()?;
            let refusal = ApiError::new(
                code.clone(),
                "the purge cannot read workspace bb's archive in pre-migration backup set set-1",
            );
            world.purger.fail_next(refusal.clone())?;
            let (id, _, purge_id) = workspace_purge(&world, number)?;
            assert!(run(&world, id).await?);
            let entry = world.records.entry(id)?;
            assert_eq!(entry.job.state, JobState::Failed, "{code:?}");
            assert!(entry.completions.is_empty(), "{code:?}");
            assert_eq!(
                entry.failures,
                [(refusal.message.clone(), true)],
                "{code:?}: retryable, its message kept"
            );
            assert_eq!(
                kept(entry.job.error.as_ref()),
                Some((code.clone(), refusal.message.clone())),
                "the code is kept"
            );
            let failed = world.records.purge(purge_id)?;
            assert_eq!(failed.state, PurgeState::Failed, "{code:?}");
            assert_eq!(
                kept(failed.error.as_ref()),
                kept(entry.job.error.as_ref()),
                "{code:?}: the purge keeps the job's error"
            );
            assert_eq!(
                events_of(&world, &EventKind::JobUpdated)?.len(),
                1,
                "{code:?}"
            );
        }
        Ok(())
    }

    /// An item purge drains nothing until storage's item purge is built (its purger answers
    /// `NotImplemented` today): no job of the workspace is cancelled.
    #[tokio::test]
    async fn an_item_purge_drains_nothing() -> TestResult {
        let world = world()?;
        let queued = job(&world, 380, workspace_scope()?, JobSpec::RebuildIndex)?;
        let purge_id =
            serde_json::from_value::<PurgeId>(json!("00000000-0000-0000-0000-0000000003e9"))?;
        let item = "00000000-0000-0000-0000-0000000000dd";
        let id = job(
            &world,
            381,
            JobScope::Tenant(tenant()?),
            JobSpec::PurgeItem {
                purge_id,
                workspace_id: serde_json::from_value(json!(WORKSPACE))?,
                item_id: serde_json::from_value(json!(item))?,
            },
        )?;
        world
            .records
            .insert_purge(serde_json::from_value::<Purge>(json!({
                "id": purge_id,
                "target": { "kind": "item", "workspace_id": WORKSPACE, "item_id": item },
                "state": "requested",
                "job_id": serde_json::to_value(id)?,
                "requested_by": "admin",
                "requested_at": "2026-10-09T00:00:00.000Z",
            }))?)?;
        assert!(run(&world, id).await?);
        assert_eq!(world.records.cancel_calls()?, Vec::new());
        assert_eq!(world.records.entry(queued)?.job.state, JobState::Queued);
        Ok(())
    }

    /// Ruling F1 is a workspace purge's: an item purge's failure is still the runtime's to
    /// class, and storage's `NotImplemented` (item purge is not built) is not retryable, since
    /// no repair changes it.
    #[tokio::test]
    async fn an_item_purge_that_is_not_built_fails_not_retryable() -> TestResult {
        let world = world()?;
        world.purger.fail_next(ApiError::new(
            ErrorCode::NotImplemented,
            "item purge is not built yet",
        ))?;
        let purge_id =
            serde_json::from_value::<PurgeId>(json!("00000000-0000-0000-0000-0000000003ea"))?;
        let item = "00000000-0000-0000-0000-0000000000dd";
        let id = job(
            &world,
            382,
            JobScope::Tenant(tenant()?),
            JobSpec::PurgeItem {
                purge_id,
                workspace_id: serde_json::from_value(json!(WORKSPACE))?,
                item_id: serde_json::from_value(json!(item))?,
            },
        )?;
        world
            .records
            .insert_purge(serde_json::from_value::<Purge>(json!({
                "id": purge_id,
                "target": { "kind": "item", "workspace_id": WORKSPACE, "item_id": item },
                "state": "requested",
                "job_id": serde_json::to_value(id)?,
                "requested_by": "admin",
                "requested_at": "2026-10-09T00:00:00.000Z",
            }))?)?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        let (message, retryable) = some(entry.failures.first(), "the failure")?;
        assert!(!retryable, "{message}");
        assert_eq!(world.records.purge(purge_id)?.state, PurgeState::Failed);
        Ok(())
    }
}
