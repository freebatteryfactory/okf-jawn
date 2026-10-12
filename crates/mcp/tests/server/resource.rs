//! The MCP App resource: declared by `api/mcp-apps.json`, read from the bundle at runtime.

use okf_jawn_contract::access::Permission;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_mcp::{APP_MIME_TYPE, APP_URI, McpServer};
use rmcp::model::ResourceContents;
use serde_json::json;

use crate::check::{TestResult, err_of, some};
use crate::support::fakes::{FakeAccess, World};
use crate::support::fixtures::{
    HOST, agent, bundle, committed, http_service, missing_bundle, post, result_of, rpc,
};

const HTML: &str = "<!doctype html><html><body><div id=\"root\"></div></body></html>";

fn world() -> World {
    World::new(FakeAccess::default().granting("alice", &[Permission::Read]))
}

#[tokio::test]
async fn the_app_is_listed_exactly_as_declared() -> TestResult {
    let world = world();
    let service = http_service(&world, bundle("resource-list.html", HTML)?)?;
    let reply = post(
        &service,
        Some(agent("alice")?),
        HOST,
        &[],
        &rpc(1, "resources/list", &json!({})),
    )
    .await?;
    let listed = some(result_of(&reply)?.get("resources"), "`resources`")?;
    let declared = committed("mcp-apps.json")?;
    assert_eq!(Some(listed), declared.get("resources"));
    Ok(())
}

#[tokio::test]
async fn the_app_is_read_from_the_bundle_with_its_declared_mime_and_csp() -> TestResult {
    let world = world();
    let service = http_service(&world, bundle("resource-read.html", HTML)?)?;
    let reply = post(
        &service,
        Some(agent("alice")?),
        HOST,
        &[],
        &rpc(1, "resources/read", &json!({"uri": APP_URI})),
    )
    .await?;
    let contents = some(
        result_of(&reply)?
            .get("contents")
            .and_then(|contents| contents.as_array())
            .and_then(|contents| contents.first()),
        "the App's contents",
    )?;
    assert_eq!(contents.get("uri"), Some(&json!(APP_URI)));
    assert_eq!(contents.get("mimeType"), Some(&json!(APP_MIME_TYPE)));
    assert_eq!(contents.get("text"), Some(&json!(HTML)));
    let declared = committed("mcp-apps.json")?;
    let csp = some(
        declared.pointer("/resources/0/_meta/ui/csp"),
        "the declared csp",
    )?;
    assert_eq!(contents.pointer("/_meta/ui/csp"), Some(csp));
    Ok(())
}

#[tokio::test]
async fn the_bundle_is_read_again_on_every_read() -> TestResult {
    let world = world();
    let path = bundle("resource-reread.html", HTML)?;
    let server = McpServer::new(world.ports, path.clone())?;
    let first = server.read_resource_at(APP_URI).await?;
    std::fs::write(&path, "<p>rebuilt</p>")?;
    let second = server.read_resource_at(APP_URI).await?;
    let texts = [first, second].map(|read| match read.contents.first() {
        Some(ResourceContents::TextResourceContents { text, .. }) => Some(text.clone()),
        _ => None,
    });
    assert_eq!(
        texts,
        [Some(HTML.to_owned()), Some("<p>rebuilt</p>".to_owned())]
    );
    Ok(())
}

#[tokio::test]
async fn a_missing_or_empty_bundle_is_unavailable() -> TestResult {
    let world = world();
    let missing = McpServer::new(
        world.ports.clone(),
        missing_bundle("resource-missing.html")?,
    )?;
    let refused: ApiError = err_of(missing.read_resource_at(APP_URI).await)?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    let empty = McpServer::new(world.ports.clone(), bundle("resource-empty.html", " \n")?)?;
    let refused: ApiError = err_of(empty.read_resource_at(APP_URI).await)?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    // Over the wire the refusal is a JSON-RPC error carrying the serialized `ApiError`.
    let service = http_service(&world, missing_bundle("resource-missing-wire.html")?)?;
    let reply = post(
        &service,
        Some(agent("alice")?),
        HOST,
        &[],
        &rpc(1, "resources/read", &json!({"uri": APP_URI})),
    )
    .await?;
    assert_eq!(reply.body.get("result"), None, "{}", reply.body);
    let data = some(reply.body.pointer("/error/data"), "the error's data")?;
    let carried: ApiError = serde_json::from_value(data.clone())?;
    assert_eq!(carried.code, ErrorCode::Unavailable);
    Ok(())
}

#[tokio::test]
async fn any_other_uri_is_not_found() -> TestResult {
    let world = world();
    let server = McpServer::new(world.ports, bundle("resource-other.html", HTML)?)?;
    let refused: ApiError = err_of(server.read_resource_at("ui://okf-jawn/other.html").await)?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    Ok(())
}
