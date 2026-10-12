//! Restoring a workspace archive, as an import into a blank workspace (`restore_import`) or as
//! part of a replacement restore (Stage 1b design section 8).
//!
//! In both, the archive is checked first, before anything is written (`checked`): an archive
//! may be a caller's file. Its entries, its manifest and every object's bytes against its
//! digest; then its records: a UTF-8 database SQLite reads, intact, migrated to this build's
//! schema, whose tables and indexes are exactly this build's, and whose rows are all of its own
//! tenant, workspace and jobs, each `record` JSON. Then the objects are put back, then the Git
//! history (the pack is indexed and every reference set), then the records in one transaction.
//! An import then writes one commit on the archive head carrying its mutation (same tree), so
//! its revision is found by that mutation like any job's write. A resumed import finds the
//! accepted head already at the archive's, or at that commit, and skips what is done; the
//! records step and the import's own record (`restores`) commit in one transaction, so it is
//! all or nothing.
//!
//! - **Import** keeps item identities and revisions, rewrites the workspace identity in every
//!   imported row (and inside its stored JSON), and gives back only the drafts whose editor is
//!   in `editors`; the others stay in the archive and are counted. It imports drafts, reviews,
//!   receipts, proposals and their comments, derived datasets and the revision map; jobs,
//!   events, grants and uploads belong to the installation the archive came from. An archive
//!   of this tenant is refused when its workspace still exists or a purge names it: purged
//!   content never comes back through an import.
//! - **Replacement** restores the workspace under its own identity with every archived row,
//!   and records, with those rows, the SHA-256 of the archive file it restored them from
//!   (`replacement_restores`). It refuses, in this order: an archive of another tenant; a
//!   workspace any recorded purge names, finished or not; and a workspace that exists, unless
//!   that record names this same archive file and its head is still the archive's, which is the
//!   same restore run again after a stop in a later workspace: it is accepted as it is.
//!
//! Known gap: a View's bindings name its workspace, and the on-disk form of a View document
//! is not decided yet (core-cli M1.6), so an import does not rewrite them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use git2::Repository;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, MutationId};
use okf_jawn_contract::workspace::RestoreReport;
use okf_jawn_core::storage::StorageScope;
use rusqlite::{Connection, OptionalExtension, params};
use tokio::runtime::Handle;

use super::archive::{Opened, PACK, RECORDS, WorkspaceManifest, invalid, oid, read_pack};
use super::snapshot::{WORKSPACE_TABLES, text};
use crate::blobs::file_reader;
use crate::data::io_error;
use crate::db::{conflict, internal, sql};
use crate::git::repo::{HEAD_REF, git, head, message_with_trailer, sync_directories, trailer_of};
use crate::pins::Pinned;
use crate::purge::references::{self, Rows};
use crate::puts;
use crate::storage::Storage;

/// How an archive is restored.
pub(crate) enum Mode {
    /// Into a blank workspace of this installation, for these editors, under this mutation.
    Import(Vec<String>, MutationId),
    /// Under its own identity, after the installation archive, from the archive file with this
    /// SHA-256.
    Replace(Digest),
}

/// Where archived rows go: the target tenant and workspace, and the archive's own workspace
/// identity, which an import rewrites.
struct Target {
    tenant: String,
    workspace: String,
    original: String,
    /// The archive's objects, put back before the rows.
    objects: Vec<Digest>,
    /// The objects the restored history's cards name.
    history: BTreeSet<Digest>,
    /// For each of the archive's objects that a restored card names as its conversion record,
    /// the objects it names (`references::record_tokens`).
    followed: BTreeMap<Digest, BTreeSet<Digest>>,
    /// For a replacement restore, the SHA-256 of the archive file, recorded with the rows.
    replaced_from: Option<Digest>,
    /// This build's schema and table order, which the rows are copied with.
    live: Live,
}

/// This build's records schema, read from the live database: every table, and the order a
/// replacement restore inserts the workspace tables in.
struct Live {
    schema: Schema,
    /// `WORKSPACE_TABLES`, each after every one its foreign keys reference.
    order: Vec<String>,
}

/// An archive opened and checked (`checked`), nothing written yet.
struct Checked {
    opened: Opened,
    /// The manifest, its objects listed from the archive's entries.
    manifest: WorkspaceManifest,
    /// The archived records, extracted, migrated and checked.
    records: PathBuf,
    /// This build's schema, which the records were checked against and are copied with.
    live: Live,
}

/// One table of a records database as its schema states it. Statements are kept with every run
/// of whitespace made one space (`spaced`).
#[derive(Debug, PartialEq, Eq)]
struct Table {
    /// Its columns, in order: the live schema's give the columns a restore copies
    /// (`copied_columns`). An archive's are not compared: its statement states each of them.
    columns: Vec<Column>,
    /// Its `CREATE TABLE` statement, which states each column (type, constraints, default and
    /// collation) and STRICT, WITHOUT ROWID, CHECK, REFERENCES and the keys.
    statement: String,
    /// Each of its indexes by name, with its `CREATE INDEX` statement (none for the index
    /// SQLite makes for a primary key or a unique column).
    indexes: BTreeMap<String, Option<String>>,
}

/// One column of a table, as `pragma_table_xinfo` gives it: name, declared type, not null,
/// default, primary-key position and hidden (non-zero for a generated column).
type Column = (String, String, i64, Option<String>, i64, i64);

/// Every ordinary table of a records database.
type Schema = BTreeMap<String, Table>;

/// Tables an import copies into the target workspace.
const IMPORTED: &[&str] = &[
    "comments",
    "derived_objects",
    "drafts",
    "proposals",
    "receipts",
    "revision_map",
    "reviews",
];
/// The message of the commit an import writes on the archive head.
const RESTORE_MESSAGE: &str = "Restore the workspace from an archive";
/// SQLite's extended result code of a failed foreign key (`SQLITE_CONSTRAINT_FOREIGNKEY`).
const FOREIGN_KEY_FAILED: std::ffi::c_int = 787;

/// Restore the archive at `archive` into `scope`, staging under `staging`. Blocking: `runtime`
/// drives the object store and the records database.
pub(crate) fn restore(
    storage: &Storage,
    runtime: &Handle,
    scope: &StorageScope,
    archive: &Path,
    staging: &Path,
    mode: &Mode,
) -> Result<(RestoreReport, WorkspaceManifest), ApiError> {
    let Checked {
        mut opened,
        manifest,
        records,
        live,
    } = checked(storage, runtime, archive, staging)?;
    // A replacement restore run again after a crash in a later workspace accepts a workspace
    // this same archive already restored in full, and leaves it as it is.
    if let Some(report) = refuse(storage, runtime, scope, &manifest, mode)? {
        return Ok((report, manifest));
    }
    // Each object is pinned to the restored workspace while the import runs, and recorded as a
    // pending put (`puts`) until the import's rows land.
    let mut pinned = Vec::new();
    let restored = put_back(
        storage,
        runtime,
        scope,
        (&mut opened, &mut pinned),
        (&manifest, &records, live),
        staging,
        mode,
    );
    for pin in pinned {
        pin.end()?;
    }
    restored
}

/// Check the archive at `archive` as a restore does before it writes anything (`checked`),
/// staging under `staging`; returns its manifest. A replacement restore checks every workspace
/// archive of its set this way before it writes the installation archive's rows, so a set with
/// a refused archive writes nothing. Blocking.
///
/// # Errors
/// Returns `InvalidInput` for an archive a restore refuses.
pub(crate) fn check(
    storage: &Storage,
    runtime: &Handle,
    archive: &Path,
    staging: &Path,
) -> Result<WorkspaceManifest, ApiError> {
    checked(storage, runtime, archive, staging).map(|checked| checked.manifest)
}

/// Open the archive at `archive` and check everything a restore reads from it, before anything
/// is written (an archive may be a caller's file):
/// - its entries (`Opened::open`: compression methods, sizes, and each object's bytes against
///   its digest), its manifest, and its objects, listed from its entries (an older manifest's
///   own list is not read);
/// - its records, extracted under `staging` (`migrated`): a UTF-8 database SQLite reads,
///   intact, and brought to this build's schema;
/// - their schema (`refuse_other_schema`) and then their rows (`refuse_foreign_rows`), against
///   this build's schema read from the live database: the one schema the rows are then copied
///   with.
///
/// # Errors
/// Returns `InvalidInput` for an archive that fails a check.
fn checked(
    storage: &Storage,
    runtime: &Handle,
    archive: &Path,
    staging: &Path,
) -> Result<Checked, ApiError> {
    let mut opened = Opened::open(archive)?;
    let mut manifest = opened.workspace()?;
    manifest.objects = opened.objects()?;
    let records = staging.join(RECORDS);
    opened.extract(RECORDS, &records)?;
    migrated(&records)?;
    let live = runtime.block_on(storage.records.call(|connection| {
        Ok(Live {
            schema: schema_of(connection)?,
            order: parents_first(connection, WORKSPACE_TABLES)?,
        })
    }))?;
    let connection = super::snapshot::read_only(&records)?;
    refuse_other_schema(&connection, &live.schema)?;
    refuse_foreign_rows(&connection, &manifest, &live.schema)?;
    drop(connection);
    Ok(Checked {
        opened,
        manifest,
        records,
        live,
    })
}

/// Check the extracted records database at `records` and bring it to this build's schema:
/// SQLite must read it as a database, `PRAGMA integrity_check` must answer `ok` (the checks'
/// count queries and the copy may take different paths through its tables and indexes, so
/// those must agree), its text encoding must be UTF-8 like the live database's (the copy
/// attaches it, which SQLite refuses for another encoding), and this build's migrations must
/// accept it.
///
/// # Errors
/// Returns `InvalidInput` for a file that is not a database, is damaged, is not UTF-8 or does
/// not migrate (a newer schema, say); a store fault only for a failure of this machine
/// (`is_fault`).
fn migrated(records: &Path) -> Result<(), ApiError> {
    let unreadable = |error: &rusqlite::Error| {
        if is_fault(error) {
            sql(error)
        } else {
            invalid(&format!("its records database cannot be read: {error}"))
        }
    };
    let mut connection = Connection::open(records).map_err(|error| unreadable(&error))?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| unreadable(&error))?;
    if integrity != "ok" {
        return Err(invalid(&format!(
            "its records database is damaged: {integrity}"
        )));
    }
    let encoding: String = connection
        .query_row("PRAGMA encoding", [], |row| row.get(0))
        .map_err(|error| unreadable(&error))?;
    if encoding != "UTF-8" {
        return Err(invalid(&format!(
            "its records database is in {encoding}, not UTF-8"
        )));
    }
    crate::schema::RECORDS
        .to_latest(&mut connection)
        .map_err(|error| match &error {
            rusqlite_migration::Error::RusqliteError { err, .. } if is_fault(err) => sql(err),
            _ => invalid(&format!(
                "its records database does not migrate to this build's schema: {error}"
            )),
        })
}

/// Whether a SQLite failure is this machine's (disk, memory, locks, permissions) rather than
/// the file's.
fn is_fault(error: &rusqlite::Error) -> bool {
    use rusqlite::ErrorCode as Code;
    error.sqlite_error().is_some_and(|failure| {
        matches!(
            failure.code,
            Code::DiskFull
                | Code::SystemIoFailure
                | Code::CannotOpen
                | Code::OutOfMemory
                | Code::ReadOnly
                | Code::DatabaseBusy
                | Code::DatabaseLocked
                | Code::PermissionDenied
                | Code::FileLockingProtocolFailed
        )
    })
}

/// Put the archive's objects back (each pin kept in `pinned`), then its history and the rows of
/// the checked records at `records`.
fn put_back(
    storage: &Storage,
    runtime: &Handle,
    scope: &StorageScope,
    (opened, pinned): (&mut Opened, &mut Vec<Pinned>),
    (manifest, records, live): (&WorkspaceManifest, &Path, Live),
    staging: &Path,
    mode: &Mode,
) -> Result<(RestoreReport, WorkspaceManifest), ApiError> {
    for digest in &manifest.objects {
        let file = staging.join(digest.as_str());
        opened.extract(&format!("objects/{}", digest.as_str()), &file)?;
        let size = std::fs::metadata(&file)
            .map_err(|error| io_error("read", &file, &error))?
            .len();
        let (_, pin) = runtime.block_on(storage.blobs.put_pinned(
            scope,
            file_reader(Some(file.clone()), 0, size),
            size,
            Some(digest.clone()),
        ))?;
        pinned.push(pin);
        std::fs::remove_file(&file).map_err(|error| io_error("remove", &file, &error))?;
    }
    let pack = staging.join(PACK);
    opened.extract(PACK, &pack)?;
    let items = history(storage, runtime, scope, manifest, &pack, mode)?;
    let restored_history = references::history(&storage.repositories.open(scope)?)?;
    let mut followed = BTreeMap::new();
    for digest in &manifest.objects {
        let named = references::record_tokens(
            &storage.blobs,
            &scope.tenant_id,
            digest,
            restored_history.records.contains(digest),
        )?;
        if !named.is_empty() {
            followed.insert(digest.clone(), named);
        }
    }
    let report = {
        let path = text(records)?.to_owned();
        let target = Target {
            tenant: scope.tenant_id.as_str().to_owned(),
            workspace: scope.workspace_id.0.to_string(),
            original: manifest.workspace_id.0.to_string(),
            objects: manifest.objects.clone(),
            history: restored_history.objects,
            followed,
            replaced_from: match mode {
                Mode::Import(..) => None,
                Mode::Replace(digest) => Some(digest.clone()),
            },
            live,
        };
        let editors = match mode {
            Mode::Import(editors, _) => {
                Some(serde_json::to_string(editors).map_err(|error| crate::db::json(&error))?)
            }
            Mode::Replace(_) => None,
        };
        let mutation = match mode {
            Mode::Import(_, mutation) => Some(*mutation),
            Mode::Replace(_) => None,
        };
        runtime.block_on(storage.records.call(move |connection| {
            copy_rows(
                connection,
                &path,
                &target,
                editors.as_deref(),
                items,
                mutation,
            )
        }))?
    };
    Ok((report, manifest.clone()))
}

/// The report of a workspace that a replacement restore of the archive file `archive` already
/// completed: the record of that restore (`replacement_restores`, written with the workspace's
/// rows, all or nothing) names this same archive file, and the workspace's accepted head is
/// still the archive's. `None` otherwise (a workspace never restored here, restored from
/// another archive, or moved on since), so it is refused as existing.
fn restored_already(
    storage: &Storage,
    runtime: &Handle,
    scope: &StorageScope,
    manifest: &WorkspaceManifest,
    archive: &Digest,
) -> Result<Option<RestoreReport>, ApiError> {
    let main = manifest
        .references
        .iter()
        .find(|(name, _)| name == HEAD_REF)
        .map(|(_, commit)| oid(commit))
        .transpose()?;
    let (tenant, workspace) = crate::db::scope_key(scope);
    let digest = archive.as_str().to_owned();
    let recorded: Option<String> = runtime.block_on(storage.records.call(move |connection| {
        connection
            .query_row(
                "SELECT restored.report FROM replacement_restores AS restored
                 JOIN workspaces USING (tenant_id, workspace_id)
                 WHERE restored.tenant_id = ?1 AND restored.workspace_id = ?2
                   AND restored.archive = ?3",
                params![tenant, workspace, digest],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sql(&error))
    }))?;
    let Some(recorded) = recorded else {
        return Ok(None);
    };
    if main.is_none() || !storage.repositories.path(scope).exists() {
        return Ok(None);
    }
    let repository = storage.repositories.open(scope)?;
    if Some(head(&repository)?) != main {
        return Ok(None);
    }
    serde_json::from_str(&recorded)
        .map(Some)
        .map_err(|error| crate::db::json(&error))
}

/// Every ordinary table of the database `connection` opens (`table_of`).
fn schema_of(connection: &Connection) -> Result<Schema, ApiError> {
    let mut schema = Schema::new();
    for table in super::snapshot::tables(connection)? {
        let read = table_of(connection, &table)?;
        schema.insert(table, read);
    }
    Ok(schema)
}

/// `tables` ordered so that each comes after every one of `tables` its foreign keys reference
/// (parents first), in name order otherwise, read from the schema of `connection`.
///
/// # Errors
/// Returns `Internal` for a foreign-key cycle, which no order satisfies.
fn parents_first(connection: &Connection, tables: &[&str]) -> Result<Vec<String>, ApiError> {
    let mut parents: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for table in tables {
        let mut statement = connection
            .prepare("SELECT \"table\" FROM pragma_foreign_key_list(?1)")
            .map_err(|error| sql(&error))?;
        let referenced = statement
            .query_map([table], |row| row.get::<_, String>(0))
            .map_err(|error| sql(&error))?
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(|error| sql(&error))?;
        parents.insert(
            table,
            referenced
                .into_iter()
                .filter(|parent| parent != table && tables.contains(&parent.as_str()))
                .collect(),
        );
    }
    let mut ordered: Vec<String> = Vec::new();
    while ordered.len() < tables.len() {
        let next = tables.iter().find(|table| {
            !ordered.iter().any(|placed| placed == **table)
                && parents
                    .get(**table)
                    .is_none_or(|needed| needed.iter().all(|parent| ordered.contains(parent)))
        });
        match next {
            Some(table) => ordered.push((*table).to_owned()),
            None => return Err(internal("the records schema's foreign keys form a cycle")),
        }
    }
    Ok(ordered)
}

/// The columns of `table` in `schema` that a restore copies: every column but an event's
/// sequence number, which the live log assigns again.
fn copied_columns(schema: &Schema, table: &str) -> Result<Vec<String>, ApiError> {
    Ok(schema
        .get(table)
        .ok_or_else(|| internal(format!("this build's schema has no table {table}")))?
        .columns
        .iter()
        .map(|(name, ..)| name.clone())
        .filter(|name| !(table == "events" && name == "seq"))
        .collect())
}

/// Refuse archived records whose schema is not exactly this build's (`live`), before any row is
/// read. A written archive is never refused (it is a `VACUUM INTO` copy of this schema, or an
/// older one this build's migrations bring to it); a refused one is a crafted file.
///
/// Each table of this build's schema must be an ordinary table of the archive under exactly its
/// name (a view, a table under another case or a missing table is refused), and equal it
/// exactly (`table_of`): the same `CREATE TABLE` statement (so the same columns, each with its
/// declared type, constraints, default and collation, and the same STRICT, WITHOUT ROWID, CHECK,
/// REFERENCES and keys) and the same indexes, each by name with the same statement. Statements
/// are compared with every run of whitespace made one space, nothing else. No statement of this
/// build's schema holds whitespace inside a quoted name or literal, so that rule widens nothing,
/// and the columns `pragma_table_xinfo` reports (all stated by the statement) are not compared
/// again. What else the archive holds (a table this build does not have, a trigger) is never
/// read, and nothing is written to the archive for a trigger to act on.
///
/// # Errors
/// Returns `InvalidInput` naming the table.
fn refuse_other_schema(connection: &Connection, live: &Schema) -> Result<(), ApiError> {
    for (table, expected) in live {
        let found: Vec<(String, String)> = {
            let mut statement = connection
                .prepare("SELECT name, type FROM sqlite_schema WHERE lower(name) = lower(?1)")
                .map_err(|error| sql(&error))?;
            statement
                .query_map([table], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|error| sql(&error))?
                .collect::<Result<_, _>>()
                .map_err(|error| sql(&error))?
        };
        if found != [(table.clone(), "table".to_owned())] {
            return Err(invalid(&format!(
                "its {table} is not this build's table (found {found:?})"
            )));
        }
        let archived = table_of(connection, table)?;
        let differs = if archived.statement != expected.statement {
            "definition (its columns, a collation, STRICT, WITHOUT ROWID, CHECK or REFERENCES)"
        } else if archived.indexes != expected.indexes {
            "indexes"
        } else {
            continue;
        };
        return Err(invalid(&format!(
            "its {table} table is not this build's: its {differs} differ"
        )));
    }
    Ok(())
}

/// Refuse archived records holding a row the copy must not write, once their schema is this
/// build's (`refuse_other_schema`). A written archive is never refused (`snapshot::filter`).
///
/// In the columns the copy selects (`copied_columns`, the same list), compared byte for byte
/// (`COLLATE BINARY`, whatever collation the archive declares):
/// - a table with `tenant_id` and `workspace_id` holds only the manifest's tenant and workspace;
/// - one keyed by `job_id` and no tenant only rows of the archive's own jobs (`job_controls`,
///   `job_outputs`);
/// - every `record` is JSON (an import rewrites the workspace identity inside it, and the
///   application reads it);
/// - and `workspaces` holds exactly one row, which is then the manifest's workspace.
///
/// Every workspace table is one of the first two kinds; one that is neither is a schema change
/// these checks must cover first, so it fails every restore (`Internal`) rather than go
/// unchecked.
///
/// # Errors
/// Returns `InvalidInput` naming the table.
fn refuse_foreign_rows(
    connection: &Connection,
    manifest: &WorkspaceManifest,
    live: &Schema,
) -> Result<(), ApiError> {
    let tenant = manifest.tenant_id.as_str();
    let workspace = manifest.workspace_id.0.to_string();
    for table in WORKSPACE_TABLES {
        let copied = copied_columns(live, table)?;
        let has = |name: &str| copied.iter().any(|column| column == name);
        let foreign = if has("tenant_id") && has("workspace_id") {
            "\"tenant_id\" COLLATE BINARY IS NOT ?1 OR \"workspace_id\" COLLATE BINARY IS NOT ?2"
        } else if has("job_id") && !has("tenant_id") {
            "\"job_id\" COLLATE BINARY NOT IN (SELECT \"job_id\" COLLATE BINARY FROM jobs) \
             AND ?1 IS NOT NULL AND ?2 IS NOT NULL"
        } else {
            return Err(internal(format!(
                "no row check of a restore covers the records table {table}"
            )));
        };
        let count: i64 = connection
            .query_row(
                &format!("SELECT count(*) FROM \"{table}\" WHERE {foreign}"),
                params![tenant, workspace],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        if count > 0 {
            return Err(invalid(&format!(
                "its {table} hold {count} rows of another tenant or workspace than its manifest \
                 names"
            )));
        }
        if has("record") {
            let unreadable: i64 = connection
                .query_row(
                    &format!("SELECT count(*) FROM \"{table}\" WHERE NOT json_valid(\"record\")"),
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            if unreadable > 0 {
                return Err(invalid(&format!(
                    "its {table} hold {unreadable} rows whose record is not JSON"
                )));
            }
        }
    }
    let workspaces: i64 = connection
        .query_row("SELECT count(*) FROM workspaces", [], |row| row.get(0))
        .map_err(|error| sql(&error))?;
    if workspaces != 1 {
        return Err(invalid(&format!(
            "its workspaces hold {workspaces} rows, not the one workspace its manifest names"
        )));
    }
    Ok(())
}

/// The one table `table` of `connection` as its schema states it: its columns
/// (`pragma_table_xinfo`, generated ones included), its `CREATE TABLE` statement and its
/// indexes, statements `spaced`.
fn table_of(connection: &Connection, table: &str) -> Result<Table, ApiError> {
    let columns = {
        let mut statement = connection
            .prepare(
                "SELECT name, type, \"notnull\", dflt_value, pk, hidden \
                 FROM pragma_table_xinfo(?1)",
            )
            .map_err(|error| sql(&error))?;
        statement
            .query_map([table], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .map_err(|error| sql(&error))?
            .collect::<Result<Vec<Column>, _>>()
            .map_err(|error| sql(&error))?
    };
    let statement: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    let indexes = {
        let mut query = connection
            .prepare(
                "SELECT name, sql FROM sqlite_schema \
                 WHERE type = 'index' AND lower(tbl_name) = lower(?1)",
            )
            .map_err(|error| sql(&error))?;
        query
            .query_map([table], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .map_err(|error| sql(&error))?
            .map(|index| index.map(|(name, text)| (name, text.as_deref().map(spaced))))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(|error| sql(&error))?
    };
    Ok(Table {
        columns,
        statement: spaced(statement.as_deref().unwrap_or_default()),
        indexes,
    })
}

/// `text` with every run of whitespace made one space, and none at either end.
fn spaced(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Refuse an archive the mode may not restore into `scope`. For a replacement restore, the
/// report of a workspace this same archive already restored (`restored_already`), which is
/// accepted as it is; a purge refuses even that one.
fn refuse(
    storage: &Storage,
    runtime: &Handle,
    scope: &StorageScope,
    manifest: &WorkspaceManifest,
    mode: &Mode,
) -> Result<Option<RestoreReport>, ApiError> {
    let tenant = manifest.tenant_id.as_str().to_owned();
    let workspace = manifest.workspace_id.0.to_string();
    let (exists, purge) = runtime.block_on(storage.records.call(move |connection| {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM workspaces WHERE tenant_id = ?1 AND workspace_id = ?2)",
                params![tenant, workspace],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        let key = format!("workspace/{workspace}");
        // Any recorded purge of the workspace, finished or not.
        let purge: Option<(String, String)> = connection
            .query_row(
                "SELECT purge_id, state FROM purges WHERE tenant_id = ?1 AND target_key = ?2
                 ORDER BY state = 'completed' DESC LIMIT 1",
                params![tenant, key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| sql(&error))?;
        Ok((exists, purge))
    }))?;
    let purged = purge.is_some();
    match mode {
        Mode::Import(..) => {
            if manifest.tenant_id == scope.tenant_id && (exists || purged) {
                return Err(conflict(
                    "an archive of a workspace this installation still holds or purged is not \
                     imported; purged content never comes back through an import",
                ));
            }
        }
        Mode::Replace(archive) => {
            if manifest.tenant_id != scope.tenant_id {
                return Err(invalid(
                    "it belongs to another tenant than the installation archive",
                ));
            }
            // A purge of the workspace, finished or not, refuses it: the operator finishes an
            // unfinished purge first, and purged content never comes back. Checked before the
            // run-again acceptance, so a workspace restored before a purge was recorded is
            // refused too, and before anything is removed.
            if let Some((purge, state)) = &purge {
                return Err(conflict(format!(
                    "purge {purge} ({state}) of this workspace is recorded here"
                )));
            }
            if let Some(report) = restored_already(storage, runtime, scope, manifest, archive)? {
                return Ok(Some(report));
            }
            if exists {
                return Err(conflict("this workspace already exists here"));
            }
            // A repository no record names yet is what a replacement restore that stopped
            // inside this workspace left behind (its rows commit last, all or nothing): it is
            // removed and the workspace restored again, with no manual cleanup.
            if storage.repositories.path(scope).exists() {
                storage.repositories.remove(scope)?;
            }
        }
    }
    Ok(None)
}

/// Index the pack and set every reference, then, for an import, write the one commit that
/// carries the import's mutation; returns the item count at the accepted head.
///
/// The restore commit has the archive head as its only parent and the archive head's tree, so
/// the import's revision is found by its mutation (`VersionStore::find_commit`) like every
/// job's write, and a mutation names at most one commit. It is written before the import's
/// rows, whose transaction records the import, so a resumed import that stopped after the
/// history finds the head at the archive's and writes the commit, and one that stopped after
/// the commit finds it at the head and writes nothing again. Under the workspace's lock, which
/// the caller holds for the whole import, an import into a workspace a purge names is refused
/// before any Git write, as a commit is (`purge::guard`).
fn history(
    storage: &Storage,
    runtime: &Handle,
    scope: &StorageScope,
    manifest: &WorkspaceManifest,
    pack: &Path,
    mode: &Mode,
) -> Result<u32, ApiError> {
    let main = manifest
        .references
        .iter()
        .find(|(name, _)| name == HEAD_REF)
        .map(|(_, commit)| oid(commit))
        .transpose()?
        .ok_or_else(|| invalid("it has no accepted head"))?;
    let repository = match mode {
        Mode::Import(..) => {
            runtime.block_on(crate::purge::guard::refuse_scope(&storage.records, scope))?;
            storage.repositories.open_durable(scope)?
        }
        Mode::Replace(_) => storage.repositories.create(scope)?,
    };
    let current = crate::git::repo::reference_target(&repository, HEAD_REF)?;
    let committed = match (mode, current) {
        (Mode::Import(_, mutation), Some(current)) => {
            is_restore_commit(&repository, current, main, *mutation)?
        }
        _ => false,
    };
    if !committed {
        if current != Some(main) {
            if let Some(current) = current {
                refuse_unless_blank(&repository, current, &manifest.references)?;
            }
            // Once the head is at the archive's, the pack was indexed before any reference
            // was set, so a resumed restore does not index it again.
            read_pack(&repository, pack)?;
        }
        // Every other reference first, each write repeatable, and the accepted head last: a
        // restore that stopped part way left the head where it was (or already at the
        // archive's), and a resumed one sets every reference again before moving it.
        for (name, commit) in manifest
            .references
            .iter()
            .filter(|(name, _)| name.as_str() != HEAD_REF)
        {
            repository
                .reference(name, oid(commit)?, true, "okf-jawn restore")
                .map_err(|error| git(&error))?;
        }
        if current != Some(main) {
            let moved = match current {
                Some(current) => {
                    repository.reference_matching(HEAD_REF, main, true, current, "okf-jawn restore")
                }
                None => repository.reference(HEAD_REF, main, true, "okf-jawn restore"),
            };
            moved.map_err(|error| git(&error))?;
            repository.set_head(HEAD_REF).map_err(|error| git(&error))?;
        }
        sync_directories(&repository, &["objects", "refs", "refs/heads"])?;
    }
    if let Mode::Import(_, mutation) = mode
        && !committed
    {
        restore_commit(&repository, main, *mutation)?;
    }
    let tree = repository
        .find_commit(head(&repository)?)
        .and_then(|commit| commit.tree())
        .map_err(|error| git(&error))?;
    u32::try_from(crate::git::items_of(&repository, &tree)?.len())
        .map_err(|_| internal("too many items to count"))
}

/// Whether `current` is the restore commit of `mutation` on the archive head `main`: its only
/// parent is `main` and its trailer names `mutation`.
fn is_restore_commit(
    repository: &Repository,
    current: git2::Oid,
    main: git2::Oid,
    mutation: MutationId,
) -> Result<bool, ApiError> {
    let commit = repository
        .find_commit(current)
        .map_err(|error| git(&error))?;
    let wanted = mutation.0.to_string();
    Ok(commit.parent_ids().collect::<Vec<_>>() == vec![main]
        && commit
            .message()
            .is_ok_and(|message| trailer_of(message) == Some(wanted.as_str())))
}

/// Write the restore commit of `mutation` on the archive head `main` (same tree, no content
/// change) and move the accepted head to it, only if the head is still `main`.
fn restore_commit(
    repository: &Repository,
    main: git2::Oid,
    mutation: MutationId,
) -> Result<(), ApiError> {
    let parent = repository.find_commit(main).map_err(|error| git(&error))?;
    let tree = parent.tree().map_err(|error| git(&error))?;
    let who = git2::Signature::now("okf-jawn", "restore").map_err(|error| git(&error))?;
    let commit = repository
        .commit(
            None,
            &who,
            &who,
            &message_with_trailer(RESTORE_MESSAGE, mutation),
            &tree,
            &[&parent],
        )
        .map_err(|error| git(&error))?;
    sync_directories(repository, &["objects"])?;
    repository
        .reference_matching(HEAD_REF, commit, true, main, "okf-jawn restore commit")
        .map_err(|error| git(&error))?;
    sync_directories(repository, &["refs", "refs/heads"])
}

/// Refuse a target workspace that holds anything beyond its creation commit. A reference the
/// archive names, already at the archive's commit, is what a stopped restore set; it does not
/// make the workspace less blank.
fn refuse_unless_blank(
    repository: &Repository,
    current: git2::Oid,
    archived: &[(String, String)],
) -> Result<(), ApiError> {
    let commit = repository
        .find_commit(current)
        .map_err(|error| git(&error))?;
    let tree = commit.tree().map_err(|error| git(&error))?;
    let mut foreign = 0_usize;
    for reference in repository.references().map_err(|error| git(&error))? {
        let reference = reference.map_err(|error| git(&error))?;
        let name = reference.name().unwrap_or_default();
        let restored = archived.iter().any(|(archived_name, commit)| {
            archived_name.as_str() != HEAD_REF
                && archived_name == name
                && reference
                    .target()
                    .map(|target| target.to_string())
                    .as_deref()
                    == Some(commit.as_str())
        });
        if name != HEAD_REF && !restored {
            foreign = foreign.saturating_add(1);
        }
    }
    if commit.parent_count() == 0 && tree.is_empty() && foreign == 0 {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::Conflict,
            "a restore fills only a blank workspace",
        ))
    }
}

/// Copy the archived rows into the live database in one transaction and return the report.
/// With `editors` this is an import into the target workspace, and the import's own record
/// (`restores`, under `mutation`) is written in the same transaction: a stop anywhere leaves
/// either nothing or the whole import with its record, so a retry completes or replays.
fn copy_rows(
    connection: &mut Connection,
    archive: &str,
    target: &Target,
    editors: Option<&str>,
    items: u32,
    mutation: Option<MutationId>,
) -> Result<RestoreReport, ApiError> {
    connection
        .execute("ATTACH DATABASE ?1 AS archive", [archive])
        .map_err(|error| sql(&error))?;
    let copied = copy_tables(connection, target, editors, items, mutation);
    connection
        .execute("DETACH DATABASE archive", [])
        .map_err(|error| sql(&error))?;
    copied
}

/// The transaction of `copy_rows`, on an attached archive.
fn copy_tables(
    connection: &mut Connection,
    target: &Target,
    editors: Option<&str>,
    items: u32,
    mutation: Option<MutationId>,
) -> Result<RestoreReport, ApiError> {
    let transaction = connection.transaction().map_err(|error| sql(&error))?;
    match write_tables(&transaction, target, editors, items, mutation) {
        Ok(report) => {
            transaction.commit().map_err(|error| sql(&error))?;
            Ok(report)
        }
        Err(error) => {
            transaction.rollback().map_err(|failure| sql(&failure))?;
            Err(error)
        }
    }
}

/// The writes of `copy_tables`, inside its transaction.
fn write_tables(
    transaction: &rusqlite::Transaction<'_>,
    target: &Target,
    editors: Option<&str>,
    items: u32,
    mutation: Option<MutationId>,
) -> Result<RestoreReport, ApiError> {
    // A replacement copies every workspace table, parents before the tables whose foreign keys
    // reference them (`jobs` before `job_controls` and `job_outputs`).
    let tables: Vec<&str> = if editors.is_some() {
        IMPORTED.to_vec()
    } else {
        target.live.order.iter().map(String::as_str).collect()
    };
    for table in tables {
        insert(transaction, table, target, editors)?;
    }
    let total: i64 = transaction
        .query_row("SELECT count(*) FROM archive.drafts", [], |row| row.get(0))
        .map_err(|error| sql(&error))?;
    let restored: i64 = transaction
        .query_row(
            "SELECT count(*) FROM drafts WHERE tenant_id = ?1 AND workspace_id = ?2",
            params![target.tenant, target.workspace],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    let count =
        |value: i64| u32::try_from(value).map_err(|_| internal("a draft count is out of range"));
    let report = RestoreReport {
        items,
        drafts_restored: count(restored)?,
        drafts_unassigned: count(total.saturating_sub(restored))?,
    };
    if let Some(mutation) = mutation {
        let text = serde_json::to_string(&report).map_err(|error| crate::db::json(&error))?;
        transaction
            .execute(
                "INSERT INTO restores (mutation_id, tenant_id, workspace_id, report)
                 VALUES (?1, ?2, ?3, ?4) ON CONFLICT (mutation_id) DO NOTHING",
                params![
                    mutation.0.to_string(),
                    target.tenant,
                    target.workspace,
                    text
                ],
            )
            .map_err(|error| sql(&error))?;
    }
    // A replacement restore records the archive file it restored the workspace from, with the
    // rows: the same restore run again accepts the workspace only on this record. The workspace
    // had no row (it is refused otherwise), so a record left from before is stale.
    if let Some(archive) = &target.replaced_from {
        let text = serde_json::to_string(&report).map_err(|error| crate::db::json(&error))?;
        transaction
            .execute(
                "INSERT INTO replacement_restores (tenant_id, workspace_id, archive, report)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (tenant_id, workspace_id)
                 DO UPDATE SET archive = excluded.archive, report = excluded.report",
                params![target.tenant, target.workspace, archive.as_str(), text],
            )
            .map_err(|error| sql(&error))?;
    }
    // The archive's objects that the workspace now names land with the rows: those its
    // restored history's cards and its rows name, and those a conversion record among them
    // names, as a purge's plan reads them. An object only rows an import does not copy named
    // (an upload's or a job output's) keeps its pending put, so a purge of the workspace
    // deletes it. A failed import rolls this back with the rows, so its pending puts stay.
    let mut named = target.history.clone();
    named.extend(references::records(
        transaction,
        &target.tenant,
        &target.workspace,
        Rows::Target,
        &BTreeSet::new(),
    )?);
    let followed: Vec<Digest> = named
        .iter()
        .filter_map(|digest| target.followed.get(digest))
        .flatten()
        .cloned()
        .collect();
    named.extend(followed);
    puts::land(
        transaction,
        &target.tenant,
        &target.workspace,
        target
            .objects
            .iter()
            .filter(|digest| named.contains(*digest)),
    )?;
    Ok(report)
}

/// Copy one table; an import rewrites the workspace identity, also inside stored JSON.
fn insert(
    connection: &Connection,
    table: &str,
    target: &Target,
    editors: Option<&str>,
) -> Result<(), ApiError> {
    let (tenant, workspace, original) = (&target.tenant, &target.workspace, &target.original);
    // The columns of this build's schema: the same list the archive's rows were checked in
    // (`refuse_foreign_rows`). An event's sequence number is the live log's; it is assigned again.
    let columns: Vec<String> = copied_columns(&target.live.schema, table)?;
    let rewrite = editors.is_some();
    let values: Vec<String> = columns
        .iter()
        .map(|column| match column.as_str() {
            "tenant_id" if rewrite => "?1".to_owned(),
            "workspace_id" if rewrite => "?2".to_owned(),
            other => format!("\"{other}\""),
        })
        .collect();
    let mut filter = "?1 IS NOT NULL AND ?2 IS NOT NULL AND ?3 IS NOT NULL".to_owned();
    if table == "drafts" {
        filter.push_str(" AND (?4 IS NULL OR editor IN (SELECT value FROM json_each(?4)))");
    } else {
        filter.push_str(" AND (?4 IS NULL OR ?4 IS NOT NULL)");
    }
    let query = format!(
        "INSERT INTO main.\"{table}\" ({}) SELECT {} FROM archive.\"{table}\" WHERE {filter}",
        columns
            .iter()
            .map(|column| format!("\"{column}\""))
            .collect::<Vec<_>>()
            .join(", "),
        values.join(", "),
    );
    connection
        .execute(&query, params![tenant, workspace, original, editors])
        .map_err(|error| copy_failure(table, &error))?;
    if rewrite && columns.iter().any(|column| column == "record") {
        rewrite_records(connection, table, target)?;
    }
    Ok(())
}

/// What a copy of `table` the live database rejected answers. Every constraint failure is the
/// archive's, so `InvalidInput`: a failed foreign key names records the archive does not hold;
/// any other (a key or unique value this installation already holds, a value of another type)
/// is the archive's rows breaking this installation's records. Anything else is a store fault.
fn copy_failure(table: &str, error: &rusqlite::Error) -> ApiError {
    match error {
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.extended_code == FOREIGN_KEY_FAILED =>
        {
            invalid(&format!(
                "its {table} reference records the archive does not hold (a foreign key failed)"
            ))
        }
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            invalid(&format!(
                "its {table} break a constraint of this installation's records ({error})"
            ))
        }
        other => sql(other),
    }
}

/// Rewrite the workspace identity inside the stored JSON of `table`'s rows of the target
/// workspace: only the structured identity fields (`workspace_id`, and the items of
/// `workspace_ids`) that hold the archive's workspace. User-authored text (a comment, a proposal
/// title or body) keeps every identity it quotes.
fn rewrite_records(connection: &Connection, table: &str, target: &Target) -> Result<(), ApiError> {
    let rows: Vec<(i64, String)> = {
        let mut statement = connection
            .prepare(&format!(
                "SELECT rowid, record FROM main.\"{table}\" WHERE tenant_id = ?1 AND workspace_id = ?2"
            ))
            .map_err(|error| sql(&error))?;
        statement
            .query_map(params![target.tenant, target.workspace], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|error| sql(&error))?
            .collect::<Result<_, _>>()
            .map_err(|error| sql(&error))?
    };
    for (rowid, record) in rows {
        let mut value: serde_json::Value =
            serde_json::from_str(&record).map_err(|error| crate::db::json(&error))?;
        if rewrite_identity(&mut value, &target.original, &target.workspace) {
            connection
                .execute(
                    &format!("UPDATE main.\"{table}\" SET record = ?1 WHERE rowid = ?2"),
                    params![value.to_string(), rowid],
                )
                .map_err(|error| sql(&error))?;
        }
    }
    Ok(())
}

/// Replace `original` with `workspace` in every `workspace_id` field and `workspace_ids` item of
/// `value`, recursively; whether anything changed.
fn rewrite_identity(value: &mut serde_json::Value, original: &str, workspace: &str) -> bool {
    let mut changed = false;
    match value {
        serde_json::Value::Object(fields) => {
            for (key, field) in fields.iter_mut() {
                match (key.as_str(), &mut *field) {
                    ("workspace_id", serde_json::Value::String(text)) if text == original => {
                        workspace.clone_into(text);
                        changed = true;
                    }
                    ("workspace_ids", serde_json::Value::Array(items)) => {
                        for item in items.iter_mut() {
                            if item.as_str() == Some(original) {
                                *item = serde_json::Value::String(workspace.to_owned());
                                changed = true;
                            }
                        }
                    }
                    _ => changed |= rewrite_identity(field, original, workspace),
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                changed |= rewrite_identity(item, original, workspace);
            }
        }
        _ => {}
    }
    changed
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    use okf_jawn_contract::error::{ApiError, ErrorCode};
    use okf_jawn_contract::identity::{TenantId, WorkspaceId};
    use rusqlite::Connection;

    use super::super::archive::{ARCHIVE_VERSION, WORKSPACE_FORMAT, WorkspaceManifest};
    use super::super::snapshot::{WORKSPACE_TABLES, text};
    use super::{
        Live, Target, insert, parents_first, refuse_foreign_rows, rewrite_identity, schema_of,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The workspace the row-check archives below belong to: its id has letters, so its
    /// upper-cased form differs.
    const WORKSPACE: u128 = 0x0000_0000_0000_4000_8000_0000_0000_abcd;

    /// A records database at this build's schema in `directory/name`, then changed by `sql`
    /// with foreign keys off (this build's SQLite turns them on by default).
    fn records_at(directory: &Path, name: &str, sql: &str) -> Result<PathBuf, ApiError> {
        let path = directory.join(name);
        let mut connection = Connection::open(&path).map_err(|error| super::sql(&error))?;
        crate::schema::RECORDS
            .to_latest(&mut connection)
            .map_err(|error| super::internal(error.to_string()))?;
        connection
            .execute_batch(&format!("PRAGMA foreign_keys = OFF; {sql}"))
            .map_err(|error| super::sql(&error))?;
        Ok(path)
    }

    /// The error a step that must fail gave.
    fn refused(result: Result<(), ApiError>) -> Result<ApiError, Box<dyn std::error::Error>> {
        match result {
            Ok(()) => Err("the step succeeded".into()),
            Err(error) => Ok(error),
        }
    }

    /// Pass-4b re-review MFKMAP: a copy whose foreign key fails is refused as `InvalidInput`
    /// naming the table and the foreign key, and any other constraint failure as `InvalidInput`
    /// too, never as a collision. The archive's schema and rows are checked first, so a crafted
    /// archive no longer reaches either; here the copy of `job_controls` runs before its job is
    /// live, and `jobs` is copied twice.
    #[test]
    fn a_constraint_failure_of_a_copy_is_invalid_input() -> TestResult {
        let directory = tempfile::tempdir()?;
        let archive = records_at(
            directory.path(),
            "archive.sqlite",
            "INSERT INTO jobs (job_id, tenant_id, workspace_id, mutation_id, initiator, spec,
                 record, state) VALUES ('j1', 'local', 'w1', 'm1', '{}', '{}', '{}', 'cancelled');
             INSERT INTO job_controls (mutation_id, job_id, action) VALUES ('m2', 'j1', 'cancel');",
        )?;
        let live = Connection::open(records_at(directory.path(), "live.sqlite", "")?)?;
        live.execute_batch("PRAGMA foreign_keys = ON")?;
        live.execute("ATTACH DATABASE ?1 AS archive", [text(&archive)?])?;
        let target = Target {
            tenant: "local".to_owned(),
            workspace: "w1".to_owned(),
            original: "w1".to_owned(),
            objects: Vec::new(),
            history: BTreeSet::new(),
            followed: BTreeMap::new(),
            replaced_from: None,
            live: Live {
                schema: schema_of(&live)?,
                order: parents_first(&live, WORKSPACE_TABLES)?,
            },
        };
        let error = refused(insert(&live, "job_controls", &target, None))?;
        assert_eq!(error.code, ErrorCode::InvalidInput, "{error}");
        assert!(
            error.message.contains("job_controls") && error.message.contains("foreign key"),
            "{error}"
        );
        insert(&live, "jobs", &target, None)?;
        let error = refused(insert(&live, "jobs", &target, None))?;
        assert_eq!(error.code, ErrorCode::InvalidInput, "{error}");
        assert!(
            error.message.contains("jobs") && error.message.contains("constraint"),
            "{error}"
        );
        Ok(())
    }

    /// Pass-4b re-review B1-R, defence in depth: the row checks compare byte for byte whatever
    /// collation the archive declares. The schema check is not run here: an archive's
    /// `drafts.workspace_id`, then its `job_controls.job_id`, is made case-insensitive, and a
    /// row whose workspace (or job) differs from the manifest's (or its job's) only in case is
    /// still refused.
    #[test]
    fn the_row_checks_compare_bytes_whatever_the_archive_collation() -> TestResult {
        let directory = tempfile::tempdir()?;
        let workspace = uuid::Uuid::from_u128(WORKSPACE).to_string();
        let upper = workspace.to_uppercase();
        let live = schema_of(&Connection::open(records_at(
            directory.path(),
            "live.sqlite",
            "",
        )?)?)?;
        let manifest = WorkspaceManifest {
            format: WORKSPACE_FORMAT.to_owned(),
            version: ARCHIVE_VERSION,
            tenant_id: TenantId::try_from("local".to_owned())?,
            workspace_id: WorkspaceId(uuid::Uuid::from_u128(WORKSPACE)),
            name: "Notes".to_owned(),
            description: String::new(),
            created_at: "2026-10-11T00:00:00.000Z".to_owned(),
            archived_at: None,
            references: Vec::new(),
            objects: Vec::new(),
        };
        let own = format!(
            "INSERT INTO workspaces VALUES ('local', '{workspace}', 'm0', 'Notes', '', 't', NULL);"
        );
        for (table, sql) in [
            (
                "drafts",
                format!(
                    "{own} CREATE TABLE drafts_new (tenant_id TEXT NOT NULL,
                         workspace_id TEXT NOT NULL COLLATE NOCASE, item_id TEXT NOT NULL,
                         editor TEXT NOT NULL, saved_at TEXT NOT NULL, record TEXT NOT NULL,
                         body TEXT NOT NULL, properties TEXT NOT NULL,
                         PRIMARY KEY (tenant_id, workspace_id, item_id, editor)) STRICT;
                     DROP TABLE drafts;
                     ALTER TABLE drafts_new RENAME TO drafts;
                     INSERT INTO drafts VALUES ('local', '{upper}', 'i', 'ana', 't', '{{}}', '',
                         '{{}}');"
                ),
            ),
            (
                "job_controls",
                format!(
                    "{own} CREATE TABLE job_controls_new (mutation_id TEXT PRIMARY KEY,
                         job_id TEXT NOT NULL COLLATE NOCASE REFERENCES jobs (job_id),
                         action TEXT NOT NULL CHECK (action IN ('cancel', 'retry'))) STRICT;
                     DROP TABLE job_controls;
                     ALTER TABLE job_controls_new RENAME TO job_controls;
                     INSERT INTO jobs (job_id, tenant_id, workspace_id, mutation_id, initiator,
                         spec, record, state)
                         VALUES ('job-a', 'local', '{workspace}', 'm1', '{{}}', '{{}}', '{{}}',
                         'cancelled');
                     INSERT INTO job_controls VALUES ('m2', 'JOB-A', 'cancel');"
                ),
            ),
        ] {
            let archive = Connection::open(records_at(
                directory.path(),
                &format!("{table}.sqlite"),
                &sql,
            )?)?;
            let error = refused(refuse_foreign_rows(&archive, &manifest, &live))?;
            assert_eq!(error.code, ErrorCode::InvalidInput, "{table}: {error}");
            assert!(error.message.contains(table), "{table}: {error}");
        }
        Ok(())
    }

    /// Backup re-review R10: the identity is rewritten in nested objects and lists too, and a
    /// text that only quotes it keeps it.
    #[test]
    fn a_nested_identity_field_is_rewritten_and_quoting_text_is_kept() -> TestResult {
        let mut value: serde_json::Value = serde_json::from_str(
            r#"{"source": {"workspace_id": "a", "note": "a"},
                "cited": [{"workspace_id": "a"}],
                "workspace_ids": ["a", "b"],
                "text": "a"}"#,
        )?;
        assert!(rewrite_identity(&mut value, "a", "z"));
        let expected: serde_json::Value = serde_json::from_str(
            r#"{"source": {"workspace_id": "z", "note": "a"},
                "cited": [{"workspace_id": "z"}],
                "workspace_ids": ["z", "b"],
                "text": "a"}"#,
        )?;
        assert_eq!(value, expected);
        Ok(())
    }
}
