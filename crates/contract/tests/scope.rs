//! Every declared operation's request type states authorization targets and retry identity
//! that agree with the canonical table.
//!
//! Requests are synthesized from each type's own deserialize schema, so a newly declared
//! operation is covered without a hand-written sample.

use okf_jawn_contract::access::{CreateConnectorRequest, IssuedConnector, Permission};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::events::ListTenantEventsRequest;
use okf_jawn_contract::identity::ConnectorId;
use okf_jawn_contract::import::{GetTenantJobRequest, ListTenantJobsRequest};
use okf_jawn_contract::item::SetLifecycleRequest;
use okf_jawn_contract::metadata::{OperationName, operations};
use okf_jawn_contract::proposal::OpenProposalRequest;
use okf_jawn_contract::purge::{GetPurgeRequest, PurgeItemRequest, PurgeWorkspaceRequest};
use okf_jawn_contract::scope::{ReplayPolicy, RequestScope, Target};
use okf_jawn_contract::search::SearchRequest;
use okf_jawn_contract::views::PresentRequest;
use okf_jawn_contract::workspace::{
    BackupInstallationRequest, RestoreWorkspaceRequest, UnarchiveWorkspaceRequest,
};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use std::error::Error;
use std::path::PathBuf;

macro_rules! scope_checks {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        fn sampled_operations() -> Result<Vec<Scoped>, Box<dyn Error>> {
            Ok(vec![$(sample::<$request>(stringify!($id), Permission::$permission)?),*])
        }

        fn decode_example(id: &str, value: Value) -> Option<Result<Scoped, serde_json::Error>> {
            match id {
                $(stringify!($id) => Some(serde_json::from_value::<$request>(value)
                    .map(|request| scoped(stringify!($id), Permission::$permission, &request))),)*
                _ => None,
            }
        }

        fn view_carrying_operations() -> Result<Vec<ViewRules>, Box<dyn Error>> {
            let mut found = Vec::new();
            $(if let Some(rules) = view_rules::<$request>(stringify!($id))? {
                found.push(rules);
            })*
            Ok(found)
        }
    };
}

struct Scoped {
    id: &'static str,
    permission: Permission,
    targets: Vec<Target>,
    keyed: bool,
    replay: ReplayPolicy,
}

/// `check_rules` of one View-carrying request, as synthesized and with every binding moved to
/// another workspace.
struct ViewRules {
    id: &'static str,
    own: Result<(), ApiError>,
    foreign: Result<(), ApiError>,
    moved: usize,
}

/// Admin-gated operations that read state and therefore carry no idempotency key.
const PRIVILEGED_READS: &[&str] = &[
    "get_purge",
    "get_tenant_job",
    "list_tenant_jobs",
    "list_tenant_events",
    "list_connectors",
];
/// Operations authorized at the tenant only: their target is gone, or never was a workspace.
const TENANT_ONLY: &[&str] = &[
    "purge_workspace",
    "get_purge",
    "backup_installation",
    "purge_item",
    "get_tenant_job",
    "list_tenant_jobs",
    "list_tenant_events",
];
/// Operations whose required permission depends on the request rather than the table column.
const ACTION_DEPENDENT: &[&str] = &["create_confirmation"];
/// Operations any signed-in principal may call; their results are filtered by grants.
const SIGN_IN_ONLY: &[&str] = &[
    "list_workspaces",
    "get_catalog",
    "get_session",
    "get_health",
    "get_readiness",
];
const OTHER_WORKSPACE: &str = "33333333-3333-4333-8333-333333333333";

fn scoped<T: RequestScope>(id: &'static str, permission: Permission, request: &T) -> Scoped {
    Scoped {
        id,
        permission,
        targets: request.targets(),
        keyed: request.idempotency_key().is_some(),
        replay: T::REPLAY,
    }
}

fn sample<T: DeserializeOwned + JsonSchema + RequestScope>(
    id: &'static str,
    permission: Permission,
) -> Result<Scoped, Box<dyn Error>> {
    let value = synthesize_for::<T>()?;
    let request: T = serde_json::from_value(value)
        .map_err(|error| format!("{id}: synthesized request does not decode: {error}"))?;
    Ok(scoped(id, permission, &request))
}

fn schema_for<T: JsonSchema>() -> Result<Value, serde_json::Error> {
    serde_json::to_value(
        SchemaSettings::draft2020_12()
            .into_generator()
            .into_root_schema_for::<T>(),
    )
}

fn synthesize_for<T: JsonSchema>() -> Result<Value, Box<dyn Error>> {
    let schema = schema_for::<T>()?;
    let defs = schema.get("$defs").cloned().unwrap_or_else(|| json!({}));
    synthesize(&schema, &defs, 0)
}

/// Check the rules of `T` when its wire schema carries a View document; `None` otherwise.
///
/// The synthesized request names one UUID everywhere, so its bindings start in its workspace.
fn view_rules<T: DeserializeOwned + JsonSchema + RequestScope>(
    id: &'static str,
) -> Result<Option<ViewRules>, Box<dyn Error>> {
    let schema = schema_for::<T>()?;
    let carries_view = schema.pointer("/$defs/ViewDocument").is_some()
        || schema.get("title") == Some(&json!("ViewDocument"));
    if !carries_view {
        return Ok(None);
    }
    let own = synthesize_for::<T>()?;
    let mut foreign = own.clone();
    let moved = move_bindings(&mut foreign, OTHER_WORKSPACE);
    let own: T = serde_json::from_value(own)?;
    let foreign: T = serde_json::from_value(foreign)?;
    Ok(Some(ViewRules {
        id,
        own: own.check_rules(),
        foreign: foreign.check_rules(),
        moved,
    }))
}

/// Point the source of every View binding anywhere in `value` at `workspace`; return how many.
fn move_bindings(value: &mut Value, workspace: &str) -> usize {
    match value {
        Value::Object(object) => object.iter_mut().fold(0, |moved, (key, child)| {
            let here = match child {
                Value::Array(bindings) if key == "bindings" => bindings
                    .iter_mut()
                    .filter_map(|binding| binding.pointer_mut("/source/workspace_id"))
                    .fold(0, |count: usize, slot| {
                        *slot = json!(workspace);
                        count.saturating_add(1)
                    }),
                _ => move_bindings(child, workspace),
            };
            moved.saturating_add(here)
        }),
        Value::Array(items) => items.iter_mut().fold(0, |moved, item| {
            moved.saturating_add(move_bindings(item, workspace))
        }),
        _ => 0,
    }
}

/// Build a minimal valid instance: required properties only, one element per array.
fn synthesize(schema: &Value, defs: &Value, depth: u8) -> Result<Value, Box<dyn Error>> {
    if depth > 40 {
        return Err("schema recursion too deep to synthesize".into());
    }
    let next = depth.saturating_add(1);
    let Some(object) = schema.as_object() else {
        return Ok(Value::Null);
    };
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        let name = reference
            .strip_prefix("#/$defs/")
            .ok_or_else(|| format!("unsupported reference {reference}"))?;
        let target = defs
            .get(name)
            .ok_or_else(|| format!("missing definition {name}"))?;
        return synthesize(target, defs, next);
    }
    if let Some(constant) = object.get("const") {
        return Ok(constant.clone());
    }
    if let Some(first) = object
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
    {
        return Ok(first.clone());
    }
    for combinator in ["oneOf", "anyOf", "allOf"] {
        if let Some(first) = object
            .get(combinator)
            .and_then(Value::as_array)
            .and_then(|branches| branches.first())
        {
            return synthesize(first, defs, next);
        }
    }
    let kind = match object.get("type") {
        Some(Value::String(kind)) => kind.as_str(),
        Some(Value::Array(kinds)) => kinds
            .iter()
            .filter_map(Value::as_str)
            .find(|kind| *kind != "null")
            .unwrap_or("null"),
        _ => return Ok(Value::Null),
    };
    match kind {
        "object" => {
            let mut result = Map::new();
            let properties = object.get("properties").and_then(Value::as_object);
            for name in object
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                let property = properties
                    .and_then(|properties| properties.get(name))
                    .ok_or_else(|| format!("required property {name} has no schema"))?;
                result.insert(name.to_owned(), synthesize(property, defs, next)?);
            }
            Ok(Value::Object(result))
        }
        "array" => match object.get("items") {
            Some(items) => Ok(Value::Array(vec![synthesize(items, defs, next)?])),
            None => Ok(json!([])),
        },
        "string" => Ok(Value::String(sample_string(object))),
        "integer" => Ok(object.get("minimum").cloned().unwrap_or_else(|| json!(1))),
        "number" => Ok(json!(0)),
        "boolean" => Ok(json!(true)),
        _ => Ok(Value::Null),
    }
}

fn sample_string(schema: &Map<String, Value>) -> String {
    match schema.get("format").and_then(Value::as_str) {
        Some("uuid") => return "11111111-1111-4111-8111-111111111111".to_owned(),
        Some("date-time") => return "2026-10-08T14:03:07.250Z".to_owned(),
        _ => {}
    }
    match schema.get("pattern").and_then(Value::as_str) {
        Some(pattern) if pattern.contains("{40}") => "a".repeat(40),
        Some(pattern) if pattern.contains("{64}") => "a".repeat(64),
        Some(_) => "local".to_owned(),
        None => "a".to_owned(),
    }
}

fn examples_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../api/examples")
}

okf_jawn_contract::for_each_operation!(scope_checks);

#[test]
fn every_request_names_its_table_permission_first() -> Result<(), Box<dyn Error>> {
    let sampled = sampled_operations()?;
    assert_eq!(sampled.len(), operations().len());
    for operation in &sampled {
        let first = operation
            .targets
            .first()
            .ok_or_else(|| format!("{} declares no authorization target", operation.id))?;
        if SIGN_IN_ONLY.contains(&operation.id) {
            assert_eq!(
                *first,
                Target::Authenticated,
                "{} needs sign-in only",
                operation.id
            );
        } else {
            assert_ne!(
                *first,
                Target::Authenticated,
                "{} must name a grant target",
                operation.id
            );
        }
        if ACTION_DEPENDENT.contains(&operation.id) {
            assert!(
                matches!(first.permission(), Permission::Review | Permission::Approve),
                "{} must require the confirmed action's permission",
                operation.id
            );
        } else {
            assert_eq!(
                first.permission(),
                operation.permission,
                "{} first target must carry the table permission",
                operation.id
            );
        }
    }
    Ok(())
}

#[test]
fn every_mutation_carries_an_idempotency_key() -> Result<(), Box<dyn Error>> {
    for operation in sampled_operations()? {
        let mutation =
            operation.permission != Permission::Read && !PRIVILEGED_READS.contains(&operation.id);
        assert_eq!(
            operation.keyed, mutation,
            "{} idempotency key presence must match whether it mutates",
            operation.id
        );
    }
    Ok(())
}

#[test]
fn every_mutation_example_carries_an_idempotency_key() -> Result<(), Box<dyn Error>> {
    let mut checked = 0_usize;
    for entry in std::fs::read_dir(examples_directory())? {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("example file name")?;
        let id = stem.replace('-', "_");
        let value: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        let Some(decoded) = decode_example(&id, value) else {
            continue;
        };
        let operation = decoded.map_err(|error| format!("{id} example: {error}"))?;
        let mutation =
            operation.permission != Permission::Read && !PRIVILEGED_READS.contains(&operation.id);
        assert_eq!(operation.keyed, mutation, "{id} example idempotency key");
        checked = checked.saturating_add(1);
    }
    assert!(
        checked > 0,
        "expected api/examples to contain request fixtures"
    );
    Ok(())
}

#[test]
fn only_connector_issuance_refuses_to_replay_its_response() -> Result<(), Box<dyn Error>> {
    let issued = ReplayPolicy::AlreadyIssued {
        id_pointer: "/connector/connector_id",
    };
    assert_eq!(<CreateConnectorRequest as RequestScope>::REPLAY, issued);
    for operation in sampled_operations()? {
        let expected = if operation.id == "create_connector" {
            issued
        } else {
            ReplayPolicy::StoredResponse
        };
        assert_eq!(operation.replay, expected, "{} replay policy", operation.id);
    }
    Ok(())
}

#[test]
fn the_already_issued_pointer_finds_the_connector_id_in_the_response() -> Result<(), Box<dyn Error>>
{
    let ReplayPolicy::AlreadyIssued { id_pointer } =
        <CreateConnectorRequest as RequestScope>::REPLAY
    else {
        return Err("create_connector must replay as already_issued".into());
    };
    let response = synthesize_for::<IssuedConnector>()?;
    let issued: IssuedConnector = serde_json::from_value(response.clone())?;
    let found = response
        .pointer(id_pointer)
        .cloned()
        .ok_or_else(|| format!("{id_pointer} is absent from IssuedConnector"))?;
    let connector_id: ConnectorId = serde_json::from_value(found)?;
    assert_eq!(connector_id, issued.connector.connector_id);
    Ok(())
}

#[test]
fn tenant_level_reads_need_sign_in_only() -> Result<(), Box<dyn Error>> {
    assert_eq!(Target::Authenticated.permission(), Permission::Read);
    let mut found = Vec::new();
    for operation in sampled_operations()? {
        if operation.targets.contains(&Target::Authenticated) {
            assert_eq!(
                operation.targets,
                vec![Target::Authenticated],
                "{} mixes sign-in with a grant target",
                operation.id
            );
            assert_eq!(operation.permission, Permission::Read, "{}", operation.id);
            assert!(!operation.keyed, "{} is a read", operation.id);
            found.push(operation.id);
        }
    }
    found.sort_unstable();
    let mut expected = SIGN_IN_ONLY.to_vec();
    expected.sort_unstable();
    assert_eq!(found, expected);
    Ok(())
}

#[test]
fn every_request_carrying_a_view_refuses_a_binding_in_another_workspace()
-> Result<(), Box<dyn Error>> {
    let found = view_carrying_operations()?;
    assert!(
        found.iter().any(|rules| rules.id == "present_view"),
        "the schema scan must find present_view"
    );
    for rules in found {
        assert!(rules.moved > 0, "{}: no binding was moved", rules.id);
        assert!(
            rules.own.is_ok(),
            "{}: a View bound to its own workspace is refused: {:?}",
            rules.id,
            rules.own
        );
        assert!(
            matches!(
                &rules.foreign,
                Err(error) if error.code == ErrorCode::InvalidInput
                    && error.field.as_deref().is_some_and(|field| field.ends_with("/bindings"))
            ),
            "{}: a View bound to another workspace must be invalid input, got {:?}",
            rules.id,
            rules.foreign
        );
    }
    Ok(())
}

#[test]
fn present_view_authorizes_only_its_own_workspace() -> Result<(), Box<dyn Error>> {
    let mut value = synthesize_for::<PresentRequest>()?;
    let binding_workspace = value
        .pointer_mut("/view/bindings/0/source/workspace_id")
        .ok_or("synthesized view has a binding")?;
    *binding_workspace = json!(OTHER_WORKSPACE);
    let request: PresentRequest = serde_json::from_value(value)?;
    // A foreign binding is not authorized: `check_rules` refuses it before any target is asked.
    assert_eq!(
        request.targets(),
        vec![Target::Workspace(request.workspace_id, Permission::Read)]
    );
    assert_eq!(
        request.check_rules().map_err(|error| error.code),
        Err(ErrorCode::InvalidInput)
    );
    Ok(())
}

#[test]
fn confirmation_permission_follows_the_confirmed_action() -> Result<(), Box<dyn Error>> {
    let mut value = synthesize_for::<okf_jawn_contract::review::CreateConfirmationRequest>()?;
    for (action, permission) in [
        ("review", Permission::Review),
        ("accept_proposal", Permission::Approve),
    ] {
        let field = value.get_mut("action").ok_or("action field")?;
        *field = json!(action);
        let request: okf_jawn_contract::review::CreateConfirmationRequest =
            serde_json::from_value(value.clone())?;
        let first = request.targets().first().copied().ok_or("target")?;
        assert_eq!(first.permission(), permission, "{action}");
    }
    Ok(())
}

#[test]
fn connector_issuance_cannot_delegate_more_than_the_issuer_holds() -> Result<(), Box<dyn Error>> {
    let mut value = synthesize_for::<CreateConnectorRequest>()?;
    let allow = value.get_mut("allow_propose").ok_or("allow_propose")?;
    *allow = json!(true);
    let request: CreateConnectorRequest = serde_json::from_value(value)?;
    let targets = request.targets();
    assert_eq!(
        targets.first().copied(),
        Some(Target::Deployment(Permission::Admin))
    );
    for workspace in &request.workspace_ids {
        assert!(targets.contains(&Target::Workspace(*workspace, Permission::Read)));
        assert!(targets.contains(&Target::Workspace(*workspace, Permission::Propose)));
    }
    Ok(())
}

#[test]
fn operation_names_match_the_table_in_order() -> Result<(), Box<dyn Error>> {
    let table = operations();
    assert_eq!(OperationName::ALL.len(), table.len());
    for (name, info) in OperationName::ALL.iter().zip(&table) {
        assert_eq!(name.as_str(), info.id);
        assert_eq!(serde_json::to_value(name)?, json!(info.id));
    }
    Ok(())
}

/// A synthesized request of type `T` with `edit` applied to its JSON before decoding.
fn edited<T: DeserializeOwned + JsonSchema>(
    edit: impl FnOnce(&mut Map<String, Value>),
) -> Result<T, Box<dyn Error>> {
    let mut value = synthesize_for::<T>()?;
    edit(value.as_object_mut().ok_or("a request is an object")?);
    Ok(serde_json::from_value(value)?)
}

#[test]
fn an_empty_search_query_needs_a_filter() -> Result<(), Box<dyn Error>> {
    for query in ["", "   "] {
        let request: SearchRequest = edited(|request| {
            request.insert("query".to_owned(), json!(query));
        })?;
        let refused = request.check_rules();
        assert!(
            matches!(&refused, Err(error) if error.code == ErrorCode::InvalidInput
                && error.field.as_deref() == Some("/query")),
            "{query:?}: {refused:?}"
        );
        let filtered: SearchRequest = edited(|request| {
            request.insert("query".to_owned(), json!(query));
            request.insert("extraction".to_owned(), json!("unprocessed"));
        })?;
        assert!(filtered.check_rules().is_ok(), "{query:?} with a filter");
    }
    let worded: SearchRequest = edited(|request| {
        request.insert("query".to_owned(), json!("revenue"));
    })?;
    assert!(worded.check_rules().is_ok());
    Ok(())
}

#[test]
fn a_supply_proposal_holds_only_supply_changes() -> Result<(), Box<dyn Error>> {
    let supply =
        |item: &str| json!({"kind": "supply_extraction", "item_id": item, "markdown": "# Text"});
    let archive = json!({"kind": "archive", "item_id": "22222222-2222-4222-8222-222222222222"});
    let first = "22222222-2222-4222-8222-222222222222";
    let second = "33333333-3333-4333-8333-333333333333";
    for (changes, accepted) in [
        (json!([supply(first)]), true),
        (json!([supply(first), supply(second)]), true),
        (json!([archive]), true),
        (json!([supply(first), archive]), false),
        (json!([supply(first), supply(first)]), false),
    ] {
        let request: OpenProposalRequest = edited(|request| {
            request.insert("changes".to_owned(), changes.clone());
        })?;
        let checked = request.check_rules();
        if accepted {
            assert!(checked.is_ok(), "{changes}: {checked:?}");
        } else {
            assert!(
                matches!(&checked, Err(error) if error.code == ErrorCode::InvalidInput
                    && error.field.as_deref() == Some("/changes")),
                "{changes}: {checked:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn set_lifecycle_needs_status_or_archived() -> Result<(), Box<dyn Error>> {
    let neither: SetLifecycleRequest = edited(|request| {
        request.remove("status");
        request.remove("archived");
    })?;
    assert!(matches!(
        neither.check_rules(),
        Err(error) if error.code == ErrorCode::InvalidInput
    ));
    for (status, archived) in [
        (Some("deprecated"), None),
        (None, Some(true)),
        (Some("stable"), Some(false)),
    ] {
        let request: SetLifecycleRequest = edited(|request| {
            if let Some(status) = status {
                request.insert("status".to_owned(), json!(status));
            }
            if let Some(archived) = archived {
                request.insert("archived".to_owned(), json!(archived));
            }
        })?;
        assert!(request.check_rules().is_ok(), "{status:?} {archived:?}");
    }
    let other: SetLifecycleRequest = edited(|request| {
        request.insert("status".to_owned(), json!("other"));
    })?;
    assert!(matches!(
        other.check_rules(),
        Err(error) if error.field.as_deref() == Some("/status")
    ));
    Ok(())
}

#[test]
fn purge_authorizes_at_the_tenant_only() -> Result<(), Box<dyn Error>> {
    for operation in sampled_operations()? {
        if TENANT_ONLY.contains(&operation.id) {
            assert_eq!(
                operation.targets,
                vec![Target::Deployment(Permission::Admin)],
                "{} names no workspace target",
                operation.id
            );
        }
    }
    let purge: PurgeWorkspaceRequest =
        serde_json::from_value(synthesize_for::<PurgeWorkspaceRequest>()?)?;
    assert_eq!(purge.targets(), vec![Target::Deployment(Permission::Admin)]);
    assert!(purge.idempotency_key().is_some());
    let item: PurgeItemRequest = serde_json::from_value(synthesize_for::<PurgeItemRequest>()?)?;
    assert_eq!(item.targets(), vec![Target::Deployment(Permission::Admin)]);
    assert!(item.idempotency_key().is_some());
    Ok(())
}

#[test]
fn get_purge_needs_only_the_tenant_grant() -> Result<(), Box<dyn Error>> {
    let request: GetPurgeRequest = serde_json::from_value(synthesize_for::<GetPurgeRequest>()?)?;
    assert_eq!(
        request.targets(),
        vec![Target::Deployment(Permission::Admin)]
    );
    assert!(request.idempotency_key().is_none());
    Ok(())
}

#[test]
fn tenant_events_need_the_tenant_admin_grant() -> Result<(), Box<dyn Error>> {
    let request: ListTenantEventsRequest =
        serde_json::from_value(synthesize_for::<ListTenantEventsRequest>()?)?;
    assert_eq!(
        request.targets(),
        vec![Target::Deployment(Permission::Admin)]
    );
    assert!(request.idempotency_key().is_none());
    Ok(())
}

#[test]
fn backup_installation_and_tenant_jobs_need_the_tenant_admin_grant() -> Result<(), Box<dyn Error>> {
    let backup: BackupInstallationRequest =
        serde_json::from_value(synthesize_for::<BackupInstallationRequest>()?)?;
    assert_eq!(
        backup.targets(),
        vec![Target::Deployment(Permission::Admin)]
    );
    assert!(backup.idempotency_key().is_some());
    let job: GetTenantJobRequest =
        serde_json::from_value(synthesize_for::<GetTenantJobRequest>()?)?;
    assert_eq!(job.targets(), vec![Target::Deployment(Permission::Admin)]);
    let jobs: ListTenantJobsRequest =
        serde_json::from_value(synthesize_for::<ListTenantJobsRequest>()?)?;
    assert_eq!(jobs.targets(), vec![Target::Deployment(Permission::Admin)]);
    Ok(())
}

#[test]
fn restore_reads_only_its_target_workspace() -> Result<(), Box<dyn Error>> {
    let request: RestoreWorkspaceRequest = edited(|request| {
        request.insert("workspace_id".to_owned(), json!(OTHER_WORKSPACE));
    })?;
    // The archive arrives as an upload into the target workspace; no other workspace is named.
    assert_eq!(
        request.targets(),
        vec![Target::Workspace(request.workspace_id, Permission::Admin)]
    );
    assert!(request.idempotency_key().is_some());
    let unarchive: UnarchiveWorkspaceRequest =
        serde_json::from_value(synthesize_for::<UnarchiveWorkspaceRequest>()?)?;
    assert_eq!(
        unarchive.targets(),
        vec![Target::Workspace(unarchive.workspace_id, Permission::Admin)]
    );
    Ok(())
}

#[test]
fn typed_error_details_are_tagged_and_closed() -> Result<(), Box<dyn Error>> {
    let connector_id = serde_json::from_value(json!("44444444-4444-4444-8444-444444444444"))?;
    let error = ApiError::new(ErrorCode::AlreadyIssued, "Connector already issued")
        .with_detail(ErrorDetail::AlreadyIssued { connector_id });
    let value = serde_json::to_value(&error)?;
    assert_eq!(
        value.pointer("/detail/kind"),
        Some(&json!("already_issued"))
    );
    assert_eq!(value.get("code"), Some(&json!("already_issued")));
    let decoded: ApiError = serde_json::from_value(value.clone())?;
    assert_eq!(decoded.detail, error.detail);
    let mut unknown = value;
    unknown
        .pointer_mut("/detail")
        .and_then(Value::as_object_mut)
        .ok_or("detail object")?
        .insert("secret".to_owned(), json!("never"));
    assert!(serde_json::from_value::<ApiError>(unknown).is_err());
    let in_progress: ErrorDetail = serde_json::from_value(json!({
        "kind": "in_progress",
        "mutation_id": "55555555-5555-4555-8555-555555555555",
        "retry_after": 2
    }))?;
    assert!(matches!(
        in_progress,
        ErrorDetail::InProgress { retry_after: 2, .. }
    ));
    Ok(())
}
