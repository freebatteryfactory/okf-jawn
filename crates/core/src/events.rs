//! Resumable workspace notifications are projections, not canonical document content.
//!
//! Comment and event inserts take `MutationId` and enforce uniqueness.

use okf_jawn_contract::events::{Event, ListEventsRequest, ListEventsResponse};
use okf_jawn_contract::identity::MutationId;

use crate::ports::PortFuture;
use crate::storage::StorageScope;

/// Append-only event log for live notifications; storage owns the implementation.
pub trait EventLog: Send + Sync {
    /// Append one notification after the durable change it projects.
    ///
    /// Unique on `mutation_id` when the event is the creating effect of a write.
    fn append<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        event: Event,
    ) -> PortFuture<'a, Event>;
    /// Look up an event created under `mutation_id`, for abandoned-lease reconciliation.
    fn find_by_mutation<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
    ) -> PortFuture<'a, Option<Event>>;
    /// Read notifications after an opaque cursor with bounded pagination.
    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        request: ListEventsRequest,
    ) -> PortFuture<'a, ListEventsResponse>;
}
