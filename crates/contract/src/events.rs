//! Durable retrieval records and live notifications have separate meanings.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What this application returned, not what an external model retained.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    /// Durable operation record.
    pub id: crate::identity::ReceiptId,
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Canonical operation identifier.
    pub operation_id: String,
    /// Authenticated acting principal.
    pub principal_subject: String,
    /// Interface route.
    pub route: crate::access::AccessRoute,
    /// Exact returned source selections.
    pub sources: Vec<crate::source::SourceReference>,
    /// RFC 3339 response time.
    pub returned_at: String,
    /// Optional diagnostic trace reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// `agent_context` or `human_display`; these are distinct.
    pub audience: String,
}

/// Inspect one durable record within workspace permissions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GetReceiptRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Receipt identity.
    pub receipt_id: crate::identity::ReceiptId,
}

/// A resumable workspace change notification, not canonical content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Event {
    /// Opaque monotonic event cursor.
    pub id: String,
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Changed, imported, `job_updated`, reviewed, or `proposal_updated`.
    pub kind: String,
    /// Content revision when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<crate::identity::Revision>,
    /// Affected item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<crate::identity::ItemId>,
    /// Affected job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<crate::identity::JobId>,
}

/// Read changes after an opaque cursor; SSE uses the same record shape.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListEventsRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Last seen event cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// Bounded change notifications.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListEventsResponse {
    /// New notifications.
    pub events: Vec<Event>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}
