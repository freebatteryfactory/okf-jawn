//! Purge (`Purger`): removing a target from every store the application manages.
//!
//! SPEC section 8: "Purge ... removes the original bytes, derivatives, index entries, historical
//! content and installation-managed backups of an item or a workspace, and marks the revisions,
//! citations and Views that referred to them as invalidated. Purge never claims to erase
//! exports, clones or backups outside the application's control." This module only removes
//! what this data directory holds; the `PurgeReport` counts what it removed and kept.
//!
//! A workspace purge runs in steps recorded in `purge_progress`, each safe to repeat, so a purge
//! that stopped resumes by repeating the call (it is never cancelled: a half-removed target is
//! worse than either end):
//!
//! 1. **Plan**, before anything is removed: the objects the workspace references (its whole Git
//!    history on every reference, the conversion records its cards name, its records), its
//!    workspace archives (recorded as artifacts or only written), its uploads, the
//!    pre-migration backups that hold it, and the
//!    counts of the report. In the same transaction every commit of the workspace is written to
//!    the revision map as invalidated by this purge with no replacement. The plan holds
//!    identities and counts, never content.
//! 2. **Backups**: every pre-migration copy that holds the workspace is rewritten without it or
//!    deleted, and its archive in every pre-migration backup set is deleted (`copies`).
//! 3. **Records**: its rows are removed and what cites it is scrubbed (`rows`); its upload files
//!    are deleted. Its retained exports and archives stop being downloadable here.
//! 4. **Index**: its index entries are removed and the full-text index is merged, so no removed
//!    term stays in an index segment.
//! 5. **Repository**: its repository is deleted under the workspace's write lock; then the rows
//!    step runs again, for a row a write in flight added after step 3.
//! 6. **Objects**: what every surviving workspace and tenant record references is read now, and
//!    each planned object nobody else references is deleted; the others are counted as kept.
//! 7. **Vacuum**: both databases are rebuilt without free pages and their write-ahead logs folded
//!    back, so removed rows leave no bytes behind; then the report is recorded.
//!
//! The purge record itself (`RecordStore::create_purge`), its tenant job and the revision map
//! outlive the workspace: they hold identities only. Installation archives hold no workspace
//! content (Stage 1b design section 8) and are left as they are.
//!
//! Known limit: an object first stored by work running in the workspace after step 1 is not in
//! the plan and stays in the object store, unreferenced; and an identical object that another
//! workspace starts to reference between step 6's read and its delete is deleted.

use std::collections::BTreeSet;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{
    Digest, ItemId, MutationId, PurgeId, TenantId, UploadId, WorkspaceId,
};
use okf_jawn_contract::purge::PurgeReport;
use okf_jawn_contract::workspace::ArtifactKind;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{Purger, StorageScope};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::data::io_error;
use crate::db::{internal, json, scope_key, sql};
use crate::storage::Storage;
use references::Rows;

/// `Purger` over every store of the opened data directory.
#[derive(Debug, Clone)]
pub struct StoragePurger {
    storage: Storage,
}

/// What a workspace purge removes, written once before anything is removed.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspacePlan {
    /// The purged workspace.
    workspace_id: WorkspaceId,
    /// Held objects the workspace referenced when the plan was made.
    objects: BTreeSet<Digest>,
    /// Its workspace archives: an upload slot elsewhere holding one is no surviving reference.
    archives: BTreeSet<Digest>,
    /// Its uploads, whose files lie outside the object store.
    uploads: Vec<UploadId>,
    /// Pre-migration records copies to rewrite, relative to their directory.
    rewrite: Vec<String>,
    /// Pre-migration copies and archives to delete, relative to their directory.
    remove: Vec<String>,
    /// The report; the object counts are filled in by the objects step.
    report: PurgeReport,
}

/// Where a purge stands.
enum Progress {
    /// The last step completed, and the plan.
    Running(i64, Box<WorkspacePlan>),
    /// Done, with its report.
    Done(PurgeReport),
}

/// The completed-step numbers recorded in `purge_progress.step`.
const PLANNED: i64 = 1;
const BACKUPS: i64 = 2;
const RECORDS: i64 = 3;
const INDEX: i64 = 4;
const REPOSITORY: i64 = 5;
const OBJECTS: i64 = 6;
const DONE: i64 = 7;

impl StoragePurger {
    pub(crate) const fn new(storage: Storage) -> Self {
        Self { storage }
    }

    /// See `Purger::purge_workspace` and the module header.
    async fn workspace(
        &self,
        tenant: &TenantId,
        mutation: MutationId,
        purge: PurgeId,
        workspace: WorkspaceId,
    ) -> Result<PurgeReport, ApiError> {
        let scope = StorageScope {
            tenant_id: tenant.clone(),
            workspace_id: workspace,
        };
        let (mut step, mut plan) = match self.progress(tenant, purge).await? {
            Some(Progress::Done(report)) => return Ok(report),
            Some(Progress::Running(step, plan)) => (step, *plan),
            None => (PLANNED, self.plan(&scope, mutation, purge).await?),
        };
        if plan.workspace_id != workspace {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "this purge identity already removes another workspace",
            ));
        }
        if step < BACKUPS {
            self.backups(&scope, purge, &plan).await?;
            step = self.advance(purge, BACKUPS, None).await?;
        }
        if step < RECORDS {
            self.records(&scope, purge, &plan.uploads).await?;
            step = self.advance(purge, RECORDS, None).await?;
        }
        if step < INDEX {
            self.index(&scope).await?;
            step = self.advance(purge, INDEX, None).await?;
        }
        if step < REPOSITORY {
            let repositories = self.storage.repositories.clone();
            let removed = scope.clone();
            blocking(move || repositories.remove(&removed)).await?;
            self.records(&scope, purge, &plan.uploads).await?;
            step = self.advance(purge, REPOSITORY, None).await?;
        }
        if step < OBJECTS {
            let (removed, kept) = self.objects(&scope, &plan).await?;
            plan.report.objects_removed = removed.to_string();
            plan.report.objects_kept_shared = kept.to_string();
            step = self.advance(purge, OBJECTS, Some(&plan)).await?;
        }
        if step < DONE {
            for db in [&self.storage.records, &self.storage.index] {
                db.call(|connection| copies::vacuum(connection)).await?;
            }
            let report = serde_json::to_string(&plan.report).map_err(|error| json(&error))?;
            self.storage
                .records
                .transaction(move |transaction| {
                    transaction
                        .execute(
                            "UPDATE purge_progress SET step = ?1, report = ?2 WHERE purge_id = ?3",
                            params![DONE, report, purge.0.to_string()],
                        )
                        .map_err(|error| sql(&error))
                })
                .await?;
        }
        Ok(plan.report)
    }

    async fn progress(
        &self,
        tenant: &TenantId,
        purge: PurgeId,
    ) -> Result<Option<Progress>, ApiError> {
        let tenant = tenant.as_str().to_owned();
        self.storage
            .records
            .call(move |connection| read_progress(connection, &tenant, purge))
            .await
    }

    /// Step 1: read what the workspace references and record the plan with the revision map.
    async fn plan(
        &self,
        scope: &StorageScope,
        mutation: MutationId,
        purge: PurgeId,
    ) -> Result<WorkspacePlan, ApiError> {
        let (tenant, workspace) = scope_key(scope);
        let (target, archives, uploads) = {
            let (tenant, workspace) = (tenant.clone(), workspace.clone());
            self.storage
                .records
                .call(move |connection| {
                    let target = references::records(
                        connection,
                        &tenant,
                        &workspace,
                        Rows::Target,
                        &BTreeSet::new(),
                    )?;
                    let archives = archives(connection, &tenant, &workspace)?;
                    let uploads = uploads(connection, &tenant, &workspace)?;
                    Ok((target, archives, uploads))
                })
                .await?
        };
        let (history, objects, copy_plan) = {
            let repositories = self.storage.repositories.clone();
            let blobs = self.storage.blobs.clone();
            let pre_migration = self.storage.data.pre_migration_path();
            let scope = scope.clone();
            let (tenant, workspace) = (tenant.clone(), workspace.clone());
            blocking(move || {
                let history = if repositories.path(&scope).exists() {
                    references::history(&repositories.open(&scope)?)?
                } else {
                    references::History::default()
                };
                let mut objects = target;
                objects.extend(history.objects.iter().cloned());
                references::follow_records(&blobs, &scope.tenant_id, &mut objects)?;
                let mut held = BTreeSet::new();
                for digest in objects {
                    if blobs.local_path(&scope.tenant_id, &digest)?.is_file() {
                        held.insert(digest);
                    }
                }
                let copy_plan = copies::plan(&pre_migration, &tenant, &workspace)?;
                Ok((history.revisions, held, copy_plan))
            })
            .await?
        };
        let workspace_id = scope.workspace_id;
        self.storage
            .records
            .transaction(move |transaction| {
                if let Some(Progress::Running(_, plan)) =
                    read_progress(transaction, &tenant, purge)?
                {
                    return Ok(*plan);
                }
                let plan = WorkspacePlan {
                    workspace_id,
                    report: report_at_plan(
                        transaction,
                        &tenant,
                        &workspace,
                        &copy_plan,
                        archives.len(),
                        history.len(),
                    )?,
                    objects,
                    archives,
                    uploads,
                    rewrite: copy_plan.rewrite,
                    remove: copy_plan.remove,
                };
                invalidate_revisions(transaction, &tenant, &workspace, purge, &history)?;
                transaction
                    .execute(
                        "INSERT INTO purge_progress (purge_id, tenant_id, mutation_id, plan, step)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            purge.0.to_string(),
                            tenant,
                            mutation.0.to_string(),
                            serde_json::to_string(&plan).map_err(|error| json(&error))?,
                            PLANNED
                        ],
                    )
                    .map_err(|error| sql(&error))?;
                Ok(plan)
            })
            .await
    }

    /// Step 2: rewrite or delete every pre-migration backup that holds the workspace.
    async fn backups(
        &self,
        scope: &StorageScope,
        purge: PurgeId,
        plan: &WorkspacePlan,
    ) -> Result<(), ApiError> {
        let directory = self.storage.data.pre_migration_path();
        let (tenant, workspace) = scope_key(scope);
        let rewrite = plan.rewrite.clone();
        let remove = plan.remove.clone();
        blocking(move || {
            for name in &rewrite {
                copies::rewrite(&directory.join(name), &tenant, &workspace, purge)?;
            }
            for name in &remove {
                copies::remove(&directory.join(name))?;
            }
            Ok(())
        })
        .await
    }

    /// Steps 3 and 5: remove the workspace's rows and upload files.
    async fn records(
        &self,
        scope: &StorageScope,
        purge: PurgeId,
        uploads: &[UploadId],
    ) -> Result<(), ApiError> {
        let (tenant, workspace) = scope_key(scope);
        self.storage
            .records
            .transaction(move |transaction| rows::remove(transaction, &tenant, &workspace, purge))
            .await?;
        let directory = self.storage.data.root().join("uploads");
        let uploads = uploads.to_vec();
        blocking(move || {
            for upload in uploads {
                for suffix in ["part", "incoming"] {
                    let path = directory.join(format!("{}.{suffix}", upload.0));
                    match std::fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(io_error("remove", &path, &error)),
                    }
                }
            }
            Ok(())
        })
        .await
    }

    /// Step 4: remove the workspace's index entries and merge the full-text index.
    async fn index(&self, scope: &StorageScope) -> Result<(), ApiError> {
        let (tenant, workspace) = scope_key(scope);
        self.storage
            .index
            .transaction(move |transaction| {
                crate::search::discard(transaction, &tenant, &workspace)?;
                transaction
                    .execute(
                        "INSERT INTO index_text (index_text) VALUES ('optimize')",
                        [],
                    )
                    .map(|_| ())
                    .map_err(|error| sql(&error))
            })
            .await
    }

    /// Step 6: delete each planned object no survivor references; (removed, kept).
    async fn objects(
        &self,
        scope: &StorageScope,
        plan: &WorkspacePlan,
    ) -> Result<(usize, usize), ApiError> {
        let (tenant, workspace) = scope_key(scope);
        let archives = plan.archives.clone();
        let mut surviving = self
            .storage
            .records
            .call(move |connection| {
                references::records(connection, &tenant, &workspace, Rows::Survivors, &archives)
            })
            .await?;
        let repositories = self.storage.repositories.clone();
        let blobs = self.storage.blobs.clone();
        let scope = scope.clone();
        let planned = plan.objects.clone();
        blocking(move || {
            let mut found = BTreeSet::new();
            for path in repositories.others(&scope.tenant_id, &scope)? {
                let repository = git2::Repository::open_bare(&path)
                    .map_err(|error| crate::git::repo::git(&error))?;
                found.extend(references::history(&repository)?.objects);
            }
            references::follow_records(&blobs, &scope.tenant_id, &mut found)?;
            surviving.extend(found);
            let mut removed = 0_usize;
            let mut kept = 0_usize;
            for digest in &planned {
                if surviving.contains(digest) {
                    kept = kept.saturating_add(1);
                } else {
                    blobs.remove_for(&scope.tenant_id, digest)?;
                    removed = removed.saturating_add(1);
                }
            }
            Ok((removed, kept))
        })
        .await
    }

    /// Record `step` as done (and the plan, when it changed); returns `step`.
    async fn advance(
        &self,
        purge: PurgeId,
        step: i64,
        plan: Option<&WorkspacePlan>,
    ) -> Result<i64, ApiError> {
        let plan = plan
            .map(|plan| serde_json::to_string(plan).map_err(|error| json(&error)))
            .transpose()?;
        self.storage
            .records
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "UPDATE purge_progress SET step = ?1, plan = coalesce(?2, plan)
                         WHERE purge_id = ?3",
                        params![step, plan, purge.0.to_string()],
                    )
                    .map_err(|error| sql(&error))?;
                Ok(step)
            })
            .await
    }
}

impl Purger for StoragePurger {
    fn purge_workspace<'a>(
        &'a self,
        tenant: &'a TenantId,
        mutation_id: MutationId,
        purge: PurgeId,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, PurgeReport> {
        Box::pin(self.workspace(tenant, mutation_id, purge, workspace))
    }

    fn purge_item<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _purge: PurgeId,
        _item: ItemId,
    ) -> PortFuture<'a, PurgeReport> {
        Box::pin(async {
            Err(ApiError::new(
                ErrorCode::NotImplemented,
                "item purge is not built yet; workspace purge is (Stage 1b design section 6)",
            ))
        })
    }
}

/// Map every revision of the workspace, and every revision an earlier item purge of it
/// mapped, to this purge with no replacement: nothing of the workspace survives.
fn invalidate_revisions(
    transaction: &Transaction<'_>,
    tenant: &str,
    workspace: &str,
    purge: PurgeId,
    revisions: &[git2::Oid],
) -> Result<(), ApiError> {
    transaction
        .execute(
            "UPDATE revision_map SET purge_id = ?3, replacement = NULL
             WHERE tenant_id = ?1 AND workspace_id = ?2",
            params![tenant, workspace, purge.0.to_string()],
        )
        .map_err(|error| sql(&error))?;
    for revision in revisions {
        transaction
            .execute(
                "INSERT INTO revision_map (tenant_id, workspace_id, old_revision, purge_id,
                     replacement)
                 VALUES (?1, ?2, ?3, ?4, NULL)
                 ON CONFLICT (tenant_id, workspace_id, old_revision)
                 DO UPDATE SET purge_id = excluded.purge_id, replacement = NULL",
                params![tenant, workspace, revision.to_string(), purge.0.to_string()],
            )
            .map_err(|error| sql(&error))?;
    }
    Ok(())
}

/// The counts a purge knows before it removes anything.
fn report_at_plan(
    transaction: &Transaction<'_>,
    tenant: &str,
    workspace: &str,
    copy_plan: &copies::CopyPlan,
    archives: usize,
    revisions: usize,
) -> Result<PurgeReport, ApiError> {
    let export = variant_text!(&ArtifactKind::Export)?;
    let view_export = variant_text!(&ArtifactKind::ViewExport)?;
    let count = |query: &str, extra: &[&dyn rusqlite::ToSql]| -> Result<u32, ApiError> {
        let mut values: Vec<&dyn rusqlite::ToSql> = vec![&tenant, &workspace];
        values.extend_from_slice(extra);
        let found: i64 = transaction
            .query_row(query, values.as_slice(), |row| row.get(0))
            .map_err(|error| sql(&error))?;
        u32::try_from(found).map_err(|_| internal("a purge count exceeds what a report holds"))
    };
    let number = |value: usize| {
        u32::try_from(value).map_err(|_| internal("a purge count exceeds what a report holds"))
    };
    Ok(PurgeReport {
        objects_removed: "0".to_owned(),
        objects_kept_shared: "0".to_owned(),
        backups_removed: number(archives.saturating_add(copy_plan.remove.len()))?,
        backups_rewritten: number(copy_plan.rewrite.len())?,
        exports_removed: count(
            "SELECT count(*) FROM artifacts WHERE tenant_id = ?1 AND workspace_id = ?2
               AND kind IN (?3, ?4)",
            &[&export, &view_export],
        )?,
        proposals_removed: count(
            "SELECT count(*) FROM proposals WHERE tenant_id = ?1 AND workspace_id = ?2",
            &[],
        )?,
        drafts_removed: count(
            "SELECT count(*) FROM drafts WHERE tenant_id = ?1 AND workspace_id = ?2",
            &[],
        )?,
        revisions_invalidated: number(revisions)?,
        citations_remapped: 0,
        citations_invalidated: rows::citations(transaction, tenant, workspace, None)?,
        views_invalidated: 0,
    })
}

/// The workspace archives of the workspace: every one recorded as an artifact, and every one
/// `Backups::write_workspace_archive` wrote (`backup_archives`), recorded as an artifact or not
/// (the backup handler may have failed, or its job been abandoned, after the write).
fn archives(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
) -> Result<BTreeSet<Digest>, ApiError> {
    let kind = variant_text!(&ArtifactKind::WorkspaceBackup)?;
    let mut statement = connection
        .prepare(
            "SELECT digest FROM artifacts WHERE tenant_id = ?1 AND workspace_id = ?2 AND kind = ?3
             UNION SELECT digest FROM backup_archives WHERE tenant_id = ?1 AND workspace_id = ?2",
        )
        .map_err(|error| sql(&error))?;
    let digests = statement
        .query_map(params![tenant, workspace, kind], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| sql(&error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    digests
        .iter()
        .map(|digest| from_text!(digest.as_str()))
        .collect()
}

/// The uploads of the workspace.
fn uploads(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
) -> Result<Vec<UploadId>, ApiError> {
    let mut statement = connection
        .prepare("SELECT upload_id FROM uploads WHERE tenant_id = ?1 AND workspace_id = ?2")
        .map_err(|error| sql(&error))?;
    let ids = statement
        .query_map(params![tenant, workspace], |row| row.get::<_, String>(0))
        .map_err(|error| sql(&error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    ids.iter().map(|id| from_text!(id.as_str())).collect()
}

fn read_progress(
    connection: &Connection,
    tenant: &str,
    purge: PurgeId,
) -> Result<Option<Progress>, ApiError> {
    let found: Option<(i64, String, Option<String>)> = connection
        .query_row(
            "SELECT step, plan, report FROM purge_progress WHERE purge_id = ?1 AND tenant_id = ?2",
            params![purge.0.to_string(), tenant],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    found
        .map(|(step, plan, report)| match report {
            Some(report) if step >= DONE => serde_json::from_str(&report)
                .map(Progress::Done)
                .map_err(|error| json(&error)),
            _ => serde_json::from_str(&plan)
                .map(|plan| Progress::Running(step, Box::new(plan)))
                .map_err(|error| json(&error)),
        })
        .transpose()
}

/// Run blocking filesystem or Git work on the blocking pool.
async fn blocking<T, F>(work: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ApiError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| internal(format!("a purge task stopped: {error}")))?
}

mod copies;
pub(crate) mod references;
mod rows;
