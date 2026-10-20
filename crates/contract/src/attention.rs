//! Diagnostics are actionable observations, never a blanket trust score.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Known maintenance conditions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// A reference does not resolve.
    BrokenLink,
    /// `stale_after` requires checking.
    Stale,
    /// Agent-authored content has no covering review.
    Unreviewed,
    /// Identical bytes have multiple visible appearances.
    Duplicate,
    /// Navigation summary is absent.
    MissingDescription,
    /// A source whose conversion is partial, failed or unsupported; action `redigest_item`.
    Unprocessed,
    /// A completed source whose conversion recorded warnings; action `read_item`.
    ExtractionWarnings,
    /// A pinned View has newer source data available.
    NewerSource,
    /// Possible instruction-like source text; advisory only.
    InstructionLike,
    /// A saved View or a citation refers to purged content; action `get_view`.
    Invalidated,
    /// An imported file's header does not parse; its bytes are kept and nothing is blocked.
    /// Action `get_item`.
    UnparseableHeader,
    /// Uploads that are complete and not imported, with their count and total size in the
    /// message; one row a workspace when any exist. Action `list_uploads`.
    UploadsWaiting,
}

/// Explainable rules in the local English instruction-warning detector.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstructionRule {
    /// Text impersonates a system, developer or administrator message.
    RoleImpersonation,
    /// Text asks for earlier instructions to be ignored or overridden.
    OverrideInstructions,
    /// Text asks for credentials, prompts or hidden instructions.
    RevealSecrets,
    /// Text asks an assistant to execute an action or send data elsewhere.
    ExternalAction,
    /// Invisible or bidirectional controls occur within the text.
    HiddenText,
}

/// One advisory match, quoted from the original text and located like an ordinary citation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstructionFinding {
    /// Rule that matched, not a verdict about intent or safety.
    pub rule_id: InstructionRule,
    /// Exact version of the rules that produced this finding.
    pub rule_set_version: String,
    /// Original matching lines; normalization never rewrites the displayed evidence.
    pub passage: String,
    /// The same revision, representation and locator used by application reads.
    pub source: crate::source::SourceReference,
}

/// Bounded current evidence for one item's instruction-like observation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstructionEvidence {
    /// Language supported by this rule set; version one is English only (`en`).
    pub language: String,
    /// First matches in source order, at most 32 for version one.
    #[schemars(length(max = 32))]
    pub findings: Vec<InstructionFinding>,
    /// Additional matches counted but not included in this bounded evidence list.
    pub omitted_findings: u64,
}

/// An observation with a useful next action.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttentionItem {
    /// Diagnostic class.
    pub kind: AttentionKind,
    /// Relevant item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<crate::identity::ItemId>,
    /// The item at the response's `revision`; absent when the observation is about no one
    /// item or the item was purged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<crate::item::ItemSummary>,
    /// The place inside the item the observation is about, in the form `read_item` takes;
    /// absent when it is about the whole item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<crate::read::Selection>,
    /// Concrete explanation.
    pub message: String,
    /// Suggested existing operation, never executable code.
    pub action: crate::metadata::OperationName,
    /// Present only for instruction-like warnings; absence never certifies safety.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction_like: Option<InstructionEvidence>,
}

/// Read workspace diagnostics at an explicit revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetAttentionRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// Bounded diagnostics; an empty list does not establish safety.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetAttentionResponse {
    /// Inspected revision.
    pub revision: crate::identity::Revision,
    /// Visible observations.
    pub items: Vec<AttentionItem>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Rebuild derived search and link indexes without deleting application records.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RebuildIndexRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}
