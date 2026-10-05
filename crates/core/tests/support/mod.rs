//! Fixture AccessControl and MutationStore for dispatch tests; not production adapters.

pub mod counting;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use okf_jawn_contract::access::{Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{
    Digest, IdempotencyKey, MutationId, TenantId, WorkspaceId,
};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::dispatch::new_mutation_id;
use okf_jawn_core::mutations::{
    AbandonedEffects, BeginOutcome, MutationKey, MutationStore, StoredResponse,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::StorageScope;
use serde_json::Value;

/// Grant table keyed by subject.
#[derive(Debug, Default, Clone)]
pub struct GrantTable {
    /// Workspace grants: subject → workspace → permissions.
    pub workspaces: BTreeMap<String, BTreeMap<WorkspaceId, Vec<Permission>>>,
    /// Tenant grants: subject → permissions.
    pub tenants: BTreeMap<String, Vec<Permission>>,
}

/// AccessControl built from an explicit grant table.
#[derive(Debug, Default)]
pub struct FixtureAccess {
    /// Raw grants before route/delegation intersection.
    pub table: Mutex<GrantTable>,
}

impl FixtureAccess {
    /// Construct from a prepared table.
    #[must_use]
    pub fn new(table: GrantTable) -> Self {
        Self {
            table: Mutex::new(table),
        }
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
            let table = self.table.lock().map_err(|_| {
                ApiError::new(ErrorCode::Internal, "grant table lock poisoned")
            })?;
            let permissions = table
                .workspaces
                .get(&principal.subject)
                .and_then(|map| map.get(&workspace))
                .cloned()
                .ok_or_else(|| {
                    ApiError::new(ErrorCode::Forbidden, "Required capability is not granted")
                })?;
            Ok(WorkspaceGrant {
                scope: StorageScope {
                    tenant_id: principal.tenant_id.clone(),
                    workspace_id: workspace,
                },
                permissions,
            })
        })
    }

    fn authorize_tenant<'a>(
        &'a self,
        principal: &'a Principal,
        _permission: Permission,
    ) -> PortFuture<'a, TenantGrant> {
        Box::pin(async move {
            let table = self.table.lock().map_err(|_| {
                ApiError::new(ErrorCode::Internal, "grant table lock poisoned")
            })?;
            let permissions = table
                .tenants
                .get(&principal.subject)
                .cloned()
                .ok_or_else(|| {
                    ApiError::new(ErrorCode::Forbidden, "Required capability is not granted")
                })?;
            Ok(TenantGrant {
                tenant_id: principal.tenant_id.clone(),
                permissions,
            })
        })
    }

    fn grants<'a>(&'a self, principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        Box::pin(async move {
            let table = self.table.lock().map_err(|_| {
                ApiError::new(ErrorCode::Internal, "grant table lock poisoned")
            })?;
            let Some(map) = table.workspaces.get(&principal.subject) else {
                return Ok(Vec::new());
            };
            Ok(map
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
            let mut table = self.table.lock().map_err(|_| {
                ApiError::new(ErrorCode::Internal, "grant table lock poisoned")
            })?;
            let permissions = vec![
                Permission::Read,
                Permission::Write,
                Permission::Propose,
                Permission::Approve,
                Permission::Review,
                Permission::Admin,
            ];
            table
                .workspaces
                .entry(principal.subject.clone())
                .or_default()
                .insert(workspace, permissions.clone());
            Ok(WorkspaceGrant {
                scope: StorageScope {
                    tenant_id: principal.tenant_id.clone(),
                    workspace_id: workspace,
                },
                permissions,
            })
        })
    }
}

#[derive(Debug, Clone)]
enum LedgerState {
    InProgress {
        mutation_id: MutationId,
        digest: Digest,
        lease_until: Instant,
        effect: Option<Value>,
    },
    Completed {
        mutation_id: MutationId,
        digest: Digest,
        body: Value,
        completed_at: Instant,
    },
    Abandoned {
        mutation_id: MutationId,
        digest: Digest,
        effect: Option<Value>,
    },
}

/// MutationStore over a Mutex map; leases are wall-clock for tests.
#[derive(Debug, Default)]
pub struct FixtureMutations {
    rows: Mutex<BTreeMap<MutationKey, LedgerState>>,
    by_id: Mutex<BTreeMap<MutationId, MutationKey>>,
    /// Lease duration used for InProgress vs Abandoned.
    pub lease: Duration,
    /// Retention for completed rows (7 days in production).
    pub completed_ttl: Duration,
}

impl FixtureMutations {
    /// Construct with a short lease suitable for InProgress tests.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rows: Mutex::new(BTreeMap::new()),
            by_id: Mutex::new(BTreeMap::new()),
            lease: Duration::from_secs(30),
            completed_ttl: Duration::from_secs(7 * 24 * 60 * 60),
        }
    }

    /// Force an existing in-progress row into Abandoned without waiting for the lease.
    ///
    /// # Errors
    /// Returns when the key is missing or not in progress.
    pub fn force_abandon(&self, key: &MutationKey) -> Result<MutationId, ApiError> {
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "mutation lock poisoned"))?;
        match rows.get_mut(key) {
            Some(LedgerState::InProgress {
                mutation_id,
                digest,
                effect,
                ..
            }) => {
                let mutation_id = *mutation_id;
                let digest = digest.clone();
                let effect = effect.clone();
                *rows.get_mut(key).expect("key present") = LedgerState::Abandoned {
                    mutation_id,
                    digest,
                    effect,
                };
                Ok(mutation_id)
            }
            _ => Err(ApiError::new(
                ErrorCode::Internal,
                "force_abandon requires an in-progress row",
            )),
        }
    }

    /// Record an effect on an in-progress or abandoned row (simulates crash after create).
    ///
    /// # Errors
    /// Returns when the mutation is unknown.
    pub fn put_effect(&self, mutation_id: MutationId, effect: Value) -> Result<(), ApiError> {
        let key = {
            let by_id = self
                .by_id
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal, "mutation lock poisoned"))?;
            by_id.get(&mutation_id).cloned().ok_or_else(|| {
                ApiError::new(ErrorCode::NotFound, "mutation not found")
            })?
        };
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "mutation lock poisoned"))?;
        match rows.get_mut(&key) {
            Some(
                LedgerState::InProgress { effect: slot, .. }
                | LedgerState::Abandoned { effect: slot, .. },
            ) => {
                *slot = Some(effect);
                Ok(())
            }
            _ => Err(ApiError::new(
                ErrorCode::Internal,
                "effect requires an open mutation",
            )),
        }
    }
}

impl MutationStore for FixtureMutations {
    fn begin<'a>(
        &'a self,
        key: &'a MutationKey,
        digest: &'a Digest,
    ) -> PortFuture<'a, BeginOutcome> {
        Box::pin(async move {
            let mut rows = self
                .rows
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal, "mutation lock poisoned"))?;
            let mut by_id = self
                .by_id
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal, "mutation lock poisoned"))?;
            let now = Instant::now();
            match rows.get(key).cloned() {
                None => {
                    let mutation_id = new_mutation_id();
                    rows.insert(
                        key.clone(),
                        LedgerState::InProgress {
                            mutation_id,
                            digest: digest.clone(),
                            lease_until: now + self.lease,
                            effect: None,
                        },
                    );
                    by_id.insert(mutation_id, key.clone());
                    Ok(BeginOutcome::New(mutation_id))
                }
                Some(LedgerState::Completed {
                    mutation_id,
                    digest: prior,
                    body,
                    completed_at,
                }) => {
                    if now.duration_since(completed_at) > self.completed_ttl {
                        let mutation_id = new_mutation_id();
                        rows.insert(
                            key.clone(),
                            LedgerState::InProgress {
                                mutation_id,
                                digest: digest.clone(),
                                lease_until: now + self.lease,
                                effect: None,
                            },
                        );
                        by_id.insert(mutation_id, key.clone());
                        return Ok(BeginOutcome::New(mutation_id));
                    }
                    if &prior != digest {
                        return Ok(BeginOutcome::Conflict {
                            operation: key.operation,
                        });
                    }
                    Ok(BeginOutcome::Replay(StoredResponse { mutation_id, body }))
                }
                Some(LedgerState::InProgress {
                    mutation_id,
                    digest: prior,
                    lease_until,
                    effect,
                }) => {
                    if &prior != digest {
                        return Ok(BeginOutcome::Conflict {
                            operation: key.operation,
                        });
                    }
                    if now < lease_until {
                        let retry_after = lease_until
                            .saturating_duration_since(now)
                            .as_secs()
                            .max(1) as u32;
                        return Ok(BeginOutcome::InProgress {
                            mutation_id,
                            retry_after,
                        });
                    }
                    rows.insert(
                        key.clone(),
                        LedgerState::InProgress {
                            mutation_id,
                            digest: prior,
                            lease_until: now + self.lease,
                            effect,
                        },
                    );
                    Ok(BeginOutcome::Abandoned { mutation_id })
                }
                Some(LedgerState::Abandoned {
                    mutation_id,
                    digest: prior,
                    effect,
                }) => {
                    if &prior != digest {
                        return Ok(BeginOutcome::Conflict {
                            operation: key.operation,
                        });
                    }
                    rows.insert(
                        key.clone(),
                        LedgerState::InProgress {
                            mutation_id,
                            digest: prior,
                            lease_until: now + self.lease,
                            effect,
                        },
                    );
                    Ok(BeginOutcome::Abandoned { mutation_id })
                }
            }
        })
    }

    fn record_effect<'a>(
        &'a self,
        mutation_id: MutationId,
        effect: Value,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move { self.put_effect(mutation_id, effect) })
    }

    fn complete<'a>(&'a self, mutation_id: MutationId, response: Value) -> PortFuture<'a, ()> {
        Box::pin(async move {
            let key = {
                let by_id = self
                    .by_id
                    .lock()
                    .map_err(|_| ApiError::new(ErrorCode::Internal, "mutation lock poisoned"))?;
                by_id.get(&mutation_id).cloned().ok_or_else(|| {
                    ApiError::new(ErrorCode::NotFound, "mutation not found")
                })?
            };
            let mut rows = self
                .rows
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal, "mutation lock poisoned"))?;
            let digest = match rows.get(&key) {
                Some(
                    LedgerState::InProgress { digest, .. }
                    | LedgerState::Abandoned { digest, .. }
                    | LedgerState::Completed { digest, .. },
                ) => digest.clone(),
                None => {
                    return Err(ApiError::new(ErrorCode::NotFound, "mutation not found"));
                }
            };
            rows.insert(
                key,
                LedgerState::Completed {
                    mutation_id,
                    digest,
                    body: response,
                    completed_at: Instant::now(),
                },
            );
            Ok(())
        })
    }

    fn find<'a>(&'a self, mutation_id: MutationId) -> PortFuture<'a, Option<StoredResponse>> {
        Box::pin(async move {
            let key = {
                let by_id = self
                    .by_id
                    .lock()
                    .map_err(|_| ApiError::new(ErrorCode::Internal, "mutation lock poisoned"))?;
                match by_id.get(&mutation_id).cloned() {
                    Some(key) => key,
                    None => return Ok(None),
                }
            };
            let rows = self
                .rows
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal, "mutation lock poisoned"))?;
            Ok(match rows.get(&key) {
                Some(LedgerState::Completed {
                    mutation_id,
                    body,
                    ..
                }) => Some(StoredResponse {
                    mutation_id: *mutation_id,
                    body: body.clone(),
                }),
                Some(
                    LedgerState::InProgress {
                        mutation_id,
                        effect: Some(body),
                        ..
                    }
                    | LedgerState::Abandoned {
                        mutation_id,
                        effect: Some(body),
                        ..
                    },
                ) => Some(StoredResponse {
                    mutation_id: *mutation_id,
                    body: body.clone(),
                }),
                _ => None,
            })
        })
    }
}

/// Abandoned effect lookup table for Git and non-Git creating stores.
#[derive(Debug, Default)]
pub struct FixtureEffects {
    /// operation + mutation_id → response body already produced.
    pub rows: Mutex<BTreeMap<(OperationName, MutationId), Value>>,
}

impl FixtureEffects {
    /// Record that a creating store already holds a row for this mutation.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn insert(
        &self,
        operation: OperationName,
        mutation_id: MutationId,
        body: Value,
    ) -> Result<(), ApiError> {
        self.rows
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "effects lock poisoned"))?
            .insert((operation, mutation_id), body);
        Ok(())
    }
}

impl AbandonedEffects for FixtureEffects {
    fn lookup<'a>(
        &'a self,
        operation: OperationName,
        mutation_id: MutationId,
    ) -> PortFuture<'a, Option<Value>> {
        Box::pin(async move {
            let rows = self
                .rows
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal, "effects lock poisoned"))?;
            Ok(rows.get(&(operation, mutation_id)).cloned())
        })
    }
}

/// Shared Arc wrappers used by dispatch tests.
pub struct FixturePorts {
    /// Grant table.
    pub access: Arc<FixtureAccess>,
    /// Mutation ledger.
    pub mutations: Arc<FixtureMutations>,
    /// Abandoned creating-store lookup.
    pub effects: Arc<FixtureEffects>,
}

impl FixturePorts {
    /// Construct empty fixtures.
    #[must_use]
    pub fn new(table: GrantTable) -> Self {
        Self {
            access: Arc::new(FixtureAccess::new(table)),
            mutations: Arc::new(FixtureMutations::new()),
            effects: Arc::new(FixtureEffects::default()),
        }
    }
}

/// Parse a fixed workspace UUID used in tests.
#[must_use]
pub fn workspace(hex: &str) -> WorkspaceId {
    serde_json::from_str(&format!("\"{hex}\"")).expect("valid workspace uuid")
}

/// Parse a fixed tenant id used in tests.
#[must_use]
pub fn tenant(value: &str) -> TenantId {
    TenantId::try_from(value.to_owned()).expect("valid tenant id")
}

/// Parse a fixed idempotency key used in tests.
#[must_use]
pub fn idempotency_key(hex: &str) -> IdempotencyKey {
    serde_json::from_str(&format!("\"{hex}\"")).expect("valid idempotency uuid")
}
