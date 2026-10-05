//! Phase 0 MCP Apps qualification harness.
//!
//! Serves built `ui/dist-apps` HTML bundles as MCP UI resources over stdio via
//! `rmcp`. This is not the product `ServerHandler` and must not be linked from
//! product crates.

use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    ErrorData as McpError, Implementation, ListResourcesResult, MetaObject, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServiceExt};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::future::Future;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestResource {
    byte_length: u64,
    csp: String,
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
    csp: String,
    mime_type: String,
    name: String,
    path: PathBuf,
    sha256: String,
    uri: String,
}

#[derive(Clone)]
struct QualifyAppsServer {
    by_uri: BTreeMap<String, BundledApp>,
    resources: Vec<BundledApp>,
}

impl QualifyAppsServer {
    fn load(dist_apps: &Path) -> Result<Self, String> {
        let manifest_path = dist_apps.join("manifest.json");
        let raw = fs::read_to_string(&manifest_path)
            .map_err(|error| format!("read {}: {error}", manifest_path.display()))?;
        let manifest: Manifest = serde_json::from_str(&raw)
            .map_err(|error| format!("parse {}: {error}", manifest_path.display()))?;
        if manifest.resources.len() != 4 {
            return Err(format!(
                "manifest must list exactly 4 resources, found {}",
                manifest.resources.len()
            ));
        }
        let mut resources = Vec::with_capacity(4);
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
            let app = BundledApp {
                byte_length,
                bytes: Arc::from(bytes.into_boxed_slice()),
                csp: entry.csp,
                mime_type: entry.mime_type,
                name: entry.name,
                path,
                sha256: digest,
                uri: entry.uri,
            };
            by_uri.insert(app.uri.clone(), app.clone());
            resources.push(app);
        }
        Ok(Self { by_uri, resources })
    }

    fn ui_meta(uri: &str, csp: &str) -> MetaObject {
        let mut meta = MetaObject::new();
        meta.insert(
            "ui".to_owned(),
            json!({
                "resourceUri": uri,
                "csp": csp,
            }),
        );
        meta
    }

    fn list_catalog(&self) -> ListResourcesResult {
        let listed = self
            .resources
            .iter()
            .map(|app| {
                Resource::new(app.uri.clone(), app.name.clone())
                    .with_mime_type(app.mime_type.clone())
                    .with_size(app.byte_length)
                    .with_meta(Self::ui_meta(&app.uri, &app.csp))
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
            .with_meta(Self::ui_meta(&app.uri, &app.csp));
        Ok(ReadResourceResult::new(vec![contents]))
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
                })
            })
            .collect();
        json!({
            "mode": "check",
            "resource_count": resources.len(),
            "resources": resources,
        })
    }
}

impl ServerHandler for QualifyAppsServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_resources().build())
            .with_server_info(Implementation::new(
                "okf-qualify-mcp-apps",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Phase 0 MCP Apps qualification harness. Serves built ui/dist-apps HTML bundles only.",
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
}

fn dist_apps_dir() -> PathBuf {
    env::var_os("OKF_MCP_APPS_DIST").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist-apps"),
        PathBuf::from,
    )
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
    Ok(())
}

async fn run() -> Result<(), String> {
    let check = env::args().any(|arg| arg == "--check");
    let dist = dist_apps_dir();
    let server = QualifyAppsServer::load(&dist)?;
    if check {
        run_check(&server)
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
