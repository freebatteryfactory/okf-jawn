//! Storage interfaces preserve byte identity, occurrence identity, and revision preconditions.
//!
//! Implementations use selected libraries; they do not infer domain approval from a Git merge.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::Pin;

use okf_jawn_contract::{
    access::Principal,
    common::MutationResult,
    history::{BlameRequest, BlameResponse, DiffRequest, DiffResponse, LogRequest, LogResponse},
    identity::{At, Digest, ItemId, Revision, WorkspaceId, WorkspacePath},
    item::{ItemDocument, ItemSummary},
    proposal::Change,
    workspace::Workspace,
};
use tokio::io::AsyncRead;

use crate::ports::PortFuture;

/// Owned streaming bytes, not a base64 document copied through application messages.
pub type ByteReader = Pin<Box<dyn AsyncRead + Send + Unpin + 'static>>;

/// Authenticated deployment storage scope, established by application policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageScope {
    /// Tenant namespace; an object hash never substitutes for this scope.
    pub tenant_id: String,
    /// Workspace to which the operation belongs.
    pub workspace_id: WorkspaceId,
}

/// Retained byte metadata; occurrence-specific names live in source cards instead.
#[derive(Debug, Clone)]
pub struct ObjectInfo {
    /// Digest verified against stored bytes.
    pub digest: Digest,
    /// Exact byte count.
    pub size: u64,
    /// Detected content type.
    pub media_type: String,
}

/// A bounded stream and the object identity to which it belongs.
pub struct ObjectRead {
    /// Whole-object metadata.
    pub object: ObjectInfo,
    /// Byte offset at which this stream starts.
    pub offset: u64,
    /// Maximum bytes exposed by this reader.
    pub length: u64,
    /// Bounded body owned by the reader.
    pub body: ByteReader,
}

/// Controlled local materialization for a converter process.
#[derive(Debug, Clone)]
pub struct LocalSource {
    /// Caller-created path; never an unchecked client pathname.
    pub path: PathBuf,
    /// Identity checked during materialization.
    pub object: ObjectInfo,
}

/// Immutable scoped object storage; use `object_store`, not another storage protocol.
pub trait BlobStore: Send + Sync {
    /// Store and hash a bounded stream; preserve an already stored identical object.
    fn put<'a>(
        &'a self,
        scope: &'a StorageScope,
        body: ByteReader,
        limit: u64,
        expected: Option<Digest>,
    ) -> PortFuture<'a, ObjectInfo>;
    /// Open an already authorized object's selected byte interval.
    fn open<'a>(
        &'a self,
        scope: &'a StorageScope,
        digest: &'a Digest,
        offset: u64,
        length: u64,
    ) -> PortFuture<'a, ObjectRead>;
    /// Materialize retained bytes for a bounded converter worker.
    fn materialize<'a>(
        &'a self,
        scope: &'a StorageScope,
        digest: &'a Digest,
    ) -> PortFuture<'a, LocalSource>;
}

/// Complete atomic Git change intent with an explicit expected head.
#[derive(Debug, Clone)]
pub struct CommitChanges {
    /// Head from which the operation was prepared.
    pub expected_head: Revision,
    /// Human or agent provenance, not a claim of review.
    pub principal: Principal,
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
        expected: Revision,
        target: Revision,
        paths: Vec<WorkspacePath>,
        principal: Principal,
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
    /// Resolve deployment storage scope only after access was checked.
    fn scope<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, StorageScope>;
}
