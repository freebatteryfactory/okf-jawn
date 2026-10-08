//! Deterministic OpenAPI and JSON Schema output from the complete Rust operation surface.
//!
//! Every schema is the schemars (draft 2020-12) schema of a contract type, published under the
//! name schemars gives it. A type has one wire shape: generation fails, naming the type, when
//! its serialize and deserialize schemas differ.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use okf_jawn_contract::access::{Permission, ResourceMetadata};
use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::health::HealthResponse;
use okf_jawn_contract::import::Upload;
use okf_jawn_contract::metadata::{OperationInfo, operations};
use okf_jawn_contract::transport::{TransportAuth, TransportOperation};
use okf_jawn_contract::views::Dataset;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde_json::{Map, Value, json};

use crate::output::write_json;

macro_rules! generate_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Pair every row of `operations()` with the schemas of its request and response types.
        fn typed_operations() -> Result<Vec<TypedOperation>, Box<dyn Error>> {
            operations()
                .into_iter()
                .map(|info| {
                    let id = info.id;
                    match id {
                        $(stringify!($id) => typed::<$request, $response>(info),)*
                        other => Err(format!("operation {other} has no declared types").into()),
                    }
                })
                .collect()
        }
    };
}

/// One declared operation with the schemas of its request and response types.
struct TypedOperation {
    info: OperationInfo,
    request: Registered,
    response: Registered,
}

/// One contract type: its component name, its standalone schema document, and every named
/// component (itself and its definitions) it contributes to the OpenAPI document.
struct Registered {
    name: String,
    document: Value,
    components: BTreeMap<String, Value>,
}

/// Types the document refers to outside the operation table.
///
/// `Dataset` is in no operation's schema: a View's dataset is fetched as bytes through
/// `read_object` and parsed with the generated `zDataset`, so it is registered on its own.
struct SharedTypes {
    api_error: Registered,
    upload: Registered,
    resource_metadata: Registered,
    health: Registered,
    dataset: Registered,
}

const COMPONENT_PREFIX: &str = "#/components/schemas/";
const DEFINITION_PREFIX: &str = "#/$defs/";
const TRANSPORT_METHODS: [&str; 4] = ["get", "put", "post", "delete"];
/// Statuses that carry `ApiError`; 501 is the generated "not implemented" outcome.
const ERROR_STATUSES: [&str; 10] = [
    "400", "401", "403", "404", "409", "413", "422", "500", "501", "503",
];
/// The one shared MCP App resource; `structuredContent` selects the feature surface.
const APP_RESOURCE_NAME: &str = "app";
const APP_RESOURCE_URI: &str = "ui://okf-jawn/app.html";
const APP_RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";

pub(crate) fn generate(directory: &Path) -> Result<(), Box<dyn Error>> {
    let typed = typed_operations()?;
    let shared = SharedTypes {
        api_error: register::<ApiError>()?,
        upload: register::<Upload>()?,
        resource_metadata: register::<ResourceMetadata>()?,
        health: register::<HealthResponse>()?,
        dataset: register::<Dataset>()?,
    };
    let document = openapi_document(&typed, &shared)?;
    write_json(&directory.join("openapi.json"), &document)?;
    std::fs::write(
        directory.join("openapi.yaml"),
        yaml_serde::to_string(&document)?,
    )?;
    let schemas = directory.join("schemas");
    for operation in &typed {
        let id = operation.info.id;
        write_json(
            &schemas.join(format!("{id}.input.json")),
            &operation.request.document,
        )?;
        write_json(
            &schemas.join(format!("{id}.output.json")),
            &operation.response.document,
        )?;
    }
    write_json(
        &directory.join("operations.json"),
        &serde_json::to_value(operations())?,
    )?;
    write_json(
        &directory.join("transports.json"),
        &serde_json::to_value(okf_jawn_contract::transport::operations())?,
    )?;
    let tools: Vec<Value> = typed
        .iter()
        .filter(|operation| !operation.info.alias.is_empty())
        .map(tool_definition)
        .collect();
    write_json(&directory.join("mcp-tools.json"), &json!({"tools": tools}))?;
    write_json(&directory.join("mcp-apps.json"), &app_resources())?;
    write_forms(&directory.join("forms"))?;
    crate::fixtures::generate(&directory.join("examples"))?;
    Ok(())
}

/// Assemble the OpenAPI 3.1 document from the registered schemas and both route tables.
fn openapi_document(
    typed: &[TypedOperation],
    shared: &SharedTypes,
) -> Result<Value, Box<dyn Error>> {
    let mut schemas = BTreeMap::new();
    let outside = [
        &shared.api_error,
        &shared.upload,
        &shared.resource_metadata,
        &shared.health,
        &shared.dataset,
    ];
    let declared = typed
        .iter()
        .flat_map(|operation| [&operation.request, &operation.response]);
    for registered in outside.into_iter().chain(declared) {
        for (name, schema) in &registered.components {
            insert_schema(&mut schemas, name.clone(), schema.clone())?;
        }
    }
    let mut paths: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for operation in typed {
        let item = paths.entry(operation.info.path.to_owned()).or_default();
        if item
            .insert("post".to_owned(), path_operation(operation, shared))
            .is_some()
        {
            return Err(format!("{} is declared twice", operation.info.path).into());
        }
    }
    for transport in okf_jawn_contract::transport::operations() {
        if !TRANSPORT_METHODS.contains(&transport.method) {
            return Err(format!(
                "transport {} declares unsupported method {}",
                transport.id, transport.method
            )
            .into());
        }
        let item = paths.entry(transport.path.to_owned()).or_default();
        if item
            .insert(
                transport.method.to_owned(),
                transport_operation(&transport, shared),
            )
            .is_some()
        {
            return Err(
                format!("{} {} is declared twice", transport.method, transport.path).into(),
            );
        }
    }
    Ok(json!({
        "openapi": "3.1.0",
        "info": {
            "title": "okf-jawn",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Complete intended API. A declared route is not a claim of implemented application behavior."
        },
        "paths": paths,
        "security": [{"bearerAuth": []}, {"browserSession": []}],
        "components": {
            "schemas": schemas,
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "Hosted: a WorkOS Connect access token. Local: an opaque connector secret issued by create_connector."
                },
                "browserSession": {
                    "type": "apiKey",
                    "in": "cookie",
                    "name": "okf-session",
                    "description": "Local-owner or WorkOS browser session; cookie-authenticated writes also require the CSRF token."
                }
            }
        }
    }))
}

/// The MCP App resources this build declares, in the shape `rmcp::model::Resource` reads.
fn app_resources() -> Value {
    json!({
        "resources": [{
            "uri": APP_RESOURCE_URI,
            "name": APP_RESOURCE_NAME,
            "mimeType": APP_RESOURCE_MIME_TYPE,
            "_meta": {"ui": {"csp": {"connectDomains": [], "resourceDomains": []}}}
        }]
    })
}

fn write_forms(directory: &Path) -> Result<(), Box<dyn Error>> {
    write_json(
        &directory.join("naming-rules.schema.json"),
        &form_schema::<okf_jawn_contract::conventions::NamingRules>()?,
    )?;
    write_json(
        &directory.join("view.schema.json"),
        &form_schema::<okf_jawn_contract::views::ViewDocument>()?,
    )?;
    write_json(
        &directory.join("type.schema.json"),
        &form_schema::<okf_jawn_contract::item::TypeDefinition>()?,
    )?;
    write_json(
        &directory.join("dataset.schema.json"),
        &form_schema::<Dataset>()?,
    )?;
    Ok(())
}

fn typed<Q: JsonSchema, R: JsonSchema>(
    info: OperationInfo,
) -> Result<TypedOperation, Box<dyn Error>> {
    Ok(TypedOperation {
        info,
        request: register::<Q>()?,
        response: register::<R>()?,
    })
}

/// Register one type under the name schemars gives it.
///
/// Fails when the type, or a type it contains, serializes with a different schema than it
/// deserializes with.
fn register<T: JsonSchema>() -> Result<Registered, Box<dyn Error>> {
    let name = T::schema_name().into_owned();
    let document = schema_document::<T>(false)?;
    let serialized = schema_document::<T>(true)?;
    if document != serialized {
        return Err(format!(
            "{}: serialize and deserialize schemas differ (reached through {name}); give the type one wire shape or two named types",
            differing_types(&name, &document, &serialized).join(", ")
        )
        .into());
    }
    let mut body = document
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{name}: schemars root schema is not an object"))?;
    let definitions = body.remove("$defs");
    body.remove("$schema");
    body.remove("title");
    let mut components = BTreeMap::new();
    if let Some(Value::Object(definitions)) = definitions {
        for (definition, schema) in definitions {
            components.insert(definition, component_references(schema));
        }
    }
    insert_schema(
        &mut components,
        name.clone(),
        component_references(Value::Object(body)),
    )?;
    Ok(Registered {
        name,
        document,
        components,
    })
}

fn schema_document<T: JsonSchema>(serialize: bool) -> Result<Value, serde_json::Error> {
    let settings = SchemaSettings::draft2020_12();
    let settings = if serialize {
        settings.for_serialize()
    } else {
        settings.for_deserialize()
    };
    serde_json::to_value(settings.into_generator().into_root_schema_for::<T>())
}

/// Names of the definitions whose two schemas differ, and the root when its own body differs.
fn differing_types(root: &str, deserialize: &Value, serialize: &Value) -> Vec<String> {
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

fn without_definitions(schema: &Value) -> Value {
    let mut body = schema.clone();
    if let Some(object) = body.as_object_mut() {
        object.remove("$defs");
    }
    body
}

/// Point every local `$defs` reference at the OpenAPI component of the same name.
fn component_references(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, child) in map {
                if key == "$ref"
                    && let Some(reference) = child.as_str()
                    && let Some(name) = reference.strip_prefix(DEFINITION_PREFIX)
                {
                    out.insert(key, Value::String(format!("{COMPONENT_PREFIX}{name}")));
                } else {
                    out.insert(key, component_references(child));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(component_references).collect()),
        other => other,
    }
}

fn component_reference(name: &str) -> Value {
    json!({"$ref": format!("{COMPONENT_PREFIX}{name}")})
}

fn optional(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn insert_schema(
    schemas: &mut BTreeMap<String, Value>,
    name: String,
    schema: Value,
) -> Result<(), Box<dyn Error>> {
    if let Some(previous) = schemas.get(&name) {
        if previous != &schema {
            return Err(format!("Different schemas share the name {name}").into());
        }
    } else {
        schemas.insert(name, schema);
    }
    Ok(())
}

fn path_operation(operation: &TypedOperation, shared: &SharedTypes) -> Value {
    let info = &operation.info;
    let mut responses = Map::new();
    responses.insert(
        info.success_status.to_string(),
        json!({
            "description": "Successful operation result",
            "content": {"application/json": {"schema": component_reference(&operation.response.name)}}
        }),
    );
    for status in ERROR_STATUSES {
        responses.insert(
            status.to_owned(),
            json!({
                "description": "Structured application failure",
                "content": {"application/json": {"schema": component_reference(&shared.api_error.name)}}
            }),
        );
    }
    json!({
        "operationId": info.id,
        "summary": info.label,
        "description": info.description,
        "tags": [info.path.split('/').nth(2).unwrap_or("operations")],
        "requestBody": {
            "required": true,
            "content": {"application/json": {"schema": component_reference(&operation.request.name)}}
        },
        "responses": responses,
        "x-agent-tool": info.visibility == "model",
        "x-mcp-tool": !info.alias.is_empty(),
        "x-tool-visibility": info.visibility,
        "x-agent-alias": info.alias,
        "x-operator-label": info.label,
        "x-cli-alias": optional(info.operator_alias),
        "x-permission": info.permission,
        "x-ui-resource": resource_uri(info.ui),
        "x-tool-annotations": annotations(info)
    })
}

fn annotations(info: &OperationInfo) -> Value {
    let read_only = info.permission == Permission::Read;
    json!({
        "readOnlyHint": read_only,
        "destructiveHint": info.destructive,
        "idempotentHint": read_only,
        "openWorldHint": false
    })
}

fn tool_definition(operation: &TypedOperation) -> Value {
    let mut tool = json!({
        "name": operation.info.alias,
        "title": operation.info.label,
        "description": operation.info.description,
        "inputSchema": operation.request.document,
        "outputSchema": operation.response.document,
        "annotations": annotations(&operation.info)
    });
    if let Some(object) = tool.as_object_mut() {
        let mut ui = json!({"visibility": if operation.info.visibility == "app" { vec!["app"] } else { vec!["model", "app"] }});
        if !operation.info.ui.is_empty()
            && let Some(metadata) = ui.as_object_mut()
        {
            metadata.insert(
                "resourceUri".to_owned(),
                json!(resource_uri(operation.info.ui)),
            );
        }
        object.insert("_meta".to_owned(), json!({"ui":ui}));
    }
    tool
}

fn resource_uri(key: &str) -> Option<&'static str> {
    (!key.is_empty()).then_some(APP_RESOURCE_URI)
}

okf_jawn_contract::for_each_operation!(generate_operations);

fn form_schema<T: JsonSchema>() -> Result<Value, serde_json::Error> {
    // RJSF's default AJV validator consumes Draft 7. The type stays the same.
    serde_json::to_value(
        SchemaSettings::draft07()
            .into_generator()
            .into_root_schema_for::<T>(),
    )
}

fn transport_operation(operation: &TransportOperation, shared: &SharedTypes) -> Value {
    let parameters: Vec<Value> = operation
        .path
        .split('/')
        .filter_map(|part| {
            part.strip_prefix('{').and_then(|part| part.strip_suffix('}')).map(|name|
            json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}}))
        })
        .collect();
    let mut result = json!({"operationId":operation.id,"description":operation.description,
        "summary":operation.id,"tags":["transport"],"parameters":parameters,
        "responses":{(operation.status.to_string()):{"description":operation.description,
            "content":{(operation.response_media):{"schema":transport_response_schema(operation, shared)}}}},
        "x-agent-tool":false,"x-transport-binding":true});
    if let Some(map) = result.as_object_mut() {
        if matches!(operation.auth, TransportAuth::Public) {
            map.insert("security".to_owned(), json!([]));
        }
        if let Some(media) = operation.request_media {
            let schema = if media == "application/octet-stream" {
                json!({"type":"string","format":"binary"})
            } else {
                json!({})
            };
            map.insert(
                "requestBody".to_owned(),
                json!({"required":true,"content":{(media):{"schema":schema}}}),
            );
        }
    }
    result
}

/// The transport table has no response-type column; these three routes return contract types.
fn transport_response_schema(operation: &TransportOperation, shared: &SharedTypes) -> Value {
    match operation.id {
        "upload_content" => component_reference(&shared.upload.name),
        "get_resource_metadata" => component_reference(&shared.resource_metadata.name),
        "liveness" => component_reference(&shared.health.name),
        _ if operation.response_media.contains("json") => json!({}),
        _ => json!({"type": "string"}),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error;
    use std::path::Path;

    use okf_jawn_contract::metadata::operations;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};

    type TestResult = Result<(), Box<dyn Error>>;

    /// Optional when read, required when written: the shape generation must refuse.
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Asymmetric {
        note: Option<String>,
    }

    /// One shape of its own, but it contains a type with two.
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Holder {
        inner: Asymmetric,
    }

    fn read(path: &Path) -> Result<Value, Box<dyn Error>> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }

    /// Every `$ref` target in `value`, in document order.
    fn references(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    if key == "$ref"
                        && let Some(reference) = child.as_str()
                    {
                        found.push(reference.to_owned());
                    } else {
                        references(child, found);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    references(item, found);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }

    #[test]
    fn a_type_with_two_wire_shapes_is_refused_by_name() -> TestResult {
        let Err(direct) = super::register::<Asymmetric>() else {
            return Err("a type with two wire shapes must not register".into());
        };
        let direct = direct.to_string();
        assert!(
            direct.starts_with("Asymmetric: serialize and deserialize schemas differ"),
            "{direct}"
        );
        let Err(nested) = super::register::<Holder>() else {
            return Err("a type containing a two-shape type must not register".into());
        };
        let nested = nested.to_string();
        assert!(
            nested.starts_with("Asymmetric: serialize and deserialize schemas differ"),
            "{nested}"
        );
        assert!(nested.contains("reached through Holder"), "{nested}");
        Ok(())
    }

    #[test]
    fn every_operation_schema_compiles_and_every_reference_resolves() -> TestResult {
        let directory = tempfile::tempdir()?;
        super::generate(directory.path())?;
        assert!(
            !directory.path().join("schemars-splits.json").exists(),
            "no split receipt is written"
        );
        let api = read(&directory.path().join("openapi.json"))?;
        let components = api
            .pointer("/components/schemas")
            .and_then(Value::as_object)
            .ok_or("components.schemas")?;
        let mut published = Vec::new();
        references(&api, &mut published);
        assert!(
            published.len() > operations().len(),
            "the document must reference its components"
        );
        for reference in &published {
            let name = reference
                .strip_prefix(super::COMPONENT_PREFIX)
                .ok_or_else(|| format!("{reference} is not a component reference"))?;
            assert!(
                components.contains_key(name),
                "{reference} names no component"
            );
        }
        for operation in operations() {
            for side in ["input", "output"] {
                let label = format!("{}.{side}", operation.id);
                let schema = read(
                    &directory
                        .path()
                        .join("schemas")
                        .join(format!("{label}.json")),
                )?;
                let definitions = schema.get("$defs").and_then(Value::as_object);
                let mut local = Vec::new();
                references(&schema, &mut local);
                for reference in &local {
                    let name = reference
                        .strip_prefix(super::DEFINITION_PREFIX)
                        .ok_or_else(|| format!("{label}: {reference} is not a local definition"))?;
                    assert!(
                        definitions.is_some_and(|definitions| definitions.contains_key(name)),
                        "{label}: {reference} names no definition"
                    );
                }
                let validator = jsonschema::validator_for(&schema)
                    .map_err(|error| format!("{label}: {error}"))?;
                assert!(
                    !validator.is_valid(&json!(7)),
                    "{label}: every request and response is an object"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn every_example_validates_against_the_operation_it_names() -> TestResult {
        let directory = tempfile::tempdir()?;
        super::generate(directory.path())?;
        let api = read(&directory.path().join("openapi.json"))?;
        let components = api.get("components").ok_or("components")?;
        let table = operations();
        let mut checked = BTreeSet::new();
        for entry in std::fs::read_dir(directory.path().join("examples"))? {
            let path = entry?.path();
            let file = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("example file name")?
                .to_owned();
            let stem = file
                .strip_suffix(".json")
                .ok_or_else(|| format!("{file}: examples are JSON files"))?;
            let id = stem.replace('-', "_");
            let operation = table
                .iter()
                .find(|operation| operation.id == id)
                .ok_or_else(|| format!("{file} names no declared operation"))?;
            let example = read(&path)?;
            let input = read(
                &directory
                    .path()
                    .join("schemas")
                    .join(format!("{id}.input.json")),
            )?;
            let pointer = format!(
                "/paths/{}/post/requestBody/content/application~1json/schema/$ref",
                operation.path.replace('/', "~1")
            );
            let reference = api
                .pointer(&pointer)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{id}: no request component"))?;
            let published = json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "$ref": reference,
                "components": components
            });
            for (source, schema) in [
                ("its input schema", &input),
                ("its OpenAPI component", &published),
            ] {
                let validator = jsonschema::validator_for(schema)
                    .map_err(|error| format!("{file}: {source}: {error}"))?;
                if let Err(error) = validator.validate(&example) {
                    return Err(
                        format!("{file} does not validate against {source}: {error}").into(),
                    );
                }
            }
            checked.insert(id);
        }
        assert!(
            checked.len() >= 3,
            "the generator writes at least its three typed examples, checked {checked:?}"
        );
        Ok(())
    }

    #[test]
    fn hints_aliases_and_the_not_implemented_response_come_from_the_table() -> TestResult {
        let directory = tempfile::tempdir()?;
        super::generate(directory.path())?;
        let api = read(&directory.path().join("openapi.json"))?;
        let mut destructive = 0_usize;
        for operation in operations() {
            let pointer = format!("/paths/{}/post", operation.path.replace('/', "~1"));
            let post = api
                .pointer(&pointer)
                .ok_or_else(|| format!("{}: no path", operation.id))?;
            assert_eq!(
                post.pointer("/x-tool-annotations/destructiveHint"),
                Some(&json!(operation.destructive)),
                "{} destructive hint",
                operation.id
            );
            let alias = if operation.operator_alias.is_empty() {
                Value::Null
            } else {
                json!(operation.operator_alias)
            };
            assert_eq!(
                post.get("x-cli-alias"),
                Some(&alias),
                "{} operator alias",
                operation.id
            );
            assert_eq!(
                post.pointer("/responses/501/content/application~1json/schema/$ref"),
                Some(&json!("#/components/schemas/ApiError")),
                "{} not-implemented response",
                operation.id
            );
            destructive = destructive.saturating_add(usize::from(operation.destructive));
        }
        assert!(destructive > 0, "the table marks destructive operations");
        Ok(())
    }

    #[test]
    fn the_app_declaration_is_an_mcp_resource() -> TestResult {
        let directory = tempfile::tempdir()?;
        super::generate(directory.path())?;
        let declaration = read(&directory.path().join("mcp-apps.json"))?;
        let resources = declaration
            .get("resources")
            .and_then(Value::as_array)
            .ok_or("resources")?;
        assert_eq!(resources.len(), 1);
        let resource = resources.first().ok_or("one resource")?;
        assert_eq!(
            resource,
            &json!({
                "uri": "ui://okf-jawn/app.html",
                "name": "app",
                "mimeType": "text/html;profile=mcp-app",
                "_meta": {"ui": {"csp": {"connectDomains": [], "resourceDomains": []}}}
            })
        );
        let tools = read(&directory.path().join("mcp-tools.json"))?;
        let mut bound = 0_usize;
        for tool in tools
            .get("tools")
            .and_then(Value::as_array)
            .ok_or("tools")?
        {
            if let Some(uri) = tool.pointer("/_meta/ui/resourceUri") {
                assert_eq!(
                    Some(uri),
                    resource.get("uri"),
                    "tool resource is the declared one"
                );
                bound = bound.saturating_add(1);
            }
        }
        assert!(bound > 0, "at least one tool renders in the App");
        Ok(())
    }

    #[test]
    fn the_dataset_is_a_component_and_a_form_though_no_operation_names_it() -> TestResult {
        let directory = tempfile::tempdir()?;
        super::generate(directory.path())?;
        let api = read(&directory.path().join("openapi.json"))?;
        assert!(
            api.pointer("/components/schemas/Dataset").is_some(),
            "Hey API generates zDataset only from a component"
        );
        let form = read(&directory.path().join("forms").join("dataset.schema.json"))?;
        assert_eq!(form.get("title"), Some(&json!("Dataset")));
        let validator = jsonschema::validator_for(&form)?;
        let dataset = json!({
            "schema_version": 1,
            "source": {
                "workspace_id": "11111111-1111-4111-8111-111111111111",
                "item_id": "22222222-2222-4222-8222-222222222222",
                "path": "data/q3.xlsx",
                "revision": "a".repeat(40),
                "selection": {"kind": "all"}
            },
            "text_origin": "converter",
            "columns": [{"name": "revenue", "kind": "number"}],
            "rows": [[1.5], [null]]
        });
        assert!(validator.is_valid(&dataset));
        let mut no_columns = dataset;
        if let Some(columns) = no_columns.get_mut("columns") {
            *columns = json!([]);
        }
        assert!(!validator.is_valid(&no_columns), "a dataset has a column");
        Ok(())
    }

    #[test]
    fn workspace_path_schema_accepts_and_rejects_through_the_generated_schema()
    -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        super::generate(directory.path())?;
        let schema: Value = serde_json::from_slice(&std::fs::read(
            directory
                .path()
                .join("schemas")
                .join("create_item.input.json"),
        )?)?;
        let validator = jsonschema::validator_for(&schema)?;
        let request = |path: &str| {
            json!({
                "workspace_id": "11111111-1111-4111-8111-111111111111",
                "base_revision": "a".repeat(40),
                "path": path,
                "title": "Note",
                "type_name": "note",
                "kind": "note",
                "body": "",
                "properties": {},
                "idempotency_key": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
            })
        };
        for path in ["Clients/one.md", "a", "a/b/c"] {
            assert!(
                validator.is_valid(&request(path)),
                "{path:?} must be accepted"
            );
        }
        for path in [
            "", "/client", "a//b", "a/", "C:/docs", "a\\b", "a\nb", "a<b", "a|b", "why?",
        ] {
            assert!(
                !validator.is_valid(&request(path)),
                "{path:?} must be rejected"
            );
        }
        Ok(())
    }
}
