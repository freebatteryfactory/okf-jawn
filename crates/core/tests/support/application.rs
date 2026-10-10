//! Fakes of every port `ApplicationService` is built over, for handler tests; not production
//! adapters. The `tests/application` target includes this file as its `fixture` module.
//!
//! Each fake answers what its port documents and nothing more: the version store commits only
//! on its expected head, replays a repeated mutation, and runs the check it is handed on a
//! staged directory; the draft store keeps one draft per (item, editor); the record store
//! answers revision maps and derived objects for exactly the (item, revision, digest) recorded.
//! Content is scripted by the test (`put_document`, `put_listing`). Two edits are applied as a
//! store would, so what `show` reads at the revision a commit produced follows them: an
//! `EditItem` (the item's document at the commit's base, with the edit's body and properties)
//! and a `RestoreItem` (the item as `from` holds it, its source appearance included, with the
//! port's `NotFound` and `Conflict`). `item_at_path` answers from the scripted documents as its port documents it: `None` when no item is at the path at
//! a known revision, `NotFound` only for an unknown revision. A port the operations
//! under test do not call answers `Internal` ("not used by this test").
//!
//! The search index, the event log and the receipt store keep their documented idempotence
//! (indexing a revision again is a no-op; an identical event or a receipt under the same
//! mutation is not recorded twice), and a test may make the next call of one of them fail
//! (`fail_next`, `fail_next_receipt`), as any port may, to show what a retry does.
//!
//! History is scripted too (`put_log`, `put_diff`, `put_blame`); `log` pages a scripted history as
//! storage does when a test asks (`page_log`). A comparison nobody scripted
//! between two known revisions lists no change, and an unknown revision is `NotFound`. The
//! fake's history is one line: the commits it wrote, and the heads a test moves with `set_head`,
//! each an unrelated commit on top of the head before it. A commit replays, and `find_commit`
//! with `since` finds it, only when it lies after the revision asked about, and an unknown
//! `since` is `NotFound`; `find_commit` without `since` searches that line back from the head,
//! as the port documents, back to the line's start with no time bound. The search
//! index
//! answers `Unavailable` for a revision it has not indexed, as its port documents, and applies
//! the query's filters to the entries a test scripted (`put_entry`, `put_link`): the archived
//! flag unless archived items are included, the extraction filter, the folder and the text.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
use okf_jawn_contract::history::{BlameLine, BlameResponse, DiffResponse, FileChange, LogResponse};
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
use okf_jawn_contract::search::{
    GetGraphResponse, GetLinksResponse, Link, LinkDirection, SearchHit, SearchResponse,
};
use okf_jawn_contract::source::{SourceAppearance, SourceLocation, SourceLocator, SourceReference};
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
use okf_jawn_core::portable::{content_digest, item_content_digest};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::proposals::{CommentPage, ProposalFilter, ProposalStore};
use okf_jawn_core::readiness::ReadinessProbe;
use okf_jawn_core::sandbox::{SandboxCapabilityStore, SandboxMint, SandboxResolved};
use okf_jawn_core::search::{GraphQuery, LinkQuery, SearchIndex, SearchQuery};
use okf_jawn_core::storage::{
    BlameQuery, BlobStore, ByteReader, CandidateChanges, CandidateCheck, CommitChanges, Committed,
    DiffQuery, FolderListing, LocalSource, LogQuery, NewWorkspace, ObjectInfo, ObjectRead, Page,
    Promotion, Provenance, StorageScope, TreeEdit, VersionStore, WorkspaceArchive,
    WorkspaceCatalog, WorkspaceUpdate,
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
    /// Dependency readiness.
    pub readiness: Arc<FakeReadiness>,
}

/// A staged candidate directory, removed when dropped.
pub struct Staged(pub PathBuf);

/// `VersionStore` over scripted documents and listings.
pub struct FakeVersions {
    state: Mutex<VersionState>,
    stage: Staged,
    shows: AtomicUsize,
    fail_next_lookup: AtomicBool,
}

#[derive(Default)]
struct VersionState {
    head: Option<Revision>,
    documents: Vec<(Revision, ItemDocument)>,
    listings: Vec<(Revision, Option<WorkspacePath>, FolderListing)>,
    types: Vec<TypeDefinition>,
    queued: Vec<Revision>,
    commits: Vec<(CommitChanges, Revision)>,
    landed: Vec<(Revision, Revision)>,
    landing: Vec<Revision>,
    log_page: Option<usize>,
    logs: Vec<(Revision, Option<ItemId>, LogResponse)>,
    diffs: Vec<(Revision, Revision, Option<ItemId>, Vec<FileChange>)>,
    blames: Vec<(Revision, ItemId, Vec<BlameLine>)>,
    log_queries: Vec<LogQuery>,
    diff_queries: Vec<DiffQuery>,
    blame_queries: Vec<BlameQuery>,
}

/// `BlobStore` over a map of digests to bytes.
#[derive(Default)]
pub struct FakeBlobs {
    objects: Mutex<BTreeMap<Digest, Vec<u8>>>,
    opens: Mutex<Vec<(Digest, u64, u64)>>,
}

/// `RecordStore` answering revision maps and derived objects, and keeping receipts unique on
/// their mutation.
#[derive(Default)]
pub struct FakeRecords {
    mappings: Mutex<BTreeMap<Revision, RevisionMapping>>,
    derived: Mutex<Vec<DerivedObject>>,
    receipts: Mutex<Vec<(Option<MutationId>, Receipt)>>,
    fail_next_receipt: AtomicBool,
}

/// `SearchIndex` that records each revision it indexed once (indexing it again is a no-op) and
/// answers queries from scripted entries of an indexed revision.
#[derive(Default)]
pub struct FakeSearch {
    indexed: Mutex<Vec<Revision>>,
    conflict_next: AtomicBool,
    builds: AtomicUsize,
    building: AtomicUsize,
    most_at_once: AtomicUsize,
    fail_next: AtomicBool,
    entries: Mutex<Vec<(Revision, ItemSummary, String)>>,
    links: Mutex<Vec<(Revision, Link)>>,
    queries: Mutex<Vec<SearchQuery>>,
    link_queries: Mutex<Vec<LinkQuery>>,
    graph_queries: Mutex<Vec<GraphQuery>>,
}

/// `ReadinessProbe` answering what a test scripted.
#[derive(Default)]
pub struct FakeReadiness {
    answer: Mutex<Option<Result<ReadinessResponse, ApiError>>>,
    probes: AtomicUsize,
}

/// `EventLog` that keeps every append, except an identical notification already appended
/// under the same mutation.
#[derive(Default)]
pub struct FakeEvents {
    appended: Mutex<Vec<(EventScope, Option<MutationId>, NewEvent)>>,
    fail_next: AtomicBool,
}

/// `DraftStore` with one draft per (item, editor).
#[derive(Default)]
pub struct FakeDrafts {
    drafts: Mutex<Vec<DraftContent>>,
    saves: Mutex<Vec<(MutationId, DraftWrite)>>,
    discarded: Mutex<Vec<(MutationId, Draft)>>,
    gets: AtomicUsize,
    saved_before_discard: Mutex<Option<DraftWrite>>,
    fail_next_discard: AtomicBool,
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
        let readiness = Arc::new(FakeReadiness::default());
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
            readiness: readiness.clone(),
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
            readiness,
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
            fail_next_lookup: AtomicBool::new(false),
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
        let mut state = lock(&self.state)?;
        if let Some(parent) = state.head.replace(revision.clone())
            && parent != revision
        {
            state.landed.push((revision, parent));
        }
        Ok(())
    }

    /// Before each of the next `commit` calls, move the head to the next of `revisions`, as an
    /// unrelated commit landing between a writer's check and its commit would.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn land_before_next_commits(&self, revisions: Vec<Revision>) -> Built<()> {
        lock(&self.state)?.landing = revisions;
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

    /// Make the next `item_at_path` answer `NotFound`, as it does for an unknown revision.
    pub fn fail_next_lookup(&self) {
        self.fail_next_lookup.store(true, Ordering::SeqCst);
    }

    /// How many times `show` was called.
    #[must_use]
    pub fn shows(&self) -> usize {
        self.shows.load(Ordering::SeqCst)
    }

    /// Answer `log` from `tip`, optionally of `item`, with `response`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_log(
        &self,
        tip: &Revision,
        item: Option<ItemId>,
        response: LogResponse,
    ) -> Built<()> {
        lock(&self.state)?.logs.push((tip.clone(), item, response));
        Ok(())
    }

    /// Answer `diff` from `from` to `to`, optionally of `item`, with `changes`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_diff(
        &self,
        from: &Revision,
        to: &Revision,
        item: Option<ItemId>,
        changes: Vec<FileChange>,
    ) -> Built<()> {
        lock(&self.state)?
            .diffs
            .push((from.clone(), to.clone(), item, changes));
        Ok(())
    }

    /// Attribute `lines` of `item` at `at`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_blame(&self, at: &Revision, item: ItemId, lines: Vec<BlameLine>) -> Built<()> {
        lock(&self.state)?.blames.push((at.clone(), item, lines));
        Ok(())
    }

    /// Page `log` as storage does: a scripted history is answered `size` commits at a time (or
    /// fewer when the query asks for fewer), from the offset its decimal cursor names, with the
    /// next offset as the cursor while commits remain. Without it, `log` answers the scripted
    /// response whole.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn page_log(&self, size: usize) -> Built<()> {
        lock(&self.state)?.log_page = Some(size);
        Ok(())
    }

    /// Every `log` query, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn log_queries(&self) -> Built<Vec<LogQuery>> {
        Ok(lock(&self.state)?.log_queries.clone())
    }

    /// Every `diff` query, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn diff_queries(&self) -> Built<Vec<DiffQuery>> {
        Ok(lock(&self.state)?.diff_queries.clone())
    }

    /// Every `blame` query, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn blame_queries(&self) -> Built<Vec<BlameQuery>> {
        Ok(lock(&self.state)?.blame_queries.clone())
    }

    fn head_now(&self) -> Result<Revision, ApiError> {
        lock_api(&self.state)?
            .head
            .clone()
            .ok_or_else(|| ApiError::new(ErrorCode::Internal, "no head"))
    }
}

impl VersionState {
    /// The commit `mutation` wrote, when it lies after `revision` on the fake's one line of
    /// history: `revision` is the head that commit was written on, or a head before that one.
    fn written_after(&self, mutation: MutationId, revision: &Revision) -> Option<Revision> {
        let (changes, written) = self
            .commits
            .iter()
            .find(|(changes, _)| changes.mutation_id == mutation)?;
        let mut base = Some(&changes.expected_head);
        // Each step moves to an earlier commit; the bound stops a scripted cycle.
        for _ in 0..=self.line_length() {
            let at = base?;
            if at == revision {
                return Some(written.clone());
            }
            base = self.parent_of(at);
        }
        None
    }

    /// The commit `mutation` wrote, when it lies on the line back from the head.
    fn on_the_line(&self, mutation: MutationId) -> Option<Revision> {
        let mut at = self.head.as_ref();
        // Each step moves to an earlier commit; the bound stops a scripted cycle.
        for _ in 0..=self.line_length() {
            let revision = at?;
            if self
                .commits
                .iter()
                .any(|(changes, produced)| produced == revision && changes.mutation_id == mutation)
            {
                return Some(revision.clone());
            }
            at = self.parent_of(revision);
        }
        None
    }

    /// The documents each `TreeEdit::RestoreItem` of `changes` restores, as its port documents
    /// it: the item as `from` holds it, its source appearance (the companion files) included;
    /// nothing for an item absent at `from`, which is removed. Consecutive `RestoreItem` edits
    /// are one group: every item of the group is looked up first (`NotFound` for one absent both
    /// at `from` and at the expected head), and the group's items have left their current paths
    /// before any is written back, so only an item outside the group holding a restored path at
    /// the expected head is a `Conflict`. A later listing of an item wins (`show` reads the
    /// newest document first).
    fn restored_items(&self, changes: &CommitChanges) -> Result<Vec<ItemDocument>, ApiError> {
        let at = |revision: &Revision, item: ItemId| {
            self.documents
                .iter()
                .find(|(held, document)| held == revision && document.summary.id == item)
                .map(|(_, document)| document.clone())
        };
        let mut restored = Vec::new();
        for group in changes
            .edits
            .split(|edit| !matches!(edit, TreeEdit::RestoreItem { .. }))
        {
            let listed: Vec<(ItemId, &Revision)> = group
                .iter()
                .filter_map(|edit| match edit {
                    TreeEdit::RestoreItem { item_id, from } => Some((*item_id, from)),
                    _ => None,
                })
                .collect();
            for (item_id, from) in &listed {
                if at(from, *item_id).is_none() && at(&changes.expected_head, *item_id).is_none() {
                    return Err(ApiError::new(ErrorCode::NotFound, "no such item"));
                }
            }
            for (item_id, from) in &listed {
                let Some(document) = at(from, *item_id) else {
                    continue;
                };
                if self.documents.iter().any(|(held, other)| {
                    *held == changes.expected_head
                        && other.summary.path == document.summary.path
                        && listed
                            .iter()
                            .all(|(grouped, _)| *grouped != other.summary.id)
                }) {
                    return Err(ApiError::new(
                        ErrorCode::Conflict,
                        "another item holds the restored path",
                    ));
                }
                restored.push(document);
            }
        }
        Ok(restored)
    }
    /// The commit before `revision` on the line: the head a written commit was based on, or the
    /// head a test moved away from.
    fn parent_of(&self, revision: &Revision) -> Option<&Revision> {
        self.commits
            .iter()
            .find(|(_, produced)| produced == revision)
            .map(|(changes, _)| &changes.expected_head)
            .or_else(|| {
                self.landed
                    .iter()
                    .find(|(produced, _)| produced == revision)
                    .map(|(_, parent)| parent)
            })
    }

    const fn line_length(&self) -> usize {
        self.commits.len().saturating_add(self.landed.len())
    }

    /// Refuse an unknown revision with `NotFound`. A revision is known when it is the head, a
    /// commit produced it, or the test scripted content at it.
    fn require_known(&self, revision: &Revision) -> Result<(), ApiError> {
        let known = self.head.as_ref() == Some(revision)
            || self.commits.iter().any(|(_, at)| at == revision)
            || self.landed.iter().any(|(at, _)| at == revision)
            || self.documents.iter().any(|(at, _)| at == revision)
            || self.listings.iter().any(|(at, _, _)| at == revision)
            || self.logs.iter().any(|(at, _, _)| at == revision)
            || self
                .diffs
                .iter()
                .any(|(from, to, _, _)| from == revision || to == revision);
        if known {
            Ok(())
        } else {
            Err(ApiError::new(ErrorCode::NotFound, "no such revision"))
        }
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

    /// Make the next `insert_receipt` fail, recording nothing, as a store that is down would.
    pub fn fail_next_receipt(&self) {
        self.fail_next_receipt.store(true, Ordering::SeqCst);
    }
}

impl FakeSearch {
    /// Every revision indexed, in the order first indexed.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn indexed(&self) -> Built<Vec<Revision>> {
        Ok(lock(&self.indexed)?.clone())
    }

    /// How many times `index_revision` ran, a no-op repeat included.
    #[must_use]
    pub fn builds(&self) -> usize {
        self.builds.load(Ordering::SeqCst)
    }

    /// The most `index_revision` calls that were running at one time.
    #[must_use]
    pub fn most_at_once(&self) -> usize {
        self.most_at_once.load(Ordering::SeqCst)
    }

    /// Make the next `index_revision` answer `Conflict`, indexing nothing, as a port that
    /// refuses after a write was committed would.
    pub fn conflict_next(&self) {
        self.conflict_next.store(true, Ordering::SeqCst);
    }

    /// Make the next `index_revision` fail, indexing nothing.
    pub fn fail_next(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }

    /// Hold `summary` with its indexed `text` in the index of `at`; it is answered only once
    /// `at` is indexed.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_entry(&self, at: &Revision, summary: ItemSummary, text: &str) -> Built<()> {
        lock(&self.entries)?.push((at.clone(), summary, text.to_owned()));
        Ok(())
    }

    /// Hold `link` in the index of `at`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_link(&self, at: &Revision, link: Link) -> Built<()> {
        lock(&self.links)?.push((at.clone(), link));
        Ok(())
    }

    /// Mark `at` indexed, as an earlier write would have.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn mark_indexed(&self, at: &Revision) -> Built<()> {
        lock(&self.indexed)?.push(at.clone());
        Ok(())
    }

    /// Every search query, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn queries(&self) -> Built<Vec<SearchQuery>> {
        Ok(lock(&self.queries)?.clone())
    }

    /// Every link query, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn link_queries(&self) -> Built<Vec<LinkQuery>> {
        Ok(lock(&self.link_queries)?.clone())
    }

    /// Every graph query, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn graph_queries(&self) -> Built<Vec<GraphQuery>> {
        Ok(lock(&self.graph_queries)?.clone())
    }

    /// Whether `at` is indexed; a query at any other revision is `Unavailable`.
    fn require_indexed(&self, at: &Revision) -> Result<(), ApiError> {
        if lock_api(&self.indexed)?.contains(at) {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorCode::Unavailable,
                "that revision is not indexed",
            ))
        }
    }
}

impl FakeReadiness {
    /// Answer every probe with `response`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn answer(&self, response: ReadinessResponse) -> Built<()> {
        *lock(&self.answer)? = Some(Ok(response));
        Ok(())
    }

    /// Fail every probe with `error`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail(&self, error: ApiError) -> Built<()> {
        *lock(&self.answer)? = Some(Err(error));
        Ok(())
    }

    /// How many probes ran.
    #[must_use]
    pub fn probes(&self) -> usize {
        self.probes.load(Ordering::SeqCst)
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

    /// Make the next `append` fail, appending nothing.
    pub fn fail_next(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
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

    /// Save `write` as its editor's draft just before the next `discard` runs, as an autosave
    /// landing between a Snapshot's read and its removal would.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn save_before_next_discard(&self, write: DraftWrite) -> Built<()> {
        *lock(&self.saved_before_discard)? = Some(write);
        Ok(())
    }

    /// Make the next `discard` fail with a store fault (`Internal`), removing nothing.
    pub fn fail_next_discard(&self) {
        self.fail_next_discard.store(true, Ordering::SeqCst);
    }

    /// Every draft held now, most recently saved first.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn held(&self) -> Built<Vec<DraftContent>> {
        Ok(lock(&self.drafts)?.clone())
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
    fn item_at_path<'a>(
        &'a self,
        _scope: &'a StorageScope,
        revision: &'a Revision,
        path: &'a WorkspacePath,
    ) -> PortFuture<'a, Option<ItemSummary>> {
        Box::pin(async move {
            if self.fail_next_lookup.swap(false, Ordering::SeqCst) {
                return Err(ApiError::new(ErrorCode::NotFound, "no such revision"));
            }
            let state = lock_api(&self.state)?;
            state.require_known(revision)?;
            Ok(state
                .documents
                .iter()
                .find(|(at, document)| at == revision && &document.summary.path == path)
                .map(|(_, document)| document.summary.clone()))
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
            if !state.landing.is_empty() {
                let landed = state.landing.remove(0);
                if let Some(parent) = state.head.replace(landed.clone()) {
                    state.landed.push((landed, parent));
                }
            }
            if let Some(revision) = state.written_after(changes.mutation_id, &changes.expected_head)
            {
                return Ok(Committed {
                    revision,
                    replayed: true,
                    warnings: Vec::new(),
                });
            }
            if state.head.as_ref() != Some(&changes.expected_head) {
                return Err(ApiError::new(ErrorCode::Conflict, "the head moved"));
            }
            let restored = state.restored_items(&changes)?;
            let warnings = check.check(&self.stage.0)?;
            if state.queued.is_empty() {
                return Err(ApiError::new(ErrorCode::Internal, "no revision queued"));
            }
            let revision = state.queued.remove(0);
            // What `show` reads of each edited item at the new revision.
            for edit in &changes.edits {
                if let TreeEdit::EditItem {
                    item_id,
                    body,
                    properties,
                } = edit
                    && let Some(mut edited) = state
                        .documents
                        .iter()
                        .find(|(at, document)| {
                            *at == changes.expected_head && document.summary.id == *item_id
                        })
                        .map(|(_, document)| document.clone())
                {
                    edited.summary.revision = revision.clone();
                    edited.body.clone_from(body);
                    edited.properties.clone_from(properties);
                    state.documents.insert(0, (revision.clone(), edited));
                }
            }
            for mut document in restored {
                document.summary.revision = revision.clone();
                state.documents.insert(0, (revision.clone(), document));
            }
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
        mutation_id: MutationId,
        since: Option<&'a Revision>,
    ) -> PortFuture<'a, Option<Revision>> {
        Box::pin(async move {
            let state = lock_api(&self.state)?;
            match since {
                Some(since) => {
                    state.require_known(since)?;
                    Ok(state.written_after(mutation_id, since))
                }
                None => Ok(state.on_the_line(mutation_id)),
            }
        })
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
    fn log<'a>(&'a self, _scope: &'a StorageScope, query: LogQuery) -> PortFuture<'a, LogResponse> {
        Box::pin(async move {
            let mut state = lock_api(&self.state)?;
            state.log_queries.push(query.clone());
            state.require_known(&query.tip)?;
            let scripted = state
                .logs
                .iter()
                .find(|(tip, item, _)| *tip == query.tip && *item == query.item_id)
                .map_or_else(
                    || LogResponse {
                        commits: Vec::new(),
                        next_cursor: None,
                    },
                    |(_, _, response)| response.clone(),
                );
            let Some(size) = state.log_page else {
                return Ok(scripted);
            };
            let skip = match query.page.cursor.as_deref() {
                None => 0,
                Some(cursor) => cursor
                    .parse::<usize>()
                    .map_err(|_| ApiError::new(ErrorCode::InvalidInput, "not a log cursor"))?,
            };
            let limit = size.min(usize::from(query.page.limit)).max(1);
            let commits: Vec<_> = scripted.commits.into_iter().skip(skip).collect();
            let more = commits.len() > limit;
            Ok(LogResponse {
                commits: commits.into_iter().take(limit).collect(),
                next_cursor: more.then(|| skip.saturating_add(limit).to_string()),
            })
        })
    }
    fn diff<'a>(
        &'a self,
        _scope: &'a StorageScope,
        query: DiffQuery,
    ) -> PortFuture<'a, DiffResponse> {
        Box::pin(async move {
            let mut state = lock_api(&self.state)?;
            state.diff_queries.push(query.clone());
            state.require_known(&query.from)?;
            state.require_known(&query.to)?;
            let changes = state
                .diffs
                .iter()
                .find(|(from, to, item, _)| {
                    *from == query.from && *to == query.to && *item == query.item_id
                })
                .map(|(_, _, _, changes)| changes.clone())
                .unwrap_or_default();
            Ok(DiffResponse {
                from: query.from,
                to: query.to,
                changes,
                warnings: Vec::new(),
            })
        })
    }
    fn blame<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: BlameQuery,
    ) -> PortFuture<'a, BlameResponse> {
        Box::pin(async move {
            let mut state = lock_api(&self.state)?;
            state.blame_queries.push(query.clone());
            let (path, lines) = {
                let document = state
                    .documents
                    .iter()
                    .find(|(at, document)| {
                        *at == query.revision && document.summary.id == query.item_id
                    })
                    .map(|(_, document)| document)
                    .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such item"))?;
                let lines: Vec<BlameLine> = state
                    .blames
                    .iter()
                    .filter(|(at, item, _)| *at == query.revision && *item == query.item_id)
                    .flat_map(|(_, _, lines)| lines.iter())
                    .filter(|line| line.line >= query.lines.start && line.line <= query.lines.end)
                    .cloned()
                    .collect();
                (document.summary.path.clone(), lines)
            };
            // What a store may put in the citation; the application replaces it.
            Ok(BlameResponse {
                source: SourceReference {
                    workspace_id: scope.workspace_id,
                    item_id: query.item_id,
                    path,
                    revision: query.revision,
                    digest: None,
                    selection: Selection::Lines { range: query.lines },
                    locations: Vec::new(),
                },
                lines,
            })
        })
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
            injected(&self.fail_next_receipt)?;
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
            injected(&self.fail_next)?;
            if self.conflict_next.swap(false, Ordering::SeqCst) {
                return Err(ApiError::new(ErrorCode::Conflict, "injected conflict"));
            }
            self.builds.fetch_add(1, Ordering::SeqCst);
            let running = self
                .building
                .fetch_add(1, Ordering::SeqCst)
                .saturating_add(1);
            self.most_at_once.fetch_max(running, Ordering::SeqCst);
            // A build takes time: let another request run meanwhile.
            tokio::task::yield_now().await;
            self.building.fetch_sub(1, Ordering::SeqCst);
            let mut indexed = lock_api(&self.indexed)?;
            if !indexed.contains(&revision) {
                indexed.push(revision);
            }
            Ok(())
        })
    }
    fn search<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: SearchQuery,
    ) -> PortFuture<'a, SearchResponse> {
        Box::pin(async move {
            lock_api(&self.queries)?.push(query.clone());
            query.check()?;
            self.require_indexed(&query.revision)?;
            let text = query.text.trim().to_lowercase();
            let matching: Vec<SearchHit> = lock_api(&self.entries)?
                .iter()
                .filter(|(at, _, _)| *at == query.revision)
                .filter(|(_, summary, _)| query.include_archived || !summary.archived)
                .filter(|(_, summary, _)| {
                    query.extraction.is_none()
                        || summary
                            .extraction
                            .as_ref()
                            .is_some_and(|extraction| extraction.status.is_unprocessed())
                })
                .filter(|(_, summary, _)| within(summary, query.folder.as_ref()))
                .filter(|(_, _, indexed)| text.is_empty() || indexed.to_lowercase().contains(&text))
                .map(|(_, summary, indexed)| SearchHit {
                    source: SourceReference {
                        workspace_id: scope.workspace_id,
                        item_id: summary.id,
                        path: summary.path.clone(),
                        revision: query.revision.clone(),
                        digest: None,
                        selection: Selection::All,
                        locations: Vec::new(),
                    },
                    title: summary.title.clone(),
                    snippet: indexed.clone(),
                    score: 1.0,
                    extraction: summary.extraction.clone(),
                })
                .collect();
            let start = query
                .page
                .cursor
                .as_deref()
                .map_or(Ok(0), str::parse::<usize>)
                .map_err(internal)?;
            let limit = usize::from(query.page.limit.max(1));
            let end = start.saturating_add(limit);
            Ok(SearchResponse {
                revision: query.revision,
                next_cursor: (end < matching.len()).then(|| end.to_string()),
                hits: matching.into_iter().skip(start).take(limit).collect(),
            })
        })
    }
    fn links<'a>(
        &'a self,
        _scope: &'a StorageScope,
        query: LinkQuery,
    ) -> PortFuture<'a, GetLinksResponse> {
        Box::pin(async move {
            lock_api(&self.link_queries)?.push(query.clone());
            self.require_indexed(&query.revision)?;
            let links = lock_api(&self.links)?
                .iter()
                .filter(|(at, _)| *at == query.revision)
                .map(|(_, link)| link)
                .filter(|link| {
                    let outgoing = link.from == query.item_id;
                    let incoming = link.to == Some(query.item_id);
                    match query.direction {
                        LinkDirection::Outgoing => outgoing,
                        LinkDirection::Incoming => incoming,
                        LinkDirection::Both => outgoing || incoming,
                    }
                })
                .take(usize::from(query.page.limit.max(1)))
                .cloned()
                .collect();
            Ok(GetLinksResponse {
                revision: query.revision,
                links,
                next_cursor: None,
            })
        })
    }
    fn graph<'a>(
        &'a self,
        _scope: &'a StorageScope,
        query: GraphQuery,
    ) -> PortFuture<'a, GetGraphResponse> {
        Box::pin(async move {
            lock_api(&self.graph_queries)?.push(query.clone());
            self.require_indexed(&query.revision)?;
            let all: Vec<ItemSummary> = lock_api(&self.entries)?
                .iter()
                .filter(|(at, summary, _)| {
                    *at == query.revision && within(summary, query.folder.as_ref())
                })
                .map(|(_, summary, _)| summary.clone())
                .collect();
            let limit = usize::try_from(query.max_nodes).unwrap_or(usize::MAX);
            let truncated = all.len() > limit;
            let nodes: Vec<ItemSummary> = all.into_iter().take(limit).collect();
            let edges = lock_api(&self.links)?
                .iter()
                .filter(|(at, _)| *at == query.revision)
                .map(|(_, link)| link)
                .filter(|link| {
                    nodes.iter().any(|node| node.id == link.from)
                        && nodes.iter().any(|node| Some(node.id) == link.to)
                })
                .cloned()
                .collect();
            Ok(GetGraphResponse {
                revision: query.revision,
                nodes,
                edges,
                truncated,
            })
        })
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
            injected(&self.fail_next)?;
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
        scope: &'a StorageScope,
        mutation_id: MutationId,
        item: ItemId,
        editor: &'a str,
        expected: Option<Digest>,
    ) -> PortFuture<'a, Draft> {
        Box::pin(async move {
            injected(&self.fail_next_discard)?;
            let landing = lock_api(&self.saved_before_discard)?.take();
            if let Some(write) = landing {
                self.save(scope, mutation(0xdead), write).await?;
            }
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
            if let Some(expected) = expected
                && drafts
                    .get(position)
                    .is_some_and(|held| held.draft.content_digest != expected)
            {
                return Err(ApiError::new(
                    ErrorCode::Conflict,
                    "the held draft is not the expected one",
                ));
            }
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

impl ReadinessProbe for FakeReadiness {
    fn probe(&self) -> PortFuture<'_, ReadinessResponse> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            lock_api(&self.answer)?
                .clone()
                .unwrap_or_else(|| Err(ApiError::new(ErrorCode::Internal, "not scripted")))
        })
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

/// A draft of `item` by `editor`, with no properties, digested as the item it would commit.
///
/// # Errors
/// Returns when a fixture value does not parse.
pub fn draft_of(item: u128, editor: &str, base: &Revision, body: &str) -> Built<DraftContent> {
    Ok(DraftContent {
        draft: Draft {
            item_id: ItemId(Uuid::from_u128(item)),
            editor: editor.to_owned(),
            base_revision: base.clone(),
            content_digest: content_digest(body, &BTreeMap::new())?,
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

/// Whether an item lies in `folder` or a folder beneath it; every item lies in no folder.
fn within(summary: &ItemSummary, folder: Option<&WorkspacePath>) -> bool {
    folder.is_none_or(|folder| {
        summary
            .path
            .as_str()
            .starts_with(&format!("{}/", folder.as_str()))
    })
}

/// Fail once when a test asked for it, then behave again.
fn injected(fail_next: &AtomicBool) -> Result<(), ApiError> {
    if fail_next.swap(false, Ordering::SeqCst) {
        return Err(ApiError::new(ErrorCode::Internal, "injected failure"));
    }
    Ok(())
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
