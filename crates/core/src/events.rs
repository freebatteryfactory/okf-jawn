//! Resumable workspace notifications are projections, not canonical document content.
//!
//! A notification is a hint to read again; it is appended after the durable change it projects.
//! The store allocates the monotonic cursor.

use okf_jawn_contract::events::{Event, EventKind, ListEventsResponse};
use okf_jawn_contract::identity::{ItemId, JobId, MutationId, Revision};

use crate::ports::PortFuture;
use crate::storage::{Page, StorageScope};

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
        scope: &'a StorageScope,
        mutation_id: Option<MutationId>,
        event: NewEvent,
    ) -> PortFuture<'a, Event>;
    /// Read notifications after a cursor, oldest first, with bounded pagination.
    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: EventQuery,
    ) -> PortFuture<'a, ListEventsResponse>;
}
