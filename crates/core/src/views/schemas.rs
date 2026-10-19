//! The presentation schemas a View is checked against, embedded from the generator's output in
//! `api/presentation/`: the approved component catalog (`catalog.json`, which `get_catalog`
//! answers), the json-render spec schema (`catalog.schema.json`) and Vega-Lite's own schema
//! (`vega-lite.schema.json`, a verbatim copy of the installed vega-lite package's). Each is
//! decoded and compiled once per process; one that does not is `Internal` (our fault, and a test
//! compiles each).
//!
//! Vega-Lite's schema does not compile as it is under jsonschema 0.58.5: the draft-07
//! meta-schema types `$ref` as a `uri-reference`, 294 of the schema's `$ref` values hold
//! characters a URI fragment may not (`<`, `>`, `|`, `[`, `]`, `"`, as in
//! `#/definitions/MarkPropDef<(Gradient|string|null)>`), and the crate always checks a schema
//! against its meta-schema (no public switch skips it). So those characters are percent-encoded
//! in every `$ref` before compiling (`encode_references`); `referencing` percent-decodes a
//! fragment before it walks the pointer, so each reference still names the same definition.
//! The file itself stays the package's copy. The workspace builds jsonschema without its
//! `resolve-http` and `resolve-file` features, and every reference of these schemas is local.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use jsonschema::Validator;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::views::CatalogResponse;
use serde_json::Value;

/// A value built once per process, or why it could not be.
type Built<T> = OnceLock<Result<T, String>>;

/// The `get_catalog` answer, as the generator wrote it.
const CATALOG: &str = include_str!("../../../../api/presentation/catalog.json");
/// The json-render spec schema over the approved components.
const CATALOG_SCHEMA: &str = include_str!("../../../../api/presentation/catalog.schema.json");
/// Vega-Lite's schema, as the installed package ships it.
const VEGA_LITE_SCHEMA: &str = include_str!("../../../../api/presentation/vega-lite.schema.json");
/// The characters percent-encoded in a Vega-Lite `$ref`, each with its encoding.
const OUTSIDE_A_FRAGMENT: [(char, &str); 6] = [
    ('<', "%3C"),
    ('>', "%3E"),
    ('|', "%7C"),
    ('[', "%5B"),
    (']', "%5D"),
    ('"', "%22"),
];

/// The approved component catalog `get_catalog` answers.
///
/// # Errors
/// Returns `Internal` when the embedded file does not decode as a `CatalogResponse`.
pub fn catalog() -> Result<&'static CatalogResponse, ApiError> {
    static CELL: Built<CatalogResponse> = OnceLock::new();
    built(CELL.get_or_init(|| {
        serde_json::from_str(CATALOG).map_err(|error| format!("catalog.json: {error}"))
    }))
}

/// The validator of a json-render spec over the approved components.
///
/// # Errors
/// Returns `Internal` when the embedded schema does not decode or compile.
pub fn catalog_schema() -> Result<&'static Validator, ApiError> {
    static CELL: Built<Validator> = OnceLock::new();
    built(CELL.get_or_init(|| compile(CATALOG_SCHEMA, "catalog.schema.json", None)))
}

/// The validator of one approved component's props, from the catalog's `properties_schema`;
/// `None` for a name the catalog does not hold.
///
/// # Errors
/// Returns `Internal` when the catalog does not decode or a props schema does not compile.
pub fn component_props(component: &str) -> Result<Option<&'static Validator>, ApiError> {
    static CELL: Built<BTreeMap<String, Validator>> = OnceLock::new();
    let validators = built(CELL.get_or_init(|| {
        let catalog: CatalogResponse =
            serde_json::from_str(CATALOG).map_err(|error| format!("catalog.json: {error}"))?;
        catalog
            .components
            .iter()
            .map(|entry| {
                jsonschema::validator_for(&entry.properties_schema)
                    .map(|validator| (entry.name.clone(), validator))
                    .map_err(|error| format!("the props schema of {}: {error}", entry.name))
            })
            .collect()
    }))?;
    Ok(validators.get(component))
}

/// The validator of a whole Vega-Lite specification (`TopLevelSpec`).
///
/// # Errors
/// Returns `Internal` when the embedded schema does not decode or compile.
pub fn vega_lite_spec() -> Result<&'static Validator, ApiError> {
    static CELL: Built<Validator> = OnceLock::new();
    built(CELL.get_or_init(|| compile(VEGA_LITE_SCHEMA, "vega-lite.schema.json", None)))
}

/// The validator of one Vega-Lite transform (`#/definitions/Transform`), what a binding's
/// `transforms` hold (ruling E3).
///
/// # Errors
/// Returns `Internal` when the embedded schema does not decode or compile.
pub fn vega_lite_transform() -> Result<&'static Validator, ApiError> {
    static CELL: Built<Validator> = OnceLock::new();
    built(CELL.get_or_init(|| {
        compile(
            VEGA_LITE_SCHEMA,
            "vega-lite.schema.json",
            Some("#/definitions/Transform"),
        )
    }))
}

/// Percent-encode, in every `$ref` string of `schema`, the characters a URI fragment may not
/// hold (`OUTSIDE_A_FRAGMENT`); answers how many `$ref` values changed.
pub fn encode_references(schema: &mut Value) -> usize {
    match schema {
        Value::Array(items) => items.iter_mut().map(encode_references).sum(),
        Value::Object(map) => map
            .iter_mut()
            .map(|(key, child)| match child {
                Value::String(reference) if key == "$ref" => {
                    let encoded: String = reference
                        .chars()
                        .map(|character| {
                            OUTSIDE_A_FRAGMENT
                                .iter()
                                .find(|(unsafe_character, _)| *unsafe_character == character)
                                .map_or_else(
                                    || character.to_string(),
                                    |(_, code)| (*code).to_owned(),
                                )
                        })
                        .collect();
                    if encoded == *reference {
                        0
                    } else {
                        *reference = encoded;
                        1
                    }
                }
                other => encode_references(other),
            })
            .sum(),
        _ => 0,
    }
}

/// Decode `text`, percent-encode its references, root it at `root` when given, and compile it.
fn compile(text: &str, name: &str, root: Option<&str>) -> Result<Validator, String> {
    let mut schema: Value =
        serde_json::from_str(text).map_err(|error| format!("{name}: {error}"))?;
    encode_references(&mut schema);
    if let (Some(root), Value::Object(map)) = (root, &mut schema) {
        map.insert("$ref".to_owned(), Value::String(root.to_owned()));
    }
    jsonschema::validator_for(&schema).map_err(|error| format!("{name}: {error}"))
}

/// The value a cell built, or its failure as `Internal`.
fn built<T>(cell: &Result<T, String>) -> Result<&T, ApiError> {
    cell.as_ref()
        .map_err(|message| ApiError::new(ErrorCode::Internal, message.clone()))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        CATALOG, VEGA_LITE_SCHEMA, catalog, catalog_schema, component_props, encode_references,
        vega_lite_spec, vega_lite_transform,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Every `$ref` string of a schema.
    fn references(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Array(items) => items.iter().for_each(|item| references(item, found)),
            Value::Object(map) => {
                for (key, child) in map {
                    match child {
                        Value::String(reference) if key == "$ref" => found.push(reference.clone()),
                        other => references(other, found),
                    }
                }
            }
            _ => {}
        }
    }

    #[test]
    fn the_committed_vega_lite_schema_is_refused_for_its_references() -> TestResult {
        let raw: Value = serde_json::from_str(VEGA_LITE_SCHEMA)?;
        let refused = jsonschema::validator_for(&raw)
            .err()
            .ok_or("the committed schema compiled as it is")?;
        assert!(refused.to_string().contains("uri-reference"), "{refused}");
        Ok(())
    }

    #[test]
    fn encoding_changes_only_the_references_that_need_it() -> TestResult {
        let raw: Value = serde_json::from_str(VEGA_LITE_SCHEMA)?;
        let mut encoded = raw.clone();
        assert_eq!(encode_references(&mut encoded), 294);
        let (mut before, mut after) = (Vec::new(), Vec::new());
        references(&raw, &mut before);
        references(&encoded, &mut after);
        assert_eq!(before.len(), after.len());
        for (old, new) in before.iter().zip(&after) {
            let decoded = new
                .replace("%3C", "<")
                .replace("%3E", ">")
                .replace("%7C", "|")
                .replace("%5B", "[")
                .replace("%5D", "]")
                .replace("%22", "\"");
            assert_eq!(&decoded, old);
            assert!(!new.contains(['<', '>', '|', '[', ']', '"']), "{new}");
        }
        let mut again = encoded.clone();
        assert_eq!(encode_references(&mut again), 0);
        Ok(())
    }

    #[test]
    fn the_embedded_schemas_compile_and_judge_a_chart() -> TestResult {
        let chart = json!({
            "data": { "name": "metrics" },
            "mark": "bar",
            "encoding": {
                "x": { "field": "category", "type": "nominal" },
                "y": { "field": "value", "type": "quantitative" }
            }
        });
        let spec = vega_lite_spec()?;
        assert!(spec.is_valid(&chart));
        assert!(!spec.is_valid(&json!({ "data": { "name": "m" }, "mark": "bogus" })));
        let transform = vega_lite_transform()?;
        assert!(transform.is_valid(&json!({ "filter": "datum.value > 1" })));
        assert!(!transform.is_valid(&json!({ "explode": true })));
        let six: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/views/six-component-spec.json"
        ))?;
        assert!(catalog_schema()?.is_valid(&six));
        let chart_props = component_props("Chart")?.ok_or("the catalog has a Chart")?;
        assert!(!chart_props.is_valid(&json!({ "binding": "m", "chart": "c" })));
        assert!(component_props("Script")?.is_none());
        Ok(())
    }

    #[test]
    fn the_catalog_is_the_generated_file() -> TestResult {
        let answered = serde_json::to_value(catalog()?)?;
        let committed: Value = serde_json::from_str(CATALOG)?;
        assert_eq!(answered, committed);
        Ok(())
    }
}
