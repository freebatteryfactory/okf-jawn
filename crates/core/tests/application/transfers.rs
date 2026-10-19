//! The byte transfers through `ApplicationService` (`transfers::Transfers`) over the tenancy
//! fakes: what each transfer asks of the ports on each route, the interval and bytes it answers,
//! and how it refuses. Every `Forbidden` a transfer answers is recorded as `permission_denied`
//! naming its transport and no operation, in the asked workspace's log; a refusal with any other
//! code is recorded nowhere.

use std::io::Cursor;

use okf_jawn_contract::access::AccessRoute::{BrowserSession, LocalOwner, McpDelegation, Service};
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{MutationId, UploadId, WorkspaceId};
use okf_jawn_contract::transport::TransportName;
use okf_jawn_core::application::ApplicationConfig;
use okf_jawn_core::events::{EventScope, NewEvent};
use okf_jawn_core::storage::{ByteReader, ObjectInfo, StorageScope};
use okf_jawn_core::transfers::{TransferCaller, Transfers};
use okf_jawn_core::uploads::UploadRecord;
use uuid::Uuid;

use crate::check::{TestResult, err_of};
use crate::fixture::{ALICE, Built, WORKSPACE, config, digest, instant, scope};
use crate::tenancy::{Tenancy, archived_instant, uploader, workspace};

/// Every route a principal arrives by.
const ROUTES: [AccessRoute; 4] = [BrowserSession, LocalOwner, Service, McpDelegation];
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
