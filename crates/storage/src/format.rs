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

use std::path::{Path, PathBuf};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use rusqlite::{Connection, OpenFlags};
use rusqlite_migration::Migrations;

use crate::data::{DataDir, io_error};
use crate::seam;

/// An opened database at the latest schema version.
#[derive(Debug)]
pub struct Migrated {
    /// Connection to the migrated database.
    pub connection: Connection,
    /// Schema version found before migrating; 0 for a new database.
    pub from: usize,
    /// Schema version after migrating.
    pub to: usize,
    /// The verified backup written before an upgrade, if one ran.
    pub backup: Option<PathBuf>,
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
pub fn migrate_database(
    data: &DataDir,
    file: &str,
    migrations: &Migrations<'_>,
) -> Result<Migrated, ApiError> {
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
        Some(back_up(&connection, path, from, backups)?)
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
        from,
        to,
        backup,
    })
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
    let copy = Connection::open_with_flags(&target, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| backup_error(&target, &error.to_string()))?;
    let integrity: String = copy
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| backup_error(&target, &error.to_string()))?;
    let version: i64 = copy
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| backup_error(&target, &error.to_string()))?;
    if integrity != "ok" || usize::try_from(version).ok() != Some(from) {
        return Err(backup_error(
            &target,
            &format!("the copy did not verify (integrity {integrity}, version {version})"),
        ));
    }
    Ok(target)
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
