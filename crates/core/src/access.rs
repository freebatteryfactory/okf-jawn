//! Capability checks: route rules and delegation ceilings are pure; grants come from adapters.
//!
//! # Cache rule
//! Hosted adapters may cache `Read`, `Write`, and `Propose` grants for at most 60 seconds.
//! `Approve`, `Review`, and `Admin` are always checked fresh (no cache). They are rare,
//! human-initiated, and the ones where a revoked role matters most. The local adapter does
//! not cache. Logging out ends the session immediately.

use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::WorkspaceId;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::operations::DRAFT_BEARING;

use crate::context::{TenantGrant, WorkspaceGrant};
use crate::ports::PortFuture;
use crate::storage::StorageScope;

/// Resolves effective workspace and tenant grants for an authenticated principal.
///
/// Adapters own WorkOS / local lookup. Pure route and delegation rules live in this module
/// and are applied by dispatch after a raw grant is returned.
pub trait AccessControl: Send + Sync {
    /// Look up the caller's raw grant on one workspace (before route/delegation intersection).
    ///
    /// Hosted: may cache only when [`may_cache`] is true, for at most 60 seconds.
    /// Approve, Review, and Admin must always be fetched fresh.
    fn authorize<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
        permission: Permission,
    ) -> PortFuture<'a, WorkspaceGrant>;

    /// Look up the caller's raw tenant grant (before route/delegation intersection).
    ///
    /// Locally the owner session is tenant Admin and connectors never are. Hosted, the
    /// WorkOS org role maps to tenant permissions.
    fn authorize_tenant<'a>(
        &'a self,
        principal: &'a Principal,
        permission: Permission,
    ) -> PortFuture<'a, TenantGrant>;

    /// Enumerate workspace grants for listing; results are filtered by the caller.
    fn grants<'a>(&'a self, principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>>;

    /// Grant the creator Admin on a newly created workspace.
    fn grant_creator<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant>;
}

/// Reject routes that cannot exercise `permission` regardless of grants.
///
/// # Errors
/// Returns `Forbidden` for agent approval/write/admin and for service review/approval.
pub fn check_route(principal: &Principal, permission: Permission) -> Result<(), ApiError> {
    let agent_overreach = principal.route == AccessRoute::McpDelegation
        && !matches!(permission, Permission::Read | Permission::Propose);
    let service_review = principal.route == AccessRoute::Service
        && matches!(permission, Permission::Approve | Permission::Review);
    if agent_overreach || service_review {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "This access route cannot perform the requested privileged action",
        ));
    }
    Ok(())
}

/// Refuse a draft-bearing operation on every route that is not a human browser session.
///
/// SPEC section 8: MCP tools and agent routes never see drafts. The route decides, not the
/// subject or the grants: a connector acting for the very person who owns the drafts is
/// refused. `LocalOwner` is the local browser session, so it is a human session here.
///
/// # Errors
/// Returns `Forbidden` when `operation` is in `DRAFT_BEARING` and the route is
/// `McpDelegation` or `Service`.
pub fn check_draft_route(principal: &Principal, operation: OperationName) -> Result<(), ApiError> {
    let human_session = match principal.route {
        AccessRoute::BrowserSession | AccessRoute::LocalOwner => true,
        AccessRoute::McpDelegation | AccessRoute::Service => false,
    };
    if human_session || !DRAFT_BEARING.contains(&operation) {
        return Ok(());
    }
    Err(ApiError::new(
        ErrorCode::Forbidden,
        "Drafts are visible only in a human browser session",
    ))
}

/// Intersect granted permissions with the principal's delegation ceiling.
///
/// A ceiling only narrows; absence of a ceiling leaves grants unchanged. When the ceiling
/// lists workspace ids, grants outside that list become empty.
#[must_use]
pub fn apply_delegation(
    principal: &Principal,
    workspace: Option<WorkspaceId>,
    permissions: &[Permission],
) -> Vec<Permission> {
    let Some(ceiling) = &principal.delegation else {
        return permissions.to_vec();
    };
    if let (Some(workspace), Some(allowed)) = (workspace, &ceiling.workspace_ids)
        && !allowed.contains(&workspace)
    {
        return Vec::new();
    }
    permissions
        .iter()
        .copied()
        .filter(|permission| ceiling.permissions.contains(permission))
        .collect()
}

/// Permissions that may be cached by a hosted adapter (≤60s). Others are always fresh.
#[must_use]
pub const fn may_cache(permission: Permission) -> bool {
    matches!(
        permission,
        Permission::Read | Permission::Write | Permission::Propose
    )
}

/// Apply route and delegation rules to a raw workspace grant, then require `permission`.
///
/// # Errors
/// Returns `Forbidden` when the route forbids the action or the effective grant lacks it.
pub fn authorize_workspace(
    principal: &Principal,
    mut grant: WorkspaceGrant,
    permission: Permission,
) -> Result<WorkspaceGrant, ApiError> {
    check_route(principal, permission)?;
    grant.permissions = apply_delegation(principal, Some(grant.workspace_id()), &grant.permissions);
    if !grant.allows(permission) {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "Required capability is not granted",
        ));
    }
    Ok(grant)
}

/// Apply route and delegation rules to a raw tenant grant, then require `permission`.
///
/// # Errors
/// Returns `Forbidden` when the route forbids the action or the effective grant lacks it.
pub fn authorize_tenant(
    principal: &Principal,
    mut grant: TenantGrant,
    permission: Permission,
) -> Result<TenantGrant, ApiError> {
    check_route(principal, permission)?;
    grant.permissions = apply_delegation(principal, None, &grant.permissions);
    if !grant.allows(permission) {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "Required capability is not granted",
        ));
    }
    Ok(grant)
}

/// Build a scope from an already-authorized workspace grant.
#[must_use]
pub fn scope_from_grant(grant: &WorkspaceGrant) -> StorageScope {
    grant.scope.clone()
}
