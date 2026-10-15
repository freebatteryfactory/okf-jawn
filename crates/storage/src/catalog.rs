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
//!
//! An archived workspace runs no job that changes its content (contract ruling 25), held by two
//! guards over the one records database: `archive` is refused with `Conflict` while an import,
//! redigest or restore job of the workspace is queued or running, checked in the archive's own
//! transaction; and `RecordStore::create_job` refuses to insert such a job for an archived
//! workspace in the insert's transaction (`refuse_content_job_while_archived`). Both run in
//! immediate transactions, so an archive and such an insert racing are ordered by the database
//! and exactly one of them is refused. A job already queued or running holds the archive off;
//! a repeated archive of an archived workspace finds none, since none could be inserted. A
//! failed job is neither queued nor running; queueing it again is `retry_job`, which core's
//! dispatch refuses in an archived workspace.

use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{MutationId, TenantId, WorkspaceId};
use okf_jawn_contract::import::{Job, JobKind};
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
use crate::purge::guard;
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

/// The kinds of job that change a workspace's content: an archived workspace accepts none, and
/// a workspace with one queued or running is not archived.
pub(crate) const CONTENT_JOBS: [JobKind; 3] = [
    JobKind::Import,
    JobKind::Redigest,
    JobKind::RestoreWorkspace,
];

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
                let (tenant, id) = scope_key(&scope);
                guard::refuse(transaction, &tenant, &id)?;
                if matches!(change, Some(Change::Archive)) {
                    refuse_archive_while_content_jobs(transaction, &tenant, &id)?;
                }
                let mut workspace = read(transaction, &repositories, &scope)?;
                if let Some(expected) = expected
                    && workspace.head != expected
                {
                    // Ruling 5: the refusal names the head; the caller names its request's field.
                    return Err(conflict("the workspace head moved since it was shown")
                        .with_detail(ErrorDetail::StaleBase {
                            head: workspace.head.clone(),
                        }));
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
            refuse_purged(transaction, &tenant, id)?;
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

/// Refuse to create a workspace whose identity a purge names: a retried creation must never
/// bring a purged workspace back under its old identity.
fn refuse_purged(
    connection: &Connection,
    tenant: &TenantId,
    workspace: WorkspaceId,
) -> Result<(), ApiError> {
    let purged: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM purges WHERE tenant_id = ?1 AND target_key = ?2",
            params![tenant.as_str(), format!("workspace/{}", workspace.0)],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    if purged.is_some() {
        return Err(conflict(
            "this workspace identity was purged; it is never created again",
        ));
    }
    Ok(())
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

/// Refuse to archive a workspace while an import, redigest or restore job of it is queued or
/// running: such a job would change an archived workspace's content. Read in the archive's own
/// transaction, from each unfinished job's stored record by its `kind`.
fn refuse_archive_while_content_jobs(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
) -> Result<(), ApiError> {
    let mut statement = connection
        .prepare(
            "SELECT record FROM jobs
             WHERE tenant_id = ?1 AND workspace_id = ?2 AND state IN ('queued', 'running')",
        )
        .map_err(|error| sql(&error))?;
    let records = statement
        .query_map(params![tenant, workspace], |row| row.get::<_, String>(0))
        .map_err(|error| sql(&error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    for record in records {
        let job: Job = serde_json::from_str(&record).map_err(|error| json(&error))?;
        if CONTENT_JOBS.contains(&job.kind) {
            return Err(conflict(format!(
                "job {} of this workspace changes its content and is queued or running; the \
                 workspace is archived once it ends or is cancelled",
                job.id.0
            )));
        }
    }
    Ok(())
}

/// Refuse to insert an import, redigest or restore job (`CONTENT_JOBS`) for an archived
/// workspace, with `Conflict` and `ErrorDetail::WorkspaceArchived`. Run in the insert's own
/// transaction (`RecordStore::create_job`). Any other kind, and a workspace that is not
/// archived or that the catalog does not hold, is let through.
pub(crate) fn refuse_content_job_while_archived(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
    kind: JobKind,
) -> Result<(), ApiError> {
    if !CONTENT_JOBS.contains(&kind) {
        return Ok(());
    }
    let archived: Option<Option<String>> = connection
        .query_row(
            "SELECT archived_at FROM workspaces WHERE tenant_id = ?1 AND workspace_id = ?2",
            params![tenant, workspace],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    match archived.flatten() {
        Some(archived_at) => Err(conflict(
            "the workspace is archived; no import, redigest or restore runs in it",
        )
        .with_detail(ErrorDetail::WorkspaceArchived {
            archived_at: from_text!(archived_at.as_str())?,
        })),
        None => Ok(()),
    }
}
