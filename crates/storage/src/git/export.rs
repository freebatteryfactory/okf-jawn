//! The retained history of the accepted line as a Git bundle (`VersionStore::write_history`).
//!
//! libgit2 writes no bundle, so the bundle is the version 2 format `git bundle` writes: the
//! line `# v2 git bundle`, one `<commit> <reference>` line for `HEAD` and for
//! `refs/heads/main`, an empty line, then a pack that libgit2's `PackBuilder` streams. The pack
//! holds the commits reachable from the revision and every object they reference, and nothing
//! else: no proposal or candidate reference is walked, and a revision that is not on the
//! accepted line (a candidate, or one a purge rewrote away) is refused, so purged content never
//! enters it. Stock `git clone <file>` makes a repository of it.
//!
//! The written object is recorded under the mutation id in the records database after it is
//! retained; a repeated id writes nothing and returns the object recorded first.

use std::io::Write as _;
use std::path::Path;

use git2::{Oid, Repository};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{MutationId, Revision};
use okf_jawn_core::storage::{ObjectInfo, StorageScope};
use rusqlite::{Connection, OptionalExtension, params};

use super::repo::{HEAD_REF, commit_of, git, head};
use crate::data::io_error;
use crate::db::{Db, scope_key, sql, to_i64, to_u64};

/// First line of a version 2 bundle.
const BUNDLE_SIGNATURE: &str = "# v2 git bundle\n";
/// Name of the bundle file inside the mutation's staging directory.
pub(crate) const BUNDLE_FILE: &str = "history.bundle";

/// The object an earlier call under `mutation` recorded, if any.
pub(crate) async fn recorded(
    db: &Db,
    scope: &StorageScope,
    mutation: MutationId,
) -> Result<Option<ObjectInfo>, ApiError> {
    let (tenant, workspace) = scope_key(scope);
    db.call(move |connection| read(connection, &tenant, &workspace, mutation))
        .await
}

/// Record the retained bundle under `mutation`, or return the object recorded first.
pub(crate) async fn record(
    db: &Db,
    scope: &StorageScope,
    mutation: MutationId,
    revision: &Revision,
    object: ObjectInfo,
) -> Result<ObjectInfo, ApiError> {
    let (tenant, workspace) = scope_key(scope);
    let revision = revision.as_str().to_owned();
    db.transaction(move |transaction| {
        transaction
            .execute(
                "INSERT INTO history_exports (mutation_id, tenant_id, workspace_id, revision,
                     digest, size)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (mutation_id) DO NOTHING",
                params![
                    mutation.0.to_string(),
                    tenant,
                    workspace,
                    revision,
                    object.digest.as_str(),
                    to_i64(object.size)?
                ],
            )
            .map_err(|error| sql(&error))?;
        read(transaction, &tenant, &workspace, mutation)?.ok_or_else(|| {
            ApiError::new(
                ErrorCode::Conflict,
                "this mutation id already wrote the history of another workspace",
            )
        })
    })
    .await
}

/// Write the bundle of the accepted line up to `revision` to `target`; returns its size.
pub(crate) fn write_bundle(
    repository: &Repository,
    revision: &Revision,
    target: &Path,
) -> Result<u64, ApiError> {
    let tip = commit_of(repository, revision)?.id();
    refuse_off_the_line(repository, tip)?;
    let mut walk = repository.revwalk().map_err(|error| git(&error))?;
    walk.push(tip).map_err(|error| git(&error))?;
    let mut pack = repository.packbuilder().map_err(|error| git(&error))?;
    pack.set_threads(1);
    pack.insert_walk(&mut walk).map_err(|error| git(&error))?;
    let mut file =
        std::fs::File::create(target).map_err(|error| io_error("create", target, &error))?;
    let header = format!("{BUNDLE_SIGNATURE}{tip} HEAD\n{tip} {HEAD_REF}\n\n");
    file.write_all(header.as_bytes())
        .map_err(|error| io_error("write", target, &error))?;
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
    let size = file
        .metadata()
        .map_err(|error| io_error("inspect", target, &error))?
        .len();
    Ok(size)
}

/// Refuse a revision that the accepted head does not contain.
fn refuse_off_the_line(repository: &Repository, tip: Oid) -> Result<(), ApiError> {
    let accepted = head(repository)?;
    let on_line = accepted == tip
        || repository
            .graph_descendant_of(accepted, tip)
            .map_err(|error| git(&error))?;
    if on_line {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::NotFound,
            "No revision with that identity on the accepted line here",
        )
        .with_field("/revision"))
    }
}

fn read(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
    mutation: MutationId,
) -> Result<Option<ObjectInfo>, ApiError> {
    let found: Option<(String, i64)> = connection
        .query_row(
            "SELECT digest, size FROM history_exports
             WHERE mutation_id = ?1 AND tenant_id = ?2 AND workspace_id = ?3",
            params![mutation.0.to_string(), tenant, workspace],
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
}
