//! Revision-bound progressive reading for people and multimodal agents.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Select a representation rather than silently truncating a source.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, PartialEq, Eq)]
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

/// Explicit bounded source selection.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, PartialEq, Eq)]
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
}

/// Read an item using an explicit representation and a single revision resolution.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
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
    #[schema(minimum = 256, maximum = 1_048_576)]
    pub max_bytes: u32,
    /// Maximum selected images.
    #[schemars(range(max = 16))]
    #[schema(maximum = 16)]
    pub max_images: u16,
    /// Opaque continuation tied to revision and representation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// A navigable structural element from a document.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OutlineEntry {
    /// Visible heading or media caption.
    pub label: String,
    /// Heading depth or zero for non-heading elements.
    pub level: u16,
    /// Precise location to request next.
    pub selection: Selection,
    /// Heading, table, figure, page, or sheet.
    pub kind: String,
}

/// An authorized image or artifact associated with a source range.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaReference {
    /// Retained image bytes.
    pub object: crate::identity::Digest,
    /// MIME type.
    pub media_type: String,
    /// Original source location.
    pub source: crate::source::SourceReference,
    /// Caption text; caption origin is separate.
    pub caption: String,
    /// source, process, human, or agent.
    pub caption_origin: String,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
}

/// What the tool actually returned, not a claim about the host model context.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
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
}
