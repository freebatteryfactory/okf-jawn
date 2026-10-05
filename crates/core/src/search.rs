//! Derived search and link indexes over committed workspace content.
//!
//! Everything behind this port is rebuildable from retained Git content. Rebuilding never
//! touches jobs, reviews, or receipts, which live only in `RecordStore` and cannot be rebuilt.
//! Every query names exactly one resolved `Revision`; drafts are never indexed.

use okf_jawn_contract::identity::{ItemId, Revision, WorkspacePath};
use okf_jawn_contract::search::{
    GetGraphResponse, GetLinksResponse, LinkDirection, SearchResponse,
};

use crate::ports::PortFuture;
use crate::storage::{Page, StorageScope};

/// A full-text query over one indexed revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// Revision to search.
    pub revision: Revision,
    /// Search expression typed by the caller; never executable SQL.
    pub text: String,
    /// Limit results to this folder and the folders beneath it.
    pub folder: Option<WorkspacePath>,
    /// Include archived and deprecated items.
    pub include_archived: bool,
    /// Bounded page.
    pub page: Page,
}

/// The links of one item at one indexed revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkQuery {
    /// Revision to read.
    pub revision: Revision,
    /// Item whose links are read.
    pub item_id: ItemId,
    /// Outgoing links, backlinks, or both.
    pub direction: LinkDirection,
    /// Bounded page.
    pub page: Page,
}

/// A bounded graph projection of one indexed revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphQuery {
    /// Revision to read.
    pub revision: Revision,
    /// Limit the graph to this folder and the folders beneath it.
    pub folder: Option<WorkspacePath>,
    /// Node limit; a result cut at this limit says so.
    pub max_nodes: u32,
}

/// Full-text, link, and graph queries; storage implements them with SQLite FTS.
///
/// The application authorizes the caller, resolves the revision once, and supplies the
/// already-scoped workspace. Implementations never widen scope and never resolve a revision.
pub trait SearchIndex: Send + Sync {
    /// Index the committed content of one revision of a workspace; indexing it again is a no-op.
    fn index_revision<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: Revision,
    ) -> PortFuture<'a, ()>;
    /// Query one indexed revision, returning `Unavailable` if that revision is not indexed.
    fn search<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: SearchQuery,
    ) -> PortFuture<'a, SearchResponse>;
    /// Read outgoing links or backlinks recorded for one indexed revision.
    fn links<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: LinkQuery,
    ) -> PortFuture<'a, GetLinksResponse>;
    /// Read a bounded graph projection from one indexed revision.
    fn graph<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: GraphQuery,
    ) -> PortFuture<'a, GetGraphResponse>;
    /// Discard and recreate this workspace's derived index data from retained content at `head`.
    fn rebuild<'a>(&'a self, scope: &'a StorageScope, head: Revision) -> PortFuture<'a, ()>;
}
