//! One allowance for security events: what one source can write in a minute is bounded, and
//! every refusal is still counted (owner decision 2026-10-14; contract `EventKind::SignInRefused`;
//! contract rulings 70 and 72).
//!
//! `SecurityEvents` wraps the one `EventLog` of the deployment, so every writer goes through it:
//! core's dispatch and the MCP server (which record a refused request as `permission_denied`),
//! the identity middleware, and the session routes. It bounds two kinds, appended without a
//! mutation id; every other append, and every read, passes through unchanged.
//!
//! - **The source.** A `permission_denied` is keyed by its actor (tenant, subject, route and
//!   client): this decorator sees no request, and an authenticated flood is best named by who
//!   sends it. A refused sign-in is keyed by the source the server's own code names
//!   (`SignInRefusals::refused`): one source for a local deployment.
//! - **The allowance.** A source's window opens at its first event and lasts [`WINDOW`]; it is
//!   over at exactly sixty seconds. The first [`ALLOWANCE`] events of a window are appended as
//!   they come. The rest are not appended but counted, and when the window is over one aggregate
//!   is appended to the tenant's log: the same kind, the source's actor (none for a sign-in),
//!   no operation, and `count`, the number not appended one by one. [`SecurityEvents::run`]
//!   writes the aggregates of the windows that are over, once a second, and those of every open
//!   window when it is stopped; a source whose next event opens a new window has its old
//!   aggregate written at that moment.
//! - **A counted append** answers `Unavailable` ("counted in an aggregate"). Every writer of
//!   these two kinds ignores the answer by its own rule (core's dispatch: "a failed append does
//!   not change the answer"); a new writer must too.
//! - **Bounds.** At most [`MAX_SOURCES`] ordinary sources have open windows, plus one overflow
//!   window. Past the ordinary bound, new sources are counted, never appended one by one.
//!   The overflow aggregate names no actor; switching its tenant or kind flushes the previous
//!   aggregate to its own tenant's log, without resetting ordinary allowances.
//! - **A refused sign-in with no tenant** (hosted, before any tenant is known) belongs to no log.
//!   It is counted, and once a minute the count is written on stderr.
//!
//! Known limits: a process that stops without its shutdown loses the counts of its open windows
//! (a minute's at most), so "every refusal is counted" holds for a clean stop. An aggregate whose
//! append fails is reported on stderr and not retried.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use std::time::Instant;

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{Event, EventActor, EventKind, ListEventsResponse};
use okf_jawn_contract::identity::{MutationId, TenantId};
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
use okf_jawn_core::ports::PortFuture;
use tokio_util::sync::CancellationToken;

use crate::grants::Clock;
use crate::identity::SignInRefusals;

/// The deployment's one event log, with the allowance on its security events.
pub struct SecurityEvents {
    inner: Arc<dyn EventLog>,
    clock: Arc<dyn Clock>,
    bound: usize,
    state: Mutex<State>,
}

/// Who a bounded event comes from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    /// The actor of a `permission_denied`: tenant, subject, route and client.
    Actor(String, String, u8, Option<String>),
    /// The source of a refused sign-in in a tenant.
    SignIn(String, String),
}

/// One source's minute.
struct Window {
    began: Instant,
    recorded: u32,
    counted: u32,
    aggregate: NewEvent,
    tenant: TenantId,
}

/// The open windows, and the refused sign-ins with no tenant.
#[derive(Default)]
struct State {
    windows: BTreeMap<Source, Window>,
    /// The one overflow window, tagged by event kind; its tenant stays in the window.
    overflow: Option<(u8, Window)>,
    no_tenant: Option<(Instant, u64)>,
}

/// What became of one bounded event.
enum Admission {
    /// Append it.
    Recorded,
    /// It was counted for its window's aggregate.
    Counted,
}

/// Events of one source in one window appended one by one.
pub const ALLOWANCE: u32 = 10;

/// How long one source's window lasts.
pub const WINDOW: Duration = Duration::from_secs(60);

/// The most ordinary sources with open windows, in addition to one overflow window.
pub const MAX_SOURCES: usize = 10_000;

/// How often [`SecurityEvents::run`] writes the aggregates of windows that are over.
const FLUSH_EVERY: Duration = Duration::from_secs(1);

impl SecurityEvents {
    /// The allowance over `inner`, timed by `clock`, with at most [`MAX_SOURCES`] ordinary
    /// sources and one overflow window.
    #[must_use]
    pub fn new(inner: Arc<dyn EventLog>, clock: Arc<dyn Clock>) -> Self {
        Self::with_bound(inner, clock, MAX_SOURCES)
    }

    /// [`Self::new`] with at most `bound` ordinary sources and one overflow window.
    /// A zero bound counts every refusal in the overflow window.
    #[must_use]
    pub fn with_bound(inner: Arc<dyn EventLog>, clock: Arc<dyn Clock>, bound: usize) -> Self {
        Self {
            inner,
            clock,
            bound,
            state: Mutex::new(State::default()),
        }
    }

    /// Append the aggregates of every window that is over, and report the sign-ins refused with
    /// no tenant in a minute that is over.
    pub async fn flush_due(&self) {
        let now = self.clock.now();
        let due = match self.lock() {
            Ok(mut state) => state.take(|began| over(began, now), now),
            Err(_) => (Vec::new(), None),
        };
        self.write(due).await;
    }

    /// Append the aggregates of every open window, whether or not it is over.
    pub async fn flush_all(&self) {
        let now = self.clock.now();
        let due = match self.lock() {
            Ok(mut state) => state.take(|_| true, now),
            Err(_) => (Vec::new(), None),
        };
        self.write(due).await;
    }

    /// Write due aggregates once a second until `stop`, then every open window's.
    pub async fn run(self: Arc<Self>, stop: CancellationToken) {
        let mut ticks = tokio::time::interval(FLUSH_EVERY);
        loop {
            tokio::select! {
                () = stop.cancelled() => break,
                _instant = ticks.tick() => self.flush_due().await,
            }
        }
        self.flush_all().await;
    }

    /// Admit one bounded event of `source` in `tenant`, answering the aggregate of a window it
    /// closed, if any.
    fn admit(
        &self,
        source: Source,
        tenant: &TenantId,
        aggregate: NewEvent,
        overflow_kind: u8,
    ) -> Result<(Admission, Vec<(TenantId, NewEvent)>), ApiError> {
        let now = self.clock.now();
        let mut state = self.lock()?;
        let mut closed = Vec::new();
        if let Some(window) = state.windows.get(&source)
            && over(window.began, now)
        {
            closed.extend(
                state
                    .windows
                    .remove(&source)
                    .and_then(Window::into_aggregate),
            );
        }
        if !state.windows.contains_key(&source) && state.windows.len() >= self.bound {
            // Past the bound: windows that are over make room first.
            closed.extend(state.take_windows(|began| over(began, now)));
        }
        let overflow = !state.windows.contains_key(&source) && state.windows.len() >= self.bound;
        let make_window = |aggregate| Window {
            began: now,
            recorded: 0,
            counted: 0,
            aggregate,
            tenant: tenant.clone(),
        };
        let window = if overflow {
            if state.overflow.as_ref().is_some_and(|(kind, window)| {
                *kind != overflow_kind || window.tenant != *tenant || over(window.began, now)
            }) {
                closed.extend(
                    state
                        .overflow
                        .take()
                        .and_then(|(_, window)| window.into_aggregate()),
                );
            }
            &mut state
                .overflow
                .get_or_insert_with(|| {
                    (
                        overflow_kind,
                        make_window(NewEvent {
                            actor: None,
                            ..aggregate
                        }),
                    )
                })
                .1
        } else {
            state
                .windows
                .entry(source)
                .or_insert_with(|| make_window(aggregate))
        };
        let admission = if !overflow && window.recorded < ALLOWANCE {
            window.recorded = window.recorded.saturating_add(1);
            Admission::Recorded
        } else {
            window.counted = window.counted.saturating_add(1);
            Admission::Counted
        };
        Ok((admission, closed))
    }

    /// Append each aggregate, and report the no-tenant count.
    async fn write(&self, due: (Vec<(TenantId, NewEvent)>, Option<u64>)) {
        let (aggregates, no_tenant) = due;
        for (tenant, aggregate) in aggregates {
            self.append_aggregate(&tenant, aggregate).await;
        }
        if let Some(count) = no_tenant {
            tracing::info!(
                count,
                "refused sign-ins in the last minute that named no tenant"
            );
        }
    }

    async fn append_aggregate(&self, tenant: &TenantId, aggregate: NewEvent) {
        let scope = EventScope::Tenant(tenant.clone());
        if let Err(error) = self.inner.append(&scope, None, aggregate).await {
            tracing::warn!(
                code = ?error.code,
                "an aggregate of refused requests could not be recorded"
            );
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>, ApiError> {
        self.state.lock().map_err(|_| {
            ApiError::new(
                ErrorCode::Internal,
                "the security-event allowance lock is poisoned",
            )
        })
    }
}

impl State {
    /// Remove the windows whose start `done` says is over, and answer their aggregates; and the
    /// no-tenant count of a minute that is over.
    fn take(
        &mut self,
        done: impl Fn(Instant) -> bool,
        now: Instant,
    ) -> (Vec<(TenantId, NewEvent)>, Option<u64>) {
        let aggregates = self.take_windows(&done);
        let no_tenant = match self.no_tenant {
            Some((began, count)) if done(began) || over(began, now) => {
                self.no_tenant = None;
                Some(count)
            }
            _ => None,
        };
        (aggregates, no_tenant)
    }

    fn take_windows(&mut self, done: impl Fn(Instant) -> bool) -> Vec<(TenantId, NewEvent)> {
        let finished: Vec<Source> = self
            .windows
            .iter()
            .filter(|(_, window)| done(window.began))
            .map(|(source, _)| source.clone())
            .collect();
        let mut aggregates: Vec<_> = finished
            .into_iter()
            .filter_map(|source| self.windows.remove(&source))
            .filter_map(Window::into_aggregate)
            .collect();
        if self
            .overflow
            .as_ref()
            .is_some_and(|(_, window)| done(window.began))
        {
            aggregates.extend(
                self.overflow
                    .take()
                    .and_then(|(_, window)| window.into_aggregate()),
            );
        }
        aggregates
    }
}

impl Window {
    /// The aggregate of a window that counted anything, with its tenant.
    fn into_aggregate(self) -> Option<(TenantId, NewEvent)> {
        (self.counted > 0).then_some({
            (
                self.tenant,
                NewEvent {
                    count: Some(self.counted),
                    ..self.aggregate
                },
            )
        })
    }
}

impl EventLog for SecurityEvents {
    fn append<'a>(
        &'a self,
        scope: &'a EventScope,
        mutation_id: Option<MutationId>,
        event: NewEvent,
    ) -> PortFuture<'a, Event> {
        if mutation_id.is_some() || event.kind != EventKind::PermissionDenied {
            return self.inner.append(scope, mutation_id, event);
        }
        Box::pin(async move {
            let tenant = match scope {
                EventScope::Tenant(tenant) => tenant.clone(),
                EventScope::Workspace(workspace) => workspace.tenant_id.clone(),
            };
            let source = actor_source(&tenant, event.actor.as_ref());
            let aggregate = NewEvent {
                operation: None,
                transport: None,
                ..event.clone()
            };
            let (admission, closed) = self.admit(source, &tenant, aggregate, 0)?;
            for (tenant, closed) in closed {
                self.append_aggregate(&tenant, closed).await;
            }
            match admission {
                Admission::Recorded => self.inner.append(scope, None, event).await,
                Admission::Counted => Err(counted()),
            }
        })
    }

    fn list<'a>(
        &'a self,
        scope: &'a EventScope,
        query: EventQuery,
    ) -> PortFuture<'a, ListEventsResponse> {
        self.inner.list(scope, query)
    }
}

impl SignInRefusals for SecurityEvents {
    fn refused<'a>(&'a self, tenant: Option<&'a TenantId>, source: &'a str) -> PortFuture<'a, ()> {
        Box::pin(async move {
            let Some(tenant) = tenant else {
                let now = self.clock.now();
                let mut state = self.lock()?;
                let (began, count) = state.no_tenant.unwrap_or((now, 0));
                state.no_tenant = Some((began, count.saturating_add(1)));
                return Ok(());
            };
            let refusal = NewEvent {
                kind: EventKind::SignInRefused,
                revision: None,
                item_id: None,
                job_id: None,
                connector_id: None,
                actor: None,
                operation: None,
                transport: None,
                proposal_id: None,
                count: None,
            };
            let key = Source::SignIn(tenant.as_str().to_owned(), source.to_owned());
            let (admission, closed) = self.admit(key, tenant, refusal.clone(), 1)?;
            for (tenant, closed) in closed {
                self.append_aggregate(&tenant, closed).await;
            }
            if let Admission::Recorded = admission {
                self.inner
                    .append(&EventScope::Tenant(tenant.clone()), None, refusal)
                    .await?;
            }
            Ok(())
        })
    }
}

/// Whether a window that began at `began` is over at `now`: at exactly [`WINDOW`].
fn over(began: Instant, now: Instant) -> bool {
    now.saturating_duration_since(began) >= WINDOW
}

/// The source of a `permission_denied` in `tenant` by `actor`.
fn actor_source(tenant: &TenantId, actor: Option<&EventActor>) -> Source {
    let route = |route: &AccessRoute| match route {
        AccessRoute::BrowserSession => 0,
        AccessRoute::McpDelegation => 1,
        AccessRoute::LocalOwner => 2,
        AccessRoute::Service => 3,
    };
    match actor {
        Some(actor) => Source::Actor(
            tenant.as_str().to_owned(),
            actor.subject.clone(),
            route(&actor.route),
            actor.client_id.clone(),
        ),
        None => Source::Actor(tenant.as_str().to_owned(), String::new(), u8::MAX, None),
    }
}

fn counted() -> ApiError {
    ApiError::new(
        ErrorCode::Unavailable,
        "Counted in this minute's aggregate refusal event",
    )
}
