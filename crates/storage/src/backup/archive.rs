//! The two archive formats (Stage 1b design section 8), written and read with `zip`.
//!
//! A **workspace archive** holds `manifest.json` (format, version, tenant, workspace, catalog
//! entry, references, objects), `records.sqlite` (the filtered snapshot, `snapshot`),
//! `history.pack` (a Git pack, written by libgit2's `PackBuilder`, of every commit on every
//! reference: the accepted line and each proposal candidate) and `objects/<digest>` (a copy of
//! every retained object the workspace references, never a reference into the live store, and
//! never an export, backup or written-history object).
//!
//! An **installation archive** holds only `installation.json`: the installation identity,
//! connector rows without any secret or secret hash, purge records and tenant-level events. It
//! holds no workspace content.
//!
//! Entries are stored, not compressed: originals are usually compressed already, and a stored
//! entry streams back without a decoder. Reading takes entries by name only.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::Path;

use git2::{Oid, Repository};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, TenantId, WorkspaceId};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::blobs::LocalBlobs;
use crate::data::io_error;
use crate::db::sql;
use crate::git::repo::git;
use crate::purge::references;

/// What a workspace archive says about itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WorkspaceManifest {
    pub(crate) format: String,
    pub(crate) version: u32,
    pub(crate) tenant_id: TenantId,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) created_at: String,
    pub(crate) archived_at: Option<String>,
    /// Every reference and the commit it names.
    pub(crate) references: Vec<(String, String)>,
    /// Every object copied under `objects/`.
    pub(crate) objects: Vec<Digest>,
}

/// What an installation archive holds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct InstallationManifest {
    pub(crate) format: String,
    pub(crate) version: u32,
    pub(crate) tenant_id: Option<TenantId>,
    /// The local identity: (subject, created at).
    pub(crate) identity: Option<(String, String)>,
    /// (connector id, mutation id, record): no secret and no secret hash.
    pub(crate) connectors: Vec<(String, String, String)>,
    /// (purge id, target key, state, mutation id, record).
    pub(crate) purges: Vec<(String, String, String, String, String)>,
    /// Tenant-level event records, oldest first.
    pub(crate) events: Vec<String>,
    /// Each event's sequence number in the source installation, in the order of `events`: with
    /// the source identity it names the event, so a restore inserts each event once whatever
    /// archive file carries it.
    #[serde(default)]
    pub(crate) event_seqs: Vec<i64>,
}

/// An opened archive.
pub(crate) struct Opened {
    zip: ZipArchive<File>,
}

/// The format name of a workspace archive.
pub(crate) const WORKSPACE_FORMAT: &str = "okf-jawn-workspace-archive";
/// The format name of an installation archive.
pub(crate) const INSTALLATION_FORMAT: &str = "okf-jawn-installation-archive";
/// The archive format version this build writes and reads.
pub(crate) const ARCHIVE_VERSION: u32 = 1;
/// The manifest entry of a workspace archive.
pub(crate) const MANIFEST: &str = "manifest.json";
/// The records entry of a workspace archive.
pub(crate) const RECORDS: &str = "records.sqlite";
/// The history entry of a workspace archive.
pub(crate) const PACK: &str = "history.pack";
/// The only entry of an installation archive.
pub(crate) const INSTALLATION: &str = "installation.json";

impl Opened {
    /// Open an archive file.
    pub(crate) fn open(path: &Path) -> Result<Self, ApiError> {
        let file = File::open(path).map_err(|error| io_error("read", path, &error))?;
        let zip = ZipArchive::new(file).map_err(|error| invalid(&error.to_string()))?;
        Ok(Self { zip })
    }

    /// The workspace manifest, checked.
    pub(crate) fn workspace(&mut self) -> Result<WorkspaceManifest, ApiError> {
        let manifest: WorkspaceManifest = serde_json::from_slice(&self.bytes(MANIFEST)?)
            .map_err(|error| invalid(&error.to_string()))?;
        if manifest.format != WORKSPACE_FORMAT || manifest.version != ARCHIVE_VERSION {
            return Err(invalid("it is not a workspace archive this build reads"));
        }
        Ok(manifest)
    }

    /// The installation manifest, checked.
    pub(crate) fn installation(&mut self) -> Result<InstallationManifest, ApiError> {
        let manifest: InstallationManifest = serde_json::from_slice(&self.bytes(INSTALLATION)?)
            .map_err(|error| invalid(&error.to_string()))?;
        if manifest.format != INSTALLATION_FORMAT || manifest.version != ARCHIVE_VERSION {
            return Err(invalid(
                "it is not an installation archive this build reads",
            ));
        }
        Ok(manifest)
    }

    /// Copy one entry to `target`.
    pub(crate) fn extract(&mut self, name: &str, target: &Path) -> Result<(), ApiError> {
        let mut entry = self
            .zip
            .by_name(name)
            .map_err(|error| invalid(&format!("{name}: {error}")))?;
        let mut file = File::create(target).map_err(|error| io_error("create", target, &error))?;
        std::io::copy(&mut entry, &mut file).map_err(|error| io_error("write", target, &error))?;
        file.sync_all()
            .map_err(|error| io_error("sync", target, &error))
    }

    fn bytes(&mut self, name: &str) -> Result<Vec<u8>, ApiError> {
        let mut entry = self
            .zip
            .by_name(name)
            .map_err(|error| invalid(&format!("{name}: {error}")))?;
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| invalid(&format!("{name}: {error}")))?;
        Ok(bytes)
    }
}

/// Write the workspace archive `target` from the repository at `repository`, the filtered
/// records at `records` and the tenant's objects; `excluded` objects are never copied.
pub(crate) fn write_workspace(
    target: &Path,
    repository: &Repository,
    records: &Path,
    blobs: &LocalBlobs,
    tenant: &TenantId,
    excluded: &BTreeSet<Digest>,
) -> Result<WorkspaceManifest, ApiError> {
    let staged_pack = target.with_extension("pack");
    let references = write_pack(repository, &staged_pack)?;
    let mut manifest = manifest_of(records, tenant, references)?;
    let mut objects = references::history(repository)?.objects;
    objects.extend(record_objects(records)?);
    references::follow_records(blobs, tenant, &mut objects)?;
    let mut held = Vec::new();
    for digest in objects {
        if !excluded.contains(&digest) && blobs.local_path(tenant, &digest)?.is_file() {
            held.push(digest);
        }
    }
    manifest.objects.clone_from(&held);
    let file = File::create(target).map_err(|error| io_error("create", target, &error))?;
    let mut zip = ZipWriter::new(file);
    let manifest_text =
        serde_json::to_vec_pretty(&manifest).map_err(|error| crate::db::json(&error))?;
    entry_bytes(&mut zip, MANIFEST, &manifest_text)?;
    entry_file(&mut zip, RECORDS, records)?;
    entry_file(&mut zip, PACK, &staged_pack)?;
    for digest in &held {
        entry_file(
            &mut zip,
            &format!("objects/{}", digest.as_str()),
            &blobs.local_path(tenant, digest)?,
        )?;
    }
    finish(zip, target)?;
    std::fs::remove_file(&staged_pack).map_err(|error| io_error("remove", &staged_pack, &error))?;
    Ok(manifest)
}

/// Write `target` as a copy of the workspace archive `source` whose records entry is the file
/// `records`; every other entry is copied as it is stored. Blocking.
pub(crate) fn replace_records(
    source: &Path,
    records: &Path,
    target: &Path,
) -> Result<(), ApiError> {
    let mut opened = Opened::open(source)?;
    let file = File::create(target).map_err(|error| io_error("create", target, &error))?;
    let mut zip = ZipWriter::new(file);
    for index in 0..opened.zip.len() {
        let entry = opened
            .zip
            .by_index_raw(index)
            .map_err(|error| invalid(&error.to_string()))?;
        if entry.name() == RECORDS {
            continue;
        }
        let name = entry.name().to_owned();
        zip.raw_copy_file(entry)
            .map_err(|error| zip_error(&name, &error))?;
    }
    entry_file(&mut zip, RECORDS, records)?;
    finish(zip, target)
}

/// Write an installation archive `target` holding `manifest`: a tenant's
/// (`installation_of`) or the installation-level one (`installation_level_of`).
pub(crate) fn write_installation(
    target: &Path,
    manifest: &InstallationManifest,
) -> Result<(), ApiError> {
    let file = File::create(target).map_err(|error| io_error("create", target, &error))?;
    let mut zip = ZipWriter::new(file);
    let text = serde_json::to_vec_pretty(manifest).map_err(|error| crate::db::json(&error))?;
    entry_bytes(&mut zip, INSTALLATION, &text)?;
    finish(zip, target)
}

/// Read the installation records of `tenant` from a records database.
/// The installation-level manifest: the installation identity and the connectors, which carry no
/// tenant, with no tenant, purge or event. Every backup set holds one, whatever tenants exist.
pub(crate) fn installation_level_of(
    connection: &Connection,
) -> Result<InstallationManifest, ApiError> {
    let identity = connection
        .query_row("SELECT subject, created_at FROM installation", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(sql(&other)),
        })?;
    let connectors = rows3(
        connection,
        "SELECT connector_id, mutation_id, record FROM connectors ORDER BY rowid",
        params![],
    )?;
    Ok(InstallationManifest {
        format: INSTALLATION_FORMAT.to_owned(),
        version: ARCHIVE_VERSION,
        tenant_id: None,
        identity,
        connectors,
        purges: Vec::new(),
        events: Vec::new(),
        event_seqs: Vec::new(),
    })
}

/// The installation archive of `tenant`: the installation-level rows, and the tenant's purge
/// records and tenant events.
pub(crate) fn installation_of(
    connection: &Connection,
    tenant: &TenantId,
) -> Result<InstallationManifest, ApiError> {
    let InstallationManifest {
        identity,
        connectors,
        ..
    } = installation_level_of(connection)?;
    let mut statement = connection
        .prepare(
            "SELECT purge_id, target_key, state, mutation_id, record FROM purges
             WHERE tenant_id = ?1 ORDER BY rowid",
        )
        .map_err(|error| sql(&error))?;
    let purges = statement
        .query_map([tenant.as_str()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .map_err(|error| sql(&error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    let mut statement = connection
        .prepare(
            "SELECT seq, record FROM events WHERE tenant_id = ?1 AND workspace_id IS NULL
             ORDER BY seq",
        )
        .map_err(|error| sql(&error))?;
    let rows: Vec<(i64, String)> = statement
        .query_map([tenant.as_str()], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|error| sql(&error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    let (event_seqs, events) = rows.into_iter().unzip();
    Ok(InstallationManifest {
        format: INSTALLATION_FORMAT.to_owned(),
        version: ARCHIVE_VERSION,
        tenant_id: Some(tenant.clone()),
        identity,
        connectors,
        purges,
        events,
        event_seqs,
    })
}

/// Write a pack of every commit on every reference to `target`; returns the references.
fn write_pack(repository: &Repository, target: &Path) -> Result<Vec<(String, String)>, ApiError> {
    let mut walk = repository.revwalk().map_err(|error| git(&error))?;
    let mut references = Vec::new();
    for reference in repository.references().map_err(|error| git(&error))? {
        let reference = reference.map_err(|error| git(&error))?;
        if let (Ok(name), Some(target)) = (reference.name(), reference.target())
            && repository.find_commit(target).is_ok()
        {
            walk.push(target).map_err(|error| git(&error))?;
            references.push((name.to_owned(), target.to_string()));
        }
    }
    references.sort();
    let mut pack = repository.packbuilder().map_err(|error| git(&error))?;
    pack.set_threads(1);
    pack.insert_walk(&mut walk).map_err(|error| git(&error))?;
    let mut file = File::create(target).map_err(|error| io_error("create", target, &error))?;
    let mut failure = None;
    pack.foreach(|chunk| match file.write_all(chunk) {
        Ok(()) => true,
        Err(error) => {
            failure = Some(error);
            false
        }
    })
    .map_err(|error| {
        failure.take().map_or_else(
            || git(&error),
            |failure| io_error("write", target, &failure),
        )
    })?;
    file.sync_all()
        .map_err(|error| io_error("sync", target, &error))?;
    Ok(references)
}

/// Index a pack into `repository`'s object database.
pub(crate) fn read_pack(repository: &Repository, pack: &Path) -> Result<(), ApiError> {
    let odb = repository.odb().map_err(|error| git(&error))?;
    let mut writer = odb.packwriter().map_err(|error| git(&error))?;
    let mut file = File::open(pack).map_err(|error| io_error("read", pack, &error))?;
    std::io::copy(&mut file, &mut writer).map_err(|error| io_error("index", pack, &error))?;
    writer.commit().map_err(|error| git(&error))?;
    Ok(())
}

/// A commit id of a manifest.
pub(crate) fn oid(text: &str) -> Result<Oid, ApiError> {
    Oid::from_str(text).map_err(|error| invalid(&error.to_string()))
}

/// A refused archive.
pub(crate) fn invalid(message: &str) -> ApiError {
    ApiError::new(
        ErrorCode::InvalidInput,
        format!("the archive cannot be restored: {message}"),
    )
    .with_field("/upload_id")
}

/// The manifest's catalog entry, read from the filtered records.
fn manifest_of(
    records: &Path,
    tenant: &TenantId,
    references: Vec<(String, String)>,
) -> Result<WorkspaceManifest, ApiError> {
    let connection = super::snapshot::read_only(records)?;
    let (workspace, name, description, created_at, archived_at): (
        String,
        String,
        String,
        String,
        Option<String>,
    ) = connection
        .query_row(
            "SELECT workspace_id, name, description, created_at, archived_at FROM workspaces",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(|error| sql(&error))?;
    Ok(WorkspaceManifest {
        format: WORKSPACE_FORMAT.to_owned(),
        version: ARCHIVE_VERSION,
        tenant_id: tenant.clone(),
        workspace_id: from_text!(workspace.as_str())?,
        name,
        description,
        created_at,
        archived_at,
        references,
        objects: Vec::new(),
    })
}

/// Objects the filtered records reference: derived datasets, completed uploads, job outputs and
/// retained artifacts (the filtered records hold no workspace archive).
fn record_objects(records: &Path) -> Result<BTreeSet<Digest>, ApiError> {
    let connection = super::snapshot::read_only(records)?;
    let mut found = BTreeSet::new();
    for query in [
        "SELECT digest FROM artifacts",
        "SELECT digest FROM derived_objects",
        "SELECT object_digest FROM uploads WHERE object_digest IS NOT NULL",
        "SELECT digest FROM job_outputs",
    ] {
        let mut statement = connection.prepare(query).map_err(|error| sql(&error))?;
        let values = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| sql(&error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| sql(&error))?;
        for value in values {
            found.insert(from_text!(value.as_str())?);
        }
    }
    Ok(found)
}

fn rows3(
    connection: &Connection,
    query: &str,
    values: &[&dyn rusqlite::ToSql],
) -> Result<Vec<(String, String, String)>, ApiError> {
    let mut statement = connection.prepare(query).map_err(|error| sql(&error))?;
    statement
        .query_map(values, |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|error| sql(&error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))
}

fn options() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(true)
}

fn entry_bytes(zip: &mut ZipWriter<File>, name: &str, bytes: &[u8]) -> Result<(), ApiError> {
    zip.start_file(name, options())
        .map_err(|error| zip_error(name, &error))?;
    zip.write_all(bytes)
        .map_err(|error| zip_error(name, &error))
}

fn entry_file(zip: &mut ZipWriter<File>, name: &str, source: &Path) -> Result<(), ApiError> {
    zip.start_file(name, options())
        .map_err(|error| zip_error(name, &error))?;
    let mut file = File::open(source).map_err(|error| io_error("read", source, &error))?;
    std::io::copy(&mut file, zip).map_err(|error| zip_error(name, &error))?;
    Ok(())
}

fn finish(zip: ZipWriter<File>, target: &Path) -> Result<(), ApiError> {
    let file = zip
        .finish()
        .map_err(|error| zip_error("the archive", &error))?;
    file.sync_all()
        .map_err(|error| io_error("sync", target, &error))
}

fn zip_error(name: &str, error: &dyn std::fmt::Display) -> ApiError {
    ApiError::new(
        ErrorCode::Unavailable,
        format!("could not write {name} into an archive: {error}"),
    )
}
