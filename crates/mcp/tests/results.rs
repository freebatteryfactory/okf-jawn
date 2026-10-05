//! Tool results checked against the generated catalog in `api/mcp-tools.json`.

use std::error::Error;
use std::path::Path;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_mcp::{decode_tools, error_result};
use rmcp::model::CallToolResult;
use serde_json::Value;

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
