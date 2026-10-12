//! The production MCP server over fakes of the application and dispatch's ports.
//!
//! `tools` covers the catalog and the result helpers, `images` a read's selected images,
//! `identity` the per-request principal and the route rule, `permissions` authorization
//! independent of visibility and the draft rule, `resource` the App resource, and `wire` the
//! Streamable HTTP service over a real loopback socket.

#[path = "../../../../tests/support/check.rs"]
mod check;
mod identity;
mod images;
mod permissions;
mod resource;
#[path = "../support/mod.rs"]
mod support;
mod tools;
mod wire;
