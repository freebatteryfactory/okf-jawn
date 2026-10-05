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

use crate::access::AccessControl;
use crate::confirmations::ConfirmationStore;
use crate::conversion::Converter;
use crate::credentials::CredentialStore;
use crate::drafts::DraftStore;
use crate::events::EventLog;
use crate::jobs::{JobQueue, RecordStore};
use crate::mutations::MutationStore;
use crate::proposals::ProposalStore;
use crate::readiness::ReadinessProbe;
use crate::sandbox::SandboxCapabilityStore;
use crate::search::SearchIndex;
use crate::storage::{BlobStore, VersionStore, WorkspaceCatalog};
use crate::uploads::UploadStore;

/// The complete set of adapters the application service coordinates.
///
/// `JobHandler` is intentionally not injected here: ingest owns the Tokio worker
/// that claims leases from `records`/`queue` and executes handlers separately.
#[derive(Clone)]
pub struct Ports {
    /// Workspace and tenant grant resolution.
    pub access: Arc<dyn AccessControl>,
    /// Idempotency ledger.
    pub mutations: Arc<dyn MutationStore>,
    /// Workspace metadata and permissions.
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
    /// Suggested change sets and discussion.
    pub proposals: Arc<dyn ProposalStore>,
    /// Authenticated upload occurrence slots.
    pub uploads: Arc<dyn UploadStore>,
    /// Resumable workspace notifications.
    pub events: Arc<dyn EventLog>,
    /// Connectors, sessions, and installation identity.
    pub credentials: Arc<dyn CredentialStore>,
    /// Per-editor drafts.
    pub drafts: Arc<dyn DraftStore>,
    /// Session-bound confirmation challenges.
    pub confirmations: Arc<dyn ConfirmationStore>,
    /// Sandbox-origin capability tokens.
    pub sandbox: Arc<dyn SandboxCapabilityStore>,
    /// Configured dependency readiness.
    pub readiness: Arc<dyn ReadinessProbe>,
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
