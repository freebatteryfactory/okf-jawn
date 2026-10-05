//! Phase 0 MCP Apps qualification harness.
//!
//! Serves built `ui/dist-apps` HTML bundles as MCP UI resources, four read-only
//! render tools and one app-only `show` fixture tool (the present view resolves its
//! bindings through it) over stdio or Streamable HTTP via `rmcp`. This is not the
//! product `ServerHandler` and must not be linked from product crates.

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

const SHOW_DESCRIPTION: &str = "App-only fixture read: returns the source fixture text under the source reference of the present-fixture binding that cites the requested item.";
const SHOW_TOOL: &str = "show";
const TUNNEL_ENV: &str = "OKF_MCP_APPS_NGROK";

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
        let tool_specs = [
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
        let mut tools = Vec::with_capacity(4);
        for (name, description, fixture_name, summary) in tool_specs {
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

        Ok(Self {
            by_uri,
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

    fn check_report(&self) -> Value {
        let resources: Vec<Value> = self
            .resources
            .iter()
            .map(|app| {
                let text = String::from_utf8(app.bytes.as_ref().to_vec()).ok();
                let readable = text.as_ref().is_some_and(|body| {
                    let lower = body.to_ascii_lowercase();
                    lower.starts_with("<!doctype html") || lower.contains("id=\"root\"")
                });
                json!({
                    "name": app.name,
                    "uri": app.uri,
                    "mimeType": app.mime_type,
                    "byteLength": app.byte_length,
                    "sha256": app.sha256,
                    "readable": readable,
                    "path": app.path,
                    "metaUi": {
                        "csp": {
                            "connectDomains": [],
                            "resourceDomains": []
                        },
                        "prefersBorder": true
                    }
                })
            })
            .collect();
        let tools: Vec<Value> = self
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "resourceUri": tool.resource_uri,
                    "hasStructuredContent": !tool.fixture.is_null(),
                })
            })
            .collect();
        json!({
            "mode": "check",
            "resource_count": resources.len(),
            "tool_count": tools.len(),
            "resources": resources,
            "tools": tools,
            "app_tools": [SHOW_TOOL],
        })
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
        } else {
            self.call_render_tool(&request.name)
        };
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
    let source = fixtures
        .present
        .get("resolved_bindings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
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
    let report = server.check_report();
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
    use super::{ShowFixtures, fixtures_dir, load_fixture, show_fixture, tunnel_hosts_allowed};
    use serde_json::{Value, json};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn fixtures() -> Result<ShowFixtures, String> {
        let dir = fixtures_dir();
        Ok(ShowFixtures {
            present: load_fixture(&dir.join("present-response.json"))?,
            read_item: load_fixture(&dir.join("source-read-item.json"))?,
        })
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
