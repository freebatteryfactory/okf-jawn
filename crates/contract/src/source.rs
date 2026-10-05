//! Content-addressed bytes and occurrence-specific provenance.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An observed source name and location; not an authority assertion.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceName {
    /// Original supplied filename.
    pub filename: String,
    /// Original relative location.
    pub folder: String,
    /// RFC 3339 observation time.
    pub observed_at: String,
    /// Server-recorded source identity.
    pub supplied_by: String,
}

/// One source occurrence; identical bytes do not imply identical meaning.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceAppearance {
    /// Immutable object content hash.
    pub object: crate::identity::Digest,
    /// Observed aliases associated with this occurrence.
    pub names: Vec<SourceName>,
    /// Sniffed MIME type.
    pub media_type: String,
    /// Decimal byte count.
    pub size: String,
    /// Extracted metadata with original field names preserved.
    pub metadata: std::collections::BTreeMap<String, serde_json::Value>,
    /// Container or message parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_item_id: Option<crate::identity::ItemId>,
    /// Explicit successor relationship; not inferred from arrival time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<crate::identity::ItemId>,
}

/// A precise citation that can be reopened independently of current state.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceReference {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Relative path at the resolved source revision.
    pub path: crate::identity::WorkspacePath,
    /// Resolved source revision.
    pub revision: crate::identity::Revision,
    /// Exact derived artifact hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<crate::identity::Digest>,
    /// Resolved page, lines, cells, section, or entire source.
    pub selection: crate::read::Selection,
}

/// Retrieve sources and provenance for one item.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetSourcesRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
}

/// Citations and source appearances for the selected item.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetSourcesResponse {
    /// Resolved version.
    pub revision: crate::identity::Revision,
    /// Supporting citations.
    pub sources: Vec<SourceReference>,
    /// Occurrence metadata if the item is a source.
    pub appearance: Option<SourceAppearance>,
}

/// Read an authorized stored object through an item reference, never by hash alone.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetObjectRequest {
    /// Authorized provenance path.
    pub source: SourceReference,
    /// Referenced object.
    pub object: crate::identity::Digest,
    /// Decimal byte offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<String>,
    /// Bounded requested byte length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u32>,
}

/// An authorized bounded binary block.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetObjectResponse {
    /// Whole-object digest.
    pub sha256: crate::identity::Digest,
    /// Actual byte offset.
    pub offset: String,
    /// Whole-object size.
    pub total_size: String,
    /// Content type.
    pub media_type: String,
    /// Bounded base64 block; large objects use the streaming route.
    pub data_base64: String,
    /// Whether more bytes remain.
    pub has_more: bool,
}
