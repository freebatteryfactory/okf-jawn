//! The workspace catalog (`WorkspaceCatalog`): names in SQLite, content in one Git repository
//! per workspace.
//!
//! `create` names the workspace `derive_workspace_id(mutation_id)`, inserts the row (unique on
//! `mutation_id`) and makes the repository and its initial commit in one SQLite transaction,
//! each Git step skipped when a resumed attempt finds it done. A crash after the repository was
//! written and before the row was committed is therefore retried under the same identity: the
//! retry finds that repository and adopts it, never allocating a second identity or leaving an
//! unreachable repository. `update`, `archive` and `unarchive` record their
//! result under their mutation id (primary key) in the same transaction as the change, so a
//! repeated id changes nothing and returns the first result. The catalog never filters and
//! returns `Workspace::permissions` empty.

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{MutationId, TenantId, WorkspaceId};
use okf_jawn_contract::workspace::Workspace;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    NewWorkspace, Provenance, StorageScope, WorkspaceArchive, WorkspaceCatalog, WorkspaceUpdate,
    derive_workspace_id,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::db::{Db, conflict, json, not_found, scope_key, sql};
use crate::git::repo::{
    HEAD_REF, Repositories, durable, git, head, message_with_trailer, reference_target,
    revision_of, signature, staged_tree, sync_directories,
};
use crate::seam;

/// `WorkspaceCatalog` over the records database and the repositories.
#[derive(Debug, Clone)]
pub struct GitCatalog {
    db: Db,
    repositories: Repositories,
}

/// What a catalog change does to the row.
#[derive(Debug, Clone, Copy)]
enum Change {
    Archive,
    Unarchive,
}

/// name, description, created at, archived at: one catalog row.
type Row = (String, String, String, Option<String>);

impl GitCatalog {
    pub(crate) const fn new(db: Db, repositories: Repositories) -> Self {
        Self { db, repositories }
    }

    async fn change(
        &self,
        scope: &StorageScope,
        mutation_id: MutationId,
        expected: Option<okf_jawn_contract::identity::Revision>,
        update: Option<(String, String)>,
        change: Option<Change>,
    ) -> Result<Workspace, ApiError> {
        let scope = scope.clone();
        let repositories = self.repositories.clone();
        self.db
            .transaction(move |transaction| {
                if let Some(prior) = prior(transaction, mutation_id)? {
                    return Ok(prior);
                }
                let mut workspace = read(transaction, &repositories, &scope)?;
                if let Some(expected) = expected
                    && workspace.head != expected
                {
                    return Err(conflict("the workspace head moved since it was shown"));
                }
                if let Some((name, description)) = update {
                    workspace.name = name;
                    workspace.description = description;
                }
                match change {
                    Some(Change::Archive) if workspace.archived_at.is_none() => {
                        workspace.archived_at = Some(seam::now()?);
                    }
                    Some(Change::Unarchive) => workspace.archived_at = None,
                    _ => {}
                }
                let (tenant, id) = scope_key(&scope);
                transaction
                    .execute(
                        "UPDATE workspaces SET name = ?1, description = ?2, archived_at = ?3
                         WHERE tenant_id = ?4 AND workspace_id = ?5",
                        params![
                            workspace.name,
                            workspace.description,
                            workspace
                                .archived_at
                                .as_ref()
                                .map(|at| at.as_str().to_owned()),
                            tenant,
                            id
                        ],
                    )
                    .map_err(|error| sql(&error))?;
                transaction
                    .execute(
                        "INSERT INTO workspace_mutations (mutation_id, tenant_id, workspace_id,
                             action, result)
                         VALUES (?1, ?2, ?3, 'change', ?4)",
                        params![
                            mutation_id.0.to_string(),
                            tenant,
                            id,
                            serde_json::to_string(&workspace).map_err(|error| json(&error))?
                        ],
                    )
                    .map_err(|error| sql(&error))?;
                Ok(workspace)
            })
            .await
    }
}

impl WorkspaceCatalog for GitCatalog {
    fn list<'a>(
        &'a self,
        tenant: &'a TenantId,
        include_archived: bool,
    ) -> PortFuture<'a, Vec<Workspace>> {
        let tenant = tenant.clone();
        let repositories = self.repositories.clone();
        Box::pin(self.db.call(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT workspace_id FROM workspaces
                     WHERE tenant_id = ?1 AND (?2 OR archived_at IS NULL) ORDER BY rowid",
                )
                .map_err(|error| sql(&error))?;
            let ids = statement
                .query_map(params![tenant.as_str(), include_archived], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|error| sql(&error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| sql(&error))?;
            ids.iter()
                .map(|id| {
                    let scope = StorageScope {
                        tenant_id: tenant.clone(),
                        workspace_id: from_text!(id.as_str())?,
                    };
                    read(connection, &repositories, &scope)
                })
                .collect()
        }))
    }

    fn create<'a>(
        &'a self,
        tenant: &'a TenantId,
        mutation_id: MutationId,
        workspace: NewWorkspace,
    ) -> PortFuture<'a, Workspace> {
        let tenant = tenant.clone();
        let repositories = self.repositories.clone();
        Box::pin(self.db.transaction(move |transaction| {
            let id: WorkspaceId = derive_workspace_id(mutation_id);
            transaction
                .execute(
                    "INSERT INTO workspaces (tenant_id, workspace_id, mutation_id, name,
                         description, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (mutation_id) DO NOTHING",
                    params![
                        tenant.as_str(),
                        id.0.to_string(),
                        mutation_id.0.to_string(),
                        workspace.name,
                        workspace.description,
                        seam::now()?.as_str()
                    ],
                )
                .map_err(|error| sql(&error))?;
            let stored: String = transaction
                .query_row(
                    "SELECT workspace_id FROM workspaces WHERE mutation_id = ?1",
                    [mutation_id.0.to_string()],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            let scope = StorageScope {
                tenant_id: tenant.clone(),
                workspace_id: from_text!(stored.as_str())?,
            };
            initialize(&repositories, &scope, mutation_id, &workspace.creator)?;
            read(transaction, &repositories, &scope)
        }))
    }

    fn open<'a>(&'a self, scope: &'a StorageScope) -> PortFuture<'a, Workspace> {
        let scope = scope.clone();
        let repositories = self.repositories.clone();
        Box::pin(
            self.db
                .call(move |connection| read(connection, &repositories, &scope)),
        )
    }

    fn update<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        update: WorkspaceUpdate,
    ) -> PortFuture<'a, Workspace> {
        Box::pin(self.change(
            scope,
            mutation_id,
            Some(update.expected_head),
            Some((update.name, update.description)),
            None,
        ))
    }

    fn archive<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        archive: WorkspaceArchive,
    ) -> PortFuture<'a, Workspace> {
        Box::pin(self.change(
            scope,
            mutation_id,
            Some(archive.expected_head),
            None,
            Some(Change::Archive),
        ))
    }

    fn unarchive<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        _author: Provenance,
    ) -> PortFuture<'a, Workspace> {
        Box::pin(self.change(scope, mutation_id, None, None, Some(Change::Unarchive)))
    }
}

/// Make the repository and its empty initial commit unless a resumed attempt already did.
fn initialize(
    repositories: &Repositories,
    scope: &StorageScope,
    mutation_id: MutationId,
    creator: &Provenance,
) -> Result<(), ApiError> {
    let path = repositories.path(scope);
    // The path is derived from this creation's mutation id, so nothing else writes there. A
    // directory a crash left that is not a repository at all is replaced; any other failure
    // to open it is reported, never answered by deleting it.
    let repository = if path.exists() {
        match git2::Repository::open_bare(&path) {
            Ok(_) => durable(&path)?,
            Err(error) if error.code() == git2::ErrorCode::NotFound => {
                repositories.create(scope)?
            }
            Err(error) => return Err(git(&error)),
        }
    } else {
        repositories.create(scope)?
    };
    if reference_target(&repository, HEAD_REF)?.is_some() {
        return Ok(());
    }
    let staging = repositories.stage(scope, mutation_id)?;
    let tree = staged_tree(&repository, &staging.path)?;
    let tree = repository.find_tree(tree).map_err(|error| git(&error))?;
    let who = signature(creator)?;
    let commit = repository
        .commit(
            None,
            &who,
            &who,
            &message_with_trailer("Create the workspace", mutation_id),
            &tree,
            &[],
        )
        .map_err(|error| git(&error))?;
    sync_directories(&repository, &["objects"])?;
    repository
        .reference(HEAD_REF, commit, false, "okf-jawn create")
        .map_err(|error| git(&error))?;
    repository.set_head(HEAD_REF).map_err(|error| git(&error))
}

fn read(
    connection: &Connection,
    repositories: &Repositories,
    scope: &StorageScope,
) -> Result<Workspace, ApiError> {
    let (tenant, id) = scope_key(scope);
    let row: Option<Row> = connection
        .query_row(
            "SELECT name, description, created_at, archived_at FROM workspaces
             WHERE tenant_id = ?1 AND workspace_id = ?2",
            params![tenant, id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let (name, description, created_at, archived_at) = row.ok_or_else(|| not_found("workspace"))?;
    let repository = repositories.open(scope).map_err(|error| {
        if error.code == ErrorCode::NotFound {
            ApiError::new(
                ErrorCode::Unavailable,
                "the workspace's repository is missing",
            )
        } else {
            error
        }
    })?;
    Ok(Workspace {
        id: scope.workspace_id,
        name,
        description,
        head: revision_of(head(&repository)?)?,
        created_at: from_text!(created_at.as_str())?,
        archived_at: archived_at.map(|at| from_text!(at.as_str())).transpose()?,
        permissions: Vec::new(),
    })
}

fn prior(
    transaction: &Transaction<'_>,
    mutation_id: MutationId,
) -> Result<Option<Workspace>, ApiError> {
    let result: Option<String> = transaction
        .query_row(
            "SELECT result FROM workspace_mutations WHERE mutation_id = ?1",
            [mutation_id.0.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    result
        .map(|result| serde_json::from_str(&result).map_err(|error| json(&error)))
        .transpose()
}
