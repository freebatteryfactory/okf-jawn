//! MCP transport helpers preserve generated tool schemas and application-level content.
//!
//! This is not a JSON-RPC implementation. `rmcp` owns protocol and transport behavior.
//! A failed call is reported as text with `isError` set and no structured content: a tool's
//! declared output schema describes its success response only.

use okf_jawn_contract::error::ApiError;
use rmcp::model::{CallToolResult, ContentBlock, Tool};
use serde_json::Value;

/// Decode the generated model-tool catalog using the actual SDK's wire types.
///
/// # Errors
/// Returns an error when the generated catalog is missing or incompatible with `rmcp`.
pub fn decode_tools(document: &Value) -> Result<Vec<Tool>, serde_json::Error> {
    serde_json::from_value(document.get("tools").cloned().unwrap_or(Value::Null))
}

/// Wrap an application response as structured data with a useful text fallback.
///
/// # Errors
/// Returns an error if serialization or SDK validation fails.
pub fn structured_result(
    value: &Value,
    summary: &str,
) -> Result<CallToolResult, serde_json::Error> {
    let mut result = CallToolResult::success(vec![ContentBlock::text(summary.to_owned())]);
    result.structured_content = Some(value.clone());
    Ok(result)
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
