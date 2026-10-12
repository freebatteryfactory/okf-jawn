//! The production MCP `ServerHandler`: every tool is an operation run through core's dispatch.
//!
//! Identity is read from the HTTP request on every call. The server's identity middleware puts
//! the authenticated `Principal` into the request's extensions, and rmcp hands the request parts
//! to the handler, so nothing about a caller outlives its request. A call without a `Principal`
//! is `Unauthenticated`. Only `AccessRoute::McpDelegation` reaches a tool: the browser session,
//! `LocalOwner` and `Service` use the HTTP API, and their calls here are `Forbidden` and recorded
//! as `permission_denied` in the tenant's log before any operation is decoded.
//!
//! Validation, authorization and the mutation ledger are dispatch's (`core::dispatch`). Nothing
//! here reads a tool's visibility or annotations to decide access: an app-only tool is
//! authorized exactly like a model tool, and dispatch refuses draft-bearing, backup, restore and
//! purge operations on this route whatever the grants.
//!
//! Results leave only through this crate's helpers: the read operation through `read_result`
//! within its request's `max_bytes`, every other success through `structured_result`, and every
//! failure through `error_result`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::request::Parts;
use okf_jawn_contract::access::{AccessRoute, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::read::ReadItemResponse;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::events::{EventLog, EventScope, NewEvent};
use okf_jawn_core::mutations::MutationStore;
use okf_jawn_core::ports::Application;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, JsonObject,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
    ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::Value;

use crate::catalog::{Catalog, operation_of_tool};
use crate::{MAX_TEXT_BYTES, error_result, read_result, structured_result};

/// The ports one MCP server holds for its whole life; the application and dispatch's own.
#[derive(Clone)]
pub struct McpPorts {
    /// The production application every tool runs.
    pub application: Arc<dyn Application>,
    /// Grant lookup for dispatch.
    pub access: Arc<dyn AccessControl>,
    /// Idempotency ledger for dispatch.
    pub mutations: Arc<dyn MutationStore>,
    /// Where refusals are recorded as `permission_denied`.
    pub events: Arc<dyn EventLog>,
}

/// The MCP server: generated tools over the application, and the App resource.
///
/// Cloning is cheap and shares everything; the Streamable HTTP service clones one per request.
#[derive(Clone)]
pub struct McpServer {
    shared: Arc<Shared>,
}

struct Shared {
    ports: McpPorts,
    catalog: Catalog,
    app_bundle: PathBuf,
}

/// What the agent sees in the host before it calls anything.
const INSTRUCTIONS: &str = "okf-jawn document workspaces. Call `workspaces` first and pin later \
     reads to a workspace's head revision. Every result carries readable text and \
     structuredContent; a failed call is text with isError set.";

impl McpServer {
    /// A server over `ports` that serves the App bundle at `app_bundle`.
    ///
    /// The bundle is read on each resource read, not here: it is build output that may be
    /// absent or rebuilt while the server runs, and tools work without it.
    ///
    /// # Errors
    /// Returns `Internal` when the committed catalogs disagree with the operation table.
    pub fn new(ports: McpPorts, app_bundle: impl Into<PathBuf>) -> Result<Self, ApiError> {
        Ok(Self {
            shared: Arc::new(Shared {
                ports,
                catalog: Catalog::load()?,
                app_bundle: app_bundle.into(),
            }),
        })
    }

    /// The tools `tools/list` serves: the generated catalog, exactly.
    #[must_use]
    pub fn tools(&self) -> &[Tool] {
        self.shared.catalog.tools()
    }

    /// The resources `resources/list` serves: the declared App, with its `_meta.ui.csp`.
    #[must_use]
    pub fn resources(&self) -> Vec<Resource> {
        vec![self.shared.catalog.app().clone()]
    }

    /// Call the tool named `tool` as `principal`, the way `tools/call` does.
    ///
    /// A name that is no operation's alias is refused as `NotFound`, also when it is a
    /// canonical operation id: only aliased operations are tools.
    ///
    /// # Errors
    /// Returns an rmcp error only when an `ApiError` cannot be serialized into its result.
    pub async fn call_tool_as(
        &self,
        principal: Option<&Principal>,
        tool: &str,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(operation) = operation_of_tool(tool) else {
            return failure(&ApiError::new(
                ErrorCode::NotFound,
                format!("No tool is named `{tool}`"),
            ));
        };
        let input = Value::Object(arguments.unwrap_or_default());
        self.call_operation(principal, operation, input).await
    }

    /// Run `operation` as `principal` with the identity rules of a tool call, then dispatch.
    ///
    /// The MCP wire reaches this only through a tool name (`call_tool_as`); naming an operation
    /// directly widens nothing, because dispatch refuses what the route may never do.
    ///
    /// # Errors
    /// Returns an rmcp error only when an `ApiError` cannot be serialized into its result.
    pub async fn call_operation(
        &self,
        principal: Option<&Principal>,
        operation: OperationName,
        input: Value,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(principal) = principal else {
            return failure(&ApiError::new(
                ErrorCode::Unauthenticated,
                "An MCP tool call needs an authenticated agent identity",
            ));
        };
        if principal.route != AccessRoute::McpDelegation {
            return failure(&self.refuse_route(principal, operation).await);
        }
        let budget = (operation == OperationName::ReadItem).then(|| read_budget(&input));
        let ports = &self.shared.ports;
        let dispatch_ports = DispatchPorts {
            access: ports.access.as_ref(),
            mutations: ports.mutations.as_ref(),
            events: ports.events.as_ref(),
        };
        let caller = Caller {
            principal,
            session_id: None,
        };
        let outcome = dispatch(
            ports.application.as_ref(),
            &dispatch_ports,
            &caller,
            operation.as_str(),
            input,
        )
        .await;
        match (outcome, budget) {
            (Ok(response), None) => Ok(structured_result(&response, MAX_TEXT_BYTES)),
            (Ok(response), Some(budget)) => read_success(response, budget),
            (Err(error), _) => failure(&error),
        }
    }

    /// Read the resource at `uri`: the App bundle's HTML, typed and constrained as declared.
    ///
    /// # Errors
    /// Returns `NotFound` for any other URI, and `Unavailable` when the bundle is missing,
    /// unreadable, not UTF-8 or empty: a host is never handed an empty App.
    pub async fn read_resource_at(&self, uri: &str) -> Result<ReadResourceResult, ApiError> {
        let catalog = &self.shared.catalog;
        let app = catalog.app();
        if uri != app.uri {
            return Err(ApiError::new(
                ErrorCode::NotFound,
                format!("No resource is served at `{uri}`"),
            ));
        }
        let html = read_bundle(self.shared.app_bundle.clone()).await?;
        let mut contents =
            ResourceContents::text(html, app.uri.clone()).with_mime_type(catalog.app_mime_type());
        if let Some(meta) = app.meta.clone() {
            contents = contents.with_meta(meta);
        }
        Ok(ReadResourceResult::new(vec![contents]))
    }

    /// Record a non-agent route's tool call as `permission_denied` and return its refusal.
    ///
    /// The refusal comes before any request is decoded, so it names no workspace and is
    /// recorded in the tenant's log, as dispatch records a refusal that is not about one
    /// workspace. A failed append does not change the answer.
    async fn refuse_route(&self, principal: &Principal, operation: OperationName) -> ApiError {
        let scope = EventScope::Tenant(principal.tenant_id.clone());
        let _recorded = self
            .shared
            .ports
            .events
            .append(
                &scope,
                None,
                NewEvent::permission_denied(principal, operation),
            )
            .await;
        ApiError::new(
            ErrorCode::Forbidden,
            "Only an MCP delegation reaches a tool; this route uses the HTTP API",
        )
    }
}

impl ServerHandler for McpServer {
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.call_tool_as(
            request_principal(&context),
            &request.name,
            request.arguments,
        )
        .await
        .map(CallToolResponse::from)
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_resources()
                .enable_tools()
                .build(),
        )
        .with_server_info(Implementation::new("okf-jawn", env!("CARGO_PKG_VERSION")))
        .with_instructions(INSTRUCTIONS)
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools().iter().find(|tool| tool.name == name).cloned()
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListResourcesResult::with_all_items(self.resources())))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(self.tools().to_vec())))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.read_resource_at(&request.uri)
            .await
            .map(ReadResourceResponse::from)
            .map_err(|error| resource_error(&error))
    }
}

/// The `Principal` the server's identity middleware put on this request, if any.
fn request_principal(context: &RequestContext<RoleServer>) -> Option<&Principal> {
    context
        .extensions
        .get::<Parts>()
        .and_then(|parts| parts.extensions.get::<Principal>())
}

/// The read's own text budget: its `max_bytes`, never more than a helper writes.
///
/// `None` when the input has no usable `max_bytes`; dispatch then refuses the input, so a read
/// that succeeded always has one.
fn read_budget(input: &Value) -> Option<usize> {
    input
        .get("max_bytes")
        .and_then(Value::as_u64)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .map(|bytes| bytes.min(MAX_TEXT_BYTES))
}

/// A successful read, through `read_result` within the request's budget.
fn read_success(response: Value, budget: Option<usize>) -> Result<CallToolResult, ErrorData> {
    let Some(budget) = budget else {
        return failure(&ApiError::new(
            ErrorCode::Internal,
            "A read succeeded without a byte budget",
        ));
    };
    let decoded = serde_json::from_value::<ReadItemResponse>(response).map_err(|_| {
        ApiError::new(
            ErrorCode::Internal,
            "The read response does not decode as a read response",
        )
    });
    match decoded.and_then(|response| {
        read_result(&response, budget).map_err(|_| {
            ApiError::new(
                ErrorCode::Internal,
                "The read response cannot be serialized",
            )
        })
    }) {
        Ok(result) => Ok(result),
        Err(error) => failure(&error),
    }
}

/// A failed call: the serialized `ApiError` as text with `isError`, through `error_result`.
fn failure(error: &ApiError) -> Result<CallToolResult, ErrorData> {
    error_result(error)
        .map_err(|_| ErrorData::internal_error("A tool failure could not be serialized", None))
}

/// A failed resource read as a JSON-RPC error whose `data` is the serialized `ApiError`.
fn resource_error(error: &ApiError) -> ErrorData {
    let data = serde_json::to_value(error).ok();
    if error.code == ErrorCode::NotFound {
        ErrorData::resource_not_found(error.message.clone(), data)
    } else {
        ErrorData::internal_error(error.message.clone(), data)
    }
}

/// The App bundle's HTML, read off the async runtime.
async fn read_bundle(path: PathBuf) -> Result<String, ApiError> {
    let read = tokio::task::spawn_blocking(move || std::fs::read_to_string(path))
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal, "The App bundle read did not finish"))?;
    match read {
        Ok(html) if !html.trim().is_empty() => Ok(html),
        Ok(_) => Err(ApiError::new(
            ErrorCode::Unavailable,
            "The MCP App bundle is empty; build the Apps bundle",
        )),
        Err(error) => Err(ApiError::new(
            ErrorCode::Unavailable,
            format!(
                "The MCP App bundle is not readable ({}); build the Apps bundle",
                error.kind()
            ),
        )),
    }
}
