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
//!   workspace-relative path when the diagnostic names a file.
//!
//! Lint is not conformance and never refuses: an edit's check (`EditCheck`) adds okf-validator's
//! lint findings of the candidate as warnings, and an import's (`portable::ImportCheck`) adds
//! those of the imported files.
//!
//! Startup hands this same check to ingest's handler, so every commit of the application is
//! judged by one policy.

use std::path::Path;

use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_validator::Severity;

use crate::echo::bounded;
use crate::portable::{lint_code, relative_path};
use crate::storage::CandidateCheck;

/// The application's OKF conformance check of a staged candidate.
#[derive(Debug, Clone, Copy, Default)]
pub struct OkfConformance;

/// The check of an edit (a create, move, status change, delete, folder or type): conformance
/// first, whose refusal stands, then okf-validator's lint of the candidate, whose findings come
/// back as warnings and never refuse (SPEC: validation and lint apply at edit).
///
/// Lint covers the whole candidate bundle, as `okf_validator::lint_bundle` judges it: a
/// finding about a file the edit did not touch is reported too, located at its path.
#[derive(Debug, Clone, Copy, Default)]
pub struct EditCheck;

/// The prefix of the `Warning::code` of a validator diagnostic; the severity follows.
const VALIDATOR_CODE: &str = "okf_validator";

impl CandidateCheck for OkfConformance {
    fn check(&self, root: &Path) -> Result<Vec<Warning>, ApiError> {
        conformance(root, &load(root)?)
    }
}

impl CandidateCheck for EditCheck {
    fn check(&self, root: &Path) -> Result<Vec<Warning>, ApiError> {
        let bundle = load(root)?;
        let mut warnings = conformance(root, &bundle)?;
        warnings.extend(
            okf_validator::lint_bundle(&bundle)
                .diagnostics
                .into_iter()
                .map(|diagnostic| {
                    let (code, message) = lint_code(&diagnostic.message);
                    Warning {
                        code,
                        message,
                        location: diagnostic
                            .path
                            .as_deref()
                            .and_then(|path| relative_path(root, path)),
                    }
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
            message: diagnostic.message,
            location: diagnostic
                .path
                .as_deref()
                .and_then(|path| relative_path(root, path)),
        })
        .collect())
}
