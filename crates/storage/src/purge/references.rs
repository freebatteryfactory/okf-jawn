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
//! conversion record is followed one level (`record_tokens`): an object of at most
//! `RECORD_LIMIT` bytes whose bytes start with `{` is scanned in memory for tokens; a larger one
//! only when a source card names it as its conversion record (`extraction.digest`), and then
//! its two lists are read structurally, streamed (`conversion_objects`). A large upload or any
//! other object is never read as a record, and no reading holds memory that grows with an
//! object's size.

use std::collections::BTreeSet;
use std::io::{BufRead, Read as _};
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
    /// The conversion records those appearances name (`extraction.digest`), read structurally:
    /// the only objects over `RECORD_LIMIT` that are ever read as records.
    pub(crate) records: BTreeSet<Digest>,
}

/// Which rows of the tenant a record query reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rows {
    /// The target workspace's rows.
    Target,
    /// Every other row of the tenant: other workspaces and tenant records.
    Survivors,
}

/// What a source appearance names as its conversion record; every other field is skipped.
#[derive(serde::Deserialize)]
struct Appearance {
    #[serde(default)]
    extraction: Option<Extraction>,
}

/// The extraction of an `Appearance`: only its conversion record's digest is read.
#[derive(serde::Deserialize)]
struct Extraction {
    #[serde(default)]
    digest: Option<String>,
}

/// A JSON reader over a conversion record of any size (`Scanner::record`). It holds at most
/// `KEY_LIMIT` bytes of a key, `DIGEST_LENGTH` bytes of a string value, and one bit per level
/// of nesting in a `u128` (at most `DEPTH_LIMIT` levels): its memory does not grow with the
/// record, whatever its keys, values or depth.
struct Scanner<'a, R> {
    reader: R,
    /// The file read, for an error.
    path: &'a Path,
}

/// Why `Scanner` stopped before the end of a record.
enum Stop {
    /// The bytes are not a JSON object this reader reads, or nest deeper than `DEPTH_LIMIT`:
    /// what was read before stays read.
    Malformed,
    /// The file could not be read, or the visitor refused.
    Failed(ApiError),
}

/// What a visitor of a conversion record's named objects is given each object with
/// (`conversion_objects`).
type Visit<'a> = dyn FnMut(Digest, &'static str) -> Result<(), ApiError> + 'a;

/// What reads one member of an object for `Scanner::members`: given the scanner, the member's
/// key (`None` past `KEY_LIMIT` bytes) and the first byte of its value, it reads the value.
type Member<'a, S> = dyn FnMut(&mut S, Option<&[u8]>, u8) -> Result<(), Stop> + 'a;

/// The largest object scanned in memory for digest tokens as a possible conversion record.
const RECORD_LIMIT: u64 = 64 << 20;
/// The tree that holds the source appearance of each card.
const SOURCES_TREE: &str = ".okf/sources";
/// The length of a SHA-256 digest in hexadecimal.
const DIGEST_LENGTH: usize = 64;
/// The most bytes of a key `Scanner` holds: more than any key it looks for.
const KEY_LIMIT: usize = 16;
/// The deepest nesting `Scanner` reads (`serde_json`'s own default limit).
const DEPTH_LIMIT: u32 = 128;
/// What a conversion record's `structured` list names.
const STRUCTURED: &str = "a conversion record's structured export";
/// What a conversion record's `assets` list names.
const ASSET: &str = "a conversion record's retained asset";

impl<R: BufRead> Scanner<'_, R> {
    /// Read a conversion record: a JSON object whose `structured` and `assets` lists hold
    /// objects with a `digest`. Each valid digest is given to `visit` as it is read, with what
    /// names it; every other key and value is skipped without being held.
    fn record(&mut self, visit: &mut Visit<'_>) -> Result<(), Stop> {
        if self.token()? != b'{' {
            return Err(Stop::Malformed);
        }
        self.members(&mut |scanner, key, value| {
            let what = match key {
                Some(b"structured") => STRUCTURED,
                Some(b"assets") => ASSET,
                _ => return scanner.skip(value),
            };
            if value != b'[' {
                return scanner.skip(value);
            }
            scanner.list(what, &mut *visit)
        })
    }

    /// Read the members of an object whose `{` was read, giving `member` each key (`None` past
    /// `KEY_LIMIT` bytes) and the first byte of its value, which `member` reads to its end.
    fn members(&mut self, member: &mut Member<'_, Self>) -> Result<(), Stop> {
        let mut first = true;
        loop {
            match self.token()? {
                b'}' if first => return Ok(()),
                b'"' => {}
                _ => return Err(Stop::Malformed),
            }
            first = false;
            let key = self.string(KEY_LIMIT)?;
            if self.token()? != b':' {
                return Err(Stop::Malformed);
            }
            let value = self.token()?;
            member(self, key.as_deref(), value)?;
            match self.token()? {
                b',' => {}
                b'}' => return Ok(()),
                _ => return Err(Stop::Malformed),
            }
        }
    }

    /// Read a list whose `[` was read: each object in it names the object of its `digest`.
    fn list(&mut self, what: &'static str, visit: &mut Visit<'_>) -> Result<(), Stop> {
        let mut first = true;
        loop {
            let value = self.token()?;
            if value == b']' && first {
                return Ok(());
            }
            first = false;
            if value == b'{' {
                self.members(&mut |scanner, key, value| {
                    if key != Some(b"digest".as_slice()) || value != b'"' {
                        return scanner.skip(value);
                    }
                    if let Some(text) = scanner.string(DIGEST_LENGTH)?
                        && let Ok(text) = String::from_utf8(text)
                        && let Ok(digest) = Digest::try_from(text)
                    {
                        visit(digest, what).map_err(Stop::Failed)?;
                    }
                    Ok(())
                })?;
            } else {
                self.skip(value)?;
            }
            match self.token()? {
                b',' => {}
                b']' => return Ok(()),
                _ => return Err(Stop::Malformed),
            }
        }
    }

    /// Skip the rest of one value whose first byte `first` was read, however long or deep.
    fn skip(&mut self, first: u8) -> Result<(), Stop> {
        match first {
            b'"' => self.string(0).map(drop),
            b'{' | b'[' => self.skip_nested(first),
            b'-' | b'0'..=b'9' | b't' | b'f' | b'n' => {
                // A number or a literal: up to the next delimiter, which is left unread.
                while let Some(byte) = self.peek()? {
                    if matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r') {
                        break;
                    }
                    self.reader.consume(1);
                }
                Ok(())
            }
            _ => Err(Stop::Malformed),
        }
    }

    /// Skip the rest of an object or list whose opening byte `first` was read: one bit per open
    /// level says whether it is an object, so each closing byte must match its level.
    fn skip_nested(&mut self, first: u8) -> Result<(), Stop> {
        let mut objects = u128::from(first == b'{');
        let mut depth: u32 = 1;
        while let Some(level) = depth.checked_sub(1) {
            match self.next()?.ok_or(Stop::Malformed)? {
                b'"' => {
                    self.string(0)?;
                }
                byte @ (b'{' | b'[') => {
                    if depth >= DEPTH_LIMIT {
                        return Err(Stop::Malformed);
                    }
                    let bit = 1_u128.checked_shl(depth).ok_or(Stop::Malformed)?;
                    objects = if byte == b'{' {
                        objects | bit
                    } else {
                        objects & !bit
                    };
                    depth = depth.saturating_add(1);
                }
                byte @ (b'}' | b']') => {
                    let bit = 1_u128.checked_shl(level).ok_or(Stop::Malformed)?;
                    if (objects & bit != 0) != (byte == b'}') {
                        return Err(Stop::Malformed);
                    }
                    depth = level;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The rest of a string whose opening quote was read, unescaped: its bytes when it holds at
    /// most `keep` of them, `None` otherwise; no more than `keep` bytes are ever held. An escaped
    /// character past ASCII is held as a byte no key or digest holds.
    fn string(&mut self, keep: usize) -> Result<Option<Vec<u8>>, Stop> {
        let mut kept = Vec::with_capacity(keep);
        let mut whole = true;
        loop {
            let byte = match self.next()?.ok_or(Stop::Malformed)? {
                b'"' => return Ok(whole.then_some(kept)),
                b'\\' => self.escape()?,
                byte => byte,
            };
            if kept.len() < keep {
                kept.push(byte);
            } else {
                whole = false;
            }
        }
    }

    /// The character an escape stands for, its backslash read, as one byte (`0xFF` for one
    /// past ASCII).
    fn escape(&mut self) -> Result<u8, Stop> {
        Ok(match self.next()?.ok_or(Stop::Malformed)? {
            byte @ (b'"' | b'\\' | b'/') => byte,
            b'b' => 0x08,
            b'f' => 0x0c,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'u' => {
                let mut unit: u32 = 0;
                for _ in 0..4 {
                    let digit = char::from(self.next()?.ok_or(Stop::Malformed)?)
                        .to_digit(16)
                        .ok_or(Stop::Malformed)?;
                    unit = unit
                        .checked_mul(16)
                        .and_then(|shifted| shifted.checked_add(digit))
                        .ok_or(Stop::Malformed)?;
                }
                u8::try_from(unit).ok().filter(u8::is_ascii).unwrap_or(0xFF)
            }
            _ => return Err(Stop::Malformed),
        })
    }

    /// The next byte that is not whitespace, read.
    fn token(&mut self) -> Result<u8, Stop> {
        loop {
            match self.next()?.ok_or(Stop::Malformed)? {
                b' ' | b'\t' | b'\n' | b'\r' => {}
                byte => return Ok(byte),
            }
        }
    }

    /// The next byte, read.
    fn next(&mut self) -> Result<Option<u8>, Stop> {
        let byte = self.peek()?;
        if byte.is_some() {
            self.reader.consume(1);
        }
        Ok(byte)
    }

    /// The next byte, left unread.
    fn peek(&mut self) -> Result<Option<u8>, Stop> {
        let path = self.path;
        Ok(self
            .reader
            .fill_buf()
            .map_err(|error| Stop::Failed(crate::data::io_error("read", path, &error)))?
            .first()
            .copied())
    }
}

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
            scan(
                repository,
                sources,
                &mut files,
                (&mut found.objects, &mut found.records),
            )?;
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

/// Give `visit` each object the held conversion record `digest` names, read structurally, with
/// what names it: each structured export (`structured[].digest`) and each retained asset
/// (`assets[].digest`), as it is read. Call it only for an object a source card names as its
/// conversion record. The record is streamed whatever its size (`Scanner`), every other field
/// skipped without being held, so this holds no memory that grows with the record; what the
/// visitor keeps is its own. Nothing for an object that is not held or not a JSON object; a
/// record malformed part way names what came before. Blocking.
///
/// # Errors
/// Returns what `visit` returns, or `Unavailable` for a record that cannot be read.
pub(crate) fn conversion_objects(
    blobs: &LocalBlobs,
    tenant: &TenantId,
    digest: &Digest,
    visit: &mut Visit<'_>,
) -> Result<(), ApiError> {
    let path = blobs.local_path(tenant, digest)?;
    let Ok(file) = std::fs::File::open(&path) else {
        return Ok(());
    };
    let mut scanner = Scanner {
        reader: std::io::BufReader::with_capacity(64 << 10, file),
        path: &path,
    };
    match scanner.record(visit) {
        Ok(()) | Err(Stop::Malformed) => Ok(()),
        Err(Stop::Failed(error)) => Err(error),
    }
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
        scan(
            repository,
            sources,
            &mut BTreeSet::new(),
            (&mut found, &mut BTreeSet::new()),
        )?;
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
/// already in `files`, and to `records` the conversion record each names (`extraction.digest`).
fn scan(
    repository: &Repository,
    sources: Oid,
    files: &mut BTreeSet<Oid>,
    (objects, records): (&mut BTreeSet<Digest>, &mut BTreeSet<Digest>),
) -> Result<(), ApiError> {
    let sources = repository.find_tree(sources).map_err(|error| git(&error))?;
    for entry in &sources {
        if entry.kind() == Some(ObjectType::Blob) && files.insert(entry.id()) {
            let blob = repository
                .find_blob(entry.id())
                .map_err(|error| git(&error))?;
            tokens(blob.content(), objects);
            if let Ok(Appearance {
                extraction:
                    Some(Extraction {
                        digest: Some(record),
                    }),
            }) = serde_json::from_slice(blob.content())
                && let Ok(record) = Digest::try_from(record)
            {
                records.insert(record);
            }
        }
    }
    Ok(())
}

/// Add to `objects` what each held object of `objects` names when it reads as a conversion
/// record (`record_tokens`); `records` are those a source card names as its conversion record.
/// Blocking.
pub(crate) fn follow_records(
    blobs: &LocalBlobs,
    tenant: &TenantId,
    objects: &mut BTreeSet<Digest>,
    records: &BTreeSet<Digest>,
) -> Result<(), ApiError> {
    let mut found = BTreeSet::new();
    for digest in objects.iter() {
        found.extend(record_tokens(
            blobs,
            tenant,
            digest,
            records.contains(digest),
        )?);
    }
    objects.extend(found);
    Ok(())
}

/// What the held object `digest` names when it reads as a conversion record; none otherwise.
/// Blocking.
/// - At most `RECORD_LIMIT` bytes, starting with `{`: its digest tokens, scanned in memory.
/// - Larger: only when `named_as_record` (a source card names it as its conversion record), the
///   objects its two lists name (`conversion_objects`), each kept only when the store holds it,
///   so memory is bounded by the tenant's objects, not by the record. A large upload or any
///   other object is never read as a record.
pub(crate) fn record_tokens(
    blobs: &LocalBlobs,
    tenant: &TenantId,
    digest: &Digest,
    named_as_record: bool,
) -> Result<BTreeSet<Digest>, ApiError> {
    let mut found = BTreeSet::new();
    let path = blobs.local_path(tenant, digest)?;
    let Ok(metadata) = std::fs::metadata(&path) else {
        return Ok(found);
    };
    if metadata.len() > RECORD_LIMIT {
        if named_as_record {
            conversion_objects(blobs, tenant, digest, &mut |named, _| {
                if blobs.local_path(tenant, &named)?.is_file() {
                    found.insert(named);
                }
                Ok(())
            })?;
        }
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
    use std::path::Path;

    use super::{ASSET, STRUCTURED, Scanner, Stop, tokens};

    /// What `Scanner::record` names in `bytes`, in order, and whether it read them to the end.
    fn named(bytes: &[u8]) -> (Vec<(String, &'static str)>, bool) {
        let mut found = Vec::new();
        let mut scanner = Scanner {
            reader: bytes,
            path: Path::new("record"),
        };
        let whole = match scanner.record(&mut |digest, what| {
            found.push((digest.as_str().to_owned(), what));
            Ok(())
        }) {
            Ok(()) => true,
            Err(Stop::Malformed | Stop::Failed(_)) => false,
        };
        (found, whole)
    }

    /// Pass-4b re-review B3 memory: the streamed reader names exactly the two top-level lists'
    /// digests, whatever surrounds them: a nested `assets` key, a key longer than it holds, an
    /// escaped key, strings full of brackets and escapes, numbers, literals, deep nesting, a
    /// digest too long or not lower-case hexadecimal.
    #[test]
    fn a_streamed_record_names_its_two_lists_and_nothing_else() {
        let (a, b, c) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
        let text = format!(
            r#" {{"converter": {{"assets": [{{"digest": "{c}"}}], "n": -1.5e3, "ok": true}},
                "structured_and_a_much_longer_key": [{{"digest": "{c}"}}],
                "markdown": "]}}\"\\ [{{ é",
                "deep": [[[[{{"x": [null, false, {{}}]}}]]]],
                "structured": [{{"media_type": "x", "digest": "{a}"}}, 7, "s", [{{"digest": "{c}"}}]],
                "assets": [{{"digest": "{b}0"}}, {{"digest": "{}"}}, {{"location": {{"digest": "{c}"}}, "digest": "{b}"}}],
                "warnings": []}} "#,
            "B".repeat(64)
        );
        let (found, whole) = named(text.as_bytes());
        assert!(whole, "read to the end");
        assert_eq!(found, vec![(a, STRUCTURED), (b, ASSET)]);
    }

    /// A record that is not a JSON object names nothing; one malformed part way, or nested past
    /// the depth limit, names what came before and stops.
    #[test]
    fn a_malformed_record_names_what_came_before() {
        let a = "a".repeat(64);
        assert_eq!(named(br#"[{"structured": []}]"#), (Vec::new(), false));
        let (found, whole) = named(
            format!(r#"{{"structured": [{{"digest": "{a}"}}], "x": [}}, "assets": []}}"#)
                .as_bytes(),
        );
        assert_eq!((found, whole), (vec![(a.clone(), STRUCTURED)], false));
        let deep = format!(
            r#"{{"structured": [{{"digest": "{a}"}}], "x": {}{}}}"#,
            "[".repeat(200),
            "]".repeat(200)
        );
        assert_eq!(named(deep.as_bytes()), (vec![(a, STRUCTURED)], false));
    }

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
