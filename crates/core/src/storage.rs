//! Storage interfaces preserve byte identity, occurrence identity, and revision preconditions.
//!
//! Implementations use selected libraries; they do not infer domain approval from a Git merge.
//! Every commit carries an `Okf-Jawn-Mutation:` trailer naming the durable write identity.
//! Ports take resolved `Revision`s and core parameter types: the application resolves selectors
//! and authorizes the caller before it calls a port.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::Pin;

use okf_jawn_contract::{
    access::{AccessRoute, Permission, Principal},
    common::{MutationResult, PageRequest},
    history::{BlameRequest, BlameResponse, DiffRequest, DiffResponse, LogRequest, LogResponse},
    identity::{
        At, Digest, ItemId, MutationId, ProposalId, Revision, TenantId, WorkspaceId, WorkspacePath,
    },
    item::{ItemDocument, ItemSummary},
    proposal::Change,
    workspace::{ArchiveWorkspaceRequest, UpdateWorkspaceRequest, Workspace},
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

/// Where `find_mutation` scans for an `Okf-Jawn-Mutation:` trailer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VersionTarget {
    /// The accepted workspace head.
    Head,
    /// A retained proposal candidate ref.
    Proposal(ProposalId),
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

/// Complete atomic Git change intent with an explicit expected head and mutation trailer.
#[derive(Debug, Clone)]
pub struct CommitChanges {
    /// Durable write identity written as `Okf-Jawn-Mutation:` in the commit.
    pub mutation_id: MutationId,
    /// Head from which the operation was prepared.
    pub expected_head: Revision,
    /// Human or agent provenance, not a claim of review.
    pub author: Provenance,
    /// Git commit message.
    pub message: String,
    /// Authored changes to apply and link rewrites to compute.
    pub changes: Vec<Change>,
}

/// Versioned workspace content; implementation supplies locking and safe library calls.
pub trait VersionStore: Send + Sync {
    /// Resolve a selector once; all following reads use the returned revision.
    fn resolve<'a>(&'a self, scope: &'a StorageScope, at: &'a At) -> PortFuture<'a, Revision>;
    /// List entries at an exact revision.
    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        path: Option<&'a WorkspacePath>,
    ) -> PortFuture<'a, Vec<ItemSummary>>;
    /// Read authored content without substituting the current working tree.
    fn show<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
        item: ItemId,
    ) -> PortFuture<'a, ItemDocument>;
    /// Commit once against `expected_head`, or return a conflict without partial application.
    fn commit<'a>(
        &'a self,
        scope: &'a StorageScope,
        changes: CommitChanges,
    ) -> PortFuture<'a, MutationResult>;
    /// Scan `since..tip(target)` for a commit whose mutation trailer matches.
    fn find_mutation<'a>(
        &'a self,
        scope: &'a StorageScope,
        target: VersionTarget,
        since: &'a Revision,
        mutation_id: MutationId,
    ) -> PortFuture<'a, Option<Revision>>;
    /// Write a retained proposal candidate at `refs/okf-jawn/proposals/<id>`.
    fn create_candidate<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal_id: ProposalId,
        base: &'a Revision,
        changes: Vec<Change>,
        mutation_id: MutationId,
    ) -> PortFuture<'a, Revision>;
    /// Promote a candidate onto head against `expected_head`.
    fn promote_candidate<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal_id: ProposalId,
        expected_head: &'a Revision,
        candidate: &'a Revision,
        mutation_id: MutationId,
    ) -> PortFuture<'a, MutationResult>;
    /// Read retained history.
    fn log<'a>(
        &'a self,
        scope: &'a StorageScope,
        request: LogRequest,
    ) -> PortFuture<'a, LogResponse>;
    /// Compare exact retained snapshots.
    fn diff<'a>(
        &'a self,
        scope: &'a StorageScope,
        request: DiffRequest,
    ) -> PortFuture<'a, DiffResponse>;
    /// Show last-changing commits for selected lines.
    fn blame<'a>(
        &'a self,
        scope: &'a StorageScope,
        request: BlameRequest,
    ) -> PortFuture<'a, BlameResponse>;
    /// Restore selected state as a new commit, never reset retained history.
    fn restore<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        expected: Revision,
        target: Revision,
        paths: Vec<WorkspacePath>,
        author: Provenance,
    ) -> PortFuture<'a, MutationResult>;
}

/// Deployment catalog is separate from the arbitrary folder structure inside each workspace.
pub trait WorkspaceCatalog: Send + Sync {
    /// List metadata already filtered for the authenticated principal.
    fn list<'a>(&'a self, principal: &'a Principal) -> PortFuture<'a, Vec<Workspace>>;
    /// Create a blank workspace with configured permissions and no sample content.
    fn create<'a>(
        &'a self,
        principal: &'a Principal,
        name: String,
        description: String,
        properties: BTreeMap<String, serde_json::Value>,
    ) -> PortFuture<'a, Workspace>;
    /// Open an existing authorized workspace.
    fn open<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, Workspace>;
    /// Update workspace presentation metadata at a known revision.
    fn update<'a>(
        &'a self,
        principal: &'a Principal,
        request: UpdateWorkspaceRequest,
    ) -> PortFuture<'a, MutationResult>;
    /// Archive without deleting retained historical content.
    fn archive<'a>(
        &'a self,
        principal: &'a Principal,
        request: ArchiveWorkspaceRequest,
    ) -> PortFuture<'a, MutationResult>;
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
