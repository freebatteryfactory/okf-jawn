//! Human-readable naming conventions with preview and collision semantics.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Supported case transformations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, PartialEq, Eq)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, PartialEq, Eq)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollisionPolicy {
    /// Require an explicit decision.
    Reject,
    /// Add a deterministic suffix.
    Suffix,
    /// Keep the original name and report the collision.
    KeepExisting,
}

/// A typed rule scoped by glob and optional OKF type.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
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
    /// Declared incoming date convention.
    pub date_order: DateOrder,
    /// Explicit output format.
    pub date_format: String,
    /// Remove filename suffixes only from normalized notes, never originals.
    pub strip_version_suffix: bool,
    /// Relative destination template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    /// Collision behavior.
    pub collision: CollisionPolicy,
}

/// Versioned naming rules rendered as a form and editable as YAML.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NamingRules {
    /// Format version; this release emits 1.
    pub schema_version: u32,
    /// Rules applied in explicit list order.
    pub rules: Vec<NamingRule>,
}

/// Read the workspace conventions at one version.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GetRulesRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
}

/// Persist naming conventions as a versioned workspace file.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetRulesRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Validated rules.
    pub rules: NamingRules,
}

/// Calculate a rename plan with no persistent mutation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
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

/// A preview bound to source state and exact conventions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RenamePlan {
    /// Workspace revision inspected.
    pub base_revision: crate::identity::Revision,
    /// Digest of the exact canonical rules.
    pub rules_digest: crate::identity::Digest,
    /// Proposed changes.
    pub entries: Vec<RenameEntry>,
    /// Plan-level warnings.
    pub warnings: Vec<crate::common::Warning>,
}

/// Apply a revision-bound preview and rewrite affected links.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyNamesRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Plan revalidated before any writes.
    pub plan: RenamePlan,
    /// Rules whose digest must match the preview.
    pub rules: NamingRules,
}
