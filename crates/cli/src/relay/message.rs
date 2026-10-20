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
//! - A version is put in force only when it can stand in the header as it is
//!   ([`usable_version`]), whoever names it. The client's request that names another is
//!   refused. The service's answer to `initialize` that names another is passed on, and no
//!   version is in force after it, as when the answer names none (`session` says so on the
//!   error stream).
//! - From protocol `2026-07-28`, a request or notification also carries `Mcp-Method`, and
//!   `Mcp-Name` when its method names a tool, prompt, resource or task.
//!
//! An error the relay writes for a message whose id it could not read has no `id` member, as
//! the `2026-07-28` revision says. A batch (a JSON array) is refused: `rmcp` 3.5.0 has no batch
//! message.
//!
//! A line is never read into a tree of values. It is read once, by the JSON library, for the
//! members the relay acts on and nothing else (`Line`, `Members`, `Params`): `jsonrpc`, `id`,
//! `method`, whether `result` or `error` stands there, and of `params` the protocol version in
//! its `_meta` and the name, URI or task id `Mcp-Name` repeats. Every other value is checked by
//! its grammar and skipped (`IgnoredAny`), so a line of 16 MiB that holds eight million numbers
//! costs no more than one that holds one string. When a member occurs twice the last one
//! counts, as it does for the JSON parsers a client is likely to use. The line is then sent in
//! the buffer it was read into, as the client wrote it.
//!
//! What reading holds beside the line: the members it reads (each at most the line; a key or a
//! string with escapes is spelled out once), and, for each level of nesting it skips, a byte of
//! the JSON library's own and a bit of the reader that finds the id as written (`json`). There
//! is no limit on that depth: the JSON library's limit of 128 levels counts only the levels the
//! relay reads into, never more than four (the message, `params`, its `_meta`, and a value
//! there), so a line nested deeper is forwarded and the service judges it.

use std::fmt::{self, Formatter};
use std::marker::PhantomData;

use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::{Number, Value};

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
    /// Nothing can be answered (a notification or a response): say on the error stream that
    /// this message was not sent, and why. The session names it, in bounded and visible words.
    Log(Kind, String),
}

/// A key the relay looks for in an object it reads, at whatever level it reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Error,
    Id,
    Jsonrpc,
    Meta,
    Method,
    Name,
    Other,
    Params,
    Result,
    TaskId,
    Uri,
    Version,
}

/// One line of the input, as far as the relay reads it.
#[derive(Debug)]
enum Line {
    /// A JSON array: a batch.
    Batch,
    /// A JSON object: a message.
    Message(Members),
    /// Any other JSON value.
    Other,
}

/// The value of one member the relay reads.
#[derive(Debug, Default)]
enum Member {
    /// The member is not there.
    #[default]
    Absent,
    /// A number.
    Number(Number),
    /// Any other value: `null`, a boolean, an array or an object, read only to be skipped.
    Other,
    /// A string, as the characters it spells.
    Text(String),
}

/// What the relay reads of a message.
#[derive(Debug, Default)]
struct Members {
    /// Whether an `error` member stands there, whatever its value.
    error: bool,
    id: Member,
    jsonrpc: Member,
    method: Member,
    params: Params,
    /// Whether a `result` member stands there, whatever its value.
    result: bool,
}

/// The `_meta` of `params`: the protocol version it names, when that is a string.
#[derive(Debug, Default)]
struct Meta(Option<String>);

/// What the headers need of `params`, each when it is a string: the protocol version in its
/// `_meta`, and the `name`, `uri` and `taskId` that `Mcp-Name` repeats. Anything but an
/// object is a `params` that holds none of them.
#[derive(Debug, Default)]
struct Params {
    name: Option<String>,
    task: Option<String>,
    uri: Option<String>,
    version: Option<String>,
}

/// Reads one JSON value as `T` makes of it ([`Shape`]).
struct Reading<T>(PhantomData<T>);

/// What one kind of value the relay reads makes of each kind of JSON value. A kind it does not
/// read is skipped, checked by its grammar alone and held nowhere.
trait Shape: Sized {
    /// What an array makes; by default it is skipped.
    ///
    /// # Errors
    /// Returns the JSON library's error for a text that is not JSON.
    fn array<'de, A: SeqAccess<'de>>(items: A) -> Result<Self, A::Error> {
        skip_items(items)?;
        Ok(Self::skipped())
    }

    /// What a number makes; by default it is skipped.
    fn number(_number: Number) -> Self {
        Self::skipped()
    }

    /// What an object makes; by default it is skipped.
    ///
    /// # Errors
    /// Returns the JSON library's error for a text that is not JSON.
    fn object<'de, A: MapAccess<'de>>(members: A) -> Result<Self, A::Error> {
        skip_members(members)?;
        Ok(Self::skipped())
    }

    /// What a value makes that is read only to be skipped: `null`, a boolean, and each kind
    /// whose method above is not overridden.
    fn skipped() -> Self;

    /// What a string makes, given the characters it spells; by default it is skipped.
    fn text(_text: &str) -> Self {
        Self::skipped()
    }
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

impl Member {
    /// The string, when the member is one.
    fn into_text(self) -> Option<String> {
        match self {
            Self::Text(text) => Some(text),
            Self::Absent | Self::Number(_) | Self::Other => None,
        }
    }
}

impl<'de, T: Shape> DeserializeSeed<'de> for Reading<T> {
    type Value = T;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<T, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de, T: Shape> Visitor<'de> for Reading<T> {
    type Value = T;

    fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E: serde::de::Error>(self, _value: bool) -> Result<T, E> {
        Ok(T::skipped())
    }

    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<T, E> {
        Ok(T::number(Number::from(value)))
    }

    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<T, E> {
        Ok(T::number(Number::from(value)))
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<T, E> {
        Ok(Number::from_f64(value).map_or_else(T::skipped, T::number))
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<T, E> {
        Ok(T::text(value))
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<T, E> {
        Ok(T::skipped())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, items: A) -> Result<T, A::Error> {
        T::array(items)
    }

    fn visit_map<A: MapAccess<'de>>(self, members: A) -> Result<T, A::Error> {
        T::object(members)
    }
}

impl Shape for Key {
    fn skipped() -> Self {
        Self::Other
    }

    fn text(text: &str) -> Self {
        match text {
            "error" => Self::Error,
            "id" => Self::Id,
            "jsonrpc" => Self::Jsonrpc,
            "_meta" => Self::Meta,
            "method" => Self::Method,
            "name" => Self::Name,
            "params" => Self::Params,
            "result" => Self::Result,
            "taskId" => Self::TaskId,
            "uri" => Self::Uri,
            META_PROTOCOL_VERSION => Self::Version,
            _ => Self::Other,
        }
    }
}

impl Shape for Line {
    fn array<'de, A: SeqAccess<'de>>(items: A) -> Result<Self, A::Error> {
        skip_items(items)?;
        Ok(Self::Batch)
    }

    fn object<'de, A: MapAccess<'de>>(mut members: A) -> Result<Self, A::Error> {
        let mut read = Members::default();
        while let Some(key) = members.next_key_seed(reading::<Key>())? {
            match key {
                Key::Error => {
                    members.next_value::<IgnoredAny>()?;
                    read.error = true;
                }
                Key::Id => read.id = members.next_value_seed(reading::<Member>())?,
                Key::Jsonrpc => read.jsonrpc = members.next_value_seed(reading::<Member>())?,
                Key::Method => read.method = members.next_value_seed(reading::<Member>())?,
                Key::Params => read.params = members.next_value_seed(reading::<Params>())?,
                Key::Result => {
                    members.next_value::<IgnoredAny>()?;
                    read.result = true;
                }
                Key::Meta | Key::Name | Key::Other | Key::TaskId | Key::Uri | Key::Version => {
                    members.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(Self::Message(read))
    }

    fn skipped() -> Self {
        Self::Other
    }
}

impl Shape for Member {
    fn number(number: Number) -> Self {
        Self::Number(number)
    }

    fn skipped() -> Self {
        Self::Other
    }

    fn text(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl Shape for Meta {
    fn object<'de, A: MapAccess<'de>>(mut members: A) -> Result<Self, A::Error> {
        let mut version = None;
        while let Some(key) = members.next_key_seed(reading::<Key>())? {
            if key == Key::Version {
                version = members.next_value_seed(reading::<Member>())?.into_text();
            } else {
                members.next_value::<IgnoredAny>()?;
            }
        }
        Ok(Self(version))
    }

    fn skipped() -> Self {
        Self(None)
    }
}

impl Shape for Params {
    fn object<'de, A: MapAccess<'de>>(mut members: A) -> Result<Self, A::Error> {
        let mut read = Self::default();
        while let Some(key) = members.next_key_seed(reading::<Key>())? {
            let place = match key {
                Key::Meta => {
                    read.version = members.next_value_seed(reading::<Meta>())?.0;
                    continue;
                }
                Key::Name => &mut read.name,
                Key::TaskId => &mut read.task,
                Key::Uri => &mut read.uri,
                Key::Error
                | Key::Id
                | Key::Jsonrpc
                | Key::Method
                | Key::Other
                | Key::Params
                | Key::Result
                | Key::Version => {
                    members.next_value::<IgnoredAny>()?;
                    continue;
                }
            };
            *place = members.next_value_seed(reading::<Member>())?.into_text();
        }
        Ok(read)
    }

    fn skipped() -> Self {
        Self::default()
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
    params: &Params,
    method: Option<&str>,
    request: bool,
    lifecycle: &mut Lifecycle,
) -> Result<Vec<(&'static str, String)>, String> {
    if request && method == Some("initialize") {
        return Ok(Vec::new());
    }
    let inline = params.version.as_deref().filter(|_| request);
    if inline.is_some_and(|version| !usable_version(version)) {
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
        if let Some(name) = name_of(method, params) {
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

/// Whether `text`, of which `noted` was noted, is one JSON-RPC 2.0 message: an object whose
/// `jsonrpc` is the string `2.0`, whatever escapes spell it.
pub(super) fn is_version_2(text: &str, noted: &Noted) -> bool {
    noted.opening.is_some()
        && spelled(text, noted.jsonrpc, SPELLED_VERSION_BYTES).as_deref() == Some("2.0")
}

/// The value the `Mcp-Name` header repeats, for the methods that have one.
fn name_of<'message>(method: &str, params: &'message Params) -> Option<&'message str> {
    match method {
        "tools/call" | "prompts/get" => params.name.as_deref(),
        "resources/read" | "resources/subscribe" | "resources/unsubscribe" => params.uri.as_deref(),
        "tasks/get" | "tasks/update" | "tasks/cancel" => params.task.as_deref(),
        _ => None,
    }
}

/// Turn one input line into a message to send, in the buffer the line was read into.
///
/// `Ok(None)` is a blank line, which is skipped. A byte order mark at its start is dropped.
pub(super) fn prepare(
    mut line: Vec<u8>,
    lifecycle: &mut Lifecycle,
) -> Result<Option<Outgoing>, Refusal> {
    if line.starts_with(BYTE_ORDER_MARK) {
        line.drain(..BYTE_ORDER_MARK.len());
    }
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let mut outgoing = read_message(&line, lifecycle)?;
    // Sent as the client wrote it, in the buffer it was read into: nothing copies it.
    outgoing.post.body = line;
    Ok(Some(outgoing))
}

/// The message on `line`, with its headers and an empty body: read in one pass for the members
/// the relay acts on (the header of this module says how).
fn read_message(line: &[u8], lifecycle: &mut Lifecycle) -> Result<Outgoing, Refusal> {
    let unreadable = |error: &dyn fmt::Display| {
        Refusal::Answer(error_line(
            None,
            PARSE_ERROR,
            &format!("Parse error: the line is not JSON ({error})"),
        ))
    };
    let text = std::str::from_utf8(line).map_err(|error| unreadable(&error))?;
    let mut reader = serde_json::Deserializer::from_str(text);
    let read = reading::<Line>()
        .deserialize(&mut reader)
        .and_then(|read| reader.end().map(|()| read))
        .map_err(|error| unreadable(&error))?;
    let invalid =
        |id: Option<&str>, why: &str| Refusal::Answer(error_line(id, INVALID_REQUEST, why));
    let message = match read {
        Line::Message(message) => message,
        Line::Batch => {
            return Err(invalid(
                None,
                "Invalid request: a JSON-RPC batch is not accepted; send one message per line",
            ));
        }
        Line::Other => return Err(invalid(None, "Invalid request: a message is a JSON object")),
    };
    // The id is written back as the client wrote it: a number is not read and spelled again.
    let written = match &message.id {
        Member::Text(spelled) => {
            Some(written_id(text).unwrap_or_else(|| Value::from(spelled.as_str()).to_string()))
        }
        Member::Number(number) => Some(written_id(text).unwrap_or_else(|| number.to_string())),
        Member::Absent | Member::Other => None,
    };
    if !matches!(&message.jsonrpc, Member::Text(version) if version == "2.0") {
        return Err(invalid(
            written.as_deref(),
            "Invalid request: jsonrpc must be \"2.0\"",
        ));
    }
    let kind = match (&message.method, &message.id, written) {
        (Member::Text(_), id, Some(written)) => Kind::Request {
            written,
            spelled: match id {
                Member::Text(spelled) => Some(spelled.clone()),
                Member::Absent | Member::Number(_) | Member::Other => None,
            },
        },
        (Member::Text(method), Member::Absent, None) => Kind::Notification {
            method: method.clone(),
        },
        (Member::Text(_), _, None) => {
            return Err(invalid(
                None,
                "Invalid request: an id is a string or a number",
            ));
        }
        (Member::Absent, _, Some(_)) if message.result || message.error => Kind::Response,
        (Member::Absent, _, written) => {
            return Err(invalid(
                written.as_deref(),
                "Invalid request: a message has a method, or an id with a result or an error",
            ));
        }
        (Member::Number(_) | Member::Other, _, written) => {
            return Err(invalid(
                written.as_deref(),
                "Invalid request: method must be a string",
            ));
        }
    };
    let method = match &message.method {
        Member::Text(method) => Some(method.as_str()),
        Member::Absent | Member::Number(_) | Member::Other => None,
    };
    let request = matches!(kind, Kind::Request { .. });
    let initialize = request && method == Some("initialize");
    let headers =
        headers(&message.params, method, request, lifecycle).map_err(|why| match &kind {
            Kind::Request { written, .. } => {
                Refusal::Answer(error_line(Some(written), INVALID_REQUEST, &why))
            }
            Kind::Notification { .. } | Kind::Response => Refusal::Log(kind.clone(), why),
        })?;
    Ok(Outgoing {
        post: Post {
            body: Vec::new(),
            headers,
        },
        kind,
        initialize,
    })
}

/// A reader of one JSON value as `T` makes of it.
fn reading<T: Shape>() -> Reading<T> {
    Reading(PhantomData)
}

/// Skip every item of an array, each checked by its grammar alone.
///
/// # Errors
/// Returns the JSON library's error for a text that is not JSON.
fn skip_items<'de, A: SeqAccess<'de>>(mut items: A) -> Result<(), A::Error> {
    while items.next_element::<IgnoredAny>()?.is_some() {}
    Ok(())
}

/// Skip every member of an object, each checked by its grammar alone.
///
/// # Errors
/// Returns the JSON library's error for a text that is not JSON.
fn skip_members<'de, A: MapAccess<'de>>(mut members: A) -> Result<(), A::Error> {
    while members.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
    Ok(())
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

/// Whether a protocol version can be put in force. It travels in the `MCP-Protocol-Version`
/// header of every later message as it is, so it is not empty and is a [`bare`] value. This
/// is the one check for a version: the one a request names in its `_meta`, and the one the
/// service names in its answer to `initialize`.
pub(super) fn usable_version(version: &str) -> bool {
    !version.is_empty() && bare(version)
}

/// The id of the message `text` holds, as the client wrote it.
fn written_id(text: &str) -> Option<String> {
    let (start, end) = noted(text)?.id?;
    Some(text.get(start..end)?.to_owned())
}
