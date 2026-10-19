//! Job control: reading a workspace's jobs and the tenant's, and retrying and cancelling a
//! workspace's job (SPEC section 5: "Import is durable work: status, attempts, cancellation,
//! retry and reconciliation survive interruption").
//!
//! Reads. `get_job` and `list_jobs` read the jobs of the workspace (`JobScope::Workspace`), and
//! `get_tenant_job` and `list_tenant_jobs` those of the tenant dispatch authorized
//! (`JobScope::Tenant`). Which scope a job belongs to is the store's filter: a tenant job asked
//! through a workspace is `NotFound`, and so is a workspace's job asked at the tenant. Pages are
//! the store's, newest first, with its cursor.
//!
//! Control. `retry_job` and `cancel_job` hold `write` on the workspace (dispatch); they load the
//! job in the workspace, then require the permission of the operation that starts a job of its
//! kind, fresh from `AccessControl` (`access::authorize_job_kind`: `admin` for a backup, a
//! restore or an index rebuild, the first two only in a human session), whose `Forbidden`
//! dispatch records as it records every handler's. Tenant jobs are never controlled here (owner
//! decision 2026-10-08): the request names a workspace, and a tenant job is not found in it.
//! The transition is the store's, under the request's mutation and once per mutation
//! (`RecordStore::retry_job`, `RecordStore::cancel_job`), and its refusals are the answer: only
//! a failed job whose failure was recorded retryable is retried, a workspace a purge names
//! refuses a retry, and a sealed job ("passed its point of no return") or a finished one is not
//! cancelled (`Conflict`).
//!
//! The store does not deliver, so a retry wakes the worker (`JobQueue::enqueue`) on every
//! attempt, as a purge's resumption does (`purges::resume`); the queue accepts a duplicate
//! delivery, and a resumed attempt whose job ran and failed again since is answered the job as
//! it stands and still woken (a wake-up for a job that is not runnable runs nothing). A failed
//! wake-up fails the request; its retry answers the same job and wakes the worker again, and the
//! worker's own sweep re-delivers every pending job in any case. A cancellation wakes nothing: a
//! running handler sees it through `RecordStore::update_progress`.
//!
//! Each retry and cancellation the store answered is announced: `job_updated` for the job in the
//! workspace's log, appended under the request's mutation after the store's change and before
//! the wake-up, so the workspace's stream also shows a job no handler touches again (a queued job
//! cancelled). The log appends an identical notification once per mutation, so a resumed attempt
//! announces nothing twice; a failed append fails the request, and its retry appends it once.
//!
//! The archive rule is dispatch's (contract ruling 25): `cancel_job` runs while the workspace is
//! archived and `retry_job` is refused before it reaches this module; storage refuses to queue
//! an import, redigest or restore job of an archived workspace again in any case.

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::{JobId, MutationId};
use okf_jawn_contract::import::{
    CancelJobRequest, GetJobRequest, GetTenantJobRequest, Job, ListJobsRequest, ListJobsResponse,
    ListTenantJobsRequest, RetryJobRequest,
};

use super::ApplicationService;
use super::shared::{mutation_id, tenant_scope, workspace_scope};
use crate::access::authorize_job_kind;
use crate::context::OperationContext;
use crate::events::{EventScope, NewEvent};
use crate::jobs::JobScope;
use crate::storage::{Page, StorageScope};

/// Read one job of the workspace.
///
/// # Errors
/// Returns `NotFound` for a job the workspace does not hold (a tenant job among them), or any
/// port error.
pub(super) async fn get_job(
    service: &ApplicationService,
    context: &OperationContext,
    request: GetJobRequest,
) -> Result<Job, ApiError> {
    let scope = JobScope::Workspace(workspace_scope(context)?);
    service
        .ports()
        .records
        .get_job(&scope, request.job_id)
        .await
}

/// List the workspace's jobs, newest first, a page at a time.
///
/// # Errors
/// Returns the store's `InvalidInput` on `/page/cursor` for a cursor it did not write, or any
/// port error.
pub(super) async fn list_jobs(
    service: &ApplicationService,
    context: &OperationContext,
    request: ListJobsRequest,
) -> Result<ListJobsResponse, ApiError> {
    let scope = JobScope::Workspace(workspace_scope(context)?);
    service
        .ports()
        .records
        .list_jobs(&scope, Page::from(request.page))
        .await
}

/// Read one installation-level job of the tenant: an installation backup or a purge.
///
/// # Errors
/// Returns `NotFound` for a job the tenant does not hold at its level (a workspace's job among
/// them), `Internal` when dispatch authorized no tenant, or any port error.
pub(super) async fn get_tenant_job(
    service: &ApplicationService,
    context: &OperationContext,
    request: GetTenantJobRequest,
) -> Result<Job, ApiError> {
    let scope = JobScope::Tenant(tenant_scope(context)?);
    service
        .ports()
        .records
        .get_job(&scope, request.job_id)
        .await
}

/// List the tenant's installation-level jobs, newest first, a page at a time.
///
/// # Errors
/// Returns the store's `InvalidInput` on `/page/cursor` for a cursor it did not write,
/// `Internal` when dispatch authorized no tenant, or any port error.
pub(super) async fn list_tenant_jobs(
    service: &ApplicationService,
    context: &OperationContext,
    request: ListTenantJobsRequest,
) -> Result<ListJobsResponse, ApiError> {
    let scope = JobScope::Tenant(tenant_scope(context)?);
    service
        .ports()
        .records
        .list_jobs(&scope, Page::from(request.page))
        .await
}

/// Queue a failed job of the workspace again under its durable identity, announce it, and wake
/// the worker.
///
/// # Errors
/// Returns `NotFound` for a job the workspace does not hold, `Forbidden` without the permission
/// of the operation that started the job (or its human session), the store's `Conflict` for a
/// job it does not retry, or any port error: a failed announcement or wake-up among them.
pub(super) async fn retry_job(
    service: &ApplicationService,
    context: &OperationContext,
    request: RetryJobRequest,
) -> Result<Job, ApiError> {
    let (scope, mutation) = controlled(service, context, request.job_id).await?;
    let ports = service.ports();
    let jobs = JobScope::Workspace(scope.clone());
    let job = ports
        .records
        .retry_job(&jobs, mutation, request.job_id)
        .await?;
    announce(service, scope, mutation, request.job_id).await?;
    ports.queue.enqueue(jobs, request.job_id).await?;
    Ok(job)
}

/// Cancel a queued or running job of the workspace and announce it; what it recorded stays.
///
/// # Errors
/// Returns `NotFound` for a job the workspace does not hold (a tenant job among them),
/// `Forbidden` without the permission of the operation that started the job (or its human
/// session), the store's `Conflict` for a sealed or finished job, or any port error: a failed
/// announcement among them.
pub(super) async fn cancel_job(
    service: &ApplicationService,
    context: &OperationContext,
    request: CancelJobRequest,
) -> Result<Job, ApiError> {
    let (scope, mutation) = controlled(service, context, request.job_id).await?;
    let job = service
        .ports()
        .records
        .cancel_job(
            &JobScope::Workspace(scope.clone()),
            mutation,
            request.job_id,
        )
        .await?;
    announce(service, scope, mutation, request.job_id).await?;
    Ok(job)
}

/// The workspace and mutation a control of `job` runs under, once the job is loaded in the
/// workspace and the caller holds, fresh, the permission of the operation that started it.
///
/// # Errors
/// Returns `NotFound` for a job the workspace does not hold, the job-kind rule's `Forbidden`,
/// `Internal` without a workspace or a mutation, or any port error.
async fn controlled(
    service: &ApplicationService,
    context: &OperationContext,
    job: JobId,
) -> Result<(StorageScope, MutationId), ApiError> {
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    let ports = service.ports();
    let loaded = ports
        .records
        .get_job(&JobScope::Workspace(scope.clone()), job)
        .await?;
    authorize_job_kind(
        ports.access.as_ref(),
        &context.principal,
        scope.workspace_id,
        loaded.kind,
    )
    .await?;
    Ok((scope, mutation))
}

/// Append `job_updated` for `job` to the workspace's log under `mutation`; the log appends it
/// once per mutation.
///
/// # Errors
/// Returns the event log's error.
async fn announce(
    service: &ApplicationService,
    scope: StorageScope,
    mutation: MutationId,
    job: JobId,
) -> Result<(), ApiError> {
    service
        .ports()
        .events
        .append(
            &EventScope::Workspace(scope),
            Some(mutation),
            NewEvent {
                kind: EventKind::JobUpdated,
                revision: None,
                item_id: None,
                job_id: Some(job),
                connector_id: None,
                actor: None,
                operation: None,
                transport: None,
                proposal_id: None,
                count: None,
            },
        )
        .await?;
    Ok(())
}
