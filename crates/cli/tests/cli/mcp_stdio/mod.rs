//! The `mcp --stdio` relay. Every test here is selected by the gate's `mcp_stdio` filter.
//!
//! `framing` covers the lines in and out, `forwarding` the messages and their headers,
//! `credential` the connector credential, `start` the start-or-connect decision and the
//! launcher, `errors` each error path, and `wire` the production transport over real HTTP.

use serde_json::{Value, json};

/// A request line.
pub fn request(id: &Value, method: &str, params: &Value) -> String {
    format!(
        "{}\n",
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    )
}

/// A notification line.
pub fn notification(method: &str) -> String {
    format!("{}\n", json!({"jsonrpc": "2.0", "method": method}))
}

/// The response the service gives to request `id`.
pub fn response(id: &Value, result: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

mod credential;
mod errors;
mod forwarding;
mod framing;
mod start;
mod wire;
