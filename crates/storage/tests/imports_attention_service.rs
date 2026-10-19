//! Imports, corrections and Attention (core M1.5b) through `ApplicationService` composed over a
//! real data directory: core's dispatch, the service's own handlers, and every storage adapter.
//!
//! - An import accepted through `start_import` and its job created in the real record store,
//!   with the specification resolved at acceptance (read back through a claim, as the worker
//!   reads it); a destination spelled otherwise than a folder of the real tree refused, an
//!   incomplete upload refused, and an upload the job took refused for a second import.
//! - A correction committed through `correct_digest` and kept across the real ingest redigest
//!   handler, plus direct store-level card replacements that restore an older corrected digest.
//! - Attention over a small real workspace with one of each kind built, read whole and in pages.
//!
//! Stand-ins, said here rather than hidden: authorization is held open (`OpenAccess`), as in
//! `uploads_job_control_service.rs`. The real import/redigest journey injects a deterministic
//! converter (no native conversion is qualified here); it runs ingest's real handler/runtime
//! over durable jobs, Git and object storage. Other setup cards are direct store writes. The
//! queue records deliveries, with an injected delivery failure for the acceptance-resume window.
#![cfg(feature = "runtime")]

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use check::{TestResult, err_of, some};
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::attention::{AttentionItem, AttentionKind, GetAttentionResponse};
use okf_jawn_contract::common::{MutationResult, TextRange};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::extraction::{ConversionSettings, ConverterIdentity, OcrPolicy};
use okf_jawn_contract::identity::{
    Digest, ItemId, JobId, MutationId, Revision, TenantId, UploadId, WorkspaceId, WorkspacePath,
};
use okf_jawn_contract::import::{Job, JobKind, JobState, ListJobsResponse, Upload};
use okf_jawn_contract::item::CreatedItem;
use okf_jawn_contract::read::Selection;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::application::{ApplicationConfig, ApplicationService, Ports};
use okf_jawn_core::conformance::OkfConformance;
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, ConversionStatus, ConvertedDocument, Converter, ConverterLimits,
    PageCount,
};
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::jobs::{JobQueue, JobScope, JobSpec, RecordStore};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    CommitChanges, LocalSource, Page, Provenance, SourceCard, StorageScope, TreeEdit, VersionStore,
};
use okf_jawn_core::uploads::UploadStore;
use okf_jawn_ingest::handler::{DRAIN_BOUND, HandlerLimits, HandlerPorts, IngestHandler};
use okf_jawn_ingest::runtime::run_one;
use okf_jawn_storage::Storage;
use serde_json::{Value, json};
use uuid::Uuid;

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// The service over one opened data directory, and the queue it delivers to.
struct World {
    service: ApplicationService,
    storage: Storage,
    queue: Arc<RecordingQueue>,
}

/// Grants every permission on every workspace and on the tenant.
struct OpenAccess;

/// `JobQueue` that records every delivery; a test explicitly runs a delivered job.
#[derive(Default)]
struct RecordingQueue {
    deliveries: Mutex<Vec<(JobScope, JobId)>>,
    fail_next: AtomicBool,
}

/// Reject conversion in tests that only exercise acceptance or raw store writes.
struct NoConverter;

/// A stated converter double with literal output; the job runtime and stores remain real.
struct FixedConverter {
    markdown: &'static str,
}

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
            backups: Arc::new(storage.backups()),
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

    /// Dispatch `operation` as the installation owner in a browser session.
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

    /// Execute one actual durable ingest job with a stated converter double, then read its result.
    async fn run_job(
        &self,
        scope: &StorageScope,
        id: JobId,
        markdown: &'static str,
    ) -> Fallible<Job> {
        let handler = IngestHandler::new(HandlerPorts {
            records: Arc::new(self.storage.records()),
            versions: Arc::new(self.storage.versions()),
            search: Arc::new(self.storage.search()),
            events: Arc::new(self.storage.events()),
            backups: Arc::new(self.storage.backups()),
            purger: Arc::new(self.storage.purger()),
            uploads: Arc::new(self.storage.uploads()),
            blobs: Arc::new(self.storage.blobs()),
            converter: Arc::new(FixedConverter { markdown }),
            check: Arc::new(OkfConformance),
            limits: HandlerLimits {
                heartbeat: Duration::from_secs(1),
                window_timeout: Duration::from_secs(120),
                drain: DRAIN_BOUND,
            },
        });
        assert!(
            run_one(
                &self.storage.records(),
                &self.storage.events(),
                &handler,
                id
            )
            .await?
        );
        let job = self
            .storage
            .records()
            .get_job(&JobScope::Workspace(scope.clone()), id)
            .await?;
        assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
        Ok(job)
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
        let fail = self.fail_next.swap(false, Ordering::SeqCst);
        Box::pin(async move {
            delivered?;
            if fail {
                Err(ApiError::new(
                    ErrorCode::Unavailable,
                    "the wake-up was delivered but its acknowledgment failed",
                ))
            } else {
                Ok(())
            }
        })
    }
}

impl Converter for FixedConverter {
    fn page_count<'a>(
        &'a self,
        _source: &'a LocalSource,
        _file_name: &'a str,
    ) -> PortFuture<'a, PageCount> {
        Box::pin(async { Ok(PageCount::Whole) })
    }

    fn convert(&self, input: ConversionInput) -> PortFuture<'_, Conversion> {
        Box::pin(async move {
            let structured = input.output_directory.join("conversion.json");
            std::fs::write(&structured, b"{\"fixture\":\"deterministic converter\"}")
                .map_err(internal)?;
            Ok(Conversion {
                source_digest: input.source.object.digest,
                converter: ConverterIdentity {
                    name: "deterministic-test-converter".to_owned(),
                    version: "1".to_owned(),
                    packages: Vec::new(),
                    settings: input.settings,
                    models: Vec::new(),
                    page_window: None,
                },
                status: ConversionStatus::Success,
                document: Some(ConvertedDocument {
                    markdown: self.markdown.to_owned(),
                    structured,
                    outline: Vec::new(),
                    assets: Vec::new(),
                    locations: Vec::new(),
                    tables: Vec::new(),
                    warnings: Vec::new(),
                }),
                coverage: None,
                issues: Vec::new(),
            })
        })
    }

    fn limits(&self) -> ConverterLimits {
        ConverterLimits {
            window_pages: 1,
            memory_limit_bytes: 1024 * 1024,
        }
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

fn author() -> Provenance {
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

/// A mutation identity numbered `n`, for commits the store is given directly.
fn mutation(n: u32) -> Fallible<MutationId> {
    Ok(serde_json::from_value(json!(format!(
        "00000000-0000-4000-9000-{n:012}"
    )))?)
}

fn digest(fill: char) -> Fallible<Digest> {
    Ok(Digest::try_from(fill.to_string().repeat(64))?)
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

/// Open a slot for `filename` in `scope` through `create_upload` under the key numbered `n`,
/// and put `bytes` into it through the store, as the `upload_content` transport does.
async fn upload(
    world: &World,
    scope: &StorageScope,
    n: u32,
    filename: &str,
    bytes: &[u8],
) -> Fallible<Upload> {
    let opened: Upload = serde_json::from_value(
        world
            .call(
                "create_upload",
                json!({
                    "workspace_id": scope.workspace_id, "filename": filename,
                    "relative_path": "", "size": BYTES.len().to_string(),
                    "idempotency_key": key(n)
                }),
            )
            .await?,
    )?;
    world
        .storage
        .uploads()
        .put_content(
            scope,
            opened.id,
            Box::pin(Cursor::new(bytes.to_vec())),
            1 << 20,
        )
        .await?;
    Ok(opened)
}

/// An upload of the ten bytes, completed through `complete_upload`.
async fn complete(
    world: &World,
    scope: &StorageScope,
    n: u32,
    filename: &str,
) -> Fallible<UploadId> {
    let opened = upload(world, scope, n, filename, BYTES).await?;
    world
        .call(
            "complete_upload",
            json!({
                "workspace_id": scope.workspace_id, "upload_id": opened.id,
                "sha256": sha256(BYTES)?, "idempotency_key": key(n.saturating_add(500))
            }),
        )
        .await?;
    Ok(opened.id)
}

fn start_import(
    scope: &StorageScope,
    base: &Revision,
    uploads: &[UploadId],
    destination: &str,
    n: u32,
) -> Value {
    json!({
        "workspace_id": scope.workspace_id, "base_revision": base, "upload_ids": uploads,
        "destination": destination, "idempotency_key": key(n), "apply_naming_rules": false
    })
}

/// A note created through `create_item` under the key numbered `n`; the head it made.
async fn note(
    world: &World,
    scope: &StorageScope,
    base: &Revision,
    n: u32,
    location: &str,
    body: &str,
    properties: Value,
) -> Fallible<(ItemId, Revision)> {
    let created: CreatedItem = serde_json::from_value(
        world
            .call(
                "create_item",
                json!({
                    "workspace_id": scope.workspace_id, "base_revision": base, "path": location,
                    "title": location, "type_name": "Note", "kind": "note", "body": body,
                    "properties": properties, "idempotency_key": key(n)
                }),
            )
            .await?,
    )?;
    Ok((created.item.summary.id, created.item.summary.revision))
}

/// A source card at `location` whose original is the object `object` names and whose
/// extraction is `extraction` (JSON), showing `body`.
fn card(
    item: ItemId,
    location: &str,
    object: char,
    extraction: Value,
    body: &str,
) -> Fallible<SourceCard> {
    Ok(SourceCard {
        item_id: item,
        path: WorkspacePath::try_from(location.to_owned())?,
        title: location.to_owned(),
        type_name: "Source".to_owned(),
        body: body.to_owned(),
        properties: BTreeMap::from([(
            "description".to_owned(),
            Value::String("An imported file.".to_owned()),
        )]),
        appearance: serde_json::from_value(json!({
            "object": object.to_string().repeat(64),
            "names": [{
                "filename": "scan.pdf",
                "folder": "",
                "observed_at": "2026-10-09T12:00:00.000Z",
                "supplied_by": "ana"
            }],
            "media_type": "application/pdf",
            "size": "10",
            "metadata": {},
            "extraction": extraction
        }))?,
    })
}

/// A completed extraction whose record is the digest `record` names, with `warnings`.
fn completed(record: char, warnings: Value) -> Value {
    json!({
        "outcome": { "status": "completed" },
        "digest": record.to_string().repeat(64),
        "text_origin": "converter",
        "corrected": false,
        "warnings": warnings
    })
}

/// How many jobs the store lists in `jobs`.
async fn job_count(world: &World, jobs: &JobScope) -> Fallible<usize> {
    let page: ListJobsResponse = world
        .storage
        .records()
        .list_jobs(
            jobs,
            Page {
                cursor: None,
                limit: 50,
            },
        )
        .await?;
    Ok(page.items.len())
}

/// Write `card` on `base` under the mutation numbered `n`, as ingest's handlers write a card,
/// through the service's own conformance check; the revision it made.
async fn write_card(
    world: &World,
    scope: &StorageScope,
    base: &Revision,
    n: u32,
    card: SourceCard,
) -> Fallible<Revision> {
    Ok(world
        .storage
        .versions()
        .commit(
            scope,
            CommitChanges {
                mutation_id: mutation(n)?,
                expected_head: base.clone(),
                author: author(),
                message: "Write a source card".to_owned(),
                edits: vec![TreeEdit::WriteSourceCard(Box::new(card))],
            },
            world.service.conformance(),
        )
        .await?
        .revision)
}

#[tokio::test]
async fn imports_an_import_through_the_service_is_accepted_and_its_job_created() -> TestResult {
    let directory = tempfile::tempdir()?;
    let world = World::new(directory.path())?;
    let (scope, initial) = world.workspace().await?;
    let first = complete(&world, &scope, 2, "report.pdf").await?;
    let second = complete(&world, &scope, 3, "notes.docx").await?;
    let unfinished = upload(&world, &scope, 4, "draft.pdf", b"0123").await?;
    let folder: MutationResult = serde_json::from_value(
        world
            .call(
                "create_folder",
                json!({
                    "workspace_id": scope.workspace_id, "base_revision": initial,
                    "folder": "Reports", "idempotency_key": key(5)
                }),
            )
            .await?,
    )?;
    let (_, base) = note(
        &world,
        &scope,
        &folder.revision,
        15,
        "archive.md",
        "Archive\n",
        json!({}),
    )
    .await?;
    // SPEC section 3: an item cannot be a destination folder or an ancestor of one.
    let item_ancestor = err_of(
        world
            .call(
                "start_import",
                start_import(&scope, &base, &[first], "ARCHIVE.md/child", 16),
            )
            .await,
    )?;
    assert_eq!(item_ancestor.code, ErrorCode::InvalidInput);
    assert_eq!(item_ancestor.field.as_deref(), Some("/destination"));
    assert!(item_ancestor.message.contains("archive.md"));
    let respelled = err_of(
        world
            .call(
                "start_import",
                start_import(&scope, &base, &[first], "reports/2026", 6),
            )
            .await,
    )?;
    assert_eq!(
        respelled.code,
        ErrorCode::InvalidInput,
        "{}",
        respelled.message
    );
    assert_eq!(respelled.field.as_deref(), Some("/destination"));
    assert!(
        respelled.message.contains("Reports"),
        "{}",
        respelled.message
    );
    let incomplete = err_of(
        world
            .call(
                "start_import",
                start_import(&scope, &base, &[first, unfinished.id], "Reports", 7),
            )
            .await,
    )?;
    assert_eq!(incomplete.code, ErrorCode::InvalidInput);
    assert_eq!(incomplete.field.as_deref(), Some("/upload_ids/1"));
    let jobs = JobScope::Workspace(scope.clone());
    let records = world.storage.records();
    assert_eq!(
        job_count(&world, &jobs).await?,
        0,
        "a refused import queues nothing"
    );
    let request = start_import(&scope, &base, &[first, second], "Reports/2026", 8);
    let response = world.call("start_import", request.clone()).await?;
    let job: Job = serde_json::from_value(response.clone())?;
    assert_eq!((job.kind, job.state), (JobKind::Import, JobState::Queued));
    assert_eq!(world.deliveries()?, vec![(jobs.clone(), job.id)]);
    // A repeat of the request is answered the first job and queues nothing more.
    assert_eq!(world.call("start_import", request).await?, response);
    assert_eq!(job_count(&world, &jobs).await?, 1);
    // The worker reads the specification resolved at acceptance.
    let claimed = some(records.claim_job(job.id).await?, "the claim")?;
    assert_eq!(
        claimed.spec,
        JobSpec::Import {
            base_revision: base.clone(),
            upload_ids: vec![first, second],
            destination: Some(WorkspacePath::try_from("Reports/2026".to_owned())?),
            apply_naming_rules: false,
            settings: ConversionSettings::default(),
        }
    );
    // The job takes its uploads, as its handler does; another import of one is refused.
    world
        .storage
        .uploads()
        .consume_batch(&scope, &[first, second], job.id)
        .await?;
    let taken = err_of(
        world
            .call(
                "start_import",
                start_import(&scope, &base, &[second], "", 9),
            )
            .await,
    )?;
    assert_eq!(taken.code, ErrorCode::Conflict, "{}", taken.message);
    assert_eq!(taken.field.as_deref(), Some("/upload_ids/0"));
    assert!(
        taken.message.contains(&job.id.0.to_string()),
        "{}",
        taken.message
    );
    assert_eq!(
        job_count(&world, &jobs).await?,
        1,
        "a foreign import creates no orphan job"
    );
    Ok(())
}

/// SPEC section 5's replay guarantee across the real ledger/records/upload stores: the
/// wake-up was delivered, its acknowledgment failed, and that job consumed its upload before
/// any pending-card commit. A retry returns the same durable job, not a conflict or an orphan.
#[tokio::test]
async fn imports_a_resume_after_consumption_before_the_pending_commit_returns_its_job() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let world = World::new(directory.path())?;
    let (scope, base) = world.workspace().await?;
    let upload = complete(&world, &scope, 2, "report.pdf").await?;
    let request = start_import(&scope, &base, &[upload], "", 10);
    world.queue.fail_next.store(true, Ordering::SeqCst);
    let interrupted = err_of(world.call("start_import", request.clone()).await)?;
    assert_eq!(interrupted.code, ErrorCode::Unavailable);
    let jobs = JobScope::Workspace(scope.clone());
    let records = world.storage.records();
    let listed = records
        .list_jobs(
            &jobs,
            Page {
                cursor: None,
                limit: 10,
            },
        )
        .await?;
    let job = some(listed.items.first(), "the durable import job")?;
    assert_eq!(listed.items.len(), 1);
    // The worker claims its lease before consuming; it has not made a pending-card commit.
    some(records.claim_job(job.id).await?, "the worker's claim")?;
    assert_eq!(
        records.get_job(&jobs, job.id).await?.state,
        JobState::Running
    );
    world
        .storage
        .uploads()
        .consume_batch(&scope, &[upload], job.id)
        .await?;
    assert_eq!(world.storage.versions().head(&scope).await?, base);
    let resumed: Job = serde_json::from_value(world.call("start_import", request).await?)?;
    assert_eq!(resumed.id, job.id);
    assert_eq!(resumed.state, JobState::Running);
    assert_eq!(job_count(&world, &jobs).await?, 1);
    assert_eq!(
        world.deliveries()?,
        vec![(jobs.clone(), job.id), (jobs, job.id)]
    );
    assert_eq!(
        world
            .storage
            .uploads()
            .get(&scope, upload)
            .await?
            .consumed_by,
        Some(job.id)
    );
    Ok(())
}

/// SPEC section 5: "Re-extraction must not erase corrections." Actual import and redigest
/// jobs run through ingest over the real stores; only conversion output is injected. The
/// expected texts are literals supplied by this test, never computed by the handler.
#[tokio::test]
async fn imports_a_correction_survives_the_actual_redigest_job() -> TestResult {
    let directory = tempfile::tempdir()?;
    let world = World::new(directory.path())?;
    let (scope, base) = world.workspace().await?;
    let upload = complete(&world, &scope, 2, "scan.pdf").await?;
    let accepted: Job = serde_json::from_value(
        world
            .call(
                "start_import",
                start_import(&scope, &base, &[upload], "sources", 10),
            )
            .await?,
    )?;
    let imported = world
        .run_job(&scope, accepted.id, "Converted text.\n")
        .await?;
    let item = *some(imported.item_ids.first(), "the imported source")?;
    let imported_at = some(imported.revision.as_ref(), "the import revision")?;
    let versions = world.storage.versions();
    let source = versions.show(&scope, imported_at, item).await?;
    assert_eq!(source.body, "Converted text.\n");
    let original_digest = some(
        some(source.source.as_ref(), "the source appearance")?
            .extraction
            .digest
            .as_ref(),
        "the initial conversion digest",
    )?
    .clone();
    let corrected: MutationResult = serde_json::from_value(world.call("correct_digest", json!({
        "workspace_id": scope.workspace_id, "item_id": item, "base_revision": imported_at,
        "digest": original_digest, "corrected_markdown": "# Fixed\n", "idempotency_key": key(11)
    })).await?)?;
    assert_eq!(
        versions.show(&scope, &corrected.revision, item).await?.body,
        "# Fixed\n"
    );
    let accepted_redigest: Job = serde_json::from_value(world.call("redigest_item", json!({
        "workspace_id": scope.workspace_id, "item_id": item, "base_revision": corrected.revision,
        "settings": ConversionSettings { ocr: OcrPolicy::Skip, ..ConversionSettings::default() },
        "unconverted_only": false, "idempotency_key": key(12)
    })).await?)?;
    let redigested = world
        .run_job(&scope, accepted_redigest.id, "New conversion.\n")
        .await?;
    assert_eq!(redigested.item_ids, [item]);
    let after = some(redigested.revision.as_ref(), "the redigest revision")?;
    assert_ne!(after, &corrected.revision);
    let replacement = versions.show(&scope, after, item).await?;
    assert_eq!(replacement.body, "New conversion.\n");
    assert_ne!(
        some(replacement.source.as_ref(), "the replacement appearance")?
            .extraction
            .digest
            .as_ref(),
        Some(&original_digest)
    );
    assert_eq!(
        versions
            .correction(&scope, after, item, &original_digest)
            .await?
            .as_deref(),
        Some("# Fixed\n"),
        "the real redigest commit retains the human correction of the earlier digest"
    );
    assert_eq!(
        world
            .storage
            .uploads()
            .get(&scope, upload)
            .await?
            .consumed_by,
        Some(accepted.id)
    );
    Ok(())
}

#[tokio::test]
async fn imports_a_correction_is_committed_and_kept_across_later_conversions() -> TestResult {
    let directory = tempfile::tempdir()?;
    let world = World::new(directory.path())?;
    let (scope, initial) = world.workspace().await?;
    let item = ItemId(Uuid::from_u128(70));
    let location = "sources/scan-pdf.md";
    let converted = write_card(
        &world,
        &scope,
        &initial,
        1,
        card(
            item,
            location,
            'd',
            completed('1', json!([])),
            "Converted text.\n",
        )?,
    )
    .await?;
    let correction = |base: &Revision, of: char, n: u32| -> Fallible<Value> {
        Ok(json!({
            "workspace_id": scope.workspace_id, "item_id": item, "base_revision": base,
            "digest": digest(of)?, "corrected_markdown": "# Fixed\n",
            "idempotency_key": key(n)
        }))
    };
    let wrong = err_of(
        world
            .call("correct_digest", correction(&converted, '2', 10)?)
            .await,
    )?;
    assert_eq!(wrong.code, ErrorCode::InvalidInput, "{}", wrong.message);
    assert_eq!(wrong.field.as_deref(), Some("/digest"));
    let corrected: MutationResult = serde_json::from_value(
        world
            .call("correct_digest", correction(&converted, '1', 11)?)
            .await?,
    )?;
    let versions = world.storage.versions();
    assert_eq!(
        versions
            .correction(&scope, &corrected.revision, item, &digest('1')?)
            .await?
            .as_deref(),
        Some("# Fixed\n")
    );
    let shown = versions.show(&scope, &corrected.revision, item).await?;
    assert_eq!(shown.body, "# Fixed\n");
    assert!(
        some(shown.source.as_ref(), "the appearance")?
            .extraction
            .corrected
    );
    // A correction on the base the head has moved from is a conflict on the base.
    let stale = err_of(
        world
            .call("correct_digest", correction(&converted, '1', 12)?)
            .await,
    )?;
    assert_eq!(stale.code, ErrorCode::Conflict, "{}", stale.message);
    assert_eq!(stale.field.as_deref(), Some("/base_revision"));
    assert!(
        matches!(stale.detail.as_deref(), Some(ErrorDetail::StaleBase { head }) if *head == corrected.revision),
        "{stale:?}"
    );
    // A later conversion with other settings replaces the card with another digest: the
    // correction of the first stays beside it.
    let other = write_card(
        &world,
        &scope,
        &corrected.revision,
        2,
        card(
            item,
            location,
            'd',
            completed('2', json!([])),
            "New conversion.\n",
        )?,
    )
    .await?;
    assert_eq!(
        versions
            .correction(&scope, &other, item, &digest('1')?)
            .await?
            .as_deref(),
        Some("# Fixed\n"),
        "re-extraction keeps the correction"
    );
    let replaced = versions.show(&scope, &other, item).await?;
    assert_eq!(replaced.body, "New conversion.\n");
    assert!(
        !some(replaced.source.as_ref(), "the appearance")?
            .extraction
            .corrected
    );
    // Converted again to the corrected digest, the card shows the correction once more.
    let again = write_card(
        &world,
        &scope,
        &other,
        3,
        card(
            item,
            location,
            'd',
            completed('1', json!([])),
            "Converted text.\n",
        )?,
    )
    .await?;
    let restored = versions.show(&scope, &again, item).await?;
    assert_eq!(restored.body, "# Fixed\n");
    assert!(
        some(restored.source.as_ref(), "the appearance")?
            .extraction
            .corrected
    );
    // A redigest of the card is accepted on its base whatever the head is now (ruling R3).
    let job: Job = serde_json::from_value(
        world
            .call(
                "redigest_item",
                json!({
                    "workspace_id": scope.workspace_id, "item_id": item,
                    "base_revision": converted, "settings": ConversionSettings::default(),
                    "unconverted_only": false, "idempotency_key": key(13)
                }),
            )
            .await?,
    )?;
    assert_eq!(job.kind, JobKind::Redigest);
    Ok(())
}

/// The rows of `kind`, by the item they name.
fn named<'r>(rows: &'r [AttentionItem], kind: &AttentionKind) -> Vec<&'r AttentionItem> {
    rows.iter().filter(|row| row.kind == *kind).collect()
}

#[tokio::test]
async fn attention_over_a_small_real_workspace_shows_one_of_each_kind_built() -> TestResult {
    let directory = tempfile::tempdir()?;
    let world = World::new(directory.path())?;
    let (scope, initial) = world.workspace().await?;
    let described = |extra: Value| -> Value {
        let mut properties = json!({ "description": "A note." });
        if let (Some(fields), Some(more)) = (properties.as_object_mut(), extra.as_object()) {
            fields.extend(more.clone());
        }
        properties
    };
    let (broken, head) = note(
        &world,
        &scope,
        &initial,
        2,
        "notes/broken.md",
        "See [the plan](plan.md).\n",
        described(json!({})),
    )
    .await?;
    let (stale, head) = note(
        &world,
        &scope,
        &head,
        3,
        "notes/stale.md",
        "Check me.\n",
        described(json!({ "stale_after": "2026-01-01T00:00:00Z" })),
    )
    .await?;
    let (generated, head) = note(
        &world,
        &scope,
        &head,
        4,
        "notes/generated.md",
        "An agent wrote this.\n",
        described(json!({
            "generated": { "by": "reference_agent/gemini-2.5-pro", "at": "2026-10-01T00:00:00Z" }
        })),
    )
    .await?;
    let (bare, head) = note(
        &world,
        &scope,
        &head,
        5,
        "notes/bare.md",
        "Nothing said about it.\n",
        json!({}),
    )
    .await?;
    let first = ItemId(Uuid::from_u128(80));
    let second = ItemId(Uuid::from_u128(81));
    let scan = ItemId(Uuid::from_u128(82));
    let warned = ItemId(Uuid::from_u128(83));
    let mut head = head;
    for (n, card) in [
        (
            10,
            card(
                first,
                "sources/a-pdf.md",
                'd',
                completed('1', json!([])),
                "A.\n",
            )?,
        ),
        (
            11,
            card(
                second,
                "sources/b-pdf.md",
                'd',
                completed('2', json!([])),
                "B.\n",
            )?,
        ),
        (
            12,
            card(
                scan,
                "sources/scan-pdf.md",
                'e',
                json!({
                    "outcome": { "status": "unsupported" }, "text_origin": "none",
                    "corrected": false, "warnings": []
                }),
                "",
            )?,
        ),
        (
            13,
            card(
                warned,
                "sources/warned-pdf.md",
                'f',
                completed('3', json!([{ "kind": "unlocated_items", "items": 2 }])),
                "W.\n",
            )?,
        ),
    ] {
        head = write_card(&world, &scope, &head, n, card).await?;
    }
    complete(&world, &scope, 20, "waiting.pdf").await?;
    let whole: GetAttentionResponse = serde_json::from_value(
        world
            .call(
                "get_attention",
                json!({
                    "workspace_id": scope.workspace_id, "at": { "kind": "latest" },
                    "page": { "limit": 200 }
                }),
            )
            .await?,
    )?;
    assert_eq!(whole.revision, head);
    assert_eq!(whole.next_cursor, None);
    let kinds: BTreeSet<String> = whole
        .items
        .iter()
        .map(|row| serde_json::to_value(&row.kind).map(|kind| kind.to_string()))
        .collect::<Result<_, _>>()?;
    let built: BTreeSet<String> = [
        "broken_link",
        "stale",
        "unreviewed",
        "duplicate",
        "missing_description",
        "unprocessed",
        "extraction_warnings",
        "uploads_waiting",
    ]
    .into_iter()
    .map(|kind| format!("\"{kind}\""))
    .collect();
    assert_eq!(kinds, built);
    let rows = &whole.items;
    let link = named(rows, &AttentionKind::BrokenLink);
    let [link] = link.as_slice() else {
        return Err(format!("one broken-link row: {rows:?}").into());
    };
    assert_eq!(link.item_id, Some(broken));
    assert_eq!(
        link.selection,
        Some(Selection::Lines {
            range: TextRange { start: 1, end: 1 }
        })
    );
    let ids = |kind: &AttentionKind| -> Vec<Option<ItemId>> {
        named(rows, kind).iter().map(|row| row.item_id).collect()
    };
    assert_eq!(ids(&AttentionKind::Stale), vec![Some(stale)]);
    assert_eq!(ids(&AttentionKind::Unreviewed), vec![Some(generated)]);
    assert_eq!(
        ids(&AttentionKind::Duplicate),
        vec![Some(first), Some(second)]
    );
    assert_eq!(ids(&AttentionKind::MissingDescription), vec![Some(bare)]);
    assert_eq!(ids(&AttentionKind::Unprocessed), vec![Some(scan)]);
    assert_eq!(ids(&AttentionKind::ExtractionWarnings), vec![Some(warned)]);
    let waiting = named(rows, &AttentionKind::UploadsWaiting);
    let [waiting] = waiting.as_slice() else {
        return Err("one uploads-waiting row".into());
    };
    assert!(waiting.message.contains("10 bytes"), "{}", waiting.message);
    // Pages of three follow the cursor through the same rows once.
    let mut paged: Vec<(String, Option<ItemId>)> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut request = json!({
            "workspace_id": scope.workspace_id, "at": { "kind": "latest" },
            "page": { "limit": 3 }
        });
        if let (Some(page), Some(next)) = (request.get_mut("page"), cursor.take())
            && let Some(fields) = page.as_object_mut()
        {
            fields.insert("cursor".to_owned(), Value::String(next));
        }
        let page: GetAttentionResponse =
            serde_json::from_value(world.call("get_attention", request).await?)?;
        for row in page.items {
            paged.push((serde_json::to_value(&row.kind)?.to_string(), row.item_id));
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    let expected: Vec<(String, Option<ItemId>)> = whole
        .items
        .iter()
        .map(|row| Ok((serde_json::to_value(&row.kind)?.to_string(), row.item_id)))
        .collect::<Result<_, serde_json::Error>>()?;
    assert_eq!(paged, expected);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
