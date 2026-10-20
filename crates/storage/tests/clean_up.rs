//! Real durable cleanup requests/passes, lifecycle design 3.3/V3.1/V3.3.
//! Expected states and counts below are literal design answers, not sweep-derived oracles.
#![cfg(feature = "runtime")]

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::identity::{Digest, MutationId, TenantId};
use okf_jawn_contract::import::{CleanUpReport, Job, JobState};
use okf_jawn_core::jobs::{JobCompletion, JobLease, JobScope, JobSpec, NewJob, RecordStore};
use okf_jawn_core::storage::{
    BlobStore, NewWorkspace, Provenance, Purger, StorageScope, WorkspaceCatalog,
};
use okf_jawn_storage::Storage;
use rusqlite::{Connection, params};

use check::{TestResult, some};

/// Each test owns one real data directory, workspace and tenant job namespace.
struct World {
    storage: Storage,
    workspace: StorageScope,
    jobs: JobScope,
    // Drop the storage/its writer lock before the disposable directory.
    _directory: tempfile::TempDir,
}

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

impl World {
    async fn new() -> Fallible<Self> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(directory.path())?;
        let tenant = TenantId::try_from("local".to_owned())?;
        let created = storage
            .catalog()
            .create(
                &tenant,
                mutation(1),
                NewWorkspace {
                    name: "Notes".to_owned(),
                    description: String::new(),
                    creator: owner(),
                },
            )
            .await?;
        Ok(Self {
            _directory: directory,
            storage,
            workspace: StorageScope {
                tenant_id: tenant.clone(),
                workspace_id: created.id,
            },
            jobs: JobScope::Tenant(tenant),
        })
    }

    async fn request(&self, number: u128) -> Fallible<Job> {
        Ok(self
            .storage
            .records()
            .create_job(
                &self.jobs,
                NewJob {
                    mutation_id: mutation(number),
                    initiator: owner(),
                    spec: JobSpec::CleanUp,
                },
            )
            .await?)
    }

    /// Legitimate legacy unreferenced bytes: retain through the blob store, then remove exactly its
    /// one pending root. This fixture isolates cleanup from the discard operation.
    async fn orphan(&self, bytes: &[u8]) -> Fallible<Digest> {
        let object = self
            .storage
            .blobs()
            .put(
                &self.workspace,
                Box::pin(std::io::Cursor::new(bytes.to_vec())),
                1024,
                None,
            )
            .await?;
        let removed = Connection::open(self.storage.data().records_path())?.execute(
            "DELETE FROM pending_puts WHERE tenant_id='local' AND workspace_id=?1 AND digest=?2",
            params![
                self.workspace.workspace_id.0.to_string(),
                object.digest.as_str()
            ],
        )?;
        assert_eq!(removed, 1);
        Ok(object.digest)
    }

    fn requests(&self) -> Fallible<(i64, i64)> {
        Ok(
            Connection::open(self.storage.data().records_path())?.query_row(
                "SELECT requested, started FROM clean_up_requests WHERE tenant_id='local'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?,
        )
    }
}

fn mutation(number: u128) -> MutationId {
    MutationId(uuid::Uuid::from_u128(number))
}

fn owner() -> Provenance {
    Provenance {
        subject: "ana".to_owned(),
        route: AccessRoute::LocalOwner,
        client_id: None,
    }
}

fn completion(lease: JobLease, report: CleanUpReport) -> JobCompletion {
    JobCompletion {
        lease,
        revision: None,
        item_ids: Vec::new(),
        artifact: None,
        restore: None,
        backup: None,
        clean_up: Some(report),
        outputs: Vec::new(),
        warnings: Vec::new(),
    }
}

/// V3.1: request mutation replay precedes increment, while another request is absorbed.
#[tokio::test]
async fn queued_cleanup_absorbs_another_request_but_replay_does_not_increment() -> TestResult {
    let world = World::new().await?;
    let first = world.request(10).await?;
    let absorbed = world.request(11).await?;
    assert_eq!(absorbed.id, first.id);
    assert_eq!(absorbed.state, JobState::Queued);
    assert_eq!(world.requests()?, (2, 0));
    assert_eq!(world.request(10).await?.id, first.id);
    assert_eq!(world.requests()?, (2, 0));
    let rows: i64 = Connection::open(world.storage.data().records_path())?.query_row(
        "SELECT count(*) FROM jobs WHERE json_extract(spec, '$.kind')='clean_up'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(rows, 1);
    Ok(())
}

/// V3.3: a request after a real pass but before completion queues the same job again.
/// Two actual CAS removals across its two passes sum to2, with the final kept count0.
#[tokio::test]
async fn request_after_a_pass_requeues_the_job_and_accumulates_real_removals() -> TestResult {
    let world = World::new().await?;
    let first_object = world.orphan(b"first discarded occurrence").await?;
    let job = world.request(10).await?;
    let records = world.storage.records();
    let claim = some(records.claim_job(job.id).await?, "the first real claim")?;
    let report = world
        .storage
        .purger()
        .clean_up(&world.workspace.tenant_id, job.id)
        .await?;
    assert_eq!(report.objects_removed, "1");
    assert_eq!(report.objects_kept, "0");
    assert_eq!(report.upload_files_removed, 0);
    assert!(
        !world
            .storage
            .data()
            .objects_path()
            .join("local")
            .join(some(first_object.as_str().get(..2), "digest shard")?)
            .join(first_object.as_str())
            .is_file()
    );
    let second_object = world.orphan(b"second discarded occurrence").await?;
    assert_eq!(world.request(11).await?.id, job.id);
    let queued = records
        .complete_job(completion(claim.lease, report))
        .await?;
    assert_eq!(queued.state, JobState::Queued);
    assert!(queued.started_at.is_none());
    assert!(queued.finished_at.is_none());
    assert_eq!(
        some(queued.clean_up, "cumulative report of first pass")?.objects_removed,
        "1"
    );
    assert_eq!(world.requests()?, (2, 1));
    let claim = some(records.claim_job(job.id).await?, "the second real claim")?;
    assert_eq!(claim.lease.attempt, 2);
    let report = world
        .storage
        .purger()
        .clean_up(&world.workspace.tenant_id, job.id)
        .await?;
    assert_eq!(report.objects_removed, "1");
    let finished = records
        .complete_job(completion(claim.lease, report))
        .await?;
    assert_eq!(finished.state, JobState::Succeeded);
    let cumulative = some(finished.clean_up, "the final cleanup report")?;
    assert_eq!(cumulative.objects_removed, "2");
    assert_eq!(cumulative.objects_kept, "0");
    assert_eq!(cumulative.upload_files_removed, 0);
    assert!(
        !world
            .storage
            .data()
            .objects_path()
            .join("local")
            .join(some(second_object.as_str().get(..2), "digest shard")?)
            .join(second_object.as_str())
            .is_file()
    );
    assert_eq!(world.requests()?, (2, 2));
    let progress: i64 = Connection::open(world.storage.data().records_path())?.query_row(
        "SELECT count(*) FROM sweep_removals",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(progress, 0);
    Ok(())
}

/// V3.3's reverse order: completion ends the old job before a later request inserts a new one.
#[tokio::test]
async fn a_request_after_completion_creates_a_new_cleanup_job() -> TestResult {
    let world = World::new().await?;
    let first = world.request(10).await?;
    let records = world.storage.records();
    let claim = some(records.claim_job(first.id).await?, "the first real claim")?;
    let report = world
        .storage
        .purger()
        .clean_up(&world.workspace.tenant_id, first.id)
        .await?;
    let done = records
        .complete_job(completion(claim.lease, report))
        .await?;
    assert_eq!(done.state, JobState::Succeeded);
    let second = world.request(11).await?;
    assert_ne!(second.id, first.id);
    assert_eq!(second.state, JobState::Queued);
    assert_eq!(world.requests()?, (2, 1));
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
