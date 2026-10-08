//! The operation table is the only source of agent exposure, operator vocabulary and hints.

use std::collections::BTreeSet;
use std::error::Error;

use okf_jawn_contract::access::Permission;
use okf_jawn_contract::metadata::{OperationInfo, OperationName, operations};
use okf_jawn_contract::operations::DRAFT_BEARING;

/// Operations that destroy or overwrite user state, in table order.
const DESTRUCTIVE: &[&str] = &[
    "archive_workspace",
    "restore_workspace",
    "discard_draft",
    "delete_item",
    "restore_items",
    "apply_names",
    "revoke_connector",
];
/// Operator and CLI vocabulary, in table order.
const OPERATOR_ALIASES: &[(&str, &str)] = &[
    ("export_workspace", "export"),
    ("log_items", "timeline"),
    ("diff_items", "changes"),
    ("commit_items", "snapshot"),
    ("restore_items", "rewind"),
    ("blame_item", "who"),
    ("accept_proposal", "approve"),
    ("decline_proposal", "decline"),
    ("create_review", "verify"),
    ("start_import", "import"),
    ("get_attention", "attention"),
];
/// Model tool names, in table order.
const MODEL_TOOLS: &[&str] = &[
    "workspaces",
    "ls",
    "show",
    "sources",
    "grep",
    "links",
    "log",
    "diff",
    "blame",
    "propose",
    "present",
    "catalog",
];

fn declared(id: &str) -> Result<OperationInfo, Box<dyn Error>> {
    operations()
        .into_iter()
        .find(|operation| operation.id == id)
        .ok_or_else(|| format!("{id} is not declared").into())
}

#[test]
fn the_surface_is_69_operations_12_model_tools_and_1_app_tool() {
    let table = operations();
    assert_eq!(table.len(), 69);
    let model: Vec<&str> = table
        .iter()
        .filter(|operation| operation.visibility == "model")
        .map(|operation| operation.alias)
        .collect();
    assert_eq!(model, MODEL_TOOLS);
    let app: Vec<&str> = table
        .iter()
        .filter(|operation| operation.visibility == "app")
        .map(|operation| operation.alias)
        .collect();
    assert_eq!(app, ["read_object"]);
}

#[test]
fn only_read_and_propose_operations_are_visible_to_agents() {
    for operation in operations() {
        assert!(
            matches!(operation.visibility, "" | "model" | "app"),
            "{} has unknown visibility {}",
            operation.id,
            operation.visibility
        );
        assert_eq!(
            operation.alias.is_empty(),
            operation.visibility.is_empty(),
            "{} has a tool alias exactly when it is visible to agents",
            operation.id
        );
        if !operation.visibility.is_empty() {
            assert!(
                matches!(operation.permission, Permission::Read | Permission::Propose),
                "{} is agent-visible but requires {:?}",
                operation.id,
                operation.permission
            );
            assert!(
                !operation.destructive,
                "{} is agent-visible and destructive",
                operation.id
            );
        }
    }
}

#[test]
fn draft_bearing_operations_are_never_agent_tools() -> Result<(), Box<dyn Error>> {
    // `alias` and `visibility` are the agent surface; `operator_alias` is CLI vocabulary.
    // SPEC section 8 names `get_item` as the operation that returns the caller's own draft.
    assert!(DRAFT_BEARING.contains(&OperationName::GetItem));
    let positions: Vec<usize> = DRAFT_BEARING
        .iter()
        .filter_map(|name| OperationName::ALL.iter().position(|each| each == name))
        .collect();
    let distinct: BTreeSet<usize> = positions.iter().copied().collect();
    assert_eq!(distinct.len(), DRAFT_BEARING.len(), "listed once each");
    assert!(positions.is_sorted(), "listed in table order");
    for name in DRAFT_BEARING {
        let id = name.as_str();
        let operation = declared(id)?;
        assert!(
            operation.alias.is_empty(),
            "{id} carries drafts and must not have a tool alias"
        );
        assert!(
            operation.visibility.is_empty(),
            "{id} carries drafts and must not be visible to agents"
        );
    }
    Ok(())
}

#[test]
fn destructive_hints_come_from_the_table() {
    let destructive: Vec<&str> = operations()
        .iter()
        .filter(|operation| operation.destructive)
        .map(|operation| operation.id)
        .collect();
    assert_eq!(destructive, DESTRUCTIVE);
}

#[test]
fn operator_vocabulary_comes_from_the_table_and_names_one_command() {
    let table = operations();
    let aliases: Vec<(&str, &str)> = table
        .iter()
        .filter(|operation| !operation.operator_alias.is_empty())
        .map(|operation| (operation.id, operation.operator_alias))
        .collect();
    assert_eq!(aliases, OPERATOR_ALIASES);
    let mut names = BTreeSet::new();
    for operation in &table {
        let mut own = BTreeSet::from([operation.id]);
        own.extend(
            [operation.alias, operation.operator_alias]
                .into_iter()
                .filter(|name| !name.is_empty()),
        );
        for name in own {
            assert!(names.insert(name), "{name} names more than one command");
        }
    }
}

#[test]
fn the_workspaces_tool_tells_the_agent_to_pin_reads() -> Result<(), Box<dyn Error>> {
    let workspaces = declared("list_workspaces")?;
    assert_eq!(workspaces.alias, "workspaces");
    assert_eq!(workspaces.visibility, "model");
    assert_eq!(workspaces.permission, Permission::Read);
    for phrase in [
        "head revision",
        "pin",
        r#"{"kind": "revision", "revision": head}"#,
        "detect change",
    ] {
        assert!(
            workspaces.description.contains(phrase),
            "the description must say: {phrase}"
        );
    }
    Ok(())
}

#[test]
fn snapshot_states_the_per_item_precondition() -> Result<(), Box<dyn Error>> {
    let commit = declared("commit_items")?;
    assert!(commit.description.contains("since its draft's base"));
    assert!(!commit.description.contains("unchanged base"));
    Ok(())
}
