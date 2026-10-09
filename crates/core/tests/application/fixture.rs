//! Fakes of every port `ApplicationService` is built over, for handler tests; not production
//! adapters.
//!
//! Each fake answers what its port documents and nothing more: the version store commits only
//! on its expected head, replays a repeated mutation, and runs the check it is handed on a
//! staged directory; the draft store keeps one draft per (item, editor); the record store
//! answers revision maps and derived objects for exactly the (item, revision, digest) recorded.
//! Content is scripted by the test (`put_document`, `put_listing`), never computed from edits,
//! so what a test asserts is what the application did with the ports. A port the operations
//! under test do not call answers `Internal` ("not used by this test").

use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use okf_jawn_contract::access::{AccessRoute, Connector, IssuedConnector, Principal};
use okf_jawn_contract::common::{PageRange, TextRange};
use okf_jawn_contract::conventions::NamingRules;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{Event, ListEventsResponse, Receipt};
use okf_jawn_contract::extraction::{
    ConversionOutcome, ConversionSettings, ConverterIdentity, Extraction, TextOrigin,
};
use okf_jawn_contract::health::ReadinessResponse;
use okf_jawn_contract::history::{BlameResponse, DiffResponse, LogResponse};
use okf_jawn_contract::identity::{
    ArtifactId, ConnectorId, Digest, ItemId, JobId, MutationId, ProposalId, PurgeId, ReceiptId,
    Revision, TenantId, Timestamp, UploadId, WorkspaceId, WorkspacePath,
};
use okf_jawn_contract::import::{Job, ListJobsResponse};
use okf_jawn_contract::item::{
    Draft, DraftContent, ItemDocument, ItemKind, ItemStatus, ItemSummary, TypeDefinition,
};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::proposal::{Comment, ListProposalsResponse, Proposal};
use okf_jawn_contract::purge::Purge;
use okf_jawn_contract::read::{AssetRole, OutlineEntry, OutlineEntryKind, Selection};
use okf_jawn_contract::review::{Confirmation, Review};
use okf_jawn_contract::source::{SourceAppearance, SourceLocation, SourceLocator};
use okf_jawn_contract::workspace::Workspace;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::application::{ApplicationConfig, ApplicationService, Ports};
use okf_jawn_core::confirmations::{ConfirmationConsume, ConfirmationCreate, ConfirmationStore};
use okf_jawn_core::context::{Attempt, OperationContext, TenantGrant, WorkspaceGrant};
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, ConversionRecord, Converter, ConverterLimits, LineLocation,
    PixelSize, RetainedAsset, WindowExport,
};
use okf_jawn_core::credentials::{
    ConnectorIssue, CredentialStore, InstallationIdentity, NewConnector, SessionRecord,
};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
use okf_jawn_core::jobs::{
    ArtifactRecord, ClaimedJob, DerivedObject, JobCompletion, JobLease, JobQueue, JobScope,
    NewArtifact, NewJob, NewPurge, RecordStore, RevisionMapping,
};
use okf_jawn_core::mutations::{BeginOutcome, MutationKey, MutationLease, MutationStore};
use okf_jawn_core::portable::item_content_digest;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::proposals::{CommentPage, ProposalFilter, ProposalStore};
use okf_jawn_core::readiness::ReadinessProbe;
use okf_jawn_core::sandbox::{SandboxCapabilityStore, SandboxMint, SandboxResolved};
use okf_jawn_core::search::{GraphQuery, LinkQuery, SearchIndex, SearchQuery};
use okf_jawn_core::storage::{
    BlameQuery, BlobStore, ByteReader, CandidateChanges, CandidateCheck, CommitChanges, Committed,
    DiffQuery, FolderListing, LocalSource, LogQuery, NewWorkspace, ObjectInfo, ObjectRead, Page,
    Promotion, Provenance, StorageScope, VersionStore, WorkspaceArchive, WorkspaceCatalog,
    WorkspaceUpdate,
};
use okf_jawn_core::uploads::{NewUpload, UploadRecord, UploadStore};
use serde_json::Value;
use uuid::Uuid;

/// Result of a fixture builder.
pub type Built<T> = Result<T, Box<dyn Error>>;

/// The composed service and the fakes behind it.
pub struct World {
    /// The service under test.
    pub service: ApplicationService,
    /// Versioned content.
    pub versions: Arc<FakeVersions>,
    /// Retained bytes.
    pub blobs: Arc<FakeBlobs>,
    /// Revision maps, derived objects and receipts.
    pub records: Arc<FakeRecords>,
    /// Indexed revisions.
    pub search: Arc<FakeSearch>,
    /// Appended events.
    pub events: Arc<FakeEvents>,
    /// Per-editor drafts.
    pub drafts: Arc<FakeDrafts>,
    /// Minted sandbox capabilities.
    pub sandbox: Arc<FakeSandbox>,
}

/// A staged candidate directory, removed when dropped.
pub struct Staged(pub PathBuf);

/// `VersionStore` over scripted documents and listings.
pub struct FakeVersions {
    state: Mutex<VersionState>,
    stage: Staged,
    shows: AtomicUsize,
}

#[derive(Default)]
struct VersionState {
    head: Option<Revision>,
    documents: Vec<(Revision, ItemDocument)>,
    listings: Vec<(Revision, Option<WorkspacePath>, FolderListing)>,
    types: Vec<TypeDefinition>,
    queued: Vec<Revision>,
    commits: Vec<(CommitChanges, Revision)>,
}

/// `BlobStore` over a map of digests to bytes.
#[derive(Default)]
pub struct FakeBlobs {
    objects: Mutex<BTreeMap<Digest, Vec<u8>>>,
    opens: Mutex<Vec<(Digest, u64, u64)>>,
}

/// `RecordStore` answering revision maps and derived objects, and keeping receipts.
#[derive(Default)]
pub struct FakeRecords {
    mappings: Mutex<BTreeMap<Revision, RevisionMapping>>,
    derived: Mutex<Vec<DerivedObject>>,
    receipts: Mutex<Vec<(Option<MutationId>, Receipt)>>,
}

/// `SearchIndex` that records the revisions it was asked to index.
#[derive(Default)]
pub struct FakeSearch {
    indexed: Mutex<Vec<Revision>>,
}

/// `EventLog` that keeps every append.
#[derive(Default)]
pub struct FakeEvents {
    appended: Mutex<Vec<(EventScope, Option<MutationId>, NewEvent)>>,
}

/// `DraftStore` with one draft per (item, editor).
#[derive(Default)]
pub struct FakeDrafts {
    drafts: Mutex<Vec<DraftContent>>,
    saves: Mutex<Vec<(MutationId, DraftWrite)>>,
    discarded: Mutex<Vec<(MutationId, Draft)>>,
    gets: AtomicUsize,
}

/// `SandboxCapabilityStore` that keeps every mint.
#[derive(Default)]
pub struct FakeSandbox {
    mints: Mutex<Vec<([u8; 32], SandboxMint)>>,
}

/// Every other port: not used by the operations under test.
pub struct Unused;

/// The fixture workspace.
pub const WORKSPACE: u128 = 1;
/// The fixture note.
pub const NOTE: u128 = 20;
/// The editor every fixture context acts for unless a test says otherwise.
pub const ALICE: &str = "alice";
/// The shown text the fixture record describes: lines 1 to 3 on page 1 under "Overview", a
/// blank line 4, and a table on lines 5 to 8 of page 2 under "Revenue".
pub const CARD_BODY: &str =
    "# Overview\nfirst\nsecond\n\n# Revenue\n| Quarter | Revenue |\n|---|---|\n| Q1 | 1200.5 |\n";

/// The sandbox origin the fixture service is configured with.
pub const SANDBOX_ORIGIN: &str = "https://sandbox.example.test";

impl World {
    /// A service over fresh fakes whose head is `revision('a')`, with a conformant staged tree.
    ///
    /// # Errors
    /// Returns when the staged directory cannot be written or a fixture value does not parse.
    pub fn new() -> Built<Self> {
        let versions = Arc::new(FakeVersions::new()?);
        let blobs = Arc::new(FakeBlobs::default());
        let records = Arc::new(FakeRecords::default());
        let search = Arc::new(FakeSearch::default());
        let events = Arc::new(FakeEvents::default());
        let drafts = Arc::new(FakeDrafts::default());
        let sandbox = Arc::new(FakeSandbox::default());
        let unused = Arc::new(Unused);
        let ports = Ports {
            access: unused.clone(),
            mutations: unused.clone(),
            catalog: unused.clone(),
            blobs: blobs.clone(),
            versions: versions.clone(),
            records: records.clone(),
            queue: unused.clone(),
            search: search.clone(),
            converter: unused.clone(),
            proposals: unused.clone(),
            uploads: unused.clone(),
            events: events.clone(),
            credentials: unused.clone(),
            drafts: drafts.clone(),
            confirmations: unused.clone(),
            sandbox: sandbox.clone(),
            readiness: unused,
        };
        let service = ApplicationService::new(ports, config())?;
        Ok(Self {
            service,
            versions,
            blobs,
            records,
            search,
            events,
            drafts,
            sandbox,
        })
    }
}

impl Staged {
    /// A directory holding one conformant note.
    ///
    /// # Errors
    /// Returns when the directory cannot be written.
    pub fn conformant() -> Built<Self> {
        let root = std::env::temp_dir().join(format!("okf-jawn-application-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("notes"))?;
        let staged = Self(root);
        staged.write(
            "notes/plan.md",
            "---\ntype: Note\ntitle: Plan\ndescription: The plan.\n---\n# Plan\n",
        )?;
        Ok(staged)
    }

    /// Write one file of the staged tree.
    ///
    /// # Errors
    /// Returns when the file cannot be written.
    pub fn write(&self, path: &str, text: &str) -> Built<()> {
        std::fs::write(self.0.join(path), text)?;
        Ok(())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        // Best effort: a leftover directory under the temporary directory harms nothing.
        let _left = std::fs::remove_dir_all(&self.0);
    }
}

impl FakeVersions {
    fn new() -> Built<Self> {
        Ok(Self {
            state: Mutex::new(VersionState {
                head: Some(revision('a')?),
                ..VersionState::default()
            }),
            stage: Staged::conformant()?,
            shows: AtomicUsize::new(0),
        })
    }

    /// The staged tree every commit's check runs on.
    #[must_use]
    pub const fn stage(&self) -> &Staged {
        &self.stage
    }

    /// Make `document` readable at `at`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_document(&self, at: &Revision, document: ItemDocument) -> Built<()> {
        lock(&self.state)?.documents.push((at.clone(), document));
        Ok(())
    }

    /// Answer `list` of `folder` at `at` with `listing`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_listing(
        &self,
        at: &Revision,
        folder: Option<WorkspacePath>,
        listing: FolderListing,
    ) -> Built<()> {
        lock(&self.state)?
            .listings
            .push((at.clone(), folder, listing));
        Ok(())
    }

    /// Answer `types` with `types`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_types(&self, types: Vec<TypeDefinition>) -> Built<()> {
        lock(&self.state)?.types = types;
        Ok(())
    }

    /// The revision the next new commit produces.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn queue_revision(&self, revision: Revision) -> Built<()> {
        lock(&self.state)?.queued.push(revision);
        Ok(())
    }

    /// Move the head, as a write by someone else would.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn set_head(&self, revision: Revision) -> Built<()> {
        lock(&self.state)?.head = Some(revision);
        Ok(())
    }

    /// Every commit that wrote, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn commits(&self) -> Built<Vec<CommitChanges>> {
        Ok(lock(&self.state)?
            .commits
            .iter()
            .map(|(changes, _)| changes.clone())
            .collect())
    }

    /// How many times `show` was called.
    #[must_use]
    pub fn shows(&self) -> usize {
        self.shows.load(Ordering::SeqCst)
    }

    fn head_now(&self) -> Result<Revision, ApiError> {
        lock_api(&self.state)?
            .head
            .clone()
            .ok_or_else(|| ApiError::new(ErrorCode::Internal, "no head"))
    }
}

impl FakeBlobs {
    /// Retain `bytes` under `digest`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_object(&self, digest: Digest, bytes: Vec<u8>) -> Built<()> {
        lock(&self.objects)?.insert(digest, bytes);
        Ok(())
    }

    /// Every `open` as (digest, offset, length), in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn opens(&self) -> Built<Vec<(Digest, u64, u64)>> {
        Ok(lock(&self.opens)?.clone())
    }
}

impl FakeRecords {
    /// Record that a purge rewrote or removed `revision`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_mapping(&self, revision: Revision, mapping: RevisionMapping) -> Built<()> {
        lock(&self.mappings)?.insert(revision, mapping);
        Ok(())
    }

    /// Record a derived object.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_derived(&self, object: DerivedObject) -> Built<()> {
        lock(&self.derived)?.push(object);
        Ok(())
    }

    /// Every receipt inserted, with the mutation it was inserted under.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn receipts(&self) -> Built<Vec<(Option<MutationId>, Receipt)>> {
        Ok(lock(&self.receipts)?.clone())
    }
}

impl FakeSearch {
    /// Every revision indexed, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn indexed(&self) -> Built<Vec<Revision>> {
        Ok(lock(&self.indexed)?.clone())
    }
}

impl FakeEvents {
    /// Every append, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn appended(&self) -> Built<Vec<(EventScope, Option<MutationId>, NewEvent)>> {
        Ok(lock(&self.appended)?.clone())
    }
}

impl FakeDrafts {
    /// Hold `content` as its editor's draft of its item.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_draft(&self, content: DraftContent) -> Built<()> {
        lock(&self.drafts)?.push(content);
        Ok(())
    }

    /// Every save that wrote, with its mutation.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn saves(&self) -> Built<Vec<(MutationId, DraftWrite)>> {
        Ok(lock(&self.saves)?.clone())
    }

    /// How many times `get` was called.
    #[must_use]
    pub fn gets(&self) -> usize {
        self.gets.load(Ordering::SeqCst)
    }
}

impl FakeSandbox {
    /// Every mint, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn mints(&self) -> Built<Vec<([u8; 32], SandboxMint)>> {
        Ok(lock(&self.mints)?.clone())
    }
}

impl VersionStore for FakeVersions {
    fn head<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Revision> {
        Box::pin(async move { self.head_now() })
    }
    fn list<'a>(
        &'a self,
        _scope: &'a StorageScope,
        revision: &'a Revision,
        folder: Option<&'a WorkspacePath>,
        _page: Page,
    ) -> PortFuture<'a, FolderListing> {
        Box::pin(async move {
            lock_api(&self.state)?
                .listings
                .iter()
                .find(|(at, listed, _)| at == revision && listed.as_ref() == folder)
                .map(|(_, _, listing)| listing.clone())
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such folder"))
        })
    }
    fn show<'a>(
        &'a self,
        _scope: &'a StorageScope,
        revision: &'a Revision,
        item: ItemId,
    ) -> PortFuture<'a, ItemDocument> {
        self.shows.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            lock_api(&self.state)?
                .documents
                .iter()
                .find(|(at, document)| at == revision && document.summary.id == item)
                .map(|(_, document)| document.clone())
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such item"))
        })
    }
    fn read_file<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _path: &'a WorkspacePath,
    ) -> PortFuture<'a, Vec<u8>> {
        unused()
    }
    fn rules<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
    ) -> PortFuture<'a, Option<NamingRules>> {
        unused()
    }
    fn types<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
    ) -> PortFuture<'a, Vec<TypeDefinition>> {
        Box::pin(async move { Ok(lock_api(&self.state)?.types.clone()) })
    }
    fn correction<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _item: ItemId,
        _digest: &'a Digest,
    ) -> PortFuture<'a, Option<String>> {
        unused()
    }
    fn commit<'a>(
        &'a self,
        _scope: &'a StorageScope,
        changes: CommitChanges,
        check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed> {
        Box::pin(async move {
            let mut state = lock_api(&self.state)?;
            if let Some((_, revision)) = state
                .commits
                .iter()
                .find(|(prior, _)| prior.mutation_id == changes.mutation_id)
            {
                return Ok(Committed {
                    revision: revision.clone(),
                    replayed: true,
                    warnings: Vec::new(),
                });
            }
            if state.head.as_ref() != Some(&changes.expected_head) {
                return Err(ApiError::new(ErrorCode::Conflict, "the head moved"));
            }
            let warnings = check.check(&self.stage.0)?;
            if state.queued.is_empty() {
                return Err(ApiError::new(ErrorCode::Internal, "no revision queued"));
            }
            let revision = state.queued.remove(0);
            state.head = Some(revision.clone());
            state.commits.push((changes, revision.clone()));
            Ok(Committed {
                revision,
                replayed: false,
                warnings,
            })
        })
    }
    fn find_commit<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _since: &'a Revision,
    ) -> PortFuture<'a, Option<Revision>> {
        unused()
    }
    fn create_candidate<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _proposal_id: ProposalId,
        _changes: CandidateChanges,
        _check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Revision> {
        unused()
    }
    fn promote_candidate<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _promotion: Promotion,
    ) -> PortFuture<'a, Committed> {
        unused()
    }
    fn log<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: LogQuery,
    ) -> PortFuture<'a, LogResponse> {
        unused()
    }
    fn diff<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: DiffQuery,
    ) -> PortFuture<'a, DiffResponse> {
        unused()
    }
    fn blame<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: BlameQuery,
    ) -> PortFuture<'a, BlameResponse> {
        unused()
    }
    fn write_history<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo> {
        unused()
    }
}

impl BlobStore for FakeBlobs {
    fn put<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _body: ByteReader,
        _limit: u64,
        _expected: Option<Digest>,
    ) -> PortFuture<'a, ObjectInfo> {
        unused()
    }
    fn open<'a>(
        &'a self,
        _scope: &'a StorageScope,
        digest: &'a Digest,
        offset: u64,
        length: u64,
    ) -> PortFuture<'a, ObjectRead> {
        Box::pin(async move {
            lock_api(&self.opens)?.push((digest.clone(), offset, length));
            let bytes = lock_api(&self.objects)?
                .get(digest)
                .cloned()
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such object"))?;
            let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            let start = usize::try_from(offset.min(size)).unwrap_or(usize::MAX);
            let end =
                usize::try_from(offset.saturating_add(length).min(size)).unwrap_or(usize::MAX);
            let interval = bytes
                .get(start..end)
                .map(<[u8]>::to_vec)
                .unwrap_or_default();
            Ok(ObjectRead {
                object: ObjectInfo {
                    digest: digest.clone(),
                    size,
                },
                offset,
                length,
                body: Box::pin(std::io::Cursor::new(interval)),
            })
        })
    }
    fn materialize<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _digest: &'a Digest,
    ) -> PortFuture<'a, LocalSource> {
        unused()
    }
}

impl RecordStore for FakeRecords {
    fn create_job<'a>(&'a self, _scope: &'a JobScope, _job: NewJob) -> PortFuture<'a, Job> {
        unused()
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
        _message: String,
        _retryable: bool,
    ) -> PortFuture<'_, Job> {
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
    fn get_purge<'a>(&'a self, _tenant: &'a TenantId, _purge: PurgeId) -> PortFuture<'a, Purge> {
        unused()
    }
    fn update_purge<'a>(&'a self, _tenant: &'a TenantId, _purge: Purge) -> PortFuture<'a, Purge> {
        unused()
    }
    fn revision_mapping<'a>(
        &'a self,
        _scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Option<RevisionMapping>> {
        Box::pin(async move { Ok(lock_api(&self.mappings)?.get(revision).cloned()) })
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
        _scope: &'a StorageScope,
        item: ItemId,
        revision: &'a Revision,
        digest: &'a Digest,
    ) -> PortFuture<'a, Option<DerivedObject>> {
        Box::pin(async move {
            Ok(lock_api(&self.derived)?
                .iter()
                .find(|object| {
                    object.item_id == item
                        && &object.revision == revision
                        && &object.digest == digest
                })
                .cloned())
        })
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
        _scope: &'a StorageScope,
        mutation_id: Option<MutationId>,
        receipt: Receipt,
    ) -> PortFuture<'a, Receipt> {
        Box::pin(async move {
            let mut receipts = lock_api(&self.receipts)?;
            if let Some(prior) = receipts
                .iter()
                .find(|(prior, _)| mutation_id.is_some() && *prior == mutation_id)
            {
                return Ok(prior.1.clone());
            }
            receipts.push((mutation_id, receipt.clone()));
            Ok(receipt)
        })
    }
    fn get_receipt<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _receipt: ReceiptId,
    ) -> PortFuture<'a, Receipt> {
        unused()
    }
}

impl SearchIndex for FakeSearch {
    fn index_revision<'a>(
        &'a self,
        _scope: &'a StorageScope,
        revision: Revision,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            lock_api(&self.indexed)?.push(revision);
            Ok(())
        })
    }
    fn search<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: SearchQuery,
    ) -> PortFuture<'a, okf_jawn_contract::search::SearchResponse> {
        unused()
    }
    fn links<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: LinkQuery,
    ) -> PortFuture<'a, okf_jawn_contract::search::GetLinksResponse> {
        unused()
    }
    fn graph<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: GraphQuery,
    ) -> PortFuture<'a, okf_jawn_contract::search::GetGraphResponse> {
        unused()
    }
    fn rebuild<'a>(&'a self, _scope: &'a StorageScope, _head: Revision) -> PortFuture<'a, ()> {
        unused()
    }
}

impl EventLog for FakeEvents {
    fn append<'a>(
        &'a self,
        scope: &'a EventScope,
        mutation_id: Option<MutationId>,
        event: NewEvent,
    ) -> PortFuture<'a, Event> {
        Box::pin(async move {
            let mut appended = lock_api(&self.appended)?;
            appended.push((scope.clone(), mutation_id, event.clone()));
            Ok(Event {
                id: appended.len().to_string(),
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
            })
        })
    }
    fn list<'a>(
        &'a self,
        _scope: &'a EventScope,
        _query: EventQuery,
    ) -> PortFuture<'a, ListEventsResponse> {
        unused()
    }
}

impl DraftStore for FakeDrafts {
    fn save<'a>(
        &'a self,
        _scope: &'a StorageScope,
        mutation_id: MutationId,
        draft: DraftWrite,
    ) -> PortFuture<'a, Draft> {
        Box::pin(async move {
            let mut saves = lock_api(&self.saves)?;
            if let Some((_, prior)) = saves
                .iter()
                .find(|(prior, write)| *prior == mutation_id && write.item_id == draft.item_id)
            {
                return metadata(prior).map_err(internal);
            }
            let stored_draft = metadata(&draft).map_err(internal)?;
            let mut drafts = lock_api(&self.drafts)?;
            drafts.retain(|held| {
                !(held.draft.item_id == draft.item_id && held.draft.editor == draft.editor)
            });
            drafts.insert(
                0,
                DraftContent {
                    draft: stored_draft.clone(),
                    body: draft.body.clone(),
                    properties: draft.properties.clone(),
                },
            );
            saves.push((mutation_id, draft));
            Ok(stored_draft)
        })
    }
    fn get<'a>(
        &'a self,
        _scope: &'a StorageScope,
        item: ItemId,
        editor: &'a str,
    ) -> PortFuture<'a, Option<DraftContent>> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(lock_api(&self.drafts)?
                .iter()
                .find(|held| held.draft.item_id == item && held.draft.editor == editor)
                .cloned())
        })
    }
    fn list<'a>(&'a self, _scope: &'a StorageScope, editor: &'a str) -> PortFuture<'a, Vec<Draft>> {
        Box::pin(async move {
            Ok(lock_api(&self.drafts)?
                .iter()
                .filter(|held| held.draft.editor == editor)
                .map(|held| held.draft.clone())
                .collect())
        })
    }
    fn discard<'a>(
        &'a self,
        _scope: &'a StorageScope,
        mutation_id: MutationId,
        item: ItemId,
        editor: &'a str,
    ) -> PortFuture<'a, Draft> {
        Box::pin(async move {
            let mut discarded = lock_api(&self.discarded)?;
            if let Some((_, prior)) = discarded
                .iter()
                .find(|(prior, draft)| *prior == mutation_id && draft.item_id == item)
            {
                return Ok(prior.clone());
            }
            let mut drafts = lock_api(&self.drafts)?;
            let position = drafts
                .iter()
                .position(|held| held.draft.item_id == item && held.draft.editor == editor)
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no draft"))?;
            let removed = drafts.remove(position).draft;
            discarded.push((mutation_id, removed.clone()));
            Ok(removed)
        })
    }
}

impl SandboxCapabilityStore for FakeSandbox {
    fn mint<'a>(
        &'a self,
        _scope: &'a StorageScope,
        hash: [u8; 32],
        mint: SandboxMint,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            lock_api(&self.mints)?.push((hash, mint));
            Ok(())
        })
    }
    fn resolve(&self, _hash: [u8; 32]) -> PortFuture<'_, Option<SandboxResolved>> {
        unused()
    }
}

impl AccessControl for Unused {
    fn authorize<'a>(
        &'a self,
        _principal: &'a Principal,
        _workspace: WorkspaceId,
        _permission: okf_jawn_contract::access::Permission,
    ) -> PortFuture<'a, WorkspaceGrant> {
        unused()
    }
    fn authorize_tenant<'a>(
        &'a self,
        _principal: &'a Principal,
        _permission: okf_jawn_contract::access::Permission,
    ) -> PortFuture<'a, TenantGrant> {
        unused()
    }
    fn grants<'a>(&'a self, _principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        unused()
    }
    fn grant_creator<'a>(
        &'a self,
        _principal: &'a Principal,
        _workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant> {
        unused()
    }
    fn editors<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Vec<String>> {
        unused()
    }
}

impl MutationStore for Unused {
    fn begin<'a>(
        &'a self,
        _key: &'a MutationKey,
        _digest: &'a Digest,
    ) -> PortFuture<'a, BeginOutcome> {
        unused()
    }
    fn complete(&self, _lease: MutationLease, _response: Value) -> PortFuture<'_, ()> {
        unused()
    }
    fn release(&self, _lease: MutationLease) -> PortFuture<'_, ()> {
        unused()
    }
}

impl WorkspaceCatalog for Unused {
    fn list<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _include_archived: bool,
    ) -> PortFuture<'a, Vec<Workspace>> {
        unused()
    }
    fn create<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _mutation_id: MutationId,
        _workspace: NewWorkspace,
    ) -> PortFuture<'a, Workspace> {
        unused()
    }
    fn open<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Workspace> {
        unused()
    }
    fn update<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _update: WorkspaceUpdate,
    ) -> PortFuture<'a, Workspace> {
        unused()
    }
    fn archive<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _archive: WorkspaceArchive,
    ) -> PortFuture<'a, Workspace> {
        unused()
    }
    fn unarchive<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _author: Provenance,
    ) -> PortFuture<'a, Workspace> {
        unused()
    }
}

impl JobQueue for Unused {
    fn enqueue(&self, _scope: JobScope, _job: JobId) -> PortFuture<'_, ()> {
        unused()
    }
}

impl Converter for Unused {
    fn page_count<'a>(
        &'a self,
        _source: &'a LocalSource,
        _file_name: &'a str,
    ) -> PortFuture<'a, Option<u32>> {
        unused()
    }
    fn convert(&self, _input: ConversionInput) -> PortFuture<'_, Conversion> {
        unused()
    }
    fn limits(&self) -> ConverterLimits {
        // Not asked by the operations under test; no conversion runs here.
        ConverterLimits {
            window_pages: 1,
            memory_limit_bytes: 1,
        }
    }
}

impl ProposalStore for Unused {
    fn insert<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _proposal: Proposal,
    ) -> PortFuture<'a, Proposal> {
        unused()
    }
    fn get<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _proposal: ProposalId,
    ) -> PortFuture<'a, Proposal> {
        unused()
    }
    fn list<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _filter: ProposalFilter,
    ) -> PortFuture<'a, ListProposalsResponse> {
        unused()
    }
    fn update<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _proposal: Proposal,
    ) -> PortFuture<'a, Proposal> {
        unused()
    }
    fn add_comment<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _proposal: ProposalId,
        _comment: Comment,
    ) -> PortFuture<'a, Comment> {
        unused()
    }
    fn list_comments<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _proposal: ProposalId,
        _page: Page,
    ) -> PortFuture<'a, CommentPage> {
        unused()
    }
}

impl UploadStore for Unused {
    fn create<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _upload: NewUpload,
    ) -> PortFuture<'a, UploadRecord> {
        unused()
    }
    fn get<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _upload: UploadId,
    ) -> PortFuture<'a, UploadRecord> {
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
}

impl CredentialStore for Unused {
    fn installation_identity(&self) -> PortFuture<'_, InstallationIdentity> {
        unused()
    }
    fn create_connector(
        &self,
        _mutation_id: MutationId,
        _connector: NewConnector,
    ) -> PortFuture<'_, ConnectorIssue> {
        unused()
    }
    fn rotate_connector_secret(&self, _connector: ConnectorId) -> PortFuture<'_, IssuedConnector> {
        unused()
    }
    fn list_connectors(&self, _include_revoked: bool) -> PortFuture<'_, Vec<Connector>> {
        unused()
    }
    fn revoke_connector(&self, _connector: ConnectorId) -> PortFuture<'_, Connector> {
        unused()
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

impl ConfirmationStore for Unused {
    fn create<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _create: ConfirmationCreate,
    ) -> PortFuture<'a, Confirmation> {
        unused()
    }
    fn consume<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _consume: ConfirmationConsume,
    ) -> PortFuture<'a, Confirmation> {
        unused()
    }
}

impl ReadinessProbe for Unused {
    fn probe(&self) -> PortFuture<'_, ReadinessResponse> {
        unused()
    }
}

/// The configuration of the fixture service.
#[must_use]
pub fn config() -> ApplicationConfig {
    ApplicationConfig {
        sandbox_origin: SANDBOX_ORIGIN.to_owned(),
        sandbox_ttl: Duration::from_secs(300),
    }
}

/// The scope of the fixture workspace.
///
/// # Errors
/// Returns when the tenant id does not parse.
pub fn scope() -> Built<StorageScope> {
    Ok(StorageScope {
        tenant_id: TenantId::try_from("local".to_owned())?,
        workspace_id: WorkspaceId(Uuid::from_u128(WORKSPACE)),
    })
}

/// The context dispatch builds for `subject` on `route` running `operation`, with every
/// permission on the fixture workspace and, for a mutation, `mutation` as its identity.
///
/// # Errors
/// Returns when a fixture value does not parse.
pub fn context(
    operation: OperationName,
    subject: &str,
    route: AccessRoute,
    mutation: Option<MutationId>,
) -> Built<OperationContext> {
    let scope = scope()?;
    Ok(OperationContext {
        principal: Principal {
            subject: subject.to_owned(),
            tenant_id: scope.tenant_id.clone(),
            route,
            client_id: None,
            delegation: None,
        },
        session_id: None,
        operation,
        tenant: None,
        grants: vec![WorkspaceGrant {
            scope,
            permissions: vec![
                okf_jawn_contract::access::Permission::Read,
                okf_jawn_contract::access::Permission::Write,
            ],
        }],
        mutation,
        attempt: Attempt::First,
    })
}

/// Alice's context in her browser session.
///
/// # Errors
/// Returns when a fixture value does not parse.
pub fn alice(operation: OperationName, mutation: Option<MutationId>) -> Built<OperationContext> {
    context(operation, ALICE, AccessRoute::LocalOwner, mutation)
}

/// A mutation identity from a number.
#[must_use]
pub const fn mutation(number: u128) -> MutationId {
    MutationId(Uuid::from_u128(number))
}

/// A revision of 40 copies of `fill`.
///
/// # Errors
/// Returns when `fill` is not a lowercase hex digit.
pub fn revision(fill: char) -> Built<Revision> {
    Ok(Revision::try_from(fill.to_string().repeat(40))?)
}

/// A digest of 64 copies of `fill`.
///
/// # Errors
/// Returns when `fill` is not a lowercase hex digit.
pub fn digest(fill: char) -> Built<Digest> {
    Ok(Digest::try_from(fill.to_string().repeat(64))?)
}

/// A workspace path.
///
/// # Errors
/// Returns when `path` is not a workspace path.
pub fn path(path: &str) -> Built<WorkspacePath> {
    Ok(WorkspacePath::try_from(path.to_owned())?)
}

/// A committed note at `at`, as a store returns it: with a stale content digest, which the
/// application must replace with its own.
///
/// # Errors
/// Returns when a fixture value does not parse.
pub fn note(
    id: u128,
    at: &Revision,
    location: &str,
    body: &str,
    properties: Value,
) -> Built<ItemDocument> {
    Ok(ItemDocument {
        summary: ItemSummary {
            id: ItemId(Uuid::from_u128(id)),
            path: path(location)?,
            title: "Plan".to_owned(),
            description: String::new(),
            type_name: "Note".to_owned(),
            kind: ItemKind::Note,
            revision: at.clone(),
            status: ItemStatus::Stable,
            archived: false,
            media_type: None,
            extraction: None,
        },
        body: body.to_owned(),
        properties: serde_json::from_value(properties)?,
        content_digest: digest('0')?,
        source: None,
        draft: None,
    })
}

/// A completed converter extraction whose conversion record is `record`.
#[must_use]
pub fn converted(record: &Digest) -> Extraction {
    Extraction {
        outcome: ConversionOutcome::Completed,
        converter: None,
        digest: Some(record.clone()),
        page_count: Some(2),
        text_origin: TextOrigin::Converter,
        corrected: false,
        supplied: None,
        warnings: Vec::new(),
    }
}

/// A source card at `at` whose original is `digest('a')` (a PDF) and whose shown text is
/// `body`.
///
/// # Errors
/// Returns when a fixture value does not parse.
pub fn source_card(
    id: u128,
    at: &Revision,
    location: &str,
    body: &str,
    extraction: Extraction,
) -> Built<ItemDocument> {
    let mut document = note(
        id,
        at,
        location,
        body,
        serde_json::json!({ "type": "Source" }),
    )?;
    document.summary.kind = ItemKind::Source;
    "Source".clone_into(&mut document.summary.type_name);
    document.summary.media_type = Some("application/pdf".to_owned());
    document.summary.extraction = Some(extraction.summary());
    document.source = Some(SourceAppearance {
        object: digest('a')?,
        names: Vec::new(),
        media_type: "application/pdf".to_owned(),
        size: "2048".to_owned(),
        metadata: BTreeMap::new(),
        parent_item_id: None,
        supersedes: None,
        extraction,
    });
    Ok(document)
}

/// A two-page conversion record of `CARD_BODY` with `assets`.
///
/// # Errors
/// Returns when a fixture digest does not parse.
pub fn record(assets: Vec<RetainedAsset>) -> Built<ConversionRecord> {
    let page = |page_no| SourceLocation::Direct {
        locator: SourceLocator::Page { page_no },
    };
    let heading = |label: &str, start, end| OutlineEntry {
        label: label.to_owned(),
        level: 1,
        selection: Selection::Lines {
            range: TextRange { start, end },
        },
        kind: OutlineEntryKind::Heading,
    };
    Ok(ConversionRecord {
        converter: ConverterIdentity {
            name: "docling".to_owned(),
            version: "2.3.0".to_owned(),
            packages: Vec::new(),
            settings: ConversionSettings::default(),
            models: Vec::new(),
            page_window: Some(4),
        },
        outcome: ConversionOutcome::Completed,
        page_count: Some(2),
        structured: vec![WindowExport {
            window: Some(PageRange { start: 1, end: 2 }),
            digest: digest('c')?,
            lines: Some(TextRange { start: 1, end: 8 }),
        }],
        markdown: digest('e')?,
        locations: vec![
            LineLocation {
                lines: TextRange { start: 1, end: 3 },
                location: page(1),
            },
            LineLocation {
                lines: TextRange { start: 5, end: 8 },
                location: page(2),
            },
        ],
        outline: vec![heading("Overview", 1, 4), heading("Revenue", 5, 8)],
        tables: Vec::new(),
        assets,
        warnings: Vec::new(),
    })
}

/// A retained image on `page_no` with a known size.
///
/// # Errors
/// Returns when `fill` is not a lowercase hex digit.
pub fn image(fill: char, role: AssetRole, page_no: u32) -> Built<RetainedAsset> {
    Ok(RetainedAsset {
        digest: digest(fill)?,
        media_type: "image/png".to_owned(),
        role,
        location: SourceLocation::Direct {
            locator: SourceLocator::Page { page_no },
        },
        pixel_size: Some(PixelSize {
            width: 800,
            height: 600,
        }),
        caption: None,
    })
}

/// The content digest the server must report for `document`.
///
/// # Errors
/// Returns when the content cannot be serialized.
pub fn served_digest(document: &ItemDocument) -> Built<Digest> {
    Ok(item_content_digest(document)?)
}

/// A draft of `item` by `editor`.
///
/// # Errors
/// Returns when a fixture value does not parse.
pub fn draft_of(item: u128, editor: &str, base: &Revision, body: &str) -> Built<DraftContent> {
    Ok(DraftContent {
        draft: Draft {
            item_id: ItemId(Uuid::from_u128(item)),
            editor: editor.to_owned(),
            base_revision: base.clone(),
            content_digest: digest('9')?,
            saved_at: instant()?,
        },
        body: body.to_owned(),
        properties: BTreeMap::new(),
    })
}

/// The fixed instant fixture records carry.
///
/// # Errors
/// Returns when the spelling does not parse.
pub fn instant() -> Result<Timestamp, okf_jawn_contract::identity::IdentityError> {
    Timestamp::try_from("2026-10-09T12:00:00.000Z".to_owned())
}

/// The metadata a draft store returns for a write.
fn metadata(write: &DraftWrite) -> Result<Draft, okf_jawn_contract::identity::IdentityError> {
    Ok(Draft {
        item_id: write.item_id,
        editor: write.editor.clone(),
        base_revision: write.base_revision.clone(),
        content_digest: write.content_digest.clone(),
        saved_at: instant()?,
    })
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, error.to_string())
}

/// Lock a fixture mutex for a test, turning poisoning into an error.
fn lock<T>(mutex: &Mutex<T>) -> Built<MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| "a fixture lock is poisoned".into())
}

/// Lock a fixture mutex inside a port, turning poisoning into an `Internal` error.
fn lock_api<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, ApiError> {
    mutex
        .lock()
        .map_err(|_| ApiError::new(ErrorCode::Internal, "a fixture lock is poisoned"))
}

/// A future that answers that the method is not used.
fn unused<'a, T: Send + 'a>() -> PortFuture<'a, T> {
    Box::pin(async { Err(ApiError::new(ErrorCode::Internal, "not used by this test")) })
}
