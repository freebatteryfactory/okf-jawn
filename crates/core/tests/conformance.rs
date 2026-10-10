//! The production `CandidateCheck` judges a staged candidate with okf-validator: a
//! conformance violation refuses the candidate with the validator's diagnostics, and softer
//! findings come back as located warnings.

use std::error::Error;
use std::path::PathBuf;

use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::identity::WorkspacePath;
use okf_jawn_core::conformance::{EditCheck, OkfConformance};
use okf_jawn_core::storage::CandidateCheck;
use uuid::Uuid;

use check::{TestResult, err_of};

/// A staged candidate directory under the system temporary directory, removed when dropped.
struct Staged(PathBuf);

impl Staged {
    fn new() -> Result<Self, Box<dyn Error>> {
        let root = std::env::temp_dir().join(format!("okf-jawn-conformance-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("notes"))?;
        Ok(Self(root))
    }

    fn write(&self, path: &str, text: &str) -> Result<(), Box<dyn Error>> {
        std::fs::write(self.0.join(path), text)?;
        Ok(())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        // Best effort: a leftover directory under the temporary directory harms nothing.
        let _left = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn candidate_check_accepts_a_conformant_bundle() -> TestResult {
    let staged = Staged::new()?;
    staged.write(
        "notes/plan.md",
        "---\ntype: Note\ntitle: Plan\ndescription: The plan.\n---\n# Plan\n",
    )?;
    let warnings = OkfConformance.check(&staged.0)?;
    assert!(
        warnings
            .iter()
            .all(|warning| warning.code.starts_with("okf_validator_")),
        "{warnings:?}"
    );
    Ok(())
}

#[test]
fn candidate_check_refuses_a_non_conformant_bundle_with_the_validators_diagnostics() -> TestResult {
    let staged = Staged::new()?;
    staged.write("notes/plan.md", "---\ntype: Note\n---\n# Plan\n")?;
    staged.write("notes/untyped.md", "---\ntitle: No type\n---\nBody\n")?;
    let refused = err_of(OkfConformance.check(&staged.0))?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert!(
        refused.message.contains("notes/untyped.md")
            && refused
                .message
                .contains("missing required frontmatter field `type`"),
        "{}",
        refused.message
    );
    assert!(
        !refused.message.contains("notes/plan.md"),
        "{}",
        refused.message
    );
    Ok(())
}

#[test]
fn candidate_check_returns_soft_findings_as_located_warnings() -> TestResult {
    let staged = Staged::new()?;
    // `status` outside OKF's words is a producer deviation the validator only warns about.
    staged.write(
        "notes/odd.md",
        "---\ntype: Note\ntitle: Odd\ndescription: Odd.\nstatus: shelved\n---\nBody\n",
    )?;
    let warnings = OkfConformance.check(&staged.0)?;
    let located: Vec<_> = warnings
        .iter()
        .filter(|warning| warning.location.as_deref() == Some("notes/odd.md"))
        .collect();
    assert!(!located.is_empty(), "{warnings:?}");
    assert!(
        located
            .iter()
            .all(|warning| warning.code == "okf_validator_warning"
                || warning.code == "okf_validator_info"),
        "{located:?}"
    );
    Ok(())
}

#[test]
fn candidate_check_refuses_a_candidate_it_cannot_read_as_internal() -> TestResult {
    let missing = std::env::temp_dir().join(format!("okf-jawn-absent-{}", Uuid::new_v4()));
    let refused = err_of(OkfConformance.check(&missing))?;
    assert_eq!(refused.code, ErrorCode::Internal);
    Ok(())
}

#[test]
fn candidate_check_of_an_edit_reports_lint_only_for_the_files_it_touched() -> TestResult {
    let staged = Staged::new()?;
    staged.write("notes/plan.md", "---\ntype: Note\n---\n\nNo heading.\n")?;
    staged.write("notes/other.md", "---\ntype: Note\n---\n\nNo heading.\n")?;
    staged.write(
        "notes/meeting.md",
        "---\ntype: Meeting\n---\n\nNo heading.\n",
    )?;
    let located = |warnings: &[Warning]| -> Vec<String> {
        let mut located: Vec<String> = warnings
            .iter()
            .filter(|warning| warning.code.starts_with("okf_lint"))
            .filter_map(|warning| warning.location.clone())
            .collect();
        located.sort();
        located.dedup();
        located
    };
    let plan = EditCheck::new(
        &[WorkspacePath::try_from("notes/plan.md".to_owned())?],
        None,
    );
    let edit = plan.check(&staged.0)?;
    assert!(
        edit.iter().any(|warning| warning.code == "okf_lint_l1"),
        "{edit:?}"
    );
    assert_eq!(located(&edit), vec!["notes/plan.md".to_owned()]);
    let type_change = EditCheck::new(&[], Some("Meeting".to_owned()));
    assert_eq!(
        located(&type_change.check(&staged.0)?),
        vec!["notes/meeting.md".to_owned()]
    );
    // The conformance check alone reports no lint.
    let conformance_only = OkfConformance.check(&staged.0)?;
    assert_eq!(located(&conformance_only), Vec::<String>::new());
    // Lint never refuses; a conformance violation still does.
    staged.write("notes/untyped.md", "---\ntitle: No type\n---\nBody\n")?;
    let refused = err_of(plan.check(&staged.0))?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    Ok(())
}
#[test]
fn candidate_check_of_an_edit_reports_validator_warnings_only_for_the_files_it_touched()
-> TestResult {
    let staged = Staged::new()?;
    // A broken cross-link is permitted; the validator reports it as an info naming no file.
    staged.write(
        "notes/plan.md",
        "---\ntype: Note\ntitle: Plan\ndescription: The plan.\n---\n# Plan\n\nSee [gone](gone.md).\n",
    )?;
    // `status` outside OKF's words is a producer deviation the validator only warns about.
    staged.write(
        "notes/odd.md",
        "---\ntype: Note\ntitle: Odd\ndescription: Odd.\nstatus: shelved\n---\n# Odd\n",
    )?;
    let validator = |warnings: &[Warning]| -> Vec<Option<String>> {
        warnings
            .iter()
            .filter(|warning| warning.code.starts_with("okf_validator_"))
            .map(|warning| warning.location.clone())
            .collect()
    };
    let odd = Some("notes/odd.md".to_owned());
    // The whole-bundle check reports the warning about the odd file and the unlocated info.
    let whole = validator(&OkfConformance.check(&staged.0)?);
    assert!(whole.contains(&odd), "{whole:?}");
    assert!(whole.contains(&None), "{whole:?}");
    // An edit of the plan reports nothing about the file it did not touch, nor any warning
    // that names no file.
    let plan = EditCheck::new(
        &[WorkspacePath::try_from("notes/plan.md".to_owned())?],
        None,
    );
    let edit = validator(&plan.check(&staged.0)?);
    assert!(
        edit.iter()
            .all(|location| location.as_deref() == Some("notes/plan.md")),
        "{edit:?}"
    );
    // An edit of the odd file reports its warning.
    let touched_odd = EditCheck::new(&[WorkspacePath::try_from("notes/odd.md".to_owned())?], None);
    assert!(validator(&touched_odd.check(&staged.0)?).contains(&odd));
    // A refusal stands whatever file it names: the untyped file is not the one edited.
    staged.write("notes/untyped.md", "---\ntitle: No type\n---\nBody\n")?;
    let refused = err_of(plan.check(&staged.0))?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert!(
        refused.message.contains("notes/untyped.md"),
        "{}",
        refused.message
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
