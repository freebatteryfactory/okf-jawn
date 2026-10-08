//! Human-readable naming conventions with preview and collision semantics.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Supported case transformations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LetterCase {
    /// Preserve case.
    Preserve,
    /// Lower case.
    Lower,
    /// Upper case.
    Upper,
    /// Title case.
    Title,
    /// Snake case.
    Snake,
    /// Kebab case.
    Kebab,
}

/// Declared date interpretation; ambiguous input is not guessed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DateOrder {
    /// Year, month, day.
    Ymd,
    /// Month, day, year.
    Mdy,
    /// Day, month, year.
    Dmy,
    /// Require unambiguous dates or a human choice.
    RejectAmbiguous,
}

/// How proposed naming collisions are handled.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollisionPolicy {
    /// Require an explicit decision.
    Reject,
    /// Add a deterministic suffix.
    Suffix,
    /// Keep the original name and report the collision.
    KeepExisting,
}

/// How filename extensions are handled when applying a naming rule.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionPolicy {
    /// Leave the extension unchanged.
    Preserve,
    /// Lower-case the extension.
    Lowercase,
    /// Drop the extension from the normalized note path (never from originals).
    Strip,
}

/// A typed rule scoped by glob and optional OKF type.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamingRule {
    /// Match against the supplied relative path.
    pub glob: String,
    /// Optional OKF type selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    /// Case transformation.
    pub case: LetterCase,
    /// Word separator.
    pub separator: String,
    /// Optional literal prefix.
    pub prefix: String,
    /// Optional literal suffix applied to the normalized base name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
    /// Declared incoming date convention.
    pub date_order: DateOrder,
    /// Explicit output format.
    pub date_format: String,
    /// Remove filename suffixes only from normalized notes, never originals.
    pub strip_version_suffix: bool,
    /// How the filename extension is transformed on normalized notes.
    pub extension_policy: ExtensionPolicy,
    /// When true, preserve existing path aliases while renaming the primary path.
    pub alias_preservation: bool,
    /// Relative destination template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    /// Collision behavior.
    pub collision: CollisionPolicy,
}

/// Versioned naming rules rendered as a form and editable as YAML.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamingRules {
    /// Format version; this release emits 1.
    pub schema_version: u32,
    /// Rules applied in explicit list order.
    pub rules: Vec<NamingRule>,
}

/// Read the workspace conventions at one version.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetRulesRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
}

/// Persist naming conventions as a versioned workspace file.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetRulesRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Validated rules.
    pub rules: NamingRules,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Calculate a rename plan with no persistent mutation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewNamesRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Selected items.
    pub item_ids: Vec<crate::identity::ItemId>,
    /// Candidate rules, not necessarily saved.
    pub rules: NamingRules,
}

/// A proposed path change with an explanation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameEntry {
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Current path.
    pub from: crate::identity::WorkspacePath,
    /// Proposed path.
    pub to: crate::identity::WorkspacePath,
    /// Ambiguity or collision details.
    pub warnings: Vec<crate::common::Warning>,
}

/// One path observed as a duplicate during rename preview, including paths that differ only in
/// case or Unicode normalization (`WorkspacePath::collision_key`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DuplicateObservation {
    /// Path observed more than once.
    pub path: crate::identity::WorkspacePath,
    /// Item identities that collide on that path.
    pub item_ids: Vec<crate::identity::ItemId>,
    /// Human-readable observation without mutating content.
    pub message: String,
}

/// A preview bound to source state and exact conventions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenamePlan {
    /// Workspace revision inspected.
    pub base_revision: crate::identity::Revision,
    /// Digest of the exact canonical rules.
    pub rules_digest: crate::identity::Digest,
    /// Proposed changes.
    pub entries: Vec<RenameEntry>,
    /// Duplicate path observations recorded during preview.
    pub duplicate_observations: Vec<DuplicateObservation>,
    /// Plan-level warnings.
    pub warnings: Vec<crate::common::Warning>,
}

/// Apply a revision-bound preview and rewrite affected links.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyNamesRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Plan revalidated before any writes.
    pub plan: RenamePlan,
    /// Rules whose digest must match the preview.
    pub rules: NamingRules,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// The file name of the source card for an original named `original`.
///
/// Split at the last `.`: the stem, `-`, the extension, then `.md` (`report.pdf` gives
/// `report-pdf.md`, `v1.2.notes.txt` gives `v1.2.notes-txt.md`). A name with no `.`, or whose
/// only `.` is its first character, gets `.md` alone (`README` gives `README.md`, `.env` gives
/// `.env.md`). Case is kept. A collision then takes the numbered-suffix rule
/// (`report-pdf-2.md`) and the naming rules apply; the original name is kept exactly as
/// supplied in the card's application header.
#[must_use]
pub fn source_card_name(original: &str) -> String {
    match original.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => format!("{stem}-{extension}.md"),
        _ => format!("{original}.md"),
    }
}
