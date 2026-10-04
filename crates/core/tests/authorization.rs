//! Capability and constraint are tested together, not by accepting universal refusal.

use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::identity::WorkspaceId;
use okf_jawn_core::access::authorize;
use std::error::Error;

#[test]
fn delegated_proposal_is_allowed_but_approval_is_not() -> Result<(), Box<dyn Error>> {
    let workspace: WorkspaceId = serde_json::from_str("\"11111111-1111-4111-8111-111111111111\"")?;
    let principal = Principal {
        subject: "test-subject".to_owned(),
        route: AccessRoute::McpDelegation,
        workspace_ids: vec![workspace],
        permissions: vec![
            Permission::Read,
            Permission::Propose,
            Permission::Approve,
            Permission::Review,
            Permission::Write,
            Permission::Admin,
        ],
        client_id: Some("test-client".to_owned()),
    };
    assert!(authorize(&principal, &Permission::Read, Some(workspace)).is_ok());
    assert!(authorize(&principal, &Permission::Propose, Some(workspace)).is_ok());
    assert!(authorize(&principal, &Permission::Approve, Some(workspace)).is_err());
    assert!(authorize(&principal, &Permission::Review, Some(workspace)).is_err());
    assert!(authorize(&principal, &Permission::Write, Some(workspace)).is_err());
    assert!(authorize(&principal, &Permission::Admin, Some(workspace)).is_err());
    let human = Principal {
        route: AccessRoute::BrowserSession,
        ..principal
    };
    assert!(authorize(&human, &Permission::Approve, Some(workspace)).is_ok());
    Ok(())
}

#[test]
fn local_owner_uses_the_same_rules_and_cannot_reach_other_workspaces() -> Result<(), Box<dyn Error>>
{
    let local: WorkspaceId = serde_json::from_str("\"11111111-1111-4111-8111-111111111111\"")?;
    let foreign: WorkspaceId = serde_json::from_str("\"33333333-3333-4333-8333-333333333333\"")?;
    let owner = Principal {
        subject: "local-installation-owner".to_owned(),
        route: AccessRoute::LocalOwner,
        workspace_ids: vec![local],
        permissions: vec![
            Permission::Read,
            Permission::Write,
            Permission::Propose,
            Permission::Approve,
            Permission::Review,
            Permission::Admin,
        ],
        client_id: None,
    };
    for permission in [
        Permission::Read,
        Permission::Write,
        Permission::Approve,
        Permission::Review,
        Permission::Admin,
    ] {
        assert!(authorize(&owner, &permission, Some(local)).is_ok());
        assert!(authorize(&owner, &permission, Some(foreign)).is_err());
    }
    let connector = Principal {
        subject: "local-installation-owner".to_owned(),
        route: AccessRoute::McpDelegation,
        workspace_ids: vec![local],
        permissions: vec![Permission::Read],
        client_id: Some("local-connector".to_owned()),
    };
    assert!(authorize(&connector, &Permission::Read, Some(local)).is_ok());
    assert!(authorize(&connector, &Permission::Propose, Some(local)).is_err());
    assert!(authorize(&connector, &Permission::Read, Some(foreign)).is_err());
    Ok(())
}
