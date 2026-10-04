//! Route only declared operations through validation and shared authorization.

use okf_jawn_contract::access::{Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::WorkspaceId;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::OnceLock;

use crate::access::authorize;
use crate::ports::Application;

macro_rules! dispatch_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal, $visibility:literal,
        $permission:ident, $ui:literal, $status:literal, $description:literal)),* $(,)?) => {
        /// Dispatch one declared JSON operation into its typed implementation.
        ///
        /// # Errors
        /// Returns input, authorization, implementation, or output serialization errors.
        pub async fn dispatch(service: &dyn Application, principal: &Principal,
            operation_id: &str, input: Value) -> Result<Value, ApiError> {
            let workspace = request_workspace(&input)?;
            match operation_id {
                $(stringify!($id) => {
                    static VALIDATOR: OnceLock<Result<jsonschema::Validator, String>> = OnceLock::new();
                    authorize(principal, &Permission::$permission, workspace)?;
                    let request: $request = decode_validated(input, &VALIDATOR)?;
                    let response = service.$id(principal, request).await?;
                    serde_json::to_value(response).map_err(|_| ApiError::new(ErrorCode::Internal, "Response serialization failed"))
                }),*
                _ => Err(ApiError::new(ErrorCode::NotFound, "Unknown operation")),
            }
        }
    };
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, ApiError> {
    serde_json::from_value(value)
        .map_err(|error| ApiError::new(ErrorCode::InvalidInput, error.to_string()))
}

fn decode_validated<T: DeserializeOwned + JsonSchema>(
    value: Value,
    cell: &OnceLock<Result<jsonschema::Validator, String>>,
) -> Result<T, ApiError> {
    let validator = cell
        .get_or_init(|| {
            let schema = SchemaSettings::draft2020_12()
                .into_generator()
                .into_root_schema_for::<T>();
            let document = serde_json::to_value(schema).map_err(|error| error.to_string())?;
            jsonschema::validator_for(&document).map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|message| ApiError::new(ErrorCode::Internal, message.clone()))?;
    validator
        .validate(&value)
        .map_err(|error| ApiError::new(ErrorCode::InvalidInput, error.to_string()))?;
    decode(value)
}

fn request_workspace(value: &Value) -> Result<Option<WorkspaceId>, ApiError> {
    let direct = value.get("workspace_id");
    let nested = value
        .get("source")
        .and_then(|source| source.get("workspace_id"));
    direct.or(nested).map(|id| decode(id.clone())).transpose()
}

okf_jawn_contract::for_each_operation!(dispatch_operations);
