//! The command line's surface and its one request path.
//!
//! `surface` walks the canonical operation table against the commands. `requests` drives the
//! real entry point against a loopback listener answered by hand, and the built executable for
//! what only a process shows.

#[path = "../../../../tests/support/check.rs"]
mod check;
mod requests;
mod support;
mod surface;
