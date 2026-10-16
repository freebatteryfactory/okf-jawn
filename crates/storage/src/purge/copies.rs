//! The managed pre-migration backups a purge must reach (SPEC section 2, Stage 1b design
//! section 6: "its archive inside every pre-migration backup storage retains ... is a managed
//! backup too").
//!
//! `<data>/backups/pre-migration` holds the `VACUUM INTO` copy of each database taken before
//! it was upgraded (`format::migrate_database`), and one directory per pre-migration backup
//! set (`backup::PRE_MIGRATION_WORKSPACES`), whose workspace archives are
//! `workspaces/<tenant>/<workspace>.zip`. For one workspace:
//!
//! - a records copy that holds anything of it is rewritten: its rows are removed and scrubbed
//!   exactly as in the live database (`rows::remove`), then the copy is vacuumed so the removed
//!   bytes are not left in free pages, and checked. A copy older than records schema 2 has a
//!   mutation ledger that records no workspace, so it holds the workspace while any completed
//!   row of the tenant but a purge's keeps a response, and the rewrite empties them all;
//! - an index copy (or any other database copy) that holds it is deleted: the index is rebuilt
//!   from Git, so the copy is not needed to restore anything;
//! - its archive in every backup set is deleted;
//! - another workspace's archive (in a backup set, or in the object store) whose records still
//!   cite it is rewritten with those records scrubbed (`rewrite_archive`).
//!
//! The purge reads these again on every attempt and once more before it completes, so a copy
//! written after the purge started (an upgrade after a failed attempt) is reached too.
//!
//! Another workspace's archive whose records cannot be read stops the purge (contract ruling
//! 14): guessing past it could leave a citation of the purged workspace in it. The error keeps
//! the reader's code and names the archive by identity, its workspace and its backup set or
//! object digest, never by path; asking for the purge again once the archive is repaired
//! resumes it (ingest records every failure of a workspace purge as retryable).

use std::path::{Path, PathBuf};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::PurgeId;
use rusqlite::{Connection, OpenFlags, TransactionBehavior};

use super::rows;
use crate::backup::PRE_MIGRATION_WORKSPACES;
use crate::backup::archive::{Opened, RECORDS, replace_records};
use crate::data::io_error;
use crate::db::sql;

/// What a purge does to the pre-migration backups, as paths relative to their directory.
#[derive(Debug, Default)]
pub(crate) struct CopyPlan {
    /// Records copies to rewrite without the workspace.
    pub(crate) rewrite: Vec<String>,
    /// Copies and archives to delete.
    pub(crate) remove: Vec<String>,
}

/// Another workspace's archive in a pre-migration backup set.
#[derive(Debug)]
pub(crate) struct OtherArchive {
    /// Its path relative to the pre-migration directory.
    pub(crate) name: String,
    /// What names it by identity in an error: its workspace and its backup set.
    pub(crate) survivor: String,
}

/// Find the pre-migration backups under `directory` that hold the workspace. Blocking.
pub(crate) fn plan(directory: &Path, tenant: &str, workspace: &str) -> Result<CopyPlan, ApiError> {
    let mut plan = CopyPlan::default();
    for (name, path) in entries(directory)? {
        if path.is_dir() {
            let archives = path.join(PRE_MIGRATION_WORKSPACES).join(tenant);
            for (archive, file) in entries(&archives)? {
                if file.file_stem().and_then(std::ffi::OsStr::to_str) == Some(workspace) {
                    plan.remove.push(format!(
                        "{name}/{PRE_MIGRATION_WORKSPACES}/{tenant}/{archive}"
                    ));
                }
            }
        } else if Path::new(&name)
            .extension()
            .is_some_and(|extension| extension == "sqlite")
        {
            let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|error| sql(&error))?;
            if rows::holds(&connection, tenant, workspace)? {
                if name.starts_with("records") {
                    plan.rewrite.push(name);
                } else {
                    plan.remove.push(name);
                }
            }
        }
    }
    Ok(plan)
}

/// The archives of the tenant's other workspaces in every pre-migration backup set under
/// `directory`. Blocking.
pub(crate) fn other_archives(
    directory: &Path,
    tenant: &str,
    workspace: &str,
) -> Result<Vec<OtherArchive>, ApiError> {
    let mut found = Vec::new();
    for (name, path) in entries(directory)? {
        if !path.is_dir() {
            continue;
        }
        let archives = path.join(PRE_MIGRATION_WORKSPACES).join(tenant);
        for (archive, file) in entries(&archives)? {
            // `entries` lists names that are UTF-8 only, so a listed file has a stem.
            if file.is_file()
                && file.extension().is_some_and(|extension| extension == "zip")
                && let Some(owner) = file.file_stem().and_then(std::ffi::OsStr::to_str)
                && owner != workspace
            {
                found.push(OtherArchive {
                    name: format!("{name}/{PRE_MIGRATION_WORKSPACES}/{tenant}/{archive}"),
                    survivor: format!(
                        "workspace {owner}'s archive in pre-migration backup set {name}"
                    ),
                });
            }
        }
    }
    Ok(found)
}

/// Write `rewritten` as the workspace archive `archive` with its records scrubbed of the
/// workspace exactly as the live records are (`rewrite`), when those records still hold
/// anything of it (a citation, a ledger response); returns whether it did. Another workspace's
/// archive holds no row of the purged one, so this scrubs citations: restoring the archive must
/// not bring back a citation of purged content. `staging` is a private directory. `survivor`
/// names the archive by identity: when its records cannot be read (opened, extracted or
/// queried), the purge stops with `unreadable`. Blocking.
pub(crate) fn rewrite_archive(
    archive: &Path,
    rewritten: &Path,
    staging: &Path,
    tenant: &str,
    workspace: &str,
    purge: PurgeId,
    survivor: &str,
) -> Result<bool, ApiError> {
    std::fs::create_dir_all(staging).map_err(|error| io_error("create", staging, &error))?;
    let records = staging.join(RECORDS);
    remove(&records)?;
    let holds = archive_holds(archive, &records, tenant, workspace)
        .map_err(|error| unreadable(survivor, &error))?;
    if holds {
        rewrite(&records, tenant, workspace, purge)?;
        replace_records(archive, &records, rewritten)?;
    }
    remove(&records)?;
    Ok(holds)
}

/// Extract the records of `archive` to `records` and say whether they hold anything of the
/// workspace. Blocking.
fn archive_holds(
    archive: &Path,
    records: &Path,
    tenant: &str,
    workspace: &str,
) -> Result<bool, ApiError> {
    Opened::open(archive)?.extract(RECORDS, records)?;
    let connection = Connection::open_with_flags(records, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| sql(&error))?;
    rows::holds(&connection, tenant, workspace)
}

/// The error a purge stops with when another workspace's archive cannot be read (contract
/// ruling 14): the reader's code kept, the archive named by `survivor`, its identity, and
/// neither the reader's message nor a path, which could name a file of the data directory.
fn unreadable(survivor: &str, error: &ApiError) -> ApiError {
    ApiError::new(
        error.code.clone(),
        format!(
            "the purge cannot read the records of {survivor}; repair that archive, then ask for \
             the purge again"
        ),
    )
}

/// Rewrite one records copy without the workspace, vacuum it and check it. Blocking.
pub(crate) fn rewrite(
    path: &Path,
    tenant: &str,
    workspace: &str,
    purge: PurgeId,
) -> Result<(), ApiError> {
    if !path.exists() {
        return Ok(());
    }
    let mut connection = Connection::open(path).map_err(|error| sql(&error))?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql(&error))?;
    rows::remove(&transaction, tenant, workspace, purge)?;
    transaction.commit().map_err(|error| sql(&error))?;
    vacuum(&connection)?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| sql(&error))?;
    if integrity != "ok" {
        return Err(ApiError::new(
            ErrorCode::Internal,
            format!("a rewritten pre-migration backup did not verify: {integrity}"),
        ));
    }
    Ok(())
}

/// Delete one copy or archive (and a database's side files); a missing one is already gone.
pub(crate) fn remove(path: &Path) -> Result<(), ApiError> {
    let mut paths = vec![path.to_path_buf()];
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        paths.push(PathBuf::from(side));
    }
    for path in paths {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("remove", &path, &error)),
        }
    }
    Ok(())
}

/// Rebuild a database without free pages, so removed rows leave no bytes behind, and fold
/// its write-ahead log back into the file.
pub(crate) fn vacuum(connection: &Connection) -> Result<(), ApiError> {
    connection
        .execute_batch("VACUUM")
        .map_err(|error| sql(&error))?;
    let busy: i64 = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
        .map_err(|error| sql(&error))?;
    if busy != 0 {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "a database's write-ahead log could not be folded back after a purge",
        ));
    }
    Ok(())
}

/// The entries of a directory as (name, path), in name order; none when it is absent.
fn entries(directory: &Path) -> Result<Vec<(String, PathBuf)>, ApiError> {
    let listing = match std::fs::read_dir(directory) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error("read", directory, &error)),
    };
    let mut found = Vec::new();
    for entry in listing {
        let entry = entry.map_err(|error| io_error("read", directory, &error))?;
        if let Ok(name) = entry.file_name().into_string() {
            found.push((name, entry.path()));
        }
    }
    found.sort();
    Ok(found)
}
