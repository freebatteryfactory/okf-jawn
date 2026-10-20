//! `okf-jawn mcp --stdio`: the relay between a local MCP client and the running service.
//!
//! A local MCP client starts this process and speaks JSON-RPC over its standard streams, one
//! message per line. The relay forwards each message to the service's MCP endpoint as one
//! Streamable HTTP `POST` carrying the connector credential, and writes each answer back as
//! one line. It opens no store and never opens the data directory, so the service stays the
//! only writer (SPEC sections 2 and 11).
//!
//! - `frame` reads bounded lines; `message` classifies them and derives the HTTP headers;
//!   `json` reads an answer of the service in one pass, without building anything from it.
//! - `room` is the one budget, counted in bytes, for what the relay holds on its way out: an
//!   answer from the moment its bytes are taken from the service until they are written.
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
//!   standalone event stream is opened: the service is stateless and offers neither;
//! - a response the service sends for another request's id is passed on as it is, so a
//!   service that misbehaves can answer one request on another's exchange;
//! - an id is matched exactly, as JSON-RPC says, and a number is matched as the text the
//!   client wrote, never as a 64-bit float: `1.0` does not answer `1`, and two integers of
//!   any length that differ in one digit are two ids. A service that writes a number id back
//!   in another spelling has therefore not answered: its text is passed on, and the request
//!   gets the relay's own error besides. A string id is the characters it spells;
//! - a line of the client has no limit on depth in the relay. It is read for the members the
//!   relay acts on and nothing else (`message`), and the JSON library's limit of 128 levels
//!   counts only the levels it reads into, never more than four; what it skips it checks by
//!   its grammar with a byte for each level. So a line nested deeper than 128 levels is
//!   forwarded, and the service judges it;
//! - an answer of the service has no limit on depth. The relay's reader (`json`) does not
//!   recurse and holds a bit for each level, so an answer nested a million deep is passed on
//!   as it is, and a client whose JSON library has a limit may then refuse it;
//! - over plain HTTP, `localhost` is rewritten to `127.0.0.1` (`Endpoint` says why): the
//!   service hears that host, and one bound to `[::1]` alone is not reached through
//!   `http://localhost`. The relay would find no service there and start a second server,
//!   which loses the data directory to the first;
//! - nothing identifies the service: whatever answers the liveness check with success at the
//!   address is sent the credential with the next message. On a loopback address that is any
//!   program of this computer that listens there first.

pub use self::frame::{Frame, Frames};
pub use self::http::HttpTransport;
pub use self::launch::{
    Arrival, ChildState, Clock, Detachment, Launcher, ProcessLauncher, Progress, StartPolicy,
    SystemClock, server_beside, start_or_connect,
};
pub use self::room::{Gathering, Held, Room};
pub use self::session::{Limits, Relay, serve};
pub use self::sse::EventParser;
pub use self::transport::{Body, Post, Probe, Reply, Transport, TransportError};

mod frame;
mod http;
mod json;
mod launch;
mod message;
mod room;
mod session;
mod sse;
mod transport;
