//! Receipts read back with their citations resolved through the revision map, the workspace's
//! notifications with security events only for its administrator, the tenant's log, and the
//! one rule a sign-in is recorded by.

use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::common::PageRequest;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{
    EventKind, GetReceiptRequest, ListEventsRequest, ListEventsResponse, ListTenantEventsRequest,
    Receipt, ReceiptAudience,
};
use okf_jawn_contract::identity::{ItemId, MutationId, PurgeId, ReceiptId, Revision, WorkspaceId};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::source::{SourceLocation, SourceReference, UnresolvedReason};
use okf_jawn_core::context::{Attempt, OperationContext, TenantGrant, WorkspaceGrant};
use okf_jawn_core::events::{EventLog, EventScope, NewEvent};
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::StorageScope;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{ALICE, Built, digest, instant, path, revision, scope};
use crate::tenancy::Tenancy;

const RECEIPT: u128 = 60;
const ITEM: u128 = 20;
const PURGE: u128 = 5;

fn principal() -> Built<Principal> {
    Ok(Principal {
        subject: ALICE.to_owned(),
        tenant_id: scope()?.tenant_id,
        route: AccessRoute::LocalOwner,
        client_id: None,
        delegation: None,
    })
}

/// The context of a read of the fixture workspace whose dispatch grant is `permissions`, or of
/// the tenant when `tenant`.
fn reader(
    operation: OperationName,
    permissions: &[Permission],
    tenant: bool,
) -> Built<OperationContext> {
    let scope = scope()?;
    Ok(OperationContext {
        principal: principal()?,
        session_id: None,
        operation,
        tenant: tenant.then(|| TenantGrant {
            tenant_id: scope.tenant_id.clone(),
            permissions: vec![Permission::Admin],
        }),
        grants: if permissions.is_empty() {
            Vec::new()
        } else {
            vec![WorkspaceGrant {
                scope,
                permissions: permissions.to_vec(),
            }]
        },
        mutation: None::<MutationId>,
        attempt: Attempt::First,
    })
}

fn source(workspace: WorkspaceId, fill: char) -> Built<SourceReference> {
    Ok(SourceReference {
        workspace_id: workspace,
        item_id: ItemId(Uuid::from_u128(ITEM)),
        path: path("notes/plan.md")?,
        revision: revision(fill)?,
        digest: Some(digest('e')?),
        selection: Selection::Lines {
            range: okf_jawn_contract::common::TextRange { start: 1, end: 2 },
        },
        locations: vec![SourceLocation::Unresolved {
            reason: UnresolvedReason::FormatHasNoLocator,
        }],
    })
}

fn receipt(sources: Vec<SourceReference>) -> Built<Receipt> {
    Ok(Receipt {
        id: ReceiptId(Uuid::from_u128(RECEIPT)),
        workspace_id: scope()?.workspace_id,
        operation_id: OperationName::ReadItem,
        principal_subject: ALICE.to_owned(),
        route: AccessRoute::LocalOwner,
        sources,
        returned_at: instant()?,
        trace_id: None,
        audience: ReceiptAudience::HumanDisplay,
        invalidated_by: None,
    })
}

/// The fixture receipt, read in the fixture workspace.
async fn read_receipt(tenancy: &Tenancy) -> Built<Result<Receipt, ApiError>> {
    Ok(tenancy
        .service
        .get_receipt(
            &reader(OperationName::GetReceipt, &[Permission::Read], false)?,
            GetReceiptRequest {
                workspace_id: scope()?.workspace_id,
                receipt_id: ReceiptId(Uuid::from_u128(RECEIPT)),
            },
        )
        .await)
}

#[tokio::test]
async fn get_receipt_answers_the_receipt_of_the_workspace_as_stored() -> TestResult {
    let tenancy = Tenancy::new()?;
    let stored = receipt(vec![source(scope()?.workspace_id, 'a')?])?;
    tenancy.jobs.put_receipt(&scope()?, stored.clone())?;
    let read = read_receipt(&tenancy).await??;
    assert_eq!(serde_json::to_value(&read)?, serde_json::to_value(&stored)?);
    Ok(())
}

#[tokio::test]
async fn a_receipt_of_another_workspace_is_not_found_whatever_the_store_answers() -> TestResult {
    let tenancy = Tenancy::new()?;
    let elsewhere = StorageScope {
        tenant_id: scope()?.tenant_id,
        workspace_id: WorkspaceId(Uuid::from_u128(2)),
    };
    let mut foreign = receipt(Vec::new())?;
    foreign.workspace_id = elsewhere.workspace_id;
    tenancy.jobs.put_receipt(&elsewhere, foreign)?;
    let refused = err_of(read_receipt(&tenancy).await?)?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    // A store that ignored the scope still yields no receipt of another workspace.
    tenancy.jobs.ignore_receipt_scope();
    let refused = err_of(read_receipt(&tenancy).await?)?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    Ok(())
}

#[tokio::test]
async fn a_stored_citation_is_shown_at_the_revision_a_purge_rewrote_it_to() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy.world.records.put_mapping(
        revision('b')?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(PURGE)),
            replacement: Some(revision('c')?),
        },
    )?;
    let kept = source(scope()?.workspace_id, 'b')?;
    tenancy
        .jobs
        .put_receipt(&scope()?, receipt(vec![kept.clone()])?)?;
    let read = read_receipt(&tenancy).await??;
    let shown = some(read.sources.first(), "the citation")?;
    assert_eq!(shown.revision, revision('c')?);
    assert_eq!(
        shown.path, kept.path,
        "another item's citation keeps its path"
    );
    assert_eq!(shown.selection, kept.selection);
    assert_eq!(shown.digest, kept.digest);
    assert_eq!(read.invalidated_by, None);
    Ok(())
}

#[tokio::test]
async fn a_citation_of_a_removed_revision_is_shown_invalidated_naming_the_purge() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy.world.records.put_mapping(
        revision('b')?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(PURGE)),
            replacement: None,
        },
    )?;
    let untouched = source(scope()?.workspace_id, 'a')?;
    tenancy.jobs.put_receipt(
        &scope()?,
        receipt(vec![untouched.clone(), source(scope()?.workspace_id, 'b')?])?,
    )?;
    let read = read_receipt(&tenancy).await??;
    assert_eq!(read.invalidated_by, Some(PurgeId(Uuid::from_u128(PURGE))));
    let first = some(read.sources.first(), "the untouched citation")?;
    assert_eq!(first.path, untouched.path);
    let removed = some(read.sources.get(1), "the invalidated citation")?;
    assert_eq!(
        removed.path,
        path(&format!("purged/{}", Uuid::from_u128(ITEM)))?
    );
    assert_eq!(removed.revision, revision('b')?, "its identities stay");
    assert_eq!(removed.selection, Selection::All);
    assert_eq!(removed.digest, None);
    assert_eq!(removed.locations, Vec::new());
    Ok(())
}

#[tokio::test]
async fn a_citation_storage_already_scrubbed_is_shown_as_it_is() -> TestResult {
    let tenancy = Tenancy::new()?;
    // Its revision has a replacement, so a citation that was not scrubbed would move to it.
    tenancy.world.records.put_mapping(
        revision('b')?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(PURGE)),
            replacement: Some(revision('c')?),
        },
    )?;
    let mut scrubbed = source(scope()?.workspace_id, 'b')?;
    scrubbed.path = path(&format!("purged/{}", Uuid::from_u128(ITEM)))?;
    scrubbed.selection = Selection::All;
    scrubbed.digest = None;
    scrubbed.locations = Vec::new();
    let mut stored = receipt(vec![scrubbed.clone()])?;
    stored.invalidated_by = Some(PurgeId(Uuid::from_u128(PURGE)));
    tenancy.jobs.put_receipt(&scope()?, stored)?;
    let read = read_receipt(&tenancy).await??;
    let shown = some(read.sources.first(), "the citation")?;
    assert_eq!(shown.revision, revision('b')?);
    assert_eq!(read.invalidated_by, Some(PurgeId(Uuid::from_u128(PURGE))));
    Ok(())
}

#[tokio::test]
async fn each_cited_revision_is_looked_up_once_in_its_own_workspace() -> TestResult {
    let tenancy = Tenancy::new()?;
    let elsewhere = WorkspaceId(Uuid::from_u128(2));
    tenancy.jobs.put_receipt(
        &scope()?,
        receipt(vec![
            source(scope()?.workspace_id, 'a')?,
            source(scope()?.workspace_id, 'a')?,
            source(elsewhere, 'a')?,
        ])?,
    )?;
    read_receipt(&tenancy).await??;
    let asked: Vec<(WorkspaceId, Revision)> = tenancy
        .jobs
        .mapping_lookups()?
        .into_iter()
        .map(|(asked_in, at)| (asked_in.workspace_id, at))
        .collect();
    assert_eq!(
        asked,
        vec![
            (scope()?.workspace_id, revision('a')?),
            (elsewhere, revision('a')?)
        ],
        "one lookup per (workspace, revision), in the cited workspace"
    );
    Ok(())
}

/// Append `kind` to the fixture workspace's log, as an operation would.
async fn append(tenancy: &Tenancy, kind: EventKind) -> TestResult {
    let event = if kind == EventKind::PermissionDenied {
        NewEvent::permission_denied(&principal()?, OperationName::CreateItem)
    } else {
        NewEvent {
            kind,
            revision: Some(revision('a')?),
            item_id: None,
            job_id: None,
            connector_id: None,
            actor: None,
            operation: None,
            proposal_id: None,
            count: None,
        }
    };
    tenancy
        .log
        .append(&EventScope::Workspace(scope()?), None, event)
        .await?;
    Ok(())
}

fn events_request(
    after: Option<&str>,
    cursor: Option<&str>,
    limit: u16,
) -> Built<ListEventsRequest> {
    Ok(ListEventsRequest {
        workspace_id: scope()?.workspace_id,
        after: after.map(str::to_owned),
        page: PageRequest {
            cursor: cursor.map(str::to_owned),
            limit,
        },
    })
}

fn kinds(page: &ListEventsResponse) -> Vec<EventKind> {
    page.events.iter().map(|event| event.kind.clone()).collect()
}

async fn list(tenancy: &Tenancy, request: ListEventsRequest) -> Built<ListEventsResponse> {
    // Dispatch authorized `read`; what the caller holds otherwise is the grant table's answer.
    Ok(tenancy
        .service
        .list_events(
            &reader(
                OperationName::ListEvents,
                &[Permission::Read, Permission::Admin],
                false,
            )?,
            request,
        )
        .await?)
}

/// A tenancy whose fixture workspace's log is changed, denied, changed, and where Alice holds
/// `permissions`.
async fn three_events(permissions: Vec<Permission>) -> Built<Tenancy> {
    let tenancy = Tenancy::new()?;
    tenancy
        .access
        .grant(ALICE, scope()?.workspace_id, permissions)?;
    append(&tenancy, EventKind::Changed).await?;
    append(&tenancy, EventKind::PermissionDenied).await?;
    append(&tenancy, EventKind::Changed).await?;
    Ok(tenancy)
}

#[tokio::test]
async fn list_events_hides_security_events_from_a_caller_without_admin() -> TestResult {
    let tenancy = three_events(vec![Permission::Read, Permission::Write]).await?;
    let page = list(&tenancy, events_request(None, None, 50)?).await?;
    assert_eq!(kinds(&page), vec![EventKind::Changed, EventKind::Changed]);
    assert_eq!(page.next_cursor, None, "the last event read was shown");
    assert_eq!(tenancy.access.lookups()?, vec![Permission::Admin]);
    Ok(())
}

#[tokio::test]
async fn an_administrator_sees_the_workspaces_security_events() -> TestResult {
    let tenancy = three_events(vec![Permission::Read, Permission::Admin]).await?;
    let page = list(&tenancy, events_request(None, None, 50)?).await?;
    assert_eq!(
        kinds(&page),
        vec![
            EventKind::Changed,
            EventKind::PermissionDenied,
            EventKind::Changed
        ]
    );
    Ok(())
}

#[tokio::test]
async fn admin_is_looked_up_fresh_on_every_call_not_taken_from_dispatchs_grant() -> TestResult {
    // Dispatch's grant (the context) holds `admin`; the grant table, asked now, decides.
    let tenancy = three_events(vec![Permission::Read, Permission::Admin]).await?;
    let shown = list(&tenancy, events_request(None, None, 50)?).await?;
    assert_eq!(shown.events.len(), 3);
    tenancy.access.grant(
        ALICE,
        scope()?.workspace_id,
        vec![Permission::Read, Permission::Write],
    )?;
    let hidden = list(&tenancy, events_request(None, None, 50)?).await?;
    assert_eq!(
        hidden.events.len(),
        2,
        "admin was withdrawn between the calls"
    );
    assert_eq!(
        tenancy.access.lookups()?,
        vec![Permission::Admin, Permission::Admin]
    );
    Ok(())
}

#[tokio::test]
async fn an_agent_route_holding_admin_is_shown_no_security_event() -> TestResult {
    // The route rule applies as dispatch applies it: an agent never holds `admin`.
    let tenancy = three_events(vec![
        Permission::Read,
        Permission::Write,
        Permission::Propose,
        Permission::Admin,
    ])
    .await?;
    let mut agent = reader(OperationName::ListEvents, &[Permission::Read], false)?;
    agent.principal.route = AccessRoute::McpDelegation;
    let page = tenancy
        .service
        .list_events(&agent, events_request(None, None, 50)?)
        .await?;
    assert_eq!(page.events.len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_caller_the_adapter_now_grants_nothing_is_shown_no_security_event() -> TestResult {
    // Dispatch authorized the read; the grant is gone when the admin lookup asks (`Forbidden`
    // from the adapter means no grant), so the listing answers without the refusals.
    let tenancy = three_events(vec![Permission::Read, Permission::Admin]).await?;
    let mut gone = reader(OperationName::ListEvents, &[Permission::Read], false)?;
    gone.principal.subject = "bob".to_owned();
    let page = tenancy
        .service
        .list_events(&gone, events_request(None, None, 50)?)
        .await?;
    assert_eq!(kinds(&page), vec![EventKind::Changed, EventKind::Changed]);
    Ok(())
}

#[tokio::test]
async fn a_page_whose_last_events_are_hidden_carries_the_cursor_after_them() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy
        .access
        .grant(ALICE, scope()?.workspace_id, vec![Permission::Read])?;
    append(&tenancy, EventKind::Changed).await?;
    append(&tenancy, EventKind::PermissionDenied).await?;
    append(&tenancy, EventKind::PermissionDenied).await?;
    let page = list(&tenancy, events_request(None, None, 50)?).await?;
    assert_eq!(kinds(&page), vec![EventKind::Changed]);
    assert_eq!(
        page.next_cursor.as_deref(),
        Some("3"),
        "after the hidden events"
    );
    // A page of hidden events only is empty and still carries a cursor.
    let empty = list(&tenancy, events_request(Some("1"), None, 50)?).await?;
    assert!(empty.events.is_empty());
    assert_eq!(empty.next_cursor.as_deref(), Some("3"));
    // A full page keeps the store's own continuation.
    let full = list(&tenancy, events_request(None, None, 2)?).await?;
    assert_eq!(kinds(&full), vec![EventKind::Changed]);
    assert_eq!(full.next_cursor.as_deref(), Some("2"));
    Ok(())
}

#[tokio::test]
async fn a_page_without_a_security_event_asks_for_no_grant() -> TestResult {
    let tenancy = Tenancy::new()?;
    append(&tenancy, EventKind::Changed).await?;
    let page = list(&tenancy, events_request(None, None, 50)?).await?;
    assert_eq!(page.events.len(), 1);
    assert_eq!(tenancy.access.lookups()?, Vec::new());
    Ok(())
}

#[tokio::test]
async fn list_events_reads_after_the_page_cursor_or_else_after_the_last_seen_event() -> TestResult {
    let tenancy = Tenancy::new()?;
    list(&tenancy, events_request(Some("4"), Some("7"), 10)?).await?;
    list(&tenancy, events_request(Some("4"), None, 10)?).await?;
    let asked: Vec<Option<String>> = tenancy
        .log
        .lists()?
        .into_iter()
        .map(|(log, query)| {
            assert!(matches!(log, EventScope::Workspace(_)));
            query.after
        })
        .collect();
    assert_eq!(asked, vec![Some("7".to_owned()), Some("4".to_owned())]);
    Ok(())
}

#[tokio::test]
async fn a_cursor_the_store_refuses_names_the_field_that_carried_it() -> TestResult {
    let tenancy = Tenancy::new()?;
    for (after, cursor, field) in [
        (Some("x"), None, "/after"),
        (None, Some("x"), "/page/cursor"),
    ] {
        let refused = err_of(
            tenancy
                .service
                .list_events(
                    &reader(OperationName::ListEvents, &[Permission::Read], false)?,
                    events_request(after, cursor, 10)?,
                )
                .await,
        )?;
        assert_eq!(refused.code, ErrorCode::InvalidInput);
        assert_eq!(refused.field.as_deref(), Some(field));
    }
    Ok(())
}

#[tokio::test]
async fn a_grant_lookup_that_fails_fails_the_listing_instead_of_hiding() -> TestResult {
    let tenancy = three_events(vec![Permission::Read, Permission::Admin]).await?;
    tenancy.access.unavailable_next();
    let refused = err_of(
        tenancy
            .service
            .list_events(
                &reader(OperationName::ListEvents, &[Permission::Read], false)?,
                events_request(None, None, 50)?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    // A grant answered for another workspace is the adapter's fault.
    tenancy.access.answer_for(StorageScope {
        tenant_id: scope()?.tenant_id,
        workspace_id: WorkspaceId(Uuid::from_u128(2)),
    })?;
    let refused = err_of(
        tenancy
            .service
            .list_events(
                &reader(OperationName::ListEvents, &[Permission::Read], false)?,
                events_request(None, None, 50)?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Internal);
    Ok(())
}

#[tokio::test]
async fn list_tenant_events_reads_the_tenants_log_whole() -> TestResult {
    let tenancy = Tenancy::new()?;
    append(&tenancy, EventKind::Changed).await?;
    tenancy.service.record_sign_in(&principal()?).await?;
    let page = tenancy
        .service
        .list_tenant_events(
            &reader(OperationName::ListTenantEvents, &[], true)?,
            ListTenantEventsRequest {
                after: None,
                page: PageRequest {
                    cursor: None,
                    limit: 10,
                },
            },
        )
        .await?;
    assert_eq!(
        kinds(&page),
        vec![EventKind::SignedIn],
        "security events included"
    );
    assert_eq!(
        page.events.first().and_then(|event| event.workspace_id),
        None
    );
    let asked = tenancy.log.lists()?;
    let (log, _) = some(asked.first(), "the read")?;
    assert_eq!(*log, EventScope::Tenant(scope()?.tenant_id));
    Ok(())
}

#[tokio::test]
async fn record_sign_in_appends_signed_in_to_the_tenants_log() -> TestResult {
    let tenancy = Tenancy::new()?;
    let mut owner = principal()?;
    owner.client_id = Some("browser".to_owned());
    tenancy.service.record_sign_in(&owner).await?;
    let appended = tenancy.log.appended()?;
    let (log, under, event) = some(appended.first(), "the sign-in")?;
    assert_eq!(*log, EventScope::Tenant(owner.tenant_id.clone()));
    assert_eq!(*under, None, "every sign-in is its own event");
    assert_eq!(*event, NewEvent::signed_in(&owner));
    assert_eq!(event.kind, EventKind::SignedIn);
    let actor = some(event.actor.as_ref(), "the actor")?;
    assert_eq!(actor.subject, ALICE);
    assert_eq!(actor.route, AccessRoute::LocalOwner);
    assert_eq!(actor.client_id.as_deref(), Some("browser"));
    Ok(())
}

#[tokio::test]
async fn a_sign_in_that_cannot_be_recorded_is_an_error() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy.log.fail_next();
    let refused = err_of(tenancy.service.record_sign_in(&principal()?).await)?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(tenancy.log.appended()?, Vec::new());
    Ok(())
}
