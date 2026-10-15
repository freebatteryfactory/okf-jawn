//! Workspaces: listing and opening them with the caller's permissions, creating one, its
//! metadata, archive and unarchive, and the four operations that start a job (a portable export,
//! a workspace backup, a restore into a blank workspace, an installation backup).
//!
//! Permissions. The catalog never filters and returns `Workspace::permissions` empty; the
//! answer carries the caller's effective permissions, from `AccessControl`: every permission of
//! the caller's grant that `access::authorize_workspace` admits for the caller's route and
//! delegation ceiling (SPEC section 11: list and open answer the caller's effective
//! permissions). `list_workspaces` lists, in creation order, only the workspaces whose
//! effective permissions include `read`, and archived ones only with `include_archived`. Its
//! cursor names the last workspace a page listed and the tenant's workspace that came after it,
//! so a workspace archived, unarchived or purged between two pages neither repeats an entry nor
//! skips one. Known limit: when both of those are purged before the next page, the listing
//! resumes at the place the second held, which skips as many entries as were purged before it.
//! `open_workspace` answers an archived workspace too, with its `archived_at`.
//!
//! Creation. `create_workspace` is the catalog's: blank, with no example content, named from
//! the mutation (`derive_workspace_id`), so a retry answers the same workspace; the creator's
//! grant is then recorded by the access adapter (`AccessControl::grant_creator`), and recording
//! it again on a retry changes nothing.
//!
//! Metadata is not a commit. `update_workspace` and `archive_workspace` take the request's
//! `base_revision`, which is checked against the revision map first and must be the head when
//! the catalog makes the change, in the catalog's own transaction: otherwise `Conflict` with
//! `ErrorDetail::StaleBase` naming the head, on `/base_revision`, so a person who decided on an
//! older state is told. `unarchive_workspace` takes no base. Each answers a `MutationResult`
//! whose `revision` is the head the catalog answered (no commit is written, so it is the head
//! the change was made at), with a receipt and no warnings. A retry under the same mutation
//! answers the catalog's first result and the first receipt.
//!
//! The archive rule is dispatch's (`dispatch`, contract ruling 25): a mutation of an archived
//! workspace that `operations::ALLOWED_WHILE_ARCHIVED` does not list is refused before its
//! handler, from the catalog entry `archived_at` reads. The two store guards are storage's: an
//! archive is refused while an import, redigest or restore job of the workspace is queued or
//! running, and no such job is accepted for an archived workspace.
//!
//! Jobs. Each job-starting operation records its job (`RecordStore::create_job`, unique on the
//! request's mutation, so a retry answers the job the first attempt created), then wakes the
//! worker (`JobQueue::enqueue`), and answers the queued `Job` (status 202). A failed wake-up
//! fails the request; its retry answers the same job and wakes the worker again, and the
//! worker's own sweep re-delivers every pending job in any case. `export_workspace` resolves `at`
//! once, at acceptance, and the job's specification carries that revision. Stated, not hidden:
//! ingest's handler does not run an export job yet (request R-I6), so an export job fails as
//! `NotImplemented`. `backup_workspace`, `restore_workspace` and `backup_installation` reach this
//! module only in a human session (dispatch's `HUMAN_SESSION_ONLY`).
//!
//! A restore is checked before its job is queued: the upload must be complete, its recorded
//! digest must equal the request's `sha256` (`Conflict` on `/sha256`) and it must not be
//! consumed (`Conflict` on `/upload_id`), and the target must be blank: its first commit only,
//! no item at its head and no draft of any of its editors or of the caller (`Conflict` on
//! `/workspace_id`). A workspace whose history is its first commit and holds no item never held
//! an item, so no draft of anyone can exist there; the drafts are asked anyway, of the editors
//! `AccessControl::editors` answers and of the caller, and a draft of a subject outside both is
//! not seen. The job's specification comes from `jobs::restore_job_spec`, with those editors.
//! The upload is consumed by the job when it starts (contract ruling 23), not here; and the job
//! itself refuses a target that is no longer blank and an upload another job took, so a write
//! that lands between these checks and the job is refused by the job. A resumed attempt whose
//! job already ran finds the workspace restored and the upload taken: when the restore's own
//! commit (it carries this mutation, `Backups::restore_import`) is on the head's history, the
//! job is answered as the first attempt created it. Known limit: a resumed attempt that comes
//! while its job has taken the upload and not yet committed is refused as `Conflict`, naming
//! the job that took the upload.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use okf_jawn_contract::access::{Permission, Principal};
use okf_jawn_contract::common::MutationResult;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{MutationId, Timestamp, WorkspaceId};
use okf_jawn_contract::import::Job;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::workspace::{
    ArchiveWorkspaceRequest, BackupInstallationRequest, BackupWorkspaceRequest,
    CreateWorkspaceRequest, ExportWorkspaceRequest, ListWorkspacesRequest, ListWorkspacesResponse,
    OpenWorkspaceRequest, RestoreWorkspaceRequest, UnarchiveWorkspaceRequest,
    UpdateWorkspaceRequest, Workspace,
};
use serde::{Deserialize, Serialize};

use super::ApplicationService;
use super::shared::{
    check_named, invalid, mutation_id, on_the_base, record_receipt, resolve, tenant_scope,
    workspace_scope,
};
use crate::access::authorize_workspace;
use crate::context::{Attempt, OperationContext, WorkspaceGrant};
use crate::jobs::{JobScope, JobSpec, NewJob, restore_job_spec};
use crate::storage::{
    LogQuery, NewWorkspace, Page, Provenance, StorageScope, WorkspaceArchive, WorkspaceUpdate,
    derive_workspace_id, workspace_with_permissions,
};

/// Where a workspace listing resumes; opaque to the caller (base64url of this JSON).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListingCursor {
    /// The operation the cursor continues: another operation's cursor is refused.
    op: String,
    /// The last workspace the earlier page listed.
    after: WorkspaceId,
    /// The tenant's workspace that came after `after` in creation order when the cursor was
    /// issued, listed or not; the listing resumes at it when `after` is gone (purged).
    next: Option<WorkspaceId>,
    /// Where `next` stood then; the listing resumes there when both are gone.
    from: usize,
}

/// The request field a listing cursor is refused on.
const PAGE_CURSOR: &str = "/page/cursor";

/// Most entries of a restore target's history read to tell its first commit from more.
const FIRST_COMMIT_ONLY: u16 = 2;

/// The workspaces the caller may read, in creation order, with the caller's effective
/// permissions; archived ones only with `include_archived`.
///
/// # Errors
/// Returns `InvalidInput` on `/page/cursor` for a cursor this operation did not issue,
/// `Internal` when the access adapter answers a grant of another tenant, or any port error.
pub(super) async fn list_workspaces(
    service: &ApplicationService,
    context: &OperationContext,
    request: ListWorkspacesRequest,
) -> Result<ListWorkspacesResponse, ApiError> {
    let principal = &context.principal;
    let ports = service.ports();
    let mut readable: BTreeMap<WorkspaceId, Vec<Permission>> = BTreeMap::new();
    for grant in ports.access.grants(principal).await? {
        if grant.scope.tenant_id != principal.tenant_id {
            return Err(foreign_grant());
        }
        let permissions = effective_permissions(principal, &grant);
        if permissions.contains(&Permission::Read) {
            readable.insert(grant.workspace_id(), permissions);
        }
    }
    let include_archived = request.include_archived.unwrap_or(false);
    // Every workspace, archived ones included, so a cursor's position does not depend on what
    // was archived or unarchived since; archived ones are left out below unless asked for.
    let every = ports.catalog.list(&principal.tenant_id, true).await?;
    let start = listing_start(context.operation, request.page.cursor.as_deref(), &every)?;
    let limit = usize::from(request.page.limit.max(1));
    let mut items: Vec<Workspace> = Vec::new();
    let mut last_position = start;
    let mut more = false;
    for (position, workspace) in every.iter().enumerate().skip(start) {
        let Some(permissions) = readable.get(&workspace.id) else {
            continue;
        };
        if workspace.archived_at.is_some() && !include_archived {
            continue;
        }
        if items.len() >= limit {
            more = true;
            break;
        }
        last_position = position;
        items.push(workspace_with_permissions(
            workspace.clone(),
            permissions.clone(),
        ));
    }
    let next_cursor = match items.last() {
        Some(last) if more => Some(listing_cursor(
            context.operation,
            last.id,
            &every,
            last_position,
        )?),
        _ => None,
    };
    Ok(ListWorkspacesResponse { items, next_cursor })
}

/// Create a blank workspace named from the request's mutation, and grant its creator.
///
/// # Errors
/// Returns `Internal` when the catalog names the workspace otherwise than
/// `derive_workspace_id` of the mutation, or the access adapter answers a grant of another
/// workspace or tenant; or any port error.
pub(super) async fn create_workspace(
    service: &ApplicationService,
    context: &OperationContext,
    request: CreateWorkspaceRequest,
) -> Result<Workspace, ApiError> {
    let mutation = mutation_id(context)?;
    let tenant = tenant_scope(context)?;
    let principal = &context.principal;
    let ports = service.ports();
    let created = ports
        .catalog
        .create(
            &tenant,
            mutation,
            NewWorkspace {
                name: request.name,
                description: request.description,
                creator: Provenance::from_principal(principal),
            },
        )
        .await?;
    // A retry answers the same workspace only because its identity comes from the mutation.
    if created.id != derive_workspace_id(mutation) {
        return Err(ApiError::new(
            ErrorCode::Internal,
            "the catalog named a new workspace otherwise than its mutation names it",
        ));
    }
    let grant = ports.access.grant_creator(principal, created.id).await?;
    if grant.workspace_id() != created.id || grant.scope.tenant_id != tenant {
        return Err(foreign_grant());
    }
    let permissions = effective_permissions(principal, &grant);
    Ok(workspace_with_permissions(created, permissions))
}

/// One workspace, archived or not, with the caller's effective permissions.
///
/// # Errors
/// Returns any port error: `NotFound` for a workspace the tenant does not hold.
pub(super) async fn open_workspace(
    service: &ApplicationService,
    context: &OperationContext,
    _request: OpenWorkspaceRequest,
) -> Result<Workspace, ApiError> {
    let scope = workspace_scope(context)?;
    let permissions = context
        .primary_workspace()
        .map(|grant| effective_permissions(&context.principal, grant))
        .unwrap_or_default();
    let workspace = service.ports().catalog.open(&scope).await?;
    Ok(workspace_with_permissions(workspace, permissions))
}

/// Rename and redescribe the workspace at the request's base revision.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated base revision, `Conflict` with
/// `ErrorDetail::StaleBase` on `/base_revision` when the head moved, or any port error.
pub(super) async fn update_workspace(
    service: &ApplicationService,
    context: &OperationContext,
    request: UpdateWorkspaceRequest,
) -> Result<MutationResult, ApiError> {
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    check_named(service, &scope, &request.base_revision).await?;
    let updated = service
        .ports()
        .catalog
        .update(
            &scope,
            mutation,
            WorkspaceUpdate {
                expected_head: request.base_revision,
                name: request.name,
                description: request.description,
                author: Provenance::from_principal(&context.principal),
            },
        )
        .await
        .map_err(on_the_base)?;
    metadata_result(service, context, &scope, mutation, updated).await
}

/// Archive the workspace at the request's base revision; its history and records stay.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated base revision, `Conflict` with
/// `ErrorDetail::StaleBase` on `/base_revision` when the head moved, the store's `Conflict`
/// while an import, redigest or restore job of the workspace is queued or running, or any port
/// error.
pub(super) async fn archive_workspace(
    service: &ApplicationService,
    context: &OperationContext,
    request: ArchiveWorkspaceRequest,
) -> Result<MutationResult, ApiError> {
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    check_named(service, &scope, &request.base_revision).await?;
    let archived = service
        .ports()
        .catalog
        .archive(
            &scope,
            mutation,
            WorkspaceArchive {
                expected_head: request.base_revision,
                author: Provenance::from_principal(&context.principal),
            },
        )
        .await
        .map_err(on_the_base)?;
    metadata_result(service, context, &scope, mutation, archived).await
}

/// Return an archived workspace to ordinary listings; one that is not archived is answered as
/// it is.
///
/// # Errors
/// Returns any port error.
pub(super) async fn unarchive_workspace(
    service: &ApplicationService,
    context: &OperationContext,
    _request: UnarchiveWorkspaceRequest,
) -> Result<MutationResult, ApiError> {
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    let unarchived = service
        .ports()
        .catalog
        .unarchive(
            &scope,
            mutation,
            Provenance::from_principal(&context.principal),
        )
        .await?;
    metadata_result(service, context, &scope, mutation, unarchived).await
}

/// Queue a portable export of the revision `at` names, resolved once now.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated revision, or any port error.
pub(super) async fn export_workspace(
    service: &ApplicationService,
    context: &OperationContext,
    request: ExportWorkspaceRequest,
) -> Result<Job, ApiError> {
    let scope = workspace_scope(context)?;
    let revision = resolve(service, &scope, &request.at).await?;
    let spec = JobSpec::ExportWorkspace {
        revision,
        include_history: request.include_history,
    };
    start_job(service, context, JobScope::Workspace(scope), spec).await
}

/// Queue a backup of the workspace into one self-contained archive.
///
/// # Errors
/// Returns any port error.
pub(super) async fn backup_workspace(
    service: &ApplicationService,
    context: &OperationContext,
    _request: BackupWorkspaceRequest,
) -> Result<Job, ApiError> {
    let scope = workspace_scope(context)?;
    start_job(
        service,
        context,
        JobScope::Workspace(scope),
        JobSpec::BackupWorkspace,
    )
    .await
}

/// Queue a restore of an uploaded workspace archive into this blank workspace.
///
/// # Errors
/// Returns `Conflict` on `/upload_id` for an upload that is not complete or that a job took,
/// on `/sha256` for an upload whose recorded digest is not the request's, on `/workspace_id`
/// for a target that is not blank; `Internal` when `jobs::restore_job_spec` answers another
/// kind; or any port error (`NotFound` for an upload the workspace does not hold).
pub(super) async fn restore_workspace(
    service: &ApplicationService,
    context: &OperationContext,
    request: RestoreWorkspaceRequest,
) -> Result<Job, ApiError> {
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    let ports = service.ports();
    let upload = ports.uploads.get(&scope, request.upload_id).await?;
    let Some(object) = upload.object.as_ref() else {
        return Err(conflict("the upload is not complete", "/upload_id"));
    };
    if object.digest != request.sha256 {
        return Err(conflict(
            "the uploaded archive does not have the SHA-256 the request names",
            "/sha256",
        ));
    }
    let spec = restore_job_spec(
        ports.access.as_ref(),
        &scope,
        request.upload_id,
        object.digest.clone(),
    )
    .await?;
    if context.attempt == Attempt::Resumed && restored_by(service, &scope, mutation).await? {
        // The job the first attempt created has restored the workspace: the store answers it.
        return start_job(service, context, JobScope::Workspace(scope), spec).await;
    }
    if let Some(job) = upload.consumed_by {
        return Err(conflict(
            format!("the upload was already taken by job {}", job.0),
            "/upload_id",
        ));
    }
    let JobSpec::RestoreWorkspace { editors, .. } = &spec else {
        return Err(ApiError::new(
            ErrorCode::Internal,
            "a restore's job specification is not a restore",
        ));
    };
    refuse_unless_blank(service, &scope, editors, &context.principal.subject).await?;
    start_job(service, context, JobScope::Workspace(scope), spec).await
}

/// Queue a backup of the installation's identities, tenant grants, connector records without
/// secrets, purge records and tenant events; no workspace content.
///
/// # Errors
/// Returns `Internal` when the request was authorized for no tenant, or any port error.
pub(super) async fn backup_installation(
    service: &ApplicationService,
    context: &OperationContext,
    _request: BackupInstallationRequest,
) -> Result<Job, ApiError> {
    let tenant = tenant_scope(context)?;
    start_job(
        service,
        context,
        JobScope::Tenant(tenant),
        JobSpec::BackupInstallation,
    )
    .await
}

/// When the workspace in `scope` was archived, from its catalog entry (`Application::archived_at`).
///
/// # Errors
/// Returns the catalog's error: `NotFound` for a workspace the tenant does not hold.
pub(super) async fn archived_at(
    service: &ApplicationService,
    scope: &StorageScope,
) -> Result<Option<Timestamp>, ApiError> {
    Ok(service.ports().catalog.open(scope).await?.archived_at)
}

/// The permissions of `grant` the caller can exercise: each one `access::authorize_workspace`
/// admits for `principal`, by its route rule and its delegation ceiling, in the grant's order.
fn effective_permissions(principal: &Principal, grant: &WorkspaceGrant) -> Vec<Permission> {
    grant
        .permissions
        .iter()
        .copied()
        .filter(|permission| authorize_workspace(principal, grant.clone(), *permission).is_ok())
        .collect()
}

/// The answer of a metadata change: the head the catalog answered, a receipt under the
/// mutation, no warnings.
///
/// # Errors
/// Returns any error of recording the receipt.
async fn metadata_result(
    service: &ApplicationService,
    context: &OperationContext,
    scope: &StorageScope,
    mutation: MutationId,
    workspace: Workspace,
) -> Result<MutationResult, ApiError> {
    let receipt_id = record_receipt(service, context, scope, Some(mutation), Vec::new()).await?;
    Ok(MutationResult {
        revision: workspace.head,
        receipt_id,
        warnings: Vec::new(),
    })
}

/// Record the job of this request under its mutation, wake the worker, and answer the job.
///
/// # Errors
/// Returns any port error.
async fn start_job(
    service: &ApplicationService,
    context: &OperationContext,
    scope: JobScope,
    spec: JobSpec,
) -> Result<Job, ApiError> {
    let ports = service.ports();
    let job = ports
        .records
        .create_job(
            &scope,
            NewJob {
                mutation_id: mutation_id(context)?,
                initiator: Provenance::from_principal(&context.principal),
                spec,
            },
        )
        .await?;
    ports.queue.enqueue(scope, job.id).await?;
    Ok(job)
}

/// Whether the restore commit of `mutation` is on the head's history: the job this mutation
/// started has restored the workspace.
///
/// # Errors
/// Returns any port error.
async fn restored_by(
    service: &ApplicationService,
    scope: &StorageScope,
    mutation: MutationId,
) -> Result<bool, ApiError> {
    Ok(service
        .ports()
        .versions
        .find_commit(scope, mutation, None)
        .await?
        .is_some())
}

/// Refuse a restore target that is not blank: more than its first commit, an item at its head,
/// or a draft of one of `editors` or of `caller`.
///
/// # Errors
/// Returns `Conflict` on `/workspace_id` for a target that is not blank, or any port error.
async fn refuse_unless_blank(
    service: &ApplicationService,
    scope: &StorageScope,
    editors: &[String],
    caller: &str,
) -> Result<(), ApiError> {
    let ports = service.ports();
    let head = ports.versions.head(scope).await?;
    let history = ports
        .versions
        .log(
            scope,
            LogQuery {
                tip: head.clone(),
                item_id: None,
                page: Page {
                    cursor: None,
                    limit: FIRST_COMMIT_ONLY,
                },
            },
        )
        .await?;
    if history.commits.len() > 1 || history.next_cursor.is_some() {
        return Err(not_blank("it has history beyond its first commit"));
    }
    let listing = ports
        .versions
        .list(
            scope,
            &head,
            None,
            Page {
                cursor: None,
                limit: 1,
            },
        )
        .await?;
    if !listing.items.is_empty() || !listing.folders.is_empty() || listing.next_cursor.is_some() {
        return Err(not_blank("it holds items"));
    }
    for editor in editors.iter().map(String::as_str).chain([caller]) {
        if !ports.drafts.list(scope, editor).await?.is_empty() {
            return Err(not_blank("it holds drafts"));
        }
    }
    Ok(())
}

/// Where a listing begins: at the start without a cursor; after the workspace a cursor names;
/// at the workspace that followed it when that one is gone; where that one stood when both are.
///
/// # Errors
/// Returns `InvalidInput` on `/page/cursor` for a cursor that is not base64url JSON of this
/// shape or that another operation issued.
fn listing_start(
    operation: OperationName,
    cursor: Option<&str>,
    every: &[Workspace],
) -> Result<usize, ApiError> {
    let Some(text) = cursor else {
        return Ok(0);
    };
    let cursor: ListingCursor = URL_SAFE_NO_PAD
        .decode(text)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| invalid("the cursor is not one this operation issues", PAGE_CURSOR))?;
    if cursor.op != operation.as_str() {
        return Err(invalid(
            "the cursor was issued by another operation",
            PAGE_CURSOR,
        ));
    }
    let at = |wanted: WorkspaceId| every.iter().position(|workspace| workspace.id == wanted);
    if let Some(found) = at(cursor.after) {
        return Ok(found.saturating_add(1));
    }
    Ok(cursor
        .next
        .and_then(at)
        .unwrap_or_else(|| cursor.from.min(every.len())))
}

/// The cursor of the page after one that ended with `after`, at `position` of `every`.
///
/// # Errors
/// Returns `Internal` when the cursor cannot be serialized.
fn listing_cursor(
    operation: OperationName,
    after: WorkspaceId,
    every: &[Workspace],
    position: usize,
) -> Result<String, ApiError> {
    let from = position.saturating_add(1);
    serde_json::to_vec(&ListingCursor {
        op: operation.as_str().to_owned(),
        after,
        next: every.get(from).map(|workspace| workspace.id),
        from,
    })
    .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
    .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
}

/// The refusal of a grant the access adapter answered for another workspace or tenant.
fn foreign_grant() -> ApiError {
    ApiError::new(
        ErrorCode::Internal,
        "Access adapter returned a grant for a different scope",
    )
}

/// A `Conflict` naming the request field at `field`.
fn conflict(message: impl Into<String>, field: &str) -> ApiError {
    ApiError::new(ErrorCode::Conflict, message).with_field(field)
}

/// The refusal of a restore target that is not blank.
fn not_blank(why: &str) -> ApiError {
    conflict(
        format!("a restore fills only a blank workspace, and {why}"),
        "/workspace_id",
    )
}
