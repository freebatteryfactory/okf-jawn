//! A portable workspace archive of one accepted revision (M2a design section 1).
//!
//! Committed text is staged a page at a time, keeping its exact bytes unless core writes
//! `verified` or the root index needs an OKF version. Only entry metadata remains in memory
//! across pages. All final paths, including references and optional history, are sorted
//! together before a stored, fixed-time ZIP is streamed into the blob store. The temporary
//! directory stays owned until every blocking writer/reader has joined, including errors.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::sync::Arc;

use okf_core::{Document, yaml::Value};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, Revision, WorkspacePath, collision_key_of};
use okf_jawn_contract::import::JobState;
use okf_jawn_core::portable::export_item_file;
use okf_jawn_core::reading::{check_revision, read_conversion_record};
use okf_jawn_core::storage::{CommittedFile, FilePage, ObjectInfo, StorageScope};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt as _;
use unicode_normalization::UnicodeNormalization as _;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

use crate::handler::{Attempt, HandlerPorts, Renewed, renewing};

/// The producer's output; only the handler records an artifact and completes its claim.
pub(crate) enum Exported {
    Ready(ObjectInfo),
    Cancelled,
    Refused(ApiError),
}

/// A staged committed file or a materialized immutable object, without its contents in memory.
struct Entry {
    name: String,
    source: PathBuf,
}

/// Only staged paths and reference identities survive each bounded tree page.
struct WorkspaceArchive {
    directory: Arc<TempDir>,
    entries: Vec<Entry>,
    originals: BTreeMap<Digest, String>,
    records: BTreeSet<Digest>,
    references: String,
    root_seen: bool,
    written: usize,
}

const FILES_PER_PAGE: u16 = 500;
const PAGE_BYTES: u64 = 32 * 1024 * 1024;
const COPY_BYTES: usize = 1024 * 1024;

impl WorkspaceArchive {
    /// Stage one bounded page, retaining only the reference identities it names.
    async fn stage_page(
        &mut self,
        ports: &HandlerPorts,
        attempt: &Attempt<'_>,
        scope: &StorageScope,
        files: Vec<CommittedFile>,
    ) -> Result<bool, ApiError> {
        for file in files {
            let CommittedFile {
                path,
                mut bytes,
                item,
            } = file;
            validate_name(&path)?;
            if let Some((folder, _)) = path.split_once('/')
                && collision_key_of(folder) == "references"
            {
                folder.clone_into(&mut self.references);
            }
            if path == "index.md" {
                self.root_seen = true;
                bytes = root_index(bytes)?;
            } else if let Some(item) = item {
                let reviews = match renewing(
                    ports,
                    attempt,
                    ports.records.list_reviews(scope, item.item_id),
                )
                .await?
                {
                    Renewed::Done(reviews) => reviews,
                    Renewed::Cancelled(_) => return Ok(false),
                };
                if let Some(rewritten) = export_item_file(&bytes, &reviews, &item.content_digest)? {
                    bytes = rewritten;
                }
                if let Some(source) = item.source {
                    // P1 gives committed path order, so the first card chooses this name.
                    self.originals
                        .entry(source.digest)
                        .or_insert(source.original_name);
                }
                if let Some(record) = item.record {
                    self.records.insert(record);
                }
            }
            match renewing(
                ports,
                attempt,
                stage(Arc::clone(&self.directory), self.written, path, bytes),
            )
            .await?
            {
                Renewed::Done(entry) => self.entries.push(entry),
                Renewed::Cancelled(_) => return Ok(false),
            }
            self.written = self.written.saturating_add(1);
        }
        Ok(true)
    }

    /// Keep the root version and first-card original naming before derivative discovery.
    async fn add_originals(
        &mut self,
        ports: &HandlerPorts,
        attempt: &Attempt<'_>,
        scope: &StorageScope,
    ) -> Result<bool, ApiError> {
        if !self.root_seen {
            match renewing(
                ports,
                attempt,
                stage(
                    Arc::clone(&self.directory),
                    self.written,
                    "index.md".to_owned(),
                    b"---\nokf_version: \"0.2\"\n---\n\n\n".to_vec(),
                ),
            )
            .await?
            {
                Renewed::Done(entry) => self.entries.push(entry),
                Renewed::Cancelled(_) => return Ok(false),
            }
        }
        for (digest, name) in std::mem::take(&mut self.originals) {
            let name = format!(
                "{}/originals/{}/{}",
                self.references,
                digest.as_str(),
                original_file_name(&name)
            );
            let Some(entry) = materialized(ports, attempt, scope, &digest, name).await? else {
                return Ok(false);
            };
            self.entries.push(entry);
        }
        Ok(true)
    }

    /// Resolve every role globally before materializing any derivative.
    async fn add_derivatives(
        &mut self,
        ports: &HandlerPorts,
        attempt: &Attempt<'_>,
        scope: &StorageScope,
    ) -> Result<bool, ApiError> {
        // All record roles are registered before any export, and all exports before any asset.
        // A digest shared across different records keeps the ruled global role priority.
        let records = std::mem::take(&mut self.records);
        let mut derivatives: BTreeMap<Digest, &str> = records
            .iter()
            .map(|digest| (digest.clone(), "json"))
            .collect();
        let mut assets = BTreeMap::new();
        for digest in records {
            let record = match renewing(
                ports,
                attempt,
                read_conversion_record(ports.blobs.as_ref(), scope, &digest),
            )
            .await?
            {
                Renewed::Done(record) => record,
                Renewed::Cancelled(_) => return Ok(false),
            };
            for export in record.structured {
                derivatives.entry(export.digest).or_insert("json");
            }
            for asset in record.assets {
                assets.entry(asset.digest).or_insert(asset.media_type);
            }
            if attempt.renew(ports).await?.state == JobState::Cancelled {
                return Ok(false);
            }
        }
        for (digest, media_type) in assets {
            derivatives
                .entry(digest)
                .or_insert(asset_extension(&media_type));
        }
        for (digest, extension) in derivatives {
            let name = format!(
                "{}/derived/{}.{extension}",
                self.references,
                digest.as_str()
            );
            let Some(entry) = materialized(ports, attempt, scope, &digest, name).await? else {
                return Ok(false);
            };
            self.entries.push(entry);
        }
        Ok(true)
    }

    /// Add references/history, then join ZIP creation and retention before completion.
    async fn finish(
        mut self,
        ports: &HandlerPorts,
        attempt: &Attempt<'_>,
        scope: &StorageScope,
        revision: &Revision,
        include_history: bool,
    ) -> Result<Exported, ApiError> {
        if !self.add_originals(ports, attempt, scope).await? {
            return Ok(Exported::Cancelled);
        }
        if attempt.report(ports, 79).await?.state == JobState::Cancelled {
            return Ok(Exported::Cancelled);
        }
        if !self.add_derivatives(ports, attempt, scope).await? {
            return Ok(Exported::Cancelled);
        }
        if attempt.report(ports, 94).await?.state == JobState::Cancelled {
            return Ok(Exported::Cancelled);
        }
        if include_history {
            let history = match renewing(
                ports,
                attempt,
                ports
                    .versions
                    .write_history(scope, revision, attempt.claimed.mutation_id),
            )
            .await
            {
                Ok(Renewed::Done(history)) => history,
                Ok(Renewed::Cancelled(_)) => return Ok(Exported::Cancelled),
                Err(error) => return revision_failure(error),
            };
            let Some(entry) = materialized(
                ports,
                attempt,
                scope,
                &history.digest,
                "history.bundle".to_owned(),
            )
            .await?
            else {
                return Ok(Exported::Cancelled);
            };
            self.entries.push(entry);
        }
        let keep = Arc::clone(&self.directory);
        let entries = self.entries;
        let (archive, size) = match renewing(
            ports,
            attempt,
            blocking(move || write_zip(keep.as_ref(), entries)),
        )
        .await?
        {
            Renewed::Done(archive) => archive,
            Renewed::Cancelled(_) => return Ok(Exported::Cancelled),
        };
        if attempt.renew(ports).await?.state == JobState::Cancelled {
            return Ok(Exported::Cancelled);
        }
        let object = match renewing(
            ports,
            attempt,
            retain_archive(ports, scope, self.directory, archive, size),
        )
        .await?
        {
            Renewed::Done(object) => object,
            Renewed::Cancelled(_) => return Ok(Exported::Cancelled),
        };
        if attempt.report(ports, 95).await?.state == JobState::Cancelled {
            return Ok(Exported::Cancelled);
        }
        Ok(Exported::Ready(object))
    }
}

/// Write the pinned tree and precisely the objects its cards name, never another reference.
pub(crate) async fn export_workspace(
    ports: &HandlerPorts,
    attempt: &Attempt<'_>,
    scope: &StorageScope,
    revision: &Revision,
    include_history: bool,
) -> Result<Exported, ApiError> {
    if let Err(error) = check_revision(ports.records.as_ref(), scope, revision).await {
        return revision_failure(error);
    }
    let directory = Arc::new(
        blocking(|| {
            tempfile::Builder::new()
                .prefix("okf-portable-")
                .tempdir()
                .map_err(|error| io_fault("create its temporary directory", error))
        })
        .await?,
    );
    let mut archive = WorkspaceArchive {
        directory,
        entries: Vec::new(),
        originals: BTreeMap::new(),
        records: BTreeSet::new(),
        references: "references".to_owned(),
        root_seen: false,
        written: 0,
    };
    let mut after = None;
    loop {
        let page = match renewing(
            ports,
            attempt,
            ports.versions.committed_files(
                scope,
                revision,
                FilePage {
                    after: after.clone(),
                    limit: FILES_PER_PAGE,
                    max_bytes: PAGE_BYTES,
                },
            ),
        )
        .await
        {
            Ok(Renewed::Done(page)) => page,
            Ok(Renewed::Cancelled(_)) => return Ok(Exported::Cancelled),
            Err(error) => return revision_failure(error),
        };
        if attempt.renew(ports).await?.state == JobState::Cancelled {
            return Ok(Exported::Cancelled);
        }
        if !archive
            .stage_page(ports, attempt, scope, page.files)
            .await?
        {
            return Ok(Exported::Cancelled);
        }
        let progress = u8::try_from((archive.written / 200).min(49)).unwrap_or(49);
        if attempt.report(ports, progress).await?.state == JobState::Cancelled {
            return Ok(Exported::Cancelled);
        }
        match page.next_after {
            Some(next) => {
                if after.as_ref().is_some_and(|previous| &next <= previous) {
                    return Err(fault("the committed-file page did not advance"));
                }
                after = Some(next);
            }
            None => break,
        }
    }
    archive
        .finish(ports, attempt, scope, revision, include_history)
        .await
}

/// An off-line or invalidated revision is permanently refused, unlike a missing object.
fn revision_failure(error: ApiError) -> Result<Exported, ApiError> {
    if error.code == ErrorCode::NotFound {
        Ok(Exported::Refused(error))
    } else {
        Err(error)
    }
}

fn validate_name(name: &str) -> Result<(), ApiError> {
    // A Windows drive prefix is absolute even when this archive is produced on Unix.
    let drive_absolute = name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && name.as_bytes().get(1) == Some(&b':')
        && name.as_bytes().get(2) == Some(&b'/');
    if drive_absolute
        || name.contains('\\')
        || name.contains('\0')
        || name
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(fault(
            "a committed path is not a plain relative archive path",
        ));
    }
    Ok(())
}

fn root_index(bytes: Vec<u8>) -> Result<Vec<u8>, ApiError> {
    let text =
        std::str::from_utf8(&bytes).map_err(|_| fault("the committed root index is not UTF-8"))?;
    let mut document = Document::parse(text)
        .map_err(|error| fault(format!("the committed root index does not parse: {error}")))?;
    if document.frontmatter.get("okf_version").is_some() {
        return Ok(bytes);
    }
    document
        .frontmatter
        .set("okf_version", Value::String("0.2".to_owned()));
    Ok(document.serialize().into_bytes())
}

fn original_file_name(original: &str) -> String {
    let normalized: String = original.nfc().collect();
    if !normalized.contains('/') && WorkspacePath::from_supplied(&normalized).is_ok() {
        return normalized
            .strip_suffix(".md")
            .map_or_else(|| normalized.clone(), |stem| format!("{stem}.markdown"));
    }
    let extension = original.rsplit_once('.').map(|(_, extension)| extension);
    match extension.filter(|extension| {
        !extension.is_empty()
            && extension.len() <= 16
            && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }) {
        Some(extension) => {
            let extension = extension.to_ascii_lowercase();
            let extension = if extension == "md" {
                "markdown"
            } else {
                &extension
            };
            format!("original.{extension}")
        }
        None => "original".to_owned(),
    }
}

fn asset_extension(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/tiff" => "tif",
        "image/bmp" => "bmp",
        _ => "bin",
    }
}

async fn stage(
    directory: Arc<TempDir>,
    ordinal: usize,
    name: String,
    bytes: Vec<u8>,
) -> Result<Entry, ApiError> {
    blocking(move || {
        // Staging paths are our counter, never a workspace-controlled path.
        let source = directory.path().join(format!("file-{ordinal}"));
        std::fs::write(&source, bytes).map_err(|error| io_fault("stage a file", error))?;
        Ok(Entry { name, source })
    })
    .await
}

async fn materialized(
    ports: &HandlerPorts,
    attempt: &Attempt<'_>,
    scope: &StorageScope,
    digest: &Digest,
    name: String,
) -> Result<Option<Entry>, ApiError> {
    if attempt.renew(ports).await?.state == JobState::Cancelled {
        return Ok(None);
    }
    match renewing(ports, attempt, ports.blobs.materialize(scope, digest)).await? {
        Renewed::Done(source) => Ok(Some(Entry {
            name,
            source: source.path,
        })),
        Renewed::Cancelled(_) => Ok(None),
    }
}

/// Every portable segment has one spelling and is either a file or a directory, never both.
/// Impossible layouts are refused rather than relocated or written as an unextractable ZIP.
fn check_archive_paths(entries: &[Entry]) -> Result<(), ApiError> {
    let mut paths = BTreeMap::new();
    for entry in entries {
        validate_name(&entry.name)?;
        let mut parts = entry.name.split('/').peekable();
        let mut prefix = String::new();
        while let Some(part) = parts.next() {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let is_file = parts.peek().is_none();
            let identity = paths
                .entry(collision_key_of(&prefix))
                .or_insert_with(|| (prefix.clone(), is_file));
            if identity.0 != prefix || identity.1 != is_file {
                return Err(fault("the portable archive holds conflicting paths"));
            }
        }
    }
    Ok(())
}

fn write_zip(directory: &TempDir, mut entries: Vec<Entry>) -> Result<(PathBuf, u64), ApiError> {
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    if entries.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(left, right)| left.name == right.name)
    }) {
        return Err(fault("the portable archive holds a repeated path"));
    }
    check_archive_paths(&entries)?;
    let archive = directory.path().join("export.zip");
    let file = File::create(&archive).map_err(|error| io_fault("create its archive", error))?;
    let mut zip = ZipWriter::new(file);
    let mut buffer = vec![0_u8; COPY_BYTES];
    for entry in entries {
        let mut source =
            File::open(&entry.source).map_err(|error| io_fault("open an archive input", error))?;
        let size = source
            .metadata()
            .map_err(|error| io_fault("inspect an archive input", error))?
            .len();
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .last_modified_time(DateTime::DEFAULT)
            .unix_permissions(0o644)
            .large_file(size >= (1_u64 << 32));
        zip.start_file(&entry.name, options)
            .map_err(|error| io_fault("start an archive entry", error))?;
        let mut copied = 0_u64;
        loop {
            let read = source
                .read(&mut buffer)
                .map_err(|error| io_fault("read an archive input", error))?;
            if read == 0 {
                break;
            }
            zip.write_all(
                buffer
                    .get(..read)
                    .ok_or_else(|| fault("an archive read exceeded its buffer"))?,
            )
            .map_err(|error| io_fault("write an archive entry", error))?;
            copied = copied.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        }
        if copied != size {
            return Err(fault("an archive input changed size while it was read"));
        }
    }
    let file = zip
        .finish()
        .map_err(|error| io_fault("finish its archive", error))?;
    file.sync_all()
        .map_err(|error| io_fault("sync its archive", error))?;
    let size = file
        .metadata()
        .map_err(|error| io_fault("inspect its archive", error))?
        .len();
    Ok((archive, size))
}

/// A bounded duplex bridges blocking file reads into the blob store's async reader.
async fn retain_archive(
    ports: &HandlerPorts,
    scope: &StorageScope,
    directory: Arc<TempDir>,
    archive: PathBuf,
    size: u64,
) -> Result<ObjectInfo, ApiError> {
    let (reader, mut writer) = tokio::io::duplex(COPY_BYTES);
    let runtime = tokio::runtime::Handle::current();
    let feeding = tokio::task::spawn_blocking(move || {
        // Keep staged content until this last reader exits even if its async owner stops.
        let _directory = directory;
        let mut file = File::open(archive).map_err(|error| io_fault("open its archive", error))?;
        let mut buffer = vec![0_u8; COPY_BYTES];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|error| io_fault("read its archive", error))?;
            if read == 0 {
                break;
            }
            runtime
                .block_on(
                    writer.write_all(
                        buffer
                            .get(..read)
                            .ok_or_else(|| fault("an archive read exceeded its buffer"))?,
                    ),
                )
                .map_err(|error| io_fault("stream its archive", error))?;
        }
        runtime
            .block_on(writer.shutdown())
            .map_err(|error| io_fault("finish its archive stream", error))
    });
    let retained = ports.blobs.put(scope, Box::pin(reader), size, None).await;
    // Always join, including a refused put: dropping its reader unblocks the writer. The
    // port's error remains primary over the resulting broken pipe, not a successful export.
    let fed = feeding
        .await
        .map_err(|error| fault(format!("the portable archive reader stopped: {error}")))?;
    match retained {
        Ok(object) => {
            fed?;
            Ok(object)
        }
        Err(error) => Err(error),
    }
}

async fn blocking<T, F>(work: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ApiError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| fault(format!("the portable archive worker stopped: {error}")))?
}

fn io_fault(action: &str, error: impl std::fmt::Display) -> ApiError {
    fault(format!("the portable export could not {action}: {error}"))
}

fn fault(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Internal, message)
}
