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
//!   (`.okf/sources/<item>.json`), which the application writes as the contract's
//!   `SourceAppearance` (contract `source.rs:150`). Two of its fields name stored objects: the
//!   original, `object` (`source.rs:152`), and the conversion record, `extraction.digest`
//!   (contract `extraction.rs:235`). No other field does. `names`, `media_type`, `size` and
//!   `metadata` are the supplied file's own text. The converter's model `sha256`
//!   (`extraction.rs:156`) and the supplied text's `content_digest` (`extraction.rs:312`) are
//!   content hashes. **The two fields are read by path** (`appearance_objects`), never through
//!   the contract type, whose `deny_unknown_fields` would make a card of another contract
//!   version unreadable: a field this build does not know, a renamed variant or a changed type
//!   anywhere else in the file leaves the two references readable. `object` is required; a
//!   missing `extraction` or `extraction.digest` is simply no record.
//! - **Files the walk does not read.** The item file's header repeats the appearance's two
//!   digests (core `SourceCard::header`, `storage.rs:861`). A correction
//!   (`.okf/corrections/<item>/<digest>.json`) and accepted supplied text
//!   (`.okf/supplied/<item>.json`, its `based_on`) each hold their own Markdown. The digest each
//!   carries is a key: it names a conversion record that a card's `extraction.digest` names on
//!   some commit, never an object only it keeps.
//! - **The conversion record a card names**, read one level, at any size, streamed
//!   (`conversion_objects`). Each structured export (`structured[].digest`, core
//!   `conversion.rs:236`) and each retained asset (`assets[].digest`, `conversion.rs:90`) is
//!   named: only the `digest` of each entry is read, and every other key, known to this build or
//!   not, is skipped unread. An entry whose `digest` is not a digest names nothing, since no
//!   stored object has such a key. Its `markdown` (`conversion.rs:259`) is the hash of the
//!   card's text. No other object is ever opened for references: not an upload, an artifact, a
//!   derived dataset or a pending put.
//! - **Its records** (`records`), each by its digest column: derived datasets, retained
//!   artifacts, completed uploads, sandbox capabilities, written histories, written workspace
//!   archives and job outputs. A conversion job's completion lists the record, its structured
//!   exports and its assets as job outputs (`RecordStore::complete_job`); before it, they are
//!   pending puts of the writing workspace, and the card-named record leads to them.
//!
//! **Damage.** Only the application writes a source appearance or a conversion record, so one
//! that does not read is damage, and exactly this is damage: a file that is not JSON (an
//! appearance that is not a JSON object with an `object`; a record that does not read to its
//! end as a JSON object), or an appearance's named field (`object`, `extraction.digest`) that
//! holds something other than a digest. So is a history whose commit needs an object the
//! repository does not hold (a parent, a tree, an appearance blob), or whose `.okf/sources`
//! holds something other than files. Nothing ever falls back to tokens. What a reader does with
//! damage depends on whose it is (`Report`):
//! - a backup of the damaged workspace fails with `Internal` naming the path or the record
//!   digest (`damaged`), and a restore refuses an archive that holds it as invalid input
//!   (`restore::refuse_unheld`), so no import plants damage;
//! - a purge of another workspace stops at its objects step with `Internal` naming the damaged
//!   workspace and the path or record digest (`refusing_in`): guessing could delete what the
//!   file names, and the message says where to look;
//! - a purge of the damaged workspace itself goes on (`history_recording`): each unread file is
//!   recorded in `History::unread` by its path or record digest, what was read stays read, and
//!   the objects only an unread file named may remain stored once the purge completes. Purging
//!   the damaged workspace is the in-product repair for a tenant whose purges its damage stopped.

use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead;
use std::path::Path;

use git2::{ObjectType, Oid, Repository, Sort};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, TenantId};
use rusqlite::{Connection, params};
use serde_json::Value;

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
    /// What a recording walk (`history_recording`) could not read, each by its path or object
    /// and why: the objects only it names are not in `objects`. Empty after a walk that refuses
    /// damage, which stops at the first.
    pub(crate) unread: Vec<String>,
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

/// What a reader does with damage, given its message (the path or object, and why): `refusing`
/// answers an error, which stops the read at the first; a recording reader keeps the message and
/// reads on (`history_recording`, for the workspace a purge removes).
pub(crate) type Report<'a> = dyn FnMut(String) -> Result<(), ApiError> + 'a;

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

/// The tree that holds the source appearance of each card, as a path.
const SOURCES_TREE: &str = ".okf/sources";
/// The application's folder at the root of a tree.
const OKF_TREE: &str = ".okf";
/// The appearance folder inside `OKF_TREE`.
const SOURCES_DIR: &str = "sources";
/// Where an appearance names its original.
const OBJECT_AT: &str = "/object";
/// Where an appearance names its conversion record.
const RECORD_AT: &str = "/extraction/digest";
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

/// The `Report` that refuses damage: it answers `unreadable` of the message, which stops the
/// read at the first damaged file.
pub(crate) fn refusing(unreadable: Unreadable) -> impl FnMut(String) -> Result<(), ApiError> {
    move |message| Err(unreadable(&message))
}

/// The `Report` that refuses damage in the surviving workspace `workspace`, read during another
/// workspace's purge: `damaged`, naming the workspace before the path or record digest, so an
/// operator knows where to look. The remedy is a fix of the damaged file or of this reader, or
/// a purge of the damaged workspace, which its own damage does not stop (`history_recording`).
pub(crate) fn refusing_in(workspace: &str) -> impl FnMut(String) -> Result<(), ApiError> {
    let workspace = workspace.to_owned();
    move |message| Err(damaged(&format!("in the workspace {workspace}, {message}")))
}

/// Walk every commit on every reference of `repository`, collecting the commits and what the
/// source appearances on them name (`scan`). Each distinct appearance tree and file is read
/// once, so a long history with few changed cards costs little more than the walk.
///
/// # Errors
/// Returns `Internal` (`damaged`) for damage: an appearance that does not read as one, an
/// entry under `.okf/sources` that is not a file, or an object a commit needs that the
/// repository does not hold.
pub(crate) fn history(repository: &Repository) -> Result<History, ApiError> {
    walk(repository, &tips(repository)?, &mut refusing(damaged))
}

/// As `history`, for the surviving workspace `workspace` read during another workspace's
/// purge: a failure names the workspace (`refusing_in`).
///
/// # Errors
/// As `history`, naming the workspace.
pub(crate) fn history_naming(
    repository: &Repository,
    workspace: &str,
) -> Result<History, ApiError> {
    walk(repository, &tips(repository)?, &mut refusing_in(workspace))
}

/// As `history`, for the workspace a purge removes: damage does not stop the walk. Each unread
/// file is recorded in `History::unread` by its path or object and why, what was read stays
/// read, and the walk goes on, so a workspace's own damage never keeps it from being purged. The
/// objects only an unread file named are not in `History::objects`, and may remain stored.
///
/// # Errors
/// Returns `Unavailable` for a failure of the version store itself; never for damage.
pub(crate) fn history_recording(repository: &Repository) -> Result<History, ApiError> {
    let mut unread = Vec::new();
    let mut found = walk(repository, &tips(repository)?, &mut |message| {
        unread.push(message);
        Ok(())
    })?;
    found.unread = unread;
    Ok(found)
}

/// The history of an uploaded archive, its pack indexed into `repository`, walked from the
/// commits its manifest names (`tips`), as `history` walks a workspace's.
///
/// # Errors
/// Returns `unreadable` for a tip the pack does not hold as a commit, or for damage of the
/// history (as `history`), which an archive's is: a restore refuses it as invalid input.
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
    walk(repository, tips, &mut refusing(unreadable))
}

/// The objects the source appearances on the one commit `commit` name: those a record of the
/// workspace names once that commit is stored.
///
/// # Errors
/// Returns `Internal` (`damaged`) for damage, as `history`.
pub(crate) fn appearances(
    repository: &Repository,
    commit: Oid,
) -> Result<BTreeSet<Digest>, ApiError> {
    let mut found = History::default();
    let mut report = refusing(damaged);
    if let Some(sources) = sources_tree(repository, commit, &mut report)? {
        scan(
            repository,
            sources,
            &mut BTreeSet::new(),
            &mut found,
            &mut report,
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
    match stream_held(blobs, tenant, digest, visit)? {
        None => Ok(()),
        Some(message) => Err(damaged(&message)),
    }
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
    match stream(reader, (path, digest), visit)? {
        None => Ok(()),
        Some(message) => Err(unreadable(&message)),
    }
}

/// Add to `found` each held object that a conversion record of `records` names
/// (`conversion_objects`); no other object is opened. Only held objects are kept, so memory is
/// bounded by the tenant's objects, not by a record. A record that does not read to its end is
/// told to `report`, with what came before kept. Blocking.
///
/// # Errors
/// What `report` returns for damage; `Unavailable` for a record that cannot be read.
pub(crate) fn follow_records(
    blobs: &LocalBlobs,
    tenant: &TenantId,
    records: &BTreeSet<Digest>,
    found: &mut BTreeSet<Digest>,
    report: &mut Report<'_>,
) -> Result<(), ApiError> {
    for record in records {
        let read = stream_held(blobs, tenant, record, &mut |named, _| {
            if blobs.local_path(tenant, &named)?.is_file() {
                found.insert(named);
            }
            Ok(())
        })?;
        if let Some(message) = read {
            report(message)?;
        }
    }
    Ok(())
}

/// `conversion_objects`, answering the message of a record that does not read to its end
/// instead of failing on it: `None` when it read whole, or the store does not hold it.
fn stream_held(
    blobs: &LocalBlobs,
    tenant: &TenantId,
    digest: &Digest,
    visit: &mut Visit<'_>,
) -> Result<Option<String>, ApiError> {
    let path = blobs.local_path(tenant, digest)?;
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(crate::data::io_error("read", &path, &error)),
    };
    stream(
        std::io::BufReader::with_capacity(64 << 10, file),
        (&path, digest),
        visit,
    )
}

/// `read_record`, answering the message of a record that does not read to its end instead of
/// failing on it: `None` when it read whole.
fn stream<R: BufRead>(
    reader: R,
    (path, digest): (&Path, &Digest),
    visit: &mut Visit<'_>,
) -> Result<Option<String>, ApiError> {
    let mut scanner = Scanner { reader, path };
    match scanner.record(visit) {
        Ok(()) => Ok(None),
        Err(Stop::Malformed) => Ok(Some(format!(
            "the conversion record {} does not read as one",
            digest.as_str()
        ))),
        Err(Stop::Failed(error)) => Err(error),
    }
}

/// The commit each reference of `repository` names, for those that name one.
fn tips(repository: &Repository) -> Result<Vec<Oid>, ApiError> {
    let mut tips = Vec::new();
    for reference in repository.references().map_err(|error| git(&error))? {
        let reference = reference.map_err(|error| git(&error))?;
        if let Some(target) = reference.target()
            && repository.find_commit(target).is_ok()
        {
            tips.push(target);
        }
    }
    Ok(tips)
}

/// Walk every commit reachable from `tips`, collecting the commits and what the appearances on
/// them name (`scan`); damage goes to `report`.
fn walk(
    repository: &Repository,
    tips: &[Oid],
    report: &mut Report<'_>,
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
        let oid = match oid {
            Ok(oid) => oid,
            Err(error) => {
                // A commit the walk reached names a parent the repository does not hold: the
                // commits read so far stay read, and the walk cannot go on past it.
                report(missing(&error, "a parent a commit names")?)?;
                break;
            }
        };
        found.revisions.push(oid);
        if let Some(sources) = sources_tree(repository, oid, report)?
            && trees.insert(sources)
        {
            scan(repository, sources, &mut files, &mut found, report)?;
        }
    }
    Ok(found)
}

/// A Git failure while reading a history. An object the history needs but the repository does
/// not hold (`NotFound`) is damage of the history, told as `what` it is, for `Report`: a restore
/// refuses the archive, a purge of the workspace records it. Any other failure is the version
/// store's own (`Unavailable`).
fn missing(error: &git2::Error, what: &str) -> Result<String, ApiError> {
    if error.code() == git2::ErrorCode::NotFound {
        Ok(format!(
            "the history does not hold {what}: {}",
            error.message()
        ))
    } else {
        Err(git(error))
    }
}

/// The tree of source appearances on `commit`, when it has one: the `sources` folder of the
/// `.okf` folder at its root. A missing tree object, or a `.okf/sources` that is not a folder,
/// is damage (`report`), and then there is nothing to read.
fn sources_tree(
    repository: &Repository,
    commit: Oid,
    report: &mut Report<'_>,
) -> Result<Option<Oid>, ApiError> {
    let root = match repository
        .find_commit(commit)
        .and_then(|commit| commit.tree())
    {
        Ok(tree) => tree,
        Err(error) => {
            report(missing(
                &error,
                &format!("the tree of the commit {commit}"),
            )?)?;
            return Ok(None);
        }
    };
    let Some(okf) = root
        .get_name(OKF_TREE)
        .filter(|entry| entry.kind() == Some(ObjectType::Tree))
    else {
        return Ok(None);
    };
    let okf_id = okf.id();
    let okf = match repository.find_tree(okf_id) {
        Ok(tree) => tree,
        Err(error) => {
            report(missing(
                &error,
                &format!("the folder {OKF_TREE} ({okf_id}) of the commit {commit}"),
            )?)?;
            return Ok(None);
        }
    };
    match okf.get_name(SOURCES_DIR) {
        None => Ok(None),
        Some(entry) if entry.kind() == Some(ObjectType::Tree) => Ok(Some(entry.id())),
        Some(_) => {
            report(format!(
                "{SOURCES_TREE} on the commit {commit} is not a folder"
            ))?;
            Ok(None)
        }
    }
}

/// Add to `found` what each file of the appearance tree `sources` not already in `files` names:
/// its original (`object`) and its conversion record (`extraction.digest`, also added to
/// `found.records`), read by path (`appearance_objects`); nothing else of it is read. An entry
/// that is not a file, a file the repository does not hold, or one that does not read is damage
/// (`report`), naming its path; the others are still read.
fn scan(
    repository: &Repository,
    sources: Oid,
    files: &mut BTreeSet<Oid>,
    found: &mut History,
    report: &mut Report<'_>,
) -> Result<(), ApiError> {
    let sources = match repository.find_tree(sources) {
        Ok(tree) => tree,
        Err(error) => {
            report(missing(
                &error,
                &format!("the folder {SOURCES_TREE} ({sources})"),
            )?)?;
            return Ok(());
        }
    };
    for entry in &sources {
        let name = String::from_utf8_lossy(entry.name_bytes()).into_owned();
        if entry.kind() != Some(ObjectType::Blob) {
            report(format!(
                "{SOURCES_TREE}/{name} is not a source appearance file"
            ))?;
            continue;
        }
        if !files.insert(entry.id()) {
            continue;
        }
        let blob = match repository.find_blob(entry.id()) {
            Ok(blob) => blob,
            Err(error) => {
                report(missing(
                    &error,
                    &format!(
                        "the source appearance {SOURCES_TREE}/{name} ({})",
                        entry.id()
                    ),
                )?)?;
                continue;
            }
        };
        let item = name.strip_suffix(".json").unwrap_or(&name);
        match appearance_objects(blob.content()) {
            Ok((object, record)) => {
                found
                    .objects
                    .entry(object)
                    .or_insert_with(|| format!("a source card's original (item {item})"));
                if let Some(record) = record {
                    found.records.insert(record.clone());
                    found.objects.entry(record).or_insert_with(|| {
                        format!("a source card's conversion record (item {item})")
                    });
                }
            }
            Err(why) => report(format!("the source appearance {SOURCES_TREE}/{name} {why}"))?,
        }
    }
    Ok(())
}

/// The two objects a source appearance's bytes name, read by path and nothing else of the file:
/// its original (`object`, required) and its conversion record (`extraction.digest`, none when
/// absent or null). Every other field, known to this build or not, is left unread, so a card
/// another contract version wrote still names its objects.
///
/// # Errors
/// Why the bytes do not read as an appearance: they are not JSON, there is no `object`, or a
/// named field holds something other than a digest.
fn appearance_objects(bytes: &[u8]) -> Result<(Digest, Option<Digest>), String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| format!("is not JSON: {error}"))?;
    let object = digest_at(&value, OBJECT_AT)?.ok_or_else(|| "has no object".to_owned())?;
    let record = digest_at(&value, RECORD_AT)?;
    Ok((object, record))
}

/// The digest `value` holds at `pointer` (a JSON pointer): `None` when nothing, or null, is
/// there.
///
/// # Errors
/// When what is there is not a digest.
fn digest_at(value: &Value, pointer: &str) -> Result<Option<Digest>, String> {
    match value.pointer(pointer) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Digest::try_from(text.clone())
            .map(Some)
            .map_err(|_| format!("holds at {pointer} a string that is not a digest")),
        Some(_) => Err(format!("holds at {pointer} a value that is not a digest")),
    }
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

    use super::{ASSET, STRUCTURED, Scanner, Stop, appearance_objects};

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

    /// B1-R round 4 (R3-1): an appearance is read by its two paths and nothing else. A field
    /// this build does not know, at the top or under `extraction`, an unknown variant in an
    /// unrelated field, a changed type elsewhere, a missing `extraction` or a null record all
    /// read; not JSON, no `object`, or a named field that is not a digest are damage, named by
    /// path.
    #[test]
    fn an_appearance_is_read_by_its_two_paths_whatever_else_it_holds() {
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        let later = format!(
            r#"{{"object": "{a}", "size": 10, "later_field": {{"x": [1, 2]}},
                "extraction": {{"digest": "{b}", "outcome": {{"status": "later_status"}},
                                "later_extraction_field": true}}}}"#
        );
        let read = |bytes: &str| {
            appearance_objects(bytes.as_bytes()).map(|(object, record)| {
                (
                    object.as_str().to_owned(),
                    record.map(|record| record.as_str().to_owned()),
                )
            })
        };
        assert_eq!(read(&later), Ok((a.clone(), Some(b.clone()))));
        assert_eq!(
            read(&format!(r#"{{"object": "{a}"}}"#)),
            Ok((a.clone(), None)),
            "no extraction is no record"
        );
        assert_eq!(
            read(&format!(
                r#"{{"object": "{a}", "extraction": {{"digest": null}}}}"#
            )),
            Ok((a.clone(), None)),
            "a null record is no record"
        );
        assert_eq!(
            read(&format!(r#"{{"object": "{a}", "extraction": "later"}}"#)),
            Ok((a.clone(), None)),
            "an extraction of another shape holds no record at its path"
        );
        for (bytes, why) in [
            ("not json", "is not JSON"),
            (r#"["a list"]"#, "has no object"),
            (r#"{"names": []}"#, "has no object"),
            (
                r#"{"object": "not a digest"}"#,
                "holds at /object a string that is not a digest",
            ),
            (
                r#"{"object": 5}"#,
                "holds at /object a value that is not a digest",
            ),
            (
                &format!(r#"{{"object": "{a}", "extraction": {{"digest": "cut"}}}}"#),
                "holds at /extraction/digest a string that is not a digest",
            ),
            (
                &format!(r#"{{"object": "{a}", "extraction": {{"digest": ["{b}"]}}}}"#),
                "holds at /extraction/digest a value that is not a digest",
            ),
        ] {
            let failed = read(bytes).err().unwrap_or_default();
            assert!(failed.starts_with(why), "{bytes}: {failed}");
        }
    }
}
