//! The production MCP `ServerHandler`: every tool is an operation run through core's dispatch.
//!
//! Identity is read from the HTTP request on every request. The server's identity middleware
//! puts the authenticated `Principal` into the request's extensions, and rmcp hands the request
//! parts to the handler, so nothing about a caller outlives its request. Every MCP request the
//! handler answers with this server's information or data (`initialize`, `server/discover`,
//! `tools/list`, `tools/call`, `resources/list`, `resources/read`) passes one check, `admit`,
//! before anything else is looked at: no `Principal` is `Unauthenticated`, and a route
//! other than `AccessRoute::McpDelegation` is `Forbidden`. The browser session, `LocalOwner` and
//! `Service` use the HTTP API; their refusal here is recorded as `permission_denied` in the
//! tenant's log before anything is decoded.
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
use okf_jawn_contract::events::{EventActor, EventKind};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::read::ReadItemResponse;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::events::{EventLog, EventScope, NewEvent};
use okf_jawn_core::mutations::MutationStore;
use okf_jawn_core::ports::Application;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, DiscoverResult, Implementation,
    InitializeRequestParams, InitializeResult, JsonObject, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    Resource, ResourceContents, ServerCapabilities, ServerConfig, Tool,
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
    /// Identity is checked before the name is looked up, so an unidentified caller learns
    /// nothing about tool names and a refused route is recorded whatever it asked for. A name
    /// that is no operation's alias is then refused as `NotFound`, also when it is a canonical
    /// operation id: only aliased operations are tools.
    ///
    /// # Errors
    /// Returns an rmcp error only when an `ApiError` cannot be serialized into its result.
    pub async fn call_tool_as(
        &self,
        principal: Option<&Principal>,
        tool: &str,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, ErrorData> {
        let named = operation_of_tool(tool);
        if let Err(refusal) = self.admit(principal, named).await {
            return failure(&refusal);
        }
        let Some(operation) = named else {
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
    /// Crate-private: the only way in from outside is a tool name (`call_tool_as`). The route
    /// rule is applied here, not on the tool-name path, so it holds for every caller.
    async fn call_operation(
        &self,
        principal: Option<&Principal>,
        operation: OperationName,
        input: Value,
    ) -> Result<CallToolResult, ErrorData> {
        let principal = match self.admit(principal, Some(operation)).await {
            Ok(principal) => principal,
            Err(refusal) => return failure(&refusal),
        };
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

    /// Read the resource at `uri` as `principal`, the way `resources/read` does: the App
    /// bundle's HTML, typed and constrained as declared.
    ///
    /// # Errors
    /// Returns `Unauthenticated` or `Forbidden` from the identity check, `NotFound` for any
    /// other URI, and `Unavailable` when the bundle is missing, unreadable, not UTF-8 or empty:
    /// a host is never handed an empty App.
    pub async fn read_resource_as(
        &self,
        principal: Option<&Principal>,
        uri: &str,
    ) -> Result<ReadResourceResult, ApiError> {
        self.admit(principal, None).await?;
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

    /// The check every MCP request passes: an authenticated principal on the agent route.
    ///
    /// `operation` is the operation a tool call names; `None` for the requests that are no
    /// operation (`initialize`, the two listings and the App read).
    ///
    /// # Errors
    /// Returns `Unauthenticated` without a principal, and `Forbidden` for any route other than
    /// `McpDelegation`, after recording the refusal (`refuse_route`).
    async fn admit<'caller>(
        &self,
        principal: Option<&'caller Principal>,
        operation: Option<OperationName>,
    ) -> Result<&'caller Principal, ApiError> {
        let Some(principal) = principal else {
            return Err(ApiError::new(
                ErrorCode::Unauthenticated,
                "An MCP request needs an authenticated agent identity",
            ));
        };
        if principal.route != AccessRoute::McpDelegation {
            return Err(self.refuse_route(principal, operation).await);
        }
        Ok(principal)
    }

    /// Record a non-agent route's request as `permission_denied` and return its refusal.
    ///
    /// The refusal comes before any request is decoded, so it is recorded in the tenant's log.
    /// Core's `denial_scope`, which records a refusal of a request about exactly one workspace
    /// in that workspace's log, is private to `core::dispatch`; this crate cannot apply it
    /// without changing core, so a refusal here always goes to the tenant log, which
    /// `list_tenant_events` reads. A failed append does not change the answer.
    async fn refuse_route(
        &self,
        principal: &Principal,
        operation: Option<OperationName>,
    ) -> ApiError {
        let scope = EventScope::Tenant(principal.tenant_id.clone());
        let _recorded = self
            .shared
            .ports
            .events
            .append(&scope, None, refusal_event(principal, operation))
            .await;
        ApiError::new(
            ErrorCode::Forbidden,
            "Only an MCP delegation reaches this server; this route uses the HTTP API",
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

    async fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, ErrorData> {
        self.admit(request_principal(&context), None)
            .await
            .map_err(|refusal| request_error(&refusal))?;
        Ok(DiscoverResult::from_server_info(
            self.supported_protocol_versions().into_owned(),
            self.get_info(),
        ))
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        self.admit(request_principal(&context), None)
            .await
            .map_err(|refusal| request_error(&refusal))?;
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.admit(request_principal(&context), None)
            .await
            .map_err(|refusal| request_error(&refusal))?;
        Ok(ListResourcesResult::with_all_items(self.resources()))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.admit(request_principal(&context), None)
            .await
            .map_err(|refusal| request_error(&refusal))?;
        Ok(ListToolsResult::with_all_items(self.tools().to_vec()))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.read_resource_as(request_principal(&context), &request.uri)
            .await
            .map(ReadResourceResponse::from)
            .map_err(|error| request_error(&error))
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

/// A refused request that is not a tool call, as a JSON-RPC error whose `data` is the
/// serialized `ApiError`: an identity refusal is an invalid request, an unknown resource is
/// `resource_not_found`, anything else an internal error.
fn request_error(error: &ApiError) -> ErrorData {
    let data = serde_json::to_value(error).ok();
    match error.code {
        ErrorCode::NotFound => ErrorData::resource_not_found(error.message.clone(), data),
        ErrorCode::Unauthenticated | ErrorCode::Forbidden => {
            ErrorData::invalid_request(error.message.clone(), data)
        }
        _ => ErrorData::internal_error(error.message.clone(), data),
    }
}

/// The `permission_denied` event of a route refusal, naming the operation when there is one.
fn refusal_event(principal: &Principal, operation: Option<OperationName>) -> NewEvent {
    operation.map_or_else(
        || NewEvent {
            kind: EventKind::PermissionDenied,
            revision: None,
            item_id: None,
            job_id: None,
            connector_id: None,
            actor: Some(EventActor {
                subject: principal.subject.clone(),
                route: principal.route.clone(),
                client_id: principal.client_id.clone(),
            }),
            operation: None,
        },
        |operation| NewEvent::permission_denied(principal, operation),
    )
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

#[cfg(test)]
mod tests {
    //! `call_operation` is crate-private, so the rules it applies itself are tested here.
    //!
    //! Source files may not include the shared test idiom (the foundation policy test), so these
    //! tests return `Result` with local helpers. No call here may reach the application: every
    //! port refuses as `Internal`, except the event log, which records.

    use std::error::Error;
    use std::sync::{Arc, Mutex};

    use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
    use okf_jawn_contract::error::{ApiError, ErrorCode};
    use okf_jawn_contract::events::{Event, ListEventsResponse};
    use okf_jawn_contract::identity::{Digest, MutationId, TenantId, WorkspaceId};
    use okf_jawn_contract::metadata::OperationName;
    use okf_jawn_core::access::AccessControl;
    use okf_jawn_core::context::{OperationContext, TenantGrant, WorkspaceGrant};
    use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
    use okf_jawn_core::mutations::{BeginOutcome, MutationKey, MutationLease, MutationStore};
    use okf_jawn_core::ports::{Application, PortFuture};
    use okf_jawn_core::storage::StorageScope;
    use rmcp::model::CallToolResult;
    use serde_json::{Value, json};

    use super::{McpPorts, McpServer};

    macro_rules! refusing_operations {
        ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
            $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
            $destructive:literal, $description:literal)),* $(,)?) => {
            impl Application for Refusing {
                $(fn $id<'a>(&'a self, _context: &'a OperationContext, _request: $request)
                    -> PortFuture<'a, $response> {
                    Box::pin(async { Err(not_reached()) })
                })*
            }
        };
    }

    type Checked = Result<(), Box<dyn Error>>;

    /// Every port call refused: these tests must never get past authorization.
    struct Refusing;

    /// An event log that records each append.
    #[derive(Default)]
    struct Recording {
        appended: Mutex<Vec<(EventScope, NewEvent)>>,
    }

    /// The refusal of a route other than the agent's, before anything is decoded.
    const ROUTE_REFUSAL: &str =
        "Only an MCP delegation reaches this server; this route uses the HTTP API";

    /// Dispatch's refusal of a draft-bearing operation on a route that is no human session.
    const DRAFT_REFUSAL: &str = "Drafts are visible only in a human browser session";

    okf_jawn_contract::for_each_operation!(refusing_operations);

    impl AccessControl for Refusing {
        fn authorize<'a>(
            &'a self,
            _principal: &'a Principal,
            _workspace: WorkspaceId,
            _permission: Permission,
        ) -> PortFuture<'a, WorkspaceGrant> {
            Box::pin(async { Err(not_reached()) })
        }

        fn authorize_tenant<'a>(
            &'a self,
            _principal: &'a Principal,
            _permission: Permission,
        ) -> PortFuture<'a, TenantGrant> {
            Box::pin(async { Err(not_reached()) })
        }

        fn grants<'a>(&'a self, _principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
            Box::pin(async { Err(not_reached()) })
        }

        fn grant_creator<'a>(
            &'a self,
            _principal: &'a Principal,
            _workspace: WorkspaceId,
        ) -> PortFuture<'a, WorkspaceGrant> {
            Box::pin(async { Err(not_reached()) })
        }

        fn editors<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Vec<String>> {
            Box::pin(async { Err(not_reached()) })
        }
    }

    impl MutationStore for Refusing {
        fn begin<'a>(
            &'a self,
            _key: &'a MutationKey,
            _digest: &'a Digest,
        ) -> PortFuture<'a, BeginOutcome> {
            Box::pin(async { Err(not_reached()) })
        }

        fn complete(&self, _lease: MutationLease, _response: Value) -> PortFuture<'_, ()> {
            Box::pin(async { Err(not_reached()) })
        }

        fn release(&self, _lease: MutationLease) -> PortFuture<'_, ()> {
            Box::pin(async { Err(not_reached()) })
        }
    }

    impl Recording {
        fn appended(&self) -> Result<Vec<(EventScope, NewEvent)>, String> {
            Ok(self
                .appended
                .lock()
                .map_err(|_| "the event record is poisoned")?
                .clone())
        }
    }

    impl EventLog for Recording {
        /// Records the event; the answer is a refusal, which no caller of `append` acts on.
        fn append<'a>(
            &'a self,
            scope: &'a EventScope,
            _mutation_id: Option<MutationId>,
            event: NewEvent,
        ) -> PortFuture<'a, Event> {
            Box::pin(async move {
                self.appended
                    .lock()
                    .map_err(|_| not_reached())?
                    .push((scope.clone(), event));
                Err(not_reached())
            })
        }

        fn list<'a>(
            &'a self,
            _scope: &'a EventScope,
            _query: EventQuery,
        ) -> PortFuture<'a, ListEventsResponse> {
            Box::pin(async { Err(not_reached()) })
        }
    }

    fn not_reached() -> ApiError {
        ApiError::new(
            ErrorCode::Internal,
            "No call reaches this port in these tests",
        )
    }

    fn server(events: Arc<Recording>) -> Result<McpServer, ApiError> {
        McpServer::new(
            McpPorts {
                application: Arc::new(Refusing),
                access: Arc::new(Refusing),
                mutations: Arc::new(Refusing),
                events,
            },
            "no-bundle.html",
        )
    }

    fn principal(route: AccessRoute) -> Result<Principal, Box<dyn Error>> {
        Ok(Principal {
            subject: "owner".to_owned(),
            tenant_id: TenantId::try_from("tenant-test".to_owned()).map_err(|error| error.0)?,
            route,
            client_id: Some("client-test".to_owned()),
            delegation: None,
        })
    }

    fn list_drafts() -> Value {
        json!({"workspace_id": "11111111-1111-4111-8111-111111111111", "page": {"limit": 10}})
    }

    /// The `ApiError` of a failed tool result shaped as `error_result` shapes it.
    fn refusal(result: &CallToolResult) -> Result<ApiError, Box<dyn Error>> {
        if result.is_error != Some(true) || result.structured_content.is_some() {
            return Err(format!("expected error_result text, got {result:?}").into());
        }
        let block = result.content.first().ok_or("no content block")?;
        let text = block.as_text().ok_or("the first block is not text")?;
        Ok(serde_json::from_str(&text.text)?)
    }

    #[tokio::test]
    async fn a_draft_bearing_operation_named_through_the_handler_is_forbidden() -> Checked {
        let events = Arc::new(Recording::default());
        let server = server(events.clone())?;
        let agent = principal(AccessRoute::McpDelegation)?;
        let result = server
            .call_operation(Some(&agent), OperationName::ListDrafts, list_drafts())
            .await?;
        let refused = refusal(&result)?;
        assert_eq!(refused.code, ErrorCode::Forbidden);
        assert_eq!(refused.message, DRAFT_REFUSAL);
        let appended = events.appended()?;
        assert_eq!(appended.len(), 1, "dispatch records its refusal once");
        Ok(())
    }

    #[tokio::test]
    async fn call_operation_applies_the_route_rule_itself() -> Checked {
        let events = Arc::new(Recording::default());
        let server = server(events.clone())?;
        let routes = [
            AccessRoute::BrowserSession,
            AccessRoute::LocalOwner,
            AccessRoute::Service,
        ];
        for route in routes {
            let caller = principal(route.clone())?;
            let result = server
                .call_operation(Some(&caller), OperationName::ListDrafts, list_drafts())
                .await?;
            let refused = refusal(&result)?;
            assert_eq!(refused.code, ErrorCode::Forbidden, "{route:?}");
            assert_eq!(refused.message, ROUTE_REFUSAL, "{route:?}");
        }
        let appended = events.appended()?;
        assert_eq!(appended.len(), 3);
        for (scope, event) in &appended {
            assert!(matches!(scope, EventScope::Tenant(_)), "{scope:?}");
            assert_eq!(event.operation, Some(OperationName::ListDrafts));
        }
        Ok(())
    }
}
