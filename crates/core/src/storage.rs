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
    error::ApiError,
    history::{BlameResponse, DiffResponse, LogResponse},
    identity::{
        Digest, ItemId, MutationId, ProposalId, PurgeId, Revision, TenantId, WorkspaceId,
        WorkspacePath,
    },
    item::{ItemDocument, ItemKind, ItemStatus, ItemSummary, TypeDefinition},
    proposal::Change,
    source::SourceAppearance,
    workspace::Workspace,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncRead;

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
        /// Built-in rendering role.
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
    DeleteItem {
        /// Item to remove.
        item_id: ItemId,
    },
    /// Create a folder together with its maintained `index.md`.
    CreateFolder {
        /// New folder path.
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
    /// Corrections recorded with `CorrectDigest` are kept when a card is replaced.
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
    /// Generated extraction shown as the card body; empty unless `extraction` is `Converted`.
    pub body: String,
    /// Preserved extension properties.
    pub properties: BTreeMap<String, serde_json::Value>,
    /// Occurrence metadata: object, observed names, media type, size, parent and successor.
    pub appearance: SourceAppearance,
    /// What the card shows about turning its bytes into text.
    pub extraction: Extraction,
}

/// The extraction state a source card shows; the original bytes are retained in every state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extraction {
    /// Conversion has not finished.
    Pending,
    /// The card body is a generated digest of the source bytes.
    Converted {
        /// Identity of the retained conversion record in the blob store.
        digest: Digest,
        /// The converter stopped early; the body covers only part of the source.
        partial: bool,
    },
    /// No extractor exists for this format; the body is empty.
    Unsupported,
    /// Conversion failed; the body is empty.
    Failed {
        /// Safe explanation shown with the item.
        message: String,
    },
}

/// What `TreeEdit::from_change` needs beyond the change itself.
#[derive(Debug, Clone)]
pub struct ChangeContext {
    /// Identity for a `Change::Create`, which carries none.
    pub new_item_id: ItemId,
    /// Rendering role for a `Change::Create`, which carries none.
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
    /// Findings the candidate check returned; empty on a replay.
    pub warnings: Vec<Warning>,
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
/// blocking task that owns that directory.
pub trait CandidateCheck: Send + Sync {
    /// Inspect the complete candidate bundle rooted at `root`.
    ///
    /// # Errors
    /// Returns the error that rejects the write; nothing is committed when this fails.
    fn check(&self, root: &Path) -> Result<Vec<Warning>, ApiError>;
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
    /// Read the naming rules stored in `.okf/rules.yaml`; `None` when no rules were saved.
    fn rules<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Option<NamingRules>>;
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
    /// Idempotent on `changes.mutation_id`: when a commit carrying this mutation's trailer
    /// already lies after `expected_head` on the head's history, that commit is returned with
    /// `replayed` set, the check is not run and nothing is written. Otherwise, when the head is
    /// not `expected_head`, the call fails with `Conflict` and writes nothing.
    ///
    /// `check` runs once on the staged candidate tree. Its error rejects the commit; its
    /// warnings are returned in the result.
    fn commit<'a>(
        &'a self,
        scope: &'a StorageScope,
        changes: CommitChanges,
        check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed>;
    /// Find the commit after `since` on the head's history that carries `mutation_id`.
    ///
    /// Only a writer whose expected head is not stable across attempts needs this before it
    /// calls `commit`: a Snapshot reads the current head, so on a resumed attempt it must ask
    /// first, with the base revision of one of its drafts as `since`.
    fn find_commit<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        since: &'a Revision,
    ) -> PortFuture<'a, Option<Revision>>;
    /// Write a proposal candidate commit on top of `changes.base` and retain it at
    /// `refs/okf-jawn/proposals/<proposal id>`. The head does not move.
    ///
    /// The caller derives `proposal_id` with `derive_proposal_id`, so a resumed attempt names
    /// the same reference.
    ///
    /// Idempotent on `changes.mutation_id`: when the reference already points at a commit
    /// carrying this mutation's trailer, that revision is returned and nothing is written.
    /// `check` runs on the staged candidate tree exactly as it does for `commit`.
    fn create_candidate<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal_id: ProposalId,
        changes: CandidateChanges,
        check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Revision>;
    /// Accept a candidate: commit the candidate's exact tree on top of `expected_head`.
    ///
    /// Fails with `Conflict`, writing nothing, when the head is not `expected_head`, when the
    /// candidate's parent is not `expected_head`, or when `candidate` is not the revision the
    /// proposal reference retains. The promoted tree is the tree that was checked when the
    /// candidate was created, so no check runs here. Idempotent on `promotion.mutation_id`
    /// exactly as `commit` is.
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
    /// Every workspace of the tenant that is not archived, unfiltered, in creation order.
    fn list<'a>(&'a self, tenant: &'a TenantId) -> PortFuture<'a, Vec<Workspace>>;
    /// Create a blank workspace: its repository and an initial commit, with no sample content.
    ///
    /// Unique on `mutation_id`: a repeated id creates nothing and returns the prior workspace.
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
    /// Fails with `Conflict` when the head is not `update.expected_head`. A repeated
    /// `mutation_id` changes nothing and returns the workspace as the first call left it.
    fn update<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        update: WorkspaceUpdate,
    ) -> PortFuture<'a, Workspace>;
    /// Archive the workspace; its history, objects and records stay.
    ///
    /// Fails with `Conflict` when the head is not `archive.expected_head`. A repeated
    /// `mutation_id` changes nothing and returns the archived workspace.
    fn archive<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        archive: WorkspaceArchive,
    ) -> PortFuture<'a, Workspace>;
}

impl TreeEdit {
    /// The tree edit a proposed change stands for.
    ///
    /// `context.new_item_id` and `context.new_item_kind` are used only for `Change::Create`,
    /// which carries neither an identity nor a rendering role; the proposal and proposer only
    /// for `Change::SupplyExtraction`.
    #[must_use]
    pub fn from_change(change: Change, context: &ChangeContext) -> Self {
        match change {
            Change::Create {
                path,
                type_name,
                body,
                properties,
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

/// The identity of the proposal opened under one mutation.
///
/// A resumed attempt derives the same identity, so it finds the candidate reference its first
/// attempt wrote instead of writing a second one.
#[must_use]
pub fn derive_proposal_id(mutation_id: MutationId) -> ProposalId {
    ProposalId(derived_uuid(b"proposal", mutation_id, 0))
}

/// The identity of the purge recorded under one mutation.
///
/// Derived, not allocated, so the tenant job that carries the purge out can name it in its
/// specification and both are written in one transaction (`RecordStore::create_purge`).
#[must_use]
pub fn derive_purge_id(mutation_id: MutationId) -> PurgeId {
    PurgeId(derived_uuid(b"purge", mutation_id, 0))
}

/// A version-8 UUID from SHA-256 of a label, a mutation identity and a count.
fn derived_uuid(label: &[u8], mutation_id: MutationId, ordinal: u32) -> uuid::Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"okf-jawn derived identity\0");
    hasher.update(label);
    hasher.update(b"\0");
    hasher.update(mutation_id.0.as_bytes());
    hasher.update(ordinal.to_be_bytes());
    let hash: [u8; 32] = hasher.finalize().into();
    let mut bytes = [0_u8; 16];
    for (target, source) in bytes.iter_mut().zip(hash) {
        *target = source;
    }
    uuid::Builder::from_custom_bytes(bytes).into_uuid()
}
