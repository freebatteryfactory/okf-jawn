//! The data directory: its single-writer lock, its layout and its format version.
//!
//! SPEC section 2: one process owns a data directory's writable Git, SQLite and object stores.
//! `DataDir::open` takes an exclusive lock on `okf-jawn.lock` (std `File::try_lock`) before any
//! writable store is opened, and the lock lives as long as the `DataDir`; a second process is
//! refused with a clear error. `Storage` and every store it hands out share the `DataDir`
//! through an `Arc`, so the lock is released only when the last store that can write drops. A data directory on a network filesystem is not supported unless
//! qualified: its lock semantics are not the local filesystem's.
//!
//! The layout itself is an application-owned format and carries a version in `format.json`.
//! Data written by a newer build is refused before anything in the directory is changed.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use okf_jawn_contract::error::{ApiError, ErrorCode};

/// An opened data directory whose single-writer lock this process holds.
#[derive(Debug)]
pub struct DataDir {
    root: PathBuf,
    /// Held for the life of the value; the operating system releases it when the file closes.
    _lock: File,
}

/// What `format.json` says about the directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatState {
    /// No format file: a new data directory, written at the current version.
    Absent,
    /// The version this build writes.
    Current,
    /// An older version this build upgrades.
    Older(u32),
}

/// File whose exclusive lock is the single-writer guarantee.
pub const LOCK_FILE: &str = "okf-jawn.lock";
/// Application-owned format marker of the directory layout.
pub const FORMAT_FILE: &str = "format.json";
/// The format name written in `format.json`.
pub const FORMAT_NAME: &str = "okf-jawn-data";
/// The layout version this build writes and reads.
pub const FORMAT_VERSION: u32 = 1;
/// The application records database, relative to the root.
pub const RECORDS_FILE: &str = "records.sqlite";
/// The rebuildable search index database, relative to the root.
pub const INDEX_FILE: &str = "index.sqlite";

impl DataDir {
    /// Take the single-writer lock on `root`, creating the directory when it is new, and check
    /// the format version without changing anything.
    ///
    /// # Errors
    /// Returns `Unavailable` naming the directory when another process holds the lock or the
    /// lock cannot be taken, and `Unsupported` when the directory was written by a newer build.
    pub fn open(root: &Path) -> Result<Self, ApiError> {
        std::fs::create_dir_all(root)
            .map_err(|error| io_error("create the data directory", root, &error))?;
        let lock_path = root.join(LOCK_FILE);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| io_error("open the lock file", &lock_path, &error))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(ApiError::new(
                    ErrorCode::Unavailable,
                    format!(
                        "another okf-jawn process already owns the data directory {}; stop it \
                         before starting another (one process owns a data directory)",
                        root.display()
                    ),
                ));
            }
            Err(TryLockError::Error(error)) => {
                return Err(io_error("lock", &lock_path, &error));
            }
        }
        let data = Self {
            root: root.to_path_buf(),
            _lock: lock,
        };
        data.format_state()?;
        Ok(data)
    }

    /// The directory root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The application records database.
    #[must_use]
    pub fn records_path(&self) -> PathBuf {
        self.root.join(RECORDS_FILE)
    }

    /// The rebuildable search index database; rebuilding it never touches the records.
    #[must_use]
    pub fn index_path(&self) -> PathBuf {
        self.root.join(INDEX_FILE)
    }

    /// The content-addressed object store.
    #[must_use]
    pub fn objects_path(&self) -> PathBuf {
        self.root.join("objects")
    }

    /// The Git repositories, one per workspace.
    #[must_use]
    pub fn repositories_path(&self) -> PathBuf {
        self.root.join("repositories")
    }

    /// Private staging directories of candidate trees.
    #[must_use]
    pub fn staging_path(&self) -> PathBuf {
        self.root.join("staging")
    }

    /// Recoverable backups taken before an upgrade migration.
    #[must_use]
    pub fn pre_migration_path(&self) -> PathBuf {
        self.root.join("backups").join("pre-migration")
    }

    /// Read `format.json` and refuse a version newer than this build.
    ///
    /// # Errors
    /// Returns `Unsupported` for a newer version or another format name, and `Unavailable`
    /// when the file cannot be read.
    pub fn format_state(&self) -> Result<FormatState, ApiError> {
        let path = self.root.join(FORMAT_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(FormatState::Absent),
            Err(error) => return Err(io_error("read", &path, &error)),
        };
        let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
            unsupported(format!(
                "{} is not a format marker: {error}",
                path.display()
            ))
        })?;
        let name = value.get("format").and_then(serde_json::Value::as_str);
        if name != Some(FORMAT_NAME) {
            return Err(unsupported(format!(
                "{} does not name the {FORMAT_NAME} format",
                path.display()
            )));
        }
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .and_then(|version| u32::try_from(version).ok())
            .ok_or_else(|| unsupported(format!("{} has no format version", path.display())))?;
        match version.cmp(&FORMAT_VERSION) {
            std::cmp::Ordering::Greater => Err(unsupported(format!(
                "the data directory {} has format version {version}, newer than version \
                 {FORMAT_VERSION} this build understands; it was left unchanged",
                self.root.display()
            ))),
            std::cmp::Ordering::Equal => Ok(FormatState::Current),
            std::cmp::Ordering::Less => Ok(FormatState::Older(version)),
        }
    }

    /// Write `format.json` at the current version.
    ///
    /// The marker is written to `format.json.tmp`, synced, renamed over `format.json`, and then
    /// the directory is synced, so a power loss leaves either the old marker or the whole new
    /// one, never an empty or truncated file. The directory sync runs on Unix only: on Windows
    /// NTFS journals the rename's metadata, and std cannot open a directory as a file there.
    ///
    /// # Errors
    /// Returns `Unavailable` when the file cannot be written.
    pub fn write_format(&self) -> Result<(), ApiError> {
        let path = self.root.join(FORMAT_FILE);
        let body = serde_json::json!({ "format": FORMAT_NAME, "version": FORMAT_VERSION });
        let temporary = self.root.join(format!("{FORMAT_FILE}.tmp"));
        let mut file =
            File::create(&temporary).map_err(|error| io_error("write", &temporary, &error))?;
        file.write_all(format!("{body}\n").as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| io_error("write and sync", &temporary, &error))?;
        drop(file);
        steps::record("sync format.json.tmp");
        std::fs::rename(&temporary, &path).map_err(|error| io_error("replace", &path, &error))?;
        steps::record("rename");
        if cfg!(unix) {
            File::open(&self.root)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| io_error("sync", &self.root, &error))?;
            steps::record("sync directory");
        }
        Ok(())
    }

    /// Remove everything a crash left under `staging`; nothing there is ever referenced.
    ///
    /// # Errors
    /// Returns `Unavailable` when the directory cannot be cleared.
    pub fn clear_staging(&self) -> Result<(), ApiError> {
        let staging = self.staging_path();
        match std::fs::remove_dir_all(&staging) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("clear", &staging, &error)),
        }
        std::fs::create_dir_all(&staging).map_err(|error| io_error("create", &staging, &error))
    }
}

/// A filesystem failure naming the path.
pub(crate) fn io_error(action: &str, path: &Path, error: &std::io::Error) -> ApiError {
    ApiError::new(
        ErrorCode::Unavailable,
        format!("could not {action} {}: {error}", path.display()),
    )
}

fn unsupported(message: String) -> ApiError {
    ApiError::new(ErrorCode::Unsupported, message)
}

/// The durability steps of `write_format`, recorded per thread in unit tests so their order
/// can be checked; nothing is recorded outside tests.
mod steps {
    #[cfg(test)]
    thread_local! {
        static DONE: std::cell::RefCell<Vec<&'static str>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }

    /// Record one step.
    #[cfg(test)]
    pub(super) fn record(step: &'static str) {
        DONE.with(|done| done.borrow_mut().push(step));
    }

    /// Record nothing outside tests.
    #[cfg(not(test))]
    pub(super) const fn record(_step: &'static str) {}

    /// The steps recorded on this thread, cleared.
    #[cfg(test)]
    pub(super) fn take() -> Vec<&'static str> {
        DONE.with(std::cell::RefCell::take)
    }
}

/// Unit tests here, not in a test target: the step record is private to the crate. They return
/// `ApiError` like core's unit tests.
#[cfg(test)]
mod tests {
    use okf_jawn_contract::error::{ApiError, ErrorCode};

    use super::{DataDir, FORMAT_FILE, steps};

    #[test]
    fn the_format_marker_is_synced_before_it_replaces_the_old_one() -> Result<(), ApiError> {
        let directory = tempfile::tempdir()
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        let data = DataDir::open(directory.path())?;
        steps::take();
        data.write_format()?;
        let done = steps::take();
        let mut expected = vec!["sync format.json.tmp", "rename"];
        if cfg!(unix) {
            expected.push("sync directory");
        }
        assert_eq!(
            done, expected,
            "synced, then renamed, then the directory synced"
        );
        assert!(directory.path().join(FORMAT_FILE).is_file());
        assert!(!directory.path().join("format.json.tmp").exists());
        Ok(())
    }
}
