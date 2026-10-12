//! Tool results checked against the generated catalog in `api/mcp-tools.json`.

use std::error::Error;
use std::path::Path;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::read::{MediaReference, ReadItemResponse};
use okf_jawn_mcp::{
    MAX_CAPTION_BYTES, MAX_TEXT_BYTES, NotInlined, ReadImage, decode_tools, error_result,
    read_result, structured_result,
};
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

/// A selected picture on page 1, object digest spelled with `digit`, with `caption`.
fn media_of(digit: char, caption: Option<&str>) -> Result<MediaReference, Box<dyn Error>> {
    let digest = digit.to_string().repeat(64);
    let mut media = json!({
        "object": digest,
        "media_type": "image/png",
        "role": "picture",
        "source": {
            "workspace_id": "11111111-1111-4111-8111-111111111111",
            "item_id": "22222222-2222-4222-8222-222222222222",
            "path": "reports/q3.pdf",
            "revision": "a".repeat(40),
            "digest": digest,
            "selection": {"kind": "all"},
            "locations": [{"provenance": "direct", "locator": {"kind": "page", "page_no": 1}}]
        },
        "width": 640,
        "height": 480
    });
    if let (Some(caption), Some(fields)) = (caption, media.as_object_mut()) {
        fields.insert("caption".to_owned(), json!(caption));
    }
    Ok(serde_json::from_value(media)?)
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
    let result = read_result(&response, MAX_TEXT_BYTES, &[])?;
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
    let markdown = "é".repeat(100);
    let response = read_response(&markdown, false, None)?;
    let result = read_result(&response, 100, &[])?;
    let text = text_of(&result)?;
    // The cut note's 75 bytes are reserved first, leaving 25: twelve two-byte characters.
    assert_eq!(
        text,
        format!(
            "{}\n\n[Text cut to the byte budget; the full response is in structuredContent.]",
            "é".repeat(12)
        )
    );
    assert!(text.len() <= 100, "{} bytes", text.len());
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
    let result = read_result(&response, MAX_TEXT_BYTES, &[])?;
    assert_eq!(
        text_of(&result)?,
        "First page.

[Partial read; continue with cursor cursor-2]"
    );
    Ok(())
}

#[test]
fn a_read_with_room_keeps_its_whole_text_and_every_image_note() -> TestResult {
    let response = read_response("# Plan\n\nShip the cure.\n", false, None)?;
    let (first, second) = (
        media_of('c', Some("Revenue  by\nquarter"))?,
        media_of('d', None)?,
    );
    let images = [
        ReadImage::NotInlined {
            media: &first,
            reason: NotInlined::Refused {
                code: ErrorCode::NotFound,
            },
        },
        ReadImage::NotInlined {
            media: &second,
            reason: NotInlined::NotInlineType {
                media_type: "image/svg+xml".to_owned(),
            },
        },
    ];
    let result = read_result(&response, MAX_TEXT_BYTES, &images)?;
    assert_eq!(
        text_of(&result)?,
        format!(
            "# Plan\n\nShip the cure.\n\n\n\
             [Image not inlined: picture \"Revenue by quarter\", page 1, object {}: \
             the fetch was refused (not_found)]\n\
             [Image not inlined: picture, page 1, object {}: \
             not an inline image type (image/svg+xml)]",
            "c".repeat(64),
            "d".repeat(64)
        )
    );
    assert_eq!(
        result.structured_content,
        Some(serde_json::to_value(&response)?)
    );
    Ok(())
}

/// A 64-byte budget holds neither a note with a huge caption nor the cut note: the Markdown
/// keeps what the summary line leaves, and the whole text stays within the budget.
#[test]
fn a_64_byte_budget_with_a_huge_caption_stays_within_budget() -> TestResult {
    let markdown = "Revenue grew in every region. ".repeat(10);
    let response = read_response(&markdown, false, None)?;
    let huge = media_of('c', Some(&"x".repeat(100_000)))?;
    let images = [ReadImage::NotInlined {
        media: &huge,
        reason: NotInlined::TooLarge {
            size: Some("9000".to_owned()),
            left: 64,
        },
    }];
    let result = read_result(&response, 64, &images)?;
    let text = text_of(&result)?;
    assert_eq!(
        text,
        "Revenue \n\n[1 more images not shown; see structuredContent.media]"
    );
    assert!(text.len() <= 64, "{} bytes", text.len());
    assert_eq!(
        result.structured_content,
        Some(serde_json::to_value(&response)?)
    );
    Ok(())
}

#[test]
fn image_notes_that_do_not_fit_end_in_one_summary_line() -> TestResult {
    let response = read_response("Short.", false, None)?;
    let selected = "0123456789abcdef"
        .chars()
        .map(|digit| media_of(digit, None))
        .collect::<Result<Vec<_>, _>>()?;
    let images: Vec<ReadImage<'_>> = selected
        .iter()
        .map(|media| ReadImage::NotInlined {
            media,
            reason: NotInlined::NotInlineType {
                media_type: "image/svg+xml".to_owned(),
            },
        })
        .collect();
    let result = read_result(&response, 1024, &images)?;
    let text = text_of(&result)?;
    assert!(text.len() <= 1024, "{} bytes", text.len());
    let (head, notes) = text.split_once("\n\n").ok_or("no notes")?;
    assert_eq!(head, "Short.");
    let lines: Vec<&str> = notes.lines().collect();
    let (summary, shown) = lines.split_last().ok_or("no lines")?;
    assert!(!shown.is_empty(), "every note was summarized: {text}");
    for (line, digit) in shown.iter().zip("0123456789abcdef".chars()) {
        assert!(
            line.contains(&format!("object {}:", digit.to_string().repeat(64))),
            "{line}"
        );
    }
    assert_eq!(
        *summary,
        format!(
            "[{} more images not shown; see structuredContent.media]",
            16_usize.saturating_sub(shown.len())
        )
    );
    Ok(())
}

/// A shown note's caption is cut to `MAX_CAPTION_BYTES` on a character boundary and marked;
/// the digest and the reason are kept. Uncut, this note would not fit the 4096-byte budget.
#[test]
fn a_long_caption_is_cut_and_marked_and_its_note_keeps_digest_and_reason() -> TestResult {
    let response = read_response("Short.", false, None)?;
    let caption = format!("a{}", "é".repeat(100_000));
    let long = media_of('c', Some(&caption))?;
    let images = [ReadImage::NotInlined {
        media: &long,
        reason: NotInlined::Refused {
            code: ErrorCode::Forbidden,
        },
    }];
    let result = read_result(&response, 4096, &images)?;
    let text = text_of(&result)?;
    let (_, note) = text.split_once("\n\n").ok_or("no note")?;
    let (shown, rest) = note
        .strip_prefix("[Image not inlined: picture \"")
        .and_then(|after| after.split_once("…\""))
        .ok_or_else(|| format!("no cut caption in {note}"))?;
    assert!(caption.starts_with(shown));
    assert!(shown.len() <= MAX_CAPTION_BYTES, "{} bytes", shown.len());
    assert!(
        shown.len() > MAX_CAPTION_BYTES.saturating_sub(2),
        "{} bytes",
        shown.len()
    );
    assert_eq!(
        rest,
        format!(
            ", page 1, object {}: the fetch was refused (forbidden)]",
            "c".repeat(64)
        )
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
