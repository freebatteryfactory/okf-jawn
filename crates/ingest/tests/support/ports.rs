//! Minimal fakes of the ports the handler calls: storage's, the converter and the check.
//!
//! Each fake answers only the methods the handler calls, encodes only what the port docs state
//! (the commit replay rule, upload consumption by one job), and records the calls so a test can
//! see that a repeated run repeated no effect. Every other method answers `NotImplemented`, so
//! a handler that starts calling one fails loudly instead of being answered with an invented
//! value.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use okf_jawn_contract::common::{PageRange, Warning};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::extraction::{ConverterIdentity, FailureReason};
use okf_jawn_contract::history::{BlameResponse, Commit, DiffResponse, LogResponse};
use okf_jawn_contract::identity::{
    Digest, ItemId, JobId, MutationId, PurgeId, Revision, TenantId, Timestamp, UploadId,
    WorkspaceId, WorkspacePath,
};
use okf_jawn_contract::item::{ItemDocument, ItemKind, ItemStatus, ItemSummary, TypeDefinition};
use okf_jawn_contract::purge::PurgeReport;
use okf_jawn_contract::search::{GetGraphResponse, GetLinksResponse, SearchResponse};
use okf_jawn_contract::workspace::RestoreReport;
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, ConversionStatus, ConvertedDocument, Converter, ConverterLimits,
    PageCount, WindowCoverage,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::search::{GraphQuery, LinkQuery, SearchIndex, SearchQuery};
use okf_jawn_core::storage::{
    Backups, BlameQuery, BlobStore, ByteReader, CandidateChanges, CandidateCheck, CommitChanges,
    Committed, CommittedFile, CommittedFiles, DiffQuery, FilePage, FolderListing, LocalSource,
    LogQuery, ObjectInfo, ObjectRead, Page, Promotion, Purger, RulesFile, SourceCard, StorageScope,
    TreeEdit, TreeItem, VersionStore, WorkspaceTree, WrittenArchive, replay_warnings_unavailable,
};
use okf_jawn_core::uploads::{NewUpload, UploadPage, UploadQuery, UploadRecord, UploadStore};

/// A `VersionStore` that keeps a line of commits and nothing of their trees.
///
/// `commit` encodes the port doc: a commit carrying the same mutation after the expected head
/// is returned as `replayed`; otherwise a head that moved is `Conflict`. It searches the line
/// from the tip, newest first, as the storage lane's `find_trailer` walk does, so a handler
/// that wrote two commits under one identity is caught replaying the wrong one. `find_commit`
/// searches the same way. A source card whose path collision key another item holds at the
/// head is refused with `Conflict`, as storage refuses a path collision. A commit that is
/// written first runs the caller's check once over a staged directory holding every item of
/// the candidate tree (the items at the expected head with the edits applied), each rendered
/// as a Markdown file of its type, title and body; its error refuses the commit and its warnings are returned, and
/// kept: a replay, by `commit` or by `find_commit`, answers the warnings kept for that commit,
/// or the one `replay_warnings_unavailable` warning for a commit none were kept for (a
/// restore's, someone else's, or any commit once a test called `forget_warnings`).
///
/// Someone else's edit is modelled as a foreign commit (`edit_elsewhere`), which may take a
/// path, or carry tree edits (`edit_elsewhere_with`: an edit, move or deletion of an item);
/// `edit_elsewhere_before_call` makes one land just before the given `commit` call, as
/// another writer racing the handler would.
///
/// `list` lists the items and child folders directly in a folder at a revision, in one page;
/// the handler reads no single file of a committed tree (it rebuilt a replayed commit's warnings
/// that way before the store kept them), so `read_file` answers `NotImplemented`. A portable
/// export reads whole trees through `committed_files`, which answers the files a test set for a
/// revision (`set_tree`) a page at a time, sorted and paged as the port documents; and
/// `write_history`, which answers the object a test set (`set_history`) and records each call.
/// Neither tree is derived from the commits: an export test states its tree.
///
/// `show` and `item_at_path` read an item at a revision by replaying, over the items present at the base
/// (`at_base`), the edits of every commit up to that revision: `WriteSourceCard` writes the
/// whole card (moving it to the card's path, as storage's `write_card` does), `EditItem`
/// replaces body and properties, `MoveItem` the path, `DeleteItem` removes it. An item absent
/// there is `NotFound` for `show` and `None` for `item_at_path`; a revision not on the line is
/// `NotFound` for both.
pub struct FakeVersions {
    /// The head before any commit.
    pub head: Revision,
    /// Commits written, in order, with the revision each made.
    pub commits: Mutex<Vec<(CommitChanges, Revision)>>,
    /// After this many commits are written, the write that reaches it is kept and then the call
    /// fails, as a process that died right after the commit would (once; the count is cleared).
    pub die_after: Mutex<Option<usize>>,
    /// Paths taken by foreign commits: collision key and the item holding it.
    pub taken: Mutex<Vec<(String, ItemId)>>,
    /// `commit` calls made so far, and the call numbers (1-based) a foreign commit precedes.
    pub calls: Mutex<(usize, Vec<usize>)>,
    /// Runs as each `commit` call starts, with its number (a test cancels the job there).
    pub on_commit: Mutex<Option<CommitHook>>,
    /// Items present at the base revision, before any commit.
    pub at_base: Mutex<Vec<SourceCard>>,
    /// The warnings the check gave for each commit this store wrote, by its mutation.
    pub warnings: Mutex<Vec<(MutationId, Vec<Warning>)>>,
    /// Every file of the trees `committed_files` reads, by revision, as a test set them
    /// (`set_tree`); a revision with none set is not on the line.
    pub trees: Mutex<BTreeMap<ExportTreeKey, Vec<CommittedFile>>>,
    /// Retained candidate revisions: present in storage, but outside the accepted line.
    pub candidates: Mutex<Vec<ExportTreeKey>>,
    /// Every page `committed_files` was asked for, in order.
    pub file_pages: Mutex<Vec<FilePage>>,
    /// The object `write_history` answers, as a test set it (`set_history`).
    pub history: Mutex<Option<ObjectInfo>>,
    /// First history object returned for a mutation, retained across retries.
    pub history_writes: Mutex<BTreeMap<MutationId, ObjectInfo>>,
    /// Every `write_history` call: the revision and the mutation.
    pub histories: Mutex<Vec<(Revision, MutationId)>>,
    /// Scoped revisions requested by export enumeration; tests assert the caller's scope.
    pub file_scopes: Mutex<Vec<StorageScope>>,
    /// Runs after a page was read, before the handler observes progress (a controlled cancel).
    pub after_file_page: Mutex<Option<FilePageHook>>,
    /// The naming rules file every revision holds (`put_rules`); `None` is no rules saved.
    pub rules: Mutex<Option<RulesFile>>,
}

/// A version tree is scoped independently from its revision identity.
pub type ExportTreeKey = (String, WorkspaceId, String);
/// Controlled callback after a page is answered; it may cancel the running export.
pub type FilePageHook = Box<dyn Fn(usize) -> Result<(), ApiError> + Send + Sync>;

fn export_tree_key(scope: &StorageScope, revision: &Revision) -> ExportTreeKey {
    (
        scope.tenant_id.as_str().to_owned(),
        scope.workspace_id,
        revision.as_str().to_owned(),
    )
}

/// An `UploadStore` holding uploads and recording which job consumed each.
///
/// `consume` encodes the port doc: an incomplete upload is `InvalidInput`, one another job
/// consumed is `Conflict`, and consuming it again for the same job changes nothing.
/// `consume_batch` encodes its own (the sentences stand above it): all of the batch, or none.
///
/// Both also encode the purge refusal of their port docs, when a test set `purged`:
/// `consume`'s "Refused with `Conflict` naming the purge, before the upload is read, when a
/// purge names the workspace", and `consume_batch`'s "Refused with `Conflict` naming the purge,
/// before any upload is read, when a purge names the workspace". The message is the storage
/// adapter's (`purge::guard::refuse`): "workspace <id> is purged by purge <id> (<state>);
/// nothing is written to it". The call is recorded, no upload is read and none is marked.
#[derive(Default)]
pub struct FakeUploads {
    /// The slots `get` returns.
    pub records: Mutex<Vec<UploadRecord>>,
    /// Uploads marked, by either call: upload and job, each pair once.
    pub consumed: Mutex<Vec<(UploadId, JobId)>>,
    /// Every `consume_batch` call, refused or not: the uploads as listed, and the job.
    pub batches: Mutex<Vec<(Vec<UploadId>, JobId)>>,
    /// Every single `consume` call, refused or not: the upload and the job.
    pub singles: Mutex<Vec<(UploadId, JobId)>>,
    /// Another job that consumes this upload just before the next `consume_batch` is applied,
    /// as a concurrent import landing between a handler's check and its batch would (once).
    pub taken_before_batch: Mutex<Option<(UploadId, JobId)>>,
    /// The purge that names the workspace, when one does: every consumption is then refused.
    pub purged: Mutex<Option<PurgeId>>,
}

/// A `BlobStore` in memory; `materialize` writes the object into a temporary directory.
pub struct FakeBlobs {
    /// Objects by digest.
    pub objects: Mutex<BTreeMap<String, Vec<u8>>>,
    /// Where materialized objects are written.
    pub directory: tempfile::TempDir,
    /// Awaited once, by the next `materialize` of this digest, before it answers.
    pub materialize_hold: Mutex<Option<(Digest, Hold)>>,
    /// Successful puts, excluding direct fixture insertion.
    pub puts: Mutex<Vec<ObjectInfo>>,
}

/// A `Converter` that converts every window of a paginated original, except the windows that
/// start on a page in `capped`, which end at the memory cap.
///
/// `on_page_count` does the same for the page count.
/// `on_window`, when set, runs as each window starts (a test cancels the job there) and says
/// whether the conversion then never ends, as a long window would; such a conversion counts in
/// `dropped` when its future is dropped.
pub struct FakeConverter {
    /// The page count it reports.
    pub pages: Option<u32>,
    /// First pages of the windows that hit the cap.
    pub capped: Vec<u32>,
    /// The bounds it reports.
    pub limits: ConverterLimits,
    /// Windows asked for.
    pub windows: Mutex<Vec<Option<PageRange>>>,
    /// Runs as a window starts; `true` makes that conversion never end.
    pub on_window: Mutex<Option<WindowHook>>,
    /// Runs as the pages are counted; `true` makes the count never end.
    pub on_page_count: Mutex<Option<PageCountHook>>,
    /// Conversions that never ended whose future was dropped.
    pub dropped: Arc<AtomicUsize>,
    /// How the page count ended without a reply, as `DoclingConverter` reports it: the count
    /// answers the document's failed whole conversion (`PageCount::Failed`).
    pub count_failed: Mutex<Option<FailureReason>>,
    /// Windows converted so far; each export names its run, so a conversion repeated on a
    /// later attempt retains different outputs from the first.
    pub runs: AtomicUsize,
    /// Each converted window also produces one picture: `picture.png` in its output directory
    /// (`picture of window <start>, run <run>`), 20 by 10 pixels, captioned `Figure <start>`.
    pub pictures: AtomicBool,
}

/// What a test runs as a window starts: `true` makes that conversion never end.
pub type WindowHook = Box<dyn Fn(Option<&PageRange>) -> bool + Send + Sync>;

/// What a test runs as the pages are counted: `true` makes the count never end.
pub type PageCountHook = Box<dyn Fn() -> bool + Send + Sync>;

/// What a test runs as the `n`-th `commit` call (1-based) starts.
pub type CommitHook = Box<dyn Fn(usize) + Send + Sync>;

/// Counts a never-ending conversion whose future was dropped.
struct DropCount(Arc<AtomicUsize>);

/// A `CandidateCheck` that accepts every tree.
pub struct AcceptAll;

/// A `CandidateCheck` that refuses every tree, as a conformance refusal would.
pub struct RefuseAll;

/// A `SearchIndex` that records the heads it was rebuilt at and the revisions it was asked to
/// index (indexing one again is a no-op, so every call is kept).
#[derive(Default)]
pub struct FakeSearch {
    /// Heads rebuilt, in order.
    pub rebuilt: Mutex<Vec<Revision>>,
    /// Revisions `index_revision` was called with, in order.
    pub indexed: Mutex<Vec<Revision>>,
    /// Awaited as a rebuild starts (a long rebuild).
    pub rebuild_hold: Mutex<Option<Hold>>,
    /// The `n`-th `index_revision` call (1-based, counting the calls that succeeded) fails
    /// with `Unavailable` and indexes nothing; once.
    pub fail_index: Mutex<Option<usize>>,
}

/// `Backups` that write one archive per mutation id and count the writes. A restore writes one
/// commit carrying its mutation into `versions`, once per mutation id.
pub struct FakeBackups {
    /// The archive every write returns.
    pub archive: ObjectInfo,
    /// Writes asked for.
    pub writes: AtomicUsize,
    /// The editors each restore was given.
    pub restored: Mutex<Vec<Vec<String>>>,
    /// Where a restore commits.
    pub versions: Arc<FakeVersions>,
    /// Awaited as a restore starts, before it writes anything (a long restore).
    pub restore_hold: Mutex<Option<Hold>>,
    /// Awaited as either archive write starts (a long backup).
    pub write_hold: Mutex<Option<Hold>>,
}

/// What a long storage call awaits before it answers: a test makes it outlast several lease
/// intervals.
pub type Hold = Arc<dyn Fn() -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A `Purger` that returns one report and counts the calls.
pub struct FakePurger {
    /// The report every call returns.
    pub report: PurgeReport,
    /// Calls made.
    pub calls: AtomicUsize,
    /// Awaited as either purge starts (a long purge).
    pub hold: Mutex<Option<Hold>>,
    /// The failure the next call answers, after its hold, once.
    pub failure: Mutex<Option<ApiError>>,
}

/// The rows `FakeBackups` says each workspace archive it writes left out.
pub const ARCHIVE_ROWS_LEFT_OUT: u32 = 2;

impl Drop for DropCount {
    fn drop(&mut self) {
        let _previous = self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl FakeSearch {
    /// The heads rebuilt so far.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn rebuilt(&self) -> Result<Vec<Revision>, ApiError> {
        self.rebuilt
            .lock()
            .map(|heads| heads.clone())
            .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))
    }

    /// The revisions indexed so far, every call in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn indexed(&self) -> Result<Vec<Revision>, ApiError> {
        self.indexed
            .lock()
            .map(|revisions| revisions.clone())
            .map_err(|_| poisoned())
    }
}

impl FakeVersions {
    /// A store whose head is `head`.
    #[must_use]
    pub fn at(head: Revision) -> Self {
        Self {
            head,
            commits: Mutex::new(Vec::new()),
            die_after: Mutex::new(None),
            taken: Mutex::new(Vec::new()),
            calls: Mutex::new((0, Vec::new())),
            on_commit: Mutex::new(None),
            at_base: Mutex::new(Vec::new()),
            warnings: Mutex::new(Vec::new()),
            trees: Mutex::new(BTreeMap::new()),
            candidates: Mutex::new(Vec::new()),
            file_pages: Mutex::new(Vec::new()),
            history: Mutex::new(None),
            history_writes: Mutex::new(BTreeMap::new()),
            histories: Mutex::new(Vec::new()),
            file_scopes: Mutex::new(Vec::new()),
            after_file_page: Mutex::new(None),
            rules: Mutex::new(None),
        }
    }

    /// Every file of the tree of `revision`, which `committed_files` answers (in any order: the
    /// fake sorts them by path, as the port answers them).
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn set_tree(
        &self,
        scope: &StorageScope,
        revision: &Revision,
        files: Vec<CommittedFile>,
    ) -> Result<(), ApiError> {
        self.trees
            .lock()
            .map_err(|_| poisoned())?
            .insert(export_tree_key(scope, revision), files);
        Ok(())
    }

    /// Retain a candidate's bytes without putting it on the accepted line.
    ///
    /// # Errors
    /// Returns when a fixture lock is poisoned.
    pub fn set_candidate_tree(
        &self,
        scope: &StorageScope,
        revision: &Revision,
        files: Vec<CommittedFile>,
    ) -> Result<(), ApiError> {
        self.set_tree(scope, revision, files)?;
        self.candidates
            .lock()
            .map_err(|_| poisoned())?
            .push(export_tree_key(scope, revision));
        Ok(())
    }

    /// The object `write_history` answers from now on.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn set_history(&self, object: ObjectInfo) -> Result<(), ApiError> {
        *self.history.lock().map_err(|_| poisoned())? = Some(object);
        Ok(())
    }

    /// Every `write_history` call so far: the revision and the mutation.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn histories(&self) -> Result<Vec<(Revision, MutationId)>, ApiError> {
        self.histories
            .lock()
            .map(|calls| calls.clone())
            .map_err(|_| poisoned())
    }

    /// Every page `committed_files` was asked for so far.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn file_pages(&self) -> Result<Vec<FilePage>, ApiError> {
        self.file_pages
            .lock()
            .map(|pages| pages.clone())
            .map_err(|_| poisoned())
    }

    /// One page of the tree of `revision`, as the port documents it: files in byte order of
    /// path after `page.after`, at most `page.limit` (clamped to 1..=500), ending with the first
    /// file that takes the page's bytes past `page.max_bytes`; `NotFound` for a revision not on
    /// the line (here: one with no tree set).
    fn page_of(
        &self,
        scope: &StorageScope,
        revision: &Revision,
        page: &FilePage,
    ) -> Result<CommittedFiles, ApiError> {
        self.file_pages
            .lock()
            .map_err(|_| poisoned())?
            .push(page.clone());
        let key = export_tree_key(scope, revision);
        if self
            .candidates
            .lock()
            .map_err(|_| poisoned())?
            .contains(&key)
        {
            return Err(ApiError::new(
                ErrorCode::NotFound,
                "candidate is not on the accepted line",
            ));
        }
        let mut files = self
            .trees
            .lock()
            .map_err(|_| poisoned())?
            .get(&key)
            .cloned()
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such revision on the line"))?;
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let limit = usize::from(page.limit.clamp(1, 500));
        let mut remaining = files
            .into_iter()
            .filter(|file| {
                page.after
                    .as_deref()
                    .is_none_or(|after| file.path.as_str() > after)
            })
            .peekable();
        let mut answered = Vec::new();
        let mut held: u64 = 0;
        for file in remaining.by_ref() {
            held = held.saturating_add(u64::try_from(file.bytes.len()).unwrap_or(u64::MAX));
            answered.push(file);
            if answered.len() >= limit || held > page.max_bytes {
                break;
            }
        }
        let next_after = if remaining.peek().is_some() {
            answered.last().map(|file| file.path.clone())
        } else {
            None
        };
        Ok(CommittedFiles {
            files: answered,
            next_after,
        })
    }

    /// Hold `file` as the naming rules file at every revision.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_rules(&self, file: RulesFile) -> Result<(), ApiError> {
        *self.rules.lock().map_err(|_| poisoned())? = Some(file);
        Ok(())
    }

    /// Forget the warnings kept for every commit so far, as a store that wrote those commits
    /// before it kept warnings would hold them.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn forget_warnings(&self) -> Result<(), ApiError> {
        self.warnings.lock().map_err(|_| poisoned())?.clear();
        Ok(())
    }

    /// A commit that already exists, answered as the port documents a replay: "A replay
    /// answers the warnings the store kept with that commit, so a retry answers what its first
    /// attempt did; for a commit the store kept no warnings for it answers the one warning
    /// `replay_warnings_unavailable` instead."
    fn replay_of(&self, mutation: MutationId, revision: Revision) -> Result<Committed, ApiError> {
        let warnings = self
            .warnings
            .lock()
            .map_err(|_| poisoned())?
            .iter()
            .find(|(kept, _)| *kept == mutation)
            .map_or_else(
                || vec![replay_warnings_unavailable("the fake store kept none")],
                |(_, warnings)| warnings.clone(),
            );
        Ok(Committed {
            revision,
            replayed: true,
            warnings,
        })
    }

    /// Someone else commits now; when `path` is given, an item of theirs takes it.
    ///
    /// # Errors
    /// Returns when a lock is poisoned or the revision cannot be made.
    pub fn edit_elsewhere(&self, path: Option<(&WorkspacePath, ItemId)>) -> Result<(), ApiError> {
        self.foreign_commit(path, Vec::new())
    }

    /// Someone else commits these tree edits now (a person editing, moving or deleting).
    ///
    /// # Errors
    /// Returns when a lock is poisoned or the revision cannot be made.
    pub fn edit_elsewhere_with(&self, edits: Vec<TreeEdit>) -> Result<(), ApiError> {
        self.foreign_commit(None, edits)
    }

    /// The item as it stands at `revision`, or `None` when it is absent there.
    ///
    /// # Errors
    /// Returns `NotFound` when the revision is not on the line, or when a lock is poisoned.
    pub fn item_at(
        &self,
        revision: &Revision,
        item: ItemId,
    ) -> Result<Option<SourceCard>, ApiError> {
        Ok(self
            .items_at(revision)?
            .into_iter()
            .find(|card| card.item_id == item))
    }

    /// Every item as it stands at `revision`.
    ///
    /// # Errors
    /// Returns `NotFound` when the revision is not on the line, or when a lock is poisoned.
    pub fn items_at(&self, revision: &Revision) -> Result<Vec<SourceCard>, ApiError> {
        let mut items: Vec<SourceCard> = self.at_base.lock().map_err(|_| poisoned())?.clone();
        if *revision != self.head {
            let commits = self.commits.lock().map_err(|_| poisoned())?;
            let upto = commits
                .iter()
                .position(|(_, made)| made == revision)
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such revision"))?;
            for (changes, _) in commits.iter().take(upto.saturating_add(1)) {
                for edit in &changes.edits {
                    apply_to_items(&mut items, edit);
                }
            }
        }
        Ok(items)
    }

    /// A restore's commit under `mutation_id`, on the head; nothing when one carries it already.
    ///
    /// # Errors
    /// Returns when a lock is poisoned or the revision cannot be made.
    pub fn restore_commit(&self, mutation_id: MutationId) -> Result<(), ApiError> {
        let mut commits = self.commits.lock().map_err(|_| poisoned())?;
        if commits
            .iter()
            .any(|(changes, _)| changes.mutation_id == mutation_id)
        {
            return Ok(());
        }
        let head = commits
            .last()
            .map_or_else(|| self.head.clone(), |(_, revision)| revision.clone());
        let revision = Revision::try_from(format!("{:040x}", commits.len().saturating_add(1)))
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        commits.push((
            CommitChanges {
                mutation_id,
                expected_head: head,
                author: okf_jawn_core::storage::Provenance {
                    subject: "restore".to_owned(),
                    route: okf_jawn_contract::access::AccessRoute::LocalOwner,
                    client_id: None,
                },
                message: "Restore a workspace archive".to_owned(),
                edits: Vec::new(),
            },
            revision,
        ));
        Ok(())
    }

    /// A commit by someone else, on the head.
    fn foreign_commit(
        &self,
        path: Option<(&WorkspacePath, ItemId)>,
        edits: Vec<TreeEdit>,
    ) -> Result<(), ApiError> {
        let mut commits = self.commits.lock().map_err(|_| poisoned())?;
        let head = commits
            .last()
            .map_or_else(|| self.head.clone(), |(_, revision)| revision.clone());
        let number = commits.len().saturating_add(1);
        let mutation_id = serde_json::from_value::<MutationId>(serde_json::Value::String(format!(
            "ffffffff-ffff-ffff-ffff-{number:012}"
        )))
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        let revision = Revision::try_from(format!("{number:040x}"))
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        commits.push((
            CommitChanges {
                mutation_id,
                expected_head: head,
                author: okf_jawn_core::storage::Provenance {
                    subject: "someone-else".to_owned(),
                    route: okf_jawn_contract::access::AccessRoute::LocalOwner,
                    client_id: None,
                },
                message: "An edit by someone else".to_owned(),
                edits,
            },
            revision,
        ));
        if let Some((path, item)) = path {
            self.taken
                .lock()
                .map_err(|_| poisoned())?
                .push((path.collision_key(), item));
        }
        Ok(())
    }

    /// Someone else commits just before the `call`-th `commit` call (1-based) is applied.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn edit_elsewhere_before_call(&self, call: usize) -> Result<(), ApiError> {
        self.calls.lock().map_err(|_| poisoned())?.1.push(call);
        Ok(())
    }

    /// Count one `commit` call and land the foreign commit planned before it, if any.
    fn count_call(&self) -> Result<(), ApiError> {
        let (call, race) = {
            let mut calls = self.calls.lock().map_err(|_| poisoned())?;
            calls.0 = calls.0.saturating_add(1);
            let call = calls.0;
            (call, calls.1.contains(&call))
        };
        if let Some(hook) = self.on_commit.lock().map_err(|_| poisoned())?.as_ref() {
            hook(call);
        }
        if race {
            self.edit_elsewhere(None)?;
        }
        Ok(())
    }

    /// The commit carrying `mutation_id` after `since` on the line (the whole line for `None`),
    /// newest first; a `since` the line never held is `NotFound`, as storage answers.
    fn found(
        &self,
        mutation_id: MutationId,
        since: Option<&Revision>,
    ) -> Result<Option<Revision>, ApiError> {
        let commits = self.commits.lock().map_err(|_| poisoned())?;
        let after: Vec<&(CommitChanges, Revision)> = match since {
            None => commits.iter().collect(),
            Some(since) if *since == self.head => commits.iter().collect(),
            Some(since) => {
                if !commits.iter().any(|(_, revision)| revision == since) {
                    return Err(ApiError::new(
                        ErrorCode::NotFound,
                        "the line never held that revision",
                    ));
                }
                commits
                    .iter()
                    .skip_while(|(_, revision)| revision != since)
                    .skip(1)
                    .collect()
            }
        };
        Ok(after
            .iter()
            .rev()
            .find(|(earlier, _)| earlier.mutation_id == mutation_id)
            .map(|(_, revision)| revision.clone()))
    }

    /// The line from `query.tip` back to the base revision, newest first, one page of it; the
    /// cursor is how many commits were already returned.
    fn history(&self, query: &LogQuery) -> Result<LogResponse, ApiError> {
        let commits = self.commits.lock().map_err(|_| poisoned())?;
        let mut line: Vec<(Revision, String, String)> =
            vec![(self.head.clone(), "Base".to_owned(), "owner".to_owned())];
        line.extend(commits.iter().map(|(changes, revision)| {
            (
                revision.clone(),
                changes.message.clone(),
                changes.author.subject.clone(),
            )
        }));
        let tip = line
            .iter()
            .position(|(revision, _, _)| *revision == query.tip)
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such revision"))?;
        let skip = query
            .page
            .cursor
            .as_deref()
            .map_or(Ok(0), str::parse::<usize>)
            .map_err(|error| ApiError::new(ErrorCode::InvalidInput, error.to_string()))?;
        let at = Timestamp::try_from("2026-10-09T00:00:00.000Z".to_owned())
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        let newest_first: Vec<Commit> = (0..=tip)
            .rev()
            .skip(skip)
            .take(usize::from(query.page.limit))
            .filter_map(|index| {
                let (revision, message, author) = line.get(index)?.clone();
                let parents = index
                    .checked_sub(1)
                    .and_then(|parent| line.get(parent))
                    .map(|(parent, _, _)| vec![parent.clone()])
                    .unwrap_or_default();
                Some(Commit {
                    revision,
                    parents,
                    message,
                    author,
                    committed_at: at.clone(),
                })
            })
            .collect();
        let returned = skip.saturating_add(newest_first.len());
        Ok(LogResponse {
            next_cursor: (returned <= tip).then(|| returned.to_string()),
            commits: newest_first,
        })
    }

    /// Die right after the `count`-th commit is written.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn die_after(&self, count: usize) -> Result<(), ApiError> {
        *self.die_after.lock().map_err(|_| poisoned())? = Some(count);
        Ok(())
    }

    /// The commits written so far.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn written(&self) -> Result<Vec<(CommitChanges, Revision)>, ApiError> {
        self.commits
            .lock()
            .map(|commits| commits.clone())
            .map_err(|_| poisoned())
    }

    /// Apply the port's replay, head and path-collision rules to one commit.
    fn apply(
        &self,
        changes: CommitChanges,
        check: &dyn CandidateCheck,
    ) -> Result<Committed, ApiError> {
        self.count_call()?;
        // A commit's replay scan never refuses an unknown head; the head rules below answer it.
        let replay = match self.found(changes.mutation_id, Some(&changes.expected_head)) {
            Err(error) if error.code == ErrorCode::NotFound => None,
            other => other?,
        };
        if let Some(revision) = replay {
            return self.replay_of(changes.mutation_id, revision);
        }
        // The candidate tree: the items at the expected head with the edits applied.
        let mut candidate = self.items_at(&changes.expected_head).unwrap_or_default();
        for edit in &changes.edits {
            apply_to_items(&mut candidate, edit);
        }
        let mut commits = self.commits.lock().map_err(|_| poisoned())?;
        let head = commits
            .last()
            .map_or_else(|| self.head.clone(), |(_, revision)| revision.clone());
        if changes.expected_head != head {
            // As storage refuses it: `StaleBase` naming the head.
            return Err(ApiError::new(ErrorCode::Conflict, "the head moved")
                .with_detail(ErrorDetail::StaleBase { head }));
        }
        let mut holders: Vec<(String, ItemId)> = self.taken.lock().map_err(|_| poisoned())?.clone();
        holders.extend(commits.iter().flat_map(|(earlier, _)| {
            earlier.edits.iter().filter_map(|edit| match edit {
                TreeEdit::WriteSourceCard(card) => Some((card.path.collision_key(), card.item_id)),
                _ => None,
            })
        }));
        for edit in &changes.edits {
            if let TreeEdit::WriteSourceCard(card) = edit
                && holders
                    .iter()
                    .any(|(key, item)| *key == card.path.collision_key() && *item != card.item_id)
            {
                return Err(ApiError::new(
                    ErrorCode::Conflict,
                    format!("{} collides with another item", card.path.as_str()),
                ));
            }
        }
        let warnings = staged_check(&candidate, check)?;
        // "The store keeps those warnings, an empty list included, in a record of its own
        // keyed by the workspace and the mutation, written before the commit lands": so they
        // are kept here, before the commit is pushed and before the store can die after it.
        {
            let mut kept = self.warnings.lock().map_err(|_| poisoned())?;
            kept.retain(|(mutation, _)| *mutation != changes.mutation_id);
            kept.push((changes.mutation_id, warnings.clone()));
        }
        let revision = Revision::try_from(format!("{:040x}", commits.len().saturating_add(1)))
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        commits.push((changes, revision.clone()));
        let mut die_after = self.die_after.lock().map_err(|_| poisoned())?;
        if *die_after == Some(commits.len()) {
            *die_after = None;
            return Err(ApiError::new(
                ErrorCode::Unavailable,
                "the process died after the commit was written",
            ));
        }
        Ok(Committed {
            revision,
            replayed: false,
            warnings,
        })
    }
}

impl FakeUploads {
    /// The consumptions so far.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn consumed(&self) -> Result<Vec<(UploadId, JobId)>, ApiError> {
        self.consumed
            .lock()
            .map(|consumed| consumed.clone())
            .map_err(|_| poisoned())
    }

    /// Every `consume_batch` call so far, refused or not.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn batches(&self) -> Result<Vec<(Vec<UploadId>, JobId)>, ApiError> {
        self.batches
            .lock()
            .map(|batches| batches.clone())
            .map_err(|_| poisoned())
    }

    /// Every single `consume` call so far, refused or not.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn singles(&self) -> Result<Vec<(UploadId, JobId)>, ApiError> {
        self.singles
            .lock()
            .map(|singles| singles.clone())
            .map_err(|_| poisoned())
    }

    /// The refusal of a consumption in `scope` when a purge names the workspace, as the
    /// storage adapter words it (`purge::guard::refuse`).
    fn refuse_purged(&self, scope: &StorageScope) -> Result<(), ApiError> {
        match *self.purged.lock().map_err(|_| poisoned())? {
            Some(purge) => Err(ApiError::new(
                ErrorCode::Conflict,
                format!(
                    "workspace {} is purged by purge {} (recorded); nothing is written to it",
                    scope.workspace_id.0, purge.0
                ),
            )),
            None => Ok(()),
        }
    }

    /// The port's rule for `consume_batch`, on the fake's records.
    fn take_batch(
        &self,
        scope: &StorageScope,
        uploads: &[UploadId],
        job: JobId,
    ) -> Result<Vec<UploadRecord>, ApiError> {
        self.batches
            .lock()
            .map_err(|_| poisoned())?
            .push((uploads.to_vec(), job));
        // "Refused with `Conflict` naming the purge, before any upload is read, when a purge
        // names the workspace."
        self.refuse_purged(scope)?;
        let mut records = self.records.lock().map_err(|_| poisoned())?;
        let mut consumed = self.consumed.lock().map_err(|_| poisoned())?;
        if let Some((upload, taker)) = self
            .taken_before_batch
            .lock()
            .map_err(|_| poisoned())?
            .take()
            && let Some(record) = records.iter_mut().find(|record| record.id == upload)
        {
            record.consumed_by = Some(taker);
            consumed.push((upload, taker));
        }
        // "The uploads are checked in the order given before any is marked, and the first that
        // cannot be taken refuses the batch, which then marks none: `NotFound` for one the
        // workspace does not hold, `InvalidInput` for an incomplete one, `Conflict` for one
        // another job consumed."
        for upload in uploads {
            let record = records
                .iter()
                .find(|record| record.id == *upload)
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such upload"))?;
            if record.object.is_none() {
                return Err(ApiError::new(
                    ErrorCode::InvalidInput,
                    "an upload of the batch is not complete",
                ));
            }
            if record.consumed_by.is_some_and(|consumer| consumer != job) {
                return Err(ApiError::new(
                    ErrorCode::Conflict,
                    "an upload of the batch was consumed by another job",
                ));
            }
        }
        // "An upload `job` already consumed stays as it is, so repeating the batch changes
        // nothing and returns the same records. An upload listed twice is the same upload at
        // each place it is listed. Returns the records in the order given."
        let mut taken = Vec::with_capacity(uploads.len());
        for upload in uploads {
            let record = records
                .iter_mut()
                .find(|record| record.id == *upload)
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such upload"))?;
            record.consumed_by = Some(job);
            if !consumed.contains(&(*upload, job)) {
                consumed.push((*upload, job));
            }
            taken.push(record.clone());
        }
        Ok(taken)
    }
}

impl FakeBlobs {
    /// An empty store.
    ///
    /// # Errors
    /// Returns when the temporary directory cannot be made.
    pub fn new() -> Result<Self, std::io::Error> {
        Ok(Self {
            objects: Mutex::new(BTreeMap::new()),
            directory: tempfile::tempdir()?,
            materialize_hold: Mutex::new(None),
            puts: Mutex::new(Vec::new()),
        })
    }

    /// Store bytes directly, as an upload would have.
    ///
    /// # Errors
    /// Returns when the lock is poisoned or the digest cannot be made.
    pub fn insert(&self, bytes: &[u8]) -> Result<ObjectInfo, ApiError> {
        let digest = okf_jawn_ingest::record::sha256_digest(bytes)?;
        self.objects
            .lock()
            .map_err(|_| poisoned())?
            .insert(digest.as_str().to_owned(), bytes.to_vec());
        Ok(ObjectInfo {
            digest,
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        })
    }

    /// The bytes stored under a digest.
    ///
    /// # Errors
    /// Returns `NotFound` when absent.
    pub fn bytes(&self, digest: &Digest) -> Result<Vec<u8>, ApiError> {
        self.objects
            .lock()
            .map_err(|_| poisoned())?
            .get(digest.as_str())
            .cloned()
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such object"))
    }

    /// Successfully retained puts; fixture insertion never counts as an export write.
    ///
    /// # Errors
    /// Returns when the fixture lock is poisoned.
    pub fn puts(&self) -> Result<Vec<ObjectInfo>, ApiError> {
        self.puts
            .lock()
            .map(|puts| puts.clone())
            .map_err(|_| poisoned())
    }

    /// Remove a fixture object to model a missing reference, without changing a card.
    ///
    /// # Errors
    /// Returns when the fixture lock is poisoned.
    pub fn forget(&self, digest: &Digest) -> Result<(), ApiError> {
        self.objects
            .lock()
            .map_err(|_| poisoned())?
            .remove(digest.as_str());
        Ok(())
    }
}

impl VersionStore for FakeVersions {
    fn head<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Revision> {
        let head = self.commits.lock().map_err(|_| poisoned()).map(|commits| {
            commits
                .last()
                .map_or_else(|| self.head.clone(), |(_, revision)| revision.clone())
        });
        Box::pin(async move { head })
    }
    fn list<'a>(
        &'a self,
        _scope: &'a StorageScope,
        revision: &'a Revision,
        folder: Option<&'a WorkspacePath>,
        _page: Page,
    ) -> PortFuture<'a, FolderListing> {
        let listed = self
            .items_at(revision)
            .and_then(|items| listing(&items, folder, revision));
        Box::pin(async move { listed })
    }
    fn item_at_path<'a>(
        &'a self,
        _scope: &'a StorageScope,
        revision: &'a Revision,
        path: &'a WorkspacePath,
    ) -> PortFuture<'a, Option<ItemSummary>> {
        // The port doc: `Ok(None)` when no item is at the path at that revision; `NotFound`
        // only for a revision the line does not hold (`items_at`).
        let found = self.items_at(revision).and_then(|items| {
            items
                .iter()
                .find(|card| card.path == *path)
                .map(|card| document_of(card, revision).map(|document| document.summary))
                .transpose()
        });
        Box::pin(async move { found })
    }
    fn show<'a>(
        &'a self,
        _scope: &'a StorageScope,
        revision: &'a Revision,
        item: ItemId,
    ) -> PortFuture<'a, ItemDocument> {
        let shown = self.item_at(revision, item).and_then(|card| {
            let card = card.ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such item"))?;
            document_of(&card, revision)
        });
        Box::pin(async move { shown })
    }
    fn read_file<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _path: &'a WorkspacePath,
    ) -> PortFuture<'a, Vec<u8>> {
        unused("read_file")
    }
    fn tree<'a>(
        &'a self,
        _scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, WorkspaceTree> {
        // The port doc: every item and folder of the revision, in path order; `NotFound` only
        // for a revision the line does not hold (`items_at`).
        let tree = self
            .items_at(revision)
            .and_then(|items| tree_of(&items, revision));
        Box::pin(async move { tree })
    }
    fn rules<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
    ) -> PortFuture<'a, Option<RulesFile>> {
        // The port doc: `None` when no rules were saved; the file's text and its reading
        // otherwise. The fake holds one file for every revision.
        let held = self
            .rules
            .lock()
            .map_err(|_| poisoned())
            .map(|rules| rules.clone());
        Box::pin(async move { held })
    }
    fn types<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
    ) -> PortFuture<'a, Vec<TypeDefinition>> {
        unused("types")
    }
    fn correction<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _item: ItemId,
        _digest: &'a Digest,
    ) -> PortFuture<'a, Option<String>> {
        unused("correction")
    }
    fn commit<'a>(
        &'a self,
        _scope: &'a StorageScope,
        changes: CommitChanges,
        check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed> {
        let committed = self.apply(changes, check.as_ref());
        Box::pin(async move { committed })
    }
    fn find_commit<'a>(
        &'a self,
        _scope: &'a StorageScope,
        mutation_id: MutationId,
        since: Option<&'a Revision>,
    ) -> PortFuture<'a, Option<Committed>> {
        let found = self.found(mutation_id, since).and_then(|found| {
            found
                .map(|revision| self.replay_of(mutation_id, revision))
                .transpose()
        });
        Box::pin(async move { found })
    }
    fn create_candidate<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _proposal_id: okf_jawn_contract::identity::ProposalId,
        _changes: CandidateChanges,
        _check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed> {
        unused("create_candidate")
    }
    fn promote_candidate<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _promotion: Promotion,
    ) -> PortFuture<'a, Committed> {
        unused("promote_candidate")
    }
    fn body_start<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _item_id: ItemId,
    ) -> PortFuture<'a, u32> {
        unused("body_start")
    }
    fn log<'a>(&'a self, _scope: &'a StorageScope, query: LogQuery) -> PortFuture<'a, LogResponse> {
        let logged = self.history(&query);
        Box::pin(async move { logged })
    }
    fn diff<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: DiffQuery,
    ) -> PortFuture<'a, DiffResponse> {
        unused("diff")
    }
    fn blame<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: BlameQuery,
    ) -> PortFuture<'a, BlameResponse> {
        unused("blame")
    }
    // The port doc: "It holds the commits reachable from `revision` on the accepted line and
    // nothing else ... Idempotent on `mutation_id`: a repeated id writes nothing and returns the
    // object the first call wrote." A revision with no tree set is not on the line: `NotFound`.
    fn write_history<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo> {
        let written = (|| {
            self.histories
                .lock()
                .map_err(|_| poisoned())?
                .push((revision.clone(), mutation_id));
            if !self
                .trees
                .lock()
                .map_err(|_| poisoned())?
                .contains_key(&export_tree_key(scope, revision))
            {
                return Err(ApiError::new(
                    ErrorCode::NotFound,
                    "No revision with that identity on the accepted line here",
                ));
            }
            if self
                .candidates
                .lock()
                .map_err(|_| poisoned())?
                .contains(&export_tree_key(scope, revision))
            {
                return Err(ApiError::new(
                    ErrorCode::NotFound,
                    "candidate history is not on the accepted line",
                ));
            }
            let mut writes = self.history_writes.lock().map_err(|_| poisoned())?;
            if let Some(prior) = writes.get(&mutation_id) {
                return Ok(prior.clone());
            }
            let object = self
                .history
                .lock()
                .map_err(|_| poisoned())?
                .clone()
                .ok_or_else(|| ApiError::new(ErrorCode::NotImplemented, "no history was set"))?;
            writes.insert(mutation_id, object.clone());
            Ok(object)
        })();
        Box::pin(async move { written })
    }
    // The port doc: "Every file of `revision`'s tree, a page at a time, in byte order of path
    // ... `NotFound` for a revision this repository does not hold or that is not on the
    // accepted line" (`page_of`).
    fn committed_files<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        page: FilePage,
    ) -> PortFuture<'a, CommittedFiles> {
        Box::pin(async move {
            self.file_scopes
                .lock()
                .map_err(|_| poisoned())?
                .push(scope.clone());
            let answered = self.page_of(scope, revision, &page)?;
            let calls = self.file_pages.lock().map_err(|_| poisoned())?.len();
            if let Some(hook) = self
                .after_file_page
                .lock()
                .map_err(|_| poisoned())?
                .as_ref()
            {
                hook(calls)?;
            }
            Ok(answered)
        })
    }
}

impl SearchIndex for FakeSearch {
    fn index_revision<'a>(
        &'a self,
        _scope: &'a StorageScope,
        revision: Revision,
    ) -> PortFuture<'a, ()> {
        let recorded = self
            .fail_index
            .lock()
            .map_err(|_| poisoned())
            .and_then(|mut fail| {
                let mut revisions = self.indexed.lock().map_err(|_| poisoned())?;
                if *fail == Some(revisions.len().saturating_add(1)) {
                    *fail = None;
                    return Err(ApiError::new(
                        ErrorCode::Unavailable,
                        "the index could not be written",
                    ));
                }
                revisions.push(revision);
                Ok(())
            });
        Box::pin(async move { recorded })
    }
    fn search<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: SearchQuery,
    ) -> PortFuture<'a, SearchResponse> {
        unused("search")
    }
    fn links<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: LinkQuery,
    ) -> PortFuture<'a, GetLinksResponse> {
        unused("links")
    }
    fn graph<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: GraphQuery,
    ) -> PortFuture<'a, GetGraphResponse> {
        unused("graph")
    }
    fn rebuild<'a>(&'a self, _scope: &'a StorageScope, head: Revision) -> PortFuture<'a, ()> {
        let hold = hold_of(&self.rebuild_hold);
        Box::pin(async move {
            held(hold).await?;
            self.rebuilt
                .lock()
                .map(|mut heads| heads.push(head))
                .map_err(|_| poisoned())
        })
    }
}

impl Backups for FakeBackups {
    fn write_workspace_archive<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
    ) -> PortFuture<'a, WrittenArchive> {
        let _previous = self.writes.fetch_add(1, Ordering::SeqCst);
        let archive = self.archive.clone();
        let hold = hold_of(&self.write_hold);
        Box::pin(async move {
            held(hold).await?;
            Ok(WrittenArchive {
                object: archive,
                rows_left_out: ARCHIVE_ROWS_LEFT_OUT,
            })
        })
    }
    fn write_installation_archive<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo> {
        let _previous = self.writes.fetch_add(1, Ordering::SeqCst);
        let archive = self.archive.clone();
        let hold = hold_of(&self.write_hold);
        Box::pin(async move {
            held(hold).await?;
            Ok(archive)
        })
    }
    fn open_installation_archive<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _digest: &'a Digest,
        _offset: u64,
        _length: u64,
    ) -> PortFuture<'a, ObjectRead> {
        unused("open_installation_archive")
    }
    fn restore_import<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _archive: Digest,
        mutation_id: MutationId,
        editors: Vec<String>,
    ) -> PortFuture<'a, RestoreReport> {
        let hold = hold_of(&self.restore_hold);
        Box::pin(async move {
            held(hold).await?;
            self.restored
                .lock()
                .map(|mut restored| restored.push(editors))
                .map_err(|_| poisoned())
                .and_then(|()| self.versions.restore_commit(mutation_id))
                .map(|()| RestoreReport {
                    items: 3,
                    drafts_restored: 1,
                    drafts_unassigned: 1,
                    rows_left_out: None,
                })
        })
    }
}

impl FakePurger {
    /// Make the next call answer `error`, after its hold, once.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_next(&self, error: ApiError) -> Result<(), ApiError> {
        *self.failure.lock().map_err(|_| poisoned())? = Some(error);
        Ok(())
    }

    /// The failure a test asked of this call, taken.
    fn failed(&self) -> Result<(), ApiError> {
        match self.failure.lock().map_err(|_| poisoned())?.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Purger for FakePurger {
    fn purge_workspace<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _mutation_id: MutationId,
        _purge: PurgeId,
        _workspace: WorkspaceId,
    ) -> PortFuture<'a, PurgeReport> {
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
        let report = self.report.clone();
        let hold = hold_of(&self.hold);
        Box::pin(async move {
            held(hold).await?;
            self.failed()?;
            Ok(report)
        })
    }
    fn purge_item<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _purge: PurgeId,
        _item: ItemId,
    ) -> PortFuture<'a, PurgeReport> {
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
        let report = self.report.clone();
        let hold = hold_of(&self.hold);
        Box::pin(async move {
            held(hold).await?;
            self.failed()?;
            Ok(report)
        })
    }
}

impl UploadStore for FakeUploads {
    fn list<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: UploadQuery,
    ) -> PortFuture<'a, UploadPage> {
        unused("list")
    }
    fn create<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _upload: NewUpload,
    ) -> PortFuture<'a, UploadRecord> {
        unused("create")
    }
    fn get<'a>(
        &'a self,
        _scope: &'a StorageScope,
        upload: UploadId,
    ) -> PortFuture<'a, UploadRecord> {
        let found = self
            .records
            .lock()
            .map_err(|_| poisoned())
            .and_then(|records| {
                records
                    .iter()
                    .find(|record| record.id == upload)
                    .cloned()
                    .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such upload"))
            });
        Box::pin(async move { found })
    }
    fn put_content<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _upload: UploadId,
        _body: ByteReader,
        _limit: u64,
    ) -> PortFuture<'a, UploadRecord> {
        unused("put_content")
    }
    fn complete<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _upload: UploadId,
        _sha256: Digest,
    ) -> PortFuture<'a, UploadRecord> {
        unused("complete")
    }
    fn consume<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
        job: JobId,
    ) -> PortFuture<'a, UploadRecord> {
        let taken = self
            .singles
            .lock()
            .map(|mut singles| singles.push((upload, job)))
            .map_err(|_| poisoned())
            // "Refused with `Conflict` naming the purge, before the upload is read, when a
            // purge names the workspace."
            .and_then(|()| self.refuse_purged(scope))
            .and_then(|()| self.records.lock().map_err(|_| poisoned()))
            .and_then(|mut records| {
                let record = records
                    .iter_mut()
                    .find(|record| record.id == upload)
                    .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such upload"))?;
                if record.object.is_none() {
                    return Err(ApiError::new(
                        ErrorCode::InvalidInput,
                        "the upload is not complete",
                    ));
                }
                if record.consumed_by.is_some_and(|consumer| consumer != job) {
                    return Err(ApiError::new(
                        ErrorCode::Conflict,
                        "the upload was consumed by another job",
                    ));
                }
                record.consumed_by = Some(job);
                Ok(())
            });
        let recorded = taken.and_then(|()| {
            self.consumed
                .lock()
                .map(|mut consumed| {
                    if !consumed.contains(&(upload, job)) {
                        consumed.push((upload, job));
                    }
                })
                .map_err(|_| poisoned())
        });
        Box::pin(async move {
            recorded?;
            self.get(scope, upload).await
        })
    }
    fn consume_batch<'a>(
        &'a self,
        scope: &'a StorageScope,
        uploads: &'a [UploadId],
        job: JobId,
    ) -> PortFuture<'a, Vec<UploadRecord>> {
        let taken = self.take_batch(scope, uploads, job);
        Box::pin(async move { taken })
    }
}

impl BlobStore for FakeBlobs {
    fn put<'a>(
        &'a self,
        _scope: &'a StorageScope,
        mut body: ByteReader,
        _limit: u64,
        expected: Option<Digest>,
    ) -> PortFuture<'a, ObjectInfo> {
        Box::pin(async move {
            let mut bytes = Vec::new();
            let _read = tokio::io::AsyncReadExt::read_to_end(&mut body, &mut bytes)
                .await
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
            let object = self.insert(&bytes)?;
            if expected.is_some_and(|expected| expected != object.digest) {
                return Err(ApiError::new(ErrorCode::Conflict, "digest mismatch"));
            }
            self.puts
                .lock()
                .map_err(|_| poisoned())?
                .push(object.clone());
            Ok(object)
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
            let bytes = self.bytes(digest)?;
            let object = ObjectInfo {
                digest: digest.clone(),
                size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            };
            let start = usize::try_from(offset)
                .unwrap_or(usize::MAX)
                .min(bytes.len());
            let end = usize::try_from(offset.saturating_add(length))
                .unwrap_or(usize::MAX)
                .min(bytes.len());
            let selected = bytes
                .get(start..end)
                .ok_or_else(|| ApiError::new(ErrorCode::Internal, "invalid fake object range"))?
                .to_vec();
            Ok(ObjectRead {
                object,
                offset,
                length: u64::try_from(selected.len()).unwrap_or(u64::MAX),
                body: Box::pin(std::io::Cursor::new(selected)),
            })
        })
    }
    fn materialize<'a>(
        &'a self,
        _scope: &'a StorageScope,
        digest: &'a Digest,
    ) -> PortFuture<'a, LocalSource> {
        let hold = self
            .materialize_hold
            .lock()
            .map(|mut slot| {
                if slot.as_ref().is_some_and(|(held, _)| held == digest) {
                    slot.take().map(|(_, hold)| hold)
                } else {
                    None
                }
            })
            .map_err(|_| poisoned());
        Box::pin(async move {
            held(hold).await?;
            let bytes = self.bytes(digest)?;
            let path = self.directory.path().join(digest.as_str());
            std::fs::write(&path, &bytes)
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
            Ok(LocalSource {
                path,
                object: ObjectInfo {
                    digest: digest.clone(),
                    size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                },
            })
        })
    }
}

impl Converter for FakeConverter {
    fn page_count<'a>(
        &'a self,
        source: &'a LocalSource,
        _file_name: &'a str,
    ) -> PortFuture<'a, PageCount> {
        let pages = self.pages;
        Box::pin(async move {
            let hang = self
                .on_page_count
                .lock()
                .map_err(|_| poisoned())?
                .as_ref()
                .is_some_and(|hook| hook());
            if hang {
                let _count = DropCount(Arc::clone(&self.dropped));
                std::future::pending::<()>().await;
            }
            let count_failed = self.count_failed.lock().map_err(|_| poisoned())?.clone();
            if let Some(reason) = count_failed {
                return Ok(PageCount::Failed(Box::new(Conversion {
                    source_digest: source.object.digest.clone(),
                    converter: ConverterIdentity {
                        name: "docling".to_owned(),
                        version: "2.3.0".to_owned(),
                        packages: Vec::new(),
                        settings: okf_jawn_contract::extraction::ConversionSettings::default(),
                        models: Vec::new(),
                        page_window: None,
                    },
                    status: ConversionStatus::Failure(reason),
                    document: None,
                    coverage: None,
                    issues: vec![okf_jawn_contract::extraction::ConverterIssue {
                        component_type: "worker".to_owned(),
                        module_name: "okf-jawn-convert".to_owned(),
                        error_message: "the page count reached its time bound".to_owned(),
                    }],
                })));
            }
            Ok(pages.map_or(PageCount::Whole, PageCount::Pages))
        })
    }
    fn convert(&self, input: ConversionInput) -> PortFuture<'_, Conversion> {
        Box::pin(async move {
            self.windows
                .lock()
                .map_err(|_| poisoned())?
                .push(input.window.clone());
            let hang = self
                .on_window
                .lock()
                .map_err(|_| poisoned())?
                .as_ref()
                .is_some_and(|hook| hook(input.window.as_ref()));
            if hang {
                let _count = DropCount(Arc::clone(&self.dropped));
                std::future::pending::<()>().await;
            }
            let converter = ConverterIdentity {
                name: "docling".to_owned(),
                version: "2.3.0".to_owned(),
                packages: Vec::new(),
                settings: input.settings.clone(),
                models: Vec::new(),
                page_window: input.window.as_ref().map(|_| self.limits.window_pages),
            };
            let source_digest = input.source.object.digest.clone();
            let start = input.window.as_ref().map_or(1, |window| window.start);
            if self.capped.contains(&start) {
                return Ok(Conversion {
                    source_digest,
                    converter,
                    status: ConversionStatus::Failure(FailureReason::MemoryLimit {
                        limit_bytes: self.limits.memory_limit_bytes.to_string(),
                    }),
                    document: None,
                    coverage: None,
                    issues: Vec::new(),
                });
            }
            let structured = input.output_directory.join("document.json");
            let run = self.runs.fetch_add(1, Ordering::SeqCst);
            std::fs::write(&structured, format!("{{\"window\":{start},\"run\":{run}}}"))
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
            let mut assets = Vec::new();
            if self.pictures.load(Ordering::SeqCst) {
                let path = input.output_directory.join("picture.png");
                std::fs::write(&path, format!("picture of window {start}, run {run}"))
                    .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
                assets.push(okf_jawn_core::conversion::ConvertedAsset {
                    path,
                    media_type: "image/png".to_owned(),
                    role: okf_jawn_contract::read::AssetRole::Picture,
                    location: okf_jawn_contract::source::SourceLocation::Direct {
                        locator: okf_jawn_contract::source::SourceLocator::Page { page_no: start },
                    },
                    pixel_size: Some(okf_jawn_core::conversion::PixelSize {
                        width: 20,
                        height: 10,
                    }),
                    caption: Some(format!("Figure {start}")),
                });
            }
            Ok(Conversion {
                source_digest,
                converter,
                status: ConversionStatus::Success,
                coverage: input.window.as_ref().map(|window| WindowCoverage {
                    window: window.clone(),
                    converted: vec![window.clone()],
                    partly_extracted: Vec::new(),
                    not_converted: Vec::new(),
                }),
                document: Some(ConvertedDocument {
                    markdown: format!("# Pages from {start}\n\nText.\n"),
                    structured,
                    outline: Vec::new(),
                    assets,
                    locations: Vec::new(),
                    tables: Vec::new(),
                    warnings: Vec::new(),
                }),
                issues: Vec::new(),
            })
        })
    }
    fn limits(&self) -> ConverterLimits {
        self.limits
    }
}

impl CandidateCheck for AcceptAll {
    fn check(
        &self,
        _root: &Path,
        _rewritten: &[okf_jawn_contract::identity::WorkspacePath],
    ) -> Result<Vec<Warning>, ApiError> {
        Ok(Vec::new())
    }
}

impl CandidateCheck for RefuseAll {
    fn check(
        &self,
        _root: &Path,
        _rewritten: &[okf_jawn_contract::identity::WorkspacePath],
    ) -> Result<Vec<Warning>, ApiError> {
        Err(ApiError::new(
            ErrorCode::InvalidInput,
            "not a conformant OKF bundle",
        ))
    }
}

/// Run `check` once over a staged directory holding every item of the candidate tree, as
/// storage runs the caller's check on its staging directory; its error refuses the commit.
fn staged_check(
    items: &[SourceCard],
    check: &dyn CandidateCheck,
) -> Result<Vec<Warning>, ApiError> {
    let internal = |error: std::io::Error| ApiError::new(ErrorCode::Internal, error.to_string());
    let staging = tempfile::tempdir().map_err(internal)?;
    for card in items {
        let file = staging.path().join(card.path.as_str());
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder).map_err(internal)?;
        }
        std::fs::write(&file, rendered(card)).map_err(internal)?;
    }
    // "Empty when the edits moved nothing": the commits this fake checks are the handler's,
    // which write source cards and move no item, so the store rewrites no other file.
    check.check(staging.path(), &[])
}

/// An item's file as the fake keeps it: its type and title as frontmatter, then its body.
fn rendered(card: &SourceCard) -> String {
    format!(
        "---\ntype: {}\ntitle: {}\n---\n\n{}\n",
        card.type_name, card.title, card.body
    )
}

/// The folder a path is in; `None` at the root.
fn folder_of(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(folder, _)| folder)
}

/// The items directly in `folder` at a revision, and the child folders directly in it.
fn listing(
    items: &[SourceCard],
    folder: Option<&WorkspacePath>,
    revision: &Revision,
) -> Result<FolderListing, ApiError> {
    let folder = folder.map(WorkspacePath::as_str);
    let mut folders: Vec<String> = Vec::new();
    for card in items {
        let mut path = card.path.as_str();
        while let Some(parent) = folder_of(path) {
            if folder_of(parent) == folder && !folders.iter().any(|known| known == parent) {
                folders.push(parent.to_owned());
            }
            path = parent;
        }
    }
    Ok(FolderListing {
        items: items
            .iter()
            .filter(|card| folder_of(card.path.as_str()) == folder)
            .map(|card| document_of(card, revision).map(|document| document.summary))
            .collect::<Result<_, _>>()?,
        folders: folders
            .into_iter()
            .map(|folder| {
                WorkspacePath::try_from(folder)
                    .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
            })
            .collect::<Result<_, _>>()?,
        next_cursor: None,
    })
}

/// Apply one tree edit to the items, as storage would.
fn apply_to_items(items: &mut Vec<SourceCard>, edit: &TreeEdit) {
    match edit {
        TreeEdit::WriteSourceCard(card) => {
            items.retain(|item| item.item_id != card.item_id);
            items.push((**card).clone());
        }
        TreeEdit::EditItem {
            item_id,
            body,
            properties,
        } => {
            for item in items.iter_mut().filter(|item| item.item_id == *item_id) {
                item.body.clone_from(body);
                item.properties.clone_from(properties);
            }
        }
        TreeEdit::MoveItem {
            item_id,
            destination,
            ..
        } => {
            for item in items.iter_mut().filter(|item| item.item_id == *item_id) {
                item.path = destination.clone();
            }
        }
        TreeEdit::DeleteItem { item_id } => items.retain(|item| item.item_id != *item_id),
        _ => {}
    }
}

/// The document `show` returns for a card at a revision.
/// The tree of `items` at `revision`: each card summarized as `show` summarizes it, with its first
/// observed name as its original's, in path order, and every folder above them.
fn tree_of(items: &[SourceCard], revision: &Revision) -> Result<WorkspaceTree, ApiError> {
    let mut held = Vec::new();
    let mut folders = std::collections::BTreeSet::new();
    for card in items {
        let path = card.path.as_str();
        folders.extend(
            path.match_indices('/')
                .filter_map(|(at, _)| path.get(..at))
                .map(str::to_owned),
        );
        held.push(TreeItem {
            summary: document_of(card, revision)?.summary,
            original_name: card
                .appearance
                .names
                .first()
                .map(|name| name.filename.clone()),
        });
    }
    held.sort_by(|left, right| left.summary.path.as_str().cmp(right.summary.path.as_str()));
    Ok(WorkspaceTree {
        items: held,
        folders: folders
            .into_iter()
            .map(WorkspacePath::try_from)
            .collect::<Result<_, _>>()
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?,
    })
}

fn document_of(card: &SourceCard, revision: &Revision) -> Result<ItemDocument, ApiError> {
    let content = serde_json::to_vec(&(&card.body, &card.properties))
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
    Ok(ItemDocument {
        summary: ItemSummary {
            id: card.item_id,
            path: card.path.clone(),
            title: card.title.clone(),
            description: String::new(),
            type_name: card.type_name.clone(),
            kind: ItemKind::Source,
            revision: revision.clone(),
            status: ItemStatus::Stable,
            archived: false,
            media_type: Some(card.appearance.media_type.clone()),
            extraction: None,
        },
        body: card.body.clone(),
        properties: card.properties.clone(),
        content_digest: okf_jawn_ingest::record::sha256_digest(&content)?,
        source: Some(card.appearance.clone()),
        draft: None,
    })
}

/// Await `hold`, when one is set.
async fn held(hold: Result<Option<Hold>, ApiError>) -> Result<(), ApiError> {
    if let Some(hold) = hold? {
        hold().await;
    }
    Ok(())
}

/// The hold in `slot`, kept for the next caller too.
fn hold_of(slot: &Mutex<Option<Hold>>) -> Result<Option<Hold>, ApiError> {
    slot.lock().map(|hold| hold.clone()).map_err(|_| poisoned())
}

fn poisoned() -> ApiError {
    ApiError::new(ErrorCode::Internal, "poisoned")
}

fn unused<'a, T: Send + 'a>(method: &'static str) -> PortFuture<'a, T> {
    Box::pin(async move {
        Err(ApiError::new(
            ErrorCode::NotImplemented,
            format!("the fake does not model {method}"),
        ))
    })
}
