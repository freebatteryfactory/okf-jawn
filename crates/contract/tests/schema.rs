//! Every wire type has one shape: its serialize and deserialize schemas are identical.
//!
//! Package D's generator refuses a type with two shapes; this is the contract-side guard.

use std::error::Error;

use okf_jawn_contract::access::ResourceMetadata;
use okf_jawn_contract::conventions::NamingRules;
use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::extraction::{ConversionSettings, ConverterIdentity, Extraction};
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
    };
}

/// Probe: a bare `Option` is optional when read and required when written.
#[derive(Serialize, Deserialize, JsonSchema)]
struct Asymmetric {
    note: Option<String>,
}

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

#[test]
fn a_field_optional_only_when_read_is_reported_by_type_name() -> Result<(), Box<dyn Error>> {
    let mut split = Vec::new();
    record_split::<Asymmetric>("probe", &mut split)?;
    assert_eq!(split, ["probe (Asymmetric)"]);
    Ok(())
}
