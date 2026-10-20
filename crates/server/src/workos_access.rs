//! The hosted raw `AccessControl`: grants resolved through WorkOS authorization role
//! assignments (SPEC section 11: "the application resolves grants through WorkOS authorization
//! role assignments (org-level and per-workspace resources)"). The grant cache (`grants`) wraps
//! it; this module returns raw grants only, and route and delegation rules stay core's.
//!
//! The vocabulary is the owner's (2026-10-18), set up in WorkOS, not in this code: the resource
//! type `workspace` (its parent the system `organization`), one permission per `Permission` wire
//! name (`workspace:read`, `workspace:write`, `workspace:propose`, `workspace:approve`,
//! `workspace:review`, `workspace:admin`), the workspace roles `workspace-viewer`,
//! `workspace-contributor`, `workspace-editor`, `workspace-reviewer` and `workspace-admin`, and
//! the organization roles `admin` and `member`. Only three names are code: the resource type,
//! the permission prefix (each slug is `workspace:` and the permission's serde name, so the
//! contract side cannot drift), the creator's role `workspace-admin`, and the organization role
//! `admin`. Which permissions each workspace role carries is WorkOS's: a role edited there takes
//! effect without a release.
//!
//! - The caller is an organization membership: `list_organization_memberships(user_id, org)`,
//!   active ones only. No membership holds nothing; two are a fault.
//! - An organization `admin` holds every permission on every workspace of its tenant and on the
//!   tenant (the owner's rule: "organization admins are admins everywhere"), by its role slug,
//!   whatever WorkOS's own inheritance says.
//! - Anyone else holds, on a workspace, the `workspace:*` slugs of
//!   `list_effective_permissions_by_external_id(membership, "workspace", <workspace id>)`, which
//!   counts roles assigned on ancestor resources too; any other slug is ignored, and a resource
//!   WorkOS does not know is no grant. The tenant grant of a member is empty.
//! - `authorize` answers the whole raw grant, never only the permission asked for: the cache
//!   reuses a `read` answer for a `write` check (server M1 review). A workspace the tenant does not
//!   hold (the catalog's `NotFound`) is an empty grant found without asking WorkOS, as the local
//!   adapter answers it, so a flood of unknown workspace ids costs local reads, never API calls.
//! - `grants` lists, for an admin, every workspace of the catalog (archived included); for a
//!   member, every held workspace evaluated for effective permissions (including organization
//!   and ancestor grants without a direct assignment), leaving out those with none.
//! - `grant_creator` creates the workspace's resource (external id = the workspace id) when
//!   WorkOS has none, and assigns `workspace-admin` on it when the creator has no such
//!   assignment: it reads before it writes, so a retry of the same creation writes nothing.
//! - `editors` lists every active member WorkOS names with `workspace:write` on the resource,
//!   inherited assignments included, and every organization admin.
//!
//! Every call is bounded by the time `WorkosAccess::new` is given (`ACCESS_TIME_LIMIT` in
//! production); past it, or when WorkOS answers `429`, `5xx`, refuses the connection or sends
//! what does not decode, the answer is `Unavailable`. A `401` or `403` is `Unavailable` too, in
//! words that point at this server's credentials. No WorkOS message is quoted: it can echo the
//! request. A list longer than [`MAX_PAGES`] pages is `Unavailable`, never cut short.
//!
//! Known limits. A workspace purged in hosted mode leaves its WorkOS resource and assignments
//! behind: no `AccessControl` method removes them (carried to the item purge work item); that is
//! harmless, because the catalog no longer holds the workspace and workspace ids are never
//! reused. WorkOS grants are not in a workspace archive, so a hosted restore into a WorkOS
//! environment without the resource leaves the workspace to the organization admins until roles
//! are assigned again. These calls have not run against WorkOS yet: the tests drive the real
//! client over a scripted transport in the 3.3.0 model shapes, and the round trip is Wave 3's
//! `workos-end-to-end`.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{StreamExt, stream};
use okf_jawn_contract::access::{Permission, Principal};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::WorkspaceId;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{StorageScope, WorkspaceCatalog};
use workos::Client;
use workos::resources::authorization::{
    AssignRoleParams, AssignRoleParamsBody, CreateResourceParams, CreateResourceParamsBody,
    ListEffectivePermissionsByExternalIdParams, ListMembershipsForResourceByExternalIdParams,
    ListRoleAssignmentsParams, ResourceTarget,
};
use workos::resources::organization_membership::ListOrganizationMembershipsParams;
use workos::{
    AuthorizationAssignment, UserOrganizationMembershipBaseListDataStatus,
    UserOrganizationMembershipStatus,
};

/// `AccessControl` over WorkOS role assignments and the workspace catalog.
pub struct WorkosAccess {
    client: Arc<Client>,
    catalog: Arc<dyn WorkspaceCatalog>,
    bound: Duration,
}

/// The caller's organization membership.
struct Membership {
    id: String,
    admin: bool,
}

/// Why a listing ended without its entries.
enum Unlisted {
    /// WorkOS answered `404`: the resource or membership it lists under does not exist.
    Absent,
    /// Any other failure, mapped.
    Failed(ApiError),
}

/// The WorkOS resource type of a workspace.
pub const RESOURCE_TYPE: &str = "workspace";

/// What every workspace permission slug begins with; the rest is the permission's wire name.
pub const PERMISSION_PREFIX: &str = "workspace:";

/// The role `grant_creator` assigns on a new workspace.
pub const CREATOR_ROLE: &str = "workspace-admin";

/// The organization role that holds every permission everywhere.
pub const ORGANIZATION_ADMIN: &str = "admin";

/// How long one `AccessControl` call may take in production.
pub const ACCESS_TIME_LIMIT: Duration = Duration::from_secs(15);

/// The most pages one WorkOS listing is read for.
pub const MAX_PAGES: usize = 100;

/// Entries asked for on one page (WorkOS's most).
const PAGE: i64 = 100;

/// Every permission, in the contract's order.
const ALL: [Permission; 6] = [
    Permission::Read,
    Permission::Write,
    Permission::Propose,
    Permission::Approve,
    Permission::Review,
    Permission::Admin,
];

impl WorkosAccess {
    /// Grants from WorkOS through `client`, workspaces from `catalog`, each call bounded by
    /// `bound` ([`ACCESS_TIME_LIMIT`] in production).
    #[must_use]
    pub fn new(client: Arc<Client>, catalog: Arc<dyn WorkspaceCatalog>, bound: Duration) -> Self {
        Self {
            client,
            catalog,
            bound,
        }
    }

    /// `work`, bounded in time.
    async fn bounded<T>(
        &self,
        work: impl Future<Output = Result<T, ApiError>>,
    ) -> Result<T, ApiError> {
        tokio::time::timeout(self.bound, work)
            .await
            .unwrap_or_else(|_| {
                Err(ApiError::new(
                    ErrorCode::Unavailable,
                    "The identity provider did not answer in time",
                ))
            })
    }

    /// The membership of `principal` in its organization, when it has an active one.
    async fn membership(&self, principal: &Principal) -> Result<Option<Membership>, ApiError> {
        let params = ListOrganizationMembershipsParams {
            user_id: Some(principal.subject.clone()),
            organization_id: Some(principal.tenant_id.as_str().to_owned()),
            limit: Some(10),
            ..ListOrganizationMembershipsParams::default()
        };
        let page = match self
            .client
            .organization_membership()
            .list_organization_memberships(params)
            .await
        {
            Ok(page) => page,
            // No such organization: no membership in it.
            Err(error) if error.is_not_found() => return Ok(None),
            Err(error) => return Err(provider(&error)),
        };
        let mut memberships = page
            .data
            .into_iter()
            .filter(|membership| membership.status == UserOrganizationMembershipStatus::Active);
        let Some(first) = memberships.next() else {
            return Ok(None);
        };
        if memberships.next().is_some() {
            return Err(ApiError::new(
                ErrorCode::Internal,
                "The identity provider listed two memberships of one user in one organization",
            ));
        }
        let admin = std::iter::once(&first.role)
            .chain(first.roles.iter())
            .any(|role| role.slug == ORGANIZATION_ADMIN);
        Ok(Some(Membership {
            id: first.id,
            admin,
        }))
    }

    /// Whether the tenant of `principal` holds `workspace`, by the catalog.
    async fn holds(&self, scope: &StorageScope) -> Result<bool, ApiError> {
        match self.catalog.open(scope).await {
            Ok(_) => Ok(true),
            Err(error) if error.code == ErrorCode::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// The workspace permissions WorkOS evaluates for a member on `workspace`.
    async fn effective(
        &self,
        membership: &Membership,
        workspace: WorkspaceId,
    ) -> Result<Vec<Permission>, ApiError> {
        if membership.admin {
            return Ok(ALL.to_vec());
        }
        let client = &*self.client;
        let id = membership.id.clone();
        let external = workspace.0.to_string();
        let slugs = every_page(|after| {
            let (id, external) = (id.clone(), external.clone());
            async move {
                let params = ListEffectivePermissionsByExternalIdParams {
                    after,
                    limit: Some(PAGE),
                    ..ListEffectivePermissionsByExternalIdParams::default()
                };
                client
                    .authorization()
                    .list_effective_permissions_by_external_id(
                        &id,
                        RESOURCE_TYPE,
                        &external,
                        params,
                    )
                    .await
                    .map(|page| {
                        let slugs = page.data.into_iter().map(|permission| permission.slug);
                        (slugs.collect(), page.list_metadata.after)
                    })
            }
        })
        .await;
        match slugs {
            Ok(slugs) => Ok(permissions_of(&slugs)),
            Err(Unlisted::Absent) => Ok(Vec::new()),
            Err(Unlisted::Failed(error)) => Err(error),
        }
    }

    /// The raw grant of `principal` on `workspace`.
    async fn raw_grant(
        &self,
        principal: &Principal,
        workspace: WorkspaceId,
    ) -> Result<WorkspaceGrant, ApiError> {
        let scope = StorageScope {
            tenant_id: principal.tenant_id.clone(),
            workspace_id: workspace,
        };
        if !self.holds(&scope).await? {
            return Ok(grant(scope, Vec::new()));
        }
        let Some(membership) = self.membership(principal).await? else {
            return Ok(grant(scope, Vec::new()));
        };
        let permissions = self.effective(&membership, workspace).await?;
        Ok(grant(scope, permissions))
    }

    /// Every workspace grant of `principal` with a permission.
    async fn every_grant(&self, principal: &Principal) -> Result<Vec<WorkspaceGrant>, ApiError> {
        let Some(membership) = self.membership(principal).await? else {
            return Ok(Vec::new());
        };
        let tenant = principal.tenant_id.clone();
        let held = self.catalog.list(&tenant, true).await?;
        let scope = |workspace_id| StorageScope {
            tenant_id: tenant.clone(),
            workspace_id,
        };
        if membership.admin {
            return Ok(held
                .into_iter()
                .map(|workspace| grant(scope(workspace.id), ALL.to_vec()))
                .collect());
        }
        let mut requests = stream::iter(held)
            .map(|workspace| {
                let membership = &membership;
                async move {
                    let permissions = self.effective(membership, workspace.id).await?;
                    Ok::<_, ApiError>((workspace.id, permissions))
                }
            })
            .buffered(4);
        let mut grants = Vec::new();
        while let Some(result) = requests.next().await {
            let (workspace, permissions) = result?;
            if !permissions.is_empty() {
                grants.push(grant(scope(workspace), permissions));
            }
        }
        Ok(grants)
    }

    /// The workspace role assignments of `membership`: each resource's external id and the
    /// role's slug; of one workspace only when `only` names it.
    async fn assigned(
        &self,
        membership: &Membership,
        only: Option<&str>,
    ) -> Result<Vec<(String, String)>, ApiError> {
        let client = &*self.client;
        let id = membership.id.clone();
        let only = only.map(str::to_owned);
        let found = every_page(|after| {
            let (id, only) = (id.clone(), only.clone());
            async move {
                let params = ListRoleAssignmentsParams {
                    after,
                    limit: Some(PAGE),
                    resource_external_id: only,
                    resource_type_slug: Some(RESOURCE_TYPE.to_owned()),
                    ..ListRoleAssignmentsParams::default()
                };
                client
                    .authorization()
                    .list_role_assignments(&id, params)
                    .await
                    .map(|page| {
                        let pairs = page
                            .data
                            .into_iter()
                            .filter(|assignment| {
                                assignment.resource.resource_type_slug == RESOURCE_TYPE
                            })
                            .map(|assignment| {
                                (assignment.resource.external_id, assignment.role.slug)
                            });
                        (pairs.collect(), page.list_metadata.after)
                    })
            }
        })
        .await;
        match found {
            Ok(found) => Ok(found),
            Err(Unlisted::Absent) => Ok(Vec::new()),
            Err(Unlisted::Failed(error)) => Err(error),
        }
    }

    /// Record `principal` as the creator of `workspace`: its resource and the creator's role.
    async fn record_creator(
        &self,
        principal: &Principal,
        workspace: WorkspaceId,
    ) -> Result<WorkspaceGrant, ApiError> {
        let Some(membership) = self.membership(principal).await? else {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                "Sign in through a membership of this organization to create a workspace",
            ));
        };
        let organization = principal.tenant_id.as_str().to_owned();
        let external = workspace.0.to_string();
        let authorization = self.client.authorization();
        match authorization
            .get_resource_by_external_id(&organization, RESOURCE_TYPE, &external)
            .await
        {
            Ok(resource) if resource.organization_id == organization => {}
            Ok(_) => {
                return Err(ApiError::new(
                    ErrorCode::Internal,
                    "The identity provider holds this workspace's resource in another organization",
                ));
            }
            Err(error) if error.is_not_found() => {
                let body = CreateResourceParamsBody::new(
                    external.clone(),
                    external.clone(),
                    RESOURCE_TYPE,
                    organization.clone(),
                );
                authorization
                    .create_resource(CreateResourceParams::new(body))
                    .await
                    .map_err(|error| provider(&error))?;
            }
            Err(error) => return Err(provider(&error)),
        }
        let assigned = self.assigned(&membership, Some(&external)).await?;
        if !assigned
            .iter()
            .any(|(resource, role)| *resource == external && role == CREATOR_ROLE)
        {
            let body = AssignRoleParamsBody::new(
                CREATOR_ROLE,
                ResourceTarget::ByExternalId {
                    resource_external_id: external.clone(),
                    resource_type_slug: RESOURCE_TYPE.to_owned(),
                },
            );
            authorization
                .assign_role(&membership.id, AssignRoleParams::new(body))
                .await
                .map_err(|error| provider(&error))?;
        }
        Ok(grant(
            StorageScope {
                tenant_id: principal.tenant_id.clone(),
                workspace_id: workspace,
            },
            ALL.to_vec(),
        ))
    }

    /// The users holding `write` on the workspace of `scope`, and every organization admin.
    async fn writers(&self, scope: &StorageScope) -> Result<Vec<String>, ApiError> {
        if !self.holds(scope).await? {
            return Ok(Vec::new());
        }
        let client = &*self.client;
        let organization = scope.tenant_id.as_str().to_owned();
        let external = scope.workspace_id.0.to_string();
        let write_slug = format!("{PERMISSION_PREFIX}{}", wire_name(Permission::Write));
        let holders = every_page(|after| {
            let (organization, external, slug) =
                (organization.clone(), external.clone(), write_slug.clone());
            async move {
                let params = ListMembershipsForResourceByExternalIdParams {
                    after,
                    limit: Some(PAGE),
                    assignment: Some(AuthorizationAssignment::Indirect),
                    ..ListMembershipsForResourceByExternalIdParams::new(slug)
                };
                client
                    .authorization()
                    .list_memberships_for_resource_by_external_id(
                        &organization,
                        RESOURCE_TYPE,
                        &external,
                        params,
                    )
                    .await
                    .map(|page| {
                        let users = page
                            .data
                            .into_iter()
                            .filter(|member| {
                                member.status
                                    == UserOrganizationMembershipBaseListDataStatus::Active
                            })
                            .map(|member| member.user_id);
                        (users.collect(), page.list_metadata.after)
                    })
            }
        })
        .await;
        let mut editors = match holders {
            Ok(users) => users,
            Err(Unlisted::Absent) => Vec::new(),
            Err(Unlisted::Failed(error)) => return Err(error),
        };
        let admins = every_page(|after| {
            let organization = organization.clone();
            async move {
                let params = ListOrganizationMembershipsParams {
                    after,
                    limit: Some(PAGE),
                    organization_id: Some(organization),
                    ..ListOrganizationMembershipsParams::default()
                };
                client
                    .organization_membership()
                    .list_organization_memberships(params)
                    .await
                    .map(|page| {
                        let users = page
                            .data
                            .into_iter()
                            .filter(|member| {
                                member.status == UserOrganizationMembershipStatus::Active
                                    && std::iter::once(&member.role)
                                        .chain(member.roles.iter())
                                        .any(|role| role.slug == ORGANIZATION_ADMIN)
                            })
                            .map(|member| member.user_id);
                        (users.collect(), page.list_metadata.after)
                    })
            }
        })
        .await;
        match admins {
            Ok(users) => editors.extend(users),
            Err(Unlisted::Absent) => {}
            Err(Unlisted::Failed(error)) => return Err(error),
        }
        Ok(editors)
    }
}

impl AccessControl for WorkosAccess {
    fn authorize<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
        _permission: Permission,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(self.bounded(self.raw_grant(principal, workspace)))
    }

    fn authorize_tenant<'a>(
        &'a self,
        principal: &'a Principal,
        _permission: Permission,
    ) -> PortFuture<'a, TenantGrant> {
        Box::pin(self.bounded(async move {
            let permissions = match self.membership(principal).await? {
                Some(membership) if membership.admin => ALL.to_vec(),
                Some(_) | None => Vec::new(),
            };
            Ok(TenantGrant {
                tenant_id: principal.tenant_id.clone(),
                permissions,
            })
        }))
    }

    fn grants<'a>(&'a self, principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        Box::pin(self.bounded(self.every_grant(principal)))
    }

    fn grant_creator<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(self.bounded(self.record_creator(principal, workspace)))
    }

    fn editors<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Vec<String>> {
        Box::pin(self.bounded(self.writers(scope)))
    }
}

/// Every page of a WorkOS listing, at most [`MAX_PAGES`]: `fetch` is given each page's `after`
/// cursor and answers the page's entries and the next cursor.
async fn every_page<T, F, Fut>(mut fetch: F) -> Result<Vec<T>, Unlisted>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<(Vec<T>, Option<String>), workos::Error>>,
{
    let mut all = Vec::new();
    let mut after = None;
    for _ in 0..MAX_PAGES {
        match fetch(after).await {
            Ok((entries, next)) => {
                all.extend(entries);
                match next {
                    Some(cursor) => after = Some(cursor),
                    None => return Ok(all),
                }
            }
            Err(error) if error.is_not_found() => return Err(Unlisted::Absent),
            Err(error) => return Err(Unlisted::Failed(provider(&error))),
        }
    }
    Err(Unlisted::Failed(ApiError::new(
        ErrorCode::Unavailable,
        format!("The identity provider listed more than {MAX_PAGES} pages"),
    )))
}

/// The permissions `slugs` name: each `workspace:<wire name>` once, in the contract's order;
/// every other slug ignored.
fn permissions_of(slugs: &[String]) -> Vec<Permission> {
    ALL.into_iter()
        .filter(|permission| {
            let slug = format!("{PERMISSION_PREFIX}{}", wire_name(*permission));
            slugs.contains(&slug)
        })
        .collect()
}

/// A permission's wire name, its serde spelling.
fn wire_name(permission: Permission) -> String {
    serde_json::to_value(permission)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn grant(scope: StorageScope, permissions: Vec<Permission>) -> WorkspaceGrant {
    WorkspaceGrant { scope, permissions }
}

/// A WorkOS failure as this service answers it, quoting nothing WorkOS said.
fn provider(error: &workos::Error) -> ApiError {
    match error.status() {
        Some(401 | 403) => ApiError::new(
            ErrorCode::Unavailable,
            "The identity provider refused this server's credentials",
        ),
        Some(429 | 500..=599) | None => ApiError::new(
            ErrorCode::Unavailable,
            "The identity provider is unavailable",
        ),
        Some(_) => ApiError::new(
            ErrorCode::Internal,
            "The identity provider answered unexpectedly",
        ),
    }
}
