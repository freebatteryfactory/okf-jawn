//! Content-addressed bytes and occurrence-specific provenance.
//!
//! A source location says how it was obtained: an inferred location is a different variant
//! from a direct one, so no consumer can present it as direct by accident, and an unresolved
//! location carries a reason, never a guessed box.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Docling's `coord_origin`: which corner `t` and `b` are measured from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoordOrigin {
    /// Docling `TOPLEFT`.
    TopLeft,
    /// Docling `BOTTOMLEFT`.
    BottomLeft,
}

/// Docling's bounding box, copied as the converter gave it (rounded to two decimals).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoundingBox {
    /// Left edge.
    pub l: f64,
    /// Top edge.
    pub t: f64,
    /// Right edge.
    pub r: f64,
    /// Bottom edge.
    pub b: f64,
    /// Corner `t` and `b` are measured from.
    pub coord_origin: CoordOrigin,
}

/// Extent of the page the box lies on, in the box's own unit (PDF points, image pixels, PPTX
/// EMU). A reader draws the box over a page image by the ratio of the two, so it needs no unit.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PageSize {
    /// Page width.
    pub width: f64,
    /// Page height.
    pub height: f64,
}

/// A box on one page, slide or image of the original.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PageRegion {
    /// One-based page or slide number.
    #[schemars(range(min = 1))]
    pub page_no: u32,
    /// The box.
    pub bbox: BoundingBox,
    /// The page's extent, so the box can be drawn over a page image on its own.
    pub page_size: PageSize,
}

/// Where in the original bytes something is.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceLocator {
    /// A whole page or slide.
    Page {
        /// One-based page or slide number.
        #[schemars(range(min = 1))]
        page_no: u32,
    },
    /// A box on a page, slide or image.
    Region {
        /// The box.
        region: PageRegion,
    },
    /// Spreadsheet cells.
    Cells {
        /// Sheet and one-based inclusive cell range.
        range: crate::common::CellRange,
    },
}

/// A location and how it was obtained; an inferred location is a different variant from a
/// direct one, so no consumer can show it as direct by accident.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "provenance", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceLocation {
    /// From the converter's own provenance.
    Direct {
        /// Where.
        locator: SourceLocator,
    },
    /// Found by matching text against the page text layer (ingest-locates-unlocated-items).
    Inferred {
        /// Where.
        locator: SourceLocator,
    },
    /// Not known; never given a guessed box.
    Unresolved {
        /// Why.
        reason: UnresolvedReason,
    },
}

/// Why a location is not known; one variant per reason the producing rule states.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UnresolvedReason {
    /// The format has no page geometry (DOCX, HTML, Markdown).
    FormatHasNoLocator,
    /// The converter gave this item no location and no rule locates it (PPTX, XLSX).
    NotLocatedByConverter,
    /// The page text layer could not be read.
    NoTextLayer,
    /// A table or picture: there is no text to look up.
    NoTextToMatch,
    /// Neither the parent nor an earlier sibling is located.
    NoPageToSearch,
    /// No text-layer item on the searched pages has exactly this text.
    NoMatch,
    /// Several text-layer items have exactly this text.
    AmbiguousMatch {
        /// How many.
        occurrences: u32,
    },
    /// The one match has no area or lies outside the page.
    MatchNotALocation,
    /// The cited text is a human correction, which has no place in the original.
    CorrectedText,
    /// The cited text was supplied by an agent; its page claims are not locations.
    SuppliedText,
}

/// An observed source name and location; not an authority assertion.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceName {
    /// Original supplied filename.
    pub filename: String,
    /// Original relative location.
    pub folder: String,
    /// Observation time.
    pub observed_at: crate::identity::Timestamp,
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
    /// Everything recorded about turning these bytes into text, converter identity included.
    pub extraction: crate::extraction::Extraction,
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
    /// Resolved page, lines, cells, region, section, or entire source.
    pub selection: crate::read::Selection,
    /// Where the cited selection of the digest lies in the original, one entry per located
    /// item it covers, in reading order.
    ///
    /// The server computes it from (item, revision, digest, selection). A response always
    /// fills it. In a request an omitted list means "fill it"; a present list must equal what
    /// the server computes, or the request is `invalid_input` on `.../locations`, so a caller
    /// never supplies a location. A saved View file does not keep locations: they are derived,
    /// and the server fills them when it returns the View.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<SourceLocation>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<SourceAppearance>,
    /// Entries of the item's OKF `sources` that are not citations of an item in this workspace
    /// at this revision, in the file's order; kept so no declared source is silently dropped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncited: Vec<UncitedSource>,
}

/// One OKF `sources` entry that is not a citation of an item at the resolved revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UncitedSource {
    /// The entry's `resource`, exactly as written in the file.
    pub resource: String,
    /// Why it is not a citation.
    pub reason: UncitedReason,
}

/// Why an OKF `sources` entry is not a citation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UncitedReason {
    /// A URL outside the workspace.
    External,
    /// A scope (a folder or a pattern) rather than one item.
    Scope,
    /// A workspace path that names no item at the resolved revision.
    NotFound,
    /// An entry with no `resource` (its `resource` here is empty).
    Malformed,
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
