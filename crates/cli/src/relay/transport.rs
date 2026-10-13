//! The seam between the relay's protocol logic and the wire.
//!
//! A [`Transport`] sends one message and reports what came back at the HTTP level; the relay
//! loop decides what that means for the MCP client. The production side is
//! `HttpTransport`; tests use doubles.

use std::future::Future;

use tokio::sync::mpsc;

use super::room::{Held, Room};

/// One message to send to the service's MCP endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    /// The JSON-RPC message, as the client wrote it.
    pub body: Vec<u8>,
    /// The MCP headers this message needs, beyond the content negotiation and the credential
    /// every `POST` carries.
    pub headers: Vec<(&'static str, String)>,
}

/// What the service sent back for one [`Post`].
#[derive(Debug)]
pub struct Reply {
    /// The HTTP status.
    pub status: u16,
    /// The `Mcp-Session-Id` response header, when the service sent one.
    pub session: Option<String>,
    /// The body.
    pub body: Body,
}

/// The body of a [`Reply`].
#[derive(Debug)]
pub enum Body {
    /// No body: an accepted notification, or any other empty answer.
    Empty,
    /// One JSON document, with the room it holds.
    Json(Held),
    /// An event stream that has ended; each event's data was passed on as it arrived.
    Events,
    /// A body that is neither JSON nor an event stream.
    Other,
}

/// The answer to "is the service running?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// The liveness endpoint answered with success.
    Up,
    /// Nothing accepts connections at the address.
    Refused,
    /// Something answered, with this HTTP status instead of success.
    Unhealthy(u16),
    /// The check failed in another way, described in plain words.
    Failed(String),
}

/// Why a message could not be exchanged: a description in plain words, without any credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError(pub String);

/// Sends messages to the service and asks whether it is running.
pub trait Transport: Send + Sync + 'static {
    /// Ask whether the service is running.
    fn probe(&self) -> impl Future<Output = Probe> + Send;

    /// Send one message.
    ///
    /// When the answer is an event stream, the data of each event is sent to `events` as it
    /// arrives, and the call returns once the stream has ended.
    ///
    /// Every text handed over holds its room: the transport takes the room for an answer's
    /// bytes from `room` before it holds them, and waits while there is none, so it reads no
    /// further from the service while the room is full. The room goes with the text.
    ///
    /// # Errors
    /// Returns a [`TransportError`] when the message could not be sent or its answer was cut
    /// off; events delivered before the failure stay delivered.
    fn send(
        &self,
        post: Post,
        events: mpsc::Sender<Held>,
        room: &Room,
    ) -> impl Future<Output = Result<Reply, TransportError>> + Send;
}
