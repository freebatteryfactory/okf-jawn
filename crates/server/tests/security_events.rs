//! A workspace's event stream opened by a caller who does not hold `admin` on it, over the real
//! application service and real storage: its security events never reach that caller
//! (contract ruling 21). The stream polls `list_events`, so the rule is the operation's; this
//! proves the composed route applies it.
#![cfg(feature = "runtime")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::StreamExt as _;
use okf_jawn_contract::access::{AccessRoute, Connector, IssuedConnector, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::{JobId, MutationId, TenantId};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::workspace::Workspace;
use okf_jawn_core::application::{ApplicationConfig, ApplicationService, Ports};
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, Converter, ConverterLimits, PageCount,
};
use okf_jawn_core::credentials::{ConnectorIssue, CredentialStore, NewConnector};
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
use okf_jawn_core::jobs::{JobQueue, JobScope};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    LocalSource, NewWorkspace, Page, Provenance, StorageScope, WorkspaceCatalog,
};
use okf_jawn_server::BoundApplication;
use okf_jawn_server::app::{ServiceParts, service};
use okf_jawn_server::events::{self, EventStreams};
use okf_jawn_storage::Storage;
use serde_json::json;
use tower::ServiceExt;

use check::TestResult;
use support::{HOST, LocalApp, OWNER};

/// A data directory of its own, removed when dropped.
struct Scratch(PathBuf);

/// The job queue: nothing in this test starts a job.
struct NoQueue;

/// The converter: nothing in this test converts.
struct NoConverter;

/// The connector's bearer secret, registered with the identity middleware.
const STREAM_SECRET: &str = "stream-connector-secret";

impl Scratch {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!(
            "okf-jawn-security-events-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a leftover directory under the temporary directory harms nothing.
        let _left = std::fs::remove_dir_all(&self.0);
    }
}

impl JobQueue for NoQueue {
    fn enqueue(&self, _scope: JobScope, _job: JobId) -> PortFuture<'_, ()> {
        Box::pin(async { Err(unused()) })
    }
}

impl Converter for NoConverter {
    fn page_count<'a>(
        &'a self,
        _source: &'a LocalSource,
        _file_name: &'a str,
    ) -> PortFuture<'a, PageCount> {
        Box::pin(async { Err(unused()) })
    }

    fn convert(&self, _input: ConversionInput) -> PortFuture<'_, Conversion> {
        Box::pin(async { Err(unused()) })
    }

    fn limits(&self) -> ConverterLimits {
        ConverterLimits {
            window_pages: 1,
            memory_limit_bytes: 1,
        }
    }
}

fn unused() -> ApiError {
    ApiError::new(ErrorCode::Internal, "not used by this test")
}

fn mutation(n: u32) -> Result<MutationId, serde_json::Error> {
    serde_json::from_value(json!(format!("00000000-0000-4000-8000-{n:012}")))
}

/// The application service over every store of `storage`.
fn application(storage: &Storage) -> Result<ApplicationService, ApiError> {
    let ports = Ports {
        access: Arc::new(storage.access()),
        mutations: Arc::new(storage.mutations()),
        catalog: Arc::new(storage.catalog()),
        blobs: Arc::new(storage.blobs()),
        versions: Arc::new(storage.versions()),
        records: Arc::new(storage.records()),
        queue: Arc::new(NoQueue),
        search: Arc::new(storage.search()),
        converter: Arc::new(NoConverter),
        proposals: Arc::new(storage.proposals()),
        uploads: Arc::new(storage.uploads()),
        events: Arc::new(storage.events()),
        credentials: Arc::new(storage.credentials()),
        drafts: Arc::new(storage.drafts()),
        confirmations: Arc::new(storage.confirmations()),
        sandbox: Arc::new(storage.sandbox()),
        readiness: Arc::new(storage.readiness()),
    };
    ApplicationService::new(
        ports,
        ApplicationConfig::new("http://127.0.0.1:8788".to_owned(), Duration::from_secs(60)),
    )
}

/// Read the stream until `events` events were seen, giving up after ten seconds.
async fn read(response: axum::response::Response, events: usize) -> Result<String, String> {
    let mut frames = response.into_body().into_data_stream();
    let mut seen = String::new();
    let reading = async {
        while seen.matches("\"kind\"").count() < events {
            match frames.next().await {
                Some(Ok(chunk)) => seen.push_str(&String::from_utf8_lossy(&chunk)),
                Some(Err(error)) => return Err(error.to_string()),
                None => return Err("the stream ended".to_owned()),
            }
        }
        Ok(())
    };
    tokio::time::timeout(Duration::from_secs(10), reading)
        .await
        .map_err(|_| format!("no {events} events in ten seconds; got {seen}"))??;
    Ok(seen)
}

/// What the test reads: a workspace of `storage` whose log is a refusal, a change, a refusal
/// and a change, and a read-only connector to it, which holds `read` there and never `admin`.
async fn seed(
    storage: &Storage,
) -> Result<(Workspace, IssuedConnector, EventScope), Box<dyn std::error::Error>> {
    let tenant = TenantId::local();
    let created = storage
        .catalog()
        .create(
            &tenant,
            mutation(1)?,
            NewWorkspace {
                name: "Shared".to_owned(),
                description: String::new(),
                creator: Provenance {
                    subject: OWNER.to_owned(),
                    route: AccessRoute::LocalOwner,
                    client_id: None,
                },
            },
        )
        .await?;
    let ConnectorIssue::Issued(issued) = storage
        .credentials()
        .create_connector(
            mutation(2)?,
            NewConnector {
                label: "agent".to_owned(),
                workspace_ids: vec![created.id],
                allow_propose: false,
            },
        )
        .await?
    else {
        return Err("a new connector is issued".into());
    };
    let scope = EventScope::Workspace(StorageScope {
        tenant_id: tenant.clone(),
        workspace_id: created.id,
    });
    let refused = Principal {
        subject: "someone".to_owned(),
        tenant_id: tenant,
        route: AccessRoute::McpDelegation,
        client_id: Some("other".to_owned()),
        delegation: None,
    };
    for denied in [true, false, true, false] {
        let event = if denied {
            NewEvent::permission_denied(&refused, OperationName::CreateItem)
        } else {
            NewEvent {
                kind: EventKind::Changed,
                revision: Some(created.head.clone()),
                item_id: None,
                job_id: None,
                connector_id: None,
                actor: None,
                operation: None,
                proposal_id: None,
                count: None,
            }
        };
        storage.events().append(&scope, None, event).await?;
    }
    Ok((created, issued, scope))
}

/// The local service with the event stream route, over the application service and `storage`;
/// its identity middleware authenticates `STREAM_SECRET` as `connector`.
fn stream_service(
    storage: &Storage,
    connector: &Connector,
) -> Result<axum::Router, Box<dyn std::error::Error>> {
    let base = LocalApp::new()?;
    base.credentials.add_connector(
        STREAM_SECRET,
        &connector.connector_id.0.to_string(),
        connector.workspace_ids.clone(),
        vec![Permission::Read],
        false,
    )?;
    let bound = BoundApplication {
        application: Arc::new(application(storage)?),
        access: Arc::new(storage.access()),
        mutations: Arc::new(storage.mutations()),
        events: Arc::new(storage.events()),
    };
    let identity = base.identity();
    let streams = EventStreams::new(bound.clone(), identity.clone(), Duration::from_millis(20))?;
    Ok(service(ServiceParts {
        bound,
        identity,
        hosts: vec![HOST.to_owned()],
        hosted: None,
        cors_origins: Vec::new(),
        ui_dir: None,
        extra: events::routes(streams),
    })?)
}

#[tokio::test]
async fn a_stream_opened_by_a_caller_without_admin_never_carries_a_security_event() -> TestResult {
    let scratch = Scratch::new()?;
    let storage = Storage::open(&scratch.0)?;
    let (created, issued, scope) = seed(&storage).await?;
    let response = stream_service(&storage, &issued.connector)?
        .oneshot(
            Request::get(format!("/api/workspaces/{}/events", created.id.0))
                .header("host", HOST)
                .header("authorization", format!("Bearer {STREAM_SECRET}"))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let seen = read(response, 2).await?;
    assert_eq!(seen.matches("\"kind\":\"changed\"").count(), 2, "{seen}");
    assert!(!seen.contains("permission_denied"), "{seen}");
    // The refusals are in the log: an administrator would read them.
    let whole = storage
        .events()
        .list(
            &scope,
            EventQuery {
                after: None,
                page: Page {
                    cursor: None,
                    limit: 10,
                },
            },
        )
        .await?;
    let refusals = whole
        .events
        .iter()
        .filter(|event| event.kind == EventKind::PermissionDenied)
        .count();
    assert_eq!(refusals, 2);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
mod support;
