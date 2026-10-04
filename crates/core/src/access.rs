//! Shared capability checks; a delegated user identity is never a human review.

use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::WorkspaceId;

/// Authorize a declared operation within the principal's explicit workspace scope.
///
/// # Errors
/// Returns `Forbidden` for missing capabilities, inaccessible workspaces, or agent approval.
pub fn authorize(principal: &Principal, permission: &Permission, workspace: Option<WorkspaceId>) -> Result<(), ApiError> {
    let agent_overreach = principal.route == AccessRoute::McpDelegation
        && !matches!(permission, Permission::Read | Permission::Propose);
    let service_review = principal.route == AccessRoute::Service
        && matches!(permission, Permission::Approve | Permission::Review);
    if agent_overreach || service_review {
        return Err(ApiError::new(ErrorCode::Forbidden, "This access route cannot perform the requested privileged action"));
    }
    if !principal.permissions.contains(permission) {
        return Err(ApiError::new(ErrorCode::Forbidden, "Required capability is not granted"));
    }
    if workspace.is_some_and(|id| !principal.workspace_ids.contains(&id)) {
        return Err(ApiError::new(ErrorCode::NotFound, "Workspace is not available"));
    }
    Ok(())
}
