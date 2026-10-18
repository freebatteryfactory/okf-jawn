//! Fakes of the ports the workspace, event and connector operations call, and the service
//! composed over them with the application fixture's fakes; not production adapters. The
//! `tests/application` target includes this file as its `tenancy` module.
//!
//! Each fake encodes the sentences of its port's doc that are quoted above the method, and
//! nothing more. Where a fake is kinder than the storage adapter, its doc says so; a rule that
//! depends on that difference is proven against real storage (`crates/storage/tests`), not here.
//!
//! - `FakeCatalog` keeps workspaces in creation order with a scripted head per workspace (no
//!   repository): it refuses no purged identity and no archive while a job runs (both
//!   storage's, tested there).
//! - `FakeGrants` answers raw grants from a table that a test changes between calls, so a
//!   lookup made fresh is told from a kept one; it counts every `authorize` by permission, and
//!   fails one `authorize` or one `grant_creator` on request.
//! - `FakeJobs` is the fixture's `FakeRecords` (revision maps, receipts) with jobs: unique on
//!   the mutation, and receipts read back within their workspace. It refuses no job for a purge
//!   or an archive (storage's).
//! - `FakeQueue` keeps every wake-up and fails one on request.
//! - `FakeUploads` answers slots a test put; it reads nothing else.
//! - `FakeCredentials` issues connectors unique on the mutation with a counter for a secret
//!   (storage draws 32 random bytes), and keeps their order for listing, newest first.
//! - `FakeLog` keeps every append, except an identical notification already appended under the
//!   same mutation, and lists one log after a decimal cursor a page at a time, answering the
//!   last row's cursor while rows remain, as storage pages it.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use okf_jawn_contract::access::{Connector, IssuedConnector, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::events::{Event, ListEventsResponse, Receipt};
use okf_jawn_contract::identity::{
    ArtifactId, ConnectorId, Digest, ItemId, JobId, MutationId, PurgeId, ReceiptId, Revision,
    TenantId, Timestamp, UploadId, WorkspaceId,
};
use okf_jawn_contract::import::{Job, JobState, ListJobsResponse};
use okf_jawn_contract::purge::{Purge, PurgeTarget};
use okf_jawn_contract::review::Review;
use okf_jawn_contract::workspace::Workspace;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::application::{ApplicationService, Ports};
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::credentials::{
    ConnectorIssue, CredentialStore, InstallationIdentity, NewConnector, SessionRecord,
};
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
use okf_jawn_core::jobs::{
    ArtifactRecord, ClaimedJob, DerivedObject, JobCompletion, JobLease, JobQueue, JobScope,
    NewArtifact, NewJob, NewPurge, RecordStore, RevisionMapping,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    ByteReader, NewWorkspace, Page, Provenance, StorageScope, WorkspaceArchive, WorkspaceCatalog,
    WorkspaceUpdate, derive_workspace_id,
};
use okf_jawn_core::uploads::{NewUpload, UploadPage, UploadQuery, UploadRecord, UploadStore};
use uuid::Uuid;

use crate::fixture::{Built, FakeRecords, Unused, World, config, instant, revision};

/// The service over the tenancy fakes, and the fixture world it reuses for versions, drafts,
/// receipts and revision maps.
pub struct Tenancy {
    /// The service under test.
    pub service: ApplicationService,
    /// The fixture's fakes: versions, drafts, blobs, search, receipts and revision maps.
    pub world: World,
    /// Workspaces.
    pub catalog: Arc<FakeCatalog>,
    /// Grants.
    pub access: Arc<FakeGrants>,
    /// Jobs, receipts and revision maps.
    pub jobs: Arc<FakeJobs>,
    /// Wake-ups.
    pub queue: Arc<FakeQueue>,
    /// Upload slots.
    pub uploads: Arc<FakeUploads>,
    /// Connectors.
    pub credentials: Arc<FakeCredentials>,
    /// Both event logs.
    pub log: Arc<FakeLog>,
}

/// `WorkspaceCatalog` over workspaces in creation order.
#[derive(Default)]
pub struct FakeCatalog {
    state: Mutex<CatalogState>,
    opens: AtomicUsize,
    misname_next: AtomicBool,
}

#[derive(Default)]
struct CatalogState {
    workspaces: Vec<(TenantId, Workspace)>,
    /// What each mutation answered first: a create, update, archive or unarchive.
    results: BTreeMap<MutationId, Workspace>,
}

/// `AccessControl` over a grant table a test may change between calls.
#[derive(Default)]
pub struct FakeGrants {
    table: Mutex<GrantState>,
    lookups: Mutex<Vec<Permission>>,
    unavailable_next: AtomicBool,
    creator_fails_next: AtomicBool,
}

#[derive(Default)]
struct GrantState {
    /// Subject, then workspace, then the raw grant.
    workspaces: BTreeMap<String, BTreeMap<WorkspaceId, Vec<Permission>>>,
    /// Answer every grant for this scope instead of the one asked, as a misbehaving adapter.
    foreign: Option<StorageScope>,
}

/// `RecordStore`: the fixture's records with jobs and receipts that can be read back.
pub struct FakeJobs {
    records: Arc<FakeRecords>,
    jobs: Mutex<Vec<(MutationId, JobScope, NewJob, Job)>>,
    receipts: Mutex<Vec<(StorageScope, Receipt)>>,
    ignore_receipt_scope: AtomicBool,
    mapping_lookups: Mutex<Vec<(StorageScope, Revision)>>,
}

/// `JobQueue` that keeps every wake-up.
#[derive(Default)]
pub struct FakeQueue {
    woken: Mutex<Vec<(JobScope, JobId)>>,
    fail_next: AtomicBool,
}

/// `UploadStore` answering the slots a test put.
#[derive(Default)]
pub struct FakeUploads {
    slots: Mutex<Vec<(StorageScope, UploadRecord)>>,
}

/// `CredentialStore` for connectors.
#[derive(Default)]
pub struct FakeCredentials {
    state: Mutex<CredentialState>,
}

#[derive(Default)]
struct CredentialState {
    /// Every connector with its mutation, oldest first.
    connectors: Vec<(MutationId, Connector)>,
    /// The secret each connector holds now.
    secrets: BTreeMap<ConnectorId, String>,
    issued: usize,
    rotations: usize,
}

/// `EventLog` over both logs, listable.
#[derive(Default)]
pub struct FakeLog {
    appended: Mutex<Vec<(EventScope, Option<MutationId>, NewEvent)>>,
    lists: Mutex<Vec<(EventScope, EventQuery)>>,
    fail_next: AtomicBool,
}

impl Tenancy {
    /// The service over fresh fakes; the fixture world's head is `revision('a')`.
    ///
    /// # Errors
    /// Returns when the fixture world cannot be built.
    pub fn new() -> Built<Self> {
        let world = World::new()?;
        let catalog = Arc::new(FakeCatalog::default());
        let access = Arc::new(FakeGrants::default());
        let jobs = Arc::new(FakeJobs::new(world.records.clone()));
        let queue = Arc::new(FakeQueue::default());
        let uploads = Arc::new(FakeUploads::default());
        let credentials = Arc::new(FakeCredentials::default());
        let log = Arc::new(FakeLog::default());
        let unused = Arc::new(Unused);
        let ports = Ports {
            access: access.clone(),
            mutations: unused.clone(),
            catalog: catalog.clone(),
            blobs: world.blobs.clone(),
            versions: world.versions.clone(),
            records: jobs.clone(),
            queue: queue.clone(),
            search: world.search.clone(),
            converter: unused.clone(),
            proposals: unused.clone(),
            uploads: uploads.clone(),
            events: log.clone(),
            credentials: credentials.clone(),
            drafts: world.drafts.clone(),
            confirmations: unused,
            sandbox: world.sandbox.clone(),
            readiness: world.readiness.clone(),
        };
        let service = ApplicationService::new(ports, config())?;
        Ok(Self {
            service,
            world,
            catalog,
            access,
            jobs,
            queue,
            uploads,
            credentials,
            log,
        })
    }
}

impl FakeCatalog {
    /// Hold `workspace` in `tenant`, after every workspace held so far.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put(&self, tenant: &TenantId, workspace: Workspace) -> Built<()> {
        lock(&self.state)?
            .workspaces
            .push((tenant.clone(), workspace));
        Ok(())
    }

    /// Move the head of the workspace in `scope`, as a commit would.
    ///
    /// # Errors
    /// Returns when the workspace is not held or the lock is poisoned.
    pub fn set_head(&self, scope: &StorageScope, head: Revision) -> Built<()> {
        let mut state = lock(&self.state)?;
        let found = state
            .find_mut(scope)
            .ok_or("the catalog does not hold the workspace")?;
        found.head = head;
        Ok(())
    }

    /// Remove the workspace in `scope`, as its completed purge does.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn purge(&self, scope: &StorageScope) -> Built<()> {
        lock(&self.state)?.workspaces.retain(|(tenant, held)| {
            !(*tenant == scope.tenant_id && held.id == scope.workspace_id)
        });
        Ok(())
    }

    /// The workspace in `scope` as the catalog holds it.
    ///
    /// # Errors
    /// Returns when the workspace is not held or the lock is poisoned.
    pub fn held(&self, scope: &StorageScope) -> Built<Workspace> {
        Ok(lock(&self.state)?
            .find(scope)
            .ok_or("the catalog does not hold the workspace")?
            .clone())
    }

    /// The number of `open` calls so far.
    #[must_use]
    pub fn opens(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }

    /// Name the next new workspace with a fresh identity instead of `derive_workspace_id`, as a
    /// catalog that broke its port's rule would.
    pub fn misname_next_create(&self) {
        self.misname_next.store(true, Ordering::SeqCst);
    }
}

impl CatalogState {
    fn find(&self, scope: &StorageScope) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|(tenant, held)| *tenant == scope.tenant_id && held.id == scope.workspace_id)
            .map(|(_, held)| held)
    }

    fn find_mut(&mut self, scope: &StorageScope) -> Option<&mut Workspace> {
        self.workspaces
            .iter_mut()
            .find(|(tenant, held)| *tenant == scope.tenant_id && held.id == scope.workspace_id)
            .map(|(_, held)| held)
    }

    /// The change every metadata write makes, under its mutation: "A repeated `mutation_id`
    /// changes nothing and returns the workspace as the first call left it"; "Fails with
    /// `Conflict` when the head is not `expected_head`, with `ErrorDetail::StaleBase` naming the
    /// head and no `field`".
    fn change(
        &mut self,
        scope: &StorageScope,
        mutation: MutationId,
        expected: Option<&Revision>,
        apply: impl FnOnce(&mut Workspace) -> Result<(), ApiError>,
    ) -> Result<Workspace, ApiError> {
        if let Some(prior) = self.results.get(&mutation) {
            return Ok(prior.clone());
        }
        let held = self
            .find_mut(scope)
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such workspace"))?;
        if let Some(expected) = expected
            && held.head != *expected
        {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "the workspace head moved since it was shown",
            )
            .with_detail(ErrorDetail::StaleBase {
                head: held.head.clone(),
            }));
        }
        apply(held)?;
        let result = held.clone();
        self.results.insert(mutation, result.clone());
        Ok(result)
    }
}

impl FakeGrants {
    /// Grant `subject` the raw `permissions` on `workspace`, replacing any earlier grant.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn grant(
        &self,
        subject: &str,
        workspace: WorkspaceId,
        permissions: Vec<Permission>,
    ) -> Built<()> {
        lock(&self.table)?
            .workspaces
            .entry(subject.to_owned())
            .or_default()
            .insert(workspace, permissions);
        Ok(())
    }

    /// Answer every later grant for `scope`, whatever was asked.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn answer_for(&self, scope: StorageScope) -> Built<()> {
        lock(&self.table)?.foreign = Some(scope);
        Ok(())
    }

    /// Make the next `authorize` fail as an unreachable adapter does.
    pub fn unavailable_next(&self) {
        self.unavailable_next.store(true, Ordering::SeqCst);
    }

    /// Make the next `grant_creator` fail as an unreachable adapter does, recording nothing.
    pub fn creator_fails_next(&self) {
        self.creator_fails_next.store(true, Ordering::SeqCst);
    }

    /// The permission of every `authorize` so far, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn lookups(&self) -> Built<Vec<Permission>> {
        Ok(lock(&self.lookups)?.clone())
    }

    fn scope_for(
        state: &GrantState,
        principal: &Principal,
        workspace: WorkspaceId,
    ) -> StorageScope {
        state.foreign.clone().unwrap_or_else(|| StorageScope {
            tenant_id: principal.tenant_id.clone(),
            workspace_id: workspace,
        })
    }
}

impl FakeJobs {
    fn new(records: Arc<FakeRecords>) -> Self {
        Self {
            records,
            jobs: Mutex::new(Vec::new()),
            receipts: Mutex::new(Vec::new()),
            ignore_receipt_scope: AtomicBool::new(false),
            mapping_lookups: Mutex::new(Vec::new()),
        }
    }

    /// Hold `receipt` in `scope`, as an earlier operation recorded it.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_receipt(&self, scope: &StorageScope, receipt: Receipt) -> Built<()> {
        lock(&self.receipts)?.push((scope.clone(), receipt));
        Ok(())
    }

    /// Answer `get_receipt` from any workspace, as a store that ignored the scope would.
    pub fn ignore_receipt_scope(&self) {
        self.ignore_receipt_scope.store(true, Ordering::SeqCst);
    }

    /// Every revision-map lookup so far: the scope and the revision asked.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn mapping_lookups(&self) -> Built<Vec<(StorageScope, Revision)>> {
        Ok(lock(&self.mapping_lookups)?.clone())
    }

    /// Every job created, with its scope and specification, oldest first.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn created(&self) -> Built<Vec<(JobScope, NewJob, Job)>> {
        Ok(lock(&self.jobs)?
            .iter()
            .map(|(_, scope, job, wire)| (scope.clone(), job.clone(), wire.clone()))
            .collect())
    }
}

impl FakeQueue {
    /// Every wake-up so far.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn woken(&self) -> Built<Vec<(JobScope, JobId)>> {
        Ok(lock(&self.woken)?.clone())
    }

    /// Make the next wake-up fail.
    pub fn fail_next(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }
}

impl FakeUploads {
    /// Hold `record` in `scope`, in place of the slot of that identity there, as a slot that
    /// was completed or taken by a job reads afterwards.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put(&self, scope: &StorageScope, record: UploadRecord) -> Built<()> {
        let mut slots = lock(&self.slots)?;
        slots.retain(|(held_in, held)| !(held_in == scope && held.id == record.id));
        slots.push((scope.clone(), record));
        Ok(())
    }
}

impl FakeCredentials {
    /// Hold `connector`, issued under `mutation`, with `secret`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put(&self, mutation: MutationId, connector: Connector, secret: &str) -> Built<()> {
        let mut state = lock(&self.state)?;
        state
            .secrets
            .insert(connector.connector_id, secret.to_owned());
        state.connectors.push((mutation, connector));
        Ok(())
    }

    /// The secret `connector` holds now.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn secret(&self, connector: ConnectorId) -> Built<Option<String>> {
        Ok(lock(&self.state)?.secrets.get(&connector).cloned())
    }

    /// How many connectors were issued and how many secrets rotated.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn counts(&self) -> Built<(usize, usize)> {
        let state = lock(&self.state)?;
        Ok((state.issued, state.rotations))
    }
}

impl FakeLog {
    /// Every append, with its log and mutation, oldest first.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn appended(&self) -> Built<Vec<(EventScope, Option<MutationId>, NewEvent)>> {
        Ok(lock(&self.appended)?.clone())
    }

    /// Make the next append fail, as an unwritable log does.
    pub fn fail_next(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }

    /// Every `list` call, with its log and query.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn lists(&self) -> Built<Vec<(EventScope, EventQuery)>> {
        Ok(lock(&self.lists)?.clone())
    }
}

impl WorkspaceCatalog for FakeCatalog {
    /// "Every workspace of the tenant, in creation order; archived ones only when
    /// `include_archived`." The catalog "returns that field [`permissions`] empty".
    fn list<'a>(
        &'a self,
        tenant: &'a TenantId,
        include_archived: bool,
    ) -> PortFuture<'a, Vec<Workspace>> {
        Box::pin(async move {
            Ok(lock_api(&self.state)?
                .workspaces
                .iter()
                .filter(|(held_by, workspace)| {
                    held_by == tenant && (include_archived || workspace.archived_at.is_none())
                })
                .map(|(_, workspace)| workspace.clone())
                .collect())
        })
    }

    /// "Unique on `mutation_id`: a repeated id creates nothing and returns the prior workspace.
    /// The workspace identity is `derive_workspace_id(mutation_id)`." Its blank head is
    /// `revision('0')`.
    fn create<'a>(
        &'a self,
        tenant: &'a TenantId,
        mutation_id: MutationId,
        workspace: NewWorkspace,
    ) -> PortFuture<'a, Workspace> {
        Box::pin(async move {
            let mut state = lock_api(&self.state)?;
            if let Some(prior) = state.results.get(&mutation_id) {
                return Ok(prior.clone());
            }
            let id = if self.misname_next.swap(false, Ordering::SeqCst) {
                WorkspaceId(Uuid::new_v4())
            } else {
                derive_workspace_id(mutation_id)
            };
            let head = revision('0').map_err(internal)?;
            let created = Workspace {
                id,
                name: workspace.name,
                description: workspace.description,
                head,
                created_at: instant().map_err(internal)?,
                archived_at: None,
                permissions: Vec::new(),
            };
            state.workspaces.push((tenant.clone(), created.clone()));
            state.results.insert(mutation_id, created.clone());
            Ok(created)
        })
    }

    /// "Read one workspace, archived or not; `NotFound` when the tenant has no such
    /// workspace."
    fn open<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Workspace> {
        Box::pin(async move {
            self.opens.fetch_add(1, Ordering::SeqCst);
            lock_api(&self.state)?
                .find(scope)
                .cloned()
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such workspace"))
        })
    }

    fn update<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        update: WorkspaceUpdate,
    ) -> PortFuture<'a, Workspace> {
        Box::pin(async move {
            lock_api(&self.state)?.change(scope, mutation_id, Some(&update.expected_head), |held| {
                held.name = update.name;
                held.description = update.description;
                Ok(())
            })
        })
    }

    /// "Archive the workspace and set `archived_at`"; an archived one keeps its first instant.
    fn archive<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        archive: WorkspaceArchive,
    ) -> PortFuture<'a, Workspace> {
        Box::pin(async move {
            lock_api(&self.state)?.change(
                scope,
                mutation_id,
                Some(&archive.expected_head),
                |held| {
                    if held.archived_at.is_none() {
                        held.archived_at = Some(instant().map_err(internal)?);
                    }
                    Ok(())
                },
            )
        })
    }

    /// "Return an archived workspace to ordinary listings and clear `archived_at`. A workspace
    /// that is not archived is returned unchanged."
    fn unarchive<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        _author: Provenance,
    ) -> PortFuture<'a, Workspace> {
        Box::pin(async move {
            lock_api(&self.state)?.change(scope, mutation_id, None, |held| {
                held.archived_at = None;
                Ok(())
            })
        })
    }
}

impl AccessControl for FakeGrants {
    /// "Look up the caller's raw grant on one workspace (before route/delegation
    /// intersection)." No grant is `Forbidden`, as the local adapter answers it.
    fn authorize<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
        permission: Permission,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move {
            lock_api(&self.lookups)?.push(permission);
            if self.unavailable_next.swap(false, Ordering::SeqCst) {
                return Err(ApiError::new(
                    ErrorCode::Unavailable,
                    "the grants are offline",
                ));
            }
            let state = lock_api(&self.table)?;
            let permissions = state
                .workspaces
                .get(&principal.subject)
                .and_then(|granted| granted.get(&workspace))
                .cloned()
                .ok_or_else(|| ApiError::new(ErrorCode::Forbidden, "no grant"))?;
            Ok(WorkspaceGrant {
                scope: Self::scope_for(&state, principal, workspace),
                permissions,
            })
        })
    }

    fn authorize_tenant<'a>(
        &'a self,
        _principal: &'a Principal,
        _permission: Permission,
    ) -> PortFuture<'a, TenantGrant> {
        unused()
    }

    /// "Enumerate workspace grants for listing; results are filtered by the caller."
    fn grants<'a>(&'a self, principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        Box::pin(async move {
            let state = lock_api(&self.table)?;
            Ok(state
                .workspaces
                .get(&principal.subject)
                .map(|granted| {
                    granted
                        .iter()
                        .map(|(workspace, permissions)| WorkspaceGrant {
                            scope: Self::scope_for(&state, principal, *workspace),
                            permissions: permissions.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default())
        })
    }

    /// "Grant the creator Admin on a newly created workspace." Recorded as every permission,
    /// as the local adapter records it; recording it again changes nothing.
    fn grant_creator<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move {
            if self.creator_fails_next.swap(false, Ordering::SeqCst) {
                return Err(ApiError::new(
                    ErrorCode::Unavailable,
                    "the grants are offline",
                ));
            }
            let every = vec![
                Permission::Read,
                Permission::Write,
                Permission::Propose,
                Permission::Approve,
                Permission::Review,
                Permission::Admin,
            ];
            let mut state = lock_api(&self.table)?;
            state
                .workspaces
                .entry(principal.subject.clone())
                .or_default()
                .insert(workspace, every.clone());
            Ok(WorkspaceGrant {
                scope: Self::scope_for(&state, principal, workspace),
                permissions: every,
            })
        })
    }

    /// "Every subject whose raw grant on the workspace in `scope` includes `write` ... in any
    /// order and possibly with repeats": answered in reverse order with the first one twice.
    fn editors<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Vec<String>> {
        Box::pin(async move {
            let mut editors: Vec<String> = lock_api(&self.table)?
                .workspaces
                .iter()
                .filter(|(_, granted)| {
                    granted
                        .get(&scope.workspace_id)
                        .is_some_and(|permissions| permissions.contains(&Permission::Write))
                })
                .map(|(subject, _)| subject.clone())
                .rev()
                .collect();
            if let Some(first) = editors.first().cloned() {
                editors.push(first);
            }
            Ok(editors)
        })
    }
}

impl RecordStore for FakeJobs {
    /// "Unique on `job.mutation_id`: a repeated id inserts nothing and returns the prior job,
    /// whatever state it has reached. The returned `Job::kind` is `job.spec.kind()`, and a
    /// tenant job's `Job::workspace_id` is absent. Refuses with `Internal` a specification that
    /// does not fit the scope."
    fn create_job<'a>(&'a self, scope: &'a JobScope, job: NewJob) -> PortFuture<'a, Job> {
        Box::pin(async move {
            let mut jobs = lock_api(&self.jobs)?;
            if let Some((_, _, _, prior)) = jobs
                .iter()
                .find(|(mutation, _, _, _)| *mutation == job.mutation_id)
            {
                return Ok(prior.clone());
            }
            if !job.spec.fits(scope) {
                return Err(ApiError::new(
                    ErrorCode::Internal,
                    "a job specification does not fit the scope it was created in",
                ));
            }
            let wire = Job {
                id: JobId(Uuid::new_v4()),
                workspace_id: scope.workspace().copied(),
                kind: job.spec.kind(),
                state: JobState::Queued,
                progress: 0,
                attempt: 0,
                warnings: Vec::new(),
                error: None,
                revision: None,
                item_ids: Vec::new(),
                artifact: None,
                restore: None,
                backup: None,
                created_at: Some(instant().map_err(internal)?),
                started_at: None,
                finished_at: None,
                retryable: None,
            };
            jobs.push((job.mutation_id, scope.clone(), job, wire.clone()));
            Ok(wire)
        })
    }
    fn get_job<'a>(&'a self, _scope: &'a JobScope, _job: JobId) -> PortFuture<'a, Job> {
        unused()
    }
    fn list_jobs<'a>(
        &'a self,
        _scope: &'a JobScope,
        _page: Page,
    ) -> PortFuture<'a, ListJobsResponse> {
        unused()
    }
    fn claim_job(&self, _job: JobId) -> PortFuture<'_, Option<ClaimedJob>> {
        unused()
    }
    fn update_progress<'a>(&'a self, _lease: &'a JobLease, _progress: u8) -> PortFuture<'a, Job> {
        unused()
    }
    fn complete_job(&self, _completion: JobCompletion) -> PortFuture<'_, Job> {
        unused()
    }
    fn fail_job(
        &self,
        _lease: JobLease,
        _error: ApiError,
        _retryable: bool,
    ) -> PortFuture<'_, Job> {
        unused()
    }
    fn seal<'a>(&'a self, _lease: &'a JobLease) -> PortFuture<'a, Job> {
        unused()
    }
    fn cancel_job<'a>(
        &'a self,
        _scope: &'a JobScope,
        _mutation_id: MutationId,
        _job: JobId,
    ) -> PortFuture<'a, Job> {
        unused()
    }
    fn retry_job<'a>(
        &'a self,
        _scope: &'a JobScope,
        _mutation_id: MutationId,
        _job: JobId,
    ) -> PortFuture<'a, Job> {
        unused()
    }
    fn record_artifact<'a>(
        &'a self,
        _scope: &'a JobScope,
        _mutation_id: MutationId,
        _artifact: NewArtifact,
    ) -> PortFuture<'a, ArtifactRecord> {
        unused()
    }
    fn get_artifact<'a>(
        &'a self,
        _scope: &'a JobScope,
        _artifact: ArtifactId,
    ) -> PortFuture<'a, ArtifactRecord> {
        unused()
    }
    fn pending_jobs(&self) -> PortFuture<'_, Vec<(JobScope, JobId)>> {
        unused()
    }
    fn expire_leases(&self) -> PortFuture<'_, u32> {
        unused()
    }
    fn create_purge<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _mutation_id: MutationId,
        _purge: NewPurge,
    ) -> PortFuture<'a, Purge> {
        unused()
    }
    fn find_purge<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _target: &'a PurgeTarget,
    ) -> PortFuture<'a, Option<Purge>> {
        unused()
    }
    fn get_purge<'a>(&'a self, _tenant: &'a TenantId, _purge: PurgeId) -> PortFuture<'a, Purge> {
        unused()
    }
    fn update_purge<'a>(&'a self, _tenant: &'a TenantId, _purge: Purge) -> PortFuture<'a, Purge> {
        unused()
    }
    fn revision_mapping<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Option<RevisionMapping>> {
        Box::pin(async move {
            lock_api(&self.mapping_lookups)?.push((scope.clone(), revision.clone()));
            self.records.revision_mapping(scope, revision).await
        })
    }
    fn record_derived_object<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _object: DerivedObject,
    ) -> PortFuture<'a, DerivedObject> {
        unused()
    }
    fn derived_object<'a>(
        &'a self,
        scope: &'a StorageScope,
        item: ItemId,
        revision: &'a Revision,
        digest: &'a Digest,
    ) -> PortFuture<'a, Option<DerivedObject>> {
        self.records.derived_object(scope, item, revision, digest)
    }
    fn insert_review<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _review: Review,
    ) -> PortFuture<'a, Review> {
        unused()
    }
    fn list_reviews<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _item: ItemId,
    ) -> PortFuture<'a, Vec<Review>> {
        unused()
    }
    fn insert_receipt<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: Option<MutationId>,
        receipt: Receipt,
    ) -> PortFuture<'a, Receipt> {
        self.records.insert_receipt(scope, mutation_id, receipt)
    }
    /// "Read one durable receipt within workspace scope."
    fn get_receipt<'a>(
        &'a self,
        scope: &'a StorageScope,
        receipt: ReceiptId,
    ) -> PortFuture<'a, Receipt> {
        Box::pin(async move {
            let anywhere = self.ignore_receipt_scope.load(Ordering::SeqCst);
            lock_api(&self.receipts)?
                .iter()
                .find(|(held_in, held)| held.id == receipt && (anywhere || held_in == scope))
                .map(|(_, held)| held.clone())
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such receipt"))
        })
    }
}

impl JobQueue for FakeQueue {
    /// "Deliver an existing durable job identity, accepting possible duplicate delivery."
    fn enqueue(&self, scope: JobScope, job: JobId) -> PortFuture<'_, ()> {
        Box::pin(async move {
            if self.fail_next.swap(false, Ordering::SeqCst) {
                return Err(ApiError::new(
                    ErrorCode::Unavailable,
                    "the queue is offline",
                ));
            }
            lock_api(&self.woken)?.push((scope, job));
            Ok(())
        })
    }
}

impl UploadStore for FakeUploads {
    fn create<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _upload: NewUpload,
    ) -> PortFuture<'a, UploadRecord> {
        unused()
    }
    /// "Read one slot; `NotFound` when the workspace has no such slot."
    fn get<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
    ) -> PortFuture<'a, UploadRecord> {
        Box::pin(async move {
            lock_api(&self.slots)?
                .iter()
                .find(|(held_in, held)| held_in == scope && held.id == upload)
                .map(|(_, held)| held.clone())
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such upload"))
        })
    }
    fn list<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: UploadQuery,
    ) -> PortFuture<'a, UploadPage> {
        unused()
    }
    fn put_content<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _upload: UploadId,
        _body: ByteReader,
        _limit: u64,
    ) -> PortFuture<'a, UploadRecord> {
        unused()
    }
    fn complete<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _upload: UploadId,
        _sha256: Digest,
    ) -> PortFuture<'a, UploadRecord> {
        unused()
    }
    fn consume<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _upload: UploadId,
        _job: JobId,
    ) -> PortFuture<'a, UploadRecord> {
        unused()
    }
    fn consume_batch<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _uploads: &'a [UploadId],
        _job: JobId,
    ) -> PortFuture<'a, Vec<UploadRecord>> {
        unused()
    }
}

impl CredentialStore for FakeCredentials {
    fn installation_identity(&self) -> PortFuture<'_, InstallationIdentity> {
        unused()
    }
    /// "Unique on `mutation_id`: a repeated id issues nothing and returns
    /// `ConnectorIssue::Existing`." Permissions: `read`, and `propose` when enabled.
    fn create_connector(
        &self,
        mutation_id: MutationId,
        connector: NewConnector,
    ) -> PortFuture<'_, ConnectorIssue> {
        Box::pin(async move {
            let mut state = lock_api(&self.state)?;
            if let Some((_, existing)) = state
                .connectors
                .iter()
                .find(|(mutation, _)| *mutation == mutation_id)
            {
                return Ok(ConnectorIssue::Existing(existing.clone()));
            }
            let mut permissions = vec![Permission::Read];
            if connector.allow_propose {
                permissions.push(Permission::Propose);
            }
            let issued = Connector {
                connector_id: ConnectorId(Uuid::new_v4()),
                label: connector.label,
                workspace_ids: connector.workspace_ids,
                permissions,
                created_at: instant().map_err(internal)?,
                revoked_at: None,
            };
            state.issued = state.issued.saturating_add(1);
            let secret = format!("okfj_issued_{}", state.issued);
            state.secrets.insert(issued.connector_id, secret.clone());
            state.connectors.push((mutation_id, issued.clone()));
            Ok(ConnectorIssue::Issued(IssuedConnector {
                connector: issued,
                secret,
            }))
        })
    }
    /// "Replace a connector's secret; the previous secret stops authenticating at once."
    fn rotate_connector_secret(&self, connector: ConnectorId) -> PortFuture<'_, IssuedConnector> {
        Box::pin(async move {
            let mut state = lock_api(&self.state)?;
            let held = state
                .connectors
                .iter()
                .find(|(_, held)| held.connector_id == connector)
                .map(|(_, held)| held.clone())
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such connector"))?;
            state.rotations = state.rotations.saturating_add(1);
            let secret = format!("okfj_rotated_{}", state.rotations);
            state.secrets.insert(connector, secret.clone());
            Ok(IssuedConnector {
                connector: held,
                secret,
            })
        })
    }
    /// "List connector metadata without secrets, newest first."
    fn list_connectors(&self, include_revoked: bool) -> PortFuture<'_, Vec<Connector>> {
        Box::pin(async move {
            Ok(lock_api(&self.state)?
                .connectors
                .iter()
                .rev()
                .map(|(_, connector)| connector.clone())
                .filter(|connector| include_revoked || connector.revoked_at.is_none())
                .collect())
        })
    }
    /// "Revoke a connector immediately; repeating it changes nothing." `NotFound` for an
    /// unknown connector.
    fn revoke_connector(&self, connector: ConnectorId) -> PortFuture<'_, Connector> {
        Box::pin(async move {
            let at = instant().map_err(internal)?;
            let mut state = lock_api(&self.state)?;
            let (_, held) = state
                .connectors
                .iter_mut()
                .find(|(_, held)| held.connector_id == connector)
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such connector"))?;
            if held.revoked_at.is_none() {
                held.revoked_at = Some(at);
            }
            Ok(held.clone())
        })
    }
    fn lookup_connector(&self, _hash: [u8; 32]) -> PortFuture<'_, Option<Connector>> {
        unused()
    }
    fn insert_session(&self, _session: SessionRecord) -> PortFuture<'_, SessionRecord> {
        unused()
    }
    fn get_session<'a>(&'a self, _session_id: &'a str) -> PortFuture<'a, Option<SessionRecord>> {
        unused()
    }
    fn revoke_session<'a>(&'a self, _session_id: &'a str) -> PortFuture<'a, ()> {
        unused()
    }
}

impl EventLog for FakeLog {
    /// "With a `mutation_id`, an identical notification already appended under that mutation is
    /// not appended again and the prior event is returned ... Without one ... the notification
    /// is always appended." The cursor is the position in the fake's one sequence.
    fn append<'a>(
        &'a self,
        scope: &'a EventScope,
        mutation_id: Option<MutationId>,
        event: NewEvent,
    ) -> PortFuture<'a, Event> {
        Box::pin(async move {
            if self.fail_next.swap(false, Ordering::SeqCst) {
                return Err(ApiError::new(ErrorCode::Unavailable, "the log is offline"));
            }
            let mut appended = lock_api(&self.appended)?;
            let prior = appended.iter().position(|(held, under, notification)| {
                mutation_id.is_some()
                    && *under == mutation_id
                    && held == scope
                    && *notification == event
            });
            let position = if let Some(position) = prior {
                position
            } else {
                appended.push((scope.clone(), mutation_id, event.clone()));
                appended.len().saturating_sub(1)
            };
            wire(position, scope, event)
        })
    }

    /// "Read notifications after a cursor, oldest first, with bounded pagination." A cursor
    /// that is not a position is `InvalidInput` on `/page/cursor`, as storage refuses it.
    fn list<'a>(
        &'a self,
        scope: &'a EventScope,
        query: EventQuery,
    ) -> PortFuture<'a, ListEventsResponse> {
        Box::pin(async move {
            lock_api(&self.lists)?.push((scope.clone(), query.clone()));
            let after = match query.after.as_deref() {
                None => None,
                Some(text) => Some(text.parse::<usize>().map_err(|_| {
                    ApiError::new(ErrorCode::InvalidInput, "not an event cursor")
                        .with_field("/page/cursor")
                })?),
            };
            let limit = usize::from(query.page.limit.max(1));
            let appended = lock_api(&self.appended)?;
            let mut rows: Vec<Event> = Vec::new();
            let mut more = false;
            for (position, (held, _, event)) in appended.iter().enumerate() {
                let cursor = position.saturating_add(1);
                if held != scope || after.is_some_and(|after| cursor <= after) {
                    continue;
                }
                if rows.len() >= limit {
                    more = true;
                    break;
                }
                rows.push(wire(position, held, event.clone())?);
            }
            let next_cursor = if more {
                rows.last().map(|row| row.id.clone())
            } else {
                None
            };
            Ok(ListEventsResponse {
                events: rows,
                next_cursor,
            })
        })
    }
}

/// The wire event of the append at `position` (its cursor is `position + 1`).
fn wire(position: usize, scope: &EventScope, event: NewEvent) -> Result<Event, ApiError> {
    Ok(Event {
        id: position.saturating_add(1).to_string(),
        workspace_id: match scope {
            EventScope::Tenant(_) => None,
            EventScope::Workspace(scope) => Some(scope.workspace_id),
        },
        kind: event.kind,
        at: instant().map_err(internal)?,
        revision: event.revision,
        item_id: event.item_id,
        job_id: event.job_id,
        connector_id: event.connector_id,
        actor: event.actor,
        operation: event.operation,
        proposal_id: event.proposal_id,
        count: event.count,
    })
}

/// A workspace as the catalog holds one: no permissions.
///
/// # Errors
/// Returns when a fixture value does not parse.
pub fn workspace(id: u128, name: &str, head: char, archived: bool) -> Built<Workspace> {
    Ok(Workspace {
        id: WorkspaceId(Uuid::from_u128(id)),
        name: name.to_owned(),
        description: String::new(),
        head: revision(head)?,
        created_at: instant()?,
        archived_at: if archived {
            Some(archived_instant()?)
        } else {
            None
        },
        permissions: Vec::new(),
    })
}

/// The instant an archived fixture workspace was archived.
///
/// # Errors
/// Returns when the spelling does not parse.
pub fn archived_instant() -> Result<Timestamp, okf_jawn_contract::identity::IdentityError> {
    Timestamp::try_from("2026-10-10T08:00:00.000Z".to_owned())
}

/// Who an upload slot is supplied by in these tests.
#[must_use]
pub fn uploader() -> Provenance {
    Provenance {
        subject: "alice".to_owned(),
        route: okf_jawn_contract::access::AccessRoute::LocalOwner,
        client_id: None,
    }
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, error.to_string())
}

/// Lock a fake's mutex for a test, turning poisoning into an error.
fn lock<T>(mutex: &Mutex<T>) -> Built<MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| "a tenancy fake's lock is poisoned".into())
}

/// Lock a fake's mutex inside a port, turning poisoning into an `Internal` error.
fn lock_api<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, ApiError> {
    mutex
        .lock()
        .map_err(|_| ApiError::new(ErrorCode::Internal, "a tenancy fake's lock is poisoned"))
}

/// A future that answers that the method is not used.
fn unused<'a, T: Send + 'a>() -> PortFuture<'a, T> {
    Box::pin(async { Err(ApiError::new(ErrorCode::Internal, "not used by this test")) })
}
