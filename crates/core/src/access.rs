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
use okf_jawn_contract::import::JobKind;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::operations::{DRAFT_BEARING, HUMAN_SESSION_ONLY};

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

    /// Every subject whose raw grant on the workspace in `scope` includes `write`, resolved as
    /// `authorize` resolves it (hosted, a tenant-wide role that grants `write` counts), in any
    /// order and possibly with repeats; the application sorts them and removes repeats.
    ///
    /// Always fetched fresh, never from the grant cache: a workspace restore gives an archived
    /// draft back only to an editor among these subjects (`JobSpec::RestoreWorkspace`), and
    /// keeps every other draft in the archive, unassigned.
    fn editors<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Vec<String>>;
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

/// Whether the principal arrived through a human session: the browser session, or the local
/// owner's browser session (`LocalOwner`).
#[must_use]
pub const fn is_human_session(principal: &Principal) -> bool {
    matches!(
        principal.route,
        AccessRoute::BrowserSession | AccessRoute::LocalOwner
    )
}

/// Refuse, on every route that is not a human session, an operation that carries a draft
/// (`DRAFT_BEARING`) or that recovers or destroys (`HUMAN_SESSION_ONLY`).
///
/// SPEC section 8: MCP tools and agent routes never see drafts; recovery and destruction
/// belong to a person (ledger 2026-10-08). The route decides, not the subject or the grants: a
/// connector acting for the very person who owns the drafts, or holding `admin`, is refused.
/// Dispatch runs this before any grant is looked up.
///
/// # Errors
/// Returns `Forbidden` when `operation` is listed and the route is `McpDelegation` or
/// `Service`.
pub fn check_human_route(principal: &Principal, operation: OperationName) -> Result<(), ApiError> {
    if is_human_session(principal) {
        return Ok(());
    }
    if DRAFT_BEARING.contains(&operation) {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "Drafts are visible only in a human browser session",
        ));
    }
    if HUMAN_SESSION_ONLY.contains(&operation) {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "Backup, restore and purge are available only in a human browser session",
        ));
    }
    Ok(())
}

/// The job-kind rule of `retry_job` and `cancel_job` (Stage 1b design section 6), applied by
/// the application after it loads the job.
///
/// Those operations take `write` on the workspace, which alone would let a writer retry a
/// backup or restore on any route. So the caller must also hold, fresh from `access`, the
/// table permission of the operation that starts a job of this kind (`JobKind::started_by`),
/// and a human session when that operation is in `HUMAN_SESSION_ONLY`: `backup_workspace`,
/// `restore_workspace` and `rebuild_index` jobs need `admin`, the first two in a human
/// session; `import`, `redigest`, `export_workspace` and `export_view` jobs keep `write`.
///
/// # Errors
/// Returns `NotFound` for a tenant kind (tenant jobs are never controlled through these
/// operations), `Forbidden` for the route or a missing grant, and any error of `access`.
pub async fn authorize_job_kind(
    access: &dyn AccessControl,
    principal: &Principal,
    workspace: WorkspaceId,
    kind: JobKind,
) -> Result<WorkspaceGrant, ApiError> {
    if kind.is_tenant() {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "No job with that identity in this workspace",
        ));
    }
    let starter = kind.started_by();
    check_human_route(principal, starter)?;
    let permission = table_permission(starter)?;
    let raw = access.authorize(principal, workspace, permission).await?;
    if raw.workspace_id() != workspace || raw.scope.tenant_id != principal.tenant_id {
        return Err(ApiError::new(
            ErrorCode::Internal,
            "Access adapter returned a grant for a different scope",
        ));
    }
    authorize_workspace(principal, raw, permission)
}

/// The permission an operation's table row declares.
fn table_permission(operation: OperationName) -> Result<Permission, ApiError> {
    okf_jawn_contract::metadata::operations()
        .into_iter()
        .find(|info| info.id == operation.as_str())
        .map(|info| info.permission)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::Internal,
                format!("{} has no row in the operation table", operation.as_str()),
            )
        })
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
