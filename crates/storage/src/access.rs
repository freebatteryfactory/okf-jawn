//! The local `AccessControl`: raw grants of a local installation, read fresh (no cache).
//!
//! - The installation owner (the `CredentialStore` installation identity), arriving as the
//!   local owner or in a browser session, holds every permission on every workspace of its
//!   tenant and is the tenant's administrator.
//! - A connector (route `mcp_delegation`, `client_id` the connector id) holds its recorded
//!   permissions (`read`, and `propose` when enabled) on the workspaces it lists, while it is not
//!   revoked, and nothing at the tenant.
//! - Any other subject holds what `grant_creator` recorded for it: every permission on a
//!   workspace it created.
//!
//! Route and delegation rules are core's; this adapter returns raw grants only.

use std::collections::BTreeSet;

use okf_jawn_contract::access::{AccessRoute, Connector, Permission, Principal};
use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::WorkspaceId;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::StorageScope;
use rusqlite::{Connection, OptionalExtension, params};

use crate::credentials::installation;
use crate::db::{Db, json, sql};
use crate::events::holds;

/// `AccessControl` over the records database.
#[derive(Debug, Clone)]
pub struct LocalAccess {
    db: Db,
}

/// Every permission, granted to an owner or a creator.
const ALL: [Permission; 6] = [
    Permission::Read,
    Permission::Write,
    Permission::Propose,
    Permission::Approve,
    Permission::Review,
    Permission::Admin,
];

impl LocalAccess {
    pub(crate) const fn new(db: Db) -> Self {
        Self { db }
    }
}

impl AccessControl for LocalAccess {
    fn authorize<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
        _permission: Permission,
    ) -> PortFuture<'a, WorkspaceGrant> {
        let principal = principal.clone();
        Box::pin(self.db.call(move |connection| {
            let permissions = permissions_on(connection, &principal, workspace)?;
            Ok(grant(&principal, workspace, permissions))
        }))
    }

    fn authorize_tenant<'a>(
        &'a self,
        principal: &'a Principal,
        _permission: Permission,
    ) -> PortFuture<'a, TenantGrant> {
        let principal = principal.clone();
        Box::pin(self.db.transaction(move |transaction| {
            let permissions = if is_owner(transaction, &principal)? {
                ALL.to_vec()
            } else {
                Vec::new()
            };
            Ok(TenantGrant {
                tenant_id: principal.tenant_id.clone(),
                permissions,
            })
        }))
    }

    fn grants<'a>(&'a self, principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        let principal = principal.clone();
        Box::pin(self.db.transaction(move |transaction| {
            let mut statement = transaction
                .prepare("SELECT workspace_id FROM workspaces WHERE tenant_id = ?1 ORDER BY rowid")
                .map_err(|error| sql(&error))?;
            let workspaces = statement
                .query_map([principal.tenant_id.as_str()], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|error| sql(&error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| sql(&error))?;
            let mut grants = Vec::new();
            for workspace in workspaces {
                let workspace: WorkspaceId = from_text!(workspace.as_str())?;
                let permissions = permissions_on(transaction, &principal, workspace)?;
                if !permissions.is_empty() {
                    grants.push(grant(&principal, workspace, permissions));
                }
            }
            Ok(grants)
        }))
    }

    fn grant_creator<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant> {
        let principal = principal.clone();
        Box::pin(self.db.transaction(move |transaction| {
            transaction
                .execute(
                    "INSERT INTO grants (tenant_id, workspace_id, subject, permissions)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (tenant_id, workspace_id, subject)
                     DO UPDATE SET permissions = excluded.permissions",
                    params![
                        principal.tenant_id.as_str(),
                        workspace.0.to_string(),
                        principal.subject,
                        serde_json::to_string(&ALL).map_err(|error| json(&error))?
                    ],
                )
                .map_err(|error| sql(&error))?;
            Ok(grant(&principal, workspace, ALL.to_vec()))
        }))
    }
}

/// The raw permissions `principal` holds on `workspace`; empty for a workspace the tenant does
/// not hold.
fn permissions_on(
    connection: &Connection,
    principal: &Principal,
    workspace: WorkspaceId,
) -> Result<Vec<Permission>, ApiError> {
    let tenant = principal.tenant_id.as_str();
    let workspace_text = workspace.0.to_string();
    if !holds(connection, tenant, &workspace_text)? {
        return Ok(Vec::new());
    }
    if principal.route == AccessRoute::McpDelegation {
        return Ok(connector_of(connection, principal)?
            .filter(|connector| connector.workspace_ids.contains(&workspace))
            .map(|connector| connector.permissions)
            .unwrap_or_default());
    }
    if is_owner(connection, principal)? {
        return Ok(ALL.to_vec());
    }
    let recorded: Option<String> = connection
        .query_row(
            "SELECT permissions FROM grants
             WHERE tenant_id = ?1 AND workspace_id = ?2 AND subject = ?3",
            params![tenant, workspace_text, principal.subject],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let permissions: BTreeSet<Permission> = recorded
        .map(|text| serde_json::from_str(&text).map_err(|error| json(&error)))
        .transpose()?
        .unwrap_or_default();
    Ok(permissions.into_iter().collect())
}

/// Whether `principal` is the installation owner in a human session.
fn is_owner(connection: &Connection, principal: &Principal) -> Result<bool, ApiError> {
    if !matches!(
        principal.route,
        AccessRoute::LocalOwner | AccessRoute::BrowserSession
    ) {
        return Ok(false);
    }
    Ok(installation(connection)?.subject == principal.subject)
}

/// The unrevoked connector a delegated principal acts through.
fn connector_of(
    connection: &Connection,
    principal: &Principal,
) -> Result<Option<Connector>, ApiError> {
    let Some(client) = &principal.client_id else {
        return Ok(None);
    };
    let record: Option<String> = connection
        .query_row(
            "SELECT record FROM connectors WHERE connector_id = ?1 AND revoked = 0",
            [client],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    record
        .map(|record| serde_json::from_str(&record).map_err(|error| json(&error)))
        .transpose()
}

fn grant(
    principal: &Principal,
    workspace: WorkspaceId,
    permissions: Vec<Permission>,
) -> WorkspaceGrant {
    WorkspaceGrant {
        scope: StorageScope {
            tenant_id: principal.tenant_id.clone(),
            workspace_id: workspace,
        },
        permissions,
    }
}
