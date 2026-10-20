//! One tenant's reachability collection, shared by workspace purge and durable clean-up.
//!
//! Register the pin watch before reading one records snapshot and every surviving history.
//! Only then stream canonical CAS filenames in bounded batches. Deletions are recorded both
//! before and after filesystem work so a retry reconciles an interrupted batch exactly.

use std::collections::BTreeSet;
use std::path::Path;

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::{Digest, TenantId, WorkspaceId};
use okf_jawn_core::storage::StorageScope;
use rusqlite::{Connection, params};

use crate::data::io_error;
use crate::db::{internal, sql};
use crate::pins::Sweep;
use crate::purge::references;
use crate::storage::Storage;

/// A durable collection identity and the workspace a purge excludes, if any.
#[derive(Clone)]
pub(crate) struct SweepRun {
    pub(crate) tenant: TenantId,
    pub(crate) target: Option<WorkspaceId>,
    pub(crate) key: String,
    /// Only upload/pending references to the removed workspace's archives are excluded.
    pub(crate) excluded: BTreeSet<Digest>,
}

/// Removed digests across this run's attempts; kept candidates of this attempt only.
#[derive(Debug)]
pub(crate) struct SweepCounts {
    pub(crate) removed: u64,
    pub(crate) kept: u64,
    /// Kept until the caller durably records counts and clears this run's progress.
    _watch: Sweep,
    _serial: tokio::sync::OwnedMutexGuard<()>,
}

/// Durable roots and live workspace identities read under one records-connection hold.
struct Roots {
    digests: BTreeSet<[u8; 32]>,
    workspaces: Vec<StorageScope>,
}

/// No records transaction or pin lock is held while this stream advances a directory.
struct Collector {
    storage: Storage,
    run: SweepRun,
    watch: Sweep,
    marked: BTreeSet<[u8; 32]>,
    pending: Vec<Digest>,
    kept: u64,
}

/// The largest pending-delete transaction and interrupted filesystem batch.
const BATCH: usize = 256;
/// Recorded workspace purges exclude a workspace from every root query, in every state.
const SURVIVOR: &str = "(?2 IS NULL OR workspace_id IS NOT ?2) AND (workspace_id IS NULL OR
    workspace_id NOT IN (SELECT substr(target_key,11) FROM purges WHERE tenant_id=?1
    AND target_key LIKE 'workspace/%'))";

impl Collector {
    fn visit(&mut self, digest: Digest) -> Result<(), ApiError> {
        if self.marked.contains(&digest_key(&digest)?) {
            self.kept = self.kept.saturating_add(1);
        } else {
            self.pending.push(digest);
            if self.pending.len() == BATCH {
                self.flush()?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), ApiError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let runtime = tokio::runtime::Handle::current();
        let (key, tenant, digests) = (
            self.run.key.clone(),
            self.run.tenant.as_str().to_owned(),
            self.pending.clone(),
        );
        runtime.block_on(self.storage.records.transaction(move |transaction| {
            for digest in digests {
                transaction
                    .execute(
                        "INSERT INTO sweep_removals (sweep,tenant_id,digest,removed)
                    VALUES (?1,?2,?3,0) ON CONFLICT DO NOTHING",
                        params![key, tenant, digest.as_str()],
                    )
                    .map_err(|error| sql(&error))?;
            }
            Ok(())
        }))?;
        let mut outcomes = Vec::with_capacity(self.pending.len());
        for digest in &self.pending {
            let removed = self.watch.delete_unless_touched(digest, || {
                self.storage.blobs.remove_for(&self.run.tenant, digest)
            })?;
            if removed.is_none() {
                self.kept = self.kept.saturating_add(1);
            }
            outcomes.push((digest.clone(), removed == Some(true)));
        }
        #[cfg(test)]
        runtime
            .block_on(crate::pause::point(&pause_point(
                &self.storage,
                &self.run,
                "batch",
            )))
            .map_err(|_| {
                okf_jawn_contract::error::ApiError::new(
                    okf_jawn_contract::error::ErrorCode::Unavailable,
                    "the sweep stopped after its test batch",
                )
            })?;
        let (key, tenant) = (self.run.key.clone(), self.run.tenant.as_str().to_owned());
        runtime.block_on(self.storage.records.transaction(move |transaction| {
            for (digest, removed) in outcomes {
                if removed {
                    transaction
                        .execute(
                            "UPDATE sweep_removals SET removed=1
                        WHERE sweep=?1 AND tenant_id=?2 AND digest=?3",
                            params![key, tenant, digest.as_str()],
                        )
                        .map_err(|error| sql(&error))?;
                } else {
                    transaction
                        .execute(
                            "DELETE FROM sweep_removals
                        WHERE sweep=?1 AND tenant_id=?2 AND digest=?3",
                            params![key, tenant, digest.as_str()],
                        )
                        .map_err(|error| sql(&error))?;
                }
            }
            Ok(())
        }))?;
        self.pending.clear();
        Ok(())
    }
}

/// Transfer serialization into the blocking worker before its first filesystem or Git work.
/// Aborting the async waiter detaches that worker; it must keep its lock until work ends.
pub(crate) async fn run(storage: &Storage, run: SweepRun) -> Result<SweepCounts, ApiError> {
    let serial = storage.blobs.pins().serial(run.tenant.as_str()).await?;
    let storage = storage.clone();
    blocking(move || collect(&storage, run, serial)).await
}

/// The worker owns the tenant lock throughout every blocking phase and returns it with the
/// watch for the caller's final report transaction. No detached phase can outlive this lock.
fn collect(
    storage: &Storage,
    run: SweepRun,
    serial: tokio::sync::OwnedMutexGuard<()>,
) -> Result<SweepCounts, ApiError> {
    let runtime = tokio::runtime::Handle::current();
    let target = run.target.map(|id| id.0.to_string());
    let (watch, touched) = storage
        .blobs
        .pins()
        .sweep_for(run.tenant.as_str(), target.as_deref())?;
    #[cfg(test)]
    runtime.block_on(crate::pause::point(&pause_point(
        storage,
        &run,
        "registered",
    )))?;
    runtime.block_on(reconcile(storage, &run))?;
    let roots = runtime.block_on(storage.records.call({
        let run = run.clone();
        move |connection| snapshot(connection, &run)
    }))?;
    let marked = mark(storage, &run.tenant, roots, touched)?;
    #[cfg(test)]
    runtime.block_on(crate::pause::point(&pause_point(storage, &run, "marked")))?;
    let directory = storage.data.objects_path().join(run.tenant.as_str());
    let mut collector = Collector {
        storage: storage.clone(),
        run: run.clone(),
        watch,
        marked,
        pending: Vec::with_capacity(BATCH),
        kept: 0,
    };
    candidates(&directory, |digest| collector.visit(digest))?;
    collector.flush()?;
    #[cfg(test)]
    runtime.block_on(crate::pause::point(&pause_point(
        storage,
        &run,
        "collected",
    )))?;
    let (tenant, key) = (run.tenant.as_str().to_owned(), run.key);
    let removed =
        runtime.block_on(storage.records.call(move |connection| {
            let count: i64 = connection.query_row(
            "SELECT count(*) FROM sweep_removals WHERE sweep=?1 AND tenant_id=?2 AND removed=1",
            params![key, tenant], |row| row.get(0),
        ).map_err(|error| sql(&error))?;
            u64::try_from(count).map_err(|_| internal("a sweep removal count is negative"))
        }))?;
    Ok(SweepCounts {
        removed,
        kept: collector.kept,
        _serial: serial,
        _watch: collector.watch,
    })
}

/// Reconcile only the unfinished batch. Existing removed rows survive until the caller
/// records the run's report. File existence is read off the records connection.
async fn reconcile(storage: &Storage, run: &SweepRun) -> Result<(), ApiError> {
    let (tenant, key) = (run.tenant.as_str().to_owned(), run.key.clone());
    let pending = storage
        .records
        .call(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT digest FROM sweep_removals
            WHERE sweep=?1 AND tenant_id=?2 AND removed=0",
                )
                .map_err(|error| sql(&error))?;
            statement
                .query_map(params![key, tenant], |row| row.get::<_, String>(0))
                .map_err(|error| sql(&error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| sql(&error))
        })
        .await?;
    let present = blocking({
        let (blobs, tenant) = (storage.blobs.clone(), run.tenant.clone());
        move || {
            pending
                .into_iter()
                .map(|text| {
                    let digest = Digest::try_from(text)
                        .map_err(|_| internal("sweep progress holds an invalid object identity"))?;
                    let path = blobs.local_path(&tenant, &digest)?;
                    match std::fs::metadata(&path) {
                        Ok(metadata) => Ok((digest, metadata.is_file())),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            Ok((digest, false))
                        }
                        Err(error) => Err(io_error("read", &path, &error)),
                    }
                })
                .collect::<Result<Vec<_>, ApiError>>()
        }
    })
    .await?;
    let (tenant, key) = (run.tenant.as_str().to_owned(), run.key.clone());
    storage.records.transaction(move|transaction| {
        for (digest,present) in present {
            let query=if present {"DELETE FROM sweep_removals WHERE sweep=?1 AND tenant_id=?2 AND digest=?3"}
                else {"UPDATE sweep_removals SET removed=1 WHERE sweep=?1 AND tenant_id=?2 AND digest=?3"};
            transaction.execute(query,params![key,tenant,digest.as_str()]).map_err(|error|sql(&error))?;
        }
        Ok(())
    }).await
}

/// All typed record roots are read while this single call holds the shared connection.
fn snapshot(connection: &Connection, run: &SweepRun) -> Result<Roots, ApiError> {
    let mut roots = Roots {
        digests: BTreeSet::new(),
        workspaces: Vec::new(),
    };
    for (column, table, excluded) in [
        ("digest", "pending_puts", true),
        ("object_digest", "uploads", true),
        ("digest", "backup_archives", false),
        ("digest", "history_exports", false),
        ("digest", "artifacts", false),
        ("digest", "derived_objects", false),
        ("object", "sandbox_capabilities", false),
    ] {
        record_roots(
            connection,
            run,
            table,
            &format!(
                "SELECT workspace_id,{column} FROM {table}
            WHERE tenant_id=?1 AND {SURVIVOR} AND {column} IS NOT NULL"
            ),
            excluded,
            &mut roots.digests,
        )?;
    }
    record_roots(
        connection,
        run,
        "job_outputs",
        &format!(
            "SELECT workspace_id,outputs.digest FROM job_outputs AS outputs
        JOIN jobs ON outputs.job_id=jobs.job_id WHERE tenant_id=?1 AND {SURVIVOR}"
        ),
        false,
        &mut roots.digests,
    )?;
    restore_roots(connection, run, &mut roots.digests)?;
    let mut statement = connection
        .prepare(&format!(
            "SELECT workspace_id FROM workspaces
        WHERE tenant_id=?1 AND {SURVIVOR}"
        ))
        .map_err(|error| sql(&error))?;
    let target = run.target.map(|id| id.0.to_string());
    let workspaces = statement
        .query_map(params![run.tenant.as_str(), target], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| sql(&error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    for workspace in workspaces {
        roots.workspaces.push(StorageScope {
            tenant_id: run.tenant.clone(),
            workspace_id: uuid::Uuid::parse_str(&workspace)
                .map(WorkspaceId)
                .map_err(|_| {
                    internal("the tenant's workspace records hold an invalid workspace identity")
                })?,
        });
    }
    Ok(roots)
}

fn record_roots(
    connection: &Connection,
    run: &SweepRun,
    table: &str,
    query: &str,
    excluded: bool,
    found: &mut BTreeSet<[u8; 32]>,
) -> Result<(), ApiError> {
    let mut statement = connection.prepare(query).map_err(|error| sql(&error))?;
    let target = run.target.map(|id| id.0.to_string());
    let mut rows = statement
        .query(params![run.tenant.as_str(), target])
        .map_err(|error| sql(&error))?;
    while let Some(row) = rows.next().map_err(|error| sql(&error))? {
        let workspace: Option<String> = row.get(0).map_err(|_| {
            internal(format!(
                "the tenant's {table} records hold an invalid workspace"
            ))
        })?;
        let location = workspace
            .as_deref()
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
            .map(WorkspaceId)
            .map_or_else(
                || "the tenant's records".to_owned(),
                |id| format!("workspace {}", id.0),
            );
        let text: String = row.get(1).map_err(|_| {
            internal(format!(
                "in {location}, {table} holds an invalid object reference"
            ))
        })?;
        let digest = Digest::try_from(text).map_err(|_| {
            internal(format!(
                "in {location}, {table} holds an invalid object reference"
            ))
        })?;
        if !excluded || !run.excluded.contains(&digest) {
            found.insert(digest_key(&digest)?);
        }
    }
    Ok(())
}

fn restore_roots(
    connection: &Connection,
    run: &SweepRun,
    found: &mut BTreeSet<[u8; 32]>,
) -> Result<(), ApiError> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT job_id,spec FROM jobs WHERE tenant_id=?1
        AND state IN ('queued','running') AND {SURVIVOR}"
        ))
        .map_err(|error| sql(&error))?;
    let target = run.target.map(|id| id.0.to_string());
    let mut rows = statement
        .query(params![run.tenant.as_str(), target])
        .map_err(|error| sql(&error))?;
    while let Some(row) = rows.next().map_err(|error| sql(&error))? {
        let job: String = row.get(0).map_err(|error| sql(&error))?;
        let job = uuid::Uuid::parse_str(&job)
            .map(okf_jawn_contract::identity::JobId)
            .map_err(|_| internal("the tenant's job records hold an invalid job identity"))?;
        let spec: String = row.get(1).map_err(|error| sql(&error))?;
        let spec: serde_json::Value = serde_json::from_str(&spec)
            .map_err(|_| internal(format!("job {} has an unreadable specification", job.0)))?;
        if spec.get("kind").and_then(serde_json::Value::as_str) != Some("restore_workspace") {
            continue;
        }
        let digest = spec
            .get("archive")
            .and_then(serde_json::Value::as_str)
            .and_then(|text| Digest::try_from(text.to_owned()).ok())
            .ok_or_else(|| {
                internal(format!(
                    "job {} holds an invalid restore archive reference",
                    job.0
                ))
            })?;
        found.insert(digest_key(&digest)?);
    }
    Ok(())
}

fn mark(
    storage: &Storage,
    tenant: &TenantId,
    roots: Roots,
    touched: BTreeSet<Digest>,
) -> Result<BTreeSet<[u8; 32]>, ApiError> {
    let mut found = roots.digests;
    let mut followed = BTreeSet::new();
    for scope in roots.workspaces {
        let workspace = scope.workspace_id.0.to_string();
        let repository = storage
            .repositories
            .open(&scope)
            .map_err(|error| references::naming(&workspace, error))?;
        let history = references::history_naming(&repository, &workspace)?;
        for digest in history.objects.into_keys() {
            found.insert(digest_key(&digest)?);
        }
        let records = history.records.difference(&followed).cloned().collect();
        let mut assets = BTreeSet::new();
        references::follow_records(
            &storage.blobs,
            tenant,
            &records,
            &mut assets,
            &mut references::refusing_in(&workspace),
        )?;
        for digest in assets {
            found.insert(digest_key(&digest)?);
        }
        followed.extend(records);
    }
    for digest in touched {
        found.insert(digest_key(&digest)?);
    }
    Ok(found)
}
/// Only canonical lower-case digest files in their exact shard are candidates. Symlinks,
/// staging files and malformed names are neither removed nor counted.
fn candidates(
    directory: &Path,
    mut visit: impl FnMut(Digest) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    let shards = match std::fs::read_dir(directory) {
        Ok(shards) => shards,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error("list", directory, &error)),
    };
    for shard in shards {
        let shard = shard.map_err(|error| io_error("list", directory, &error))?;
        if !shard
            .file_type()
            .map_err(|error| io_error("read", &shard.path(), &error))?
            .is_dir()
        {
            continue;
        }
        let name = shard.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.len() != 2
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            continue;
        }
        for entry in std::fs::read_dir(shard.path())
            .map_err(|error| io_error("list", &shard.path(), &error))?
        {
            let entry = entry.map_err(|error| io_error("list", &shard.path(), &error))?;
            if !entry
                .file_type()
                .map_err(|error| io_error("read", &entry.path(), &error))?
                .is_file()
            {
                continue;
            }
            let file = entry.file_name();
            let Some(file) = file.to_str() else {
                continue;
            };
            if !file.starts_with(name) {
                continue;
            }
            if let Ok(digest) = Digest::try_from(file.to_owned()) {
                visit(digest)?;
            }
        }
    }
    Ok(())
}

fn digest_key(digest: &Digest) -> Result<[u8; 32], ApiError> {
    let mut key = [0; 32];
    for (slot, pair) in key
        .iter_mut()
        .zip(digest.as_str().as_bytes().as_chunks::<2>().0.iter())
    {
        let text = std::str::from_utf8(pair)
            .map_err(|_| internal("an object identity is not hexadecimal"))?;
        *slot = u8::from_str_radix(text, 16)
            .map_err(|_| internal("an object identity is not hexadecimal"))?;
    }
    Ok(key)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| internal(format!("a sweep task stopped: {error}")))?
}

#[cfg(test)]
fn pause_point(storage: &Storage, run: &SweepRun, stage: &str) -> String {
    format!(
        "sweep|{}|{}|{stage}",
        storage.data.root().display(),
        run.key
    )
}
