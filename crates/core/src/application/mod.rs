//! Production application service: operation behavior composed over injected core-owned ports.
//!
//! Core never depends on a concrete storage, ingestion, or transport crate. Server startup
//! constructs the real adapters and passes them in through `Ports`, with the deployment's
//! `ApplicationConfig`; startup wires dependencies and does not hold application behavior. The
//! `ports::Application` implementation for `ApplicationService` is organized as cohesive child
//! modules of this one: `items` (folders, items and types), `drafts`, `reads` (`read_item` and
//! sandbox capabilities), `sources` (citations and object reads), `history` (Timeline, Changes,
//! Who, Snapshot and Rewind), `navigation` (search, links and the graph), `proposals`
//! (proposals, their acceptance and discussion), `reviews` (confirmations and reviews),
//! `conventions` (the naming rules: reading and saving them, a rename preview and its apply),
//! `status` (health, readiness and the session), `purges` (recording, resuming and reading
//! a workspace purge), `workspaces` (the catalog, archive, export, backup and restore),
//! `events` (receipts and the two event logs), `connectors` (local connector credentials),
//! `uploads` (upload slots) and `job_control` (reading jobs, retrying and cancelling them),
//! over the shared steps in `shared` (revision resolution, commits, receipts). Besides the
//! operations, the service answers dispatch's archive question (`Application::archived_at`)
//! from its catalog and records a sign-in for the server's session routes
//! (`ApplicationService::record_sign_in`).
//!
//! Operations whose part of the construction plan has not landed answer `NotImplemented`,
//! naming the operation; nothing here claims an operation works that is not built, and no
//! fixture or in-memory store stands in for one.

use std::collections::BTreeMap;
use std::future::Ready;
use std::net::Ipv6Addr;
use std::str::FromStr as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use okf_jawn_contract::access::Principal;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::ConversionSettings;
use okf_jawn_contract::identity::{TenantId, Timestamp, WorkspaceId};
use okf_jawn_contract::import::DEFAULT_MAX_UPLOAD_BYTES;
use time::OffsetDateTime;

use crate::access::AccessControl;
use crate::confirmations::ConfirmationStore;
use crate::conformance::OkfConformance;
use crate::context::OperationContext;
use crate::conversion::Converter;
use crate::credentials::CredentialStore;
use crate::drafts::DraftStore;
use crate::events::{EventLog, record_sign_in};
use crate::jobs::{JobQueue, RecordStore};
use crate::mutations::MutationStore;
use crate::ports::{Application, PortFuture};
use crate::proposals::ProposalStore;
use crate::readiness::ReadinessProbe;
use crate::sandbox::SandboxCapabilityStore;
use crate::search::SearchIndex;
use crate::storage::{BlobStore, CandidateCheck, StorageScope, VersionStore, WorkspaceCatalog};
use crate::uploads::UploadStore;

pub use reviews::CONFIRMATION_TTL;
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
    (log_items) => {
        history::log_items
    };
    (diff_items) => {
        history::diff_items
    };
    (commit_items) => {
        history::commit_items
    };
    (restore_items) => {
        history::restore_items
    };
    (blame_item) => {
        history::blame_item
    };
    (search_items) => {
        navigation::search_items
    };
    (get_links) => {
        navigation::get_links
    };
    (get_graph) => {
        navigation::get_graph
    };
    (get_health) => {
        status::get_health
    };
    (get_readiness) => {
        status::get_readiness
    };
    (get_session) => {
        status::get_session
    };
    (list_workspaces) => {
        workspaces::list_workspaces
    };
    (create_workspace) => {
        workspaces::create_workspace
    };
    (open_workspace) => {
        workspaces::open_workspace
    };
    (update_workspace) => {
        workspaces::update_workspace
    };
    (archive_workspace) => {
        workspaces::archive_workspace
    };
    (unarchive_workspace) => {
        workspaces::unarchive_workspace
    };
    (export_workspace) => {
        workspaces::export_workspace
    };
    (backup_workspace) => {
        workspaces::backup_workspace
    };
    (restore_workspace) => {
        workspaces::restore_workspace
    };
    (backup_installation) => {
        workspaces::backup_installation
    };
    (get_receipt) => {
        events::get_receipt
    };
    (list_events) => {
        events::list_events
    };
    (list_tenant_events) => {
        events::list_tenant_events
    };
    (create_connector) => {
        connectors::create_connector
    };
    (list_connectors) => {
        connectors::list_connectors
    };
    (revoke_connector) => {
        connectors::revoke_connector
    };
    (purge_workspace) => {
        purges::purge_workspace
    };
    (get_purge) => {
        purges::get_purge
    };
    (open_proposal) => {
        proposals::open_proposal
    };
    (list_proposals) => {
        proposals::list_proposals
    };
    (get_proposal) => {
        proposals::get_proposal
    };
    (accept_proposal) => {
        proposals::accept_proposal
    };
    (decline_proposal) => {
        proposals::decline_proposal
    };
    (add_comment) => {
        proposals::add_comment
    };
    (list_comments) => {
        proposals::list_comments
    };
    (create_confirmation) => {
        reviews::create_confirmation
    };
    (create_review) => {
        reviews::create_review
    };
    (list_reviews) => {
        reviews::list_reviews
    };
    (get_rules) => {
        conventions::get_rules
    };
    (set_rules) => {
        conventions::set_rules
    };
    (preview_names) => {
        conventions::preview_names
    };
    (apply_names) => {
        conventions::apply_names
    };
    (create_upload) => {
        uploads::create_upload
    };
    (complete_upload) => {
        uploads::complete_upload
    };
    (get_upload) => {
        uploads::get_upload
    };
    (list_uploads) => {
        uploads::list_uploads
    };
    (get_job) => {
        job_control::get_job
    };
    (list_jobs) => {
        job_control::list_jobs
    };
    (get_tenant_job) => {
        job_control::get_tenant_job
    };
    (list_tenant_jobs) => {
        job_control::list_tenant_jobs
    };
    (retry_job) => {
        job_control::retry_job
    };
    (cancel_job) => {
        job_control::cancel_job
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

            fn archived_at<'a>(&'a self, scope: &'a StorageScope)
                -> PortFuture<'a, Option<Timestamp>> {
                Box::pin(workspaces::archived_at(self, scope))
            }
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
    /// The most bytes of one upload; above zero. `ApplicationConfig::new` sets the contract's
    /// `import::DEFAULT_MAX_UPLOAD_BYTES`, and a deployment may set another. `get_session`
    /// reports it (`Limits::upload_bytes`).
    pub max_upload_bytes: u64,
    /// Conversion settings resolved into an import job when the request supplies none.
    /// `ApplicationConfig::new` uses the contract's `ConversionSettings::default`.
    pub conversion_defaults: ConversionSettings,
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
    index_builds: IndexBuilds,
}

/// One gate per workspace that an on-demand index build holds (`navigation`), so a workspace
/// builds at most one revision's index at a time and a request that waited finds the build done.
type IndexBuilds = Mutex<BTreeMap<(TenantId, WorkspaceId), Arc<tokio::sync::Mutex<()>>>>;

impl ApplicationConfig {
    /// The settings of a deployment with the default upload limit
    /// (`import::DEFAULT_MAX_UPLOAD_BYTES`) and contract conversion settings.
    #[must_use]
    pub fn new(sandbox_origin: String, sandbox_ttl: Duration) -> Self {
        Self {
            sandbox_origin,
            sandbox_ttl,
            max_upload_bytes: DEFAULT_MAX_UPLOAD_BYTES,
            conversion_defaults: ConversionSettings::default(),
        }
    }

    /// Refuse settings no deployment may run with.
    ///
    /// # Errors
    /// Returns `InvalidInput` naming `sandbox_origin` when it is not an absolute `http` or
    /// `https` origin of a host and an optional port in `1..=65535`, without a path, query,
    /// fragment or user information, naming `sandbox_ttl` when it is zero, and naming
    /// `max_upload_bytes` when it is zero.
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
        if self.max_upload_bytes == 0 {
            return Err(ApiError::new(
                ErrorCode::InvalidInput,
                "an upload must be allowed some bytes",
            )
            .with_field("max_upload_bytes"));
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
            index_builds: Mutex::new(BTreeMap::new()),
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

    /// The gate an on-demand index build of `scope`'s workspace holds while it builds.
    ///
    /// # Errors
    /// Returns `Internal` when the gate map's lock is poisoned.
    pub(super) fn index_gate(
        &self,
        scope: &StorageScope,
    ) -> Result<Arc<tokio::sync::Mutex<()>>, ApiError> {
        let mut gates = self.index_builds.lock().map_err(|_| {
            ApiError::new(ErrorCode::Internal, "the index build gates are poisoned")
        })?;
        Ok(Arc::clone(
            gates
                .entry((scope.tenant_id.clone(), scope.workspace_id))
                .or_default(),
        ))
    }

    /// The OKF conformance check every commit of this service runs; startup hands the same
    /// check to ingest's handler.
    #[must_use]
    pub fn conformance(&self) -> Arc<dyn CandidateCheck> {
        Arc::clone(&self.conformance)
    }

    /// Append `signed_in` for `principal` to its tenant's log (Stage 1b design section 7), by
    /// the one rule `events::record_sign_in` holds; the server's session routes record a
    /// sign-in by that rule too, before they send the session's cookies.
    ///
    /// # Errors
    /// Returns the event log's error; a caller that opened a session undoes and refuses it.
    pub async fn record_sign_in(&self, principal: &Principal) -> Result<(), ApiError> {
        record_sign_in(self.ports.events.as_ref(), principal).await
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

mod connectors;
mod conventions;
mod drafts;
mod events;
mod history;
mod items;
mod job_control;
mod navigation;
mod proposals;
mod purges;
mod reads;
mod reviews;
mod shared;
mod sources;
mod status;
mod uploads;
mod workspaces;
