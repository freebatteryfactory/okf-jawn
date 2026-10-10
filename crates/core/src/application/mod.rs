//! Production application service: operation behavior composed over injected core-owned ports.
//!
//! Core never depends on a concrete storage, ingestion, or transport crate. Server startup
//! constructs the real adapters and passes them in through `Ports`, with the deployment's
//! `ApplicationConfig`; startup wires dependencies and does not hold application behavior. The
//! `ports::Application` implementation for `ApplicationService` is organized as cohesive child
//! modules of this one: `items` (folders, items and types), `drafts`, `reads` (`read_item` and
//! sandbox capabilities) and `sources` (citations and object reads), over the shared steps in
//! `shared` (revision resolution, commits, receipts).
//!
//! Operations whose part of the construction plan has not landed answer `NotImplemented`,
//! naming the operation; nothing here claims an operation works that is not built, and no
//! fixture or in-memory store stands in for one.

use std::future::Ready;
use std::net::Ipv6Addr;
use std::str::FromStr as _;
use std::sync::Arc;
use std::time::Duration;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use time::OffsetDateTime;

use crate::access::AccessControl;
use crate::confirmations::ConfirmationStore;
use crate::conformance::OkfConformance;
use crate::context::OperationContext;
use crate::conversion::Converter;
use crate::credentials::CredentialStore;
use crate::drafts::DraftStore;
use crate::events::EventLog;
use crate::jobs::{JobQueue, RecordStore};
use crate::mutations::MutationStore;
use crate::ports::{Application, PortFuture};
use crate::proposals::ProposalStore;
use crate::readiness::ReadinessProbe;
use crate::sandbox::SandboxCapabilityStore;
use crate::search::SearchIndex;
use crate::storage::{BlobStore, CandidateCheck, VersionStore, WorkspaceCatalog};
use crate::uploads::UploadStore;

pub use sources::MAX_OBJECT_BLOCK;

/// The handler of one operation: its function in a child module, or `pending` while the
/// operation's construction part has not landed.
macro_rules! route {
    (list_items) => {
        items::list_items
    };
    (get_item) => {
        items::get_item
    };
    (create_item) => {
        items::create_item
    };
    (save_draft) => {
        drafts::save_draft
    };
    (list_drafts) => {
        drafts::list_drafts
    };
    (discard_draft) => {
        drafts::discard_draft
    };
    (move_item) => {
        items::move_item
    };
    (set_lifecycle) => {
        items::set_lifecycle
    };
    (delete_item) => {
        items::delete_item
    };
    (create_folder) => {
        items::create_folder
    };
    (list_types) => {
        items::list_types
    };
    (set_type) => {
        items::set_type
    };
    (read_item) => {
        reads::read_item
    };
    (create_sandbox_capability) => {
        reads::create_sandbox_capability
    };
    (get_sources) => {
        sources::get_sources
    };
    (get_object) => {
        sources::get_object
    };
    ($pending:ident) => {
        pending
    };
}

macro_rules! application_operations {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        impl Application for ApplicationService {
            $(fn $id<'a>(&'a self, context: &'a OperationContext, request: $request)
                -> PortFuture<'a, $response> {
                Box::pin(route!($id)(self, context, request))
            })*
        }
    };
}

/// The complete set of adapters the application service coordinates.
///
/// `JobHandler` is intentionally not injected here: ingest owns the Tokio worker
/// that claims leases from `records`/`queue` and executes handlers separately.
/// `CandidateCheck` is not injected either: it is this crate's own OKF conformance policy
/// (`conformance::OkfConformance`), handed to `versions` with every commit and every proposal
/// candidate.
#[derive(Clone)]
pub struct Ports {
    /// Workspace and tenant grant resolution.
    pub access: Arc<dyn AccessControl>,
    /// Idempotency ledger.
    pub mutations: Arc<dyn MutationStore>,
    /// Workspace metadata keyed by tenant; the caller's permissions come from `access`.
    pub catalog: Arc<dyn WorkspaceCatalog>,
    /// Immutable content-addressed bytes.
    pub blobs: Arc<dyn BlobStore>,
    /// Git-versioned notes and source cards.
    pub versions: Arc<dyn VersionStore>,
    /// Durable, non-rebuildable jobs, reviews, receipts, and artifact records.
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

/// Deployment settings the operations need beyond the ports.
///
/// Checked when the service is built (`ApplicationService::new`). That the sandbox origin
/// differs from the application's own origin is the server's check: core does not know the
/// application origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationConfig {
    /// The sandbox origin hostile representations are served from, such as
    /// `https://sandbox.example.test` or `http://127.0.0.1:8788`: an absolute `http` or `https`
    /// origin, a host and an optional port in `1..=65535`, with no path, query, fragment or user
    /// information.
    pub sandbox_origin: String,
    /// How long a minted sandbox capability resolves; above zero.
    pub sandbox_ttl: Duration,
}

/// Where the service reads the current instant: the system clock unless a test fixes it
/// (`ApplicationService::with_clock`). A sandbox capability expires at this instant plus the
/// configured ttl, and a receipt records it as the time content was returned.
pub type Clock = fn() -> OffsetDateTime;

/// Coordinates authorization, revision resolution, and port calls for every declared operation.
pub struct ApplicationService {
    ports: Ports,
    config: ApplicationConfig,
    conformance: Arc<dyn CandidateCheck>,
    clock: Clock,
}

impl ApplicationConfig {
    /// Refuse settings no deployment may run with.
    ///
    /// # Errors
    /// Returns `InvalidInput` naming `sandbox_origin` when it is not an absolute `http` or
    /// `https` origin of a host and an optional port in `1..=65535`, without a path, query,
    /// fragment or user information, and naming
    /// `sandbox_ttl` when it is zero.
    pub fn check(&self) -> Result<(), ApiError> {
        if !is_origin(&self.sandbox_origin) {
            return Err(ApiError::new(
                ErrorCode::InvalidInput,
                "the sandbox origin must be an absolute http or https origin with no path",
            )
            .with_field("sandbox_origin"));
        }
        if self.sandbox_ttl.is_zero() {
            return Err(ApiError::new(
                ErrorCode::InvalidInput,
                "a sandbox capability must resolve for some time",
            )
            .with_field("sandbox_ttl"));
        }
        Ok(())
    }
}

impl ApplicationService {
    /// Compose the service from adapters and settings constructed by server startup.
    ///
    /// # Errors
    /// Returns the refusal of `ApplicationConfig::check`.
    pub fn new(ports: Ports, config: ApplicationConfig) -> Result<Self, ApiError> {
        config.check()?;
        Ok(Self {
            ports,
            config,
            conformance: Arc::new(OkfConformance),
            clock: OffsetDateTime::now_utc,
        })
    }

    /// The same service reading the current instant from `clock` instead of the system clock.
    #[must_use]
    pub fn with_clock(self, clock: Clock) -> Self {
        Self { clock, ..self }
    }

    /// The current instant, from the service's clock.
    #[must_use]
    pub fn now(&self) -> OffsetDateTime {
        (self.clock)()
    }

    /// The injected adapters, for operation modules within this service.
    #[must_use]
    pub fn ports(&self) -> &Ports {
        &self.ports
    }

    /// The deployment settings the service was built with.
    #[must_use]
    pub fn config(&self) -> &ApplicationConfig {
        &self.config
    }

    /// The OKF conformance check every commit of this service runs; startup hands the same
    /// check to ingest's handler.
    #[must_use]
    pub fn conformance(&self) -> Arc<dyn CandidateCheck> {
        Arc::clone(&self.conformance)
    }
}

okf_jawn_contract::for_each_operation!(application_operations);

/// The answer of an operation whose construction part has not landed: `NotImplemented`,
/// naming it. Never a success.
fn pending<Request, Response>(
    _service: &ApplicationService,
    context: &OperationContext,
    _request: Request,
) -> Ready<Result<Response, ApiError>> {
    std::future::ready(Err(ApiError::new(
        ErrorCode::NotImplemented,
        format!(
            "{} is not implemented in this build",
            context.operation.as_str()
        ),
    )))
}

/// Whether `text` is an origin and nothing else: the scheme `http` or `https`, `://`, a host,
/// and an optional `:port` with a port in `1..=65535`; no path, query, fragment or user
/// information.
///
/// Core does not depend on the workspace's `url` crate, so the check is written out exactly.
/// The host is an IPv6 address in brackets, parsed by `std::net::Ipv6Addr`, or a name or IPv4
/// address of ASCII letters, digits, `-` and `.`.
fn is_origin(text: &str) -> bool {
    let Some(authority) = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
    else {
        return false;
    };
    // A path, a query, a fragment or user information.
    if authority.contains(['/', '?', '#', '@']) {
        return false;
    }
    let (host_ok, port) = if let Some(literal) = authority.strip_prefix('[') {
        let Some((inner, after)) = literal.split_once(']') else {
            return false;
        };
        let port = if after.is_empty() {
            None
        } else if let Some(port) = after.strip_prefix(':') {
            Some(port)
        } else {
            return false;
        };
        (Ipv6Addr::from_str(inner).is_ok(), port)
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        let host_ok = !host.is_empty()
            && host.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '.')
            });
        (host_ok, port)
    };
    let port_ok = port.is_none_or(|port| {
        port.bytes().all(|byte| byte.is_ascii_digit())
            && port.parse::<u16>().is_ok_and(|number| number >= 1)
    });
    host_ok && port_ok
}

mod drafts;
mod items;
mod reads;
mod shared;
mod sources;
