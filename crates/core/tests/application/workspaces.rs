//! Workspaces: listing and opening them with the caller's effective permissions, creating one,
//! the metadata changes at the head, the archive the service reads for dispatch, and the four
//! operations that start a job, a restore's checks among them.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use okf_jawn_contract::access::{AccessRoute, DelegationCeiling, Permission, Principal};
use okf_jawn_contract::common::PageRequest;
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::history::{Commit, LogResponse};
use okf_jawn_contract::identity::{
    At, Digest, IdempotencyKey, JobId, MutationId, PurgeId, Revision, UploadId, WorkspaceId,
};
use okf_jawn_contract::import::JobKind;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::workspace::{
    ArchiveWorkspaceRequest, BackupInstallationRequest, BackupWorkspaceRequest,
    CreateWorkspaceRequest, ExportWorkspaceRequest, ListWorkspacesRequest, ListWorkspacesResponse,
    OpenWorkspaceRequest, RestoreWorkspaceRequest, UnarchiveWorkspaceRequest,
    UpdateWorkspaceRequest,
};
use okf_jawn_core::context::{Attempt, OperationContext, TenantGrant, WorkspaceGrant};
use okf_jawn_core::jobs::{JobScope, JobSpec, RevisionMapping};
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::{
    CommitChanges, FolderListing, ObjectInfo, Provenance, StorageScope, VersionStore,
    WorkspaceArchive, WorkspaceCatalog, derive_workspace_id,
};
use okf_jawn_core::uploads::UploadRecord;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    ALICE, Built, digest, draft_of, instant, mutation, note, path, revision, scope,
};
use crate::tenancy::{Tenancy, archived_instant, uploader, workspace};

/// Every permission a grant can hold, in the contract's order.
const EVERY: [Permission; 6] = [
    Permission::Read,
    Permission::Write,
    Permission::Propose,
    Permission::Approve,
    Permission::Review,
    Permission::Admin,
];

/// The archive this module's restores upload.
const UPLOAD: u128 = 70;

fn key() -> Built<IdempotencyKey> {
    Ok(serde_json::from_value(serde_json::json!(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    ))?)
}

fn id(number: u128) -> WorkspaceId {
    WorkspaceId(Uuid::from_u128(number))
}

fn scope_of(number: u128) -> Built<StorageScope> {
    Ok(StorageScope {
        tenant_id: scope()?.tenant_id,
        workspace_id: id(number),
    })
}

fn principal(subject: &str, route: AccessRoute) -> Built<Principal> {
    Ok(Principal {
        subject: subject.to_owned(),
        tenant_id: scope()?.tenant_id,
        route,
        client_id: None,
        delegation: None,
    })
}

/// The context dispatch builds for `principal` running `operation`: the tenant's grant when
/// `tenant`, a grant of `permissions` on the fixture workspace, and `mutation`.
fn context(
    principal: Principal,
    operation: OperationName,
    tenant: bool,
    permissions: &[Permission],
    mutation: Option<MutationId>,
) -> Built<OperationContext> {
    let scope = scope()?;
    Ok(OperationContext {
        tenant: tenant.then(|| TenantGrant {
            tenant_id: scope.tenant_id.clone(),
            permissions: EVERY.to_vec(),
        }),
        grants: if permissions.is_empty() {
            Vec::new()
        } else {
            vec![WorkspaceGrant {
                scope,
                permissions: permissions.to_vec(),
            }]
        },
        principal,
        session_id: None,
        operation,
        mutation,
        attempt: Attempt::First,
    })
}

/// Alice, the local owner, with every permission on the fixture workspace and the tenant.
fn owner(operation: OperationName, mutation: Option<MutationId>) -> Built<OperationContext> {
    context(
        principal(ALICE, AccessRoute::LocalOwner)?,
        operation,
        true,
        &EVERY,
        mutation,
    )
}

fn listing(
    limit: u16,
    cursor: Option<String>,
    include_archived: Option<bool>,
) -> ListWorkspacesRequest {
    ListWorkspacesRequest {
        page: PageRequest { cursor, limit },
        include_archived,
    }
}

fn ids(response: &ListWorkspacesResponse) -> Vec<WorkspaceId> {
    response.items.iter().map(|item| item.id).collect()
}

/// A tenancy whose catalog holds workspaces 11 to 15, all of which Alice may read and write.
fn five_workspaces() -> Built<Tenancy> {
    let tenancy = Tenancy::new()?;
    let tenant = scope()?.tenant_id;
    for (number, name) in [
        (11, "one"),
        (12, "two"),
        (13, "three"),
        (14, "four"),
        (15, "five"),
    ] {
        tenancy
            .catalog
            .put(&tenant, workspace(number, name, 'a', false)?)?;
        tenancy
            .access
            .grant(ALICE, id(number), vec![Permission::Read, Permission::Write])?;
    }
    Ok(tenancy)
}

#[tokio::test]
async fn list_workspaces_lists_only_what_the_caller_may_read_with_its_effective_permissions()
-> TestResult {
    let tenancy = Tenancy::new()?;
    let tenant = scope()?.tenant_id;
    for (number, name) in [
        (11, "all"),
        (12, "propose only"),
        (13, "none"),
        (14, "read"),
    ] {
        tenancy
            .catalog
            .put(&tenant, workspace(number, name, 'a', false)?)?;
    }
    tenancy.access.grant(ALICE, id(11), EVERY.to_vec())?;
    tenancy
        .access
        .grant(ALICE, id(12), vec![Permission::Propose])?;
    tenancy
        .access
        .grant(ALICE, id(14), vec![Permission::Read, Permission::Write])?;
    let listed = tenancy
        .service
        .list_workspaces(
            &owner(OperationName::ListWorkspaces, None)?,
            listing(50, None, None),
        )
        .await?;
    assert_eq!(
        ids(&listed),
        vec![id(11), id(14)],
        "creation order, readable only"
    );
    let permissions: Vec<Vec<Permission>> = listed
        .items
        .iter()
        .map(|item| item.permissions.clone())
        .collect();
    assert_eq!(
        permissions,
        vec![EVERY.to_vec(), vec![Permission::Read, Permission::Write]],
        "the catalog's empty permissions are the caller's grant"
    );
    assert_eq!(listed.next_cursor, None);
    Ok(())
}

#[tokio::test]
async fn list_workspaces_leaves_archived_ones_out_unless_asked() -> TestResult {
    let tenancy = Tenancy::new()?;
    let tenant = scope()?.tenant_id;
    tenancy
        .catalog
        .put(&tenant, workspace(11, "kept", 'a', true)?)?;
    tenancy
        .catalog
        .put(&tenant, workspace(12, "live", 'a', false)?)?;
    tenancy.access.grant(ALICE, id(11), EVERY.to_vec())?;
    tenancy.access.grant(ALICE, id(12), EVERY.to_vec())?;
    let caller = owner(OperationName::ListWorkspaces, None)?;
    for (asked, expected) in [
        (None, vec![id(12)]),
        (Some(false), vec![id(12)]),
        (Some(true), vec![id(11), id(12)]),
    ] {
        let listed = tenancy
            .service
            .list_workspaces(&caller, listing(50, None, asked))
            .await?;
        assert_eq!(ids(&listed), expected, "include_archived {asked:?}");
    }
    Ok(())
}

#[tokio::test]
async fn the_permissions_a_workspace_answers_follow_the_route_and_the_delegation_ceiling()
-> TestResult {
    let tenancy = Tenancy::new()?;
    let tenant = scope()?.tenant_id;
    tenancy
        .catalog
        .put(&tenant, workspace(1, "home", 'a', false)?)?;
    // A connector whose raw grant is everything, under a ceiling of read and write: an agent
    // route never writes or administers (`access::check_route`), and the ceiling takes the rest.
    let mut agent = principal("agent", AccessRoute::McpDelegation)?;
    agent.client_id = Some("connector".to_owned());
    agent.delegation = Some(DelegationCeiling {
        permissions: vec![Permission::Read, Permission::Write],
        workspace_ids: None,
    });
    tenancy.access.grant("agent", id(1), EVERY.to_vec())?;
    let listed = tenancy
        .service
        .list_workspaces(
            &context(
                agent.clone(),
                OperationName::ListWorkspaces,
                false,
                &[],
                None,
            )?,
            listing(50, None, None),
        )
        .await?;
    let only = some(listed.items.first(), "the readable workspace")?;
    assert_eq!(only.permissions, vec![Permission::Read]);
    // `open_workspace` applies the same rule to the grant dispatch looked up.
    let opened = tenancy
        .service
        .open_workspace(
            &context(agent, OperationName::OpenWorkspace, false, &EVERY, None)?,
            OpenWorkspaceRequest {
                workspace_id: id(1),
            },
        )
        .await?;
    assert_eq!(opened.permissions, vec![Permission::Read]);
    Ok(())
}

#[tokio::test]
async fn list_workspaces_pages_in_creation_order_and_resumes_after_the_last_listed() -> TestResult {
    let tenancy = five_workspaces()?;
    let caller = owner(OperationName::ListWorkspaces, None)?;
    let first = tenancy
        .service
        .list_workspaces(&caller, listing(2, None, None))
        .await?;
    assert_eq!(ids(&first), vec![id(11), id(12)]);
    let second = tenancy
        .service
        .list_workspaces(&caller, listing(2, first.next_cursor.clone(), None))
        .await?;
    assert_eq!(ids(&second), vec![id(13), id(14)]);
    let third = tenancy
        .service
        .list_workspaces(&caller, listing(2, second.next_cursor.clone(), None))
        .await?;
    assert_eq!(ids(&third), vec![id(15)]);
    assert_eq!(third.next_cursor, None, "the last page has no cursor");
    // A page that ends exactly at the last workspace has no cursor either.
    let whole = tenancy
        .service
        .list_workspaces(&caller, listing(5, None, None))
        .await?;
    assert_eq!(whole.items.len(), 5);
    assert_eq!(whole.next_cursor, None);
    Ok(())
}

#[tokio::test]
async fn a_listing_resumes_where_it_stood_when_a_workspace_is_archived_or_purged_between_pages()
-> TestResult {
    let tenancy = five_workspaces()?;
    let caller = owner(OperationName::ListWorkspaces, None)?;
    let first = tenancy
        .service
        .list_workspaces(&caller, listing(2, None, None))
        .await?;
    assert_eq!(ids(&first), vec![id(11), id(12)]);
    // The workspace the cursor names is archived: the next page still begins after it.
    tenancy
        .catalog
        .archive(
            &scope_of(12)?,
            mutation(90),
            WorkspaceArchive {
                expected_head: revision('a')?,
                author: uploader(),
            },
        )
        .await?;
    let second = tenancy
        .service
        .list_workspaces(&caller, listing(2, first.next_cursor.clone(), None))
        .await?;
    assert_eq!(ids(&second), vec![id(13), id(14)]);
    // The workspace the cursor names is purged, and one listed before it: the next page
    // begins where the entry after it stood, neither repeating nor skipping one.
    tenancy.catalog.purge(&scope_of(14)?)?;
    tenancy.catalog.purge(&scope_of(11)?)?;
    let third = tenancy
        .service
        .list_workspaces(&caller, listing(2, second.next_cursor.clone(), None))
        .await?;
    assert_eq!(ids(&third), vec![id(15)]);
    Ok(())
}

#[tokio::test]
async fn a_listing_cursor_this_operation_did_not_issue_is_refused() -> TestResult {
    let tenancy = five_workspaces()?;
    let caller = owner(OperationName::ListWorkspaces, None)?;
    let foreign = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&serde_json::json!({
        "op": "list_items",
        "after": id(11),
        "from": 1
    }))?);
    for cursor in ["not base64 json!".to_owned(), foreign] {
        let refused = err_of(
            tenancy
                .service
                .list_workspaces(&caller, listing(2, Some(cursor), None))
                .await,
        )?;
        assert_eq!(refused.code, ErrorCode::InvalidInput);
        assert_eq!(refused.field.as_deref(), Some("/page/cursor"));
    }
    Ok(())
}

#[tokio::test]
async fn a_grant_the_adapter_answers_for_another_tenant_fails_the_listing() -> TestResult {
    let tenancy = five_workspaces()?;
    tenancy.access.answer_for(StorageScope {
        tenant_id: okf_jawn_contract::identity::TenantId::try_from("elsewhere".to_owned())?,
        workspace_id: id(11),
    })?;
    let refused = err_of(
        tenancy
            .service
            .list_workspaces(
                &owner(OperationName::ListWorkspaces, None)?,
                listing(5, None, None),
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Internal);
    Ok(())
}

fn create_request() -> Built<CreateWorkspaceRequest> {
    Ok(CreateWorkspaceRequest {
        name: "Research".to_owned(),
        description: "Papers and notes".to_owned(),
        idempotency_key: key()?,
    })
}

#[tokio::test]
async fn create_workspace_creates_a_blank_workspace_named_from_its_mutation_and_grants_its_creator()
-> TestResult {
    let tenancy = Tenancy::new()?;
    let created = tenancy
        .service
        .create_workspace(
            &owner(OperationName::CreateWorkspace, Some(mutation(40)))?,
            create_request()?,
        )
        .await?;
    assert_eq!(created.id, derive_workspace_id(mutation(40)));
    assert_eq!(created.name, "Research");
    assert_eq!(created.description, "Papers and notes");
    assert_eq!(created.archived_at, None);
    assert_eq!(created.permissions, EVERY.to_vec(), "the creator's grant");
    // The grant is recorded: the creator now lists the workspace.
    let listed = tenancy
        .service
        .list_workspaces(
            &owner(OperationName::ListWorkspaces, None)?,
            listing(5, None, None),
        )
        .await?;
    assert_eq!(ids(&listed), vec![created.id]);
    Ok(())
}

#[tokio::test]
async fn a_retried_create_workspace_answers_the_same_workspace() -> TestResult {
    let tenancy = Tenancy::new()?;
    let first = tenancy
        .service
        .create_workspace(
            &owner(OperationName::CreateWorkspace, Some(mutation(40)))?,
            create_request()?,
        )
        .await?;
    let mut resumed = owner(OperationName::CreateWorkspace, Some(mutation(40)))?;
    resumed.attempt = Attempt::Resumed;
    let again = tenancy
        .service
        .create_workspace(&resumed, create_request()?)
        .await?;
    assert_eq!(again.id, first.id);
    let every = tenancy
        .service
        .list_workspaces(
            &owner(OperationName::ListWorkspaces, None)?,
            listing(5, None, None),
        )
        .await?;
    assert_eq!(ids(&every), vec![first.id], "one workspace");
    Ok(())
}

#[tokio::test]
async fn a_catalog_that_names_a_new_workspace_otherwise_than_its_mutation_is_refused() -> TestResult
{
    let tenancy = Tenancy::new()?;
    tenancy.catalog.misname_next_create();
    let refused = err_of(
        tenancy
            .service
            .create_workspace(
                &owner(OperationName::CreateWorkspace, Some(mutation(40)))?,
                create_request()?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Internal);
    Ok(())
}

#[tokio::test]
async fn a_creator_grant_for_another_workspace_is_refused() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy.access.answer_for(scope_of(99)?)?;
    let refused = err_of(
        tenancy
            .service
            .create_workspace(
                &owner(OperationName::CreateWorkspace, Some(mutation(40)))?,
                create_request()?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Internal);
    Ok(())
}

#[tokio::test]
async fn open_workspace_answers_an_archived_workspace_with_the_callers_permissions() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy
        .catalog
        .put(&scope()?.tenant_id, workspace(1, "home", 'a', true)?)?;
    let caller = context(
        principal(ALICE, AccessRoute::LocalOwner)?,
        OperationName::OpenWorkspace,
        false,
        &[Permission::Read, Permission::Write],
        None,
    )?;
    let opened = tenancy
        .service
        .open_workspace(
            &caller,
            OpenWorkspaceRequest {
                workspace_id: id(1),
            },
        )
        .await?;
    assert_eq!(opened.archived_at, Some(archived_instant()?));
    assert_eq!(
        opened.permissions,
        vec![Permission::Read, Permission::Write]
    );
    Ok(())
}

fn update_request(base: Revision) -> Built<UpdateWorkspaceRequest> {
    Ok(UpdateWorkspaceRequest {
        workspace_id: id(1),
        base_revision: base,
        name: "Renamed".to_owned(),
        description: "New purpose".to_owned(),
        idempotency_key: key()?,
    })
}

fn archive_request(base: Revision) -> Built<ArchiveWorkspaceRequest> {
    Ok(ArchiveWorkspaceRequest {
        workspace_id: id(1),
        base_revision: base,
        idempotency_key: key()?,
    })
}

/// A tenancy whose catalog holds the fixture workspace at head `revision('b')`.
fn at_head_b() -> Built<Tenancy> {
    let tenancy = Tenancy::new()?;
    tenancy
        .catalog
        .put(&scope()?.tenant_id, workspace(1, "home", 'b', false)?)?;
    Ok(tenancy)
}

#[tokio::test]
async fn update_workspace_changes_the_catalog_at_the_head_and_answers_the_head_with_a_receipt()
-> TestResult {
    let tenancy = at_head_b()?;
    let result = tenancy
        .service
        .update_workspace(
            &owner(OperationName::UpdateWorkspace, Some(mutation(41)))?,
            update_request(revision('b')?)?,
        )
        .await?;
    assert_eq!(
        result.revision,
        revision('b')?,
        "no commit: the head it was made at"
    );
    assert!(result.warnings.is_empty());
    let held = tenancy.catalog.held(&scope()?)?;
    assert_eq!(held.name, "Renamed");
    assert_eq!(held.description, "New purpose");
    let receipts = tenancy.world.records.receipts()?;
    let (under, receipt) = some(receipts.first(), "the receipt")?;
    assert_eq!(*under, Some(mutation(41)));
    assert_eq!(receipt.id, result.receipt_id);
    assert_eq!(receipt.operation_id, OperationName::UpdateWorkspace);
    assert!(tenancy.world.versions.commits()?.is_empty(), "no commit");
    assert!(tenancy.log.appended()?.is_empty(), "no event");
    Ok(())
}

#[tokio::test]
async fn a_metadata_change_on_a_stale_base_is_a_conflict_naming_the_head_on_base_revision()
-> TestResult {
    let tenancy = at_head_b()?;
    let refused = err_of(
        tenancy
            .service
            .update_workspace(
                &owner(OperationName::UpdateWorkspace, Some(mutation(41)))?,
                update_request(revision('c')?)?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert_eq!(refused.field.as_deref(), Some("/base_revision"));
    assert!(matches!(
        refused.detail.as_deref(),
        Some(ErrorDetail::StaleBase { head }) if *head == revision('b')?
    ));
    let refused = err_of(
        tenancy
            .service
            .archive_workspace(
                &owner(OperationName::ArchiveWorkspace, Some(mutation(42)))?,
                archive_request(revision('c')?)?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert_eq!(refused.field.as_deref(), Some("/base_revision"));
    let held = tenancy.catalog.held(&scope()?)?;
    assert_eq!(held.name, "home", "nothing changed");
    assert_eq!(held.archived_at, None);
    assert!(tenancy.world.records.receipts()?.is_empty(), "no receipt");
    Ok(())
}

#[tokio::test]
async fn a_metadata_change_on_a_purged_base_is_refused_before_the_catalog_is_asked() -> TestResult {
    let tenancy = at_head_b()?;
    tenancy.world.records.put_mapping(
        revision('b')?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(5)),
            replacement: Some(revision('c')?),
        },
    )?;
    let refused = err_of(
        tenancy
            .service
            .archive_workspace(
                &owner(OperationName::ArchiveWorkspace, Some(mutation(42)))?,
                archive_request(revision('b')?)?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert!(matches!(
        refused.detail.as_deref(),
        Some(ErrorDetail::Invalidated { .. })
    ));
    assert_eq!(tenancy.catalog.held(&scope()?)?.archived_at, None);
    Ok(())
}

#[tokio::test]
async fn archive_and_unarchive_set_and_clear_archived_at_and_answer_the_head() -> TestResult {
    let tenancy = at_head_b()?;
    let archived = tenancy
        .service
        .archive_workspace(
            &owner(OperationName::ArchiveWorkspace, Some(mutation(42)))?,
            archive_request(revision('b')?)?,
        )
        .await?;
    assert_eq!(archived.revision, revision('b')?);
    assert!(tenancy.catalog.held(&scope()?)?.archived_at.is_some());
    // The head moves; unarchive takes no base and answers the head it was made at.
    tenancy.catalog.set_head(&scope()?, revision('c')?)?;
    let unarchived = tenancy
        .service
        .unarchive_workspace(
            &owner(OperationName::UnarchiveWorkspace, Some(mutation(43)))?,
            UnarchiveWorkspaceRequest {
                workspace_id: id(1),
                idempotency_key: key()?,
            },
        )
        .await?;
    assert_eq!(unarchived.revision, revision('c')?);
    assert_eq!(tenancy.catalog.held(&scope()?)?.archived_at, None);
    assert_eq!(tenancy.world.records.receipts()?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_retried_metadata_change_answers_its_first_head_and_receipt() -> TestResult {
    let tenancy = at_head_b()?;
    let caller = owner(OperationName::UpdateWorkspace, Some(mutation(41)))?;
    let first = tenancy
        .service
        .update_workspace(&caller, update_request(revision('b')?)?)
        .await?;
    // Another write moves the head; the retry under the same mutation is the first result.
    tenancy.catalog.set_head(&scope()?, revision('c')?)?;
    let mut resumed = caller.clone();
    resumed.attempt = Attempt::Resumed;
    let again = tenancy
        .service
        .update_workspace(&resumed, update_request(revision('b')?)?)
        .await?;
    assert_eq!(again.revision, first.revision);
    assert_eq!(again.receipt_id, first.receipt_id);
    assert_eq!(tenancy.world.records.receipts()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn the_service_answers_dispatchs_archive_question_from_its_catalog() -> TestResult {
    let tenancy = Tenancy::new()?;
    let tenant = scope()?.tenant_id;
    tenancy
        .catalog
        .put(&tenant, workspace(11, "kept", 'a', true)?)?;
    tenancy
        .catalog
        .put(&tenant, workspace(12, "live", 'a', false)?)?;
    let service: &dyn Application = &tenancy.service;
    assert_eq!(
        service.archived_at(&scope_of(11)?).await?,
        Some(archived_instant()?)
    );
    assert_eq!(service.archived_at(&scope_of(12)?).await?, None);
    let unknown = err_of(service.archived_at(&scope_of(13)?).await)?;
    assert_eq!(unknown.code, ErrorCode::NotFound);
    assert_eq!(tenancy.catalog.opens(), 3, "one catalog read a question");
    Ok(())
}

fn export_request(at: At) -> Built<ExportWorkspaceRequest> {
    Ok(ExportWorkspaceRequest {
        workspace_id: id(1),
        at,
        include_history: true,
        idempotency_key: key()?,
    })
}

#[tokio::test]
async fn export_workspace_resolves_at_once_and_queues_its_job_with_that_revision() -> TestResult {
    let tenancy = Tenancy::new()?;
    let job = tenancy
        .service
        .export_workspace(
            &owner(OperationName::ExportWorkspace, Some(mutation(44)))?,
            export_request(At::Latest)?,
        )
        .await?;
    assert_eq!(job.kind, JobKind::ExportWorkspace);
    assert_eq!(job.workspace_id, Some(id(1)));
    let created = tenancy.jobs.created()?;
    let (job_scope, new_job, _) = some(created.first(), "the job")?;
    assert_eq!(*job_scope, JobScope::Workspace(scope()?));
    assert_eq!(new_job.mutation_id, mutation(44));
    assert_eq!(
        new_job.spec,
        JobSpec::ExportWorkspace {
            revision: revision('a')?,
            include_history: true,
        },
        "latest is the head when the request is accepted"
    );
    assert_eq!(new_job.initiator.subject, ALICE);
    assert_eq!(
        tenancy.queue.woken()?,
        vec![(JobScope::Workspace(scope()?), job.id)]
    );
    Ok(())
}

#[tokio::test]
async fn an_export_at_a_purged_revision_is_refused_and_queues_nothing() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy.world.records.put_mapping(
        revision('c')?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(5)),
            replacement: None,
        },
    )?;
    let refused = err_of(
        tenancy
            .service
            .export_workspace(
                &owner(OperationName::ExportWorkspace, Some(mutation(44)))?,
                export_request(At::Revision {
                    revision: revision('c')?,
                })?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert!(tenancy.jobs.created()?.is_empty());
    assert_eq!(tenancy.queue.woken()?, Vec::new());
    Ok(())
}

#[tokio::test]
async fn backup_workspace_and_backup_installation_queue_a_workspace_and_a_tenant_job() -> TestResult
{
    let tenancy = Tenancy::new()?;
    let backup = tenancy
        .service
        .backup_workspace(
            &owner(OperationName::BackupWorkspace, Some(mutation(45)))?,
            BackupWorkspaceRequest {
                workspace_id: id(1),
                idempotency_key: key()?,
            },
        )
        .await?;
    assert_eq!(backup.kind, JobKind::BackupWorkspace);
    assert_eq!(backup.workspace_id, Some(id(1)));
    let installation = tenancy
        .service
        .backup_installation(
            &owner(OperationName::BackupInstallation, Some(mutation(46)))?,
            BackupInstallationRequest {
                idempotency_key: key()?,
            },
        )
        .await?;
    assert_eq!(installation.kind, JobKind::BackupInstallation);
    assert_eq!(installation.workspace_id, None, "a tenant job");
    let created = tenancy.jobs.created()?;
    let scopes: Vec<(JobScope, JobSpec)> = created
        .iter()
        .map(|(job_scope, job, _)| (job_scope.clone(), job.spec.clone()))
        .collect();
    assert_eq!(
        scopes,
        vec![
            (JobScope::Workspace(scope()?), JobSpec::BackupWorkspace),
            (
                JobScope::Tenant(scope()?.tenant_id),
                JobSpec::BackupInstallation
            ),
        ]
    );
    assert_eq!(tenancy.queue.woken()?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_retried_job_start_answers_the_job_its_first_attempt_created() -> TestResult {
    let tenancy = Tenancy::new()?;
    let caller = owner(OperationName::ExportWorkspace, Some(mutation(44)))?;
    let first = tenancy
        .service
        .export_workspace(&caller, export_request(At::Latest)?)
        .await?;
    // The head moved: the retry resolves `latest` again, and the store answers the first job.
    tenancy.world.versions.set_head(revision('b')?)?;
    let mut resumed = caller.clone();
    resumed.attempt = Attempt::Resumed;
    let again = tenancy
        .service
        .export_workspace(&resumed, export_request(At::Latest)?)
        .await?;
    assert_eq!(again.id, first.id);
    assert_eq!(tenancy.jobs.created()?.len(), 1);
    // Waking twice is the queue's documented duplicate delivery.
    assert_eq!(tenancy.queue.woken()?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_failed_wake_up_fails_the_request_and_its_retry_answers_the_same_job() -> TestResult {
    let tenancy = Tenancy::new()?;
    let caller = owner(OperationName::BackupWorkspace, Some(mutation(45)))?;
    let request = || -> Built<BackupWorkspaceRequest> {
        Ok(BackupWorkspaceRequest {
            workspace_id: id(1),
            idempotency_key: key()?,
        })
    };
    tenancy.queue.fail_next();
    let failed = err_of(tenancy.service.backup_workspace(&caller, request()?).await)?;
    assert_eq!(failed.code, ErrorCode::Unavailable);
    let mut resumed = caller.clone();
    resumed.attempt = Attempt::Resumed;
    let job = tenancy
        .service
        .backup_workspace(&resumed, request()?)
        .await?;
    let created = tenancy.jobs.created()?;
    assert_eq!(created.len(), 1, "the job was accepted once");
    let (_, _, first) = some(created.first(), "the job")?;
    assert_eq!(job.id, first.id);
    assert_eq!(tenancy.queue.woken()?.len(), 1);
    Ok(())
}

/// A complete upload of the archive into the fixture workspace, of `sha` and consumed by
/// `consumed_by`; incomplete when `complete` is false.
fn upload(sha: &Digest, complete: bool, consumed_by: Option<JobId>) -> Built<UploadRecord> {
    Ok(UploadRecord {
        id: UploadId(Uuid::from_u128(UPLOAD)),
        filename: "workspace.okfj-workspace.zip".to_owned(),
        relative_path: String::new(),
        expected_size: 4096,
        expected_sha256: Some(sha.clone()),
        supplied_by: uploader(),
        created_at: instant()?,
        received_bytes: if complete { 4096 } else { 10 },
        object: complete.then(|| ObjectInfo {
            digest: sha.clone(),
            size: 4096,
        }),
        consumed_by,
    })
}

fn restore_request(sha: Digest) -> Built<RestoreWorkspaceRequest> {
    Ok(RestoreWorkspaceRequest {
        workspace_id: id(1),
        upload_id: UploadId(Uuid::from_u128(UPLOAD)),
        sha256: sha,
        idempotency_key: key()?,
    })
}

/// A blank target: its history is one commit, its head lists nothing; Bob and Alice may write.
fn blank_target(record: UploadRecord) -> Built<Tenancy> {
    target(record, vec![commit('a')?], None)
}

/// A restore target holding `record`, whose history from its head is `commits` and, when
/// `more` names a cursor, continues on another page; its head lists nothing; Bob and Alice
/// may write and Carol may only read.
fn target(record: UploadRecord, commits: Vec<Commit>, more: Option<&str>) -> Built<Tenancy> {
    let tenancy = Tenancy::new()?;
    tenancy.uploads.put(&scope()?, record)?;
    tenancy.world.versions.put_log(
        &revision('a')?,
        None,
        LogResponse {
            commits,
            next_cursor: more.map(str::to_owned),
        },
    )?;
    tenancy.world.versions.put_listing(
        &revision('a')?,
        None,
        FolderListing {
            items: Vec::new(),
            folders: Vec::new(),
            next_cursor: None,
        },
    )?;
    tenancy
        .access
        .grant("bob", id(1), vec![Permission::Write])?;
    tenancy
        .access
        .grant(ALICE, id(1), vec![Permission::Read, Permission::Write])?;
    tenancy
        .access
        .grant("carol", id(1), vec![Permission::Read])?;
    Ok(tenancy)
}

fn commit(fill: char) -> Built<Commit> {
    Ok(Commit {
        revision: revision(fill)?,
        parents: Vec::new(),
        message: "Create the workspace".to_owned(),
        author: "okf-jawn".to_owned(),
        committed_at: instant()?,
    })
}

fn restorer(attempt: Attempt) -> Built<OperationContext> {
    let mut caller = owner(OperationName::RestoreWorkspace, Some(mutation(47)))?;
    caller.attempt = attempt;
    Ok(caller)
}

#[tokio::test]
async fn restore_workspace_checks_the_upload_and_the_target_then_queues_the_job_with_the_editors()
-> TestResult {
    let sha = digest('d')?;
    let tenancy = blank_target(upload(&sha, true, None)?)?;
    let job = tenancy
        .service
        .restore_workspace(&restorer(Attempt::First)?, restore_request(sha.clone())?)
        .await?;
    assert_eq!(job.kind, JobKind::RestoreWorkspace);
    let created = tenancy.jobs.created()?;
    let (_, new_job, _) = some(created.first(), "the job")?;
    assert_eq!(
        new_job.spec,
        JobSpec::RestoreWorkspace {
            upload_id: UploadId(Uuid::from_u128(UPLOAD)),
            archive: sha,
            editors: vec![ALICE.to_owned(), "bob".to_owned()],
        },
        "the editors from AccessControl, sorted and without repeats"
    );
    assert_eq!(tenancy.queue.woken()?.len(), 1);
    Ok(())
}

/// Assert `refused` is a `Conflict` on `field`, and that nothing was queued.
fn refused_and_queued_nothing(
    tenancy: &Tenancy,
    refused: &okf_jawn_contract::error::ApiError,
    field: &str,
) -> TestResult {
    assert_eq!(refused.code, ErrorCode::Conflict, "{refused:?}");
    assert_eq!(refused.field.as_deref(), Some(field), "{refused:?}");
    assert!(tenancy.jobs.created()?.is_empty(), "no job");
    assert!(tenancy.queue.woken()?.is_empty(), "no wake-up");
    Ok(())
}

#[tokio::test]
async fn a_restore_refuses_an_incomplete_upload() -> TestResult {
    let sha = digest('d')?;
    let tenancy = blank_target(upload(&sha, false, None)?)?;
    let refused = err_of(
        tenancy
            .service
            .restore_workspace(&restorer(Attempt::First)?, restore_request(sha)?)
            .await,
    )?;
    refused_and_queued_nothing(&tenancy, &refused, "/upload_id")
}

#[tokio::test]
async fn a_restore_refuses_an_upload_whose_digest_is_not_the_requests() -> TestResult {
    let tenancy = blank_target(upload(&digest('d')?, true, None)?)?;
    let refused = err_of(
        tenancy
            .service
            .restore_workspace(&restorer(Attempt::First)?, restore_request(digest('e')?)?)
            .await,
    )?;
    refused_and_queued_nothing(&tenancy, &refused, "/sha256")
}

#[tokio::test]
async fn a_restore_refuses_an_upload_a_job_took() -> TestResult {
    let sha = digest('d')?;
    let taker = JobId(Uuid::from_u128(80));
    let tenancy = blank_target(upload(&sha, true, Some(taker))?)?;
    let refused = err_of(
        tenancy
            .service
            .restore_workspace(&restorer(Attempt::First)?, restore_request(sha)?)
            .await,
    )?;
    assert!(
        refused.message.contains(&taker.0.to_string()),
        "names the job"
    );
    refused_and_queued_nothing(&tenancy, &refused, "/upload_id")
}

#[tokio::test]
async fn a_restore_refuses_a_target_with_history_beyond_its_first_commit() -> TestResult {
    let sha = digest('d')?;
    for (commits, more) in [
        (vec![commit('a')?, commit('b')?], None),
        (vec![commit('a')?], Some("1")),
    ] {
        let tenancy = target(upload(&sha, true, None)?, commits, more)?;
        let refused = err_of(
            tenancy
                .service
                .restore_workspace(&restorer(Attempt::First)?, restore_request(sha.clone())?)
                .await,
        )?;
        refused_and_queued_nothing(&tenancy, &refused, "/workspace_id")?;
        let asked = tenancy.world.versions.log_queries()?;
        let query = some(asked.first(), "the history read")?;
        assert_eq!(
            query.page.limit, 2,
            "two entries tell the first commit from more"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_restore_refuses_a_target_that_holds_an_item_or_a_folder() -> TestResult {
    let sha = digest('d')?;
    let item = note(
        20,
        &revision('a')?,
        "notes/a.md",
        "Body",
        serde_json::json!({}),
    )?;
    for listed in [
        FolderListing {
            items: vec![item.summary.clone()],
            folders: Vec::new(),
            next_cursor: None,
        },
        FolderListing {
            items: Vec::new(),
            folders: vec![path("notes")?],
            next_cursor: None,
        },
        FolderListing {
            items: Vec::new(),
            folders: Vec::new(),
            next_cursor: Some("1".to_owned()),
        },
    ] {
        let tenancy = Tenancy::new()?;
        tenancy.uploads.put(&scope()?, upload(&sha, true, None)?)?;
        tenancy.world.versions.put_log(
            &revision('a')?,
            None,
            LogResponse {
                commits: vec![commit('a')?],
                next_cursor: None,
            },
        )?;
        tenancy
            .world
            .versions
            .put_listing(&revision('a')?, None, listed)?;
        let refused = err_of(
            tenancy
                .service
                .restore_workspace(&restorer(Attempt::First)?, restore_request(sha.clone())?)
                .await,
        )?;
        refused_and_queued_nothing(&tenancy, &refused, "/workspace_id")?;
    }
    Ok(())
}

#[tokio::test]
async fn a_restore_refuses_a_target_with_a_draft_of_an_editor_or_of_the_caller() -> TestResult {
    let sha = digest('d')?;
    for (editor, attempt_owner) in [("bob", "an editor"), ("dave", "the caller")] {
        let tenancy = blank_target(upload(&sha, true, None)?)?;
        tenancy
            .world
            .drafts
            .put_draft(draft_of(20, editor, &revision('a')?, "draft")?)?;
        let mut caller = restorer(Attempt::First)?;
        if editor == "dave" {
            caller.principal.subject = "dave".to_owned();
        }
        let refused = err_of(
            tenancy
                .service
                .restore_workspace(&caller, restore_request(sha.clone())?)
                .await,
        )?;
        assert!(
            refused.message.contains("draft"),
            "{attempt_owner}: {refused:?}"
        );
        refused_and_queued_nothing(&tenancy, &refused, "/workspace_id")?;
    }
    Ok(())
}

#[tokio::test]
async fn a_resumed_restore_whose_job_has_run_answers_that_job() -> TestResult {
    let sha = digest('d')?;
    let tenancy = blank_target(upload(&sha, true, None)?)?;
    let first = tenancy
        .service
        .restore_workspace(&restorer(Attempt::First)?, restore_request(sha.clone())?)
        .await?;
    // The job ran: it took the upload and wrote its restore commit under this mutation, so the
    // workspace is no longer blank.
    tenancy
        .uploads
        .put(&scope()?, upload(&sha, true, Some(first.id))?)?;
    restore_commit(&tenancy, mutation(47)).await?;
    // The restored head holds the archive's history: every check of a first attempt refuses it.
    tenancy.world.versions.put_log(
        &revision('f')?,
        None,
        LogResponse {
            commits: vec![commit('f')?, commit('e')?, commit('a')?],
            next_cursor: None,
        },
    )?;
    let refused = err_of(
        tenancy
            .service
            .restore_workspace(&restorer(Attempt::First)?, restore_request(sha.clone())?)
            .await,
    )?;
    assert_eq!(
        refused.code,
        ErrorCode::Conflict,
        "a first attempt is refused now"
    );
    let again = tenancy
        .service
        .restore_workspace(&restorer(Attempt::Resumed)?, restore_request(sha)?)
        .await?;
    assert_eq!(again.id, first.id);
    assert_eq!(tenancy.jobs.created()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_resumed_restore_without_its_restore_commit_is_checked_as_a_first_one() -> TestResult {
    let sha = digest('d')?;
    let taker = JobId(Uuid::from_u128(80));
    let tenancy = blank_target(upload(&sha, true, Some(taker))?)?;
    // A commit of another mutation is no evidence of this restore.
    restore_commit(&tenancy, mutation(99)).await?;
    let refused = err_of(
        tenancy
            .service
            .restore_workspace(&restorer(Attempt::Resumed)?, restore_request(sha)?)
            .await,
    )?;
    refused_and_queued_nothing(&tenancy, &refused, "/upload_id")
}

#[tokio::test]
async fn a_restore_of_an_upload_the_workspace_does_not_hold_is_not_found() -> TestResult {
    let sha = digest('d')?;
    let tenancy = Tenancy::new()?;
    tenancy
        .uploads
        .put(&scope_of(2)?, upload(&sha, true, None)?)?;
    let refused = err_of(
        tenancy
            .service
            .restore_workspace(&restorer(Attempt::First)?, restore_request(sha)?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert!(tenancy.jobs.created()?.is_empty());
    Ok(())
}

/// Write a commit carrying `mutation` on the fixture workspace's head, as a restore job's own
/// commit does (`Backups::restore_import`).
async fn restore_commit(tenancy: &Tenancy, mutation: MutationId) -> TestResult {
    tenancy.world.versions.queue_revision(revision('f')?)?;
    tenancy
        .world
        .versions
        .commit(
            &scope()?,
            CommitChanges {
                mutation_id: mutation,
                expected_head: revision('a')?,
                author: Provenance {
                    subject: "okf-jawn".to_owned(),
                    route: AccessRoute::Service,
                    client_id: None,
                },
                message: "Restore the workspace".to_owned(),
                edits: Vec::new(),
            },
            Arc::clone(&tenancy.service.conformance()),
        )
        .await?;
    Ok(())
}
