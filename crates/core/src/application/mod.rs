//! Production application service: operation behavior composed over injected core-owned ports.
//!
//! Core never depends on a concrete storage, ingestion, or transport crate. Server startup
//! constructs the real adapters and passes them in through `Ports`; startup wires dependencies
//! and does not hold application behavior. The `ports::Application` implementation for
//! `ApplicationService` is construction work for the core lane, organized as cohesive child
//! modules of this one (workspaces, items, reading, search, history, proposals, reviews,
//! imports, conventions, attention, views, events, session). Until it exists, nothing in this
//! crate claims those operations work, and no fixture or in-memory store may stand in for it.

use std::sync::Arc;

use crate::conversion::Converter;
use crate::jobs::{JobQueue, RecordStore};
use crate::search::SearchIndex;
use crate::storage::{BlobStore, VersionStore, WorkspaceCatalog};

/// The complete set of adapters the application service coordinates.
#[derive(Clone)]
pub struct Ports {
    /// Workspace metadata, permissions, and storage scope.
    pub catalog: Arc<dyn WorkspaceCatalog>,
    /// Immutable content-addressed bytes.
    pub blobs: Arc<dyn BlobStore>,
    /// Git-versioned notes and source cards.
    pub versions: Arc<dyn VersionStore>,
    /// Durable, non-rebuildable jobs, reviews, and receipts.
    pub records: Arc<dyn RecordStore>,
    /// Wake-up delivery for jobs already persisted in `records`.
    pub queue: Arc<dyn JobQueue>,
    /// Rebuildable full-text, link, and graph data.
    pub search: Arc<dyn SearchIndex>,
    /// Bounded document conversion.
    pub converter: Arc<dyn Converter>,
}

/// Coordinates authorization, revision resolution, and port calls for every declared operation.
pub struct ApplicationService {
    ports: Ports,
}

impl ApplicationService {
    /// Compose the service from adapters constructed by server startup.
    #[must_use]
    pub fn new(ports: Ports) -> Self {
        Self { ports }
    }

    /// The injected adapters, for operation modules within this service.
    #[must_use]
    pub fn ports(&self) -> &Ports {
        &self.ports
    }
}
