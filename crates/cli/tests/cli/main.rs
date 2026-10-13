//! The command line's surface, its one request path and its `mcp --stdio` relay.
//!
//! `surface` walks the canonical operation table against the commands. `requests` drives the
//! real entry point against a loopback listener answered by hand, and the built executable for
//! what only a process shows. `mcp_stdio` holds every relay test, so the filter of the
//! `cli-mcp-stdio-relay` gate selects them: most use doubles of the transport and the
//! launcher, and `mcp_stdio::wire` drives the production transport against the listener.

#[path = "../../../../tests/support/check.rs"]
mod check;
mod mcp_stdio;
mod requests;
mod support;
mod surface;
