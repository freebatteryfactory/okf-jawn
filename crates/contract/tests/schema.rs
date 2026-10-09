//! Every wire type has one shape: its serialize and deserialize schemas are identical.
//!
//! Package D's generator refuses a type with two shapes; this is the contract-side guard.
//! Every object a published schema describes is closed, except the maps named in
//! `DECLARED_MAPS`, so a type that loses `deny_unknown_fields` is caught here and not only by
//! the drift check after someone regenerates.

use std::collections::BTreeSet;
use std::error::Error;

use okf_jawn_contract::access::ResourceMetadata;
use okf_jawn_contract::conventions::NamingRules;
use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::extraction::{ConversionSettings, ConverterIdentity, Extraction};
use okf_jawn_contract::health::HealthResponse;
use okf_jawn_contract::import::Upload;
use okf_jawn_contract::item::TypeDefinition;
use okf_jawn_contract::views::{Dataset, ViewDocument};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

macro_rules! schema_checks {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        fn split_operation_types() -> Result<Vec<String>, Box<dyn Error>> {
            let mut split = Vec::new();
            $(
                record_split::<$request>(concat!(stringify!($id), " request"), &mut split)?;
                record_split::<$response>(concat!(stringify!($id), " response"), &mut split)?;
            )*
            Ok(split)
        }

        /// The schema of every operation input and output, as the generator publishes it.
        fn operation_schemas() -> Result<Vec<(String, Value)>, serde_json::Error> {
            Ok(vec![$(
                (concat!(stringify!($id), ".input").to_owned(), schema_for::<$request>(false)?),
                (concat!(stringify!($id), ".output").to_owned(), schema_for::<$response>(false)?),
            )*])
        }
    };
}

/// Probe: a bare `Option` is optional when read and required when written.
#[derive(Serialize, Deserialize, JsonSchema)]
struct Asymmetric {
    note: Option<String>,
}

/// Objects that are maps by design, as `<type>/<JSON Pointer inside it>`: their keys are data
/// (user properties, units, chart names, converter metadata), never field names, so they cannot
/// be closed. Anything else that is open is a type that lost `deny_unknown_fields`.
const DECLARED_MAPS: &[&str] = &[
    "Change/oneOf/0/properties/properties",
    "Change/oneOf/1/properties/properties",
    "CreateItemRequest/properties/properties",
    // An OKF sources entry exactly as written: OKF defines its fields, and none is dropped.
    "DeclaredSource/properties/entry",
    "DraftContent/properties/properties",
    "ItemDocument/properties/properties",
    "SaveDraftRequest/properties/properties",
    "SourceAppearance/properties/metadata",
    "ViewBinding/properties/units",
    "ViewDocument/properties/charts",
];

fn schema_for<T: JsonSchema>(serialize: bool) -> Result<Value, serde_json::Error> {
    let settings = SchemaSettings::draft2020_12();
    let settings = if serialize {
        settings.for_serialize()
    } else {
        settings.for_deserialize()
    };
    serde_json::to_value(settings.into_generator().into_root_schema_for::<T>())
}

fn without_definitions(schema: &Value) -> Value {
    let mut body = schema.clone();
    if let Some(object) = body.as_object_mut() {
        object.remove("$defs");
    }
    body
}

/// Names of the definitions whose two schemas differ, and the root when its own body differs.
fn differing(root: &str, deserialize: &Value, serialize: &Value) -> Vec<String> {
    let empty = Map::new();
    let before = deserialize
        .get("$defs")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let after = serialize
        .get("$defs")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let mut names: Vec<String> = before
        .keys()
        .chain(after.keys())
        .filter(|key| before.get(*key) != after.get(*key))
        .cloned()
        .collect();
    names.sort();
    names.dedup();
    if without_definitions(deserialize) != without_definitions(serialize) {
        names.push(root.to_owned());
    }
    names
}

fn record_split<T: JsonSchema>(role: &str, split: &mut Vec<String>) -> Result<(), Box<dyn Error>> {
    let deserialize = schema_for::<T>(false)?;
    let serialize = schema_for::<T>(true)?;
    if deserialize != serialize {
        let root = T::schema_name();
        split.push(format!(
            "{role} ({})",
            differing(&root, &deserialize, &serialize).join(", ")
        ));
    }
    Ok(())
}

okf_jawn_contract::for_each_operation!(schema_checks);

#[test]
fn every_request_and_response_has_one_wire_shape() -> Result<(), Box<dyn Error>> {
    let mut split = split_operation_types()?;
    record_split::<ApiError>("ApiError", &mut split)?;
    record_split::<ResourceMetadata>("ResourceMetadata", &mut split)?;
    record_split::<NamingRules>("NamingRules", &mut split)?;
    record_split::<ViewDocument>("ViewDocument", &mut split)?;
    record_split::<TypeDefinition>("TypeDefinition", &mut split)?;
    record_split::<Extraction>("Extraction", &mut split)?;
    record_split::<ConverterIdentity>("ConverterIdentity", &mut split)?;
    record_split::<ConversionSettings>("ConversionSettings", &mut split)?;
    record_split::<Dataset>("Dataset", &mut split)?;
    assert!(
        split.is_empty(),
        "serialize and deserialize schemas differ for: {}",
        split.join("; ")
    );
    Ok(())
}

/// Every object schema in `schema` that does not refuse unknown properties, as
/// `<type>/<pointer>`; `owner` is the type the current subtree belongs to.
fn open_objects(owner: &str, pointer: &str, schema: &Value, found: &mut BTreeSet<String>) {
    match schema {
        Value::Object(object) => {
            let typed_object = match object.get("type") {
                Some(Value::String(kind)) => kind == "object",
                Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "object"),
                _ => false,
            };
            let closed = object.get("additionalProperties") == Some(&Value::Bool(false))
                || object.get("unevaluatedProperties") == Some(&Value::Bool(false));
            if (typed_object || object.contains_key("properties")) && !closed {
                found.insert(format!("{owner}{pointer}"));
            }
            for (key, child) in object {
                match key.as_str() {
                    "$defs" => {
                        for (name, definition) in child.as_object().into_iter().flatten() {
                            open_objects(name, "", definition, found);
                        }
                    }
                    // A map from field name to field schema, not a schema itself.
                    "properties" | "patternProperties" => {
                        for (name, field) in child.as_object().into_iter().flatten() {
                            open_objects(owner, &format!("{pointer}/{key}/{name}"), field, found);
                        }
                    }
                    _ => open_objects(owner, &format!("{pointer}/{key}"), child, found),
                }
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                open_objects(owner, &format!("{pointer}/{index}"), item, found);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn published_schemas() -> Result<Vec<(String, Value)>, serde_json::Error> {
    let mut schemas = operation_schemas()?;
    for (name, schema) in [
        ("ApiError", schema_for::<ApiError>(false)?),
        ("Upload", schema_for::<Upload>(false)?),
        ("ResourceMetadata", schema_for::<ResourceMetadata>(false)?),
        ("HealthResponse", schema_for::<HealthResponse>(false)?),
        ("Dataset", schema_for::<Dataset>(false)?),
        ("NamingRules", schema_for::<NamingRules>(false)?),
        ("ViewDocument", schema_for::<ViewDocument>(false)?),
        ("TypeDefinition", schema_for::<TypeDefinition>(false)?),
    ] {
        schemas.push((name.to_owned(), schema));
    }
    Ok(schemas)
}

#[test]
fn every_published_object_is_closed_except_the_declared_maps() -> Result<(), Box<dyn Error>> {
    let mut open = BTreeSet::new();
    for (label, schema) in published_schemas()? {
        let root = schema
            .get("title")
            .and_then(Value::as_str)
            .map_or(label, str::to_owned);
        open_objects(&root, "", &schema, &mut open);
    }
    let declared: BTreeSet<String> = DECLARED_MAPS.iter().map(|map| (*map).to_owned()).collect();
    let undeclared: Vec<&String> = open.difference(&declared).collect();
    assert!(
        undeclared.is_empty(),
        "open object schemas that are not declared maps: {undeclared:?}"
    );
    let unused: Vec<&String> = declared.difference(&open).collect();
    assert!(
        unused.is_empty(),
        "declared maps no published schema has any more: {unused:?}"
    );
    Ok(())
}

#[test]
fn an_object_that_accepts_unknown_fields_is_reported_by_type_name() {
    let mut open = BTreeSet::new();
    open_objects(
        "Probe",
        "",
        &serde_json::json!({
            "type": "object",
            "properties": {"note": {"type": "string"}},
            "$defs": {"Inner": {"type": "object", "additionalProperties": false}}
        }),
        &mut open,
    );
    assert_eq!(open.into_iter().collect::<Vec<_>>(), ["Probe"]);
}

#[test]
fn a_field_optional_only_when_read_is_reported_by_type_name() -> Result<(), Box<dyn Error>> {
    let mut split = Vec::new();
    record_split::<Asymmetric>("probe", &mut split)?;
    assert_eq!(split, ["probe (Asymmetric)"]);
    Ok(())
}
