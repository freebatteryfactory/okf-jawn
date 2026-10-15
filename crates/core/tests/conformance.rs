//! The production `CandidateCheck` judges a staged candidate with okf-validator: a
//! conformance violation refuses the candidate with the validator's diagnostics, and softer
//! findings come back as located warnings.

use std::error::Error;
use std::path::PathBuf;

use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::identity::WorkspacePath;
use okf_jawn_core::conformance::{EditCheck, MISSING_FILE_LINK, OkfConformance};
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
    let warnings = OkfConformance.check(&staged.0, &[])?;
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
    let refused = err_of(OkfConformance.check(&staged.0, &[]))?;
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
    let warnings = OkfConformance.check(&staged.0, &[])?;
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
    let refused = err_of(OkfConformance.check(&missing, &[]))?;
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
    let edit = plan.check(&staged.0, &[])?;
    assert!(
        edit.iter().any(|warning| warning.code == "okf_lint_l1"),
        "{edit:?}"
    );
    assert_eq!(located(&edit), vec!["notes/plan.md".to_owned()]);
    let type_change = EditCheck::new(&[], Some("Meeting".to_owned()));
    assert_eq!(
        located(&type_change.check(&staged.0, &[])?),
        vec!["notes/meeting.md".to_owned()]
    );
    // The conformance check alone reports no lint.
    let conformance_only = OkfConformance.check(&staged.0, &[])?;
    assert_eq!(located(&conformance_only), Vec::<String>::new());
    // Lint never refuses; a conformance violation still does.
    staged.write("notes/untyped.md", "---\ntitle: No type\n---\nBody\n")?;
    let refused = err_of(plan.check(&staged.0, &[]))?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    Ok(())
}
#[test]
fn candidate_check_of_an_edit_reports_validator_warnings_only_for_the_files_it_touched()
-> TestResult {
    let staged = Staged::new()?;
    // A broken cross-link is permitted; the validator reports it as an info naming the concept
    // that holds it, and the check locates it at that concept's file.
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
    // The whole-bundle check reports the warning about the odd file, and the plan's broken link
    // located at the plan: no finding of this bundle is left without a location.
    let whole = validator(&OkfConformance.check(&staged.0, &[])?);
    assert!(whole.contains(&odd), "{whole:?}");
    assert!(!whole.contains(&None), "{whole:?}");
    // An edit of the plan reports nothing about the file it did not touch.
    let plan = EditCheck::new(
        &[WorkspacePath::try_from("notes/plan.md".to_owned())?],
        None,
    );
    let edit = validator(&plan.check(&staged.0, &[])?);
    assert!(
        edit.iter()
            .all(|location| location.as_deref() == Some("notes/plan.md")),
        "{edit:?}"
    );
    // An edit of the odd file reports its warning.
    let touched_odd = EditCheck::new(&[WorkspacePath::try_from("notes/odd.md".to_owned())?], None);
    assert!(validator(&touched_odd.check(&staged.0, &[])?).contains(&odd));
    // A refusal stands whatever file it names: the untyped file is not the one edited.
    staged.write("notes/untyped.md", "---\ntitle: No type\n---\nBody\n")?;
    let refused = err_of(plan.check(&staged.0, &[]))?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert!(
        refused.message.contains("notes/untyped.md"),
        "{}",
        refused.message
    );
    Ok(())
}

/// R-I12: the files the store reports it rewrote on its own (a move's referrers) count as
/// touched, for lint and for the validator's warnings alike; a file neither the edit nor the
/// store touched stays unreported.
#[test]
fn candidate_check_of_an_edit_counts_the_files_the_store_rewrote_as_touched() -> TestResult {
    let staged = Staged::new()?;
    staged.write(
        "notes/plan.md",
        "---\ntype: Note\ntitle: Plan\ndescription: The plan.\n---\n# Plan\n",
    )?;
    // No heading (lint) and a status outside OKF's words (a validator warning), twice.
    let flawed =
        "---\ntype: Note\ntitle: Odd\ndescription: Odd.\nstatus: shelved\n---\n\nNo heading.\n";
    staged.write("notes/referrer.md", flawed)?;
    staged.write("notes/bystander.md", flawed)?;
    let located = |warnings: &[Warning], code: &str| -> Vec<String> {
        let mut located: Vec<String> = warnings
            .iter()
            .filter(|warning| warning.code.starts_with(code))
            .filter_map(|warning| warning.location.clone())
            .filter(|location| location != "notes/plan.md")
            .collect();
        located.sort();
        located.dedup();
        located
    };
    let moved = EditCheck::new(
        &[WorkspacePath::try_from("notes/plan.md".to_owned())?],
        None,
    );
    // The edit alone names the plan: nothing about either other file.
    let alone = moved.check(&staged.0, &[])?;
    assert_eq!(located(&alone, "okf_lint"), Vec::<String>::new());
    assert_eq!(located(&alone, "okf_validator_"), Vec::<String>::new());
    // The store rewrote the referrer's link: its findings are reported, the bystander's not.
    let rewritten = [WorkspacePath::try_from("notes/referrer.md".to_owned())?];
    let told = moved.check(&staged.0, &rewritten)?;
    assert_eq!(located(&told, "okf_lint"), ["notes/referrer.md"]);
    assert_eq!(located(&told, "okf_validator_"), ["notes/referrer.md"]);
    assert!(
        told.iter().any(|warning| warning.code == "okf_lint_l1"
            && warning.location.as_deref() == Some("notes/referrer.md")),
        "{told:?}"
    );
    // The conformance check alone reports no lint, whatever was rewritten.
    let conformance_only = OkfConformance.check(&staged.0, &rewritten)?;
    assert_eq!(located(&conformance_only, "okf_lint"), Vec::<String>::new());
    Ok(())
}

/// The broken-link findings among `warnings`, as (location, message), sorted.
fn broken_links(warnings: &[Warning]) -> Vec<(Option<String>, String)> {
    let mut broken: Vec<(Option<String>, String)> = warnings
        .iter()
        .filter(|warning| warning.message.contains("does not resolve to a concept"))
        .map(|warning| (warning.location.clone(), warning.message.clone()))
        .collect();
    broken.sort();
    broken
}

/// The broken-link finding the validator gives for the link target `raw`, located at `file`.
fn broken_at(file: &str, raw: &str) -> (Option<String>, String) {
    (
        Some(file.to_owned()),
        format!("link target does not resolve to a concept in the bundle: {raw}"),
    )
}

/// A note that is conformant and links to `target`, which the tests leave out of the bundle.
fn note_linking_to(title: &str, target: &str) -> String {
    format!(
        "---\ntype: Note\ntitle: {title}\ndescription: A note.\n---\n# {title}\n\nSee [it]({target}).\n"
    )
}

/// The validator's broken-link info names the concept that holds the link and no file. The
/// check locates it at the file the validated bundle holds that concept in, as the tree spells
/// it, once for each broken link; a file with no broken link gets none.
#[test]
fn candidate_check_locates_a_finding_that_names_a_concept_at_that_concepts_file() -> TestResult {
    let staged = Staged::new()?;
    staged.write("notes/plan.md", &note_linking_to("Plan", "gone.md"))?;
    staged.write("notes/Mixed-Case.md", &note_linking_to("Mixed", "lost.md"))?;
    staged.write(
        "notes/whole.md",
        "---\ntype: Note\ntitle: Whole\ndescription: A note.\n---\n# Whole\n\nSee [the plan](plan.md).\n",
    )?;
    let warnings = OkfConformance.check(&staged.0, &[])?;
    assert_eq!(
        broken_links(&warnings),
        [
            broken_at("notes/Mixed-Case.md", "lost.md"),
            broken_at("notes/plan.md", "gone.md"),
        ]
    );
    assert!(
        warnings
            .iter()
            .filter(|warning| warning.message.contains("does not resolve to a concept"))
            .all(|warning| warning.code == "okf_validator_info"),
        "{warnings:?}"
    );
    // Every finding of this bundle is located: none is left for the wrappers to drop.
    assert!(
        warnings.iter().all(|warning| warning.location.is_some()),
        "{warnings:?}"
    );
    Ok(())
}

/// An edit of a note with a broken link reports the link, located at the note; a broken link in
/// a file the edit did not touch is not reported, unless the store rewrote that file.
#[test]
fn candidate_check_of_an_edit_reports_the_broken_links_of_the_files_it_touched_only() -> TestResult
{
    let staged = Staged::new()?;
    staged.write("notes/plan.md", &note_linking_to("Plan", "gone.md"))?;
    staged.write("notes/other.md", &note_linking_to("Other", "lost.md"))?;
    let plan = WorkspacePath::try_from("notes/plan.md".to_owned())?;
    let other = WorkspacePath::try_from("notes/other.md".to_owned())?;
    let edit = EditCheck::new(std::slice::from_ref(&plan), None);
    assert_eq!(
        broken_links(&edit.check(&staged.0, &[])?),
        [broken_at("notes/plan.md", "gone.md")]
    );
    // An edit that touched neither file reports neither link.
    let elsewhere = EditCheck::new(&[], None);
    assert_eq!(broken_links(&elsewhere.check(&staged.0, &[])?), Vec::new());
    // The store rewrote the other file: its link is reported too.
    assert_eq!(
        broken_links(&edit.check(&staged.0, std::slice::from_ref(&other))?),
        [
            broken_at("notes/other.md", "lost.md"),
            broken_at("notes/plan.md", "gone.md"),
        ]
    );
    Ok(())
}

/// Core's findings about links to files that are not Markdown, as (location, message), sorted.
fn missing_files(warnings: &[Warning]) -> Vec<(Option<String>, String)> {
    let mut missing: Vec<(Option<String>, String)> = warnings
        .iter()
        .filter(|warning| warning.code == MISSING_FILE_LINK)
        .map(|warning| (warning.location.clone(), warning.message.clone()))
        .collect();
    missing.sort();
    missing
}

/// A conformant note whose body is `body`.
fn note_with(title: &str, body: &str) -> String {
    format!("---\ntype: Note\ntitle: {title}\ndescription: A note.\n---\n# {title}\n\n{body}\n")
}

/// Links to files the candidate holds, written every way okf-core reads a link (relative,
/// from the root, percent-encoded, with an anchor), give no warning at all: not the
/// validator's broken-link info, which okf-core 0.2.7 gives for each of them, and not core's
/// missing-file warning. The production check and an edit's check that touched the note agree.
#[test]
fn candidate_check_reports_nothing_for_a_link_to_a_file_the_candidate_holds() -> TestResult {
    let staged = Staged::new()?;
    std::fs::create_dir_all(staged.0.join("notes/pics"))?;
    std::fs::create_dir_all(staged.0.join("pics"))?;
    staged.write("notes/pics/p.png", "png")?;
    staged.write("notes/pics/my p.png", "png")?;
    staged.write("pics/q.png", "png")?;
    staged.write(
        "notes/a.md",
        &note_with(
            "A",
            "![p](pics/p.png) ![q](/pics/q.png) ![s](pics/my%20p.png) [r](pics/p.png#top)",
        ),
    )?;
    let a = WorkspacePath::try_from("notes/a.md".to_owned())?;
    for warnings in [
        OkfConformance.check(&staged.0, &[])?,
        EditCheck::new(std::slice::from_ref(&a), None).check(&staged.0, &[])?,
    ] {
        assert_eq!(broken_links(&warnings), Vec::new(), "{warnings:?}");
        assert_eq!(missing_files(&warnings), Vec::new(), "{warnings:?}");
    }
    Ok(())
}

/// A link to a file the candidate does not hold is core's own warning, located at the note
/// that holds the link, in place of the validator's broken-link info; a folder or a file
/// elsewhere with that name does not count as holding it.
#[test]
fn candidate_check_reports_a_link_to_a_missing_file_at_the_note_that_holds_it() -> TestResult {
    let staged = Staged::new()?;
    // Not the linked file: a folder of its name, and the same name in another folder (`q.pdf`
    // at the root for a link from `notes/`, `notes/y.png` for a link to the root's `y.png`).
    std::fs::create_dir_all(staged.0.join("notes/pics/p.png"))?;
    staged.write("q.pdf", "pdf")?;
    staged.write("notes/y.png", "png")?;
    staged.write(
        "notes/a.md",
        &note_with("A", "![p](pics/p.png) [q](q.pdf) ![y](../y.png)"),
    )?;
    let a = WorkspacePath::try_from("notes/a.md".to_owned())?;
    for warnings in [
        OkfConformance.check(&staged.0, &[])?,
        EditCheck::new(std::slice::from_ref(&a), None).check(&staged.0, &[])?,
    ] {
        assert_eq!(broken_links(&warnings), Vec::new(), "{warnings:?}");
        assert_eq!(
            missing_files(&warnings),
            [
                (
                    Some("notes/a.md".to_owned()),
                    "link to a missing file: ../y.png".to_owned()
                ),
                (
                    Some("notes/a.md".to_owned()),
                    "link to a missing file: pics/p.png".to_owned()
                ),
                (
                    Some("notes/a.md".to_owned()),
                    "link to a missing file: q.pdf".to_owned()
                ),
            ],
            "{warnings:?}"
        );
    }
    Ok(())
}

/// Review N5 (probe P6): a link to a file outside the candidate, written from the root with the
/// file's absolute path (`/C:/...` on Windows), names nothing the candidate holds, even though
/// that file exists: it is reported as a missing file. On Windows okf-core reads the target as
/// the segments `C:`, `Users`, ..., and joining `C:/Users/...` onto the root would replace the
/// root with the drive path, so without `holds_file`'s plain-segment guard the lookup would find
/// the outside file and report nothing; the Windows run pins the guard. On Linux (CI) there is no
/// drive prefix: the same link is read below the candidate's root, where no such file is, so
/// there the test holds whether or not the guard is in place.
#[test]
fn candidate_check_never_finds_a_linked_file_outside_the_candidate() -> TestResult {
    let outside = Staged::new()?;
    outside.write("notes/secret.png", "png")?;
    let file = outside.0.join("notes").join("secret.png");
    assert!(file.is_file(), "the outside file exists");
    let absolute = file.display().to_string().replace('\\', "/");
    let target = format!("/{}", absolute.trim_start_matches('/'));
    let staged = Staged::new()?;
    staged.write("notes/a.md", &note_with("A", &format!("![s]({target})")))?;
    let a = WorkspacePath::try_from("notes/a.md".to_owned())?;
    for warnings in [
        OkfConformance.check(&staged.0, &[])?,
        EditCheck::new(std::slice::from_ref(&a), None).check(&staged.0, &[])?,
    ] {
        assert_eq!(
            missing_files(&warnings),
            [(
                Some("notes/a.md".to_owned()),
                format!("link to a missing file: {target}")
            )],
            "{warnings:?}"
        );
    }
    Ok(())
}

/// A broken link to a note, written with `.md` (with an anchor too) or with no extension (in a
/// folder whose name has a dot too), keeps the validator's broken-link info as it is, beside
/// core's warning for a missing file in the same note; and the validator's other findings about a
/// missing file (a path field, V27) are kept as they are.
#[test]
fn candidate_check_keeps_the_validators_finding_for_a_link_to_a_missing_note() -> TestResult {
    let staged = Staged::new()?;
    // A path field naming a missing file: the validator's own finding (V27), not a link.
    let with_resource = note_with(
        "A",
        "[g](gone.md) [h](lost) [s](gone.md#part) [v](v1.2/lost) [i](pics/i.png)",
    )
    .replacen("---\n", "---\nresource: ./data.csv\n", 1);
    staged.write("notes/a.md", &with_resource)?;
    let warnings = OkfConformance.check(&staged.0, &[])?;
    assert!(
        warnings.iter().any(|warning| {
            warning.code == "okf_validator_info"
                && warning.location.as_deref() == Some("notes/a.md")
                && warning.message
                    == "`resource` does not resolve to a file in the bundle: ./data.csv"
        }),
        "{warnings:?}"
    );
    assert_eq!(
        broken_links(&warnings),
        [
            broken_at("notes/a.md", "gone.md"),
            broken_at("notes/a.md", "gone.md#part"),
            broken_at("notes/a.md", "lost"),
            broken_at("notes/a.md", "v1.2/lost"),
        ]
    );
    assert!(
        warnings
            .iter()
            .filter(|warning| warning.message.contains("does not resolve to a concept"))
            .all(|warning| warning.code == "okf_validator_info"),
        "{warnings:?}"
    );
    assert_eq!(
        missing_files(&warnings),
        [(
            Some("notes/a.md".to_owned()),
            "link to a missing file: pics/i.png".to_owned()
        )]
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
