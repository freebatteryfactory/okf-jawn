//! A workspace's event stream opened by a caller who does not hold `admin` on it, over the real
//! application service and real storage: its security events never reach that caller
//! (contract ruling 21). The stream polls `list_events`, so the rule is the operation's; this
//! proves the composed route applies it. A page may then hold fewer events than it read, or
//! none, and carry a cursor past them: the stream resumes there once the page is delivered and
//! polls again at once, so a run of hidden events longer than a page never stalls it, whether
//! the run is at the start of the log, after a delivered event, or arrives while it waits.
#![cfg(feature = "runtime")]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::StreamExt as _;
use okf_jawn_contract::access::{AccessRoute, Connector, IssuedConnector, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::{JobId, MutationId, Revision, TenantId};
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
use okf_jawn_server::events::{self, EventStreams, PAGE_LIMIT};
use okf_jawn_storage::Storage;
use serde_json::json;
use tower::ServiceExt;

use check::{TestResult, some};
use support::{HOST, LocalApp, OWNER};

/// A data directory of its own, removed when dropped.
struct Scratch(PathBuf);

/// An open stream's body, read event by event.
struct Reader {
    frames: axum::body::BodyDataStream,
    /// Text received and not yet split into whole events.
    partial: String,
}

/// The job queue: nothing in this test starts a job.
struct NoQueue;

/// The converter: nothing in this test converts.
struct NoConverter;

/// The connector's bearer secret, registered with the identity middleware.
const STREAM_SECRET: &str = "stream-connector-secret";

/// A poll interval no test waits out: an event that arrives within a test's bound arrived by a
/// poll made at once.
const NEVER: Duration = Duration::from_secs(3600);

/// How long a test waits for the events it expects.
const WITHIN: Duration = Duration::from_secs(10);

/// Numbers each scratch directory of this process: two tests may start within one clock tick.
static SCRATCH: AtomicU64 = AtomicU64::new(0);

impl Scratch {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!(
            "okf-jawn-security-events-{}-{nanos}-{}",
            std::process::id(),
            SCRATCH.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Reader {
    fn new(response: axum::response::Response) -> Self {
        Self {
            frames: response.into_body().into_data_stream(),
            partial: String::new(),
        }
    }

    /// Read until `changes` more `changed` events arrived, giving up after `within`; every
    /// event read, as (SSE id, kind), in order.
    async fn until_changes(
        &mut self,
        changes: usize,
        within: Duration,
    ) -> Result<Vec<(String, String)>, String> {
        let mut read: Vec<(String, String)> = Vec::new();
        let reading = async {
            while read.iter().filter(|(_, kind)| kind == "changed").count() < changes {
                match self.frames.next().await {
                    Some(Ok(chunk)) => self.partial.push_str(&String::from_utf8_lossy(&chunk)),
                    Some(Err(error)) => return Err(error.to_string()),
                    None => return Err("the stream ended".to_owned()),
                }
                while let Some(end) = self.partial.find(
                    "

",
                ) {
                    let block: String = self.partial.drain(..end.saturating_add(2)).collect();
                    if let Some(event) = sse_event(&block)? {
                        read.push(event);
                    }
                }
            }
            Ok(())
        };
        let outcome = tokio::time::timeout(within, reading).await;
        match outcome {
            Ok(Ok(())) => Ok(read),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(format!(
                "no {changes} changes within {within:?}; read {read:?}"
            )),
        }
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
        backups: Arc::new(storage.backups()),
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

/// A workspace of `storage` and a read-only connector to it, which holds `read` there and never
/// `admin`; and the workspace's log.
async fn workspace_and_connector(
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
        tenant_id: tenant,
        workspace_id: created.id,
    });
    Ok((created, issued, scope))
}

/// Append to `scope`'s log, in order, a refusal of someone else for each `true` of `denied`
/// and a change at `head` for each `false`; the ids of the changes, in order.
async fn append(
    storage: &Storage,
    scope: &EventScope,
    head: &Revision,
    denied: impl IntoIterator<Item = bool>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let refused = Principal {
        subject: "someone".to_owned(),
        tenant_id: TenantId::local(),
        route: AccessRoute::McpDelegation,
        client_id: Some("other".to_owned()),
        delegation: None,
    };
    let mut changes = Vec::new();
    for denied in denied {
        let event = if denied {
            NewEvent::permission_denied(&refused, OperationName::CreateItem)
        } else {
            NewEvent {
                kind: EventKind::Changed,
                revision: Some(head.clone()),
                item_id: None,
                job_id: None,
                connector_id: None,
                actor: None,
                operation: None,
                transport: None,
                proposal_id: None,
                count: None,
            }
        };
        let appended = storage.events().append(scope, None, event).await?;
        if !denied {
            changes.push(appended.id);
        }
    }
    Ok(changes)
}

/// What the first test reads: the workspace and connector, and a log of a refusal, a change, a
/// refusal and a change.
async fn seed(
    storage: &Storage,
) -> Result<(Workspace, IssuedConnector, EventScope), Box<dyn std::error::Error>> {
    let (created, issued, scope) = workspace_and_connector(storage).await?;
    append(storage, &scope, &created.head, [true, false, true, false]).await?;
    Ok((created, issued, scope))
}

/// `count` refusals in a row.
fn hidden(count: u16) -> impl Iterator<Item = bool> {
    std::iter::repeat_n(true, usize::from(count))
}

/// One SSE event of `block` as (id, kind), or none for a block that carries no data (a
/// keep-alive comment).
fn sse_event(block: &str) -> Result<Option<(String, String)>, String> {
    let mut id = None;
    let mut data = None;
    for line in block.lines() {
        if let Some(value) = line.strip_prefix("id:") {
            id = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("data:") {
            data = Some(value.trim().to_owned());
        }
    }
    let Some(data) = data else {
        return Ok(None);
    };
    let event: serde_json::Value =
        serde_json::from_str(&data).map_err(|error| error.to_string())?;
    let kind = event
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("an event without a kind: {data}"))?;
    Ok(Some((
        id.ok_or_else(|| format!("an event without an id: {data}"))?,
        kind.to_owned(),
    )))
}

/// Open the workspace's stream as the connector.
async fn open(
    service: axum::Router,
    created: &Workspace,
) -> Result<axum::response::Response, Box<dyn std::error::Error>> {
    let response = service
        .oneshot(
            Request::get(format!("/api/workspaces/{}/events", created.id.0))
                .header("host", HOST)
                .header("authorization", format!("Bearer {STREAM_SECRET}"))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(response)
}

/// The local service with the event stream route, over the application service and `storage`,
/// polling every `interval`; its identity middleware authenticates `STREAM_SECRET` as
/// `connector`.
fn stream_service(
    storage: &Storage,
    connector: &Connector,
    interval: Duration,
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
    let streams = EventStreams::new(bound.clone(), identity.clone(), interval)?;
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
    let response = stream_service(&storage, &issued.connector, Duration::from_millis(20))?
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

/// A run of hidden events longer than a page at the very start of the log: the first poll's
/// page shows nothing and carries a cursor past it, and the stream resumes there at once (the
/// interval is an hour), so the change after the run arrives and no refusal does.
#[tokio::test]
async fn a_stream_crosses_a_hidden_run_longer_than_a_page_at_the_start_of_the_log() -> TestResult {
    let scratch = Scratch::new()?;
    let storage = Storage::open(&scratch.0)?;
    let (created, issued, scope) = workspace_and_connector(&storage).await?;
    let run = PAGE_LIMIT.saturating_mul(2).saturating_add(1);
    let changes = append(&storage, &scope, &created.head, hidden(run).chain([false])).await?;
    let response = open(
        stream_service(&storage, &issued.connector, NEVER)?,
        &created,
    )
    .await?;
    let read = Reader::new(response).until_changes(1, WITHIN).await?;
    let ids: Vec<&str> = read.iter().map(|(id, _)| id.as_str()).collect();
    let expected: Vec<&str> = changes.iter().map(String::as_str).collect();
    assert_eq!(ids, expected, "{read:?}");
    Ok(())
}

/// A change, then a hidden run longer than a page, then a change: the first page delivers the
/// first change and carries a cursor past the hidden events it read; the cursor, not the
/// delivered change's id, is where the next poll resumes, so each change arrives once.
#[tokio::test]
async fn a_stream_delivers_a_page_s_changes_once_and_resumes_past_its_hidden_tail() -> TestResult {
    let scratch = Scratch::new()?;
    let storage = Storage::open(&scratch.0)?;
    let (created, issued, scope) = workspace_and_connector(&storage).await?;
    let run = PAGE_LIMIT.saturating_add(50);
    let changes = append(
        &storage,
        &scope,
        &created.head,
        [false].into_iter().chain(hidden(run)).chain([false]),
    )
    .await?;
    let response = open(
        stream_service(&storage, &issued.connector, NEVER)?,
        &created,
    )
    .await?;
    let read = Reader::new(response).until_changes(2, WITHIN).await?;
    let ids: Vec<&str> = read.iter().map(|(id, _)| id.as_str()).collect();
    let expected: Vec<&str> = changes.iter().map(String::as_str).collect();
    assert_eq!(ids, expected, "each change once, no refusal: {read:?}");
    Ok(())
}

/// A stream that is caught up, then a hidden run longer than a page and a change arriving while
/// it waits between polls: the polls that follow the wait cross the run, so the change arrives.
#[tokio::test]
async fn a_stream_crosses_a_hidden_run_that_arrives_while_it_waits() -> TestResult {
    let scratch = Scratch::new()?;
    let storage = Storage::open(&scratch.0)?;
    let (created, issued, scope) = workspace_and_connector(&storage).await?;
    let first = append(&storage, &scope, &created.head, [false]).await?;
    let response = open(
        stream_service(&storage, &issued.connector, Duration::from_millis(20))?,
        &created,
    )
    .await?;
    let mut reader = Reader::new(response);
    let read = reader.until_changes(1, WITHIN).await?;
    assert_eq!(
        read,
        vec![(
            some(first.first(), "the first change")?.clone(),
            "changed".to_owned()
        )]
    );
    let run = PAGE_LIMIT.saturating_add(1);
    let later = append(&storage, &scope, &created.head, hidden(run).chain([false])).await?;
    let read = reader.until_changes(1, WITHIN).await?;
    let ids: Vec<&str> = read.iter().map(|(id, _)| id.as_str()).collect();
    let expected: Vec<&str> = later.iter().map(String::as_str).collect();
    assert_eq!(ids, expected, "{read:?}");
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
mod support;
