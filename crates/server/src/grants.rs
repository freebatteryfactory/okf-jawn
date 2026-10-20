//! The hosted grant cache: `Read`, `Write` and `Propose` for at most 60 seconds, nothing else.
//!
//! SPEC section 11: "The hosted adapter may cache Read, Write and Propose grants for at most 60
//! seconds; Approve, Review and Admin are always checked fresh. The local adapter does not
//! cache." `CachedAccess` wraps the hosted raw adapter:
//!
//! - A check of a cacheable permission (`core::access::may_cache`) is answered from a raw grant
//!   fetched less than [`GRANT_CACHE_TTL`] ago for the same caller and workspace (or tenant).
//!   What a cached answer carries is only its cacheable permissions: an `approve`, `review` or
//!   `admin` fetched earlier never leaves the cache, so nothing downstream can act on one that
//!   may have been revoked since.
//! - A check of `approve`, `review` or `admin` always asks the source, and its fresh answer
//!   refreshes the cache.
//! - `grants`, `editors` and `grant_creator` always ask the source; `grant_creator` replaces the
//!   cached grant of that workspace.
//! - A failed lookup is never cached.
//! - An entry is stamped with the instant its lookup began, not the instant the answer arrived,
//!   and an answer never replaces one whose lookup began later: a slow source cannot stretch
//!   the 60 seconds, and a late, older answer cannot overwrite a newer one.
//! - Each of the two maps (workspace grants, tenant grants) holds at most
//!   [`GRANT_CACHE_MAX_ENTRIES`] entries, so a flood of distinct lookups cannot grow it without
//!   end (server M1 review, N-g): an insert drops the stale entries first, and when the map is
//!   still full it evicts the entry whose lookup began first. An entry refreshed in place takes no
//!   place of another. An empty grant is cached like any other: that keeps a flood of one unknown
//!   workspace id at one lookup a minute.
//!
//! The caller is keyed by tenant, subject, route and client: a connector and its owner, or two
//! clients of one subject, never share an entry. Time comes from a [`Clock`], so the bound is
//! tested with a controllable one; an entry is stale at exactly 60 seconds.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::WorkspaceId;
use okf_jawn_core::access::{AccessControl, may_cache};
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::StorageScope;

use crate::config::AuthKind;

/// Where the cache reads the current instant.
pub trait Clock: Send + Sync {
    /// The current instant; never earlier than an instant this clock returned before.
    fn now(&self) -> Instant;
}

/// The operating system's monotonic clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

/// `AccessControl` over a hosted raw adapter, caching cacheable grants for at most 60 seconds.
pub struct CachedAccess {
    source: Arc<dyn AccessControl>,
    clock: Arc<dyn Clock>,
    workspaces: Mutex<BTreeMap<(CallerKey, WorkspaceId), Fetched<WorkspaceGrant>>>,
    tenants: Mutex<BTreeMap<CallerKey, Fetched<TenantGrant>>>,
    /// The most entries each map holds.
    capacity: usize,
}

/// Who a cached grant belongs to: tenant, subject, route and client.
type CallerKey = (String, String, u8, Option<String>);

/// A raw grant and when it was fetched.
#[derive(Clone)]
struct Fetched<T> {
    grant: T,
    at: Instant,
}

/// How long a cached `Read`, `Write` or `Propose` grant answers; stale at exactly this age.
pub const GRANT_CACHE_TTL: Duration = Duration::from_secs(60);

/// The most entries each of the cache's two maps holds (workspace grants, tenant grants).
pub const GRANT_CACHE_MAX_ENTRIES: usize = 10_000;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

impl CachedAccess {
    /// A cache over the hosted raw adapter `source`, reading time from `clock`.
    #[must_use]
    pub fn new(source: Arc<dyn AccessControl>, clock: Arc<dyn Clock>) -> Self {
        Self::with_capacity(source, clock, GRANT_CACHE_MAX_ENTRIES)
    }

    /// [`Self::new`], holding at most `capacity` entries in each map.
    #[must_use]
    pub fn with_capacity(
        source: Arc<dyn AccessControl>,
        clock: Arc<dyn Clock>,
        capacity: usize,
    ) -> Self {
        Self {
            source,
            clock,
            workspaces: Mutex::new(BTreeMap::new()),
            tenants: Mutex::new(BTreeMap::new()),
            capacity: capacity.max(1),
        }
    }

    /// The cached workspace grant for this caller, when fresh and `permission` may be cached.
    fn cached_workspace(
        &self,
        key: &(CallerKey, WorkspaceId),
        permission: Permission,
    ) -> Result<Option<WorkspaceGrant>, ApiError> {
        if !may_cache(permission) {
            return Ok(None);
        }
        let now = self.clock.now();
        Ok(lock(&self.workspaces)?
            .get(key)
            .filter(|fetched| fresh(fetched.at, now))
            .map(|fetched| WorkspaceGrant {
                scope: fetched.grant.scope.clone(),
                permissions: cacheable(&fetched.grant.permissions),
            }))
    }

    /// Keep `grant`, stamped with `began`, the instant its lookup began: the source may have
    /// read the role at any moment after that, so the 60 seconds count from there, however long
    /// the answer took. An answer never replaces one whose lookup began later.
    fn remember_workspace(
        &self,
        key: (CallerKey, WorkspaceId),
        grant: &WorkspaceGrant,
        began: Instant,
    ) -> Result<(), ApiError> {
        let now = self.clock.now();
        let mut entries = lock(&self.workspaces)?;
        entries.retain(|_, fetched| fresh(fetched.at, now));
        if entries.get(&key).is_none_or(|held| held.at <= began) {
            if !entries.contains_key(&key) && entries.len() >= self.capacity {
                evict_oldest(&mut entries);
            }
            entries.insert(
                key,
                Fetched {
                    grant: grant.clone(),
                    at: began,
                },
            );
        }
        Ok(())
    }

    fn cached_tenant(
        &self,
        key: &CallerKey,
        permission: Permission,
    ) -> Result<Option<TenantGrant>, ApiError> {
        if !may_cache(permission) {
            return Ok(None);
        }
        let now = self.clock.now();
        Ok(lock(&self.tenants)?
            .get(key)
            .filter(|fetched| fresh(fetched.at, now))
            .map(|fetched| TenantGrant {
                tenant_id: fetched.grant.tenant_id.clone(),
                permissions: cacheable(&fetched.grant.permissions),
            }))
    }

    /// [`Self::remember_workspace`], for a tenant grant.
    fn remember_tenant(
        &self,
        key: CallerKey,
        grant: &TenantGrant,
        began: Instant,
    ) -> Result<(), ApiError> {
        let now = self.clock.now();
        let mut entries = lock(&self.tenants)?;
        entries.retain(|_, fetched| fresh(fetched.at, now));
        if entries.get(&key).is_none_or(|held| held.at <= began) {
            if !entries.contains_key(&key) && entries.len() >= self.capacity {
                evict_oldest(&mut entries);
            }
            entries.insert(
                key,
                Fetched {
                    grant: grant.clone(),
                    at: began,
                },
            );
        }
        Ok(())
    }
}

impl AccessControl for CachedAccess {
    fn authorize<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
        permission: Permission,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move {
            let key = (caller_key(principal), workspace);
            if let Some(grant) = self.cached_workspace(&key, permission)? {
                return Ok(grant);
            }
            let began = self.clock.now();
            let grant = self
                .source
                .authorize(principal, workspace, permission)
                .await?;
            self.remember_workspace(key, &grant, began)?;
            Ok(grant)
        })
    }

    fn authorize_tenant<'a>(
        &'a self,
        principal: &'a Principal,
        permission: Permission,
    ) -> PortFuture<'a, TenantGrant> {
        Box::pin(async move {
            let key = caller_key(principal);
            if let Some(grant) = self.cached_tenant(&key, permission)? {
                return Ok(grant);
            }
            let began = self.clock.now();
            let grant = self.source.authorize_tenant(principal, permission).await?;
            self.remember_tenant(key, &grant, began)?;
            Ok(grant)
        })
    }

    fn grants<'a>(&'a self, principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        self.source.grants(principal)
    }

    fn grant_creator<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move {
            let began = self.clock.now();
            let grant = self.source.grant_creator(principal, workspace).await?;
            self.remember_workspace((caller_key(principal), workspace), &grant, began)?;
            Ok(grant)
        })
    }

    fn editors<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Vec<String>> {
        self.source.editors(scope)
    }
}

/// The `AccessControl` the service authorizes through: the hosted raw adapter behind the grant
/// cache, or the local adapter exactly as it is (the local adapter does not cache).
#[must_use]
pub fn select_access(
    kind: AuthKind,
    raw: Arc<dyn AccessControl>,
    clock: Arc<dyn Clock>,
) -> Arc<dyn AccessControl> {
    match kind {
        AuthKind::Hosted => Arc::new(CachedAccess::new(raw, clock)),
        AuthKind::Local => raw,
    }
}

/// Remove the entry whose lookup began first, to make room for another.
fn evict_oldest<K: Ord + Clone, T>(entries: &mut BTreeMap<K, Fetched<T>>) {
    let oldest = entries
        .iter()
        .min_by_key(|(_, fetched)| fetched.at)
        .map(|(key, _)| key.clone());
    if let Some(oldest) = oldest {
        let _evicted = entries.remove(&oldest);
    }
}

/// Whether a grant fetched at `at` still answers at `now`.
fn fresh(at: Instant, now: Instant) -> bool {
    now.checked_duration_since(at)
        .is_some_and(|age| age < GRANT_CACHE_TTL)
}

/// The permissions of `permissions` a cache may answer with.
fn cacheable(permissions: &[Permission]) -> Vec<Permission> {
    permissions
        .iter()
        .copied()
        .filter(|permission| may_cache(*permission))
        .collect()
}

fn caller_key(principal: &Principal) -> CallerKey {
    let route = match principal.route {
        AccessRoute::BrowserSession => 0,
        AccessRoute::McpDelegation => 1,
        AccessRoute::LocalOwner => 2,
        AccessRoute::Service => 3,
    };
    (
        principal.tenant_id.as_str().to_owned(),
        principal.subject.clone(),
        route,
        principal.client_id.clone(),
    )
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, ApiError> {
    mutex
        .lock()
        .map_err(|_| ApiError::new(ErrorCode::Internal, "The grant cache lock is poisoned"))
}
