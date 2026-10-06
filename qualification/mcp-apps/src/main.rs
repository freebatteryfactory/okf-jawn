//! Phase 0 MCP Apps qualification harness.
//!
//! Serves built `ui/dist-apps` HTML bundles as MCP UI resources, four read-only
//! render tools and two app-only fixture tools over stdio or Streamable HTTP via `rmcp`:
//! `show` (the present view resolves its bindings through it) and `read_object` (the
//! present view fetches its retained dataset through it, in blocks of at most
//! `READ_OBJECT_MAX_BYTES` so the App's ranged loop runs more than once). Every tool call
//! is reported on stderr as one `TOOL_CALL_LOG_PREFIX` line; the orchestrator counts the
//! App's calls from those lines. This is not the product `ServerHandler` and must not be
//! linked from product crates.

use axum::Router;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData as McpError,
    Implementation, JsonObject, ListResourcesResult, ListToolsResult, MetaObject,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    Resource, ResourceContents, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{RoleServer, ServiceExt};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::future::Future;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tower_http::cors::{Any, CorsLayer};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestResource {
    byte_length: u64,
    mime_type: String,
    name: String,
    sha256: String,
    uri: String,
}

#[derive(Clone, Debug, Deserialize)]
struct Manifest {
    resources: Vec<ManifestResource>,
}

#[derive(Clone)]
struct BundledApp {
    byte_length: u64,
    bytes: Arc<[u8]>,
    mime_type: String,
    name: String,
    path: PathBuf,
    sha256: String,
    uri: String,
}

#[derive(Clone)]
struct RenderTool {
    description: &'static str,
    fixture: Value,
    name: &'static str,
    resource_uri: String,
    summary: String,
}

#[derive(Clone)]
struct QualifyAppsServer {
    by_uri: BTreeMap<String, BundledApp>,
    dataset: Dataset,
    resources: Vec<BundledApp>,
    show: ShowFixtures,
    tools: Vec<RenderTool>,
}

/// Fixtures the app-only `show` tool answers from.
#[derive(Clone)]
struct ShowFixtures {
    present: Value,
    read_item: Value,
}

/// The retained dataset the app-only `read_object` tool serves, addressed by its own SHA-256.
#[derive(Clone, Debug)]
struct Dataset {
    bytes: Arc<[u8]>,
    sha256: String,
}

const DATASET_FIXTURE: &str = "present-metrics-dataset.json";
const DATASET_MEDIA_TYPE: &str = "application/json";
const READ_OBJECT_DESCRIPTION: &str = "App-only fixture read: returns one bounded block of the dataset a present-fixture binding retains, authorized through that binding's source reference; a digest alone is refused.";
/// Largest block one `read_object` call returns; smaller than the dataset on purpose.
const READ_OBJECT_MAX_BYTES: usize = 64;
const READ_OBJECT_TOOL: &str = "read_object";
/// Each render tool: name, description, fixture file and text fallback.
const RENDER_TOOL_SPECS: [(&str, &str, &str, &str); 4] = [
    (
        "render_source",
        "Render the source excerpt App from a contract-valid ReadItemResponse fixture.",
        "source-read-item.json",
        "Qualification source fixture (text fallback).",
    ),
    (
        "render_changes",
        "Render the changes App from a contract-valid DiffResponse fixture.",
        "changes-diff.json",
        "Qualification changes fixture (text fallback).",
    ),
    (
        "render_timeline",
        "Render the timeline App from a contract-valid LogResponse fixture.",
        "timeline-log.json",
        "Qualification timeline fixture (text fallback).",
    ),
    (
        "render_present",
        "Render the present App from a contract-valid PresentResponse fixture.",
        "present-response.json",
        "Qualification present fixture (text fallback).",
    ),
];
const SHOW_DESCRIPTION: &str = "App-only fixture read: returns the source fixture text under the source reference of the present-fixture binding that cites the requested item.";
const SHOW_TOOL: &str = "show";
/// Prefix of the one stderr line written per tool call; the rest of the line is JSON.
const TOOL_CALL_LOG_PREFIX: &str = "okf-qualify-mcp-apps tool-call ";
const TUNNEL_ENV: &str = "OKF_MCP_APPS_NGROK";

impl Dataset {
    /// Reads the dataset fixture and requires the present fixture to retain exactly its bytes.
    fn load(fixtures: &Path, present: &Value) -> Result<Self, String> {
        let path = fixtures.join(DATASET_FIXTURE);
        let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        Self::retained(bytes, present)
    }

    /// The dataset over `bytes`, provided a resolved binding of `present` names their digest.
    fn retained(bytes: Vec<u8>, present: &Value) -> Result<Self, String> {
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let retained: Vec<&str> = resolved_bindings(present)
            .filter_map(|binding| binding.get("materialized").and_then(Value::as_str))
            .collect();
        if !retained.contains(&sha256.as_str()) {
            return Err(format!(
                "{DATASET_FIXTURE} sha256 {sha256} is not a digest the present fixture retains ({retained:?})"
            ));
        }
        Ok(Self {
            bytes: Arc::from(bytes.into_boxed_slice()),
            sha256,
        })
    }
}

impl QualifyAppsServer {
    fn load(dist_apps: &Path) -> Result<Self, String> {
        let manifest_path = dist_apps.join("manifest.json");
        let raw = fs::read_to_string(&manifest_path)
            .map_err(|error| format!("read {}: {error}", manifest_path.display()))?;
        let manifest: Manifest = serde_json::from_str(&raw)
            .map_err(|error| format!("parse {}: {error}", manifest_path.display()))?;
        if manifest.resources.len() != 1 {
            return Err(format!(
                "manifest must list exactly 1 shared App resource, found {}",
                manifest.resources.len()
            ));
        }
        let mut resources = Vec::with_capacity(1);
        let mut by_uri = BTreeMap::new();
        for entry in manifest.resources {
            let path = dist_apps.join(format!("{}.html", entry.name));
            let bytes =
                fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
            let digest = format!("{:x}", Sha256::digest(&bytes));
            let byte_length = u64::try_from(bytes.len())
                .map_err(|_| format!("{} length exceeds u64", path.display()))?;
            if byte_length != entry.byte_length {
                return Err(format!(
                    "{} byteLength mismatch: manifest {} vs file {}",
                    entry.name, entry.byte_length, byte_length
                ));
            }
            if digest != entry.sha256 {
                return Err(format!(
                    "{} sha256 mismatch: manifest {} vs file {}",
                    entry.name, entry.sha256, digest
                ));
            }
            if entry.uri != "ui://okf-jawn/app.html" {
                return Err(format!(
                    "shared App uri must be ui://okf-jawn/app.html, got {}",
                    entry.uri
                ));
            }
            let app = BundledApp {
                byte_length,
                bytes: Arc::from(bytes.into_boxed_slice()),
                mime_type: entry.mime_type,
                name: entry.name,
                path,
                sha256: digest,
                uri: entry.uri,
            };
            by_uri.insert(app.uri.clone(), app.clone());
            resources.push(app);
        }

        let shared = resources
            .first()
            .ok_or_else(|| "manifest missing shared App resource".to_owned())?;
        let fixtures = fixtures_dir();
        let mut tools = Vec::with_capacity(4);
        for (name, description, fixture_name, summary) in RENDER_TOOL_SPECS {
            tools.push(RenderTool {
                description,
                fixture: load_fixture(&fixtures.join(fixture_name))?,
                name,
                resource_uri: shared.uri.clone(),
                summary: summary.to_owned(),
            });
        }

        let show = ShowFixtures {
            present: load_fixture(&fixtures.join("present-response.json"))?,
            read_item: load_fixture(&fixtures.join("source-read-item.json"))?,
        };
        let dataset = Dataset::load(&fixtures, &show.present)?;

        Ok(Self {
            by_uri,
            dataset,
            resources,
            show,
            tools,
        })
    }

    fn list_catalog(&self) -> ListResourcesResult {
        let listed = self
            .resources
            .iter()
            .map(|app| {
                Resource::new(app.uri.clone(), app.name.clone())
                    .with_mime_type(app.mime_type.clone())
                    .with_size(app.byte_length)
                    .with_meta(resource_ui_meta())
            })
            .collect();
        ListResourcesResult::with_all_items(listed)
    }

    fn read_catalog(&self, uri: &str) -> Result<ReadResourceResult, McpError> {
        let Some(app) = self.by_uri.get(uri) else {
            return Err(McpError::resource_not_found(
                "resource_not_found",
                Some(json!({ "uri": uri })),
            ));
        };
        let text = String::from_utf8(app.bytes.as_ref().to_vec()).map_err(|error| {
            McpError::invalid_params(format!("resource {} is not utf-8: {error}", app.uri), None)
        })?;
        let contents = ResourceContents::text(text, app.uri.clone())
            .with_mime_type(app.mime_type.clone())
            .with_meta(resource_ui_meta());
        Ok(ReadResourceResult::new(vec![contents]))
    }

    fn tool_definitions(&self) -> Vec<Tool> {
        let schema = empty_object_schema();
        let mut tools: Vec<Tool> = self
            .tools
            .iter()
            .map(|tool| {
                Tool::new(tool.name, tool.description, schema.clone())
                    .with_annotations(ToolAnnotations::new().read_only(true))
                    .with_meta(tool_ui_meta(&tool.resource_uri))
            })
            .collect();
        tools.push(
            Tool::new(SHOW_TOOL, SHOW_DESCRIPTION, show_schema())
                .with_annotations(ToolAnnotations::new().read_only(true))
                .with_meta(app_only_meta()),
        );
        tools.push(
            Tool::new(
                READ_OBJECT_TOOL,
                READ_OBJECT_DESCRIPTION,
                read_object_schema(),
            )
            .with_annotations(ToolAnnotations::new().read_only(true))
            .with_meta(app_only_meta()),
        );
        tools
    }

    fn call_render_tool(&self, name: &str) -> Result<CallToolResult, McpError> {
        let Some(tool) = self.tools.iter().find(|tool| tool.name == name) else {
            return Err(McpError::invalid_params(
                format!("unknown tool: {name}"),
                Some(json!({ "name": name })),
            ));
        };
        let mut result = CallToolResult::success(vec![ContentBlock::text(tool.summary.clone())]);
        result.structured_content = Some(tool.fixture.clone());
        Ok(result)
    }

    fn call_show(&self, arguments: Option<&JsonObject>) -> Result<CallToolResult, McpError> {
        let source = show_fixture(&self.show, arguments)
            .map_err(|message| McpError::invalid_params(message, None))?;
        let mut result = CallToolResult::success(vec![ContentBlock::text(
            "Qualification show fixture (text fallback).".to_owned(),
        )]);
        result.structured_content = Some(source);
        Ok(result)
    }

    /// A refused read is a tool error (`isError`), as the product MCP binding reports one.
    fn call_read_object(&self, arguments: Option<&JsonObject>) -> CallToolResult {
        match read_object_block(&self.dataset, &self.show.present, arguments) {
            Ok(block) => {
                let mut result = CallToolResult::success(vec![ContentBlock::text(
                    "Qualification read_object block (text fallback).".to_owned(),
                )]);
                result.structured_content = Some(block);
                result
            }
            Err(message) => CallToolResult::error(vec![ContentBlock::text(message)]),
        }
    }

    /// What this server answers, taken from the answers themselves: each resource as
    /// `resources/read` returns it, each tool as `tools/list` lists it and each render tool's
    /// result as `tools/call` produces it. Nothing here restates a value the handlers hold.
    fn check_report(&self) -> Result<Value, String> {
        let mut resources = Vec::with_capacity(self.resources.len());
        for app in &self.resources {
            let read = self
                .read_catalog(&app.uri)
                .ok()
                .and_then(|read| serde_json::to_value(read).ok());
            let content = read.as_ref().and_then(|read| read.pointer("/contents/0"));
            let readable = content
                .and_then(|content| content.get("text"))
                .and_then(Value::as_str)
                .is_some_and(|body| {
                    let lower = body.to_ascii_lowercase();
                    lower.starts_with("<!doctype html") || lower.contains("id=\"root\"")
                });
            resources.push(json!({
                "name": app.name,
                "uri": app.uri,
                "mimeType": app.mime_type,
                "byteLength": app.byte_length,
                "sha256": app.sha256,
                "readable": readable,
                "path": app.path,
                "metaUi": content.and_then(|content| content.pointer("/_meta/ui")),
            }));
        }

        let listed = serde_json::to_value(self.tool_definitions())
            .map_err(|error| format!("serialize the tool list: {error}"))?;
        let mut tools = Vec::new();
        let mut app_tools = Vec::new();
        for tool in listed.as_array().into_iter().flatten() {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("listed tool has no name: {tool}"))?;
            if tool.pointer("/_meta/ui/visibility") == Some(&json!(["app"])) {
                app_tools.push(json!(name));
                continue;
            }
            let structured = self
                .call_render_tool(name)
                .ok()
                .and_then(|result| result.structured_content)
                .is_some_and(|content| !content.is_null());
            tools.push(json!({
                "name": name,
                "resourceUri": tool.pointer("/_meta/ui/resourceUri"),
                "hasStructuredContent": structured,
            }));
        }
        Ok(json!({
            "mode": "check",
            "resource_count": resources.len(),
            "tool_count": tools.len(),
            "resources": resources,
            "tools": tools,
            "app_tools": app_tools,
            "dataset": {
                "fixture": DATASET_FIXTURE,
                "sha256": self.dataset.sha256,
                "byteLength": self.dataset.bytes.len(),
                "maxBlockBytes": READ_OBJECT_MAX_BYTES,
            },
        }))
    }
}

impl ServerHandler for QualifyAppsServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_resources()
                .enable_tools()
                .build(),
        )
        .with_server_info(Implementation::new(
            "okf-qualify-mcp-apps",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(
            "Phase 0 MCP Apps qualification harness. Serves built ui/dist-apps HTML bundles and read-only fixture render tools.",
        )
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        std::future::ready(Ok(self.list_catalog()))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResponse, McpError>> + Send + '_ {
        std::future::ready(self.read_catalog(&request.uri).map(Into::into))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(self.tool_definitions())))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_definitions()
            .into_iter()
            .find(|tool| tool.name == name)
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, McpError>> + Send + '_ {
        let result = if request.name == SHOW_TOOL {
            self.call_show(request.arguments.as_ref())
        } else if request.name == READ_OBJECT_TOOL {
            Ok(self.call_read_object(request.arguments.as_ref()))
        } else {
            self.call_render_tool(&request.name)
        };
        let record = tool_call_record(&request.name, result.as_ref().ok());
        let _ = writeln!(io::stderr(), "{TOOL_CALL_LOG_PREFIX}{record}");
        std::future::ready(result.map(Into::into))
    }
}

fn empty_object_schema() -> Arc<JsonObject> {
    let mut schema = Map::new();
    schema.insert("type".to_owned(), json!("object"));
    schema.insert("properties".to_owned(), json!({}));
    schema.insert("additionalProperties".to_owned(), json!(false));
    Arc::new(schema)
}

fn resource_ui_meta() -> MetaObject {
    let mut meta = MetaObject::new();
    meta.insert(
        "ui".to_owned(),
        json!({
            "csp": {
                "connectDomains": [],
                "resourceDomains": []
            },
            "prefersBorder": true
        }),
    );
    meta
}

fn tool_ui_meta(resource_uri: &str) -> MetaObject {
    let mut meta = MetaObject::new();
    meta.insert(
        "ui".to_owned(),
        json!({
            "resourceUri": resource_uri
        }),
    );
    meta
}

fn app_only_meta() -> MetaObject {
    let mut meta = MetaObject::new();
    meta.insert("ui".to_owned(), json!({ "visibility": ["app"] }));
    meta
}

fn show_schema() -> Arc<JsonObject> {
    let mut schema = Map::new();
    schema.insert("type".to_owned(), json!("object"));
    schema.insert(
        "properties".to_owned(),
        json!({ "item_id": { "type": "string" }, "at": { "type": "object" } }),
    );
    schema.insert("required".to_owned(), json!(["item_id", "at"]));
    Arc::new(schema)
}

/// The input shape of the product `read_object` tool (`GetObjectRequest` in `api/mcp-tools.json`).
fn read_object_schema() -> Arc<JsonObject> {
    let mut schema = Map::new();
    schema.insert("type".to_owned(), json!("object"));
    schema.insert(
        "properties".to_owned(),
        json!({
            "source": { "type": "object" },
            "object": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "offset": { "type": ["string", "null"] },
            "length": { "type": ["integer", "null"], "minimum": 0 },
        }),
    );
    schema.insert("required".to_owned(), json!(["source", "object"]));
    schema.insert("additionalProperties".to_owned(), json!(false));
    Arc::new(schema)
}

/// The bindings the present fixture hands the App.
fn resolved_bindings(present: &Value) -> impl Iterator<Item = &Value> {
    present
        .get("resolved_bindings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// The decimal byte offset a `read_object` request names; absent or null means the start.
fn requested_offset(input: &JsonObject) -> Result<usize, String> {
    match input.get("offset") {
        None | Some(Value::Null) => Ok(0),
        Some(Value::String(text))
            if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            text.parse()
                .map_err(|error| format!("read_object: offset {text}: {error}"))
        }
        Some(other) => Err(format!(
            "read_object: offset must be a decimal string, got {other}"
        )),
    }
}

/// The block length a `read_object` request gets: what it asked for, capped at the maximum.
fn requested_length(input: &JsonObject) -> Result<usize, String> {
    match input.get("length") {
        None | Some(Value::Null) => Ok(READ_OBJECT_MAX_BYTES),
        Some(value) => {
            let length = value
                .as_u64()
                .and_then(|length| u32::try_from(length).ok())
                .ok_or_else(|| format!("read_object: length must be a uint32, got {value}"))?;
            Ok(
                usize::try_from(length).map_or(READ_OBJECT_MAX_BYTES, |length| {
                    length.min(READ_OBJECT_MAX_BYTES)
                }),
            )
        }
    }
}

/// One block of the dataset as a `GetObjectResponse`, for a request shaped like the product's
/// `GetObjectRequest`. The object must be the dataset's digest and `source` must be the source
/// of a present-fixture binding that retains it: a digest alone never grants the read.
fn read_object_block(
    dataset: &Dataset,
    present: &Value,
    arguments: Option<&JsonObject>,
) -> Result<Value, String> {
    let input = arguments.ok_or_else(|| "read_object requires arguments".to_owned())?;
    let object = input
        .get("object")
        .and_then(Value::as_str)
        .ok_or_else(|| "read_object requires a string object digest".to_owned())?;
    if object != dataset.sha256 {
        return Err(format!("read_object: no retained object {object}"));
    }
    let source = input
        .get("source")
        .filter(|source| source.is_object())
        .ok_or_else(|| "read_object requires a source reference".to_owned())?;
    let authorized = resolved_bindings(present).any(|binding| {
        binding.get("materialized").and_then(Value::as_str) == Some(object)
            && binding.get("source") == Some(source)
    });
    if !authorized {
        return Err(format!(
            "read_object: object {object} is not retained by a binding with that source"
        ));
    }
    let total = dataset.bytes.len();
    let offset = requested_offset(input)?;
    if offset > total {
        return Err(format!(
            "read_object: offset {offset} is beyond the object's {total} bytes"
        ));
    }
    let end = offset.saturating_add(requested_length(input)?).min(total);
    let block = dataset
        .bytes
        .get(offset..end)
        .ok_or_else(|| format!("read_object: no bytes {offset}..{end} of {total}"))?;
    Ok(json!({
        "sha256": dataset.sha256,
        "offset": offset.to_string(),
        "total_size": total.to_string(),
        "media_type": DATASET_MEDIA_TYPE,
        "data_base64": base64_encode(block),
        "has_more": end < total,
    }))
}

/// Standard padded base64 (RFC 4648 section 4). The crate manifest is fixed for this
/// harness, so the encoder the `GetObjectResponse` field needs is written out here.
fn base64_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for group in bytes.chunks(3) {
        let first = group.first().copied().unwrap_or(0);
        let second = group.get(1).copied();
        let third = group.get(2).copied();
        let bits = u32::from_be_bytes([0, first, second.unwrap_or(0), third.unwrap_or(0)]);
        encoded.push(base64_digit(bits >> 18));
        encoded.push(base64_digit(bits >> 12));
        encoded.push(second.map_or('=', |_| base64_digit(bits >> 6)));
        encoded.push(third.map_or('=', |_| base64_digit(bits)));
    }
    encoded
}

/// The base64 digit for the low six bits of `bits`.
fn base64_digit(bits: u32) -> char {
    let [.., low] = bits.to_be_bytes();
    let six = low & 0x3f;
    match six {
        0..=25 => char::from(b'A'.saturating_add(six)),
        26..=51 => char::from(b'a'.saturating_add(six.saturating_sub(26))),
        52..=61 => char::from(b'0'.saturating_add(six.saturating_sub(52))),
        62 => '+',
        _ => '/',
    }
}

/// What one tool call is reported as on stderr: the tool, whether it succeeded and, for a
/// `read_object` block, which object and range it served. `result` is `None` for a call the
/// server rejected at the protocol level.
fn tool_call_record(name: &str, result: Option<&CallToolResult>) -> Value {
    let ok = result.is_some_and(|result| result.is_error != Some(true));
    let mut record = Map::new();
    record.insert("tool".to_owned(), json!(name));
    record.insert("ok".to_owned(), json!(ok));
    let block = result
        .filter(|_| ok && name == READ_OBJECT_TOOL)
        .and_then(|result| result.structured_content.as_ref());
    if let Some(block) = block {
        for (field, key) in [
            ("object", "sha256"),
            ("offset", "offset"),
            ("has_more", "has_more"),
        ] {
            if let Some(value) = block.get(key) {
                record.insert(field.to_owned(), value.clone());
            }
        }
    }
    Value::Object(record)
}

/// The read result the present fixture's binding for the requested item resolves to.
fn show_fixture(fixtures: &ShowFixtures, arguments: Option<&JsonObject>) -> Result<Value, String> {
    let item_id = arguments
        .and_then(|input| input.get("item_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| "show requires a string item_id".to_owned())?;
    let revision = arguments
        .and_then(|input| input.get("at"))
        .and_then(|at| at.get("revision"))
        .and_then(Value::as_str)
        .ok_or_else(|| "show requires at.revision; qualification reads are pinned".to_owned())?;
    let source = resolved_bindings(&fixtures.present)
        .filter_map(|binding| binding.get("source"))
        .find(|source| source.get("item_id").and_then(Value::as_str) == Some(item_id))
        .ok_or_else(|| format!("show: no present-fixture binding cites item {item_id}"))?;
    if source.get("revision").and_then(Value::as_str) != Some(revision) {
        return Err(format!(
            "show: {revision} is not the revision bound for item {item_id}"
        ));
    }
    let mut result = fixtures.read_item.clone();
    let fields = result
        .as_object_mut()
        .ok_or_else(|| "source-read-item.json must be a JSON object".to_owned())?;
    fields.insert("source".to_owned(), source.clone());
    Ok(result)
}

/// Only the exact value `1` lifts Host protection; `0`, `true` or an empty value keep it.
fn tunnel_hosts_allowed(value: Option<&str>) -> bool {
    value == Some("1")
}

fn fixtures_dir() -> PathBuf {
    env::var_os("OKF_MCP_APPS_FIXTURES").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/views"),
        PathBuf::from,
    )
}

fn load_fixture(path: &Path) -> Result<Value, String> {
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("read fixture {}: {error}", path.display()))?;
    serde_json::from_str(&raw).map_err(|error| format!("parse fixture {}: {error}", path.display()))
}

fn dist_apps_dir() -> PathBuf {
    env::var_os("OKF_MCP_APPS_DIST").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist-apps"),
        PathBuf::from,
    )
}

fn parse_http_bind() -> Result<Option<SocketAddr>, String> {
    let mut args = env::args().skip(1);
    let mut bind: Option<String> = env::var("OKF_MCP_APPS_HTTP")
        .ok()
        .filter(|value| !value.is_empty());
    while let Some(arg) = args.next() {
        if arg == "--check" {
            continue;
        }
        if arg == "--http" {
            let Some(value) = args.next() else {
                return Err("--http requires 127.0.0.1:PORT".to_owned());
            };
            bind = Some(value);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--http=") {
            bind = Some(value.to_owned());
            continue;
        }
        return Err(format!("unknown argument: {arg}"));
    }
    bind.map(|value| {
        value
            .parse::<SocketAddr>()
            .map_err(|error| format!("invalid --http / OKF_MCP_APPS_HTTP bind `{value}`: {error}"))
    })
    .transpose()
}

async fn serve_stdio(server: QualifyAppsServer) -> Result<(), String> {
    let running = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| format!("serve stdio: {error}"))?;
    running
        .waiting()
        .await
        .map_err(|error| format!("waiting: {error}"))?;
    Ok(())
}

async fn serve_http(server: QualifyAppsServer, addr: SocketAddr) -> Result<(), String> {
    let cancellation = CancellationToken::new();
    let mut config = StreamableHttpServerConfig::default()
        .with_cancellation_token(cancellation.clone())
        .with_json_response(true)
        // Include host:port forms so clients targeting an ephemeral local port succeed.
        .with_allowed_hosts([
            "localhost".to_owned(),
            "127.0.0.1".to_owned(),
            "::1".to_owned(),
            format!("localhost:{}", addr.port()),
            format!("127.0.0.1:{}", addr.port()),
            format!("[::1]:{}", addr.port()),
        ]);
    if tunnel_hosts_allowed(env::var(TUNNEL_ENV).ok().as_deref()) {
        // Public tunnel Host headers vary; allow any Host for the short-lived harness session.
        config = config.disable_allowed_hosts();
    }

    let factory_server = server.clone();
    let service: StreamableHttpService<QualifyAppsServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(factory_server.clone()),
            Arc::new(LocalSessionManager::default()),
            config,
        );
    let router = Router::new().nest_service("/mcp", service).layer(
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
            .expose_headers(Any),
    );
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|error| format!("bind {addr}: {error}"))?;
    let bound = listener
        .local_addr()
        .map_err(|error| format!("local_addr: {error}"))?;
    let endpoint = format!("http://{bound}/mcp");
    let _ = writeln!(io::stderr(), "okf-qualify-mcp-apps listening on {endpoint}");
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            cancellation.cancel();
        })
        .await
        .map_err(|error| format!("serve http: {error}"))?;
    Ok(())
}

fn run_check(server: &QualifyAppsServer) -> Result<(), String> {
    let report = server.check_report()?;
    let json = serde_json::to_string(&report).map_err(|error| error.to_string())?;
    writeln!(io::stdout(), "{json}").map_err(|error| error.to_string())?;
    let resources = report
        .get("resources")
        .and_then(Value::as_array)
        .ok_or_else(|| "check report missing resources".to_owned())?;
    for item in resources {
        let readable = item
            .get("readable")
            .and_then(Value::as_bool)
            .ok_or_else(|| "check report resource missing readable bool".to_owned())?;
        if !readable {
            return Err("one or more MCP App resources were not readable".to_owned());
        }
    }
    let tools = report
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| "check report missing tools".to_owned())?;
    if tools.len() != 4 {
        return Err(format!("expected 4 render tools, found {}", tools.len()));
    }
    Ok(())
}

async fn run() -> Result<(), String> {
    let check = env::args().any(|arg| arg == "--check");
    let http_bind = parse_http_bind()?;
    let dist = dist_apps_dir();
    let server = QualifyAppsServer::load(&dist)?;
    if check {
        run_check(&server)
    } else if let Some(addr) = http_bind {
        serve_http(server, addr).await
    } else {
        serve_stdio(server).await
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "okf-qualify-mcp-apps: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BundledApp, DATASET_FIXTURE, Dataset, QualifyAppsServer, READ_OBJECT_MAX_BYTES,
        READ_OBJECT_TOOL, RenderTool, SHOW_TOOL, ShowFixtures, base64_encode, fixtures_dir,
        load_fixture, read_object_block, resolved_bindings, resource_ui_meta, show_fixture,
        tool_call_record, tunnel_hosts_allowed,
    };
    use rmcp::model::{CallToolResult, ContentBlock};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn fixtures() -> Result<ShowFixtures, String> {
        let dir = fixtures_dir();
        Ok(ShowFixtures {
            present: load_fixture(&dir.join("present-response.json"))?,
            read_item: load_fixture(&dir.join("source-read-item.json"))?,
        })
    }

    fn dataset_bytes() -> Result<Vec<u8>, std::io::Error> {
        fs::read(fixtures_dir().join(DATASET_FIXTURE))
    }

    /// The `metrics` binding: the one whose dataset the present fixture retains.
    fn metrics_binding(present: &Value) -> Result<&Value, String> {
        resolved_bindings(present)
            .find(|binding| binding.get("name").and_then(Value::as_str) == Some("metrics"))
            .ok_or_else(|| "present fixture has no metrics binding".to_owned())
    }

    /// A `read_object` request as `ui/src/features/views/PresentView.tsx` sends it.
    fn read_request(binding: &Value, object: &str, offset: Option<&str>, length: u32) -> Value {
        let mut request = json!({
            "source": binding.get("source"),
            "object": object,
            "length": length,
        });
        if let (Some(offset), Some(fields)) = (offset, request.as_object_mut()) {
            fields.insert("offset".to_owned(), json!(offset));
        }
        request
    }

    fn field<'a>(value: &'a Value, name: &str) -> Result<&'a Value, String> {
        value
            .get(name)
            .ok_or_else(|| format!("{value} has no {name}"))
    }

    #[test]
    fn the_present_fixture_retains_exactly_the_dataset_fixture_bytes() -> TestResult {
        let fixtures = fixtures()?;
        let bytes = dataset_bytes()?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let dataset = Dataset::load(&fixtures_dir(), &fixtures.present)?;
        assert_eq!(dataset.sha256, digest);
        assert_eq!(dataset.bytes.as_ref(), bytes.as_slice());

        // The view document and the resolved bindings name the same retained digest.
        let view_bindings = fixtures
            .present
            .pointer("/view/bindings")
            .and_then(Value::as_array)
            .ok_or("present fixture has no view.bindings")?;
        let retained: Vec<&Value> = resolved_bindings(&fixtures.present)
            .chain(view_bindings)
            .filter_map(|binding| binding.get("materialized"))
            .collect();
        assert_eq!(retained, [&json!(digest), &json!(digest)]);
        assert_eq!(
            metrics_binding(&fixtures.present)?.get("materialized"),
            Some(&json!(digest))
        );

        // The App parses the bytes as a JSON array of flat rows.
        let rows: Vec<BTreeMap<String, Value>> = serde_json::from_slice(&bytes)?;
        assert_eq!(rows.len(), 5);
        for row in &rows {
            assert!(
                row.values()
                    .all(|cell| cell.is_string() || cell.is_number())
            );
        }
        Ok(())
    }

    #[test]
    fn a_dataset_that_differs_by_one_byte_is_not_the_retained_one() -> TestResult {
        let fixtures = fixtures()?;
        let mut bytes = dataset_bytes()?;
        assert!(Dataset::retained(bytes.clone(), &fixtures.present).is_ok());
        let last = bytes.last_mut().ok_or("dataset fixture is empty")?;
        *last ^= 1;
        let refused = Dataset::retained(bytes, &fixtures.present);
        let message = refused.err().ok_or("a corrupted dataset was accepted")?;
        assert!(message.contains("is not a digest the present fixture retains"));
        Ok(())
    }

    #[test]
    fn read_object_serves_the_whole_dataset_in_bounded_blocks() -> TestResult {
        let fixtures = fixtures()?;
        let dataset = Dataset::load(&fixtures_dir(), &fixtures.present)?;
        let binding = metrics_binding(&fixtures.present)?;
        let total = dataset.bytes.len();
        assert!(total > READ_OBJECT_MAX_BYTES);

        // The App's loop: ask for a megabyte at the running offset until has_more is false.
        let mut expected_blocks = dataset.bytes.chunks(READ_OBJECT_MAX_BYTES);
        let mut offset = 0_usize;
        let mut calls = 0_usize;
        loop {
            let request = read_request(
                binding,
                &dataset.sha256,
                Some(&offset.to_string()),
                1_048_576,
            );
            let block = read_object_block(&dataset, &fixtures.present, request.as_object())?;
            let expected = expected_blocks.next().ok_or("more blocks than bytes")?;
            assert_eq!(field(&block, "sha256")?, &json!(dataset.sha256));
            assert_eq!(field(&block, "offset")?, &json!(offset.to_string()));
            assert_eq!(field(&block, "total_size")?, &json!(total.to_string()));
            assert_eq!(field(&block, "media_type")?, &json!("application/json"));
            assert_eq!(
                field(&block, "data_base64")?,
                &json!(base64_encode(expected))
            );
            offset = offset.saturating_add(expected.len());
            calls = calls.saturating_add(1);
            assert_eq!(field(&block, "has_more")?, &json!(offset < total));
            if offset >= total {
                break;
            }
        }
        assert_eq!(offset, total);
        assert_eq!(calls, total.div_ceil(READ_OBJECT_MAX_BYTES));
        assert!(calls > 1);
        assert!(expected_blocks.next().is_none());
        Ok(())
    }

    #[test]
    fn read_object_honours_offset_and_length() -> TestResult {
        let fixtures = fixtures()?;
        let dataset = Dataset::load(&fixtures_dir(), &fixtures.present)?;
        let binding = metrics_binding(&fixtures.present)?;
        let total = dataset.bytes.len();
        let read =
            |request: &Value| read_object_block(&dataset, &fixtures.present, request.as_object());

        let ranged = read(&read_request(binding, &dataset.sha256, Some("10"), 5))?;
        let expected = dataset.bytes.get(10..15).ok_or("dataset is too short")?;
        assert_eq!(
            field(&ranged, "data_base64")?,
            &json!(base64_encode(expected))
        );
        assert_eq!(field(&ranged, "offset")?, &json!("10"));
        assert_eq!(field(&ranged, "has_more")?, &json!(true));

        // No offset, or a null one, is the start; no length is one full block.
        let first = dataset
            .bytes
            .get(..READ_OBJECT_MAX_BYTES)
            .ok_or("dataset is shorter than one block")?;
        for request in [
            json!({ "source": binding.get("source"), "object": dataset.sha256 }),
            json!({ "source": binding.get("source"), "object": dataset.sha256, "offset": null, "length": null }),
        ] {
            let block = read(&request)?;
            assert_eq!(field(&block, "offset")?, &json!("0"));
            assert_eq!(field(&block, "data_base64")?, &json!(base64_encode(first)));
        }

        // The end of the object is a valid, empty, final block; a zero length makes no progress.
        let end = read(&read_request(
            binding,
            &dataset.sha256,
            Some(&total.to_string()),
            64,
        ))?;
        assert_eq!(field(&end, "data_base64")?, &json!(""));
        assert_eq!(field(&end, "has_more")?, &json!(false));
        let empty = read(&read_request(binding, &dataset.sha256, Some("0"), 0))?;
        assert_eq!(field(&empty, "data_base64")?, &json!(""));
        assert_eq!(field(&empty, "has_more")?, &json!(true));

        let beyond = total.saturating_add(1).to_string();
        assert!(read(&read_request(binding, &dataset.sha256, Some(&beyond), 64)).is_err());
        for offset in ["", "-1", "+1", "0x10", "1.5"] {
            let request = read_request(binding, &dataset.sha256, Some(offset), 64);
            assert!(read(&request).is_err(), "offset {offset:?} was accepted");
        }
        let numeric_offset =
            json!({ "source": binding.get("source"), "object": dataset.sha256, "offset": 0 });
        assert!(read(&numeric_offset).is_err());
        let negative_length =
            json!({ "source": binding.get("source"), "object": dataset.sha256, "length": -1 });
        assert!(read(&negative_length).is_err());
        Ok(())
    }

    #[test]
    fn read_object_refuses_an_unknown_digest_and_a_source_that_does_not_retain_it() -> TestResult {
        let fixtures = fixtures()?;
        let dataset = Dataset::load(&fixtures_dir(), &fixtures.present)?;
        let binding = metrics_binding(&fixtures.present)?;
        let read =
            |request: &Value| read_object_block(&dataset, &fixtures.present, request.as_object());

        let unknown = format!("{:x}", Sha256::digest(b"not the dataset"));
        let refused = read(&read_request(binding, &unknown, Some("0"), 64));
        let message = refused.err().ok_or("an unknown digest was served")?;
        assert!(message.contains("no retained object"));

        // Knowing the digest is not enough: the venue binding does not retain the dataset.
        let venue = resolved_bindings(&fixtures.present)
            .find(|binding| binding.get("name").and_then(Value::as_str) == Some("venue"))
            .ok_or("present fixture has no venue binding")?;
        assert!(read(&read_request(venue, &dataset.sha256, Some("0"), 64)).is_err());
        assert!(read(&json!({ "object": dataset.sha256 })).is_err());
        assert!(read(&json!({ "source": binding.get("source") })).is_err());
        assert!(read_object_block(&dataset, &fixtures.present, None).is_err());
        Ok(())
    }

    #[test]
    fn read_object_is_listed_app_only_and_reports_a_refusal_as_a_tool_error() -> TestResult {
        let show = fixtures()?;
        let dataset = Dataset::load(&fixtures_dir(), &show.present)?;
        let binding = metrics_binding(&show.present)?.clone();
        let server = QualifyAppsServer {
            by_uri: BTreeMap::new(),
            dataset: dataset.clone(),
            resources: Vec::new(),
            show,
            tools: Vec::new(),
        };

        let listed = serde_json::to_value(server.tool_definitions())?;
        let tools = listed.as_array().ok_or("tool list is not an array")?;
        let names: Vec<&Value> = tools.iter().filter_map(|tool| tool.get("name")).collect();
        assert_eq!(names, [&json!(SHOW_TOOL), &json!(READ_OBJECT_TOOL)]);
        for tool in tools {
            assert_eq!(tool.pointer("/_meta/ui/visibility"), Some(&json!(["app"])));
            assert_eq!(
                tool.pointer("/annotations/readOnlyHint"),
                Some(&json!(true))
            );
        }
        let schema = tools
            .iter()
            .find(|tool| tool.get("name") == Some(&json!(READ_OBJECT_TOOL)))
            .and_then(|tool| tool.get("inputSchema"))
            .ok_or("read_object has no inputSchema")?;
        assert_eq!(schema.get("required"), Some(&json!(["source", "object"])));
        for property in ["source", "object", "offset", "length"] {
            assert!(schema.pointer(&format!("/properties/{property}")).is_some());
        }

        let first = server.call_read_object(
            read_request(&binding, &dataset.sha256, Some("0"), 1_048_576).as_object(),
        );
        assert_eq!(first.is_error, Some(false));
        assert_eq!(first.content.len(), 1, "one text fallback block");
        let block = first.structured_content.ok_or("no structured block")?;
        assert_eq!(field(&block, "has_more")?, &json!(true));

        let unknown = "0".repeat(64);
        let refused =
            server.call_read_object(read_request(&binding, &unknown, Some("0"), 64).as_object());
        assert_eq!(refused.is_error, Some(true));
        assert!(refused.structured_content.is_none());
        Ok(())
    }

    #[test]
    fn the_check_report_is_taken_from_what_the_server_answers() -> TestResult {
        let show = fixtures()?;
        let dataset = Dataset::load(&fixtures_dir(), &show.present)?;
        let app = |name: &str, bytes: &[u8]| BundledApp {
            byte_length: 0,
            bytes: Arc::from(bytes),
            mime_type: "text/html;profile=mcp-app".to_owned(),
            name: name.to_owned(),
            path: PathBuf::from(format!("{name}.html")),
            sha256: String::new(),
            uri: format!("ui://okf-jawn/{name}.html"),
        };
        let html = app("app", b"<!doctype html><div id=\"root\"></div>");
        let binary = app("binary", &[0xff, 0xfe]);
        let server = QualifyAppsServer {
            by_uri: [html.clone(), binary.clone()]
                .into_iter()
                .map(|app| (app.uri.clone(), app))
                .collect(),
            dataset,
            resources: vec![html.clone(), binary],
            tools: vec![
                RenderTool {
                    description: "present",
                    fixture: show.present.clone(),
                    name: "render_present",
                    resource_uri: html.uri.clone(),
                    summary: "text".to_owned(),
                },
                RenderTool {
                    description: "empty",
                    fixture: Value::Null,
                    name: "render_nothing",
                    resource_uri: html.uri.clone(),
                    summary: "text".to_owned(),
                },
            ],
            show,
        };
        let report = server.check_report()?;

        // The UI metadata is the one resources/read attaches, and only where a read succeeds.
        let read = serde_json::to_value(
            server
                .read_catalog(&html.uri)
                .map_err(|error| error.message)?,
        )?;
        let attached = read
            .pointer("/contents/0/_meta/ui")
            .ok_or("resources/read attached no _meta.ui")?;
        assert_eq!(Some(attached), resource_ui_meta().get("ui"));
        assert_eq!(report.pointer("/resources/0/metaUi"), Some(attached));
        assert_eq!(report.pointer("/resources/0/readable"), Some(&json!(true)));
        assert_eq!(report.pointer("/resources/1/metaUi"), Some(&Value::Null));
        assert_eq!(report.pointer("/resources/1/readable"), Some(&json!(false)));

        // Tools come from the listing; structured content from calling the tool.
        assert_eq!(
            field(&report, "app_tools")?,
            &json!([SHOW_TOOL, READ_OBJECT_TOOL])
        );
        assert_eq!(
            field(&report, "tools")?,
            &json!([
                { "name": "render_present", "resourceUri": html.uri, "hasStructuredContent": true },
                { "name": "render_nothing", "resourceUri": html.uri, "hasStructuredContent": false },
            ])
        );
        assert_eq!(field(&report, "tool_count")?, &json!(2));
        assert_eq!(field(&report, "resource_count")?, &json!(2));
        Ok(())
    }

    #[test]
    fn a_tool_call_is_reported_with_its_outcome_and_the_block_it_served() {
        let mut block = CallToolResult::success(vec![ContentBlock::text("block".to_owned())]);
        block.structured_content = Some(json!({
            "sha256": "ab", "offset": "64", "total_size": "223", "media_type": "application/json",
            "data_base64": "", "has_more": true,
        }));
        assert_eq!(
            tool_call_record(READ_OBJECT_TOOL, Some(&block)),
            json!({ "tool": "read_object", "ok": true, "object": "ab", "offset": "64", "has_more": true })
        );
        // Only read_object blocks carry a range; a render tool's content is not echoed.
        assert_eq!(
            tool_call_record("render_present", Some(&block)),
            json!({ "tool": "render_present", "ok": true })
        );
        let refused = CallToolResult::error(vec![ContentBlock::text("refused".to_owned())]);
        assert_eq!(
            tool_call_record(READ_OBJECT_TOOL, Some(&refused)),
            json!({ "tool": "read_object", "ok": false })
        );
        assert_eq!(
            tool_call_record(SHOW_TOOL, None),
            json!({ "tool": "show", "ok": false })
        );
    }

    #[test]
    fn base64_matches_the_rfc_4648_test_vectors() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(plain.as_bytes()), encoded);
        }
        // Every digit class, including the two symbols.
        assert_eq!(base64_encode(&[0xfb, 0xff, 0xbe]), "+/++");
        assert_eq!(
            base64_encode(&[0x00, 0x10, 0x83, 0x10, 0x51, 0x87]),
            "ABCDEFGH"
        );
        assert_eq!(base64_encode(&[0x69, 0xb7, 0x1d]), "abcd");
        assert_eq!(
            base64_encode(&[0xd3, 0x5d, 0xb7, 0xe3, 0x9e, 0xbb]),
            "01234567"
        );
        assert_eq!(base64_encode(&[0x67, 0x3f, 0x7f]), "Zz9/");
    }

    #[test]
    fn show_answers_each_present_binding_with_its_own_source() -> TestResult {
        let fixtures = fixtures()?;
        let bindings = fixtures
            .present
            .get("resolved_bindings")
            .and_then(Value::as_array)
            .ok_or("present fixture has no resolved_bindings")?;
        assert_eq!(bindings.len(), 2);
        for binding in bindings {
            let source = binding.get("source").ok_or("binding has no source")?;
            let arguments = json!({
                "item_id": source.get("item_id"),
                "at": { "kind": "revision", "revision": source.get("revision") },
            });
            let shown = show_fixture(&fixtures, arguments.as_object())?;
            assert_eq!(shown.get("source"), Some(source));
            assert_eq!(shown.get("markdown"), fixtures.read_item.get("markdown"));
        }
        Ok(())
    }

    #[test]
    fn show_refuses_items_and_revisions_the_present_fixture_does_not_bind() -> TestResult {
        let fixtures = fixtures()?;
        let unknown = json!({
            "item_id": "99999999-9999-4999-8999-999999999999",
            "at": { "revision": "0123456789abcdef0123456789abcdef01234567" },
        });
        assert!(show_fixture(&fixtures, unknown.as_object()).is_err());
        let moved = json!({
            "item_id": "11111111-2222-4333-8444-555555555555",
            "at": { "revision": "89abcdef0123456789abcdef0123456789abcdef" },
        });
        assert!(show_fixture(&fixtures, moved.as_object()).is_err());
        assert!(show_fixture(&fixtures, None).is_err());
        Ok(())
    }

    #[test]
    fn only_the_value_one_lifts_host_protection() {
        assert!(tunnel_hosts_allowed(Some("1")));
        for value in [Some("0"), Some("true"), Some(""), None] {
            assert!(!tunnel_hosts_allowed(value));
        }
    }
}
