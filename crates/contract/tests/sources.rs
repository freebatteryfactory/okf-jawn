//! An OKF `sources` entry that is not a citation is kept on the wire, never dropped.

use okf_jawn_contract::source::{GetSourcesResponse, UncitedReason, UncitedSource};
use serde_json::json;

use check::{TestResult, err_of};

#[test]
fn a_response_without_uncited_entries_reads_as_none_and_writes_none() -> TestResult {
    let response: GetSourcesResponse =
        serde_json::from_value(json!({ "revision": "a".repeat(40), "sources": [] }))?;
    assert_eq!(response.uncited, Vec::new());
    let written = serde_json::to_value(&response)?;
    assert_eq!(written.get("uncited"), None);
    Ok(())
}

#[test]
fn uncited_entries_keep_their_resource_reason_and_order() -> TestResult {
    let response: GetSourcesResponse = serde_json::from_value(json!({
        "revision": "a".repeat(40),
        "sources": [],
        "uncited": [
            { "resource": "https://example.org/report", "reason": "external" },
            { "resource": "notes/", "reason": "scope" },
            { "resource": "notes/missing.md", "reason": "not_found" },
            { "resource": "", "reason": "malformed" },
        ],
    }))?;
    assert_eq!(
        response.uncited,
        vec![
            UncitedSource {
                resource: "https://example.org/report".to_owned(),
                reason: UncitedReason::External,
            },
            UncitedSource {
                resource: "notes/".to_owned(),
                reason: UncitedReason::Scope,
            },
            UncitedSource {
                resource: "notes/missing.md".to_owned(),
                reason: UncitedReason::NotFound,
            },
            UncitedSource {
                resource: String::new(),
                reason: UncitedReason::Malformed,
            },
        ]
    );
    Ok(())
}

#[test]
fn an_uncited_entry_refuses_unknown_fields_and_reasons() -> TestResult {
    err_of(serde_json::from_value::<UncitedSource>(
        json!({ "resource": "x", "reason": "external", "guess": true }),
    ))?;
    err_of(serde_json::from_value::<UncitedSource>(
        json!({ "resource": "x", "reason": "web" }),
    ))?;
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
