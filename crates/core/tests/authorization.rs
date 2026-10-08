//! Capability and constraint are tested together, not by accepting universal refusal.

use okf_jawn_contract::access::{AccessRoute, DelegationCeiling, Permission, Principal};
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::identity::{IdentityError, TenantId, WorkspaceId};
use okf_jawn_core::access::{authorize_tenant, authorize_workspace, check_route, may_cache};
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::storage::StorageScope;

use check::{TestResult, err_of};

fn local_tenant() -> Result<TenantId, IdentityError> {
    TenantId::try_from("tenant-local".to_owned())
}

fn grant(
    workspace: WorkspaceId,
    permissions: Vec<Permission>,
) -> Result<WorkspaceGrant, IdentityError> {
    Ok(WorkspaceGrant {
        scope: StorageScope {
            tenant_id: local_tenant()?,
            workspace_id: workspace,
        },
        permissions,
    })
}

fn every_permission() -> Vec<Permission> {
    vec![
        Permission::Read,
        Permission::Propose,
        Permission::Approve,
        Permission::Review,
        Permission::Write,
        Permission::Admin,
    ]
}

#[test]
fn delegated_proposal_is_allowed_but_approval_is_not() -> TestResult {
    let workspace: WorkspaceId = serde_json::from_str("\"11111111-1111-4111-8111-111111111111\"")?;
    let principal = Principal {
        subject: "test-subject".to_owned(),
        tenant_id: local_tenant()?,
        route: AccessRoute::McpDelegation,
        client_id: Some("test-client".to_owned()),
        delegation: Some(DelegationCeiling {
            permissions: every_permission(),
            workspace_ids: Some(vec![workspace]),
        }),
    };
    let raw = grant(workspace, every_permission())?;
    for allowed in [Permission::Read, Permission::Propose] {
        let effective = authorize_workspace(&principal, raw.clone(), allowed)?;
        assert!(effective.allows(allowed));
    }
    for forbidden in [
        Permission::Approve,
        Permission::Review,
        Permission::Write,
        Permission::Admin,
    ] {
        let refused = err_of(authorize_workspace(&principal, raw.clone(), forbidden))?;
        assert_eq!(refused.code, ErrorCode::Forbidden);
    }
    let human = Principal {
        route: AccessRoute::BrowserSession,
        delegation: None,
        ..principal
    };
    check_route(&human, Permission::Approve)?;
    let approving = grant(workspace, vec![Permission::Approve])?;
    let effective = authorize_workspace(&human, approving, Permission::Approve)?;
    assert!(effective.allows(Permission::Approve));
    Ok(())
}

#[test]
fn local_owner_uses_the_same_rules_and_cannot_reach_other_workspaces() -> TestResult {
    let local: WorkspaceId = serde_json::from_str("\"11111111-1111-4111-8111-111111111111\"")?;
    let foreign: WorkspaceId = serde_json::from_str("\"33333333-3333-4333-8333-333333333333\"")?;
    let owner = Principal {
        subject: "local-installation-owner".to_owned(),
        tenant_id: local_tenant()?,
        route: AccessRoute::LocalOwner,
        client_id: None,
        delegation: None,
    };
    for permission in [
        Permission::Read,
        Permission::Write,
        Permission::Approve,
        Permission::Review,
        Permission::Admin,
    ] {
        let granted = grant(local, vec![permission])?;
        assert!(authorize_workspace(&owner, granted, permission)?.allows(permission));
        let ungranted = grant(foreign, Vec::new())?;
        let refused = err_of(authorize_workspace(&owner, ungranted, permission))?;
        assert_eq!(refused.code, ErrorCode::Forbidden);
    }
    let connector = Principal {
        subject: "local-installation-owner".to_owned(),
        tenant_id: local_tenant()?,
        route: AccessRoute::McpDelegation,
        client_id: Some("local-connector".to_owned()),
        delegation: Some(DelegationCeiling {
            permissions: vec![Permission::Read],
            workspace_ids: Some(vec![local]),
        }),
    };
    let read_and_propose = vec![Permission::Read, Permission::Propose];
    let reading = authorize_workspace(
        &connector,
        grant(local, read_and_propose.clone())?,
        Permission::Read,
    )?;
    assert_eq!(reading.permissions, vec![Permission::Read]);
    let beyond_ceiling = err_of(authorize_workspace(
        &connector,
        grant(local, read_and_propose)?,
        Permission::Propose,
    ))?;
    assert_eq!(beyond_ceiling.code, ErrorCode::Forbidden);
    let outside_workspaces = err_of(authorize_workspace(
        &connector,
        grant(foreign, vec![Permission::Read])?,
        Permission::Read,
    ))?;
    assert_eq!(outside_workspaces.code, ErrorCode::Forbidden);
    let tenant = TenantGrant {
        tenant_id: owner.tenant_id.clone(),
        permissions: vec![Permission::Admin],
    };
    assert!(authorize_tenant(&owner, tenant, Permission::Admin)?.allows(Permission::Admin));
    Ok(())
}

#[test]
fn only_read_write_and_propose_grants_may_be_cached() {
    // SPEC section 11: Approve, Review and Admin are always checked fresh.
    for (permission, cacheable) in [
        (Permission::Read, true),
        (Permission::Write, true),
        (Permission::Propose, true),
        (Permission::Approve, false),
        (Permission::Review, false),
        (Permission::Admin, false),
    ] {
        assert_eq!(may_cache(permission), cacheable, "{permission:?}");
    }
}

#[test]
fn a_service_route_may_administer_but_never_review_or_approve() -> TestResult {
    let workspace: WorkspaceId = serde_json::from_str("\"11111111-1111-4111-8111-111111111111\"")?;
    let service = Principal {
        subject: "test-service".to_owned(),
        tenant_id: local_tenant()?,
        route: AccessRoute::Service,
        client_id: None,
        delegation: None,
    };
    for forbidden in [Permission::Review, Permission::Approve] {
        let refused = err_of(check_route(&service, forbidden))?;
        assert_eq!(refused.code, ErrorCode::Forbidden, "{forbidden:?}");
        // A grant holding every permission does not lift the route rule.
        let refused = err_of(authorize_workspace(
            &service,
            grant(workspace, every_permission())?,
            forbidden,
        ))?;
        assert_eq!(refused.code, ErrorCode::Forbidden, "{forbidden:?}");
    }
    for allowed in [
        Permission::Read,
        Permission::Write,
        Permission::Propose,
        Permission::Admin,
    ] {
        check_route(&service, allowed)?;
        let effective = authorize_workspace(&service, grant(workspace, vec![allowed])?, allowed)?;
        assert!(effective.allows(allowed), "{allowed:?}");
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
