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
//! 1. **Plan**, before anything is removed: the objects the workspace references, read
//!    structurally (`references`: what the cards of its whole Git history on every reference
//!    name, what the conversion records among them name, and its records), its
//!    workspace archives (recorded as artifacts or only written), its uploads, the
//!    pre-migration backups that hold it, and the counts of the report. Its records and
//!    history are read under the workspace's write lock, under which every Git write checks
//!    the purge refusal (`guard`), so a commit either is in the plan or is refused. In the
//!    same transaction every commit of the workspace is written to
//!    the revision map as invalidated by this purge with no replacement. The plan holds
//!    identities and counts, never content.
//! 2. **Backups**: every pre-migration copy that holds the workspace is rewritten without it or
//!    deleted, its archive in every pre-migration backup set is deleted, and every archive of
//!    another workspace (in a backup set or in the object store) whose records still cite it is
//!    rewritten with them scrubbed (`copies`). The pre-migration backups are read again on
//!    every attempt, and this step runs once more before the purge completes, so a backup
//!    written after the plan is reached too.
//! 3. **Records**: its rows are removed and what cites it is scrubbed (`rows`); its upload files
//!    are deleted. Its retained exports and archives stop being downloadable here.
//! 4. **Index**: its index entries are removed and the full-text index is merged, so no removed
//!    term stays in an index segment.
//! 5. **Repository**: its repository is deleted under the workspace's write lock; then the rows
//!    and index steps run again, for a row a write in flight added after step 3 and for index
//!    entries an indexing that read the repository before it went stored after step 4 (an
//!    indexing reads and stores under the same lock).
//! 6. **Objects**: a sweep of the in-flight pins begins (`pins`), then what every surviving
//!    workspace and tenant record references is read, other workspaces' pending puts included
//!    (`puts`), except that a survivor's upload slot or pending put holding one of the target's
//!    workspace archives keeps nothing (`references::except_archives`); the step waits for the
//!    target's running writes, adds the objects the target's
//!    pending puts name, deletes each planned object that no surviving reference names and no
//!    other workspace pinned since the sweep began, and clears the target's pending puts; the
//!    others are counted as kept.
//! 7. **Vacuum**: both databases are rebuilt without free pages and their write-ahead logs folded
//!    back, so removed rows leave no bytes behind; then the report is recorded.
//!
//! The purge record itself (`RecordStore::create_purge`), its tenant job and the revision map
//! outlive the workspace: they hold identities only. Installation archives hold no workspace
//! content (Stage 1b design section 8) and are left as they are.
//!
//! What a write in flight can and cannot leave behind (the `Purger` contract in core):
//!
//! - Storage refuses every write to the workspace that the `guard` header lists, new jobs,
//!   job retries and job claims included, from the moment the purge is recorded; a Git write
//!   checks under the workspace's lock, under which step 1 reads. An object put pins, then
//!   records a durable pending put (`puts`) in the transaction of its second refusal check,
//!   before it retains; only a record of the workspace that names the object deletes it. Step 6
//!   waits for the target's running writes (at most ten minutes, then `Unavailable` having
//!   deleted none of the target's objects; repeating the purge resumes it) and deletes the
//!   objects the target's pending puts name, the bytes of a refused, failed, crashed or
//!   replayed write among them; pending puts have no lifetime.
//! - Another workspace's object is deleted only when no reference step 6 read names it (that
//!   workspace's pending puts included, a copy of the target's archive excepted) and no write
//!   of another workspace pinned it since step 6 began (`pins`).
//! - A workspace archive and a written history are refused under the workspace's lock and
//!   record themselves before they release it, so step 1 either refused them or reads them.
//! - Known limit: job work core did not drain before calling the purger can still store what
//!   the plan does not name: an import copies its rows without the refusal.

use std::collections::BTreeSet;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{
    Digest, ItemId, MutationId, PurgeId, TenantId, UploadId, WorkspaceId,
};
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
    /// Other workspaces' archives rewritten without a citation of the workspace: a backup-set
    /// archive by its name relative to the pre-migration directory, an archive in the object
    /// store by the digest it had.
    #[serde(default)]
    rewritten: BTreeSet<String>,
    /// Archive objects a rewrite replaced; each is deleted, never counted as a kept object.
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
            self.index(&scope).await?;
            step = self.advance(purge, REPOSITORY, None).await?;
        }
        if step < OBJECTS {
            let (removed, kept) = self.objects(&scope, &plan).await?;
            plan.report.objects_removed = removed.to_string();
            plan.report.objects_kept_shared = kept.to_string();
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

    /// Step 1: read what the workspace references and record the plan with the revision map.
    async fn plan(
        &self,
        scope: &StorageScope,
        mutation: MutationId,
        purge: PurgeId,
    ) -> Result<WorkspacePlan, ApiError> {
        let (tenant, workspace) = scope_key(scope);
        let (history, objects, copy_plan, archives, uploads) = {
            let repositories = self.storage.repositories.clone();
            let records = self.storage.records.clone();
            let blobs = self.storage.blobs.clone();
            let pre_migration = self.storage.data.pre_migration_path();
            let scope = scope.clone();
            let (tenant, workspace) = (tenant.clone(), workspace.clone());
            blocking(move || {
                // Read under the workspace's write lock: a Git write checks the purge refusal
                // under the same lock, so it either landed before this read or is refused.
                let (target, archives, uploads, history) = repositories.locked(&scope, || {
                    let (tenant, workspace) = (tenant.clone(), workspace.clone());
                    let (target, archives, uploads) = tokio::runtime::Handle::current().block_on(
                        records.call(move |connection| {
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
                        }),
                    )?;
                    let history = if repositories.path(&scope).exists() {
                        references::history(&repositories.open(&scope)?)?
                    } else {
                        references::History::default()
                    };
                    Ok((target, archives, uploads, history))
                })?;
                // The same structural sets a survivor is read by (`objects`): what the cards
                // name, and what the conversion records among them name.
                let mut objects = target;
                objects.extend(history.objects.keys().cloned());
                references::follow_records(
                    &blobs,
                    &scope.tenant_id,
                    &history.records,
                    &mut objects,
                )?;
                let mut held = BTreeSet::new();
                for digest in objects {
                    if blobs.local_path(&scope.tenant_id, &digest)?.is_file() {
                        held.insert(digest);
                    }
                }
                let copy_plan = copies::plan(&pre_migration, &tenant, &workspace)?;
                Ok((history.revisions, held, copy_plan, archives, uploads))
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
                    rewritten: BTreeSet::new(),
                    superseded: BTreeSet::new(),
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
                for name in copies::other_archives(&directory, &tenant, &workspace)? {
                    let archive = directory.join(&name);
                    let next = archive.with_extension("rewrite");
                    if copies::rewrite_archive(
                        &archive, &next, &staging, &tenant, &workspace, purge,
                    )? {
                        std::fs::rename(&next, &archive)
                            .map_err(|error| io_error("replace", &archive, &error))?;
                        rewritten.push(name);
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
    /// the workspace: the rewritten archive is stored, every record naming the old one is moved
    /// to it (with the plan, in one transaction), and the old one is deleted.
    async fn stored_archives(
        &self,
        scope: &StorageScope,
        purge: PurgeId,
        plan: &mut WorkspacePlan,
        staging: &std::path::Path,
    ) -> Result<(), ApiError> {
        let (tenant, workspace) = scope_key(scope);
        let digests = {
            let (tenant, workspace) = (tenant.clone(), workspace.clone());
            self.storage
                .records
                .call(move |connection| other_archives(connection, &tenant, &workspace))
                .await?
        };
        for old in &plan.superseded {
            self.storage.blobs.remove_for(&scope.tenant_id, old)?;
        }
        for old in digests {
            let path = self.storage.blobs.local_path(&scope.tenant_id, &old)?;
            if !path.is_file() {
                continue;
            }
            let next = staging.join("rewritten.zip");
            let rewritten = {
                let (next, staging) = (next.clone(), staging.to_path_buf());
                let (tenant, workspace) = (tenant.clone(), workspace.clone());
                blocking(move || {
                    copies::rewrite_archive(&path, &next, &staging, &tenant, &workspace, purge)
                })
                .await?
            };
            if !rewritten {
                continue;
            }
            let size = std::fs::metadata(&next)
                .map_err(|error| io_error("read", &next, &error))?
                .len();
            let new = self
                .storage
                .blobs
                .put_for(
                    &scope.tenant_id,
                    crate::blobs::file_reader(Some(next), 0, size),
                    size,
                    None,
                )
                .await?;
            plan.rewritten.insert(old.as_str().to_owned());
            plan.superseded.insert(old.clone());
            plan.recount();
            let text = serde_json::to_string(&*plan).map_err(|error| json(&error))?;
            let tenant = tenant.clone();
            let moved = old.clone();
            self.storage
                .records
                .transaction(move |transaction| {
                    move_archive(transaction, &tenant, &moved, &new)?;
                    store_plan(transaction, purge, &text)
                })
                .await?;
            self.storage.blobs.remove_for(&scope.tenant_id, &old)?;
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
    ///
    /// The sweep (`pins`) begins before any surviving reference is read. An object is deleted
    /// only when no durable reference the step read names it (another workspace's pending puts
    /// included, `puts`, but no survivor's copy of the target's archive) and no other
    /// workspace's write pinned it at any moment from the start
    /// of that read until the delete, which runs under the pin table's lock. The target's own
    /// pending puts, read once its running writes have ended, name bytes no record of it names:
    /// they are deleted with it, and the rows are cleared.
    async fn objects(
        &self,
        scope: &StorageScope,
        plan: &WorkspacePlan,
    ) -> Result<(usize, usize), ApiError> {
        let (tenant, workspace) = scope_key(scope);
        let blobs = self.storage.blobs.clone();
        let (sweep, pinned) = blobs.pins().sweep(&tenant, &workspace)?;
        let archives = plan.archives.clone();
        let (mut surviving, pending) = {
            let (tenant, workspace) = (tenant.clone(), workspace.clone());
            self.storage
                .records
                .call(move |connection| {
                    let surviving = references::records(
                        connection,
                        &tenant,
                        &workspace,
                        Rows::Survivors,
                        &archives,
                    )?;
                    // Another workspace's pending put of the target's archive keeps it no more
                    // than its upload slot would (`references::except_archives`).
                    let pending = references::except_archives(
                        puts::others(connection, &tenant, &workspace)?,
                        &archives,
                    )
                    .collect::<Vec<_>>();
                    Ok((surviving, pending))
                })
                .await?
        };
        let repositories = self.storage.repositories.clone();
        let scope = scope.clone();
        let surviving = {
            let (blobs, scope) = (blobs.clone(), scope.clone());
            blocking(move || {
                let (mut found, mut records) = (BTreeSet::new(), BTreeSet::new());
                for path in repositories.others(&scope.tenant_id, &scope)? {
                    let repository = git2::Repository::open_bare(&path)
                        .map_err(|error| crate::git::repo::git(&error))?;
                    // Read structurally, as the plan reads the target: a survivor's card or
                    // conversion record that does not read fails the purge (`damaged`) rather
                    // than risk deleting what it names.
                    let history = references::history(&repository)?;
                    found.extend(history.objects.into_keys());
                    records.extend(history.records);
                }
                // The objects other workspaces were putting when the sweep began, and those of
                // their pending puts, are about to be referenced, and kept. A conversion record
                // is followed only once a card names it; until then its exports and assets are
                // pending puts of the same write, kept as such, and its job's completion then
                // names each as a job output.
                found.extend(pinned);
                found.extend(pending);
                references::follow_records(&blobs, &scope.tenant_id, &records, &mut found)?;
                surviving.extend(found);
                Ok(surviving)
            })
            .await?
        };
        // Every write of the target that pinned before the purge was recorded has ended; one
        // that pins from now on records and stores nothing (`LocalBlobs::put_pinned`). The
        // bytes its pending puts name, a refused upload completion's and a crashed write's
        // among them, go with the target.
        sweep.drain().await?;
        let held = {
            let (tenant, workspace) = (tenant.clone(), workspace.clone());
            self.storage
                .records
                .call(move |connection| puts::of(connection, &tenant, &workspace))
                .await?
        };
        let mut planned = plan.objects.clone();
        for digest in held {
            if blobs.local_path(&scope.tenant_id, &digest)?.is_file() {
                planned.insert(digest);
            }
        }
        let (removed, kept) = blocking(move || {
            let mut removed = 0_usize;
            let mut kept = 0_usize;
            for digest in &planned {
                let deleted = !surviving.contains(digest)
                    && sweep.delete_unless_touched(digest, || {
                        blobs.remove_for(&scope.tenant_id, digest)
                    })?;
                if deleted {
                    removed = removed.saturating_add(1);
                } else {
                    kept = kept.saturating_add(1);
                }
            }
            Ok((removed, kept))
        })
        .await?;
        self.storage
            .records
            .transaction(move |transaction| puts::clear(transaction, &tenant, &workspace))
            .await?;
        Ok((removed, kept))
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

/// The workspace archives of the tenant's other workspaces in the object store: those written
/// (`backup_archives`) and those recorded as artifacts.
fn other_archives(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
) -> Result<Vec<Digest>, ApiError> {
    let kind = variant_text!(&ArtifactKind::WorkspaceBackup)?;
    let mut statement = connection
        .prepare(
            "SELECT digest FROM backup_archives
               WHERE tenant_id = ?1 AND workspace_id IS NOT NULL AND workspace_id <> ?2
             UNION SELECT digest FROM artifacts
               WHERE tenant_id = ?1 AND workspace_id <> ?2 AND kind = ?3",
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
mod rows;
#[cfg(test)]
mod tests;
