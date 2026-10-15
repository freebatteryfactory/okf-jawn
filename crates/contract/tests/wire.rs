//! Wire names and bounds the bindings share with every client, pinned to their published values.
//!
//! A change to one of these is a wire change: the server, workspace-ui and agents read them, so
//! a new value needs a deliberate contract change, not an edit that a test silently follows.
//! Every write field carrying an item's text is checked at `MAX_ITEM_TEXT_BYTES` and one byte
//! over it, with a text whose character count is still within the bound, so the check is seen
//! to count bytes.

use check::{TestResult, err_of, some};
use okf_jawn_contract::access::{CSRF_COOKIE, CSRF_HEADER, SESSION_COOKIE, SESSION_COOKIE_HTTPS};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{
    LOCAL_TENANT_ID, MAX_WORKSPACE_PATH_BYTES, TenantId, WorkspacePath,
};
use okf_jawn_contract::import::{
    CorrectDigestRequest, CreateUploadRequest, DEFAULT_MAX_UPLOAD_BYTES, MAX_IMPORT_UPLOADS,
    MAX_UPLOAD_FILENAME_BYTES, MAX_UPLOAD_RELATIVE_PATH_BYTES, StartImportRequest,
};
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
    assert_eq!(SESSION_COOKIE, "okf-session", "loopback http");
    assert_eq!(
        SESSION_COOKIE_HTTPS, "__Host-okf-session",
        "https: the __Host- prefix needs Secure, Path=/ and no Domain"
    );
    assert_eq!(
        SESSION_COOKIE_HTTPS.strip_prefix("__Host-"),
        Some(SESSION_COOKIE),
        "one session cookie under two names"
    );
    for session in [SESSION_COOKIE, SESSION_COOKIE_HTTPS] {
        assert_ne!(
            CSRF_COOKIE, session,
            "the CSRF cookie is not the session cookie"
        );
    }
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

/// Ruling 34: a bound counted in UTF-8 bytes is stated as `x-max-utf8-bytes` with its sentence,
/// never as `maxLength`, which JSON Schema (and the generated client) counts in characters.
fn assert_byte_bound(property: Option<&Value>, bytes: usize, label: &str) -> TestResult {
    let property = some(property, label)?;
    assert_eq!(property.get("maxLength"), None, "{label} carries maxLength");
    assert_eq!(
        property.get("x-max-utf8-bytes"),
        Some(&json!(bytes)),
        "{label}"
    );
    let description = some(
        property.get("description").and_then(Value::as_str),
        "the bounded field's description",
    )?;
    let sentence = format!("At most {bytes} bytes in UTF-8: a character takes one to four.");
    assert!(description.contains(&sentence), "{label}: {description}");
    Ok(())
}

/// Every node of `schema` that states a byte bound states no `maxLength` beside it.
fn no_byte_bound_has_a_max_length(schema: &Value, at: &str) -> TestResult {
    match schema {
        Value::Object(fields) => {
            assert!(
                !(fields.contains_key("x-max-utf8-bytes") && fields.contains_key("maxLength")),
                "{at} states its byte bound as maxLength too"
            );
            for (key, value) in fields {
                no_byte_bound_has_a_max_length(value, &format!("{at}/{key}"))?;
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                no_byte_bound_has_a_max_length(value, &format!("{at}/{index}"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[test]
fn every_byte_bounded_field_states_its_bound_in_bytes_never_as_max_length() -> TestResult {
    for (label, schema, pointer) in [
        (
            "create_item",
            schema_for::<CreateItemRequest>()?,
            "/properties/body",
        ),
        (
            "save_draft",
            schema_for::<SaveDraftRequest>()?,
            "/properties/body",
        ),
        (
            "correct_digest",
            schema_for::<CorrectDigestRequest>()?,
            "/properties/corrected_markdown",
        ),
    ] {
        assert_byte_bound(
            schema.pointer(pointer),
            MAX_ITEM_TEXT_BYTES,
            &format!("{label} {pointer}"),
        )?;
        no_byte_bound_has_a_max_length(&schema, label)?;
    }
    let path = schema_for::<WorkspacePath>()?;
    assert_byte_bound(Some(&path), MAX_WORKSPACE_PATH_BYTES, "WorkspacePath")?;
    let proposal = schema_for::<OpenProposalRequest>()?;
    no_byte_bound_has_a_max_length(&proposal, "open_proposal")?;
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
                assert_byte_bound(
                    Some(property),
                    MAX_ITEM_TEXT_BYTES,
                    &format!("{kind}.{field}"),
                )?;
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

fn create_upload(filename: &str, relative_path: &str) -> Value {
    json!({
        "workspace_id": WORKSPACE, "filename": filename, "relative_path": relative_path,
        "size": "4", "idempotency_key": KEY
    })
}

fn start_import(uploads: &[String]) -> Value {
    json!({
        "workspace_id": WORKSPACE, "base_revision": revision(), "upload_ids": uploads,
        "destination": "", "idempotency_key": KEY, "apply_naming_rules": false
    })
}

/// The `index`th distinct upload identity.
fn upload_id(index: usize) -> String {
    format!("00000000-0000-4000-8000-{index:012}")
}

/// A name of `bytes` ASCII bytes in segments of at most 200, so every segment is a storable name.
fn segmented(bytes: usize) -> String {
    (0..bytes)
        .map(|position| if position % 201 == 200 { '/' } else { 'a' })
        .collect()
}

/// `check_rules` of a decoded `T`.
fn rules_of<T: DeserializeOwned + RequestScope>(
    value: Value,
) -> Result<Result<(), ApiError>, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value::<T>(value)?.check_rules())
}

#[test]
fn the_upload_and_import_bounds_are_pinned() {
    assert_eq!(DEFAULT_MAX_UPLOAD_BYTES, 536_870_912, "512 MiB");
    assert_eq!(MAX_UPLOAD_FILENAME_BYTES, 1024);
    assert_eq!(
        MAX_UPLOAD_RELATIVE_PATH_BYTES, 4096,
        "the bound of a workspace path"
    );
    assert_eq!(MAX_UPLOAD_RELATIVE_PATH_BYTES, MAX_WORKSPACE_PATH_BYTES);
    assert_eq!(MAX_IMPORT_UPLOADS, 5000);
}

/// A name of exactly its bound passes; one byte more is `too_large` on its field, although it
/// has no more characters than the bound.
#[test]
fn an_upload_name_passes_at_its_bound_and_is_too_large_one_byte_over() -> TestResult {
    // The name at the bound is in folders of 200 bytes, so its source card has a path: a single
    // segment that long is refused on another rule (`WorkspacePath`'s 255 UTF-16 units).
    for (pointer, bound) in [
        ("/filename", MAX_UPLOAD_FILENAME_BYTES),
        ("/relative_path", MAX_UPLOAD_RELATIVE_PATH_BYTES),
    ] {
        let within = segmented(bound);
        assert_eq!(within.len(), bound);
        let accepted =
            checked::<CreateUploadRequest>(&create_upload("a.pdf", ""), pointer, &within)?;
        assert!(accepted.is_ok(), "{pointer} at the bound: {accepted:?}");
        let mut over = segmented(bound.saturating_sub(1));
        over.push('é');
        assert_eq!(over.len(), bound.saturating_add(1));
        assert_eq!(over.chars().count(), bound);
        let refused = err_of(checked::<CreateUploadRequest>(
            &create_upload("a.pdf", ""),
            pointer,
            &over,
        )?)?;
        assert_eq!(refused.code, ErrorCode::TooLarge, "{pointer}");
        assert_eq!(refused.field.as_deref(), Some(pointer));
    }
    Ok(())
}

#[test]
fn an_upload_name_that_gives_its_card_no_path_is_refused_on_filename() -> TestResult {
    // The card of `CON.pdf` is `CON-pdf.md`, a fine path; the card of `CON` is `CON.md`, whose
    // stem is a Windows device name. `a:b.pdf` holds a character Windows refuses.
    for name in ["report.pdf", "CON.pdf", "Q3 Results.XLSX", "README"] {
        let accepted = rules_of::<CreateUploadRequest>(create_upload(name, "2026/q3"))?;
        assert!(accepted.is_ok(), "{name}: {accepted:?}");
    }
    for name in ["CON", "a:b.pdf", "notes?.txt", "tab\tname.pdf"] {
        let refused = err_of(rules_of::<CreateUploadRequest>(create_upload(name, ""))?)?;
        assert_eq!(refused.code, ErrorCode::InvalidInput, "{name}");
        assert_eq!(refused.field.as_deref(), Some("/filename"), "{name}");
    }
    Ok(())
}

#[test]
fn an_import_takes_one_to_the_bound_of_uploads_each_once() -> TestResult {
    let at_bound: Vec<String> = (0..MAX_IMPORT_UPLOADS).map(upload_id).collect();
    let accepted = rules_of::<StartImportRequest>(start_import(&at_bound))?;
    assert!(accepted.is_ok(), "{accepted:?}");
    let one = rules_of::<StartImportRequest>(start_import(&[upload_id(7)]))?;
    assert!(one.is_ok(), "{one:?}");
    let over: Vec<String> = (0..=MAX_IMPORT_UPLOADS).map(upload_id).collect();
    let refused = err_of(rules_of::<StartImportRequest>(start_import(&over))?)?;
    assert_eq!(refused.code, ErrorCode::TooLarge);
    assert_eq!(refused.field.as_deref(), Some("/upload_ids"));
    let none = err_of(rules_of::<StartImportRequest>(start_import(&[]))?)?;
    assert_eq!(none.code, ErrorCode::InvalidInput);
    assert_eq!(none.field.as_deref(), Some("/upload_ids"));
    let twice = [upload_id(1), upload_id(2), upload_id(3), upload_id(2)];
    let repeated = err_of(rules_of::<StartImportRequest>(start_import(&twice))?)?;
    assert_eq!(repeated.code, ErrorCode::InvalidInput);
    assert_eq!(
        repeated.field.as_deref(),
        Some("/upload_ids/3"),
        "the second place"
    );
    Ok(())
}

#[test]
fn the_upload_and_import_bounds_are_published_in_the_schemas() -> TestResult {
    let upload = schema_for::<CreateUploadRequest>()?;
    assert_byte_bound(
        upload.pointer("/properties/filename"),
        MAX_UPLOAD_FILENAME_BYTES,
        "create_upload /filename",
    )?;
    assert_byte_bound(
        upload.pointer("/properties/relative_path"),
        MAX_UPLOAD_RELATIVE_PATH_BYTES,
        "create_upload /relative_path",
    )?;
    no_byte_bound_has_a_max_length(&upload, "create_upload")?;
    let import = schema_for::<StartImportRequest>()?;
    assert_eq!(
        import.pointer("/properties/upload_ids/minItems"),
        Some(&json!(1))
    );
    assert_eq!(
        import.pointer("/properties/upload_ids/maxItems"),
        Some(&json!(MAX_IMPORT_UPLOADS))
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
