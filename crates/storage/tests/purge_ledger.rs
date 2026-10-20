//! A workspace purge and the mutation ledger, through core's dispatch over the real records
//! database (Codex P1 on PR #20, `purge/rows.rs`): the purge empties the completed ledger rows
//! of the purged workspace by the workspace each row records, keeps its own, and touches no
//! other workspace's row, whatever its response text quotes.
//!
//! `Harness` stands in for the application handlers of `create_workspace`, `save_draft`,
//! `add_comment` and `purge_workspace` (of these core has built only `save_draft`): each makes
//! the one storage call its operation's contract names, and records each run with the mutation
//! it ran under, so a test tells a ledger replay (no run) from a re-run. The purge's tenant job
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
use okf_jawn_contract::identity::{Digest, MutationId, PurgeId, TenantId, Timestamp, WorkspaceId};
use okf_jawn_contract::item::{Draft, SaveDraftRequest};
use okf_jawn_contract::proposal::{AddCommentRequest, Comment, Proposal};
use okf_jawn_contract::purge::{Purge, PurgeTarget, PurgeWorkspaceRequest};
use okf_jawn_contract::workspace::{CreateWorkspaceRequest, Workspace};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::context::{Attempt, OperationContext, TenantGrant, WorkspaceGrant};
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch, new_mutation_id};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::jobs::{JobSpec, NewPurge, RecordStore};
use okf_jawn_core::ports::{Application, PortFuture};
use okf_jawn_core::proposals::ProposalStore;
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

            /// Every workspace is live: these tests archive none. The answer is not the
            /// catalog's because authorization is held open (`OpenAccess`): the catalog would
            /// answer `NotFound` for the purged target where the local access adapter refuses
            /// first, and these tests replay requests that name it. The archive rule is core
            /// dispatch's, tested in core against `ApplicationService`'s catalog answer.
            fn archived_at<'a>(&'a self, _scope: &'a StorageScope)
                -> PortFuture<'a, Option<Timestamp>> {
                Box::pin(async { Ok(None) })
            }
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
    (add_comment) => {
        Harness::add_comment
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
/// target, a comment written in each workspace that cites the other, and the request and first
/// response of each mutation.
struct Purged {
    harness: Harness,
    target: WorkspaceId,
    survivor: WorkspaceId,
    purge: PurgeId,
    create_target: (Value, Value),
    create_survivor: (Value, Value),
    draft: (Value, Value),
    /// Written in the target, citing the survivor; its text carries the marker.
    comment_in_target: (Value, Value),
    /// Written in the survivor, citing the target.
    comment_in_survivor: (Value, Value),
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
/// The marker of the target's content: its name, its draft and the comment written in it.
const MARKER: &[u8] = b"zanzibar";
const ITEM: &str = "00000000-0000-4000-8000-000000000010";
/// The proposal each workspace's comment is written on; a proposal's key includes its workspace.
const PROPOSAL: &str = "00000000-0000-4000-8000-000000000020";
const INSTANT: &str = "2026-10-09T12:00:00.000Z";

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

    async fn add_comment(
        &self,
        context: &OperationContext,
        request: AddCommentRequest,
    ) -> Result<Comment, ApiError> {
        let mutation = self.ran("add_comment", context)?;
        let scope = StorageScope {
            tenant_id: context.principal.tenant_id.clone(),
            workspace_id: request.workspace_id,
        };
        // The application names and stamps the comment; any identity and instant serve the store.
        let created_at = Timestamp::try_from(INSTANT.to_owned())
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        self.storage
            .proposals()
            .add_comment(
                &scope,
                mutation,
                request.proposal_id,
                Comment {
                    id: mutation.0.to_string(),
                    author: context.principal.subject.clone(),
                    text: request.text,
                    created_at,
                    invalidated_by: None,
                    source: request.source,
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
             AND (state <> 'purged' OR (digest IS NULL AND purge_id IS NOT NULL))
         ORDER BY operation",
    )?;
    let rows = statement
        .query_map([workspace.0.to_string(), state.to_owned()], |row| {
            row.get(0)
        })?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(rows)
}

/// Store an open proposal in `workspace` at `head`, for a comment to be written on. Seeded
/// through the store: `open_proposal` is not what these tests exercise.
async fn propose(harness: &Harness, workspace: WorkspaceId, head: &Value) -> Fallible<()> {
    let proposal: Proposal = serde_json::from_value(json!({
        "id": PROPOSAL,
        "workspace_id": workspace,
        "base_revision": head,
        "proposal_revision": head,
        "content_digest": "a".repeat(64),
        "title": "Plan",
        "description": "",
        "changes": [],
        "kind": "content",
        "status": "open",
        "created_by": "ana",
        "created_via": "local_owner",
        "created_at": INSTANT
    }))?;
    let scope = StorageScope {
        tenant_id: tenant()?,
        workspace_id: workspace,
    };
    harness
        .storage
        .proposals()
        .insert(&scope, new_mutation_id(), proposal)
        .await?;
    Ok(())
}

/// An `add_comment` request written in `writes` that cites an item of `cites` at `cited_head`.
fn comment_request(
    writes: WorkspaceId,
    (cites, cited_head): (WorkspaceId, &Value),
    text: &str,
    n: u32,
) -> Value {
    json!({
        "workspace_id": writes,
        "proposal_id": PROPOSAL,
        "text": text,
        "source": {
            "workspace_id": cites,
            "item_id": ITEM,
            "path": "notes/plan.md",
            "revision": cited_head,
            "selection": { "kind": "all" }
        },
        "idempotency_key": key(n)
    })
}

/// Create the target and the survivor, draft in the target, comment in each citing the other,
/// purge the target through dispatch and carry the purge out as its tenant job does.
async fn purged(directory: &Path) -> Fallible<Purged> {
    purged_after(directory, |_| Ok(())).await
}

/// `purged`, with `before_job` run on the records database after the purge is requested and
/// before its tenant job runs, to set the ledger's clock.
async fn purged_after(
    directory: &Path,
    before_job: impl FnOnce(&Path) -> TestResult,
) -> Fallible<Purged> {
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
    let survivor_head = some(create_survivor.1.get("head"), "the survivor's head")?.clone();
    propose(&harness, target, &head).await?;
    propose(&harness, survivor, &survivor_head).await?;
    let comment_in_target = first(
        &harness,
        "add_comment",
        comment_request(
            target,
            (survivor, &survivor_head),
            "zanzibar ferry times",
            5,
        ),
    )
    .await?;
    let comment_in_survivor = first(
        &harness,
        "add_comment",
        comment_request(survivor, (target, &head), "Compare the two plans", 6),
    )
    .await?;
    let purge_request = first(
        &harness,
        "purge_workspace",
        json!({ "workspace_id": target, "base_revision": head, "idempotency_key": key(4) }),
    )
    .await?;
    before_job(&directory.join("records.sqlite"))?;
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
        comment_in_target,
        comment_in_survivor,
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
    // Section5.4: begin refuses the terminal row before the original handler runs again.
    let runs = purged.harness.runs_of("save_draft")?;
    assert_eq!(runs.len(), 1);
    assert_eq!(
        some(runs.first(), "the original draft")?.attempt,
        Attempt::First
    );
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
    assert!(
        refused.message.contains(&purged.purge.0.to_string()),
        "{}",
        refused.message
    );
    let runs = purged.harness.runs_of("create_workspace")?;
    assert_eq!(
        runs.len(),
        2,
        "both original creates ran once; begin refused the retry"
    );
    assert_eq!(
        some(runs.first(), "the target create")?.attempt,
        Attempt::First
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
        ledger_rows(directory.path(), purged.target, "purged")?,
        ["add_comment", "create_workspace", "save_draft"]
    );
    assert_eq!(
        ledger_rows(directory.path(), purged.survivor, "completed")?,
        ["add_comment", "create_workspace"]
    );
    Ok(())
}

/// Round 1, item 1: a comment written in the target that cites the survivor is recorded under
/// the target, the workspace it was written in, so the purge of the target reaches it.
#[tokio::test]
async fn purge_ledger_a_comment_citing_another_workspace_goes_with_the_one_it_was_written_in()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let purged = purged(directory.path()).await?;
    let (request, _) = purged.comment_in_target.clone();
    let refused = err_of(call(&purged.harness, "add_comment", request).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict, "{}", refused.message);
    assert!(
        refused.message.contains(&purged.purge.0.to_string()),
        "the refusal names the purge: {}",
        refused.message
    );
    // Section5.4: the target and survivor comments ran once each; begin refuses the retry.
    let runs = purged.harness.runs_of("add_comment")?;
    assert_eq!(runs.len(), 2);
    assert_eq!(
        some(runs.first(), "the original comment")?.attempt,
        Attempt::First
    );
    assert_eq!(
        marked_files(directory.path())?,
        Vec::<String>::new(),
        "the comment's text is in no file"
    );
    Ok(())
}

/// Round 1, item 1: a comment written in the survivor that cites the target is the survivor's.
/// The purge of the workspace it cites leaves its row whole, so its retry replays.
#[tokio::test]
async fn purge_ledger_a_comment_citing_the_purged_workspace_still_replays() -> TestResult {
    let directory = tempfile::tempdir()?;
    let purged = purged(directory.path()).await?;
    let (request, response) = purged.comment_in_survivor.clone();
    let replayed = call(&purged.harness, "add_comment", request).await?;
    assert_eq!(replayed, response);
    assert_eq!(
        purged.harness.runs_of("add_comment")?.len(),
        2,
        "the two comments ran once each; the retry is a replay"
    );
    Ok(())
}

/// Round 2, B2: the purge keeps an emptied row's `finished_ms`, so it expires when the completed
/// row would have, seven days after it completed, not seven days after the purge. The rows
/// completed six days ago; the purge leaves that time as it was; two days later a `begin` drops
/// them. A purge that cleared the time would leave rows no sweep ever reaches.
#[tokio::test]
async fn purge_ledger_an_emptied_row_expires_on_the_clock_of_the_completed_one() -> TestResult {
    const DAY_MS: i64 = 24 * 60 * 60 * 1000;
    let directory = tempfile::tempdir()?;
    let records = directory.path().join("records.sqlite");
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?;
    let six_days_ago = now - 6 * DAY_MS;
    let purged = purged_after(directory.path(), |records| {
        rusqlite::Connection::open(records)?
            .execute("UPDATE mutations SET finished_ms = ?1", [six_days_ago])?;
        Ok(())
    })
    .await?;
    let finished = |state: &str| -> Fallible<Vec<Option<i64>>> {
        let connection = rusqlite::Connection::open(&records)?;
        let mut statement = connection
            .prepare("SELECT finished_ms FROM mutations WHERE workspace_id = ?1 AND state = ?2")?;
        let times = statement
            .query_map([purged.target.0.to_string(), state.to_owned()], |row| {
                row.get(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(times)
    };
    assert_eq!(
        finished("purged")?,
        vec![Some(six_days_ago); 3],
        "the create, the draft and the comment, emptied, keep the time they completed"
    );
    let refused = err_of(call(&purged.harness, "save_draft", purged.draft.0.clone()).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(
        refused.message.contains(&purged.purge.0.to_string()),
        "{}",
        refused.message
    );
    assert_eq!(
        finished("purged")?,
        vec![Some(six_days_ago); 3],
        "section5.2: a refused retry never moves the original retention clocks"
    );
    call(
        &purged.harness,
        "create_workspace",
        json!({ "name": "Sooner", "description": "", "idempotency_key": key(8) }),
    )
    .await?;
    assert_eq!(
        ledger_rows(directory.path(), purged.target, "purged")?.len(),
        3,
        "six days after they completed, a begin leaves the emptied rows where they are"
    );
    rusqlite::Connection::open(&records)?.execute(
        "UPDATE mutations SET finished_ms = finished_ms - ?1",
        [2 * DAY_MS],
    )?;
    call(
        &purged.harness,
        "create_workspace",
        json!({ "name": "Another", "description": "", "idempotency_key": key(9) }),
    )
    .await?;
    assert_eq!(
        ledger_rows(directory.path(), purged.target, "purged")?,
        Vec::<String>::new(),
        "eight days after they completed, the next begin dropped the emptied rows"
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
