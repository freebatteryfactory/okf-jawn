//! `okf-jawn mcp --stdio`: the relay between a local MCP client and the running service.
//!
//! A local MCP client starts this process and speaks JSON-RPC over its standard streams, one
//! message per line. The relay forwards each message to the service's MCP endpoint as one
//! Streamable HTTP `POST` carrying the connector credential, and writes each answer back as
//! one line. It opens no store and never opens the data directory, so the service stays the
//! only writer (SPEC sections 2 and 11).
//!
//! - `frame` reads bounded lines; `message` classifies them and derives the HTTP headers;
//!   `json` reads an answer of the service without writing it again.
//! - `transport` and `launch` are the two seams: sending a message and asking whether the
//!   service is up, and starting the server process. `http` and `launch::ProcessLauncher` are
//!   the production sides; tests use doubles.
//! - `session` is the relay loop.
//!
//! The wire rules are those of the MCP revisions `rmcp` 3.5.0 implements (`2024-11-05` to
//! `2026-07-28`). Known limits, each refused or reported and never silent:
//! - a name that needs the Base64 header form of protocol `2026-07-28` is refused, because the
//!   CLI has no Base64 dependency;
//! - `Mcp-Param-*` headers are not sent: the service's tools declare none;
//! - a session the service might issue is carried on later requests but never deleted, and no
//!   standalone event stream is opened: the service is stateless and offers neither.

pub use self::frame::{Frame, Frames};
pub use self::http::HttpTransport;
pub use self::launch::{
    Arrival, ChildState, Clock, Detachment, Launcher, ProcessLauncher, Progress, StartPolicy,
    SystemClock, server_beside, start_or_connect,
};
pub use self::session::{Limits, Relay, serve};
pub use self::sse::EventParser;
pub use self::transport::{Body, Post, Probe, Reply, Transport, TransportError};

mod frame;
mod http;
mod json;
mod launch;
mod message;
mod session;
mod sse;
mod transport;
