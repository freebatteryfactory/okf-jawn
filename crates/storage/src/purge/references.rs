//! What a workspace references in the tenant's shared object store.
//!
//! Objects are shared inside a tenant (`BlobStore`), so a purge may delete an object only when
//! no surviving workspace and no tenant record references it; a workspace archive copies exactly
//! the objects its workspace references (`archive::refuse_missing`); and a restore refuses an
//! archive whose history or rows reference an object it does not hold (`restore::refuse_unheld`).
//! Each of them reads references here, in one way.
//!
//! A reference is read structurally, from a typed field that names a stored object, and never
//! as a token of free text: a digest someone types into a file name, a caption or a table cell
//! keeps and copies nothing. A workspace references an object through exactly these fields:
//!
//! - **Its Git history**, on every commit of every reference (proposal candidates included).
//!   The walk reads one kind of file, the source appearance of each card
//!   (`.okf/sources/<item>.json`), which is the contract's `SourceAppearance` (contract
//!   `source.rs:150`). Two of its fields name stored objects: the original, `object`
//!   (`source.rs:152`), and the conversion record, `extraction.digest` (contract
//!   `extraction.rs:235`). No other field does. `names`, `media_type`, `size` and `metadata` are
//!   the supplied file's own text. The converter's model `sha256` (`extraction.rs:156`) and the
//!   supplied text's `content_digest` (`extraction.rs:312`) are content hashes.
//! - **Files the walk does not read.** The item file's header repeats the appearance's two
//!   digests (core `SourceCard::header`, `storage.rs:861`). A correction
//!   (`.okf/corrections/<item>/<digest>.json`) and accepted supplied text
//!   (`.okf/supplied/<item>.json`, its `based_on`) each hold their own Markdown. The digest each
//!   carries is a key: it names a conversion record that a card's `extraction.digest` names on
//!   some commit, never an object only it keeps.
//! - **The conversion record a card names**, read one level, at any size, streamed
//!   (`conversion_objects`). Each structured export (`structured[].digest`, core
//!   `conversion.rs:236`) and each retained asset (`assets[].digest`, `conversion.rs:90`) is
//!   named. Its `markdown` (`conversion.rs:259`) is the hash of the card's text, and every other
//!   field is skipped unread. No other object is ever opened for references: not an upload, an
//!   artifact, a derived dataset or a pending put.
//! - **Its records** (`records`), each by its digest column: derived datasets, retained
//!   artifacts, completed uploads, sandbox capabilities, written histories, written workspace
//!   archives and job outputs. A conversion job's completion lists the record, its structured
//!   exports and its assets as job outputs (`RecordStore::complete_job`); before it, they are
//!   pending puts of the writing workspace, and the card-named record leads to them.
//!
//! Only the application writes a source appearance or a conversion record. So one that does not
//! read as its type is damage: a backup or a purge fails with `Internal` naming it (`damaged`),
//! and a restore refuses the archive as invalid input. Nothing ever falls back to tokens.

use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead;
use std::path::Path;

use git2::{ObjectType, Oid, Repository, Sort};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, TenantId};
use okf_jawn_contract::source::SourceAppearance;
use rusqlite::{Connection, params};

use crate::blobs::LocalBlobs;
use crate::db::sql;
use crate::git::repo::git;

/// Every commit of a repository on the walked references, and the objects its cards name.
#[derive(Debug, Default)]
pub(crate) struct History {
    /// Every commit reachable from a walked reference.
    pub(crate) revisions: Vec<Oid>,
    /// Every object a source appearance on those commits names, its original (`object`) or its
    /// conversion record (`extraction.digest`), with what first names it.
    pub(crate) objects: BTreeMap<Digest, String>,
    /// The objects of `objects` a card names as its conversion record: the only objects ever
    /// opened as records.
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

/// What a file that does not read structurally answers: `damaged` for the store's own files, a
/// refusal for an uploaded archive's.
pub(crate) type Unreadable = fn(&str) -> ApiError;

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
pub(crate) type Visit<'a> = dyn FnMut(Digest, &'static str) -> Result<(), ApiError> + 'a;

/// What reads one member of an object for `Scanner::members`: given the scanner, the member's
/// key (`None` past `KEY_LIMIT` bytes) and the first byte of its value, it reads the value.
type Member<'a, S> = dyn FnMut(&mut S, Option<&[u8]>, u8) -> Result<(), Stop> + 'a;

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

/// A store's own file that does not read as its type. Only the application writes a source
/// appearance or a conversion record, so this is damage, and the backup or purge reading it
/// fails rather than guess what it names.
pub(crate) fn damaged(message: &str) -> ApiError {
    ApiError::new(
        ErrorCode::Internal,
        format!("the store holds damaged data: {message}"),
    )
}

/// Walk every commit on every reference of `repository`, collecting the commits and what the
/// source appearances on them name (`scan`). Each distinct appearance tree and file is read
/// once, so a long history with few changed cards costs little more than the walk.
///
/// # Errors
/// Returns `Internal` (`damaged`) for an appearance file that does not read as one.
pub(crate) fn history(repository: &Repository) -> Result<History, ApiError> {
    let mut tips = Vec::new();
    for reference in repository.references().map_err(|error| git(&error))? {
        let reference = reference.map_err(|error| git(&error))?;
        if let Some(target) = reference.target()
            && repository.find_commit(target).is_ok()
        {
            tips.push(target);
        }
    }
    walk(repository, &tips, damaged)
}

/// The history of an uploaded archive, its pack indexed into `repository`, walked from the
/// commits its manifest names (`tips`), as `history` walks a workspace's.
///
/// # Errors
/// Returns `unreadable` for a tip the pack does not hold as a commit, or for an appearance file
/// that does not read as one.
pub(crate) fn history_from(
    repository: &Repository,
    tips: &[Oid],
    unreadable: Unreadable,
) -> Result<History, ApiError> {
    for tip in tips {
        if repository.find_commit(*tip).is_err() {
            return Err(unreadable(&format!(
                "its manifest names the commit {tip}, which its history does not hold"
            )));
        }
    }
    walk(repository, tips, unreadable)
}

/// The objects the source appearances on the one commit `commit` name: those a record of the
/// workspace names once that commit is stored.
///
/// # Errors
/// Returns `Internal` (`damaged`) for an appearance file that does not read as one.
pub(crate) fn appearances(
    repository: &Repository,
    commit: Oid,
) -> Result<BTreeSet<Digest>, ApiError> {
    let mut found = History::default();
    if let Some(sources) = sources_tree(repository, commit, damaged)? {
        scan(
            repository,
            sources,
            &mut BTreeSet::new(),
            &mut found,
            damaged,
        )?;
    }
    Ok(found.objects.into_keys().collect())
}

/// Give `visit` each object the held conversion record `digest` names, read structurally, with
/// what names it: each structured export (`structured[].digest`) and each retained asset
/// (`assets[].digest`), as it is read. Call it only for an object a source card names as its
/// conversion record. Nothing for an object the store does not hold. Blocking.
///
/// # Errors
/// Returns what `visit` returns, `Unavailable` for a record that cannot be read, and `Internal`
/// (`damaged`) for one that does not read as a conversion record.
pub(crate) fn conversion_objects(
    blobs: &LocalBlobs,
    tenant: &TenantId,
    digest: &Digest,
    visit: &mut Visit<'_>,
) -> Result<(), ApiError> {
    let path = blobs.local_path(tenant, digest)?;
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(crate::data::io_error("read", &path, &error)),
    };
    read_record(
        std::io::BufReader::with_capacity(64 << 10, file),
        (&path, digest),
        visit,
        damaged,
    )
}

/// Read the conversion record `digest` from `reader` (its file `path`, named in a read error):
/// give `visit` each object its two lists name, as it is read (`conversion_objects`). The record
/// is streamed whatever its size (`Scanner`), every other field skipped without being held, so
/// this holds no memory that grows with the record; what the visitor keeps is its own.
///
/// # Errors
/// Returns what `visit` returns, `Unavailable` for bytes that cannot be read, and `unreadable`
/// for a record that does not read to its end as a JSON object (`visit` was given what came
/// before).
pub(crate) fn read_record<R: BufRead>(
    reader: R,
    (path, digest): (&Path, &Digest),
    visit: &mut Visit<'_>,
    unreadable: Unreadable,
) -> Result<(), ApiError> {
    let mut scanner = Scanner { reader, path };
    match scanner.record(visit) {
        Ok(()) => Ok(()),
        Err(Stop::Malformed) => Err(unreadable(&format!(
            "the conversion record {} does not read as one",
            digest.as_str()
        ))),
        Err(Stop::Failed(error)) => Err(error),
    }
}

/// Add to `found` each held object that a conversion record of `records` names
/// (`conversion_objects`); no other object is opened. Only held objects are kept, so memory is
/// bounded by the tenant's objects, not by a record. Blocking.
///
/// # Errors
/// As `conversion_objects`.
pub(crate) fn follow_records(
    blobs: &LocalBlobs,
    tenant: &TenantId,
    records: &BTreeSet<Digest>,
    found: &mut BTreeSet<Digest>,
) -> Result<(), ApiError> {
    for record in records {
        conversion_objects(blobs, tenant, record, &mut |named, _| {
            if blobs.local_path(tenant, &named)?.is_file() {
                found.insert(named);
            }
            Ok(())
        })?;
    }
    Ok(())
}

/// Walk every commit reachable from `tips`, collecting the commits and what the appearances on
/// them name (`scan`).
fn walk(
    repository: &Repository,
    tips: &[Oid],
    unreadable: Unreadable,
) -> Result<History, ApiError> {
    let mut found = History::default();
    if tips.is_empty() {
        return Ok(found);
    }
    let mut walk = repository.revwalk().map_err(|error| git(&error))?;
    walk.set_sorting(Sort::TOPOLOGICAL)
        .map_err(|error| git(&error))?;
    for tip in tips {
        walk.push(*tip).map_err(|error| git(&error))?;
    }
    let (mut trees, mut files) = (BTreeSet::new(), BTreeSet::new());
    for oid in walk {
        let oid = oid.map_err(|error| git(&error))?;
        found.revisions.push(oid);
        if let Some(sources) = sources_tree(repository, oid, unreadable)?
            && trees.insert(sources)
        {
            scan(repository, sources, &mut files, &mut found, unreadable)?;
        }
    }
    Ok(found)
}

/// The tree of source appearances on `commit`, when it has one.
fn sources_tree(
    repository: &Repository,
    commit: Oid,
    unreadable: Unreadable,
) -> Result<Option<Oid>, ApiError> {
    let tree = repository
        .find_commit(commit)
        .and_then(|commit| commit.tree())
        .map_err(|error| git(&error))?;
    match tree.get_path(Path::new(SOURCES_TREE)) {
        Ok(entry) if entry.kind() == Some(ObjectType::Tree) => Ok(Some(entry.id())),
        Ok(_) => Err(unreadable(&format!(
            "{SOURCES_TREE} on the commit {commit} is not a folder"
        ))),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(git(&error)),
    }
}

/// Add to `found` what each file of the appearance tree `sources` not already in `files` names:
/// each is a `SourceAppearance`, read as one, which names its original (`object`) and its
/// conversion record (`extraction.digest`, also added to `found.records`); nothing else of it
/// is read. An entry that is not such a file is `unreadable`, naming its path.
fn scan(
    repository: &Repository,
    sources: Oid,
    files: &mut BTreeSet<Oid>,
    found: &mut History,
    unreadable: Unreadable,
) -> Result<(), ApiError> {
    let sources = repository.find_tree(sources).map_err(|error| git(&error))?;
    for entry in &sources {
        let name = String::from_utf8_lossy(entry.name_bytes()).into_owned();
        if entry.kind() != Some(ObjectType::Blob) {
            return Err(unreadable(&format!(
                "{SOURCES_TREE}/{name} is not a source appearance file"
            )));
        }
        if !files.insert(entry.id()) {
            continue;
        }
        let blob = repository
            .find_blob(entry.id())
            .map_err(|error| git(&error))?;
        let appearance: SourceAppearance =
            serde_json::from_slice(blob.content()).map_err(|error| {
                unreadable(&format!(
                    "the source appearance {SOURCES_TREE}/{name} does not read as one: {error}"
                ))
            })?;
        let item = name.strip_suffix(".json").unwrap_or(&name);
        found
            .objects
            .entry(appearance.object)
            .or_insert_with(|| format!("a source card's original (item {item})"));
        if let Some(record) = appearance.extraction.digest {
            found.records.insert(record.clone());
            found
                .objects
                .entry(record)
                .or_insert_with(|| format!("a source card's conversion record (item {item})"));
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ASSET, STRUCTURED, Scanner, Stop};

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
    /// the depth limit, names what came before and stops, which `read_record` then refuses.
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
}
