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
use okf_jawn_contract::review::{ImportedClaim, Review, ReviewCoverage};
use okf_jawn_contract::source::SourceReference;
use okf_jawn_core::conformance::OkfConformance;
use okf_jawn_core::portable::{
    ImportCheck, VERIFIED_KEY, claim_coverage, coverage_at, draft_digest, exported_verified,
    imported_verification, item_content_digest, lint_imported,
};
use okf_jawn_core::storage::CandidateCheck;
use serde_json::{Value, json};
use uuid::Uuid;

use check::{TestResult, err_of, some};

/// The conformance half of an import commit's check: answers as the test sets it.
struct ScriptedConformance(Result<Vec<Warning>, ApiError>);

/// A conformance half that answers one warning for each rewritten path it is handed, located at
/// that path, so a test sees what an `ImportCheck` passed on.
struct EchoingRewritten;

/// A candidate directory under the system temporary directory, removed when dropped.
struct Candidate(PathBuf);

impl CandidateCheck for ScriptedConformance {
    fn check(
        &self,
        _root: &Path,
        _rewritten: &[okf_jawn_contract::identity::WorkspacePath],
    ) -> Result<Vec<Warning>, ApiError> {
        self.0.clone()
    }
}

impl CandidateCheck for EchoingRewritten {
    fn check(&self, _root: &Path, rewritten: &[WorkspacePath]) -> Result<Vec<Warning>, ApiError> {
        Ok(rewritten
            .iter()
            .map(|path| Warning {
                code: "handed".to_owned(),
                message: "the store rewrote this file".to_owned(),
                location: Some(path.as_str().to_owned()),
            })
            .collect())
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
    let mut document = ItemDocument {
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
        content_digest: digest('0')?,
        source: None,
        draft: None,
    };
    // The digest is derived from the content, never typed.
    document.content_digest = item_content_digest(&document)?;
    Ok(document)
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

/// A committed note for M2a's export writer (design VW1-VW3), with an extension and body to keep.
const STORED_EXPORT_NOTE: &str = concat!(
    "---\n",
    "type: Note\n",
    "title: Plan\n",
    "generated:\n",
    "  by: agent:writer\n",
    "  at: '2026-10-01T08:00:00Z'\n",
    "stale_after: '2027-01-01'\n",
    "x_team: 7\n",
    "okf_jawn:\n",
    "  item_id: 00000000-0000-0000-0000-00000000000a\n",
    "  kind: note\n",
    "---\n",
    "\n",
    "# Plan\n",
    "\n",
    "Ship it.\n",
);

/// The actual content fingerprint of the fixed note, using the existing item test helper.
fn export_note_fingerprint() -> Result<Digest, Box<dyn Error>> {
    let stored = okf_core::Document::parse(STORED_EXPORT_NOTE)?;
    Ok(document(
        &stored.body,
        json!({
            "type": "Note",
            "title": "Plan",
            "generated": { "by": "agent:writer", "at": "2026-10-01T08:00:00Z" },
            "stale_after": "2027-01-01",
            "x_team": 7
        }),
    )?
    .content_digest)
}

#[test]
fn export_item_file_adds_only_the_current_whole_review() -> TestResult {
    // The caller supplies the item's fingerprint. The review names that same content;
    // the expected verification below is the literal VW1 answer, not the value writer's output.
    let content = export_note_fingerprint()?;
    let reviews = [review(
        "alice",
        "2026-10-08T09:00:00.000Z",
        content.clone(),
        Selection::All,
        ReviewCoverage::Current,
    )?];
    let exported = some(
        okf_jawn_core::portable::export_item_file(
            STORED_EXPORT_NOTE.as_bytes(),
            &reviews,
            &content,
        )?,
        "the file with its current review written",
    )?;
    let after = okf_core::Document::parse(std::str::from_utf8(&exported)?)?;
    let before = okf_core::Document::parse(STORED_EXPORT_NOTE)?;
    assert_eq!(
        after.frontmatter.get(VERIFIED_KEY),
        Some(&yaml(&json!([
            { "by": "human:alice", "at": "2026-10-08T09:00:00.000Z" }
        ])))
    );
    assert_eq!(
        after.frontmatter.keys().collect::<Vec<_>>(),
        vec![
            "type",
            "title",
            "generated",
            "verified",
            "stale_after",
            "x_team",
            APP_HEADER_KEY,
        ]
    );
    assert_eq!(after.body, before.body);
    for key in before.frontmatter.keys() {
        assert_eq!(after.frontmatter.get(key), before.frontmatter.get(key), "{key}");
    }
    Ok(())
}

#[test]
fn export_item_file_without_whole_current_review_keeps_stored_bytes() -> TestResult {
    let content = export_note_fingerprint()?;
    let line_review = review(
        "alice",
        "2026-10-08T09:00:00.000Z",
        content.clone(),
        Selection::Lines {
            range: TextRange { start: 1, end: 1 },
        },
        ReviewCoverage::Current,
    )?;
    for reviews in [Vec::new(), vec![line_review]] {
        // VW2-VW3: None instructs the caller to export the exact original bytes; an empty
        // verification or a reserialized copy would change a file that has no whole-item review.
        assert_eq!(
            okf_jawn_core::portable::export_item_file(
                STORED_EXPORT_NOTE.as_bytes(),
                &reviews,
                &content,
            )?,
            None
        );
    }
    Ok(())
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
fn export_orders_the_reviews_it_writes_whatever_order_it_is_given() -> TestResult {
    let item = document("# Plan\n", json!({ "type": "Note" }))?;
    let content = item_content_digest(&item)?;
    let current = |id: u128, subject: &str, at: &str| -> Result<Review, Box<dyn Error>> {
        Ok(Review {
            id: ReviewId(Uuid::from_u128(id)),
            ..review(
                subject,
                at,
                content.clone(),
                Selection::All,
                ReviewCoverage::Current,
            )?
        })
    };
    // Newest first, with two reviews at the same instant whose identities order the other way
    // round from their subjects, so the subject is what breaks the tie.
    let reviews = vec![
        current(1, "user_2", "2026-10-08T10:00:00.000Z")?,
        current(3, "user_9", "2026-10-08T11:00:00.000Z")?,
        current(4, "user_1", "2026-10-08T09:00:00.000Z")?,
        current(2, "user_0", "2026-10-08T10:00:00.000Z")?,
    ];
    let expected = json!([
        { "by": "human:user_1", "at": "2026-10-08T09:00:00.000Z" },
        { "by": "human:user_0", "at": "2026-10-08T10:00:00.000Z" },
        { "by": "human:user_2", "at": "2026-10-08T10:00:00.000Z" },
        { "by": "human:user_9", "at": "2026-10-08T11:00:00.000Z" },
    ]);
    assert_eq!(
        exported_verified(None, &reviews, &content),
        Some(expected.clone())
    );
    let reversed: Vec<Review> = reviews.into_iter().rev().collect();
    assert_eq!(exported_verified(None, &reversed, &content), Some(expected));
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

/// A rename records the old path in `aliases`; the review fingerprint leaves that property out,
/// so a pure rename keeps a review current, while any other property still counts.
#[test]
fn the_reviewed_content_digest_leaves_out_aliases_and_nothing_else() -> TestResult {
    let base = document("# Plan\n", json!({ "type": "Note", "status": "stable" }))?;
    let digest_of_base = item_content_digest(&base)?;
    let renamed = document(
        "# Plan\n",
        json!({ "type": "Note", "status": "stable", "aliases": ["notes/old-plan.md"] }),
    )?;
    assert_eq!(item_content_digest(&renamed)?, digest_of_base);
    let renamed_twice = document(
        "# Plan\n",
        json!({
            "type": "Note",
            "status": "stable",
            "aliases": ["notes/old-plan.md", "plans/plan.md"],
        }),
    )?;
    assert_eq!(item_content_digest(&renamed_twice)?, digest_of_base);
    let tagged = document(
        "# Plan\n",
        json!({ "type": "Note", "status": "stable", "tags": ["notes/old-plan.md"] }),
    )?;
    assert_ne!(item_content_digest(&tagged)?, digest_of_base);
    let deprecated = document(
        "# Plan\n",
        json!({ "type": "Note", "status": "deprecated", "aliases": ["notes/old-plan.md"] }),
    )?;
    assert_ne!(item_content_digest(&deprecated)?, digest_of_base);
    Ok(())
}

/// Review N1: a draft is compared by its whole content. `draft_digest` keeps `aliases`, which
/// the review fingerprint leaves out, and like it ignores the application header and key order;
/// it is the digest of a body and a property map whether they come from a draft or an item.
#[test]
fn the_draft_digest_covers_aliases_and_ignores_the_header() -> TestResult {
    let base = document("# Plan\n", json!({ "type": "Note", "status": "stable" }))?;
    let digest_of_base = draft_digest(&base.body, &base.properties)?;
    let renamed = document(
        "# Plan\n",
        json!({ "type": "Note", "status": "stable", "aliases": ["notes/old-plan.md"] }),
    )?;
    assert_ne!(
        draft_digest(&renamed.body, &renamed.properties)?,
        digest_of_base,
        "an alias alone is a change of the draft"
    );
    assert_eq!(
        item_content_digest(&renamed)?,
        item_content_digest(&base)?,
        "and not of the review fingerprint"
    );
    let headed = document(
        "# Plan\n",
        json!({
            "status": "stable",
            "type": "Note",
            APP_HEADER_KEY: { "item_id": Uuid::from_u128(10), "archived": true },
        }),
    )?;
    assert_eq!(
        draft_digest(&headed.body, &headed.properties)?,
        digest_of_base
    );
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
            ImportedClaim {
                by: Some("human:walter".to_owned()),
                at: Some("2026-06-25T09:00:00Z".to_owned()),
            },
            ImportedClaim {
                by: Some("process:nightly".to_owned()),
                at: Some("2026-06-26T02:00:00Z".to_owned()),
            },
        ]
    );
    // A `human:` actor in a file is not review evidence: every claim is imported coverage.
    assert!(
        claims
            .iter()
            .all(|claim| claim_coverage(claim) == ReviewCoverage::Imported)
    );

    // OKF's rule: a bare mapping is a one-element list; any other shape claims nothing.
    let bare = properties(json!({ VERIFIED_KEY: { "by": "human:walter" } }))?;
    assert_eq!(
        imported_verification(&bare),
        vec![ImportedClaim {
            by: Some("human:walter".to_owned()),
            at: None,
        }]
    );
    let scalar = properties(json!({ VERIFIED_KEY: "yes" }))?;
    let nothing: Vec<ImportedClaim> = Vec::new();
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
    // A conformance warning about the imported file is the import's to report.
    let conformant = Warning {
        code: "conformance".to_owned(),
        message: "kept".to_owned(),
        location: Some("notes/imported.md".to_owned()),
    };
    let check = ImportCheck::new(
        Arc::new(ScriptedConformance(Ok(vec![Warning {
            code: conformant.code.clone(),
            message: conformant.message.clone(),
            location: conformant.location.clone(),
        }]))),
        imported.clone(),
    );

    let warnings = check.check(&candidate.0, &[])?;
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
fn an_import_check_hands_the_rewritten_paths_to_its_conformance_check() -> TestResult {
    let candidate = Candidate::new()?;
    candidate.write(
        "notes/imported.md",
        "---
type: Note
---

No heading.
",
    )?;
    candidate.write(
        "notes/referrer.md",
        "---
type: Note
---

No heading.
",
    )?;
    let check = ImportCheck::new(
        Arc::new(EchoingRewritten),
        vec![WorkspacePath::try_from("notes/imported.md".to_owned())?],
    );
    let rewritten = [WorkspacePath::try_from("notes/referrer.md".to_owned())?];
    let warnings = check.check(&candidate.0, &rewritten)?;
    let located = |code: &str| -> Vec<&str> {
        warnings
            .iter()
            .filter(|warning| warning.code.starts_with(code))
            .filter_map(|warning| warning.location.as_deref())
            .collect()
    };
    // The conformance half was handed the path the store rewrote, as it was.
    assert_eq!(located("handed"), ["notes/referrer.md"]);
    // The import's own lint stays on the imported file (an import commit moves nothing).
    let lint = located("okf_lint");
    assert!(lint.contains(&"notes/imported.md"), "{warnings:?}");
    assert!(!lint.contains(&"notes/referrer.md"), "{warnings:?}");
    // With nothing rewritten, nothing is handed on.
    let none = check.check(&candidate.0, &[])?;
    assert!(
        none.iter().all(|warning| warning.code != "handed"),
        "{none:?}"
    );
    Ok(())
}

/// Review N2: an import commit's check reports what that commit wrote. The conformance check
/// answers for the whole candidate (as `OkfConformance` does: every file's findings, and
/// findings that name no file); of those, only the ones located at a file this commit writes,
/// or at a file the store rewrote, are passed on, each once and in the order given.
#[test]
fn an_import_check_passes_on_the_conformance_warnings_of_the_files_its_commit_wrote_only()
-> TestResult {
    let candidate = Candidate::new()?;
    candidate.write("notes/imported.md", "---\ntype: Note\n---\n\n# Imported\n")?;
    let about = |code: &str, location: Option<&str>| Warning {
        code: code.to_owned(),
        message: "a validator finding".to_owned(),
        location: location.map(str::to_owned),
    };
    let whole_candidate = vec![
        about("at_another_card", Some("notes/other.md")),
        about("at_the_imported", Some("notes/imported.md")),
        about("names_no_file", None),
        about("at_a_rewritten", Some("notes/referrer.md")),
        about("at_a_bystander", Some("notes/bystander.md")),
        // A path that only starts like the imported one is another file.
        about("at_a_longer_path", Some("notes/imported.md.bak")),
    ];
    let check = ImportCheck::new(
        Arc::new(ScriptedConformance(Ok(whole_candidate))),
        vec![WorkspacePath::try_from("notes/imported.md".to_owned())?],
    );
    let passed_on = |warnings: &[Warning]| -> Vec<String> {
        warnings
            .iter()
            .filter(|warning| !warning.code.starts_with("okf_lint"))
            .map(|warning| warning.code.clone())
            .collect()
    };
    assert_eq!(
        passed_on(&check.check(&candidate.0, &[])?),
        ["at_the_imported"]
    );
    let rewritten = [WorkspacePath::try_from("notes/referrer.md".to_owned())?];
    assert_eq!(
        passed_on(&check.check(&candidate.0, &rewritten)?),
        ["at_the_imported", "at_a_rewritten"]
    );
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
    let refused = err_of(check.check(&candidate.0, &[]))?;
    assert_eq!(refused.code, refusal.code);
    assert_eq!(refused.message, refusal.message);

    // A candidate that cannot be read is a fault, reported without the machine path.
    let missing = candidate.0.join("absent");
    let fault = err_of(lint_imported(&missing, &[]))?;
    assert_eq!(fault.code, ErrorCode::Internal);
    assert!(!fault.message.contains(&missing.display().to_string()));
    Ok(())
}

/// An import commit checked by the production conformance check (`OkfConformance`), not a
/// scripted one. A card the commit writes holds a broken link: the import reports that link,
/// located at the card, once. A broken link in a file the commit does not write is not
/// reported, and no warning about that file is. A non-conformant file elsewhere still refuses
/// the commit.
#[test]
fn an_import_reports_a_broken_link_of_the_card_it_wrote_located_at_the_card_once() -> TestResult {
    let candidate = Candidate::new()?;
    let linking = |title: &str, target: &str| {
        format!("---\ntype: Note\ntitle: {title}\n---\n# {title}\n\nSee [it]({target}).\n")
    };
    candidate.write(
        "notes/imported.md",
        &linking("Imported", "missing-thing.md"),
    )?;
    candidate.write("notes/other.md", &linking("Other", "elsewhere.md"))?;
    let imported = WorkspacePath::try_from("notes/imported.md".to_owned())?;
    let check = ImportCheck::new(Arc::new(OkfConformance), vec![imported]);
    let warnings = check.check(&candidate.0, &[])?;
    let broken: Vec<(Option<&str>, &str)> = warnings
        .iter()
        .filter(|warning| warning.message.contains("does not resolve to a concept"))
        .map(|warning| (warning.location.as_deref(), warning.message.as_str()))
        .collect();
    assert_eq!(
        broken,
        [(
            Some("notes/imported.md"),
            "link target does not resolve to a concept in the bundle: missing-thing.md"
        )]
    );
    assert!(
        warnings
            .iter()
            .all(|warning| warning.location.as_deref() == Some("notes/imported.md")),
        "{warnings:?}"
    );
    // The production check does report the other file's link, located there: the import's
    // check is what leaves it out.
    let whole = OkfConformance.check(&candidate.0, &[])?;
    assert!(
        whole.iter().any(|warning| {
            warning.location.as_deref() == Some("notes/other.md")
                && warning.message.ends_with("elsewhere.md")
        }),
        "{whole:?}"
    );

    // A refusal is a refusal as before, whatever file it names.
    candidate.write("notes/bad.md", "---\ntitle: No type\n---\nBody\n")?;
    let refused = err_of(check.check(&candidate.0, &[]))?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert!(
        refused.message.contains("notes/bad.md"),
        "{}",
        refused.message
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
