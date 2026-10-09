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

    /// Open an existing workspace repository, with durable writes (`durable`); `NotFound` when
    /// the workspace has none.
    pub(crate) fn open(&self, scope: &StorageScope) -> Result<Repository, ApiError> {
        let path = self.path(scope);
        if !path.exists() {
            return Err(ApiError::new(
                ErrorCode::NotFound,
                "No workspace with that identity here",
            ));
        }
        durable(&path)
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

/// The commit a `Revision` names; `NotFound` when the repository does not hold it.
pub(crate) fn commit_of<'r>(
    repository: &'r Repository,
    revision: &Revision,
) -> Result<Commit<'r>, ApiError> {
    let oid = Oid::from_str(revision.as_str()).map_err(|error| git(&error))?;
    repository.find_commit(oid).map_err(|error| {
        if error.code() == git2::ErrorCode::NotFound {
            ApiError::new(ErrorCode::NotFound, "No revision with that identity here")
        } else {
            git(&error)
        }
    })
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
    let wanted = format!("{TRAILER} {}", mutation.0);
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
            .is_ok_and(|message| message.lines().any(|line| line.trim() == wanted))
        {
            return Ok(Some(oid));
        }
    }
    Ok(None)
}

/// The message of a commit with its trailer.
pub(crate) fn message_with_trailer(message: &str, mutation: MutationId) -> String {
    format!("{}\n\n{TRAILER} {}\n", message.trim_end(), mutation.0)
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

/// Write the files of `tree` under `root`.
pub(crate) fn materialize(
    repository: &Repository,
    tree: &Tree<'_>,
    root: &Path,
) -> Result<(), ApiError> {
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
                materialize(repository, &child, &target)?;
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

/// A Git failure as an unavailable store.
pub(crate) fn git(error: &git2::Error) -> ApiError {
    ApiError::new(
        ErrorCode::Unavailable,
        format!("the version store failed: {}", error.message()),
    )
}

pub(crate) fn internal(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Internal, message)
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
