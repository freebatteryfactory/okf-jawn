//! What a workspace references in the tenant's shared object store.
//!
//! Objects are shared inside a tenant (`BlobStore`), so a purge may delete an object only when
//! no surviving workspace and no tenant record references it. A workspace references an object
//! through its Git history (the source appearance of every card, on every commit of every
//! reference, proposal candidates included), through the conversion record such an appearance
//! names (its structured exports and assets), and through its records (derived datasets,
//! retained artifacts, uploads, sandbox capabilities, written histories, written workspace
//! archives and job outputs).
//!
//! References are read as every 64-character lower-case hexadecimal token of those files, not
//! through one decoded shape: a file written by an older build still counts, and a token that
//! only looks like a digest can only keep an object, or name one the target already held. A
//! conversion record is followed one level, as an object whose bytes start with `{`.

use std::collections::BTreeSet;
use std::io::Read as _;
use std::path::Path;

use git2::{ObjectType, Oid, Repository, Sort};
use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::{Digest, TenantId};
use rusqlite::{Connection, params};

use crate::blobs::LocalBlobs;
use crate::db::sql;
use crate::git::repo::git;

/// Every commit of a repository, on every reference, and the objects its cards reference.
#[derive(Debug, Default)]
pub(crate) struct History {
    /// Every commit reachable from any reference.
    pub(crate) revisions: Vec<Oid>,
    /// Digest tokens of every source appearance on those commits.
    pub(crate) objects: BTreeSet<Digest>,
}

/// Which rows of the tenant a record query reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rows {
    /// The target workspace's rows.
    Target,
    /// Every other row of the tenant: other workspaces and tenant records.
    Survivors,
}

/// The largest object read as a possible conversion record.
const RECORD_LIMIT: u64 = 64 << 20;
/// The tree that holds the source appearance of each card.
const SOURCES_TREE: &str = ".okf/sources";
/// The length of a SHA-256 digest in hexadecimal.
const DIGEST_LENGTH: usize = 64;

/// Walk every commit on every reference of `repository`, collecting the commits and the
/// digest tokens of every source appearance. Each distinct appearance tree and file is read
/// once, so a long history with few changed cards costs little more than the walk.
pub(crate) fn history(repository: &Repository) -> Result<History, ApiError> {
    let mut walk = repository.revwalk().map_err(|error| git(&error))?;
    walk.set_sorting(Sort::TOPOLOGICAL)
        .map_err(|error| git(&error))?;
    let mut pushed = false;
    for reference in repository.references().map_err(|error| git(&error))? {
        let reference = reference.map_err(|error| git(&error))?;
        if let Some(target) = reference.target()
            && repository.find_commit(target).is_ok()
        {
            walk.push(target).map_err(|error| git(&error))?;
            pushed = true;
        }
    }
    let mut found = History::default();
    if !pushed {
        return Ok(found);
    }
    let mut trees = BTreeSet::new();
    let mut files = BTreeSet::new();
    for oid in walk {
        let oid = oid.map_err(|error| git(&error))?;
        found.revisions.push(oid);
        let Some(sources) = sources_tree(repository, oid)? else {
            continue;
        };
        if trees.insert(sources) {
            scan(repository, sources, &mut files, &mut found.objects)?;
        }
    }
    Ok(found)
}

/// The objects the source cards of every commit on every reference of `repository` name, read
/// structurally rather than as tokens: each appearance's original (`object`) and its conversion
/// record (`extraction.digest`), as (originals, conversion records). An appearance that does not
/// parse as JSON names nothing here.
pub(crate) fn card_objects(
    repository: &Repository,
) -> Result<(BTreeSet<Digest>, BTreeSet<Digest>), ApiError> {
    let (mut originals, mut records) = (BTreeSet::new(), BTreeSet::new());
    let mut walk = repository.revwalk().map_err(|error| git(&error))?;
    let mut pushed = false;
    for reference in repository.references().map_err(|error| git(&error))? {
        let reference = reference.map_err(|error| git(&error))?;
        if let Some(target) = reference.target()
            && repository.find_commit(target).is_ok()
        {
            walk.push(target).map_err(|error| git(&error))?;
            pushed = true;
        }
    }
    if !pushed {
        return Ok((originals, records));
    }
    let (mut trees, mut files) = (BTreeSet::new(), BTreeSet::new());
    for oid in walk {
        let oid = oid.map_err(|error| git(&error))?;
        let Some(sources) = sources_tree(repository, oid)? else {
            continue;
        };
        if !trees.insert(sources) {
            continue;
        }
        let sources = repository.find_tree(sources).map_err(|error| git(&error))?;
        for entry in &sources {
            if entry.kind() != Some(ObjectType::Blob) || !files.insert(entry.id()) {
                continue;
            }
            let blob = repository
                .find_blob(entry.id())
                .map_err(|error| git(&error))?;
            let Ok(appearance) = serde_json::from_slice::<serde_json::Value>(blob.content()) else {
                continue;
            };
            if let Some(digest) = digest_at(&appearance, &["object"]) {
                originals.insert(digest);
            }
            if let Some(digest) = digest_at(&appearance, &["extraction", "digest"]) {
                records.insert(digest);
            }
        }
    }
    Ok((originals, records))
}

/// The objects the held conversion record `digest` names, read structurally: each structured
/// export (`structured[].digest`) and each retained asset (`assets[].digest`), with what names
/// it. Nothing for an object that is not a JSON conversion record. Blocking.
pub(crate) fn conversion_objects(
    blobs: &LocalBlobs,
    tenant: &TenantId,
    digest: &Digest,
) -> Result<Vec<(Digest, &'static str)>, ApiError> {
    let path = blobs.local_path(tenant, digest)?;
    let mut found = Vec::new();
    let Ok(metadata) = std::fs::metadata(&path) else {
        return Ok(found);
    };
    if metadata.len() > RECORD_LIMIT {
        return Ok(found);
    }
    let bytes =
        std::fs::read(&path).map_err(|error| crate::data::io_error("read", &path, &error))?;
    let Ok(record) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(found);
    };
    for (list, what) in [
        ("structured", "a conversion record's structured export"),
        ("assets", "a conversion record's retained asset"),
    ] {
        for item in record
            .get(list)
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(named) = digest_at(item, &["digest"]) {
                found.push((named, what));
            }
        }
    }
    Ok(found)
}

/// The digest at `path` inside `value`, when it is a digest string.
fn digest_at(value: &serde_json::Value, path: &[&str]) -> Option<Digest> {
    let mut current = value;
    for key in path {
        current = current.get(key)?;
    }
    Digest::try_from(current.as_str()?.to_owned()).ok()
}

/// The digest tokens of every source appearance on the one commit `commit`: the objects a
/// record of the workspace names once that commit is stored.
pub(crate) fn appearances(
    repository: &Repository,
    commit: Oid,
) -> Result<BTreeSet<Digest>, ApiError> {
    let mut found = BTreeSet::new();
    if let Some(sources) = sources_tree(repository, commit)? {
        scan(repository, sources, &mut BTreeSet::new(), &mut found)?;
    }
    Ok(found)
}

/// The tree of source appearances on `commit`, when it has one.
fn sources_tree(repository: &Repository, commit: Oid) -> Result<Option<Oid>, ApiError> {
    let tree = repository
        .find_commit(commit)
        .and_then(|commit| commit.tree())
        .map_err(|error| git(&error))?;
    match tree.get_path(Path::new(SOURCES_TREE)) {
        Ok(entry) => Ok(Some(entry.id())),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(git(&error)),
    }
}

/// Add to `objects` the digest tokens of every file of the appearance tree `sources` not
/// already in `files`.
fn scan(
    repository: &Repository,
    sources: Oid,
    files: &mut BTreeSet<Oid>,
    objects: &mut BTreeSet<Digest>,
) -> Result<(), ApiError> {
    let sources = repository.find_tree(sources).map_err(|error| git(&error))?;
    for entry in &sources {
        if entry.kind() == Some(ObjectType::Blob) && files.insert(entry.id()) {
            let blob = repository
                .find_blob(entry.id())
                .map_err(|error| git(&error))?;
            tokens(blob.content(), objects);
        }
    }
    Ok(())
}

/// Add to `objects` the digest tokens of every held object of `objects` that reads as a
/// conversion record (at most `RECORD_LIMIT` bytes, starting with `{`). Blocking.
pub(crate) fn follow_records(
    blobs: &LocalBlobs,
    tenant: &TenantId,
    objects: &mut BTreeSet<Digest>,
) -> Result<(), ApiError> {
    let mut found = BTreeSet::new();
    for digest in objects.iter() {
        found.extend(record_tokens(blobs, tenant, digest)?);
    }
    objects.extend(found);
    Ok(())
}

/// The digest tokens of the held object `digest` when it reads as a conversion record (at most
/// `RECORD_LIMIT` bytes, starting with `{`); none otherwise. Blocking.
pub(crate) fn record_tokens(
    blobs: &LocalBlobs,
    tenant: &TenantId,
    digest: &Digest,
) -> Result<BTreeSet<Digest>, ApiError> {
    let mut found = BTreeSet::new();
    let path = blobs.local_path(tenant, digest)?;
    let Ok(metadata) = std::fs::metadata(&path) else {
        return Ok(found);
    };
    if metadata.len() > RECORD_LIMIT {
        return Ok(found);
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|error| crate::data::io_error("read", &path, &error))?;
    if bytes
        .iter()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| *byte == b'{')
    {
        tokens(&bytes, &mut found);
    }
    Ok(found)
}

/// The objects the records of `rows` reference. A survivor's upload slot whose object is in
/// `not_surviving` (an archive of the target) is not a surviving reference (Stage 1b design
/// section 6).
pub(crate) fn records(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
    rows: Rows,
    not_surviving: &BTreeSet<Digest>,
) -> Result<BTreeSet<Digest>, ApiError> {
    let side = match rows {
        Rows::Target => "workspace_id = ?2",
        Rows::Survivors => "workspace_id IS NOT ?2",
    };
    let mut found = BTreeSet::new();
    for (column, table) in [
        ("digest", "derived_objects"),
        ("digest", "artifacts"),
        ("object", "sandbox_capabilities"),
        ("digest", "history_exports"),
        ("digest", "backup_archives"),
    ] {
        collect(
            connection,
            &format!("SELECT {column} FROM {table} WHERE tenant_id = ?1 AND {side}"),
            tenant,
            workspace,
            &mut found,
        )?;
    }
    collect(
        connection,
        &format!(
            "SELECT outputs.digest FROM job_outputs AS outputs
             JOIN jobs ON jobs.job_id = outputs.job_id
             WHERE jobs.tenant_id = ?1 AND jobs.{side}"
        ),
        tenant,
        workspace,
        &mut found,
    )?;
    let mut uploads = BTreeSet::new();
    collect(
        connection,
        &format!(
            "SELECT object_digest FROM uploads
             WHERE tenant_id = ?1 AND {side} AND object_digest IS NOT NULL"
        ),
        tenant,
        workspace,
        &mut uploads,
    )?;
    match rows {
        Rows::Target => found.extend(uploads),
        Rows::Survivors => found.extend(except_archives(uploads, not_surviving)),
    }
    Ok(found)
}

/// The objects of `held` that a survivor's upload slot or pending put may keep: all but the
/// target's workspace archives (`archives`). A copy of the target's archive that another
/// workspace holds in an upload slot or names in a pending put is no surviving reference
/// (Stage 1b design section 6), so the archive goes with the target. A write of another
/// workspace that is putting the same bytes during the objects step pins them, and keeps them as
/// it keeps any pinned object (`pins`); this helper does not apply to pins.
pub(crate) fn except_archives<'a>(
    held: impl IntoIterator<Item = Digest> + 'a,
    archives: &'a BTreeSet<Digest>,
) -> impl Iterator<Item = Digest> + 'a {
    held.into_iter()
        .filter(move |digest| !archives.contains(digest))
}

fn collect(
    connection: &Connection,
    query: &str,
    tenant: &str,
    workspace: &str,
    found: &mut BTreeSet<Digest>,
) -> Result<(), ApiError> {
    let mut statement = connection.prepare(query).map_err(|error| sql(&error))?;
    let values = statement
        .query_map(params![tenant, workspace], |row| row.get::<_, String>(0))
        .map_err(|error| sql(&error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    for value in values {
        found.insert(from_text!(value.as_str())?);
    }
    Ok(())
}

/// Add every 64-character run of lower-case hexadecimal in `bytes`, bounded by non-hex bytes.
pub(crate) fn tokens(bytes: &[u8], found: &mut BTreeSet<Digest>) {
    let is_hex = |byte: &u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte);
    for run in bytes.split(|byte| !is_hex(byte)) {
        if run.len() == DIGEST_LENGTH
            && let Ok(text) = std::str::from_utf8(run)
            && let Ok(digest) = Digest::try_from(text.to_owned())
        {
            found.insert(digest);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::tokens;

    #[test]
    fn only_whole_digest_tokens_are_references() {
        let mut found = BTreeSet::new();
        let digest = "a".repeat(64);
        let text = format!(
            "{{\"object\":\"{digest}\",\"longer\":\"{}\",\"upper\":\"{}\"}}",
            "b".repeat(65),
            "C".repeat(64)
        );
        tokens(text.as_bytes(), &mut found);
        let found: Vec<String> = found.iter().map(|d| d.as_str().to_owned()).collect();
        assert_eq!(found, vec![digest]);
    }
}
