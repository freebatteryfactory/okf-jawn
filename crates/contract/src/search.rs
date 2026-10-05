//! Search and link navigation over retained, authorized workspace content.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Search the workspace at one revision with explicit scope and pagination.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Search expression; not executable SQL.
    pub query: String,
    /// Limit to a relative folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Include historical lifecycle items.
    pub include_archived: bool,
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// A bounded excerpt linked to its exact source.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchHit {
    /// Reopenable citation.
    pub source: crate::source::SourceReference,
    /// Item title.
    pub title: String,
    /// Matching text.
    pub snippet: String,
    /// Ranking signal, never a truth or trust score.
    pub score: f64,
}

/// A paged authorized search result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchResponse {
    /// Resolved revision.
    pub revision: crate::identity::Revision,
    /// Matching excerpts.
    pub hits: Vec<SearchHit>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Which side of the link graph to inspect.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LinkDirection {
    /// References made by this item.
    Outgoing,
    /// References pointing to this item.
    Incoming,
    /// Both directions.
    Both,
}

/// A source-level relationship between items.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Link {
    /// Linking item.
    pub from: crate::identity::ItemId,
    /// Target path as written.
    pub to_path: String,
    /// Resolved target, absent for broken links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<crate::identity::ItemId>,
    /// Link label.
    pub label: String,
    /// Source line.
    pub line: u32,
}

/// Read outgoing links or backlinks without loading every document.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetLinksRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Requested direction.
    pub direction: LinkDirection,
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// Links and their resolution at one revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetLinksResponse {
    /// Resolved revision.
    pub revision: crate::identity::Revision,
    /// Bounded edges.
    pub links: Vec<Link>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Read a bounded link graph for the human graph projection.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetGraphRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Relative graph scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Node limit; truncation is explicit.
    pub max_nodes: u32,
}

/// Graph nodes and edges from the same authorized revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetGraphResponse {
    /// Resolved revision.
    pub revision: crate::identity::Revision,
    /// Authorized items.
    pub nodes: Vec<crate::item::ItemSummary>,
    /// Known links among selected items.
    pub edges: Vec<Link>,
    /// The graph is partial due to budget.
    pub truncated: bool,
}
