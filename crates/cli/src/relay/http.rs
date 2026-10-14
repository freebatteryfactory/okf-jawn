//! The production transport: Streamable HTTP to the service's MCP endpoint over `reqwest`.
//!
//! Every `POST` accepts both `application/json` and `text/event-stream`, declares a JSON body
//! and carries the connector credential as a bearer token. The liveness check is a `GET` of
//! the public liveness path and carries no credential. The client comes from
//! `Endpoint::client`, so no redirect is followed. One answer, a JSON body or one event of a
//! stream, is read up to the answer limit and no further, and its bytes are held only in room
//! taken for them first (`Room`): while the room is full, nothing more is read from the service.
//!
//! A message has no overall time limit, because an event stream has no fixed length; a
//! connection attempt and a liveness check are bounded.

use std::time::Duration;

use okf_jawn_contract::error::ApiError;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use tokio::sync::mpsc;

use super::room::{Gathering, Held, Room};
use super::sse::EventParser;
use super::transport::{Body, Post, Probe, Reply, Transport, TransportError};
use crate::credential::Credential;
use crate::endpoint::Endpoint;
use crate::request::chain;

/// Streamable HTTP to one service.
#[derive(Debug, Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
    messages: reqwest::Url,
    liveness: reqwest::Url,
    bearer: HeaderValue,
    answer_bytes: usize,
}

/// Both media types a Streamable HTTP `POST` must accept.
const ACCEPT_BOTH: &str = "application/json, text/event-stream";
const EVENT_STREAM: &str = "text/event-stream";
const JSON: &str = "application/json";
/// The public liveness path (`liveness` in the contract's transport table).
const LIVENESS_PATH: &str = "/healthz";
/// How long one liveness check may take. Windows takes about two seconds to report that a
/// loopback port refuses connections, so the bound is well above that.
const LIVENESS_TIMEOUT: Duration = Duration::from_secs(5);
/// The MCP endpoint path (`mcp_request` in the contract's transport table).
const MESSAGES_PATH: &str = "/mcp";
const SESSION_HEADER: &str = "mcp-session-id";

impl HttpTransport {
    /// A transport to `endpoint` that authenticates with `credential` and reads at most
    /// `answer_bytes` of one answer.
    ///
    /// # Errors
    /// Returns `Internal` when the HTTP client or the credential header cannot be built.
    pub fn new(
        endpoint: &Endpoint,
        credential: &Credential,
        answer_bytes: usize,
    ) -> Result<Self, ApiError> {
        Ok(Self {
            client: endpoint.client(None)?,
            messages: endpoint.url(MESSAGES_PATH),
            liveness: endpoint.url(LIVENESS_PATH),
            bearer: credential.bearer()?,
            answer_bytes,
        })
    }

    /// Read a whole body, up to the answer limit, into room taken for it.
    async fn body(
        &self,
        response: &mut reqwest::Response,
        room: &Room,
    ) -> Result<Gathering, TransportError> {
        let mut body = room.gather(self.answer_bytes);
        // The length the service announces spares the buffer its growing and decides nothing:
        // the bytes that arrive are what is checked against the limit.
        if let Some(length) = response.content_length() {
            body.expect(usize::try_from(length).unwrap_or(usize::MAX))
                .await;
        }
        while let Some(chunk) = response.chunk().await.map_err(|error| failed(&error))? {
            if body.len().saturating_add(chunk.len()) > self.answer_bytes {
                return Err(TransportError(format!(
                    "the service's answer exceeds {} bytes",
                    self.answer_bytes
                )));
            }
            body.extend(&chunk).await;
        }
        Ok(body)
    }
}

impl Transport for HttpTransport {
    async fn probe(&self) -> Probe {
        match self
            .client
            .get(self.liveness.clone())
            .timeout(LIVENESS_TIMEOUT)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => Probe::Up,
            Ok(response) => Probe::Unhealthy(response.status().as_u16()),
            Err(error) if refused(&error) => Probe::Refused,
            Err(error) => Probe::Failed(failed(&error).0),
        }
    }

    async fn send(
        &self,
        post: Post,
        events: mpsc::Sender<Held>,
        room: &Room,
    ) -> Result<Reply, TransportError> {
        let mut request = self
            .client
            .post(self.messages.clone())
            .header(ACCEPT, ACCEPT_BOTH)
            .header(CONTENT_TYPE, JSON)
            .header(AUTHORIZATION, self.bearer.clone());
        for (name, value) in &post.headers {
            request = request.header(*name, value);
        }
        let mut response = request
            .body(post.body)
            .send()
            .await
            .map_err(|error| failed(&error))?;
        let status = response.status();
        let session = response
            .headers()
            .get(SESSION_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let media = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let body = if status.is_success() && media.starts_with(EVENT_STREAM) {
            let mut parser = EventParser::new(self.answer_bytes, room);
            while let Some(chunk) = response.chunk().await.map_err(|error| failed(&error))? {
                // Each event is passed on as soon as it is whole, so what a chunk completes
                // is passed on even when its tail is refused.
                let mut rest = chunk.as_ref();
                while let Some(data) = parser.next(&mut rest).await? {
                    events.send(data).await.map_err(|_| {
                        TransportError("the relay stopped reading the event stream".to_owned())
                    })?;
                }
            }
            Body::Events
        } else {
            let bytes = self.body(&mut response, room).await?;
            if bytes.is_empty() {
                Body::Empty
            } else if media.starts_with(JSON) {
                Body::Json(bytes.into_text().ok_or_else(|| {
                    TransportError("the service's JSON answer is not UTF-8".to_owned())
                })?)
            } else {
                Body::Other
            }
        };
        Ok(Reply {
            status: status.as_u16(),
            session,
            body,
        })
    }
}

/// A `reqwest` failure in plain words. It may name the URL, which holds no credentials
/// (`Endpoint` refuses them); a header is not part of it.
fn failed(error: &reqwest::Error) -> TransportError {
    TransportError(chain(error))
}

/// Whether a failure is the operating system refusing the connection: nothing listens there.
fn refused(error: &reqwest::Error) -> bool {
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        if cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::ConnectionRefused)
        {
            return true;
        }
        source = cause.source();
    }
    false
}
