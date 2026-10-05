//! `ApiError` stays small enough to return by value and keeps typed context on the wire.

use std::error::Error;

use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use serde_json::json;

// Clippy's `result_large_err` fires at 128 bytes; every `Result<_, ApiError>` relies on this.
const _: () = assert!(std::mem::size_of::<ApiError>() < 128);

#[test]
fn typed_detail_is_unchanged_on_the_wire() -> Result<(), Box<dyn Error>> {
    let connector_id = serde_json::from_value(json!("44444444-4444-4444-8444-444444444444"))?;
    let error = ApiError::new(ErrorCode::AlreadyIssued, "Connector already issued")
        .with_detail(ErrorDetail::AlreadyIssued { connector_id });
    let detail: &ErrorDetail = error.detail.as_deref().ok_or("detail must be present")?;
    assert_eq!(detail, &ErrorDetail::AlreadyIssued { connector_id });
    assert_eq!(
        serde_json::to_value(&error)?,
        json!({
            "code": "already_issued",
            "message": "Connector already issued",
            "detail": {
                "kind": "already_issued",
                "connector_id": "44444444-4444-4444-8444-444444444444"
            }
        })
    );
    Ok(())
}

#[test]
fn with_field_names_the_input_to_correct() -> Result<(), Box<dyn Error>> {
    let error = ApiError::new(ErrorCode::InvalidInput, "path must be relative").with_field("/path");
    assert_eq!(error.field.as_deref(), Some("/path"));
    assert_eq!(
        serde_json::to_value(&error)?.get("field"),
        Some(&json!("/path"))
    );
    Ok(())
}

#[test]
fn not_implemented_is_a_wire_code_of_its_own() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        serde_json::to_value(ErrorCode::NotImplemented)?,
        json!("not_implemented")
    );
    let decoded: ErrorCode = serde_json::from_value(json!("not_implemented"))?;
    assert_eq!(decoded, ErrorCode::NotImplemented);
    Ok(())
}
