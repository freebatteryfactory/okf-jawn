//! The production MCP `ServerHandler`: every tool is an operation run through core's dispatch.
//!
//! Identity is read from the HTTP request on every request. The server's identity middleware
//! puts the authenticated `Principal` into the request's extensions, and rmcp hands the request
//! parts to the handler, so nothing about a caller outlives its request. Every MCP request the
//! handler answers with this server's information or data (`initialize`, `server/discover`,
//! `tools/list`, `tools/call`, `resources/list`, `resources/read`) passes one check, `admit`,
//! before anything else is looked at: no `Principal` is `Unauthenticated`, and a route
//! other than `AccessRoute::McpDelegation` is `Forbidden`. The browser session, `LocalOwner` and
//! `Service` use the HTTP API; their refusal here is recorded as `permission_denied` before the
//! request is validated or run. A refused tool call is recorded by core's rule for a refusal a
//! binding makes itself (`core::dispatch::record_refusal`), so it lands in the log dispatch's
//! own route refusal of the same request lands in; a refused request that is no operation is
//! recorded in the tenant's log.
//!
//! Validation, authorization and the mutation ledger are dispatch's (`core::dispatch`). Nothing
//! here reads a tool's visibility or annotations to decide access: an app-only tool is
//! authorized exactly like a model tool, and dispatch refuses draft-bearing, backup, restore and
//! purge operations on this route whatever the grants.
//!
//! Results leave only through this crate's helpers: the read operation through `read_result`
//! within its request's `max_bytes`, every other success through `structured_result`, and every
//! failure through `error_result`.
//!
//! A read's selected images are fetched after the read succeeds, each through `read_object`
//! (`get_object`) by dispatch as the read's own principal, so each fetch is authorized like any
//! other call and a refusal is reported, never worked around. An image is inlined only as a
//! raster type, and only when its bytes begin with the signature of the type they were served
//! as; the type is never guessed from the bytes. A fetch that fails, is refused or cannot be
//! inlined never fails the read: that image is described in the text instead.

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::request::Parts;
use okf_jawn_contract::access::{AccessRoute, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{EventActor, EventKind};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::read::{MediaReference, ReadItemResponse, Selection};
use okf_jawn_contract::source::{GetObjectRequest, GetObjectResponse, SourceReference};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch, record_refusal};
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
use crate::{MAX_TEXT_BYTES, NotInlined, ReadImage, error_result, read_result, structured_result};

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

/// Whether an object's leading bytes carry one raster format's signature.
type Signature = fn(&[u8]) -> bool;

/// What the agent sees in the host before it calls anything.
const INSTRUCTIONS: &str = "okf-jawn document workspaces. Call `workspaces` first and pin later \
     reads to a workspace's head revision. Every result carries readable text and \
     structuredContent; a failed call is text with isError set.";

/// The only media types a read's selected image is inlined as, each with the check that an
/// object's leading bytes carry that format's signature: raster formats, which carry no active
/// content. SVG can carry script and the document is untrusted input, so it and every other
/// type, `image/*` or not, are described in the text instead. A type is inlined only through
/// its own check here, so a type without a signature check cannot be listed.
const INLINE_IMAGE_TYPES: [(&str, Signature); 4] = [
    ("image/png", is_png),
    ("image/jpeg", is_jpeg),
    ("image/gif", is_gif),
    ("image/webp", is_webp),
];

/// The eight bytes a PNG file begins with.
const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// The three bytes a JPEG file begins with.
const JPEG_SIGNATURE: [u8; 3] = [0xFF, 0xD8, 0xFF];

/// The leading bytes the longest signature spans: WebP's `RIFF`, four bytes of size, `WEBP`.
const SIGNATURE_SPAN: usize = 12;

/// The base64 characters that encode the first `SIGNATURE_SPAN` bytes: four for every three.
const SIGNATURE_CHARACTERS: usize = 16;

/// The standard base64 alphabet (RFC 4648 section 4); a character's place is its six bits.
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

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
    /// nothing about tool names and a refused route is recorded whatever it asked for: with
    /// the operation and its arguments when the name is a tool, as a request that is no
    /// operation otherwise. A name that is no operation's alias is then refused as `NotFound`,
    /// also when it is a canonical operation id: only aliased operations are tools.
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
        let input = Value::Object(arguments.unwrap_or_default());
        let asked = named.map(|operation| (operation, &input));
        if let Err(refusal) = self.admit(principal, asked).await {
            return failure(&refusal);
        }
        let Some(operation) = named else {
            return failure(&ApiError::new(
                ErrorCode::NotFound,
                format!("No tool is named `{tool}`"),
            ));
        };
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
        let principal = match self.admit(principal, Some((operation, &input))).await {
            Ok(principal) => principal,
            Err(refusal) => return failure(&refusal),
        };
        let budget = (operation == OperationName::ReadItem).then(|| read_budget(&input));
        match (self.run(principal, operation, input).await, budget) {
            (Ok(response), None) => Ok(structured_result(&response, MAX_TEXT_BYTES)),
            (Ok(response), Some(budget)) => self.read_success(principal, response, budget).await,
            (Err(error), _) => failure(&error),
        }
    }

    /// Dispatch `operation` as `principal`, already admitted: validation, authorization and
    /// the ledger are dispatch's.
    ///
    /// # Errors
    /// Returns dispatch's error.
    async fn run(
        &self,
        principal: &Principal,
        operation: OperationName,
        input: Value,
    ) -> Result<Value, ApiError> {
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
        dispatch(
            ports.application.as_ref(),
            &dispatch_ports,
            &caller,
            operation.as_str(),
            input,
        )
        .await
    }

    /// A successful read, through `read_result` within the request's budget, with its selected
    /// images (`selected_images`).
    ///
    /// # Errors
    /// Returns an rmcp error only when an `ApiError` cannot be serialized into its result.
    async fn read_success(
        &self,
        principal: &Principal,
        response: Value,
        budget: Option<usize>,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(budget) = budget else {
            return failure(&ApiError::new(
                ErrorCode::Internal,
                "A read succeeded without a byte budget",
            ));
        };
        let Ok(response) = serde_json::from_value::<ReadItemResponse>(response) else {
            return failure(&ApiError::new(
                ErrorCode::Internal,
                "The read response does not decode as a read response",
            ));
        };
        let images = self.selected_images(principal, &response, budget).await;
        match read_result(&response, budget, &images) {
            Ok(result) => Ok(result),
            Err(_) => failure(&ApiError::new(
                ErrorCode::Internal,
                "The read response cannot be serialized",
            )),
        }
    }

    /// What becomes of each image the read selected, in media order. Core has already applied
    /// `max_images`, so every entry of `media` is one the caller asked for.
    ///
    /// An image is inlined when its media type, as declared and as served, is an inline raster
    /// type (`INLINE_IMAGE_TYPES`), its fetch is complete (`has_more` false), it fits what
    /// is left of the image budget and its bytes begin with the signature of the type they
    /// were served as. The block names its type by the canonical essence of the served type,
    /// never by the served string. Bytes that are another type are described, not shown under
    /// the type they claim and not sniffed to the type they might be. The image budget is the
    /// read's own `budget` (its `max_bytes`, capped at `MAX_TEXT_BYTES`), counted again and
    /// separately from the text's: the text and the images may each use all of it. It counts
    /// image bytes, the decoded size of each inlined image, so the base64 on the wire is about
    /// four thirds of it. Each fetch asks for no more than is left, so a complete answer from
    /// the application always fits; the size check guards the rest.
    async fn selected_images<'read>(
        &self,
        principal: &Principal,
        response: &'read ReadItemResponse,
        budget: usize,
    ) -> Vec<ReadImage<'read>> {
        let mut left = budget;
        let mut images = Vec::with_capacity(response.media.len());
        for media in &response.media {
            images.push(
                match self
                    .fetch_image(principal, &response.source, media, left)
                    .await
                {
                    Ok((image, size)) => {
                        left = left.saturating_sub(size);
                        image
                    }
                    Err(reason) => ReadImage::NotInlined { media, reason },
                },
            );
        }
        images
    }

    /// One selected image, inlined with its decoded size, or the reason it is not.
    ///
    /// A media type the read declares as no inline image type is not fetched, nor is any image
    /// once the budget is spent.
    ///
    /// The checks run in this order, and the first that fails is the reason given: the
    /// declared type, the budget, the fetch, the served type, the size, the signature. So an
    /// object that is too large is reported as too large whatever its bytes are.
    ///
    /// # Errors
    /// Returns why the image is not inlined: not an inline image type (declared or served),
    /// too large, the fetch refused with its `ApiError` code, or bytes that do not begin with
    /// the signature of the type they were served as.
    async fn fetch_image(
        &self,
        principal: &Principal,
        read: &SourceReference,
        media: &MediaReference,
        left: usize,
    ) -> Result<(ReadImage<'static>, usize), NotInlined> {
        if !is_inline_image_type(&media.media_type) {
            return Err(NotInlined::NotInlineType {
                media_type: media.media_type.clone(),
            });
        }
        if left == 0 {
            return Err(NotInlined::TooLarge { size: None, left });
        }
        let block = self
            .fetch_object(principal, read, media, left)
            .await
            .map_err(|refusal| NotInlined::Refused { code: refusal.code })?;
        let Some((media_type, signature)) = inline_type(&block.media_type) else {
            return Err(NotInlined::NotInlineType {
                media_type: block.media_type,
            });
        };
        let size = decoded_size(&block.data_base64);
        if block.has_more || size > left {
            return Err(NotInlined::TooLarge {
                size: Some(block.total_size),
                left,
            });
        }
        if !signature(&leading_bytes(&block.data_base64)) {
            return Err(NotInlined::NotItsType {
                media_type: block.media_type,
            });
        }
        Ok((
            ReadImage::Inline {
                data_base64: block.data_base64,
                mime_type: media_type.to_owned(),
            },
            size,
        ))
    }

    /// Fetch an image's bytes through `read_object` (`get_object`), never from a store.
    ///
    /// The request cites the read's own resolution: its workspace, item, path and resolved
    /// revision, with the image's digest as the citation's digest and the object. Locations are
    /// omitted, so the server fills them; a caller never supplies one. It asks for at most
    /// `left` bytes from the start of the object.
    ///
    /// # Errors
    /// Returns dispatch's refusal or the application's error, or `Internal` when the request
    /// or the response does not convert.
    async fn fetch_object(
        &self,
        principal: &Principal,
        read: &SourceReference,
        media: &MediaReference,
        left: usize,
    ) -> Result<GetObjectResponse, ApiError> {
        let request = GetObjectRequest {
            source: SourceReference {
                workspace_id: read.workspace_id,
                item_id: read.item_id,
                path: read.path.clone(),
                revision: read.revision.clone(),
                digest: Some(media.object.clone()),
                selection: Selection::All,
                locations: Vec::new(),
            },
            object: media.object.clone(),
            offset: None,
            length: Some(u32::try_from(left).unwrap_or(u32::MAX)),
        };
        let input = serde_json::to_value(&request).map_err(|_| {
            ApiError::new(
                ErrorCode::Internal,
                "An object request cannot be serialized",
            )
        })?;
        let response = self.run(principal, OperationName::GetObject, input).await?;
        serde_json::from_value(response).map_err(|_| {
            ApiError::new(
                ErrorCode::Internal,
                "The object response does not decode as an object response",
            )
        })
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
    /// `asked` is the operation a tool call names, with its arguments as they arrived; `None`
    /// for the requests that are no operation (`initialize`, `server/discover`, the two
    /// listings, the App read, and a tool call whose name is no tool). The arguments are read
    /// only to find the log a refusal is recorded in (`refuse_route`).
    ///
    /// # Errors
    /// Returns `Unauthenticated` without a principal, and `Forbidden` for any route other than
    /// `McpDelegation`, after recording the refusal (`refuse_route`).
    async fn admit<'caller>(
        &self,
        principal: Option<&'caller Principal>,
        asked: Option<(OperationName, &Value)>,
    ) -> Result<&'caller Principal, ApiError> {
        let Some(principal) = principal else {
            return Err(ApiError::new(
                ErrorCode::Unauthenticated,
                "An MCP request needs an authenticated agent identity",
            ));
        };
        if principal.route != AccessRoute::McpDelegation {
            return Err(self.refuse_route(principal, asked).await);
        }
        Ok(principal)
    }

    /// Record a non-agent route's request as `permission_denied` and return its refusal.
    ///
    /// A refused tool call is recorded by `core::dispatch::record_refusal`, which applies
    /// dispatch's own rule for a refusal made before any target is evaluated to the request
    /// the arguments decode to. Which log that is, that only a `Forbidden` is recorded and
    /// that a failed append does not change the answer are that function's rules, not this
    /// one's. A request that is no operation has nothing to decode and names no workspace, so
    /// its refusal goes to the tenant's log, which `list_tenant_events` reads, naming no
    /// operation; a failed append does not change that answer either.
    async fn refuse_route(
        &self,
        principal: &Principal,
        asked: Option<(OperationName, &Value)>,
    ) -> ApiError {
        let events = self.shared.ports.events.as_ref();
        let refusal = ApiError::new(
            ErrorCode::Forbidden,
            "Only an MCP delegation reaches this server; this route uses the HTTP API",
        );
        if let Some((operation, input)) = asked {
            return record_refusal(events, principal, operation, input, refusal).await;
        }
        let scope = EventScope::Tenant(principal.tenant_id.clone());
        let _recorded = events
            .append(&scope, None, unnamed_refusal_event(principal))
            .await;
        refusal
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

/// Whether `media_type` is one of `INLINE_IMAGE_TYPES`, ignoring case and any parameters.
fn is_inline_image_type(media_type: &str) -> bool {
    inline_type(media_type).is_some()
}

/// The inline image type `media_type` names, ignoring case, surrounding spaces and any
/// parameters: its canonical essence (one of `INLINE_IMAGE_TYPES`, lower case, as listed) and
/// its signature check; `None` when it names no listed type.
///
/// The one place both are chosen, so the type an image block carries and the check its bytes
/// passed are of the same type by construction.
fn inline_type(media_type: &str) -> Option<(&'static str, Signature)> {
    let essence = media_type
        .split_once(';')
        .map_or(media_type, |(essence, _)| essence)
        .trim();
    INLINE_IMAGE_TYPES
        .iter()
        .find(|(inline, _)| essence.eq_ignore_ascii_case(inline))
        .map(|(inline, signature)| (*inline, *signature))
}

/// Whether `bytes` begin as a PNG file does.
fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&PNG_SIGNATURE)
}

/// Whether `bytes` begin as a JPEG file does.
fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&JPEG_SIGNATURE)
}

/// Whether `bytes` begin as a GIF file of either version does.
fn is_gif(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")
}

/// Whether `bytes` begin as a WebP file does: a RIFF container (`RIFF`, then four bytes of
/// size, which are not compared) whose form type is `WEBP`.
fn is_webp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"RIFF") && bytes.get(8..SIGNATURE_SPAN) == Some(b"WEBP".as_slice())
}

/// The first bytes padded base64 text decodes to, at most `SIGNATURE_SPAN` of them.
///
/// Only the leading `SIGNATURE_CHARACTERS` are read, by the standard alphabet. Decoding ends at
/// the padding and at any other character outside the alphabet, so malformed text yields the
/// bytes before the fault and no others. Text past those characters is not looked at here.
fn leading_bytes(base64: &str) -> Vec<u8> {
    let sextets: Vec<u32> = base64
        .bytes()
        .take(SIGNATURE_CHARACTERS)
        .map_while(|character| {
            BASE64_ALPHABET
                .iter()
                .position(|letter| *letter == character)
                .and_then(|place| u32::try_from(place).ok())
        })
        .collect();
    sextets
        .chunks(4)
        .flat_map(|quad| {
            // Four sextets are three bytes; three are two, two are one, and one is none.
            let word = (0..4).fold(0_u32, |word, at| {
                (word << 6) | quad.get(at).copied().unwrap_or(0)
            });
            let [_, first, second, third] = word.to_be_bytes();
            [first, second, third]
                .into_iter()
                .take(quad.len().saturating_sub(1))
        })
        .collect()
}

/// The number of bytes padded base64 text decodes to: three per four characters, less the
/// padding.
fn decoded_size(base64: &str) -> usize {
    let padding = base64
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'=')
        .count();
    base64.len().saturating_sub(padding).saturating_mul(3) / 4
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

/// The `permission_denied` event of a route refusal of a request that is no operation: it
/// names the caller and no operation.
fn unnamed_refusal_event(principal: &Principal) -> NewEvent {
    NewEvent {
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

    /// The workspace `list_drafts` names.
    const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";

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
            _workspace: Option<WorkspaceId>,
        ) -> PortFuture<'a, BeginOutcome> {
            Box::pin(async { Err(not_reached()) })
        }

        fn complete(
            &self,
            _lease: MutationLease,
            _response: Value,
            _workspace: Option<WorkspaceId>,
        ) -> PortFuture<'_, ()> {
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
        json!({"workspace_id": WORKSPACE, "page": {"limit": 10}})
    }

    /// The log of the workspace `list_drafts` names, in the test principal's tenant.
    fn workspace_log() -> Result<EventScope, Box<dyn Error>> {
        Ok(EventScope::Workspace(StorageScope {
            tenant_id: TenantId::try_from("tenant-test".to_owned()).map_err(|error| error.0)?,
            workspace_id: serde_json::from_value(json!(WORKSPACE))?,
        }))
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
        // The request names one workspace, so each refusal is recorded in that workspace's log.
        let appended = events.appended()?;
        assert_eq!(appended.len(), 3);
        for (scope, event) in &appended {
            assert_eq!(scope, &workspace_log()?);
            assert_eq!(event.operation, Some(OperationName::ListDrafts));
        }
        Ok(())
    }

    /// Dispatch refuses an agent a draft-bearing operation by route, before any target; this
    /// server refuses a browser session the same request by route, before dispatch. Both are
    /// recorded in the same log, each naming its own caller.
    #[tokio::test]
    async fn a_route_refusal_lands_where_dispatchs_own_route_refusal_lands() -> Checked {
        let events = Arc::new(Recording::default());
        let server = server(events.clone())?;
        let callers = [
            (AccessRoute::McpDelegation, DRAFT_REFUSAL),
            (AccessRoute::BrowserSession, ROUTE_REFUSAL),
        ];
        for (route, message) in callers {
            let caller = principal(route.clone())?;
            let result = server
                .call_operation(Some(&caller), OperationName::ListDrafts, list_drafts())
                .await?;
            assert_eq!(refusal(&result)?.message, message, "{route:?}");
        }
        let appended = events.appended()?;
        let [(dispatched, by_dispatch), (refused, by_server)] = appended.as_slice() else {
            return Err(format!("expected two recorded refusals, got {appended:?}").into());
        };
        assert_eq!(dispatched, &workspace_log()?);
        assert_eq!(refused, dispatched);
        assert_eq!(by_dispatch.operation, by_server.operation);
        let routes: Vec<_> = [by_dispatch, by_server]
            .iter()
            .filter_map(|event| event.actor.as_ref())
            .map(|actor| actor.route.clone())
            .collect();
        assert_eq!(
            routes,
            vec![AccessRoute::McpDelegation, AccessRoute::BrowserSession]
        );
        Ok(())
    }

    /// The decoder of an image's leading bytes agrees with RFC 4648's vectors, never yields
    /// more than the longest signature spans, and stops at whatever is not standard base64.
    #[test]
    fn leading_bytes_are_the_first_decoded_bytes_and_end_at_what_is_not_base64() {
        let cases: [(&str, &[u8]); 14] = [
            ("", b""),
            ("Zg==", b"f"),
            ("Zm8=", b"fo"),
            ("Zm9v", b"foo"),
            ("Zm9vYg==", b"foob"),
            ("Zm9vYmE=", b"fooba"),
            ("Zm9vYmFy", b"foobar"),
            ("+/+/", &[0xFB, 0xFF, 0xBF]),
            // Exactly the span, and more than the span: never more than twelve bytes.
            ("Zm9vYmFyZm9vYmFy", b"foobarfoobar"),
            ("Zm9vYmFyZm9vYmFyZm9vYmFy", b"foobarfoobar"),
            // A sextet with no partner carries no byte.
            ("Zm9vY", b"foo"),
            // A character outside the standard alphabet ends the decode where it stands.
            ("Zm9v*mFy", b"foo"),
            ("Zm9v-_-_", b"foo"),
            ("Zm9vYg==YmFy", b"foob"),
        ];
        for (base64, bytes) in cases {
            assert_eq!(super::leading_bytes(base64), bytes, "{base64}");
        }
    }

    /// A request that is no operation is refused by route whether or not its refusal can be
    /// recorded: the log here fails every append, and the answer is still the route refusal.
    #[tokio::test]
    async fn a_refused_request_that_is_no_operation_is_answered_though_the_log_fails() -> Checked {
        let events = Arc::new(Recording::default());
        let server = server(events.clone())?;
        let browser = principal(AccessRoute::BrowserSession)?;
        let Err(read) = server.read_resource_as(Some(&browser), "ui://none").await else {
            return Err("a browser session read a resource".into());
        };
        assert_eq!(
            (read.code, read.message.as_str()),
            (ErrorCode::Forbidden, ROUTE_REFUSAL)
        );
        let result = server
            .call_tool_as(Some(&browser), "no_such_tool", None)
            .await?;
        let called = refusal(&result)?;
        assert_eq!(
            (called.code, called.message.as_str()),
            (ErrorCode::Forbidden, ROUTE_REFUSAL)
        );
        let appended = events.appended()?;
        assert_eq!(appended.len(), 2);
        for (scope, event) in &appended {
            assert_eq!(scope, &EventScope::Tenant(browser.tenant_id.clone()));
            assert_eq!(event.operation, None);
        }
        Ok(())
    }
}
