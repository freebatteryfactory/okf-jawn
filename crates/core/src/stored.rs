//! Values read back from storage meet their schema, not only serde.
//!
//! Serde's `deny_unknown_fields` does not reach the unit variants of an internally tagged enum:
//! `{"kind": "rebuild_index", "unexpected": 1}` decodes as `JobSpec::RebuildIndex`, and a
//! `{"status": "completed", ...}` outcome keeps whatever sits beside the tag. A value written by
//! an older or faulty writer, or edited in Git, would then be accepted silently. Every value
//! core decodes from a store (a job specification, a conversion record, an application header)
//! is first validated against the JSON Schema of its own type, as dispatch validates requests;
//! a stored value that fails is a fault of the store (`Internal`), named by its JSON Pointer.

use std::sync::OnceLock;

use schemars::{JsonSchema, generate::SchemaSettings};
use serde::de::DeserializeOwned;
use serde_json::Value;

use okf_jawn_contract::error::{ApiError, ErrorCode};

use crate::echo::bounded;

/// A compiled validator for one type, built on first use.
pub type ValidatorCell = OnceLock<Result<jsonschema::Validator, String>>;

/// Validate `value` against `T`'s schema, then decode it.
///
/// `what` names the stored value in the error, such as `a stored job specification`.
///
/// The text of the violation or decode error quotes the stored value, so it is cut to
/// [`crate::echo::ECHO_LIMIT`] bytes.
///
/// # Errors
/// Returns `Internal` naming the first violation's JSON Pointer when the value does not meet
/// the schema or does not decode, and when the schema itself cannot be compiled.
pub fn decode_stored<T: DeserializeOwned + JsonSchema>(
    value: Value,
    what: &str,
    cell: &ValidatorCell,
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
    if let Err(error) = validator.validate(&value) {
        let refused = ApiError::new(
            ErrorCode::Internal,
            format!(
                "{what} does not meet its schema: {}",
                bounded(error.to_string())
            ),
        );
        let location = error.instance_path().as_str().to_owned();
        return Err(if location.is_empty() {
            refused
        } else {
            refused.with_field(location)
        });
    }
    serde_json::from_value(value).map_err(|error| {
        ApiError::new(
            ErrorCode::Internal,
            format!("{what} does not decode: {}", bounded(error.to_string())),
        )
    })
}
