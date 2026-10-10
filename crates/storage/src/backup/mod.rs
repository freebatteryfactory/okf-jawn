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
//! A pre-migration backup set is one directory under `<data>/backups/pre-migration`, holding
//! `installation/<tenant>.zip` and `workspaces/<tenant>/<workspace>.zip`. A purge deletes a
//! workspace's archive from every set (`purge::copies`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, MutationId, TenantId};
use okf_jawn_contract::workspace::RestoreReport;
use okf_jawn_core::credentials::secret_hash;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{Backups, ObjectInfo, ObjectRead, StorageScope};
use rusqlite::{Connection, OptionalExtension, params};
use tokio::runtime::Handle;

use crate::blobs::{LocalBlobs, file_reader};
use crate::data::{DataDir, io_error};
use crate::db::{internal, json, not_found, sql, to_i64, to_u64};
use crate::git::repo::Repositories;
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

impl StorageBackups {
    pub(crate) const fn new(storage: Storage) -> Self {
        Self { storage }
    }

    /// The offline backup: every installation archive and every workspace archive (of `only`,
    /// when given) into `output`, as `installation/<tenant>.zip` and
    /// `workspaces/<tenant>/<workspace>.zip`. Run it with no service on the data directory.
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

    /// The replacement restore: the installation archive first (identity, purge records,
    /// tenant events, connectors restored as revoked, since no secret is kept), then each
    /// workspace archive under its own identity. Run it with no service on the data directory.
    ///
    /// # Errors
    /// Returns `InvalidInput` for an archive this build cannot read or of another tenant, and
    /// `Conflict` for a workspace that exists or that a completed purge removed.
    pub async fn restore_replacement(
        &self,
        installation: &Path,
        workspaces: &[PathBuf],
    ) -> Result<Vec<RestoreReport>, ApiError> {
        let manifest = archive::Opened::open(installation)?.installation()?;
        let tenant = manifest
            .tenant_id
            .clone()
            .ok_or_else(|| archive::invalid("the installation archive names no tenant"))?;
        self.storage
            .records
            .transaction(move |transaction| install(transaction, &manifest))
            .await?;
        let mut reports = Vec::new();
        for path in workspaces {
            let mut opened = archive::Opened::open(path)?;
            let scope = StorageScope {
                tenant_id: tenant.clone(),
                workspace_id: opened.workspace()?.workspace_id,
            };
            let storage = self.storage.clone();
            let runtime = Handle::current();
            let path = path.clone();
            let staging = self
                .storage
                .data
                .staging_path()
                .join(format!("restore-{}", scope.workspace_id.0));
            let report = tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(&staging)
                    .map_err(|error| io_error("create", &staging, &error))?;
                let restored =
                    restore::restore(&storage, &runtime, &scope, &path, &staging, &Mode::Replace);
                let _ = std::fs::remove_dir_all(&staging);
                restored.map(|(report, _)| report)
            })
            .await
            .map_err(|error| internal(format!("a restore task stopped: {error}")))??;
            reports.push(report);
        }
        Ok(reports)
    }

    async fn workspace_archive(
        &self,
        scope: &StorageScope,
        mutation: MutationId,
    ) -> Result<ObjectInfo, ApiError> {
        if let Some(object) = self.recorded(mutation).await? {
            return Ok(object);
        }
        let storage = self.storage.clone();
        let runtime = Handle::current();
        let locked_scope = scope.clone();
        let object = tokio::task::spawn_blocking(move || {
            let scope = &locked_scope;
            storage.repositories.locked(scope, || {
                let staging = storage.repositories.stage(scope, mutation)?;
                let zip = staging.path.join("archive.zip");
                let directory = staging.path.clone();
                let full = runtime.block_on(
                    storage
                        .records
                        .call(move |connection| snapshot::copy_live(connection, &directory)),
                )?;
                workspace_zip(
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
                runtime.block_on(storage.blobs.put_for(
                    &scope.tenant_id,
                    file_reader(Some(zip.clone()), 0, size),
                    size,
                    None,
                ))
            })
        })
        .await
        .map_err(|error| internal(format!("a backup task stopped: {error}")))??;
        self.record(mutation, &scope.tenant_id, Some(scope), object)
            .await
    }

    async fn installation_archive(
        &self,
        tenant: &TenantId,
        mutation: MutationId,
    ) -> Result<ObjectInfo, ApiError> {
        if let Some(object) = self.recorded(mutation).await? {
            return Ok(object);
        }
        let manifest = {
            let tenant = tenant.clone();
            self.storage
                .records
                .call(move |connection| archive::installation_of(connection, &tenant))
                .await?
        };
        let directory = self
            .storage
            .data
            .staging_path()
            .join(INSTALLATIONS)
            .join(mutation.0.to_string());
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
            .put_for(tenant, file_reader(Some(zip), 0, size), size, None)
            .await;
        let _ = std::fs::remove_dir_all(&directory);
        self.record(mutation, tenant, None, object?).await
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
                    &Mode::Import(editors),
                )
                .map(|(report, _)| report)
            })
        })
        .await
        .map_err(|error| internal(format!("a restore task stopped: {error}")))??;
        let (tenant, workspace) = crate::db::scope_key(scope);
        let text = serde_json::to_string(&report).map_err(|error| json(&error))?;
        self.storage
            .records
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "INSERT INTO restores (mutation_id, tenant_id, workspace_id, report)
                         VALUES (?1, ?2, ?3, ?4) ON CONFLICT (mutation_id) DO NOTHING",
                        params![mutation.0.to_string(), tenant, workspace, text],
                    )
                    .map_err(|error| sql(&error))
            })
            .await?;
        Ok(report)
    }

    async fn recorded(&self, mutation: MutationId) -> Result<Option<ObjectInfo>, ApiError> {
        let key = mutation.0.to_string();
        self.storage
            .records
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

    async fn record(
        &self,
        mutation: MutationId,
        tenant: &TenantId,
        scope: Option<&StorageScope>,
        object: ObjectInfo,
    ) -> Result<ObjectInfo, ApiError> {
        let tenant = tenant.as_str().to_owned();
        let workspace = scope.map(|scope| scope.workspace_id.0.to_string());
        let key = mutation.0.to_string();
        self.storage
            .records
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
                Ok(object)
            })
            .await
    }
}

impl Backups for StorageBackups {
    fn write_workspace_archive<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
    ) -> PortFuture<'a, ObjectInfo> {
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
    let mut statement = connection
        .prepare(
            "SELECT tenant_id, workspace_id FROM workspaces
             UNION SELECT tenant_id, NULL FROM purges ORDER BY 1, 2",
        )
        .map_err(|error| sql(&error))?;
    let rows: Vec<(String, Option<String>)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|error| sql(&error))?
        .collect::<Result<_, _>>()
        .map_err(|error| sql(&error))?;
    let mut written = Vec::new();
    let mut tenants = BTreeSet::new();
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
    Ok(written)
}

/// Filter a full copy to the workspace and write its archive to `target`. Blocking.
fn workspace_zip(
    repositories: &Repositories,
    blobs: &LocalBlobs,
    scope: &StorageScope,
    full: &snapshot::FullCopy,
    directory: &Path,
    target: &Path,
) -> Result<(), ApiError> {
    let (tenant, workspace) = crate::db::scope_key(scope);
    let excluded = snapshot::archive_objects(full, &tenant)?;
    let records = snapshot::filter(full, directory, &tenant, &workspace)?;
    let repository = repositories.open(scope)?;
    archive::write_workspace(
        target,
        &repository,
        &records,
        blobs,
        &scope.tenant_id,
        &excluded,
    )?;
    Ok(())
}

/// Restore the installation records of a replacement restore.
fn install(
    transaction: &rusqlite::Transaction<'_>,
    manifest: &InstallationManifest,
) -> Result<(), ApiError> {
    let tenant = manifest
        .tenant_id
        .as_ref()
        .map(|tenant| tenant.as_str().to_owned())
        .ok_or_else(|| archive::invalid("the installation archive names no tenant"))?;
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
    for record in &manifest.events {
        transaction
            .execute(
                "INSERT INTO events (tenant_id, workspace_id, replay_key, record)
                 VALUES (?1, NULL, NULL, ?2)",
                params![tenant, record],
            )
            .map_err(|error| sql(&error))?;
    }
    Ok(())
}

mod archive;
mod restore;
pub(crate) mod snapshot;
