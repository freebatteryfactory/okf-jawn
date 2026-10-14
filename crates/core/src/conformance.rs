//! The production `CandidateCheck`: OKF conformance of a staged candidate, judged by
//! okf-validator (SPEC section 3; `crates/core/AGENTS.md`).
//!
//! Storage stages every candidate tree and calls the check it is handed; it never decides
//! conformance itself. This check loads the staged directory with okf-core and runs
//! `okf_validator::validate_bundle` on it:
//!
//! - an `error` diagnostic is a conformance violation, so the candidate is refused as
//!   `InvalidInput` and the message carries the validator's error diagnostics;
//! - `warning` and `info` diagnostics never refuse; they come back as warnings, located at the
//!   workspace-relative path when the diagnostic names a file, or, when it names only a concept
//!   (a broken link names the concept that holds it), at the file the validated bundle holds
//!   that concept in. A finding that names neither, or a concept the bundle does not hold, has
//!   no location.
//!
//! Lint is not conformance and never refuses: an edit's check (`EditCheck`) adds okf-validator's
//! lint findings about the files the edit touched as warnings, and an import's
//! (`portable::ImportCheck`) adds those of the imported files. The files an edit touched are the
//! ones its caller names and the ones the store reports it rewrote on its own (a move's
//! referrers, `CandidateCheck::check`). An edit's check also reports the
//! validator's own warnings and infos only for the files it touched; its refusal stands whatever
//! file it names.
//!
//! Startup hands this same check to ingest's handler, so every commit of the application is
//! judged by one policy.

use std::collections::BTreeSet;
use std::path::Path;

use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::WorkspacePath;
use okf_validator::Severity;

use crate::echo::bounded;
use crate::portable::{lint_code, relative_path};
use crate::storage::CandidateCheck;

/// The application's OKF conformance check of a staged candidate.
#[derive(Debug, Clone, Copy, Default)]
pub struct OkfConformance;

/// The check of an edit (a create, move, status change, delete, folder or type): conformance
/// first, then okf-validator's lint, whose findings come back as warnings and never refuse
/// (SPEC: validation and lint apply at edit).
///
/// The whole candidate is validated and linted, because a rule can read across files. A
/// validation refusal stands whatever file it names, because a non-conformant candidate is never
/// committed. Every other finding, the validator's warnings and infos and every lint finding, is
/// reported only for the files the edit touched, as `portable::ImportCheck` reports only the
/// imported files': the paths it wrote, moved (old and new) or deleted, the files the store
/// rewrote on its own while it applied the edit (the referrers whose links a move rewrote, which
/// the store reports to the check as `rewritten`), and for a type change
/// the concepts of that type in the candidate. So an edit reports no warning about a file it did
/// not touch, and a warning that names no file is not reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditCheck {
    /// Workspace-relative paths the edit wrote, moved from or to, or deleted.
    touched: BTreeSet<String>,
    /// The type a type change redefined; its concepts count as touched.
    of_type: Option<String>,
}

/// The prefix of the `Warning::code` of a validator diagnostic; the severity follows.
const VALIDATOR_CODE: &str = "okf_validator";

impl CandidateCheck for OkfConformance {
    fn check(&self, root: &Path, _rewritten: &[WorkspacePath]) -> Result<Vec<Warning>, ApiError> {
        conformance(root, &load(root)?)
    }
}

impl EditCheck {
    /// The check of an edit that touched `touched` and, for a type change, redefined `of_type`.
    #[must_use]
    pub fn new(touched: &[WorkspacePath], of_type: Option<String>) -> Self {
        Self {
            touched: touched
                .iter()
                .map(|path| path.as_str().to_owned())
                .collect(),
            of_type,
        }
    }
}

impl CandidateCheck for EditCheck {
    fn check(&self, root: &Path, rewritten: &[WorkspacePath]) -> Result<Vec<Warning>, ApiError> {
        let bundle = load(root)?;
        // A refusal stands whatever file it names: a non-conformant candidate is never committed.
        let validated = conformance(root, &bundle)?;
        let mut reported = self.touched.clone();
        // The files the store rewrote on its own (a move's referrers) are touched too.
        reported.extend(rewritten.iter().map(|path| path.as_str().to_owned()));
        if let Some(type_name) = self.of_type.as_deref() {
            reported.extend(
                bundle
                    .concepts_of_type(type_name)
                    .filter_map(|concept| relative_path(root, &concept.path)),
            );
        }
        let mut warnings: Vec<Warning> = validated
            .into_iter()
            .filter(|warning| {
                warning
                    .location
                    .as_ref()
                    .is_some_and(|location| reported.contains(location))
            })
            .collect();
        warnings.extend(
            okf_validator::lint_bundle(&bundle)
                .diagnostics
                .into_iter()
                .filter_map(|diagnostic| {
                    let location = diagnostic
                        .path
                        .as_deref()
                        .and_then(|path| relative_path(root, path))
                        .filter(|location| reported.contains(location))?;
                    let (code, message) = lint_code(&diagnostic.message);
                    Some(Warning {
                        code,
                        message,
                        location: Some(location),
                    })
                }),
        );
        Ok(warnings)
    }
}

/// Load the staged candidate with okf-core.
fn load(root: &Path) -> Result<okf_core::Bundle, ApiError> {
    okf_core::Bundle::load(root).map_err(|error| {
        ApiError::new(
            ErrorCode::Internal,
            format!(
                "the candidate bundle could not be read: {}",
                error
                    .io_kind()
                    .map_or_else(|| "not a directory".to_owned(), |kind| kind.to_string())
            ),
        )
    })
}

/// Judge a loaded candidate: an error diagnostic refuses it, the others are warnings.
fn conformance(root: &Path, bundle: &okf_core::Bundle) -> Result<Vec<Warning>, ApiError> {
    let report = okf_validator::validate_bundle(bundle);
    if !report.is_conformant() {
        let violations: Vec<String> = report
            .of(Severity::Error)
            .map(|diagnostic| {
                match diagnostic
                    .path
                    .as_deref()
                    .and_then(|path| relative_path(root, path))
                {
                    Some(location) => format!("{location}: {}", diagnostic.message),
                    None => diagnostic.message.clone(),
                }
            })
            .collect();
        return Err(ApiError::new(
            ErrorCode::InvalidInput,
            bounded(format!(
                "the change would leave the workspace a non-conformant OKF bundle: {}",
                violations.join("; ")
            )),
        ));
    }
    Ok(report
        .diagnostics
        .into_iter()
        .map(|diagnostic| Warning {
            code: format!("{VALIDATOR_CODE}_{}", diagnostic.severity.as_str()),
            location: location(root, bundle, &diagnostic),
            message: diagnostic.message,
        })
        .collect())
}

/// Where a validator finding is located: the file it names, else the file of the concept it
/// names, as a workspace-relative path the way the tree spells it; `None` when it names neither,
/// or a concept the bundle does not hold.
///
/// okf-validator 0.2.7 names a concept and no file in one finding only, the broken-link info
/// (`validate.rs`, "link target does not resolve to a concept in the bundle"), and that concept
/// is the one holding the link. The bundle resolves a concept to its file itself
/// (`okf_core::Bundle::get`): it keys its concepts by id, and an id is the file's path below the
/// root without `.md` (`ConceptId::from_path`), so two files never share one.
fn location(
    root: &Path,
    bundle: &okf_core::Bundle,
    diagnostic: &okf_validator::Diagnostic,
) -> Option<String> {
    match (&diagnostic.path, &diagnostic.concept) {
        (Some(path), _) => relative_path(root, path),
        (None, Some(concept)) => bundle
            .get(concept)
            .and_then(|found| relative_path(root, &found.path)),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use okf_core::ConceptId;
    use okf_validator::{Diagnostic, Severity};

    use super::location;

    /// Result of a test that reports failure through `?` instead of panicking.
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A bundle directory under the system temporary directory, removed when dropped.
    struct Staged(PathBuf);

    impl Drop for Staged {
        fn drop(&mut self) {
            // Best effort: a leftover directory under the temporary directory harms nothing.
            let _left = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A finding of the validator's shape that names `path` and `concept`.
    fn finding(path: Option<PathBuf>, concept: Option<&str>) -> Result<Diagnostic, String> {
        Ok(Diagnostic {
            severity: Severity::Info,
            path,
            concept: concept
                .map(ConceptId::parse)
                .transpose()
                .map_err(|error| error.to_string())?,
            message: "a finding".to_owned(),
            fixable: false,
        })
    }

    /// The real validator names a concept and no file only for a concept the bundle holds (the
    /// one holding a broken link), so the cases it cannot give are built here: a finding that
    /// names a concept the bundle does not hold, or nothing, has no location.
    #[test]
    fn a_finding_is_located_at_its_file_else_at_its_concepts_file_else_nowhere() -> TestResult {
        let staged = Staged(
            std::env::temp_dir().join(format!("okf-jawn-location-{}", uuid::Uuid::new_v4())),
        );
        std::fs::create_dir_all(staged.0.join("notes"))?;
        std::fs::write(
            staged.0.join("notes/plan.md"),
            "---\ntype: Note\n---\n# Plan\n",
        )?;
        let bundle = okf_core::Bundle::load(&staged.0)?;
        let at_concept = finding(None, Some("notes/plan"))?;
        assert_eq!(
            location(&staged.0, &bundle, &at_concept).as_deref(),
            Some("notes/plan.md")
        );
        let unknown = finding(None, Some("notes/gone"))?;
        assert_eq!(location(&staged.0, &bundle, &unknown), None);
        assert_eq!(location(&staged.0, &bundle, &finding(None, None)?), None);
        // A file it names comes first, whatever concept it names.
        let named = finding(Some(staged.0.join("notes/other.md")), Some("notes/plan"))?;
        assert_eq!(
            location(&staged.0, &bundle, &named).as_deref(),
            Some("notes/other.md")
        );
        Ok(())
    }
}
