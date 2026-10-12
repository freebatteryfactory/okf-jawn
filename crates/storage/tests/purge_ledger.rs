//! A workspace purge and the mutation ledger, through core's dispatch over the real records
//! database (Codex P1 on PR #20, `purge/rows.rs`): the purge empties the completed ledger rows
//! of the purged workspace by the workspace each row records, keeps its own, and touches no
//! other workspace's row, whatever its response text quotes.
//!
//! `Harness` stands in for the application handlers of `create_workspace`, `save_draft` and
//! `purge_workspace` (core's handlers of the first and last are not built yet): each makes the
//! one storage call its operation's contract names, and records each run with the mutation it
//! ran under, so a test tells a ledger replay (no run) from a re-run. The purge's tenant job
//! is carried out as ingest's handler does: claimed, then the purger called with the job's
//! mutation. Authorization is held open (`OpenAccess`): the local access adapter refuses the
//! purged workspace before the ledger is reached, and what these tests check is that neither
//! the ledger nor a store repeats a write whatever authorization lets through.
#![cfg(feature = "runtime")]

use std::future::Ready;
use std::path::Path;
use std::sync::Mutex;

use check::{TestResult, err_of, some};
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, MutationId, PurgeId, TenantId, WorkspaceId};
use okf_jawn_contract::item::{Draft, SaveDraftRequest};
use okf_jawn_contract::purge::{Purge, PurgeTarget, PurgeWorkspaceRequest};
use okf_jawn_contract::workspace::{CreateWorkspaceRequest, Workspace};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::context::{Attempt, OperationContext, TenantGrant, WorkspaceGrant};
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::jobs::{JobSpec, NewPurge, RecordStore};
use okf_jawn_core::ports::{Application, PortFuture};
use okf_jawn_core::storage::{
    NewWorkspace, Provenance, Purger, StorageScope, WorkspaceCatalog, derive_purge_id,
};
use okf_jawn_storage::Storage;
use serde_json::{Value, json};

macro_rules! harness_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        impl Application for Harness {
            $(fn $id<'a>(&'a self, context: &'a OperationContext, request: $request)
                -> PortFuture<'a, $response> {
                Box::pin(handler!($id)(self, context, request))
            })*
        }
    };
}

/// The harness handler of one operation; every other operation is not built here.
macro_rules! handler {
    (create_workspace) => {
        Harness::create_workspace
    };
    (save_draft) => {
        Harness::save_draft
    };
    (purge_workspace) => {
        Harness::purge_workspace
    };
    ($other:ident) => {
        not_built
    };
}

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// Handlers over one opened data directory that record what ran.
struct Harness {
    storage: Storage,
    /// Each handler run: its operation, mutation and attempt, in order.
    runs: Mutex<Vec<Run>>,
}

/// One handler run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Run {
    operation: &'static str,
    mutation: MutationId,
    attempt: Attempt,
}

/// Grants every permission on every workspace and on the tenant.
struct OpenAccess;

/// A purged target, a survivor whose description quotes the target's identity, a draft of the
/// target, and the request and first response of each mutation.
struct Purged {
    harness: Harness,
    target: WorkspaceId,
    survivor: WorkspaceId,
    purge: PurgeId,
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
/// The marker of the target's content: its name and its draft.
const MARKER: &[u8] = b"zanzibar";
const ITEM: &str = "00000000-0000-4000-8000-000000000010";

impl Harness {
    fn new(storage: Storage) -> Self {
        Self {
            storage,
            runs: Mutex::new(Vec::new()),
        }
    }

    /// Record a run of `operation` and return the mutation it runs under.
    fn ran(
        &self,
        operation: &'static str,
        context: &OperationContext,
    ) -> Result<MutationId, ApiError> {
        let mutation = context
            .mutation
            .ok_or_else(|| ApiError::new(ErrorCode::Internal, "a mutation runs under an id"))?;
        self.runs.lock().map_err(|_| poisoned())?.push(Run {
            operation,
            mutation,
            attempt: context.attempt,
        });
        Ok(mutation)
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

    async fn create_workspace(
        &self,
        context: &OperationContext,
        request: CreateWorkspaceRequest,
    ) -> Result<Workspace, ApiError> {
        let mutation = self.ran("create_workspace", context)?;
        self.storage
            .catalog()
            .create(
                &context.principal.tenant_id,
                mutation,
                NewWorkspace {
                    name: request.name,
                    description: request.description,
                    creator: provenance(context),
                },
            )
            .await
    }

    async fn save_draft(
        &self,
        context: &OperationContext,
        request: SaveDraftRequest,
    ) -> Result<Draft, ApiError> {
        let mutation = self.ran("save_draft", context)?;
        let scope = StorageScope {
            tenant_id: context.principal.tenant_id.clone(),
            workspace_id: request.workspace_id,
        };
        // The application digests the drafted document; the store keeps whatever it is given.
        let content_digest = Digest::try_from("e".repeat(64))
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        self.storage
            .drafts()
            .save(
                &scope,
                mutation,
                DraftWrite {
                    item_id: request.item_id,
                    editor: context.principal.subject.clone(),
                    base_revision: request.base_revision,
                    body: request.body,
                    properties: request.properties,
                    content_digest,
                },
            )
            .await
    }

    async fn purge_workspace(
        &self,
        context: &OperationContext,
        request: PurgeWorkspaceRequest,
    ) -> Result<Purge, ApiError> {
        let mutation = self.ran("purge_workspace", context)?;
        self.storage
            .records()
            .create_purge(
                &context.principal.tenant_id,
                mutation,
                NewPurge {
                    id: derive_purge_id(mutation),
                    target: PurgeTarget::Workspace {
                        workspace_id: request.workspace_id,
                    },
                    initiator: provenance(context),
                },
            )
            .await
    }
}

okf_jawn_contract::for_each_operation!(harness_operations);

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

/// The answer of an operation the harness does not build.
fn not_built<Request, Response>(
    _harness: &Harness,
    context: &OperationContext,
    _request: Request,
) -> Ready<Result<Response, ApiError>> {
    std::future::ready(Err(ApiError::new(
        ErrorCode::NotImplemented,
        format!(
            "{} is not built in this harness",
            context.operation.as_str()
        ),
    )))
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

fn provenance(context: &OperationContext) -> Provenance {
    Provenance {
        subject: context.principal.subject.clone(),
        route: context.principal.route.clone(),
        client_id: context.principal.client_id.clone(),
    }
}

fn poisoned() -> ApiError {
    ApiError::new(ErrorCode::Internal, "the harness record is poisoned")
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
async fn call(harness: &Harness, operation: &str, input: Value) -> Result<Value, ApiError> {
    let principal =
        owner().map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
    let mutations = harness.storage.mutations();
    let events = harness.storage.events();
    let ports = DispatchPorts {
        access: &OpenAccess,
        mutations: &mutations,
        events: &events,
    };
    let caller = Caller {
        principal: &principal,
        session_id: None,
    };
    dispatch(harness, &ports, &caller, operation, input).await
}

/// Dispatch `operation` and keep the request with its first response.
async fn first(harness: &Harness, operation: &str, input: Value) -> Fallible<(Value, Value)> {
    let response = call(harness, operation, input.clone()).await?;
    Ok((input, response))
}

fn workspace_of(response: &Value) -> Fallible<WorkspaceId> {
    Ok(serde_json::from_value(
        some(response.get("id"), "the workspace id")?.clone(),
    )?)
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

/// Create the target and the survivor, draft in the target, purge the target through dispatch
/// and carry the purge out as its tenant job does.
async fn purged(directory: &Path) -> Fallible<Purged> {
    let harness = Harness::new(Storage::open(directory)?);
    let create_target = first(
        &harness,
        "create_workspace",
        json!({ "name": "zanzibar trip", "description": "", "idempotency_key": key(1) }),
    )
    .await?;
    let target = workspace_of(&create_target.1)?;
    let head = some(create_target.1.get("head"), "the target's head")?.clone();
    // Its description quotes the target's identity in user text.
    let create_survivor = first(
        &harness,
        "create_workspace",
        json!({
            "name": "Garden",
            "description": format!("Seeds swapped with {}", target.0),
            "idempotency_key": key(2)
        }),
    )
    .await?;
    let survivor = workspace_of(&create_survivor.1)?;
    let draft = first(
        &harness,
        "save_draft",
        json!({
            "workspace_id": target,
            "item_id": ITEM,
            "base_revision": head,
            "body": "zanzibar spice market",
            "properties": {},
            "idempotency_key": key(3)
        }),
    )
    .await?;
    let purge_request = first(
        &harness,
        "purge_workspace",
        json!({ "workspace_id": target, "base_revision": head, "idempotency_key": key(4) }),
    )
    .await?;
    let recorded: Purge = serde_json::from_value(purge_request.1.clone())?;
    let records = harness.storage.records();
    let claimed = some(records.claim_job(recorded.job_id).await?, "the purge job")?;
    let JobSpec::PurgeWorkspace {
        purge_id,
        workspace_id,
    } = claimed.spec
    else {
        return Err("the purge's tenant job removes a workspace".into());
    };
    harness
        .storage
        .purger()
        .purge_workspace(&tenant()?, claimed.mutation_id, purge_id, workspace_id)
        .await?;
    Ok(Purged {
        harness,
        target,
        survivor,
        purge: recorded.id,
        create_target,
        create_survivor,
        draft,
        purge_request,
    })
}

#[tokio::test]
async fn purge_ledger_a_retried_purge_replays_its_stored_response() -> TestResult {
    let directory = tempfile::tempdir()?;
    let purged = purged(directory.path()).await?;
    let (request, response) = purged.purge_request.clone();
    let replayed = call(&purged.harness, "purge_workspace", request).await?;
    assert_eq!(replayed, response);
    assert_eq!(
        purged.harness.runs_of("purge_workspace")?.len(),
        1,
        "a replay, not a second run"
    );
    Ok(())
}

#[tokio::test]
async fn purge_ledger_a_retried_mutation_of_the_purged_workspace_is_refused_and_writes_nothing()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let purged = purged(directory.path()).await?;
    let (request, _) = purged.draft.clone();
    let refused = err_of(call(&purged.harness, "save_draft", request).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(
        refused.message.contains(&purged.purge.0.to_string()),
        "the refusal names the purge: {}",
        refused.message
    );
    // It ran again under the same mutation, so no store could take it for a new write.
    let runs = purged.harness.runs_of("save_draft")?;
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
    let item = serde_json::from_value(json!(ITEM))?;
    let drafts = purged.harness.storage.drafts();
    assert!(drafts.get(&scope, item, "ana").await?.is_none(), "no draft");
    Ok(())
}

#[tokio::test]
async fn purge_ledger_another_workspace_s_mutation_quoting_the_purged_identity_still_replays()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let purged = purged(directory.path()).await?;
    let (request, response) = purged.create_survivor.clone();
    let replayed = call(&purged.harness, "create_workspace", request).await?;
    assert_eq!(replayed, response);
    assert_eq!(workspace_of(&replayed)?, purged.survivor);
    assert_eq!(
        purged.harness.runs_of("create_workspace")?.len(),
        2,
        "the two creates ran once each; the retry is a replay"
    );
    Ok(())
}

#[tokio::test]
async fn purge_ledger_a_retried_create_of_the_purged_workspace_is_refused_and_creates_nothing()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let purged = purged(directory.path()).await?;
    let (request, _) = purged.create_target.clone();
    let refused = err_of(call(&purged.harness, "create_workspace", request).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict, "{}", refused.message);
    let runs = purged.harness.runs_of("create_workspace")?;
    let (original, retry) = (
        some(runs.first(), "the create")?,
        some(runs.get(2), "the retry")?,
    );
    assert_eq!(
        retry.mutation, original.mutation,
        "the retry re-runs under the purged workspace's own mutation"
    );
    let listed = purged
        .harness
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
async fn purge_ledger_no_file_keeps_a_response_of_the_purged_workspace() -> TestResult {
    let directory = tempfile::tempdir()?;
    let purged = purged(directory.path()).await?;
    assert_eq!(
        marked_files(directory.path())?,
        Vec::<String>::new(),
        "the target's name and draft are in no file, its ledger responses included"
    );
    // The ledger as the purge left it: of the target only the purge's own row keeps a response,
    // its other rows are emptied with their identity kept, and the survivor's row is whole.
    assert_eq!(
        ledger_rows(directory.path(), purged.target, "completed")?,
        ["purge_workspace"]
    );
    assert_eq!(
        ledger_rows(directory.path(), purged.target, "released")?,
        ["create_workspace", "save_draft"]
    );
    assert_eq!(
        ledger_rows(directory.path(), purged.survivor, "completed")?,
        ["create_workspace"]
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
