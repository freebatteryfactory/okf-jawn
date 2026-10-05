//! Resumable workspace notifications are projections, not canonical document content.

use okf_jawn_contract::events::{Event, ListEventsRequest, ListEventsResponse};

use crate::ports::PortFuture;
use crate::storage::StorageScope;

/// Append-only event log for live notifications; storage owns the implementation.
pub trait EventLog: Send + Sync {
    /// Append one notification after the durable change it projects.
    fn append<'a>(&'a self, scope: &'a StorageScope, event: Event) -> PortFuture<'a, Event>;
    /// Read notifications after an opaque cursor with bounded pagination.
    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        request: ListEventsRequest,
    ) -> PortFuture<'a, ListEventsResponse>;
}
