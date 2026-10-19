//! Hosted mode against WorkOS, with the real `workos::Client` driving a scripted transport, so
//! each call's path, query, body and decoding run here with no network:
//! - the raw `AccessControl` over WorkOS role assignments: the organization-admin rule, the
//!   whole raw grant, unknown workspaces without a WorkOS call, `grant_creator` writing only
//!   what is absent, `editors`, the error mapping and the time bound;
//! - the AuthKit sign-in: the authorize URL names the AuthKit provider, and the code exchange.
#![cfg(feature = "runtime")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::Method;
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{MutationId, TenantId, WorkspaceId};
use okf_jawn_contract::workspace::Workspace;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    NewWorkspace, Provenance, StorageScope, WorkspaceArchive, WorkspaceCatalog, WorkspaceUpdate,
};
use okf_jawn_server::authkit::WorkosLogin;
use okf_jawn_server::config::HostedConfig;
use okf_jawn_server::grants::{CachedAccess, SystemClock};
use okf_jawn_server::hosted::HostedLogin;
use okf_jawn_server::outbound::workos_client;
use okf_jawn_server::workos_access::WorkosAccess;
use serde_json::{Value, json};

use check::{TestResult, err_of, some};
use support::workos::{
    Reply, Scripted, assignment, error, list, membership, permission, resource, resource_member,
    user_value,
};

/// Workspaces a tenant holds, by id, and whether each is archived; every other method refuses.
struct Held {
    tenant: TenantId,
    workspaces: Mutex<Vec<(WorkspaceId, bool)>>,
}

const ORG: &str = "org_1";
const USER: &str = "user_1";
const MEMBERSHIP: &str = "om_1";
const A: &str = "11111111-1111-4111-8111-111111111111";
const B: &str = "22222222-2222-4222-8222-222222222222";
const C: &str = "33333333-3333-4333-8333-333333333333";

const ALL: [Permission; 6] = [
    Permission::Read,
    Permission::Write,
    Permission::Propose,
    Permission::Approve,
    Permission::Review,
    Permission::Admin,
];

impl WorkspaceCatalog for Held {
    fn list<'a>(
        &'a self,
        tenant: &'a TenantId,
        include_archived: bool,
    ) -> PortFuture<'a, Vec<Workspace>> {
        Box::pin(async move {
            if *tenant != self.tenant {
                return Ok(Vec::new());
            }
            let held = self
                .workspaces
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))?
                .clone();
            held.into_iter()
                .filter(|(_, archived)| include_archived || !archived)
                .map(|(id, archived)| workspace_value(id, archived))
                .collect()
        })
    }

    fn create<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _mutation_id: MutationId,
        _workspace: NewWorkspace,
    ) -> PortFuture<'a, Workspace> {
        Box::pin(async { Err(not_here()) })
    }

    fn open<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Workspace> {
        Box::pin(async move {
            let held = self
                .workspaces
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))?
                .clone();
            match held.into_iter().find(|(id, _)| *id == scope.workspace_id) {
                Some((id, archived)) if scope.tenant_id == self.tenant => {
                    workspace_value(id, archived)
                }
                _ => Err(ApiError::new(ErrorCode::NotFound, "No such workspace")),
            }
        })
    }

    fn update<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _update: WorkspaceUpdate,
    ) -> PortFuture<'a, Workspace> {
        Box::pin(async { Err(not_here()) })
    }

    fn archive<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _archive: WorkspaceArchive,
    ) -> PortFuture<'a, Workspace> {
        Box::pin(async { Err(not_here()) })
    }

    fn unarchive<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _author: Provenance,
    ) -> PortFuture<'a, Workspace> {
        Box::pin(async { Err(not_here()) })
    }
}

fn not_here() -> ApiError {
    ApiError::new(ErrorCode::Internal, "not used by the access adapter")
}

fn workspace_value(id: WorkspaceId, archived: bool) -> Result<Workspace, ApiError> {
    let at = "2026-10-18T00:00:00.000Z";
    serde_json::from_value(json!({
        "id": id,
        "name": "w",
        "description": "",
        "head": "0".repeat(40),
        "created_at": at,
        "archived_at": archived.then_some(at),
        "permissions": [],
    }))
    .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
}

fn id(text: &str) -> Result<WorkspaceId, serde_json::Error> {
    serde_json::from_value(Value::String(text.to_owned()))
}

fn config() -> HostedConfig {
    HostedConfig {
        client_id: "client_01".to_owned(),
        api_key: "test-api-key".to_owned(),
        issuer: "https://auth.example.test".to_owned(),
        resource: "https://okf.example.test/mcp".to_owned(),
        redirect_uri: "https://okf.example.test/auth/callback".to_owned(),
    }
}

fn caller() -> Result<Principal, Box<dyn std::error::Error>> {
    Ok(Principal {
        subject: USER.to_owned(),
        tenant_id: TenantId::try_from(ORG.to_owned())?,
        route: AccessRoute::BrowserSession,
        client_id: None,
        delegation: None,
    })
}

/// The adapter over `script`, its tenant holding `held` (id, archived).
fn access(
    script: &Arc<Scripted>,
    held: &[(&str, bool)],
    bound: Duration,
) -> Result<WorkosAccess, Box<dyn std::error::Error>> {
    let mut workspaces = Vec::new();
    for (text, archived) in held {
        workspaces.push((id(text)?, *archived));
    }
    let catalog = Held {
        tenant: TenantId::try_from(ORG.to_owned())?,
        workspaces: Mutex::new(workspaces),
    };
    let client = workos_client(&config(), Arc::clone(script) as _)?;
    Ok(WorkosAccess::new(client, Arc::new(catalog), bound))
}

const BOUND: Duration = Duration::from_secs(30);

/// The membership listing's answer: one member with `roles`.
fn member(roles: &[&str]) -> Reply {
    Reply::Json(
        200,
        list(vec![membership(MEMBERSHIP, USER, ORG, roles)], None),
    )
}

fn permissions(slugs: &[&str], after: Option<&str>) -> Reply {
    Reply::Json(
        200,
        list(slugs.iter().map(|slug| permission(slug)).collect(), after),
    )
}

fn permissions_path(workspace: &str) -> String {
    format!(
        "/authorization/organization_memberships/{MEMBERSHIP}/resources/workspace/{workspace}/permissions"
    )
}

/// The scripted transport answered everything it was asked and nothing was left over.
fn exhausted(script: &Scripted) -> Result<(), String> {
    if !script.unscripted().is_empty() {
        return Err(format!("unscripted requests: {:?}", script.unscripted()));
    }
    if script.left() != 0 {
        return Err(format!("{} scripted replies were not used", script.left()));
    }
    Ok(())
}

#[tokio::test]
async fn a_member_s_grant_is_every_workspace_permission_workos_evaluates() -> TestResult {
    let script = Scripted::new(vec![
        member(&["member"]),
        permissions(
            &["workspace:read", "workspace:propose", "billing:view"],
            None,
        ),
    ]);
    let adapter = access(&script, &[(A, false)], BOUND)?;
    let grant = adapter
        .authorize(&caller()?, id(A)?, Permission::Read)
        .await?;
    assert_eq!(
        grant.permissions,
        vec![Permission::Read, Permission::Propose]
    );
    assert_eq!(grant.scope.workspace_id, id(A)?);
    assert_eq!(grant.scope.tenant_id.as_str(), ORG);
    let seen = script.seen();
    let first = some(seen.first(), "the membership request")?;
    assert_eq!(first.method, Method::GET);
    assert_eq!(first.path, "/user_management/organization_memberships");
    assert_eq!(first.query.get("user_id").map(String::as_str), Some(USER));
    assert_eq!(
        first.query.get("organization_id").map(String::as_str),
        Some(ORG)
    );
    assert_eq!(first.authorization.as_deref(), Some("Bearer test-api-key"));
    let second = some(seen.get(1), "the permissions request")?;
    assert_eq!(second.method, Method::GET);
    assert_eq!(second.path, permissions_path(A));
    assert_eq!(second.query.get("limit").map(String::as_str), Some("100"));
    exhausted(&script)?;
    Ok(())
}

#[tokio::test]
async fn the_whole_grant_lets_the_cache_answer_a_write_after_a_read() -> TestResult {
    let script = Scripted::new(vec![
        member(&["member"]),
        permissions(&["workspace:read", "workspace:write"], None),
    ]);
    let cached = CachedAccess::new(
        Arc::new(access(&script, &[(A, false)], BOUND)?),
        Arc::new(SystemClock),
    );
    let principal = caller()?;
    cached
        .authorize(&principal, id(A)?, Permission::Read)
        .await?;
    let write = cached
        .authorize(&principal, id(A)?, Permission::Write)
        .await?;
    assert_eq!(write.permissions, vec![Permission::Read, Permission::Write]);
    assert_eq!(script.seen().len(), 2, "the write check asked WorkOS again");
    exhausted(&script)?;
    Ok(())
}

#[tokio::test]
async fn every_page_is_read_and_slugs_outside_the_vocabulary_are_ignored() -> TestResult {
    let script = Scripted::new(vec![
        member(&["member"]),
        permissions(&["workspace:read"], Some("perm_page_2")),
        permissions(
            &["workspace:delete", "workspace:write", "workspace:Read"],
            None,
        ),
    ]);
    let adapter = access(&script, &[(A, false)], BOUND)?;
    let grant = adapter
        .authorize(&caller()?, id(A)?, Permission::Write)
        .await?;
    assert_eq!(grant.permissions, vec![Permission::Read, Permission::Write]);
    let seen = script.seen();
    let third = some(seen.get(2), "the second page")?;
    assert_eq!(
        third.query.get("after").map(String::as_str),
        Some("perm_page_2")
    );
    exhausted(&script)?;
    Ok(())
}

#[tokio::test]
async fn a_workspace_the_tenant_does_not_hold_is_no_grant_and_asks_workos_nothing() -> TestResult {
    let script = Scripted::new(Vec::new());
    let adapter = access(&script, &[(A, false)], BOUND)?;
    for asked in [Permission::Read, Permission::Admin] {
        let grant = adapter.authorize(&caller()?, id(B)?, asked).await?;
        assert!(grant.permissions.is_empty());
        assert_eq!(grant.scope.workspace_id, id(B)?);
    }
    assert!(
        adapter
            .editors(&StorageScope {
                tenant_id: TenantId::try_from(ORG.to_owned())?,
                workspace_id: id(B)?,
            })
            .await?
            .is_empty()
    );
    assert!(script.seen().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_caller_without_a_membership_holds_nothing() -> TestResult {
    let script = Scripted::new(vec![
        Reply::Json(200, list(Vec::new(), None)),
        Reply::Json(200, list(Vec::new(), None)),
        Reply::Json(200, list(Vec::new(), None)),
        // No organization by that id: no membership either.
        error(404, "organization_not_found"),
    ]);
    let adapter = access(&script, &[(A, false)], BOUND)?;
    let principal = caller()?;
    assert!(
        adapter
            .authorize(&principal, id(A)?, Permission::Read)
            .await?
            .permissions
            .is_empty()
    );
    assert!(
        adapter
            .authorize_tenant(&principal, Permission::Admin)
            .await?
            .permissions
            .is_empty()
    );
    assert!(adapter.grants(&principal).await?.is_empty());
    assert!(
        adapter
            .authorize(&principal, id(A)?, Permission::Read)
            .await?
            .permissions
            .is_empty()
    );
    exhausted(&script)?;
    Ok(())
}

#[tokio::test]
async fn two_memberships_of_one_user_in_one_organization_are_a_fault() -> TestResult {
    let script = Scripted::new(vec![Reply::Json(
        200,
        list(
            vec![
                membership(MEMBERSHIP, USER, ORG, &["member"]),
                membership("om_2", USER, ORG, &["admin"]),
            ],
            None,
        ),
    )]);
    let adapter = access(&script, &[(A, false)], BOUND)?;
    let error = err_of(
        adapter
            .authorize(&caller()?, id(A)?, Permission::Read)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Internal);
    Ok(())
}

#[tokio::test]
async fn an_organization_admin_holds_everything_without_asking_for_permissions() -> TestResult {
    for roles in [&["admin"][..], &["member", "admin"][..]] {
        let script = Scripted::new(vec![member(roles), member(roles)]);
        let adapter = access(&script, &[(A, false)], BOUND)?;
        let principal = caller()?;
        let grant = adapter
            .authorize(&principal, id(A)?, Permission::Approve)
            .await?;
        assert_eq!(grant.permissions, ALL.to_vec(), "{roles:?}");
        let tenant = adapter
            .authorize_tenant(&principal, Permission::Admin)
            .await?;
        assert_eq!(tenant.permissions, ALL.to_vec());
        assert_eq!(tenant.tenant_id.as_str(), ORG);
        exhausted(&script)?;
    }
    // A member is no tenant administrator.
    let script = Scripted::new(vec![member(&["member"])]);
    let adapter = access(&script, &[(A, false)], BOUND)?;
    let tenant = adapter
        .authorize_tenant(&caller()?, Permission::Admin)
        .await?;
    assert!(tenant.permissions.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_workspace_workos_has_no_resource_for_is_no_grant() -> TestResult {
    let script = Scripted::new(vec![member(&["member"]), error(404, "resource_not_found")]);
    let adapter = access(&script, &[(A, false)], BOUND)?;
    let grant = adapter
        .authorize(&caller()?, id(A)?, Permission::Read)
        .await?;
    assert!(grant.permissions.is_empty());
    exhausted(&script)?;
    Ok(())
}

#[tokio::test]
async fn a_provider_fault_is_unavailable_retried_once_and_never_cached() -> TestResult {
    for fault in [error(500, "boom"), error(429, "slow down"), Reply::Down] {
        let script = Scripted::new(vec![
            fault.clone(),
            fault.clone(),
            member(&["member"]),
            permissions(&["workspace:read"], None),
        ]);
        let cached = CachedAccess::new(
            Arc::new(access(&script, &[(A, false)], BOUND)?),
            Arc::new(SystemClock),
        );
        let principal = caller()?;
        let error = err_of(cached.authorize(&principal, id(A)?, Permission::Read).await)?;
        assert_eq!(error.code, ErrorCode::Unavailable, "{fault:?}");
        assert_eq!(error.message, "The identity provider is unavailable");
        assert_eq!(script.seen().len(), 2, "one retry, no more: {fault:?}");
        // The failure was not cached: the next check asks again and succeeds.
        let grant = cached
            .authorize(&principal, id(A)?, Permission::Read)
            .await?;
        assert_eq!(grant.permissions, vec![Permission::Read]);
        exhausted(&script)?;
    }
    Ok(())
}

#[tokio::test]
async fn refused_credentials_and_unexpected_answers_quote_nothing_workos_said() -> TestResult {
    for (status, code, words) in [
        (
            401,
            ErrorCode::Unavailable,
            "The identity provider refused this server's credentials",
        ),
        (
            403,
            ErrorCode::Unavailable,
            "The identity provider refused this server's credentials",
        ),
        (
            418,
            ErrorCode::Internal,
            "The identity provider answered unexpectedly",
        ),
    ] {
        let script = Scripted::new(vec![error(status, "SECRET-ECHO test-api-key")]);
        let adapter = access(&script, &[(A, false)], BOUND)?;
        let error = err_of(
            adapter
                .authorize(&caller()?, id(A)?, Permission::Read)
                .await,
        )?;
        assert_eq!(error.code, code, "{status}");
        assert_eq!(error.message, words);
        assert!(!format!("{error:?}").contains("SECRET-ECHO"));
        // Not retried: a refusal of the credentials does not change with a second try.
        assert_eq!(script.seen().len(), 1);
    }
    Ok(())
}

#[tokio::test]
async fn a_call_that_never_ends_is_unavailable_within_its_bound() -> TestResult {
    let script = Scripted::new(vec![Reply::Hang]);
    let adapter = access(&script, &[(A, false)], Duration::from_millis(100))?;
    let began = std::time::Instant::now();
    let error = err_of(
        adapter
            .authorize(&caller()?, id(A)?, Permission::Read)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert_eq!(
        error.message,
        "The identity provider did not answer in time"
    );
    assert!(began.elapsed() < Duration::from_secs(10));
    Ok(())
}

#[tokio::test]
async fn an_admin_s_listing_is_every_workspace_the_tenant_holds() -> TestResult {
    let script = Scripted::new(vec![member(&["admin"])]);
    let adapter = access(&script, &[(A, true), (B, false)], BOUND)?;
    let grants = adapter.grants(&caller()?).await?;
    let listed: Vec<_> = grants
        .iter()
        .map(|grant| (grant.scope.workspace_id, grant.permissions.clone()))
        .collect();
    assert_eq!(listed, vec![(id(A)?, ALL.to_vec()), (id(B)?, ALL.to_vec())]);
    exhausted(&script)?;
    Ok(())
}

#[tokio::test]
async fn a_member_s_listing_is_its_assigned_held_workspaces_with_permissions() -> TestResult {
    let script = Scripted::new(vec![
        member(&["member"]),
        Reply::Json(
            200,
            list(
                vec![
                    assignment(MEMBERSHIP, "workspace-editor", A),
                    assignment(MEMBERSHIP, "workspace-viewer", B),
                    assignment(MEMBERSHIP, "workspace-viewer", C),
                    assignment(MEMBERSHIP, "workspace-viewer", "not-a-uuid"),
                ],
                None,
            ),
        ),
        permissions(
            &["workspace:read", "workspace:write", "workspace:propose"],
            None,
        ),
        permissions(&[], None),
    ]);
    let adapter = access(&script, &[(A, false), (B, false)], BOUND)?;
    let grants = adapter.grants(&caller()?).await?;
    let listed: Vec<_> = grants
        .iter()
        .map(|grant| (grant.scope.workspace_id, grant.permissions.clone()))
        .collect();
    assert_eq!(
        listed,
        vec![(
            id(A)?,
            vec![Permission::Read, Permission::Write, Permission::Propose]
        )]
    );
    let seen = script.seen();
    let assignments = some(seen.get(1), "the assignments request")?;
    assert_eq!(
        assignments.path,
        format!("/authorization/organization_memberships/{MEMBERSHIP}/role_assignments")
    );
    assert_eq!(
        assignments
            .query
            .get("resource_type_slug")
            .map(String::as_str),
        Some("workspace")
    );
    let paths: Vec<_> = seen
        .iter()
        .skip(2)
        .map(|request| request.path.clone())
        .collect();
    assert_eq!(paths, vec![permissions_path(A), permissions_path(B)]);
    exhausted(&script)?;
    Ok(())
}

#[tokio::test]
async fn the_creator_s_resource_and_role_are_written_when_absent() -> TestResult {
    let script = Scripted::new(vec![
        member(&["admin"]),
        error(404, "resource_not_found"),
        Reply::Json(201, resource(A, ORG)),
        Reply::Json(200, list(Vec::new(), None)),
        Reply::Json(201, assignment(MEMBERSHIP, "workspace-admin", A)),
    ]);
    let adapter = access(&script, &[(A, false)], BOUND)?;
    let grant = adapter.grant_creator(&caller()?, id(A)?).await?;
    assert_eq!(grant.permissions, ALL.to_vec());
    assert_eq!(grant.scope.workspace_id, id(A)?);
    let seen = script.seen();
    let looked = some(seen.get(1), "the resource lookup")?;
    assert_eq!(looked.method, Method::GET);
    assert_eq!(
        looked.path,
        format!("/authorization/organizations/{ORG}/resources/workspace/{A}")
    );
    let created = some(seen.get(2), "the resource creation")?;
    assert_eq!(created.method, Method::POST);
    assert_eq!(created.path, "/authorization/resources");
    assert_eq!(
        created.body,
        Some(json!({
            "external_id": A,
            "name": A,
            "resource_type_slug": "workspace",
            "organization_id": ORG,
        }))
    );
    let listed = some(seen.get(3), "the assignment lookup")?;
    assert_eq!(
        listed.query.get("resource_external_id").map(String::as_str),
        Some(A)
    );
    let assigned = some(seen.get(4), "the role assignment")?;
    assert_eq!(assigned.method, Method::POST);
    assert_eq!(
        assigned.path,
        format!("/authorization/organization_memberships/{MEMBERSHIP}/role_assignments")
    );
    assert_eq!(
        assigned.body,
        Some(json!({
            "role_slug": "workspace-admin",
            "resource_external_id": A,
            "resource_type_slug": "workspace",
        }))
    );
    exhausted(&script)?;
    Ok(())
}

#[tokio::test]
async fn a_retried_creation_finds_both_and_writes_nothing() -> TestResult {
    let script = Scripted::new(vec![
        member(&["admin"]),
        Reply::Json(200, resource(A, ORG)),
        Reply::Json(
            200,
            list(vec![assignment(MEMBERSHIP, "workspace-admin", A)], None),
        ),
    ]);
    let adapter = access(&script, &[(A, false)], BOUND)?;
    adapter.grant_creator(&caller()?, id(A)?).await?;
    assert!(
        script
            .seen()
            .iter()
            .all(|request| request.method == Method::GET)
    );
    exhausted(&script)?;
    Ok(())
}

#[tokio::test]
async fn a_resource_of_another_organization_is_a_fault_and_nothing_is_written() -> TestResult {
    let script = Scripted::new(vec![
        member(&["admin"]),
        Reply::Json(200, resource(A, "org_2")),
    ]);
    let adapter = access(&script, &[(A, false)], BOUND)?;
    let error = err_of(adapter.grant_creator(&caller()?, id(A)?).await)?;
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(
        script
            .seen()
            .iter()
            .all(|request| request.method == Method::GET)
    );
    exhausted(&script)?;
    Ok(())
}

#[tokio::test]
async fn a_creator_without_a_membership_is_refused() -> TestResult {
    let script = Scripted::new(vec![Reply::Json(200, list(Vec::new(), None))]);
    let adapter = access(&script, &[(A, false)], BOUND)?;
    let error = err_of(adapter.grant_creator(&caller()?, id(A)?).await)?;
    assert_eq!(error.code, ErrorCode::Forbidden);
    exhausted(&script)?;
    Ok(())
}

#[tokio::test]
async fn editors_are_active_write_holders_and_every_organization_admin() -> TestResult {
    let script = Scripted::new(vec![
        Reply::Json(
            200,
            list(
                vec![
                    resource_member("om_u1", "user_u1", ORG, "active"),
                    resource_member("om_u2", "user_u2", ORG, "inactive"),
                ],
                None,
            ),
        ),
        Reply::Json(
            200,
            list(
                vec![
                    membership("om_u2", "user_u2", ORG, &["admin"]),
                    membership("om_u3", "user_u3", ORG, &["member", "admin"]),
                    membership("om_u4", "user_u4", ORG, &["member"]),
                ],
                None,
            ),
        ),
    ]);
    let adapter = access(&script, &[(A, false)], BOUND)?;
    let mut editors = adapter
        .editors(&StorageScope {
            tenant_id: TenantId::try_from(ORG.to_owned())?,
            workspace_id: id(A)?,
        })
        .await?;
    editors.sort();
    editors.dedup();
    assert_eq!(editors, vec!["user_u1", "user_u2", "user_u3"]);
    let seen = script.seen();
    let holders = some(seen.first(), "the holders request")?;
    assert_eq!(
        holders.path,
        format!(
            "/authorization/organizations/{ORG}/resources/workspace/{A}/organization_memberships"
        )
    );
    assert_eq!(
        holders.query.get("permission_slug").map(String::as_str),
        Some("workspace:write")
    );
    assert_eq!(
        holders.query.get("assignment").map(String::as_str),
        Some("indirect")
    );
    let admins = some(seen.get(1), "the admins request")?;
    assert_eq!(admins.path, "/user_management/organization_memberships");
    assert_eq!(
        admins.query.get("organization_id").map(String::as_str),
        Some(ORG)
    );
    assert!(!admins.query.contains_key("user_id"));
    exhausted(&script)?;
    Ok(())
}

#[tokio::test]
async fn a_listing_longer_than_the_page_bound_is_refused_never_cut() -> TestResult {
    let mut replies = vec![member(&["member"])];
    for page in 0..okf_jawn_server::workos_access::MAX_PAGES {
        replies.push(permissions(
            &["workspace:read"],
            Some(&format!("page_{page}")),
        ));
    }
    let script = Scripted::new(replies);
    let adapter = access(&script, &[(A, false)], BOUND)?;
    let error = err_of(
        adapter
            .authorize(&caller()?, id(A)?, Permission::Read)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Unavailable);
    exhausted(&script)?;
    Ok(())
}

#[tokio::test]
async fn a_sign_in_begins_at_the_authkit_authorize_url() -> TestResult {
    let script = Scripted::new(Vec::new());
    let login = WorkosLogin::new(
        workos_client(&config(), Arc::clone(&script) as _)?,
        &config(),
    );
    let start = login.begin()?;
    let url = url::Url::parse(&start.url)?;
    assert_eq!(url.origin().ascii_serialization(), "https://api.workos.com");
    assert_eq!(url.path(), "/user_management/authorize");
    let query: std::collections::BTreeMap<String, String> = url
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    let get = |name: &str| query.get(name).map(String::as_str);
    assert_eq!(get("provider"), Some("authkit"));
    assert_eq!(get("client_id"), Some("client_01"));
    assert_eq!(
        get("redirect_uri"),
        Some("https://okf.example.test/auth/callback")
    );
    assert_eq!(get("response_type"), Some("code"));
    assert_eq!(get("code_challenge_method"), Some("S256"));
    assert_eq!(get("state"), Some(start.state.as_str()));
    assert!(some(get("code_challenge"), "the code challenge")?.len() >= 43);
    // Building the URL asks WorkOS nothing.
    assert!(script.seen().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_completed_sign_in_names_the_user_and_organization() -> TestResult {
    let script = Scripted::new(vec![Reply::Json(
        200,
        json!({
            "user": user_value(USER),
            "organization_id": ORG,
            "access_token": "an-access-token",
            "refresh_token": "a-refresh-token",
        }),
    )]);
    let login = WorkosLogin::new(
        workos_client(&config(), Arc::clone(&script) as _)?,
        &config(),
    );
    let user = login.complete("the-code", "the-verifier").await?;
    assert_eq!(user.subject, USER);
    assert_eq!(user.organization_id, ORG);
    let seen = script.seen();
    let exchange = some(seen.first(), "the code exchange")?;
    assert_eq!(exchange.method, Method::POST);
    assert_eq!(exchange.path, "/user_management/authenticate");
    let body = some(exchange.body.clone(), "the exchange body")?;
    assert_eq!(body.get("code"), Some(&json!("the-code")));
    assert_eq!(body.get("code_verifier"), Some(&json!("the-verifier")));
    assert_eq!(body.get("grant_type"), Some(&json!("authorization_code")));
    exhausted(&script)?;
    Ok(())
}

#[tokio::test]
async fn a_sign_in_without_an_organization_or_refused_by_workos_opens_nothing() -> TestResult {
    let script = Scripted::new(vec![
        Reply::Json(
            200,
            json!({
                "user": user_value(USER),
                "access_token": "an-access-token",
                "refresh_token": "a-refresh-token",
            }),
        ),
        error(400, "invalid_grant test-api-key the-code"),
    ]);
    let login = WorkosLogin::new(
        workos_client(&config(), Arc::clone(&script) as _)?,
        &config(),
    );
    let no_organization = err_of(login.complete("the-code", "the-verifier").await)?;
    assert_eq!(no_organization.code, ErrorCode::Forbidden);
    let refused = err_of(login.complete("the-code", "the-verifier").await)?;
    assert_eq!(refused.code, ErrorCode::Unauthenticated);
    assert_eq!(refused.message, "The sign-in was not accepted");
    assert!(!format!("{refused:?}").contains("test-api-key"));
    exhausted(&script)?;
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
mod support {
    pub mod workos;
}
