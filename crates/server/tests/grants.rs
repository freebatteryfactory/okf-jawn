//! The hosted grant cache against a controllable clock: a revoked cacheable role is denied within
//! 60 seconds, `approve`, `review` and `admin` are always checked fresh, and the local adapter
//! is never cached.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::WorkspaceId;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::StorageScope;
use okf_jawn_server::grants::Clock;

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
            lookups: AtomicUsize::new(0),
        }
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
            self.lookups.fetch_add(1, Ordering::SeqCst);
            Ok(WorkspaceGrant {
                scope: StorageScope {
                    tenant_id: principal.tenant_id.clone(),
                    workspace_id: workspace,
                },
                permissions: self.workspace.lock().map_err(|_| poisoned())?.clone(),
            })
        })
    }

    fn authorize_tenant<'a>(
        &'a self,
        principal: &'a Principal,
        _permission: Permission,
    ) -> PortFuture<'a, TenantGrant> {
        Box::pin(async move {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            Ok(TenantGrant {
                tenant_id: principal.tenant_id.clone(),
                permissions: self.tenant.lock().map_err(|_| poisoned())?.clone(),
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

mod grant_cache_ttl {
    use std::sync::Arc;
    use std::time::Duration;

    use okf_jawn_contract::access::Permission;
    use okf_jawn_core::access::AccessControl;
    use okf_jawn_server::config::AuthKind;
    use okf_jawn_server::grants::{CachedAccess, select_access};

    use super::{Roles, TestClock, allows, member};
    use crate::check::TestResult;

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
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "../../core/tests/support/mod.rs"]
mod fixtures;
