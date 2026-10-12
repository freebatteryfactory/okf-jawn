//! The Streamable HTTP service over a real loopback socket, and its transport configuration.
//!
//! The end-to-end test binds the service in an axum router on `127.0.0.1:0`, with axum's
//! `Extension` layer standing in for the server's identity middleware, and speaks HTTP/1.1 to
//! it over a `TcpStream`. The client side is a minimal request writer and response reader:
//! the workspace declares no MCP or HTTP client for this crate's tests (see the lane report's
//! request for rmcp's streamable HTTP client), so the exchange stops at what one JSON request
//! and one JSON response need.

use std::net::SocketAddr;

use axum::{Extension, Router};
use okf_jawn_contract::access::Permission;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_mcp::{APP_URI, McpConfig, streamable_http_service};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::check::{TestResult, err_of, some};
use crate::support::fakes::{FakeAccess, World};
use crate::support::fixtures::{
    Outcome, agent, bundle, call, committed, initialize, missing_bundle, post, rpc,
    workspaces_input, workspaces_response,
};

/// One HTTP/1.1 exchange's status and JSON body (`Null` when empty).
struct Exchange {
    status: u16,
    body: Value,
}

const HTML: &str = "<!doctype html><html><body>okf-jawn app</body></html>";

/// POST `body` to `/mcp` at `address` on a fresh connection and read the whole reply.
async fn exchange(address: SocketAddr, body: &Value) -> Outcome<Exchange> {
    let payload = body.to_string();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {address}\r\nAccept: application/json, text/event-stream\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    let mut stream = TcpStream::connect(address).await?;
    stream.write_all(request.as_bytes()).await?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await?;
    let reply = String::from_utf8(raw)?;
    let (head, content) = some(reply.split_once("\r\n\r\n"), "the end of the headers")?;
    if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        return Err(format!("expected a sized JSON reply, got {head}").into());
    }
    let status = some(head.split(' ').nth(1), "a status code")?.parse()?;
    let body = if content.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(content).map_err(|error| format!("{error}: {content}"))?
    };
    Ok(Exchange { status, body })
}

fn result(exchange: &Exchange) -> Outcome<&Value> {
    if exchange.status != 200 {
        return Err(format!("expected 200, got {}: {}", exchange.status, exchange.body).into());
    }
    Ok(some(exchange.body.get("result"), "a JSON-RPC result")?)
}

#[tokio::test]
async fn initialize_list_call_and_read_over_real_http() -> TestResult {
    let world = World::new(FakeAccess::default().granting("alice", &[Permission::Read]));
    world
        .application
        .answer_with("list_workspaces", workspaces_response())?;
    let service = streamable_http_service(
        world.ports.clone(),
        McpConfig {
            app_bundle: bundle("wire-app.html", HTML)?,
            allowed_hosts: vec!["127.0.0.1".to_owned()],
        },
    )?;
    let router = Router::new()
        .nest_service("/mcp", service)
        .layer(Extension(agent("alice")?));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move { axum::serve(listener, router).await });

    let opened = exchange(address, &initialize(1)).await?;
    let info = result(&opened)?;
    assert_eq!(info.pointer("/serverInfo/name"), Some(&json!("okf-jawn")));
    assert!(info.pointer("/capabilities/tools").is_some(), "{info}");
    assert!(info.pointer("/capabilities/resources").is_some(), "{info}");

    let initialized = exchange(
        address,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await?;
    assert_eq!(initialized.status, 202);

    let listed = exchange(address, &rpc(2, "tools/list", &json!({}))).await?;
    assert_eq!(
        result(&listed)?.get("tools"),
        committed("mcp-tools.json")?.get("tools")
    );

    let called = exchange(address, &call(3, "workspaces", &workspaces_input())).await?;
    let outcome = result(&called)?;
    assert_eq!(outcome.get("isError"), Some(&json!(false)));
    assert_eq!(
        outcome.get("structuredContent"),
        Some(&workspaces_response())
    );

    let read = exchange(address, &rpc(4, "resources/read", &json!({"uri": APP_URI}))).await?;
    assert_eq!(
        result(&read)?.pointer("/contents/0/text"),
        Some(&json!(HTML))
    );
    server.abort();
    Ok(())
}

#[tokio::test]
async fn the_configured_hosts_are_the_ones_accepted() -> TestResult {
    let world = World::new(FakeAccess::default().granting("alice", &[Permission::Read]));
    let service = streamable_http_service(
        world.ports.clone(),
        McpConfig {
            app_bundle: missing_bundle("wire-hosts.html")?,
            allowed_hosts: vec!["mcp.example.org".to_owned()],
        },
    )?;
    let accepted = post(
        &service,
        Some(agent("alice")?),
        "mcp.example.org",
        &[],
        &initialize(1),
    )
    .await?;
    assert_eq!(accepted.status, 200, "{}", accepted.body);
    let rebound = post(
        &service,
        Some(agent("alice")?),
        "localhost",
        &[],
        &initialize(1),
    )
    .await?;
    assert_eq!(rebound.status, 403);
    Ok(())
}

#[test]
fn an_empty_host_list_is_refused() -> TestResult {
    let world = World::new(FakeAccess::default());
    let built = streamable_http_service(
        world.ports,
        McpConfig {
            app_bundle: missing_bundle("wire-no-hosts.html")?,
            allowed_hosts: Vec::new(),
        },
    );
    let refused: ApiError = err_of(built.map(|_| "a service"))?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    Ok(())
}
