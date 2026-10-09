//! The application header of an item file is server-owned (Stage 1b design section 13).
//!
//! Every item file carries one mapping under `APP_HEADER_KEY`: the item id, the archived flag
//! when set, and on a source card the original name and digest and the extraction. Index
//! rebuild and export read it back, so it is written only by the server's own edits
//! (`TreeEdit::SetStatus`, a redigest's `WriteSourceCard`, `CorrectDigest`,
//! `SupplyExtraction`). A caller's write that would change it is refused as `InvalidInput` on
//! the header's field before any port is touched:
//!
//! - `create_item` and `Change::Create` may not supply it at all; the server assigns it.
//! - `save_draft` and `Change::Edit` may leave it out (the stored header is kept) or echo it
//!   unchanged, as `get_item` returned it; any other value is refused.
//! - `set_type` may not declare or require a property of that name at any depth of its schema,
//!   and a type schema is evaluated against an item's properties without the header
//!   (`without_header`), so no type can constrain or refuse it.
//!
//! `VersionStore` applies the same rule to the edits it is handed, so a handler that skipped
//! the check still cannot write a changed header.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use okf_jawn_contract::{
    error::{ApiError, ErrorCode},
    extraction::Extraction,
    identity::{Digest, ItemId},
    item::{APP_HEADER_KEY, TypeDefinition},
};

use crate::stored::{ValidatorCell, decode_stored};

/// The server-owned header of one item file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplicationHeader {
    /// Stable application identity of the item.
    pub item_id: ItemId,
    /// Present and true when the item is archived.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    /// The retained original of a source card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceHeader>,
    /// What turning a source card's bytes into text recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction: Option<Extraction>,
}

/// The original a source card stands for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceHeader {
    /// The name the file was supplied with, kept exactly.
    pub original_name: String,
    /// Digest of the retained original bytes.
    pub digest: Digest,
}

/// JSON Schema keywords whose value is one subschema.
const SCHEMA_KEYWORDS: &[&str] = &[
    "additionalItems",
    "additionalProperties",
    "contains",
    "else",
    "if",
    "not",
    "propertyNames",
    "then",
    "unevaluatedItems",
    "unevaluatedProperties",
];

/// JSON Schema keywords whose value maps names to subschemas. A nested property named like the
/// header is refused too, so the rule needs no `$ref` resolution.
const SCHEMA_MAP_KEYWORDS: &[&str] = &[
    "$defs",
    "definitions",
    "dependencies",
    "dependentSchemas",
    "patternProperties",
    "properties",
];

/// JSON Schema keywords whose value is a list of subschemas (`items` may also be one).
const SCHEMA_LIST_KEYWORDS: &[&str] = &["allOf", "anyOf", "items", "oneOf", "prefixItems"];

impl ApplicationHeader {
    /// The header as the property value stored under `APP_HEADER_KEY`.
    ///
    /// # Errors
    /// Returns `Internal` if the header cannot be represented as JSON.
    pub fn to_property(&self) -> Result<serde_json::Value, ApiError> {
        serde_json::to_value(self).map_err(|error| {
            ApiError::new(
                ErrorCode::Internal,
                format!("the application header did not serialize: {error}"),
            )
        })
    }

    /// The header of a stored item's properties; `None` when the file has none.
    ///
    /// # Errors
    /// Returns `InvalidInput` on the header's field when a value is present but is not a
    /// header: an import shows such a file as needing attention (`UnparseableHeader`).
    pub fn from_properties(
        properties: &BTreeMap<String, serde_json::Value>,
    ) -> Result<Option<Self>, ApiError> {
        static SCHEMA: ValidatorCell = ValidatorCell::new();
        properties
            .get(APP_HEADER_KEY)
            .map(|value| {
                decode_stored(value.clone(), "the application header", &SCHEMA).map_err(|error| {
                    header_error(
                        &format!("the application header does not parse: {}", error.message),
                        "/properties",
                    )
                })
            })
            .transpose()
    }
}

/// Refuse a new item's properties that supply the application header (`create_item`,
/// `Change::Create`).
///
/// `at` is the JSON Pointer of the properties map in the request, such as `/properties` or
/// `/changes/0/properties`.
///
/// # Errors
/// Returns `InvalidInput` on `{at}/okf_jawn` when the key is present.
pub fn refuse_supplied_header(
    properties: &BTreeMap<String, serde_json::Value>,
    at: &str,
) -> Result<(), ApiError> {
    if properties.contains_key(APP_HEADER_KEY) {
        return Err(header_error(
            "the application header is assigned by the server; leave it out of a new item",
            at,
        ));
    }
    Ok(())
}

/// Refuse written properties whose application header differs from the stored item's
/// (`save_draft`, `Change::Edit`, and every caller edit `VersionStore` applies).
///
/// Leaving the header out keeps the stored one; echoing it unchanged is accepted.
///
/// # Errors
/// Returns `InvalidInput` on `{at}/okf_jawn` when the written header is present and is not
/// exactly the stored one, including when the stored item has none.
pub fn refuse_header_change(
    stored: &BTreeMap<String, serde_json::Value>,
    written: &BTreeMap<String, serde_json::Value>,
    at: &str,
) -> Result<(), ApiError> {
    match written.get(APP_HEADER_KEY) {
        None => Ok(()),
        Some(value) if stored.get(APP_HEADER_KEY) == Some(value) => Ok(()),
        Some(_) => Err(header_error(
            "the application header is server-owned and cannot be changed by a write",
            at,
        )),
    }
}

/// Refuse a type definition that declares a property named `APP_HEADER_KEY` (`set_type`).
///
/// # Errors
/// Returns `InvalidInput` on `/definition/properties_schema/properties/okf_jawn`.
pub fn refuse_header_in_type(definition: &TypeDefinition) -> Result<(), ApiError> {
    match header_declaration(
        &definition.properties_schema,
        "/definition/properties_schema",
    ) {
        Some(at) => Err(header_error(
            "a type cannot define or require the server-owned application header",
            &at,
        )),
        None => Ok(()),
    }
}

/// An item's properties without its application header: what a type schema is evaluated
/// against, so no type, whatever its schema (`additionalProperties: false` included), can
/// constrain or refuse the header the server writes.
#[must_use]
pub fn without_header(
    properties: &BTreeMap<String, serde_json::Value>,
) -> BTreeMap<String, serde_json::Value> {
    properties
        .iter()
        .filter(|(key, _)| key.as_str() != APP_HEADER_KEY)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Where `schema` names the header, at any depth of its subschemas (inside `allOf`, `$defs`,
/// `items` and every other applicator): as a key of a `properties` map, or in a `required`
/// list. The result is the JSON Pointer of the map or list, below `at`. Only keywords whose
/// values are schemas are descended; annotations and instance values (`examples`, `default`,
/// `const`, `enum`) are data, so a header-shaped value there declares nothing.
fn header_declaration(schema: &serde_json::Value, at: &str) -> Option<String> {
    let object = schema.as_object()?;
    let declares = object
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|properties| properties.contains_key(APP_HEADER_KEY));
    if declares {
        return Some(format!("{at}/properties"));
    }
    let requires = object
        .get("required")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|required| required.iter().any(|name| name == APP_HEADER_KEY));
    if requires {
        return Some(format!("{at}/required"));
    }
    object.iter().find_map(|(key, value)| {
        let below = format!("{at}/{}", pointer_token(key));
        if SCHEMA_KEYWORDS.contains(&key.as_str()) {
            header_declaration(value, &below)
        } else if SCHEMA_MAP_KEYWORDS.contains(&key.as_str()) {
            value.as_object()?.iter().find_map(|(name, subschema)| {
                header_declaration(subschema, &format!("{below}/{}", pointer_token(name)))
            })
        } else if SCHEMA_LIST_KEYWORDS.contains(&key.as_str()) {
            match value {
                // Draft 4 to 2019-09 `items` may be a list of schemas.
                serde_json::Value::Array(list) => {
                    list.iter().enumerate().find_map(|(index, subschema)| {
                        header_declaration(subschema, &format!("{below}/{index}"))
                    })
                }
                single => header_declaration(single, &below),
            }
        } else {
            None
        }
    })
}

/// A key escaped as one JSON Pointer reference token (RFC 6901).
fn pointer_token(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn header_error(message: &str, at: &str) -> ApiError {
    ApiError::new(ErrorCode::InvalidInput, message).with_field(format!("{at}/{APP_HEADER_KEY}"))
}
