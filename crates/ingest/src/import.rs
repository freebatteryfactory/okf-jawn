//! The import and redigest handlers: the windowing loop over the `Converter`, the record
//! assembled and retained, and the source card committed (plan 3.2, Stage 1b design 9.1).
//!
//! Import:
//! 1. Each upload must be complete; it is consumed by this job.
//! 2. Each card is named `conventions::source_card_name` of its file name, with the
//!    numbered-suffix rule (`report-pdf-2.md`) against the destination folder at the base
//!    revision and against the other cards of the job.
//! 3. All cards are committed first with outcome `pending`, through `portable::ImportCheck`
//!    (the injected `CandidateCheck`, then OKF lint of the imported files).
//! 4. Each source is converted window by window (`windows_of(page_count, window_pages)`); each
//!    window's export and assets are retained, the windows are assembled
//!    (`record::assemble`), the record is retained, and the card is replaced with the
//!    converter's text and the contract `Extraction`, in every outcome.
//!
//! A mutation identity names at most one commit. The `n`-th commit of an import (0 for the
//! pending cards, `i + 1` for card `i`, counted the same way on every attempt) carries
//! `derive_commit_mutation_id(job, n)`, and new cards are named with `derive_item_id`. Before
//! it writes, an import commit asks `VersionStore::find_commit` whether an earlier attempt
//! already wrote it after the job's base revision; if so it is replayed and nothing is written
//! again, whatever order the store searches in.
//!
//! An import does not conflict with unrelated edits (SPEC: "Create, move, delete, import ...
//! remain immediate commits"; integration-owner ruling R-I8). Each commit is written on the
//! head as the attempt reads it: the base revision is what the import was accepted against
//! (the card names are chosen there), not a lock on the head. A head that moved between the
//! read and the write is read again. A commit the store refuses with `Conflict` while the head
//! stands still is a real collision with what the import writes (a card path another item took
//! after acceptance): the job fails and is not retried, and what it already committed stays.
//!
//! A redigest writes one commit, under ordinal 0, on its base revision.
//!
//! A job stops when `update_progress` shows it cancelled, and writes no completion: while a
//! window converts (it is called every `HandlerLimits::heartbeat`, and dropping the conversion
//! kills its child), between windows, and between card commits. Cards already committed keep
//! the state they were committed with; a source not yet converted stays `pending`, with no job
//! left to finish it (review N10), until it is redigested.
//!
//! Construction limitations, stated rather than hidden:
//! - `apply_naming_rules: true` fails as `NotImplemented`: core has no function that names a
//!   new card under the workspace rules (request R-I4).
//! - Redigest of only the unconverted pages (`pages: Some`) fails as `NotImplemented`: the
//!   retained record cannot give back the previous text of each window (request R-I5).
//! - The type name of a new source card is `SOURCE_CARD_TYPE`; no SPEC sentence fixes it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use okf_jawn_contract::common::PageRange;
use okf_jawn_contract::conventions::source_card_name;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{
    ConversionOutcome, ConversionSettings, ConverterIdentity, Extraction, SuppliedText, TextOrigin,
};
use okf_jawn_contract::identity::{Digest, ItemId, Revision, UploadId, WorkspacePath};
use okf_jawn_contract::import::JobState;
use okf_jawn_contract::source::{SourceAppearance, SourceName};
use okf_jawn_core::conversion::{Conversion, ConversionInput, ConvertedDocument, RetainedAsset};
use okf_jawn_core::items::without_header;
use okf_jawn_core::jobs::ClaimedJob;
use okf_jawn_core::portable::ImportCheck;
use okf_jawn_core::storage::{
    CandidateCheck, CommitChanges, Committed, LocalSource, Page, SourceCard, StorageScope,
    TreeEdit, derive_commit_mutation_id, derive_item_id,
};

use crate::handler::{Done, HandlerPorts};
use crate::record::{
    Assembled, ConvertedWindow, WindowOutcome, assemble, sha256_digest, windows_of,
};

/// The inputs of an `Import` job, as its specification holds them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ImportRequest<'a> {
    /// Revision the import was accepted against: card names are chosen there, and an earlier
    /// attempt's commits are found after it. Not a lock on the head.
    pub(crate) base_revision: &'a Revision,
    /// Completed upload slots to import.
    pub(crate) upload_ids: &'a [UploadId],
    /// Destination folder; `None` is the workspace root.
    pub(crate) destination: Option<&'a WorkspacePath>,
    /// Apply the workspace naming rules to the new cards.
    pub(crate) apply_naming_rules: bool,
    /// Conversion settings.
    pub(crate) settings: &'a ConversionSettings,
}

/// A source converted and its record retained.
struct Converted {
    /// The assembled outcome, record and joined Markdown.
    assembled: Assembled,
    /// The converter's identity; `None` when it found the format unsupported.
    converter: Option<ConverterIdentity>,
    /// Pages of a paginated original.
    page_count: Option<u32>,
    /// Digest of the retained record, when a document was produced.
    record: Option<Digest>,
    /// Every object retained on the way.
    outputs: Vec<Digest>,
}

/// How one commit of an import landed.
enum Landed {
    /// Written now, or replayed from an earlier attempt.
    Committed(Committed),
    /// Refused while the head stood still: a real collision with what the job writes.
    Refused(ApiError),
}

/// The type name a new source card gets: no SPEC sentence fixes one, so this is the lane's
/// choice, stated in the lane report for the owner to confirm.
pub const SOURCE_CARD_TYPE: &str = "source";

/// The most bytes of one retained output the handler reads back to store it.
pub const MAX_RETAINED_OUTPUT_BYTES: u64 = 1 << 30;

/// How many entries one folder listing asks for.
const LIST_PAGE: u16 = 500;

/// How many times one import commit reads the head again after it moved under the write,
/// before the attempt gives up as retryable.
const HEAD_RACES: u32 = 8;

/// Run an `Import` job.
pub(crate) async fn import(
    ports: &HandlerPorts,
    claimed: &ClaimedJob,
    scope: &StorageScope,
    request: ImportRequest<'_>,
) -> Result<Done, ApiError> {
    if request.apply_naming_rules {
        return Err(ApiError::new(
            ErrorCode::NotImplemented,
            "naming rules at import wait for core's rule function (request R-I4); import without them",
        ));
    }
    let cards = pending_cards(ports, claimed, scope, &request).await?;
    let check: Arc<dyn CandidateCheck> = Arc::new(ImportCheck::new(
        Arc::clone(&ports.check),
        cards.iter().map(|(card, _)| card.path.clone()).collect(),
    ));
    let base = request.base_revision;
    let pending = match commit(
        ports,
        claimed,
        scope,
        (base, 0),
        format!("Import {} sources, pending conversion", cards.len()),
        cards
            .iter()
            .map(|(card, _)| TreeEdit::WriteSourceCard(Box::new(card.clone())))
            .collect(),
        Arc::clone(&check),
    )
    .await?
    {
        Landed::Committed(committed) => committed,
        Landed::Refused(error) => return Ok(refused(&error)),
    };
    let mut done = Done {
        revision: Some(pending.revision),
        warnings: pending.warnings,
        ..Done::default()
    };
    let total = u32::try_from(cards.len()).unwrap_or(u32::MAX);
    for ((mut card, local), ordinal) in cards.into_iter().zip(1_u32..) {
        let file_name = card
            .appearance
            .names
            .first()
            .map(|name| name.filename.clone())
            .unwrap_or_default();
        let Some(converted) =
            convert_document(ports, claimed, scope, &local, &file_name, request.settings).await?
        else {
            done.cancelled = true;
            return Ok(done);
        };
        card.body.clone_from(&converted.assembled.markdown);
        card.appearance.extraction = extraction_of(&converted, None);
        let message = format!("Import {}: {}", card.title, outcome_word(&converted));
        let committed = match commit(
            ports,
            claimed,
            scope,
            (base, ordinal),
            message,
            vec![TreeEdit::WriteSourceCard(Box::new(card.clone()))],
            Arc::clone(&check),
        )
        .await?
        {
            Landed::Committed(committed) => committed,
            Landed::Refused(error) => return Ok(refused(&error)),
        };
        done.revision = Some(committed.revision);
        done.warnings.extend(committed.warnings);
        done.outputs.extend(converted.outputs);
        done.item_ids.push(card.item_id);
        // Between card commits: a job cancelled now converts no further source.
        if ordinal < total {
            let progress = percent(
                usize::try_from(ordinal).unwrap_or(usize::MAX),
                usize::try_from(total).unwrap_or(usize::MAX),
            );
            let job = ports
                .records
                .update_progress(&claimed.lease, progress)
                .await?;
            if job.state == JobState::Cancelled {
                done.cancelled = true;
                return Ok(done);
            }
        }
    }
    Ok(done)
}

/// A job refused by a real collision: it fails and is not retried.
fn refused(error: &ApiError) -> Done {
    Done {
        refused: Some(ApiError::new(
            ErrorCode::Conflict,
            format!(
                "the import collides with what the workspace holds now: {}",
                error.message
            ),
        )),
        ..Done::default()
    }
}

/// The cards of an import, each with outcome `pending`, beside its materialized original. Each
/// upload must be complete and is consumed by this job.
async fn pending_cards(
    ports: &HandlerPorts,
    claimed: &ClaimedJob,
    scope: &StorageScope,
    request: &ImportRequest<'_>,
) -> Result<Vec<(SourceCard, LocalSource)>, ApiError> {
    let mut sources = Vec::new();
    for upload_id in request.upload_ids {
        let upload = ports.uploads.get(scope, *upload_id).await?;
        let object = upload.object.clone().ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidInput,
                "an upload to import is not complete",
            )
            .with_field("/upload_ids")
        })?;
        let _consumed = ports
            .uploads
            .consume(scope, *upload_id, claimed.lease.job_id)
            .await?;
        sources.push((upload, object));
    }
    let mut taken = taken_keys(ports, scope, request.base_revision, request.destination).await?;
    let mut cards = Vec::new();
    for (ordinal, (upload, object)) in (0_u32..).zip(&sources) {
        let local = ports.blobs.materialize(scope, &object.digest).await?;
        cards.push((
            SourceCard {
                item_id: derive_item_id(claimed.mutation_id, ordinal),
                path: free_path(request.destination, &upload.filename, &mut taken)?,
                title: upload.filename.clone(),
                type_name: SOURCE_CARD_TYPE.to_owned(),
                body: String::new(),
                properties: BTreeMap::new(),
                appearance: SourceAppearance {
                    object: object.digest.clone(),
                    names: vec![SourceName {
                        filename: upload.filename.clone(),
                        folder: upload.relative_path.clone(),
                        observed_at: upload.created_at.clone(),
                        supplied_by: upload.supplied_by.subject.clone(),
                    }],
                    media_type: media_type(&local.path, &upload.filename),
                    size: object.size.to_string(),
                    metadata: BTreeMap::new(),
                    parent_item_id: None,
                    supersedes: None,
                    extraction: pending(),
                },
            },
            local,
        ));
    }
    Ok(cards)
}

/// Run a `Redigest` job: convert the source again and replace its card.
pub(crate) async fn redigest(
    ports: &HandlerPorts,
    claimed: &ClaimedJob,
    scope: &StorageScope,
    item_id: ItemId,
    base_revision: &Revision,
    settings: &ConversionSettings,
    pages: Option<&Vec<PageRange>>,
) -> Result<Done, ApiError> {
    if pages.is_some() {
        return Err(ApiError::new(
            ErrorCode::NotImplemented,
            "a redigest of only the unconverted pages waits for the record to keep each window's text (request R-I5); redigest the whole source",
        ));
    }
    let document = ports.versions.show(scope, base_revision, item_id).await?;
    let appearance = document.source.clone().ok_or_else(|| {
        ApiError::new(
            ErrorCode::InvalidInput,
            "only a source item can be redigested",
        )
        .with_field("/item_id")
    })?;
    let file_name = appearance
        .names
        .first()
        .map(|name| name.filename.clone())
        .ok_or_else(|| fault("a source card records the name it was observed under"))?;
    let local = ports.blobs.materialize(scope, &appearance.object).await?;
    let Some(converted) =
        convert_document(ports, claimed, scope, &local, &file_name, settings).await?
    else {
        return Ok(Done {
            cancelled: true,
            ..Done::default()
        });
    };
    let previous_supplied = appearance.extraction.supplied.clone();
    let card = SourceCard {
        item_id,
        path: document.summary.path.clone(),
        title: document.summary.title.clone(),
        type_name: document.summary.type_name.clone(),
        body: converted.assembled.markdown.clone(),
        properties: without_header(&document.properties),
        appearance: SourceAppearance {
            extraction: extraction_of(&converted, previous_supplied),
            ..appearance
        },
    };
    let committed = commit_on(
        ports,
        claimed,
        scope,
        (base_revision.clone(), 0),
        format!("Redigest {}: {}", card.title, outcome_word(&converted)),
        vec![TreeEdit::WriteSourceCard(Box::new(card))],
        Arc::clone(&ports.check),
    )
    .await?;
    Ok(Done {
        revision: Some(committed.revision),
        item_ids: vec![item_id],
        outputs: converted.outputs,
        warnings: committed.warnings,
        ..Done::default()
    })
}

/// Convert one source window by window and retain what it produced; `None` when the job was
/// cancelled on the way.
async fn convert_document(
    ports: &HandlerPorts,
    claimed: &ClaimedJob,
    scope: &StorageScope,
    local: &LocalSource,
    file_name: &str,
    settings: &ConversionSettings,
) -> Result<Option<Converted>, ApiError> {
    let limits = ports.converter.limits();
    let page_count = ports
        .converter
        .page_count(local, file_name)
        .await?
        .filter(|pages| *pages > 0);
    let windows: Vec<Option<PageRange>> = match page_count {
        Some(pages) => windows_of(pages, limits.window_pages)
            .into_iter()
            .map(Some)
            .collect(),
        None => vec![None],
    };
    let total = windows.len();
    let mut outcomes = Vec::new();
    let mut identity: Option<ConverterIdentity> = None;
    let mut outputs = Vec::new();
    for (index, window) in windows.into_iter().enumerate() {
        let directory = tempfile::tempdir()
            .map_err(|error| fault(&format!("a window directory was refused: {error}")))?;
        let input = ConversionInput {
            source: local.clone(),
            file_name: file_name.to_owned(),
            settings: settings.clone(),
            window: window.clone(),
            timeout: ports.limits.window_timeout,
            output_directory: directory.path().to_path_buf(),
        };
        let progress = percent(index, total);
        let Some(conversion) =
            with_heartbeat(ports, claimed, progress, ports.converter.convert(input)).await?
        else {
            return Ok(None);
        };
        if identity.is_none() {
            identity = Some(conversion.converter.clone());
        }
        outcomes.push(window_outcome(ports, scope, window, conversion, &mut outputs).await?);
        let job = ports
            .records
            .update_progress(&claimed.lease, percent(index.saturating_add(1), total))
            .await?;
        if job.state == JobState::Cancelled {
            return Ok(None);
        }
    }
    let identity = identity.ok_or_else(|| fault("a conversion ran at least one window"))?;
    let assembled = assemble(identity.clone(), page_count, outcomes)?;
    let record = match &assembled.record {
        Some(record) => {
            let bytes = serde_json::to_vec(record).map_err(|error| {
                fault(&format!("a conversion record did not serialize: {error}"))
            })?;
            let digest = retain(ports, scope, bytes).await?;
            outputs.push(digest.clone());
            Some(digest)
        }
        None => None,
    };
    let converter = (assembled.outcome != ConversionOutcome::Unsupported).then_some(identity);
    Ok(Some(Converted {
        assembled,
        converter,
        page_count,
        record,
        outputs,
    }))
}

/// Run `work` while renewing the lease every `heartbeat`; `None` when the job was found
/// cancelled, in which case `work` is dropped.
async fn with_heartbeat<T>(
    ports: &HandlerPorts,
    claimed: &ClaimedJob,
    progress: u8,
    work: impl Future<Output = Result<T, ApiError>>,
) -> Result<Option<T>, ApiError> {
    let mut work = std::pin::pin!(work);
    let period = ports.limits.heartbeat.max(Duration::from_millis(1));
    let now = tokio::time::Instant::now();
    let mut ticks = tokio::time::interval_at(now.checked_add(period).unwrap_or(now), period);
    loop {
        tokio::select! {
            result = &mut work => return result.map(Some),
            _instant = ticks.tick() => {
                let job = ports.records.update_progress(&claimed.lease, progress).await?;
                if job.state == JobState::Cancelled {
                    return Ok(None);
                }
            }
        }
    }
}

/// One window's conversion with its export and assets retained.
async fn window_outcome(
    ports: &HandlerPorts,
    scope: &StorageScope,
    window: Option<PageRange>,
    conversion: Conversion,
    outputs: &mut Vec<Digest>,
) -> Result<WindowOutcome, ApiError> {
    let converted = match conversion.document {
        Some(document) => Some(retain_window(ports, scope, document, outputs).await?),
        None => None,
    };
    Ok(WindowOutcome {
        window,
        status: conversion.status,
        coverage: conversion.coverage,
        issues: conversion.issues,
        converted,
    })
}

/// Retain a window's export and assets and keep the rest of its document.
async fn retain_window(
    ports: &HandlerPorts,
    scope: &StorageScope,
    document: ConvertedDocument,
    outputs: &mut Vec<Digest>,
) -> Result<ConvertedWindow, ApiError> {
    let export = retain(ports, scope, read_output(&document.structured).await?).await?;
    outputs.push(export.clone());
    let mut assets = Vec::new();
    for asset in document.assets {
        let digest = retain(ports, scope, read_output(&asset.path).await?).await?;
        outputs.push(digest.clone());
        assets.push(RetainedAsset {
            digest,
            media_type: asset.media_type,
            role: asset.role,
            location: asset.location,
            pixel_size: asset.pixel_size,
            caption: asset.caption,
        });
    }
    Ok(ConvertedWindow {
        markdown: document.markdown,
        export,
        locations: document.locations,
        tables: document.tables,
        assets,
        warnings: document.warnings,
    })
}

/// Store bytes in the blob store under their digest.
async fn retain(
    ports: &HandlerPorts,
    scope: &StorageScope,
    bytes: Vec<u8>,
) -> Result<Digest, ApiError> {
    let digest = sha256_digest(&bytes)?;
    let limit = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let object = ports
        .blobs
        .put(
            scope,
            Box::pin(std::io::Cursor::new(bytes)),
            limit,
            Some(digest),
        )
        .await?;
    Ok(object.digest)
}

/// The `ordinal`-th commit of an import, on the current head.
///
/// Replayed when an earlier attempt already wrote it after `base` (`find_commit` on the
/// commit's own derived identity). Otherwise written on the head as read now; a head that moved
/// before the write is read again, at most `HEAD_RACES` times. A `Conflict` while the head
/// stood still is the store refusing these edits, a real collision: `Landed::Refused`.
async fn commit(
    ports: &HandlerPorts,
    claimed: &ClaimedJob,
    scope: &StorageScope,
    (base, ordinal): (&Revision, u32),
    message: String,
    edits: Vec<TreeEdit>,
    check: Arc<dyn CandidateCheck>,
) -> Result<Landed, ApiError> {
    let mutation_id = derive_commit_mutation_id(claimed.mutation_id, ordinal);
    if let Some(revision) = ports.versions.find_commit(scope, mutation_id, base).await? {
        return Ok(Landed::Committed(Committed {
            revision,
            replayed: true,
            warnings: Vec::new(),
        }));
    }
    for _race in 0..HEAD_RACES {
        let head = ports.versions.head(scope).await?;
        let written = commit_on(
            ports,
            claimed,
            scope,
            (head.clone(), ordinal),
            message.clone(),
            edits.clone(),
            Arc::clone(&check),
        )
        .await;
        match written {
            Ok(committed) => return Ok(Landed::Committed(committed)),
            Err(error) if error.code == ErrorCode::Conflict => {
                if ports.versions.head(scope).await? == head {
                    return Ok(Landed::Refused(error));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Err(ApiError::new(
        ErrorCode::Conflict,
        "the workspace head kept moving under the import's commit; the job is retried",
    ))
}

/// The `ordinal`-th commit of the job, expecting `expected_head`, under its derived identity.
async fn commit_on(
    ports: &HandlerPorts,
    claimed: &ClaimedJob,
    scope: &StorageScope,
    (expected_head, ordinal): (Revision, u32),
    message: String,
    edits: Vec<TreeEdit>,
    check: Arc<dyn CandidateCheck>,
) -> Result<Committed, ApiError> {
    ports
        .versions
        .commit(
            scope,
            CommitChanges {
                mutation_id: derive_commit_mutation_id(claimed.mutation_id, ordinal),
                expected_head,
                author: claimed.initiator.clone(),
                message,
                edits,
            },
            check,
        )
        .await
}

/// The collision keys of every item and folder in the destination at the base revision.
async fn taken_keys(
    ports: &HandlerPorts,
    scope: &StorageScope,
    revision: &Revision,
    destination: Option<&WorkspacePath>,
) -> Result<BTreeSet<String>, ApiError> {
    let mut taken = BTreeSet::new();
    let mut cursor = None;
    loop {
        let listing = ports
            .versions
            .list(
                scope,
                revision,
                destination,
                Page {
                    cursor: cursor.take(),
                    limit: LIST_PAGE,
                },
            )
            .await?;
        taken.extend(listing.items.iter().map(|item| item.path.collision_key()));
        taken.extend(listing.folders.iter().map(WorkspacePath::collision_key));
        match listing.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(taken),
        }
    }
}

/// The card path for `file_name` in `destination`, with the numbered-suffix rule against the
/// keys already taken; the chosen key is taken too.
fn free_path(
    destination: Option<&WorkspacePath>,
    file_name: &str,
    taken: &mut BTreeSet<String>,
) -> Result<WorkspacePath, ApiError> {
    let name = source_card_name(file_name);
    let stem = name.strip_suffix(".md").unwrap_or(&name).to_owned();
    for number in 1_u32.. {
        let candidate = if number == 1 {
            name.clone()
        } else {
            format!("{stem}-{number}.md")
        };
        let full = match destination {
            Some(folder) => format!("{}/{candidate}", folder.as_str()),
            None => candidate,
        };
        let path = WorkspacePath::from_supplied(&full).map_err(|error| {
            ApiError::new(
                ErrorCode::InvalidInput,
                format!("the file name {file_name:?} gives no workspace path: {error}"),
            )
        })?;
        if taken.insert(path.collision_key()) {
            return Ok(path);
        }
    }
    Err(fault("no numbered name was free"))
}

/// The media type of an occurrence, detected from its bytes, else from its extension.
fn media_type(path: &Path, file_name: &str) -> String {
    if let Ok(Some(kind)) = infer::get_from_path(path) {
        return kind.mime_type().to_owned();
    }
    let extension = Path::new(file_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("md" | "markdown") => "text/markdown",
        Some("txt") => "text/plain",
        Some("csv") => "text/csv",
        Some("html" | "htm") => "text/html",
        Some("json") => "application/json",
        Some(_) | None => "application/octet-stream",
    }
    .to_owned()
}

/// The extraction a card shows before its conversion.
fn pending() -> Extraction {
    Extraction {
        outcome: ConversionOutcome::Pending,
        converter: None,
        digest: None,
        page_count: None,
        text_origin: TextOrigin::None,
        corrected: false,
        supplied: None,
        warnings: Vec::new(),
    }
}

/// The extraction a card shows after its conversion. A new digest has no correction; an
/// agent's earlier supplied text stays in history and the converter's text is shown.
fn extraction_of(converted: &Converted, supplied: Option<SuppliedText>) -> Extraction {
    let record = converted.assembled.record.as_ref();
    Extraction {
        outcome: converted.assembled.outcome.clone(),
        converter: converted.converter.clone(),
        digest: converted.record.clone(),
        page_count: converted.page_count,
        text_origin: if record.is_some() {
            TextOrigin::Converter
        } else {
            TextOrigin::None
        },
        corrected: false,
        supplied,
        warnings: record
            .map(|record| record.warnings.clone())
            .unwrap_or_default(),
    }
}

/// A word for the commit message.
fn outcome_word(converted: &Converted) -> &'static str {
    match converted.assembled.outcome {
        ConversionOutcome::Pending => "pending",
        ConversionOutcome::Completed => "converted",
        ConversionOutcome::Partial { .. } => "partly converted",
        ConversionOutcome::Failed { .. } => "not converted",
        ConversionOutcome::Unsupported => "unsupported",
    }
}

/// Progress after `done` of `total` windows, 0 to 99; completion writes the rest.
fn percent(done: usize, total: usize) -> u8 {
    let total = total.max(1);
    u8::try_from(done.saturating_mul(99).checked_div(total).unwrap_or(0)).unwrap_or(99)
}

/// Read back one output the converter wrote, on a blocking thread (the workspace tokio has no
/// `fs` feature).
async fn read_output(path: &Path) -> Result<Vec<u8>, ApiError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let unreadable = |error: std::io::Error| {
            fault(&format!("{} could not be read: {error}", path.display()))
        };
        let size = std::fs::metadata(&path).map_err(unreadable)?.len();
        if size > MAX_RETAINED_OUTPUT_BYTES {
            return Err(fault(&format!(
                "{} is larger than a retained output may be",
                path.display()
            )));
        }
        std::fs::read(&path).map_err(unreadable)
    })
    .await
    .map_err(|error| fault(&format!("reading an output stopped: {error}")))?
}

/// A worker fault.
fn fault(message: &str) -> ApiError {
    ApiError::new(ErrorCode::Internal, message)
}
