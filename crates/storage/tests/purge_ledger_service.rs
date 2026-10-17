//! The `purge_ledger` scenarios again (core M1.4b, ruling P6), through `ApplicationService`
//! composed over the real data directory instead of a test application: core's dispatch, the
//! service's own `create_workspace`, `purge_workspace`, `create_item` and `save_draft`, and
//! every storage adapter.
//! The purge empties the completed ledger rows of the purged workspace, keeps its own, touches
//! no other workspace's row; a purge asked for again after it completed answers it, and one
//! asked for again after its job failed is resumed through the store's retry of a tenant job.
//!
//! One stand-in, said here rather than hidden:
//! - The purge's tenant job is carried out as ingest's purge handler does (`carry_out`): claimed,
//!   its purge marked running, the purger called with the job's mutation, the report recorded
//!   and the job completed; or, when the purger fails, the purge recorded `Failed` with its
//!   error and the job failed with it, retryable whatever its code (ingest's
//!   `a_workspace_purge_that_fails_is_retryable_whatever_its_code` pins that the handler does
//!   so). Storage's tests cannot depend on ingest, so the drain of ingest's handler is not run
//!   here; the workspace has no job to drain.
//!
//! Authorization is held open (`OpenAccess`), as in `purge_ledger.rs`: the local access adapter
//! refuses a purged workspace before the ledger is reached, and what these tests check is that
//! neither the ledger nor a store repeats a write, whatever authorization lets through. The
//! comment scenarios of `purge_ledger.rs` are not repeated: `add_comment` is core M1.3a's.
#![cfg(feature = "runtime")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use check::{TestResult, err_of, some};
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{JobId, MutationId, TenantId, Timestamp, WorkspaceId};
use okf_jawn_contract::import::JobState;
use okf_jawn_contract::purge::{Purge, PurgeState};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::application::{ApplicationConfig, ApplicationService, Ports};
use okf_jawn_core::context::{Attempt, OperationContext, TenantGrant, WorkspaceGrant};
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, Converter, ConverterLimits, PageCount,
};
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::drafts::DraftStore;
use okf_jawn_core::jobs::{JobCompletion, JobQueue, JobScope, JobSpec, RecordStore};
use okf_jawn_core::ports::{Application, PortFuture};
use okf_jawn_core::storage::{LocalSource, Purger, StorageScope, WorkspaceCatalog};
use okf_jawn_storage::Storage;
use serde_json::{Value, json};

macro_rules! composed_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        impl Application for Composed {
            $(fn $id<'a>(&'a self, context: &'a OperationContext, request: $request)
                -> PortFuture<'a, $response> {
                Box::pin(async move {
                    self.ran(stringify!($id), context)?;
                    self.service.$id(context, request).await
                })
            })*

            /// The service's own answer, from its catalog: a wrapper delegates every port method.
            fn archived_at<'a>(&'a self, scope: &'a StorageScope)
                -> PortFuture<'a, Option<Timestamp>> {
                self.service.archived_at(scope)
            }
        }
    };
}

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// `ApplicationService` over one opened data directory, recording each mutation it ran.
struct Composed {
    service: ApplicationService,
    storage: Storage,
    queue: Arc<RecordingQueue>,
    runs: Mutex<Vec<Run>>,
}

/// One run of a mutation's handler.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Run {
    operation: &'static str,
    mutation: MutationId,
    attempt: Attempt,
}

/// Grants every permission on every workspace and on the tenant.
struct OpenAccess;

/// `JobQueue` that records every delivery: the purge's job is carried out by the test.
#[derive(Default)]
struct RecordingQueue {
    deliveries: Mutex<Vec<(JobScope, JobId)>>,
}

/// No conversion runs in these tests.
struct NoConverter;

/// A purge recorded while a survivor's archive does not read (`unreadable_survivor`).
struct Unreadable {
    composed: Composed,
    target: WorkspaceId,
    survivor: WorkspaceId,
    /// The survivor's archive in the pre-migration backup set.
    archive: PathBuf,
    /// Its bytes as written, before they were spoiled.
    readable: Vec<u8>,
    purge: Purge,
}

/// What the scenarios start from: the target purged, the survivor whole.
struct Purged {
    composed: Composed,
    target: WorkspaceId,
    survivor: WorkspaceId,
    purge: Purge,
    create_target: (Value, Value),
    create_survivor: (Value, Value),
    draft: (Value, Value),
    purge_request: (Value, Value),
}

const EVERY: [Permission; 6] = [
    Permission::Read,
    Permission::Write,
    Permission::Propose,
    Permission::Approve,
    Permission::Review,
    Permission::Admin,
];
/// The marker of the target's content: its name, its note and its draft.
const MARKER: &[u8] = b"zanzibar";

impl Composed {
    fn new(storage: Storage) -> Fallible<Self> {
        let queue = Arc::new(RecordingQueue::default());
        let ports = Ports {
            access: Arc::new(OpenAccess),
            mutations: Arc::new(storage.mutations()),
            catalog: Arc::new(storage.catalog()),
            blobs: Arc::new(storage.blobs()),
            versions: Arc::new(storage.versions()),
            records: Arc::new(storage.records()),
            queue: queue.clone(),
            search: Arc::new(storage.search()),
            converter: Arc::new(NoConverter),
            proposals: Arc::new(storage.proposals()),
            uploads: Arc::new(storage.uploads()),
            events: Arc::new(storage.events()),
            credentials: Arc::new(storage.credentials()),
            drafts: Arc::new(storage.drafts()),
            confirmations: Arc::new(storage.confirmations()),
            sandbox: Arc::new(storage.sandbox()),
            readiness: Arc::new(storage.readiness()),
        };
        let config = ApplicationConfig::new(
            "https://sandbox.example.test".to_owned(),
            Duration::from_secs(300),
        );
        Ok(Self {
            service: ApplicationService::new(ports, config)?,
            storage,
            queue,
            runs: Mutex::new(Vec::new()),
        })
    }

    /// Record a run of a mutation's handler.
    fn ran(&self, operation: &'static str, context: &OperationContext) -> Result<(), ApiError> {
        if let Some(mutation) = context.mutation {
            self.runs.lock().map_err(|_| poisoned())?.push(Run {
                operation,
                mutation,
                attempt: context.attempt,
            });
        }
        Ok(())
    }

    /// Every run of `operation`, in order.
    fn runs_of(&self, operation: &str) -> Result<Vec<Run>, ApiError> {
        Ok(self
            .runs
            .lock()
            .map_err(|_| poisoned())?
            .iter()
            .filter(|run| run.operation == operation)
            .cloned()
            .collect())
    }
}

okf_jawn_contract::for_each_operation!(composed_operations);

impl AccessControl for OpenAccess {
    fn authorize<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
        _permission: Permission,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move { Ok(open_grant(principal, workspace)) })
    }

    fn authorize_tenant<'a>(
        &'a self,
        principal: &'a Principal,
        _permission: Permission,
    ) -> PortFuture<'a, TenantGrant> {
        Box::pin(async move {
            Ok(TenantGrant {
                tenant_id: principal.tenant_id.clone(),
                permissions: EVERY.to_vec(),
            })
        })
    }

    fn grants<'a>(&'a self, _principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn grant_creator<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move { Ok(open_grant(principal, workspace)) })
    }

    fn editors<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Vec<String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

impl JobQueue for RecordingQueue {
    fn enqueue(&self, scope: JobScope, job: JobId) -> PortFuture<'_, ()> {
        let delivered = self
            .deliveries
            .lock()
            .map(|mut deliveries| deliveries.push((scope, job)))
            .map_err(|_| poisoned());
        Box::pin(async move { delivered })
    }
}

impl Converter for NoConverter {
    fn page_count<'a>(
        &'a self,
        _source: &'a LocalSource,
        _file_name: &'a str,
    ) -> PortFuture<'a, PageCount> {
        Box::pin(async { Err(no_conversion()) })
    }

    fn convert(&self, _input: ConversionInput) -> PortFuture<'_, Conversion> {
        Box::pin(async { Err(no_conversion()) })
    }

    fn limits(&self) -> ConverterLimits {
        ConverterLimits {
            window_pages: 1,
            memory_limit_bytes: 1,
        }
    }
}

fn open_grant(principal: &Principal, workspace: WorkspaceId) -> WorkspaceGrant {
    WorkspaceGrant {
        scope: StorageScope {
            tenant_id: principal.tenant_id.clone(),
            workspace_id: workspace,
        },
        permissions: EVERY.to_vec(),
    }
}

fn no_conversion() -> ApiError {
    ApiError::new(ErrorCode::Internal, "no conversion runs in these tests")
}

fn poisoned() -> ApiError {
    ApiError::new(ErrorCode::Internal, "the test's record is poisoned")
}

fn tenant() -> Fallible<TenantId> {
    Ok(TenantId::try_from("local".to_owned())?)
}

fn owner() -> Fallible<Principal> {
    Ok(Principal {
        subject: "ana".to_owned(),
        tenant_id: tenant()?,
        route: AccessRoute::LocalOwner,
        client_id: None,
        delegation: None,
    })
}

/// A caller-chosen idempotency key numbered `n`.
fn key(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

/// Dispatch `operation` as the installation owner, awaited inline as a binding does.
async fn call(composed: &Composed, operation: &str, input: Value) -> Result<Value, ApiError> {
    let principal =
        owner().map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
    let mutations = composed.storage.mutations();
    let events = composed.storage.events();
    let ports = DispatchPorts {
        access: &OpenAccess,
        mutations: &mutations,
        events: &events,
    };
    let caller = Caller {
        principal: &principal,
        session_id: Some("session"),
    };
    dispatch(composed, &ports, &caller, operation, input).await
}

/// Dispatch `operation` and keep the request with its first response.
async fn first(composed: &Composed, operation: &str, input: Value) -> Fallible<(Value, Value)> {
    let response = call(composed, operation, input.clone()).await?;
    Ok((input, response))
}

fn workspace_of(response: &Value) -> Fallible<WorkspaceId> {
    Ok(serde_json::from_value(
        some(response.get("id"), "the workspace id")?.clone(),
    )?)
}

/// Carry out the purge's tenant job as ingest's purge handler does, with no job of the
/// workspace to drain: claim it, mark the purge running, call the purger with the job's
/// mutation, record the report as completed, and complete the job. When the purger fails, the
/// purge is recorded `Failed` with its error and the job fails with that error, retryable
/// whatever its code (core M1.4b fix round 1, ruling F1), and the purger's error is answered.
async fn carry_out(storage: &Storage, purge: &Purge) -> Fallible<Result<(), ApiError>> {
    let records = storage.records();
    let tenant = tenant()?;
    let claimed = some(records.claim_job(purge.job_id).await?, "the purge's job")?;
    let JobSpec::PurgeWorkspace {
        purge_id,
        workspace_id,
    } = claimed.spec
    else {
        return Err("the purge's tenant job removes a workspace".into());
    };
    let mut record = records.get_purge(&tenant, purge_id).await?;
    record.state = PurgeState::Running;
    record.error = None;
    let mut record = records.update_purge(&tenant, record).await?;
    let report = match storage
        .purger()
        .purge_workspace(&tenant, claimed.mutation_id, purge_id, workspace_id)
        .await
    {
        Ok(report) => report,
        Err(error) => {
            record.state = PurgeState::Failed;
            record.error = Some(error.clone());
            records.update_purge(&tenant, record).await?;
            records.fail_job(claimed.lease, error.clone(), true).await?;
            return Ok(Err(error));
        }
    };
    record.report = Some(report);
    record.state = PurgeState::Completed;
    record.completed_at = None;
    records.update_purge(&tenant, record).await?;
    records
        .complete_job(JobCompletion {
            lease: claimed.lease,
            revision: None,
            item_ids: Vec::new(),
            artifact: None,
            restore: None,
            backup: None,
            outputs: Vec::new(),
            warnings: Vec::new(),
        })
        .await?;
    Ok(Ok(()))
}

/// Every file under `directory` whose bytes contain the marker.
fn marked_files(directory: &Path) -> Fallible<Vec<String>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.file_name().is_some_and(|name| name == "okf-jawn.lock") {
            // Locked by this process on Windows, and empty.
            continue;
        }
        if path.is_dir() {
            found.extend(marked_files(&path)?);
        } else if std::fs::read(&path)?
            .windows(MARKER.len())
            .any(|window| window == MARKER)
        {
            found.push(path.display().to_string());
        }
    }
    Ok(found)
}

/// The operations of the ledger rows that record `workspace` and stand in `state`, in name
/// order; a row in a state other than `completed` must hold no response.
fn ledger_rows(data: &Path, workspace: WorkspaceId, state: &str) -> Fallible<Vec<String>> {
    let records = rusqlite::Connection::open(data.join("records.sqlite"))?;
    let mut statement = records.prepare(
        "SELECT operation FROM mutations
         WHERE workspace_id = ?1 AND state = ?2 AND (state = 'completed') = (response IS NOT NULL)
         ORDER BY operation",
    )?;
    let rows = statement
        .query_map([workspace.0.to_string(), state.to_owned()], |row| {
            row.get(0)
        })?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(rows)
}

/// Create the target and the survivor, write a note in the target and draft it, purge the
/// target through dispatch and the service's own handler, and carry the purge out.
async fn purged(directory: &Path) -> Fallible<Purged> {
    let composed = Composed::new(Storage::open(directory)?)?;
    let create_target = first(
        &composed,
        "create_workspace",
        json!({ "name": "zanzibar trip", "description": "", "idempotency_key": key(1) }),
    )
    .await?;
    let target = workspace_of(&create_target.1)?;
    let initial = some(create_target.1.get("head"), "the target's head")?.clone();
    // Its description quotes the target's identity in user text.
    let create_survivor = first(
        &composed,
        "create_workspace",
        json!({
            "name": "Garden",
            "description": format!("Seeds swapped with {}", target.0),
            "idempotency_key": key(2)
        }),
    )
    .await?;
    let survivor = workspace_of(&create_survivor.1)?;
    let created = call(
        &composed,
        "create_item",
        json!({
            "workspace_id": target,
            "base_revision": initial,
            "path": "notes/ferry.md",
            "title": "Ferry",
            "type_name": "Note",
            "kind": "note",
            "body": "zanzibar ferry times",
            "properties": { "description": "When the ferries run." },
            "idempotency_key": key(3)
        }),
    )
    .await?;
    let item = some(created.pointer("/item/summary/id"), "the note's id")?.clone();
    let head = some(
        created.pointer("/item/summary/revision"),
        "the note's revision",
    )?
    .clone();
    let draft = first(
        &composed,
        "save_draft",
        json!({
            "workspace_id": target,
            "item_id": item,
            "base_revision": head,
            "body": "zanzibar spice market",
            "properties": { "description": "When the ferries run." },
            "idempotency_key": key(4)
        }),
    )
    .await?;
    let purge_request = first(
        &composed,
        "purge_workspace",
        json!({ "workspace_id": target, "base_revision": head, "idempotency_key": key(5) }),
    )
    .await?;
    let purge: Purge = serde_json::from_value(purge_request.1.clone())?;
    carry_out(&composed.storage, &purge).await??;
    Ok(Purged {
        composed,
        target,
        survivor,
        purge,
        create_target,
        create_survivor,
        draft,
        purge_request,
    })
}

/// The service's own handler recorded the purge on the target's head with its tenant job, and
/// queued the job once; the job carried it out.
#[tokio::test]
async fn purge_ledger_service_records_the_purge_and_queues_its_job_once() -> TestResult {
    let directory = tempfile::tempdir()?;
    let purged = purged(directory.path()).await?;
    assert_eq!(
        purged
            .composed
            .queue
            .deliveries
            .lock()
            .map_err(|_| poisoned())?
            .clone(),
        [(JobScope::Tenant(tenant()?), purged.purge.job_id)]
    );
    let stored = purged
        .composed
        .storage
        .records()
        .get_purge(&tenant()?, purged.purge.id)
        .await?;
    assert_eq!(stored.state, PurgeState::Completed);
    Ok(())
}

#[tokio::test]
async fn purge_ledger_service_a_retried_purge_replays_its_stored_response() -> TestResult {
    let directory = tempfile::tempdir()?;
    let purged = purged(directory.path()).await?;
    let (request, response) = purged.purge_request.clone();
    let replayed = call(&purged.composed, "purge_workspace", request).await?;
    assert_eq!(replayed, response);
    assert_eq!(
        purged.composed.runs_of("purge_workspace")?.len(),
        1,
        "a replay, not a second run"
    );
    Ok(())
}

/// Contract ruling 24, through the service and real storage: a purge asked for again under
/// another key after it completed answers the completed purge, records no second one and
/// queues nothing.
#[tokio::test]
async fn purge_ledger_service_a_purge_asked_for_again_answers_the_completed_one() -> TestResult {
    let directory = tempfile::tempdir()?;
    let purged = purged(directory.path()).await?;
    let (mut request, _) = purged.purge_request.clone();
    let _first_key = some(request.as_object_mut(), "the request")?
        .insert("idempotency_key".to_owned(), json!(key(6)));
    let answered: Purge =
        serde_json::from_value(call(&purged.composed, "purge_workspace", request).await?)?;
    assert_eq!(answered.id, purged.purge.id);
    assert_eq!(answered.state, PurgeState::Completed);
    assert_eq!(
        purged
            .composed
            .queue
            .deliveries
            .lock()
            .map_err(|_| poisoned())?
            .len(),
        1,
        "only the first request queued the job"
    );
    let records = rusqlite::Connection::open(directory.path().join("records.sqlite"))?;
    let recorded: i64 = records.query_row("SELECT count(*) FROM purges", [], |row| row.get(0))?;
    assert_eq!(recorded, 1, "no second purge of the workspace");
    Ok(())
}

/// Contract ruling 24 and P5, through the service and real storage: a purge whose tenant job
/// failed (ingest's handler then records the purge failed) is resumed by asking again under
/// another key, on a base that names nothing: the store's retry of the tenant job makes it
/// runnable, it is queued again, and the purge completes.
#[tokio::test]
async fn purge_ledger_service_a_failed_purge_asked_for_again_is_resumed() -> TestResult {
    let directory = tempfile::tempdir()?;
    let composed = Composed::new(Storage::open(directory.path())?)?;
    let created = call(
        &composed,
        "create_workspace",
        json!({ "name": "Trip", "description": "", "idempotency_key": key(1) }),
    )
    .await?;
    let workspace = workspace_of(&created)?;
    let head = some(created.get("head"), "the head")?.clone();
    let purge: Purge = serde_json::from_value(
        call(
            &composed,
            "purge_workspace",
            json!({ "workspace_id": workspace, "base_revision": head, "idempotency_key": key(2) }),
        )
        .await?,
    )?;
    let records = composed.storage.records();
    let tenant = tenant()?;
    let jobs = JobScope::Tenant(tenant.clone());
    let refusal = ApiError::new(ErrorCode::Unavailable, "the drain timed out");
    let claimed = some(records.claim_job(purge.job_id).await?, "the purge's job")?;
    records
        .fail_job(claimed.lease, refusal.clone(), true)
        .await?;
    let mut failed = records.get_purge(&tenant, purge.id).await?;
    failed.state = PurgeState::Failed;
    failed.error = Some(refusal);
    records.update_purge(&tenant, failed).await?;
    let answered: Purge = serde_json::from_value(
        call(
            &composed,
            "purge_workspace",
            json!({
                "workspace_id": workspace,
                "base_revision": "f".repeat(40),
                "idempotency_key": key(3)
            }),
        )
        .await?,
    )?;
    assert_eq!(
        (answered.id, answered.state),
        (purge.id, PurgeState::Failed)
    );
    assert_eq!(
        records.get_job(&jobs, purge.job_id).await?.state,
        JobState::Queued,
        "the store's retry made the tenant job runnable"
    );
    assert_eq!(
        composed
            .queue
            .deliveries
            .lock()
            .map_err(|_| poisoned())?
            .clone(),
        vec![(jobs.clone(), purge.job_id); 2]
    );
    carry_out(&composed.storage, &purge).await??;
    let completed = records.get_purge(&tenant, purge.id).await?;
    assert_eq!(completed.state, PurgeState::Completed);
    assert!(completed.error.is_none());
    Ok(())
}

/// The target and a survivor whose archive in the pre-migration backup set `set-1` holds bytes
/// that do not read, and the target's purge recorded through the service; the archive's
/// readable bytes are kept, to repair it with.
async fn unreadable_survivor(directory: &Path) -> Fallible<Unreadable> {
    let composed = Composed::new(Storage::open(directory)?)?;
    let created = call(
        &composed,
        "create_workspace",
        json!({ "name": "Trip", "description": "", "idempotency_key": key(1) }),
    )
    .await?;
    let target = workspace_of(&created)?;
    let head = some(created.get("head"), "the head")?.clone();
    let survivor = workspace_of(
        &call(
            &composed,
            "create_workspace",
            json!({ "name": "Garden", "description": "", "idempotency_key": key(2) }),
        )
        .await?,
    )?;
    let set = directory.join("backups/pre-migration/set-1");
    let _written = composed
        .storage
        .backups()
        .write_backup_set(&set, None)
        .await?;
    let archive = set
        .join("workspaces/local")
        .join(format!("{}.zip", survivor.0));
    let readable = std::fs::read(&archive)?;
    std::fs::write(&archive, b"not a zip")?;
    let purge: Purge = serde_json::from_value(
        call(
            &composed,
            "purge_workspace",
            json!({ "workspace_id": target, "base_revision": head, "idempotency_key": key(3) }),
        )
        .await?,
    )?;
    Ok(Unreadable {
        composed,
        target,
        survivor,
        archive,
        readable,
        purge,
    })
}

/// Core M1.4b fix round 1, ruling F1 (contract ruling 14), the review's probe through the
/// service and real storage: a survivor's archive in a pre-migration backup set whose bytes do
/// not read stops the purge. Its job fails retryable with the reader's code, the error naming
/// the archive by identity and by no path, and the purge is recorded `Failed` with the same
/// error. Asked for again while the archive is still bad, the `Failed` purge is answered with
/// its job made runnable and queued again, and it stops the same way; once the archive is
/// repaired, asking again resumes it and it completes.
#[tokio::test]
async fn purge_ledger_service_a_purge_stopped_by_a_survivors_unreadable_archive_resumes_once_it_is_repaired()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let Unreadable {
        composed,
        target,
        survivor,
        archive,
        readable,
        purge,
    } = unreadable_survivor(directory.path()).await?;
    let (records, tenant) = (composed.storage.records(), tenant()?);
    let jobs = JobScope::Tenant(tenant.clone());
    let stopped = err_of(carry_out(&composed.storage, &purge).await?)?;
    assert_eq!(stopped.code, ErrorCode::InvalidInput, "{}", stopped.message);
    assert!(
        stopped.message.contains(&survivor.0.to_string()) && stopped.message.contains("set-1"),
        "the survivor's archive is named by identity: {}",
        stopped.message
    );
    assert!(
        !stopped
            .message
            .contains(&directory.path().display().to_string()),
        "{}",
        stopped.message
    );
    let failed = records.get_purge(&tenant, purge.id).await?;
    assert_eq!(failed.state, PurgeState::Failed);
    assert_eq!(
        failed
            .error
            .as_ref()
            .map(|error| (&error.code, error.message.as_str())),
        Some((&stopped.code, stopped.message.as_str()))
    );
    let job = records.get_job(&jobs, purge.job_id).await?;
    assert_eq!((job.state, job.retryable), (JobState::Failed, Some(true)));
    // Asked for again while the archive is still bad: answered, made runnable, queued again.
    let ask = |n: u32| {
        json!({
            "workspace_id": target,
            "base_revision": "f".repeat(40),
            "idempotency_key": key(n)
        })
    };
    let answered: Purge =
        serde_json::from_value(call(&composed, "purge_workspace", ask(4)).await?)?;
    assert_eq!(
        (answered.id, answered.state),
        (purge.id, PurgeState::Failed)
    );
    assert_eq!(
        records.get_job(&jobs, purge.job_id).await?.state,
        JobState::Queued
    );
    let again = err_of(carry_out(&composed.storage, &purge).await?)?;
    assert_eq!((again.code, again.message), (stopped.code, stopped.message));
    // Repaired: asked for again, it completes.
    std::fs::write(&archive, readable)?;
    let answered: Purge =
        serde_json::from_value(call(&composed, "purge_workspace", ask(5)).await?)?;
    assert_eq!(
        (answered.id, answered.state),
        (purge.id, PurgeState::Failed)
    );
    carry_out(&composed.storage, &purge).await??;
    let completed = records.get_purge(&tenant, purge.id).await?;
    assert_eq!(completed.state, PurgeState::Completed);
    assert!(completed.error.is_none());
    assert_eq!(
        composed
            .queue
            .deliveries
            .lock()
            .map_err(|_| poisoned())?
            .clone(),
        vec![(jobs, purge.job_id); 3],
        "queued by the request and by each request after a failure"
    );
    assert!(archive.is_file(), "the survivor's archive stays");
    Ok(())
}

/// The draft's retry re-runs under its own mutation (the purge emptied its row) and the real
/// handler refuses it: its base revision is one the purge invalidated, so it is the typed
/// `NotFound` naming the purge, before any store is written. No draft is left.
#[tokio::test]
async fn purge_ledger_service_a_retried_mutation_of_the_purged_workspace_is_refused_and_writes_nothing()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let purged = purged(directory.path()).await?;
    let (request, _) = purged.draft.clone();
    let refused = err_of(call(&purged.composed, "save_draft", request).await)?;
    assert_eq!(refused.code, ErrorCode::NotFound, "{}", refused.message);
    assert_eq!(
        refused.detail.as_deref(),
        Some(&ErrorDetail::Invalidated {
            purge_id: purged.purge.id,
            replacement: None
        })
    );
    let runs = purged.composed.runs_of("save_draft")?;
    let (original, retry) = (
        some(runs.first(), "the draft")?,
        some(runs.get(1), "the retry")?,
    );
    assert_eq!(runs.len(), 2);
    assert_eq!(retry.mutation, original.mutation);
    assert_eq!(retry.attempt, Attempt::Resumed);
    let scope = StorageScope {
        tenant_id: tenant()?,
        workspace_id: purged.target,
    };
    let item =
        serde_json::from_value(some(purged.draft.0.get("item_id"), "the drafted item")?.clone())?;
    let drafts = purged.composed.storage.drafts();
    assert!(drafts.get(&scope, item, "ana").await?.is_none(), "no draft");
    Ok(())
}

#[tokio::test]
async fn purge_ledger_service_another_workspace_s_mutation_quoting_the_purged_identity_still_replays()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let purged = purged(directory.path()).await?;
    let (request, response) = purged.create_survivor.clone();
    let replayed = call(&purged.composed, "create_workspace", request).await?;
    assert_eq!(replayed, response);
    assert_eq!(workspace_of(&replayed)?, purged.survivor);
    assert_eq!(
        purged.composed.runs_of("create_workspace")?.len(),
        2,
        "the two creates ran once each; the retry is a replay"
    );
    Ok(())
}

#[tokio::test]
async fn purge_ledger_service_a_retried_create_of_the_purged_workspace_is_refused_and_creates_nothing()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let purged = purged(directory.path()).await?;
    let (request, _) = purged.create_target.clone();
    let refused = err_of(call(&purged.composed, "create_workspace", request).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict, "{}", refused.message);
    let runs = purged.composed.runs_of("create_workspace")?;
    let (original, retry) = (
        some(runs.first(), "the create")?,
        some(runs.get(2), "the retry")?,
    );
    assert_eq!(
        retry.mutation, original.mutation,
        "the retry re-runs under the purged workspace's own mutation"
    );
    let listed = purged
        .composed
        .storage
        .catalog()
        .list(&tenant()?, true)
        .await?;
    let ids: Vec<WorkspaceId> = listed.iter().map(|workspace| workspace.id).collect();
    assert_eq!(
        ids,
        vec![purged.survivor],
        "no second workspace was created"
    );
    Ok(())
}

#[tokio::test]
async fn purge_ledger_service_no_file_keeps_a_response_of_the_purged_workspace() -> TestResult {
    let directory = tempfile::tempdir()?;
    let purged = purged(directory.path()).await?;
    assert_eq!(
        marked_files(directory.path())?,
        Vec::<String>::new(),
        "the target's name, note and draft are in no file, its ledger responses included"
    );
    assert_eq!(
        ledger_rows(directory.path(), purged.target, "completed")?,
        ["purge_workspace"]
    );
    assert_eq!(
        ledger_rows(directory.path(), purged.target, "released")?,
        ["create_item", "create_workspace", "save_draft"]
    );
    assert_eq!(
        ledger_rows(directory.path(), purged.survivor, "completed")?,
        ["create_workspace"]
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
