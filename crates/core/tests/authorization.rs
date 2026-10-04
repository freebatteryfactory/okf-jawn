//! Capability and constraint are tested together, not by accepting universal refusal.

use std::error::Error;
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::identity::WorkspaceId;
use okf_jawn_core::access::authorize;

#[test]
fn delegated_proposal_is_allowed_but_approval_is_not() -> Result<(), Box<dyn Error>> {
    let workspace: WorkspaceId = serde_json::from_str("\"11111111-1111-4111-8111-111111111111\"")?;
    let principal = Principal { subject:"test-subject".to_owned(),route:AccessRoute::McpDelegation,
        workspace_ids:vec![workspace],permissions:vec![Permission::Read,Permission::Propose,Permission::Approve,Permission::Review,Permission::Write,Permission::Admin],client_id:Some("test-client".to_owned()) };
    assert!(authorize(&principal, &Permission::Read, Some(workspace)).is_ok());
    assert!(authorize(&principal, &Permission::Propose, Some(workspace)).is_ok());
    assert!(authorize(&principal, &Permission::Approve, Some(workspace)).is_err());
    assert!(authorize(&principal, &Permission::Review, Some(workspace)).is_err());
    assert!(authorize(&principal, &Permission::Write, Some(workspace)).is_err());
    assert!(authorize(&principal, &Permission::Admin, Some(workspace)).is_err());
    let human = Principal { route:AccessRoute::BrowserSession, ..principal };
    assert!(authorize(&human, &Permission::Approve, Some(workspace)).is_ok());
    Ok(())
}
