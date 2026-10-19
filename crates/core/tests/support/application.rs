//! Fakes of every port `ApplicationService` is built over, for handler tests; not production
//! adapters. The `tests/application` target includes this file as its `fixture` module.
//!
//! Each fake answers what its port documents and nothing more: the version store commits only
//! on its expected head, replays a repeated mutation, and runs the check it is handed on a
//! staged directory, keeping the warnings that check gave so a replay (by `commit` or by
//! `find_commit`) answers them, or the one `replay_warnings_unavailable` warning once a test
//! made it forget them (`forget_warnings`); the draft store keeps one draft per (item, editor); the record store
//! answers revision maps and derived objects for exactly the (item, revision, digest) recorded, and records
//! one once on that triple; the blob store retains what `put` hashes, refusing an expected digest
//! that differs and bytes past the limit, as the port documents.
//! Content is scripted by the test (`put_document`, `put_listing`). Three edits are applied, in
//! order, to the items the commit's base holds, so what `show` reads at the revision a commit
//! produced follows them (`VersionState::applied`): an `EditItem` (the edit's body and
//! properties), a `MoveItem` (the item is at its destination, or the port's `NotFound` and
//! `Conflict` for a destination whose collision key is taken) and a `RestoreItem` (the item as
//! `from` holds it, its source appearance included, with the port's `NotFound` and
//! `Conflict`). The fake does NOT rewrite links: a move or a restore changes no other item, so
//! a rule about what links say after a write is proven only against real storage. A test
//! scripts which files link to an item (`put_referrer`), and a commit that moves the item
//! reports them to its check as rewritten, as the port documents, though the fake changes none
//! of their text. Its `MoveItem`
//! and restore are also kinder than storage's about a destination: they do not refuse a folder
//! at the destination (a file under `destination/`), a file standing where a folder of the
//! destination is needed, or a reserved or non-Markdown destination (`check_item_path`'s
//! `InvalidInput`), so a rule about those is proven only against real storage. It applies no
//! other edit (a delete, a status change, a folder), and only the items an applied edit wrote
//! are readable at the new revision. A test may make the next `show` fail with a code it
//! names (`fail_next_show`, `fail_show_number`), as a store fault would, and so may the next `log`, the next `list` and the next `diff` (`fail_next_log`, `fail_next_list`, `fail_next_diff`). `item_at_path` answers from the scripted documents as its port documents it: `None` when no item is at the path at
//! a known revision, `NotFound` only for an unknown revision. `tree` answers the documents at a
//! revision, one per item, in path order, each source card with its header's original name, and
//! every folder above them; it holds no folder no document lies under. `rules` answers the file a test
//! scripted at a revision (`put_rules`), and none elsewhere. A port the operations
//! under test do not call answers `Internal` ("not used by this test").
//!
//! The search index, the event log and the receipt store keep their documented idempotence
//! (indexing a revision again is a no-op; an identical event or a receipt under the same
//! mutation is not recorded twice), and a test may make the next call of one of them fail
//! (`fail_next`, `fail_next_receipt`), as any port may, to show what a retry does.
//!
//! An item's file is its document as storage writes one (`file_text`): frontmatter of its
//! properties, a blank line, then the body. `body_start` answers the line its body starts on
//! there, and `file_patch` writes a patch between two such files numbered over the whole file,
//! as libgit2 does, for a test to script as a diff. `read_file` answers that file at a
//! document's path, and `list` of a folder no test scripted is derived from the documents at
//! the revision, so a replayed commit's tree can be read back. The fake keeps no folder
//! `index.md` and no `log.md`.
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
//! Search hits and links are paged as storage pages them: from the offset the decimal cursor
//! names, with the next offset as the cursor while entries remain. `list` records the page each
//! call asked for (`list_queries`). It answers a listing whole, whatever the page, unless a test
//! asks for pages (`page_listings`): then a listing, scripted or derived, is paged as storage
//! pages a folder, its child folders first.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use okf_jawn_contract::access::{AccessRoute, Connector, IssuedConnector, Principal};
use okf_jawn_contract::common::{PageRange, TextRange, Warning};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
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
use okf_jawn_contract::import::{DEFAULT_MAX_UPLOAD_BYTES, Job, ListJobsResponse};
use okf_jawn_contract::item::{
    Draft, DraftContent, ItemDocument, ItemKind, ItemStatus, ItemSummary, TypeDefinition,
};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::proposal::{Comment, ListProposalsResponse, Proposal};
use okf_jawn_contract::purge::{Purge, PurgeTarget};
use okf_jawn_contract::read::{AssetRole, OutlineEntry, OutlineEntryKind, Selection};
use okf_jawn_contract::review::{Confirmation, Review};
use okf_jawn_contract::search::{
    GetGraphResponse, GetLinksResponse, Link, LinkDirection, SearchHit, SearchResponse,
};
use okf_jawn_contract::source::{SourceAppearance, SourceLocation, SourceLocator, SourceReference};
use okf_jawn_contract::workspace::{RestoreReport, Workspace};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::application::{ApplicationConfig, ApplicationService, Ports};
use okf_jawn_core::confirmations::{ConfirmationConsume, ConfirmationCreate, ConfirmationStore};
use okf_jawn_core::context::{Attempt, OperationContext, TenantGrant, WorkspaceGrant};
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, ConversionRecord, Converter, ConverterLimits, LineLocation,
    PageCount, PixelSize, RetainedAsset, WindowExport,
};
use okf_jawn_core::credentials::{
    ConnectorIssue, CredentialStore, InstallationIdentity, NewConnector, SessionRecord,
};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
use okf_jawn_core::items::ApplicationHeader;
use okf_jawn_core::jobs::{
    ArtifactRecord, ClaimedJob, DerivedObject, JobCompletion, JobLease, JobQueue, JobScope,
    NewArtifact, NewJob, NewPurge, RecordStore, RevisionMapping,
};
use okf_jawn_core::mutations::{BeginOutcome, MutationKey, MutationLease, MutationStore};
use okf_jawn_core::portable::{draft_digest, item_content_digest};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::proposals::{CommentPage, ProposalFilter, ProposalStore};
use okf_jawn_core::readiness::ReadinessProbe;
use okf_jawn_core::sandbox::{SandboxCapabilityStore, SandboxMint, SandboxResolved};
use okf_jawn_core::search::{GraphQuery, LinkQuery, SearchIndex, SearchQuery};
use okf_jawn_core::storage::{
    Backups, BlameQuery, BlobStore, ByteReader, CandidateChanges, CandidateCheck, CommitChanges,
    Committed, DiffQuery, FolderListing, LocalSource, LogQuery, NewWorkspace, ObjectInfo,
    ObjectRead, Page, Promotion, Provenance, RulesFile, StorageScope, TreeEdit, TreeItem,
    VersionStore, WorkspaceArchive, WorkspaceCatalog, WorkspaceTree, WorkspaceUpdate,
    WrittenArchive, replay_warnings_unavailable,
};
use okf_jawn_core::uploads::{NewUpload, UploadPage, UploadQuery, UploadRecord, UploadStore};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt as _;
use uuid::Uuid;

/// Result of a fixture builder.
pub type Built<T> = Result<T, Box<dyn Error>>;

/// What a commit's edits did to the fake's tree (`VersionState::applied`): each written item
/// with whether it replaces a scripted document, and the items an edit moved.
type Applied = (Vec<(ItemDocument, bool)>, Vec<ItemId>);

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

/// One line of a line diff (`file_patch`): kept, removed from the old file or added in the new
/// one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    Keep,
    Remove,
    Add,
}

/// `VersionStore` over scripted documents and listings.
pub struct FakeVersions {
    state: Mutex<VersionState>,
    stage: Staged,
    shows: AtomicUsize,
    trees: AtomicUsize,
    fail_next_lookup: AtomicBool,
    fail_show: Mutex<Option<(usize, ErrorCode)>>,
    fail_next_log: Mutex<Option<ErrorCode>>,
    fail_next_list: Mutex<Option<ErrorCode>>,
    fail_next_diff: Mutex<Option<ErrorCode>>,
}

#[derive(Default)]
struct VersionState {
    head: Option<Revision>,
    documents: Vec<(Revision, ItemDocument)>,
    listings: Vec<(Revision, Option<WorkspacePath>, FolderListing)>,
    types: Vec<TypeDefinition>,
    queued: Vec<Revision>,
    commits: Vec<(CommitChanges, Revision)>,
    /// Files that link to an item, as a test scripted them (`put_referrer`).
    referrers: Vec<(ItemId, WorkspacePath)>,
    /// The warnings the check gave for each commit, by its mutation, kept before it landed.
    warnings: Vec<(MutationId, Vec<Warning>)>,
    landed: Vec<(Revision, Revision)>,
    landing: Vec<Revision>,
    log_page: Option<usize>,
    list_page: Option<usize>,
    logs: Vec<(Revision, Option<ItemId>, LogResponse)>,
    diffs: Vec<(Revision, Revision, Option<ItemId>, Vec<FileChange>)>,
    blames: Vec<(Revision, ItemId, Vec<BlameLine>)>,
    log_queries: Vec<LogQuery>,
    diff_queries: Vec<DiffQuery>,
    blame_queries: Vec<BlameQuery>,
    list_queries: Vec<(Revision, Option<WorkspacePath>, Page)>,
    /// The naming rules file each revision holds; a revision with none has no rules saved.
    rules: Vec<(Revision, RulesFile)>,
}

/// `BlobStore` over a map of digests to bytes.
#[derive(Default)]
pub struct FakeBlobs {
    objects: Mutex<BTreeMap<Digest, Vec<u8>>>,
    opens: Mutex<Vec<(Digest, u64, u64)>>,
    puts: Mutex<Vec<Digest>>,
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
/// The lines of context around a change in a hunk `file_patch` writes, libgit2's default.
const PATCH_CONTEXT: usize = 3;

impl World {
    /// A service over fresh fakes whose head is `revision('a')`, with a conformant staged tree.
    ///
    /// # Errors
    /// Returns when the staged directory cannot be written or a fixture value does not parse.
    pub fn new() -> Built<Self> {
        Self::with_config(config())
    }

    /// The same world, its service built with `config`.
    ///
    /// # Errors
    /// As `World::new`, or the refusal of `config`.
    pub fn with_config(config: ApplicationConfig) -> Built<Self> {
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
            backups: unused.clone(),
        };
        let service = ApplicationService::new(ports, config)?;
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
            trees: AtomicUsize::new(0),
            fail_next_lookup: AtomicBool::new(false),
            fail_show: Mutex::new(None),
            fail_next_log: Mutex::new(None),
            fail_next_list: Mutex::new(None),
            fail_next_diff: Mutex::new(None),
        })
    }

    /// Script a file that links to `item`. The fake rewrites no link, so it cannot find a
    /// referrer itself; with this, a commit that moves `item` (a `MoveItem` that changes its
    /// path, or a `RestoreItem` that puts it back at another path than the tree held it at)
    /// reports `referrer` to its check as rewritten, as storage does.
    ///
    /// # Errors
    /// Returns an error when the state lock is poisoned.
    pub fn put_referrer(&self, item: ItemId, referrer: WorkspacePath) -> Built<()> {
        lock(&self.state)?.referrers.push((item, referrer));
        Ok(())
    }

    /// Forget the warnings kept for every commit so far, as a store that wrote those commits
    /// before it kept warnings would hold them: a replay of one then answers the one warning
    /// that they are not available.
    ///
    /// # Errors
    /// Returns an error when the state lock is poisoned.
    pub fn forget_warnings(&self) -> Built<()> {
        lock(&self.state)?.warnings.clear();
        Ok(())
    }

    /// The warnings the check gave the commit of `mutation`, as the store kept them.
    ///
    /// # Errors
    /// Returns an error when the state lock is poisoned or no warnings were kept for it.
    pub fn kept_warnings(&self, mutation: MutationId) -> Built<Vec<Warning>> {
        lock(&self.state)?
            .warnings
            .iter()
            .find(|(kept, _)| *kept == mutation)
            .map(|(_, warnings)| warnings.clone())
            .ok_or_else(|| "no warnings were kept for that commit".into())
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

    /// Make `document` the one readable at `at` under its identity, in place of any earlier
    /// one, as a test that changes what a commit holds after it was written needs.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn replace_document(&self, at: &Revision, document: ItemDocument) -> Built<()> {
        let mut state = lock(&self.state)?;
        let id = document.summary.id;
        state
            .documents
            .retain(|(revision, held)| !(revision == at && held.summary.id == id));
        state.documents.push((at.clone(), document));
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

    /// Answer `rules` at `at` with `file`, in place of what was scripted there before; a
    /// revision nothing was scripted at answers `None` (no rules saved).
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_rules(&self, at: &Revision, file: RulesFile) -> Built<()> {
        let mut state = lock(&self.state)?;
        state.rules.retain(|(held, _)| held != at);
        state.rules.push((at.clone(), file));
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

    /// Make the next `show` fail with `code`, as a store fault would.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_next_show(&self, code: ErrorCode) -> Built<()> {
        self.fail_show_number(1, code)
    }

    /// Make the `number`-th `show` from now (1 is the next) fail with `code`, as a store
    /// fault in the middle of a handler's reads would; the shows before it answer.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_show_number(&self, number: usize, code: ErrorCode) -> Built<()> {
        *lock(&self.fail_show)? = Some((number, code));
        Ok(())
    }

    /// Make the next `log` fail with `code`, as a store fault would.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_next_log(&self, code: ErrorCode) -> Built<()> {
        *lock(&self.fail_next_log)? = Some(code);
        Ok(())
    }

    /// Make the next `list` fail with `code`, as a store fault would.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_next_list(&self, code: ErrorCode) -> Built<()> {
        *lock(&self.fail_next_list)? = Some(code);
        Ok(())
    }

    /// Make the next `diff` fail with `code`, as a store fault, or a base the store no longer
    /// holds (`NotFound`), would.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_next_diff(&self, code: ErrorCode) -> Built<()> {
        *lock(&self.fail_next_diff)? = Some(code);
        Ok(())
    }

    /// How many times `show` was called.
    #[must_use]
    pub fn shows(&self) -> usize {
        self.shows.load(Ordering::SeqCst)
    }

    /// How many times `tree` was called.
    #[must_use]
    pub fn trees(&self) -> usize {
        self.trees.load(Ordering::SeqCst)
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

    /// Page `list` as storage does (`paged_listing`): a listing, scripted or derived, is
    /// answered `size` entries at a time. Without it, `list` answers the listing whole.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn page_listings(&self, size: usize) -> Built<()> {
        lock(&self.state)?.list_page = Some(size);
        Ok(())
    }

    /// Every `list` call, in order: the revision, the folder and the page asked for.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn list_queries(&self) -> Built<Vec<(Revision, Option<WorkspacePath>, Page)>> {
        Ok(lock(&self.state)?.list_queries.clone())
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
    /// A commit that already exists, answered as the port documents a replay: "A replay
    /// answers the warnings the store kept with that commit, so a retry answers what its first
    /// attempt did; for a commit the store kept no warnings for it answers the one warning
    /// `replay_warnings_unavailable` instead."
    fn replay_of(&self, mutation: MutationId, revision: Revision) -> Committed {
        let warnings = self
            .warnings
            .iter()
            .find(|(kept, _)| *kept == mutation)
            .map_or_else(
                || vec![replay_warnings_unavailable("the fake store kept none")],
                |(_, warnings)| warnings.clone(),
            );
        Committed {
            revision,
            replayed: true,
            warnings,
        }
    }

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

    /// What `show` reads, at the revision a commit of `changes` produces, of each item an edit
    /// wrote, with whether it replaces a document a test scripted there. The edits apply in
    /// order to the items the expected head holds, as the port documents each:
    ///
    /// - `EditItem`: the item's document with the edit's body and properties.
    /// - `MoveItem` ("Move an item"): the item is at its destination afterwards; `NotFound`
    ///   for an item the tree does not hold, and `Conflict` when another item's path has the
    ///   destination's collision key (the item's own does not count, so a move that changes
    ///   only letter case lands, as storage moves it). A move to the path the item holds
    ///   changes nothing.
    /// - `RestoreItem`: the item as `from` holds it, its source appearance (the companion
    ///   files) included; an item absent at `from` is removed. Consecutive `RestoreItem`
    ///   edits are one group: every item of the group is looked up first (`NotFound` for one
    ///   absent both at `from` and in the tree), and the group's items have left their paths
    ///   before any is written back, so only another item holding a restored path (by
    ///   collision key) is a `Conflict`. A later listing of an item wins.
    ///
    /// Every other edit is not applied. The second value lists the items an edit moved: by a
    /// `MoveItem` that changed the path, or by a `RestoreItem` whose `from` holds the item at
    /// another path than the tree held it at.
    fn applied(&self, changes: &CommitChanges) -> Result<Applied, ApiError> {
        let at = |revision: &Revision, item: ItemId| {
            self.documents
                .iter()
                .find(|(held, document)| held == revision && document.summary.id == item)
                .map(|(_, document)| document.clone())
        };
        let mut tree: Vec<ItemDocument> = Vec::new();
        for (held, document) in &self.documents {
            let listed = tree
                .iter()
                .any(|kept| kept.summary.id == document.summary.id);
            if *held == changes.expected_head && !listed {
                tree.push(document.clone());
            }
        }
        let mut written: Vec<(ItemId, bool)> = Vec::new();
        let mut moved: Vec<ItemId> = Vec::new();
        let mut wrote = |item: ItemId, replaces: bool| {
            let before = written
                .iter()
                .any(|(id, replaced)| *id == item && *replaced);
            written.retain(|(id, _)| *id != item);
            written.push((item, replaces || before));
        };
        let restores = |edit: &TreeEdit| matches!(edit, TreeEdit::RestoreItem { .. });
        for group in changes
            .edits
            .chunk_by(|first, second| restores(first) && restores(second))
        {
            match group {
                [
                    TreeEdit::EditItem {
                        item_id,
                        body,
                        properties,
                    },
                ] => {
                    if let Some(edited) = tree.iter_mut().find(|held| held.summary.id == *item_id) {
                        edited.body.clone_from(body);
                        edited.properties.clone_from(properties);
                        wrote(*item_id, true);
                    }
                }
                [
                    TreeEdit::MoveItem {
                        item_id,
                        destination,
                        ..
                    },
                ] => {
                    if move_in(&mut tree, *item_id, destination)? {
                        wrote(*item_id, false);
                        moved.push(*item_id);
                    }
                }
                _ => {
                    // The group's items, each with the document its `from` holds.
                    let listed: Vec<(ItemId, Option<ItemDocument>)> = group
                        .iter()
                        .filter_map(|edit| match edit {
                            TreeEdit::RestoreItem { item_id, from } => {
                                Some((*item_id, at(from, *item_id)))
                            }
                            _ => None,
                        })
                        .collect();
                    for (item_id, earlier) in &listed {
                        let held = tree.iter().find(|held| held.summary.id == *item_id);
                        if let (Some(held), Some(earlier)) = (held, earlier)
                            && held.summary.path != earlier.summary.path
                        {
                            moved.push(*item_id);
                        }
                    }
                    restore_in(&mut tree, &listed)?;
                    for (item_id, _) in &listed {
                        wrote(*item_id, true);
                    }
                }
            }
        }
        let written = written
            .into_iter()
            .filter_map(|(item, replaces)| {
                tree.iter()
                    .find(|held| held.summary.id == item)
                    .map(|held| (held.clone(), replaces))
            })
            .collect();
        Ok((written, moved))
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
    /// The listing of `folder` at `revision` from the documents there, when no test scripted
    /// one: the items directly inside it and its child folders. A folder no document lies
    /// under has an empty listing, as real storage answers it; only an unknown revision is
    /// `NotFound`.
    fn derived_listing(
        &self,
        revision: &Revision,
        folder: Option<&WorkspacePath>,
    ) -> Result<FolderListing, ApiError> {
        self.require_known(revision)?;
        let prefix = folder.map(|folder| format!("{}/", folder.as_str()));
        let mut items = Vec::new();
        let mut folders = std::collections::BTreeSet::new();
        for (_, document) in self.documents.iter().filter(|(at, _)| at == revision) {
            let path = document.summary.path.as_str();
            let rest = match prefix.as_deref() {
                Some(prefix) => match path.strip_prefix(prefix) {
                    Some(rest) => rest,
                    None => continue,
                },
                None => path,
            };
            match rest.split_once('/') {
                None => items.push(document.summary.clone()),
                Some((child, _)) => {
                    let child = match prefix.as_deref() {
                        Some(prefix) => format!("{prefix}{child}"),
                        None => child.to_owned(),
                    };
                    folders.insert(child);
                }
            }
        }
        Ok(FolderListing {
            items,
            folders: folders
                .into_iter()
                .map(WorkspacePath::try_from)
                .collect::<Result<_, _>>()
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?,
            next_cursor: None,
        })
    }

    /// `tree` at `revision` from the documents scripted there: one per item (the first, as
    /// `show` reads it), in path order, each with its header's original name when it is a
    /// source card, and every folder above them. Only an unknown revision is `NotFound`.
    fn tree_at(&self, revision: &Revision) -> Result<WorkspaceTree, ApiError> {
        self.require_known(revision)?;
        let mut items: Vec<TreeItem> = Vec::new();
        for (at, document) in &self.documents {
            if at != revision
                || items
                    .iter()
                    .any(|held| held.summary.id == document.summary.id)
            {
                continue;
            }
            let original_name = ApplicationHeader::from_properties(&document.properties)?
                .and_then(|header| header.source)
                .map(|source| source.original_name);
            items.push(TreeItem {
                summary: document.summary.clone(),
                original_name,
            });
        }
        items.sort_by(|left, right| left.summary.path.as_str().cmp(right.summary.path.as_str()));
        let mut folders = std::collections::BTreeSet::new();
        for item in &items {
            let text = item.summary.path.as_str();
            folders.extend(
                text.match_indices('/')
                    .filter_map(|(at, _)| text.get(..at))
                    .map(str::to_owned),
            );
        }
        Ok(WorkspaceTree {
            items,
            folders: folders
                .into_iter()
                .map(WorkspacePath::try_from)
                .collect::<Result<_, _>>()
                .map_err(internal)?,
        })
    }

    fn require_known(&self, revision: &Revision) -> Result<(), ApiError> {
        let known = self.head.as_ref() == Some(revision)
            || self.commits.iter().any(|(changes, at)| {
                at == revision || &changes.expected_head == revision
            })
            // Moving the head does not erase the revision it moved from, including an empty tree.
            || self.landed.iter().any(|(at, parent)| at == revision || parent == revision)
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

    /// The bytes retained under `digest`, put by a test or by `put`.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn bytes(&self, digest: &Digest) -> Built<Option<Vec<u8>>> {
        Ok(lock(&self.objects)?.get(digest).cloned())
    }

    /// The digest of every `put` that retained its bytes, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn puts(&self) -> Built<Vec<Digest>> {
        Ok(lock(&self.puts)?.clone())
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

    /// Every derived object held, scripted or recorded, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn derived(&self) -> Built<Vec<DerivedObject>> {
        Ok(lock(&self.derived)?.clone())
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
        page: Page,
    ) -> PortFuture<'a, FolderListing> {
        Box::pin(async move {
            let mut state = lock_api(&self.state)?;
            state
                .list_queries
                .push((revision.clone(), folder.cloned(), page.clone()));
            if let Some(code) = lock_api(&self.fail_next_list)?.take() {
                return Err(ApiError::new(code, "the store failed"));
            }
            let whole = match state
                .listings
                .iter()
                .find(|(at, listed, _)| at == revision && listed.as_ref() == folder)
            {
                Some((_, _, listing)) => listing.clone(),
                None => state.derived_listing(revision, folder)?,
            };
            match state.list_page {
                Some(size) => paged_listing(whole, &page, size),
                None => Ok(whole),
            }
        })
    }
    // "Every item and every folder of `revision` in one pass over its tree ... it includes the
    // items under a folder whose name starts with `.`, and it is not paged. `NotFound` only
    // when the revision itself is unknown." Archived items are among them
    // (`WorkspaceTree::items`).
    fn tree<'a>(
        &'a self,
        _scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, WorkspaceTree> {
        self.trees.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { lock_api(&self.state)?.tree_at(revision) })
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
            {
                let mut failing = lock_api(&self.fail_show)?;
                match failing.take() {
                    Some((number, code)) if number <= 1 => {
                        return Err(ApiError::new(code, "the store failed"));
                    }
                    Some((number, code)) => *failing = Some((number.saturating_sub(1), code)),
                    None => {}
                }
            }
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
        revision: &'a Revision,
        path: &'a WorkspacePath,
    ) -> PortFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let state = lock_api(&self.state)?;
            state.require_known(revision)?;
            state
                .documents
                .iter()
                .find(|(at, document)| at == revision && &document.summary.path == path)
                .map(|(_, document)| file_text(document).into_bytes())
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No file at that path here"))
        })
    }
    fn rules<'a>(
        &'a self,
        _scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Option<RulesFile>> {
        // "`None` when no rules were saved there; otherwise the file's text and what reading its
        // bytes as `NamingRules` gave ... `NotFound` only when the revision itself is unknown."
        Box::pin(async move {
            let state = lock_api(&self.state)?;
            state.require_known(revision)?;
            Ok(state
                .rules
                .iter()
                .find(|(at, _)| at == revision)
                .map(|(_, file)| file.clone()))
        })
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
                return Ok(state.replay_of(changes.mutation_id, revision));
            }
            if state.head.as_ref() != Some(&changes.expected_head) {
                // As storage refuses it: `StaleBase` naming the head (`moved_head`).
                let moved = ApiError::new(ErrorCode::Conflict, "the head moved");
                return Err(match state.head.clone() {
                    Some(head) => moved.with_detail(ErrorDetail::StaleBase { head }),
                    None => moved,
                });
            }
            let (written, moved) = state.applied(&changes)?;
            // "`rewritten` names the files of the candidate that the store rewrote on its own
            // while it applied the edits ...: the referrers whose links a move rewrote, for a
            // `MoveItem` and for a `RestoreItem` that moves an item back to its earlier path
            // ... in path order and each once". The fake rewrites nothing, so the referrers
            // are the ones a test scripted for the items this commit moved.
            let mut rewritten: Vec<WorkspacePath> = state
                .referrers
                .iter()
                .filter(|(item, _)| moved.contains(item))
                .map(|(_, referrer)| referrer.clone())
                .collect();
            rewritten.sort_by(|left, right| left.as_str().cmp(right.as_str()));
            rewritten.dedup();
            let warnings = check.check(&self.stage.0, &rewritten)?;
            // "The store keeps those warnings, an empty list included, in a record of its own
            // keyed by the workspace and the mutation, written before the commit lands"; a
            // record an attempt left without a commit is replaced.
            let mutation_id = changes.mutation_id;
            state.warnings.retain(|(kept, _)| *kept != mutation_id);
            state.warnings.push((mutation_id, warnings.clone()));
            if state.queued.is_empty() {
                return Err(ApiError::new(ErrorCode::Internal, "no revision queued"));
            }
            let revision = state.queued.remove(0);
            // What `show` reads of each written item at the new revision. A document a test
            // scripted there stands against a move, which the fake applies to the path only.
            for (mut document, replaces) in written {
                let scripted = state
                    .documents
                    .iter()
                    .any(|(at, held)| *at == revision && held.summary.id == document.summary.id);
                if replaces || !scripted {
                    document.summary.revision = revision.clone();
                    state.documents.insert(0, (revision.clone(), document));
                }
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
    ) -> PortFuture<'a, Option<Committed>> {
        Box::pin(async move {
            let state = lock_api(&self.state)?;
            let found = match since {
                Some(since) => {
                    state.require_known(since)?;
                    state.written_after(mutation_id, since)
                }
                None => state.on_the_line(mutation_id),
            };
            Ok(found.map(|revision| state.replay_of(mutation_id, revision)))
        })
    }
    fn create_candidate<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _proposal_id: ProposalId,
        _changes: CandidateChanges,
        _check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed> {
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
            if let Some(code) = lock_api(&self.fail_next_log)?.take() {
                return Err(ApiError::new(code, "the store failed"));
            }
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
    // "Compare two retained revisions." The comparison is the one a test scripted
    // (`put_diff`); an unscripted one between two known revisions lists no change, which is
    // kinder than a store that compares trees, so a rule that reads a comparison scripts it.
    fn diff<'a>(
        &'a self,
        _scope: &'a StorageScope,
        query: DiffQuery,
    ) -> PortFuture<'a, DiffResponse> {
        Box::pin(async move {
            let mut state = lock_api(&self.state)?;
            state.diff_queries.push(query.clone());
            if let Some(code) = lock_api(&self.fail_next_diff)?.take() {
                return Err(ApiError::new(code, "the store failed"));
            }
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
    fn body_start<'a>(
        &'a self,
        _scope: &'a StorageScope,
        revision: &'a Revision,
        item_id: ItemId,
    ) -> PortFuture<'a, u32> {
        Box::pin(async move {
            let state = lock_api(&self.state)?;
            let document = state
                .documents
                .iter()
                .find(|(at, document)| at == revision && document.summary.id == item_id)
                .map(|(_, document)| document)
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such item"))?;
            body_start_of(document)
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
    /// "Store and hash a bounded stream. Fails with `TooLarge` past `limit`, and with `Conflict`
    /// when `expected` is given and differs from the computed digest; in both cases nothing is
    /// retained. Storing bytes that are already retained writes nothing and returns the
    /// existing identity."
    fn put<'a>(
        &'a self,
        _scope: &'a StorageScope,
        body: ByteReader,
        limit: u64,
        expected: Option<Digest>,
    ) -> PortFuture<'a, ObjectInfo> {
        Box::pin(async move {
            let mut bytes = Vec::new();
            body.take(limit.saturating_add(1))
                .read_to_end(&mut bytes)
                .await
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
            let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            if size > limit {
                return Err(ApiError::new(ErrorCode::TooLarge, "past the limit"));
            }
            let hash = Sha256::digest(&bytes);
            let digest = Digest::try_from(format!("{hash:x}"))
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
            if expected.as_ref().is_some_and(|wanted| *wanted != digest) {
                return Err(ApiError::new(ErrorCode::Conflict, "another digest"));
            }
            lock_api(&self.puts)?.push(digest.clone());
            lock_api(&self.objects)?
                .entry(digest.clone())
                .or_insert(bytes);
            Ok(ObjectInfo { digest, size })
        })
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
    fn job_mutation<'a>(&'a self, _scope: &'a JobScope, _job: JobId) -> PortFuture<'a, MutationId> {
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
    fn get_purge<'a>(&'a self, _tenant: &'a TenantId, _purge: PurgeId) -> PortFuture<'a, Purge> {
        unused()
    }
    fn find_purge<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _target: &'a PurgeTarget,
    ) -> PortFuture<'a, Option<Purge>> {
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
    /// "Unique on (item, revision, digest): a repeated record inserts nothing and returns the
    /// prior one."
    fn record_derived_object<'a>(
        &'a self,
        _scope: &'a StorageScope,
        object: DerivedObject,
    ) -> PortFuture<'a, DerivedObject> {
        Box::pin(async move {
            let mut derived = lock_api(&self.derived)?;
            if let Some(prior) = derived.iter().find(|held| {
                held.item_id == object.item_id
                    && held.revision == object.revision
                    && held.digest == object.digest
            }) {
                return Ok(prior.clone());
            }
            derived.push(object.clone());
            Ok(object)
        })
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
            let matching: Vec<Link> = lock_api(&self.links)?
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
                .cloned()
                .collect();
            let start = query
                .page
                .cursor
                .as_deref()
                .map_or(Ok(0), str::parse::<usize>)
                .map_err(internal)?;
            let limit = usize::from(query.page.limit.max(1));
            let end = start.saturating_add(limit);
            Ok(GetLinksResponse {
                revision: query.revision,
                next_cursor: (end < matching.len()).then(|| end.to_string()),
                links: matching.into_iter().skip(start).take(limit).collect(),
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
                transport: event.transport,
                proposal_id: event.proposal_id,
                count: event.count,
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

impl Backups for Unused {
    fn write_workspace_archive<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
    ) -> PortFuture<'a, WrittenArchive> {
        unused()
    }
    fn write_installation_archive<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo> {
        unused()
    }
    fn open_installation_archive<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _digest: &'a Digest,
        _offset: u64,
        _length: u64,
    ) -> PortFuture<'a, ObjectRead> {
        unused()
    }
    fn restore_import<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _archive: Digest,
        _mutation_id: MutationId,
        _editors: Vec<String>,
    ) -> PortFuture<'a, RestoreReport> {
        unused()
    }
}

impl MutationStore for Unused {
    fn begin<'a>(
        &'a self,
        _key: &'a MutationKey,
        _digest: &'a Digest,
        _workspace: Option<WorkspaceId>,
    ) -> PortFuture<'a, BeginOutcome> {
        unused()
    }
    fn complete(
        &self,
        _lease: MutationLease,
        _response: Value,
        _workspace: Option<WorkspaceId>,
    ) -> PortFuture<'_, ()> {
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
    ) -> PortFuture<'a, PageCount> {
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
        _mutation_id: MutationId,
        _proposal: Proposal,
        _expected: okf_jawn_contract::proposal::ProposalStatus,
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
    fn list<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: UploadQuery,
    ) -> PortFuture<'a, UploadPage> {
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
    fn consume_batch<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _uploads: &'a [UploadId],
        _job: JobId,
    ) -> PortFuture<'a, Vec<UploadRecord>> {
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
        max_upload_bytes: DEFAULT_MAX_UPLOAD_BYTES,
        conversion_defaults: ConversionSettings::default(),
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

/// The item file the fake holds for `document`, as storage writes one: frontmatter of its
/// properties (one `key: <JSON>` line each, which YAML reads as the same value), the closing
/// delimiter and a blank line, then the body ending in a newline; a document without
/// properties is its body alone.
#[must_use]
pub fn file_text(document: &ItemDocument) -> String {
    let mut body = document.body.clone();
    if !body.ends_with('\n') {
        body.push('\n');
    }
    if document.properties.is_empty() {
        return body;
    }
    let mut text = String::from("---\n");
    for (key, value) in &document.properties {
        text.push_str(key);
        text.push_str(": ");
        text.push_str(&value.to_string());
        text.push('\n');
    }
    text.push_str("---\n\n");
    text.push_str(&body);
    text
}

/// The 1-based line of `file_text(document)` on which the body starts, counted as storage
/// counts it: the file's lines less the body's.
///
/// # Errors
/// Returns `Internal` when the line does not fit a `u32`.
pub fn body_start_of(document: &ItemDocument) -> Result<u32, ApiError> {
    let offset = file_text(document)
        .lines()
        .count()
        .saturating_sub(document.body.lines().count());
    u32::try_from(offset.saturating_add(1))
        .map_err(|_| ApiError::new(ErrorCode::Internal, "a line number does not fit"))
}

/// The unified patch between the item files of `old` and `new` (`file_text`; `None` is an
/// absent file), numbered over the whole file, frontmatter included, as libgit2 writes it:
/// hunks with three lines of context, merged when their context would touch, a count of 1
/// omitted and an empty side numbered by the line before it.
#[must_use]
pub fn file_patch(old: Option<&ItemDocument>, new: Option<&ItemDocument>) -> String {
    let old_text = old.map(file_text).unwrap_or_default();
    let new_text = new.map(file_text).unwrap_or_default();
    let before: Vec<&str> = old_text.lines().collect();
    let after: Vec<&str> = new_text.lines().collect();
    let edits = line_diff(&before, &after);
    let name = |document: Option<&ItemDocument>, side: &str| {
        document.map_or_else(
            || "/dev/null".to_owned(),
            |document| format!("{side}/{}", document.summary.path.as_str()),
        )
    };
    let mut patch = format!("--- {}\n+++ {}\n", name(old, "a"), name(new, "b"));
    // Positions: (old lines before, new lines before) each edit.
    let mut positions = Vec::with_capacity(edits.len());
    let (mut at_old, mut at_new) = (0_usize, 0_usize);
    for (edit, _) in &edits {
        positions.push((at_old, at_new));
        match edit {
            Edit::Keep => {
                at_old = at_old.saturating_add(1);
                at_new = at_new.saturating_add(1);
            }
            Edit::Remove => at_old = at_old.saturating_add(1),
            Edit::Add => at_new = at_new.saturating_add(1),
        }
    }
    let changed: Vec<usize> = edits
        .iter()
        .enumerate()
        .filter(|(_, (edit, _))| *edit != Edit::Keep)
        .map(|(index, _)| index)
        .collect();
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for index in changed {
        let start = index.saturating_sub(PATCH_CONTEXT);
        let end = index
            .saturating_add(PATCH_CONTEXT)
            .saturating_add(1)
            .min(edits.len());
        match hunks.last_mut() {
            Some((_, last_end)) if start <= *last_end => *last_end = end,
            _ => hunks.push((start, end)),
        }
    }
    for (start, end) in hunks {
        let slice = edits.get(start..end).unwrap_or_default();
        let (old_before, new_before) = positions.get(start).copied().unwrap_or_default();
        let old_count = slice.iter().filter(|(edit, _)| *edit != Edit::Add).count();
        let new_count = slice
            .iter()
            .filter(|(edit, _)| *edit != Edit::Remove)
            .count();
        patch.push_str("@@ -");
        patch.push_str(&hunk_span(old_before, old_count));
        patch.push_str(" +");
        patch.push_str(&hunk_span(new_before, new_count));
        patch.push_str(" @@\n");
        for (edit, line) in slice {
            let mark = match edit {
                Edit::Keep => ' ',
                Edit::Remove => '-',
                Edit::Add => '+',
            };
            patch.push(mark);
            patch.push_str(line);
            patch.push('\n');
        }
    }
    patch
}

/// One side of a hunk header: `start,count`, the count omitted when 1, and an empty side
/// numbered by the line before it.
fn hunk_span(before: usize, count: usize) -> String {
    match count {
        0 => format!("{before},0"),
        1 => before.saturating_add(1).to_string(),
        _ => format!("{},{count}", before.saturating_add(1)),
    }
}

/// A shortest edit from `before` to `after` by longest common subsequence, removals of a
/// replaced run before its additions.
fn line_diff<'t>(before: &[&'t str], after: &[&'t str]) -> Vec<(Edit, &'t str)> {
    // `common[i][j]`: the longest common subsequence of `before[i..]` and `after[j..]`.
    let mut common =
        vec![vec![0_usize; after.len().saturating_add(1)]; before.len().saturating_add(1)];
    let at = |table: &[Vec<usize>], i: usize, j: usize| {
        table
            .get(i)
            .and_then(|row| row.get(j))
            .copied()
            .unwrap_or(0)
    };
    for i in (0..before.len()).rev() {
        for j in (0..after.len()).rev() {
            let value = if before.get(i) == after.get(j) {
                at(&common, i.saturating_add(1), j.saturating_add(1)).saturating_add(1)
            } else {
                at(&common, i.saturating_add(1), j).max(at(&common, i, j.saturating_add(1)))
            };
            if let Some(cell) = common.get_mut(i).and_then(|row| row.get_mut(j)) {
                *cell = value;
            }
        }
    }
    let mut edits = Vec::new();
    let (mut i, mut j) = (0_usize, 0_usize);
    loop {
        match (before.get(i), after.get(j)) {
            (None, None) => break,
            (Some(old), Some(new)) if old == new => {
                edits.push((Edit::Keep, *old));
                i = i.saturating_add(1);
                j = j.saturating_add(1);
            }
            (Some(old), Some(_))
                if at(&common, i.saturating_add(1), j) >= at(&common, i, j.saturating_add(1)) =>
            {
                edits.push((Edit::Remove, *old));
                i = i.saturating_add(1);
            }
            (Some(old), None) => {
                edits.push((Edit::Remove, *old));
                i = i.saturating_add(1);
            }
            (_, Some(new)) => {
                edits.push((Edit::Add, *new));
                j = j.saturating_add(1);
            }
        }
    }
    edits
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

/// A draft of `item` by `editor` whose properties are its type, `Note`, digested as the item
/// it would commit.
///
/// # Errors
/// Returns when a fixture value does not parse.
pub fn draft_of(item: u128, editor: &str, base: &Revision, body: &str) -> Built<DraftContent> {
    // A draft holds the item's whole property map, its type included, as an editor saves it.
    let properties = BTreeMap::from([("type".to_owned(), Value::String("Note".to_owned()))]);
    Ok(DraftContent {
        draft: Draft {
            item_id: ItemId(Uuid::from_u128(item)),
            editor: editor.to_owned(),
            base_revision: base.clone(),
            content_digest: draft_digest(body, &properties)?,
            saved_at: instant()?,
        },
        body: body.to_owned(),
        properties,
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

/// One page of `whole` as storage pages a folder: its child folders, then its items, from the
/// offset the decimal cursor names, at most `size` entries (or fewer when the page asks for
/// fewer), with the next offset as the cursor while entries remain. A cursor that is no offset
/// is `InvalidInput` on `/page/cursor`, as storage answers it.
fn paged_listing(
    whole: FolderListing,
    page: &Page,
    size: usize,
) -> Result<FolderListing, ApiError> {
    let start = match page.cursor.as_deref() {
        None => 0,
        Some(cursor) => cursor.parse::<usize>().map_err(|_| {
            ApiError::new(ErrorCode::InvalidInput, "not a listing cursor")
                .with_field("/page/cursor")
        })?,
    };
    let limit = size.min(usize::from(page.limit)).max(1);
    let folders = whole.folders.len();
    let total = folders.saturating_add(whole.items.len());
    let end = start.saturating_add(limit).min(total);
    Ok(FolderListing {
        folders: whole
            .folders
            .into_iter()
            .enumerate()
            .filter(|(index, _)| (start..end).contains(index))
            .map(|(_, folder)| folder)
            .collect(),
        items: whole
            .items
            .into_iter()
            .enumerate()
            .filter(|(index, _)| (start..end).contains(&index.saturating_add(folders)))
            .map(|(_, item)| item)
            .collect(),
        next_cursor: (end < total).then(|| end.to_string()),
    })
}

/// Apply one `MoveItem` to the fake's tree, and say whether the item changed path: `NotFound`
/// for an item the tree does not hold, `Conflict` when another item's path has the destination's
/// collision key. The item's own path does not count, so a move that changes only letter case
/// lands, as storage takes it (one move through a parked name, PR #34).
fn move_in(
    tree: &mut [ItemDocument],
    item: ItemId,
    destination: &WorkspacePath,
) -> Result<bool, ApiError> {
    let taken = tree.iter().any(|held| {
        held.summary.id != item && held.summary.path.collision_key() == destination.collision_key()
    });
    let moved = tree
        .iter_mut()
        .find(|held| held.summary.id == item)
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such item to move"))?;
    if moved.summary.path == *destination {
        return Ok(false);
    }
    if taken {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            format!("{} is already taken", destination.as_str()),
        ));
    }
    moved.summary.path = destination.clone();
    Ok(true)
}

/// Apply one group of consecutive `RestoreItem` edits to the fake's tree: `listed` holds each
/// item with the document its `from` holds. Every item is looked up first, the group's items
/// leave their paths, and each is then written back in order (a later listing wins); an item
/// its `from` does not hold stays removed.
fn restore_in(
    tree: &mut Vec<ItemDocument>,
    listed: &[(ItemId, Option<ItemDocument>)],
) -> Result<(), ApiError> {
    for (item, earlier) in listed {
        if earlier.is_none() && tree.iter().all(|held| held.summary.id != *item) {
            return Err(ApiError::new(
                ErrorCode::NotFound,
                "no such item at either revision",
            ));
        }
    }
    tree.retain(|held| listed.iter().all(|(id, _)| *id != held.summary.id));
    for (item, earlier) in listed {
        tree.retain(|held| held.summary.id != *item);
        let Some(document) = earlier else {
            continue;
        };
        let key = document.summary.path.collision_key();
        if tree
            .iter()
            .any(|held| held.summary.path.collision_key() == key)
        {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "another item holds the restored path",
            ));
        }
        tree.push(document.clone());
    }
    Ok(())
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
