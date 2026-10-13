//! One line of the relay's input as a JSON-RPC message, the Streamable HTTP headers it needs,
//! and the JSON-RPC errors the relay itself answers with.
//!
//! The header names and the rules for them are those of `rmcp` 3.5.0
//! (`transport/common/http_header.rs`, `mcp_headers.rs` and the client worker):
//! - `initialize` carries no `MCP-Protocol-Version`: the handshake is what negotiates it.
//! - After it, every message carries the version the service answered with.
//! - A request whose `_meta` names a protocol version carries that version, which then also
//!   applies to the messages that follow.
//! - From protocol `2026-07-28`, a request or notification also carries `Mcp-Method`, and
//!   `Mcp-Name` when its method names a tool, prompt, resource or task.
//!
//! An error the relay writes for a message whose id it could not read has no `id` member, as
//! the `2026-07-28` revision says; a batch (a JSON array) is refused, as every revision since
//! `2025-06-18` has no batches.

use serde_json::{Map, Value, json};

use super::transport::Post;

/// What kind of message a line holds; it decides what the relay owes the client afterwards.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Kind {
    /// A request: exactly one response or error with this id must reach the client.
    Request {
        /// The request's id, a string or a number.
        id: Value,
    },
    /// A notification: nothing is owed; a failure is logged.
    Notification {
        /// The notification's method, for the log.
        method: String,
    },
    /// The client's response to a request the service made.
    Response,
}

/// The protocol state that later messages inherit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Lifecycle {
    /// The protocol version in force, once one is known.
    pub(super) version: Option<String>,
    /// The session the service issued at `initialize`, if it issued one.
    pub(super) session: Option<String>,
}

/// A message ready to send.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Outgoing {
    /// The HTTP request.
    pub(super) post: Post,
    /// What the relay owes the client for it.
    pub(super) kind: Kind,
    /// Whether this is the `initialize` request, which is exchanged alone.
    pub(super) initialize: bool,
}

/// Why a line is not forwarded.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Refusal {
    /// Answer the client with this JSON-RPC error line.
    Answer(String),
    /// Nothing can be answered (a notification); log this instead.
    Log(String),
}

/// JSON-RPC: the service or the relay failed while handling a request.
pub(super) const INTERNAL_ERROR: i64 = -32603;
/// JSON-RPC: the JSON is not a valid request object.
pub(super) const INVALID_REQUEST: i64 = -32600;
/// JSON-RPC: the text is not JSON.
pub(super) const PARSE_ERROR: i64 = -32700;

/// The request `_meta` key that names a per-request protocol version.
const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const HEADER_METHOD: &str = "Mcp-Method";
const HEADER_NAME: &str = "Mcp-Name";
const HEADER_PROTOCOL_VERSION: &str = "MCP-Protocol-Version";
const HEADER_SESSION: &str = "Mcp-Session-Id";
/// The first protocol version whose requests carry `Mcp-Method` and `Mcp-Name`.
const STANDARD_HEADERS_SINCE: &str = "2026-07-28";
/// The sentinel that wraps a Base64 header value from `2026-07-28` on.
const BASE64_PREFIX: &str = "=?base64?";
const BASE64_SUFFIX: &str = "?=";
/// The UTF-8 byte order mark a line may begin with.
const BYTE_ORDER_MARK: &[u8] = b"\xEF\xBB\xBF";

/// Whether a value can travel as an HTTP header value as it is: printable ASCII with no
/// leading or trailing blank, and not shaped like the Base64 sentinel.
fn bare(value: &str) -> bool {
    let blank = |byte: Option<&u8>| matches!(byte, Some(b' ' | b'\t'));
    let bytes = value.as_bytes();
    if blank(bytes.first()) || blank(bytes.last()) {
        return false;
    }
    if value.starts_with(BASE64_PREFIX) && value.ends_with(BASE64_SUFFIX) {
        return false;
    }
    bytes.iter().all(|byte| (0x20..=0x7E).contains(byte))
}

/// One JSON-RPC error message as an output line; `id` is left out when it could not be read.
pub(super) fn error_line(id: Option<&Value>, code: i64, message: &str) -> String {
    let mut error = Map::new();
    error.insert("jsonrpc".to_owned(), json!("2.0"));
    if let Some(id) = id {
        error.insert("id".to_owned(), id.clone());
    }
    error.insert(
        "error".to_owned(),
        json!({"code": code, "message": message}),
    );
    Value::Object(error).to_string()
}

/// The headers of one message, which also moves `lifecycle` forward.
fn headers(
    message: &Map<String, Value>,
    method: Option<&str>,
    request: bool,
    lifecycle: &mut Lifecycle,
) -> Result<Vec<(&'static str, String)>, String> {
    if request && method == Some("initialize") {
        *lifecycle = Lifecycle::default();
        return Ok(Vec::new());
    }
    let inline = message
        .get("params")
        .and_then(|params| params.get("_meta"))
        .and_then(|meta| meta.get(META_PROTOCOL_VERSION))
        .and_then(Value::as_str)
        .filter(|_| request);
    if let Some(version) = inline {
        if !bare(version) || version.is_empty() {
            return Err(
                "The protocol version in the request's _meta cannot travel in an HTTP header"
                    .to_owned(),
            );
        }
        lifecycle.version = Some(version.to_owned());
    }
    let mut headers = Vec::new();
    if let Some(version) = &lifecycle.version {
        headers.push((HEADER_PROTOCOL_VERSION, version.clone()));
    }
    if let Some(session) = &lifecycle.session {
        headers.push((HEADER_SESSION, session.clone()));
    }
    let standard = lifecycle
        .version
        .as_deref()
        .is_some_and(|version| version >= STANDARD_HEADERS_SINCE);
    if let (true, Some(method)) = (standard, method) {
        if !bare(method) {
            return Err("The method cannot travel in the Mcp-Method header".to_owned());
        }
        headers.push((HEADER_METHOD, method.to_owned()));
        if let Some(name) = name_of(method, message.get("params")) {
            if !bare(name) {
                return Err(format!(
                    "The {HEADER_NAME} header of {method} needs the Base64 form, which this \
                     relay does not build; use a name of printable ASCII"
                ));
            }
            headers.push((HEADER_NAME, name.to_owned()));
        }
    }
    Ok(headers)
}

/// Whether a JSON value is one JSON-RPC 2.0 message.
pub(super) fn is_message(value: &Value) -> bool {
    value.get("jsonrpc").and_then(Value::as_str) == Some("2.0") && value.is_object()
}

/// The value the `Mcp-Name` header repeats, for the methods that have one.
fn name_of<'message>(method: &str, params: Option<&'message Value>) -> Option<&'message str> {
    let key = match method {
        "tools/call" | "prompts/get" => "name",
        "resources/read" | "resources/subscribe" | "resources/unsubscribe" => "uri",
        "tasks/get" | "tasks/update" | "tasks/cancel" => "taskId",
        _ => return None,
    };
    params?.get(key)?.as_str()
}

/// Turn one input line into a message to send.
///
/// `Ok(None)` is a blank line, which is skipped.
pub(super) fn prepare(line: &[u8], lifecycle: &mut Lifecycle) -> Result<Option<Outgoing>, Refusal> {
    let line = line.strip_prefix(BYTE_ORDER_MARK).unwrap_or(line);
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let value = serde_json::from_slice::<Value>(line).map_err(|error| {
        Refusal::Answer(error_line(
            None,
            PARSE_ERROR,
            &format!("Parse error: the line is not JSON ({error})"),
        ))
    })?;
    let invalid =
        |id: Option<&Value>, why: &str| Refusal::Answer(error_line(id, INVALID_REQUEST, why));
    let Value::Object(message) = &value else {
        return Err(invalid(
            None,
            if value.is_array() {
                "Invalid request: a JSON-RPC batch is not accepted; send one message per line"
            } else {
                "Invalid request: a message is a JSON object"
            },
        ));
    };
    let id = message
        .get("id")
        .filter(|id| id.is_string() || id.is_number());
    if !is_message(&value) {
        return Err(invalid(id, "Invalid request: jsonrpc must be \"2.0\""));
    }
    let (kind, method) = match (message.get("method"), message.get("id"), id) {
        (Some(Value::String(method)), _, Some(id)) => {
            (Kind::Request { id: id.clone() }, Some(method.as_str()))
        }
        (Some(Value::String(method)), None, None) => (
            Kind::Notification {
                method: method.clone(),
            },
            Some(method.as_str()),
        ),
        (Some(Value::String(_)), Some(_), None) => {
            return Err(invalid(
                None,
                "Invalid request: an id is a string or a number",
            ));
        }
        (Some(_), _, _) => return Err(invalid(id, "Invalid request: method must be a string")),
        (None, _, Some(_)) if message.contains_key("result") || message.contains_key("error") => {
            (Kind::Response, None)
        }
        (None, _, _) => {
            return Err(invalid(
                id,
                "Invalid request: a message has a method, or an id with a result or an error",
            ));
        }
    };
    let request = matches!(kind, Kind::Request { .. });
    let initialize = request && method == Some("initialize");
    let headers = headers(message, method, request, lifecycle).map_err(|why| match &kind {
        Kind::Request { id } => Refusal::Answer(error_line(Some(id), INVALID_REQUEST, &why)),
        Kind::Notification { method } => {
            Refusal::Log(format!("the notification {method:?} was not sent: {why}"))
        }
        Kind::Response => Refusal::Log(format!("a response was not sent: {why}")),
    })?;
    Ok(Some(Outgoing {
        post: Post {
            body: line.to_vec(),
            headers,
        },
        kind,
        initialize,
    }))
}
