//! The production MCP server over the application, and the result helpers it answers through.
//!
//! This is not a JSON-RPC implementation. `rmcp` owns protocol and transport behavior;
//! `server` is the `ServerHandler` over core's dispatch, `http` the Streamable HTTP service the
//! server crate mounts, and `catalog` the committed tool and App catalogs.
//! A successful result always carries text a host can show without reading
//! `structuredContent`: the document Markdown for a read, the serialized response otherwise.
//! A read's selected images follow its text as image blocks; an image the server did not
//! inline is named in the text with its source location, its digest and the reason. A read's
//! whole text block, its notes included, stays within the read's text budget.
//! A failed call is reported as text with `isError` set and no structured content: a tool's
//! declared output schema describes its success response only.

use std::fmt::{Display, Formatter};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::read::{AssetRole, MediaReference, ReadItemResponse};
use okf_jawn_contract::source::{SourceLocation, SourceLocator};
use rmcp::model::{CallToolResult, ContentBlock, Tool};
use serde_json::Value;

pub use catalog::{APP_MIME_TYPE, APP_URI, operation_of_tool};
pub use http::{
    DEFAULT_MAX_REQUEST_BODY_BYTES, McpConfig, McpHttpService, streamable_http_service,
};
pub use server::{McpPorts, McpServer};

/// What became of one image a read selected (an entry of `ReadItemResponse::media`).
#[derive(Debug, Clone)]
pub enum ReadImage<'a> {
    /// Inlined: an image block after the text.
    Inline {
        /// The image's bytes in base64, exactly as `read_object` returned them.
        data_base64: String,
        /// The raster MIME type the bytes were served with, one of the inline image types.
        mime_type: String,
    },
    /// Not inlined: one line of the text block names the image and gives the reason.
    NotInlined {
        /// The image, as the read selected it.
        media: &'a MediaReference,
        /// Why it is not inlined.
        reason: NotInlined,
    },
}

/// Why a selected image is described in the text instead of inlined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotInlined {
    /// Its media type, as the read declared it or as its bytes were served, is not one of the
    /// raster types inlined (PNG, JPEG, GIF, WebP): SVG and any other type are never inlined.
    NotInlineType {
        /// The media type.
        media_type: String,
    },
    /// The whole image does not fit what is left of the image budget.
    TooLarge {
        /// The whole object's size in bytes as `read_object` reported it; `None` when the
        /// budget was spent before the image was fetched.
        size: Option<String>,
        /// Bytes of the image budget left when the image was reached.
        left: usize,
    },
    /// `read_object` refused the fetch.
    Refused {
        /// The refusal's `ApiError` code.
        code: ErrorCode,
    },
}

/// Largest text block a helper writes; equal to the largest `max_bytes` a read may request.
pub const MAX_TEXT_BYTES: usize = 1_048_576;

/// Most bytes of a document's caption an image note shows; a longer caption is cut on a
/// character boundary and marked with `…`. Captions are document text with no length bound.
pub const MAX_CAPTION_BYTES: usize = 120;

/// The note after Markdown that was cut to fit the text budget.
const CUT_NOTE: &str =
    "\n\n[Text cut to the byte budget; the full response is in structuredContent.]";

impl Display for NotInlined {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInlineType { media_type } => {
                write!(f, "not an inline image type ({media_type})")
            }
            Self::TooLarge {
                size: Some(size),
                left,
            } => write!(
                f,
                "too large ({size} bytes; {left} bytes of the image budget left)"
            ),
            Self::TooLarge { size: None, .. } => f.write_str("too large (no image budget left)"),
            Self::Refused { code } => write!(f, "the fetch was refused ({})", code_name(code)),
        }
    }
}

/// Decode the generated model-tool catalog using the actual SDK's wire types.
///
/// # Errors
/// Returns an error when the generated catalog is missing or incompatible with `rmcp`.
pub fn decode_tools(document: &Value) -> Result<Vec<Tool>, serde_json::Error> {
    serde_json::from_value(document.get("tools").cloned().unwrap_or(Value::Null))
}

/// Wrap a read response: its Markdown is the text, the whole response is the structured content.
///
/// The whole text block stays within `max_text_bytes` (`read_text`): the Markdown, cut at a
/// character boundary when it does not fit, then bracketed notes: that the text was cut here,
/// that the read itself is partial (with its continuation cursor), and one line for each of
/// `images` that is not inlined, naming it (its role and caption, its source location and its
/// digest) and the reason. Each inlined image becomes an image block after the text, in order.
/// The structured content is the response, unchanged, whatever became of its images; with no
/// images the result is text alone.
///
/// # Errors
/// Returns an error if the response cannot be serialized.
pub fn read_result(
    response: &ReadItemResponse,
    max_text_bytes: usize,
    images: &[ReadImage<'_>],
) -> Result<CallToolResult, serde_json::Error> {
    let structured = serde_json::to_value(response)?;
    let mut blocks = Vec::new();
    let mut notes = Vec::new();
    for image in images {
        match image {
            ReadImage::Inline {
                data_base64,
                mime_type,
            } => blocks.push(ContentBlock::image(data_base64.clone(), mime_type.clone())),
            ReadImage::NotInlined { media, reason } => notes.push(image_note(media, reason)),
        }
    }
    let text = read_text(response, max_text_bytes, &notes);
    let mut content = vec![ContentBlock::text(text)];
    content.extend(blocks);
    let mut result = CallToolResult::success(content);
    result.structured_content = Some(structured);
    Ok(result)
}

/// Wrap any other response: its serialized JSON is the text, the value is the structured content.
///
/// JSON is never cut: a response larger than `max_text_bytes` gets a note that points at
/// `structuredContent` instead of an unparseable prefix.
#[must_use]
pub fn structured_result(value: &Value, max_text_bytes: usize) -> CallToolResult {
    let serialized = value.to_string();
    let text = if serialized.len() <= max_text_bytes {
        serialized
    } else {
        format!(
            "[The response is {} bytes of JSON, over the {max_text_bytes}-byte text budget; \
             read structuredContent or request a smaller page.]",
            serialized.len()
        )
    };
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = Some(value.clone());
    result
}

/// Report a failed tool call as text the caller can read and parse.
///
/// The text block is the serialized `ApiError`. `structured_content` stays empty, because an
/// error object can never satisfy the tool's declared output schema.
///
/// # Errors
/// Returns an error if the `ApiError` cannot be serialized.
pub fn error_result(error: &ApiError) -> Result<CallToolResult, serde_json::Error> {
    Ok(CallToolResult::error(vec![ContentBlock::text(
        serde_json::to_string(error)?,
    )]))
}

/// The text block of a read, never longer than `max_text_bytes`: the Markdown, then the notes a
/// text-only host needs (`image_notes` are the lines of the images that were not inlined).
///
/// Room for the notes is reserved before the Markdown is cut, and a note that does not fit is
/// left out: the cut note takes its room first, then the partial-read note, then the image
/// lines (`image_lines`), and the Markdown keeps what is left. The cut note appears only when
/// the Markdown is in fact cut.
fn read_text(response: &ReadItemResponse, max_text_bytes: usize, image_notes: &[String]) -> String {
    let markdown = response.markdown.as_str();
    let partial = partial_note(response);
    // First as if the Markdown were whole: the notes take their room, the Markdown the rest.
    let mut room = max_text_bytes;
    let partial_kept = reserve(&mut room, &partial);
    let images = image_lines(image_notes, room);
    if markdown.len().saturating_add(images.len()) <= room {
        return format!("{markdown}{partial_kept}{images}");
    }
    // The Markdown does not fit beside the notes, so the cut note is reserved as well.
    let mut room = max_text_bytes;
    let cut = reserve(&mut room, CUT_NOTE);
    let partial_kept = reserve(&mut room, &partial);
    let images = image_lines(image_notes, room);
    let room = room.saturating_sub(images.len());
    match within_budget(markdown, room) {
        (kept, true) => format!("{kept}{cut}{partial_kept}{images}"),
        (whole, false) => format!("{whole}{partial_kept}{images}"),
    }
}

/// The note of a partial read, with its continuation cursor when it has one; empty otherwise.
fn partial_note(response: &ReadItemResponse) -> String {
    match (&response.next_cursor, response.truncated) {
        (_, false) => String::new(),
        (Some(cursor), true) => format!("\n\n[Partial read; continue with cursor {cursor}]"),
        (None, true) => "\n\n[Partial read.]".to_owned(),
    }
}

/// `note` when it fits what is left of `room`, which it then takes; otherwise nothing.
fn reserve<'note>(room: &mut usize, note: &'note str) -> &'note str {
    if note.len() <= *room {
        *room = room.saturating_sub(note.len());
        note
    } else {
        ""
    }
}

/// The lines of the images that were not inlined, within `room`: as many of `notes` as fit, in
/// order, then one line counting the rest; nothing when not even that line fits. A note is
/// shown whole or not at all, so a shown note always keeps its digest and its reason.
fn image_lines(notes: &[String], room: usize) -> String {
    (0..=notes.len())
        .rev()
        .map(|shown| {
            let hidden = notes.len().saturating_sub(shown);
            let mut lines: Vec<String> = notes.iter().take(shown).cloned().collect();
            if hidden > 0 {
                lines.push(format!(
                    "[{hidden} more images not shown; see structuredContent.media]"
                ));
            }
            if lines.is_empty() {
                String::new()
            } else {
                format!("\n\n{}", lines.join("\n"))
            }
        })
        .find(|lines| lines.len() <= room)
        .unwrap_or_default()
}

/// The longest prefix of `text` that fits `max_bytes` without splitting a character.
fn within_budget(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_owned(), false);
    }
    let kept: String = text
        .char_indices()
        .take_while(|(start, character)| start.saturating_add(character.len_utf8()) <= max_bytes)
        .map(|(_, character)| character)
        .collect();
    (kept, true)
}

/// The text line of an image that is not inlined: what it is, where it is in the original,
/// its digest, and why. A caption is folded onto the one line and cut to `MAX_CAPTION_BYTES`.
fn image_note(media: &MediaReference, reason: &NotInlined) -> String {
    let role = match media.role {
        AssetRole::PageImage => "page image",
        AssetRole::Picture => "picture",
    };
    let caption = media
        .caption
        .as_deref()
        .map(|caption| {
            let folded = caption.split_whitespace().collect::<Vec<_>>().join(" ");
            let (mut shown, cut) = within_budget(&folded, MAX_CAPTION_BYTES);
            if cut {
                shown.push('…');
            }
            format!(" \"{shown}\"")
        })
        .unwrap_or_default();
    let location = if media.source.locations.is_empty() {
        "no recorded location".to_owned()
    } else {
        media
            .source
            .locations
            .iter()
            .map(location_text)
            .collect::<Vec<_>>()
            .join("; ")
    };
    format!(
        "[Image not inlined: {role}{caption}, {location}, object {}: {reason}]",
        media.object.as_str()
    )
}

/// Where an image lies in the original, in words; the exact box is in `structuredContent`.
fn location_text(location: &SourceLocation) -> String {
    let locator = |locator: &SourceLocator| match locator {
        SourceLocator::Page { page_no } => format!("page {page_no}"),
        SourceLocator::Region { region } => format!("a region of page {}", region.page_no),
        SourceLocator::Cells { range } => format!("cells of sheet \"{}\"", range.sheet),
    };
    match location {
        SourceLocation::Direct { locator: at } => locator(at),
        SourceLocation::Inferred { locator: at } => format!("{} (inferred)", locator(at)),
        SourceLocation::Unresolved { .. } => "location unresolved".to_owned(),
    }
}

/// The wire spelling of an error code, as an `ApiError` serializes it.
fn code_name(code: &ErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{code:?}"))
}

mod catalog;
mod http;
mod server;
