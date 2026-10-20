//! One allowance for security events (owner decision 2026-10-14): of `permission_denied` (by
//! actor) and `sign_in_refused` (by source), the first ten of a source's minute are recorded
//! one by one, and the rest of that minute as one aggregate event with their `count`, written
//! when the minute ends or when the server stops. Every other event passes through untouched.
#![cfg(feature = "runtime")]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use okf_jawn_contract::access::{AccessRoute, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{Event, EventKind, ListEventsResponse};
use okf_jawn_contract::identity::{MutationId, TenantId};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_server::allowance::{ALLOWANCE, SecurityEvents, WINDOW};
use okf_jawn_server::grants::Clock;
use okf_jawn_server::identity::SignInRefusals as _;
use tokio_util::sync::CancellationToken;

use check::{TestResult, err_of, some};

/// A clock that moves only when a test advances it.
struct TestClock {
    start: Instant,
    elapsed_ms: AtomicU64,
}

/// An event log that records what reaches it, and can refuse.
#[derive(Default)]
struct Recorded {
    events: Mutex<Vec<(EventScope, Option<MutationId>, NewEvent)>>,
    refusing: AtomicBool,
}

impl TestClock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            start: Instant::now(),
            elapsed_ms: AtomicU64::new(0),
        })
    }

    fn advance(&self, by: Duration) -> Result<(), std::num::TryFromIntError> {
        self.elapsed_ms
            .fetch_add(u64::try_from(by.as_millis())?, Ordering::SeqCst);
        Ok(())
    }
}

impl Clock for TestClock {
    fn now(&self) -> Instant {
        self.start
            .checked_add(Duration::from_millis(
                self.elapsed_ms.load(Ordering::SeqCst),
            ))
            .unwrap_or(self.start)
    }
}

impl Recorded {
    fn all(&self) -> Result<Vec<(EventScope, Option<MutationId>, NewEvent)>, ApiError> {
        Ok(self
            .events
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))?
            .clone())
    }

    fn of(&self, kind: &EventKind) -> Result<Vec<NewEvent>, ApiError> {
        Ok(self
            .all()?
            .into_iter()
            .filter(|(_, _, event)| event.kind == *kind)
            .map(|(_, _, event)| event)
            .collect())
    }
}

impl EventLog for Recorded {
    fn append<'a>(
        &'a self,
        scope: &'a EventScope,
        mutation_id: Option<MutationId>,
        event: NewEvent,
    ) -> PortFuture<'a, Event> {
        Box::pin(async move {
            if self.refusing.load(Ordering::SeqCst) {
                return Err(ApiError::new(ErrorCode::Unavailable, "the log is down"));
            }
            let mut events = self
                .events
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))?;
            events.push((scope.clone(), mutation_id, event.clone()));
            serde_json::from_value(serde_json::json!({
                "id": events.len().to_string(),
                "kind": event.kind,
                "at": "2026-10-18T00:00:00.000Z",
            }))
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
        })
    }

    fn list<'a>(
        &'a self,
        _scope: &'a EventScope,
        _query: EventQuery,
    ) -> PortFuture<'a, ListEventsResponse> {
        Box::pin(async {
            Ok(ListEventsResponse {
                events: Vec::new(),
                next_cursor: None,
            })
        })
    }
}

fn principal(subject: &str) -> Principal {
    Principal {
        subject: subject.to_owned(),
        tenant_id: TenantId::local(),
        route: AccessRoute::McpDelegation,
        client_id: Some("connector-1".to_owned()),
        delegation: None,
    }
}

fn tenant_scope() -> EventScope {
    EventScope::Tenant(TenantId::local())
}

async fn deny(
    events: &SecurityEvents,
    who: &Principal,
    times: usize,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut counted: usize = 0;
    for _ in 0..times {
        let appended = events
            .append(
                &EventScope::Tenant(who.tenant_id.clone()),
                None,
                NewEvent::permission_denied(who, OperationName::CreateItem),
            )
            .await;
        if let Err(error) = appended {
            assert_eq!(error.code, ErrorCode::Unavailable);
            counted = some(counted.checked_add(1), "the refusal count")?;
        }
    }
    Ok(counted)
}

fn new(clock: &Arc<TestClock>) -> (Arc<Recorded>, SecurityEvents) {
    let inner = Arc::new(Recorded::default());
    let events = SecurityEvents::new(inner.clone(), clock.clone());
    (inner, events)
}

#[tokio::test]
async fn ten_refusals_in_a_minute_are_ten_events_and_no_aggregate() -> TestResult {
    assert_eq!(ALLOWANCE, 10);
    assert_eq!(WINDOW, Duration::from_secs(60));
    let clock = TestClock::new();
    let (inner, events) = new(&clock);
    assert_eq!(deny(&events, &principal("agent"), 10).await?, 0);
    clock.advance(WINDOW)?;
    events.flush_due().await;
    let denied = inner.of(&EventKind::PermissionDenied)?;
    assert_eq!(denied.len(), 10);
    assert!(denied.iter().all(|event| event.count.is_none()));
    Ok(())
}

#[tokio::test]
async fn the_rest_of_the_minute_is_one_aggregate_with_its_count() -> TestResult {
    let clock = TestClock::new();
    let (inner, events) = new(&clock);
    let agent = principal("agent");
    assert_eq!(deny(&events, &agent, 25).await?, 15);
    assert_eq!(inner.of(&EventKind::PermissionDenied)?.len(), 10);
    // Not before the minute ends.
    clock.advance(Duration::from_millis(59_999))?;
    events.flush_due().await;
    assert_eq!(inner.of(&EventKind::PermissionDenied)?.len(), 10);
    clock.advance(Duration::from_millis(1))?;
    events.flush_due().await;
    let denied = inner.of(&EventKind::PermissionDenied)?;
    assert_eq!(denied.len(), 11);
    let aggregate = denied.last().ok_or("the aggregate")?;
    assert_eq!(aggregate.count, Some(15));
    assert_eq!(
        aggregate.actor.as_ref().map(|actor| actor.subject.as_str()),
        Some("agent")
    );
    assert_eq!(aggregate.operation, None);
    // The aggregate is in the tenant's log, and written once.
    let scopes: Vec<_> = inner.all()?.into_iter().map(|(scope, ..)| scope).collect();
    assert!(scopes.iter().all(|scope| *scope == tenant_scope()));
    events.flush_due().await;
    assert_eq!(inner.of(&EventKind::PermissionDenied)?.len(), 11);
    Ok(())
}

#[tokio::test]
async fn a_refusal_at_the_end_of_the_minute_opens_the_next() -> TestResult {
    let clock = TestClock::new();
    let (inner, events) = new(&clock);
    let agent = principal("agent");
    assert_eq!(deny(&events, &agent, 11).await?, 1);
    clock.advance(Duration::from_millis(59_999))?;
    // Still the first minute: counted.
    assert_eq!(deny(&events, &agent, 1).await?, 1);
    clock.advance(Duration::from_millis(1))?;
    // The next minute: recorded again, and the first minute's aggregate is written.
    assert_eq!(deny(&events, &agent, 1).await?, 0);
    events.flush_due().await;
    let counts: Vec<_> = inner
        .of(&EventKind::PermissionDenied)?
        .into_iter()
        .filter_map(|event| event.count)
        .collect();
    assert_eq!(counts, vec![2]);
    assert_eq!(inner.of(&EventKind::PermissionDenied)?.len(), 12);
    Ok(())
}

#[tokio::test]
async fn two_actors_have_their_own_allowance() -> TestResult {
    let clock = TestClock::new();
    let (inner, events) = new(&clock);
    assert_eq!(deny(&events, &principal("first"), 10).await?, 0);
    assert_eq!(deny(&events, &principal("second"), 10).await?, 0);
    let mut other_client = principal("first");
    other_client.client_id = Some("connector-2".to_owned());
    assert_eq!(deny(&events, &other_client, 10).await?, 0);
    assert_eq!(inner.of(&EventKind::PermissionDenied)?.len(), 30);
    Ok(())
}

#[tokio::test]
async fn shutdown_writes_the_aggregates_of_open_minutes() -> TestResult {
    let clock = TestClock::new();
    let (inner, events) = new(&clock);
    assert_eq!(deny(&events, &principal("agent"), 13).await?, 3);
    let events = Arc::new(events);
    let stop = CancellationToken::new();
    let running = tokio::spawn(events.clone().run(stop.clone()));
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(10), running).await??;
    let counts: Vec<_> = inner
        .of(&EventKind::PermissionDenied)?
        .into_iter()
        .filter_map(|event| event.count)
        .collect();
    assert_eq!(counts, vec![3]);
    Ok(())
}

#[tokio::test]
async fn every_other_event_passes_through_with_its_mutation() -> TestResult {
    let clock = TestClock::new();
    let (inner, events) = new(&clock);
    let mutation: MutationId =
        serde_json::from_value(serde_json::json!("00000000-0000-4000-8000-000000000001"))?;
    let agent = principal("agent");
    for _ in 0..15 {
        events
            .append(&tenant_scope(), None, NewEvent::signed_in(&agent))
            .await?;
    }
    // A refusal appended under a mutation is not one this allowance bounds.
    for _ in 0..15 {
        events
            .append(
                &tenant_scope(),
                Some(mutation),
                NewEvent::permission_denied(&agent, OperationName::CreateItem),
            )
            .await?;
    }
    let all = inner.all()?;
    assert_eq!(all.len(), 30);
    assert!(
        all.iter()
            .filter(|(_, _, event)| event.kind == EventKind::PermissionDenied)
            .all(|(_, recorded, _)| *recorded == Some(mutation))
    );
    Ok(())
}

#[tokio::test]
async fn refused_sign_ins_are_bounded_by_their_source() -> TestResult {
    let clock = TestClock::new();
    let (inner, events) = new(&clock);
    let local = TenantId::local();
    for _ in 0..11 {
        events.refused(Some(&local), "local").await?;
    }
    let refused = inner.of(&EventKind::SignInRefused)?;
    assert_eq!(refused.len(), 10);
    assert!(refused.iter().all(|event| event.actor.is_none()));
    // A refusal with no tenant is written to no log.
    events.refused(None, "203.0.113.7").await?;
    clock.advance(WINDOW)?;
    events.flush_due().await;
    let refused = inner.of(&EventKind::SignInRefused)?;
    assert_eq!(refused.len(), 11);
    assert_eq!(refused.last().and_then(|event| event.count), Some(1));
    assert_eq!(inner.all()?.len(), 11);
    Ok(())
}

#[tokio::test]
async fn a_failed_aggregate_is_not_retried_and_stops_nothing() -> TestResult {
    let clock = TestClock::new();
    let (inner, events) = new(&clock);
    assert_eq!(deny(&events, &principal("agent"), 12).await?, 2);
    inner.refusing.store(true, Ordering::SeqCst);
    clock.advance(WINDOW)?;
    events.flush_due().await;
    inner.refusing.store(false, Ordering::SeqCst);
    events.flush_due().await;
    assert!(
        inner
            .of(&EventKind::PermissionDenied)?
            .iter()
            .all(|event| event.count.is_none())
    );
    Ok(())
}

#[tokio::test]
async fn past_its_source_bound_new_sources_share_one_counted_aggregate() -> TestResult {
    let clock = TestClock::new();
    let inner = Arc::new(Recorded::default());
    let events = SecurityEvents::with_bound(inner.clone(), clock.clone(), 2);
    assert_eq!(deny(&events, &principal("first"), 1).await?, 0);
    assert_eq!(deny(&events, &principal("second"), 1).await?, 0);
    // The third and fourth sources are past the bound: counted, never recorded one by one.
    assert_eq!(deny(&events, &principal("third"), 1).await?, 1);
    assert_eq!(deny(&events, &principal("fourth"), 2).await?, 2);
    clock.advance(WINDOW)?;
    events.flush_due().await;
    let denied = inner.of(&EventKind::PermissionDenied)?;
    let counts: Vec<_> = denied.iter().filter_map(|event| event.count).collect();
    assert_eq!(counts, vec![3]);
    assert_eq!(denied.len(), 3);
    // An overflow aggregate names no actor.
    assert!(
        denied
            .iter()
            .filter(|event| event.count.is_some())
            .all(|event| event.actor.is_none())
    );
    // After the minute the stale windows are gone: the third and fourth sources are tracked and
    // recorded, and the bound is full again.
    assert_eq!(deny(&events, &principal("third"), 1).await?, 0);
    assert_eq!(deny(&events, &principal("fourth"), 1).await?, 0);
    assert_eq!(inner.of(&EventKind::PermissionDenied)?.len(), 5);
    // A counted refusal answers `unavailable`, which every writer of these kinds ignores.
    let counted = err_of(
        events
            .append(
                &tenant_scope(),
                None,
                NewEvent::permission_denied(&principal("fifth"), OperationName::CreateItem),
            )
            .await,
    )?;
    assert_eq!(counted.code, ErrorCode::Unavailable);
    Ok(())
}

/// SPEC 11 records every refusal in its tenant's log; the bounded overflow must rotate,
/// rather than retain a new window per tenant/kind or reset an ordinary source's allowance.
#[tokio::test]
async fn overflow_rotation_preserves_tenant_kind_counts_and_existing_allowances() -> TestResult {
    let clock = TestClock::new();
    let inner = Arc::new(Recorded::default());
    let events = SecurityEvents::with_bound(inner.clone(), clock.clone(), 2);
    let ordinary = principal("ordinary");
    assert_eq!(deny(&events, &ordinary, 12).await?, 2);
    assert_eq!(deny(&events, &principal("second"), 1).await?, 0);
    let mut alpha = principal("alpha-agent");
    alpha.tenant_id = TenantId::try_from("org_alpha".to_owned())?;
    let mut beta = principal("beta-agent");
    beta.tenant_id = TenantId::try_from("org_beta".to_owned())?;
    assert_eq!(deny(&events, &alpha, 3).await?, 3);
    for _ in 0..2 {
        events.refused(Some(&alpha.tenant_id), "sign-in").await?;
    }
    let early = inner.all()?;
    let (_, mutation, aggregate) = some(early.last(), "the rotated alpha denial aggregate")?;
    assert_eq!(early.len(), 12);
    assert_eq!(mutation, &None);
    assert_eq!(aggregate.kind, EventKind::PermissionDenied);
    assert_eq!(aggregate.count, Some(3));
    assert!(aggregate.actor.is_none());
    assert_eq!(deny(&events, &beta, 4).await?, 4);
    assert_eq!(deny(&events, &alpha, 5).await?, 5);
    assert_eq!(deny(&events, &ordinary, 2).await?, 2);
    events.flush_all().await;
    let all = inner.all()?;
    assert_eq!(all.len(), 16);
    let mut aggregates = Vec::new();
    for (scope, mutation, event) in all {
        if let Some(count) = event.count {
            assert_eq!(mutation, None);
            assert_eq!(event.operation, None);
            assert_eq!(event.transport, None);
            aggregates.push((
                scope,
                event.kind,
                count,
                event.actor.map(|actor| actor.subject),
            ));
        }
    }
    assert_eq!(
        aggregates,
        vec![
            (
                EventScope::Tenant(alpha.tenant_id.clone()),
                EventKind::PermissionDenied,
                3,
                None
            ),
            (
                EventScope::Tenant(alpha.tenant_id.clone()),
                EventKind::SignInRefused,
                2,
                None
            ),
            (
                EventScope::Tenant(beta.tenant_id),
                EventKind::PermissionDenied,
                4,
                None
            ),
            (
                tenant_scope(),
                EventKind::PermissionDenied,
                4,
                Some("ordinary".to_owned())
            ),
            (
                EventScope::Tenant(alpha.tenant_id),
                EventKind::PermissionDenied,
                5,
                None
            ),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn overflow_does_not_consume_a_free_ordinary_source_slot() -> TestResult {
    let clock = TestClock::new();
    let inner = Arc::new(Recorded::default());
    let events = SecurityEvents::with_bound(inner.clone(), clock.clone(), 1);
    assert_eq!(deny(&events, &principal("old"), 1).await?, 0);
    clock.advance(Duration::from_millis(1))?;
    let mut other = principal("other");
    other.tenant_id = TenantId::try_from("org_other".to_owned())?;
    assert_eq!(deny(&events, &other, 1).await?, 1);
    clock.advance(Duration::from_millis(59_999))?;
    assert_eq!(deny(&events, &principal("new"), 1).await?, 0);
    events.flush_all().await;
    let all = inner.all()?;
    assert_eq!(all.len(), 3);
    let (scope, mutation, aggregate) = some(all.last(), "the remaining overflow aggregate")?;
    assert_eq!(scope, &EventScope::Tenant(other.tenant_id));
    assert_eq!(mutation, &None);
    assert_eq!(aggregate.kind, EventKind::PermissionDenied);
    assert_eq!(aggregate.count, Some(1));
    assert!(aggregate.actor.is_none());
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
