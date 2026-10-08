//! Drafts, backup, restore and purge belong to a human session: no grant lets an agent or a
//! service identity reach them, and every such refusal is recorded.

use okf_jawn_contract::access::{AccessRoute, DelegationCeiling, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::IdentityError;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::operations::{DRAFT_BEARING, HUMAN_SESSION_ONLY};
use okf_jawn_core::access::{check_human_route, is_human_session};
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::events::EventScope;
use okf_jawn_core::storage::StorageScope;
use serde_json::{Value, json};

use check::{TestResult, err_of};
use support::counting::CountingApplication;
use support::{FixturePorts, GrantTable, all_permissions, tenant, workspace};

const WORKSPACE_A: &str = "11111111-1111-4111-8111-111111111111";
const ITEM: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const KEY: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const UPLOAD: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const REVISION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn principal(route: AccessRoute) -> Result<Principal, IdentityError> {
    Ok(Principal {
        subject: "alice".to_owned(),
        tenant_id: tenant("tenant-local")?,
        route,
        client_id: None,
        delegation: None,
    })
}

/// Alice's connector, delegated every permission she holds.
fn connector() -> Result<Principal, IdentityError> {
    let mut delegated = principal(AccessRoute::McpDelegation)?;
    delegated.client_id = Some("connector".to_owned());
    delegated.delegation = Some(DelegationCeiling {
        permissions: all_permissions(),
        workspace_ids: None,
    });
    Ok(delegated)
}

/// Alice holds every permission on workspace A and on the tenant.
fn every_grant() -> Result<FixturePorts, serde_json::Error> {
    let mut table = GrantTable::default();
    table
        .workspaces
        .entry("alice".to_owned())
        .or_default()
        .insert(workspace(WORKSPACE_A)?, all_permissions());
    table.tenants.insert("alice".to_owned(), all_permissions());
    Ok(FixturePorts::new(table))
}

/// A request each listed operation's schema accepts.
fn input(operation: OperationName) -> Result<Value, String> {
    match operation {
        OperationName::GetItem => {
            Ok(json!({ "workspace_id": WORKSPACE_A, "item_id": ITEM, "at": { "kind": "latest" } }))
        }
        OperationName::SaveDraft => Ok(json!({
            "workspace_id": WORKSPACE_A, "item_id": ITEM, "base_revision": REVISION,
            "body": "drafted", "properties": {}, "idempotency_key": KEY
        })),
        OperationName::ListDrafts => {
            Ok(json!({ "workspace_id": WORKSPACE_A, "page": { "limit": 10 } }))
        }
        OperationName::DiscardDraft => {
            Ok(json!({ "workspace_id": WORKSPACE_A, "item_id": ITEM, "idempotency_key": KEY }))
        }
        OperationName::CommitItems => Ok(json!({
            "workspace_id": WORKSPACE_A, "item_ids": [ITEM], "message": "snapshot",
            "idempotency_key": KEY
        })),
        OperationName::PurgeWorkspace => Ok(json!({
            "workspace_id": WORKSPACE_A, "base_revision": REVISION, "idempotency_key": KEY
        })),
        OperationName::BackupWorkspace => {
            Ok(json!({ "workspace_id": WORKSPACE_A, "idempotency_key": KEY }))
        }
        OperationName::RestoreWorkspace => Ok(json!({
            "workspace_id": WORKSPACE_A, "upload_id": UPLOAD, "sha256": "c".repeat(64),
            "idempotency_key": KEY
        })),
        OperationName::BackupInstallation => Ok(json!({ "idempotency_key": KEY })),
        OperationName::PurgeItem => Ok(json!({
            "workspace_id": WORKSPACE_A, "item_id": ITEM, "base_revision": REVISION,
            "idempotency_key": KEY
        })),
        other => Err(format!("{} has no request here", other.as_str())),
    }
}

async fn call(
    app: &CountingApplication,
    ports: &FixturePorts,
    principal: &Principal,
    operation: &str,
    input: Value,
) -> Result<Value, ApiError> {
    let caller = Caller {
        principal,
        session_id: None,
    };
    let dispatch_ports = DispatchPorts {
        access: ports.access.as_ref(),
        mutations: ports.mutations.as_ref(),
        events: ports.events.as_ref(),
    };
    dispatch(app, &dispatch_ports, &caller, operation, input).await
}

#[tokio::test]
async fn human_session_only_operations_are_refused_on_agent_and_service_routes() -> TestResult {
    let ports = every_grant()?;
    let app = CountingApplication::default();
    let agents = [connector()?, principal(AccessRoute::Service)?];
    let listed: Vec<OperationName> = HUMAN_SESSION_ONLY
        .iter()
        .chain(DRAFT_BEARING.iter())
        .copied()
        .collect();
    assert_eq!(listed.len(), 10);
    let workspace_scope = EventScope::Workspace(StorageScope {
        tenant_id: tenant("tenant-local")?,
        workspace_id: workspace(WORKSPACE_A)?,
    });
    let tenant_scope = EventScope::Tenant(tenant("tenant-local")?);
    for operation in listed {
        let id = operation.as_str();
        for agent in &agents {
            let before = ports.events.appended()?.len();
            let refused = err_of(call(&app, &ports, agent, id, input(operation)?).await)?;
            assert_eq!(
                refused.code,
                ErrorCode::Forbidden,
                "{id} via {:?}",
                agent.route
            );
            // The route decides, before any grant is asked for and with every grant held.
            assert!(check_human_route(agent, operation).is_err(), "{id}");
            let appended = ports.events.appended()?;
            assert_eq!(appended.len(), before + 1, "{id} via {:?}", agent.route);
            let (scope, event) = appended.last().ok_or("an event was recorded")?;
            assert_eq!(event.kind, EventKind::PermissionDenied);
            assert_eq!(event.operation, Some(operation));
            let actor = event.actor.as_ref().ok_or("a refusal names its actor")?;
            assert_eq!(actor.subject, "alice");
            assert_eq!(actor.route, agent.route);
            // Purges and the installation backup authorize at the tenant only (Stage 1b design
            // sections 6 and 8), so their refusal is a tenant event.
            let tenant_level = matches!(
                operation,
                OperationName::BackupInstallation
                    | OperationName::PurgeWorkspace
                    | OperationName::PurgeItem
            );
            let expected = if tenant_level {
                &tenant_scope
            } else {
                &workspace_scope
            };
            assert_eq!(scope, expected, "{id}");
        }
        assert_eq!(app.call_count(id)?, 0, "{id}");
    }
    assert_eq!(ports.access.lookups(), 0);
    Ok(())
}

#[test]
fn a_browser_session_and_the_local_owner_are_human_sessions() -> TestResult {
    for (route, human) in [
        (AccessRoute::BrowserSession, true),
        (AccessRoute::LocalOwner, true),
        (AccessRoute::McpDelegation, false),
        (AccessRoute::Service, false),
    ] {
        let caller = principal(route.clone())?;
        assert_eq!(is_human_session(&caller), human, "{route:?}");
        for operation in HUMAN_SESSION_ONLY.iter().chain(DRAFT_BEARING.iter()) {
            assert_eq!(check_human_route(&caller, *operation).is_ok(), human);
        }
        // An ordinary write is not a human-session operation.
        check_human_route(&caller, OperationName::CreateItem)?;
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
mod support;
