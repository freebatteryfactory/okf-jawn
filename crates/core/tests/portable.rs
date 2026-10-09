//! The portable export and import rules: export writes a current whole-item app review as OKF
//! `verified`, an imported `verified` stays an unconfirmed claim, and OKF lint runs at import.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use okf_core::trust::{TrustTier, Verification};
use okf_core::yaml::{Mapping, Value as YamlValue};
use okf_jawn_contract::common::{PageRange, TextRange, Warning};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{
    Digest, ItemId, ReviewId, Revision, Timestamp, WorkspaceId, WorkspacePath,
};
use okf_jawn_contract::item::{APP_HEADER_KEY, ItemDocument, ItemKind, ItemStatus, ItemSummary};
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::review::{Review, ReviewCoverage};
use okf_jawn_contract::source::SourceReference;
use okf_jawn_core::portable::{
    ImportCheck, ImportedVerification, VERIFIED_KEY, coverage_at, exported_verified,
    imported_verification, item_content_digest, lint_imported,
};
use okf_jawn_core::storage::CandidateCheck;
use serde_json::{Value, json};
use uuid::Uuid;

use check::{TestResult, err_of, some};

/// The conformance half of an import commit's check: answers as the test sets it.
struct ScriptedConformance(Result<Vec<Warning>, ApiError>);

/// A candidate directory under the system temporary directory, removed when dropped.
struct Candidate(PathBuf);

impl CandidateCheck for ScriptedConformance {
    fn check(&self, _root: &Path) -> Result<Vec<Warning>, ApiError> {
        self.0.clone()
    }
}

impl Candidate {
    fn new() -> Result<Self, Box<dyn Error>> {
        let root = std::env::temp_dir().join(format!("okf-jawn-portable-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("notes"))?;
        Ok(Self(root))
    }

    fn write(&self, path: &str, text: &str) -> Result<(), Box<dyn Error>> {
        std::fs::write(self.0.join(path), text)?;
        Ok(())
    }
}

impl Drop for Candidate {
    fn drop(&mut self) {
        // Best effort: a leftover directory under the temporary directory harms nothing.
        let _left = std::fs::remove_dir_all(&self.0);
    }
}

fn digest(fill: char) -> Result<Digest, Box<dyn Error>> {
    Ok(Digest::try_from(fill.to_string().repeat(64))?)
}

fn instant(spelling: &str) -> Result<Timestamp, Box<dyn Error>> {
    Ok(Timestamp::try_from(spelling.to_owned())?)
}

fn document(body: &str, properties: Value) -> Result<ItemDocument, Box<dyn Error>> {
    Ok(ItemDocument {
        summary: ItemSummary {
            id: ItemId(Uuid::from_u128(10)),
            path: WorkspacePath::try_from("notes/plan.md".to_owned())?,
            title: "Plan".to_owned(),
            description: String::new(),
            type_name: "Note".to_owned(),
            kind: ItemKind::Note,
            revision: Revision::try_from("a".repeat(40))?,
            status: ItemStatus::Stable,
            archived: false,
            media_type: None,
            extraction: None,
        },
        body: body.to_owned(),
        properties: serde_json::from_value(properties)?,
        source: None,
        draft: None,
    })
}

fn review(
    subject: &str,
    at: &str,
    content_digest: Digest,
    selection: Selection,
    coverage: ReviewCoverage,
) -> Result<Review, Box<dyn Error>> {
    Ok(Review {
        id: ReviewId(Uuid::new_v4()),
        source: SourceReference {
            workspace_id: WorkspaceId(Uuid::from_u128(1)),
            item_id: ItemId(Uuid::from_u128(10)),
            path: WorkspacePath::try_from("notes/plan.md".to_owned())?,
            revision: Revision::try_from("a".repeat(40))?,
            digest: None,
            selection,
            locations: Vec::new(),
        },
        content_digest,
        reviewer_subject: subject.to_owned(),
        reviewed_at: instant(at)?,
        coverage,
    })
}

/// The okf-core value of a JSON value, so the test reads the result with okf-core itself.
fn yaml(value: &Value) -> YamlValue {
    match value {
        Value::Null => YamlValue::Null,
        Value::Bool(flag) => YamlValue::Bool(*flag),
        Value::Number(number) => YamlValue::String(number.to_string()),
        Value::String(text) => YamlValue::String(text.clone()),
        Value::Array(items) => YamlValue::Sequence(items.iter().map(yaml).collect()),
        Value::Object(map) => {
            let mut mapping = Mapping::new();
            for (key, child) in map {
                mapping.insert(key.clone(), yaml(child));
            }
            YamlValue::Mapping(mapping)
        }
    }
}

fn properties(value: Value) -> Result<BTreeMap<String, Value>, Box<dyn Error>> {
    Ok(serde_json::from_value(value)?)
}

#[test]
fn export_writes_only_current_whole_item_reviews_as_okf_verified() -> TestResult {
    let item = document("# Plan\n\nShip it.\n", json!({ "type": "Note" }))?;
    let content = item_content_digest(&item)?;
    let lines = Selection::Lines {
        range: TextRange { start: 1, end: 2 },
    };
    let reviews = vec![
        review(
            "user_1",
            "2026-10-08T09:00:00.000Z",
            content.clone(),
            Selection::All,
            ReviewCoverage::Current,
        )?,
        // The item changed after this review.
        review(
            "user_2",
            "2026-10-07T09:00:00.000Z",
            digest('e')?,
            Selection::All,
            ReviewCoverage::Current,
        )?,
        // A review of two lines, of one section or of some pages is not a review of the
        // concept, however current: OKF `verified` speaks for the whole concept.
        review(
            "user_3",
            "2026-10-08T10:00:00.000Z",
            content.clone(),
            lines,
            ReviewCoverage::Current,
        )?,
        review(
            "user_5",
            "2026-10-08T10:30:00.000Z",
            content.clone(),
            Selection::Section {
                heading: "Plan".to_owned(),
            },
            ReviewCoverage::Current,
        )?,
        review(
            "user_6",
            "2026-10-08T10:45:00.000Z",
            content.clone(),
            Selection::Pages {
                range: PageRange { start: 1, end: 1 },
            },
            ReviewCoverage::Current,
        )?,
        // Purged content: the review covers nothing that exists.
        review(
            "user_4",
            "2026-10-08T11:00:00.000Z",
            content.clone(),
            Selection::All,
            ReviewCoverage::Invalidated,
        )?,
    ];

    let written = some(
        exported_verified(None, &reviews, &content),
        "the exported verified value",
    )?;
    assert_eq!(
        written,
        json!([{ "by": "human:user_1", "at": "2026-10-08T09:00:00.000Z" }])
    );
    // okf-core reads it as one valid human verification.
    let events = Verification::list_from_value(&yaml(&written));
    assert_eq!(events.len(), 1);
    assert!(events.iter().all(Verification::is_valid));
    assert_eq!(TrustTier::derive(&events), TrustTier::HumanReviewed);

    // No current review: nothing is written, not even an empty key.
    let stale = reviews.get(1..).ok_or("the stale reviews")?;
    assert_eq!(exported_verified(None, stale, &content), None);
    Ok(())
}

#[test]
fn export_keeps_the_files_own_verified_and_writes_each_review_once() -> TestResult {
    let item = document("# Plan\n", json!({ "type": "Note" }))?;
    let content = item_content_digest(&item)?;
    let current = vec![review(
        "user_1",
        "2026-10-08T09:00:00.000Z",
        content.clone(),
        Selection::All,
        ReviewCoverage::Current,
    )?];
    let entry = json!({ "by": "human:user_1", "at": "2026-10-08T09:00:00.000Z" });
    // An imported claim as a bare mapping, which OKF reads as a one-element list.
    let claim = json!({ "by": "human:walter", "at": "2026-06-25T09:00:00Z" });

    let merged = some(
        exported_verified(Some(&claim), &current, &content),
        "the merged value",
    )?;
    assert_eq!(merged, json!([claim.clone(), entry.clone()]));
    // Exporting the exported file again adds nothing.
    assert_eq!(
        exported_verified(Some(&merged), &current, &content),
        Some(merged.clone())
    );
    // With no current review the file's own value is kept exactly, still a bare mapping.
    assert_eq!(
        exported_verified(Some(&claim), &[], &content),
        Some(claim.clone())
    );
    // A value of no OKF shape is kept, first, where OKF readers ignore it as before.
    assert_eq!(
        exported_verified(Some(&json!("yes")), &current, &content),
        Some(json!(["yes", entry]))
    );
    Ok(())
}

#[test]
fn coverage_follows_the_content_and_never_promotes_purged_or_imported() -> TestResult {
    let content = digest('d')?;
    let at = "2026-10-08T09:00:00.000Z";
    let recorded = |coverage| review("user_1", at, content.clone(), Selection::All, coverage);
    assert_eq!(
        coverage_at(&recorded(ReviewCoverage::Current)?, &content),
        ReviewCoverage::Current
    );
    assert_eq!(
        coverage_at(&recorded(ReviewCoverage::Current)?, &digest('e')?),
        ReviewCoverage::Changed
    );
    // The same content again is covered again; coverage is never extended to other content.
    assert_eq!(
        coverage_at(&recorded(ReviewCoverage::Changed)?, &content),
        ReviewCoverage::Current
    );
    for kept in [
        ReviewCoverage::Invalidated,
        ReviewCoverage::Imported,
        ReviewCoverage::Unreviewed,
    ] {
        assert_eq!(coverage_at(&recorded(kept.clone())?, &content), kept);
    }
    Ok(())
}

#[test]
fn the_reviewed_content_digest_ignores_the_header_and_key_order() -> TestResult {
    let header = json!({ "item_id": Uuid::from_u128(10) });
    let base = document(
        "# Plan\n",
        json!({ "type": "Note", "extra": { "b": 1, "a": 2 }, APP_HEADER_KEY: header }),
    )?;
    let digest_of_base = item_content_digest(&base)?;
    let archived = document(
        "# Plan\n",
        json!({
            "type": "Note",
            "extra": { "a": 2, "b": 1 },
            APP_HEADER_KEY: { "item_id": Uuid::from_u128(10), "archived": true },
        }),
    )?;
    assert_eq!(item_content_digest(&archived)?, digest_of_base);
    let edited = document(
        "# Plan\n\nMore.\n",
        json!({ "type": "Note", "extra": { "b": 1, "a": 2 } }),
    )?;
    assert_ne!(item_content_digest(&edited)?, digest_of_base);
    let retyped = document(
        "# Plan\n",
        json!({ "type": "Decision", "extra": { "b": 1, "a": 2 } }),
    )?;
    assert_ne!(item_content_digest(&retyped)?, digest_of_base);
    Ok(())
}

#[test]
fn an_imported_verified_is_an_unconfirmed_claim() -> TestResult {
    let listed = properties(json!({
        "type": "Note",
        VERIFIED_KEY: [
            { "by": "human:walter", "at": "2026-06-25T09:00:00Z" },
            { "by": "process:nightly", "at": "2026-06-26T02:00:00Z" },
        ],
    }))?;
    let claims = imported_verification(&listed);
    assert_eq!(
        claims,
        vec![
            ImportedVerification {
                by: Some("human:walter".to_owned()),
                at: Some("2026-06-25T09:00:00Z".to_owned()),
            },
            ImportedVerification {
                by: Some("process:nightly".to_owned()),
                at: Some("2026-06-26T02:00:00Z".to_owned()),
            },
        ]
    );
    // A `human:` actor in a file is not review evidence: every claim is imported coverage.
    assert!(
        claims
            .iter()
            .all(|claim| claim.coverage() == ReviewCoverage::Imported)
    );

    // OKF's rule: a bare mapping is a one-element list; any other shape claims nothing.
    let bare = properties(json!({ VERIFIED_KEY: { "by": "human:walter" } }))?;
    assert_eq!(
        imported_verification(&bare),
        vec![ImportedVerification {
            by: Some("human:walter".to_owned()),
            at: None,
        }]
    );
    let scalar = properties(json!({ VERIFIED_KEY: "yes" }))?;
    let nothing: Vec<ImportedVerification> = Vec::new();
    assert_eq!(imported_verification(&scalar), nothing);
    assert_eq!(
        imported_verification(&properties(json!({ "type": "Note" }))?),
        nothing
    );
    Ok(())
}

#[test]
fn lint_at_import_reports_the_imported_files_findings_as_warnings() -> TestResult {
    let candidate = Candidate::new()?;
    // Neither body opens with a top-level heading (lint rule L1).
    candidate.write("notes/imported.md", "---\ntype: Note\n---\n\nNo heading.\n")?;
    candidate.write("notes/other.md", "---\ntype: Note\n---\n\nNo heading.\n")?;
    let imported = vec![WorkspacePath::try_from("notes/imported.md".to_owned())?];
    let conformant = Warning {
        code: "conformance".to_owned(),
        message: "kept".to_owned(),
        location: None,
    };
    let check = ImportCheck::new(
        Arc::new(ScriptedConformance(Ok(vec![Warning {
            code: conformant.code.clone(),
            message: conformant.message.clone(),
            location: None,
        }]))),
        imported.clone(),
    );

    let warnings = check.check(&candidate.0)?;
    let first = some(warnings.first(), "the conformance warning")?;
    assert_eq!(
        (first.code.as_str(), first.message.as_str()),
        (conformant.code.as_str(), conformant.message.as_str())
    );
    let lint: Vec<&Warning> = warnings.iter().skip(1).collect();
    assert!(
        lint.iter().any(|warning| warning.code == "okf_lint_l1"
            && warning.location.as_deref() == Some("notes/imported.md")),
        "{lint:?}"
    );
    // Only the imported file's findings, located by workspace path, never by machine path.
    assert!(
        lint.iter()
            .all(|warning| warning.location.as_deref() == Some("notes/imported.md")),
        "{lint:?}"
    );
    assert!(
        lint.iter()
            .all(|warning| warning.code.starts_with("okf_lint"))
    );
    let root = candidate.0.display().to_string();
    assert!(
        lint.iter().all(|warning| !warning.message.contains(&root)),
        "{lint:?}"
    );
    assert_eq!(lint_imported(&candidate.0, &imported)?.len(), lint.len());
    Ok(())
}

#[test]
fn lint_at_import_never_overrides_a_conformance_refusal() -> TestResult {
    let candidate = Candidate::new()?;
    candidate.write("notes/imported.md", "---\ntype: Note\n---\n\nNo heading.\n")?;
    let refusal = ApiError::new(ErrorCode::InvalidInput, "not a conformant OKF bundle");
    let check = ImportCheck::new(
        Arc::new(ScriptedConformance(Err(refusal.clone()))),
        vec![WorkspacePath::try_from("notes/imported.md".to_owned())?],
    );
    let refused = err_of(check.check(&candidate.0))?;
    assert_eq!(refused.code, refusal.code);
    assert_eq!(refused.message, refusal.message);

    // A candidate that cannot be read is a fault, reported without the machine path.
    let missing = candidate.0.join("absent");
    let fault = err_of(lint_imported(&missing, &[]))?;
    assert_eq!(fault.code, ErrorCode::Internal);
    assert!(!fault.message.contains(&missing.display().to_string()));
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
