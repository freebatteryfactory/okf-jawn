//! Principals, tool inputs, application responses and HTTP helpers shared by the server tests.
//!
//! Inputs are valid against the generated input schemas, so dispatch reaches authorization;
//! responses are valid contract responses, so the fake application can return them.

use std::error::Error;
use std::path::PathBuf;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use okf_jawn_contract::access::{AccessRoute, Principal};
use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::TenantId;
use okf_jawn_mcp::{
    DEFAULT_MAX_REQUEST_BODY_BYTES, McpConfig, McpHttpService, streamable_http_service,
};
use rmcp::model::{CallToolResult, JsonObject};
use serde_json::{Value, json};

use super::fakes::World;
use crate::check::some;

/// A fallible test step.
pub type Outcome<T> = Result<T, Box<dyn Error>>;

/// An HTTP reply with its JSON body (`Null` when the body is empty).
pub struct Reply {
    /// Status code.
    pub status: StatusCode,
    /// Response headers.
    pub headers: HeaderMap,
    /// Parsed body.
    pub body: Value,
}

/// The workspace every fixture names.
pub const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";

/// The item every fixture names.
pub const ITEM: &str = "22222222-2222-4222-8222-222222222222";

/// A second workspace, which a test's access may refuse.
pub const OTHER_WORKSPACE: &str = "66666666-6666-4666-8666-666666666666";

/// The path of the item the image fixtures read.
pub const REPORT: &str = "reports/q3.pdf";

/// The Markdown of the item the image fixtures read.
pub const REPORT_MARKDOWN: &str = "# Q3 report\n\nRevenue grew in every region.";

/// The `Host` the test services accept.
pub const HOST: &str = "localhost";

/// A principal of `subject` that arrived through `route`.
///
/// # Errors
/// Returns an error when the test tenant id is refused.
pub fn principal(subject: &str, route: AccessRoute) -> Outcome<Principal> {
    Ok(Principal {
        subject: subject.to_owned(),
        tenant_id: TenantId::try_from("tenant-test".to_owned()).map_err(|error| error.0)?,
        route,
        client_id: Some("client-test".to_owned()),
        delegation: None,
    })
}

/// An external agent (`McpDelegation`) acting as `subject`.
///
/// # Errors
/// Returns an error when the test tenant id is refused.
pub fn agent(subject: &str) -> Outcome<Principal> {
    principal(subject, AccessRoute::McpDelegation)
}

/// A resolved revision.
#[must_use]
pub fn revision() -> String {
    "a".repeat(40)
}

/// An object digest.
#[must_use]
pub fn digest() -> String {
    "b".repeat(64)
}

/// The source reference the `read_object` fixtures use.
#[must_use]
pub fn source() -> Value {
    json!({
        "workspace_id": WORKSPACE,
        "item_id": ITEM,
        "path": "notes/plan.md",
        "revision": revision(),
        "selection": {"kind": "all"}
    })
}

/// `read_object` (`get_object`) arguments.
#[must_use]
pub fn read_object_input() -> Value {
    json!({"source": source(), "object": digest()})
}

/// A `get_object` response.
#[must_use]
pub fn object_response() -> Value {
    json!({
        "sha256": digest(),
        "offset": "0",
        "total_size": "5",
        "media_type": "text/plain",
        "data_base64": "aGVsbG8=",
        "has_more": false
    })
}

/// `show` (`read_item`) arguments with a `max_bytes` budget.
#[must_use]
pub fn read_input(max_bytes: u32) -> Value {
    json!({
        "workspace_id": WORKSPACE,
        "item_id": ITEM,
        "at": {"kind": "latest"},
        "view": "text",
        "selection": {"kind": "all"},
        "max_bytes": max_bytes,
        "max_images": 0
    })
}

/// A complete, untruncated `read_item` response carrying `markdown`.
#[must_use]
pub fn read_response(markdown: &str) -> Value {
    json!({
        "source": source(),
        "view": "text",
        "markdown": markdown,
        "outline": [],
        "media": [],
        "warnings": [],
        "truncated": false,
        "receipt_id": "33333333-3333-4333-8333-333333333333"
    })
}

/// `show` arguments for a read of `view` (`multimodal` or `pages`) that selects images.
#[must_use]
pub fn image_read_input(view: &str, max_bytes: u32, max_images: u16) -> Value {
    json!({
        "workspace_id": WORKSPACE,
        "item_id": ITEM,
        "at": {"kind": "latest"},
        "view": view,
        "selection": {"kind": "all"},
        "max_bytes": max_bytes,
        "max_images": max_images
    })
}

/// A complete read of `view` whose citation names `workspace` and which selected `media`.
#[must_use]
pub fn image_read_response(workspace: &str, view: &str, media: &[Value]) -> Value {
    json!({
        "source": {
            "workspace_id": workspace,
            "item_id": ITEM,
            "path": REPORT,
            "revision": revision(),
            "selection": {"kind": "all"}
        },
        "view": view,
        "markdown": REPORT_MARKDOWN,
        "outline": [],
        "media": media,
        "warnings": [],
        "truncated": false,
        "receipt_id": "33333333-3333-4333-8333-333333333333"
    })
}

/// A selected image: object `digest` of `media_type`, a picture located at `location`, with
/// its caption when it has one.
#[must_use]
pub fn media(digest: &str, media_type: &str, caption: Option<&str>, location: &Value) -> Value {
    let mut media = json!({
        "object": digest,
        "media_type": media_type,
        "role": "picture",
        "source": {
            "workspace_id": WORKSPACE,
            "item_id": ITEM,
            "path": REPORT,
            "revision": revision(),
            "digest": digest,
            "selection": {"kind": "all"},
            "locations": [location]
        },
        "width": 640,
        "height": 480
    });
    if let (Some(caption), Some(fields)) = (caption, media.as_object_mut()) {
        fields.insert("caption".to_owned(), json!(caption));
    }
    media
}

/// A location the converter gave: a whole page.
#[must_use]
pub fn on_page(page_no: u32) -> Value {
    json!({"provenance": "direct", "locator": {"kind": "page", "page_no": page_no}})
}

/// A location the converter gave: a box on a page.
#[must_use]
pub fn in_region(page_no: u32) -> Value {
    json!({
        "provenance": "direct",
        "locator": {"kind": "region", "region": {
            "page_no": page_no,
            "bbox": {"l": 72.5, "t": 100.0, "r": 300.0, "b": 400.0, "coord_origin": "top_left"},
            "page_size": {"width": 612.0, "height": 792.0}
        }}
    })
}

/// A `get_object` block of the object `digest`: `data_base64` served as `media_type`, from a
/// whole object of `total_size` bytes.
#[must_use]
pub fn object_block(
    digest: &str,
    media_type: &str,
    data_base64: &str,
    total_size: usize,
    has_more: bool,
) -> Value {
    json!({
        "sha256": digest,
        "offset": "0",
        "total_size": total_size.to_string(),
        "media_type": media_type,
        "data_base64": data_base64,
        "has_more": has_more
    })
}

/// Padded base64 of `bytes` zero bytes.
#[must_use]
pub fn zeros_base64(bytes: usize) -> String {
    let tail = match bytes % 3 {
        1 => "AA==",
        2 => "AAA=",
        _ => "",
    };
    format!("{}{tail}", "AAAA".repeat(bytes / 3))
}

/// A distinct object digest spelled with the hex digit `digit`.
#[must_use]
pub fn digest_of(digit: char) -> String {
    digit.to_string().repeat(64)
}

/// The `get_object` request the server sends for the image `digest` of a read citing
/// `workspace`, asking for at most `length` bytes.
#[must_use]
pub fn object_request(workspace: &str, digest: &str, length: usize) -> Value {
    json!({
        "source": {
            "workspace_id": workspace,
            "item_id": ITEM,
            "path": REPORT,
            "revision": revision(),
            "digest": digest,
            "selection": {"kind": "all"}
        },
        "object": digest,
        "length": length
    })
}

/// `workspaces` (`list_workspaces`) arguments.
#[must_use]
pub fn workspaces_input() -> Value {
    json!({"page": {"limit": 10}})
}

/// A `list_workspaces` response with one workspace.
#[must_use]
pub fn workspaces_response() -> Value {
    json!({
        "items": [{
            "id": WORKSPACE,
            "name": "Plans",
            "description": "Planning notes.",
            "head": revision(),
            "created_at": "2026-10-11T00:00:00.000Z",
            "permissions": ["read"]
        }]
    })
}

/// `propose` (`open_proposal`) arguments: a keyed mutation.
#[must_use]
pub fn propose_input() -> Value {
    json!({
        "workspace_id": WORKSPACE,
        "base_revision": revision(),
        "title": "Retire the old plan",
        "description": "Superseded.",
        "changes": [{"kind": "archive", "item_id": ITEM}],
        "idempotency_key": "55555555-5555-4555-8555-555555555555"
    })
}

/// The proposal `open_proposal` answers with.
#[must_use]
pub fn proposal_response() -> Value {
    json!({
        "id": "44444444-4444-4444-8444-444444444444",
        "workspace_id": WORKSPACE,
        "base_revision": revision(),
        "proposal_revision": "c".repeat(40),
        "content_digest": digest(),
        "title": "Retire the old plan",
        "description": "Superseded.",
        "changes": [{"kind": "archive", "item_id": ITEM}],
        "kind": "content",
        "status": "open",
        "created_by": "alice",
        "created_via": "mcp_delegation",
        "created_at": "2026-10-11T00:00:00.000Z"
    })
}

/// The object form of tool arguments.
///
/// # Errors
/// Returns an error when `value` is not a JSON object.
pub fn arguments(value: Value) -> Outcome<JsonObject> {
    match value {
        Value::Object(map) => Ok(map),
        other => Err(format!("tool arguments must be an object, got {other}").into()),
    }
}

/// A committed generated document under `api/`, read independently of the crate.
///
/// # Errors
/// Returns an error when the file is missing or not JSON.
pub fn committed(name: &str) -> Outcome<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../api")
        .join(name);
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

/// Write an App bundle named `name` holding `html` into the test scratch directory.
///
/// # Errors
/// Returns an error when the file cannot be written.
pub fn bundle(name: &str, html: &str) -> Outcome<PathBuf> {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, html)?;
    Ok(path)
}

/// A bundle path in the scratch directory where no file exists.
///
/// # Errors
/// Returns an error when a stale file there cannot be removed.
pub fn missing_bundle(name: &str) -> Outcome<PathBuf> {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(path)
}

/// The first text block of a tool result.
///
/// # Errors
/// Returns an error when the result has no text block.
pub fn text_of(result: &CallToolResult) -> Outcome<&str> {
    let block = some(result.content.first(), "a content block")?;
    Ok(some(block.as_text(), "a text block")?.text.as_str())
}

/// The `ApiError` of a failed tool result, checking it has the shape of `error_result`.
///
/// # Errors
/// Returns an error when the result is not `isError` text without structured content.
pub fn refusal_of(result: &CallToolResult) -> Outcome<ApiError> {
    if result.is_error != Some(true) {
        return Err(format!("expected a failed tool result, got {result:?}").into());
    }
    if result.structured_content.is_some() {
        return Err("a failed tool result carries structured content".into());
    }
    Ok(serde_json::from_str(text_of(result)?)?)
}

/// The Streamable HTTP service over `world`'s ports, accepting `HOST`.
///
/// # Errors
/// Returns the constructor's error.
pub fn http_service(world: &World, app_bundle: PathBuf) -> Outcome<McpHttpService> {
    Ok(streamable_http_service(
        world.ports.clone(),
        McpConfig {
            app_bundle,
            allowed_hosts: vec![HOST.to_owned()],
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
        },
    )?)
}

/// A JSON-RPC request.
#[must_use]
pub fn rpc(id: u64, method: &str, params: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// The `initialize` request an MCP client opens with.
#[must_use]
pub fn initialize(id: u64) -> Value {
    rpc(
        id,
        "initialize",
        &json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "okf-jawn-mcp-tests", "version": "0"}
        }),
    )
}

/// POST `body` to `service` as the identity middleware would deliver it: `principal`, when
/// present, in the request's extensions.
///
/// # Errors
/// Returns an error when the request cannot be built or the body is not JSON.
pub async fn post(
    service: &McpHttpService,
    principal: Option<Principal>,
    host: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> Outcome<Reply> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/")
        .header(header::HOST, host)
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let mut request = builder.body(Body::from(body.to_string()))?;
    if let Some(principal) = principal {
        request.extensions_mut().insert(principal);
    }
    let (parts, body) = service.handle(request).await.into_parts();
    let bytes = axum::body::to_bytes(Body::new(body), usize::MAX).await?;
    // rmcp answers a transport refusal (a disallowed host, say) in plain text.
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    Ok(Reply {
        status: parts.status,
        headers: parts.headers,
        body,
    })
}

/// The JSON-RPC `result` of a 200 reply.
///
/// # Errors
/// Returns an error when the reply is not a 200 JSON-RPC result.
pub fn result_of(reply: &Reply) -> Outcome<&Value> {
    if reply.status != StatusCode::OK {
        return Err(format!("expected 200, got {}: {}", reply.status, reply.body).into());
    }
    Ok(some(reply.body.get("result"), "a JSON-RPC result")?)
}

/// The `ApiError` a refused JSON-RPC request carries in its error's `data`.
///
/// # Errors
/// Returns an error when the reply has a result or its error carries no `ApiError`.
pub fn rpc_refusal(reply: &Reply) -> Outcome<ApiError> {
    if reply.body.get("result").is_some() {
        return Err(format!("expected a refusal, got {}", reply.body).into());
    }
    let data = some(reply.body.pointer("/error/data"), "the error's data")?;
    Ok(serde_json::from_value(data.clone())?)
}

/// The tool result of a `tools/call` reply.
///
/// # Errors
/// Returns an error when the reply carries no tool result.
pub fn call_result(reply: &Reply) -> Outcome<CallToolResult> {
    Ok(serde_json::from_value(result_of(reply)?.clone())?)
}

/// A `tools/call` request for `tool` with `input`.
#[must_use]
pub fn call(id: u64, tool: &str, input: &Value) -> Value {
    rpc(id, "tools/call", &json!({"name": tool, "arguments": input}))
}
