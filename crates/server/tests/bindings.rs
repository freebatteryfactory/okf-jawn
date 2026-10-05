//! Real router plus a test-only Application fixture checks binding behavior, not storage.

use axum::{
    Extension,
    body::{Body, to_bytes},
    http::Request,
};
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{MutationId, TenantId, WorkspaceId};
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::mutations::{AbandonedEffects, BeginOutcome, MutationKey, MutationStore, StoredResponse};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::StorageScope;
use okf_jawn_server::BoundApplication;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn real_route_uses_the_injected_application() -> Result<(), Box<dyn Error>> {
    let principal = Principal {
        subject: "control".to_owned(),
        tenant_id: TenantId::try_from("tenant-local".to_owned())?,
        route: AccessRoute::LocalOwner,
        client_id: None,
        delegation: None,
    };
    let fixture = support::FixtureApplication {
        responses: BTreeMap::from([(
            "list_workspaces".to_owned(),
            json!({"items":[]}),
        )]),
    };
    let bound = BoundApplication {
        application: Arc::new(fixture),
        access: Arc::new(PermitAllAccess),
        mutations: Arc::new(NoopMutations),
        effects: Arc::new(NoopEffects),
    };
    let app = okf_jawn_server::router(bound).layer(Extension(principal));
    let request = Request::post("/api/workspaces/list-workspaces")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"page":{"limit":10}}"#))?;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), 200);
    let bytes = to_bytes(response.into_body(), 1_048_576).await?;
    let body: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(body.get("items"), Some(&json!([])));
    Ok(())
}

struct PermitAllAccess;

impl AccessControl for PermitAllAccess {
    fn authorize<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
        _permission: Permission,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move {
            Ok(WorkspaceGrant {
                scope: StorageScope {
                    tenant_id: principal.tenant_id.clone(),
                    workspace_id: workspace,
                },
                permissions: vec![
                    Permission::Read,
                    Permission::Write,
                    Permission::Propose,
                    Permission::Approve,
                    Permission::Review,
                    Permission::Admin,
                ],
            })
        })
    }

    fn authorize_tenant<'a>(
        &'a self,
        principal: &'a Principal,
        _permission: Permission,
    ) -> PortFuture<'a, TenantGrant> {
        Box::pin(async move {
            Ok(TenantGrant {
                tenant_id: principal.tenant_id.clone(),
                permissions: vec![
                    Permission::Read,
                    Permission::Write,
                    Permission::Propose,
                    Permission::Approve,
                    Permission::Review,
                    Permission::Admin,
                ],
            })
        })
    }

    fn grants<'a>(&'a self, _principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn grant_creator<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant> {
        self.authorize(principal, workspace, Permission::Admin)
    }
}

struct NoopMutations;

impl MutationStore for NoopMutations {
    fn begin<'a>(
        &'a self,
        _key: &'a MutationKey,
        _digest: &'a okf_jawn_contract::identity::Digest,
    ) -> PortFuture<'a, BeginOutcome> {
        Box::pin(async {
            Err(ApiError::new(
                ErrorCode::Internal,
                "mutations unused in binding smoke test",
            ))
        })
    }

    fn record_effect<'a>(
        &'a self,
        _mutation_id: MutationId,
        _effect: Value,
    ) -> PortFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn complete<'a>(&'a self, _mutation_id: MutationId, _response: Value) -> PortFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn find<'a>(&'a self, _mutation_id: MutationId) -> PortFuture<'a, Option<StoredResponse>> {
        Box::pin(async { Ok(None) })
    }
}

struct NoopEffects;

impl AbandonedEffects for NoopEffects {
    fn lookup<'a>(
        &'a self,
        _operation: okf_jawn_contract::metadata::OperationName,
        _mutation_id: MutationId,
    ) -> PortFuture<'a, Option<Value>> {
        Box::pin(async { Ok(None) })
    }
}

#[path = "../../../tests/support/application.rs"]
mod support;
