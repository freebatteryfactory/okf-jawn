//! Revision-bound progressive reading for people and multimodal agents.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Select a representation rather than silently truncating a source.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadView {
    /// Headings, pages, and media descriptions.
    Outline,
    /// Structured Markdown and source locators.
    Text,
    /// Markdown and selected images.
    Multimodal,
    /// Selected rendered page images.
    Pages,
    /// Authorized access to original bytes.
    Original,
}

/// Structural element types an outline can point to.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutlineEntryKind {
    /// A document heading.
    Heading,
    /// A table.
    Table,
    /// A figure or image.
    Figure,
    /// A document page.
    Page,
    /// A spreadsheet sheet.
    Sheet,
}

/// What a retained image is.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssetRole {
    /// A render of one whole page (docling page image).
    PageImage,
    /// A picture item's own image (docling `PictureItem` image).
    Picture,
}

/// Explicit bounded source selection.
///
/// `PartialEq` only: a `region` holds coordinates as `f64`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selection {
    /// The complete source, subject to a visible response budget.
    All,
    /// Inclusive line range.
    Lines {
        #[doc = "Range."]
        range: crate::common::TextRange,
    },
    /// Inclusive page range.
    Pages {
        #[doc = "Range."]
        range: crate::common::PageRange,
    },
    /// Rectangular sheet range.
    Cells {
        #[doc = "Range."]
        range: crate::common::CellRange,
    },
    /// An unambiguous heading identifier.
    Section {
        #[doc = "Heading."]
        heading: String,
    },
    /// A box on one page: the extracted items whose location lies inside it; the `pages` view
    /// returns that page's image.
    Region {
        #[doc = "Region."]
        region: crate::source::PageRegion,
    },
}

/// Read an item using an explicit representation and a single revision resolution.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadItemRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Requested representation.
    pub view: ReadView,
    /// Section, range, or full source.
    pub selection: Selection,
    /// Response text budget; partial reads are marked and have continuation.
    #[schemars(range(min = 256, max = 1_048_576))]
    pub max_bytes: u32,
    /// Maximum selected images.
    #[schemars(range(max = 16))]
    pub max_images: u16,
    /// Opaque continuation tied to revision and representation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// A navigable structural element from a document.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OutlineEntry {
    /// Visible heading or media caption.
    pub label: String,
    /// Heading depth or zero for non-heading elements.
    pub level: u16,
    /// Precise location to request next. A heading entry selects its section as lines: from the
    /// heading line to the line before the next heading of the same or a higher level, or to
    /// the end of the text.
    pub selection: Selection,
    /// Structural element type.
    pub kind: OutlineEntryKind,
}

/// Request a short-lived sandbox-origin URL for one hostile representation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSandboxCapabilityRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Exact resolved revision whose representation is served.
    pub revision: crate::identity::Revision,
    /// Original or derived object served; it must belong to this item revision.
    pub object: crate::identity::Digest,
}

/// A capability URL on the sandbox origin; the token is the only credential and is never logged.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SandboxCapability {
    /// Absolute `/sandbox/{capability}` URL on the configured sandbox origin.
    pub url: String,
    /// Expiry after which the capability resolves nothing.
    pub expires_at: crate::identity::Timestamp,
}

/// An authorized image or artifact associated with a source range.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaReference {
    /// Retained image bytes.
    pub object: crate::identity::Digest,
    /// MIME type.
    pub media_type: String,
    /// What the image is.
    pub role: AssetRole,
    /// Original source location: a page image's location is `direct` `page`; a picture's is
    /// its region.
    pub source: crate::source::SourceReference,
    /// The document's own caption or alternative text; absent when the document gives none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
}

/// What the tool actually returned, not a claim about the host model context.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadItemResponse {
    /// Exact source resolution.
    pub source: crate::source::SourceReference,
    /// Representation returned.
    pub view: ReadView,
    /// Text within the declared budget.
    pub markdown: String,
    /// Structural navigation.
    pub outline: Vec<OutlineEntry>,
    /// Selected images or original-file references.
    pub media: Vec<MediaReference>,
    /// Extraction or selection warnings.
    pub warnings: Vec<crate::common::Warning>,
    /// True only when the response is partial.
    pub truncated: bool,
    /// Resume this exact resolved read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Durable record of returned content.
    pub receipt_id: crate::identity::ReceiptId,
    /// For a source: whether its text is partial, supplied by an agent or corrected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction: Option<crate::extraction::ExtractionSummary>,
}
