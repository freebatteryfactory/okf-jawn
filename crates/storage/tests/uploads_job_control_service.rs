//! Upload slots and job control (core M1.5a) through `ApplicationService` composed over a real
//! data directory: core's dispatch, the service's own handlers, and every storage adapter.
//!
//! Uploads: slots opened through `create_upload`, their bytes put through the store's
//! `put_content` (the port of the `upload_content` transport), completed through
//! `complete_upload` (a digest that is not the bytes' refused with the store's mismatch), one
//! taken by a job, read back through `get_upload` and listed in each state through
//! `list_uploads`; and `create_upload` run again under its mutation (a resumed attempt, called
//! on the service directly) answered the first slot by the store's uniqueness. Jobs: a failed
//! job retried and a queued job cancelled through `retry_job`
//! and `cancel_job`, each announced once in the workspace's log and the retry delivered; a
//! sealed job's cancellation refused; and a tenant job not found through the workspace, but
//! through the tenant.
//!
//! Stand-ins, said here rather than hidden: authorization is held open (`OpenAccess`), as in
//! `purge_ledger_service.rs` (the job-kind rule and dispatch's archive rule are core's tests);
//! jobs are created through the store and claimed, failed and sealed as ingest's runtime and
//! handlers would (`RecordStore::claim_job`, `fail_job`, `seal`); no handler runs; and the queue
//! records each delivery.
#![cfg(feature = "runtime")]

use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use check::{TestResult, err_of, some};
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::extraction::ConversionSettings;
use okf_jawn_contract::identity::{
    Digest, JobId, MutationId, Revision, TenantId, UploadId, WorkspaceId,
};
use okf_jawn_contract::import::{
    CreateUploadRequest, Job, JobState, ListJobsResponse, ListUploadsResponse, Upload,
};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::application::{ApplicationConfig, ApplicationService, Ports};
use okf_jawn_core::context::{Attempt, OperationContext, TenantGrant, WorkspaceGrant};
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, Converter, ConverterLimits, PageCount,
};
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::events::{EventLog, EventQuery, EventScope};
use okf_jawn_core::jobs::{JobQueue, JobScope, JobSpec, NewJob, RecordStore};
use okf_jawn_core::ports::{Application, PortFuture};
use okf_jawn_core::storage::{LocalSource, Page, Provenance, StorageScope};
use okf_jawn_core::uploads::UploadStore;
use okf_jawn_storage::Storage;
use serde_json::{Value, json};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// The service over one opened data directory, and the queue it delivers to.
struct World {
    service: ApplicationService,
    storage: Storage,
    queue: Arc<RecordingQueue>,
}

/// Grants every permission on every workspace and on the tenant.
struct OpenAccess;

/// `JobQueue` that records every delivery: no worker runs in these tests.
#[derive(Default)]
struct RecordingQueue {
    deliveries: Mutex<Vec<(JobScope, JobId)>>,
}

/// No conversion runs in these tests.
struct NoConverter;

const EVERY: [Permission; 6] = [
    Permission::Read,
    Permission::Write,
    Permission::Propose,
    Permission::Approve,
    Permission::Review,
    Permission::Admin,
];
/// The ten bytes a complete upload holds.
const BYTES: &[u8] = b"0123456789";

impl World {
    fn new(directory: &std::path::Path) -> Fallible<Self> {
        let storage = Storage::open(directory)?;
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
        })
    }

    /// Dispatch `operation` as the installation owner in a browser session, awaited inline as
    /// a binding does.
    async fn call(&self, operation: &str, input: Value) -> Result<Value, ApiError> {
        let principal = owner().map_err(internal)?;
        let mutations = self.storage.mutations();
        let events = self.storage.events();
        let ports = DispatchPorts {
            access: &OpenAccess,
            mutations: &mutations,
            events: &events,
        };
        let caller = Caller {
            principal: &principal,
            session_id: Some("session"),
        };
        dispatch(&self.service, &ports, &caller, operation, input).await
    }

    /// A new workspace, created through the service, with its head.
    async fn workspace(&self) -> Fallible<(StorageScope, Revision)> {
        let created = self
            .call(
                "create_workspace",
                json!({ "name": "Imports", "description": "", "idempotency_key": key(1) }),
            )
            .await?;
        let workspace_id: WorkspaceId =
            serde_json::from_value(some(created.get("id"), "the workspace id")?.clone())?;
        let head: Revision =
            serde_json::from_value(some(created.get("head"), "the head")?.clone())?;
        Ok((
            StorageScope {
                tenant_id: tenant()?,
                workspace_id,
            },
            head,
        ))
    }

    fn deliveries(&self) -> Result<Vec<(JobScope, JobId)>, ApiError> {
        Ok(self
            .queue
            .deliveries
            .lock()
            .map_err(|_| poisoned())?
            .clone())
    }
}

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
        Box::pin(async { Err(internal("no conversion runs in these tests")) })
    }

    fn convert(&self, _input: ConversionInput) -> PortFuture<'_, Conversion> {
        Box::pin(async { Err(internal("no conversion runs in these tests")) })
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

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, error.to_string())
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

fn initiator() -> Provenance {
    Provenance {
        subject: "ana".to_owned(),
        route: AccessRoute::LocalOwner,
        client_id: None,
    }
}

/// A caller-chosen idempotency key numbered `n`.
fn key(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

/// A mutation identity numbered `n`, for jobs the store is given directly.
fn mutation(n: u32) -> Fallible<MutationId> {
    Ok(serde_json::from_value(json!(format!(
        "00000000-0000-4000-9000-{n:012}"
    )))?)
}

/// The SHA-256 of `bytes`, as an upload is completed with it.
fn sha256(bytes: &[u8]) -> Fallible<Digest> {
    use sha2::Digest as _;
    use std::fmt::Write as _;
    let mut text = String::new();
    for byte in sha2::Sha256::digest(bytes) {
        write!(text, "{byte:02x}")?;
    }
    Ok(Digest::try_from(text)?)
}

/// Open a slot for `filename`, announcing ten bytes, in `scope` through `create_upload` under
/// the key numbered `n`.
async fn open(world: &World, scope: &StorageScope, n: u32, filename: &str) -> Fallible<Upload> {
    Ok(serde_json::from_value(
        world
            .call(
                "create_upload",
                json!({
                    "workspace_id": scope.workspace_id, "filename": filename,
                    "relative_path": "2026/q3", "size": BYTES.len().to_string(),
                    "idempotency_key": key(n)
                }),
            )
            .await?,
    )?)
}

/// Put `bytes` into the slot through the store, as the `upload_content` transport does.
async fn put(world: &World, scope: &StorageScope, upload: UploadId, bytes: &[u8]) -> TestResult {
    world
        .storage
        .uploads()
        .put_content(
            scope,
            upload,
            Box::pin(Cursor::new(bytes.to_vec())),
            1 << 20,
        )
        .await?;
    Ok(())
}

fn upload_ids(response: &Value) -> Fallible<Vec<UploadId>> {
    let listed: ListUploadsResponse = serde_json::from_value(response.clone())?;
    Ok(listed.items.iter().map(|item| item.id).collect())
}

#[tokio::test]
async fn uploads_through_the_service_are_opened_filled_completed_read_and_listed_by_state()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let world = World::new(directory.path())?;
    let (scope, head) = world.workspace().await?;
    let incomplete = open(&world, &scope, 2, "draft.pdf").await?;
    let waiting = open(&world, &scope, 3, "report.pdf").await?;
    let consumed = open(&world, &scope, 4, "archive.pdf").await?;
    put(&world, &scope, incomplete.id, b"0123").await?;
    put(&world, &scope, waiting.id, BYTES).await?;
    put(&world, &scope, consumed.id, BYTES).await?;
    let read: Upload = serde_json::from_value(
        world
            .call(
                "get_upload",
                json!({ "workspace_id": scope.workspace_id, "upload_id": incomplete.id }),
            )
            .await?,
    )?;
    assert_eq!((read.received_bytes.as_str(), read.complete), ("4", false));
    assert_eq!(read.relative_path.as_deref(), Some("2026/q3"));
    let complete = |upload: UploadId, sha256: Digest, n: u32| {
        json!({
            "workspace_id": scope.workspace_id, "upload_id": upload, "sha256": sha256,
            "idempotency_key": key(n)
        })
    };
    let refused = err_of(
        world
            .call(
                "complete_upload",
                complete(waiting.id, sha256(b"other")?, 5),
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict, "{}", refused.message);
    assert_eq!(
        refused.detail.as_deref(),
        Some(&ErrorDetail::UploadMismatch {
            expected_bytes: "10".to_owned(),
            received_bytes: "10".to_owned(),
            digest_matches: Some(false),
        })
    );
    for (upload, n) in [(waiting.id, 6), (consumed.id, 7)] {
        let done: Upload = serde_json::from_value(
            world
                .call("complete_upload", complete(upload, sha256(BYTES)?, n))
                .await?,
        )?;
        assert!(done.complete);
    }
    // An import job takes one of them, as its handler does when it starts.
    let job = world
        .storage
        .records()
        .create_job(
            &JobScope::Workspace(scope.clone()),
            NewJob {
                mutation_id: mutation(1)?,
                initiator: initiator(),
                spec: JobSpec::Import {
                    base_revision: head,
                    upload_ids: vec![consumed.id],
                    destination: None,
                    apply_naming_rules: false,
                    settings: ConversionSettings::default(),
                },
            },
        )
        .await?;
    world
        .storage
        .uploads()
        .consume(&scope, consumed.id, job.id)
        .await?;
    for (state, expected) in [
        (None, vec![consumed.id, waiting.id, incomplete.id]),
        (Some("incomplete"), vec![incomplete.id]),
        (Some("waiting"), vec![waiting.id]),
        (Some("consumed"), vec![consumed.id]),
    ] {
        let mut request = json!({ "workspace_id": scope.workspace_id, "page": { "limit": 50 } });
        if let (Some(state), Some(fields)) = (state, request.as_object_mut()) {
            fields.insert("state".to_owned(), json!(state));
        }
        let listed = world.call("list_uploads", request).await?;
        assert_eq!(upload_ids(&listed)?, expected, "{state:?}");
    }
    let taken: Upload = serde_json::from_value(
        world
            .call(
                "get_upload",
                json!({ "workspace_id": scope.workspace_id, "upload_id": consumed.id }),
            )
            .await?,
    )?;
    assert_eq!(taken.consumed_by, Some(job.id));
    Ok(())
}

/// Ruling U1's repeat against the real store: the handler run again under the same mutation, as
/// dispatch runs a resumed attempt, is answered the first slot (`UploadStore::create` is unique
/// on the mutation), and another mutation opens another slot.
#[tokio::test]
async fn uploads_a_create_run_again_under_its_mutation_answers_the_first_slot() -> TestResult {
    let directory = tempfile::tempdir()?;
    let world = World::new(directory.path())?;
    let (scope, _) = world.workspace().await?;
    let context = |n: u32, attempt: Attempt| -> Fallible<OperationContext> {
        let principal = owner()?;
        Ok(OperationContext {
            grants: vec![open_grant(&principal, scope.workspace_id)],
            principal,
            session_id: Some("session".to_owned()),
            operation: OperationName::CreateUpload,
            tenant: None,
            mutation: Some(mutation(n)?),
            attempt,
        })
    };
    let request = || -> Fallible<CreateUploadRequest> {
        Ok(serde_json::from_value(json!({
            "workspace_id": scope.workspace_id, "filename": "report.pdf", "relative_path": "",
            "size": "10", "idempotency_key": key(2)
        }))?)
    };
    let first = world
        .service
        .create_upload(&context(20, Attempt::First)?, request()?)
        .await?;
    let again = world
        .service
        .create_upload(&context(20, Attempt::Resumed)?, request()?)
        .await?;
    assert_eq!(again.id, first.id, "the first slot");
    let other = world
        .service
        .create_upload(&context(21, Attempt::First)?, request()?)
        .await?;
    assert_ne!(other.id, first.id);
    let listed = world
        .call(
            "list_uploads",
            json!({ "workspace_id": scope.workspace_id, "page": { "limit": 50 } }),
        )
        .await?;
    assert_eq!(upload_ids(&listed)?, vec![other.id, first.id], "two slots");
    Ok(())
}

/// A job of the workspace created through the store under the mutation numbered `n`.
async fn job_of(world: &World, scope: &StorageScope, n: u32, spec: JobSpec) -> Fallible<Job> {
    Ok(world
        .storage
        .records()
        .create_job(
            &JobScope::Workspace(scope.clone()),
            NewJob {
                mutation_id: mutation(n)?,
                initiator: initiator(),
                spec,
            },
        )
        .await?)
}

/// The jobs the workspace's log announces as updated, in order.
async fn announced(world: &World, scope: &StorageScope) -> Fallible<Vec<Option<JobId>>> {
    let page = world
        .storage
        .events()
        .list(
            &EventScope::Workspace(scope.clone()),
            EventQuery {
                after: None,
                page: Page {
                    cursor: None,
                    limit: 100,
                },
            },
        )
        .await?;
    Ok(page
        .events
        .iter()
        .filter(|event| event.kind == EventKind::JobUpdated)
        .map(|event| event.job_id)
        .collect())
}

fn control(scope: &StorageScope, job: JobId, n: u32) -> Value {
    json!({ "workspace_id": scope.workspace_id, "job_id": job, "idempotency_key": key(n) })
}

#[tokio::test]
async fn job_control_through_the_service_retries_and_cancels_real_jobs_and_announces_them()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let world = World::new(directory.path())?;
    let (scope, head) = world.workspace().await?;
    let records = world.storage.records();
    let export = || JobSpec::ExportWorkspace {
        revision: head.clone(),
        include_history: false,
    };
    let failed = job_of(&world, &scope, 1, export()).await?;
    let claimed = some(records.claim_job(failed.id).await?, "the claim")?;
    records
        .fail_job(
            claimed.lease,
            ApiError::new(ErrorCode::Unavailable, "the disk was full"),
            true,
        )
        .await?;
    let queued = job_of(&world, &scope, 2, JobSpec::BackupWorkspace).await?;
    let retry = control(&scope, failed.id, 10);
    let response = world.call("retry_job", retry.clone()).await?;
    let retried: Job = serde_json::from_value(response.clone())?;
    assert_eq!((retried.id, retried.state), (failed.id, JobState::Queued));
    assert_eq!((retried.error.is_none(), retried.retryable), (true, None));
    let jobs = JobScope::Workspace(scope.clone());
    assert_eq!(
        records.get_job(&jobs, failed.id).await?.state,
        JobState::Queued
    );
    assert_eq!(world.deliveries()?, vec![(jobs.clone(), failed.id)]);
    let cancelled: Job = serde_json::from_value(
        world
            .call("cancel_job", control(&scope, queued.id, 11))
            .await?,
    )?;
    assert_eq!(cancelled.state, JobState::Cancelled);
    assert!(cancelled.finished_at.is_some());
    assert_eq!(
        announced(&world, &scope).await?,
        vec![Some(failed.id), Some(queued.id)]
    );
    // The retry's repeat is the ledger's replay: no second delivery, no second announcement.
    assert_eq!(world.call("retry_job", retry).await?, response);
    assert_eq!(world.deliveries()?.len(), 1);
    // The store's refusals: a cancelled job is not cancelled again, nor a sealed one.
    let again = err_of(
        world
            .call("cancel_job", control(&scope, queued.id, 12))
            .await,
    )?;
    assert_eq!(again.code, ErrorCode::Conflict, "{}", again.message);
    let exporting = job_of(&world, &scope, 3, export()).await?;
    let running = some(records.claim_job(exporting.id).await?, "the claim")?;
    records.seal(&running.lease).await?;
    let sealed = err_of(
        world
            .call("cancel_job", control(&scope, exporting.id, 13))
            .await,
    )?;
    assert_eq!(sealed.code, ErrorCode::Conflict);
    assert!(
        sealed.message.contains("point of no return"),
        "{}",
        sealed.message
    );
    assert_eq!(
        records.get_job(&jobs, exporting.id).await?.state,
        JobState::Running
    );
    assert_eq!(
        announced(&world, &scope).await?,
        vec![Some(failed.id), Some(queued.id)],
        "a refusal announces nothing"
    );
    Ok(())
}

/// The store's scope filter, through the service: a tenant job is not found through the
/// workspace (not read, listed or cancelled there), and is found at the tenant, which lists no
/// workspace job.
#[tokio::test]
async fn job_control_through_the_service_keeps_tenant_and_workspace_jobs_apart() -> TestResult {
    let directory = tempfile::tempdir()?;
    let world = World::new(directory.path())?;
    let (scope, _) = world.workspace().await?;
    let workspace_job = job_of(&world, &scope, 1, JobSpec::BackupWorkspace).await?;
    let tenant_job = world
        .storage
        .records()
        .create_job(
            &JobScope::Tenant(tenant()?),
            NewJob {
                mutation_id: mutation(2)?,
                initiator: initiator(),
                spec: JobSpec::BackupInstallation,
            },
        )
        .await?;
    let hidden = err_of(
        world
            .call(
                "get_job",
                json!({ "workspace_id": scope.workspace_id, "job_id": tenant_job.id }),
            )
            .await,
    )?;
    assert_eq!(hidden.code, ErrorCode::NotFound);
    let refused = err_of(
        world
            .call("cancel_job", control(&scope, tenant_job.id, 10))
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    let listed: ListJobsResponse = serde_json::from_value(
        world
            .call(
                "list_jobs",
                json!({ "workspace_id": scope.workspace_id, "page": { "limit": 50 } }),
            )
            .await?,
    )?;
    let ids: Vec<JobId> = listed.items.iter().map(|job| job.id).collect();
    assert_eq!(ids, vec![workspace_job.id]);
    let found: Job = serde_json::from_value(
        world
            .call("get_tenant_job", json!({ "job_id": tenant_job.id }))
            .await?,
    )?;
    assert_eq!((found.id, found.workspace_id), (tenant_job.id, None));
    let hidden = err_of(
        world
            .call("get_tenant_job", json!({ "job_id": workspace_job.id }))
            .await,
    )?;
    assert_eq!(hidden.code, ErrorCode::NotFound);
    let listed: ListJobsResponse = serde_json::from_value(
        world
            .call("list_tenant_jobs", json!({ "page": { "limit": 50 } }))
            .await?,
    )?;
    let ids: Vec<JobId> = listed.items.iter().map(|job| job.id).collect();
    assert_eq!(ids, vec![tenant_job.id]);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
