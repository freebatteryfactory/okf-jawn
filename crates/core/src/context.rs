//! Per-request authorization and mutation identity assembled by dispatch.
//!
//! Identity (`Principal`) is not authorization. Effective permissions arrive as grants;
//! every mutation that runs carries a durable `MutationId`.

use okf_jawn_contract::access::{Permission, Principal};
use okf_jawn_contract::identity::{MutationId, TenantId, WorkspaceId};
use okf_jawn_contract::metadata::OperationName;

use crate::storage::StorageScope;

/// Effective permissions on one workspace after route and delegation rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGrant {
    /// Storage scope the grant authorizes.
    pub scope: StorageScope,
    /// Capabilities the caller may exercise in this workspace.
    pub permissions: Vec<Permission>,
}

/// Effective permissions on the caller's tenant (deployment-scoped operations).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantGrant {
    /// Tenant boundary this grant covers.
    pub tenant_id: TenantId,
    /// Capabilities the caller may exercise at tenant scope.
    pub permissions: Vec<Permission>,
}

/// Authorization and mutation identity for one operation invocation.
#[derive(Debug, Clone)]
pub struct OperationContext {
    /// Authenticated caller; never taken from a request body.
    pub principal: Principal,
    /// Canonical operation being executed.
    pub operation: OperationName,
    /// Tenant grant when the request authorized a deployment target.
    pub tenant: Option<TenantGrant>,
    /// Workspace grants for every authorized workspace target, in target order.
    pub grants: Vec<WorkspaceGrant>,
    /// Durable write identity when this invocation is a mutation under an idempotency key.
    pub mutation: Option<MutationId>,
}

impl WorkspaceGrant {
    /// Whether this grant includes `permission`.
    #[must_use]
    pub fn allows(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }

    /// Workspace this grant covers.
    #[must_use]
    pub fn workspace_id(&self) -> WorkspaceId {
        self.scope.workspace_id
    }
}

impl TenantGrant {
    /// Whether this grant includes `permission`.
    #[must_use]
    pub fn allows(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }
}

impl OperationContext {
    /// First workspace grant, when the operation named a workspace target.
    #[must_use]
    pub fn primary_workspace(&self) -> Option<&WorkspaceGrant> {
        self.grants.first()
    }
}
