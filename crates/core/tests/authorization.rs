//! Capability and constraint are tested together, not by accepting universal refusal.

use okf_jawn_contract::access::{AccessRoute, DelegationCeiling, Permission, Principal};
use okf_jawn_contract::identity::{TenantId, WorkspaceId};
use okf_jawn_core::access::{authorize_tenant, authorize_workspace, check_route};
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::storage::StorageScope;
use std::error::Error;

fn grant(workspace: WorkspaceId, permissions: Vec<Permission>) -> WorkspaceGrant {
    WorkspaceGrant {
        scope: StorageScope {
            tenant_id: TenantId::try_from("tenant-local".to_owned()).expect("tenant"),
            workspace_id: workspace,
        },
        permissions,
    }
}

#[test]
fn delegated_proposal_is_allowed_but_approval_is_not() -> Result<(), Box<dyn Error>> {
    let workspace: WorkspaceId = serde_json::from_str("\"11111111-1111-4111-8111-111111111111\"")?;
    let principal = Principal {
        subject: "test-subject".to_owned(),
        tenant_id: TenantId::try_from("tenant-local".to_owned())?,
        route: AccessRoute::McpDelegation,
        client_id: Some("test-client".to_owned()),
        delegation: Some(DelegationCeiling {
            permissions: vec![
                Permission::Read,
                Permission::Propose,
                Permission::Approve,
                Permission::Review,
                Permission::Write,
                Permission::Admin,
            ],
            workspace_ids: Some(vec![workspace]),
        }),
    };
    let raw = grant(
        workspace,
        vec![
            Permission::Read,
            Permission::Propose,
            Permission::Approve,
            Permission::Review,
            Permission::Write,
            Permission::Admin,
        ],
    );
    assert!(authorize_workspace(&principal, raw.clone(), Permission::Read).is_ok());
    assert!(authorize_workspace(&principal, raw.clone(), Permission::Propose).is_ok());
    assert!(authorize_workspace(&principal, raw.clone(), Permission::Approve).is_err());
    assert!(authorize_workspace(&principal, raw.clone(), Permission::Review).is_err());
    assert!(authorize_workspace(&principal, raw.clone(), Permission::Write).is_err());
    assert!(authorize_workspace(&principal, raw, Permission::Admin).is_err());
    let human = Principal {
        route: AccessRoute::BrowserSession,
        delegation: None,
        ..principal
    };
    assert!(check_route(&human, Permission::Approve).is_ok());
    assert!(
        authorize_workspace(
            &human,
            grant(workspace, vec![Permission::Approve]),
            Permission::Approve
        )
        .is_ok()
    );
    Ok(())
}

#[test]
fn local_owner_uses_the_same_rules_and_cannot_reach_other_workspaces() -> Result<(), Box<dyn Error>>
{
    let local: WorkspaceId = serde_json::from_str("\"11111111-1111-4111-8111-111111111111\"")?;
    let foreign: WorkspaceId = serde_json::from_str("\"33333333-3333-4333-8333-333333333333\"")?;
    let owner = Principal {
        subject: "local-installation-owner".to_owned(),
        tenant_id: TenantId::try_from("tenant-local".to_owned())?,
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
        assert!(authorize_workspace(&owner, grant(local, vec![permission]), permission).is_ok());
        assert!(authorize_workspace(&owner, grant(foreign, Vec::new()), permission).is_err());
    }
    let connector = Principal {
        subject: "local-installation-owner".to_owned(),
        tenant_id: TenantId::try_from("tenant-local".to_owned())?,
        route: AccessRoute::McpDelegation,
        client_id: Some("local-connector".to_owned()),
        delegation: Some(DelegationCeiling {
            permissions: vec![Permission::Read],
            workspace_ids: Some(vec![local]),
        }),
    };
    assert!(
        authorize_workspace(
            &connector,
            grant(local, vec![Permission::Read, Permission::Propose]),
            Permission::Read
        )
        .is_ok()
    );
    assert!(
        authorize_workspace(
            &connector,
            grant(local, vec![Permission::Read, Permission::Propose]),
            Permission::Propose
        )
        .is_err()
    );
    assert!(
        authorize_workspace(
            &connector,
            grant(foreign, vec![Permission::Read]),
            Permission::Read
        )
        .is_err()
    );
    let tenant = TenantGrant {
        tenant_id: owner.tenant_id.clone(),
        permissions: vec![Permission::Admin],
    };
    assert!(authorize_tenant(&owner, tenant, Permission::Admin).is_ok());
    Ok(())
}
