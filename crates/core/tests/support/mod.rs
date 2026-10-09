//! Fixture `AccessControl`, `MutationStore` and `EventLog` for dispatch and binding tests; not
//! production adapters.
//!
//! The including test target declares `mod check;` (`tests/support/check.rs`) at its crate
//! root. Every public item here is exercised by the self-tests at the end of this file, so a
//! target that uses only part of the fixture still compiles without dead-code warnings.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use okf_jawn_contract::access::{Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{Event, ListEventsResponse};
use okf_jawn_contract::identity::{
    Digest, IdempotencyKey, IdentityError, MutationId, TenantId, Timestamp, WorkspaceId,
};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::dispatch::new_mutation_id;
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
use okf_jawn_core::mutations::{
    BeginOutcome, MutationKey, MutationLease, MutationStore, StoredResponse,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::StorageScope;
use serde_json::Value;

/// Grant table keyed by subject.
#[derive(Debug, Default, Clone)]
pub struct GrantTable {
    /// Workspace grants: subject, then workspace, then permissions.
    pub workspaces: BTreeMap<String, BTreeMap<WorkspaceId, Vec<Permission>>>,
    /// Tenant grants: subject, then permissions.
    pub tenants: BTreeMap<String, Vec<Permission>>,
}

/// `AccessControl` built from an explicit grant table.
///
/// It answers for the scope that was asked unless a test tells it to answer for another one,
/// which is how a misbehaving adapter is simulated.
#[derive(Debug, Default)]
pub struct FixtureAccess {
    table: Mutex<GrantTable>,
    workspace_answer: Mutex<Option<StorageScope>>,
    tenant_answer: Mutex<Option<TenantId>>,
    lookups: AtomicUsize,
}

/// One ledger row: its identity, the digest it was begun with, the token of its latest grant,
/// and where it stands.
#[derive(Debug, Clone)]
struct Row {
    mutation_id: MutationId,
    digest: Digest,
    token: u64,
    phase: Phase,
}

#[derive(Debug, Clone)]
enum Phase {
    /// A handler holds the lease until this instant.
    Leased { until: Instant },
    /// The lease was released after a handler error; the next `begin` resumes at once.
    Released,
    /// The handler finished and its response is retained.
    Completed { body: Value },
}

#[derive(Debug, Default)]
struct Ledger {
    by_key: BTreeMap<MutationKey, Row>,
    by_id: BTreeMap<MutationId, MutationKey>,
    /// Number of leases granted so far; also the token of the latest grant.
    granted: u64,
}

/// A `MutationStore` call that a test can make fail once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LedgerCall {
    /// `MutationStore::begin`.
    Begin,
    /// `MutationStore::complete`.
    Complete,
    /// `MutationStore::release`.
    Release,
}

/// `MutationStore` over a `Mutex` map; leases use the wall clock, and every grant carries a
/// token no earlier grant carried.
#[derive(Debug)]
pub struct FixtureMutations {
    ledger: Mutex<Ledger>,
    failures: Mutex<BTreeMap<LedgerCall, ApiError>>,
    lease: Duration,
}

/// The fixture ports one dispatch needs, shared by reference-counted handles.
pub struct FixturePorts {
    /// Grant table.
    pub access: Arc<FixtureAccess>,
    /// Mutation ledger.
    pub mutations: Arc<FixtureMutations>,
    /// Event log that keeps what was appended.
    pub events: Arc<FixtureEvents>,
}

/// `EventLog` that keeps every appended event with its scope, in order.
#[derive(Debug, Default)]
pub struct FixtureEvents {
    appended: Mutex<Vec<(EventScope, NewEvent)>>,
}

impl FixtureEvents {
    /// Every event appended so far, with its scope, oldest first.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn appended(&self) -> Result<Vec<(EventScope, NewEvent)>, ApiError> {
        Ok(lock(&self.appended, "event log")?.clone())
    }
}

impl EventLog for FixtureEvents {
    fn append<'a>(
        &'a self,
        scope: &'a EventScope,
        _mutation_id: Option<MutationId>,
        event: NewEvent,
    ) -> PortFuture<'a, Event> {
        Box::pin(async move {
            let mut appended = lock(&self.appended, "event log")?;
            appended.push((scope.clone(), event.clone()));
            wire_event(appended.len(), scope, event)
        })
    }

    fn list<'a>(
        &'a self,
        scope: &'a EventScope,
        _query: EventQuery,
    ) -> PortFuture<'a, ListEventsResponse> {
        Box::pin(async move {
            let appended = lock(&self.appended, "event log")?;
            let events = appended
                .iter()
                .enumerate()
                .filter(|(_, (stored, _))| stored == scope)
                .map(|(index, (stored, event))| {
                    wire_event(index.saturating_add(1), stored, event.clone())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ListEventsResponse {
                events,
                next_cursor: None,
            })
        })
    }
}

impl FixtureAccess {
    /// Construct from a prepared table.
    #[must_use]
    pub fn new(table: GrantTable) -> Self {
        Self {
            table: Mutex::new(table),
            ..Self::default()
        }
    }

    /// Answer every later workspace lookup with a grant for `scope`, whatever was asked.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn answer_workspaces_as(&self, scope: StorageScope) -> Result<(), ApiError> {
        *lock(&self.workspace_answer, "workspace answer")? = Some(scope);
        Ok(())
    }

    /// Answer every later tenant lookup with a grant for `tenant`, whoever asked.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn answer_tenants_as(&self, tenant: TenantId) -> Result<(), ApiError> {
        *lock(&self.tenant_answer, "tenant answer")? = Some(tenant);
        Ok(())
    }

    /// Number of `authorize` and `authorize_tenant` lookups made so far.
    #[must_use]
    pub fn lookups(&self) -> usize {
        self.lookups.load(Ordering::SeqCst)
    }
}

impl AccessControl for FixtureAccess {
    fn authorize<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
        _permission: Permission,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            let permissions = lock(&self.table, "grant table")?
                .workspaces
                .get(&principal.subject)
                .and_then(|granted| granted.get(&workspace))
                .cloned()
                .ok_or_else(not_granted)?;
            let scope = lock(&self.workspace_answer, "workspace answer")?
                .clone()
                .unwrap_or_else(|| StorageScope {
                    tenant_id: principal.tenant_id.clone(),
                    workspace_id: workspace,
                });
            Ok(WorkspaceGrant { scope, permissions })
        })
    }

    fn authorize_tenant<'a>(
        &'a self,
        principal: &'a Principal,
        _permission: Permission,
    ) -> PortFuture<'a, TenantGrant> {
        Box::pin(async move {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            let permissions = lock(&self.table, "grant table")?
                .tenants
                .get(&principal.subject)
                .cloned()
                .ok_or_else(not_granted)?;
            let tenant_id = lock(&self.tenant_answer, "tenant answer")?
                .clone()
                .unwrap_or_else(|| principal.tenant_id.clone());
            Ok(TenantGrant {
                tenant_id,
                permissions,
            })
        })
    }

    fn grants<'a>(&'a self, principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        Box::pin(async move {
            let table = lock(&self.table, "grant table")?;
            let Some(granted) = table.workspaces.get(&principal.subject) else {
                return Ok(Vec::new());
            };
            Ok(granted
                .iter()
                .map(|(workspace_id, permissions)| WorkspaceGrant {
                    scope: StorageScope {
                        tenant_id: principal.tenant_id.clone(),
                        workspace_id: *workspace_id,
                    },
                    permissions: permissions.clone(),
                })
                .collect())
        })
    }

    fn grant_creator<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move {
            lock(&self.table, "grant table")?
                .workspaces
                .entry(principal.subject.clone())
                .or_default()
                .insert(workspace, all_permissions());
            Ok(WorkspaceGrant {
                scope: StorageScope {
                    tenant_id: principal.tenant_id.clone(),
                    workspace_id: workspace,
                },
                permissions: all_permissions(),
            })
        })
    }
}

impl Row {
    /// Whether `lease` is the current grant on this row: the latest token, and still leased.
    ///
    /// A lease whose time ran out stays the current grant until another attempt takes it over.
    fn held_by(&self, lease: MutationLease) -> bool {
        self.token == lease.token && matches!(self.phase, Phase::Leased { .. })
    }
}

impl Ledger {
    fn row_mut(&mut self, mutation_id: MutationId) -> Result<&mut Row, ApiError> {
        let key = self.by_id.get(&mutation_id).ok_or_else(unknown_mutation)?;
        self.by_key.get_mut(key).ok_or_else(unknown_mutation)
    }

    /// Grant a lease on `mutation_id` under a token no earlier grant carried.
    fn grant(&mut self, mutation_id: MutationId) -> Result<MutationLease, ApiError> {
        self.granted = self
            .granted
            .checked_add(1)
            .ok_or_else(|| ApiError::new(ErrorCode::Internal, "lease tokens are exhausted"))?;
        Ok(MutationLease {
            mutation_id,
            token: self.granted,
        })
    }
}

impl FixtureMutations {
    /// Construct an empty ledger with a 30-second lease.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ledger: Mutex::new(Ledger::default()),
            failures: Mutex::new(BTreeMap::new()),
            lease: Duration::from_secs(30),
        }
    }

    /// Make the next `call` fail with `error` without touching the ledger; later calls answer
    /// normally. This is how an unreachable or failing store is simulated.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn fail_next(&self, call: LedgerCall, error: ApiError) -> Result<(), ApiError> {
        lock(&self.failures, "ledger failures")?.insert(call, error);
        Ok(())
    }

    /// Take the failure scripted for `call`, if a test set one.
    fn scripted_failure(&self, call: LedgerCall) -> Result<(), ApiError> {
        match lock(&self.failures, "ledger failures")?.remove(&call) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Let every live lease lapse, as if its holder had crashed and the lease time had passed.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn expire_leases(&self) -> Result<(), ApiError> {
        let now = Instant::now();
        for row in lock(&self.ledger, "mutation ledger")?.by_key.values_mut() {
            if let Phase::Leased { until } = &mut row.phase {
                *until = now;
            }
        }
        Ok(())
    }

    /// The response body retained for a completed mutation; `None` until it completes.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn stored_body(&self, mutation_id: MutationId) -> Result<Option<Value>, ApiError> {
        let ledger = lock(&self.ledger, "mutation ledger")?;
        let Some(Phase::Completed { body }) = ledger
            .by_id
            .get(&mutation_id)
            .and_then(|key| ledger.by_key.get(key))
            .map(|row| &row.phase)
        else {
            return Ok(None);
        };
        Ok(Some(body.clone()))
    }
}

impl Default for FixtureMutations {
    fn default() -> Self {
        Self::new()
    }
}

impl MutationStore for FixtureMutations {
    fn begin<'a>(
        &'a self,
        key: &'a MutationKey,
        digest: &'a Digest,
    ) -> PortFuture<'a, BeginOutcome> {
        Box::pin(async move {
            self.scripted_failure(LedgerCall::Begin)?;
            let mut ledger = lock(&self.ledger, "mutation ledger")?;
            let now = Instant::now();
            let lease_until = now
                .checked_add(self.lease)
                .ok_or_else(|| ApiError::new(ErrorCode::Internal, "lease overflows the clock"))?;
            let Some(row) = ledger.by_key.get(key).cloned() else {
                let lease = ledger.grant(new_mutation_id())?;
                ledger.by_key.insert(
                    key.clone(),
                    Row {
                        mutation_id: lease.mutation_id,
                        digest: digest.clone(),
                        token: lease.token,
                        phase: Phase::Leased { until: lease_until },
                    },
                );
                ledger.by_id.insert(lease.mutation_id, key.clone());
                return Ok(BeginOutcome::New(lease));
            };
            if row.digest != *digest {
                return Ok(BeginOutcome::Conflict {
                    operation: key.operation,
                });
            }
            let mutation_id = row.mutation_id;
            match row.phase {
                Phase::Completed { body } => {
                    Ok(BeginOutcome::Replay(StoredResponse { mutation_id, body }))
                }
                Phase::Leased { until } if now < until => Ok(BeginOutcome::InProgress {
                    mutation_id,
                    retry_after: whole_seconds(until.saturating_duration_since(now)),
                }),
                Phase::Leased { .. } | Phase::Released => {
                    let lease = ledger.grant(mutation_id)?;
                    ledger.by_key.insert(
                        key.clone(),
                        Row {
                            mutation_id,
                            digest: row.digest,
                            token: lease.token,
                            phase: Phase::Leased { until: lease_until },
                        },
                    );
                    Ok(BeginOutcome::Abandoned { lease })
                }
            }
        })
    }

    fn complete(&self, lease: MutationLease, response: Value) -> PortFuture<'_, ()> {
        Box::pin(async move {
            self.scripted_failure(LedgerCall::Complete)?;
            let mut ledger = lock(&self.ledger, "mutation ledger")?;
            let row = ledger.row_mut(lease.mutation_id)?;
            if !row.held_by(lease) {
                return Err(lease_lost());
            }
            row.phase = Phase::Completed { body: response };
            Ok(())
        })
    }

    fn release(&self, lease: MutationLease) -> PortFuture<'_, ()> {
        Box::pin(async move {
            self.scripted_failure(LedgerCall::Release)?;
            let mut ledger = lock(&self.ledger, "mutation ledger")?;
            let row = ledger.row_mut(lease.mutation_id)?;
            if row.held_by(lease) {
                row.phase = Phase::Released;
            }
            Ok(())
        })
    }
}

impl FixturePorts {
    /// Construct fixtures over `table` with an empty ledger.
    #[must_use]
    pub fn new(table: GrantTable) -> Self {
        Self {
            access: Arc::new(FixtureAccess::new(table)),
            mutations: Arc::new(FixtureMutations::new()),
            events: Arc::new(FixtureEvents::default()),
        }
    }
}

/// The wire form of the `cursor`-th appended event, stamped with a fixed instant.
fn wire_event(cursor: usize, scope: &EventScope, event: NewEvent) -> Result<Event, ApiError> {
    Ok(Event {
        id: cursor.to_string(),
        workspace_id: match scope {
            EventScope::Tenant(_) => None,
            EventScope::Workspace(scope) => Some(scope.workspace_id),
        },
        kind: event.kind,
        at: fixed_instant()?,
        revision: event.revision,
        item_id: event.item_id,
        job_id: event.job_id,
        connector_id: event.connector_id,
        actor: event.actor,
        operation: event.operation,
    })
}

/// A valid instant for fixture records; `Timestamp` accepts this spelling.
fn fixed_instant() -> Result<Timestamp, ApiError> {
    Timestamp::try_from("2026-10-08T12:00:00.000Z".to_owned())
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
}

/// Lock a fixture mutex, turning poisoning into an error instead of a panic.
fn lock<'a, T>(mutex: &'a Mutex<T>, what: &str) -> Result<MutexGuard<'a, T>, ApiError> {
    mutex
        .lock()
        .map_err(|_| ApiError::new(ErrorCode::Internal, format!("{what} lock poisoned")))
}

fn not_granted() -> ApiError {
    ApiError::new(ErrorCode::Forbidden, "Required capability is not granted")
}

fn unknown_mutation() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "mutation not found")
}

fn lease_lost() -> ApiError {
    ApiError::new(
        ErrorCode::Conflict,
        "The lease on this mutation is held by another attempt",
    )
}

/// Whole seconds a caller should wait, never less than one.
fn whole_seconds(wait: Duration) -> u32 {
    u32::try_from(wait.as_secs()).unwrap_or(u32::MAX).max(1)
}

/// Every permission, in declaration order.
#[must_use]
pub fn all_permissions() -> Vec<Permission> {
    vec![
        Permission::Read,
        Permission::Write,
        Permission::Propose,
        Permission::Approve,
        Permission::Review,
        Permission::Admin,
    ]
}

/// Parse a fixed workspace UUID used in tests.
///
/// # Errors
/// Returns when `hex` is not a UUID.
pub fn workspace(hex: &str) -> Result<WorkspaceId, serde_json::Error> {
    serde_json::from_value(Value::String(hex.to_owned()))
}

/// Parse a fixed tenant id used in tests.
///
/// # Errors
/// Returns when `value` is not a valid tenant id.
pub fn tenant(value: &str) -> Result<TenantId, IdentityError> {
    TenantId::try_from(value.to_owned())
}

/// Parse a fixed idempotency key used in tests.
///
/// # Errors
/// Returns when `hex` is not a UUID.
pub fn idempotency_key(hex: &str) -> Result<IdempotencyKey, serde_json::Error> {
    serde_json::from_value(Value::String(hex.to_owned()))
}

pub mod counting;

#[cfg(test)]
mod tests {
    use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
    use okf_jawn_contract::error::{ApiError, ErrorCode};
    use okf_jawn_contract::identity::{Digest, IdentityError};
    use okf_jawn_contract::metadata::OperationName;
    use okf_jawn_core::access::AccessControl;
    use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
    use okf_jawn_core::mutations::{BeginOutcome, MutationKey, MutationStore};
    use okf_jawn_core::storage::{Page, StorageScope};
    use serde_json::json;

    use super::{
        FixturePorts, GrantTable, LedgerCall, all_permissions, idempotency_key, tenant, workspace,
    };
    use crate::check::{TestResult, err_of};

    const HOME: &str = "11111111-1111-4111-8111-111111111111";
    const ELSEWHERE: &str = "22222222-2222-4222-8222-222222222222";

    fn digest_of(letter: &str) -> Result<Digest, IdentityError> {
        letter.repeat(64).parse()
    }

    #[tokio::test]
    async fn fixture_events_keep_each_append_in_its_own_log() -> TestResult {
        let ports = FixturePorts::new(GrantTable::default());
        let alice = Principal {
            subject: "alice".to_owned(),
            tenant_id: tenant("tenant-local")?,
            route: AccessRoute::Service,
            client_id: None,
            delegation: None,
        };
        let tenant_log = EventScope::Tenant(tenant("tenant-local")?);
        let home = EventScope::Workspace(StorageScope {
            tenant_id: tenant("tenant-local")?,
            workspace_id: workspace(HOME)?,
        });
        let event = NewEvent::permission_denied(&alice, OperationName::PurgeItem);
        let appended = ports
            .events
            .append(&tenant_log, None, event.clone())
            .await?;
        assert_eq!(appended.workspace_id, None);
        assert_eq!(appended.operation, Some(OperationName::PurgeItem));
        ports.events.append(&home, None, event.clone()).await?;
        assert_eq!(
            ports.events.appended()?,
            vec![(tenant_log.clone(), event.clone()), (home.clone(), event)]
        );
        let query = || EventQuery {
            after: None,
            page: Page {
                cursor: None,
                limit: 10,
            },
        };
        assert_eq!(
            ports.events.list(&tenant_log, query()).await?.events.len(),
            1
        );
        let listed = ports.events.list(&home, query()).await?;
        assert_eq!(listed.events.len(), 1);
        assert_eq!(
            listed.events.first().and_then(|event| event.workspace_id),
            Some(workspace(HOME)?)
        );
        Ok(())
    }

    #[tokio::test]
    async fn fixture_ledger_leases_releases_resumes_and_replays() -> TestResult {
        let ports = FixturePorts::new(GrantTable::default());
        let ledger = ports.mutations.as_ref();
        let key = MutationKey {
            tenant_id: tenant("tenant-local")?,
            subject: "alice".to_owned(),
            client_id: None,
            operation: OperationName::CreateItem,
            key: idempotency_key("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")?,
        };
        let digest = digest_of("a")?;
        let BeginOutcome::New(first) = ledger.begin(&key, &digest).await? else {
            return Err("the first begin must be New".into());
        };
        let id = first.mutation_id;
        assert!(matches!(
            ledger.begin(&key, &digest).await?,
            BeginOutcome::InProgress { mutation_id, retry_after }
                if mutation_id == id && retry_after >= 1
        ));
        assert!(matches!(
            ledger.begin(&key, &digest_of("b")?).await?,
            BeginOutcome::Conflict {
                operation: OperationName::CreateItem
            }
        ));

        ledger.release(first).await?;
        let BeginOutcome::Abandoned { lease: second } = ledger.begin(&key, &digest).await? else {
            return Err("a released mutation must resume as Abandoned".into());
        };
        assert_eq!(second.mutation_id, id);
        assert_ne!(second.token, first.token);

        // The first lease is stale: it can neither end nor complete the second grant.
        ledger.release(first).await?;
        assert!(matches!(
            ledger.begin(&key, &digest).await?,
            BeginOutcome::InProgress { mutation_id, .. } if mutation_id == id
        ));
        let stale = err_of(ledger.complete(first, json!({ "stale": true })).await)?;
        assert_eq!(stale.code, ErrorCode::Conflict);
        assert_eq!(ledger.stored_body(id)?, None);

        ledger.expire_leases()?;
        let BeginOutcome::Abandoned { lease: third } = ledger.begin(&key, &digest).await? else {
            return Err("an expired lease must resume as Abandoned".into());
        };
        assert_eq!(third.mutation_id, id);
        assert_ne!(third.token, second.token);
        let expired = err_of(ledger.complete(second, json!({ "stale": true })).await)?;
        assert_eq!(expired.code, ErrorCode::Conflict);

        ledger.complete(third, json!({ "done": true })).await?;
        ledger.release(third).await?;
        assert_eq!(ledger.stored_body(id)?, Some(json!({ "done": true })));
        let BeginOutcome::Replay(stored) = ledger.begin(&key, &digest).await? else {
            return Err("a completed mutation must replay".into());
        };
        assert_eq!(stored.mutation_id, id);
        assert_eq!(stored.body, json!({ "done": true }));
        Ok(())
    }

    #[tokio::test]
    async fn fixture_ledger_fails_one_scripted_call_and_leaves_the_row_alone() -> TestResult {
        let ports = FixturePorts::new(GrantTable::default());
        let ledger = ports.mutations.as_ref();
        let key = MutationKey {
            tenant_id: tenant("tenant-local")?,
            subject: "alice".to_owned(),
            client_id: None,
            operation: OperationName::CreateItem,
            key: idempotency_key("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")?,
        };
        let digest = digest_of("a")?;
        let offline = || ApiError::new(ErrorCode::Unavailable, "ledger offline");

        ledger.fail_next(LedgerCall::Begin, offline())?;
        let refused = err_of(ledger.begin(&key, &digest).await)?;
        assert_eq!(refused.code, ErrorCode::Unavailable);
        // The failed begin wrote nothing, so the next one starts the mutation.
        let BeginOutcome::New(lease) = ledger.begin(&key, &digest).await? else {
            return Err("the begin after a failed begin must be New".into());
        };

        ledger.fail_next(LedgerCall::Release, offline())?;
        let refused = err_of(ledger.release(lease).await)?;
        assert_eq!(refused.code, ErrorCode::Unavailable);
        assert!(matches!(
            ledger.begin(&key, &digest).await?,
            BeginOutcome::InProgress { .. }
        ));

        ledger.fail_next(LedgerCall::Complete, offline())?;
        let refused = err_of(ledger.complete(lease, json!({ "done": true })).await)?;
        assert_eq!(refused.code, ErrorCode::Unavailable);
        assert_eq!(ledger.stored_body(lease.mutation_id)?, None);
        ledger.complete(lease, json!({ "done": true })).await?;
        assert_eq!(
            ledger.stored_body(lease.mutation_id)?,
            Some(json!({ "done": true }))
        );
        Ok(())
    }

    #[tokio::test]
    async fn fixture_access_answers_from_its_table_unless_told_otherwise() -> TestResult {
        let alice = Principal {
            subject: "alice".to_owned(),
            tenant_id: tenant("tenant-local")?,
            route: AccessRoute::LocalOwner,
            client_id: None,
            delegation: None,
        };
        let home = workspace(HOME)?;
        let elsewhere = workspace(ELSEWHERE)?;
        let mut table = GrantTable::default();
        table
            .workspaces
            .entry("alice".to_owned())
            .or_default()
            .insert(home, vec![Permission::Read]);
        table.tenants.insert("alice".to_owned(), all_permissions());
        let ports = FixturePorts::new(table);
        let access = ports.access.as_ref();

        assert_eq!(access.lookups(), 0);
        let granted = access.authorize(&alice, home, Permission::Read).await?;
        assert_eq!(granted.workspace_id(), home);
        assert_eq!(granted.permissions, vec![Permission::Read]);
        let refused = err_of(access.authorize(&alice, elsewhere, Permission::Read).await)?;
        assert_eq!(refused.code, ErrorCode::Forbidden);
        let tenant_grant = access.authorize_tenant(&alice, Permission::Admin).await?;
        assert_eq!(tenant_grant.tenant_id, alice.tenant_id);
        assert_eq!(access.lookups(), 3);

        let created = access.grant_creator(&alice, elsewhere).await?;
        assert_eq!(created.permissions, all_permissions());
        assert_eq!(access.grants(&alice).await?.len(), 2);

        access.answer_workspaces_as(StorageScope {
            tenant_id: tenant("tenant-other")?,
            workspace_id: elsewhere,
        })?;
        access.answer_tenants_as(tenant("tenant-other")?)?;
        let misrouted = access.authorize(&alice, home, Permission::Read).await?;
        assert_eq!(misrouted.workspace_id(), elsewhere);
        assert_eq!(misrouted.scope.tenant_id, tenant("tenant-other")?);
        let foreign = access.authorize_tenant(&alice, Permission::Admin).await?;
        assert_eq!(foreign.tenant_id, tenant("tenant-other")?);
        Ok(())
    }
}
