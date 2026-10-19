//! The binary's offline commands: `backup --output <directory> [--tenant <id>]` and `restore
//! --replacement <installation archive> [<workspace archive>...]` (Stage 1b design section 8).
//!
//! They hold no behaviour of their own: each checks its arguments, opens the data directory with
//! `Storage::open` (which takes the single-writer lock first, so either is refused while a
//! service or another command owns the directory, and migrates a directory an older build wrote,
//! with its pre-migration backup), and calls storage's one function for it
//! (`StorageBackups::write_backup_set`, `StorageBackups::restore_replacement`). They read only
//! the mode and the data directory (`settings::offline_settings`): no WorkOS credential, no
//! listener.
//!
//! - `backup` names the local tenant itself in a local installation, so its installation
//!   archive is written even before the tenant has a record (ledger: the unfiltered backup of a
//!   record-less tenant). `--tenant` is for a hosted deployment and is refused locally. The
//!   output is created; it must not exist or be an empty directory, so a set is never mixed with
//!   an older set's archives, and it must not lie inside the data directory: a backup is
//!   recoverable after the data directory is deleted (SPEC section 12), and purges reach sets
//!   under `<data>/backups`.
//! - `restore` checks that every path names a file before the directory is opened; every other
//!   refusal (an archive of another tenant, one this build cannot read, a workspace that exists or
//!   was purged) is storage's, with its own words and codes.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::TenantId;
use okf_jawn_contract::workspace::RestoreReport;
use okf_jawn_storage::Storage;
use okf_jawn_storage::data::RECORDS_FILE;
use okf_jawn_storage::format::needs_upgrade;
use okf_jawn_storage::schema;

use crate::config::{AuthKind, invalid};

/// What `backup` is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupArgs {
    /// Where the set is written (`--output`); relative to the working directory when relative.
    pub output: PathBuf,
    /// The one tenant to back up (`--tenant`), hosted only.
    pub tenant: Option<String>,
}

/// What `restore` is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreArgs {
    /// The installation archive (`--replacement`), restored first.
    pub installation: PathBuf,
    /// The workspace archives, each restored after it.
    pub workspaces: Vec<PathBuf>,
}

/// Write the backup set of the data directory `data_dir` and answer every archive written.
///
/// # Errors
/// Returns `InvalidInput` naming `--tenant` for a tenant named locally or one that is not a
/// tenant id, and naming `--output` for an output that exists and is not an empty directory or
/// that lies inside the data directory; storage's `Unavailable` when another process owns the
/// directory; and storage's errors of the backup itself.
pub async fn backup(
    kind: AuthKind,
    data_dir: &Path,
    args: &BackupArgs,
) -> Result<Vec<PathBuf>, ApiError> {
    let only = match (kind, &args.tenant) {
        (AuthKind::Local, Some(_)) => {
            return Err(invalid(
                "--tenant",
                "is for a hosted deployment: a local installation has one tenant",
            ));
        }
        (AuthKind::Local, None) => Some(TenantId::local()),
        (AuthKind::Hosted, Some(tenant)) => Some(
            TenantId::try_from(tenant.clone())
                .map_err(|_| invalid("--tenant", "must be a tenant id"))?,
        ),
        (AuthKind::Hosted, None) => None,
    };
    let output = std::path::absolute(&args.output)
        .map_err(|_| invalid("--output", "must be a path this system can resolve"))?;
    if output.exists() && !is_empty_directory(&output) {
        return Err(invalid(
            "--output",
            "must not exist or be an empty directory: a backup set is never mixed with another",
        ));
    }
    if inside(&output, data_dir)? {
        return Err(invalid(
            "--output",
            "must not lie inside the data directory: a backup must outlive it",
        ));
    }
    let storage = open_storage(data_dir).await?;
    std::fs::create_dir_all(&output).map_err(|error| {
        ApiError::new(
            ErrorCode::Unavailable,
            format!(
                "the output directory could not be created ({:?})",
                error.kind()
            ),
        )
    })?;
    storage
        .backups()
        .write_backup_set(&output, only.as_ref())
        .await
}

/// Replace the installation in the data directory `data_dir` from a backup set, and answer what
/// each workspace's restore did.
///
/// # Errors
/// Returns `InvalidInput` naming `--replacement` or `workspace archive` for a path that names no
/// file; storage's `Unavailable` when another process owns the directory; and storage's
/// refusals of the archives.
pub async fn restore(data_dir: &Path, args: &RestoreArgs) -> Result<Vec<RestoreReport>, ApiError> {
    if !args.installation.is_file() {
        return Err(invalid(
            "--replacement",
            "must name the installation archive, a file",
        ));
    }
    if args.workspaces.iter().any(|path| !path.is_file()) {
        return Err(invalid(
            "workspace archive",
            "must each name a workspace archive, a file",
        ));
    }
    let storage = open_storage(data_dir).await?;
    storage
        .backups()
        .restore_replacement(&args.installation, &args.workspaces)
        .await
}

/// Whether `path` is a directory that holds nothing.
fn is_empty_directory(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

/// Whether `path` is `directory` or lies under it, compared by components after resolving the
/// part of each that exists (links and, on Windows, letter case resolved).
fn inside(path: &Path, directory: &Path) -> Result<bool, ApiError> {
    let path = resolved(path)?;
    let directory = resolved(directory)?;
    let mut path = path.components();
    for part in directory.components() {
        match path.next() {
            Some(other) if same(part.as_os_str(), other.as_os_str()) => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// `path` with its longest existing ancestor canonicalized and the rest appended as written.
fn resolved(path: &Path) -> Result<PathBuf, ApiError> {
    let absolute = std::path::absolute(path)
        .map_err(|_| invalid("--output", "must be a path this system can resolve"))?;
    let mut rest = Vec::new();
    let mut existing = absolute.as_path();
    loop {
        if let Ok(canonical) = existing.canonicalize() {
            let mut whole = canonical;
            for part in rest.iter().rev() {
                whole.push(part);
            }
            return Ok(whole);
        }
        let Some(parent) = existing.parent() else {
            return Ok(absolute.clone());
        };
        if let Some(name) = existing.file_name() {
            rest.push(name.to_os_string());
        } else if !matches!(existing.components().next_back(), Some(Component::CurDir)) {
            return Ok(absolute.clone());
        }
        existing = parent;
    }
}

/// Whether two path components name the same entry: exactly on Unix, ignoring ASCII case on
/// Windows, whose file systems compare names that way.
fn same(left: &OsStr, right: &OsStr) -> bool {
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

/// Open the data directory `data_dir` for this process alone, off the async threads: the
/// single-writer lock, the refusal of newer data, the pre-migration backup and the migrations
/// (`Storage::open`). When the records database needs an upgrade, a line on stderr says so first,
/// so a person or a launcher waiting on a long pre-migration backup can see why.
///
/// # Errors
/// Returns storage's refusal: `Unavailable` when another process owns the directory,
/// `Unsupported` for data newer than this build, `Internal` for a migration that does not
/// verify.
pub(crate) async fn open_storage(data_dir: &Path) -> Result<Storage, ApiError> {
    let directory = data_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let records = directory.join(RECORDS_FILE);
        // An unreadable version is storage's to report, by the open that follows.
        if records.is_file() && needs_upgrade(&records, &schema::RECORDS).unwrap_or(false) {
            announce(&format!(
                "migrating {RECORDS_FILE} to this build's schema (a pre-migration backup is \
                 written first)"
            ));
        }
        Storage::open(&directory)
    })
    .await
    .map_err(|_| ApiError::new(ErrorCode::Internal, "the data directory's opening stopped"))?
}

/// One line on stderr, `okf-jawn-server: <text>`; an error writing it is ignored, as a closed
/// stderr must not stop the server.
pub(crate) fn announce(text: &str) {
    use std::io::Write as _;
    let _written = writeln!(std::io::stderr().lock(), "okf-jawn-server: {text}");
}
