//! The operation table is the only source of agent exposure, operator vocabulary and hints.

use std::collections::BTreeSet;
use std::error::Error;

use okf_jawn_contract::access::Permission;
use okf_jawn_contract::metadata::{OperationInfo, OperationName, operations};
use okf_jawn_contract::operations::{DRAFT_BEARING, HUMAN_SESSION_ONLY};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde_json::Value;

macro_rules! schema_names {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Every operation with the type names its generated input and output schemas contain.
        fn schema_type_names() -> Result<Vec<(&'static str, BTreeSet<String>)>, serde_json::Error> {
            Ok(vec![$((stringify!($id), {
                let mut names = type_names::<$request>()?;
                names.extend(type_names::<$response>()?);
                names
            })),*])
        }
    };
}

/// SPEC section 8, held by name and independently of `DRAFT_BEARING`: the operations that
/// return, store, list, remove or commit the caller's own drafts.
const EXPECTED_DRAFT_BEARING: &[&str] = &[
    "get_item",
    "save_draft",
    "list_drafts",
    "discard_draft",
    "commit_items",
];
/// The recovery and destruction operations a person runs in a browser session, never an agent
/// or a service identity (ledger 2026-10-08), held by name and independently of the list.
const EXPECTED_HUMAN_SESSION_ONLY: &[&str] = &[
    "purge_workspace",
    "backup_workspace",
    "restore_workspace",
    "backup_installation",
    "purge_item",
];
/// Contract types that hold a draft or its content.
const DRAFT_TYPES: &[&str] = &["Draft", "DraftContent"];
/// Contract types named `Draft…` that hold no draft content, each with why. A new `Draft…` type
/// must be added here or to `DRAFT_TYPES`.
const DRAFT_NAMED_NOT_DRAFT: &[(&str, &str)] = &[(
    "DraftConflictItem",
    "The `draft_conflict` error detail of commit_items, reachable from any schema holding an      ApiError (a Job's error). It names the item, the draft's base revision and the committed      changes since; never the drafted body or properties.",
)];
/// Operations whose schemas name a draft type but which never carry a draft, each with why.
const DRAFT_SCHEMA_EXEMPT: &[(&str, &str)] = &[(
    "create_item",
    "Its response is an ItemDocument, whose optional `draft` field is how get_item returns the      caller's draft. A created item is an immediate commit with a fresh identity, so no draft      of it can exist: the create_item handler must return `draft: None` on every route.",
)];

/// Operations that destroy or overwrite user state, in table order.
const DESTRUCTIVE: &[&str] = &[
    "purge_workspace",
    "restore_workspace",
    "discard_draft",
    "delete_item",
    "purge_item",
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

/// The root title and every `$defs` name of `T`'s generated schema.
fn type_names<T: JsonSchema>() -> Result<BTreeSet<String>, serde_json::Error> {
    let schema = serde_json::to_value(
        SchemaSettings::draft2020_12()
            .into_generator()
            .into_root_schema_for::<T>(),
    )?;
    let mut names: BTreeSet<String> = schema
        .get("$defs")
        .and_then(Value::as_object)
        .map(|defs| defs.keys().cloned().collect())
        .unwrap_or_default();
    if let Some(title) = schema.get("title").and_then(Value::as_str) {
        names.insert(title.to_owned());
    }
    Ok(names)
}

fn declared(id: &str) -> Result<OperationInfo, Box<dyn Error>> {
    operations()
        .into_iter()
        .find(|operation| operation.id == id)
        .ok_or_else(|| format!("{id} is not declared").into())
}

#[test]
fn the_surface_is_77_operations_12_model_tools_and_1_app_tool() {
    let table = operations();
    assert_eq!(table.len(), 77);
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
fn the_draft_bearing_list_is_exactly_the_spec_operations() {
    let listed: Vec<&str> = DRAFT_BEARING.iter().map(|name| name.as_str()).collect();
    assert_eq!(listed, EXPECTED_DRAFT_BEARING);
}

#[test]
fn human_session_only_operations_are_never_agent_tools() -> Result<(), Box<dyn Error>> {
    let positions: Vec<usize> = HUMAN_SESSION_ONLY
        .iter()
        .filter_map(|name| OperationName::ALL.iter().position(|each| each == name))
        .collect();
    let distinct: BTreeSet<usize> = positions.iter().copied().collect();
    assert_eq!(distinct.len(), HUMAN_SESSION_ONLY.len(), "listed once each");
    assert!(positions.is_sorted(), "listed in table order");
    for name in HUMAN_SESSION_ONLY {
        let id = name.as_str();
        let operation = declared(id)?;
        assert!(
            operation.alias.is_empty(),
            "{id} must not have a tool alias"
        );
        assert!(
            operation.visibility.is_empty(),
            "{id} must not be visible to agents"
        );
        assert_eq!(
            operation.permission,
            Permission::Admin,
            "{id} is an administrator's operation"
        );
    }
    Ok(())
}

#[test]
fn the_human_session_only_list_is_exactly_recovery_and_purge() {
    let listed: Vec<&str> = HUMAN_SESSION_ONLY
        .iter()
        .map(|name| name.as_str())
        .collect();
    assert_eq!(listed, EXPECTED_HUMAN_SESSION_ONLY);
}

#[test]
fn no_new_operation_is_an_agent_tool() -> Result<(), Box<dyn Error>> {
    for id in [
        "unarchive_workspace",
        "purge_workspace",
        "get_purge",
        "backup_installation",
        "purge_item",
        "get_tenant_job",
        "list_tenant_jobs",
        "list_tenant_events",
    ] {
        let operation = declared(id)?;
        assert!(
            operation.alias.is_empty() && operation.visibility.is_empty(),
            "{id} is not an MCP tool"
        );
        assert_eq!(operation.permission, Permission::Admin, "{id}");
    }
    Ok(())
}

#[test]
fn agent_tool_descriptions_name_the_unprocessed_filter_and_supplied_text()
-> Result<(), Box<dyn Error>> {
    assert!(
        declared("list_items")?
            .description
            .contains(r#"With extraction = "unprocessed", list only the source files"#)
    );
    assert!(
        declared("search_items")?
            .description
            .contains("The query may be empty only with a filter")
    );
    assert!(
        declared("open_proposal")?
            .description
            .contains("labelled as supplied by an agent")
    );
    Ok(())
}

#[test]
fn every_operation_whose_schema_names_a_draft_type_is_draft_bearing_or_exempt()
-> Result<(), Box<dyn Error>> {
    let bearing: BTreeSet<&str> = DRAFT_BEARING.iter().map(|name| name.as_str()).collect();
    let exempt: BTreeSet<&str> = DRAFT_SCHEMA_EXEMPT.iter().map(|(id, _)| *id).collect();
    let mut found = BTreeSet::new();
    for (id, names) in schema_type_names()? {
        for name in names.iter().filter(|name| name.starts_with("Draft")) {
            assert!(
                DRAFT_TYPES.contains(&name.as_str())
                    || DRAFT_NAMED_NOT_DRAFT.iter().any(|(other, _)| other == name),
                "{id}: classify the new type {name} as a draft type or not"
            );
        }
        let drafts: Vec<&String> = names
            .iter()
            .filter(|name| DRAFT_TYPES.contains(&name.as_str()))
            .collect();
        if drafts.is_empty() {
            continue;
        }
        found.insert(id);
        assert!(
            bearing.contains(id) || exempt.contains(id),
            "{id} names {drafts:?} in its schema but is neither draft-bearing nor exempt"
        );
        assert!(
            !(bearing.contains(id) && exempt.contains(id)),
            "{id} is both draft-bearing and exempt"
        );
    }
    // Every exemption is still needed, and the scan sees the draft types at all.
    for id in &exempt {
        assert!(found.contains(id), "{id} is exempt but names no draft type");
    }
    assert!(found.contains("list_drafts"), "the scan must see Draft");
    assert!(found.contains("get_item"), "the scan must see DraftContent");
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

okf_jawn_contract::for_each_operation!(schema_names);
