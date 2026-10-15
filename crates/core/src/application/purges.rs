//! Workspace purge: recording one, resuming one and reading one (SPEC section 8; Stage 1b design
//! section 6; contract ruling 24).
//!
//! A purge authorizes at the tenant (`admin`), and only in a human session (dispatch refuses
//! the operations of `HUMAN_SESSION_ONLY` on every other route): it removes the workspace's
//! grants, so a workspace target could not authorize the retry of a half-finished purge.
//!
//! `purge_workspace` answers in the contract's order:
//!
//! 1. A purge already recorded for the workspace is answered, whatever its idempotency key, and
//!    before `base_revision` is looked at, so a moved or removed head never blocks it
//!    (`RecordStore::find_purge`). An unfinished one is resumed: its tenant job is queued again
//!    (`JobQueue::enqueue`), after it is made runnable again (`RecordStore::retry_job`, under
//!    this request's mutation) when it failed, since a failed job does not run again by itself
//!    (`PurgeState::Failed`, which ingest's purge handler records). A completed one is answered
//!    as it is and nothing is queued: a workspace is purged once.
//! 2. Otherwise `base_revision` must be the head now. A revision a purge invalidated is the
//!    typed `NotFound` (`reading::check_revision`); any other revision that is not the head is
//!    `conflict` with `StaleBase { head }` on `/base_revision`; a workspace the tenant does not
//!    hold is `NotFound` (`WorkspaceCatalog::open`).
//! 3. Then the purge is recorded with its tenant job (`RecordStore::create_purge`, under the
//!    identity `derive_purge_id` gives the mutation). From that moment storage refuses every
//!    write to the workspace (`Purger`). The job is queued and the purge answered (202). A
//!    purge of the same workspace recorded between steps 1 and 3 is answered by `create_purge`
//!    instead of a second one, and resumed as in step 1.
//!
//! Every step is idempotent on the request's mutation: the record is unique on its identity,
//! the retry of a failed job is unique on the mutation, and a delivery may repeat. So a resumed
//! attempt finds the purge its first attempt recorded in step 1 and answers it. A failed job is
//! retried at most once per mutation (the store's rule): a resumed attempt that finds it failed
//! again since queues it without making it runnable, and a repeat under another key resumes
//! it. Ingest's purge handler writes every failure of a workspace purge's job as retryable,
//! whatever its code: the purge was accepted and its precondition holds, so whatever stopped it
//! (a survivor's archive that does not read, an I/O error, the drain's bound) is a state to
//! repair, after which asking again resumes it (contract ruling 14). A job the store holds as
//! not retryable (a failure some other writer recorded so) cannot be made runnable: the store's
//! `conflict` is then the answer. The work, the drain of the workspace's jobs and the
//! removal, is the tenant job's (ingest's purge handler); a purge is never cancelled. No event
//! is appended here: the job's own writes are announced by its handler.
//!
//! `get_purge` reads one purge of the caller's tenant in any state; it outlives the workspace.
//!
//! `purge_item` is not built and answers `NotImplemented` (`application::pending`): storage's
//! item purge does not exist yet (SPEC section 8: "Workspace purge is built first; item purge
//! follows with the same guarantees"), so a recorded item purge could never complete.

use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{MutationId, TenantId};
use okf_jawn_contract::import::JobState;
use okf_jawn_contract::purge::{
    GetPurgeRequest, Purge, PurgeState, PurgeTarget, PurgeWorkspaceRequest,
};

use super::ApplicationService;
use super::shared::{check_named, mutation_id};
use crate::context::OperationContext;
use crate::jobs::{JobScope, NewPurge};
use crate::storage::{Provenance, StorageScope, derive_purge_id};

/// Record the purge of a workspace, or answer the purge already recorded for it.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated `base_revision`, `NotFound` for a workspace
/// the tenant does not hold, `Conflict` with `ErrorDetail::StaleBase` on `/base_revision` when
/// the head moved, the store's `Conflict` when a failed purge job cannot be retried, or any
/// port error.
pub(super) async fn purge_workspace(
    service: &ApplicationService,
    context: &OperationContext,
    request: PurgeWorkspaceRequest,
) -> Result<Purge, ApiError> {
    let tenant = tenant_of(context)?;
    let mutation = mutation_id(context)?;
    let ports = service.ports();
    let target = PurgeTarget::Workspace {
        workspace_id: request.workspace_id,
    };
    // Step 1, before the base: a recorded purge of the workspace is answered.
    if let Some(recorded) = ports.records.find_purge(&tenant, &target).await? {
        return resume(service, &tenant, mutation, recorded).await;
    }
    // Step 2: the purge is based on the head.
    let scope = StorageScope {
        tenant_id: tenant.clone(),
        workspace_id: request.workspace_id,
    };
    check_named(service, &scope, &request.base_revision).await?;
    let head = ports.catalog.open(&scope).await?.head;
    if head != request.base_revision {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "the workspace head moved since the purge was asked for",
        )
        .with_field("/base_revision")
        .with_detail(ErrorDetail::StaleBase { head }));
    }
    // Step 3: recorded with its tenant job; from here storage refuses the workspace's writes.
    let recorded = ports
        .records
        .create_purge(
            &tenant,
            mutation,
            NewPurge {
                id: derive_purge_id(mutation),
                target,
                initiator: Provenance::from_principal(&context.principal),
            },
        )
        .await?;
    resume(service, &tenant, mutation, recorded).await
}

/// Read one purge of the caller's tenant.
///
/// # Errors
/// Returns `NotFound` when the tenant has no such purge, or any port error.
pub(super) async fn get_purge(
    service: &ApplicationService,
    context: &OperationContext,
    request: GetPurgeRequest,
) -> Result<Purge, ApiError> {
    let tenant = tenant_of(context)?;
    service
        .ports()
        .records
        .get_purge(&tenant, request.purge_id)
        .await
}

/// Answer a recorded purge: a completed one as it is; an unfinished one with its tenant job
/// queued again, made runnable first when it failed.
///
/// # Errors
/// Returns the store's `Conflict` when the failed job cannot be retried, or any port error.
async fn resume(
    service: &ApplicationService,
    tenant: &TenantId,
    mutation: MutationId,
    purge: Purge,
) -> Result<Purge, ApiError> {
    if purge.state == PurgeState::Completed {
        return Ok(purge);
    }
    let ports = service.ports();
    let scope = JobScope::Tenant(tenant.clone());
    // By the job's state, not the purge's: a failure the handler could not record leaves the
    // purge `running` while its job failed, and a purge marked failed by an attempt that lost
    // its lease has a job that runs again by itself.
    if ports.records.get_job(&scope, purge.job_id).await?.state == JobState::Failed {
        let _queued = ports
            .records
            .retry_job(&scope, mutation, purge.job_id)
            .await?;
    }
    ports.queue.enqueue(scope, purge.job_id).await?;
    Ok(purge)
}

/// The tenant dispatch authorized the operation at.
///
/// # Errors
/// Returns `Internal` when dispatch authorized no tenant, which only a request without a
/// deployment target could cause.
fn tenant_of(context: &OperationContext) -> Result<TenantId, ApiError> {
    context
        .tenant
        .as_ref()
        .map(|grant| grant.tenant_id.clone())
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::Internal,
                "the operation was authorized at no tenant",
            )
        })
}
