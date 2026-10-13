//! Wire names and bounds the bindings share with every client, pinned to their published values.
//!
//! A change to one of these is a wire change: the server, workspace-ui and agents read them, so
//! a new value needs a deliberate contract change, not an edit that a test silently follows.
//! Every write field carrying an item's text is checked at `MAX_ITEM_TEXT_BYTES` and one byte
//! over it, with a text whose character count is still within the bound, so the check is seen
//! to count bytes.

use check::{TestResult, err_of, some};
use okf_jawn_contract::access::{CSRF_COOKIE, CSRF_HEADER};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{LOCAL_TENANT_ID, TenantId};
use okf_jawn_contract::import::CorrectDigestRequest;
use okf_jawn_contract::item::{CreateItemRequest, MAX_ITEM_TEXT_BYTES, SaveDraftRequest};
use okf_jawn_contract::proposal::OpenProposalRequest;
use okf_jawn_contract::scope::RequestScope;
use okf_jawn_contract::transport::MAX_REQUEST_BODY_BYTES;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const ITEM: &str = "22222222-2222-4222-8222-222222222222";
const OTHER_ITEM: &str = "33333333-3333-4333-8333-333333333333";
const KEY: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

fn revision() -> String {
    "a".repeat(40)
}

fn create_item() -> Value {
    json!({
        "workspace_id": WORKSPACE, "base_revision": revision(), "path": "notes/a.md",
        "title": "a", "type_name": "note", "kind": "note", "body": "", "properties": {},
        "idempotency_key": KEY
    })
}

fn save_draft() -> Value {
    json!({
        "workspace_id": WORKSPACE, "item_id": ITEM, "base_revision": revision(), "body": "",
        "properties": {}, "idempotency_key": KEY
    })
}

fn correct_digest() -> Value {
    json!({
        "workspace_id": WORKSPACE, "item_id": ITEM, "base_revision": revision(),
        "digest": "a".repeat(64), "corrected_markdown": "", "idempotency_key": KEY
    })
}

fn proposal(changes: &Value) -> Value {
    json!({
        "workspace_id": WORKSPACE, "base_revision": revision(), "title": "t",
        "description": "d", "changes": changes, "idempotency_key": KEY
    })
}

fn supply(item: &str) -> Value {
    json!({"kind": "supply_extraction", "item_id": item, "markdown": ""})
}

/// `check_rules` of the request `template` with `text` at `pointer`.
fn checked<T: DeserializeOwned + RequestScope>(
    template: &Value,
    pointer: &str,
    text: &str,
) -> Result<Result<(), ApiError>, Box<dyn std::error::Error>> {
    let mut value = template.clone();
    *some(value.pointer_mut(pointer), pointer)? = json!(text);
    let request: T = serde_json::from_value(value)?;
    Ok(request.check_rules())
}

/// A text of exactly the bound passes; one byte more is `too_large` on `pointer`, although it
/// has no more characters than the bound.
fn assert_bounded<T: DeserializeOwned + RequestScope>(
    operation: &str,
    template: &Value,
    pointer: &str,
) -> TestResult {
    let within = "a".repeat(MAX_ITEM_TEXT_BYTES);
    let accepted = checked::<T>(template, pointer, &within)?;
    assert!(
        accepted.is_ok(),
        "{operation} {pointer}: a text of exactly the bound is refused: {accepted:?}"
    );
    let mut over = "a".repeat(MAX_ITEM_TEXT_BYTES.saturating_sub(1));
    over.push('é');
    assert_eq!(over.len(), MAX_ITEM_TEXT_BYTES.saturating_add(1));
    assert_eq!(over.chars().count(), MAX_ITEM_TEXT_BYTES);
    let refused = err_of(checked::<T>(template, pointer, &over)?)?;
    assert_eq!(refused.code, ErrorCode::TooLarge, "{operation} {pointer}");
    assert_eq!(
        refused.field.as_deref(),
        Some(pointer),
        "{operation}: the refusal names the field"
    );
    Ok(())
}

fn schema_for<T: JsonSchema>() -> Result<Value, serde_json::Error> {
    serde_json::to_value(
        SchemaSettings::draft2020_12()
            .into_generator()
            .into_root_schema_for::<T>(),
    )
}

#[test]
fn the_csrf_double_submit_names_are_pinned() {
    assert_eq!(CSRF_COOKIE, "okf-csrf");
    assert_eq!(CSRF_HEADER, "x-okf-csrf");
    assert_ne!(
        CSRF_COOKIE, "okf-session",
        "the CSRF cookie is not the session cookie"
    );
    assert!(
        CSRF_HEADER
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-'),
        "an HTTP header name, written in lowercase as HTTP/2 sends it"
    );
}

/// Backups and archives carry the local tenant, so its value never changes; `TenantId::local`
/// builds it without parsing, so it must equal the validated parse of the same value.
#[test]
fn the_local_tenant_is_pinned_and_is_a_valid_tenant_id() -> TestResult {
    assert_eq!(LOCAL_TENANT_ID, "local");
    let parsed = TenantId::try_from(LOCAL_TENANT_ID.to_owned())?;
    assert_eq!(TenantId::local(), parsed);
    assert_eq!(TenantId::local().as_str(), LOCAL_TENANT_ID);
    assert_eq!(
        serde_json::to_value(TenantId::local())?,
        json!(LOCAL_TENANT_ID)
    );
    Ok(())
}

#[test]
fn the_item_text_and_request_body_bounds_are_pinned() {
    assert_eq!(MAX_ITEM_TEXT_BYTES, 8_388_608, "8 MiB");
    assert_eq!(
        MAX_REQUEST_BODY_BYTES, 16_777_216,
        "16 MiB: twice the item text bound, so a request carrying one text at the bound fits"
    );
}

#[test]
fn every_item_text_passes_at_the_bound_and_is_too_large_one_byte_over() -> TestResult {
    assert_bounded::<CreateItemRequest>("create_item", &create_item(), "/body")?;
    assert_bounded::<SaveDraftRequest>("save_draft", &save_draft(), "/body")?;
    assert_bounded::<CorrectDigestRequest>(
        "correct_digest",
        &correct_digest(),
        "/corrected_markdown",
    )?;
    let create = json!([
        {"kind": "archive", "item_id": OTHER_ITEM},
        {"kind": "create", "path": "notes/a.md", "type_name": "note", "body": "", "properties": {}}
    ]);
    assert_bounded::<OpenProposalRequest>(
        "open_proposal create",
        &proposal(&create),
        "/changes/1/body",
    )?;
    let edit = json!([{"kind": "edit", "item_id": ITEM, "body": "", "properties": {}}]);
    assert_bounded::<OpenProposalRequest>(
        "open_proposal edit",
        &proposal(&edit),
        "/changes/0/body",
    )?;
    let supplied = json!([supply(OTHER_ITEM), supply(ITEM)]);
    assert_bounded::<OpenProposalRequest>(
        "open_proposal supply_extraction",
        &proposal(&supplied),
        "/changes/1/markdown",
    )?;
    Ok(())
}

#[test]
fn every_bounded_field_publishes_the_bound_as_its_maximum_length() -> TestResult {
    let bound = json!(MAX_ITEM_TEXT_BYTES);
    for (label, schema, pointer) in [
        (
            "create_item",
            schema_for::<CreateItemRequest>()?,
            "/properties/body/maxLength",
        ),
        (
            "save_draft",
            schema_for::<SaveDraftRequest>()?,
            "/properties/body/maxLength",
        ),
        (
            "correct_digest",
            schema_for::<CorrectDigestRequest>()?,
            "/properties/corrected_markdown/maxLength",
        ),
    ] {
        assert_eq!(schema.pointer(pointer), Some(&bound), "{label} {pointer}");
    }
    let proposal = schema_for::<OpenProposalRequest>()?;
    let variants = some(
        proposal
            .pointer("/$defs/Change/oneOf")
            .and_then(Value::as_array),
        "the Change variants",
    )?;
    let mut bounded = Vec::new();
    for variant in variants {
        let kind = some(
            variant
                .pointer("/properties/kind/const")
                .and_then(Value::as_str),
            "a Change variant's kind",
        )?;
        for field in ["body", "markdown"] {
            if let Some(property) = variant.pointer(&format!("/properties/{field}")) {
                assert_eq!(property.get("maxLength"), Some(&bound), "{kind}.{field}");
                bounded.push(format!("{kind}.{field}"));
            }
        }
    }
    assert_eq!(
        bounded,
        ["create.body", "edit.body", "supply_extraction.markdown"]
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
