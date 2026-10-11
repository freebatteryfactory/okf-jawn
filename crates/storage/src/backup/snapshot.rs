//! The records part of a workspace archive: a `VACUUM INTO` snapshot filtered to one workspace.
//!
//! Stage 1b design section 8: "a `VACUUM INTO` snapshot of the SQLite records filtered to that
//! workspace before it is published (temporary file, integrity check, publish on success)". The
//! live database is copied with `VACUUM INTO` into a private file; every table not listed in
//! `WORKSPACE_TABLES` is emptied and the listed ones keep only the workspace's rows; the result
//! is vacuumed again into a second file, so nothing removed is left in free pages; that file is
//! checked (`integrity_check`, `foreign_key_check`, schema version) before the caller publishes
//! it.
//!
//! Every records table is in exactly one of the two lists (a test pins this), so a new table is
//! a decision, not a silent inclusion.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::Digest;
use okf_jawn_contract::workspace::ArtifactKind;
use rusqlite::{Connection, OpenFlags, params};

use crate::data::io_error;
use crate::db::sql;

/// A copy of the live records database, before it is filtered.
pub(crate) struct FullCopy {
    pub(crate) path: PathBuf,
}

/// Records tables a workspace archive keeps, filtered to the workspace's rows. Artifacts are
/// application records with retained bytes (SPEC section 12: "a consistent checkpoint of its
/// versions, retained objects and application records"), so the workspace's exports and View
/// exports are kept; its own workspace archives are not, so an archive never embeds a backup.
pub(crate) const WORKSPACE_TABLES: &[&str] = &[
    "artifacts",
    "comments",
    "derived_objects",
    "drafts",
    "events",
    "grants",
    "job_controls",
    "job_outputs",
    "jobs",
    "proposals",
    "receipts",
    "revision_map",
    "reviews",
    "uploads",
    "workspaces",
];

/// Records tables a workspace archive leaves empty: tenant and installation records (the
/// installation archive carries them); retry records, which mean something only to the live
/// ledger; short-lived capabilities and challenges; the records of backup archives, written
/// histories and restores, whose bytes an archive never holds; and pending puts, which belong
/// to writes of this installation.
pub(crate) const NOT_IN_WORKSPACE_ARCHIVE: &[&str] = &[
    "backup_archives",
    "confirmations",
    "connectors",
    "draft_mutations",
    "history_exports",
    "installation",
    "mutations",
    "pending_puts",
    "purge_progress",
    "purges",
    "restores",
    "sandbox_capabilities",
    "sessions",
    "workspace_mutations",
];

/// Job states an archive keeps: a queued or running job would run again after a restore.
const FINISHED_JOBS: &str = "('succeeded', 'failed', 'cancelled')";

/// Copy the live records database to `directory/full.sqlite` with `VACUUM INTO`.
pub(crate) fn copy_live(live: &Connection, directory: &Path) -> Result<FullCopy, ApiError> {
    let path = directory.join("full.sqlite");
    live.execute("VACUUM INTO ?1", [text(&path)?])
        .map_err(|error| sql(&error))?;
    Ok(FullCopy { path })
}

/// The digests of backup archives (workspace and installation) and written histories in a full
/// copy: an archive never holds them, even where a record of the workspace names one, so no
/// backup embeds another. Retained exports and View exports are not among them.
pub(crate) fn archive_objects(full: &FullCopy, tenant: &str) -> Result<BTreeSet<Digest>, ApiError> {
    let connection = read_only(&full.path)?;
    let mut found = BTreeSet::new();
    let present = tables(&connection)?;
    let backups = format!(
        "kind IN ('{}', '{}')",
        variant_text!(&ArtifactKind::WorkspaceBackup)?,
        variant_text!(&ArtifactKind::InstallationBackup)?
    );
    // An older copy (before a migration) may lack a table; it then holds none of those.
    for (table, only) in [
        ("artifacts", backups.as_str()),
        ("history_exports", "1"),
        ("backup_archives", "1"),
    ] {
        if !present.iter().any(|name| name == table) {
            continue;
        }
        let query = format!("SELECT digest FROM {table} WHERE tenant_id = ?1 AND {only}");
        let mut statement = connection.prepare(&query).map_err(|error| sql(&error))?;
        let values = statement
            .query_map([tenant], |row| row.get::<_, String>(0))
            .map_err(|error| sql(&error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| sql(&error))?;
        for value in values {
            found.insert(from_text!(value.as_str())?);
        }
    }
    Ok(found)
}

/// Filter the full copy to the workspace and vacuum it into `directory/records.sqlite`,
/// checked; the full copy is removed.
pub(crate) fn filter(
    full: &FullCopy,
    directory: &Path,
    tenant: &str,
    workspace: &str,
) -> Result<PathBuf, ApiError> {
    {
        let mut connection = Connection::open(&full.path).map_err(|error| sql(&error))?;
        let transaction = connection.transaction().map_err(|error| sql(&error))?;
        let kept_jobs = format!(
            "SELECT job_id FROM jobs WHERE tenant_id = ?1 AND workspace_id IS ?2
               AND state IN {FINISHED_JOBS}"
        );
        let workspace_backup = variant_text!(&ArtifactKind::WorkspaceBackup)?;
        for table in tables(&transaction)? {
            let query = match table.as_str() {
                "artifacts" => format!(
                    "DELETE FROM artifacts WHERE NOT (tenant_id = ?1 AND workspace_id IS ?2)
                       OR kind = '{workspace_backup}'"
                ),
                "job_controls" | "job_outputs" => {
                    format!("DELETE FROM \"{table}\" WHERE job_id NOT IN ({kept_jobs})")
                }
                "jobs" => format!("DELETE FROM jobs WHERE job_id NOT IN ({kept_jobs})"),
                "uploads" => "DELETE FROM uploads WHERE NOT (tenant_id = ?1 AND workspace_id = ?2
                       AND object_digest IS NOT NULL)"
                    .to_owned(),
                name if WORKSPACE_TABLES.contains(&name) => format!(
                    "DELETE FROM \"{table}\" WHERE NOT (tenant_id = ?1 AND workspace_id IS ?2)"
                ),
                name if NOT_IN_WORKSPACE_ARCHIVE.contains(&name) => {
                    format!("DELETE FROM \"{table}\" WHERE ?1 IS NOT NULL OR ?2 IS NOT NULL")
                }
                _ => {
                    return Err(ApiError::new(
                        ErrorCode::Internal,
                        format!("no archive decision covers the records table {table}"),
                    ));
                }
            };
            transaction
                .execute(&query, params![tenant, workspace])
                .map_err(|error| sql(&error))?;
        }
        transaction.commit().map_err(|error| sql(&error))?;
        let target = directory.join("records.sqlite");
        connection
            .execute("VACUUM INTO ?1", [text(&target)?])
            .map_err(|error| sql(&error))?;
    }
    std::fs::remove_file(&full.path).map_err(|error| io_error("remove", &full.path, &error))?;
    let target = directory.join("records.sqlite");
    verify(&target)?;
    Ok(target)
}

/// Check an archived records database: intact, no dangling reference, at a schema version this
/// build knows.
pub(crate) fn verify(path: &Path) -> Result<(), ApiError> {
    let connection = read_only(path)?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| sql(&error))?;
    let dangling: i64 = connection
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .map_err(|error| sql(&error))?;
    crate::format::refuse_newer(path, &crate::schema::RECORDS)?;
    if integrity != "ok" || dangling != 0 {
        return Err(ApiError::new(
            ErrorCode::Internal,
            format!(
                "an archived records database did not verify (integrity {integrity}, {dangling} dangling rows)"
            ),
        ));
    }
    Ok(())
}

/// The ordinary tables of a database, in name order.
pub(crate) fn tables(connection: &Connection) -> Result<Vec<String>, ApiError> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .map_err(|error| sql(&error))?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(|error| sql(&error))?
        .collect::<Result<_, _>>()
        .map_err(|error| sql(&error))
}

pub(crate) fn read_only(path: &Path) -> Result<Connection, ApiError> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|error| sql(&error))
}

/// A path as the UTF-8 text SQLite takes.
pub(crate) fn text(path: &Path) -> Result<&str, ApiError> {
    path.to_str().ok_or_else(|| {
        ApiError::new(
            ErrorCode::Unavailable,
            format!("the path {} is not UTF-8", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use okf_jawn_contract::error::{ApiError, ErrorCode};

    use super::{NOT_IN_WORKSPACE_ARCHIVE, WORKSPACE_TABLES, tables};

    #[test]
    fn every_records_table_is_either_archived_or_left_out() -> Result<(), ApiError> {
        let mut connection = rusqlite::Connection::open_in_memory()
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        crate::schema::RECORDS
            .to_latest(&mut connection)
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        let mut listed: Vec<&str> = WORKSPACE_TABLES
            .iter()
            .chain(NOT_IN_WORKSPACE_ARCHIVE)
            .copied()
            .collect();
        listed.sort_unstable();
        assert_eq!(tables(&connection)?, listed);
        Ok(())
    }
}
