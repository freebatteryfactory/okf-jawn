//! MCP transport helpers preserve generated tool schemas and application-level content.
//!
//! This is not a JSON-RPC implementation. `rmcp` owns protocol and transport behavior.
//! A successful result always carries text a host can show without reading
//! `structuredContent`: the document Markdown for a read, the serialized response otherwise.
//! A failed call is reported as text with `isError` set and no structured content: a tool's
//! declared output schema describes its success response only.

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::read::ReadItemResponse;
use rmcp::model::{CallToolResult, ContentBlock, Tool};
use serde_json::Value;

/// Largest text block a helper writes; equal to the largest `max_bytes` a read may request.
pub const MAX_TEXT_BYTES: usize = 1_048_576;

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
///
/// # Errors
/// Returns an error if the response cannot be serialized.
pub fn read_result(
    response: &ReadItemResponse,
    max_text_bytes: usize,
) -> Result<CallToolResult, serde_json::Error> {
    let structured = serde_json::to_value(response)?;
    let mut result = CallToolResult::success(vec![ContentBlock::text(read_text(
        response,
        max_text_bytes,
    ))]);
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
