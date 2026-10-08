//! Suggested changes are separate from both review and acceptance.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Permitted proposal mutations; no review or acceptance variants exist.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
    /// Create a note or View with retained properties.
    Create {
        #[doc = "Path."]
        path: crate::identity::WorkspacePath,
        #[doc = "Type name."]
        type_name: String,
        #[doc = "Body."]
        body: String,
        #[doc = "Properties."]
        properties: std::collections::BTreeMap<String, serde_json::Value>,
    },
    /// Replace authored content with preconditions.
    Edit {
        #[doc = "Item id."]
        item_id: crate::identity::ItemId,
        #[doc = "Body."]
        body: String,
        #[doc = "Properties."]
        properties: std::collections::BTreeMap<String, serde_json::Value>,
    },
    /// Move and rewrite links.
    Move {
        #[doc = "Item id."]
        item_id: crate::identity::ItemId,
        #[doc = "Destination."]
        destination: crate::identity::WorkspacePath,
    },
    /// Retire an item without deleting retained objects.
    Archive {
        #[doc = "Item id."]
        item_id: crate::identity::ItemId,
    },
    /// Propose the complete text of an unprocessed source.
    SupplyExtraction {
        #[doc = "Source item."]
        item_id: crate::identity::ItemId,
        #[doc = "Digest the agent saw; absent when the source has none. A different current digest is a conflict."]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        based_on: Option<crate::identity::Digest>,
        #[doc = "Pages the text covers, as the agent claims; empty for the whole."]
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pages: Vec<crate::common::PageRange>,
        #[doc = "The complete proposed text, Markdown."]
        markdown: String,
    },
}

/// What a proposal does; derived from its changes, never chosen by the proposer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    /// Content changes: create, edit, move, archive.
    Content,
    /// Agent-supplied extraction text.
    SupplyExtraction,
}

/// Lifecycle of a suggestion, not approval of a fact in it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    /// Available for review.
    Open,
    /// Merged by an authorized action.
    Accepted,
    /// Closed without applying changes.
    Declined,
    /// Base state changed; reconciliation required.
    Conflict,
}

/// A revision-bound proposed change set.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    /// Proposal identity.
    pub id: crate::identity::ProposalId,
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Base on which the proposal was drafted.
    pub base_revision: crate::identity::Revision,
    /// Exact proposed content.
    pub proposal_revision: crate::identity::Revision,
    /// Server-computed digest of the exact displayed change set.
    pub content_digest: crate::identity::Digest,
    /// Human-readable reason.
    pub title: String,
    /// Rationale and limitations.
    pub description: String,
    /// Proposed mutations.
    pub changes: Vec<Change>,
    /// What the proposal does, derived from `changes` by the server.
    pub kind: ProposalKind,
    /// Current suggestion state.
    pub status: ProposalStatus,
    /// Server-recorded principal.
    pub created_by: String,
    /// Route the proposal came by, recorded by the server.
    pub created_via: crate::access::AccessRoute,
    /// OAuth or connector client it came through, recorded by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Creation time.
    pub created_at: crate::identity::Timestamp,
}

/// Suggest changes without changing the accepted workspace.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenProposalRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Proposal title.
    pub title: String,
    /// Reason and limitations.
    pub description: String,
    /// Allowed suggestion mutations.
    pub changes: Vec<Change>,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// List suggestions for an authorized workspace.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListProposalsRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// State filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ProposalStatus>,
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// Paged proposals.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListProposalsResponse {
    /// Visible proposals.
    pub items: Vec<Proposal>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Open one proposal and its exact content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetProposalRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Suggestion identity.
    pub proposal_id: crate::identity::ProposalId,
}

/// Accept exactly the displayed proposal against an unchanged workspace head.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptProposalRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Suggestion identity.
    pub proposal_id: crate::identity::ProposalId,
    /// Workspace head shown during confirmation.
    pub expected_head: crate::identity::Revision,
    /// Proposal version shown during confirmation.
    pub proposal_revision: crate::identity::Revision,
    /// Server-issued session-bound confirmation; never a model-supplied reviewer name.
    pub confirmation_id: crate::identity::ConfirmationId,
    /// Retry identity; a retry with the same mutation may re-consume its own confirmation.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Close a proposal without changing accepted content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeclineProposalRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Suggestion identity.
    pub proposal_id: crate::identity::ProposalId,
    /// Human-readable closure reason.
    pub reason: String,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Add a discussion comment without recording content verification.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddCommentRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Suggestion being discussed.
    pub proposal_id: crate::identity::ProposalId,
    /// Comment text.
    pub text: String,
    /// Optional line or range under discussion; the caller must be able to read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::source::SourceReference>,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// A server-attributed discussion entry.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Comment {
    /// Comment identity.
    pub id: String,
    /// Authenticated subject.
    pub author: String,
    /// Discussion text.
    pub text: String,
    /// Creation time.
    pub created_at: crate::identity::Timestamp,
    /// The purge that invalidated the comment's `source`, whose stored path was scrubbed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidated_by: Option<crate::identity::PurgeId>,
}

impl ProposalKind {
    /// The kind of a proposal with these changes.
    ///
    /// # Errors
    /// Returns `InvalidInput` on `/changes` when supply changes are mixed with other changes,
    /// or when one proposal supplies text for the same item twice.
    pub fn of(changes: &[Change]) -> Result<Self, crate::error::ApiError> {
        let mut supplied: Vec<crate::identity::ItemId> = Vec::new();
        for change in changes {
            if let Change::SupplyExtraction { item_id, .. } = change {
                if supplied.contains(item_id) {
                    return Err(changes_error(
                        "a proposal supplies the text of one item at most once",
                    ));
                }
                supplied.push(*item_id);
            }
        }
        if supplied.is_empty() {
            Ok(Self::Content)
        } else if supplied.len() == changes.len() {
            Ok(Self::SupplyExtraction)
        } else {
            Err(changes_error(
                "a supply_extraction proposal holds only supply_extraction changes",
            ))
        }
    }
}

fn changes_error(message: &str) -> crate::error::ApiError {
    crate::error::ApiError::new(crate::error::ErrorCode::InvalidInput, message)
        .with_field("/changes")
}
