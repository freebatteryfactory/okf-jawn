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
//! - `set_type` may not declare a property of that name.
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
        properties
            .get(APP_HEADER_KEY)
            .map(|value| {
                serde_json::from_value(value.clone()).map_err(|error| {
                    header_error(
                        &format!("the application header does not parse: {error}"),
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
    let declares = definition
        .properties_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|properties| properties.contains_key(APP_HEADER_KEY));
    if declares {
        return Err(header_error(
            "a type cannot define the server-owned application header",
            "/definition/properties_schema/properties",
        ));
    }
    Ok(())
}

fn header_error(message: &str, at: &str) -> ApiError {
    ApiError::new(ErrorCode::InvalidInput, message).with_field(format!("{at}/{APP_HEADER_KEY}"))
}
