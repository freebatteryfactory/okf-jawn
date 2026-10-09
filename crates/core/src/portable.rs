//! The rules a portable export and an import apply to OKF trust metadata, and OKF lint at
//! import (SPEC sections 8 and 12; Stage 1b design section 8 and owner decision F).
//!
//! - Export writes each app review that is `current` at the exported revision into the item's
//!   file as an OKF `verified` entry `{ by: human:<subject>, at }`. Only a review of the whole
//!   item (`Selection::All`) is written: OKF `verified` speaks for the whole concept, and a review
//!   of some lines does not.
//! - An incoming `verified` in an imported file is an unconfirmed claim. It is read with OKF's own
//!   rules (a bare `{ by, at }` mapping is a one-element list), it is shown as
//!   `ReviewCoverage::Imported`, and it never becomes a review record: no `human:` prefix in a
//!   file is review evidence (SPEC section 8).
//! - OKF lint runs at import. `ImportCheck` wraps the conformance check of an import commit and
//!   adds okf-validator's lint findings for the imported files as warnings; lint never refuses.
//!
//! Ingest's export and import handlers call these functions and never re-implement them. They
//! hold the rules and do no IO beyond reading the candidate directory a `CandidateCheck` is
//! handed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};
use std::sync::Arc;

use okf_core::trust::Verification;
use okf_core::yaml::{Mapping, Value as YamlValue};
use okf_jawn_contract::{
    common::Warning,
    error::{ApiError, ErrorCode},
    identity::{Digest, WorkspacePath},
    item::ItemDocument,
    read::Selection,
    review::{Review, ReviewCoverage},
};
use serde::Serialize;
use serde_json::Value;

use crate::items::without_header;
use crate::mutations::request_digest;
use crate::storage::CandidateCheck;

/// One `verified` event an imported file claims, kept exactly as written.
///
/// It is a claim, never a review: it has no review identity, no authenticated reviewer and no
/// confirmed content, and its coverage is always `ReviewCoverage::Imported`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedVerification {
    /// The claimed verifier as written; `None` when the entry names none.
    pub by: Option<String>,
    /// The claimed instant as written; `None` when the entry has none.
    pub at: Option<String>,
}

/// The candidate check of an import commit: the conformance check first, whose refusal stands,
/// then OKF lint of the imported files, whose findings are added to its warnings.
///
/// Lint findings never refuse a commit; okf-validator's lint is not a conformance check.
pub struct ImportCheck {
    conformance: Arc<dyn CandidateCheck>,
    imported: Vec<WorkspacePath>,
}

/// What a whole-item review covers: the body and the properties without the application
/// header, which is server bookkeeping (an archive or a redigest's header update is not a
/// content change).
#[derive(Serialize)]
struct ReviewedContent<'a> {
    body: &'a str,
    properties: BTreeMap<String, Value>,
}

/// The OKF frontmatter key of verification events.
pub const VERIFIED_KEY: &str = "verified";

/// The OKF actor prefix of a person; a reviewer is always one (only a human session reviews).
const HUMAN_ACTOR: &str = "human:";

/// The prefix of the `Warning::code` of an okf-validator lint finding; the rule follows.
const LINT_CODE: &str = "okf_lint";

impl ImportedVerification {
    /// How the claim is shown: imported coverage, whoever it names and however valid it is.
    #[must_use]
    pub const fn coverage(&self) -> ReviewCoverage {
        ReviewCoverage::Imported
    }
}

impl ImportCheck {
    /// Wrap `conformance` (the production `CandidateCheck`) for a commit that writes the
    /// `imported` paths.
    #[must_use]
    pub fn new(conformance: Arc<dyn CandidateCheck>, imported: Vec<WorkspacePath>) -> Self {
        Self {
            conformance,
            imported,
        }
    }
}

impl CandidateCheck for ImportCheck {
    fn check(&self, root: &Path) -> Result<Vec<Warning>, ApiError> {
        let mut warnings = self.conformance.check(root)?;
        warnings.extend(lint_imported(root, &self.imported)?);
        Ok(warnings)
    }
}

/// The digest a review of a whole item records, computed from the item's committed content.
///
/// SHA-256 over the body and the properties without the application header, with object keys
/// sorted (`mutations::request_digest`), so it does not depend on how a store orders keys.
///
/// # Errors
/// Returns `Internal` when the content cannot be serialized.
pub fn item_content_digest(document: &ItemDocument) -> Result<Digest, ApiError> {
    request_digest(&ReviewedContent {
        body: &document.body,
        properties: without_header(&document.properties),
    })
}

/// A recorded review's coverage of the content it names, at a revision where that content has
/// the digest `content`.
///
/// `Invalidated` (the reviewed content was purged) and `Imported` are never promoted. Otherwise
/// the review is `Current` exactly when `content` is the digest it recorded, and `Changed` when
/// it is not; a later edit never extends a review.
#[must_use]
pub fn coverage_at(review: &Review, content: &Digest) -> ReviewCoverage {
    match review.coverage {
        ReviewCoverage::Current | ReviewCoverage::Changed => {
            if review.content_digest == *content {
                ReviewCoverage::Current
            } else {
                ReviewCoverage::Changed
            }
        }
        ReviewCoverage::Imported | ReviewCoverage::Unreviewed | ReviewCoverage::Invalidated => {
            review.coverage.clone()
        }
    }
}

/// The `verified` value a portable export writes into one item's file.
///
/// - `existing` is the file's own `verified` at the exported revision, such as an imported
///   claim; it is kept as written.
/// - `reviews` are the item's app reviews (`RecordStore::list_reviews`).
/// - `content` is `item_content_digest` of the item at the exported revision.
///
/// Each review of the whole item whose `coverage_at` that content is `Current` is appended,
/// oldest first, as `{ by: "human:<reviewer subject>", at: <reviewed_at> }`, unless the list
/// already holds that exact entry. A bare mapping is first read as a one-element list, as OKF
/// requires; any other shape is kept as the first element, where OKF readers ignore it as they
/// did before. Returns `existing` unchanged when no review is written, and `None` when there is
/// nothing to write, so an export never adds an empty `verified`.
#[must_use]
pub fn exported_verified(
    existing: Option<&Value>,
    reviews: &[Review],
    content: &Digest,
) -> Option<Value> {
    let written: Vec<Value> = reviews
        .iter()
        .filter(|review| {
            review.source.selection == Selection::All
                && coverage_at(review, content) == ReviewCoverage::Current
        })
        .map(verified_entry)
        .collect();
    if written.is_empty() {
        return existing.cloned();
    }
    let mut entries = match existing {
        None => Vec::new(),
        Some(Value::Array(entries)) => entries.clone(),
        Some(other) => vec![other.clone()],
    };
    for entry in written {
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    Some(Value::Array(entries))
}

/// The verification events an imported file claims in its frontmatter `properties`, read with
/// okf-core's own `verified` rules: a list of `{ by, at }` mappings, or a bare mapping read as
/// a one-element list; any other shape claims nothing.
///
/// Each claim's coverage is `ReviewCoverage::Imported`. The import writes no review record for
/// any of them, and the property stays in the file as written.
#[must_use]
pub fn imported_verification(properties: &BTreeMap<String, Value>) -> Vec<ImportedVerification> {
    let Some(value) = properties.get(VERIFIED_KEY) else {
        return Vec::new();
    };
    Verification::list_from_value(&yaml_value(value))
        .into_iter()
        .map(|event| ImportedVerification {
            by: event.by.map(|by| by.as_str().to_owned()),
            at: event.at.map(|at| at.raw),
        })
        .collect()
}

/// okf-validator's lint findings for the `imported` files of the candidate bundle at `root`,
/// as warnings located at the workspace-relative path.
///
/// Findings about other files, and bundle-level findings that name no file, are not the
/// import's and are left out. Each warning's code is `okf_lint_` and the lowercase rule code
/// (`okf_lint_l1` for rule `L1`), and its message is the finding's text.
///
/// # Errors
/// Returns `Internal` when the candidate directory cannot be read as a bundle.
pub fn lint_imported(root: &Path, imported: &[WorkspacePath]) -> Result<Vec<Warning>, ApiError> {
    let bundle = okf_core::Bundle::load(root).map_err(|error| {
        ApiError::new(
            ErrorCode::Internal,
            format!(
                "the candidate bundle could not be read for lint: {}",
                error
                    .io_kind()
                    .map_or_else(|| "not a directory".to_owned(), |kind| kind.to_string())
            ),
        )
    })?;
    let imported: BTreeSet<&str> = imported.iter().map(WorkspacePath::as_str).collect();
    Ok(okf_validator::lint_bundle(&bundle)
        .diagnostics
        .into_iter()
        .filter_map(|diagnostic| {
            let location = diagnostic
                .path
                .as_deref()
                .and_then(|path| relative_path(root, path))
                .filter(|location| imported.contains(location.as_str()))?;
            let (code, message) = lint_code(&diagnostic.message);
            Some(Warning {
                code,
                message,
                location: Some(location),
            })
        })
        .collect())
}

/// The OKF `verified` entry of one app review.
fn verified_entry(review: &Review) -> Value {
    let mut entry = serde_json::Map::new();
    entry.insert(
        "by".to_owned(),
        Value::String(format!("{HUMAN_ACTOR}{}", review.reviewer_subject)),
    );
    entry.insert(
        "at".to_owned(),
        Value::String(review.reviewed_at.as_str().to_owned()),
    );
    Value::Object(entry)
}

/// The okf-core value of a JSON property, so okf-core's own readers apply to it.
fn yaml_value(value: &Value) -> YamlValue {
    match value {
        Value::Null => YamlValue::Null,
        Value::Bool(flag) => YamlValue::Bool(*flag),
        Value::Number(number) => number.as_i64().map_or_else(
            || {
                number
                    .as_f64()
                    .map_or_else(|| YamlValue::String(number.to_string()), YamlValue::Float)
            },
            YamlValue::Int,
        ),
        Value::String(text) => YamlValue::String(text.clone()),
        Value::Array(items) => YamlValue::Sequence(items.iter().map(yaml_value).collect()),
        Value::Object(map) => {
            let mut mapping = Mapping::new();
            for (key, child) in map {
                mapping.insert(key.clone(), yaml_value(child));
            }
            YamlValue::Mapping(mapping)
        }
    }
}

/// `path` relative to `root`, `/`-separated; `None` when it is not a plain path below `root`.
fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let segments = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|component| match component {
            Component::Normal(segment) => segment.to_str(),
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => None,
        })
        .collect::<Option<Vec<&str>>>()?;
    (!segments.is_empty()).then(|| segments.join("/"))
}

/// The warning code and text of a lint message, which okf-validator writes as `[L1] text`.
fn lint_code(message: &str) -> (String, String) {
    message
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("] "))
        .map_or_else(
            || (LINT_CODE.to_owned(), message.to_owned()),
            |(rule, text)| {
                (
                    format!("{LINT_CODE}_{}", rule.to_ascii_lowercase()),
                    text.to_owned(),
                )
            },
        )
}
