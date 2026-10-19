//! Storage interfaces preserve byte identity, occurrence identity, and revision preconditions.
//!
//! Implementations use selected libraries; they do not infer domain approval from a Git merge.
//! Every commit carries an `Okf-Jawn-Mutation:` trailer naming the durable write identity.
//! Ports take resolved `Revision`s and core parameter types: the application resolves selectors
//! and authorizes the caller before it calls a port.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use okf_jawn_contract::{
    access::{AccessRoute, Permission, Principal},
    common::{PageRange, PageRequest, TextRange, Warning},
    conventions::NamingRules,
    error::{ApiError, ErrorCode},
    history::{BlameResponse, DiffResponse, LogResponse},
    identity::{
        Digest, ItemId, JobId, MutationId, ProposalId, PurgeId, ReviewId, Revision, TenantId,
        WorkspaceId, WorkspacePath,
    },
    item::{ItemDocument, ItemKind, ItemStatus, ItemSummary, TypeDefinition},
    proposal::Change,
    purge::PurgeReport,
    source::SourceAppearance,
    workspace::{RestoreReport, Workspace},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncRead;

use crate::items::{ApplicationHeader, SourceHeader};
use crate::ports::PortFuture;

/// Owned streaming bytes, not a base64 document copied through application messages.
pub type ByteReader = Pin<Box<dyn AsyncRead + Send + Unpin + 'static>>;

/// Authenticated deployment storage scope, established by application policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageScope {
    /// Tenant namespace; an object hash never substitutes for this scope.
    pub tenant_id: TenantId,
    /// Workspace to which the operation belongs.
    pub workspace_id: WorkspaceId,
}

/// A bounded page of a listing; the cursor is opaque and scoped to the query that issued it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// Continuation cursor returned with the previous page, if any.
    pub cursor: Option<String>,
    /// Maximum results wanted; an implementation may return fewer.
    pub limit: u16,
}

/// Who a write or a job acts for, retained in Git metadata and job records; not a review claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// Validated identity-provider subject.
    pub subject: String,
    /// Verified authentication route.
    pub route: AccessRoute,
    /// OAuth client when delegation is present.
    pub client_id: Option<String>,
}

/// Retained byte identity.
///
/// The media type is not stored with the bytes. It is detected per occurrence and recorded by
/// whatever refers to the object: a source card, a converted asset, or a sandbox capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectInfo {
    /// Digest verified against stored bytes.
    pub digest: Digest,
    /// Exact byte count.
    pub size: u64,
}

/// A bounded stream and the object identity to which it belongs.
pub struct ObjectRead {
    /// Whole-object identity.
    pub object: ObjectInfo,
    /// Byte offset at which this stream starts.
    pub offset: u64,
    /// Maximum bytes exposed by this reader.
    pub length: u64,
    /// Bounded body owned by the reader.
    pub body: ByteReader,
}

/// Where a retained object lies on the local filesystem, for a bounded converter worker.
#[derive(Debug, Clone)]
pub struct LocalSource {
    /// Absolute path of the retained bytes inside the store. The caller only reads it; the
    /// path carries no file extension and is never an unchecked client pathname.
    pub path: PathBuf,
    /// Identity checked during materialization.
    pub object: ObjectInfo,
}

/// Immutable content-addressed bytes, keyed by tenant and SHA-256 digest.
///
/// Objects are shared inside one tenant and never across tenants; the workspace in `scope` is
/// not part of the key. Knowing a digest authorizes nothing: the application authorizes a read
/// through an item reference before it calls `open`. Use `object_store`, not another protocol.
pub trait BlobStore: Send + Sync {
    /// Store and hash a bounded stream.
    ///
    /// Fails with `TooLarge` past `limit`, and with `Conflict` when `expected` is given and
    /// differs from the computed digest; in both cases nothing is retained. Storing bytes that
    /// are already retained writes nothing and returns the existing identity.
    fn put<'a>(
        &'a self,
        scope: &'a StorageScope,
        body: ByteReader,
        limit: u64,
        expected: Option<Digest>,
    ) -> PortFuture<'a, ObjectInfo>;
    /// Open an already authorized object's selected byte interval; `NotFound` when absent.
    fn open<'a>(
        &'a self,
        scope: &'a StorageScope,
        digest: &'a Digest,
        offset: u64,
        length: u64,
    ) -> PortFuture<'a, ObjectRead>;
    /// Locate retained bytes on the local filesystem for a bounded converter worker.
    fn materialize<'a>(
        &'a self,
        scope: &'a StorageScope,
        digest: &'a Digest,
    ) -> PortFuture<'a, LocalSource>;
}

/// One change to the versioned tree. A commit applies its edits in order, all or nothing.
///
/// The caller allocates the identity of every item it creates; see `derive_item_id`. An edit
/// that names an item or path absent from the tree it is applied to fails the whole commit
/// with `NotFound`; an edit that would overwrite another item's path fails it with `Conflict`.
#[derive(Debug, Clone)]
pub enum TreeEdit {
    /// Create a note or View document at a new path.
    CreateItem {
        /// Identity of the new item.
        item_id: ItemId,
        /// Path of the new document.
        path: WorkspacePath,
        /// Display title; `None` leaves it to the `title` property or, failing that, the file stem.
        title: Option<String>,
        /// User-selected OKF type name.
        type_name: String,
        /// Built-in rendering role; storage writes it into the new item's application header
        /// (`ApplicationHeader::kind`) and reads it back from there, never from the body.
        kind: ItemKind,
        /// Markdown body.
        body: String,
        /// Complete property map, including unknown extensions.
        properties: BTreeMap<String, serde_json::Value>,
    },
    /// Replace an item's Markdown body and complete property map.
    EditItem {
        /// Item to edit.
        item_id: ItemId,
        /// New Markdown body.
        body: String,
        /// New complete property map, including unknown extensions.
        properties: BTreeMap<String, serde_json::Value>,
    },
    /// Move an item and rewrite the links that point at it.
    MoveItem {
        /// Item to move.
        item_id: ItemId,
        /// New path.
        destination: WorkspacePath,
        /// Before the move, record the item's path in its frontmatter `aliases`, a list of its
        /// former workspace paths, oldest first, each once (a naming rule's
        /// `alias_preservation`): an absent `aliases` becomes a list of that path, a string
        /// becomes a list of the string and the path, and a list of strings gains the path unless
        /// it holds it already. Any other value refuses the commit with `InvalidInput` naming
        /// the item's path, and the value is not written over. `false` writes nothing, and a
        /// move to the path the item holds records nothing either. The review fingerprint
        /// leaves `aliases` out (`portable::item_content_digest`), so recording one changes no
        /// review.
        keep_alias: bool,
    },
    /// Set an item's OKF status word, its archived flag, or both; `None` leaves one unchanged.
    SetStatus {
        /// Item to change.
        item_id: ItemId,
        /// New OKF status word.
        status: Option<ItemStatus>,
        /// Archive (`true`) or unarchive (`false`).
        archived: Option<bool>,
    },
    /// Remove an item from the tree; history and retained objects stay.
    ///
    /// Every companion file the item owns (the source appearance, the accepted supplied text
    /// and every recorded correction, as `RestoreItem` names them) is removed with it.
    DeleteItem {
        /// Item to remove.
        item_id: ItemId,
    },
    /// Create a folder together with its maintained `index.md`.
    CreateFolder {
        /// New folder path.
        folder: WorkspacePath,
    },
    /// Move a folder, with every item and file under it, archived items included, to a new path.
    ///
    /// Every item under the folder moves as `MoveItem` moves one (no alias is recorded): the
    /// links and `sources` entries that point at it are rewritten, and so are its own relative
    /// links. So does every other Markdown file okf-core reads as a concept. Every other file
    /// under it moves as it is, and the links and frontmatter paths (`sources[].resource`,
    /// `computation`, `executor.resource`, `attester.resource`) that point at it are rewritten
    /// to its new path. The files whose links this rewrote are told to the check as `rewritten`
    /// (`CandidateCheck::check`). A draft is keyed by its item, so it stays with it.
    ///
    /// A destination with the folder's collision key but another spelling (`Notes` to `notes`)
    /// renames the folder: it moves first to a free private folder beside it and then to the
    /// destination, so a filesystem that holds both spellings as one never sees them at once.
    /// The private folder is in no commit.
    ///
    /// Refused, writing nothing: a reserved folder (`.okf`, `index.md`, `log.md` as any segment,
    /// in any letter case) or one the tree spells another way, `InvalidInput` on `/folder`; a
    /// folder the tree does not hold, `NotFound` on `/folder`; a reserved destination, the folder
    /// itself, a path inside it, or a destination one of whose folders the tree spells another
    /// way, `InvalidInput` on `/destination`; a destination whose collision key (or a key below
    /// it) any other file, folder or item holds, or below a file, `Conflict` on `/destination`.
    ///
    /// When files that are not items moved (the folder indexes the store regenerates are not
    /// counted), the commit's warnings end with one `FOLDER_OTHER_FILES` warning saying how many
    /// (`folder_other_files`).
    MoveFolder {
        /// The folder to move.
        folder: WorkspacePath,
        /// Its new path.
        destination: WorkspacePath,
    },
    /// Remove a folder with every item and file under it, archived items included; history and
    /// retained objects stay.
    ///
    /// Every item under the folder is removed as `DeleteItem` removes one, with every companion
    /// file it owns; then every other file under it, and the folder. A draft is keyed by its
    /// item, so a deleted item's draft is kept as `DeleteItem` keeps it.
    ///
    /// Refused, writing nothing: a reserved folder (`.okf`, `index.md`, `log.md` as any segment,
    /// in any letter case) or one the tree spells another way, `InvalidInput` on `/folder`; a
    /// folder the tree does not hold, `NotFound` on `/folder`. When files that are not items
    /// went with it (the folder indexes the store regenerates are not counted), the commit's
    /// warnings end with one `FOLDER_OTHER_FILES` warning saying how many
    /// (`folder_files_deleted`).
    DeleteFolder {
        /// The folder to remove.
        folder: WorkspacePath,
    },
    /// Create or replace one user-defined type definition.
    SetType {
        /// Definition to store under its own name.
        definition: TypeDefinition,
    },
    /// Replace the naming rules stored in `.okf/rules.yaml`.
    SetRules {
        /// Complete rule set.
        rules: NamingRules,
    },
    /// Create an import source card, or replace the card that has the same identity.
    ///
    /// Corrections recorded with `CorrectDigest` are kept when a card is replaced, and so is
    /// the stored `archived` flag of its application header (`SourceCard::header`).
    WriteSourceCard(Box<SourceCard>),
    /// Write the accepted agent-supplied text of a source beside its card, with the proposal
    /// and the supplier it came from. The promotion commit's committer is the approver.
    SupplyExtraction {
        /// Source item whose text is supplied.
        item_id: ItemId,
        /// Digest the agent saw; absent when the source had none.
        based_on: Option<Digest>,
        /// Pages the agent claims the text covers; empty for the whole.
        pages: Vec<PageRange>,
        /// The complete supplied Markdown.
        markdown: String,
        /// Proposal that carried the text.
        proposal_id: ProposalId,
        /// Agent that proposed it.
        supplier: Provenance,
    },
    /// Record a human correction of one digest, beside the generated extraction.
    CorrectDigest {
        /// Source item whose digest is corrected.
        item_id: ItemId,
        /// Digest the correction applies to.
        digest: Digest,
        /// Corrected Markdown, kept separate from the generated text.
        corrected_markdown: String,
        /// The supplied-text proposal a person adopted as the correction, when it came from one.
        adopts: Option<ProposalId>,
    },
    /// Restore one item, and every companion file it owns, to its state at an earlier revision.
    ///
    /// The item's own file is written as `from` holds it, at the item's path there; when the
    /// item moved since, its file at the current path is removed. Every companion file the item
    /// owns is restored with it as `from` holds it, and a companion that did not exist at
    /// `from` is removed: the source appearance, the accepted supplied text and every recorded
    /// correction of a source item. Storage owns where those files are. An item absent at
    /// `from` is removed with its companions; an item absent both at `from` and in the tree the
    /// edit is applied to fails the commit with `NotFound`, and restoring the item's file over
    /// another item's path fails it with `Conflict`, as for every edit.
    ///
    /// A Rewind of several items is one `RestoreItem` per item in one commit. Consecutive
    /// `RestoreItem` edits apply as one group: every listed item is looked up first, then every
    /// listed item's current file is removed, and only then is each written back with its
    /// companions, in order; so two items that swapped paths since `from` are rewound together,
    /// and an item listed twice is restored as its last listing says. A `RestoreItem` separated
    /// from the others by a different edit is not part of their group.
    RestoreItem {
        /// Item to restore.
        item_id: ItemId,
        /// Revision to restore from.
        from: Revision,
    },
    /// Restore the listed paths to their content at an earlier revision.
    ///
    /// A listed path that is absent at `from` is removed.
    RestorePaths {
        /// Revision to restore from.
        from: Revision,
        /// Paths to restore.
        paths: Vec<WorkspacePath>,
    },
    /// Restore the whole tree to an earlier revision.
    RestoreWorkspace {
        /// Revision to restore from.
        from: Revision,
    },
}

/// One imported source occurrence written as an OKF concept.
#[derive(Debug, Clone)]
pub struct SourceCard {
    /// Identity of the card, allocated by the caller so sibling cards can refer to each other.
    pub item_id: ItemId,
    /// Path of the card in the workspace tree.
    pub path: WorkspacePath,
    /// Display title.
    pub title: String,
    /// User-selected OKF type name.
    pub type_name: String,
    /// The shown text (Stage 1b design section 5, rule 3): the correction, the supplied text or
    /// the converter text, as `appearance.extraction.text_origin` says; empty when it is `none`.
    pub body: String,
    /// Preserved extension properties.
    pub properties: BTreeMap<String, serde_json::Value>,
    /// Occurrence metadata: object, observed names, media type, size, parent and successor,
    /// and the contract `Extraction` the card shows. The import handler commits the card with
    /// outcome `pending` before converting and replaces it afterwards.
    pub appearance: SourceAppearance,
}

/// What `TreeEdit::from_change` needs beyond the change itself.
#[derive(Debug, Clone)]
pub struct ChangeContext {
    /// Identity for a `Change::Create`, which carries none.
    pub new_item_id: ItemId,
    /// Rendering role for a `Change::Create`: the role its `item_kind` names, a note when it
    /// names none (`ChangeContext::for_change` fills it so).
    pub new_item_kind: ItemKind,
    /// Proposal the change belongs to.
    pub proposal_id: ProposalId,
    /// Who proposed it.
    pub proposer: Provenance,
}

/// One commit on the accepted head.
#[derive(Debug, Clone)]
pub struct CommitChanges {
    /// Durable write identity written as `Okf-Jawn-Mutation:` in the commit.
    pub mutation_id: MutationId,
    /// Head the edits were prepared against; also where the replay search starts.
    pub expected_head: Revision,
    /// Who the commit acts for; not a claim of review.
    pub author: Provenance,
    /// Commit message, without the trailer.
    pub message: String,
    /// Edits applied in order.
    pub edits: Vec<TreeEdit>,
}

/// One retained proposal candidate commit.
#[derive(Debug, Clone)]
pub struct CandidateChanges {
    /// Durable write identity written as `Okf-Jawn-Mutation:` in the candidate commit.
    pub mutation_id: MutationId,
    /// Revision the proposal is drafted against; it need not be the head.
    pub base: Revision,
    /// Who proposes the change.
    pub author: Provenance,
    /// Commit message, without the trailer.
    pub message: String,
    /// Edits applied in order on top of `base`.
    pub edits: Vec<TreeEdit>,
}

/// Acceptance of one exact candidate onto an unchanged head.
#[derive(Debug, Clone)]
pub struct Promotion {
    /// Durable write identity of the acceptance, written as `Okf-Jawn-Mutation:`.
    pub mutation_id: MutationId,
    /// Proposal whose candidate is promoted.
    pub proposal_id: ProposalId,
    /// Head shown when the approver confirmed.
    pub expected_head: Revision,
    /// Exact candidate revision shown when the approver confirmed.
    pub candidate: Revision,
    /// Who accepts; recorded as the committer, while the proposer stays the author.
    pub approver: Provenance,
    /// Commit message, without the trailer.
    pub message: String,
}

/// The commit that carries one mutation.
#[derive(Debug, Clone)]
pub struct Committed {
    /// Commit whose `Okf-Jawn-Mutation:` trailer names the mutation.
    pub revision: Revision,
    /// `true` when that commit already existed and this call wrote nothing.
    pub replayed: bool,
    /// Findings the candidate check returned when the commit was written, followed by what the
    /// edits themselves report (`FOLDER_OTHER_FILES` of a `TreeEdit::MoveFolder` or a
    /// `TreeEdit::DeleteFolder`). A replay answers the warnings the store kept with that
    /// commit, so a retry answers what its first attempt did; for a commit the store kept no
    /// warnings for it answers the one warning `replay_warnings_unavailable` instead. Warnings
    /// kept under the same mutation for another attempt's commit are not this commit's, and are
    /// never answered for it (`VersionStore::commit`).
    pub warnings: Vec<Warning>,
}

/// The naming rules file as one revision stores it (`VersionStore::rules`).
#[derive(Debug, Clone)]
pub struct RulesFile {
    /// The file's text. Bytes that are not UTF-8 are replaced with U+FFFD
    /// (`String::from_utf8_lossy`); such a file never reads as rules (`parsed` is an error).
    pub text: String,
    /// The stored bytes read as `NamingRules` (YAML, unknown keys refused), or why they do not
    /// read. Nothing beyond the reader's own checks is applied here: whether the rules can be
    /// applied is the application's (`conventions::CompiledRules`).
    pub parsed: Result<NamingRules, RulesParseError>,
}

/// Why a stored rules file does not read as `NamingRules`: the reader's message, and where in
/// the file it stopped when it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulesParseError {
    /// The reader's message.
    pub message: String,
    /// The 1-based line the reader stopped at; `None` when it names none.
    pub line: Option<u32>,
    /// The 1-based column on that line; `None` when it names none.
    pub column: Option<u32>,
}

/// One page of a folder at one revision.
#[derive(Debug, Clone)]
pub struct FolderListing {
    /// Items directly inside the folder.
    pub items: Vec<ItemSummary>,
    /// Child folders directly inside the folder.
    pub folders: Vec<WorkspacePath>,
    /// Continuation cursor, when more entries remain.
    pub next_cursor: Option<String>,
}

/// Every item of one revision with what the naming rules read of it, and every folder
/// (`VersionStore::tree`).
#[derive(Debug, Clone)]
pub struct WorkspaceTree {
    /// Every item, in path order, archived items and items under a folder whose name starts
    /// with `.` included.
    pub items: Vec<TreeItem>,
    /// Every folder that holds a file, at any depth, in path order: each folder above each file
    /// of the tree, `.okf` and the folders under it included.
    pub folders: Vec<WorkspacePath>,
}

/// One item of a `WorkspaceTree`.
#[derive(Debug, Clone)]
pub struct TreeItem {
    /// The item, summarized as `list` of its folder summarizes it.
    pub summary: ItemSummary,
    /// A source card's original file name (`items::SourceHeader::original_name`); `None` for
    /// every other item.
    pub original_name: Option<String>,
}

/// History of a workspace or of one item, newest first.
#[derive(Debug, Clone)]
pub struct LogQuery {
    /// Revision to walk back from.
    pub tip: Revision,
    /// Limit history to commits that changed this item, following its moves.
    pub item_id: Option<ItemId>,
    /// Bounded page.
    pub page: Page,
}

/// A comparison of two revisions.
#[derive(Debug, Clone)]
pub struct DiffQuery {
    /// Base revision.
    pub from: Revision,
    /// Compared revision.
    pub to: Revision,
    /// Limit the comparison to this item, following its moves.
    pub item_id: Option<ItemId>,
}

/// Last-change attribution for lines of one item.
#[derive(Debug, Clone)]
pub struct BlameQuery {
    /// Revision whose content is attributed.
    pub revision: Revision,
    /// Item to attribute.
    pub item_id: ItemId,
    /// One-based inclusive lines of the item body, as `show` returns it.
    pub lines: TextRange,
}

/// Application policy run on a candidate tree before it can become a commit.
///
/// The implementation lives in core and decides OKF conformance; storage calls it and never
/// decides. It is synchronous because it reads a directory, and storage calls it from the
/// blocking task that owns that directory. It evaluates an item's properties against its type
/// schema without the application header (`items::without_header`): the header is the
/// server's, and no type may constrain or refuse it.
pub trait CandidateCheck: Send + Sync {
    /// Inspect the complete candidate bundle rooted at `root`.
    ///
    /// `rewritten` names the files of the candidate that the store rewrote on its own while it
    /// applied the edits, beyond the paths the edits name: the referrers whose links a move
    /// rewrote (`okf_core::move_concept`), for a `MoveItem` and for a `RestoreItem` that moves
    /// an item back to its earlier path, of every such move of the commit or the candidate. The
    /// caller that built the edits cannot know them, so the store reports them here, as
    /// workspace paths spelled as the tree spells them, in path order and each once; a moved
    /// item's own old and new path are not among them. A `MoveFolder` moves many items and
    /// files: the files whose links or frontmatter paths its moves rewrote are named, a file
    /// under the folder among them at the path the folder move gave it, though the caller knows
    /// those paths. It names what the store rewrote while
    /// it applied the edits, not what differs at the end: a file rewritten and then rewritten
    /// back (the referrer of an item moved away and back by one commit) is named, though its
    /// bytes end as they began. A referrer a later edit of the same commit removed or moved
    /// away is left out at the path it no longer has, and so is a rewritten file whose path is
    /// no `WorkspacePath` (no item has such a path). A check that reports findings only for the
    /// files a write touched (`conformance::EditCheck`) counts these as touched. Empty when the
    /// edits moved nothing, and for a candidate no edit produced.
    ///
    /// # Errors
    /// Returns the error that rejects the write; nothing is committed when this fails.
    fn check(&self, root: &Path, rewritten: &[WorkspacePath]) -> Result<Vec<Warning>, ApiError>;
}

/// Versioned workspace content: one Git repository per workspace.
///
/// Every write follows one path inside the implementation, under a per-workspace lock:
/// materialize the base tree in a private staging directory, apply the edits there, maintain
/// the folder indexes and the change log, run the caller's `CandidateCheck` on that directory,
/// write it back as a Git tree, create the commit, and only then move the reference.
///
/// The staging directory is `<data>/staging/<tenant>/<workspace>/<mutation>`. It is removed
/// when the call returns, whether it succeeded or failed, and any directory a crash left there
/// is removed at startup and before it is reused. Nothing in it is ever referenced by Git.
///
/// Reads take a resolved `Revision` and never the current working state.
pub trait VersionStore: Send + Sync {
    /// The accepted head of the workspace.
    fn head<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Revision>;
    /// List the items and child folders directly inside `folder`; `None` is the root.
    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        folder: Option<&'a WorkspacePath>,
        page: Page,
    ) -> PortFuture<'a, FolderListing>;
    /// Every item and every folder of `revision` in one pass over its tree, each item file read
    /// once (`WorkspaceTree`): what the naming rules read to name a selection against the whole
    /// workspace. Unlike `list`, it includes the items under a folder whose name starts with
    /// `.`, and it is not paged. `NotFound` only when the revision itself is unknown.
    fn tree<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, WorkspaceTree>;
    /// The item whose path is exactly `path` at `revision`, summarized as `list` of its folder
    /// summarizes it.
    ///
    /// `Ok(None)` when no item is at that path at that revision; `NotFound` only when the
    /// revision itself is unknown.
    fn item_at_path<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        path: &'a WorkspacePath,
    ) -> PortFuture<'a, Option<ItemSummary>>;
    /// Read one item's committed content. The returned document never carries a draft.
    fn show<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        item: ItemId,
    ) -> PortFuture<'a, ItemDocument>;
    /// Read the bytes of any versioned file, such as a folder `index.md`; `NotFound` when absent.
    fn read_file<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        path: &'a WorkspacePath,
    ) -> PortFuture<'a, Vec<u8>>;
    /// Read the naming rules file `.okf/rules.yaml` at `revision`: `None` when no rules were
    /// saved there; otherwise the file's text and what reading its bytes as `NamingRules` gave
    /// (`RulesFile`). A file that does not read as rules (a YAML error, an unknown key, bytes
    /// that are not UTF-8) is answered with its text and the reader's error, never as an error
    /// of this call: a person or an import wrote it, and `get_rules` gives it back to be
    /// repaired. `NotFound` only when the revision itself is unknown.
    fn rules<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Option<RulesFile>>;
    /// Read every user-defined type definition.
    fn types<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Vec<TypeDefinition>>;
    /// Read the human correction recorded for one digest of a source item, if any.
    fn correction<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        item: ItemId,
        digest: &'a Digest,
    ) -> PortFuture<'a, Option<String>>;
    /// Apply `changes.edits` in order on top of `changes.expected_head` as one commit.
    ///
    /// Idempotent on `changes.mutation_id`, which names at most one commit: a caller that writes
    /// several commits under one job derives one identity per commit
    /// (`derive_commit_mutation_id`). When a commit carrying this mutation's trailer
    /// already lies after `expected_head` on the head's history, that commit is returned with
    /// `replayed` set, the check is not run and nothing is written. Otherwise, when the head is
    /// not `expected_head` (an `expected_head` the repository does not hold included), the call
    /// fails with `Conflict` and writes nothing. That refusal carries `ErrorDetail::StaleBase`
    /// naming the head and no `field`: the caller, which knows its request, names the field.
    ///
    /// `check` runs once on the staged candidate tree, and is told which other files the edits
    /// made the store rewrite (`CandidateCheck::check`, `rewritten`). Its error rejects the
    /// commit; its warnings are returned in the result, followed by what the edits report
    /// (`Committed::warnings`).
    ///
    /// The store keeps those warnings, an empty list included, in a record of its own keyed by
    /// the workspace and the mutation, written before the commit lands, and the record names
    /// the commit it was kept for. The rule a caller can rely on: a replay answers the warnings
    /// that commit was given, or says they are unavailable; never another attempt's. A crash
    /// between that record and the commit leaves a record without a commit; the next attempt
    /// finds no commit, runs the check again, replaces the record and commits. A replay of a
    /// commit the store holds no such record for (a commit written before the store kept them,
    /// one a restore brought from an archive, which carries none, or a record that no longer
    /// reads) answers the one warning `replay_warnings_unavailable`
    /// (`REPLAY_WARNINGS_UNAVAILABLE`) and still succeeds: the commit has landed. So does a
    /// replay of a commit whose record a later attempt replaced: an attempt of the same
    /// mutation whose `expected_head` is at or after the landed commit does not find it, runs
    /// its check and keeps its own warnings, and when it then cannot land (the head moved) the
    /// record is that attempt's. The landed commit is from then on answered with the one
    /// warning, not with the warnings of an attempt that wrote another tree; what its own check
    /// gave is no longer held. The records are removed only with their workspace, by
    /// its purge (`Purger`), so they grow by one for each commit. An item purge is not built;
    /// when it is, it has to remove the records of the workspace too, because a warning can
    /// quote a path or a title.
    ///
    /// Refuses, writing nothing, an edit whose path collides with another item (Stage 1b design
    /// section 7) and a caller edit that changes an application header
    /// (`items::refuse_header_change`): `CreateItem` and `EditItem` properties, a `SetType`
    /// definition (`items::refuse_header_in_type`), or a draft. The server's own header
    /// updates are allowed: `SetStatus`, `WriteSourceCard`, `CorrectDigest` and
    /// `SupplyExtraction`, and the server's restores, which write each restored item's header
    /// as the earlier revision holds it: `RestorePaths`, `RestoreItem` and `RestoreWorkspace`.
    fn commit<'a>(
        &'a self,
        scope: &'a StorageScope,
        changes: CommitChanges,
        check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed>;
    /// Find the commit on the head's history that carries `mutation_id`.
    ///
    /// `Some(since)` searches only the commits after `since`; a `since` the repository does not
    /// hold is the caller's error and answers `NotFound` (never `Unavailable`, which a caller
    /// would retry). `None` searches the accepted line back from the head, newest first,
    /// following first parents, and reads no other reference, so a proposal candidate is never
    /// found. It searches the whole accepted line, back to the root, with no time bound: a
    /// mutation whose lease expired without completion or release stays until it is reconciled
    /// (SPEC section 8), so its commit may be of any age.
    ///
    /// Only a writer whose expected head is not stable across attempts needs this before it
    /// calls `commit`: a Snapshot reads the current head, so on a resumed attempt it asks first,
    /// with `None`. It is asked only on a resumed attempt, so the walk's cost falls on resumes.
    /// It never asks from one draft's base: a draft saved again after the commit has a base the
    /// commit does not lie after.
    ///
    /// The commit is answered as a replay of it by `commit` is: `replayed` set, with the
    /// warnings the store kept for that commit, or the one warning
    /// `replay_warnings_unavailable` when it kept none or holds only those of another attempt's
    /// commit under the mutation. A commit no check ran for and that neither `commit` nor
    /// `promote_candidate` wrote (the one `Backups::restore_import` writes, a workspace's first)
    /// has no such record, so it is answered with that one warning; its caller reads the
    /// revision only.
    fn find_commit<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        since: Option<&'a Revision>,
    ) -> PortFuture<'a, Option<Committed>>;
    /// Write a proposal candidate commit on top of `changes.base` and retain it at
    /// `refs/okf-jawn/proposals/<proposal id>`. The head does not move.
    ///
    /// The caller derives `proposal_id` with `derive_proposal_id`, so a resumed attempt names
    /// the same reference.
    ///
    /// Idempotent on `changes.mutation_id`: when the reference already points at a commit
    /// carrying this mutation's trailer, that revision is returned and nothing is written.
    /// `check` runs on the staged candidate tree exactly as it does for `commit`, and the
    /// path-collision and application-header refusals of `commit` apply. The candidate is
    /// answered with the check's findings (`Committed::warnings`), which the store keeps with
    /// it as it keeps a commit's, so a repeat answers the same findings, `replayed` set (or
    /// the one warning `replay_warnings_unavailable` when it kept none for that candidate).
    /// When another write already set the reference, the call fails with a plain `Conflict`: it
    /// is about no base, so it carries no `StaleBase`.
    fn create_candidate<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal_id: ProposalId,
        changes: CandidateChanges,
        check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed>;
    /// Accept a candidate: commit the candidate's exact tree on top of `expected_head`.
    ///
    /// Fails with `Conflict`, writing nothing, when the head is not `expected_head` (with
    /// `ErrorDetail::StaleBase` naming the head and no `field`, as `commit`); when `candidate` is
    /// not the revision the proposal reference retains (a plain `Conflict`: it is about no base);
    /// and when the candidate's parent is not `expected_head`, which is then the head, also with
    /// `StaleBase` naming the head and no `field`: by contract ruling 5 a candidate drafted on a
    /// base that is not the head is stale. The promoted tree is the tree that was checked when the
    /// candidate was created, so no check runs here and the result has no warnings. Idempotent
    /// on `promotion.mutation_id` exactly as `commit` is: the store keeps that empty answer as it
    /// keeps a commit's warnings, so a replay answers no warnings either.
    fn promote_candidate<'a>(
        &'a self,
        scope: &'a StorageScope,
        promotion: Promotion,
    ) -> PortFuture<'a, Committed>;
    /// Read retained history.
    fn log<'a>(&'a self, scope: &'a StorageScope, query: LogQuery) -> PortFuture<'a, LogResponse>;
    /// Compare two retained revisions.
    fn diff<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: DiffQuery,
    ) -> PortFuture<'a, DiffResponse>;
    /// Show the commit that last changed each selected line.
    fn blame<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: BlameQuery,
    ) -> PortFuture<'a, BlameResponse>;
    /// The 1-based line of the item file at `revision`, counted in the stored bytes, on which the
    /// item's body starts: 1 for a file without frontmatter; otherwise the line after the closing
    /// `---`, or the line after that when it is empty (the one blank line a parse strips). For an
    /// empty body it is the line where a body would begin, which can be past the end of the file.
    /// Lines a diff's patch numbers from this line on are body lines `line - start + 1`, the
    /// lines a read and a blame cite; a line before it is frontmatter. `NotFound` when no item
    /// with that identity is at `revision`.
    fn body_start<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        item_id: ItemId,
    ) -> PortFuture<'a, u32>;
    /// Write the retained history of the accepted line up to `revision` into the blob store,
    /// for a portable export that asks for it (`JobSpec::ExportWorkspace::include_history`).
    ///
    /// The object is a Git repository that stock `git` can clone once the export archive is
    /// extracted; storage picks the concrete form (a bundle, or an archived bare repository).
    /// It holds the commits reachable from `revision` on the accepted line and nothing else: no
    /// proposal or candidate reference, and nothing a purge removed. Idempotent on
    /// `mutation_id`: a repeated id writes nothing and returns the object the first call wrote.
    /// The export handler adds the returned object to the export archive.
    fn write_history<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo>;
}

/// A new blank workspace.
#[derive(Debug, Clone)]
pub struct NewWorkspace {
    /// Display name.
    pub name: String,
    /// Purpose of the workspace.
    pub description: String,
    /// Who creates it; recorded as the author of the initial commit.
    pub creator: Provenance,
}

/// New presentation metadata for a workspace.
#[derive(Debug, Clone)]
pub struct WorkspaceUpdate {
    /// Head the caller saw; a moved head conflicts.
    pub expected_head: Revision,
    /// New display name.
    pub name: String,
    /// New purpose.
    pub description: String,
    /// Who makes the change.
    pub author: Provenance,
}

/// Archival of a workspace without deleting anything it retains.
#[derive(Debug, Clone)]
pub struct WorkspaceArchive {
    /// Head the caller saw; a moved head conflicts.
    pub expected_head: Revision,
    /// Who archives it.
    pub author: Provenance,
}

/// The deployment's workspaces, separate from the folder structure inside each one.
///
/// The catalog knows tenants and workspaces, not callers. It never receives a `Principal` and
/// never filters: the application lists by the grants `AccessControl` returns and fills
/// `Workspace::permissions` with `workspace_with_permissions`. The catalog returns that field
/// empty.
pub trait WorkspaceCatalog: Send + Sync {
    /// Every workspace of the tenant, in creation order; archived ones only when
    /// `include_archived`.
    fn list<'a>(
        &'a self,
        tenant: &'a TenantId,
        include_archived: bool,
    ) -> PortFuture<'a, Vec<Workspace>>;
    /// Create a blank workspace: its repository and an initial commit, with no sample content.
    ///
    /// Unique on `mutation_id`: a repeated id creates nothing and returns the prior workspace.
    /// The workspace identity is `derive_workspace_id(mutation_id)`, so a creation interrupted
    /// between writing the repository and committing the record completes on retry.
    fn create<'a>(
        &'a self,
        tenant: &'a TenantId,
        mutation_id: MutationId,
        workspace: NewWorkspace,
    ) -> PortFuture<'a, Workspace>;
    /// Read one workspace, archived or not; `NotFound` when the tenant has no such workspace.
    fn open<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Workspace>;
    /// Replace the name and description.
    ///
    /// Fails with `Conflict` when the head is not `update.expected_head`, with
    /// `ErrorDetail::StaleBase` naming the head and no `field`: the caller names its request's.
    /// The head is compared and the change made as one step against commits: under the
    /// workspace's write lock that every commit holds from its own head comparison to its new
    /// head, so no commit lands between the two (`archive` likewise). A repeated `mutation_id`
    /// changes nothing and returns the workspace as the first call left it.
    fn update<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        update: WorkspaceUpdate,
    ) -> PortFuture<'a, Workspace>;
    /// Archive the workspace and set `archived_at`; its history, objects and records stay.
    ///
    /// Fails with `Conflict` when the head is not `archive.expected_head`, with
    /// `ErrorDetail::StaleBase` naming the head and no `field`, as `update` does. Fails with
    /// `Conflict` while an import, redigest or restore job of the workspace is queued or
    /// running, checked in the archive's own transaction, so an archive and such a job's
    /// insert (`RecordStore::create_job`) racing end with one of them refused (contract ruling
    /// 25). A repeated `mutation_id` changes nothing and returns the archived workspace.
    fn archive<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        archive: WorkspaceArchive,
    ) -> PortFuture<'a, Workspace>;
    /// Return an archived workspace to ordinary listings and clear `archived_at`.
    ///
    /// A workspace that is not archived is returned unchanged. A repeated `mutation_id`
    /// changes nothing and returns the workspace as the first call left it.
    fn unarchive<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        author: Provenance,
    ) -> PortFuture<'a, Workspace>;
}

/// Removes a purge target from every store that holds it; storage implements it, because one
/// call must own every store it clears (Stage 1b design section 6).
///
/// Both calls are idempotent on the mutation id and resumable after a crash. A workspace purge
/// returns only when nothing section 6 lists still holds the workspace, and counts the objects
/// kept because a surviving workspace or tenant record still references, pends or pins them. A
/// purge is never cancelled; a failed call is resumed by repeating it with the same mutation
/// and purge ids.
///
/// What the caller must do:
///
/// - Record the purge first (`RecordStore::create_purge`), and only then call the purger.
/// - Before the first call, cancel every queued or running job of the target workspace and
///   wait until none is running (the drain). Running is the store's state: a job the drain
///   cancelled is `cancelled` at once, though its attempt may still be inside a storage call,
///   and each write that attempt then makes to the workspace is one of those refused below; a
///   sealed restore is never cancelled, so while its attempt runs, its import, which storage
///   does not refuse, is waited for. From the moment the purge is recorded storage
///   refuses to create, retry or claim a job of the workspace (`RecordStore::create_job`,
///   `retry_job`, `claim_job`, each with `Conflict` naming the purge; a refused claim leaves
///   the job as it is, for the caller to cancel), but it cannot stop a job that already runs.
///   A job past its point of no return (a restore that sealed itself, `RecordStore::seal`)
///   refuses its cancellation: the drain waits for it to finish instead. A workspace archive
///   and a written history check the refusal before and again under the
///   workspace's lock, and store their files as workspace-scoped puts: a pending put is
///   recorded before the bytes are retained, and their record lands it, so a stop before the
///   record leaves bytes the objects step deletes. An import copies its rows without the
///   refusal; it runs only in a sealed restore, which the drain waits for. Ingest's purge job
///   handler is this caller.
///
/// What storage guarantees on its own, from the moment the purge is recorded and after the
/// purge completes:
///
/// - Every write scoped to the target workspace listed here is refused with `Conflict` naming
///   the purge, checked inside the write's own SQLite transaction: new jobs, job retries and
///   claims, drafts (save, discard), upload slots, upload content, upload completions (also
///   checked before anything is read, so a completion after the purge is refused rather than
///   answered `NotFound`), upload consumptions, one upload or a batch (`UploadStore::consume`
///   and `consume_batch`, each checked before anything is read as well), artifacts, job
///   completions, derived objects, receipts, reviews,
///   proposals (insert, update), comments, confirmations (create, consume), sandbox
///   capabilities and catalog changes. Commits, candidates and promotions are refused under the
///   workspace's write lock, under which the purge reads the workspace's records and history
///   for its plan, so each either is in the plan or is refused. Every content-bearing write a
///   conversion makes is among these: its object puts, its commits, its derived objects and its
///   job completion. Events are not refused: an event carries identities only. Job
///   cancellation is never refused because of the purge (only a sealed job's, by its seal).
/// - A workspace-scoped object put (`BlobStore::put`, an upload completion, an import) pins
///   the object for as long as the write runs and then, in one transaction, checks the refusal
///   again and records a durable pending put, before it retains the object. Apart from the
///   purge of its own workspace, which clears it, a pending put is deleted only in the
///   transaction of a record of the workspace that names the object (a commit's or a
///   candidate's just after it landed). What a record deletes is what the stored record names:
///   a write answered by an earlier record (a replayed artifact, commit or candidate) deletes
///   only the pending puts of objects that record names, and an import only those of the
///   archive objects its restored history and rows name, or a conversion record among them
///   names. So a put whose record never lands (a failed or cancelled job, a failed retain, a
///   refused record write, a crash, a replay of a record that names different bytes) keeps its
///   pending put, which has no lifetime.
/// - Before the purge deletes the target's objects it waits until no write of the target that
///   pinned is still running, for at most ten minutes; after that the call fails with
///   `Unavailable` having deleted none of the target's objects (its earlier steps, which remove
///   the target's managed backups, rows, index entries and repository, may already have run),
///   and repeating it resumes the purge. It then deletes every object the target's pending
///   puts name that no surviving reference names, and clears those pending puts.
/// - Writes to other workspaces of the tenant continue during the purge. An object is deleted
///   only when no durable reference the purge read names it (another workspace's pending puts
///   included) and no other workspace's write pinned it at any moment from the start of that
///   read until the delete; so an object another workspace puts and then references while the
///   purge runs is kept. One exception: a copy of one of the target's workspace archives that
///   another workspace holds in an upload slot or names in a pending put is no surviving
///   reference, and the archive goes with the target. A write of another workspace that is
///   putting the same bytes during the objects step pins them and keeps them, as it keeps any
///   pinned object. A pending put that never gets a record keeps an identical object of
///   another workspace's purge, counted as kept, until the next purge of its own workspace
///   deletes the bytes and clears it.
/// - Index entries an indexing of the target stored after the index step are removed by the
///   index pass the purge runs again after it deleted the repository; an indexing reads and
///   stores under the workspace's write lock, so a later one finds no repository.
/// - Managed backups are read again on every attempt and once more before the purge
///   completes, so a backup written while the purge was unfinished is reached as well.
///
/// Pins live in the storage process's memory, and the data directory has one writer process,
/// so a crash ends every write in flight; what such a write stored is named by its pending put,
/// which carries it to the next purge of its workspace.
pub trait Purger: Send + Sync {
    /// Remove a workspace: its repository, the objects only it references, its records, index
    /// entries, managed backups and retained exports.
    fn purge_workspace<'a>(
        &'a self,
        tenant: &'a TenantId,
        mutation_id: MutationId,
        purge: PurgeId,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, PurgeReport>;
    /// Remove one item's bytes, derivatives, index entries and history, rewrite or delete the
    /// managed backups that hold it, and write the revision map
    /// (`RecordStore::revision_mapping`).
    fn purge_item<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        purge: PurgeId,
        item: ItemId,
    ) -> PortFuture<'a, PurgeReport>;
}

/// A workspace archive written into the blob store, and what it left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenArchive {
    /// Identity and size of the archive's bytes.
    pub object: ObjectInfo,
    /// Rows of the workspace's records left out of the archive because they named a managed
    /// backup or a written history; the archive's manifest holds the same count, and a restore
    /// of the archive reports it (`RestoreReport::rows_left_out`).
    pub rows_left_out: u32,
}

/// Independently recoverable backups (Stage 1b design section 8); storage implements them.
///
/// The offline backup, the pre-migration backup and the replacement restore are storage
/// functions the server command and startup call, not ports.
pub trait Backups: Send + Sync {
    /// Write a workspace archive into the blob store and answer it with the rows it left out;
    /// the backup handler records it as a `WorkspaceBackup` artifact and its job carries the
    /// count (`Job::backup`). Idempotent on the mutation id: a repeat answers the archive
    /// already written, with the count its manifest holds.
    fn write_workspace_archive<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
    ) -> PortFuture<'a, WrittenArchive>;
    /// Write an installation archive (identities, grants, connector records without secrets,
    /// purge records, tenant events; no workspace content) into the tenant's store; the
    /// handler records it as an `InstallationBackup` artifact. Idempotent on the mutation id.
    fn write_installation_archive<'a>(
        &'a self,
        tenant: &'a TenantId,
        mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo>;
    /// Open a selected byte interval of an installation archive in the tenant's store, for the
    /// `download_tenant_artifact` transport; `NotFound` when the tenant holds no such object.
    ///
    /// `BlobStore` is workspace-scoped and a tenant may have no workspace, so the store that
    /// writes installation archives also reads them back. Like `BlobStore::open`, a digest
    /// authorizes nothing: the caller has already authorized the artifact record.
    fn open_installation_archive<'a>(
        &'a self,
        tenant: &'a TenantId,
        digest: &'a Digest,
        offset: u64,
        length: u64,
    ) -> PortFuture<'a, ObjectRead>;
    /// Import a workspace archive into `scope`. `editors` are the subjects with `write` on
    /// the target, computed by core from `AccessControl`: a draft whose editor is not among them
    /// stays in the archive and is counted as unassigned. Idempotent on the mutation id.
    ///
    /// After installing the archive's history it writes one commit on top of the restored head
    /// carrying `mutation_id` (the restored head's tree, no content change), so
    /// `VersionStore::find_commit(scope, mutation_id, None)` finds the revision the restore
    /// produced, on a first or a resumed attempt alike; a repeat writes no second commit.
    fn restore_import<'a>(
        &'a self,
        scope: &'a StorageScope,
        archive: Digest,
        mutation_id: MutationId,
        editors: Vec<String>,
    ) -> PortFuture<'a, RestoreReport>;
}

/// The code of the one warning a replayed commit answers when the store holds no warnings for
/// it (`VersionStore::commit`); see `replay_warnings_unavailable`.
pub const REPLAY_WARNINGS_UNAVAILABLE: &str = "replay_warnings_unavailable";

/// The code of the one warning a folder move or delete answers when files that are not items
/// went with the folder (`TreeEdit::MoveFolder`, `TreeEdit::DeleteFolder`); see
/// `folder_other_files` and `folder_files_deleted`.
pub const FOLDER_OTHER_FILES: &str = "folder_other_files";

/// The item owning a versioned companion path: its source appearance, accepted supplied text,
/// or any file under its corrections folder. IDs use the canonical spelling written by the
/// store; a differently spelled or unrelated path belongs to no item.
///
/// Folder conflict checks and item-filtered history share this ownership rule so a change to
/// an item outside its visible folder is still a change to that item.
#[must_use]
pub fn item_companion_owner(path: &str) -> Option<ItemId> {
    let text = if let Some(file) = path
        .strip_prefix(".okf/sources/")
        .or_else(|| path.strip_prefix(".okf/supplied/"))
    {
        file.strip_suffix(".json")?
    } else {
        path.strip_prefix(".okf/corrections/")?.split_once('/')?.0
    };
    let id = uuid::Uuid::parse_str(text).ok()?;
    (id.to_string() == text).then_some(ItemId(id))
}

impl ChangeContext {
    /// The context of `change`, with `new_item_kind` filled from it: the role its `item_kind`
    /// names, a note when it names none (`Change::created_kind`). A change that creates no item
    /// leaves it a note, which `TreeEdit::from_change` does not read.
    #[must_use]
    pub fn for_change(
        change: &Change,
        new_item_id: ItemId,
        proposal_id: ProposalId,
        proposer: Provenance,
    ) -> Self {
        Self {
            new_item_id,
            new_item_kind: change.created_kind().unwrap_or(ItemKind::Note),
            proposal_id,
            proposer,
        }
    }
}

impl TreeEdit {
    /// The tree edit a proposed change stands for.
    ///
    /// `context.new_item_id` and `context.new_item_kind` are used only for `Change::Create`,
    /// which carries no identity and whose role the context holds (`ChangeContext::for_change`
    /// fills it from the change); the proposal and proposer only for `Change::SupplyExtraction`.
    #[must_use]
    pub fn from_change(change: Change, context: &ChangeContext) -> Self {
        match change {
            Change::Create {
                path,
                type_name,
                body,
                properties,
                item_kind: _,
            } => Self::CreateItem {
                item_id: context.new_item_id,
                path,
                title: None,
                type_name,
                kind: context.new_item_kind.clone(),
                body,
                properties,
            },
            Change::Edit {
                item_id,
                body,
                properties,
            } => Self::EditItem {
                item_id,
                body,
                properties,
            },
            Change::Move {
                item_id,
                destination,
            } => Self::MoveItem {
                item_id,
                destination,
                keep_alias: false,
            },
            Change::Archive { item_id } => Self::SetStatus {
                item_id,
                status: None,
                archived: Some(true),
            },
            Change::SupplyExtraction {
                item_id,
                based_on,
                pages,
                markdown,
            } => Self::SupplyExtraction {
                item_id,
                based_on,
                pages,
                markdown,
                proposal_id: context.proposal_id,
                supplier: context.proposer.clone(),
            },
        }
    }
}

impl From<PageRequest> for Page {
    fn from(page: PageRequest) -> Self {
        Self {
            cursor: page.cursor,
            limit: page.limit,
        }
    }
}

impl SourceCard {
    /// The application header the card is written with: its item id, kind `Source`,
    /// `archived`, the original's first observed name and digest, and the extraction it shows.
    ///
    /// `archived` is the flag the item has now: `false` for a new card, and the stored flag
    /// when a redigest replaces a card, so replacing an archived source never unarchives it.
    ///
    /// # Errors
    /// Returns `Internal` when the appearance has no observed name: the header records the
    /// original's name exactly, and an empty one would be written as if it were observed.
    pub fn header(&self, archived: bool) -> Result<ApplicationHeader, ApiError> {
        let original_name = self
            .appearance
            .names
            .first()
            .map(|name| name.filename.clone())
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::Internal,
                    "a source card needs an observed name for its application header",
                )
            })?;
        Ok(ApplicationHeader {
            archived,
            ..ApplicationHeader::assigned(
                self.item_id,
                Some(SourceHeader {
                    original_name,
                    digest: self.appearance.object.clone(),
                }),
                Some(self.appearance.extraction.clone()),
            )
        })
    }
}

impl Provenance {
    /// Record the authenticated principal as the author of a write or the initiator of a job.
    #[must_use]
    pub fn from_principal(principal: &Principal) -> Self {
        Self {
            subject: principal.subject.clone(),
            route: principal.route.clone(),
            client_id: principal.client_id.clone(),
        }
    }
}

/// The one warning a replayed commit answers in place of warnings the store does not hold for
/// it, saying `why` (`VersionStore::commit`). A version store, real or fake, answers a replay
/// with exactly this, so a caller sees one code whatever store it runs over.
#[must_use]
pub fn replay_warnings_unavailable(why: &str) -> Warning {
    Warning {
        code: REPLAY_WARNINGS_UNAVAILABLE.to_owned(),
        message: format!(
            "the commit landed on an earlier attempt, and the warnings its check gave then are \
             not available: {why}"
        ),
        location: None,
    }
}

/// The one warning that `count` files that are not items went with a folder: moved with it to
/// `at` (`TreeEdit::MoveFolder`). It names the folder where the files are now, so a person can
/// find them; the store adds it to the commit's warnings (`Committed::warnings`) only when
/// `count` is not zero.
#[must_use]
pub fn folder_other_files(at: &WorkspacePath, count: usize) -> Warning {
    Warning {
        code: FOLDER_OTHER_FILES.to_owned(),
        message: format!(
            "{} moved with the folder to {}",
            not_items(count),
            at.as_str()
        ),
        location: Some(at.as_str().to_owned()),
    }
}

/// The one warning that `count` files that are not items were deleted with the folder `folder`
/// (`TreeEdit::DeleteFolder`), naming it; added to the commit's warnings only when `count` is
/// not zero. The revisions before the delete still hold them.
#[must_use]
pub fn folder_files_deleted(folder: &WorkspacePath, count: usize) -> Warning {
    let verb = if count == 1 { "was" } else { "were" };
    Warning {
        code: FOLDER_OTHER_FILES.to_owned(),
        message: format!(
            "{} {verb} deleted with the folder {}",
            not_items(count),
            folder.as_str()
        ),
        location: Some(folder.as_str().to_owned()),
    }
}

/// Attach the caller's effective permissions onto a workspace summary.
#[must_use]
pub fn workspace_with_permissions(
    mut workspace: Workspace,
    permissions: Vec<Permission>,
) -> Workspace {
    workspace.permissions = permissions;
    workspace
}

/// The identity of the `ordinal`-th item created under one mutation.
///
/// The caller allocates the identity of every item it creates. A resumed attempt re-runs its
/// handler under the same `MutationId`, so deriving identities from that id and a running count
/// makes the repeated edits name exactly the items the first attempt committed.
#[must_use]
pub fn derive_item_id(mutation_id: MutationId, ordinal: u32) -> ItemId {
    ItemId(derived_uuid(b"item", mutation_id, ordinal))
}

/// The identity of the `ordinal`-th commit a job writes under one mutation.
///
/// A mutation identity names at most one commit (`VersionStore::commit` replays on it), so a
/// handler that writes several commits (an import writes its pending cards, then each card)
/// commits the `n`-th under `derive_commit_mutation_id(job, n)`. A resumed attempt derives the
/// same identities, so each commit replays on its own identity whatever order the store
/// searches in, and none is written twice.
#[must_use]
pub fn derive_commit_mutation_id(mutation_id: MutationId, ordinal: u32) -> MutationId {
    MutationId(derived_uuid(b"commit", mutation_id, ordinal))
}

/// The identity of the workspace created under one mutation.
///
/// Derived, not allocated, so a creation that crashed after its repository was written and
/// before its record was committed is retried under the same identity, finds that repository and
/// completes it instead of leaving it unreachable.
#[must_use]
pub fn derive_workspace_id(mutation_id: MutationId) -> WorkspaceId {
    WorkspaceId(derived_uuid(b"workspace", mutation_id, 0))
}

/// The identity of the proposal opened under one mutation.
///
/// A resumed attempt derives the same identity, so it finds the candidate reference its first
/// attempt wrote instead of writing a second one.
#[must_use]
pub fn derive_proposal_id(mutation_id: MutationId) -> ProposalId {
    ProposalId(derived_uuid(b"proposal", mutation_id, 0))
}

/// The identity of the comment written under one mutation (`add_comment`, and the reason a
/// `decline_proposal` records).
///
/// A resumed attempt derives the same identity, so a handler can tell the comment its first
/// attempt wrote from every other comment of the proposal.
#[must_use]
pub fn derive_comment_id(mutation_id: MutationId) -> String {
    derived_uuid(b"comment", mutation_id, 0).to_string()
}

/// The identity of the review recorded under one mutation.
///
/// A resumed attempt derives the same identity, so it finds among the item's reviews the one its
/// first attempt recorded.
#[must_use]
pub fn derive_review_id(mutation_id: MutationId) -> ReviewId {
    ReviewId(derived_uuid(b"review", mutation_id, 0))
}

/// The identity of the purge recorded under one mutation.
///
/// Derived, not allocated, so the tenant job that carries the purge out can name it in its
/// specification and both are written in one transaction (`RecordStore::create_purge`).
#[must_use]
pub fn derive_purge_id(mutation_id: MutationId) -> PurgeId {
    PurgeId(derived_uuid(b"purge", mutation_id, 0))
}

/// The identity under which a purge's drain cancels one job of the purged workspace
/// (`RecordStore::cancel_job`; the `Purger` caller's duty).
///
/// Derived from the purge job's mutation and the cancelled job, so a resumed purge cancels each
/// job under the identity its first attempt used, and the store makes no second transition.
#[must_use]
pub fn derive_drain_mutation_id(purge_mutation: MutationId, job: JobId) -> MutationId {
    let mut hasher = Sha256::new();
    hasher.update(b"okf-jawn derived identity\0");
    hasher.update(b"drain\0");
    hasher.update(purge_mutation.0.as_bytes());
    hasher.update(job.0.as_bytes());
    MutationId(version_8(hasher.finalize().into()))
}

/// `count` files that are not items, in words: "1 file that is not an item", "3 files that are
/// not items".
fn not_items(count: usize) -> String {
    if count == 1 {
        "1 file that is not an item".to_owned()
    } else {
        format!("{count} files that are not items")
    }
}

/// A version-8 UUID from SHA-256 of a label, a mutation identity and a count.
fn derived_uuid(label: &[u8], mutation_id: MutationId, ordinal: u32) -> uuid::Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"okf-jawn derived identity\0");
    hasher.update(label);
    hasher.update(b"\0");
    hasher.update(mutation_id.0.as_bytes());
    hasher.update(ordinal.to_be_bytes());
    version_8(hasher.finalize().into())
}

/// The version-8 UUID of the first sixteen bytes of a hash.
fn version_8(hash: [u8; 32]) -> uuid::Uuid {
    let mut bytes = [0_u8; 16];
    for (target, source) in bytes.iter_mut().zip(hash) {
        *target = source;
    }
    uuid::Builder::from_custom_bytes(bytes).into_uuid()
}
