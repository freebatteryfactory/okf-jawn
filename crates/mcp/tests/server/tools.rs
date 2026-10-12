//! The tool catalog and the result helpers every tool call answers through.

use std::collections::BTreeSet;

use okf_jawn_contract::access::Permission;
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::metadata::{OperationName, operations};
use okf_jawn_contract::operations::{DRAFT_BEARING, HUMAN_SESSION_ONLY};
use okf_jawn_mcp::{McpServer, operation_of_tool};
use serde_json::{Value, json};

use crate::check::{TestResult, some};
use crate::support::fakes::{Call, FakeAccess, World};
use crate::support::fixtures::{
    HOST, agent, arguments, bundle, committed, http_service, post, proposal_response,
    propose_input, read_input, read_response, refusal_of, result_of, rpc, text_of,
    workspaces_input, workspaces_response,
};

fn reader() -> World {
    World::new(FakeAccess::default().granting("alice", &[Permission::Read, Permission::Propose]))
}

#[tokio::test]
async fn the_tool_listing_is_the_generated_catalog_exactly() -> TestResult {
    let world = reader();
    let service = http_service(&world, bundle("tools-listing.html", "<p>app</p>")?)?;
    let reply = post(
        &service,
        Some(agent("alice")?),
        HOST,
        &[],
        &rpc(1, "tools/list", &json!({})),
    )
    .await?;
    let listed = some(result_of(&reply)?.get("tools"), "`tools` in the listing")?;
    let catalog = committed("mcp-tools.json")?;
    let declared = some(catalog.get("tools"), "`tools` in api/mcp-tools.json")?;
    assert_eq!(listed, declared);
    let app_only = some(
        listed.as_array().and_then(|tools| {
            tools
                .iter()
                .find(|tool| tool.get("name") == Some(&json!("read_object")))
        }),
        "read_object in the listing",
    )?;
    assert_eq!(
        app_only.pointer("/_meta/ui/visibility"),
        Some(&json!(["app"]))
    );
    Ok(())
}

#[test]
fn every_listed_tool_is_an_operation_and_none_carries_a_draft() -> TestResult {
    let world = reader();
    let server = McpServer::new(world.ports, bundle("tools-operations.html", "<p>app</p>")?)?;
    assert_eq!(server.tools().len(), 13);
    for tool in server.tools() {
        let operation = some(operation_of_tool(&tool.name), "the tool's operation")?;
        assert!(
            !DRAFT_BEARING.contains(&operation),
            "{} reveals drafts",
            tool.name
        );
        assert!(
            !HUMAN_SESSION_ONLY.contains(&operation),
            "{} is a human-session operation",
            tool.name
        );
    }
    assert_eq!(operation_of_tool("show"), Some(OperationName::ReadItem));
    assert_eq!(
        operation_of_tool("read_object"),
        Some(OperationName::GetObject)
    );
    Ok(())
}

/// Only the agent alias column names a tool: never a canonical id, an operator/CLI alias or a
/// label, and every agent alias the table declares is listed.
#[tokio::test]
async fn only_agent_aliases_are_tool_names() -> TestResult {
    let world = reader();
    let server = McpServer::new(world.ports, bundle("tools-names.html", "<p>app</p>")?)?;
    let listed: BTreeSet<String> = server
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    let aliases: BTreeSet<String> = operations()
        .into_iter()
        .filter(|info| !info.alias.is_empty())
        .map(|info| info.alias.to_owned())
        .collect();
    assert_eq!(listed, aliases);
    let alice = agent("alice")?;
    for info in operations() {
        if !info.alias.is_empty() {
            let operation = some(operation_of_tool(info.alias), "the alias's operation")?;
            assert_eq!(operation.as_str(), info.id);
        }
        for name in [info.id, info.operator_alias, info.label] {
            if name.is_empty() || aliases.contains(name) {
                continue;
            }
            assert_eq!(operation_of_tool(name), None, "{name} ({})", info.id);
            let result = server.call_tool_as(Some(&alice), name, None).await?;
            assert_eq!(refusal_of(&result)?.code, ErrorCode::NotFound, "{name}");
        }
    }
    assert_eq!(world.application.calls()?, Vec::new());
    Ok(())
}

#[tokio::test]
async fn a_canonical_operation_id_is_not_a_tool_name() -> TestResult {
    let world = reader();
    let server = McpServer::new(world.ports, bundle("tools-canonical.html", "<p>app</p>")?)?;
    for name in ["read_item", "list_drafts", "save_draft", "no_such_tool"] {
        let result = server
            .call_tool_as(Some(&agent("alice")?), name, None)
            .await?;
        assert_eq!(refusal_of(&result)?.code, ErrorCode::NotFound, "{name}");
    }
    assert_eq!(world.application.calls()?, Vec::new());
    Ok(())
}

#[tokio::test]
async fn the_read_tool_answers_through_read_result_within_its_budget() -> TestResult {
    let world = reader();
    let markdown = format!("# Plan\n\n{}", "é".repeat(400));
    world
        .application
        .answer_with("read_item", read_response(&markdown))?;
    let server = McpServer::new(world.ports, bundle("tools-read.html", "<p>app</p>")?)?;
    let result = server
        .call_tool_as(
            Some(&agent("alice")?),
            "show",
            Some(arguments(read_input(256))?),
        )
        .await?;
    assert_eq!(result.is_error, Some(false));
    let text = text_of(&result)?;
    // The whole text block, the cut note included, stays within the budget and nearly fills it.
    assert!(text.len() <= 256, "{} bytes", text.len());
    assert!(text.len() > 250, "only {} bytes", text.len());
    let (kept, note) = some(text.split_once("\n\n[Text cut"), "the cut note")?;
    assert!(markdown.starts_with(kept));
    assert!(note.contains("structuredContent"), "{note}");
    assert_eq!(result.structured_content, Some(read_response(&markdown)));
    assert_eq!(
        world.application.calls()?,
        vec![Call {
            operation: "read_item",
            subject: "alice".to_owned()
        }]
    );
    Ok(())
}

#[tokio::test]
async fn another_tool_answers_through_structured_result() -> TestResult {
    let world = reader();
    world
        .application
        .answer_with("list_workspaces", workspaces_response())?;
    let server = McpServer::new(world.ports, bundle("tools-structured.html", "<p>app</p>")?)?;
    let result = server
        .call_tool_as(
            Some(&agent("alice")?),
            "workspaces",
            Some(arguments(workspaces_input())?),
        )
        .await?;
    assert_eq!(result.is_error, Some(false));
    let text: Value = serde_json::from_str(text_of(&result)?)?;
    assert_eq!(text, workspaces_response());
    assert_eq!(result.structured_content, Some(workspaces_response()));
    Ok(())
}

#[tokio::test]
async fn a_failure_is_error_result_text_without_structured_content() -> TestResult {
    let world = reader();
    let server = McpServer::new(world.ports, bundle("tools-failure.html", "<p>app</p>")?)?;
    let alice = agent("alice")?;
    // The application refuses: no response is configured for read_item.
    let unavailable = server
        .call_tool_as(Some(&alice), "show", Some(arguments(read_input(256))?))
        .await?;
    let refused = refusal_of(&unavailable)?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(refused.message, "No test response configured");
    // Dispatch refuses: the input violates the generated schema.
    let invalid = server
        .call_tool_as(
            Some(&alice),
            "show",
            Some(arguments(json!({"max_bytes": 1}))?),
        )
        .await?;
    assert_eq!(refusal_of(&invalid)?.code, ErrorCode::InvalidInput);
    Ok(())
}

#[tokio::test]
async fn a_proposal_runs_under_the_servers_mutation_ledger() -> TestResult {
    let world = reader();
    world
        .application
        .answer_with("open_proposal", proposal_response())?;
    let ledger = world.ledger.clone();
    let server = McpServer::new(world.ports, bundle("tools-propose.html", "<p>app</p>")?)?;
    let result = server
        .call_tool_as(
            Some(&agent("alice")?),
            "propose",
            Some(arguments(propose_input())?),
        )
        .await?;
    assert_eq!(result.is_error, Some(false), "{result:?}");
    assert_eq!(result.structured_content, Some(proposal_response()));
    assert_eq!(ledger.completed()?.len(), 1);
    Ok(())
}
