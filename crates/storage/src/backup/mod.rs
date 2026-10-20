//! Managed backups (`Backups`, Stage 1b design section 8) and the offline functions the server
//! command and startup call.
//!
//! - `write_workspace_archive` takes a consistent checkpoint under the workspace's write lock:
//!   the records snapshot (`snapshot`: `VACUUM INTO` a private file, filtered, vacuumed again,
//!   checked), a Git pack of every reference, and a copy of every retained object the workspace
//!   references; it zips them (`archive`) and only then publishes the archive into the blob
//!   store. Drafts are included; anything a purge removed is not, because a purge removed it
//!   from every store the archive is read from.
//! - `write_installation_archive` holds no workspace content: the installation identity,
//!   connectors without secrets, purge records and tenant events.
//! - `open_installation_archive` reads back only an object this store wrote as an installation
//!   archive of that tenant.
//! - `restore_import` fills a blank workspace (`restore`).
//! - `StorageBackups::write_backup_set` is the offline backup (every installation archive and
//!   every workspace archive into a directory); `write_pre_migration_set` writes the same set
//!   under `<data>/backups/pre-migration` before an upgrade migration; and
//!   `StorageBackups::restore_replacement` is the replacement restore.
//!
//! Each archive write is idempotent on its mutation id (`backup_archives`), and so is each
//! import (`restores`).
//!
//! A backup set (offline, or pre-migration under `<data>/backups/pre-migration`) holds
//! `installation.zip` (the installation-level archive: identity and connectors, no tenant),
//! `installation/<tenant>.zip` and `workspaces/<tenant>/<workspace>.zip`. A purge deletes a
//! workspace's archive from every set (`purge::copies`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, MutationId, TenantId};
use okf_jawn_contract::workspace::RestoreReport;
use okf_jawn_core::credentials::secret_hash;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::search::SearchIndex as _;
use okf_jawn_core::storage::{
    Backups, ObjectInfo, ObjectRead, StorageScope, VersionStore as _, WrittenArchive,
};
use rusqlite::{Connection, OptionalExtension, params};
use tokio::runtime::Handle;

use crate::blobs::{LocalBlobs, file_reader};
use crate::data::{DataDir, io_error};
use crate::db::{internal, json, not_found, sql, to_i64, to_u64};
use crate::git::repo::Repositories;
use crate::puts;
use crate::seam;
use crate::storage::Storage;
use archive::InstallationManifest;
use restore::Mode;

/// `Backups` over every store of the opened data directory.
#[derive(Debug, Clone)]
pub struct StorageBackups {
    storage: Storage,
}

/// The directory of a pre-migration backup set that holds its workspace archives.
pub(crate) const PRE_MIGRATION_WORKSPACES: &str = "workspaces";
/// The directory of a backup set that holds its installation archives.
const INSTALLATIONS: &str = "installation";
/// The installation-level archive of a backup set (identity and connectors, no tenant).
const INSTALLATION_LEVEL: &str = "installation.zip";

impl StorageBackups {
    pub(crate) const fn new(storage: Storage) -> Self {
        Self { storage }
    }

    /// The offline backup: the installation-level archive (`installation.zip`), every tenant's
    /// installation archive and every workspace archive (of `only`, when given) into `output`,
    /// as `installation/<tenant>.zip` and `workspaces/<tenant>/<workspace>.zip`. Run it with no
    /// service on the data directory.
    ///
    /// # Errors
    /// Returns `Unavailable` when a file cannot be written, `Internal` when a snapshot does not
    /// verify.
    pub async fn write_backup_set(
        &self,
        output: &Path,
        only: Option<&TenantId>,
    ) -> Result<Vec<PathBuf>, ApiError> {
        let repositories = self.storage.repositories.clone();
        let blobs = self.storage.blobs.clone();
        let staging = self
            .storage
            .data
            .staging_path()
            .join(format!("backup-{}", seam::hex(&seam::random_bytes::<8>()?)));
        let output = output.to_path_buf();
        let only = only.cloned();
        self.storage
            .records
            .call(move |connection| {
                let written = write_set(
                    connection,
                    &repositories,
                    &blobs,
                    &staging,
                    &output,
                    only.as_ref(),
                );
                let _ = std::fs::remove_dir_all(&staging);
                written
            })
            .await
    }

    /// The replacement restore: every workspace archive is checked first, without writing
    /// anything (`restore::check`: its entries, objects, records, schema and rows, and its
    /// tenant against the installation archive's), so a set with a refused archive writes
    /// nothing at all. Then the installation archive (identity, purge records, tenant events
    /// under their own keys, connectors restored as revoked, since no secret is kept), then each
    /// workspace archive under its own identity, whose restored head is then indexed for search.
    /// Run it with no service on the data directory. Run again (after a stop in a later
    /// workspace, say), it accepts and leaves as it is a workspace this same archive file
    /// already restored in full, unless a purge of it is now recorded.
    ///
    /// The check reads every archive before anything is written, but a few faults show only at
    /// the copy of a workspace's rows: a constraint the live database enforces across archives
    /// (two archives of one set whose workspaces carry one creation mutation), or an encoding
    /// the check did not read. A run that fails there leaves exactly what it wrote before the
    /// failing copy: the installation archive's rows; every workspace restored before the
    /// failing one, in full (its rows, history and objects, and the record of this restore);
    /// and of the failing workspace its objects, each still recorded as a pending
    /// put of it, and its history (the repository and every reference), but none of its rows.
    /// No later workspace is touched. A run again with a good set completes: the installation
    /// rows are written again over themselves, each workspace already restored in full is
    /// accepted as it is, and the failing one's repository, which no record names, is replaced
    /// by its restore.
    ///
    /// # Errors
    /// Returns `InvalidInput` for an archive this build cannot read, refuses, or of another
    /// tenant, and `Conflict` for a workspace any recorded purge names (finished or not) or that
    /// exists and was not restored from this same archive.
    pub async fn restore_replacement(
        &self,
        installation: &Path,
        workspaces: &[PathBuf],
    ) -> Result<Vec<RestoreReport>, ApiError> {
        let manifest = archive::Opened::open(installation)?.installation()?;
        // An installation-level archive (no tenant) restores the identity and connectors only.
        let tenant = manifest.tenant_id.clone();
        if tenant.is_none() && !workspaces.is_empty() {
            return Err(archive::invalid(
                "an installation-level archive restores no workspace; give the tenant's archive",
            ));
        }
        let mut scopes = Vec::new();
        for path in workspaces {
            let checked = self.check_archive(path).await?;
            if tenant.as_ref() != Some(&checked.tenant_id) {
                return Err(archive::invalid(
                    "it belongs to another tenant than the installation archive",
                ));
            }
            scopes.push(StorageScope {
                tenant_id: checked.tenant_id,
                workspace_id: checked.workspace_id,
            });
        }
        self.storage
            .records
            .transaction(move |transaction| install(transaction, &manifest))
            .await?;
        let mut reports = Vec::new();
        for (path, scope) in workspaces.iter().zip(scopes) {
            let restored_scope = scope.clone();
            let storage = self.storage.clone();
            let runtime = Handle::current();
            let path = path.clone();
            let staging = self
                .storage
                .data
                .staging_path()
                .join(format!("restore-{}", scope.workspace_id.0));
            let report = tokio::task::spawn_blocking(move || {
                let mode = Mode::Replace(crate::blobs::hash_file(&path)?);
                std::fs::create_dir_all(&staging)
                    .map_err(|error| io_error("create", &staging, &error))?;
                let restored = restore::restore(&storage, &runtime, &scope, &path, &staging, &mode);
                let _ = std::fs::remove_dir_all(&staging);
                restored.map(|(report, _)| report)
            })
            .await
            .map_err(|error| internal(format!("a restore task stopped: {error}")))??;
            // The search index is derived and starts empty in a replaced directory: the
            // restored head is indexed at once, so search and links answer there without a
            // rebuild (older revisions follow core's indexing window like any revision).
            // Indexing an indexed revision again is a no-op, so a run again indexes too.
            let head = self.storage.versions().head(&restored_scope).await?;
            self.storage
                .search()
                .index_revision(&restored_scope, head)
                .await?;
            reports.push(report);
        }
        Ok(reports)
    }

    /// Check the workspace archive at `path` as a restore does before it writes anything
    /// (`restore::check`), staging under a private directory removed afterwards; returns its
    /// manifest.
    async fn check_archive(&self, path: &Path) -> Result<archive::WorkspaceManifest, ApiError> {
        let storage = self.storage.clone();
        let runtime = Handle::current();
        let path = path.to_path_buf();
        let staging = self.storage.data.staging_path().join(format!(
            "restore-check-{}",
            seam::hex(&seam::random_bytes::<8>()?)
        ));
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&staging)
                .map_err(|error| io_error("create", &staging, &error))?;
            let checked = restore::check(&storage, &runtime, &path, &staging);
            let _ = std::fs::remove_dir_all(&staging);
            checked
        })
        .await
        .map_err(|error| internal(format!("a restore task stopped: {error}")))?
    }

    async fn workspace_archive(
        &self,
        scope: &StorageScope,
        mutation: MutationId,
    ) -> Result<WrittenArchive, ApiError> {
        if let Some(object) = self.recorded(mutation).await? {
            let (blobs, tenant) = (self.storage.blobs.clone(), scope.tenant_id.clone());
            return tokio::task::spawn_blocking(move || written_archive(&blobs, &tenant, object))
                .await
                .map_err(|error| internal(format!("a backup task stopped: {error}")))?;
        }
        crate::purge::guard::refuse_scope(&self.storage.records, scope).await?;
        #[cfg(test)]
        crate::pause::point(&pause_point(&self.storage, scope)).await?;
        let storage = self.storage.clone();
        let runtime = Handle::current();
        let locked_scope = scope.clone();
        tokio::task::spawn_blocking(move || {
            let scope = &locked_scope;
            storage.repositories.locked(scope, || {
                // Checked again under the lock, under which a purge plans the workspace, and the
                // archive is recorded before the lock is released: a purge either refuses this
                // archive or finds its record in the plan (`purge::archives`).
                runtime.block_on(crate::purge::guard::refuse_scope(&storage.records, scope))?;
                // A call under the same mutation that held the lock first recorded its archive
                // under it: answer that one and store nothing, so no second archive object exists
                // that no record names (and a purge could not reach).
                if let Some(object) =
                    runtime.block_on(recorded_archive(&storage.records, mutation))?
                {
                    return written_archive(&storage.blobs, &scope.tenant_id, object);
                }
                let staging = storage.repositories.stage(scope, mutation)?;
                let zip = staging.path.join("archive.zip");
                let directory = staging.path.clone();
                let full = runtime.block_on(
                    storage
                        .records
                        .call(move |connection| snapshot::copy_live(connection, &directory)),
                )?;
                let rows_left_out = workspace_zip(
                    &storage.repositories,
                    &storage.blobs,
                    scope,
                    &full,
                    &staging.path,
                    &zip,
                )?;
                let size = std::fs::metadata(&zip)
                    .map_err(|error| io_error("read", &zip, &error))?
                    .len();
                // A workspace-scoped put, like every other: pinned, and recorded as a pending put
                // before the bytes are retained, which the archive's record lands. A stop between
                // the two leaves the pending put, so a purge of the workspace deletes the bytes.
                let (object, pinned) = runtime.block_on(storage.blobs.put_pinned(
                    scope,
                    file_reader(Some(zip.clone()), 0, size),
                    size,
                    None,
                ))?;
                let recorded = runtime.block_on(record(
                    &storage.records,
                    mutation,
                    &scope.tenant_id,
                    Some(scope),
                    object,
                ));
                // The write ends only once its record transaction has ended.
                pinned.end()?;
                #[cfg(test)]
                runtime.block_on(crate::pause::point(&recorded_point(&storage, mutation)))?;
                recorded.map(|object| WrittenArchive {
                    object,
                    rows_left_out,
                })
            })
        })
        .await
        .map_err(|error| internal(format!("a backup task stopped: {error}")))?
    }

    async fn installation_archive(
        &self,
        tenant: &TenantId,
        mutation: MutationId,
    ) -> Result<ObjectInfo, ApiError> {
        if let Some(object) = self.recorded(mutation).await? {
            return Ok(object);
        }
        if let Some(object) = self.adopt_installation(tenant, mutation).await? {
            return Ok(object);
        }
        let manifest = {
            let tenant = tenant.clone();
            self.storage
                .records
                .call(move |connection| archive::installation_of(connection, &tenant))
                .await?
        };
        // One staging directory per call: two calls under one mutation never share a file.
        let directory = self
            .storage
            .data
            .staging_path()
            .join(INSTALLATIONS)
            .join(format!(
                "{}-{}",
                mutation.0,
                seam::hex(&seam::random_bytes::<8>()?)
            ));
        let zip = directory.join("installation.zip");
        let size = {
            let (directory, zip) = (directory.clone(), zip.clone());
            tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(&directory)
                    .map_err(|error| io_error("create", &directory, &error))?;
                archive::write_installation(&zip, &manifest)?;
                std::fs::metadata(&zip)
                    .map(|metadata| metadata.len())
                    .map_err(|error| io_error("read", &zip, &error))
            })
            .await
            .map_err(|error| internal(format!("a backup task stopped: {error}")))??
        };
        let object = self
            .storage
            .blobs
            .put_tracked(tenant, mutation, file_reader(Some(zip), 0, size), size)
            .await;
        let _ = std::fs::remove_dir_all(&directory);
        let (object, pinned) = object?;
        #[cfg(test)]
        crate::pause::point(&installation_stored_point(&self.storage, mutation)).await?;
        let recorded = record(&self.storage.records, mutation, tenant, None, object).await;
        pinned.end()?;
        recorded
    }

    /// A retry adopts the first retained tenant archive under this mutation. Pin before
    /// looking for its bytes; the pending row protects it while no writer is running.
    async fn adopt_installation(
        &self,
        tenant: &TenantId,
        mutation: MutationId,
    ) -> Result<Option<ObjectInfo>, ApiError> {
        let key = tenant.as_str().to_owned();
        let pending = self
            .storage
            .records
            .call(move |connection| puts::tenant_pending(connection, &key, mutation))
            .await?;
        for digest in pending {
            let pinned = self
                .storage
                .blobs
                .pins()
                .pin_for(tenant.as_str(), None, &digest)?;
            let object = match self.storage.blobs.open_for(tenant, &digest, 0, 0).await {
                Ok(read) => read.object,
                Err(error) if error.code == ErrorCode::NotFound => {
                    pinned.end()?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let recorded = record(&self.storage.records, mutation, tenant, None, object).await;
            pinned.end()?;
            return recorded.map(Some);
        }
        Ok(None)
    }

    async fn open_installation(
        &self,
        tenant: &TenantId,
        digest: &Digest,
        offset: u64,
        length: u64,
    ) -> Result<ObjectRead, ApiError> {
        let (tenant_key, digest_key) = (tenant.as_str().to_owned(), digest.as_str().to_owned());
        let written = self
            .storage
            .records
            .call(move |connection| {
                connection
                    .query_row(
                        "SELECT 1 FROM backup_archives
                         WHERE tenant_id = ?1 AND workspace_id IS NULL AND digest = ?2",
                        params![tenant_key, digest_key],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
                    .map_err(|error| sql(&error))
            })
            .await?;
        if written.is_none() {
            return Err(not_found("installation archive"));
        }
        self.storage
            .blobs
            .open_for(tenant, digest, offset, length)
            .await
    }

    async fn import(
        &self,
        scope: &StorageScope,
        archive: Digest,
        mutation: MutationId,
        editors: Vec<String>,
    ) -> Result<RestoreReport, ApiError> {
        let key = mutation.0.to_string();
        let prior: Option<String> = self
            .storage
            .records
            .call(move |connection| {
                connection
                    .query_row(
                        "SELECT report FROM restores WHERE mutation_id = ?1",
                        [key],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| sql(&error))
            })
            .await?;
        if let Some(prior) = prior {
            return serde_json::from_str(&prior).map_err(|error| json(&error));
        }
        let path = self.storage.blobs.local_path(&scope.tenant_id, &archive)?;
        if !path.is_file() {
            return Err(not_found("archive"));
        }
        let storage = self.storage.clone();
        let runtime = Handle::current();
        let locked_scope = scope.clone();
        let report = tokio::task::spawn_blocking(move || {
            let scope = &locked_scope;
            storage.repositories.locked(scope, || {
                let staging = storage.repositories.stage(scope, mutation)?;
                restore::restore(
                    &storage,
                    &runtime,
                    scope,
                    &path,
                    &staging.path,
                    &Mode::Import(editors, mutation),
                )
                .map(|(report, _)| report)
            })
        })
        .await
        .map_err(|error| internal(format!("a restore task stopped: {error}")))??;
        Ok(report)
    }

    async fn recorded(&self, mutation: MutationId) -> Result<Option<ObjectInfo>, ApiError> {
        recorded_archive(&self.storage.records, mutation).await
    }
}

impl Backups for StorageBackups {
    fn managed_kind<'a>(
        &'a self,
        tenant: &'a TenantId,
        digest: &'a Digest,
    ) -> PortFuture<'a, Option<okf_jawn_core::storage::ManagedKind>> {
        let tenant = tenant.as_str().to_owned();
        let digest = digest.clone();
        Box::pin(self.storage.records.call(move |connection| {
            Ok(managed::objects(connection, &tenant, Some(&digest))?.remove(&digest))
        }))
    }
    fn write_workspace_archive<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
    ) -> PortFuture<'a, WrittenArchive> {
        Box::pin(self.workspace_archive(scope, mutation_id))
    }

    fn write_installation_archive<'a>(
        &'a self,
        tenant: &'a TenantId,
        mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo> {
        Box::pin(self.installation_archive(tenant, mutation_id))
    }

    fn open_installation_archive<'a>(
        &'a self,
        tenant: &'a TenantId,
        digest: &'a Digest,
        offset: u64,
        length: u64,
    ) -> PortFuture<'a, ObjectRead> {
        Box::pin(self.open_installation(tenant, digest, offset, length))
    }

    fn restore_import<'a>(
        &'a self,
        scope: &'a StorageScope,
        archive: Digest,
        mutation_id: MutationId,
        editors: Vec<String>,
    ) -> PortFuture<'a, RestoreReport> {
        Box::pin(self.import(scope, archive, mutation_id, editors))
    }
}

/// The archive recorded under `mutation`, if any.
async fn recorded_archive(
    records: &crate::db::Db,
    mutation: MutationId,
) -> Result<Option<ObjectInfo>, ApiError> {
    let key = mutation.0.to_string();
    records
        .call(move |connection| {
            let found: Option<(String, i64)> = connection
                .query_row(
                    "SELECT digest, size FROM backup_archives WHERE mutation_id = ?1",
                    [key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| sql(&error))?;
            found
                .map(|(digest, size)| {
                    Ok(ObjectInfo {
                        digest: from_text!(digest.as_str())?,
                        size: to_u64(size)?,
                    })
                })
                .transpose()
        })
        .await
}

/// Record the archive `object` a backup wrote under `mutation` and return the recorded archive:
/// the first writer's, when another call under the same mutation recorded one first, so every
/// caller answers the archive the store keeps (and a purge reaches).
async fn record(
    records: &crate::db::Db,
    mutation: MutationId,
    tenant: &TenantId,
    scope: Option<&StorageScope>,
    object: ObjectInfo,
) -> Result<ObjectInfo, ApiError> {
    let tenant = tenant.as_str().to_owned();
    let workspace = scope.map(|scope| scope.workspace_id.0.to_string());
    let key = mutation.0.to_string();
    records
        .transaction(move |transaction| {
            transaction
                .execute(
                    "INSERT INTO backup_archives (mutation_id, tenant_id, workspace_id, digest,
                         size)
                     VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (mutation_id) DO NOTHING",
                    params![
                        key,
                        tenant,
                        workspace,
                        object.digest.as_str(),
                        to_i64(object.size)?
                    ],
                )
                .map_err(|error| sql(&error))?;
            // A workspace archive's record lands the pending put of the object it wrote.
            if let Some(workspace) = &workspace {
                puts::land(transaction, &tenant, workspace, [&object.digest])?;
            } else {
                puts::land_tenant(transaction, &tenant, mutation)?;
            }
            let (digest, size): (String, i64) = transaction
                .query_row(
                    "SELECT digest, size FROM backup_archives WHERE mutation_id = ?1",
                    [&key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|error| sql(&error))?;
            Ok(ObjectInfo {
                digest: from_text!(digest.as_str())?,
                size: to_u64(size)?,
            })
        })
        .await
}

/// Write the pre-migration backup set of the locked `data` directory before its records are
/// upgraded (SPEC section 2: "takes a recoverable backup before an upgrade migration"). The set
/// is written under a `.partial-` name and renamed into place only when complete. Blocking.
///
/// # Errors
/// Returns `Unavailable` when the set cannot be written; nothing is migrated then.
pub(crate) fn write_pre_migration_set(data: &std::sync::Arc<DataDir>) -> Result<PathBuf, ApiError> {
    let connection = snapshot::read_only(&data.records_path())?;
    let repositories = Repositories::new(std::sync::Arc::clone(data));
    let blobs = LocalBlobs::open(std::sync::Arc::clone(data))?;
    let stamp = seam::now_ms()?;
    let partial = data
        .pre_migration_path()
        .join(format!(".partial-set-{stamp}"));
    let staging = data.staging_path().join(format!("pre-migration-{stamp}"));
    let written = write_set(&connection, &repositories, &blobs, &staging, &partial, None);
    let _ = std::fs::remove_dir_all(&staging);
    written.map_err(|error| {
        ApiError::new(
            ErrorCode::Unavailable,
            format!(
                "the pre-migration backup failed, so nothing was migrated: {}",
                error.message
            ),
        )
    })?;
    let set = data.pre_migration_path().join(format!("set-{stamp}"));
    std::fs::rename(&partial, &set).map_err(|error| io_error("publish", &set, &error))?;
    Ok(set)
}

/// Write every installation archive and every workspace archive (of `only`) into `output`.
fn write_set(
    connection: &Connection,
    repositories: &Repositories,
    blobs: &LocalBlobs,
    staging: &Path,
    output: &Path,
    only: Option<&TenantId>,
) -> Result<Vec<PathBuf>, ApiError> {
    // Every tenant any record names, from every table with a `tenant_id` column (read from the
    // schema, so a new table is never missed): an installation archive is written for each,
    // even one with no workspace and no purge. The installation identity and connectors have
    // no tenant column; they travel in the installation-level archive (and in each tenant's).
    let mut sources = vec!["SELECT tenant_id, workspace_id FROM workspaces".to_owned()];
    for table in snapshot::tables(connection)? {
        let tenant_keyed: bool = connection
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM pragma_table_info(?1) WHERE name = 'tenant_id')",
                [&table],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        if tenant_keyed && table != "workspaces" {
            sources.push(format!("SELECT tenant_id, NULL FROM \"{table}\""));
        }
    }
    let mut statement = connection
        .prepare(&format!("{} ORDER BY 1, 2", sources.join(" UNION ")))
        .map_err(|error| sql(&error))?;
    let rows: Vec<(String, Option<String>)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|error| sql(&error))?
        .collect::<Result<_, _>>()
        .map_err(|error| sql(&error))?;
    let mut written = Vec::new();
    // The requested tenant always gets its installation archive.
    let mut tenants: BTreeSet<TenantId> = only.into_iter().cloned().collect();
    for (tenant, workspace) in rows {
        let tenant: TenantId = from_text!(tenant.as_str())?;
        if only.is_some_and(|only| *only != tenant) {
            continue;
        }
        tenants.insert(tenant.clone());
        let Some(workspace) = workspace else {
            continue;
        };
        let scope = StorageScope {
            tenant_id: tenant,
            workspace_id: from_text!(workspace.as_str())?,
        };
        let directory = staging.join(&workspace);
        let target_directory = output
            .join(PRE_MIGRATION_WORKSPACES)
            .join(scope.tenant_id.as_str());
        for path in [&directory, &target_directory] {
            std::fs::create_dir_all(path).map_err(|error| io_error("create", path, &error))?;
        }
        let target = target_directory.join(format!("{workspace}.zip"));
        let full = snapshot::copy_live(connection, &directory)?;
        workspace_zip(repositories, blobs, &scope, &full, &directory, &target)?;
        written.push(target);
    }
    for tenant in tenants {
        let directory = output.join(INSTALLATIONS);
        std::fs::create_dir_all(&directory)
            .map_err(|error| io_error("create", &directory, &error))?;
        let target = directory.join(format!("{}.zip", tenant.as_str()));
        archive::write_installation(&target, &archive::installation_of(connection, &tenant)?)?;
        written.push(target);
    }
    // The installation identity and connectors carry no tenant: one installation-level archive
    // holds them in every set, whatever tenants exist (even none).
    std::fs::create_dir_all(output).map_err(|error| io_error("create", output, &error))?;
    let level = output.join(INSTALLATION_LEVEL);
    archive::write_installation(&level, &archive::installation_level_of(connection)?)?;
    written.push(level);
    Ok(written)
}

/// Filter a full copy to the workspace and write its archive to `target`; answers the rows it
/// left out. Blocking.
fn workspace_zip(
    repositories: &Repositories,
    blobs: &LocalBlobs,
    scope: &StorageScope,
    full: &snapshot::FullCopy,
    directory: &Path,
    target: &Path,
) -> Result<u32, ApiError> {
    let (tenant, workspace) = crate::db::scope_key(scope);
    let excluded = snapshot::archive_objects(full, &tenant)?;
    let (records, rows_left_out) =
        snapshot::filter(full, directory, (&tenant, &workspace), &excluded)?;
    let repository = repositories.open(scope)?;
    archive::write_workspace(
        target,
        &repository,
        (&records, rows_left_out),
        blobs,
        &scope.tenant_id,
        &excluded,
    )?;
    Ok(rows_left_out)
}

/// The workspace archive `object` of `tenant` with the count of rows its manifest says it left
/// out: the answer to a repeat of the mutation that wrote it. Blocking: it reads the archive,
/// which `archive::Opened::open` checks whole.
fn written_archive(
    blobs: &LocalBlobs,
    tenant: &TenantId,
    object: ObjectInfo,
) -> Result<WrittenArchive, ApiError> {
    let path = blobs.local_path(tenant, &object.digest)?;
    let rows_left_out = archive::Opened::open(&path)?.workspace()?.rows_left_out;
    Ok(WrittenArchive {
        object,
        rows_left_out,
    })
}

/// Restore the installation records of a replacement restore.
fn install(
    transaction: &rusqlite::Transaction<'_>,
    manifest: &InstallationManifest,
) -> Result<(), ApiError> {
    // Purge records and tenant events belong to a tenant; an installation-level archive has none.
    if manifest.tenant_id.is_none() && !(manifest.purges.is_empty() && manifest.events.is_empty()) {
        return Err(archive::invalid(
            "an archive with purges or events names no tenant",
        ));
    }
    let tenant = manifest
        .tenant_id
        .as_ref()
        .map(|tenant| tenant.as_str().to_owned())
        .unwrap_or_default();
    if let Some((subject, created_at)) = &manifest.identity {
        transaction
            .execute(
                "INSERT INTO installation (singleton, subject, created_at) VALUES (1, ?1, ?2)
                 ON CONFLICT (singleton) DO UPDATE SET subject = ?1, created_at = ?2",
                params![subject, created_at],
            )
            .map_err(|error| sql(&error))?;
    }
    let now = seam::now()?;
    for (connector, mutation, record) in &manifest.connectors {
        let mut record: serde_json::Value =
            serde_json::from_str(record).map_err(|error| json(&error))?;
        if let Some(fields) = record.as_object_mut()
            && fields
                .get("revoked_at")
                .is_none_or(serde_json::Value::is_null)
        {
            fields.insert("revoked_at".to_owned(), serde_json::json!(now.as_str()));
        }
        // No secret is kept, so the restored connector holds the hash of a secret nobody has.
        let unusable = secret_hash(&seam::hex(&seam::random_bytes::<32>()?));
        transaction
            .execute(
                "INSERT INTO connectors (connector_id, mutation_id, secret_hash, revoked, record)
                 VALUES (?1, ?2, ?3, 1, ?4) ON CONFLICT DO NOTHING",
                params![connector, mutation, unusable.as_slice(), record.to_string()],
            )
            .map_err(|error| sql(&error))?;
    }
    for (purge, target, state, mutation, record) in &manifest.purges {
        transaction
            .execute(
                "INSERT INTO purges (purge_id, tenant_id, target_key, state, mutation_id, record)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT DO NOTHING",
                params![purge, tenant, target, state, mutation, record],
            )
            .map_err(|error| sql(&error))?;
    }
    // Each archived tenant event is inserted under its own permanent key, copied verbatim and
    // never recomputed: any archive of any installation that carries the event restores it
    // once, whatever sequence numbers the installations gave it.
    for (position, record) in manifest.events.iter().enumerate() {
        transaction
            .execute(
                "INSERT INTO events (tenant_id, workspace_id, event_key, record)
                 VALUES (?1, NULL, ?2, ?3) ON CONFLICT (event_key) DO NOTHING",
                params![tenant, event_key(manifest, &tenant, position)?, record],
            )
            .map_err(|error| sql(&error))?;
    }
    Ok(())
}

/// The key of the tenant event at `position` of `manifest`: the archived key; or, for an archive
/// without keys (written before they existed), the position in the tenant's log under the
/// source installation, the key records schema 2 gave every tenant event that existed before it
/// (`schema::RECORDS_V2`). A position is stable for one installation, whose tenant log is
/// append-only, but not across two installations restored from one set (the known limit of
/// archives without keys).
fn event_key(
    manifest: &InstallationManifest,
    tenant: &str,
    position: usize,
) -> Result<String, ApiError> {
    if manifest.event_keys.is_empty() {
        let source = manifest.identity.as_ref().map_or_else(
            || "no-identity".to_owned(),
            |(subject, created_at)| format!("{subject}/{created_at}"),
        );
        return Ok(format!("legacy/{source}/{tenant}/{position}"));
    }
    if manifest.event_keys.len() != manifest.events.len() {
        return Err(archive::invalid(
            "its events and their keys do not correspond",
        ));
    }
    manifest
        .event_keys
        .get(position)
        .cloned()
        .ok_or_else(|| archive::invalid("an event has no key"))
}

pub(crate) mod archive;
pub(crate) mod fence;
mod managed;
mod restore;
pub(crate) mod snapshot;

/// The name of the test-only pause point in `write_workspace_archive` (`crate::pause`), after
/// its first checks and before it takes the workspace's lock: a test holds the next archive
/// call of a workspace there, so a second call under the same mutation can run to completion
/// first, deterministically. It names the data directory (a temporary directory unique to each
/// test) and the workspace, so parallel tests whose workspaces share an identity never meet.
#[cfg(test)]
fn pause_point(storage: &Storage, scope: &StorageScope) -> String {
    format!(
        "archive|{}|{}",
        storage.data.root().display(),
        scope.workspace_id.0
    )
}

/// The name of the test-only pause point (`crate::pause`) of a workspace archive under
/// `mutation`: after it is recorded, before it releases the workspace's lock.
#[cfg(test)]
fn recorded_point(storage: &Storage, mutation: MutationId) -> String {
    format!(
        "archive-recorded|{}|{}",
        storage.data.root().display(),
        mutation.0
    )
}

/// Test-only publication boundary: bytes retained, before an installation archive is recorded.
#[cfg(test)]
fn installation_stored_point(storage: &Storage, mutation: MutationId) -> String {
    format!(
        "installation-stored|{}|{}",
        storage.data.root().display(),
        mutation.0
    )
}
/// Unit tests here, not in `tests/`: they hold a workspace's write lock, which is private.
/// They return `ApiError` like core's unit tests.
#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use okf_jawn_contract::access::AccessRoute;
    use okf_jawn_contract::error::{ApiError, ErrorCode};
    use okf_jawn_contract::identity::{MutationId, TenantId};
    use okf_jawn_core::jobs::RecordStore;
    use okf_jawn_core::storage::{
        Backups, NewWorkspace, ObjectInfo, Provenance, StorageScope, WorkspaceCatalog,
    };

    use crate::storage::Storage;

    fn fault(message: &dyn std::fmt::Display) -> ApiError {
        ApiError::new(ErrorCode::Internal, message.to_string())
    }

    /// A workspace "Notes" of tenant `local` created under mutation 1.
    async fn notes(storage: &Storage) -> Result<StorageScope, ApiError> {
        let tenant = TenantId::try_from("local".to_owned()).map_err(|error| fault(&error))?;
        let created = storage
            .catalog()
            .create(
                &tenant,
                MutationId(uuid::Uuid::from_u128(1)),
                NewWorkspace {
                    name: "Notes".to_owned(),
                    description: String::new(),
                    creator: Provenance {
                        subject: "ana".to_owned(),
                        route: AccessRoute::LocalOwner,
                        client_id: None,
                    },
                },
            )
            .await?;
        Ok(StorageScope {
            tenant_id: tenant,
            workspace_id: created.id,
        })
    }

    /// Read the small private fixture's stored digest files independently of the root reader.
    fn sweep_publication_objects(
        storage: &Storage,
        tenant: &TenantId,
    ) -> Result<Vec<okf_jawn_contract::identity::Digest>, ApiError> {
        let mut found = Vec::new();
        for shard in std::fs::read_dir(storage.data().objects_path().join(tenant.as_str()))
            .map_err(|error| fault(&error))?
        {
            let shard = shard.map_err(|error| fault(&error))?;
            for entry in std::fs::read_dir(shard.path()).map_err(|error| fault(&error))? {
                let entry = entry.map_err(|error| fault(&error))?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| fault(&"fixture object filename is not UTF-8"))?;
                found.push(
                    okf_jawn_contract::identity::Digest::try_from(name)
                        .map_err(|error| fault(&error))?,
                );
            }
        }
        Ok(found)
    }

    /// DesignV6.2: a retained installation archive has both a durable pending row and a live
    /// tenant pin before its referencing record exists. Both captured values are literal.
    #[tokio::test(flavor = "multi_thread")]
    async fn sweep_publication_installation_archive_is_protected_before_its_record()
    -> Result<(), ApiError> {
        let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
        let storage = Storage::open(directory.path())?;
        let scope = notes(&storage).await?;
        let mutation = MutationId(uuid::Uuid::from_u128(40));
        let (arrival, release) =
            crate::pause::hold(super::installation_stored_point(&storage, mutation))?;
        let first = {
            let (backups, tenant) = (storage.backups(), scope.tenant_id.clone());
            tokio::spawn(async move { backups.write_installation_archive(&tenant, mutation).await })
        };
        crate::pause::arrived(arrival).await?;
        let objects = sweep_publication_objects(&storage, &scope.tenant_id)?;
        let first_object = objects
            .first()
            .ok_or_else(|| fault(&"the retained installation object"))?;
        let (watch, touched) = storage
            .blobs
            .pins()
            .sweep(scope.tenant_id.as_str(), &scope.workspace_id.0.to_string())?;
        let key = mutation.0.to_string();
        let pending: i64 = storage.records.call(move |connection| connection.query_row(
            "SELECT count(*) FROM pending_puts WHERE tenant_id='local' AND workspace_id IS NULL AND mutation_id=?1",
            [key], |row| row.get(0),
        ).map_err(|error| fault(&error))).await?;
        let pinned = touched.contains(first_object);
        drop(watch);
        // Release and join before assertions so a behavioral RED leaves no blocked writer.
        release.send(()).map_err(|error| fault(&error))?;
        let recorded = first.await.map_err(|error| fault(&error))??;
        assert_eq!(objects.len(), 1);
        assert_eq!((pending, pinned), (1, true));
        assert_eq!(&recorded.digest, first_object);
        Ok(())
    }

    /// DesignV6.1: a stop after retention must be adopted under the same mutation, even after
    /// the installation manifest changed. The failed attempt's one object is the result.
    #[tokio::test(flavor = "multi_thread")]
    async fn sweep_publication_retry_adopts_the_first_unrecorded_installation_archive()
    -> Result<(), ApiError> {
        let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
        let storage = Storage::open(directory.path())?;
        let scope = notes(&storage).await?;
        let mutation = MutationId(uuid::Uuid::from_u128(40));
        let (arrival, release) =
            crate::pause::hold(super::installation_stored_point(&storage, mutation))?;
        let first = {
            let (backups, tenant) = (storage.backups(), scope.tenant_id.clone());
            tokio::spawn(async move { backups.write_installation_archive(&tenant, mutation).await })
        };
        crate::pause::arrived(arrival).await?;
        let objects = sweep_publication_objects(&storage, &scope.tenant_id)?;
        drop(release); // The existing pause returns an error after bytes, before recording.
        let stopped = first.await.map_err(|error| fault(&error))?;
        assert!(
            stopped.is_err(),
            "the injected stop must not record an archive"
        );
        assert_eq!(objects.len(), 1);
        storage
            .records()
            .create_purge(
                &scope.tenant_id,
                MutationId(uuid::Uuid::from_u128(80)),
                okf_jawn_core::jobs::NewPurge {
                    id: okf_jawn_contract::identity::PurgeId(uuid::Uuid::from_u128(81)),
                    target: okf_jawn_contract::purge::PurgeTarget::Workspace {
                        workspace_id: scope.workspace_id,
                    },
                    initiator: Provenance {
                        subject: "ana".to_owned(),
                        route: AccessRoute::LocalOwner,
                        client_id: None,
                    },
                },
            )
            .await?;
        let retried = storage
            .backups()
            .write_installation_archive(&scope.tenant_id, mutation)
            .await?;
        assert_eq!(
            Some(&retried.digest),
            objects.first(),
            "adopt the first bytes, not the changed manifest"
        );
        assert_eq!(
            sweep_publication_objects(&storage, &scope.tenant_id)?.len(),
            1
        );
        let counts: (i64, i64) = storage.records.call(move |connection| connection.query_row(
            "SELECT (SELECT count(*) FROM backup_archives WHERE mutation_id=?1), (SELECT count(*) FROM pending_puts WHERE workspace_id IS NULL AND mutation_id=?1)",
            [mutation.0.to_string()], |row| Ok((row.get(0)?,row.get(1)?)),
        ).map_err(|error| fault(&error))).await?;
        assert_eq!(counts, (1, 0));
        Ok(())
    }
    /// DesignV6.3: recording one archive lands only its own tenant pending puts, while the
    /// other mutation's retained-but-unrecorded archive remains durable and protected.
    #[tokio::test(flavor = "multi_thread")]
    async fn sweep_publication_one_archive_does_not_land_another_mutations_put()
    -> Result<(), ApiError> {
        let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
        let storage = Storage::open(directory.path())?;
        let scope = notes(&storage).await?;
        let first_id = MutationId(uuid::Uuid::from_u128(40));
        let second_id = MutationId(uuid::Uuid::from_u128(41));
        let (first_arrival, first_release) =
            crate::pause::hold(super::installation_stored_point(&storage, first_id))?;
        let (second_arrival, second_release) =
            crate::pause::hold(super::installation_stored_point(&storage, second_id))?;
        let first = {
            let (backups, tenant) = (storage.backups(), scope.tenant_id.clone());
            tokio::spawn(async move { backups.write_installation_archive(&tenant, first_id).await })
        };
        crate::pause::arrived(first_arrival).await?;
        let second = {
            let (backups, tenant) = (storage.backups(), scope.tenant_id.clone());
            tokio::spawn(
                async move { backups.write_installation_archive(&tenant, second_id).await },
            )
        };
        crate::pause::arrived(second_arrival).await?;
        first_release.send(()).map_err(|error| fault(&error))?;
        first.await.map_err(|error| fault(&error))??;
        let pending_other: i64 = storage.records.call(move |connection| connection.query_row(
            "SELECT count(*) FROM pending_puts WHERE tenant_id='local' AND workspace_id IS NULL AND mutation_id=?1",
            [second_id.0.to_string()], |row| row.get(0),
        ).map_err(|error| fault(&error))).await?;
        second_release.send(()).map_err(|error| fault(&error))?;
        second.await.map_err(|error| fault(&error))??;
        assert_eq!(
            pending_other, 1,
            "the other mutation remains pending until its own record"
        );
        let counts: (i64, i64) = storage.records.call(|connection| connection.query_row(
            "SELECT (SELECT count(*) FROM backup_archives),(SELECT count(*) FROM pending_puts WHERE workspace_id IS NULL)",
            [], |row| Ok((row.get(0)?,row.get(1)?)),
        ).map_err(|error| fault(&error))).await?;
        assert_eq!(counts, (2, 0));
        Ok(())
    }
    /// Backup re-review B1, deterministic: the first archive call under mutation 40 is held
    /// after its first checks (the pause point), a second call under the same mutation runs to
    /// completion and records its archive, a record of the workspace then changes, and the first
    /// is released. It re-reads the record under the lock and stores nothing: one archive object.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_held_archive_call_under_a_recorded_mutation_stores_nothing() -> Result<(), ApiError>
    {
        use okf_jawn_core::drafts::{DraftStore, DraftWrite};
        use okf_jawn_core::storage::VersionStore;
        let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
        let storage = Storage::open(directory.path())?;
        let scope = notes(&storage).await?;
        let mutation = MutationId(uuid::Uuid::from_u128(40));
        let (arrival, release) = crate::pause::hold(super::pause_point(&storage, &scope))?;
        let first = {
            let (backups, scope) = (storage.backups(), scope.clone());
            tokio::spawn(async move { backups.write_workspace_archive(&scope, mutation).await })
        };
        crate::pause::arrived(arrival).await?;
        let second = storage
            .backups()
            .write_workspace_archive(&scope, mutation)
            .await?;
        // A record of the workspace changes, so an archive written now would be another object.
        storage
            .drafts()
            .save(
                &scope,
                MutationId(uuid::Uuid::from_u128(41)),
                DraftWrite {
                    item_id: okf_jawn_contract::identity::ItemId(uuid::Uuid::from_u128(10)),
                    editor: "ana".to_owned(),
                    base_revision: storage.versions().head(&scope).await?,
                    body: "changed between the two lock holds".to_owned(),
                    properties: std::collections::BTreeMap::new(),
                    content_digest: okf_jawn_contract::identity::Digest::try_from("e".repeat(64))
                        .map_err(|error| fault(&error))?,
                },
            )
            .await?;
        release.send(()).map_err(|error| fault(&error))?;
        let first = first.await.map_err(|error| fault(&error))??;
        assert_eq!(first, second, "both answer the recorded archive");
        let mut pending = vec![storage.data().objects_path()];
        let mut stored = 0_usize;
        while let Some(path) = pending.pop() {
            for entry in std::fs::read_dir(&path).map_err(|error| fault(&error))? {
                let path = entry.map_err(|error| fault(&error))?.path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    stored = stored.saturating_add(1);
                }
            }
        }
        assert_eq!(stored, 1, "one archive object is stored");
        Ok(())
    }

    /// PR #20 finding (`CodeRabbit`, `backup/mod.rs:382`): a second record under the same mutation
    /// (two concurrent writes of one archive mutation) answers the archive recorded first, not
    /// the object it wrote itself.
    #[tokio::test]
    async fn a_second_record_under_one_mutation_answers_the_first_archive() -> Result<(), ApiError>
    {
        let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
        let storage = Storage::open(directory.path())?;
        let tenant = TenantId::try_from("local".to_owned()).map_err(|error| fault(&error))?;
        let object = |fill: &str, size: u64| -> Result<ObjectInfo, ApiError> {
            Ok(ObjectInfo {
                digest: okf_jawn_contract::identity::Digest::try_from(fill.repeat(64))
                    .map_err(|error| fault(&error))?,
                size,
            })
        };
        let mutation = MutationId(uuid::Uuid::from_u128(7));
        let first =
            super::record(&storage.records, mutation, &tenant, None, object("a", 1)?).await?;
        let second =
            super::record(&storage.records, mutation, &tenant, None, object("b", 2)?).await?;
        assert_eq!(first, object("a", 1)?);
        assert_eq!(
            second, first,
            "the recorded archive, not the second caller's"
        );
        Ok(())
    }

    /// Wait until a caller of `scope`'s write lock besides the test's own holder has taken the
    /// lock's handle: it passed every check it makes before the lock, and it waits for the lock.
    async fn until_waiting_for_the_lock(
        storage: &Storage,
        scope: &StorageScope,
    ) -> Result<(), ApiError> {
        let started = std::time::Instant::now();
        while storage.repositories.lock_users(scope)? < 2 {
            if started.elapsed() > Duration::from_secs(120) {
                return Err(fault(&"no write began to wait for the workspace's lock"));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(())
    }

    /// PR #20 finding (`CodeRabbit`, `backup/mod.rs:161`): the archive is recorded before the
    /// workspace's lock is released, so a purge that plans under the lock right after it finds
    /// the record. The archive is held at its pause point after its record, inside the lock:
    /// the lock is still its own, and the record is already there.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_workspace_archive_is_recorded_before_it_releases_the_lock() -> Result<(), ApiError> {
        let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
        let storage = Storage::open(directory.path())?;
        let scope = notes(&storage).await?;
        let mutation = MutationId(uuid::Uuid::from_u128(2));
        let (arrival, release) = crate::pause::hold(super::recorded_point(&storage, mutation))?;
        let writer = {
            let (backups, scope) = (storage.backups(), scope.clone());
            tokio::spawn(async move { backups.write_workspace_archive(&scope, mutation).await })
        };
        crate::pause::arrived(arrival).await?;
        let holding = storage.repositories.lock_users(&scope)?;
        let key = mutation.0.to_string();
        let recorded = storage
            .records
            .call(move |connection| {
                connection
                    .query_row(
                        "SELECT count(*) FROM backup_archives WHERE mutation_id = ?1",
                        [key],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(|error| fault(&error))
            })
            .await?;
        release.send(()).map_err(|error| fault(&error))?;
        writer.await.map_err(|error| fault(&error))??;
        assert_eq!(holding, 1, "the archive still holds the workspace's lock");
        assert_eq!(recorded, 1, "recorded while the lock was still held");
        Ok(())
    }

    /// PR #20 finding (`CodeRabbit`, `backup/mod.rs:161`): an archive that passed its first check
    /// and waits for the workspace's lock while the purge is recorded is refused under the lock.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_workspace_archive_waiting_for_the_lock_when_the_purge_is_recorded_is_refused()
    -> Result<(), ApiError> {
        let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
        let storage = Storage::open(directory.path())?;
        let scope = notes(&storage).await?;
        let (held, release) = (mpsc::channel(), mpsc::channel::<()>());
        let holder = {
            let repositories = storage.repositories.clone();
            let scope = scope.clone();
            std::thread::spawn(move || {
                repositories.locked(&scope, || {
                    held.0.send(()).map_err(|error| fault(&error))?;
                    release.1.recv().map_err(|error| fault(&error))
                })
            })
        };
        held.1.recv().map_err(|error| fault(&error))?;
        let writer = {
            let (backups, scope) = (storage.backups(), scope.clone());
            tokio::spawn(async move {
                backups
                    .write_workspace_archive(&scope, MutationId(uuid::Uuid::from_u128(2)))
                    .await
            })
        };
        // The archive passed its first check and waits for the lock.
        until_waiting_for_the_lock(&storage, &scope).await?;
        storage
            .records()
            .create_purge(
                &scope.tenant_id,
                MutationId(uuid::Uuid::from_u128(50)),
                okf_jawn_core::jobs::NewPurge {
                    id: okf_jawn_contract::identity::PurgeId(uuid::Uuid::from_u128(60)),
                    target: okf_jawn_contract::purge::PurgeTarget::Workspace {
                        workspace_id: scope.workspace_id,
                    },
                    initiator: Provenance {
                        subject: "ana".to_owned(),
                        route: AccessRoute::LocalOwner,
                        client_id: None,
                    },
                },
            )
            .await?;
        release.0.send(()).map_err(|error| fault(&error))?;
        holder
            .join()
            .map_err(|_| fault(&"the lock holder stopped"))??;
        let error = match writer.await.map_err(|error| fault(&error))? {
            Ok(object) => return Err(fault(&format!("the archive was written: {object:?}"))),
            Err(error) => error,
        };
        assert_eq!(error.code, ErrorCode::Conflict, "{error}");
        assert!(error.message.contains("purged by purge"), "{error}");
        Ok(())
    }

    /// Pass-4 re-review B4: the writer refuses a manifest a restore would refuse to read, so a
    /// written archive is never unrestorable: an installation archive whose manifest would pass
    /// 64 MiB fails with `Internal`, and no file is left.
    #[test]
    fn an_installation_archive_whose_manifest_passes_the_cap_is_not_written() -> Result<(), ApiError>
    {
        let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
        let target = directory.path().join("installation.zip");
        let manifest = super::archive::InstallationManifest {
            format: super::archive::INSTALLATION_FORMAT.to_owned(),
            version: super::archive::ARCHIVE_VERSION,
            events: vec!["e".repeat(65 << 20)],
            ..super::archive::InstallationManifest::default()
        };
        let error = match super::archive::write_installation(&target, &manifest) {
            Ok(()) => return Err(fault(&"the archive was written")),
            Err(error) => error,
        };
        assert_eq!(error.code, ErrorCode::Internal, "{error}");
        assert!(!target.exists(), "no archive file is left");
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn backup_a_workspace_archive_is_a_checkpoint_under_the_workspace_lock()
    -> Result<(), ApiError> {
        let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
        let storage = Storage::open(directory.path())?;
        let tenant = TenantId::try_from("local".to_owned()).map_err(|error| fault(&error))?;
        let mutation = |n: u128| MutationId(uuid::Uuid::from_u128(n));
        let created = storage
            .catalog()
            .create(
                &tenant,
                mutation(1),
                NewWorkspace {
                    name: "Notes".to_owned(),
                    description: String::new(),
                    creator: Provenance {
                        subject: "ana".to_owned(),
                        route: AccessRoute::LocalOwner,
                        client_id: None,
                    },
                },
            )
            .await?;
        let scope = StorageScope {
            tenant_id: tenant,
            workspace_id: created.id,
        };
        let staging = storage
            .data()
            .staging_path()
            .join("local")
            .join(scope.workspace_id.0.to_string())
            .join(mutation(2).0.to_string());
        let (held, release) = (mpsc::channel(), mpsc::channel::<()>());
        let holder = {
            let repositories = storage.repositories.clone();
            let scope = scope.clone();
            std::thread::spawn(move || {
                repositories.locked(&scope, || {
                    held.0.send(()).map_err(|error| fault(&error))?;
                    release.1.recv().map_err(|error| fault(&error))
                })
            })
        };
        held.1.recv().map_err(|error| fault(&error))?;
        let backups = storage.backups();
        let writer = {
            let scope = scope.clone();
            tokio::spawn(async move { backups.write_workspace_archive(&scope, mutation(2)).await })
        };
        // Give the archive ample time to start: under another write's lock it never does.
        // The staging directory appears within milliseconds of a start, so a wrong lock is
        // seen at once, not only when the whole archive finishes.
        for _ in 0..50 {
            if staging.exists() || writer.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            !staging.exists() && !writer.is_finished(),
            "no archive is staged while another write holds the workspace"
        );
        release.0.send(()).map_err(|error| fault(&error))?;
        holder
            .join()
            .map_err(|_| fault(&"the lock holder stopped"))??;
        writer.await.map_err(|error| fault(&error))??;
        assert!(
            !staging.exists(),
            "the staging directory is removed afterwards"
        );
        Ok(())
    }
}
