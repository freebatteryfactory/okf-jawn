//! The byte transfers through `ApplicationService` (`transfers::Transfers`) over the tenancy
//! fakes: what each transfer asks of the ports on each route, the interval and bytes it answers,
//! and how it refuses. Every `Forbidden` a transfer answers is recorded as `permission_denied`
//! naming its transport and no operation, in the asked workspace's log; a refusal with any other
//! code is recorded nowhere.

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use okf_jawn_contract::access::AccessRoute::{BrowserSession, LocalOwner, McpDelegation, Service};
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{Digest, ItemId, MutationId, PurgeId, UploadId, WorkspaceId};
use okf_jawn_contract::transport::TransportName;
use okf_jawn_core::application::{ApplicationConfig, ApplicationService};
use okf_jawn_core::events::{EventScope, NewEvent};
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    BlobStore, ByteReader, LocalSource, ObjectInfo, ObjectRead, StorageScope,
};
use okf_jawn_core::transfers::{ByteRange, Download, TransferCaller, Transfers};
use okf_jawn_core::uploads::UploadRecord;
use tokio::io::AsyncReadExt as _;
use uuid::Uuid;

use crate::check::{TestResult, err_of};
use crate::fixture::{
    ALICE, Built, CARD_BODY, WORKSPACE, config, converted, digest, instant, record, revision,
    scope, source_card,
};
use crate::tenancy::{Tenancy, archived_instant, uploader, workspace};

/// What a download said and the bytes its body yielded.
#[derive(Debug, PartialEq, Eq)]
struct Received {
    media_type: String,
    total_size: u64,
    offset: u64,
    length: u64,
    bytes: Vec<u8>,
}

/// A blob store that opens an object from the offset asked but ignores the length: its body runs
/// to the object's end, as a store that took `length` for a hint would.
struct Overlong {
    objects: Mutex<Vec<(Digest, Vec<u8>)>>,
}

/// Every route a principal arrives by.
const ROUTES: [AccessRoute; 4] = [BrowserSession, LocalOwner, Service, McpDelegation];
/// The fixture card.
const CARD: u128 = 10;
/// The bytes of the card's original, `digest('a')`.
const ORIGINAL: &[u8] = b"hello world";
/// Another workspace of the fixture tenant.
const OTHER: u128 = 2;
/// Every permission but `write`.
const ALL_BUT_WRITE: [Permission; 5] = [
    Permission::Read,
    Permission::Propose,
    Permission::Approve,
    Permission::Review,
    Permission::Admin,
];
/// Every permission but `read`.
const ALL_BUT_READ: [Permission; 5] = [
    Permission::Write,
    Permission::Propose,
    Permission::Approve,
    Permission::Review,
    Permission::Admin,
];

impl BlobStore for Overlong {
    fn put<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _body: ByteReader,
        _limit: u64,
        _expected: Option<Digest>,
    ) -> PortFuture<'a, ObjectInfo> {
        Box::pin(async { Err(internal("not used by this test")) })
    }
    fn open<'a>(
        &'a self,
        _scope: &'a StorageScope,
        digest: &'a Digest,
        offset: u64,
        length: u64,
    ) -> PortFuture<'a, ObjectRead> {
        Box::pin(async move {
            let bytes = self
                .objects
                .lock()
                .map_err(|_| internal("the fake's lock is poisoned"))?
                .iter()
                .find(|(held, _)| held == digest)
                .map(|(_, bytes)| bytes.clone())
                .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such object"))?;
            let size = u64::try_from(bytes.len()).map_err(internal)?;
            let start = usize::try_from(offset.min(size)).map_err(internal)?;
            let rest = bytes.get(start..).map(<[u8]>::to_vec).unwrap_or_default();
            Ok(ObjectRead {
                object: ObjectInfo {
                    digest: digest.clone(),
                    size,
                },
                offset,
                length,
                body: Box::pin(Cursor::new(rest)),
            })
        })
    }
    fn materialize<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _digest: &'a Digest,
    ) -> PortFuture<'a, LocalSource> {
        Box::pin(async { Err(internal("not used by this test")) })
    }
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, error.to_string())
}

fn principal(route: AccessRoute) -> Built<Principal> {
    Ok(Principal {
        subject: ALICE.to_owned(),
        tenant_id: scope()?.tenant_id,
        route,
        client_id: None,
        delegation: None,
    })
}

fn caller(route: AccessRoute) -> Built<TransferCaller> {
    Ok(TransferCaller {
        principal: principal(route)?,
        session_id: None,
    })
}

/// The tenancy fakes over `config`, with the fixture workspace live in the catalog and Alice
/// holding the raw `permissions` on it.
fn world_with(config: ApplicationConfig, permissions: &[Permission]) -> Built<Tenancy> {
    let tenancy = Tenancy::with_config(config)?;
    let scope = scope()?;
    tenancy.catalog.put(
        &scope.tenant_id,
        workspace(WORKSPACE, "Fixture", 'a', false)?,
    )?;
    tenancy
        .access
        .grant(ALICE, scope.workspace_id, permissions.to_vec())?;
    Ok(tenancy)
}

fn world(permissions: &[Permission]) -> Built<Tenancy> {
    world_with(config(), permissions)
}

/// The fixture card at the head `revision('a')`: its original `digest('a')` holds `ORIGINAL`
/// and its conversion record is `digest('b')`.
fn with_card(tenancy: &Tenancy) -> Built<()> {
    let at = revision('a')?;
    tenancy.world.versions.put_document(
        &at,
        source_card(
            CARD,
            &at,
            "notes/report-pdf.md",
            CARD_BODY,
            converted(&digest('b')?),
        )?,
    )?;
    tenancy
        .world
        .blobs
        .put_object(digest('b')?, serde_json::to_vec(&record(Vec::new())?)?)?;
    tenancy
        .world
        .blobs
        .put_object(digest('a')?, ORIGINAL.to_vec())?;
    Ok(())
}

/// The log of the fixture workspace.
fn workspace_log() -> Built<EventScope> {
    Ok(EventScope::Workspace(scope()?))
}

/// The one record a refusal of `transport` to Alice on `route` leaves in `log`.
fn refusal(
    route: AccessRoute,
    transport: TransportName,
    log: EventScope,
) -> Built<Vec<(EventScope, Option<MutationId>, NewEvent)>> {
    Ok(vec![(
        log,
        None,
        NewEvent::transport_denied(&principal(route)?, transport),
    )])
}

/// The fixture workspace's neighbour in the tenant.
fn other_scope() -> Built<StorageScope> {
    Ok(StorageScope {
        workspace_id: WorkspaceId(Uuid::from_u128(OTHER)),
        ..scope()?
    })
}

fn body(bytes: &[u8]) -> ByteReader {
    Box::pin(Cursor::new(bytes.to_vec()))
}

/// An empty slot announcing `expected` bytes.
fn slot(expected: u64) -> Built<UploadRecord> {
    Ok(UploadRecord {
        id: UploadId(Uuid::from_u128(30)),
        filename: "report.pdf".to_owned(),
        relative_path: String::new(),
        expected_size: expected,
        expected_sha256: None,
        supplied_by: uploader(),
        created_at: instant()?,
        received_bytes: 0,
        object: None,
        consumed_by: None,
    })
}

/// Put `bytes` into the fixture workspace's slot `upload` as Alice on `route`.
async fn upload(
    tenancy: &Tenancy,
    route: AccessRoute,
    upload: UploadId,
    bytes: &[u8],
) -> Result<(), ApiError> {
    let caller = caller(route).map_err(internal)?;
    let workspace = scope().map_err(internal)?.workspace_id;
    tenancy
        .service
        .upload_content(&caller, workspace, upload, body(bytes))
        .await
        .map(drop)
}

/// Read a download whole.
async fn received(download: Download) -> Built<Received> {
    let mut bytes = Vec::new();
    let mut reader = download.body;
    reader.read_to_end(&mut bytes).await?;
    Ok(Received {
        media_type: download.media_type,
        total_size: download.total_size,
        offset: download.offset,
        length: download.length,
        bytes,
    })
}

fn whole(media_type: &str, bytes: &[u8]) -> Built<Received> {
    let size = u64::try_from(bytes.len())?;
    Ok(Received {
        media_type: media_type.to_owned(),
        total_size: size,
        offset: 0,
        length: size,
        bytes: bytes.to_vec(),
    })
}

fn range(start: u64, end: Option<u64>) -> ByteRange {
    ByteRange { start, end }
}

/// The object `digest` of the fixture card at `revision('a')`, asked by Alice on `route` as
/// `range`.
async fn object(
    service: &ApplicationService,
    route: AccessRoute,
    digest: Digest,
    range: Option<ByteRange>,
) -> Result<Download, ApiError> {
    let caller = caller(route).map_err(internal)?;
    service
        .download_object(
            &caller,
            scope().map_err(internal)?.workspace_id,
            ItemId(Uuid::from_u128(CARD)),
            revision('a').map_err(internal)?,
            digest,
            range,
        )
        .await
}

/// The card's original, asked by Alice on `route` as `range`.
async fn original(
    tenancy: &Tenancy,
    route: AccessRoute,
    range: Option<ByteRange>,
) -> Result<Download, ApiError> {
    object(
        &tenancy.service,
        route,
        digest('a').map_err(internal)?,
        range,
    )
    .await
}

// ---- upload_content ----

#[tokio::test]
async fn upload_content_appends_the_body_under_the_deployments_limit_and_answers_the_slot()
-> TestResult {
    let limit = 64;
    let tenancy = world_with(
        ApplicationConfig {
            max_upload_bytes: limit,
            ..config()
        },
        &[Permission::Write],
    )?;
    let scope = scope()?;
    let held = slot(10)?;
    tenancy.uploads.put(&scope, held.clone())?;
    let answer = tenancy
        .service
        .upload_content(
            &caller(LocalOwner)?,
            scope.workspace_id,
            held.id,
            body(b"0123456789"),
        )
        .await?;
    let stands = tenancy.uploads.held(&scope, held.id)?;
    assert_eq!(stands.received_bytes, 10);
    assert_eq!(
        serde_json::to_value(&answer)?,
        serde_json::to_value(stands.to_wire(scope.workspace_id))?,
        "the slot's wire form"
    );
    assert_eq!(answer.received_bytes, "10");
    assert!(
        !answer.complete,
        "the digest is complete_upload's to verify"
    );
    assert_eq!(
        tenancy.uploads.limits()?,
        vec![limit],
        "the deployment's limit"
    );
    assert_eq!(tenancy.access.lookups()?, vec![Permission::Write]);
    assert!(tenancy.log.appended()?.is_empty(), "no event is recorded");
    Ok(())
}

#[tokio::test]
async fn upload_content_on_each_route_refuses_only_a_connector_and_records_it() -> TestResult {
    for route in ROUTES {
        let tenancy = world(&[Permission::Write])?;
        let scope = scope()?;
        let held = slot(10)?;
        tenancy.uploads.put(&scope, held.clone())?;
        let outcome = upload(&tenancy, route.clone(), held.id, b"0123").await;
        if route == McpDelegation {
            let refused = err_of(outcome)?;
            assert_eq!(refused.code, ErrorCode::Forbidden, "{route:?}");
            assert!(
                tenancy.access.lookups()?.is_empty(),
                "refused before any lookup"
            );
            assert!(
                tenancy.uploads.limits()?.is_empty(),
                "no byte reached the store"
            );
            assert_eq!(
                tenancy.log.appended()?,
                refusal(route, TransportName::UploadContent, workspace_log()?)?
            );
        } else {
            outcome?;
            assert_eq!(
                tenancy.uploads.held(&scope, held.id)?.received_bytes,
                4,
                "{route:?}"
            );
            assert!(tenancy.log.appended()?.is_empty(), "{route:?}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn upload_content_needs_write_on_the_workspace() -> TestResult {
    let tenancy = world(&ALL_BUT_WRITE)?;
    let scope = scope()?;
    let held = slot(10)?;
    tenancy.uploads.put(&scope, held.clone())?;
    let refused = err_of(upload(&tenancy, LocalOwner, held.id, b"0123").await)?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert!(
        tenancy.uploads.limits()?.is_empty(),
        "no byte reached the store"
    );
    assert_eq!(
        tenancy.log.appended()?,
        refusal(LocalOwner, TransportName::UploadContent, workspace_log()?)?
    );
    // A workspace the adapter holds no grant on: its own `Forbidden` is recorded the same way,
    // in that workspace's log.
    let other = other_scope()?;
    let refused = err_of(
        tenancy
            .service
            .upload_content(
                &caller(LocalOwner)?,
                other.workspace_id,
                held.id,
                body(b"0"),
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    let expected = refusal(
        LocalOwner,
        TransportName::UploadContent,
        EventScope::Workspace(other),
    )?;
    assert_eq!(tenancy.log.appended()?.get(1), expected.first());
    Ok(())
}

#[tokio::test]
async fn upload_content_is_refused_in_an_archived_workspace_after_authorization() -> TestResult {
    let tenancy = Tenancy::new()?;
    let scope = scope()?;
    tenancy.catalog.put(
        &scope.tenant_id,
        workspace(WORKSPACE, "Fixture", 'a', true)?,
    )?;
    tenancy
        .access
        .grant(ALICE, scope.workspace_id, vec![Permission::Write])?;
    let held = slot(10)?;
    tenancy.uploads.put(&scope, held.clone())?;
    // A caller the rules refuse learns nothing of the workspace: the catalog is not asked.
    let refused = err_of(upload(&tenancy, McpDelegation, held.id, b"0123").await)?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert_eq!(tenancy.catalog.opens(), 0);
    let recorded = tenancy.log.appended()?.len();
    let refused = err_of(upload(&tenancy, LocalOwner, held.id, b"0123").await)?;
    assert_eq!(refused.code, ErrorCode::Conflict, "{}", refused.message);
    assert_eq!(
        refused.detail.as_deref(),
        Some(&ErrorDetail::WorkspaceArchived {
            archived_at: archived_instant()?
        })
    );
    assert!(
        tenancy.uploads.limits()?.is_empty(),
        "no byte reached the store"
    );
    assert_eq!(tenancy.uploads.held(&scope, held.id)?.received_bytes, 0);
    assert_eq!(
        tenancy.log.appended()?.len(),
        recorded,
        "an archived workspace is no denial"
    );
    Ok(())
}

#[tokio::test]
async fn upload_content_answers_the_stores_refusals_as_they_are_and_records_none() -> TestResult {
    let tenancy = world(&[Permission::Write])?;
    let scope = scope()?;
    let held = slot(10)?;
    tenancy.uploads.put(&scope, held.clone())?;
    let long = err_of(upload(&tenancy, LocalOwner, held.id, b"0123456789A").await)?;
    assert_eq!(long.code, ErrorCode::TooLarge, "{}", long.message);
    assert_eq!(tenancy.uploads.held(&scope, held.id)?.received_bytes, 0);
    let unknown = err_of(upload(&tenancy, LocalOwner, UploadId(Uuid::from_u128(31)), b"0").await)?;
    assert_eq!(unknown.code, ErrorCode::NotFound);
    tenancy.uploads.put(
        &scope,
        UploadRecord {
            received_bytes: 10,
            object: Some(ObjectInfo {
                digest: digest('9')?,
                size: 10,
            }),
            ..held.clone()
        },
    )?;
    let complete = err_of(upload(&tenancy, LocalOwner, held.id, b"0").await)?;
    assert_eq!(complete.code, ErrorCode::Conflict);
    assert!(tenancy.log.appended()?.is_empty(), "no event is recorded");
    Ok(())
}

#[tokio::test]
async fn upload_content_refuses_a_foreign_grant_and_keeps_its_refusal_when_the_log_fails()
-> TestResult {
    let tenancy = world(&[Permission::Write])?;
    let held = slot(10)?;
    tenancy.uploads.put(&scope()?, held.clone())?;
    tenancy.log.fail_next();
    let refused = err_of(upload(&tenancy, McpDelegation, held.id, b"0").await)?;
    assert_eq!(refused.code, ErrorCode::Forbidden, "{}", refused.message);
    assert!(tenancy.log.appended()?.is_empty(), "the append failed");
    tenancy.access.answer_for(other_scope()?)?;
    let foreign = err_of(upload(&tenancy, LocalOwner, held.id, b"0").await)?;
    assert_eq!(foreign.code, ErrorCode::Internal, "{}", foreign.message);
    assert!(
        tenancy.uploads.limits()?.is_empty(),
        "no byte reached the store"
    );
    assert!(tenancy.log.appended()?.is_empty(), "no event is recorded");
    Ok(())
}

// ---- download_object ----

#[tokio::test]
async fn download_object_sends_an_object_of_the_cited_revision_whole_with_its_media_type()
-> TestResult {
    let tenancy = world(&[Permission::Read])?;
    with_card(&tenancy)?;
    let sent = received(original(&tenancy, LocalOwner, None).await?).await?;
    assert_eq!(sent, whole("application/pdf", ORIGINAL)?);
    let conversion = object(&tenancy.service, LocalOwner, digest('b')?, None).await?;
    assert_eq!(conversion.media_type, "application/json");
    assert_eq!(
        tenancy.access.lookups()?,
        vec![Permission::Read, Permission::Read]
    );
    assert!(
        tenancy.world.records.receipts()?.is_empty(),
        "a download writes no receipt"
    );
    assert!(tenancy.log.appended()?.is_empty(), "no event is recorded");
    Ok(())
}

#[tokio::test]
async fn download_object_sends_the_range_asked_and_cuts_an_end_past_the_last_byte() -> TestResult {
    let tenancy = world(&[Permission::Read])?;
    with_card(&tenancy)?;
    let part = |offset: u64, bytes: &[u8]| -> Built<Received> {
        Ok(Received {
            media_type: "application/pdf".to_owned(),
            total_size: 11,
            offset,
            length: u64::try_from(bytes.len())?,
            bytes: bytes.to_vec(),
        })
    };
    for (asked, expected) in [
        (Some(range(2, Some(5))), part(2, b"llo ")?),
        (Some(range(6, None)), part(6, b"world")?),
        (Some(range(6, Some(100))), part(6, b"world")?),
        (Some(range(0, Some(0))), part(0, b"h")?),
        (Some(range(10, None)), part(10, b"d")?),
        (Some(range(10, Some(10))), part(10, b"d")?),
    ] {
        let sent = received(original(&tenancy, LocalOwner, asked).await?).await?;
        assert_eq!(sent, expected, "{asked:?}");
    }
    // A start at or past the end is no interval of the object: `invalid_input`, as the
    // contract has no 416.
    for asked in [
        Some(range(11, None)),
        Some(range(11, Some(20))),
        Some(range(12, None)),
    ] {
        let refused = err_of(original(&tenancy, LocalOwner, asked).await.map(drop))?;
        assert_eq!(refused.code, ErrorCode::InvalidInput, "{asked:?}");
    }
    // An inverted range is refused before anything is opened.
    let opened = tenancy.world.blobs.opens()?.len();
    let inverted = err_of(
        original(&tenancy, LocalOwner, Some(range(5, Some(2))))
            .await
            .map(drop),
    )?;
    assert_eq!(inverted.code, ErrorCode::InvalidInput);
    assert_eq!(tenancy.world.blobs.opens()?.len(), opened);
    assert!(
        tenancy.log.appended()?.is_empty(),
        "no refusal here is a denial"
    );
    Ok(())
}

#[tokio::test]
async fn download_object_body_yields_no_more_than_the_length_it_announces() -> TestResult {
    let tenancy = world(&[Permission::Read])?;
    with_card(&tenancy)?;
    let mut ports = tenancy.service.ports().clone();
    ports.blobs = Arc::new(Overlong {
        objects: Mutex::new(vec![
            (digest('a')?, ORIGINAL.to_vec()),
            (digest('b')?, serde_json::to_vec(&record(Vec::new())?)?),
        ]),
    });
    let service = ApplicationService::new(ports, config())?;
    let sent = received(object(&service, LocalOwner, digest('a')?, Some(range(2, Some(5)))).await?)
        .await?;
    assert_eq!((sent.length, sent.bytes), (4, b"llo ".to_vec()));
    Ok(())
}

#[tokio::test]
async fn download_object_checks_the_cited_revision_against_the_purge_map_first() -> TestResult {
    let tenancy = world(&[Permission::Read])?;
    with_card(&tenancy)?;
    let purge_id = PurgeId(Uuid::from_u128(5));
    tenancy.world.records.put_mapping(
        revision('a')?,
        RevisionMapping {
            purge_id,
            replacement: None,
        },
    )?;
    let refused = err_of(original(&tenancy, LocalOwner, None).await.map(drop))?;
    assert_eq!(
        refused.detail.as_deref(),
        Some(&ErrorDetail::Invalidated {
            purge_id,
            replacement: None
        })
    );
    assert_eq!(tenancy.world.versions.shows(), 0);
    assert!(
        tenancy.world.blobs.opens()?.is_empty(),
        "nothing was opened"
    );
    assert!(tenancy.log.appended()?.is_empty(), "no event is recorded");
    Ok(())
}

#[tokio::test]
async fn download_object_never_serves_an_object_by_its_digest_alone() -> TestResult {
    let tenancy = world(&[Permission::Read])?;
    with_card(&tenancy)?;
    // Bytes the store holds, a backup's say, that are none of the cited revision's objects.
    let backup = digest('f')?;
    tenancy
        .world
        .blobs
        .put_object(backup.clone(), b"a backup".to_vec())?;
    let refused = err_of(
        object(&tenancy.service, LocalOwner, backup.clone(), None)
            .await
            .map(drop),
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert!(
        tenancy
            .world
            .blobs
            .opens()?
            .iter()
            .all(|(opened, _, _)| *opened != backup),
        "its bytes are never opened"
    );
    assert!(
        tenancy.log.appended()?.is_empty(),
        "a not_found is no denial"
    );
    Ok(())
}

#[tokio::test]
async fn download_object_on_each_route_refuses_only_a_connector_and_records_it() -> TestResult {
    for route in ROUTES {
        let tenancy = world(&[Permission::Read])?;
        with_card(&tenancy)?;
        let outcome = original(&tenancy, route.clone(), None).await;
        if route == McpDelegation {
            let refused = err_of(outcome.map(drop))?;
            assert_eq!(refused.code, ErrorCode::Forbidden, "{route:?}");
            assert!(
                tenancy.access.lookups()?.is_empty(),
                "refused before any lookup"
            );
            assert!(
                tenancy.world.blobs.opens()?.is_empty(),
                "nothing was opened"
            );
            assert_eq!(
                tenancy.log.appended()?,
                refusal(route, TransportName::DownloadObject, workspace_log()?)?
            );
        } else {
            assert_eq!(
                received(outcome?).await?,
                whole("application/pdf", ORIGINAL)?,
                "{route:?}"
            );
            assert!(tenancy.log.appended()?.is_empty(), "{route:?}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn download_object_needs_read_on_the_workspace() -> TestResult {
    let tenancy = world(&ALL_BUT_READ)?;
    with_card(&tenancy)?;
    let refused = err_of(original(&tenancy, LocalOwner, None).await.map(drop))?;
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert!(
        tenancy.world.blobs.opens()?.is_empty(),
        "nothing was opened"
    );
    assert_eq!(
        tenancy.log.appended()?,
        refusal(LocalOwner, TransportName::DownloadObject, workspace_log()?)?
    );
    Ok(())
}

#[tokio::test]
async fn download_object_refuses_a_foreign_grant_and_keeps_its_refusal_when_the_log_fails()
-> TestResult {
    let tenancy = world(&[Permission::Read])?;
    with_card(&tenancy)?;
    tenancy.log.fail_next();
    let refused = err_of(original(&tenancy, McpDelegation, None).await.map(drop))?;
    assert_eq!(refused.code, ErrorCode::Forbidden, "{}", refused.message);
    assert!(tenancy.log.appended()?.is_empty(), "the append failed");
    tenancy.access.answer_for(other_scope()?)?;
    let foreign = err_of(original(&tenancy, LocalOwner, None).await.map(drop))?;
    assert_eq!(foreign.code, ErrorCode::Internal, "{}", foreign.message);
    assert!(
        tenancy.world.blobs.opens()?.is_empty(),
        "nothing was opened"
    );
    assert!(tenancy.log.appended()?.is_empty(), "no event is recorded");
    Ok(())
}
