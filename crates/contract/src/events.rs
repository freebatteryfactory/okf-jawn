//! Durable retrieval records and live notifications have separate meanings.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where returned content went; the two are distinct evidence.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptAudience {
    /// Supplied to an agent's context through a tool result.
    AgentContext,
    /// Shown to a person in the workspace.
    HumanDisplay,
}

/// Kinds of workspace change notification.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Committed content changed.
    Changed,
    /// An import produced items.
    Imported,
    /// A job's durable state changed.
    JobUpdated,
    /// A review was recorded.
    Reviewed,
    /// A proposal's state or discussion changed.
    ProposalUpdated,
    /// A local connector credential was issued (tenant-level).
    ConnectorIssued,
    /// A local connector credential was revoked (tenant-level).
    ConnectorRevoked,
    /// A browser session began (tenant-level).
    SignedIn,
    /// A sign-in was refused (tenant-level). It carries `at`, and `actor` only when a subject
    /// was established.
    ///
    /// Each refusal from one source is recorded as its own event up to an allowance a minute;
    /// past it, the refusals of that source in that minute are recorded as one event with their
    /// `count`, written at the end of that minute or when the server stops. So every refusal is
    /// counted, and what one source can write is bounded. The source and the allowance are the
    /// server's. A refusal before any tenant is known (a hosted callback that names no
    /// organization) belongs to no tenant's log: the server's ingress limit and its diagnostics
    /// count it.
    SignInRefused,
    /// An authenticated request was refused for lack of a grant or by its route.
    PermissionDenied,
}

/// Who caused a security event.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EventActor {
    /// Authenticated subject.
    pub subject: String,
    /// Route it came by.
    pub route: crate::access::AccessRoute,
    /// OAuth or connector client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

/// What this application returned, not what an external model retained.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    /// Durable operation record.
    pub id: crate::identity::ReceiptId,
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Canonical operation identifier.
    pub operation_id: crate::metadata::OperationName,
    /// Authenticated acting principal.
    pub principal_subject: String,
    /// Interface route.
    pub route: crate::access::AccessRoute,
    /// Exact returned source selections.
    pub sources: Vec<crate::source::SourceReference>,
    /// Response time.
    pub returned_at: crate::identity::Timestamp,
    /// Optional diagnostic trace reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Agent context or human display; these are distinct.
    pub audience: ReceiptAudience,
    /// The purge that invalidated a source this receipt cites; its stored path was scrubbed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidated_by: Option<crate::identity::PurgeId>,
}

/// Inspect one durable record within workspace permissions.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetReceiptRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Receipt identity.
    pub receipt_id: crate::identity::ReceiptId,
}

/// A resumable change notification, not canonical content.
///
/// A tenant-level event (a sign-in or a refused one, a connector issued or revoked, a refusal
/// outside a workspace) has no `workspace_id`. Security events (`EventKind::is_security`) carry
/// their `actor`, except a `sign_in_refused` event before a subject was established.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Event {
    /// Opaque monotonic event cursor.
    pub id: String,
    /// Workspace whose permissions and storage scope apply; absent for a tenant-level event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<crate::identity::WorkspaceId>,
    /// What changed.
    pub kind: EventKind,
    /// When the store recorded it.
    pub at: crate::identity::Timestamp,
    /// Content revision when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<crate::identity::Revision>,
    /// Affected item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<crate::identity::ItemId>,
    /// Affected job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<crate::identity::JobId>,
    /// Affected connector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_id: Option<crate::identity::ConnectorId>,
    /// Who caused it; present on security events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<EventActor>,
    /// The refused operation of a `permission_denied` event; absent when a byte transport was
    /// refused (`transport`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<crate::metadata::OperationName>,
    /// The refused byte transport of a `permission_denied` event (a download, or an upload's
    /// bytes, which are not operations); `operation` is then absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<crate::transport::TransportName>,
    /// Affected proposal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_id: Option<crate::identity::ProposalId>,
    /// How many occurrences this one event stands for; absent means one. A `sign_in_refused`
    /// event past its source's allowance counts that source's refusals of the minute, and a
    /// `permission_denied` event may count the same way when the server bounds its flood.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

/// Read changes after an opaque cursor; SSE uses the same record shape.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

/// Read installation-level events after an opaque cursor.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTenantEventsRequest {
    /// Last seen event cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// Bounded pagination with an opaque cursor.
    pub page: crate::common::PageRequest,
}

/// Bounded change notifications.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListEventsResponse {
    /// New notifications.
    pub events: Vec<Event>,
    /// Continuation cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl EventKind {
    /// Whether this is a security event: who signed in or was refused a sign-in, a connector
    /// issued or revoked, and a request refused (`permission_denied`).
    ///
    /// In a workspace's log only `permission_denied` occurs; the others are tenant-level and
    /// live in the tenant's log, which `list_tenant_events` reads with the tenant's `admin`.
    /// `list_events` and `stream_events` return a workspace's security events only to a caller
    /// holding `admin` on it.
    #[must_use]
    pub const fn is_security(&self) -> bool {
        matches!(
            self,
            Self::ConnectorIssued
                | Self::ConnectorRevoked
                | Self::SignedIn
                | Self::SignInRefused
                | Self::PermissionDenied
        )
    }
}
