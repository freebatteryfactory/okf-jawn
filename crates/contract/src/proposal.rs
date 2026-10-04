//! Suggested changes are separate from both review and acceptance.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Permitted proposal mutations; no review or acceptance variants exist.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, PartialEq, Eq)]
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
}

/// Lifecycle of a suggestion, not approval of a fact in it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, PartialEq, Eq)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
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
    /// Current suggestion state.
    pub status: ProposalStatus,
    /// Server-recorded principal.
    pub created_by: String,
    /// RFC 3339 creation time.
    pub created_at: String,
}

/// Suggest changes without changing the accepted workspace.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
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
    pub idempotency_key: String,
}

/// List suggestions for an authorized workspace.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListProposalsResponse {
    /// Visible proposals.
    pub items: Vec<Proposal>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Open one proposal and its exact content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GetProposalRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Suggestion identity.
    pub proposal_id: crate::identity::ProposalId,
}

/// Accept exactly the displayed proposal against an unchanged workspace head.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
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
    pub confirmation_id: String,
}

/// Close a proposal without changing accepted content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeclineProposalRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Suggestion identity.
    pub proposal_id: crate::identity::ProposalId,
    /// Human-readable closure reason.
    pub reason: String,
}

/// Add a discussion comment without recording content verification.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AddCommentRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Suggestion being discussed.
    pub proposal_id: crate::identity::ProposalId,
    /// Comment text.
    pub text: String,
    /// Optional line or range under discussion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::source::SourceReference>,
}

/// A server-attributed discussion entry.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Comment {
    /// Comment identity.
    pub id: String,
    /// Authenticated subject.
    pub author: String,
    /// Discussion text.
    pub text: String,
    /// RFC 3339 timestamp.
    pub created_at: String,
}
