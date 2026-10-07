//! Tool results checked against the generated catalog in `api/mcp-tools.json`.

use std::error::Error;
use std::path::Path;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::read::ReadItemResponse;
use okf_jawn_mcp::{MAX_TEXT_BYTES, decode_tools, error_result, read_result, structured_result};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

fn catalog() -> Result<Value, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/mcp-tools.json");
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn declared_tools(document: &Value) -> Result<&Vec<Value>, Box<dyn Error>> {
    Ok(document
        .get("tools")
        .and_then(Value::as_array)
        .ok_or("api/mcp-tools.json has no `tools` array")?)
}

fn text_of(result: &CallToolResult) -> Result<&str, Box<dyn Error>> {
    let block = result
        .content
        .first()
        .ok_or("the result has no content block")?;
    let text = block.as_text().ok_or("the first block is not text")?;
    Ok(text.text.as_str())
}

fn read_response(
    markdown: &str,
    truncated: bool,
    next_cursor: Option<&str>,
) -> Result<ReadItemResponse, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "source": {
            "workspace_id": "11111111-1111-4111-8111-111111111111",
            "item_id": "22222222-2222-4222-8222-222222222222",
            "path": "notes/plan.md",
            "revision": "a".repeat(40),
            "selection": {"kind": "all"}
        },
        "view": "text",
        "markdown": markdown,
        "outline": [],
        "media": [],
        "warnings": [],
        "truncated": truncated,
        "next_cursor": next_cursor,
        "receipt_id": "33333333-3333-4333-8333-333333333333"
    }))?)
}

#[test]
fn catalog_decodes_every_generated_tool() -> TestResult {
    let document = catalog()?;
    let declared = declared_tools(&document)?;
    let tools = decode_tools(&document)?;
    assert_ne!(tools.len(), 0, "the catalog declares no tools");
    assert_eq!(tools.len(), declared.len());
    for tool in &tools {
        assert!(
            tool.output_schema.is_some(),
            "{} declares no output schema",
            tool.name
        );
    }
    Ok(())
}

#[test]
fn no_generated_output_schema_admits_an_error_object() -> TestResult {
    let document = catalog()?;
    for tool in declared_tools(&document)? {
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .ok_or("tool without a name")?;
        let schema = tool
            .get("outputSchema")
            .ok_or("tool without an outputSchema")?;
        assert_eq!(
            schema.get("additionalProperties"),
            Some(&Value::Bool(false)),
            "{name} would accept an `error` property"
        );
        let properties = schema
            .get("properties")
            .and_then(Value::as_object)
            .ok_or("outputSchema without properties")?;
        assert!(
            !properties.contains_key("error"),
            "{name} declares an `error` property"
        );
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .ok_or("outputSchema without required")?;
        assert!(!required.is_empty(), "{name} requires nothing");
    }
    Ok(())
}

#[test]
fn error_result_is_text_with_is_error_and_no_structured_content() -> TestResult {
    let error = ApiError::new(ErrorCode::NotFound, "No such item");
    let result = error_result(&error)?;
    assert_eq!(result.is_error, Some(true));
    assert!(result.structured_content.is_none());
    let parsed: ApiError = serde_json::from_str(text_of(&result)?)?;
    assert_eq!(parsed.code, ErrorCode::NotFound);
    assert_eq!(parsed.message, "No such item");
    Ok(())
}

#[test]
fn read_result_text_is_the_document_markdown() -> TestResult {
    let response = read_response(
        "# Plan

Ship the cure.
",
        false,
        None,
    )?;
    let result = read_result(&response, MAX_TEXT_BYTES)?;
    assert_eq!(
        text_of(&result)?,
        "# Plan

Ship the cure.
"
    );
    assert_eq!(result.is_error, Some(false));
    assert_eq!(
        result.structured_content,
        Some(serde_json::to_value(&response)?)
    );
    Ok(())
}

#[test]
fn read_result_cuts_text_at_a_character_boundary_and_says_so() -> TestResult {
    let markdown = "é".repeat(10);
    let response = read_response(&markdown, false, None)?;
    let result = read_result(&response, 5)?;
    let text = text_of(&result)?;
    assert!(
        text.starts_with(
            "éé

[Text cut"
        ),
        "{text}"
    );
    let structured = result
        .structured_content
        .as_ref()
        .ok_or("no structured content")?;
    assert_eq!(
        structured.get("markdown").and_then(Value::as_str),
        Some(markdown.as_str())
    );
    Ok(())
}

#[test]
fn read_result_reports_a_partial_read_with_its_cursor() -> TestResult {
    let response = read_response("First page.", true, Some("cursor-2"))?;
    let result = read_result(&response, MAX_TEXT_BYTES)?;
    assert_eq!(
        text_of(&result)?,
        "First page.

[Partial read; continue with cursor cursor-2]"
    );
    Ok(())
}

#[test]
fn structured_result_text_is_the_serialized_response() -> TestResult {
    let value = json!({"revision": "a".repeat(40), "items": [], "folders": ["notes"]});
    let result = structured_result(&value, MAX_TEXT_BYTES);
    let parsed: Value = serde_json::from_str(text_of(&result)?)?;
    assert_eq!(parsed, value);
    assert_eq!(result.structured_content, Some(value));
    assert_eq!(result.is_error, Some(false));
    Ok(())
}

#[test]
fn structured_result_never_emits_cut_json() -> TestResult {
    let value = json!({"revision": "a".repeat(40), "items": [], "folders": ["notes"]});
    let result = structured_result(&value, 8);
    let text = text_of(&result)?;
    assert!(serde_json::from_str::<Value>(text).is_err(), "{text}");
    assert!(text.contains("structuredContent"), "{text}");
    assert_eq!(result.structured_content, Some(value));
    Ok(())
}
