//! The hosted grant cache against a controllable clock: a revoked cacheable role is denied within
//! 60 seconds, `approve`, `review` and `admin` are always checked fresh, and the local adapter
//! is never cached.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::WorkspaceId;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::StorageScope;
use okf_jawn_server::grants::Clock;
use tokio::sync::Notify;
use tokio::time::error::Elapsed;
use tokio::time::timeout;

use fixtures::{tenant, workspace};

/// A clock that moves only when a test advances it.
struct TestClock {
    start: Instant,
    elapsed_ms: AtomicU64,
}

/// A hosted raw adapter whose roles a test can change, counting every lookup.
#[derive(Default)]
struct Roles {
    workspace: Mutex<Vec<Permission>>,
    tenant: Mutex<Vec<Permission>>,
    lookups: AtomicUsize,
    /// The next lookup fails, as a provider that is down.
    failing: AtomicBool,
    /// Every lookup reads the roles, then lets this much time pass on the clock before it answers.
    delay: Mutex<Option<(Arc<TestClock>, Duration)>>,
    /// The next lookup reads the roles, then waits here until it is let go.
    gate: Mutex<Option<Arc<Notify>>>,
    /// Signalled when a lookup has read the roles and is waiting at the gate.
    waiting: Notify,
}

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";

impl TestClock {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            elapsed_ms: AtomicU64::new(0),
        }
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

impl Roles {
    fn holding(permissions: &[Permission]) -> Self {
        Self {
            workspace: Mutex::new(permissions.to_vec()),
            tenant: Mutex::new(permissions.to_vec()),
            ..Self::default()
        }
    }

    fn fail_next(&self) {
        self.failing.store(true, Ordering::SeqCst);
    }

    fn answer_after(&self, clock: Arc<TestClock>, delay: Duration) -> Result<(), ApiError> {
        *self.delay.lock().map_err(|_| poisoned())? = Some((clock, delay));
        Ok(())
    }

    /// Make the next lookup wait after it read the roles; the returned handle lets it go.
    fn hold_next(&self) -> Result<Arc<Notify>, ApiError> {
        let gate = Arc::new(Notify::new());
        *self.gate.lock().map_err(|_| poisoned())? = Some(gate.clone());
        Ok(gate)
    }

    /// Wait until a lookup is held at the gate. When none begins within ten seconds this is an
    /// error, so a test whose lookup never starts fails instead of waiting for ever.
    async fn held(&self) -> Result<(), Elapsed> {
        timeout(Duration::from_secs(10), self.waiting.notified()).await
    }

    /// What a lookup does between reading the roles and answering: fail, wait, or take time.
    async fn answering(&self) -> Result<(), ApiError> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        if self.failing.swap(false, Ordering::SeqCst) {
            return Err(ApiError::new(
                ErrorCode::Unavailable,
                "the provider is down",
            ));
        }
        let gate = self.gate.lock().map_err(|_| poisoned())?.take();
        if let Some(gate) = gate {
            self.waiting.notify_one();
            gate.notified().await;
        }
        let delay = self.delay.lock().map_err(|_| poisoned())?.clone();
        if let Some((clock, delay)) = delay {
            clock
                .advance(delay)
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        }
        Ok(())
    }

    fn set(&self, permissions: &[Permission]) -> Result<(), ApiError> {
        *self.workspace.lock().map_err(|_| poisoned())? = permissions.to_vec();
        *self.tenant.lock().map_err(|_| poisoned())? = permissions.to_vec();
        Ok(())
    }

    fn lookups(&self) -> usize {
        self.lookups.load(Ordering::SeqCst)
    }
}

impl AccessControl for Roles {
    fn authorize<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
        _permission: Permission,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move {
            // The provider reads the role first; what follows is the answer's way back.
            let permissions = self.workspace.lock().map_err(|_| poisoned())?.clone();
            self.answering().await?;
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
            let permissions = self.tenant.lock().map_err(|_| poisoned())?.clone();
            self.answering().await?;
            Ok(TenantGrant {
                tenant_id: principal.tenant_id.clone(),
                permissions,
            })
        })
    }

    fn grants<'a>(&'a self, _principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        Box::pin(async move {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        })
    }

    fn grant_creator<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant> {
        self.authorize(principal, workspace, Permission::Admin)
    }

    fn editors<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Vec<String>> {
        Box::pin(async move {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        })
    }
}

fn poisoned() -> ApiError {
    ApiError::new(ErrorCode::Internal, "test roles lock poisoned")
}

fn member() -> Result<Principal, Box<dyn std::error::Error>> {
    Ok(Principal {
        subject: "user_01".to_owned(),
        tenant_id: tenant("org_01")?,
        route: AccessRoute::BrowserSession,
        client_id: None,
        delegation: None,
    })
}

/// Whether the cache answers `permission` on the workspace as granted.
async fn allows(
    access: &dyn AccessControl,
    principal: &Principal,
    permission: Permission,
) -> Result<bool, Box<dyn std::error::Error>> {
    let grant = access
        .authorize(principal, workspace(WORKSPACE)?, permission)
        .await?;
    Ok(grant.allows(permission))
}

/// Whether the cache answers `permission` on the caller's tenant as granted.
async fn tenant_allows(
    access: &dyn AccessControl,
    principal: &Principal,
    permission: Permission,
) -> Result<bool, ApiError> {
    Ok(access
        .authorize_tenant(principal, permission)
        .await?
        .allows(permission))
}

mod grant_cache_ttl {
    use std::sync::Arc;
    use std::time::Duration;

    use okf_jawn_contract::access::{AccessRoute, Permission};
    use okf_jawn_contract::error::ErrorCode;
    use okf_jawn_core::access::AccessControl;
    use okf_jawn_server::config::AuthKind;
    use okf_jawn_server::grants::{CachedAccess, select_access};

    use super::{Roles, TestClock, allows, member, tenant_allows};
    use crate::check::{TestResult, err_of};

    #[tokio::test]
    async fn a_revoked_write_role_is_denied_once_sixty_seconds_have_passed() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read, Permission::Write]));
        let clock = Arc::new(TestClock::new());
        let cache = CachedAccess::new(roles.clone(), clock.clone());
        let principal = member()?;

        assert!(allows(&cache, &principal, Permission::Write).await?);
        roles.set(&[Permission::Read])?;
        clock.advance(Duration::from_secs(59))?;
        // Still inside the window: answered from the cache, without asking the source.
        assert!(allows(&cache, &principal, Permission::Write).await?);
        assert_eq!(roles.lookups(), 1);
        clock.advance(Duration::from_secs(1))?;
        // Exactly 60 seconds after the fetch: stale, asked again, and denied.
        assert!(!allows(&cache, &principal, Permission::Write).await?);
        assert_eq!(roles.lookups(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn approve_review_and_admin_are_always_checked_fresh() -> TestResult {
        for permission in [Permission::Approve, Permission::Review, Permission::Admin] {
            let roles = Arc::new(Roles::holding(&[Permission::Read, permission]));
            let clock = Arc::new(TestClock::new());
            let cache = CachedAccess::new(roles.clone(), clock);
            let principal = member()?;

            assert!(allows(&cache, &principal, permission).await?);
            roles.set(&[Permission::Read])?;
            // No time passes: a cached answer would still allow it.
            assert!(!allows(&cache, &principal, permission).await?);
            assert_eq!(roles.lookups(), 2);
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_cached_answer_never_carries_approve_review_or_admin() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read, Permission::Admin]));
        let clock = Arc::new(TestClock::new());
        let cache = CachedAccess::new(roles.clone(), clock);
        let principal = member()?;

        assert!(allows(&cache, &principal, Permission::Admin).await?);
        let workspace = super::workspace(super::WORKSPACE)?;
        let cached = cache
            .authorize(&principal, workspace, Permission::Read)
            .await?;
        assert_eq!(roles.lookups(), 1);
        assert_eq!(cached.permissions, vec![Permission::Read]);
        Ok(())
    }

    #[tokio::test]
    async fn a_revoked_tenant_role_is_denied_once_sixty_seconds_have_passed() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read]));
        let clock = Arc::new(TestClock::new());
        let cache = CachedAccess::new(roles.clone(), clock.clone());
        let principal = member()?;

        assert!(
            cache
                .authorize_tenant(&principal, Permission::Read)
                .await?
                .allows(Permission::Read)
        );
        roles.set(&[])?;
        clock.advance(Duration::from_secs(30))?;
        assert_eq!(roles.lookups(), 1);
        clock.advance(Duration::from_secs(30))?;
        let after = cache.authorize_tenant(&principal, Permission::Read).await?;
        assert!(!after.allows(Permission::Read));
        assert_eq!(roles.lookups(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn two_clients_of_one_subject_never_share_an_entry() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read]));
        let clock = Arc::new(TestClock::new());
        let cache = CachedAccess::new(roles.clone(), clock);
        let mut first = member()?;
        first.client_id = Some("client_a".to_owned());
        let mut second = member()?;
        second.client_id = Some("client_b".to_owned());

        assert!(allows(&cache, &first, Permission::Read).await?);
        assert!(allows(&cache, &second, Permission::Read).await?);
        assert_eq!(roles.lookups(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn the_local_adapter_is_never_cached() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read, Permission::Write]));
        let clock = Arc::new(TestClock::new());
        let access = select_access(AuthKind::Local, roles.clone(), clock);
        let principal = member()?;

        assert!(allows(access.as_ref(), &principal, Permission::Write).await?);
        roles.set(&[Permission::Read])?;
        assert!(!allows(access.as_ref(), &principal, Permission::Write).await?);
        assert_eq!(roles.lookups(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn the_hosted_adapter_is_selected_behind_the_cache() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read]));
        let clock = Arc::new(TestClock::new());
        let access = select_access(AuthKind::Hosted, roles.clone(), clock);
        let principal = member()?;

        assert!(allows(access.as_ref(), &principal, Permission::Read).await?);
        assert!(allows(access.as_ref(), &principal, Permission::Read).await?);
        assert_eq!(roles.lookups(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn two_tenants_two_subjects_and_two_routes_never_share_an_entry() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read]));
        let clock = Arc::new(TestClock::new());
        let cache = CachedAccess::new(roles.clone(), clock);
        let first = member()?;
        assert!(allows(&cache, &first, Permission::Read).await?);
        assert_eq!(roles.lookups(), 1);

        let mut other_tenant = member()?;
        other_tenant.tenant_id = super::tenant("org_02")?;
        let mut other_subject = member()?;
        other_subject.subject = "user_02".to_owned();
        let mut other_route = member()?;
        other_route.route = AccessRoute::McpDelegation;
        let mut expected = 1_usize;
        for principal in [&other_tenant, &other_subject, &other_route] {
            // The source answers for the tenant it is asked about; a shared entry would answer
            // with the first caller's grant and no lookup.
            let grant = cache
                .authorize(
                    principal,
                    super::workspace(super::WORKSPACE)?,
                    Permission::Read,
                )
                .await?;
            expected = expected.saturating_add(1);
            assert_eq!(roles.lookups(), expected);
            assert_eq!(grant.scope.tenant_id, principal.tenant_id);
        }
        // Each is now cached on its own: asking again costs nothing.
        for principal in [&first, &other_tenant, &other_subject, &other_route] {
            assert!(allows(&cache, principal, Permission::Read).await?);
        }
        assert_eq!(roles.lookups(), expected);

        // The same holds for tenant grants.
        for principal in [&first, &other_tenant, &other_subject, &other_route] {
            let grant = cache.authorize_tenant(principal, Permission::Read).await?;
            assert_eq!(grant.tenant_id, principal.tenant_id);
        }
        assert_eq!(roles.lookups(), expected.saturating_add(4));
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_lookup_is_never_cached() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read]));
        let clock = Arc::new(TestClock::new());
        let cache = CachedAccess::new(roles.clone(), clock);
        let principal = member()?;
        roles.fail_next();
        let refused = cache
            .authorize(
                &principal,
                super::workspace(super::WORKSPACE)?,
                Permission::Read,
            )
            .await;
        assert_eq!(err_of(refused)?.code, ErrorCode::Unavailable);
        // The next request asks the source again and is answered.
        assert!(allows(&cache, &principal, Permission::Read).await?);
        assert_eq!(roles.lookups(), 2);
        roles.fail_next();
        let refused = cache.authorize_tenant(&principal, Permission::Read).await;
        assert_eq!(err_of(refused)?.code, ErrorCode::Unavailable);
        assert!(
            cache
                .authorize_tenant(&principal, Permission::Read)
                .await?
                .allows(Permission::Read)
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_sixty_seconds_count_from_when_the_lookup_began() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read, Permission::Write]));
        let clock = Arc::new(TestClock::new());
        // The source reads the role, then takes ten seconds to answer.
        roles.answer_after(clock.clone(), Duration::from_secs(10))?;
        let cache = CachedAccess::new(roles.clone(), clock.clone());
        let principal = member()?;
        assert!(allows(&cache, &principal, Permission::Write).await?);
        // Revoked right after the source read it; 55 seconds pass after the answer arrived,
        // 65 since the source last saw the role.
        roles.set(&[Permission::Read])?;
        clock.advance(Duration::from_secs(55))?;
        assert!(!allows(&cache, &principal, Permission::Write).await?);
        assert_eq!(roles.lookups(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn an_older_answer_never_replaces_one_whose_lookup_began_later() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read, Permission::Admin]));
        let clock = Arc::new(TestClock::new());
        let cache = Arc::new(CachedAccess::new(roles.clone(), clock.clone()));
        let principal = member()?;
        // The first lookup (an admin check, always fresh) reads the old role and then waits.
        let gate = roles.hold_next()?;
        let slow = {
            let cache = cache.clone();
            let principal = principal.clone();
            tokio::spawn(async move {
                allows(cache.as_ref(), &principal, Permission::Admin)
                    .await
                    .map_err(|error| error.to_string())
            })
        };
        roles.held().await?;
        // Meanwhile the role is revoked and a later lookup caches the newer answer.
        roles.set(&[Permission::Admin])?;
        clock.advance(Duration::from_secs(5))?;
        assert!(allows(cache.as_ref(), &principal, Permission::Admin).await?);
        // The first lookup's answer arrives last, carrying the revoked read.
        gate.notify_one();
        assert!(slow.await??);
        // A read is answered from the cache: it must be the newer answer, without read.
        assert!(!allows(cache.as_ref(), &principal, Permission::Read).await?);
        assert_eq!(roles.lookups(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn the_sixty_seconds_of_a_tenant_grant_count_from_when_the_lookup_began() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read, Permission::Write]));
        let clock = Arc::new(TestClock::new());
        // The source reads the tenant role, then takes ten seconds to answer.
        roles.answer_after(clock.clone(), Duration::from_secs(10))?;
        let cache = CachedAccess::new(roles.clone(), clock.clone());
        let principal = member()?;
        assert!(tenant_allows(&cache, &principal, Permission::Write).await?);
        // Revoked right after the source read it. 49 seconds after the answer arrived is 59
        // since the source last saw the role: still answered from the cache.
        roles.set(&[Permission::Read])?;
        clock.advance(Duration::from_secs(49))?;
        assert!(tenant_allows(&cache, &principal, Permission::Write).await?);
        assert_eq!(roles.lookups(), 1);
        // One second more is 60 since the lookup began, 50 since its answer: asked again.
        clock.advance(Duration::from_secs(1))?;
        assert!(!tenant_allows(&cache, &principal, Permission::Write).await?);
        assert_eq!(roles.lookups(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn an_older_tenant_answer_never_replaces_one_whose_lookup_began_later() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read, Permission::Admin]));
        let clock = Arc::new(TestClock::new());
        let cache = Arc::new(CachedAccess::new(roles.clone(), clock.clone()));
        let principal = member()?;
        // The first tenant lookup (an admin check, always fresh) reads the old role and waits.
        let gate = roles.hold_next()?;
        let slow = {
            let cache = cache.clone();
            let principal = principal.clone();
            tokio::spawn(async move {
                tenant_allows(cache.as_ref(), &principal, Permission::Admin).await
            })
        };
        roles.held().await?;
        // Meanwhile read is revoked and a later lookup caches the newer answer.
        roles.set(&[Permission::Admin])?;
        clock.advance(Duration::from_secs(5))?;
        assert!(tenant_allows(cache.as_ref(), &principal, Permission::Admin).await?);
        // The first lookup's answer arrives last, carrying the revoked read.
        gate.notify_one();
        assert!(slow.await??);
        // A read is answered from the cache: it must be the newer answer, without read.
        assert!(!tenant_allows(cache.as_ref(), &principal, Permission::Read).await?);
        assert_eq!(roles.lookups(), 2);
        Ok(())
    }
}

/// N-g: the cache holds a bounded number of entries, so a flood of distinct lookups cannot grow
/// it without end; a stale entry goes before any fresh one, and then the oldest fresh one.
mod grant_cache_bound {
    use std::sync::Arc;
    use std::time::Duration;

    use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
    use okf_jawn_contract::identity::WorkspaceId;
    use okf_jawn_core::access::AccessControl;
    use okf_jawn_server::grants::{CachedAccess, GRANT_CACHE_MAX_ENTRIES};

    use super::{Roles, TestClock, member};
    use crate::check::TestResult;
    use crate::fixtures::workspace;

    /// The `n`th of a run of distinct workspace ids.
    fn nth(n: usize) -> Result<WorkspaceId, serde_json::Error> {
        workspace(&format!("00000000-0000-4000-8000-{n:012}"))
    }

    async fn read(
        cache: &CachedAccess,
        principal: &Principal,
        n: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        cache
            .authorize(principal, nth(n)?, Permission::Read)
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn a_full_cache_evicts_its_oldest_entry_for_a_new_one() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read]));
        let clock = Arc::new(TestClock::new());
        let cache = CachedAccess::with_capacity(roles.clone(), clock.clone(), 3);
        let principal = member()?;
        for n in 1..=3 {
            read(&cache, &principal, n).await?;
            clock.advance(Duration::from_secs(1))?;
        }
        assert_eq!(roles.lookups(), 3);
        read(&cache, &principal, 4).await?;
        assert_eq!(roles.lookups(), 4);
        // The second and third are still cached; the first, the oldest, was evicted.
        read(&cache, &principal, 2).await?;
        read(&cache, &principal, 3).await?;
        assert_eq!(roles.lookups(), 4);
        read(&cache, &principal, 1).await?;
        assert_eq!(roles.lookups(), 5);
        Ok(())
    }

    #[tokio::test]
    async fn a_stale_entry_goes_before_any_fresh_one() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read]));
        let clock = Arc::new(TestClock::new());
        let cache = CachedAccess::with_capacity(roles.clone(), clock.clone(), 3);
        let principal = member()?;
        read(&cache, &principal, 1).await?;
        clock.advance(Duration::from_secs(59))?;
        read(&cache, &principal, 2).await?;
        read(&cache, &principal, 3).await?;
        clock.advance(Duration::from_secs(2))?;
        read(&cache, &principal, 4).await?;
        assert_eq!(roles.lookups(), 4);
        // The stale first went; the two fresh ones stayed.
        read(&cache, &principal, 2).await?;
        read(&cache, &principal, 3).await?;
        assert_eq!(roles.lookups(), 4);
        Ok(())
    }

    #[tokio::test]
    async fn a_refreshed_entry_takes_no_place_of_another() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read]));
        let clock = Arc::new(TestClock::new());
        let cache = CachedAccess::with_capacity(roles.clone(), clock.clone(), 2);
        let principal = member()?;
        read(&cache, &principal, 2).await?;
        clock.advance(Duration::from_secs(1))?;
        read(&cache, &principal, 1).await?;
        // A fresh check of the first replaces its own entry and evicts nothing.
        cache
            .authorize(&principal, nth(1)?, Permission::Admin)
            .await?;
        assert_eq!(roles.lookups(), 3);
        read(&cache, &principal, 2).await?;
        assert_eq!(roles.lookups(), 3);
        Ok(())
    }

    #[tokio::test]
    async fn the_tenant_entries_are_bounded_the_same_way() -> TestResult {
        let roles = Arc::new(Roles::holding(&[Permission::Read]));
        let clock = Arc::new(TestClock::new());
        let cache = CachedAccess::with_capacity(roles.clone(), clock.clone(), 2);
        let caller = |n: usize| -> Result<Principal, Box<dyn std::error::Error>> {
            Ok(Principal {
                subject: format!("user_{n}"),
                route: AccessRoute::BrowserSession,
                ..member()?
            })
        };
        for n in 1..=3 {
            cache
                .authorize_tenant(&caller(n)?, Permission::Read)
                .await?;
            clock.advance(Duration::from_secs(1))?;
        }
        assert_eq!(roles.lookups(), 3);
        cache
            .authorize_tenant(&caller(3)?, Permission::Read)
            .await?;
        cache
            .authorize_tenant(&caller(2)?, Permission::Read)
            .await?;
        assert_eq!(roles.lookups(), 3);
        cache
            .authorize_tenant(&caller(1)?, Permission::Read)
            .await?;
        assert_eq!(roles.lookups(), 4);
        Ok(())
    }

    #[tokio::test]
    async fn the_production_cache_holds_ten_thousand_entries() -> TestResult {
        assert_eq!(GRANT_CACHE_MAX_ENTRIES, 10_000);
        let roles = Arc::new(Roles::holding(&[Permission::Read]));
        let clock = Arc::new(TestClock::new());
        let cache = CachedAccess::new(roles.clone(), clock.clone());
        let principal = member()?;
        for n in 0..=GRANT_CACHE_MAX_ENTRIES {
            read(&cache, &principal, n).await?;
            clock.advance(Duration::from_millis(1))?;
        }
        let before = roles.lookups();
        // The ten thousand most recent are cached; the first, one past the bound, is not.
        read(&cache, &principal, 1).await?;
        read(&cache, &principal, GRANT_CACHE_MAX_ENTRIES).await?;
        assert_eq!(roles.lookups(), before);
        read(&cache, &principal, 0).await?;
        assert_eq!(roles.lookups(), before.saturating_add(1));
        Ok(())
    }
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
