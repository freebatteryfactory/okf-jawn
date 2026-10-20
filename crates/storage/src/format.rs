//! Opening a SQLite database: refuse newer data, back up before an upgrade, migrate, verify.
//!
//! SPEC section 2: schemas migrate with `rusqlite_migration` (`user_version`, `to_latest`,
//! `pending_migrations`). A database whose version is newer than this build knows is refused
//! and left unchanged: the check opens it read-only. Before an upgrade of a database that
//! already holds a schema, a recoverable backup is written with `VACUUM INTO` and checked; then
//! the migration runs in one transaction; then the result is verified (no pending step,
//! `integrity_check`, `foreign_key_check`). A new, empty database needs no backup.
//!
//! The backup covers what the migration changes, the SQLite file. The managed pre-migration
//! backup of the whole installation (every workspace archive and the installation archive) is
//! written by `Backups` in storage M2 and joins this step there.

use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use rusqlite::{Connection, OpenFlags};
use rusqlite_migration::Migrations;

use crate::data::{DataDir, RECORDS_FILE, io_error};
use crate::schema;
use crate::seam;

/// An opened database at the latest schema version.
///
/// The connection is reachable only by borrowing it from this value, which borrows the locked
/// `DataDir`, so no connection that `migrate_database` opened outlives the single-writer lock:
///
/// ```compile_fail
/// # use okf_jawn_storage::{data::DataDir, format::migrate_database, schema};
/// # fn main() -> Result<(), okf_jawn_contract::error::ApiError> {
/// let data = DataDir::open(std::path::Path::new("data"))?;
/// let migrated = migrate_database(&data, "records.sqlite", &schema::RECORDS)?;
/// drop(data); // the lock is released while the connection is still borrowed
/// let deleted = migrated.connection().execute_batch("DELETE FROM jobs");
/// # drop(deleted);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Migrated<'data> {
    /// Connection to the migrated database.
    connection: Connection,
    /// The locked directory the database belongs to, borrowed for the connection's life.
    data: PhantomData<&'data DataDir>,
    /// Schema version found before migrating; 0 for a new database.
    pub from: usize,
    /// Schema version after migrating.
    pub to: usize,
    /// The verified backup written before an upgrade, if one ran.
    pub backup: Option<PathBuf>,
}

/// A records-specific preparation of the verified copy before its source is migrated.
type PrepareBackup = fn(&Path, usize) -> Result<(), ApiError>;

impl Migrated<'_> {
    /// The connection, borrowed: it lives no longer than the locked directory.
    #[must_use]
    pub const fn connection(&self) -> &Connection {
        &self.connection
    }

    /// The connection, for a store that keeps its own share of the lock (`Db::new`).
    pub(crate) fn into_connection(self) -> Connection {
        self.connection
    }
}

/// Open the database `file` of the locked `data` directory and bring it to the latest step of
/// `migrations`, backing it up under `data`'s pre-migration directory first.
///
/// Taking the opened `DataDir` is the proof that this process holds the directory's
/// single-writer lock, so nothing public opens a writable database without it.
///
/// # Errors
/// Returns `Unsupported`, changing nothing, when the database is newer than `migrations`;
/// `Unavailable` when the backup cannot be written or checked (nothing is migrated then);
/// `Internal` when a step fails (the transaction rolls back) or the migrated result does not
/// verify (the backup is kept and named).
pub fn migrate_database<'data>(
    data: &'data DataDir,
    file: &str,
    migrations: &Migrations<'_>,
) -> Result<Migrated<'data>, ApiError> {
    migrate_with_backup(data, file, migrations, None)
}

/// Migrate records after sanitizing the one pre-schema5 copy (lifecycle ruling9).
/// Generic/index migrations keep their original copy behavior.
pub(crate) fn migrate_records(data: &DataDir) -> Result<Migrated<'_>, ApiError> {
    migrate_with_backup(
        data,
        RECORDS_FILE,
        &schema::RECORDS,
        Some(sanitize_before_v5),
    )
}

/// The shared migration transaction, with an optional preparation only of its new copy.
fn migrate_with_backup<'data>(
    data: &'data DataDir,
    file: &str,
    migrations: &Migrations<'_>,
    prepare_backup: Option<PrepareBackup>,
) -> Result<Migrated<'data>, ApiError> {
    let path = data.root().join(file);
    let path = path.as_path();
    let backups = data.pre_migration_path();
    let backups = backups.as_path();
    if path.exists() {
        refuse_newer(path, migrations)?;
    }
    let mut connection = Connection::open(path).map_err(|error| open_error(path, &error))?;
    connection
        .execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
        .map_err(|error| open_error(path, &error))?;
    let from: usize = migrations
        .current_version(&connection)
        .map_err(|error| migration_error(path, &error))?
        .into();
    let pending = migrations
        .pending_migrations(&connection)
        .map_err(|error| migration_error(path, &error))?;
    let backup = if pending > 0 && from > 0 {
        let copy = back_up(&connection, path, from, backups)?;
        if let Some(prepare) = prepare_backup {
            prepare(&copy, from)?;
        }
        Some(copy)
    } else {
        None
    };
    migrations
        .to_latest(&mut connection)
        .map_err(|error| migration_error(path, &error))?;
    verify(&connection, migrations, path, backup.as_deref())?;
    let to: usize = migrations
        .current_version(&connection)
        .map_err(|error| migration_error(path, &error))?
        .into();
    Ok(Migrated {
        connection,
        data: PhantomData,
        from,
        to,
        backup,
    })
}

/// Whether an existing database holds a schema that `migrations` would upgrade.
///
/// # Errors
/// Returns `Unavailable` or `Internal` when its version cannot be read. Nothing is written.
pub fn needs_upgrade(path: &Path, migrations: &Migrations<'_>) -> Result<bool, ApiError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| open_error(path, &error))?;
    let from: usize = migrations
        .current_version(&connection)
        .map_err(|error| migration_error(path, &error))?
        .into();
    let pending = migrations
        .pending_migrations(&connection)
        .map_err(|error| migration_error(path, &error))?;
    Ok(from > 0 && pending > 0)
}

/// Refuse, through a read-only connection, a database newer than `migrations`.
///
/// # Errors
/// Returns `Unsupported` for a newer database, and `Unavailable` or `Internal` when its
/// version cannot be read. Nothing is written in any case.
pub fn refuse_newer(path: &Path, migrations: &Migrations<'_>) -> Result<(), ApiError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| open_error(path, &error))?;
    let pending = migrations
        .pending_migrations(&connection)
        .map_err(|error| migration_error(path, &error))?;
    if pending < 0 {
        let found: usize = migrations
            .current_version(&connection)
            .map_err(|error| migration_error(path, &error))?
            .into();
        let ahead = usize::try_from(pending.unsigned_abs()).unwrap_or(usize::MAX);
        return Err(ApiError::new(
            ErrorCode::Unsupported,
            format!(
                "{} has schema version {found}, newer than version {} this build knows; it \
                 was left unchanged",
                path.display(),
                found.saturating_sub(ahead),
            ),
        ));
    }
    Ok(())
}

/// Write a checked `VACUUM INTO` copy of the database before an upgrade.
fn back_up(
    connection: &Connection,
    path: &Path,
    from: usize,
    backups: &Path,
) -> Result<PathBuf, ApiError> {
    std::fs::create_dir_all(backups).map_err(|error| io_error("create", backups, &error))?;
    let stem = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("database");
    let target = backups.join(format!("{stem}-v{from}-{}.sqlite", seam::now_ms()?));
    let target_text = target.to_str().ok_or_else(|| {
        ApiError::new(
            ErrorCode::Unavailable,
            format!("the backup path {} is not UTF-8", target.display()),
        )
    })?;
    connection
        .execute("VACUUM INTO ?1", [target_text])
        .map_err(|error| backup_error(&target, &error.to_string()))?;
    verify_copy(&target, from)?;
    Ok(target)
}

/// Check the copy both before and after records-only sanitation; its released schema stays.
fn verify_copy(target: &Path, from: usize) -> Result<(), ApiError> {
    let copy = Connection::open_with_flags(target, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| backup_error(target, &error.to_string()))?;
    let integrity: String = copy
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| backup_error(target, &error.to_string()))?;
    let version: i64 = copy
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| backup_error(target, &error.to_string()))?;
    if integrity != "ok" || usize::try_from(version).ok() != Some(from) {
        return Err(backup_error(
            target,
            &format!("the copy did not verify (integrity {integrity}, version {version})"),
        ));
    }
    Ok(())
}

/// Empty recorded purged-workspace request digests in this newly written pre-V5 copy only.
/// No installation predates schema2 (ruling9); older released fixtures and later copies are
/// unchanged. Secure deletion plus VACUUM removes old cell/free-page bytes as well as SQL values.
fn sanitize_before_v5(target: &Path, from: usize) -> Result<(), ApiError> {
    if !(2..=4).contains(&from) {
        return Ok(());
    }
    let copy = Connection::open_with_flags(target, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|error| backup_error(target, &error.to_string()))?;
    copy.execute_batch(
        "PRAGMA secure_delete = ON;
         BEGIN IMMEDIATE;
         UPDATE mutations SET state = 'released', response = NULL, digest = ''
         WHERE state IN ('completed', 'released')
           AND operation NOT IN ('purge_workspace', 'purge_item')
           AND EXISTS (SELECT 1 FROM purges
               WHERE purges.tenant_id = mutations.tenant_id
                 AND purges.target_key = 'workspace/' || mutations.workspace_id);
         COMMIT;
         VACUUM;",
    )
    .map_err(|error| backup_error(target, &error.to_string()))?;
    drop(copy);
    verify_copy(target, from)
}

/// Check the migrated database: nothing pending, intact, no dangling reference.
fn verify(
    connection: &Connection,
    migrations: &Migrations<'_>,
    path: &Path,
    backup: Option<&Path>,
) -> Result<(), ApiError> {
    let kept = backup.map_or_else(String::new, |backup| {
        format!("; the pre-migration backup is {}", backup.display())
    });
    let fault = |what: String| {
        ApiError::new(
            ErrorCode::Internal,
            format!(
                "{} did not verify after migrating: {what}{kept}",
                path.display()
            ),
        )
    };
    let pending = migrations
        .pending_migrations(connection)
        .map_err(|error| fault(error.to_string()))?;
    if pending != 0 {
        return Err(fault(format!("{pending} steps still pending")));
    }
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| fault(error.to_string()))?;
    if integrity != "ok" {
        return Err(fault(integrity));
    }
    let dangling: i64 = connection
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .map_err(|error| fault(error.to_string()))?;
    if dangling != 0 {
        return Err(fault(format!("{dangling} rows reference a missing row")));
    }
    Ok(())
}

fn open_error(path: &Path, error: &rusqlite::Error) -> ApiError {
    ApiError::new(
        ErrorCode::Unavailable,
        format!("could not open {}: {error}", path.display()),
    )
}

fn migration_error(path: &Path, error: &rusqlite_migration::Error) -> ApiError {
    ApiError::new(
        ErrorCode::Internal,
        format!("could not migrate {}: {error}", path.display()),
    )
}

fn backup_error(target: &Path, message: &str) -> ApiError {
    ApiError::new(
        ErrorCode::Unavailable,
        format!(
            "the pre-migration backup {} failed, so nothing was migrated: {message}",
            target.display()
        ),
    )
}
