//! One bare Git repository per workspace, and the tree and commit plumbing over it.
//!
//! A repository is `<data>/repositories/<tenant>/<workspace>.git`; the accepted head is
//! `refs/heads/main` and a proposal candidate is `refs/okf-jawn/proposals/<proposal id>`. A
//! commit message ends with the `Okf-Jawn-Mutation:` trailer naming its write identity. Every
//! write to one workspace holds that workspace's lock for its whole stage-check-commit path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use git2::{Commit, ObjectType, Oid, Repository, Signature, Sort, Tree};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{MutationId, Revision};
use okf_jawn_core::storage::{Provenance, StorageScope};

use super::item::Holder;
use crate::data::{DataDir, io_error};

/// Repositories of every workspace, and their write locks.
#[derive(Debug, Clone)]
pub(crate) struct Repositories {
    root: PathBuf,
    staging: PathBuf,
    locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    /// The data directory's single-writer lock, held while any copy of this value lives.
    _lock: Arc<DataDir>,
}

/// A staging directory that is removed when dropped, success or failure.
#[derive(Debug)]
pub(crate) struct Staging {
    pub(crate) path: PathBuf,
}

/// The accepted head of a workspace.
pub(crate) const HEAD_REF: &str = "refs/heads/main";
/// The trailer every commit carries.
pub(crate) const TRAILER: &str = "Okf-Jawn-Mutation:";
/// The repository setting that makes libgit2 sync object and reference writes.
pub(crate) const FSYNC_SETTING: &str = "core.fsyncObjectFiles";
/// File mode of a regular blob.
const BLOB_MODE: i32 = 0o100_644;
/// File mode of a tree.
const TREE_MODE: i32 = 0o040_000;
/// The length of a commit id these repositories write: SHA-1, in hex.
const SHA1_HEX_LEN: usize = 40;
/// The folders of the data directory whose contents libgit2 reads and writes: the
/// repositories, and the staging directories of candidates, archives and restores (a restore's
/// scratch repository among them). Every path a Git failure here names lies under one of them.
const GIT_FOLDERS: [&str; 2] = ["repositories", "staging"];
/// How a Git failure's message names an absolute path under neither of `GIT_FOLDERS`.
const OUTSIDE: &str = "a path outside the data directory";

impl Repositories {
    /// The repositories and staging directory of the locked `data` directory.
    pub(crate) fn new(data: Arc<DataDir>) -> Self {
        Self {
            root: data.repositories_path(),
            staging: data.staging_path(),
            locks: Arc::new(Mutex::new(HashMap::new())),
            _lock: data,
        }
    }

    /// The repository directory of a workspace.
    pub(crate) fn path(&self, scope: &StorageScope) -> PathBuf {
        self.root
            .join(scope.tenant_id.as_str())
            .join(format!("{}.git", scope.workspace_id.0))
    }

    /// Open an existing workspace repository to read it; `NotFound` when the workspace has
    /// none. A read changes nothing, not even the repository's config.
    pub(crate) fn open(&self, scope: &StorageScope) -> Result<Repository, ApiError> {
        let path = self.existing(scope)?;
        Repository::open_bare(&path).map_err(|error| git(&error))
    }

    /// Open an existing workspace repository to write it, with durable writes (`durable`);
    /// `NotFound` when the workspace has none.
    pub(crate) fn open_durable(&self, scope: &StorageScope) -> Result<Repository, ApiError> {
        durable(&self.existing(scope)?)
    }

    /// The repository directory of a workspace that has one.
    fn existing(&self, scope: &StorageScope) -> Result<PathBuf, ApiError> {
        let path = self.path(scope);
        if path.exists() {
            Ok(path)
        } else {
            Err(ApiError::new(
                ErrorCode::NotFound,
                "No workspace with that identity here",
            ))
        }
    }

    /// Create a new bare repository with durable writes, replacing a partial one a crash left.
    pub(crate) fn create(&self, scope: &StorageScope) -> Result<Repository, ApiError> {
        let path = self.path(scope);
        if path.exists() {
            std::fs::remove_dir_all(&path).map_err(|error| io_error("clear", &path, &error))?;
        }
        drop(Repository::init_bare(&path).map_err(|error| git(&error))?);
        durable(&path)
    }

    /// Delete a workspace's repository and its staging directories under its write lock, so
    /// no write is half done in it; `true` when a repository was there.
    pub(crate) fn remove(&self, scope: &StorageScope) -> Result<bool, ApiError> {
        self.locked(scope, || {
            let staging = self
                .staging
                .join(scope.tenant_id.as_str())
                .join(scope.workspace_id.0.to_string());
            remove_tree(&staging)?;
            remove_tree(&self.path(scope))
        })
    }

    /// Hold the workspace's write lock while `work` runs.
    pub(crate) fn locked<T>(
        &self,
        scope: &StorageScope,
        work: impl FnOnce() -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let key = format!("{}/{}", scope.tenant_id.as_str(), scope.workspace_id.0);
        let lock = {
            let mut locks = self
                .locks
                .lock()
                .map_err(|_| internal("the workspace lock table was poisoned"))?;
            Arc::clone(locks.entry(key).or_default())
        };
        let _guard = lock
            .lock()
            .map_err(|_| internal("a workspace lock was poisoned"))?;
        work()
    }

    /// How many hold the workspace's write lock or wait for it (or are about to): the callers
    /// of `locked` that took the lock's handle and have not ended. A test that holds the lock
    /// itself sees a second caller as 2, once that caller passed every check it makes before
    /// `locked`.
    #[cfg(test)]
    pub(crate) fn lock_users(&self, scope: &StorageScope) -> Result<usize, ApiError> {
        let key = format!("{}/{}", scope.tenant_id.as_str(), scope.workspace_id.0);
        let locks = self
            .locks
            .lock()
            .map_err(|_| internal("the workspace lock table was poisoned"))?;
        // The table holds one handle of its own.
        Ok(locks
            .get(&key)
            .map_or(0, |lock| Arc::strong_count(lock).saturating_sub(1)))
    }

    /// A fresh staging directory for one mutation, removed when the guard drops.
    pub(crate) fn stage(
        &self,
        scope: &StorageScope,
        mutation: MutationId,
    ) -> Result<Staging, ApiError> {
        let path = self
            .staging
            .join(scope.tenant_id.as_str())
            .join(scope.workspace_id.0.to_string())
            .join(mutation.0.to_string());
        if path.exists() {
            std::fs::remove_dir_all(&path).map_err(|error| io_error("clear", &path, &error))?;
        }
        std::fs::create_dir_all(&path).map_err(|error| io_error("create", &path, &error))?;
        Ok(Staging { path })
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        // A directory that cannot be removed now is removed at the next startup.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Open the bare repository at `path` with `core.fsyncObjectFiles` set in its own config.
///
/// git2 0.21 exposes no safe global fsync option (`GIT_OPT_ENABLE_FSYNC_GITDIR` is reachable
/// only through `raw`), but libgit2 1.9 reads this repository setting when it loads the object
/// database (`git_odb__set_caps`, so every loose object is written through a synced file and
/// its directory is synced) and the reference database (`refdb_fs`, so every reference write
/// is synced with its directory). A commit's objects and its reference move are therefore on
/// disk when `VersionStore::commit` returns, before core records the result in SQLite. A
/// repository that lacks the setting gets it here and is reopened, so the setting is read.
pub(crate) fn durable(path: &Path) -> Result<Repository, ApiError> {
    let repository = Repository::open_bare(path).map_err(|error| git(&error))?;
    let config = repository.config().map_err(|error| git(&error))?;
    if config.get_bool(FSYNC_SETTING).unwrap_or(false) {
        return Ok(repository);
    }
    config
        .open_level(git2::ConfigLevel::Local)
        .and_then(|mut local| local.set_bool(FSYNC_SETTING, true))
        .map_err(|error| git(&error))?;
    drop(config);
    drop(repository);
    Repository::open_bare(path).map_err(|error| git(&error))
}

/// Sync directories of the repository (relative to its Git directory) whose entries a write
/// just added: libgit2 syncs a new object or reference file and its parent, but not a new
/// fan-out directory (`objects/xx/`) or a new `refs/okf-jawn/proposals/` in its own parent.
pub(crate) fn sync_directories(repository: &Repository, relative: &[&str]) -> Result<(), ApiError> {
    // Unix only. Not needed on Windows: NTFS journals directory metadata, so a new directory
    // entry is as durable as the file written into it (libgit2's own directory sync is a no-op
    // there, and std cannot open a directory as a file there).
    if cfg!(unix) {
        for name in relative {
            let path = repository.path().join(name);
            std::fs::File::open(&path)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| io_error("sync", &path, &error))?;
        }
    }
    Ok(())
}

/// The accepted head of a repository.
pub(crate) fn head(repository: &Repository) -> Result<Oid, ApiError> {
    reference_target(repository, HEAD_REF)?
        .ok_or_else(|| internal("a workspace repository has no head"))
}

/// The commit a reference points at, if the reference exists.
pub(crate) fn reference_target(
    repository: &Repository,
    name: &str,
) -> Result<Option<Oid>, ApiError> {
    match repository.find_reference(name) {
        Ok(reference) => Ok(reference.target()),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(git(&error)),
    }
}

/// The commit a `Revision` names; `NotFound` when the repository does not hold it, or when the
/// revision cannot name a commit here at all (`oid_of`).
pub(crate) fn commit_of<'r>(
    repository: &'r Repository,
    revision: &Revision,
) -> Result<Commit<'r>, ApiError> {
    let oid = oid_of(revision)?;
    repository.find_commit(oid).map_err(|error| {
        if error.code() == git2::ErrorCode::NotFound {
            no_such_revision()
        } else {
            git(&error)
        }
    })
}

/// The object id a `Revision` names. `Revision` holds 40 or 64 lower-case hex characters, and
/// these repositories use SHA-1, so a revision of 64 (a SHA-256 commit id) can never name a
/// commit here: it is `NotFound` (`no_such_revision`), the answer to a revision the repository
/// does not hold, never `Unavailable`, which a caller would retry. The field is the caller's to
/// name, as for every revision the store does not hold.
pub(crate) fn oid_of(revision: &Revision) -> Result<Oid, ApiError> {
    if revision.as_str().len() != SHA1_HEX_LEN {
        return Err(no_such_revision());
    }
    Oid::from_str(revision.as_str()).map_err(|error| git(&error))
}

/// The answer to a revision that names no commit of the repository.
pub(crate) fn no_such_revision() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "No revision with that identity here")
}

/// The `Revision` of a commit id.
pub(crate) fn revision_of(oid: Oid) -> Result<Revision, ApiError> {
    Revision::try_from(oid.to_string())
        .map_err(|error| internal(format!("a commit id is not a revision: {error}")))
}

/// The newest commit after `since`, up to `tip`, whose trailer names `mutation`.
pub(crate) fn find_trailer(
    repository: &Repository,
    tip: Oid,
    since: Oid,
    mutation: MutationId,
) -> Result<Option<Oid>, ApiError> {
    let wanted = mutation.0.to_string();
    let mut walk = repository.revwalk().map_err(|error| git(&error))?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)
        .map_err(|error| git(&error))?;
    walk.push(tip).map_err(|error| git(&error))?;
    walk.hide(since).map_err(|error| git(&error))?;
    for oid in walk {
        let oid = oid.map_err(|error| git(&error))?;
        let commit = repository.find_commit(oid).map_err(|error| git(&error))?;
        if commit
            .message()
            .is_ok_and(|message| trailer_of(message) == Some(wanted.as_str()))
        {
            return Ok(Some(oid));
        }
    }
    Ok(None)
}

/// The newest commit on the accepted line, from `tip` back through first parents to the root,
/// whose trailer names `mutation`.
///
/// No commit time is read and nothing bounds the walk but the root. It follows only `tip`'s
/// first parents, so a proposal candidate, the second parent of a merge, or any other
/// reference is never seen.
pub(crate) fn find_on_line(
    repository: &Repository,
    tip: Oid,
    mutation: MutationId,
) -> Result<Option<Oid>, ApiError> {
    let wanted = mutation.0.to_string();
    let mut next = Some(tip);
    while let Some(oid) = next {
        let commit = repository.find_commit(oid).map_err(|error| git(&error))?;
        if commit
            .message()
            .is_ok_and(|message| trailer_of(message) == Some(wanted.as_str()))
        {
            return Ok(Some(oid));
        }
        next = commit.parent_ids().next();
    }
    Ok(None)
}

/// The message of a commit with its trailer.
pub(crate) fn message_with_trailer(message: &str, mutation: MutationId) -> String {
    format!("{}\n\n{TRAILER} {}\n", message.trim_end(), mutation.0)
}

/// The mutation id a commit message's trailer block names: only the message's last
/// paragraph is the trailer block, and only a line that starts with the exact key counts. A
/// body that quotes another commit's trailer line names nothing.
pub(crate) fn trailer_of(message: &str) -> Option<&str> {
    let (_, block) = split_trailer_block(message);
    block?
        .lines()
        .find_map(|line| line.strip_prefix(TRAILER))
        .and_then(|value| value.strip_prefix(' '))
        .map(str::trim_end)
}

/// A message split into its text and its trailer block (the last paragraph, when the message
/// has more than one paragraph and that paragraph holds the trailer key at a line start).
pub(crate) fn split_trailer_block(message: &str) -> (&str, Option<&str>) {
    let trimmed = message.trim_end();
    match trimmed.rsplit_once("\n\n") {
        Some((text, block)) if block.lines().any(|line| line.starts_with(TRAILER)) => {
            (text, Some(block))
        }
        _ => (trimmed, None),
    }
}

/// A commit signature for `who` at the current time.
pub(crate) fn signature(who: &Provenance) -> Result<Signature<'static>, ApiError> {
    let route = serde_json::to_value(&who.route)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    let email = who
        .client_id
        .as_ref()
        .map_or_else(|| route.clone(), |client| format!("{route}+{client}"));
    Signature::now(&sanitize(&who.subject), &sanitize(&email)).map_err(|error| git(&error))
}

/// Write the files of a workspace tree under `root` (`materialize_at` for the whole tree).
pub(crate) fn materialize(
    repository: &Repository,
    tree: &Tree<'_>,
    root: &Path,
    holder: Holder,
) -> Result<(), ApiError> {
    materialize_at(repository, tree, root, "", holder)
}

/// Write the files of `tree`, the folder at `at` of a workspace tree (empty for the whole
/// tree), under `root`.
///
/// A tree that holds two paths sharing a collision key (two spellings of one file or folder),
/// or a path the server writes spelled another way (`item::respelled`), is refused as
/// `Conflict` before anything is written, naming both spellings and the repair
/// (`Respelled::held`: for `holder`'s `Head`, a whole-tree restore of a revision from before
/// they appeared; for a revision a Rewind restores, `Restored`, the choice of an earlier one):
/// on a case-insensitive filesystem the staged copy would merge the two, or the server's next
/// write would land in the other spelling, so a commit made from it would silently respell,
/// drop or hide a file. Git, and a case-sensitive filesystem, can hold such a tree (a commit of
/// an earlier build, an archive); it is refused the same way on every system.
pub(crate) fn materialize_at(
    repository: &Repository,
    tree: &Tree<'_>,
    root: &Path,
    at: &str,
    holder: Holder,
) -> Result<(), ApiError> {
    let paths: Vec<String> = files(repository, tree)?
        .into_iter()
        .map(|(path, _)| {
            if at.is_empty() {
                path
            } else {
                format!("{at}/{path}")
            }
        })
        .collect();
    if let Some(found) = super::item::respelled(paths.iter().map(String::as_str)) {
        return Err(found.held(holder));
    }
    write_files(repository, tree, root)
}

/// What `materialize` would refuse in `tree`, a tree that an archive's reference `reference`
/// names, in the words of a refused archive (`item::respelled`, `Respelled::archived`); `None`
/// when the tree holds no such thing.
pub(crate) fn respelled_in_archive(
    repository: &Repository,
    tree: &Tree<'_>,
    reference: &str,
) -> Result<Option<String>, ApiError> {
    let paths = files(repository, tree)?;
    Ok(
        super::item::respelled(paths.iter().map(|(path, _)| path.as_str()))
            .map(|found| found.archived(reference)),
    )
}

/// Write the files of `tree` under `root`, as `materialize` does once the tree is checked.
fn write_files(repository: &Repository, tree: &Tree<'_>, root: &Path) -> Result<(), ApiError> {
    for entry in tree {
        let name = entry
            .name()
            .ok()
            .ok_or_else(|| internal("a tree entry name is not UTF-8"))?;
        let target = root.join(name);
        match entry.kind() {
            Some(ObjectType::Tree) => {
                std::fs::create_dir_all(&target)
                    .map_err(|error| io_error("create", &target, &error))?;
                let child = repository
                    .find_tree(entry.id())
                    .map_err(|error| git(&error))?;
                write_files(repository, &child, &target)?;
            }
            Some(ObjectType::Blob) => {
                let blob = repository
                    .find_blob(entry.id())
                    .map_err(|error| git(&error))?;
                std::fs::write(&target, blob.content())
                    .map_err(|error| io_error("write", &target, &error))?;
            }
            _ => {
                return Err(internal(
                    "a workspace tree holds an entry that is not a file",
                ));
            }
        }
    }
    Ok(())
}

/// Write the files under `root` as a tree; an empty directory is left out.
pub(crate) fn write_tree(repository: &Repository, root: &Path) -> Result<Option<Oid>, ApiError> {
    let mut builder = repository.treebuilder(None).map_err(|error| git(&error))?;
    let mut entries: Vec<_> = std::fs::read_dir(root)
        .map_err(|error| io_error("read", root, &error))?
        .collect::<Result<_, _>>()
        .map_err(|error| io_error("read", root, &error))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| internal("a staged file name is not UTF-8"))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|error| io_error("inspect", &path, &error))?;
        if kind.is_dir() {
            if let Some(oid) = write_tree(repository, &path)? {
                builder
                    .insert(&name, oid, TREE_MODE)
                    .map_err(|error| git(&error))?;
            }
        } else if kind.is_file() {
            let bytes = std::fs::read(&path).map_err(|error| io_error("read", &path, &error))?;
            let oid = repository.blob(&bytes).map_err(|error| git(&error))?;
            builder
                .insert(&name, oid, BLOB_MODE)
                .map_err(|error| git(&error))?;
        }
    }
    if builder.is_empty() {
        return Ok(None);
    }
    builder.write().map(Some).map_err(|error| git(&error))
}

/// The tree of a staging directory, the empty tree when it holds nothing.
pub(crate) fn staged_tree(repository: &Repository, root: &Path) -> Result<Oid, ApiError> {
    match write_tree(repository, root)? {
        Some(oid) => Ok(oid),
        None => repository
            .treebuilder(None)
            .and_then(|builder| builder.write())
            .map_err(|error| git(&error)),
    }
}

/// Every file of a tree as (path, blob id), in path order.
pub(crate) fn files(
    repository: &Repository,
    tree: &Tree<'_>,
) -> Result<Vec<(String, Oid)>, ApiError> {
    let mut found = Vec::new();
    collect(repository, tree, "", &mut found)?;
    found.sort();
    Ok(found)
}

fn collect(
    repository: &Repository,
    tree: &Tree<'_>,
    prefix: &str,
    found: &mut Vec<(String, Oid)>,
) -> Result<(), ApiError> {
    for entry in tree {
        let name = entry
            .name()
            .ok()
            .ok_or_else(|| internal("a tree entry name is not UTF-8"))?;
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        match entry.kind() {
            Some(ObjectType::Tree) => {
                let child = repository
                    .find_tree(entry.id())
                    .map_err(|error| git(&error))?;
                collect(repository, &child, &path, found)?;
            }
            Some(ObjectType::Blob) => found.push((path, entry.id())),
            _ => {}
        }
    }
    Ok(())
}

/// The bytes of a blob.
pub(crate) fn blob_bytes(repository: &Repository, oid: Oid) -> Result<Vec<u8>, ApiError> {
    Ok(repository
        .find_blob(oid)
        .map_err(|error| git(&error))?
        .content()
        .to_vec())
}

/// A Git failure as an unavailable store: libgit2's class and code of the failure, and its
/// message with every absolute path named relative to the data directory (`git_message`).
pub(crate) fn git(error: &git2::Error) -> ApiError {
    ApiError::new(
        ErrorCode::Unavailable,
        format!(
            "the version store failed ({:?}, {:?}): {}",
            error.class(),
            error.code(),
            git_message(error)
        ),
    )
}

/// libgit2's message of `error` with every absolute path in it (`'D:/data/repositories/...'`,
/// `/home/...`, `\\?\C:\...`) named relative to the data directory (`relative_to_data`), so an
/// error mapped from Git never carries a machine path, a user name or a drive letter; the rest
/// of the message is kept. Every message of a Git failure goes through here.
pub(crate) fn git_message(error: &git2::Error) -> String {
    without_machine_paths(error.message())
}

/// `message` with each absolute path in it replaced by `relative_to_data` of it. A path starts
/// at the start of the message or after a space, a quote, `(`, `=` or `,`; a quoted one ends at
/// its closing quote (one followed by the end, a space or punctuation, so a quote inside a
/// path does not end it), an unquoted one at the next space or quote, without the punctuation
/// that follows it.
fn without_machine_paths(message: &str) -> String {
    let mut kept = String::with_capacity(message.len());
    let mut rest = message;
    while let Some((start, quote)) = absolute_start(rest) {
        let Some((before, from)) = rest.split_at_checked(start) else {
            break;
        };
        let Some((path, after)) = from.split_at_checked(path_end(from, quote)) else {
            kept.push_str(before);
            kept.push_str(OUTSIDE);
            return kept;
        };
        kept.push_str(before);
        kept.push_str(&relative_to_data(path));
        rest = after;
    }
    kept.push_str(rest);
    kept
}

/// Where the first absolute path of `text` starts, and the quote just before it, if any.
fn absolute_start(text: &str) -> Option<(usize, Option<char>)> {
    let mut previous = None;
    for (at, character) in text.char_indices() {
        let opens = previous.is_none_or(|before: char| {
            before.is_whitespace() || matches!(before, '\'' | '"' | '(' | '=' | ',')
        });
        if opens && text.get(at..).is_some_and(is_absolute) {
            return Some((at, previous.filter(|before| matches!(before, '\'' | '"'))));
        }
        previous = Some(character);
    }
    None
}

/// Whether `text` starts with an absolute path: `/`, `\\` (a UNC or `\\?\` path) or a drive
/// letter and its separator.
fn is_absolute(text: &str) -> bool {
    let mut characters = text.chars();
    match (characters.next(), characters.next(), characters.next()) {
        (Some('/'), ..) | (Some('\\'), Some('\\'), _) => true,
        (Some(drive), Some(':'), Some('/' | '\\')) => drive.is_ascii_alphabetic(),
        _ => false,
    }
}

/// The length of the path `from` starts with (see `without_machine_paths`).
fn path_end(from: &str, quote: Option<char>) -> usize {
    let ends = |after: Option<char>| {
        after.is_none_or(|after| {
            after.is_whitespace() || matches!(after, ':' | ',' | ';' | ')' | '.')
        })
    };
    if let Some(quote) = quote {
        return from
            .char_indices()
            .find(|(at, character)| {
                *character == quote
                    && ends(
                        from.get(at.saturating_add(1)..)
                            .and_then(|after| after.chars().next()),
                    )
            })
            .map_or(from.len(), |(at, _)| at);
    }
    let token = from
        .char_indices()
        .find(|(_, character)| character.is_whitespace() || matches!(character, '\'' | '"'))
        .map_or(from, |(at, _)| from.get(..at).unwrap_or(from));
    token.trim_end_matches([':', ',', ';', ')', '.']).len()
}

/// An absolute path named from its last `repositories` or `staging` component on, the folders
/// of the data directory that libgit2 reads and writes (`GIT_FOLDERS`); `OUTSIDE` for a path
/// under neither, such as a configuration file in a home directory. Known limit: a tenant, or a
/// reference name in a crafted archive, spelled exactly `repositories` or `staging` makes the
/// named path start after the data directory's own folder, never before it.
fn relative_to_data(path: &str) -> String {
    let parts: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();
    parts
        .iter()
        .rposition(|part| GIT_FOLDERS.contains(part))
        .and_then(|at| parts.get(at..))
        .map_or_else(|| OUTSIDE.to_owned(), |inside| inside.join("/"))
}

pub(crate) fn internal(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Internal, message)
}

/// Remove a directory tree; `true` when it was there.
fn remove_tree(path: &Path) -> Result<bool, ApiError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error("remove", path, &error)),
    }
}

/// A signature field without the characters Git refuses in one.
fn sanitize(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .filter(|character| !matches!(character, '<' | '>' | '\n' | '\r' | '\0'))
        .collect();
    if cleaned.trim().is_empty() {
        "unknown".to_owned()
    } else {
        cleaned
    }
}

/// Unit tests here, not in a test target: the path scrub is private to the crate.
#[cfg(test)]
mod tests {
    use super::without_machine_paths;

    /// Messages of the shapes libgit2 writes, with the data directory's absolute path in them,
    /// and what each becomes: every path relative to the data directory, every other word kept.
    #[test]
    fn a_git_message_names_every_path_relative_to_the_data_directory() {
        let cases = [
            (
                "failed to lock file 'D:/okf/tmp/.tmpAb12/repositories/local/w.git/refs/heads/main.lock' for writing: File exists",
                "failed to lock file 'repositories/local/w.git/refs/heads/main.lock' for writing: File exists",
            ),
            (
                "could not find repository at '/home/ana/.local/share/okf/repositories/local/w.git/'",
                "could not find repository at 'repositories/local/w.git'",
            ),
            (
                r"failed to resolve path '\\?\C:\Users\ana\data\staging\restore-1\history-check.git': gone",
                "failed to resolve path 'staging/restore-1/history-check.git': gone",
            ),
            (
                "failed to map '/srv/okf/repositories/local/w.git/objects/pack/p.pack': denied, and \"C:/x/staging/a\"",
                "failed to map 'repositories/local/w.git/objects/pack/p.pack': denied, and \"staging/a\"",
            ),
            (
                "unquoted /var/okf/repositories/local/w.git/config: bad",
                "unquoted repositories/local/w.git/config: bad",
            ),
            (
                "failed to parse config file 'C:/Users/O'Brien/.gitconfig' (in line 2)",
                "failed to parse config file 'a path outside the data directory' (in line 2)",
            ),
            (
                "object not found - no match for id (0123abcd)",
                "object not found - no match for id (0123abcd)",
            ),
            (
                "reference 'refs/heads/main' not found",
                "reference 'refs/heads/main' not found",
            ),
        ];
        for (message, kept) in cases {
            assert_eq!(without_machine_paths(message), kept, "{message}");
        }
    }
}
