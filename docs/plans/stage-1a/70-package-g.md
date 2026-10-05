## Package G: MCP helpers

**Branch:** `cure/mcp` (created from `integration/foundation-cure` after package C is merged)
**Worktree:** `D:\okf\cure\mcp`
**Wave:** 2, in parallel with D, E and F.

**Files allowed (exact):**

- Modify `crates/mcp/src/lib.rs`
- Create `crates/mcp/tests/results.rs`
- Modify `crates/mcp/AGENTS.md`

**Must not touch:** anything outside `crates/mcp/`; `crates/mcp/Cargo.toml` (manifests and
`Cargo.lock` belong to the integration owner, and no new dependency is needed); `api/**`
(generated; read only).

**SPEC / AGENTS sentences served:**

- SPEC §9: "Return supported text/image/resource-link and structured content through SDK
  constructors." and "Keep tools usable with a textual fallback when a host cannot display an App."
- SPEC §7: "Return exact resolved citations, warnings and an explicit truncation indicator" and
  "Text-only clients retain usable descriptions and source references."
- SPEC §9: "Generate MCP schemas and annotations from the declared operation semantics."
- Design §3: "MCP text fallback | Tool `content` carries the document text within budget, not a summary."
- `crates/mcp/AGENTS.md`: "Use rmcp 3.5 constructors and selected protocol APIs."

**What is wrong at the base commit** (`crates/mcp/src/lib.rs`):

- `:20-27` `structured_result(value, summary)` puts a caller-written one-line `summary` in
  `content`. A host that ignores `structuredContent` gets no document text.
- `:33-39` `error_result` sets `structured_content = {"error": …}`. Every tool in
  `api/mcp-tools.json` declares an `outputSchema` with `additionalProperties: false` and its own
  `required` list, so that object violates the tool's declared output contract.

**rmcp 3.5.0 API relied on** (read in
`~/.cargo/registry/src/index.crates.io-*/rmcp-3.5.0/src/model.rs` and `model/content.rs`):

```rust
// model.rs — #[non_exhaustive]: build with a constructor, then assign public fields.
pub struct CallToolResult {
    pub result_type: Option<ResultType>,
    pub content: Vec<ContentBlock>,
    pub structured_content: Option<Value>,
    pub is_error: Option<bool>,
    pub meta: Option<MetaObject>,
}
impl CallToolResult {
    pub fn success(content: Vec<ContentBlock>) -> Self;   // structured_content: None, is_error: Some(false)
    pub fn error(content: Vec<ContentBlock>) -> Self;     // structured_content: None, is_error: Some(true)
    pub fn structured(value: Value) -> Self;              // content: [text(value.to_string())], is_error: Some(false)
    pub fn structured_error(value: Value) -> Self;        // NOT used: it fills structured_content on an error
}
// model/content.rs
impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self;
    pub fn as_text(&self) -> Option<&TextContent>;        // TextContent { pub text: String, .. }
}
// model/tool.rs — Deserialize, serde(rename_all = "camelCase")
pub struct Tool { pub name: Cow<'static, str>, pub input_schema: Arc<JsonObject>,
                  pub output_schema: Option<Arc<JsonObject>>, /* title, description, annotations, icons, meta */ }
```

**Test idiom note.** `tests/support/check.rs` is produced by package E in this same wave and does
not exist in this worktree. `crates/mcp/tests/results.rs` therefore declares
`type TestResult = Result<(), Box<dyn Error>>;` itself and uses only `?`, `assert!`, `assert_eq!`
and `.ok_or("…")?`. See Deviations.

**Lint note.** Without `--no-deps`, `cargo clippy -p okf-jawn-mcp` also lints the workspace
dependency `okf-jawn-core`, whose remaining lint errors are cured by packages E and F in
parallel. Inside this worktree use `--no-deps`; the orchestrator runs the plain command after
integration.

---

### Task G.1: An error result is text with `isError`, never structured content

**Files:**

- Create: `crates/mcp/tests/results.rs`
- Modify: `crates/mcp/src/lib.rs:29-39` (`error_result`) and `:1-6` (header and imports)
- Test: `crates/mcp/tests/results.rs`

**Interfaces:**

- Consumes: `okf_jawn_contract::error::{ApiError, ErrorCode}` (package C: `detail` is boxed; `ApiError::new(code, message)`), `rmcp::model::{CallToolResult, ContentBlock, Tool}`, the generated `api/mcp-tools.json`.
- Produces:

```rust
/// # Errors
/// Returns an error if the generated catalog is missing or incompatible with `rmcp`.
pub fn decode_tools(document: &Value) -> Result<Vec<Tool>, serde_json::Error>;   // unchanged

/// # Errors
/// Returns an error if the `ApiError` cannot be serialized.
pub fn error_result(error: &ApiError) -> Result<CallToolResult, serde_json::Error>;
// content == [text(<ApiError as JSON>)], structured_content == None, is_error == Some(true)
```

- [ ] **Step 1: Write the tests.** Create `crates/mcp/tests/results.rs` with exactly:

```rust
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
    assert!(!tools.is_empty());
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
        let name = tool.get("name").and_then(Value::as_str).ok_or("tool without a name")?;
        let schema = tool.get("outputSchema").ok_or("tool without an outputSchema")?;
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
```

- [ ] **Step 2: Run and watch the error test fail.** From PowerShell in `D:\okf\cure\mcp`:

```powershell
cargo test --locked -p okf-jawn-mcp --test results
```

Expected: `catalog_decodes_every_generated_tool ... ok`,
`no_generated_output_schema_admits_an_error_object ... ok`, and
`error_result_is_text_with_is_error_and_no_structured_content ... FAILED` with
`assertion failed: result.structured_content.is_none()`. Result line: `2 passed; 1 failed`.
(The two passing tests pin what must keep holding: `decode_tools` reads the real catalog, and
the catalog is why an `{"error": …}` object can never be valid structured content. They fail if
`decode_tools` breaks or the generator stops closing its output schemas.)

- [ ] **Step 3: Implement.** In `crates/mcp/src/lib.rs` replace lines 1-6 (header and imports) with:

```rust
//! MCP transport helpers preserve generated tool schemas and application-level content.
//!
//! This is not a JSON-RPC implementation. `rmcp` owns protocol and transport behavior.
//! A failed call is reported as text with `isError` set and no structured content: a tool's
//! declared output schema describes its success response only.

use okf_jawn_contract::error::ApiError;
use rmcp::model::{CallToolResult, ContentBlock, Tool};
use serde_json::Value;
```

and replace the whole `error_result` item (doc comment through closing brace, lines 29-39) with:

```rust
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
```

Leave `decode_tools` and `structured_result` as they are in this task (`structured_result`
still compiles: it uses `Value`, `CallToolResult` and `ContentBlock`, all still imported).

- [ ] **Step 4: Format and run.**

```powershell
rustfmt --edition 2024 crates/mcp/src/lib.rs crates/mcp/tests/results.rs
cargo test --locked -p okf-jawn-mcp --test results
```

Expected: `test result: ok. 3 passed; 0 failed`.

- [ ] **Step 5: Lint the package only.**

```powershell
cargo clippy --locked -p okf-jawn-mcp --all-targets --no-deps -- -D warnings
```

Expected: `Finished` with no `warning:` or `error:` line.

- [ ] **Step 6: Commit.**

```powershell
git add crates/mcp/src/lib.rs crates/mcp/tests/results.rs
git commit -m @'
fix(mcp): report tool errors as text with isError, not as structured content.

Why: error_result put {"error": ...} in structuredContent. Every tool in
api/mcp-tools.json declares an outputSchema with additionalProperties false,
so that object violated the tool's own output contract (SPEC 9: generate MCP
schemas from the declared operation semantics).
What changed: error_result returns CallToolResult::error with one text block
holding the serialized ApiError; structured_content is None; is_error is
Some(true). decode_tools is unchanged and now pinned by a test that reads the
real generated catalog.
Verified: cargo test --locked -p okf-jawn-mcp --test results -> 3 passed;
cargo clippy --locked -p okf-jawn-mcp --all-targets --no-deps -- -D warnings
-> clean.
Next: G.2 makes success results carry the document text.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task G.2: A success result carries the document text within a byte budget

**Files:**

- Modify: `crates/mcp/src/lib.rs` (header; imports; replace `structured_result`; add `MAX_TEXT_BYTES`, `read_result`, two private helpers)
- Modify: `crates/mcp/tests/results.rs` (imports; five new tests)
- Test: `crates/mcp/tests/results.rs`

**Interfaces:**

- Consumes: `okf_jawn_contract::read::ReadItemResponse` (fields used: `markdown: String`, `truncated: bool`, `next_cursor: Option<String>`; `Serialize`), `CallToolResult::success`, `ContentBlock::text`.
- Produces:

```rust
/// Largest text block a helper writes; equal to the largest `max_bytes` a read may request.
pub const MAX_TEXT_BYTES: usize = 1_048_576;

/// Text = the response Markdown, cut at a character boundary to `max_text_bytes`, followed by a
/// bracketed note when it was cut or when the read itself is partial.
/// structured_content = the whole response. is_error = Some(false).
///
/// # Errors
/// Returns an error if the response cannot be serialized.
pub fn read_result(
    response: &ReadItemResponse,
    max_text_bytes: usize,
) -> Result<CallToolResult, serde_json::Error>;

/// Text = `value` serialized as JSON when it fits `max_text_bytes`; otherwise a bracketed note
/// (never a cut JSON prefix). structured_content = `value`. is_error = Some(false).
#[must_use]
pub fn structured_result(value: &Value, max_text_bytes: usize) -> CallToolResult;
```

The handler that the MCP lane writes later calls `read_result(&response, request.max_bytes as usize)`
for `show`, and `structured_result(&serde_json::to_value(&response)?, MAX_TEXT_BYTES)` for every
other tool. The old `structured_result(value, summary)` signature is removed; nothing else in
the workspace calls it (checked with a workspace-wide search at the base commit).

- [ ] **Step 1: Write the failing tests.** In `crates/mcp/tests/results.rs` replace the three
`use okf_jawn_…`/`use rmcp…`/`use serde_json…` lines with:

```rust
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::read::ReadItemResponse;
use okf_jawn_mcp::{MAX_TEXT_BYTES, decode_tools, error_result, read_result, structured_result};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
```

Add this helper after `text_of`:

```rust
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
```

Append these tests at the end of the file:

```rust
#[test]
fn read_result_text_is_the_document_markdown() -> TestResult {
    let response = read_response("# Plan\n\nShip the cure.\n", false, None)?;
    let result = read_result(&response, MAX_TEXT_BYTES)?;
    assert_eq!(text_of(&result)?, "# Plan\n\nShip the cure.\n");
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
    assert!(text.starts_with("éé\n\n[Text cut"), "{text}");
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
        "First page.\n\n[Partial read; continue with cursor cursor-2]"
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
```

- [ ] **Step 2: Run and watch it fail to compile.**

```powershell
cargo test --locked -p okf-jawn-mcp --test results
```

Expected: `error[E0432]: unresolved imports` naming `okf_jawn_mcp::MAX_TEXT_BYTES` and
`okf_jawn_mcp::read_result`; the test target does not build.

- [ ] **Step 3: Implement.** Replace the whole of `crates/mcp/src/lib.rs` with:

```rust
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
```

- [ ] **Step 4: Format and run.**

```powershell
rustfmt --edition 2024 crates/mcp/src/lib.rs crates/mcp/tests/results.rs
cargo test --locked -p okf-jawn-mcp --test results
```

Expected: `test result: ok. 8 passed; 0 failed`.

- [ ] **Step 5: Prove the tests guard the rule.** Temporarily change the first line of
`read_text` to `let (mut text, cut) = within_budget("summary", max_text_bytes);`, run the same
test command, and confirm `read_result_text_is_the_document_markdown ... FAILED`. Then undo that
one-line edit by hand (do not use `git checkout`; Step 3 is not committed yet) and rerun until
`8 passed`.

- [ ] **Step 6: Lint the package only.**

```powershell
cargo clippy --locked -p okf-jawn-mcp --all-targets --no-deps -- -D warnings
```

Expected: `Finished` with no `warning:` or `error:` line.

- [ ] **Step 7: Commit.**

```powershell
git add crates/mcp/src/lib.rs crates/mcp/tests/results.rs
git commit -m @'
fix(mcp): put the document text, not a summary, in tool result content.

Why: structured_result took a caller-written one-line summary as the only
text. A host that ignores structuredContent got no document (SPEC 9: keep
tools usable with a textual fallback; SPEC 7: text-only clients retain usable
descriptions and an explicit truncation indicator).
What changed: read_result(&ReadItemResponse, max_text_bytes) writes the
response Markdown as the text block, cut at a character boundary with a
bracketed note, plus a continuation note when the read is partial.
structured_result(&Value, max_text_bytes) writes the serialized response and
never a cut JSON prefix; its summary parameter is gone and it is infallible.
MAX_TEXT_BYTES is 1048576, the largest max_bytes a read may request.
Verified: cargo test --locked -p okf-jawn-mcp --test results -> 8 passed;
with read_text fed a fixed string the markdown test fails; cargo clippy
--locked -p okf-jawn-mcp --all-targets --no-deps -- -D warnings -> clean.
Next: G.3 updates the lane brief and runs the package gate.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Task G.3: Lane brief names the result rules; package gate

**Files:**

- Modify: `crates/mcp/AGENTS.md:13` (the paragraph that starts "Use rmcp 3.5 constructors")
- Test: none new; the package gate.

**Interfaces:**

- Consumes: the three helpers from G.1 and G.2.
- Produces: no code.

- [ ] **Step 1: Edit the brief.** In `crates/mcp/AGENTS.md`, directly after the paragraph that
starts "Use rmcp 3.5 constructors and selected protocol APIs." insert this paragraph (keep one
blank line before and after it):

```markdown
Tool results go through this crate's helpers. `read_result` puts the document Markdown in `content` within the request's byte budget and the whole response in `structuredContent`; `structured_result` does the same with the serialized response for every other tool. `error_result` returns the serialized `ApiError` as text with `isError` and no `structuredContent`, because each tool's generated `outputSchema` describes its success response only. Do not pass a hand-written summary as the text, and do not use `CallToolResult::structured_error`.
```

- [ ] **Step 2: Run the package gate.**

```powershell
cargo fmt --all --check
cargo clippy --locked -p okf-jawn-mcp --all-targets --no-deps -- -D warnings
cargo test --locked -p okf-jawn-mcp
```

Expected: clippy clean; tests `8 passed; 0 failed` in `results` and `0 passed` in the lib unit
target. `cargo fmt --all --check` must print no diff for `crates/mcp/src/lib.rs` or
`crates/mcp/tests/results.rs`; a diff it prints for a file outside `crates/mcp/` belongs to the
package that owns that file and is reported to the orchestrator, not fixed here.

- [ ] **Step 3: Commit.**

```powershell
git add crates/mcp/AGENTS.md
git commit -m @'
docs(mcp): state the tool result rules in the lane brief.

Why: the MCP lane builds its ServerHandler on these helpers; the brief said
only "preserve text, images, structured output" and did not say what the text
is or how an error is shaped.
What changed: crates/mcp/AGENTS.md names read_result, structured_result and
error_result, the byte budget, and the rule that an error never fills
structuredContent.
Verified: cargo test --locked -p okf-jawn-mcp -> 8 passed; cargo clippy
--locked -p okf-jawn-mcp --all-targets --no-deps -- -D warnings -> clean;
cargo fmt --all --check prints no diff under crates/mcp.
Next: orchestrator verifies package G and merges cure/mcp.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
'@
```

---

### Package G acceptance

A verifying agent that did not write the package runs, from PowerShell in `D:\okf\cure\mcp`:

| Command | Expected |
| --- | --- |
| `git diff --name-only (git merge-base HEAD integration/foundation-cure) HEAD` | exactly `crates/mcp/AGENTS.md`, `crates/mcp/src/lib.rs`, `crates/mcp/tests/results.rs` |
| `cargo fmt --all --check` | no diff printed for any file under `crates/mcp/` |
| `cargo clippy --locked -p okf-jawn-mcp --all-targets --no-deps -- -D warnings` | exit 0, no `warning:` or `error:` line |
| `cargo test --locked -p okf-jawn-mcp` | `results`: `8 passed; 0 failed` |

Two source checks, each expected to print nothing:

```powershell
Select-String -Path crates/mcp/src/lib.rs -Pattern 'structured_error|summary|\.unwrap\(|\.expect\(|\[allow|\[expect'
Select-String -Path crates/mcp/tests/results.rs -Pattern '\.unwrap\(|\.expect\(|panic!|\[allow|\[expect'
```

After packages E and F are merged, the orchestrator also runs the plain gate
`cargo clippy --locked -p okf-jawn-mcp --all-targets -- -D warnings` (no `--no-deps`) and expects
exit 0.

Behaviour check by reading `crates/mcp/tests/results.rs`: the error test asserts
`structured_content.is_none()` and parses the text back into `ApiError`; the read test asserts the
text equals the response Markdown byte for byte; the catalog tests read `api/mcp-tools.json` from
disk, not a copy.

### Deviations

1. **Shared test helper not used.** The shared test idiom lives in `tests/support/check.rs`,
   which package E creates in this same wave; it does not exist on this branch. `results.rs`
   declares `type TestResult = Result<(), Box<dyn Error>>;` locally and needs neither `err_of`
   nor `some`. After integration the orchestrator may replace the local alias with
   `#[path = "../../../tests/support/check.rs"] mod check;` — a one-line follow-up, not part of
   this package.
2. **Clippy is run with `--no-deps` inside the worktree.** The brief's gate is
   `cargo clippy --locked -p okf-jawn-mcp --all-targets -- -D warnings`. Without `--no-deps`
   Clippy also lints the workspace dependency `okf-jawn-core`, which still carries the lint errors
   packages E and F cure in parallel (CI run 37352882339 lists them in `access.rs`, `dispatch.rs`,
   `mutations.rs`, `credentials.rs:41`, `storage.rs:57/77`). The plain command is run by the
   orchestrator after integration.
3. **`structured_result` changed signature and became infallible.** It was
   `(value: &Value, summary: &str) -> Result<CallToolResult, serde_json::Error>` and is now
   `(value: &Value, max_text_bytes: usize) -> CallToolResult`. The summary parameter is the
   defect; nothing can fail once it is gone. No other crate calls it at the base commit.
4. **A text-only host still gets JSON, not prose, for the eleven non-read tools.** That is rmcp's
   own convention (`CallToolResult::structured` writes `value.to_string()`), and is the most the
   helper crate can do without per-tool rendering. Per-tool prose (for example one line per
   search hit) is MCP-lane construction work, not a helper fix.
