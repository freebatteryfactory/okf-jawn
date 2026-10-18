//! Job control over the tenancy fakes. `get_job` and `list_jobs` read the workspace's jobs and
//! `get_tenant_job` and `list_tenant_jobs` the tenant's, each by the store's scope. `retry_job`
//! and `cancel_job` load the job in the workspace, apply the job-kind rule
//! (`access::authorize_job_kind`) fresh, and ask the store under the request's mutation; a retry
//! then wakes the worker on every attempt, and both announce the job (`job_updated`) once per
//! mutation. Through dispatch over the same service: a job-kind refusal is recorded as
//! `permission_denied`, and the archive rule lets a cancellation through and refuses a retry.

use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::common::PageRequest;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::{IdempotencyKey, JobId, MutationId, TenantId, WorkspaceId};
use okf_jawn_contract::import::{
    CancelJobRequest, GetJobRequest, GetTenantJobRequest, Job, JobKind, JobState, ListJobsRequest,
    ListJobsResponse, ListTenantJobsRequest, RetryJobRequest,
};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::context::{Attempt, OperationContext, TenantGrant};
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::events::{EventScope, NewEvent};
use okf_jawn_core::jobs::{JobScope, RecordStore};
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::StorageScope;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{ALICE, Built, context, instant, mutation, scope};
use crate::support::FixtureMutations;
use crate::tenancy::{Tenancy, archived_instant, workspace};

/// Every permission a grant can hold.
const EVERY: [Permission; 6] = [
    Permission::Read,
    Permission::Write,
    Permission::Propose,
    Permission::Approve,
    Permission::Review,
    Permission::Admin,
];

fn key() -> Built<IdempotencyKey> {
    Ok(serde_json::from_value(json!(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    ))?)
}

fn id(number: u128) -> JobId {
    JobId(Uuid::from_u128(number))
}

fn workspace_id() -> Built<WorkspaceId> {
    Ok(scope()?.workspace_id)
}

/// The fixture workspace's jobs.
fn here() -> Built<JobScope> {
    Ok(JobScope::Workspace(scope()?))
}

/// The fixture tenant's jobs.
fn tenant_jobs() -> Built<JobScope> {
    Ok(JobScope::Tenant(scope()?.tenant_id))
}

/// `subject` on `route` running `operation` in the fixture workspace, under `number`'s
/// mutation when it has one.
fn caller(
    subject: &str,
    route: AccessRoute,
    operation: OperationName,
    number: Option<u128>,
) -> Built<OperationContext> {
    context(operation, subject, route, number.map(mutation))
}

/// Alice's local session running `operation`, under `number`'s mutation when it has one.
fn alice(operation: OperationName, number: Option<u128>) -> Built<OperationContext> {
    caller(ALICE, AccessRoute::LocalOwner, operation, number)
}

/// Alice running a tenant operation: dispatch authorized it at the tenant, at no workspace.
fn at_the_tenant(operation: OperationName) -> Built<OperationContext> {
    let mut context = alice(operation, None)?;
    context.tenant = Some(TenantGrant {
        tenant_id: scope()?.tenant_id,
        permissions: vec![Permission::Admin],
    });
    context.grants = Vec::new();
    Ok(context)
}

/// A job numbered `number` in `state`; a failed one failed retryable, as a store records it.
fn job(number: u128, kind: JobKind, state: JobState, workspace: Option<WorkspaceId>) -> Built<Job> {
    let failed = state == JobState::Failed;
    Ok(Job {
        id: id(number),
        workspace_id: workspace,
        kind,
        state,
        progress: 0,
        attempt: 1,
        warnings: Vec::new(),
        error: failed.then(|| ApiError::new(ErrorCode::Unavailable, "the converter stopped")),
        revision: None,
        item_ids: Vec::new(),
        artifact: None,
        restore: None,
        backup: None,
        created_at: Some(instant()?),
        started_at: None,
        finished_at: failed.then(instant).transpose()?,
        retryable: failed.then_some(true),
    })
}

/// Hold a job of the fixture workspace numbered `number`, of `kind`, in `state`.
fn put(tenancy: &Tenancy, number: u128, kind: JobKind, state: JobState) -> Built<JobId> {
    tenancy.jobs.put_job(
        &here()?,
        job(number, kind, state, Some(workspace_id()?))?,
        false,
    )?;
    Ok(id(number))
}

fn retry(job: JobId) -> Built<RetryJobRequest> {
    Ok(RetryJobRequest {
        workspace_id: workspace_id()?,
        job_id: job,
        idempotency_key: key()?,
    })
}

fn cancel(job: JobId) -> Built<CancelJobRequest> {
    Ok(CancelJobRequest {
        workspace_id: workspace_id()?,
        job_id: job,
        idempotency_key: key()?,
    })
}

/// The `job_updated` of `job` in the fixture workspace's log, under `number`'s mutation.
fn announced(job: JobId, number: u128) -> Built<(EventScope, Option<MutationId>, NewEvent)> {
    Ok((
        EventScope::Workspace(scope()?),
        Some(mutation(number)),
        NewEvent {
            kind: EventKind::JobUpdated,
            revision: None,
            item_id: None,
            job_id: Some(job),
            connector_id: None,
            actor: None,
            operation: None,
            proposal_id: None,
            count: None,
        },
    ))
}

fn ids(response: &ListJobsResponse) -> Vec<JobId> {
    response.items.iter().map(|job| job.id).collect()
}

/// Nothing was woken and nothing announced.
fn assert_quiet(tenancy: &Tenancy) -> TestResult {
    assert_eq!(tenancy.queue.woken()?, Vec::new(), "no wake-up");
    assert_eq!(tenancy.log.appended()?, Vec::new(), "no event");
    Ok(())
}

#[tokio::test]
async fn get_job_and_list_jobs_read_only_the_workspaces_jobs() -> TestResult {
    let tenancy = Tenancy::new()?;
    let elsewhere = StorageScope {
        tenant_id: scope()?.tenant_id,
        workspace_id: WorkspaceId(Uuid::from_u128(2)),
    };
    put(&tenancy, 41, JobKind::Import, JobState::Queued)?;
    tenancy.jobs.put_job(
        &tenant_jobs()?,
        job(42, JobKind::BackupInstallation, JobState::Succeeded, None)?,
        false,
    )?;
    tenancy.jobs.put_job(
        &JobScope::Workspace(elsewhere.clone()),
        job(
            43,
            JobKind::Import,
            JobState::Queued,
            Some(elsewhere.workspace_id),
        )?,
        false,
    )?;
    put(&tenancy, 44, JobKind::ExportWorkspace, JobState::Failed)?;
    let reader = alice(OperationName::GetJob, None)?;
    let get = |job| -> Built<GetJobRequest> {
        Ok(GetJobRequest {
            workspace_id: workspace_id()?,
            job_id: job,
        })
    };
    let read = tenancy.service.get_job(&reader, get(id(41))?).await?;
    assert_eq!((read.id, read.state), (id(41), JobState::Queued));
    // A tenant job asked through the workspace, and another workspace's job, are not found.
    for hidden in [42, 43] {
        let refused = err_of(tenancy.service.get_job(&reader, get(id(hidden))?).await)?;
        assert_eq!(refused.code, ErrorCode::NotFound, "{hidden}");
    }
    let lister = alice(OperationName::ListJobs, None)?;
    let list = |limit, cursor| -> Built<ListJobsRequest> {
        Ok(ListJobsRequest {
            workspace_id: workspace_id()?,
            page: PageRequest { cursor, limit },
        })
    };
    let page = tenancy.service.list_jobs(&lister, list(1, None)?).await?;
    assert_eq!(ids(&page), vec![id(44)], "newest first");
    let cursor = some(page.next_cursor, "the cursor")?;
    let rest = tenancy
        .service
        .list_jobs(&lister, list(1, Some(cursor))?)
        .await?;
    assert_eq!(ids(&rest), vec![id(41)]);
    assert_eq!(rest.next_cursor, None);
    let refused = err_of(
        tenancy
            .service
            .list_jobs(&lister, list(1, Some("not-a-position".to_owned()))?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/page/cursor"));
    Ok(())
}

#[tokio::test]
async fn get_tenant_job_and_list_tenant_jobs_read_only_the_tenants_jobs() -> TestResult {
    let tenancy = Tenancy::new()?;
    put(&tenancy, 41, JobKind::Import, JobState::Queued)?;
    tenancy.jobs.put_job(
        &tenant_jobs()?,
        job(42, JobKind::BackupInstallation, JobState::Succeeded, None)?,
        false,
    )?;
    tenancy.jobs.put_job(
        &JobScope::Tenant(TenantId::try_from("elsewhere".to_owned())?),
        job(45, JobKind::PurgeWorkspace, JobState::Queued, None)?,
        false,
    )?;
    let reader = at_the_tenant(OperationName::GetTenantJob)?;
    let read = tenancy
        .service
        .get_tenant_job(&reader, GetTenantJobRequest { job_id: id(42) })
        .await?;
    assert_eq!(read.id, id(42));
    // A workspace's job asked at the tenant, and another tenant's job, are not found.
    for hidden in [41, 45] {
        let refused = err_of(
            tenancy
                .service
                .get_tenant_job(&reader, GetTenantJobRequest { job_id: id(hidden) })
                .await,
        )?;
        assert_eq!(refused.code, ErrorCode::NotFound, "{hidden}");
    }
    let listed = tenancy
        .service
        .list_tenant_jobs(
            &at_the_tenant(OperationName::ListTenantJobs)?,
            ListTenantJobsRequest {
                page: PageRequest {
                    cursor: None,
                    limit: 50,
                },
            },
        )
        .await?;
    assert_eq!(ids(&listed), vec![id(42)]);
    Ok(())
}

#[tokio::test]
async fn retry_job_queues_a_failed_job_again_wakes_the_worker_and_announces_it() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy.access.grant(
        ALICE,
        workspace_id()?,
        vec![Permission::Read, Permission::Write],
    )?;
    let failed = put(&tenancy, 51, JobKind::Import, JobState::Failed)?;
    let retried = tenancy
        .service
        .retry_job(&alice(OperationName::RetryJob, Some(61))?, retry(failed)?)
        .await?;
    assert_eq!((retried.id, retried.state), (failed, JobState::Queued));
    assert!(retried.error.is_none() && retried.retryable.is_none());
    assert_eq!(tenancy.jobs.job(failed)?.state, JobState::Queued);
    assert_eq!(tenancy.queue.woken()?, vec![(here()?, failed)]);
    assert_eq!(tenancy.log.appended()?, vec![announced(failed, 61)?]);
    assert_eq!(
        tenancy.access.lookups()?,
        vec![Permission::Write],
        "the permission of start_import, looked up for the job's kind"
    );
    Ok(())
}

/// The job-kind rule after the job is loaded: a backup job needs `admin` and a human session,
/// for its retry and its cancellation, looked up fresh; a writer and a service holding `admin`
/// are refused, and an administrator in a browser session is not.
#[tokio::test]
async fn retry_job_and_cancel_job_need_the_permission_of_the_operation_that_started_the_job()
-> TestResult {
    let tenancy = Tenancy::new()?;
    let workspace = workspace_id()?;
    tenancy
        .access
        .grant("bob", workspace, vec![Permission::Read, Permission::Write])?;
    tenancy.access.grant("carol", workspace, EVERY.to_vec())?;
    let failed = put(&tenancy, 52, JobKind::BackupWorkspace, JobState::Failed)?;
    let queued = put(&tenancy, 53, JobKind::BackupWorkspace, JobState::Queued)?;
    for (subject, route) in [
        ("bob", AccessRoute::BrowserSession),
        ("carol", AccessRoute::Service),
    ] {
        let refused = err_of(
            tenancy
                .service
                .retry_job(
                    &caller(subject, route.clone(), OperationName::RetryJob, Some(61))?,
                    retry(failed)?,
                )
                .await,
        )?;
        assert_eq!(refused.code, ErrorCode::Forbidden, "{subject}");
        let refused = err_of(
            tenancy
                .service
                .cancel_job(
                    &caller(subject, route, OperationName::CancelJob, Some(62))?,
                    cancel(queued)?,
                )
                .await,
        )?;
        assert_eq!(refused.code, ErrorCode::Forbidden, "{subject}");
    }
    assert_eq!(tenancy.jobs.job(failed)?.state, JobState::Failed);
    assert_eq!(tenancy.jobs.job(queued)?.state, JobState::Queued);
    assert_quiet(&tenancy)?;
    let admin = |operation, number| {
        caller(
            "carol",
            AccessRoute::BrowserSession,
            operation,
            Some(number),
        )
    };
    let retried = tenancy
        .service
        .retry_job(&admin(OperationName::RetryJob, 63)?, retry(failed)?)
        .await?;
    assert_eq!(retried.state, JobState::Queued);
    let cancelled = tenancy
        .service
        .cancel_job(&admin(OperationName::CancelJob, 64)?, cancel(queued)?)
        .await?;
    assert_eq!(cancelled.state, JobState::Cancelled);
    // The service is refused by its route before any lookup; each lookup asks for `admin`.
    assert_eq!(tenancy.access.lookups()?, vec![Permission::Admin; 4]);
    Ok(())
}

#[tokio::test]
async fn a_retry_the_store_refuses_wakes_nothing_and_announces_nothing() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy
        .access
        .grant(ALICE, workspace_id()?, EVERY.to_vec())?;
    let mut not_retryable = job(54, JobKind::Import, JobState::Failed, Some(workspace_id()?))?;
    not_retryable.retryable = Some(false);
    tenancy.jobs.put_job(&here()?, not_retryable, false)?;
    let queued = put(&tenancy, 55, JobKind::Import, JobState::Queued)?;
    let succeeded = put(&tenancy, 56, JobKind::Import, JobState::Succeeded)?;
    for (number, job) in [(61, id(54)), (62, queued), (63, succeeded)] {
        let refused = err_of(
            tenancy
                .service
                .retry_job(&alice(OperationName::RetryJob, Some(number))?, retry(job)?)
                .await,
        )?;
        assert_eq!(refused.code, ErrorCode::Conflict, "{number}");
    }
    assert_eq!(tenancy.jobs.job(id(54))?.state, JobState::Failed);
    assert_quiet(&tenancy)
}

/// A wake-up that failed fails the request, though the store already queued the job; the
/// request's retry answers the same job, wakes the worker and announces the job once.
#[tokio::test]
async fn a_retry_whose_wake_up_failed_is_completed_by_its_retry_under_the_same_mutation()
-> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy
        .access
        .grant(ALICE, workspace_id()?, EVERY.to_vec())?;
    let failed = put(&tenancy, 57, JobKind::Import, JobState::Failed)?;
    tenancy.queue.fail_next();
    let first = alice(OperationName::RetryJob, Some(61))?;
    let refused = err_of(tenancy.service.retry_job(&first, retry(failed)?).await)?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(tenancy.jobs.job(failed)?.state, JobState::Queued);
    let mut resumed = first;
    resumed.attempt = Attempt::Resumed;
    let again = tenancy.service.retry_job(&resumed, retry(failed)?).await?;
    assert_eq!((again.id, again.state), (failed, JobState::Queued));
    assert_eq!(tenancy.queue.woken()?, vec![(here()?, failed)]);
    assert_eq!(tenancy.log.appended()?, vec![announced(failed, 61)?]);
    Ok(())
}

/// The store does not deliver, so every attempt wakes the worker, even one that finds the job
/// failed again since (the store answers it as it stands); the announcement is made once.
#[tokio::test]
async fn a_resumed_retry_wakes_the_worker_again_and_announces_once() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy
        .access
        .grant(ALICE, workspace_id()?, EVERY.to_vec())?;
    let failed = put(&tenancy, 58, JobKind::Redigest, JobState::Failed)?;
    let first = alice(OperationName::RetryJob, Some(61))?;
    tenancy.service.retry_job(&first, retry(failed)?).await?;
    // The worker ran it and it failed again.
    put(&tenancy, 58, JobKind::Redigest, JobState::Failed)?;
    let mut resumed = first;
    resumed.attempt = Attempt::Resumed;
    let again = tenancy.service.retry_job(&resumed, retry(failed)?).await?;
    assert_eq!(again.state, JobState::Failed, "the job as it stands");
    assert_eq!(tenancy.queue.woken()?, vec![(here()?, failed); 2]);
    assert_eq!(tenancy.log.appended()?, vec![announced(failed, 61)?]);
    Ok(())
}

/// An announcement that failed fails the request; its retry announces the job once.
#[tokio::test]
async fn a_retry_whose_announcement_failed_announces_once_on_its_retry() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy
        .access
        .grant(ALICE, workspace_id()?, EVERY.to_vec())?;
    let failed = put(&tenancy, 59, JobKind::Import, JobState::Failed)?;
    tenancy.log.fail_next();
    let first = alice(OperationName::RetryJob, Some(61))?;
    let refused = err_of(tenancy.service.retry_job(&first, retry(failed)?).await)?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    let mut resumed = first;
    resumed.attempt = Attempt::Resumed;
    tenancy.service.retry_job(&resumed, retry(failed)?).await?;
    assert_eq!(tenancy.log.appended()?, vec![announced(failed, 61)?]);
    assert_eq!(
        tenancy.queue.woken()?.last(),
        Some(&(here()?, failed)),
        "the worker is woken"
    );
    Ok(())
}

#[tokio::test]
async fn cancel_job_cancels_queued_or_running_work_and_announces_it() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy.access.grant(
        ALICE,
        workspace_id()?,
        vec![Permission::Read, Permission::Write],
    )?;
    let queued = put(&tenancy, 71, JobKind::Import, JobState::Queued)?;
    let running = put(&tenancy, 72, JobKind::Redigest, JobState::Running)?;
    for (number, job) in [(81, queued), (82, running)] {
        let cancelled = tenancy
            .service
            .cancel_job(
                &alice(OperationName::CancelJob, Some(number))?,
                cancel(job)?,
            )
            .await?;
        assert_eq!((cancelled.id, cancelled.state), (job, JobState::Cancelled));
        assert!(cancelled.finished_at.is_some());
        assert_eq!(tenancy.jobs.job(job)?.state, JobState::Cancelled);
    }
    assert_eq!(
        tenancy.queue.woken()?,
        Vec::new(),
        "a cancellation wakes nothing"
    );
    assert_eq!(
        tenancy.log.appended()?,
        vec![announced(queued, 81)?, announced(running, 82)?]
    );
    assert_eq!(
        tenancy.access.lookups()?,
        vec![Permission::Write; 2],
        "the permission of start_import and of redigest_item"
    );
    Ok(())
}

/// The store's refusals are the answer as it gave them: a sealed job has passed its point of
/// no return, and a finished job cannot be cancelled.
#[tokio::test]
async fn cancel_job_answers_the_stores_refusal_of_a_sealed_or_finished_job() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy
        .access
        .grant(ALICE, workspace_id()?, EVERY.to_vec())?;
    tenancy.jobs.put_job(
        &here()?,
        job(
            73,
            JobKind::RestoreWorkspace,
            JobState::Running,
            Some(workspace_id()?),
        )?,
        true,
    )?;
    let succeeded = put(&tenancy, 74, JobKind::Import, JobState::Succeeded)?;
    let failed = put(&tenancy, 75, JobKind::Import, JobState::Failed)?;
    let cancelled = put(&tenancy, 76, JobKind::Import, JobState::Cancelled)?;
    for (number, job, state) in [
        (81, id(73), JobState::Running),
        (82, succeeded, JobState::Succeeded),
        (83, failed, JobState::Failed),
        (84, cancelled, JobState::Cancelled),
    ] {
        let refused = err_of(
            tenancy
                .service
                .cancel_job(
                    &alice(OperationName::CancelJob, Some(number))?,
                    cancel(job)?,
                )
                .await,
        )?;
        let direct = err_of(
            tenancy
                .jobs
                .cancel_job(&here()?, mutation(number), job)
                .await,
        )?;
        assert_eq!(
            serde_json::to_value(&refused)?,
            serde_json::to_value(&direct)?,
            "{number}: the store's refusal, unchanged"
        );
        assert_eq!(refused.code, ErrorCode::Conflict, "{number}");
        assert_eq!(tenancy.jobs.job(job)?.state, state, "{number}: unchanged");
    }
    let sealed = err_of(
        tenancy
            .service
            .cancel_job(&alice(OperationName::CancelJob, Some(85))?, cancel(id(73))?)
            .await,
    )?;
    assert!(
        sealed.message.contains("point of no return"),
        "{}",
        sealed.message
    );
    assert_quiet(&tenancy)
}

/// Tenant jobs are not controlled through a workspace (owner decision 2026-10-08): the request
/// names a workspace, and a tenant job is not found there.
#[tokio::test]
async fn a_tenant_job_is_not_found_through_a_workspaces_cancel_or_retry() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy
        .access
        .grant(ALICE, workspace_id()?, EVERY.to_vec())?;
    tenancy.jobs.put_job(
        &tenant_jobs()?,
        job(77, JobKind::BackupInstallation, JobState::Queued, None)?,
        false,
    )?;
    tenancy.jobs.put_job(
        &tenant_jobs()?,
        job(78, JobKind::PurgeWorkspace, JobState::Failed, None)?,
        false,
    )?;
    let refused = err_of(
        tenancy
            .service
            .cancel_job(&alice(OperationName::CancelJob, Some(81))?, cancel(id(77))?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    let refused = err_of(
        tenancy
            .service
            .retry_job(&alice(OperationName::RetryJob, Some(82))?, retry(id(78))?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(tenancy.jobs.job(id(77))?.state, JobState::Queued);
    assert_eq!(tenancy.jobs.job(id(78))?.state, JobState::Failed);
    assert_quiet(&tenancy)
}

/// A cancellation whose announcement failed fails the request, and its retry announces the job
/// once; a further repeat of the mutation appends nothing (the log's replay key).
#[tokio::test]
async fn a_repeated_cancel_announces_the_job_once() -> TestResult {
    let tenancy = Tenancy::new()?;
    tenancy
        .access
        .grant(ALICE, workspace_id()?, EVERY.to_vec())?;
    let queued = put(&tenancy, 79, JobKind::Import, JobState::Queued)?;
    tenancy.log.fail_next();
    let first = alice(OperationName::CancelJob, Some(83))?;
    let refused = err_of(tenancy.service.cancel_job(&first, cancel(queued)?).await)?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(tenancy.jobs.job(queued)?.state, JobState::Cancelled);
    let mut resumed = first;
    resumed.attempt = Attempt::Resumed;
    for _ in 0..2 {
        let again = tenancy
            .service
            .cancel_job(&resumed, cancel(queued)?)
            .await?;
        assert_eq!(again.state, JobState::Cancelled);
    }
    assert_eq!(tenancy.log.appended()?, vec![announced(queued, 83)?]);
    Ok(())
}

/// Dispatch `operation` with `input` over the tenancy's service, as `principal` in a browser
/// session: the grants and the log are the tenancy's, the ledger is `ledger`.
async fn dispatched(
    tenancy: &Tenancy,
    ledger: &FixtureMutations,
    principal: &Principal,
    operation: &str,
    input: Value,
) -> Result<Value, ApiError> {
    let ports = DispatchPorts {
        access: tenancy.access.as_ref(),
        mutations: ledger,
        events: tenancy.log.as_ref(),
    };
    let caller = Caller {
        principal,
        session_id: Some("session"),
    };
    dispatch(&tenancy.service, &ports, &caller, operation, input).await
}

fn person(subject: &str, route: AccessRoute) -> Built<Principal> {
    Ok(Principal {
        subject: subject.to_owned(),
        tenant_id: scope()?.tenant_id,
        route,
        client_id: None,
        delegation: None,
    })
}

fn body(job: JobId, idempotency_key: &str) -> Built<Value> {
    Ok(json!({
        "workspace_id": workspace_id()?, "job_id": job, "idempotency_key": idempotency_key
    }))
}

/// The handler's own `Forbidden` (the job-kind rule) is recorded by dispatch as every refusal
/// is: `permission_denied` naming the caller and the operation, in the workspace's log.
#[tokio::test]
async fn a_job_kind_refusal_through_dispatch_is_recorded_as_permission_denied() -> TestResult {
    let tenancy = Tenancy::new()?;
    let ledger = FixtureMutations::new();
    tenancy
        .catalog
        .put(&scope()?.tenant_id, workspace(1, "home", 'a', false)?)?;
    tenancy.access.grant(
        "bob",
        workspace_id()?,
        vec![Permission::Read, Permission::Write],
    )?;
    let failed = put(&tenancy, 91, JobKind::BackupWorkspace, JobState::Failed)?;
    let queued = put(&tenancy, 92, JobKind::BackupWorkspace, JobState::Queued)?;
    let bob = person("bob", AccessRoute::BrowserSession)?;
    for (operation, job, idempotency_key) in [
        ("retry_job", failed, "11111111-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        ("cancel_job", queued, "22222222-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
    ] {
        let refused = err_of(
            dispatched(
                &tenancy,
                &ledger,
                &bob,
                operation,
                body(job, idempotency_key)?,
            )
            .await,
        )?;
        assert_eq!(refused.code, ErrorCode::Forbidden, "{operation}");
    }
    let log = EventScope::Workspace(scope()?);
    assert_eq!(
        tenancy.log.appended()?,
        vec![
            (
                log.clone(),
                None,
                NewEvent::permission_denied(&bob, OperationName::RetryJob)
            ),
            (
                log,
                None,
                NewEvent::permission_denied(&bob, OperationName::CancelJob)
            ),
        ]
    );
    assert_eq!(tenancy.jobs.job(failed)?.state, JobState::Failed);
    assert_eq!(tenancy.jobs.job(queued)?.state, JobState::Queued);
    assert_eq!(tenancy.queue.woken()?, Vec::new());
    Ok(())
}

/// The archive rule is dispatch's (contract ruling 25): a cancellation runs in an archived
/// workspace, and a retry is refused there before its handler, with the archive named.
#[tokio::test]
async fn through_dispatch_an_archived_workspace_cancels_a_job_and_refuses_a_retry() -> TestResult {
    let tenancy = Tenancy::new()?;
    let ledger = FixtureMutations::new();
    tenancy
        .catalog
        .put(&scope()?.tenant_id, workspace(1, "home", 'a', true)?)?;
    tenancy
        .access
        .grant(ALICE, workspace_id()?, EVERY.to_vec())?;
    let queued = put(&tenancy, 93, JobKind::Import, JobState::Queued)?;
    let failed = put(&tenancy, 94, JobKind::Import, JobState::Failed)?;
    let alice = person(ALICE, AccessRoute::LocalOwner)?;
    let cancelled: Job = serde_json::from_value(
        dispatched(
            &tenancy,
            &ledger,
            &alice,
            "cancel_job",
            body(queued, "33333333-aaaa-4aaa-8aaa-aaaaaaaaaaaa")?,
        )
        .await?,
    )?;
    assert_eq!(cancelled.state, JobState::Cancelled);
    let refused = err_of(
        dispatched(
            &tenancy,
            &ledger,
            &alice,
            "retry_job",
            body(failed, "44444444-aaaa-4aaa-8aaa-aaaaaaaaaaaa")?,
        )
        .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(matches!(
        refused.detail.as_deref(),
        Some(ErrorDetail::WorkspaceArchived { archived_at }) if *archived_at == archived_instant()?
    ));
    assert_eq!(
        tenancy.jobs.job(failed)?.state,
        JobState::Failed,
        "not retried"
    );
    assert_eq!(tenancy.queue.woken()?, Vec::new());
    let announced: Vec<(EventKind, Option<JobId>)> = tenancy
        .log
        .appended()?
        .into_iter()
        .map(|(_, _, event)| (event.kind, event.job_id))
        .collect();
    assert_eq!(announced, vec![(EventKind::JobUpdated, Some(queued))]);
    Ok(())
}
