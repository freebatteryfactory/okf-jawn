//! Every declared operation's request type states authorization targets and retry identity
//! that agree with the canonical table.
//!
//! Requests are synthesized from each type's own deserialize schema, so a newly declared
//! operation is covered without a hand-written sample.

use okf_jawn_contract::access::{CreateConnectorRequest, Permission};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::metadata::{OperationName, operations};
use okf_jawn_contract::scope::{ReplayPolicy, RequestScope, Target};
use okf_jawn_contract::views::PresentRequest;
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
    };
}

struct Scoped {
    id: &'static str,
    permission: Permission,
    targets: Vec<Target>,
    keyed: bool,
    replay: ReplayPolicy,
}

/// Admin-gated operations that read state and therefore carry no idempotency key.
const PRIVILEGED_READS: &[&str] = &["list_connectors"];
/// Operations whose required permission depends on the request rather than the table column.
const ACTION_DEPENDENT: &[&str] = &["create_confirmation"];
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

fn synthesize_for<T: JsonSchema>() -> Result<Value, Box<dyn Error>> {
    let schema = serde_json::to_value(
        SchemaSettings::draft2020_12()
            .into_generator()
            .into_root_schema_for::<T>(),
    )?;
    let defs = schema.get("$defs").cloned().unwrap_or_else(|| json!({}));
    synthesize(&schema, &defs, 0)
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
    if schema.get("format").and_then(Value::as_str) == Some("uuid") {
        return "11111111-1111-4111-8111-111111111111".to_owned();
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
    assert_eq!(
        <CreateConnectorRequest as RequestScope>::REPLAY,
        ReplayPolicy::AlreadyIssued
    );
    for operation in sampled_operations()? {
        let expected = if operation.id == "create_connector" {
            ReplayPolicy::AlreadyIssued
        } else {
            ReplayPolicy::StoredResponse
        };
        assert_eq!(operation.replay, expected, "{} replay policy", operation.id);
    }
    Ok(())
}

#[test]
fn present_view_authorizes_every_binding_workspace() -> Result<(), Box<dyn Error>> {
    let mut value = synthesize_for::<PresentRequest>()?;
    let binding_workspace = value
        .pointer_mut("/view/bindings/0/source/workspace_id")
        .ok_or("synthesized view has a binding")?;
    *binding_workspace = json!(OTHER_WORKSPACE);
    let request: PresentRequest = serde_json::from_value(value)?;
    let other = serde_json::from_value(json!(OTHER_WORKSPACE))?;
    assert!(
        request
            .targets()
            .contains(&Target::Workspace(other, Permission::Read))
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
