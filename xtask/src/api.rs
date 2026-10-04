//! Deterministic OpenAPI and JSON Schema output from the complete Rust operation surface.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use okf_jawn_contract::access::Permission;
use okf_jawn_contract::metadata::{OperationInfo, operations};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde_json::{Value, json};
use utoipa::{PartialSchema, ToSchema};
use utoipa::openapi::{ComponentsBuilder, OpenApiBuilder, RefOr};
use utoipa::openapi::info::InfoBuilder;
use utoipa::openapi::schema::Schema;

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
    components: Vec<(String, RefOr<Schema>)>,
    input: Value,
    output: Value,
}

pub(crate) fn generate(directory: &Path) -> Result<(), Box<dyn Error>> {
    let typed = typed_operations()?;
    let mut schemas = BTreeMap::new();
    register::<okf_jawn_contract::error::ApiError>(&mut schemas)?;
    register::<okf_jawn_contract::access::ResourceMetadata>(&mut schemas)?;
    for operation in &typed {
        for (name, schema) in &operation.components { insert_schema(&mut schemas, name.clone(), schema.clone())?; }
    }
    let components = ComponentsBuilder::new().schemas_from_iter(schemas).build();
    let mut api = OpenApiBuilder::new().info(InfoBuilder::new().title("okf-jawn")
        .version(env!("CARGO_PKG_VERSION"))
        .description(Some("Complete intended API. A declared route is not a claim of implemented application behavior."))
        .build()).components(Some(components)).build();
    for operation in &typed {
        let path: utoipa::openapi::path::PathItem = serde_json::from_value(json!({"post": path_operation(operation)}))?;
        api.paths.paths.insert(operation.info.path.to_owned(), path);
        write_json(&directory.join("schemas").join(format!("{}.input.json", operation.info.id)), &operation.input)?;
        write_json(&directory.join("schemas").join(format!("{}.output.json", operation.info.id)), &operation.output)?;
    }
    for transport in okf_jawn_contract::transport::operations() {
        let path: utoipa::openapi::path::PathItem = serde_json::from_value(json!({(transport.method): transport_operation(&transport)}))?;
        let current = api.paths.paths.entry(transport.path.to_owned()).or_default();
        match transport.method {
            "get" => current.get = path.get,
            "put" => current.put = path.put,
            "post" => current.post = path.post,
            "delete" => current.delete = path.delete,
            _ => return Err("Unsupported declared transport method".into()),
        }
    }
    let mut document = serde_json::to_value(api)?;
    let root = document.as_object_mut().ok_or("OpenAPI must serialize to an object")?;
    root.insert("security".to_owned(), json!([{"bearerAuth": []},{"browserSession":[]}]));
    if let Some(components) = root.get_mut("components").and_then(Value::as_object_mut) {
        components.insert("securitySchemes".to_owned(), json!({"bearerAuth":{"type":"http","scheme":"bearer","bearerFormat":"JWT"},"browserSession":{"type":"apiKey","in":"cookie","name":"okf-session"}}));
    }
    write_json(&directory.join("openapi.json"), &document)?;
    std::fs::write(directory.join("openapi.yaml"), yaml_serde::to_string(&document)?)?;
    write_json(&directory.join("operations.json"), &serde_json::to_value(operations())?)?;
    write_json(&directory.join("transports.json"), &serde_json::to_value(okf_jawn_contract::transport::operations())?)?;
    let tools: Vec<Value> = typed.iter().filter(|op| !op.info.alias.is_empty()).map(tool_definition).collect();
    write_json(&directory.join("mcp-tools.json"), &json!({"tools":tools}))?;
    write_json(&directory.join("forms/naming-rules.schema.json"), &form_schema::<okf_jawn_contract::conventions::NamingRules>()?)?;
    write_json(&directory.join("forms/view.schema.json"), &form_schema::<okf_jawn_contract::views::ViewDocument>()?)?;
    write_json(&directory.join("forms/type.schema.json"), &form_schema::<okf_jawn_contract::item::TypeDefinition>()?)?;
    crate::fixtures::generate(&directory.join("examples"))?;
    Ok(())
}

fn typed<Q: JsonSchema + ToSchema, R: JsonSchema + ToSchema>(info: OperationInfo) -> Result<TypedOperation, Box<dyn Error>> {
    let mut components = Vec::new();
    components.push((Q::name().into_owned(), <Q as PartialSchema>::schema())); Q::schemas(&mut components);
    components.push((R::name().into_owned(), <R as PartialSchema>::schema())); R::schemas(&mut components);
    Ok(TypedOperation { info, request_name: Q::name().into_owned(), response_name: R::name().into_owned(),
        components, input: schema::<Q>(false)?, output: schema::<R>(true)? })
}

fn schema<T: JsonSchema>(serialize: bool) -> Result<Value, serde_json::Error> {
    let settings = SchemaSettings::draft2020_12();
    let settings = if serialize { settings.for_serialize() } else { settings };
    serde_json::to_value(settings.into_generator().into_root_schema_for::<T>())
}

fn register<T: ToSchema>(schemas: &mut BTreeMap<String, RefOr<Schema>>) -> Result<(), Box<dyn Error>> {
    insert_schema(schemas, T::name().into_owned(), <T as PartialSchema>::schema())?;
    let mut nested = Vec::new(); T::schemas(&mut nested);
    for (name, schema) in nested { insert_schema(schemas, name, schema)?; }
    Ok(())
}

fn insert_schema(schemas: &mut BTreeMap<String, RefOr<Schema>>, name: String, schema: RefOr<Schema>) -> Result<(), Box<dyn Error>> {
    if let Some(previous) = schemas.get(&name) {
        if serde_json::to_value(previous)? != serde_json::to_value(&schema)? {
            return Err(format!("Different schemas share the name {name}").into());
        }
    } else { schemas.insert(name, schema); }
    Ok(())
}

fn path_operation(operation: &TypedOperation) -> Value {
    let mut responses = serde_json::Map::new();
    responses.insert(operation.info.success_status.to_string(), json!({"description":"Successful operation result",
        "content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{}",operation.response_name)}}}}));
    for status in ["400","401","403","404","409","413","422","500","503"] {
        responses.insert(status.to_owned(), json!({"description":"Structured application failure",
            "content":{"application/json":{"schema":{"$ref":"#/components/schemas/ApiError"}}}}));
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
    let destructive = matches!(info.id, "delete_item" | "restore_items" | "apply_names" | "archive_workspace");
    json!({"readOnlyHint":read_only,"destructiveHint":destructive,
        "idempotentHint":read_only,"openWorldHint":false})
}

fn tool_definition(operation: &TypedOperation) -> Value {
    let mut tool = json!({"name":operation.info.alias,"title":operation.info.label,
        "description":operation.info.description,"inputSchema":operation.input,"outputSchema":operation.output,
        "annotations":annotations(&operation.info)});
    if let Some(object) = tool.as_object_mut() {
        let mut ui = json!({"visibility": if operation.info.visibility == "app" { vec!["app"] } else { vec!["model", "app"] }});
        if !operation.info.ui.is_empty() {
            if let Some(metadata) = ui.as_object_mut() { metadata.insert("resourceUri".to_owned(), json!(resource_uri(operation.info.ui))); }
        }
        object.insert("_meta".to_owned(),json!({"ui":ui}));
    }
    tool
}

fn resource_uri(key: &str) -> Option<String> {
    if key.is_empty() { None } else { Some(format!("ui://okf-jawn/{key}.html")) }
}

okf_jawn_contract::for_each_operation!(generate_operations);


fn form_schema<T: JsonSchema>() -> Result<Value, serde_json::Error> {
    // RJSF's default AJV validator consumes Draft 7. The type stays the same.
    serde_json::to_value(SchemaSettings::draft07().into_generator().into_root_schema_for::<T>())
}

fn transport_operation(operation: &okf_jawn_contract::transport::TransportOperation) -> Value {
    let parameters: Vec<Value> = operation.path.split('/').filter_map(|part| {
        part.strip_prefix('{').and_then(|part| part.strip_suffix('}')).map(|name|
            json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}}))
    }).collect();
    let mut result = json!({"operationId":operation.id,"description":operation.description,
        "summary":operation.id,"tags":["transport"],"parameters":parameters,
        "responses":{(operation.status.to_string()):{"description":operation.description,
            "content":{(operation.response_media):{"schema":transport_response_schema(operation.id, operation.response_media)}}}},
        "x-agent-tool":false,"x-transport-binding":true});
    if let Some(map) = result.as_object_mut() {
        if !operation.authenticated { map.insert("security".to_owned(),json!([])); }
        if let Some(media) = operation.request_media {
            let schema = if media == "application/octet-stream" { json!({"type":"string","format":"binary"}) } else { json!({}) };
            map.insert("requestBody".to_owned(),json!({"required":true,"content":{(media):{"schema":schema}}}));
        }
    }
    result
}


fn transport_response_schema(id: &str, media: &str) -> Value {
    match id {
        "upload_content" => json!({"$ref":"#/components/schemas/Upload"}),
        "get_resource_metadata" => json!({"$ref":"#/components/schemas/ResourceMetadata"}),
        "liveness" => json!({"$ref":"#/components/schemas/HealthResponse"}),
        _ if media.contains("json") => json!({}),
        _ => json!({"type":"string"}),
    }
}


#[cfg(test)]
mod tests {
    use std::error::Error;
    use serde_json::{Value, json};

    #[test]
    fn request_schemas_agree_on_boundary_fixtures() -> Result<(), Box<dyn Error>> {
        let directory=tempfile::tempdir()?;
        super::generate(directory.path())?;
        let input:Value=serde_json::from_slice(&std::fs::read(directory.path().join("schemas/read_item.input.json"))?)?;
        let api:Value=serde_json::from_slice(&std::fs::read(directory.path().join("openapi.json"))?)?;
        let published=json!({"$schema":"https://json-schema.org/draft/2020-12/schema",
            "$ref":"#/components/schemas/ReadItemRequest","components":api.get("components").ok_or("missing components")?});
        let input_validator=jsonschema::validator_for(&input)?;
        let published_validator=jsonschema::validator_for(&published)?;
        let valid=json!({"workspace_id":"11111111-1111-4111-8111-111111111111",
            "item_id":"22222222-2222-4222-8222-222222222222","at":{"kind":"latest"},
            "view":"text","selection":{"kind":"all"},"max_bytes":4096,"max_images":0});
        assert!(input_validator.is_valid(&valid));
        assert!(published_validator.is_valid(&valid));
        let mut invalid=valid;
        invalid.as_object_mut().ok_or("fixture object")?.insert("at".to_owned(),json!({"kind":"revision","revision":"latest"}));
        assert!(!input_validator.is_valid(&invalid));
        assert!(!published_validator.is_valid(&invalid));
        Ok(())
    }
}
