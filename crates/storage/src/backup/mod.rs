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

    /// The replacement restore: the installation archive first (identity, purge records,
    /// tenant events under their own keys, connectors restored as revoked, since no secret is
    /// kept), then each workspace archive under its own identity. Run it with no service on the
    /// data directory. Run again (after a stop in a later workspace, say), it accepts and leaves
    /// as it is a workspace this same archive file already restored in full, unless a purge of
    /// it is now recorded.
    ///
    /// # Errors
    /// Returns `InvalidInput` for an archive this build cannot read or of another tenant, and
    /// `Conflict` for a workspace any recorded purge names (finished or not) or that exists and
    /// was not restored from this same archive.
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
        self.storage
            .records
            .transaction(move |transaction| install(transaction, &manifest))
            .await?;
        let mut reports = Vec::new();
        for path in workspaces {
            let mut opened = archive::Opened::open(path)?;
            let scope = StorageScope {
                tenant_id: tenant
                    .clone()
                    .ok_or_else(|| archive::invalid("the installation archive names no tenant"))?,
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
                let mode = Mode::Replace(crate::blobs::hash_file(&path)?);
                std::fs::create_dir_all(&staging)
                    .map_err(|error| io_error("create", &staging, &error))?;
                let restored = restore::restore(&storage, &runtime, &scope, &path, &staging, &mode);
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
        crate::purge::guard::refuse_scope(&self.storage.records, scope).await?;
        #[cfg(test)]
        pause::point(&self.storage, scope).await?;
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
                    return Ok(object);
                }
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
                let object = runtime.block_on(storage.blobs.put_for(
                    &scope.tenant_id,
                    file_reader(Some(zip.clone()), 0, size),
                    size,
                    None,
                ))?;
                runtime.block_on(record(
                    &storage.records,
                    mutation,
                    &scope.tenant_id,
                    Some(scope),
                    object,
                ))
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
            .put_for(tenant, file_reader(Some(zip), 0, size), size, None)
            .await;
        let _ = std::fs::remove_dir_all(&directory);
        record(&self.storage.records, mutation, tenant, None, object?).await
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
mod restore;
pub(crate) mod snapshot;

/// A test-only pause point in `write_workspace_archive`, after its first checks and before it
/// takes the workspace's lock: a test holds the next archive call of a workspace there, so a
/// second call under the same mutation can run to completion first, deterministically.
#[cfg(test)]
mod pause {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, Sender, channel};

    use okf_jawn_contract::error::ApiError;
    use okf_jawn_core::storage::StorageScope;

    use crate::db::internal;
    use crate::storage::Storage;

    /// The held workspace's arrival signal and release, by data directory and workspace.
    type Held = BTreeMap<String, (Sender<()>, Receiver<()>)>;

    static HELD: Mutex<Held> = Mutex::new(BTreeMap::new());

    /// The held call's key: the data directory (a temporary directory unique to each test) and
    /// the workspace, so parallel tests whose workspaces share an identity never meet.
    fn key(storage: &Storage, scope: &StorageScope) -> String {
        format!("{}|{}", storage.data.root().display(), scope.workspace_id.0)
    }

    /// Hold the next archive call of `scope`; returns (told when it arrives, its release).
    pub(super) fn hold(
        storage: &Storage,
        scope: &StorageScope,
    ) -> Result<(Receiver<()>, Sender<()>), ApiError> {
        let (arrived, arrival) = channel();
        let (release, released) = channel();
        HELD.lock()
            .map_err(|_| internal("the pause table was poisoned"))?
            .insert(key(storage, scope), (arrived, released));
        Ok((arrival, release))
    }

    /// Where an archive call of `scope` waits while a test holds it (once per hold).
    pub(super) async fn point(storage: &Storage, scope: &StorageScope) -> Result<(), ApiError> {
        let held = HELD
            .lock()
            .map_err(|_| internal("the pause table was poisoned"))?
            .remove(&key(storage, scope));
        if let Some((arrived, released)) = held {
            arrived
                .send(())
                .map_err(|error| internal(error.to_string()))?;
            tokio::task::spawn_blocking(move || released.recv())
                .await
                .map_err(|error| internal(error.to_string()))?
                .map_err(|error| internal(error.to_string()))?;
        }
        Ok(())
    }
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
        let (arrival, release) = super::pause::hold(&storage, &scope)?;
        let first = {
            let (backups, scope) = (storage.backups(), scope.clone());
            tokio::spawn(async move { backups.write_workspace_archive(&scope, mutation).await })
        };
        tokio::task::spawn_blocking(move || arrival.recv())
            .await
            .map_err(|error| fault(&error))?
            .map_err(|error| fault(&error))?;
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

    /// PR #20 finding (`CodeRabbit`, `backup/mod.rs:161`): the archive is recorded before the
    /// workspace's lock is released, so a purge that plans under the lock right after it finds
    /// the record. The test takes the lock as soon as the archive lets it go (the archive is seen
    /// to hold it by its staging directory) and reads the record there.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_workspace_archive_is_recorded_before_it_releases_the_lock() -> Result<(), ApiError> {
        let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
        let storage = Storage::open(directory.path())?;
        let scope = notes(&storage).await?;
        let mutation = MutationId(uuid::Uuid::from_u128(2));
        let staging = storage
            .data()
            .staging_path()
            .join("local")
            .join(scope.workspace_id.0.to_string())
            .join(mutation.0.to_string());
        let writer = {
            let (backups, scope) = (storage.backups(), scope.clone());
            tokio::spawn(async move { backups.write_workspace_archive(&scope, mutation).await })
        };
        let started = std::time::Instant::now();
        while !staging.exists() {
            if writer.is_finished() || started.elapsed() > Duration::from_secs(60) {
                return Err(fault(&"the archive was never seen holding the lock"));
            }
            tokio::task::yield_now().await;
        }
        let recorded = {
            let (storage, scope) = (storage.clone(), scope.clone());
            tokio::task::spawn_blocking(move || {
                storage.repositories.locked(&scope, || {
                    let key = mutation.0.to_string();
                    tokio::runtime::Handle::current().block_on(storage.records.call(
                        move |connection| {
                            connection
                                .query_row(
                                    "SELECT count(*) FROM backup_archives WHERE mutation_id = ?1",
                                    [key],
                                    |row| row.get::<_, i64>(0),
                                )
                                .map_err(|error| fault(&error))
                        },
                    ))
                })
            })
            .await
            .map_err(|error| fault(&error))??
        };
        writer.await.map_err(|error| fault(&error))??;
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
        // The archive passes its first check at once and then waits for the lock (if it did
        // not get there first, it is refused by that first check instead).
        tokio::time::sleep(Duration::from_millis(500)).await;
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
