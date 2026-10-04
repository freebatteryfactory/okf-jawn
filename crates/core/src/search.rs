//! Derived search and link indexes over committed workspace content.
//!
//! Everything behind this port is rebuildable from retained Git content. Rebuilding never
//! touches jobs, reviews, or receipts, which live only in `RecordStore` and cannot be rebuilt.

use okf_jawn_contract::identity::Revision;
use okf_jawn_contract::search::{
    GetGraphRequest, GetGraphResponse, GetLinksRequest, GetLinksResponse, SearchRequest,
    SearchResponse,
};

use crate::ports::PortFuture;
use crate::storage::StorageScope;

/// Full-text, link, and graph queries; storage implements them with SQLite FTS.
///
/// The application authorizes the caller, resolves `At` to a `Revision`, and supplies the
/// already-scoped workspace. Implementations never widen scope or re-resolve the revision.
pub trait SearchIndex: Send + Sync {
    /// Index the committed content of one revision of a workspace.
    fn index_revision<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: Revision,
    ) -> PortFuture<'a, ()>;
    /// Query one indexed revision, returning `Unavailable` if that revision is not indexed.
    fn search<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: Revision,
        request: SearchRequest,
    ) -> PortFuture<'a, SearchResponse>;
    /// Read outgoing links or backlinks recorded for one indexed revision.
    fn links<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: Revision,
        request: GetLinksRequest,
    ) -> PortFuture<'a, GetLinksResponse>;
    /// Read a bounded graph projection from the same indexed revision.
    fn graph<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: Revision,
        request: GetGraphRequest,
    ) -> PortFuture<'a, GetGraphResponse>;
    /// Discard and recreate this workspace's derived index data from retained content at `head`.
    fn rebuild<'a>(&'a self, scope: &'a StorageScope, head: Revision) -> PortFuture<'a, ()>;
}
