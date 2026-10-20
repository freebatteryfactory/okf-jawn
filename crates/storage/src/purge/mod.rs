//! Workspace purge over the application's managed stores (SPEC section 8).
//!
//! Durable `purge_progress` steps resume after a stop:
//! 1. Plan from rows and managed-copy identities, without opening or locking the target's Git
//!    repository. Earlier revision mappings are invalidated; the recorded workspace purge
//!    independently invalidates every revision, including a revision no per-commit row names.
//! 2. Rewrite or remove managed copies. Survivor archive rewrites are tenant-level tracked
//!    puts under the recorded purge mutation; moving their references lands only that mutation's
//!    pending puts. Superseded objects are left to the shared sweep, never deleted here.
//! 3. Collect archive/upload identities and remove scoped records transactionally, then unlink
//!    received upload files. Downloads stop being available when their records go.
//! 4. Remove and merge the index.
//! 5. Delete the target repository under its write lock, then repeat records and index passes
//!    to catch writes that held the lock before the purge was recorded.
//! 6. Drain target pins, then run the tenant-serialized reachability sweep (`sweep`). It reads
//!    one durable-root snapshot and every surviving reference/history before streaming canonical
//!    object files. A recorded-purged workspace is never a survivor. Malformed survivor data
//!    stops before deleting any candidate and names only workspace/item/table/object identities.
//!    Durable removal rows survive an interrupted batch; the plan/counts and removal-row clear
//!    land together. The sweep lock/watch remain held through that report transaction.
//! 7. Reach managed backups again, vacuum both databases, and record the completed report.
//!
//! No target history is read, so target corruption cannot prevent its own removal. A survivor's
//! damage must be repaired or that workspace itself recorded for purge before collection can
//! proceed. Retained exports and archives outside this data directory are outside the purge's
//! guarantee. Reports count the bytes this sweep actually removed and every candidate it kept;
//! they do not infer which survivor objects once belonged to the target.
//!
//! Publication is pin-before-retain and durable-pending-before-retain. Workspace records land
//! the objects they actually name; tenant archive records land only their producing mutation's
//! pending puts. A dead write's pending puts and object-store crash staging files have no age
//! reaper. Tenant clean-up uses this collector and durable coalescing job completion; connecting
//! the discard port/handler remains construction work. Item purge remains `NotImplemented`.
use std::collections::BTreeSet;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{
    Digest, ItemId, JobId, MutationId, PurgeId, TenantId, UploadId, WorkspaceId,
};
use okf_jawn_contract::import::CleanUpReport;
use okf_jawn_contract::purge::PurgeReport;
use okf_jawn_contract::workspace::ArtifactKind;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{ObjectInfo, Purger, StorageScope};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::data::io_error;
use crate::db::{internal, json, scope_key, sql};
use crate::puts;
use crate::storage::Storage;
use crate::sweep::{SweepCounts, SweepRun};

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
    /// Its workspace archives: an upload slot elsewhere holding one is no surviving reference.
    archives: BTreeSet<Digest>,
    /// Its uploads, whose files lie outside the object store.
    uploads: Vec<UploadId>,
    /// Pre-migration records copies to rewrite, relative to their directory.
    rewrite: Vec<String>,
    /// Pre-migration copies and archives to delete, relative to their directory.
    remove: Vec<String>,
    /// Other workspaces' archives rewritten without a citation of the workspace: a backup-set
    /// archive by its name relative to the pre-migration directory, an archive in the object
    /// store by the digest it had.
    #[serde(default)]
    rewritten: BTreeSet<String>,
    /// Archive objects a rewrite replaced. Old progress plans retain this field, but only the
    /// shared sweep decides whether those bytes can be removed.
    #[serde(default)]
    superseded: BTreeSet<Digest>,
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

impl WorkspacePlan {
    /// Count the managed backups the plan removes and rewrites, as they stand now.
    fn recount(&mut self) {
        let count = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
        self.report.backups_removed = count(self.archives.len().saturating_add(self.remove.len()));
        self.report.backups_rewritten =
            count(self.rewrite.len().saturating_add(self.rewritten.len()));
    }
}

impl StoragePurger {
    pub(crate) const fn new(storage: Storage) -> Self {
        Self { storage }
    }

    /// One tenant pass. Requests arriving after this start clock are served by another pass
    /// when the real job completion compares its request generations (lifecycle design 3.3).
    async fn clean_up_pass(
        &self,
        tenant: &TenantId,
        job: JobId,
    ) -> Result<CleanUpReport, ApiError> {
        let key = tenant.as_str().to_owned();
        self.storage
            .records
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "UPDATE clean_up_requests SET started=requested WHERE tenant_id=?1",
                        [key],
                    )
                    .map(|_| ())
                    .map_err(|error| sql(&error))
            })
            .await?;
        let counts = crate::sweep::run(
            &self.storage,
            SweepRun {
                tenant: tenant.clone(),
                target: None,
                key: format!("clean_up/{}", job.0),
                excluded: BTreeSet::new(),
            },
        )
        .await?;
        let key = tenant.as_str().to_owned();
        let uploads = self
            .storage
            .records
            .call(move |connection| {
                let mut statement = connection.prepare(
                "SELECT upload_id FROM discarded_uploads WHERE tenant_id=?1 ORDER BY upload_id",
            ).map_err(|error| sql(&error))?;
                let uploads = statement
                    .query_map([key], |row| row.get::<_, String>(0))
                    .map_err(|error| sql(&error))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| sql(&error))?;
                uploads
                    .into_iter()
                    .map(|text| from_text!(text.as_str()))
                    .collect::<Result<Vec<UploadId>, _>>()
            })
            .await?;
        let directory = self.storage.data.root().join("uploads");
        // Keep the sweep's lock/watch with the actual blocking work if its async owner aborts.
        blocking(move || {
            // Capture the whole guard-bearing value, not only its two numeric fields.
            let protection = counts;
            let mut removed = 0_u32;
            for upload in uploads {
                for suffix in ["part", "incoming"] {
                    let path = directory.join(format!("{}.{suffix}", upload.0));
                    match std::fs::remove_file(&path) {
                        Ok(()) => {
                            removed = removed.checked_add(1).ok_or_else(|| {
                                internal("a clean-up file count exceeds its report")
                            })?;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(io_error("remove", &path, &error)),
                    }
                }
            }
            Ok(CleanUpReport {
                objects_removed: protection.removed.to_string(),
                objects_kept: protection.kept.to_string(),
                upload_files_removed: removed,
            })
        })
        .await
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
            self.backups(&scope, purge, &mut plan).await?;
            step = self.advance(purge, BACKUPS, None).await?;
        }
        if step < RECORDS {
            self.records(&scope, purge, &mut plan).await?;
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
            self.records(&scope, purge, &mut plan).await?;
            self.index(&scope).await?;
            step = self.advance(purge, REPOSITORY, None).await?;
        }
        if step < OBJECTS {
            let counts = self.objects(&scope, purge, &plan).await?;
            plan.report.objects_removed = counts.removed.to_string();
            plan.report.objects_kept_shared = counts.kept.to_string();
            step = self.advance(purge, OBJECTS, Some(&plan)).await?;
        }
        if step < DONE {
            // A managed backup written while the purge ran (an upgrade between attempts takes
            // a pre-migration set, which may hold the workspace) is reached before completing.
            self.backups(&scope, purge, &mut plan).await?;
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

    /// Step 1 records only durable rows and copy identities. The target repository is never
    /// opened or locked: the repository step waits for earlier Git writers before its repeat
    /// of the records pass, so that pass catches every archive/upload that landed meanwhile.
    async fn plan(
        &self,
        scope: &StorageScope,
        mutation: MutationId,
        purge: PurgeId,
    ) -> Result<WorkspacePlan, ApiError> {
        let (tenant, workspace) = scope_key(scope);
        let directory = self.storage.data.pre_migration_path();
        let copy_plan = {
            let (tenant, workspace) = (tenant.clone(), workspace.clone());
            blocking(move || copies::plan(&directory, &tenant, &workspace)).await?
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
                let named_revisions = revisions::named(transaction, &tenant, &workspace)?;
                let plan = WorkspacePlan {
                    workspace_id,
                    report: report_at_plan(
                        transaction,
                        &tenant,
                        &workspace,
                        &copy_plan,
                        0,
                        named_revisions,
                    )?,
                    archives: BTreeSet::new(),
                    uploads: Vec::new(),
                    rewrite: copy_plan.rewrite,
                    remove: copy_plan.remove,
                    rewritten: BTreeSet::new(),
                    superseded: BTreeSet::new(),
                };
                invalidate_revisions(transaction, &tenant, &workspace, purge)?;
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
    /// Step 2, and again before the purge completes: rewrite or delete every managed backup that
    /// holds the workspace (`copies`). The pre-migration backups are read again on every pass,
    /// so a set written after the plan (an upgrade after a failed attempt) is reached too.
    async fn backups(
        &self,
        scope: &StorageScope,
        purge: PurgeId,
        plan: &mut WorkspacePlan,
    ) -> Result<(), ApiError> {
        let directory = self.storage.data.pre_migration_path();
        let staging = self
            .storage
            .data
            .staging_path()
            .join(format!("purge-{}", purge.0));
        let (tenant, workspace) = scope_key(scope);
        let (rewrite, remove, rewritten) = {
            let (directory, staging) = (directory.clone(), staging.clone());
            let (tenant, workspace) = (tenant.clone(), workspace.clone());
            let (mut rewrite, mut remove) = (plan.rewrite.clone(), plan.remove.clone());
            blocking(move || {
                let fresh = copies::plan(&directory, &tenant, &workspace)?;
                for name in fresh.rewrite {
                    if !rewrite.contains(&name) {
                        rewrite.push(name);
                    }
                }
                for name in fresh.remove {
                    if !remove.contains(&name) {
                        remove.push(name);
                    }
                }
                for name in &rewrite {
                    copies::rewrite(&directory.join(name), &tenant, &workspace, purge)?;
                }
                for name in &remove {
                    copies::remove(&directory.join(name))?;
                }
                let mut rewritten = Vec::new();
                for other in copies::other_archives(&directory, &tenant, &workspace)? {
                    let archive = directory.join(&other.name);
                    let next = archive.with_extension("rewrite");
                    if copies::rewrite_archive(
                        &archive,
                        &next,
                        &staging,
                        &tenant,
                        &workspace,
                        purge,
                        &other.survivor,
                    )? {
                        std::fs::rename(&next, &archive)
                            .map_err(|error| io_error("replace", &archive, &error))?;
                        rewritten.push(other.name);
                    }
                }
                Ok((rewrite, remove, rewritten))
            })
            .await?
        };
        plan.rewrite = rewrite;
        plan.remove = remove;
        plan.rewritten.extend(rewritten);
        plan.recount();
        self.save_plan(purge, plan).await?;
        self.stored_archives(scope, purge, plan, &staging).await?;
        let _ = std::fs::remove_dir_all(&staging);
        Ok(())
    }

    /// Rewrite every archive of another workspace in the object store whose records still cite
    /// the workspace: a tracked tenant put protects the rewrite until its references and plan
    /// land atomically. Superseded bytes become ordinary candidates of the shared sweep.
    async fn stored_archives(
        &self,
        scope: &StorageScope,
        purge: PurgeId,
        plan: &mut WorkspacePlan,
        staging: &std::path::Path,
    ) -> Result<(), ApiError> {
        let (tenant, workspace) = scope_key(scope);
        let (digests, mutation) = {
            let (tenant, workspace) = (tenant.clone(), workspace.clone());
            self.storage
                .records
                .call(move |connection| {
                    let mutation: String = connection.query_row(
                        "SELECT mutation_id FROM purge_progress WHERE tenant_id=?1 AND purge_id=?2",
                        params![tenant, purge.0.to_string()], |row| row.get(0),
                    ).map_err(|error| sql(&error))?;
                    let mutation =
                        uuid::Uuid::parse_str(&mutation)
                            .map(MutationId)
                            .map_err(|_| {
                                internal("the purge progress holds an invalid mutation identity")
                            })?;
                    Ok((other_archives(connection, &tenant, &workspace)?, mutation))
                })
                .await?
        };
        for (old, owner) in digests {
            let path = self.storage.blobs.local_path(&scope.tenant_id, &old)?;
            if !path.is_file() {
                continue;
            }
            let next = staging.join("rewritten.zip");
            let rewritten = {
                let (next, staging) = (next.clone(), staging.to_path_buf());
                let (tenant, workspace) = (tenant.clone(), workspace.clone());
                let survivor = format!(
                    "workspace {owner}'s archive {} in the object store",
                    old.as_str()
                );
                blocking(move || {
                    copies::rewrite_archive(
                        &path, &next, &staging, &tenant, &workspace, purge, &survivor,
                    )
                })
                .await?
            };
            if !rewritten {
                continue;
            }
            let size = std::fs::metadata(&next)
                .map_err(|error| io_error("read", &next, &error))?
                .len();
            let (new, pinned) = self
                .storage
                .blobs
                .put_tracked(
                    &scope.tenant_id,
                    mutation,
                    crate::blobs::file_reader(Some(next), 0, size),
                    size,
                )
                .await?;
            #[cfg(test)]
            crate::pause::point(&rewrite_stored_point(&self.storage, purge)).await?;
            plan.rewritten.insert(old.as_str().to_owned());
            plan.superseded.insert(old.clone());
            plan.recount();
            let text = serde_json::to_string(&*plan).map_err(|error| json(&error))?;
            let tenant = tenant.clone();
            let moved = old.clone();
            let moved = self
                .storage
                .records
                .transaction(move |transaction| {
                    move_archive(transaction, &tenant, &moved, &new)?;
                    puts::land_tenant(transaction, &tenant, mutation)?;
                    store_plan(transaction, purge, &text)
                })
                .await;
            pinned.end()?;
            moved?;
        }
        Ok(())
    }

    /// Record the plan as it stands now.
    async fn save_plan(&self, purge: PurgeId, plan: &WorkspacePlan) -> Result<(), ApiError> {
        let text = serde_json::to_string(plan).map_err(|error| json(&error))?;
        self.storage
            .records
            .transaction(move |transaction| store_plan(transaction, purge, &text))
            .await
    }

    /// Steps 3 and 5: remove the workspace's rows and upload files.
    async fn records(
        &self,
        scope: &StorageScope,
        purge: PurgeId,
        plan: &mut WorkspacePlan,
    ) -> Result<(), ApiError> {
        let (tenant, workspace) = scope_key(scope);
        let mut next = plan.clone();
        let written = self
            .storage
            .records
            .transaction(move |transaction| {
                next.archives
                    .extend(archives(transaction, &tenant, &workspace)?);
                for upload in uploads(transaction, &tenant, &workspace)? {
                    if !next.uploads.contains(&upload) {
                        next.uploads.push(upload);
                    }
                }
                rows::remove(transaction, &tenant, &workspace, purge)?;
                next.recount();
                store_plan(
                    transaction,
                    purge,
                    &serde_json::to_string(&next).map_err(|error| json(&error))?,
                )?;
                Ok(next)
            })
            .await?;
        *plan = written;
        let directory = self.storage.data.root().join("uploads");
        let uploads = plan.uploads.clone();
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

    /// Step 6 shares the tenant reachability sweep with clean-up. It drains target writers
    /// before marking and never reads the target repository. The returned counts own the
    /// tenant lock/watch until the caller records them and clears this sweep's progress.
    async fn objects(
        &self,
        scope: &StorageScope,
        purge: PurgeId,
        plan: &WorkspacePlan,
    ) -> Result<SweepCounts, ApiError> {
        self.storage
            .blobs
            .pins()
            .drain(scope.tenant_id.as_str(), &scope.workspace_id.0.to_string())
            .await?;
        crate::sweep::run(
            &self.storage,
            SweepRun {
                tenant: scope.tenant_id.clone(),
                target: Some(scope.workspace_id),
                key: format!("purge/{}", purge.0),
                excluded: plan.archives.clone(),
            },
        )
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
                if step == OBJECTS {
                    // Target writers have drained and the sweep completed. Clear its dead
                    // publication roots atomically with the durable objects report, never
                    // another workspace's or a tenant-level archive mutation's pending puts.
                    transaction
                        .execute(
                            "DELETE FROM pending_puts WHERE tenant_id=(
                                SELECT tenant_id FROM purge_progress WHERE purge_id=?1)
                             AND workspace_id=(SELECT json_extract(plan, '$.workspace_id')
                                FROM purge_progress WHERE purge_id=?1)",
                            [purge.0.to_string()],
                        )
                        .map_err(|error| sql(&error))?;
                    transaction
                        .execute(
                            "DELETE FROM sweep_removals WHERE sweep=?1 AND tenant_id=(
                            SELECT tenant_id FROM purge_progress WHERE purge_id=?2)",
                            params![format!("purge/{}", purge.0), purge.0.to_string()],
                        )
                        .map_err(|error| sql(&error))?;
                }
                Ok(step)
            })
            .await
    }
}

impl Purger for StoragePurger {
    fn clean_up<'a>(&'a self, tenant: &'a TenantId, job: JobId) -> PortFuture<'a, CleanUpReport> {
        Box::pin(self.clean_up_pass(tenant, job))
    }

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

/// Test-only boundary after retaining a rewrite and before moving its durable references.
#[cfg(test)]
fn rewrite_stored_point(storage: &Storage, purge: PurgeId) -> String {
    format!(
        "purge|{}|{}|rewrite-stored",
        storage.data.root().display(),
        purge.0
    )
}

/// Map every revision of the workspace, and every revision an earlier item purge of it
/// mapped, to this purge with no replacement: nothing of the workspace survives.
fn invalidate_revisions(
    transaction: &Transaction<'_>,
    tenant: &str,
    workspace: &str,
    purge: PurgeId,
) -> Result<(), ApiError> {
    transaction
        .execute(
            "UPDATE revision_map SET purge_id = ?3, replacement = NULL
             WHERE tenant_id = ?1 AND workspace_id = ?2",
            params![tenant, workspace, purge.0.to_string()],
        )
        .map_err(|error| sql(&error))?;
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

/// The workspace archives of the tenant's other workspaces in the object store: those written
/// (`backup_archives`) and those recorded as artifacts, each with the workspace it is of.
fn other_archives(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
) -> Result<Vec<(Digest, String)>, ApiError> {
    let kind = variant_text!(&ArtifactKind::WorkspaceBackup)?;
    let mut statement = connection
        .prepare(
            "SELECT digest, workspace_id FROM backup_archives
               WHERE tenant_id = ?1 AND workspace_id IS NOT NULL AND workspace_id <> ?2
             UNION SELECT digest, workspace_id FROM artifacts
               WHERE tenant_id = ?1 AND workspace_id <> ?2 AND kind = ?3",
        )
        .map_err(|error| sql(&error))?;
    let archives = statement
        .query_map(params![tenant, workspace, kind], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| sql(&error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    archives
        .into_iter()
        .map(|(digest, owner)| Ok((from_text!(digest.as_str())?, owner)))
        .collect()
}

/// Point every record of the tenant that names the archive object `old` at its rewrite `new`:
/// the archive and artifact records, a job's download, a job output and an upload slot that
/// holds it.
fn move_archive(
    transaction: &Transaction<'_>,
    tenant: &str,
    old: &Digest,
    new: &ObjectInfo,
) -> Result<(), ApiError> {
    let size = crate::db::to_i64(new.size)?;
    let (old, digest) = (old.as_str(), new.digest.as_str());
    for (query, values) in [
        (
            "UPDATE backup_archives SET digest = ?3, size = ?4 WHERE tenant_id = ?1 AND digest = ?2",
            params![tenant, old, digest, size],
        ),
        (
            "UPDATE artifacts SET digest = ?3, size = ?4 WHERE tenant_id = ?1 AND digest = ?2",
            params![tenant, old, digest, size],
        ),
        (
            "UPDATE uploads SET object_digest = ?3, object_size = ?4
             WHERE tenant_id = ?1 AND object_digest = ?2",
            params![tenant, old, digest, size],
        ),
        (
            "UPDATE jobs SET record = json_set(record, '$.artifact.sha256', ?3,
                 '$.artifact.size', CAST(?4 AS TEXT))
             WHERE tenant_id = ?1 AND json_extract(record, '$.artifact.sha256') = ?2",
            params![tenant, old, digest, size],
        ),
        (
            "UPDATE OR IGNORE job_outputs SET digest = ?3
             WHERE digest = ?2 AND job_id IN (SELECT job_id FROM jobs WHERE tenant_id = ?1)
               AND ?4 IS NOT NULL",
            params![tenant, old, digest, size],
        ),
        (
            "DELETE FROM job_outputs
             WHERE digest = ?2 AND job_id IN (SELECT job_id FROM jobs WHERE tenant_id = ?1)
               AND ?3 IS NOT NULL AND ?4 IS NOT NULL",
            params![tenant, old, digest, size],
        ),
    ] {
        transaction
            .execute(query, values)
            .map_err(|error| sql(&error))?;
    }
    Ok(())
}

/// Record `plan` (JSON) as the purge's plan.
fn store_plan(transaction: &Transaction<'_>, purge: PurgeId, plan: &str) -> Result<(), ApiError> {
    transaction
        .execute(
            "UPDATE purge_progress SET plan = ?1 WHERE purge_id = ?2",
            params![plan, purge.0.to_string()],
        )
        .map(|_| ())
        .map_err(|error| sql(&error))
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
pub(crate) mod guard;
pub(crate) mod references;
mod revisions;
mod rows;
#[cfg(test)]
mod tests;
