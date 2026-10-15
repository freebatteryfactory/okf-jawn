//! `purge_workspace`, `get_purge` and `purge_item` over the purge fakes: the contract's order
//! (a recorded purge is answered before the base is looked at; then the base; then the
//! record), the resumption of an unfinished or failed purge, the answer of a completed one, and
//! a retry under the same mutation.

use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{IdempotencyKey, JobId, PurgeId, TenantId, Timestamp};
use okf_jawn_contract::import::JobState;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::purge::{
    GetPurgeRequest, Purge, PurgeItemRequest, PurgeState, PurgeTarget, PurgeWorkspaceRequest,
};
use okf_jawn_contract::workspace::Workspace;
use okf_jawn_core::context::{Attempt, OperationContext, TenantGrant};
use okf_jawn_core::jobs::{JobScope, RevisionMapping};
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::{Provenance, derive_purge_id};
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{Built, mutation, revision};
use crate::purge_fixture::{PurgeWorld, workspace_id};

/// The workspace the tests purge.
const TARGET: u128 = 7;
/// The tenant administrator who asks.
const ADMIN: &str = "admin";

fn tenant() -> Built<TenantId> {
    Ok(TenantId::try_from("local".to_owned())?)
}

fn other_tenant() -> Built<TenantId> {
    Ok(TenantId::try_from("elsewhere".to_owned())?)
}

/// The context dispatch builds for a tenant administrator in a browser session running
/// `operation`, under `number`'s mutation when it is a mutation.
fn admin(operation: OperationName, number: Option<u128>) -> Built<OperationContext> {
    let tenant = tenant()?;
    Ok(OperationContext {
        principal: Principal {
            subject: ADMIN.to_owned(),
            tenant_id: tenant.clone(),
            route: AccessRoute::BrowserSession,
            client_id: None,
            delegation: None,
        },
        session_id: Some("session".to_owned()),
        operation,
        tenant: Some(TenantGrant {
            tenant_id: tenant,
            permissions: vec![Permission::Admin],
        }),
        grants: Vec::new(),
        mutation: number.map(mutation),
        attempt: Attempt::First,
    })
}

fn key(number: u128) -> Built<IdempotencyKey> {
    Ok(serde_json::from_value(json!(
        Uuid::from_u128(number).to_string()
    ))?)
}

fn request(base: char, number: u128) -> Built<PurgeWorkspaceRequest> {
    Ok(PurgeWorkspaceRequest {
        workspace_id: workspace_id(TARGET),
        base_revision: revision(base)?,
        idempotency_key: key(number)?,
    })
}

/// A world whose catalog holds the target at head `a`.
fn world() -> Built<PurgeWorld> {
    let world = PurgeWorld::new()?;
    world.catalog.put(
        &tenant()?,
        Workspace {
            id: workspace_id(TARGET),
            name: "Trip".to_owned(),
            description: String::new(),
            head: revision('a')?,
            created_at: Timestamp::try_from("2026-10-01T00:00:00.000Z".to_owned())?,
            archived_at: None,
            permissions: Vec::new(),
        },
    )?;
    Ok(world)
}

/// A purge of the target that another request recorded, numbered `number`, in `state`.
fn recorded(number: u128, state: PurgeState) -> Built<Purge> {
    Ok(serde_json::from_value(json!({
        "id": Uuid::from_u128(number).to_string(),
        "target": { "kind": "workspace", "workspace_id": workspace_id(TARGET) },
        "state": state,
        "job_id": Uuid::from_u128(number.saturating_add(1)).to_string(),
        "requested_by": "someone",
        "requested_at": "2026-10-02T00:00:00.000Z",
    }))?)
}

fn tenant_scope() -> Built<JobScope> {
    Ok(JobScope::Tenant(tenant()?))
}

/// The purge's tenant job was delivered `times` times, and nothing else.
fn assert_delivered(world: &PurgeWorld, job: JobId, times: usize) -> TestResult {
    let scope = tenant_scope()?;
    assert_eq!(world.queue.deliveries()?, vec![(scope, job); times]);
    Ok(())
}

#[tokio::test]
async fn a_purge_of_the_head_is_recorded_with_its_tenant_job_and_the_job_queued() -> TestResult {
    let world = world()?;
    let context = admin(OperationName::PurgeWorkspace, Some(40))?;
    let purge = world
        .service
        .purge_workspace(&context, request('a', 40)?)
        .await?;
    assert_eq!(purge.id, derive_purge_id(mutation(40)));
    assert_eq!(purge.state, PurgeState::Requested);
    assert_eq!(
        purge.target,
        PurgeTarget::Workspace {
            workspace_id: workspace_id(TARGET)
        }
    );
    assert_eq!(purge.requested_by, ADMIN);
    let recorded = world.records.recorded()?;
    let (stored_mutation, initiator, _) = some(recorded.first(), "the record")?;
    assert_eq!(recorded.len(), 1);
    assert_eq!(*stored_mutation, Some(mutation(40)));
    assert_eq!(
        initiator.as_ref(),
        Some(&Provenance {
            subject: ADMIN.to_owned(),
            route: AccessRoute::BrowserSession,
            client_id: None,
        })
    );
    assert_delivered(&world, purge.job_id, 1)?;
    assert_eq!(world.records.job(purge.job_id)?.state, JobState::Queued);
    Ok(())
}

#[tokio::test]
async fn a_purge_on_a_moved_head_is_a_conflict_naming_the_head_and_records_nothing() -> TestResult {
    let world = world()?;
    let context = admin(OperationName::PurgeWorkspace, Some(41))?;
    let refused = err_of(
        world
            .service
            .purge_workspace(&context, request('b', 41)?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert_eq!(refused.field.as_deref(), Some("/base_revision"));
    assert_eq!(
        refused.detail.as_deref(),
        Some(&ErrorDetail::StaleBase {
            head: revision('a')?
        })
    );
    assert!(world.records.recorded()?.is_empty());
    assert_eq!(world.queue.deliveries()?, Vec::new());
    Ok(())
}

#[tokio::test]
async fn a_purge_of_a_workspace_the_tenant_does_not_hold_is_not_found_and_records_nothing()
-> TestResult {
    let world = PurgeWorld::new()?;
    let context = admin(OperationName::PurgeWorkspace, Some(42))?;
    let refused = err_of(
        world
            .service
            .purge_workspace(&context, request('a', 42)?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert!(world.records.recorded()?.is_empty());
    assert_eq!(world.queue.deliveries()?, Vec::new());
    Ok(())
}

/// The base is checked against the revision map before it is compared with the head: a
/// revision a purge invalidated is the typed `NotFound`, even when the catalog still shows it.
#[tokio::test]
async fn an_invalidated_base_is_the_typed_not_found_and_records_nothing() -> TestResult {
    let world = world()?;
    let purge: PurgeId = serde_json::from_value(json!(Uuid::from_u128(90).to_string()))?;
    world.records.map(
        revision('a')?,
        RevisionMapping {
            purge_id: purge,
            replacement: None,
        },
    )?;
    let context = admin(OperationName::PurgeWorkspace, Some(43))?;
    let refused = err_of(
        world
            .service
            .purge_workspace(&context, request('a', 43)?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(
        refused.detail.as_deref(),
        Some(&ErrorDetail::Invalidated {
            purge_id: purge,
            replacement: None
        })
    );
    assert!(world.records.recorded()?.is_empty());
    Ok(())
}

/// Contract ruling 24 and the design's final-review decision 4: an unfinished purge of the
/// workspace is answered whatever the key, before the base is looked at, so a moved or removed
/// head never blocks it, and its job is queued again. Nothing new is recorded.
#[tokio::test]
async fn an_unfinished_purge_is_answered_before_the_base_and_its_job_queued_again() -> TestResult {
    for state in [PurgeState::Requested, PurgeState::Running] {
        // No workspace in the catalog, and a base that is no head.
        let world = PurgeWorld::new()?;
        let other = recorded(70, state)?;
        world
            .records
            .put(&tenant()?, other.clone(), JobState::Queued)?;
        let context = admin(OperationName::PurgeWorkspace, Some(44))?;
        let answered = world
            .service
            .purge_workspace(&context, request('f', 44)?)
            .await?;
        assert_eq!(answered.id, other.id, "{state:?}");
        assert_eq!(answered.state, state);
        assert_delivered(&world, other.job_id, 1)?;
        assert_eq!(
            world.records.recorded()?.len(),
            1,
            "nothing new is recorded"
        );
        assert_eq!(world.catalog.opens()?, 0, "the base is not looked at");
        assert_eq!(
            world.records.retries()?,
            Vec::new(),
            "a queued job is not retried"
        );
    }
    Ok(())
}

/// Contract ruling 24: a completed purge of the workspace is answered as it is, whatever the
/// key and the base, and nothing is queued or retried.
#[tokio::test]
async fn a_completed_purge_is_answered_as_it_is_and_queues_nothing() -> TestResult {
    let world = PurgeWorld::new()?;
    let completed = recorded(72, PurgeState::Completed)?;
    world
        .records
        .put(&tenant()?, completed.clone(), JobState::Succeeded)?;
    let context = admin(OperationName::PurgeWorkspace, Some(45))?;
    let answered = world
        .service
        .purge_workspace(&context, request('f', 45)?)
        .await?;
    assert_eq!(answered.id, completed.id);
    assert_eq!(answered.state, PurgeState::Completed);
    assert_eq!(world.queue.deliveries()?, Vec::new());
    assert_eq!(world.records.calls()?, ["find_purge"]);
    Ok(())
}

/// Contract ruling 24: a purge whose job failed and will not run again by itself is resumed by
/// asking again: its job is made runnable (the store's retry, under this request's mutation)
/// and queued, and the purge is answered as it stands.
#[tokio::test]
async fn a_failed_purge_asked_for_again_makes_its_job_runnable_and_queues_it() -> TestResult {
    let world = PurgeWorld::new()?;
    let failed = recorded(74, PurgeState::Failed)?;
    world
        .records
        .put(&tenant()?, failed.clone(), JobState::Failed)?;
    let context = admin(OperationName::PurgeWorkspace, Some(46))?;
    let answered = world
        .service
        .purge_workspace(&context, request('f', 46)?)
        .await?;
    assert_eq!(
        (answered.id, answered.state),
        (failed.id, PurgeState::Failed)
    );
    assert_eq!(world.records.retries()?, [mutation(46)]);
    assert_eq!(world.records.job(failed.job_id)?.state, JobState::Queued);
    assert_delivered(&world, failed.job_id, 1)?;
    Ok(())
}

/// The retry follows the job's state, not the purge's: a failure its handler could not record
/// leaves the purge `running` while its job failed, and the job is still made runnable.
#[tokio::test]
async fn an_unfinished_purge_whose_job_failed_is_made_runnable_by_the_jobs_state() -> TestResult {
    let world = PurgeWorld::new()?;
    let running = recorded(76, PurgeState::Running)?;
    world
        .records
        .put(&tenant()?, running.clone(), JobState::Failed)?;
    let context = admin(OperationName::PurgeWorkspace, Some(47))?;
    world
        .service
        .purge_workspace(&context, request('f', 47)?)
        .await?;
    assert_eq!(world.records.retries()?, [mutation(47)]);
    assert_delivered(&world, running.job_id, 1)?;
    Ok(())
}

/// A purge marked failed whose job is queued again by itself (its attempt lost its lease) is
/// not retried, which the store would refuse; its job is queued.
#[tokio::test]
async fn a_failed_purge_whose_job_runs_again_by_itself_is_only_queued() -> TestResult {
    let world = PurgeWorld::new()?;
    let failed = recorded(78, PurgeState::Failed)?;
    world
        .records
        .put(&tenant()?, failed.clone(), JobState::Queued)?;
    let context = admin(OperationName::PurgeWorkspace, Some(48))?;
    world
        .service
        .purge_workspace(&context, request('f', 48)?)
        .await?;
    assert!(!world.records.calls()?.contains(&"retry_job"));
    assert_delivered(&world, failed.job_id, 1)?;
    Ok(())
}

/// A failure the store holds as not retryable cannot be made runnable: the store's refusal is
/// the answer, and nothing is queued.
#[tokio::test]
async fn a_failed_purge_whose_job_cannot_be_retried_answers_the_stores_conflict() -> TestResult {
    let world = PurgeWorld::new()?;
    let failed = recorded(80, PurgeState::Failed)?;
    world
        .records
        .put(&tenant()?, failed.clone(), JobState::Failed)?;
    world
        .records
        .set_job(failed.job_id, JobState::Failed, false)?;
    let context = admin(OperationName::PurgeWorkspace, Some(49))?;
    let refused = err_of(
        world
            .service
            .purge_workspace(&context, request('f', 49)?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(
        refused.message.contains("not retryable"),
        "{}",
        refused.message
    );
    assert_eq!(world.queue.deliveries()?, Vec::new());
    Ok(())
}

/// A retry under the same mutation answers the purge its first attempt recorded: the first
/// attempt recorded it and failed to deliver its job; the resumed attempt finds it, delivers
/// the job and records nothing else.
#[tokio::test]
async fn a_resumed_attempt_answers_the_purge_its_first_attempt_recorded() -> TestResult {
    let world = world()?;
    world.queue.fail_next()?;
    let first = admin(OperationName::PurgeWorkspace, Some(50))?;
    let failed = err_of(
        world
            .service
            .purge_workspace(&first, request('a', 50)?)
            .await,
    )?;
    assert_eq!(failed.code, ErrorCode::Unavailable);
    // Someone commits meanwhile: the resumption never looks at the base again.
    world.catalog.put(
        &tenant()?,
        Workspace {
            id: workspace_id(TARGET),
            name: String::new(),
            description: String::new(),
            head: revision('c')?,
            created_at: Timestamp::try_from("2026-10-01T00:00:00.000Z".to_owned())?,
            archived_at: None,
            permissions: Vec::new(),
        },
    )?;
    let resumed = OperationContext {
        attempt: Attempt::Resumed,
        ..first
    };
    let purge = world
        .service
        .purge_workspace(&resumed, request('a', 50)?)
        .await?;
    assert_eq!(purge.id, derive_purge_id(mutation(50)));
    assert_eq!(world.records.recorded()?.len(), 1);
    assert_delivered(&world, purge.job_id, 1)?;
    Ok(())
}

/// A purge another request records between this request's lookup and its record is answered
/// by the store instead of a second record, and its job is queued.
#[tokio::test]
async fn a_purge_recorded_meanwhile_is_answered_instead_of_a_second() -> TestResult {
    let world = world()?;
    let other = recorded(82, PurgeState::Requested)?;
    world.records.record_meanwhile(&tenant()?, other.clone())?;
    let context = admin(OperationName::PurgeWorkspace, Some(51))?;
    let answered = world
        .service
        .purge_workspace(&context, request('a', 51)?)
        .await?;
    assert_eq!(answered.id, other.id);
    let recorded = world.records.recorded()?;
    assert_eq!(recorded.len(), 1, "no second purge of the workspace");
    assert_delivered(&world, other.job_id, 1)?;
    Ok(())
}

/// Each step's failure fails the request with nothing answered: the lookup, the record, the
/// read of the job and its retry.
#[tokio::test]
async fn a_failing_store_fails_the_purge_request_with_its_error() -> TestResult {
    for method in ["find_purge", "create_purge", "get_job", "retry_job"] {
        let world = world()?;
        if method != "find_purge" && method != "create_purge" {
            let failed = recorded(84, PurgeState::Failed)?;
            world.records.put(&tenant()?, failed, JobState::Failed)?;
        }
        world.records.fail_next(method, ErrorCode::Unavailable)?;
        let context = admin(OperationName::PurgeWorkspace, Some(52))?;
        let refused = err_of(
            world
                .service
                .purge_workspace(&context, request('a', 52)?)
                .await,
        )?;
        assert_eq!(refused.code, ErrorCode::Unavailable, "{method}");
        assert_eq!(world.queue.deliveries()?, Vec::new(), "{method}");
    }
    Ok(())
}

#[tokio::test]
async fn get_purge_reads_a_purge_of_the_callers_tenant_only() -> TestResult {
    let world = PurgeWorld::new()?;
    let ours = recorded(86, PurgeState::Completed)?;
    world
        .records
        .put(&tenant()?, ours.clone(), JobState::Succeeded)?;
    let theirs = recorded(88, PurgeState::Running)?;
    world
        .records
        .put(&other_tenant()?, theirs.clone(), JobState::Running)?;
    let context = admin(OperationName::GetPurge, None)?;
    let read = world
        .service
        .get_purge(&context, GetPurgeRequest { purge_id: ours.id })
        .await?;
    assert_eq!(serde_json::to_value(&read)?, serde_json::to_value(&ours)?);
    let refused = err_of(
        world
            .service
            .get_purge(
                &context,
                GetPurgeRequest {
                    purge_id: theirs.id,
                },
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    Ok(())
}

/// SPEC section 8: "Workspace purge is built first; item purge follows". `purge_item` is not
/// built: it answers `NotImplemented` and records nothing.
#[tokio::test]
async fn purge_item_is_not_implemented_and_records_nothing() -> TestResult {
    let world = world()?;
    let context = admin(OperationName::PurgeItem, Some(53))?;
    let refused = err_of(
        world
            .service
            .purge_item(
                &context,
                PurgeItemRequest {
                    workspace_id: workspace_id(TARGET),
                    item_id: serde_json::from_value(json!(Uuid::from_u128(20).to_string()))?,
                    base_revision: revision('a')?,
                    idempotency_key: key(53)?,
                },
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotImplemented);
    assert_eq!(world.records.calls()?, Vec::<&str>::new());
    assert_eq!(world.queue.deliveries()?, Vec::new());
    Ok(())
}
