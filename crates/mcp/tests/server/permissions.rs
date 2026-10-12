//! Permissions apply independently of visibility hints, and the agent route never sees drafts.

use okf_jawn_contract::access::Permission;
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::operations::DRAFT_BEARING;
use okf_jawn_mcp::McpServer;
use serde_json::json;

use crate::check::{TestResult, some};
use crate::support::fakes::{FakeAccess, World};
use crate::support::fixtures::{
    agent, arguments, bundle, list_drafts_input, object_response, read_object_input, refusal_of,
    text_of,
};

#[tokio::test]
async fn an_app_only_tool_called_by_any_client_is_authorized_like_any_other() -> TestResult {
    let world = World::new(FakeAccess::default().granting("alice", &[Permission::Read]));
    world
        .application
        .answer_with("get_object", object_response())?;
    let server = McpServer::new(world.ports, bundle("permissions-app.html", "<p>app</p>")?)?;
    let declared = some(
        server
            .tools()
            .iter()
            .find(|tool| tool.name == "read_object"),
        "read_object in the catalog",
    )?;
    let visibility = declared
        .meta
        .as_ref()
        .and_then(|meta| meta.0.get("ui"))
        .and_then(|ui| ui.get("visibility"));
    assert_eq!(visibility, Some(&json!(["app"])), "read_object is app-only");
    // A model-facing client calls it directly: it is authorized, not refused for visibility.
    let result = server
        .call_tool_as(
            Some(&agent("alice")?),
            "read_object",
            Some(arguments(read_object_input())?),
        )
        .await?;
    assert_eq!(result.is_error, Some(false), "{}", text_of(&result)?);
    assert_eq!(result.structured_content, Some(object_response()));
    Ok(())
}

#[tokio::test]
async fn an_app_only_tool_refuses_a_caller_without_the_permission() -> TestResult {
    // bob holds a grant, but not `read`; mallory holds none.
    let world = World::new(FakeAccess::default().granting("bob", &[Permission::Propose]));
    world
        .application
        .answer_with("get_object", object_response())?;
    let events = world.events.clone();
    let application = world.application.clone();
    let server = McpServer::new(
        world.ports,
        bundle("permissions-denied.html", "<p>app</p>")?,
    )?;
    for subject in ["bob", "mallory"] {
        let result = server
            .call_tool_as(
                Some(&agent(subject)?),
                "read_object",
                Some(arguments(read_object_input())?),
            )
            .await?;
        assert_eq!(refusal_of(&result)?.code, ErrorCode::Forbidden, "{subject}");
    }
    assert_eq!(application.calls()?, Vec::new());
    let appended = events.appended()?;
    assert_eq!(appended.len(), 2);
    assert!(
        appended
            .iter()
            .all(|(_, event)| event.kind == EventKind::PermissionDenied
                && event.operation == Some(OperationName::GetObject))
    );
    Ok(())
}

#[tokio::test]
async fn a_draft_bearing_operation_named_through_the_handler_is_forbidden() -> TestResult {
    assert!(DRAFT_BEARING.contains(&OperationName::ListDrafts));
    // alice holds every permission dispatch could ask for; the route alone refuses.
    let world = World::new(FakeAccess::default().granting(
        "alice",
        &[Permission::Read, Permission::Write, Permission::Admin],
    ));
    world
        .application
        .answer_with("list_drafts", json!({"items": []}))?;
    let application = world.application.clone();
    let server = McpServer::new(
        world.ports,
        bundle("permissions-drafts.html", "<p>app</p>")?,
    )?;
    let result = server
        .call_operation(
            Some(&agent("alice")?),
            OperationName::ListDrafts,
            list_drafts_input(),
        )
        .await?;
    let refused = refusal_of(&result)?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert_eq!(
        refused.message,
        "Drafts are visible only in a human browser session"
    );
    assert_eq!(application.calls()?, Vec::new());
    Ok(())
}
