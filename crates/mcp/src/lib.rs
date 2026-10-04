//! MCP transport helpers preserve generated tool schemas and application-level content.
//!
//! This is not a JSON-RPC implementation. `rmcp` owns protocol and transport behavior.

use rmcp::model::{CallToolResult, ContentBlock, Tool};
use serde_json::{Value, json};

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
pub fn structured_result(value: &Value, summary: &str) -> Result<CallToolResult, serde_json::Error> {
    let mut result = CallToolResult::success(vec![ContentBlock::text(summary.to_owned())]);
    result.structured_content = Some(value.clone());
    Ok(result)
}

/// Preserve explicit tool-execution failure rather than fabricating an empty successful result.
///
/// # Errors
/// Returns an SDK serialization error.
pub fn error_result(error: &okf_jawn_contract::error::ApiError) -> Result<CallToolResult, serde_json::Error> {
    let mut result = CallToolResult::error(vec![ContentBlock::text(error.message.clone())]);
    result.structured_content = Some(json!({"error":error}));
    Ok(result)
}
