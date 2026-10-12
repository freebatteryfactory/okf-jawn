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

/// `list_drafts` arguments: a draft-bearing operation, never a tool.
#[must_use]
pub fn list_drafts_input() -> Value {
    json!({"workspace_id": WORKSPACE, "page": {"limit": 10}})
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
