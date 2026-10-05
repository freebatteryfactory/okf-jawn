//! Deterministic OpenAPI and JSON Schema output from the complete Rust operation surface.
//!
//! OpenAPI `components.schemas` are built from schemars (draft 2020-12). Request types use the
//! deserialize contract; response types use `for_serialize()`. When the two contracts for one
//! type differ, generation emits `<Name>Input` and `<Name>Output`.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use okf_jawn_contract::access::Permission;
use okf_jawn_contract::metadata::{OperationInfo, operations};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde_json::{Map, Value, json};
use utoipa::openapi::info::InfoBuilder;
use utoipa::openapi::OpenApiBuilder;

use crate::output::write_json;

macro_rules! generate_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal, $visibility:literal,
        $permission:ident, $ui:literal, $status:literal, $description:literal)),* $(,)?) => {
        fn typed_operations() -> Result<Vec<TypedOperation>, Box<dyn Error>> {
            let mut result = Vec::new();
            $(result.push(typed::<$request, $response>(OperationInfo {
                id: stringify!($id), path: $path, label: $label, alias: $alias, visibility: $visibility,
                permission: Permission::$permission, ui: $ui, success_status: $status,
                description: $description,
            })?);)*
            Ok(result)
        }
    };
}

pub(crate) struct TypedOperation {
    info: OperationInfo,
    request_name: String,
    response_name: String,
    /// Named component schemas contributed by this operation (flattened `$defs` + root).
    components: BTreeMap<String, Value>,
    /// Input/Output pairs emitted when serialize≠deserialize for this operation's types.
    splits: SplitLog,
    input: Value,
    output: Value,
}

/// Split names produced when serialize and deserialize contracts differ for one type.
#[derive(Debug, Default)]
struct SplitLog {
    pairs: Vec<(String, String)>,
}

pub(crate) fn generate(directory: &Path) -> Result<(), Box<dyn Error>> {
    let typed = typed_operations()?;
    let mut splits = SplitLog::default();
    let mut schemas = BTreeMap::new();
    register_type::<okf_jawn_contract::error::ApiError>(&mut schemas, &mut splits, true)?;
    register_type::<okf_jawn_contract::access::ResourceMetadata>(&mut schemas, &mut splits, true)?;
    // Transport response refs assume these serialize names.
    register_type::<okf_jawn_contract::import::Upload>(&mut schemas, &mut splits, true)?;
    register_type::<okf_jawn_contract::health::HealthResponse>(&mut schemas, &mut splits, true)?;
    for operation in &typed {
        for (name, schema) in &operation.components {
            insert_schema(&mut schemas, name.clone(), schema.clone())?;
        }
        for (input, output) in &operation.splits.pairs {
            record_split(&mut splits, input, output);
        }
    }
    let mut api = OpenApiBuilder::new()
        .info(
            InfoBuilder::new()
                .title("okf-jawn")
                .version(env!("CARGO_PKG_VERSION"))
                .description(Some(
                    "Complete intended API. A declared route is not a claim of implemented application behavior.",
                ))
                .build(),
        )
        .build();
    for operation in &typed {
        let path: utoipa::openapi::path::PathItem =
            serde_json::from_value(json!({"post": path_operation(operation)}))
                .map_err(|error| format!("operation {}: {error}", operation.info.id))?;
        api.paths
            .paths
            .insert(operation.info.path.to_owned(), path);
        write_json(
            &directory
                .join("schemas")
                .join(format!("{}.input.json", operation.info.id)),
            &operation.input,
        )?;
        write_json(
            &directory
                .join("schemas")
                .join(format!("{}.output.json", operation.info.id)),
            &operation.output,
        )?;
    }
    for transport in okf_jawn_contract::transport::operations() {
        let path: utoipa::openapi::path::PathItem =
            serde_json::from_value(json!({(transport.method): transport_operation(&transport)}))
                .map_err(|error| format!("transport {}: {error}", transport.id))?;
        let current = api
            .paths
            .paths
            .entry(transport.path.to_owned())
            .or_default();
        match transport.method {
            "get" => current.get = path.get,
            "put" => current.put = path.put,
            "post" => current.post = path.post,
            "delete" => current.delete = path.delete,
            _ => return Err("Unsupported declared transport method".into()),
        }
    }
    let mut document = serde_json::to_value(api)?;
    let root = document
        .as_object_mut()
        .ok_or("OpenAPI must serialize to an object")?;
    root.insert(
        "security".to_owned(),
        json!([{"bearerAuth": []},{"browserSession":[]}]),
    );
    let mut components = Map::new();
    components.insert(
        "schemas".to_owned(),
        Value::Object(schemas.into_iter().collect()),
    );
    components.insert(
        "securitySchemes".to_owned(),
        json!({
            "bearerAuth":{"type":"http","scheme":"bearer","description":"Hosted: a WorkOS Connect access token. Local: an opaque connector secret issued by create_connector."},
            "browserSession":{"type":"apiKey","in":"cookie","name":"okf-session","description":"Local-owner or WorkOS browser session; cookie-authenticated writes also require the CSRF token."}
        }),
    );
    root.insert("components".to_owned(), Value::Object(components));
    if !splits.pairs.is_empty() {
        let receipt: Value = json!({
            "input_output_splits": splits.pairs.iter().map(|(input, output)| {
                json!({"input": input, "output": output})
            }).collect::<Vec<_>>()
        });
        write_json(&directory.join("schemars-splits.json"), &receipt)?;
    }
    write_json(&directory.join("openapi.json"), &document)?;
    std::fs::write(
        directory.join("openapi.yaml"),
        yaml_serde::to_string(&document)?,
    )?;
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
        .filter(|op| !op.info.alias.is_empty())
        .map(tool_definition)
        .collect();
    write_json(&directory.join("mcp-tools.json"), &json!({"tools":tools}))?;
    write_json(
        &directory.join("mcp-apps.json"),
        &json!({
            "resources": [{
                "uri": "ui://okf-jawn/app.html",
                "mimeType": "text/html;profile=mcp-app",
                "csp": {
                    "connectDomains": [],
                    "resourceDomains": []
                }
            }]
        }),
    )?;
    write_json(
        &directory.join("forms/naming-rules.schema.json"),
        &form_schema::<okf_jawn_contract::conventions::NamingRules>()?,
    )?;
    write_json(
        &directory.join("forms/view.schema.json"),
        &form_schema::<okf_jawn_contract::views::ViewDocument>()?,
    )?;
    write_json(
        &directory.join("forms/type.schema.json"),
        &form_schema::<okf_jawn_contract::item::TypeDefinition>()?,
    )?;
    crate::fixtures::generate(&directory.join("examples"))?;
    Ok(())
}

fn typed<Q: JsonSchema, R: JsonSchema>(
    info: OperationInfo,
) -> Result<TypedOperation, Box<dyn Error>> {
    let mut splits = SplitLog::default();
    let mut components = BTreeMap::new();
    let request_name = register_request_response::<Q>(&mut components, &mut splits, false)?;
    let response_name = register_request_response::<R>(&mut components, &mut splits, true)?;
    Ok(TypedOperation {
        info,
        request_name,
        response_name,
        components,
        splits,
        input: schema::<Q>(false)?,
        output: schema::<R>(true)?,
    })
}

fn register_type<T: JsonSchema>(
    schemas: &mut BTreeMap<String, Value>,
    splits: &mut SplitLog,
    prefer_serialize: bool,
) -> Result<String, Box<dyn Error>> {
    let mut local = BTreeMap::new();
    let name = register_request_response::<T>(&mut local, splits, prefer_serialize)?;
    for (n, s) in local {
        insert_schema(schemas, n, s)?;
    }
    Ok(name)
}

/// Register one type. When used as a response (`prefer_serialize`), the serialize contract is
/// primary; when used as a request, the deserialize contract is. If both contracts differ, both
/// `Input` and `Output` names are registered and the preferred name is returned. Nested `$defs`
/// that differ between contracts also receive Input/Output suffixes.
fn register_request_response<T: JsonSchema>(
    schemas: &mut BTreeMap<String, Value>,
    splits: &mut SplitLog,
    prefer_serialize: bool,
) -> Result<String, Box<dyn Error>> {
    let base = T::schema_name().into_owned();
    let mut de = flatten_root::<T>(false)?;
    let mut ser = flatten_root::<T>(true)?;
    let def_names: BTreeMap<String, ()> = de
        .defs
        .keys()
        .chain(ser.defs.keys())
        .map(|k| (k.clone(), ()))
        .collect();
    let mut rename_de = BTreeMap::new();
    let mut rename_ser = BTreeMap::new();
    for name in def_names.keys() {
        let de_def = de.defs.get(name);
        let ser_def = ser.defs.get(name);
        match (de_def, ser_def) {
            (Some(a), Some(b)) if a == b => {
                // Placeholder; rewritten after rename maps are complete.
                let _ = (a, b);
            }
            (Some(_), Some(_)) => {
                let input = format!("{name}Input");
                let output = format!("{name}Output");
                record_split(splits, &input, &output);
                rename_de.insert(name.clone(), input);
                rename_ser.insert(name.clone(), output);
            }
            (Some(_), None) | (None, Some(_)) | (None, None) => {}
        }
    }
    // Apply renames to def bodies, then insert under final names.
    for name in def_names.keys() {
        let de_def = de.defs.get(name).cloned();
        let ser_def = ser.defs.get(name).cloned();
        match (de_def, ser_def) {
            (Some(a), Some(b)) if a == b => {
                insert_schema(schemas, name.clone(), apply_renames(a, &rename_de))?;
            }
            (Some(a), Some(b)) => {
                let input = rename_de.get(name).cloned().unwrap_or_else(|| name.clone());
                let output = rename_ser.get(name).cloned().unwrap_or_else(|| name.clone());
                insert_schema(schemas, input, apply_renames(a, &rename_de))?;
                insert_schema(schemas, output, apply_renames(b, &rename_ser))?;
            }
            (Some(a), None) => {
                insert_schema(schemas, name.clone(), apply_renames(a, &rename_de))?;
            }
            (None, Some(b)) => {
                insert_schema(schemas, name.clone(), apply_renames(b, &rename_ser))?;
            }
            (None, None) => {}
        }
    }
    de.root = apply_renames(de.root, &rename_de);
    ser.root = apply_renames(ser.root, &rename_ser);
    if de.root == ser.root {
        insert_schema(schemas, base.clone(), de.root)?;
        return Ok(base);
    }
    let input_name = format!("{base}Input");
    let output_name = format!("{base}Output");
    record_split(splits, &input_name, &output_name);
    insert_schema(schemas, input_name.clone(), de.root)?;
    insert_schema(schemas, output_name.clone(), ser.root)?;
    Ok(if prefer_serialize {
        output_name
    } else {
        input_name
    })
}

fn record_split(splits: &mut SplitLog, input: &str, output: &str) {
    if !splits
        .pairs
        .iter()
        .any(|(i, o)| i == input && o == output)
    {
        splits.pairs.push((input.to_owned(), output.to_owned()));
    }
}

fn apply_renames(value: Value, renames: &BTreeMap<String, String>) -> Value {
    if renames.is_empty() {
        return value;
    }
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, child) in map {
                if key == "$ref"
                    && let Some(reference) = child.as_str()
                    && let Some(name) = reference.strip_prefix("#/components/schemas/")
                    && let Some(new_name) = renames.get(name)
                {
                    out.insert(
                        key,
                        Value::String(format!("#/components/schemas/{new_name}")),
                    );
                } else {
                    out.insert(key, apply_renames(child, renames));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| apply_renames(item, renames))
                .collect(),
        ),
        other => other,
    }
}

struct Flattened {
    root: Value,
    defs: BTreeMap<String, Value>,
}

fn flatten_root<T: JsonSchema>(serialize: bool) -> Result<Flattened, Box<dyn Error>> {
    let root = schema::<T>(serialize)?;
    let mut object = root
        .as_object()
        .cloned()
        .ok_or("schemars root must be an object")?;
    let defs_value = object.remove("$defs").unwrap_or_else(|| json!({}));
    let mut defs = BTreeMap::new();
    if let Some(map) = defs_value.as_object() {
        for (name, def) in map {
            defs.insert(name.clone(), rewrite_refs(def.clone()));
        }
    }
    object.remove("$schema");
    object.remove("title");
    let body = rewrite_refs(Value::Object(object));
    Ok(Flattened {
        root: body,
        defs,
    })
}

fn rewrite_refs(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, child) in map {
                if key == "$ref"
                    && let Some(reference) = child.as_str()
                    && let Some(name) = reference.strip_prefix("#/$defs/")
                {
                    out.insert(
                        key,
                        Value::String(format!("#/components/schemas/{name}")),
                    );
                } else {
                    out.insert(key, rewrite_refs(child));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(rewrite_refs).collect()),
        other => other,
    }
}

fn schema<T: JsonSchema>(serialize: bool) -> Result<Value, serde_json::Error> {
    let settings = SchemaSettings::draft2020_12();
    let settings = if serialize {
        settings.for_serialize()
    } else {
        settings
    };
    serde_json::to_value(settings.into_generator().into_root_schema_for::<T>())
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

fn path_operation(operation: &TypedOperation) -> Value {
    let mut responses = serde_json::Map::new();
    responses.insert(operation.info.success_status.to_string(), json!({"description":"Successful operation result",
        "content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{}",operation.response_name)}}}}));
    for status in [
        "400", "401", "403", "404", "409", "413", "422", "500", "503",
    ] {
        responses.insert(
            status.to_owned(),
            json!({"description":"Structured application failure",
            "content":{"application/json":{"schema":{"$ref":"#/components/schemas/ApiError"}}}}),
        );
    }
    json!({"operationId":operation.info.id, "summary":operation.info.label,
        "description":operation.info.description, "tags":[operation.info.path.split('/').nth(2).unwrap_or("operations")],
        "requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{}",operation.request_name)}}}},
        "responses":responses, "x-agent-tool":operation.info.visibility == "model",
        "x-mcp-tool":!operation.info.alias.is_empty(),"x-tool-visibility":operation.info.visibility,
        "x-agent-alias":operation.info.alias,"x-operator-label":operation.info.label,
        "x-cli-alias":okf_jawn_contract::labels::operator_alias(operation.info.id),
        "x-permission":operation.info.permission,"x-ui-resource":resource_uri(operation.info.ui),
        "x-tool-annotations":annotations(&operation.info)})
}

fn annotations(info: &OperationInfo) -> Value {
    let read_only = info.permission == Permission::Read;
    let destructive = matches!(
        info.id,
        "delete_item" | "restore_items" | "apply_names" | "archive_workspace"
    );
    json!({"readOnlyHint":read_only,"destructiveHint":destructive,
        "idempotentHint":read_only,"openWorldHint":false})
}

fn tool_definition(operation: &TypedOperation) -> Value {
    let mut tool = json!({"name":operation.info.alias,"title":operation.info.label,
        "description":operation.info.description,"inputSchema":operation.input,"outputSchema":operation.output,
        "annotations":annotations(&operation.info)});
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

fn resource_uri(key: &str) -> Option<String> {
    if key.is_empty() {
        None
    } else {
        // One shared App resource; structuredContent selects the feature surface.
        Some("ui://okf-jawn/app.html".to_owned())
    }
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

fn transport_operation(operation: &okf_jawn_contract::transport::TransportOperation) -> Value {
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
            "content":{(operation.response_media):{"schema":transport_response_schema(operation.id, operation.response_media)}}}},
        "x-agent-tool":false,"x-transport-binding":true});
    if let Some(map) = result.as_object_mut() {
        if matches!(
            operation.auth,
            okf_jawn_contract::transport::TransportAuth::Public
        ) {
            map.insert("security".to_owned(), json!([]));
        }
        if let Some(media) = operation.request_media {
            let schema = if media == "application/octet-stream" {
                json!({"type":"string","format":"binary"})
            } else {
                any_value_schema()
            };
            map.insert(
                "requestBody".to_owned(),
                json!({"required":true,"content":{(media):{"schema":schema}}}),
            );
        }
    }
    result
}

fn transport_response_schema(id: &str, media: &str) -> Value {
    match id {
        "upload_content" => json!({"$ref":"#/components/schemas/Upload"}),
        "get_resource_metadata" => json!({"$ref":"#/components/schemas/ResourceMetadata"}),
        "liveness" => json!({"$ref":"#/components/schemas/HealthResponse"}),
        _ if media.contains("json") => any_value_schema(),
        _ => json!({"type":"string"}),
    }
}

fn any_value_schema() -> Value {
    // utoipa 6 omits `type` when serializing SchemaType::AnyValue but requires the field when
    // deserializing; null is that untagged unit variant and is published as `{}`.
    json!({"type": null})
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use std::error::Error;
    use std::path::PathBuf;

    #[test]
    fn request_schemas_agree_on_all_example_fixtures() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        super::generate(directory.path())?;
        let api: Value =
            serde_json::from_slice(&std::fs::read(directory.path().join("openapi.json"))?)?;
        let components = api.get("components").ok_or("missing components")?;
        let examples_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../api/examples");
        let mut checked = 0usize;
        for entry in std::fs::read_dir(&examples_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or("example stem")?;
            // Fixture files use kebab-case operation ids (read-item.json → read_item).
            let operation_id = stem.replace('-', "_");
            let input_path = directory
                .path()
                .join("schemas")
                .join(format!("{operation_id}.input.json"));
            if !input_path.exists() {
                continue;
            }
            let input: Value = serde_json::from_slice(&std::fs::read(&input_path)?)?;
            let fixture: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            let paths = api
                .pointer("/paths")
                .and_then(Value::as_object)
                .ok_or("paths")?;
            let mut request_ref = None;
            for item in paths.values() {
                if let Some(post) = item.get("post")
                    && post.get("operationId").and_then(Value::as_str) == Some(operation_id.as_str())
                {
                    request_ref = post
                        .pointer("/requestBody/content/application~1json/schema/$ref")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    break;
                }
            }
            let request_ref = request_ref.ok_or_else(|| format!("no path for {operation_id}"))?;
            let name = request_ref
                .strip_prefix("#/components/schemas/")
                .ok_or("bad ref")?;
            let published = json!({
                "$schema":"https://json-schema.org/draft/2020-12/schema",
                "$ref": format!("#/components/schemas/{name}"),
                "components": components
            });
            let input_validator = jsonschema::validator_for(&input)?;
            let published_validator = jsonschema::validator_for(&published)?;
            assert!(
                input_validator.is_valid(&fixture),
                "{operation_id} fixture must validate against schemars input"
            );
            assert!(
                published_validator.is_valid(&fixture),
                "{operation_id} fixture must validate against OpenAPI component {name}"
            );
            checked += 1;
        }
        assert!(
            checked > 0,
            "expected at least one api/examples fixture to be checked"
        );
        Ok(())
    }
}
