//! The `purge_ledger` scenarios again (core M1.4b, ruling P6), through `ApplicationService`
//! composed over the real data directory instead of a test application: core's dispatch, the
//! service's own `purge_workspace`, `create_item` and `save_draft`, and every storage adapter.
//! The purge empties the completed ledger rows of the purged workspace, keeps its own, touches
//! no other workspace's row, and a purge asked for again after it completed answers it.
//!
//! Two stand-ins, said here rather than hidden:
//! - `create_workspace` is not built on this branch (core M1.4a builds it in parallel), so
//!   `Composed` answers it with the one catalog call its contract names
//!   (`WorkspaceCatalog::create` under the request's mutation), as `purge_ledger.rs`'s harness
//!   does. Every other operation goes to `ApplicationService`. Once M1.4a lands, that arm goes.
//! - The purge's tenant job is carried out as ingest's purge handler does (`carry_out`): claimed,
//!   its purge marked running, the purger called with the job's mutation, the report recorded
//!   and the job completed. Storage's tests cannot depend on ingest, so the drain of ingest's
//!   handler is not run here; the workspace has no job to drain.
//!
//! Authorization is held open (`OpenAccess`), as in `purge_ledger.rs`: the local access adapter
//! refuses a purged workspace before the ledger is reached, and what these tests check is that
//! neither the ledger nor a store repeats a write, whatever authorization lets through. The
//! comment scenarios of `purge_ledger.rs` are not repeated: `add_comment` is core M1.3a's.
#![cfg(feature = "runtime")]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use check::{TestResult, err_of, some};
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{JobId, MutationId, TenantId, WorkspaceId};
use okf_jawn_contract::purge::{Purge, PurgeState};
use okf_jawn_contract::workspace::{CreateWorkspaceRequest, Workspace};
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
use okf_jawn_core::storage::{
    LocalSource, NewWorkspace, Provenance, Purger, StorageScope, WorkspaceCatalog,
};
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
                    handler!($id, self, context, request)
                })
            })*
        }
    };
}

/// Where one operation goes: `create_workspace` to the stand-in, every other to the service.
macro_rules! handler {
    (create_workspace, $composed:ident, $context:ident, $request:ident) => {
        $composed.create_workspace($context, $request).await
    };
    ($id:ident, $composed:ident, $context:ident, $request:ident) => {
        $composed.service.$id($context, $request).await
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

    /// The stand-in for `create_workspace` (core M1.4a): the one catalog call its contract
    /// names, under the request's mutation.
    async fn create_workspace(
        &self,
        context: &OperationContext,
        request: CreateWorkspaceRequest,
    ) -> Result<Workspace, ApiError> {
        let mutation = context
            .mutation
            .ok_or_else(|| ApiError::new(ErrorCode::Internal, "a mutation runs under an id"))?;
        self.storage
            .catalog()
            .create(
                &context.principal.tenant_id,
                mutation,
                NewWorkspace {
                    name: request.name,
                    description: request.description,
                    creator: Provenance::from_principal(&context.principal),
                },
            )
            .await
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
/// mutation, record the report as completed, and complete the job.
async fn carry_out(storage: &Storage, purge: &Purge) -> TestResult {
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
    let mut record = records.update_purge(&tenant, record).await?;
    let report = storage
        .purger()
        .purge_workspace(&tenant, claimed.mutation_id, purge_id, workspace_id)
        .await?;
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
    Ok(())
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
    carry_out(&composed.storage, &purge).await?;
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
