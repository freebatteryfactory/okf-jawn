//! Every OKF `sources` entry reaches the wire whole, with what it resolved to; none is dropped.

use okf_jawn_contract::source::{
    DeclaredOutcome, DeclaredSource, GetSourcesResponse, UncitedReason,
};
use serde_json::json;

use check::{TestResult, err_of, some};

#[test]
fn a_response_without_declared_entries_reads_as_none_and_writes_none() -> TestResult {
    let response: GetSourcesResponse =
        serde_json::from_value(json!({ "revision": "a".repeat(40), "sources": [] }))?;
    assert_eq!(response.declared, Vec::new());
    let written = serde_json::to_value(&response)?;
    assert_eq!(written.get("declared"), None);
    Ok(())
}

#[test]
fn a_declared_entry_keeps_every_field_as_written_and_its_outcome() -> TestResult {
    let response: GetSourcesResponse = serde_json::from_value(json!({
        "revision": "a".repeat(40),
        "sources": [],
        "declared": [
            {
                "entry": {
                    "id": "s1",
                    "resource": "https://example.org/report",
                    "title": "Report",
                    "author": "A. Writer",
                    "last_modified": "2026-01-02",
                    "usage_count": 3,
                },
                "outcome": { "kind": "uncited", "reason": "external" },
            },
            {
                "entry": { "id": "s2", "resource": "notes/plan.md" },
                "outcome": { "kind": "cited", "index": 0 },
            },
            { "entry": { "id": "s3" }, "outcome": { "kind": "uncited", "reason": "malformed" } },
            { "entry": { "resource": "notes/" }, "outcome": { "kind": "uncited", "reason": "scope" } },
            {
                "entry": { "resource": "notes/missing.md" },
                "outcome": { "kind": "uncited", "reason": "not_found" },
            },
        ],
    }))?;
    let first = some(response.declared.first(), "the first declared entry")?;
    // The footnote key and every credibility field survive.
    assert_eq!(first.entry.get("id"), Some(&json!("s1")));
    assert_eq!(first.entry.get("usage_count"), Some(&json!(3)));
    assert_eq!(first.entry.len(), 6);
    let outcomes: Vec<&DeclaredOutcome> = response
        .declared
        .iter()
        .map(|declared| &declared.outcome)
        .collect();
    assert_eq!(
        outcomes,
        vec![
            &DeclaredOutcome::Uncited {
                reason: UncitedReason::External
            },
            &DeclaredOutcome::Cited { index: 0 },
            &DeclaredOutcome::Uncited {
                reason: UncitedReason::Malformed
            },
            &DeclaredOutcome::Uncited {
                reason: UncitedReason::Scope
            },
            &DeclaredOutcome::Uncited {
                reason: UncitedReason::NotFound
            },
        ]
    );
    Ok(())
}

#[test]
fn a_declared_entry_refuses_unknown_fields_and_outcomes() -> TestResult {
    err_of(serde_json::from_value::<DeclaredSource>(json!({
        "entry": {},
        "outcome": { "kind": "cited", "index": 0 },
        "guess": true,
    })))?;
    err_of(serde_json::from_value::<DeclaredSource>(json!({
        "entry": {},
        "outcome": { "kind": "uncited", "reason": "web" },
    })))?;
    err_of(serde_json::from_value::<DeclaredSource>(json!({
        "entry": {},
        "outcome": { "kind": "probably" },
    })))?;
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
