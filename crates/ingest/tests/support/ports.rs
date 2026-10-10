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
use std::sync::atomic::{AtomicUsize, Ordering};

use okf_jawn_contract::common::{PageRange, Warning};
use okf_jawn_contract::conventions::NamingRules;
use okf_jawn_contract::error::{ApiError, ErrorCode};
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
    WindowCoverage,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::search::{GraphQuery, LinkQuery, SearchIndex, SearchQuery};
use okf_jawn_core::storage::{
    Backups, BlameQuery, BlobStore, ByteReader, CandidateChanges, CandidateCheck, CommitChanges,
    Committed, DiffQuery, FolderListing, LocalSource, LogQuery, ObjectInfo, ObjectRead, Page,
    Promotion, Purger, SourceCard, StorageScope, TreeEdit, VersionStore,
};
use okf_jawn_core::uploads::{NewUpload, UploadRecord, UploadStore};

/// A `VersionStore` that keeps a line of commits and nothing of their trees.
///
/// `commit` encodes the port doc: a commit carrying the same mutation after the expected head
/// is returned as `replayed`; otherwise a head that moved is `Conflict`. It searches the line
/// from the tip, newest first, as the storage lane's `find_trailer` walk does, so a handler
/// that wrote two commits under one identity is caught replaying the wrong one. `find_commit`
/// searches the same way. A source card whose path collision key another item holds at the
/// head is refused with `Conflict`, as storage refuses a path collision. A commit that is
/// written first runs the caller's check once over a staged directory holding the source cards
/// it writes; its error refuses the commit and its warnings are returned. Folders list empty.
///
/// Someone else's edit is modelled as a foreign commit (`edit_elsewhere`), which may take a
/// path, or carry tree edits (`edit_elsewhere_with`: an edit, move or deletion of an item);
/// `edit_elsewhere_before_call` makes one land just before the given `commit` call, as
/// another writer racing the handler would.
///
/// `show` reads an item at a revision by replaying, over the items present at the base
/// (`at_base`), the edits of every commit up to that revision: `WriteSourceCard` writes the
/// whole card (moving it to the card's path, as storage's `write_card` does), `EditItem`
/// replaces body and properties, `MoveItem` the path, `DeleteItem` removes it. An item absent
/// there is `NotFound`.
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
}

/// An `UploadStore` holding complete uploads and recording which job consumed each.
#[derive(Default)]
pub struct FakeUploads {
    /// The slots `get` returns.
    pub records: Mutex<Vec<UploadRecord>>,
    /// Consumptions asked for: upload and job.
    pub consumed: Mutex<Vec<(UploadId, JobId)>>,
}

/// A `BlobStore` in memory; `materialize` writes the object into a temporary directory.
pub struct FakeBlobs {
    /// Objects by digest.
    pub objects: Mutex<BTreeMap<String, Vec<u8>>>,
    /// Where materialized objects are written.
    pub directory: tempfile::TempDir,
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

/// A `SearchIndex` that records the heads it was rebuilt at.
#[derive(Default)]
pub struct FakeSearch {
    /// Heads rebuilt, in order.
    pub rebuilt: Mutex<Vec<Revision>>,
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
}

/// A `Purger` that returns one report and counts the calls.
pub struct FakePurger {
    /// The report every call returns.
    pub report: PurgeReport,
    /// Calls made.
    pub calls: AtomicUsize,
}

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
        }
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
        Ok(items.into_iter().find(|card| card.item_id == item))
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

    /// The commit carrying `mutation_id` after `since` on the line, newest first.
    fn found(
        &self,
        mutation_id: MutationId,
        since: &Revision,
    ) -> Result<Option<Revision>, ApiError> {
        let commits = self.commits.lock().map_err(|_| poisoned())?;
        let after: Vec<&(CommitChanges, Revision)> = if *since == self.head {
            commits.iter().collect()
        } else {
            commits
                .iter()
                .skip_while(|(_, revision)| revision != since)
                .skip(1)
                .collect()
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
        if let Some(revision) = self.found(changes.mutation_id, &changes.expected_head)? {
            return Ok(Committed {
                revision,
                replayed: true,
                warnings: Vec::new(),
            });
        }
        let mut commits = self.commits.lock().map_err(|_| poisoned())?;
        let head = commits
            .last()
            .map_or_else(|| self.head.clone(), |(_, revision)| revision.clone());
        if changes.expected_head != head {
            return Err(ApiError::new(ErrorCode::Conflict, "the head moved"));
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
        let warnings = staged_check(&changes.edits, check)?;
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
        _revision: &'a Revision,
        _folder: Option<&'a WorkspacePath>,
        _page: Page,
    ) -> PortFuture<'a, FolderListing> {
        Box::pin(async move {
            Ok(FolderListing {
                items: Vec::new(),
                folders: Vec::new(),
                next_cursor: None,
            })
        })
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
    fn rules<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
    ) -> PortFuture<'a, Option<NamingRules>> {
        unused("rules")
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
        since: &'a Revision,
    ) -> PortFuture<'a, Option<Revision>> {
        let found = self.found(mutation_id, since);
        Box::pin(async move { found })
    }
    fn create_candidate<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _proposal_id: okf_jawn_contract::identity::ProposalId,
        _changes: CandidateChanges,
        _check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Revision> {
        unused("create_candidate")
    }
    fn promote_candidate<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _promotion: Promotion,
    ) -> PortFuture<'a, Committed> {
        unused("promote_candidate")
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
    fn write_history<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo> {
        unused("write_history")
    }
}

impl SearchIndex for FakeSearch {
    fn index_revision<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: Revision,
    ) -> PortFuture<'a, ()> {
        unused("index_revision")
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
        let recorded = self
            .rebuilt
            .lock()
            .map(|mut heads| heads.push(head))
            .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"));
        Box::pin(async move { recorded })
    }
}

impl Backups for FakeBackups {
    fn write_workspace_archive<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo> {
        let _previous = self.writes.fetch_add(1, Ordering::SeqCst);
        let archive = self.archive.clone();
        Box::pin(async move { Ok(archive) })
    }
    fn write_installation_archive<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo> {
        let _previous = self.writes.fetch_add(1, Ordering::SeqCst);
        let archive = self.archive.clone();
        Box::pin(async move { Ok(archive) })
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
        let recorded = self
            .restored
            .lock()
            .map(|mut restored| restored.push(editors))
            .map_err(|_| poisoned())
            .and_then(|()| self.versions.restore_commit(mutation_id))
            .map(|()| RestoreReport {
                items: 3,
                drafts_restored: 1,
                drafts_unassigned: 1,
            });
        Box::pin(async move { recorded })
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
        Box::pin(async move { Ok(report) })
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
        Box::pin(async move { Ok(report) })
    }
}

impl UploadStore for FakeUploads {
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
        let recorded = self
            .consumed
            .lock()
            .map(|mut consumed| {
                if !consumed.contains(&(upload, job)) {
                    consumed.push((upload, job));
                }
            })
            .map_err(|_| poisoned());
        Box::pin(async move {
            recorded?;
            self.get(scope, upload).await
        })
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
            Ok(object)
        })
    }
    fn open<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _digest: &'a Digest,
        _offset: u64,
        _length: u64,
    ) -> PortFuture<'a, ObjectRead> {
        unused("open")
    }
    fn materialize<'a>(
        &'a self,
        _scope: &'a StorageScope,
        digest: &'a Digest,
    ) -> PortFuture<'a, LocalSource> {
        Box::pin(async move {
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
        _source: &'a LocalSource,
        _file_name: &'a str,
    ) -> PortFuture<'a, Option<u32>> {
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
            Ok(pages)
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
            std::fs::write(&structured, format!("{{\"window\":{start}}}"))
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
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
                    assets: Vec::new(),
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
    fn check(&self, _root: &Path) -> Result<Vec<Warning>, ApiError> {
        Ok(Vec::new())
    }
}

impl CandidateCheck for RefuseAll {
    fn check(&self, _root: &Path) -> Result<Vec<Warning>, ApiError> {
        Err(ApiError::new(
            ErrorCode::InvalidInput,
            "not a conformant OKF bundle",
        ))
    }
}

/// Run `check` once over a staged directory holding the source cards `edits` write, as storage
/// runs the caller's check on its staging directory; its error refuses the commit.
fn staged_check(edits: &[TreeEdit], check: &dyn CandidateCheck) -> Result<Vec<Warning>, ApiError> {
    let internal = |error: std::io::Error| ApiError::new(ErrorCode::Internal, error.to_string());
    let staging = tempfile::tempdir().map_err(internal)?;
    for edit in edits {
        if let TreeEdit::WriteSourceCard(card) = edit {
            let file = staging.path().join(card.path.as_str());
            if let Some(folder) = file.parent() {
                std::fs::create_dir_all(folder).map_err(internal)?;
            }
            std::fs::write(
                &file,
                format!(
                    "---\ntype: {}\ntitle: {}\n---\n\n{}\n",
                    card.type_name, card.title, card.body
                ),
            )
            .map_err(internal)?;
        }
    }
    check.check(staging.path())
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
