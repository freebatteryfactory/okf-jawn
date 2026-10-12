//! The production MCP server over the application, and the result helpers it answers through.
//!
//! This is not a JSON-RPC implementation. `rmcp` owns protocol and transport behavior;
//! `server` is the `ServerHandler` over core's dispatch, `http` the Streamable HTTP service the
//! server crate mounts, and `catalog` the committed tool and App catalogs.
//! A successful result always carries text a host can show without reading
//! `structuredContent`: the document Markdown for a read, the serialized response otherwise.
//! A read's selected images follow its text as image blocks; an image the server did not
//! inline is named in the text with its source location, its digest and the reason.
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
        /// The `image/*` MIME type the bytes were served with.
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
    /// Its media type, as the read declared it or as its bytes were served, is not `image/*`.
    NotAnImage {
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

impl Display for NotInlined {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnImage { media_type } => write!(f, "not an image type ({media_type})"),
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
/// The text is cut at a character boundary to `max_text_bytes`. A bracketed note follows when
/// the text was cut here, and when the read itself is partial and has a continuation cursor.
/// Then each of `images` in order: an inlined image becomes an image block after the text, and
/// an image that is not inlined becomes one line of the text naming it (its caption and role,
/// its source location and its digest) and the reason. The structured content is the response,
/// unchanged, whatever became of its images; with no images the result is text alone.
///
/// # Errors
/// Returns an error if the response cannot be serialized.
pub fn read_result(
    response: &ReadItemResponse,
    max_text_bytes: usize,
    images: &[ReadImage<'_>],
) -> Result<CallToolResult, serde_json::Error> {
    let structured = serde_json::to_value(response)?;
    let mut text = read_text(response, max_text_bytes);
    let mut blocks = Vec::new();
    let mut described = 0_usize;
    for image in images {
        match image {
            ReadImage::Inline {
                data_base64,
                mime_type,
            } => blocks.push(ContentBlock::image(data_base64.clone(), mime_type.clone())),
            ReadImage::NotInlined { media, reason } => {
                text.push_str(if described == 0 { "\n\n" } else { "\n" });
                text.push_str(&image_note(media, reason));
                described = described.saturating_add(1);
            }
        }
    }
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

/// The text block of a read: budgeted Markdown plus the notes a text-only host needs.
fn read_text(response: &ReadItemResponse, max_text_bytes: usize) -> String {
    let (mut text, cut) = within_budget(&response.markdown, max_text_bytes);
    if cut {
        text.push_str(
            "\n\n[Text cut to the byte budget; the full response is in structuredContent.]",
        );
    }
    if response.truncated {
        match &response.next_cursor {
            Some(cursor) => {
                text.push_str("\n\n[Partial read; continue with cursor ");
                text.push_str(cursor);
                text.push(']');
            }
            None => text.push_str("\n\n[Partial read.]"),
        }
    }
    text
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
/// its digest, and why. A caption is folded onto the one line.
fn image_note(media: &MediaReference, reason: &NotInlined) -> String {
    let role = match media.role {
        AssetRole::PageImage => "page image",
        AssetRole::Picture => "picture",
    };
    let caption = media
        .caption
        .as_deref()
        .map(|caption| {
            format!(
                " \"{}\"",
                caption.split_whitespace().collect::<Vec<_>>().join(" ")
            )
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
