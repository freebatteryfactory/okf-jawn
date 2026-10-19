//! Byte transfers through the production `ApplicationService` over Git, SQLite, local blobs and
//! local authorization. Expected upload/object bytes are literal fixtures; backup bytes are
//! compared with the archive opened independently through the storage port. No conversion or
//! worker runs here: jobs are recorded only to retain their real backup artifacts.
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::io::Cursor;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use check::{TestResult, err_of, some};
use okf_jawn_contract::access::{AccessRoute, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::{
    Digest, ItemId, JobId, LOCAL_TENANT_ID, MutationId, Revision, TenantId, WorkspacePath,
};
use okf_jawn_contract::transport::TransportName;
use okf_jawn_core::application::{ApplicationConfig, ApplicationService, Ports};
use okf_jawn_core::conformance::OkfConformance;
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, Converter, ConverterLimits, PageCount,
};
use okf_jawn_core::credentials::CredentialStore;
use okf_jawn_core::events::{EventLog, EventQuery, EventScope};
use okf_jawn_core::jobs::{
    ArtifactKind, ArtifactRecord, JobQueue, JobScope, JobSpec, NewArtifact, NewJob, RecordStore,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    Backups, BlobStore, CommitChanges, LocalSource, NewWorkspace, Page, Provenance, SourceCard,
    StorageScope, TreeEdit, VersionStore, WorkspaceCatalog,
};
use okf_jawn_core::transfers::{ByteRange, Download, TransferCaller, Transfers};
use okf_jawn_core::uploads::{NewUpload, UploadStore};
use okf_jawn_storage::Storage;
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt as _, ReadBuf};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// Production adapters over one isolated data directory and its local owner.
struct World {
    storage: Storage,
    service: ApplicationService,
    caller: TransferCaller,
    scope: StorageScope,
    head: Revision,
}

/// No worker or converter is part of a byte transfer.
struct Unused;

/// A reader which returns small chunks, counting only bytes actually polled by storage.
struct Counted {
    remaining: usize,
    taken: Arc<AtomicUsize>,
}

const CONTENT: &[u8] = b"0123456789";
const READ_CHUNK: usize = 7;
const LIMIT: u64 = 20;

impl AsyncRead for Counted {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let count = self.remaining.min(READ_CHUNK).min(buf.remaining());
        for _ in 0..count {
            buf.put_slice(b"x");
        }
        // `count` is at most `remaining`, by the minimum above.
        self.remaining = self.remaining.saturating_sub(count);
        self.taken.fetch_add(count, Ordering::SeqCst);
        Poll::Ready(Ok(()))
    }
}

impl JobQueue for Unused {
    fn enqueue(&self, _scope: JobScope, _job: JobId) -> PortFuture<'_, ()> {
        Box::pin(async { Err(internal("no worker runs in transfer tests")) })
    }
}

impl Converter for Unused {
    fn page_count<'a>(
        &'a self,
        _source: &'a LocalSource,
        _name: &'a str,
    ) -> PortFuture<'a, PageCount> {
        Box::pin(async { Err(internal("no conversion runs in transfer tests")) })
    }
    fn convert(&self, _input: ConversionInput) -> PortFuture<'_, Conversion> {
        Box::pin(async { Err(internal("no conversion runs in transfer tests")) })
    }
    fn limits(&self) -> ConverterLimits {
        ConverterLimits {
            window_pages: 1,
            memory_limit_bytes: 1,
        }
    }
}

impl World {
    async fn new(path: &std::path::Path) -> Fallible<Self> {
        let storage = Storage::open(path)?;
        let identity = storage.credentials().installation_identity().await?;
        let principal = Principal {
            subject: identity.subject,
            tenant_id: TenantId::try_from(LOCAL_TENANT_ID.to_owned())?,
            route: AccessRoute::LocalOwner,
            client_id: None,
            delegation: None,
        };
        let created = storage
            .catalog()
            .create(
                &principal.tenant_id,
                mutation(1)?,
                NewWorkspace {
                    name: "Transfers".to_owned(),
                    description: String::new(),
                    creator: provenance(&principal),
                },
            )
            .await?;
        let scope = StorageScope {
            tenant_id: principal.tenant_id.clone(),
            workspace_id: created.id,
        };
        let ports = Ports {
            access: Arc::new(storage.access()),
            mutations: Arc::new(storage.mutations()),
            catalog: Arc::new(storage.catalog()),
            blobs: Arc::new(storage.blobs()),
            versions: Arc::new(storage.versions()),
            records: Arc::new(storage.records()),
            queue: Arc::new(Unused),
            search: Arc::new(storage.search()),
            converter: Arc::new(Unused),
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
        let mut config = ApplicationConfig::new(
            "https://sandbox.example.test".to_owned(),
            Duration::from_secs(300),
        );
        config.max_upload_bytes = LIMIT;
        Ok(Self {
            storage,
            service: ApplicationService::new(ports, config)?,
            caller: TransferCaller {
                principal,
                session_id: None,
            },
            scope,
            head: created.head,
        })
    }

    async fn slot(&self, n: u32, size: u64) -> Fallible<okf_jawn_core::uploads::UploadRecord> {
        Ok(self
            .storage
            .uploads()
            .create(
                &self.scope,
                mutation(n)?,
                NewUpload {
                    filename: "source.txt".to_owned(),
                    relative_path: "incoming".to_owned(),
                    expected_size: size,
                    expected_sha256: None,
                    supplied_by: provenance(&self.caller.principal),
                },
            )
            .await?)
    }

    async fn archive(&self, kind: ArtifactKind) -> Fallible<(ArtifactRecord, Vec<u8>)> {
        let backups = self.storage.backups();
        let (scope, spec, object, read) = if kind == ArtifactKind::WorkspaceBackup {
            let object = backups
                .write_workspace_archive(&self.scope, mutation(30)?)
                .await?
                .object;
            let read = self
                .storage
                .blobs()
                .open(&self.scope, &object.digest, 0, object.size)
                .await?;
            (
                JobScope::Workspace(self.scope.clone()),
                JobSpec::BackupWorkspace,
                object,
                read,
            )
        } else {
            let object = backups
                .write_installation_archive(&self.scope.tenant_id, mutation(30)?)
                .await?;
            let read = backups
                .open_installation_archive(&self.scope.tenant_id, &object.digest, 0, object.size)
                .await?;
            (
                JobScope::Tenant(self.scope.tenant_id.clone()),
                JobSpec::BackupInstallation,
                object,
                read,
            )
        };
        let mut bytes = Vec::new();
        let mut body = read.body;
        body.read_to_end(&mut bytes).await?;
        let job = self
            .storage
            .records()
            .create_job(
                &scope,
                NewJob {
                    mutation_id: mutation(31)?,
                    initiator: provenance(&self.caller.principal),
                    spec,
                },
            )
            .await?;
        let record = self
            .storage
            .records()
            .record_artifact(
                &scope,
                mutation(32)?,
                NewArtifact {
                    kind,
                    object,
                    media_type: "application/zip".to_owned(),
                    created_by_job: job.id,
                },
            )
            .await?;
        Ok((record, bytes))
    }
}

fn internal(message: &str) -> ApiError {
    ApiError::new(ErrorCode::Internal, message)
}

fn mutation(n: u32) -> Fallible<MutationId> {
    Ok(serde_json::from_value(json!(format!(
        "00000000-0000-4000-9000-{n:012}"
    )))?)
}

fn provenance(principal: &Principal) -> Provenance {
    Provenance {
        subject: principal.subject.clone(),
        route: principal.route.clone(),
        client_id: principal.client_id.clone(),
    }
}

async fn bytes(
    mut download: Download,
    expected: &[u8],
    media: &str,
    total: u64,
    offset: u64,
) -> TestResult {
    assert_eq!(download.media_type, media);
    assert_eq!(download.total_size, total);
    assert_eq!(download.offset, offset);
    assert_eq!(download.length, u64::try_from(expected.len())?);
    let mut received = Vec::new();
    download.body.read_to_end(&mut received).await?;
    assert_eq!(received, expected);
    Ok(())
}

/// T3/T7: the returned bytes and digest are the ten literal bytes, not an expected value made
/// by the transfer under test. Completion checks this published SHA-256 for "0123456789".
#[tokio::test]
async fn streamed_upload_completes_against_the_real_store() -> TestResult {
    let directory = tempfile::tempdir()?;
    let world = World::new(directory.path()).await?;
    let slot = world.slot(2, 10).await?;
    let answer = world
        .service
        .upload_content(
            &world.caller,
            world.scope.workspace_id,
            slot.id,
            Box::pin(Cursor::new(CONTENT)),
        )
        .await?;
    assert_eq!(answer.received_bytes, "10");
    let expected = Digest::try_from(
        "84d89877f0d4041efb6bf91a16f0248f2fd573e6af05c19f96bedb9f882f7882".to_owned(),
    )?;
    let complete = world
        .storage
        .uploads()
        .complete(&world.scope, slot.id, expected.clone())
        .await?;
    let held = some(complete.object, "the retained completed object")?;
    assert_eq!(held.digest, expected);
    assert_eq!(held.size, 10);
    let read = world
        .storage
        .blobs()
        .open(&world.scope, &held.digest, 0, 10)
        .await?;
    let mut received = Vec::new();
    let mut body = read.body;
    body.read_to_end(&mut received).await?;
    assert_eq!(received, CONTENT);
    Ok(())
}

/// T3/T7 and server review N-r: a one-million-byte body must stop within one seven-byte
/// reader chunk of the configured limit; the refused append keeps the previous three bytes.
#[tokio::test]
async fn over_limit_upload_stops_reading_and_appends_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let world = World::new(directory.path()).await?;
    let slot = world.slot(2, 1_000_003).await?;
    world
        .service
        .upload_content(
            &world.caller,
            world.scope.workspace_id,
            slot.id,
            Box::pin(Cursor::new(b"old")),
        )
        .await?;
    let taken = Arc::new(AtomicUsize::new(0));
    let reader = Counted {
        remaining: 1_000_000,
        taken: taken.clone(),
    };
    let refused = err_of(
        world
            .service
            .upload_content(
                &world.caller,
                world.scope.workspace_id,
                slot.id,
                Box::pin(reader),
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::TooLarge);
    let read = taken.load(Ordering::SeqCst);
    assert!(read > usize::try_from(LIMIT - 3)?);
    assert!(
        read <= usize::try_from(LIMIT - 3)?.saturating_add(READ_CHUNK),
        "read {read} bytes"
    );
    let held = world.storage.uploads().get(&world.scope, slot.id).await?;
    assert_eq!(held.received_bytes, 3);
    assert!(held.object.is_none());
    let source = world
        .storage
        .data()
        .root()
        .join("uploads")
        .join(format!("{}.part", slot.id.0));
    assert_eq!(std::fs::read(source)?, b"old");
    Ok(())
}

/// T4/T7: the exact committed source occurrence, authorized at its Git revision, yields the
/// literal original whole and the inclusive range 2..5 as "2345".
#[tokio::test]
async fn committed_original_downloads_whole_and_as_a_range() -> TestResult {
    let directory = tempfile::tempdir()?;
    let world = World::new(directory.path()).await?;
    let object = world
        .storage
        .blobs()
        .put(&world.scope, Box::pin(Cursor::new(CONTENT)), 10, None)
        .await?;
    let item: ItemId = serde_json::from_value(json!("00000000-0000-4000-9000-000000000010"))?;
    let card = SourceCard {
        item_id: item,
        path: WorkspacePath::try_from("sources/source.md".to_owned())?,
        title: "source.txt".to_owned(),
        type_name: "source".to_owned(),
        body: String::new(),
        properties: BTreeMap::new(),
        appearance: serde_json::from_value(json!({
            "object": object.digest, "names": [{ "filename": "source.txt", "folder": "incoming", "observed_at": "2026-10-19T00:00:00.000Z", "supplied_by": world.caller.principal.subject }],
            "media_type": "text/plain", "size": "10", "metadata": {},
            "extraction": { "outcome": { "status": "unsupported" }, "text_origin": "none", "corrected": false, "warnings": [] }
        }))?,
    };
    let committed = world
        .storage
        .versions()
        .commit(
            &world.scope,
            CommitChanges {
                mutation_id: mutation(3)?,
                expected_head: world.head.clone(),
                author: provenance(&world.caller.principal),
                message: "Retain source".to_owned(),
                edits: vec![TreeEdit::WriteSourceCard(Box::new(card))],
            },
            Arc::new(OkfConformance),
        )
        .await?;
    let whole = world
        .service
        .download_object(
            &world.caller,
            world.scope.workspace_id,
            item,
            committed.revision.clone(),
            object.digest.clone(),
            None,
        )
        .await?;
    bytes(whole, CONTENT, "text/plain", 10, 0).await?;
    let range = world
        .service
        .download_object(
            &world.caller,
            world.scope.workspace_id,
            item,
            committed.revision,
            object.digest,
            Some(ByteRange {
                start: 2,
                end: Some(5),
            }),
        )
        .await?;
    bytes(range, b"2345", "text/plain", 10, 2).await?;
    Ok(())
}

/// T5/T6/T7 and SPEC §12: real workspace and installation backups are returned only to the
/// human administrator. A connector, even naming the same owner, receives a recorded denial.
#[tokio::test]
async fn backup_artifacts_download_to_the_human_admin_and_refuse_the_connector() -> TestResult {
    for kind in [
        ArtifactKind::WorkspaceBackup,
        ArtifactKind::InstallationBackup,
    ] {
        let directory = tempfile::tempdir()?;
        let world = World::new(directory.path()).await?;
        let (record, expected) = world.archive(kind).await?;
        assert!(expected.starts_with(b"PK"));
        let mut connector = world.caller.clone();
        connector.principal.route = AccessRoute::McpDelegation;
        let (download, refused, scope, transport) = match kind {
            ArtifactKind::WorkspaceBackup => (
                world
                    .service
                    .download_artifact(&world.caller, world.scope.workspace_id, record.id)
                    .await?,
                world
                    .service
                    .download_artifact(&connector, world.scope.workspace_id, record.id)
                    .await,
                EventScope::Workspace(world.scope.clone()),
                TransportName::DownloadArtifact,
            ),
            _ => (
                world
                    .service
                    .download_tenant_artifact(&world.caller, record.id)
                    .await?,
                world
                    .service
                    .download_tenant_artifact(&connector, record.id)
                    .await,
                EventScope::Tenant(world.scope.tenant_id.clone()),
                TransportName::DownloadTenantArtifact,
            ),
        };
        bytes(
            download,
            &expected,
            "application/zip",
            record.object.size,
            0,
        )
        .await?;
        assert_eq!(err_of(refused.map(drop))?.code, ErrorCode::Forbidden);
        let events = world
            .storage
            .events()
            .list(
                &scope,
                EventQuery {
                    after: None,
                    page: Page {
                        limit: 50,
                        cursor: None,
                    },
                },
            )
            .await?;
        let denied: Vec<_> = events
            .events
            .iter()
            .filter(|event| event.kind == EventKind::PermissionDenied)
            .collect();
        assert_eq!(denied.len(), 1);
        let event = some(denied.first(), "one recorded denial")?;
        assert_eq!(event.transport, Some(transport));
        assert!(event.operation.is_none());
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
