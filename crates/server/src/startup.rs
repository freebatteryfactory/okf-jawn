//! Startup: the service composed from the real adapters, served on its two listeners until a
//! shutdown signal, then stopped in order. Startup wires dependencies and holds no operation
//! behaviour (SPEC section 2; crates/server/AGENTS.md).
//!
//! In order (server-m2-design.md section 1.3), each step refusing startup with the error that
//! names what is wrong, and nothing after it touched:
//! 1. every setting (`settings`), and the three files: the UI build, the MCP App bundle, the
//!    conversion child; hosted, jsonwebtoken's crypto provider is installed before anything else
//!    in the process can use jsonwebtoken;
//! 2. the outbound HTTP client and, hosted, the one WorkOS client (`outbound`);
//! 3. the application listener, then the sandbox listener (a port in use refuses startup, so a
//!    second server started on the same port ends here), and the origins a port-0 bind leaves
//!    open (`Settings::resolve`);
//! 4. the data directory, opened for this process alone (`offline::open_storage`: the lock, the
//!    refusal of newer data, the pre-migration backup with its stderr line, the migrations): the
//!    loser of two starts on one data directory ends here;
//! 5. the converter, the ingest handler and the job runtime, whose first sweep reconciles what a
//!    previous process left unfinished;
//! 6. `ApplicationService`, and every binding over the same ports: one access control (hosted:
//!    `WorkosAccess` behind the grant cache), one mutation ledger, one event log (behind the
//!    security-event allowance) for dispatch, MCP, the identity middleware, the session routes,
//!    the handler and the runtime;
//! 7. the application origin's service and the sandbox origin's; locally, the first launch link.
//!
//! Then the listening lines are printed and both listeners are served. Connections that arrive
//! while startup runs wait in the listen backlog. Liveness (`/healthz`) is answered once the
//! service is composed; readiness is the `get_readiness` operation. A long pre-migration backup
//! can outlast the CLI relay's 45-second wait (a stated limit, server-m2 rulings Q13).
//!
//! Shutdown, when `stop` fires: both listeners stop accepting and every open event stream ends;
//! requests in flight get [`SHUTDOWN_GRACE`]; the security-event aggregates are written; the job
//! runtime stops (a running job is abandoned under its lease, for the next process); the launch
//! link file is removed; every store is dropped, which releases the data directory's lock.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::application::{ApplicationConfig, ApplicationService, Ports};
use okf_jawn_core::conformance::OkfConformance;
use okf_jawn_core::conversion::{Converter, ConverterLimits};
use okf_jawn_core::credentials::CredentialStore;
use okf_jawn_core::events::EventLog;
use okf_jawn_core::jobs::{JobHandler, RecordStore};
use okf_jawn_core::mutations::MutationStore;
use okf_jawn_ingest::converter::{ConverterConfig, DEFAULT_WINDOW_PAGES, DoclingConverter};
use okf_jawn_ingest::handler::{DRAIN_BOUND, HandlerLimits, HandlerPorts, IngestHandler};
use okf_jawn_ingest::runtime::{JobRuntime, RuntimeConfig};
use okf_jawn_mcp::{DEFAULT_MAX_REQUEST_BODY_BYTES, McpConfig, McpPorts, streamable_http_service};
use okf_jawn_storage::Storage;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use workos::transport::SharedTransport;

use crate::BoundApplication;
use crate::allowance::SecurityEvents;
use crate::app::{ServiceParts, service};
use crate::authkit::WorkosLogin;
use crate::config::{AuthConfig, AuthKind, HostedConfig};
use crate::connect::{ConnectTokens, IssuerKeys, install_crypto};
use crate::cookies::CrateCookies;
use crate::events::{EventStreams, POLL_INTERVAL};
use crate::grants::{SystemClock, select_access};
use crate::identity::{BearerAuth, Identity, IdentityPorts};
use crate::ingress::{SignInLimit, guard};
use crate::launch::LaunchLink;
use crate::local::{LAUNCH_TOKEN_LIFETIME, LaunchTokens};
use crate::offline::open_storage;
use crate::outbound;
use crate::public::RESOURCE_METADATA_PATH;
use crate::sandbox::SandboxParts;
use crate::sessions::{OsSecrets, SessionEntry, SessionParts, Sessions};
use crate::settings::{Places, Resolved, Settings};
use crate::workos_access::{ACCESS_TIME_LIMIT, WorkosAccess};

/// The composed service, ready to serve.
pub struct Composition {
    /// The application origin's service.
    pub app: Router,
    /// The sandbox origin's service.
    pub sandbox: Router,
    /// The job runtime, running since composition.
    pub runtime: JobRuntime,
    /// The one event log's allowance, whose aggregates `run` writes.
    pub security: Arc<SecurityEvents>,
    /// Locally, the launch link: its first link printed, then kept published.
    pub launch: Option<LaunchLink>,
    /// Cancelled at shutdown: ends every open event stream.
    pub streams_stop: CancellationToken,
}

/// Fallible outbound clients, built before listeners or the data directory are touched.
struct Clients {
    transport: SharedTransport,
    hosted: Option<(HostedConfig, Arc<workos::Client>)>,
}

/// How long a local browser session lasts (owner, 2026-10-18).
pub const SESSION_LIFETIME_LOCAL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How long a hosted browser session lasts (owner, 2026-10-18).
pub const SESSION_LIFETIME_HOSTED: Duration = Duration::from_secs(12 * 60 * 60);

/// How long a minted sandbox capability resolves.
pub const SANDBOX_TTL: Duration = Duration::from_secs(300);

/// How often a converting job renews its lease: a fifth of storage's 300-second lease.
pub const HEARTBEAT: Duration = Duration::from_secs(60);

/// The time budget of one conversion window.
pub const WINDOW_TIMEOUT: Duration = Duration::from_secs(300);

/// Jobs run at once.
pub const WORKERS: usize = 2;

/// How often leases are expired and pending jobs re-delivered: a third of the lease, so
/// `DRAIN_BOUND` holds.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(100);

/// How long requests in flight are given at shutdown.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Run the service with the settings `lookup` reads and this machine's `places`, until `stop`.
/// Each stdout line goes through `print` (`okf-jawn-server: <key> <value>`).
///
/// # Errors
/// Returns the first refusal of startup (a setting, a file, a listener, the data directory), or
/// a listener's failure while serving.
pub async fn serve(
    lookup: impl Fn(&str) -> Option<String>,
    places: &Places,
    print: &(dyn Fn(&str) + Send + Sync),
    stop: CancellationToken,
) -> Result<(), ApiError> {
    let settings = Settings::from_lookup(lookup, places)?;
    serve_settings(settings, print, stop).await
}

/// Serve settings already read and refused by the process-environment boundary.
pub(crate) async fn serve_settings(
    settings: Settings,
    print: &(dyn Fn(&str) + Send + Sync),
    stop: CancellationToken,
) -> Result<(), ApiError> {
    settings.check_files()?;
    let clients = clients(&settings)?;
    let app_listener = bind("OKF_BIND", settings.bind).await?;
    let sandbox_listener = bind("OKF_SANDBOX_BIND", settings.sandbox_bind).await?;
    let resolved = settings.resolve(
        address("OKF_BIND", &app_listener)?,
        address("OKF_SANDBOX_BIND", &sandbox_listener)?,
    )?;
    let storage = open_storage(&settings.data_dir).await?;
    let composition = compose_with_clients(&settings, &resolved, &storage, clients)?;
    let Composition {
        app,
        sandbox,
        runtime,
        security,
        launch,
        streams_stop,
    } = composition;
    print(&format!(
        "okf-jawn-server: listening {}",
        resolved.public_origin
    ));
    print(&format!(
        "okf-jawn-server: sandbox {}",
        resolved.sandbox_origin
    ));
    // Each is stopped only once nothing is served any more, so nothing a late request writes
    // is lost.
    let after = CancellationToken::new();
    let keeping = match launch {
        Some(link) => {
            let first = link.publish_first()?;
            print(&format!("okf-jawn-server: launch {first}"));
            Some(tokio::spawn(link.keep(after.clone())))
        }
        None => None,
    };
    let writing = tokio::spawn(Arc::clone(&security).run(after.clone()));
    let served = serve_until(
        app_listener,
        app,
        sandbox_listener,
        sandbox,
        &stop,
        &streams_stop,
    )
    .await;
    // In order: the streams ended and the listeners stopped (above); the aggregates, the jobs,
    // the launch file; then every store drops, releasing the lock.
    after.cancel();
    let _written = writing.await;
    runtime.shutdown().await;
    if let Some(keeping) = keeping {
        let _kept = keeping.await;
    }
    drop(storage);
    served
}

/// Compose the service from `storage`'s stores, the converter and the job runtime, and every
/// binding over the same ports. Call from within a Tokio runtime: the job runtime starts here.
///
/// # Errors
/// Returns the refusal of a part: the outbound client, the WorkOS client, `ApplicationService`'s
/// configuration check, the Connect verifier, the event streams, the MCP service, the
/// application or the sandbox service.
pub fn compose(
    settings: &Settings,
    resolved: &Resolved,
    storage: &Storage,
) -> Result<Composition, ApiError> {
    compose_with_clients(settings, resolved, storage, clients(settings)?)
}

/// Validate the JWT provider and build both clients without opening storage or doing network IO.
fn clients(settings: &Settings) -> Result<Clients, ApiError> {
    if matches!(settings.auth, AuthConfig::Hosted(_)) {
        // Before anything in the process can use jsonwebtoken (server M1 re-review 3, N8).
        install_crypto()?;
    }
    let transport = outbound::transport(outbound::client()?);
    let hosted = match &settings.auth {
        AuthConfig::Hosted(config) => Some((
            config.clone(),
            outbound::workos_client(config, transport.clone())?,
        )),
        AuthConfig::Local => None,
    };
    Ok(Clients { transport, hosted })
}

/// Compose with the exact preflighted clients; the one transport is shared by every consumer.
fn compose_with_clients(
    settings: &Settings,
    resolved: &Resolved,
    storage: &Storage,
    clients: Clients,
) -> Result<Composition, ApiError> {
    let Clients { transport, hosted } = clients;
    let security = Arc::new(SecurityEvents::new(
        Arc::new(storage.events()),
        Arc::new(SystemClock),
    ));
    let events: Arc<dyn EventLog> = security.clone();
    let records: Arc<dyn RecordStore> = Arc::new(storage.records());
    let access: Arc<dyn AccessControl> = match &hosted {
        Some((_, client)) => select_access(
            AuthKind::Hosted,
            Arc::new(WorkosAccess::new(
                client.clone(),
                Arc::new(storage.catalog()),
                ACCESS_TIME_LIMIT,
            )),
            Arc::new(SystemClock),
        ),
        None => Arc::new(storage.access()),
    };
    let mutations: Arc<dyn MutationStore> = Arc::new(storage.mutations());
    let converter: Arc<dyn Converter> = Arc::new(DoclingConverter::new(ConverterConfig {
        child: settings.converter.clone(),
        limits: ConverterLimits {
            window_pages: DEFAULT_WINDOW_PAGES,
            memory_limit_bytes: settings.converter_memory_limit_bytes,
        },
    }));
    let handler: Arc<dyn JobHandler> = Arc::new(IngestHandler::new(HandlerPorts {
        records: records.clone(),
        versions: Arc::new(storage.versions()),
        search: Arc::new(storage.search()),
        events: events.clone(),
        backups: Arc::new(storage.backups()),
        purger: Arc::new(storage.purger()),
        uploads: Arc::new(storage.uploads()),
        blobs: Arc::new(storage.blobs()),
        converter: converter.clone(),
        check: Arc::new(OkfConformance),
        limits: HandlerLimits {
            heartbeat: HEARTBEAT,
            window_timeout: WINDOW_TIMEOUT,
            drain: DRAIN_BOUND,
        },
    }));
    let (runtime, queue) = JobRuntime::start(
        &records,
        &events,
        &handler,
        RuntimeConfig {
            workers: WORKERS,
            sweep_interval: SWEEP_INTERVAL,
        },
    );
    let credentials: Arc<dyn CredentialStore> = Arc::new(storage.credentials());
    let application = Arc::new(ApplicationService::new(
        Ports {
            access: access.clone(),
            mutations: mutations.clone(),
            catalog: Arc::new(storage.catalog()),
            blobs: Arc::new(storage.blobs()),
            versions: Arc::new(storage.versions()),
            records,
            queue: Arc::new(queue),
            search: Arc::new(storage.search()),
            converter,
            proposals: Arc::new(storage.proposals()),
            uploads: Arc::new(storage.uploads()),
            events: events.clone(),
            credentials: credentials.clone(),
            drafts: Arc::new(storage.drafts()),
            confirmations: Arc::new(storage.confirmations()),
            sandbox: Arc::new(storage.sandbox()),
            readiness: Arc::new(storage.readiness()),
            backups: Arc::new(storage.backups()),
        },
        ApplicationConfig::new(resolved.sandbox_origin.clone(), SANDBOX_TTL),
    )?);
    let bound = BoundApplication {
        application: application.clone(),
        access: access.clone(),
        mutations: mutations.clone(),
        events: events.clone(),
    };
    let bearer = match &hosted {
        Some((config, _)) => BearerAuth::Hosted {
            connect: Arc::new(ConnectTokens::new(
                Arc::new(IssuerKeys::new(transport, &config.issuer)),
                config,
                Arc::new(SystemClock),
            )?),
            resource_metadata: format!("{}{RESOURCE_METADATA_PATH}", resolved.public_origin),
        },
        None => BearerAuth::Local,
    };
    let identity = Identity::new(
        bearer,
        IdentityPorts {
            credentials: credentials.clone(),
            events: events.clone(),
            cookies: Arc::new(CrateCookies),
            refusals: security.clone(),
        },
        vec![resolved.public_origin.clone()],
    );
    let tokens = Arc::new(LaunchTokens::new(
        LAUNCH_TOKEN_LIFETIME,
        Arc::new(SystemClock),
    ));
    let (entry, lifetime) = match &hosted {
        Some((config, client)) => (
            SessionEntry::Hosted {
                login: Arc::new(WorkosLogin::new(client.clone(), config)),
            },
            SESSION_LIFETIME_HOSTED,
        ),
        None => (
            SessionEntry::Local {
                tokens: tokens.clone(),
            },
            SESSION_LIFETIME_LOCAL,
        ),
    };
    let sessions = crate::sessions::routes(Sessions::new(SessionParts {
        credentials,
        events: events.clone(),
        refusals: security.clone(),
        secrets: Arc::new(OsSecrets),
        policy: identity.cookie_policy(lifetime),
        entry,
    }));
    let sessions = match &hosted {
        Some(_) => guard(
            Arc::new(SignInLimit::new(
                settings.trusted_proxies.clone(),
                Arc::new(SystemClock),
            )),
            sessions,
        ),
        None => sessions,
    };
    let streams_stop = CancellationToken::new();
    let streams = EventStreams::new(bound.clone(), identity.clone(), POLL_INTERVAL)?
        .with_stop(streams_stop.clone());
    let mcp = streamable_http_service(
        McpPorts {
            application: application.clone(),
            access,
            mutations,
            events,
        },
        McpConfig {
            app_bundle: settings.mcp_app.clone(),
            allowed_hosts: resolved.hosts.clone(),
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
        },
    )?;
    let extra = sessions
        .merge(crate::events::routes(streams))
        .merge(crate::schemas::routes())
        .merge(crate::transfers::routes(application))
        .merge(crate::mcp::routes(mcp));
    let app = service(ServiceParts {
        bound,
        identity,
        hosts: resolved.hosts.clone(),
        hosted: hosted.as_ref().map(|(config, _)| config.clone()),
        cors_origins: Vec::new(),
        ui_dir: Some(settings.ui_dir.clone()),
        sandbox_origin: Some(resolved.sandbox_origin.clone()),
        extra,
    })?;
    let sandbox = crate::sandbox::service(SandboxParts {
        capabilities: Arc::new(storage.sandbox()),
        blobs: Arc::new(storage.blobs()),
        host: resolved.sandbox_host.clone(),
        app_origin: resolved.public_origin.clone(),
    })?;
    let launch = hosted.is_none().then(|| {
        LaunchLink::new(
            tokens,
            Arc::new(OsSecrets),
            &resolved.public_origin,
            &settings.data_dir,
        )
    });
    Ok(Composition {
        app,
        sandbox,
        runtime,
        security,
        launch,
        streams_stop,
    })
}

/// Serve both services until `stop` (or until one fails); then end the event streams, give the
/// requests in flight [`SHUTDOWN_GRACE`], and drop what is still open.
async fn serve_until(
    app_listener: TcpListener,
    app: Router,
    sandbox_listener: TcpListener,
    sandbox: Router,
    stop: &CancellationToken,
    streams_stop: &CancellationToken,
) -> Result<(), ApiError> {
    let app_stop = stop.clone();
    let mut app_task = tokio::spawn(async move {
        axum::serve(
            app_listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(app_stop.cancelled_owned())
        .await
    });
    let sandbox_stop = stop.clone();
    let mut sandbox_task = tokio::spawn(async move {
        axum::serve(sandbox_listener, sandbox)
            .with_graceful_shutdown(sandbox_stop.cancelled_owned())
            .await
    });
    // Which server ended first, if one did before `stop`: a task already awaited to its end is
    // never awaited again.
    let (failed, app_running, sandbox_running) = tokio::select! {
        () = stop.cancelled() => (None, true, true),
        ended = &mut app_task => (Some(ended), false, true),
        ended = &mut sandbox_task => (Some(ended), true, false),
    };
    // Whatever ended serving, everything stops now.
    stop.cancel();
    streams_stop.cancel();
    let (app_abort, sandbox_abort) = (app_task.abort_handle(), sandbox_task.abort_handle());
    let drained = tokio::time::timeout(SHUTDOWN_GRACE, async {
        if app_running {
            let _app = app_task.await;
        }
        if sandbox_running {
            let _sandbox = sandbox_task.await;
        }
    })
    .await;
    if drained.is_err() {
        app_abort.abort();
        sandbox_abort.abort();
    }
    match failed {
        None | Some(Ok(Ok(()))) => Ok(()),
        Some(Ok(Err(error))) => Err(ApiError::new(
            ErrorCode::Unavailable,
            format!("a listener failed while serving ({:?})", error.kind()),
        )),
        Some(Err(_)) => Err(ApiError::new(
            ErrorCode::Internal,
            "a listener's task stopped",
        )),
    }
}

/// A listener on `address`, the setting `name` names.
async fn bind(name: &str, address: SocketAddr) -> Result<TcpListener, ApiError> {
    TcpListener::bind(address).await.map_err(|error| {
        ApiError::new(
            ErrorCode::InvalidInput,
            format!(
                "{name} {address} cannot be listened on ({:?})",
                error.kind()
            ),
        )
        .with_field(name)
    })
}

/// The address `listener` was bound to.
fn address(name: &str, listener: &TcpListener) -> Result<SocketAddr, ApiError> {
    listener.local_addr().map_err(|_| {
        ApiError::new(
            ErrorCode::Internal,
            format!("the address {name} was bound to cannot be read"),
        )
    })
}
