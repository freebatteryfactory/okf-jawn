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
//! entry streams back without a decoder. Reading takes entries by name only, after `Opened::open`
//! has checked every entry against its bounds (an archive may be a caller's upload).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::Path;

use git2::{Oid, Repository};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, TenantId, WorkspaceId};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
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
    /// Every object copied under `objects/`. This build writes it empty and reads the list from
    /// the archive's `objects/<digest>` entries (`Opened::objects`), so a manifest's size does
    /// not grow with a workspace's objects.
    #[serde(default)]
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
    /// Each event's permanent key (`events.event_key`), in the order of `events`, copied
    /// verbatim: a restore inserts each event once under it, whatever archive file or
    /// installation carries it. Empty in an archive written before keys existed (or from a
    /// records database older than them); a restore then keys each event by its position.
    #[serde(default)]
    pub(crate) event_keys: Vec<String>,
}

/// An opened archive.
pub(crate) struct Opened {
    zip: ZipArchive<File>,
}

/// A sink that keeps nothing it is given, and hashes it when it holds a hasher.
struct Hashing(Option<Sha256>);

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
/// The largest manifest read (`manifest.json`, `installation.json`), 64 MiB. A manifest is read
/// whole into memory; the largest real one is an installation archive's, whose tenant events
/// (a few hundred bytes each) would reach this only past about 200 000 events.
const MANIFEST_LIMIT: u64 = 64 << 20;
/// How many times its own size an archive may expand to in all (`expansion_bound`).
const EXPANSION: u64 = 8;
/// Bytes every archive may expand to beyond `EXPANSION` times its size, for a small archive.
const EXPANSION_SLACK: u64 = 1 << 20;
impl Opened {
    /// Open an archive file and check every entry before anything is read from it: a stored or
    /// deflated entry only, each manifest within `MANIFEST_LIMIT`, every entry's decompressed
    /// size exactly what it declares, all of them together within the expansion bound of the
    /// file (`expansion_bound`), and each `objects/<digest>` entry's bytes of that SHA-256
    /// digest. An archive may be a caller's upload, so nothing in it is trusted to be small or
    /// honest: the check streams each entry once, counting (and hashing an object), and stores
    /// nothing.
    ///
    /// # Errors
    /// Returns `InvalidInput` for an archive that fails a check.
    pub(crate) fn open(path: &Path) -> Result<Self, ApiError> {
        let file = File::open(path).map_err(|error| io_error("read", path, &error))?;
        let size = file
            .metadata()
            .map_err(|error| io_error("read", path, &error))?
            .len();
        let mut zip = ZipArchive::new(file).map_err(|error| invalid(&error.to_string()))?;
        let budget = expansion_bound(size);
        let mut total: u64 = 0;
        for index in 0..zip.len() {
            let mut entry = zip
                .by_index(index)
                .map_err(|error| invalid(&error.to_string()))?;
            let name = entry.name().to_owned();
            if !matches!(
                entry.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            ) {
                return Err(invalid(&format!(
                    "{name} uses a compression method this build does not read"
                )));
            }
            let declared = entry.size();
            let bound = if name == MANIFEST || name == INSTALLATION {
                MANIFEST_LIMIT
            } else {
                budget
            };
            if declared > bound {
                return Err(invalid(&format!(
                    "{name} declares {declared} bytes, more than the {bound} it may hold"
                )));
            }
            total = total.saturating_add(declared);
            if total > budget {
                return Err(invalid(&format!(
                    "its entries declare more than {budget} bytes in all, for a file of {size}"
                )));
            }
            // An object entry is hashed as it is counted: its bytes must be its digest.
            let digest = name
                .strip_prefix("objects/")
                .and_then(|digest| Digest::try_from(digest.to_owned()).ok());
            let mut sink = Hashing(digest.as_ref().map(|_| Sha256::new()));
            let actual = std::io::copy(
                &mut (&mut entry).take(declared.saturating_add(1)),
                &mut sink,
            )
            .map_err(|error| invalid(&format!("{name}: {error}")))?;
            if actual != declared {
                return Err(invalid(&format!(
                    "{name} declares {declared} bytes and holds a different amount"
                )));
            }
            if let (Some(digest), Some(hasher)) = (digest, sink.0)
                && crate::seam::hex(&hasher.finalize()) != digest.as_str()
            {
                return Err(invalid(&format!(
                    "{name} holds bytes whose SHA-256 is not its digest"
                )));
            }
        }
        Ok(Self { zip })
    }

    /// The objects the archive holds: the digest of each `objects/<digest>` entry. The entries,
    /// not the manifest, list them (`WorkspaceManifest::objects`).
    ///
    /// # Errors
    /// Returns `InvalidInput` for an `objects/` entry whose name is not a digest.
    pub(crate) fn objects(&self) -> Result<Vec<Digest>, ApiError> {
        self.zip
            .file_names()
            .filter_map(|name| name.strip_prefix("objects/"))
            .map(|digest| {
                Digest::try_from(digest.to_owned())
                    .map_err(|_| invalid(&format!("objects/{digest} is not an object's digest")))
            })
            .collect()
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

    /// Copy one entry to `target`; never more than the size it declares, which `open` checked.
    pub(crate) fn extract(&mut self, name: &str, target: &Path) -> Result<(), ApiError> {
        let entry = self
            .zip
            .by_name(name)
            .map_err(|error| invalid(&format!("{name}: {error}")))?;
        let declared = entry.size();
        let mut file = File::create(target).map_err(|error| io_error("create", target, &error))?;
        std::io::copy(&mut entry.take(declared), &mut file)
            .map_err(|error| io_error("write", target, &error))?;
        file.sync_all()
            .map_err(|error| io_error("sync", target, &error))
    }

    /// The bytes of a manifest entry, never more than `MANIFEST_LIMIT`.
    fn bytes(&mut self, name: &str) -> Result<Vec<u8>, ApiError> {
        let entry = self
            .zip
            .by_name(name)
            .map_err(|error| invalid(&format!("{name}: {error}")))?;
        let mut bytes = Vec::new();
        entry
            .take(MANIFEST_LIMIT.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| invalid(&format!("{name}: {error}")))?;
        if u64::try_from(bytes.len()).map_or(true, |read| read > MANIFEST_LIMIT) {
            return Err(invalid(&format!(
                "{name} is larger than {MANIFEST_LIMIT} bytes"
            )));
        }
        Ok(bytes)
    }
}

impl std::io::Write for Hashing {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(hasher) = &mut self.0 {
            hasher.update(bytes);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// How many decompressed bytes an archive file of `size` bytes may hold in all: `EXPANSION`
/// times its size, plus `EXPANSION_SLACK`. This build writes every entry stored, so its own
/// archives hold about their size; the factor leaves room for an archive deflated by another
/// tool and refuses a bomb, whose ratio is in the hundreds or more.
fn expansion_bound(size: u64) -> u64 {
    size.saturating_mul(EXPANSION)
        .saturating_add(EXPANSION_SLACK)
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
    let required = refuse_missing(repository, records, blobs, tenant, excluded)?;
    let staged_pack = target.with_extension("pack");
    let references = write_pack(repository, &staged_pack)?;
    let mut manifest = manifest_of(records, tenant, references)?;
    let history = references::history(repository)?;
    let mut objects = history.objects;
    objects.extend(record_objects(records)?);
    references::follow_records(blobs, tenant, &mut objects, &history.records)?;
    // Every object a row names is copied, whatever found it.
    objects.extend(required.keys().cloned());
    let mut held = Vec::new();
    for digest in objects {
        if !excluded.contains(&digest) && blobs.local_path(tenant, &digest)?.is_file() {
            held.push(digest);
        }
    }
    // The archive copies every required object, not only the store holding it: an archive
    // that claims success is independently recoverable.
    if let Some((digest, what)) = required
        .iter()
        .find(|&(digest, _)| !excluded.contains(digest) && !held.contains(digest))
    {
        let _ = std::fs::remove_file(&staged_pack);
        return Err(ApiError::new(
            ErrorCode::Internal,
            format!(
                "the workspace archive was not written: {what} names the object {}, which it \
                 would not copy",
                digest.as_str()
            ),
        ));
    }
    // The archive's `objects/<digest>` entries list its objects; the manifest does not repeat
    // them, so its size does not grow with the workspace's objects (`MANIFEST_LIMIT`).
    let manifest_text = {
        let mut written = manifest.clone();
        written.objects = Vec::new();
        serde_json::to_vec_pretty(&written).map_err(|error| crate::db::json(&error))?
    };
    if let Err(error) = refuse_large_manifest(&manifest_text) {
        let _ = std::fs::remove_file(&staged_pack);
        return Err(error);
    }
    manifest.objects.clone_from(&held);
    let file = File::create(target).map_err(|error| io_error("create", target, &error))?;
    let mut zip = ZipWriter::new(file);
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
    let text = serde_json::to_vec_pretty(manifest).map_err(|error| crate::db::json(&error))?;
    refuse_large_manifest(&text)?;
    let file = File::create(target).map_err(|error| io_error("create", target, &error))?;
    let mut zip = ZipWriter::new(file);
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
        event_keys: Vec::new(),
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
    // A records database older than event keys (the pre-migration set reads one) has none: its
    // archive is written without keys, as an archive of that build was.
    let keyed: bool = connection
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM pragma_table_info('events') WHERE name = 'event_key')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    let key = if keyed { "event_key" } else { "NULL" };
    let mut statement = connection
        .prepare(&format!(
            "SELECT {key}, record FROM events WHERE tenant_id = ?1 AND workspace_id IS NULL
             ORDER BY seq"
        ))
        .map_err(|error| sql(&error))?;
    let rows: Vec<(Option<String>, String)> = statement
        .query_map([tenant.as_str()], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|error| sql(&error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    let (keys, events): (Vec<Option<String>>, Vec<String>) = rows.into_iter().unzip();
    let event_keys = keys.into_iter().flatten().collect();
    Ok(InstallationManifest {
        format: INSTALLATION_FORMAT.to_owned(),
        version: ARCHIVE_VERSION,
        tenant_id: Some(tenant.clone()),
        identity,
        connectors,
        purges,
        events,
        event_keys,
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

/// Refuse to write an archive that would leave out an object a row it holds names (SPEC section
/// 12: an archive must be independently recoverable): a source card's original or conversion
/// record (on any commit), a conversion record's structured exports and assets, a retained
/// artifact (an export, a View export), a derived dataset, a completed upload or a job output.
/// Backup and written-history objects (`excluded`) are never archived and are not required.
/// Objects named only as digest-shaped tokens are copied when held and are not required: a token
/// need not name an object. Each named object is checked as it is read, so what this holds is
/// bounded by the objects the store holds, not by a conversion record's size. Blocking.
///
/// # Errors
/// Returns `Internal` naming the missing digest and the row kind that names it; nothing is
/// written. Otherwise the required objects the store holds (all of them), each with what first
/// names it.
fn refuse_missing(
    repository: &Repository,
    records: &Path,
    blobs: &LocalBlobs,
    tenant: &TenantId,
    excluded: &BTreeSet<Digest>,
) -> Result<BTreeMap<Digest, String>, ApiError> {
    let mut required: BTreeMap<Digest, String> = BTreeMap::new();
    let mut require = |digest: Digest, what: &str| -> Result<(), ApiError> {
        if excluded.contains(&digest) || required.contains_key(&digest) {
            return Ok(());
        }
        if !blobs.local_path(tenant, &digest)?.is_file() {
            return Err(ApiError::new(
                ErrorCode::Internal,
                format!(
                    "the workspace archive was not written: {what} names the object {}, which \
                     the store does not hold",
                    digest.as_str()
                ),
            ));
        }
        required.insert(digest, what.to_owned());
        Ok(())
    };
    let (originals, conversions) = references::card_objects(repository)?;
    for digest in originals {
        require(digest, "a source card's original")?;
    }
    for record in &conversions {
        references::conversion_objects(blobs, tenant, record, &mut |digest, what| {
            require(digest, what)
        })?;
    }
    for record in conversions {
        require(record, "a source card's conversion record")?;
    }
    let connection = super::snapshot::read_only(records)?;
    for (query, what) in [
        ("SELECT digest, kind FROM artifacts", "artifact"),
        (
            "SELECT digest, NULL FROM derived_objects",
            "a derived dataset",
        ),
        (
            "SELECT object_digest, NULL FROM uploads WHERE object_digest IS NOT NULL",
            "a completed upload",
        ),
        ("SELECT digest, NULL FROM job_outputs", "a job output"),
    ] {
        let mut statement = connection.prepare(query).map_err(|error| sql(&error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .map_err(|error| sql(&error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| sql(&error))?;
        for (digest, kind) in rows {
            let what = kind.map_or_else(
                || what.to_owned(),
                |kind| format!("an {what} of kind {kind}"),
            );
            require(from_text!(digest.as_str())?, &what)?;
        }
    }
    Ok(required)
}

/// Refuse to write a manifest a restore would refuse to read (`MANIFEST_LIMIT`): the backup
/// fails loudly rather than writing an archive no restore reads.
///
/// # Errors
/// Returns `Internal`; nothing is written.
fn refuse_large_manifest(text: &[u8]) -> Result<(), ApiError> {
    let size = u64::try_from(text.len()).unwrap_or(u64::MAX);
    if size > MANIFEST_LIMIT {
        return Err(ApiError::new(
            ErrorCode::Internal,
            format!(
                "the archive was not written: its manifest is {size} bytes, more than the \
                 {MANIFEST_LIMIT} a restore reads"
            ),
        ));
    }
    Ok(())
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
