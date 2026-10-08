//! Resumable notifications are projections, not canonical document content.
//!
//! A notification is a hint to read again; it is appended after the durable change it projects.
//! The store allocates the monotonic cursor and stamps `at`. Workspace events (content, imports,
//! jobs, reviews, proposals, a refusal inside a workspace) and tenant events (sign-ins,
//! connector issue and revocation, a refusal outside a workspace) are separate logs
//! (`EventScope`); a security event names its actor.

use okf_jawn_contract::access::Principal;
use okf_jawn_contract::events::{Event, EventActor, EventKind, ListEventsResponse};
use okf_jawn_contract::identity::{ConnectorId, ItemId, JobId, MutationId, Revision, TenantId};
use okf_jawn_contract::metadata::OperationName;

use crate::ports::PortFuture;
use crate::storage::{Page, StorageScope};

/// Which log an event belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventScope {
    /// Installation-level events; the wire `Event` has no `workspace_id`.
    Tenant(TenantId),
    /// Events of one workspace.
    Workspace(StorageScope),
}

/// One notification to append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
    /// What changed.
    pub kind: EventKind,
    /// Content revision, when the change produced one.
    pub revision: Option<Revision>,
    /// Affected item.
    pub item_id: Option<ItemId>,
    /// Affected job.
    pub job_id: Option<JobId>,
    /// Affected connector.
    pub connector_id: Option<ConnectorId>,
    /// Who caused it; present on security events.
    pub actor: Option<EventActor>,
    /// The refused operation of a `permission_denied` event.
    pub operation: Option<OperationName>,
}

/// Which notifications to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventQuery {
    /// Last event cursor the reader has seen; `None` reads from the start.
    pub after: Option<String>,
    /// Bounded page.
    pub page: Page,
}

/// Append-only event log for live notifications; storage owns the implementation.
pub trait EventLog: Send + Sync {
    /// Append one notification after the durable change it projects.
    ///
    /// With a `mutation_id`, an identical notification already appended under that mutation is
    /// not appended again and the prior event is returned. Without one, as for job progress, the
    /// notification is always appended.
    fn append<'a>(
        &'a self,
        scope: &'a EventScope,
        mutation_id: Option<MutationId>,
        event: NewEvent,
    ) -> PortFuture<'a, Event>;
    /// Read notifications after a cursor, oldest first, with bounded pagination.
    fn list<'a>(
        &'a self,
        scope: &'a EventScope,
        query: EventQuery,
    ) -> PortFuture<'a, ListEventsResponse>;
}

impl NewEvent {
    /// A `permission_denied` event: `principal` was refused `operation`.
    #[must_use]
    pub fn permission_denied(principal: &Principal, operation: OperationName) -> Self {
        Self {
            kind: EventKind::PermissionDenied,
            revision: None,
            item_id: None,
            job_id: None,
            connector_id: None,
            actor: Some(EventActor {
                subject: principal.subject.clone(),
                route: principal.route.clone(),
                client_id: principal.client_id.clone(),
            }),
            operation: Some(operation),
        }
    }
}
