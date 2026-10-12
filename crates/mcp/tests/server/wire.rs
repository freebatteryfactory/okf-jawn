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
use okf_jawn_mcp::{APP_URI, DEFAULT_MAX_REQUEST_BODY_BYTES, McpConfig, streamable_http_service};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::check::{TestResult, err_of, some};
use crate::support::fakes::{FakeAccess, World};
use crate::support::fixtures::{
    HOST, Outcome, REPORT_MARKDOWN, WORKSPACE, agent, bundle, call, call_result, committed,
    digest_of, http_service, image_read_input, image_read_response, initialize, media,
    missing_bundle, object_block, on_page, post, rpc, workspaces_input, workspaces_response,
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

/// Serve `world` at `/mcp` on a loopback port as alice, with the App bundle `name`.
async fn serve(
    world: &World,
    name: &str,
) -> Outcome<(SocketAddr, JoinHandle<std::io::Result<()>>)> {
    let service = streamable_http_service(
        world.ports.clone(),
        McpConfig {
            app_bundle: bundle(name, HTML)?,
            allowed_hosts: vec!["127.0.0.1".to_owned()],
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
        },
    )?;
    let router = Router::new()
        .nest_service("/mcp", service)
        .layer(Extension(agent("alice")?));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    Ok((
        address,
        tokio::spawn(async move { axum::serve(listener, router).await }),
    ))
}

#[tokio::test]
async fn initialize_list_call_and_read_over_real_http() -> TestResult {
    let world = World::new(FakeAccess::default().granting("alice", &[Permission::Read]));
    world
        .application
        .answer_with("list_workspaces", workspaces_response())?;
    let (address, server) = serve(&world, "wire-app.html").await?;

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

/// A read that selects a page image answers, on the wire, with its text and then the image
/// block, typed as served.
#[tokio::test]
async fn a_read_carries_its_image_block_over_real_http() -> TestResult {
    let world = World::new(FakeAccess::default().granting("alice", &[Permission::Read]));
    let page = digest_of('c');
    let response = image_read_response(
        WORKSPACE,
        "pages",
        &[media(&page, "image/png", Some("Page 1"), &on_page(1))],
    );
    world
        .application
        .answer_with("read_item", response.clone())?;
    world.application.serve_object(
        &page,
        Ok(object_block(&page, "image/png", "iVBORw0KGgo=", 8, false)),
    )?;
    let (address, server) = serve(&world, "wire-images.html").await?;
    let called = exchange(
        address,
        &call(1, "show", &image_read_input("pages", 1024, 1)),
    )
    .await?;
    let outcome = result(&called)?;
    assert_eq!(outcome.get("isError"), Some(&json!(false)));
    assert_eq!(
        outcome.get("content"),
        Some(&json!([
            {"type": "text", "text": REPORT_MARKDOWN},
            {"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"}
        ]))
    );
    assert_eq!(outcome.get("structuredContent"), Some(&response));
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
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
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

#[tokio::test]
async fn a_request_over_the_configured_body_limit_is_refused() -> TestResult {
    let world = World::new(FakeAccess::default().granting("alice", &[Permission::Read]));
    world
        .application
        .answer_with("list_workspaces", workspaces_response())?;
    let service = streamable_http_service(
        world.ports.clone(),
        McpConfig {
            app_bundle: missing_bundle("wire-limit.html")?,
            allowed_hosts: vec![HOST.to_owned()],
            max_request_body_bytes: 2048,
        },
    )?;
    let small = call(1, "workspaces", &workspaces_input());
    let accepted = post(&service, Some(agent("alice")?), HOST, &[], &small).await?;
    assert_eq!(accepted.status, 200, "{}", accepted.body);
    let large = call(
        2,
        "workspaces",
        &json!({"page": {"limit": 10, "cursor": "a".repeat(4096)}}),
    );
    assert!(large.to_string().len() > 2048);
    let refused = post(&service, Some(agent("alice")?), HOST, &[], &large).await?;
    assert_eq!(refused.status, 413, "{}", refused.body);
    assert_eq!(
        world.application.calls()?.len(),
        1,
        "only the small call ran"
    );
    Ok(())
}

/// The configured limit can be raised, not only lowered: an 8 MiB limit admits a call over the
/// 4 MiB default.
#[tokio::test]
async fn a_raised_body_limit_admits_a_larger_request() -> TestResult {
    let world = World::new(FakeAccess::default().granting("alice", &[Permission::Read]));
    world
        .application
        .answer_with("list_workspaces", workspaces_response())?;
    let service = streamable_http_service(
        world.ports.clone(),
        McpConfig {
            app_bundle: missing_bundle("wire-raised.html")?,
            allowed_hosts: vec![HOST.to_owned()],
            max_request_body_bytes: 2 * DEFAULT_MAX_REQUEST_BODY_BYTES,
        },
    )?;
    let large = call(
        1,
        "workspaces",
        &json!({"page": {"limit": 10, "cursor": "a".repeat(DEFAULT_MAX_REQUEST_BODY_BYTES)}}),
    );
    let reply = post(&service, Some(agent("alice")?), HOST, &[], &large).await?;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(call_result(&reply)?.is_error, Some(false));
    assert_eq!(world.application.calls()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn the_provisional_default_body_limit_is_rmcps_4_mib() -> TestResult {
    assert_eq!(DEFAULT_MAX_REQUEST_BODY_BYTES, 4 * 1024 * 1024);
    let world = World::new(FakeAccess::default().granting("alice", &[Permission::Read]));
    let service = http_service(&world, missing_bundle("wire-default-limit.html")?)?;
    let over = call(
        1,
        "workspaces",
        &json!({"page": {"limit": 10, "cursor": "a".repeat(DEFAULT_MAX_REQUEST_BODY_BYTES)}}),
    );
    let refused = post(&service, Some(agent("alice")?), HOST, &[], &over).await?;
    assert_eq!(refused.status, 413, "{}", refused.body);
    assert_eq!(world.application.calls()?, Vec::new());
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
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
        },
    );
    let refused: ApiError = err_of(built.map(|_| "a service"))?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    Ok(())
}
