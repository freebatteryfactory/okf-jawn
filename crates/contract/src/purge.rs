//! Archive is reversible; purge is explicit, destructive and recorded.
//!
//! A purge authorizes at the tenant only, because it removes the workspace's grants: a
//! workspace target could not authorize the retry of a half-finished purge. A purge is unique
//! per target while it is not completed, so repeating the request (any idempotency key)
//! returns the unfinished purge and queues its job again; that is checked before
//! `base_revision`, so a moved or removed head never blocks the resumption. A purge cannot be
//! cancelled. Its record holds identities and counts, never content of the target.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Permanently remove a workspace.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PurgeWorkspaceRequest {
    /// Workspace to remove.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this purge is based; a moved head conflicts.
    pub base_revision: crate::identity::Revision,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Permanently remove one item, its history and its derivatives.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PurgeItemRequest {
    /// Workspace of the item.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Item to remove.
    pub item_id: crate::identity::ItemId,
    /// Exact revision on which this purge is based; a moved head conflicts.
    pub base_revision: crate::identity::Revision,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Read one purge record.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetPurgeRequest {
    /// Purge identity.
    pub purge_id: crate::identity::PurgeId,
}

/// What a purge removes; identities only, never a name or a path.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PurgeTarget {
    /// A whole workspace.
    Workspace {
        /// Removed workspace.
        workspace_id: crate::identity::WorkspaceId,
    },
    /// One item.
    Item {
        /// Its workspace.
        workspace_id: crate::identity::WorkspaceId,
        /// Removed item.
        item_id: crate::identity::ItemId,
    },
}

/// Progress of a purge.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PurgeState {
    /// Accepted; nothing removed yet.
    Requested,
    /// Removing.
    Running,
    /// Every application-managed copy is removed or rewritten; no affected backup is restorable.
    Completed,
    /// Stopped; repeating the request resumes it.
    Failed,
}

/// What a completed purge did; counts only.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PurgeReport {
    /// Stored objects deleted, as a decimal string.
    pub objects_removed: String,
    /// Objects kept because a surviving item still references them, as a decimal string.
    pub objects_kept_shared: String,
    /// Managed backups deleted (workspace archives, and their copies in pre-migration backups).
    pub backups_removed: u32,
    /// Managed backups rewritten without the target.
    pub backups_rewritten: u32,
    /// Retained export and View export artifacts deleted.
    pub exports_removed: u32,
    /// Proposals deleted with their candidate references and comments.
    pub proposals_removed: u32,
    /// Drafts deleted.
    pub drafts_removed: u32,
    /// Revisions that no longer exist or were rewritten.
    pub revisions_invalidated: u32,
    /// Stored citations of other items remapped to a rewritten revision.
    pub citations_remapped: u32,
    /// Stored citations (reviews, receipts, comments, View bindings) of the target invalidated.
    pub citations_invalidated: u32,
    /// Saved Views with an invalidated binding.
    pub views_invalidated: u32,
}

/// A durable, tenant-level purge record; it holds no content of the target.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Purge {
    /// Identity.
    pub id: crate::identity::PurgeId,
    /// What is removed.
    pub target: PurgeTarget,
    /// Progress.
    pub state: PurgeState,
    /// The tenant job doing the work.
    pub job_id: crate::identity::JobId,
    /// Administrator subject.
    pub requested_by: String,
    /// When it was accepted.
    pub requested_at: crate::identity::Timestamp,
    /// When it completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<crate::identity::Timestamp>,
    /// Present exactly when completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<PurgeReport>,
    /// Last failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<crate::error::ApiError>,
}
