//! Small macros the stores share; declared first so every module sees them.

/// Decode a stored spelling (a UUID, an operation name, a revision) into the type the context
/// names, through that type's own `Deserialize`; a value that does not decode is a store fault.
macro_rules! from_text {
    ($text:expr) => {
        serde_json::from_value(serde_json::Value::String(::std::string::String::from(
            $text,
        )))
        .map_err(|error| $crate::db::json(&error))
    };
}

/// A fresh random identity of the type the context names.
macro_rules! new_id {
    () => {
        serde_json::from_value($crate::seam::new_uuid()).map_err(|error| $crate::db::json(&error))
    };
}

/// The wire spelling of a unit enum value, such as `succeeded` or `workspace_backup`.
macro_rules! variant_text {
    ($value:expr) => {
        serde_json::to_value($value)
            .map_err(|error| $crate::db::json(&error))
            .and_then(|value| {
                value
                    .as_str()
                    .map(::std::borrow::ToOwned::to_owned)
                    .ok_or_else(|| $crate::db::internal("a stored value is not a unit variant"))
            })
    };
}
