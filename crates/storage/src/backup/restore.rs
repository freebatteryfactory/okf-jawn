//! Restoring a workspace archive, as an import into a blank workspace (`restore_import`) or as
//! part of a replacement restore (Stage 1b design section 8).
//!
//! In both, the objects are put back first (each checked against its digest), then the Git
//! history (the pack is indexed and every reference set), then the records in one transaction.
//! A resumed import finds the accepted head already at the archive's and skips the Git step;
//! the records step and the import's own record (`restores`) commit in one transaction, so it
//! is all or nothing.
//!
//! - **Import** keeps item identities and revisions, rewrites the workspace identity in every
//!   imported row (and inside its stored JSON), and gives back only the drafts whose editor is
//!   in `editors`; the others stay in the archive and are counted. It imports drafts, reviews,
//!   receipts, proposals and their comments, derived datasets and the revision map; jobs,
//!   events, grants and uploads belong to the installation the archive came from. An archive
//!   of this tenant is refused when its workspace still exists or a purge names it: purged
//!   content never comes back through an import.
//! - **Replacement** restores the workspace under its own identity with every archived row,
//!   and refuses an archive of another tenant, of a workspace that exists, or of one a
//!   completed purge removed.
//!
//! Known gap: a View's bindings name its workspace, and the on-disk form of a View document
//! is not decided yet (core-cli M1.6), so an import does not rewrite them.

use std::path::Path;

use git2::Repository;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, MutationId};
use okf_jawn_contract::workspace::RestoreReport;
use okf_jawn_core::storage::StorageScope;
use rusqlite::{Connection, params};
use tokio::runtime::Handle;

use super::archive::{Opened, PACK, RECORDS, WorkspaceManifest, invalid, oid, read_pack};
use super::snapshot::{WORKSPACE_TABLES, text};
use crate::blobs::file_reader;
use crate::data::io_error;
use crate::db::{conflict, internal, sql};
use crate::git::repo::{HEAD_REF, git, head, sync_directories};
use crate::pins::Pinned;
use crate::puts;
use crate::storage::Storage;

/// How an archive is restored.
pub(crate) enum Mode {
    /// Into a blank workspace of this installation, for these editors, under this mutation.
    Import(Vec<String>, MutationId),
    /// Under its own identity, after the installation archive.
    Replace,
}

/// Where archived rows go: the target tenant and workspace, and the archive's own workspace
/// identity, which an import rewrites.
struct Target {
    tenant: String,
    workspace: String,
    original: String,
    /// The archive's objects, put back before the rows.
    objects: Vec<Digest>,
}

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
    let mut opened = Opened::open(archive)?;
    let manifest = opened.workspace()?;
    refuse(storage, runtime, scope, &manifest, mode)?;
    // Each object is pinned to the restored workspace while the import runs, and recorded as a
    // pending put (`puts`) until the import's rows land.
    let mut pinned = Vec::new();
    let restored = put_back(
        storage,
        runtime,
        scope,
        (&mut opened, &mut pinned),
        &manifest,
        staging,
        mode,
    );
    for pin in pinned {
        pin.end()?;
    }
    restored
}

/// Put the archive's objects back (each pin kept in `pinned`), then its history and rows.
fn put_back(
    storage: &Storage,
    runtime: &Handle,
    scope: &StorageScope,
    (opened, pinned): (&mut Opened, &mut Vec<Pinned>),
    manifest: &WorkspaceManifest,
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
    let items = history(storage, scope, manifest, &pack, mode)?;
    let records = staging.join(RECORDS);
    opened.extract(RECORDS, &records)?;
    let mut archived = Connection::open(&records).map_err(|error| sql(&error))?;
    crate::schema::RECORDS
        .to_latest(&mut archived)
        .map_err(|error| {
            internal(format!(
                "an archived records database did not migrate: {error}"
            ))
        })?;
    drop(archived);
    let report = {
        let path = text(&records)?.to_owned();
        let target = Target {
            tenant: scope.tenant_id.as_str().to_owned(),
            workspace: scope.workspace_id.0.to_string(),
            original: manifest.workspace_id.0.to_string(),
            objects: manifest.objects.clone(),
        };
        let editors = match mode {
            Mode::Import(editors, _) => {
                Some(serde_json::to_string(editors).map_err(|error| crate::db::json(&error))?)
            }
            Mode::Replace => None,
        };
        let mutation = match mode {
            Mode::Import(_, mutation) => Some(*mutation),
            Mode::Replace => None,
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

/// Refuse an archive the mode may not restore into `scope`.
fn refuse(
    storage: &Storage,
    runtime: &Handle,
    scope: &StorageScope,
    manifest: &WorkspaceManifest,
    mode: &Mode,
) -> Result<(), ApiError> {
    let tenant = manifest.tenant_id.as_str().to_owned();
    let workspace = manifest.workspace_id.0.to_string();
    let (exists, purged, completed) = runtime.block_on(storage.records.call(move |connection| {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM workspaces WHERE tenant_id = ?1 AND workspace_id = ?2)",
                params![tenant, workspace],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        let key = format!("workspace/{workspace}");
        let (purged, completed): (bool, bool) = connection
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM purges WHERE tenant_id = ?1 AND target_key = ?2),
                        EXISTS (SELECT 1 FROM purges WHERE tenant_id = ?1 AND target_key = ?2
                                  AND state = 'completed')",
                params![tenant, key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| sql(&error))?;
        Ok((exists, purged, completed))
    }))?;
    match mode {
        Mode::Import(..) => {
            if manifest.tenant_id == scope.tenant_id && (exists || purged) {
                return Err(conflict(
                    "an archive of a workspace this installation still holds or purged is not \
                     imported; purged content never comes back through an import",
                ));
            }
        }
        Mode::Replace => {
            if manifest.tenant_id != scope.tenant_id {
                return Err(invalid(
                    "it belongs to another tenant than the installation archive",
                ));
            }
            if completed {
                return Err(conflict(
                    "the installation archive records a purge of this workspace",
                ));
            }
            if exists || storage.repositories.path(scope).exists() {
                return Err(conflict("this workspace already exists here"));
            }
        }
    }
    Ok(())
}

/// Index the pack and set every reference; returns the item count at the accepted head.
fn history(
    storage: &Storage,
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
        Mode::Import(..) => storage.repositories.open_durable(scope)?,
        Mode::Replace => storage.repositories.create(scope)?,
    };
    let current = crate::git::repo::reference_target(&repository, HEAD_REF)?;
    if current != Some(main) {
        if let Some(current) = current {
            refuse_unless_blank(&repository, current)?;
        }
        read_pack(&repository, pack)?;
        for (name, commit) in &manifest.references {
            let target = oid(commit)?;
            let moved = match (name.as_str(), current) {
                (HEAD_REF, Some(current)) => {
                    repository.reference_matching(name, target, true, current, "okf-jawn restore")
                }
                _ => repository.reference(name, target, true, "okf-jawn restore"),
            };
            moved.map_err(|error| git(&error))?;
        }
        repository.set_head(HEAD_REF).map_err(|error| git(&error))?;
        sync_directories(&repository, &["objects", "refs", "refs/heads"])?;
    }
    let tree = repository
        .find_commit(head(&repository)?)
        .and_then(|commit| commit.tree())
        .map_err(|error| git(&error))?;
    u32::try_from(crate::git::items_of(&repository, &tree)?.len())
        .map_err(|_| internal("too many items to count"))
}

/// Refuse a target workspace that holds anything beyond its creation commit.
fn refuse_unless_blank(repository: &Repository, current: git2::Oid) -> Result<(), ApiError> {
    let commit = repository
        .find_commit(current)
        .map_err(|error| git(&error))?;
    let tree = commit.tree().map_err(|error| git(&error))?;
    let references = repository
        .references()
        .map_err(|error| git(&error))?
        .count();
    if commit.parent_count() == 0 && tree.is_empty() && references == 1 {
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
    let tables: &[&str] = if editors.is_some() {
        IMPORTED
    } else {
        WORKSPACE_TABLES
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
    // The archive's objects are referenced now: their pending puts land with the rows. A
    // failed import rolls this back with the rows, so its pending puts stay and a purge of
    // the workspace deletes the bytes.
    puts::land(
        transaction,
        &target.tenant,
        &target.workspace,
        &target.objects,
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
    let columns: Vec<String> = {
        let mut statement = connection
            .prepare("SELECT name FROM pragma_table_info(?1)")
            .map_err(|error| sql(&error))?;
        statement
            .query_map([table], |row| row.get(0))
            .map_err(|error| sql(&error))?
            .collect::<Result<_, _>>()
            .map_err(|error| sql(&error))?
    };
    // An event's sequence number is the live log's; it is assigned again.
    let columns: Vec<String> = columns
        .into_iter()
        .filter(|name| !(table == "events" && name == "seq"))
        .collect();
    let rewrite = editors.is_some();
    let values: Vec<String> = columns
        .iter()
        .map(|column| match column.as_str() {
            "tenant_id" if rewrite => "?1".to_owned(),
            "workspace_id" if rewrite => "?2".to_owned(),
            "record" if rewrite => "replace(record, ?3, ?2)".to_owned(),
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
        .map(|_| ())
        .map_err(|error| match error {
            rusqlite::Error::SqliteFailure(failure, _)
                if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                conflict(format!(
                    "the archive's {table} collide with records this installation already holds"
                ))
            }
            other => sql(&other),
        })
}
