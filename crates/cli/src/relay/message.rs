//! One line of the relay's input as a JSON-RPC message, the Streamable HTTP headers it needs,
//! and the JSON-RPC errors the relay itself answers with.
//!
//! The header names and the rules for them are those of `rmcp` 3.5.0
//! (`transport/common/http_header.rs`, `mcp_headers.rs` and the client worker):
//! - `initialize` carries no `MCP-Protocol-Version`: the handshake is what negotiates it.
//! - After it, every message carries the version the service answered with.
//! - A request whose `_meta` names a protocol version carries that version, which then also
//!   applies to the messages that follow. A request the relay refuses is not sent, and the
//!   version it names is not put in force: the service never heard of it.
//! - From protocol `2026-07-28`, a request or notification also carries `Mcp-Method`, and
//!   `Mcp-Name` when its method names a tool, prompt, resource or task.
//!
//! An error the relay writes for a message whose id it could not read has no `id` member, as
//! the `2026-07-28` revision says. A batch (a JSON array) is refused: `rmcp` 3.5.0 has no batch
//! message.

use serde_json::{Map, Value};

use super::json::{Noted, Span, noted};
use super::transport::Post;

/// What kind of message a line holds; it decides what the relay owes the client afterwards.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Kind {
    /// A request: exactly one response or error with this id must reach the client.
    Request {
        /// The id as the client wrote it, which is how the relay writes it back.
        written: String,
        /// The characters the id spells, when it is a string; `None` for a number.
        spelled: Option<String>,
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
/// The longest spelling of the JSON-RPC version `2.0`: three characters of six bytes each as
/// escapes, and the two quotes.
const SPELLED_VERSION_BYTES: usize = 20;

impl Kind {
    /// Whether `answer`, an id as the service wrote it, is the id of this request.
    ///
    /// An id is matched exactly, as JSON-RPC says. A number is the text the client wrote: it
    /// is not read into a number, so `1.0` is not `1` and two long integers that differ in
    /// their last digit differ. A string is the characters it spells, because a service that
    /// reads an id and writes it again may spell a character with another escape.
    pub(super) fn answered_by(&self, answer: &str) -> bool {
        match self {
            Self::Request {
                written,
                spelled: None,
            } => answer == written,
            Self::Request {
                spelled: Some(spelled),
                ..
            } => {
                // No spelling of a character is longer than six bytes for each of its own, so
                // a longer text cannot spell this id and is not read at all.
                let longest = spelled.len().saturating_mul(6).saturating_add(2);
                answer.len() <= longest
                    && serde_json::from_str::<String>(answer).ok().as_deref()
                        == Some(spelled.as_str())
            }
            Self::Notification { .. } | Self::Response => false,
        }
    }
}

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

/// One JSON-RPC error message as an output line. `id` is the request's id as the client
/// wrote it, and is written back as it stands; it is left out when it could not be read.
pub(super) fn error_line(id: Option<&str>, code: i64, message: &str) -> String {
    let message = Value::from(message);
    let id = id.map(|id| format!("\"id\":{id},")).unwrap_or_default();
    format!("{{\"jsonrpc\":\"2.0\",{id}\"error\":{{\"code\":{code},\"message\":{message}}}}}")
}

/// The headers of one message, which also moves `lifecycle` forward.
///
/// `lifecycle` moves only when the headers are returned: the message is then going to be
/// sent. A message that is refused here leaves it as it was, so the protocol version a
/// refused request names is not in force for the messages that follow.
fn headers(
    message: &Map<String, Value>,
    method: Option<&str>,
    request: bool,
    lifecycle: &mut Lifecycle,
) -> Result<Vec<(&'static str, String)>, String> {
    if request && method == Some("initialize") {
        return Ok(Vec::new());
    }
    let inline = message
        .get("params")
        .and_then(|params| params.get("_meta"))
        .and_then(|meta| meta.get(META_PROTOCOL_VERSION))
        .and_then(Value::as_str)
        .filter(|_| request);
    if inline.is_some_and(|version| !bare(version) || version.is_empty()) {
        return Err(
            "The protocol version in the request's _meta cannot travel in an HTTP header"
                .to_owned(),
        );
    }
    // The version this message travels under: its own, or else the one in force.
    let version = inline.or(lifecycle.version.as_deref());
    let mut headers = Vec::new();
    if let Some(version) = version {
        headers.push((HEADER_PROTOCOL_VERSION, version.to_owned()));
    }
    if let Some(session) = &lifecycle.session {
        headers.push((HEADER_SESSION, session.clone()));
    }
    let standard = version.is_some_and(|version| version >= STANDARD_HEADERS_SINCE);
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
    // Nothing refuses the message after this, so the version it names is now in force.
    if let Some(version) = inline {
        lifecycle.version = Some(version.to_owned());
    }
    Ok(headers)
}

/// Whether `text`, of which `noted` was noted, has an `error` member that is an object.
pub(super) fn has_error_object(text: &str, noted: &Noted) -> bool {
    noted
        .error
        .is_some_and(|(start, _)| text.as_bytes().get(start) == Some(&b'{'))
}

/// Whether a JSON value is one JSON-RPC 2.0 message.
pub(super) fn is_message(value: &Value) -> bool {
    value.get("jsonrpc").and_then(Value::as_str) == Some("2.0") && value.is_object()
}

/// Whether `text`, of which `noted` was noted, is one JSON-RPC 2.0 message: an object whose
/// `jsonrpc` is the string `2.0`, whatever escapes spell it.
pub(super) fn is_version_2(text: &str, noted: &Noted) -> bool {
    noted.opening.is_some()
        && spelled(text, noted.jsonrpc, SPELLED_VERSION_BYTES).as_deref() == Some("2.0")
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
        |id: Option<&str>, why: &str| Refusal::Answer(error_line(id, INVALID_REQUEST, why));
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
    // The id is written back as the client wrote it: a number is not read and spelled again.
    let written = id.map(|id| written_id(line).unwrap_or_else(|| id.to_string()));
    let id = id.zip(written.clone());
    let written = written.as_deref();
    if !is_message(&value) {
        return Err(invalid(written, "Invalid request: jsonrpc must be \"2.0\""));
    }
    let (kind, method) = match (message.get("method"), message.get("id"), id) {
        (Some(Value::String(method)), _, Some((id, written))) => (
            Kind::Request {
                written,
                spelled: id.as_str().map(str::to_owned),
            },
            Some(method.as_str()),
        ),
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
        (Some(_), _, _) => {
            return Err(invalid(written, "Invalid request: method must be a string"));
        }
        (None, _, Some(_)) if message.contains_key("result") || message.contains_key("error") => {
            (Kind::Response, None)
        }
        (None, _, _) => {
            return Err(invalid(
                written,
                "Invalid request: a message has a method, or an id with a result or an error",
            ));
        }
    };
    let request = matches!(kind, Kind::Request { .. });
    let initialize = request && method == Some("initialize");
    let headers = headers(message, method, request, lifecycle).map_err(|why| match &kind {
        Kind::Request { written, .. } => {
            Refusal::Answer(error_line(Some(written), INVALID_REQUEST, &why))
        }
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

/// The string that stands at `place` in `text`, when a string stands there that is no longer
/// than `most` bytes as it is written. A longer one is not read at all, so reading costs no
/// more than `most`.
pub(super) fn spelled(text: &str, place: Option<Span>, most: usize) -> Option<String> {
    let (start, end) = place?;
    let written = text.get(start..end)?;
    if written.len() > most {
        return None;
    }
    serde_json::from_str::<String>(written).ok()
}

/// The id of the message on `line`, as the client wrote it.
fn written_id(line: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(line).ok()?;
    let (start, end) = noted(text)?.id?;
    Some(text.get(start..end)?.to_owned())
}
