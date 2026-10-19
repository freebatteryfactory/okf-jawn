//! Resumable notifications are projections, not canonical document content.
//!
//! A notification is a hint to read again; it is appended after the durable change it projects.
//! The store allocates the monotonic cursor and stamps `at`. Workspace events (content, imports,
//! jobs, reviews, proposals, a refusal inside a workspace) and tenant events (sign-ins,
//! connector issue and revocation, a refusal outside a workspace) are separate logs
//! (`EventScope`); a security event names its actor.
//!
//! A sign-in is recorded in one place, `record_sign_in`: the server's session routes call it
//! with their log, and `ApplicationService::record_sign_in` with the service's (Stage 1b design
//! section 7).

use okf_jawn_contract::access::Principal;
use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::events::{Event, EventActor, EventKind, ListEventsResponse};
use okf_jawn_contract::identity::{
    ConnectorId, ItemId, JobId, MutationId, ProposalId, Revision, TenantId,
};
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
    /// The refused byte transport of a `permission_denied` event; `operation` is then absent.
    pub transport: Option<okf_jawn_contract::transport::TransportName>,
    /// Affected proposal.
    pub proposal_id: Option<ProposalId>,
    /// How many occurrences this one event stands for; `None` means one (a `sign_in_refused`
    /// aggregate past its source's allowance).
    pub count: Option<u32>,
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
    /// not appended again and the prior event is returned; two that differ in any field, the
    /// proposal or the count included, are both appended. Without one, as for job progress, the
    /// notification is always appended.
    ///
    /// An append to `EventScope::Workspace` for a workspace the tenant does not hold (never
    /// created, or purged) records the event at tenant level instead. It never creates or
    /// revives rows for that workspace, so a refusal that names any workspace id cannot write
    /// there.
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
            operation: Some(operation),
            ..Self::security(EventKind::PermissionDenied, principal)
        }
    }

    /// A `permission_denied` event for a refused byte transport: `principal` was refused
    /// `transport`, which is not an operation (`Event::transport`).
    #[must_use]
    pub fn transport_denied(
        principal: &Principal,
        transport: okf_jawn_contract::transport::TransportName,
    ) -> Self {
        Self {
            transport: Some(transport),
            ..Self::security(EventKind::PermissionDenied, principal)
        }
    }

    /// A `signed_in` event: `principal` began a browser session (tenant-level).
    #[must_use]
    pub fn signed_in(principal: &Principal) -> Self {
        Self::security(EventKind::SignedIn, principal)
    }

    /// A `connector_issued` event: `principal` issued `connector` (tenant-level).
    #[must_use]
    pub fn connector_issued(principal: &Principal, connector: ConnectorId) -> Self {
        Self {
            connector_id: Some(connector),
            ..Self::security(EventKind::ConnectorIssued, principal)
        }
    }

    /// A `connector_revoked` event: `principal` revoked `connector` (tenant-level).
    #[must_use]
    pub fn connector_revoked(principal: &Principal, connector: ConnectorId) -> Self {
        Self {
            connector_id: Some(connector),
            ..Self::security(EventKind::ConnectorRevoked, principal)
        }
    }

    /// A security event of `kind` naming `principal` as its actor, and nothing else.
    fn security(kind: EventKind, principal: &Principal) -> Self {
        Self {
            kind,
            revision: None,
            item_id: None,
            job_id: None,
            connector_id: None,
            actor: Some(EventActor {
                subject: principal.subject.clone(),
                route: principal.route.clone(),
                client_id: principal.client_id.clone(),
            }),
            operation: None,
            transport: None,
            proposal_id: None,
            count: None,
        }
    }
}

/// Append `signed_in` for `principal` to its tenant's log: the one rule for recording a
/// sign-in.
///
/// It is appended without a mutation identity, so every sign-in is its own event. A caller that
/// opens a session refuses and undoes it when this fails (the server's session routes do).
///
/// # Errors
/// Returns the log's error.
pub async fn record_sign_in(events: &dyn EventLog, principal: &Principal) -> Result<(), ApiError> {
    events
        .append(
            &EventScope::Tenant(principal.tenant_id.clone()),
            None,
            NewEvent::signed_in(principal),
        )
        .await
        .map(|_| ())
}
