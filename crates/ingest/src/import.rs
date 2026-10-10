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
//! again, whatever order the store searches in. A replayed commit keeps the warnings its check
//! returned: storage gives none back on a replay, so the same check runs again over the
//! replayed commit's tree, and the completion does not depend on where an attempt died.
//!
//! An import does not conflict with unrelated edits (SPEC: "Create, move, delete, import ...
//! remain immediate commits"; integration-owner ruling R-I8). Each commit is written on the
//! head as the attempt reads it: the base revision is what the import was accepted against
//! (the card names are chosen there), not a lock on the head. A head that moved between the
//! read and the write is read again. A commit the store refuses with `Conflict` while the head
//! stands still is a real collision with what the import writes (a card path another item took
//! after acceptance): the job fails and is not retried, and what it already committed stays.
//!
//! A card commit follows SPEC's Snapshot rule (fix-2 ruling 2, review N1): it is blocked only
//! when the item it writes was itself changed or deleted since its base, here the import's
//! pending commit. Before writing a card, the card at the current head is compared with what
//! the pending commit wrote. If a person changed or deleted it, that card is not written and
//! not retried, and the completion says which card and why (`CARD_NOT_WRITTEN`): the person's
//! edit is never overwritten. Otherwise the card is written onto the head's version.
//!
//! A commit an earlier attempt wrote is found before its source is converted: a resumed
//! import converts no card it already committed, and a resumed redigest converts nothing. The
//! completion then lists the outputs the committed card's extraction references (its record's
//! window exports and assets, and the record), the same list a first conversion gives, read from
//! the same record, so every retained output a card references stays listed.
//!
//! A redigest writes one commit, under ordinal 0, by the same rule against its base revision
//! (R-I8 extended): replayed when an earlier attempt wrote it, refused (not retried) only when
//! the item was deleted or its original changed since the base, and otherwise written onto the
//! head's version of the card, keeping its path, title, properties and supplied text.
//!
//! A job stops when `update_progress` shows it cancelled, and writes no completion: while the
//! pages are counted and while a window converts (it is called every `HandlerLimits::heartbeat`,
//! which also renews the lease, and dropping the work kills its child), between windows, and
//! between card commits. Cards already committed keep
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

use okf_jawn_contract::common::{PageRange, Warning};
use okf_jawn_contract::conventions::source_card_name;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{
    ConversionOutcome, ConversionSettings, ConverterIdentity, Extraction, SuppliedText, TextOrigin,
};
use okf_jawn_contract::identity::{Digest, ItemId, Revision, UploadId, WorkspacePath};
use okf_jawn_contract::import::JobState;
use okf_jawn_contract::item::ItemDocument;
use okf_jawn_contract::source::{SourceAppearance, SourceName};
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, ConversionRecord, ConvertedDocument, PageCount, RetainedAsset,
};
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
    /// The retained objects the record references, and the record (`outputs_of`).
    outputs: Vec<Digest>,
}

/// How one commit of a job landed.
enum Landed {
    /// Written now, or replayed from an earlier attempt.
    Committed(Committed),
    /// Refused while the head stood still: a real collision with what the job writes.
    Refused(ApiError),
    /// Not written: the item it writes was changed or deleted since its base, by someone else;
    /// why.
    Kept(String),
}

/// What one commit writes, given the head it is written on.
enum Writes<'a> {
    /// These edits, whatever the head holds.
    Edits(Vec<TreeEdit>),
    /// This card, onto the head's version of its item, by `rule` against the item at `since`.
    Card {
        /// The card as the job would write it.
        card: &'a SourceCard,
        /// The item's base: the import's pending commit, or the redigest's base revision.
        since: &'a Revision,
        /// What change since the base blocks the write.
        rule: Blocked,
    },
}

/// What change to a card's item since its base blocks writing the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Blocked {
    /// Any change or deletion (an import's converted card against its pending card).
    AnyChange,
    /// A deletion, or a change of the original the card shows (a redigest).
    OriginalChanged,
}

/// The type name a new source card gets: no SPEC sentence fixes one, so this is the lane's
/// choice, stated in the lane report for the owner to confirm.
pub const SOURCE_CARD_TYPE: &str = "source";

/// The most bytes of one retained output the handler reads back to store it.
pub const MAX_RETAINED_OUTPUT_BYTES: u64 = 1 << 30;

/// The warning code of a converted card an import did not write because a person changed or
/// deleted the pending card meanwhile.
pub const CARD_NOT_WRITTEN: &str = "import_card_not_written";

/// How many entries one folder listing asks for.
const LIST_PAGE: u16 = 500;

/// The file name of a folder's maintained index, a Markdown file of the bundle.
const MAINTAINED_INDEX: &str = "index.md";

/// The workspace's change log at the root, a Markdown file of the bundle.
const CHANGE_LOG: &str = "log.md";

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
        Writes::Edits(
            cards
                .iter()
                .map(|(card, _)| TreeEdit::WriteSourceCard(Box::new(card.clone())))
                .collect(),
        ),
        Arc::clone(&check),
    )
    .await?
    {
        Landed::Committed(committed) => committed,
        Landed::Refused(error) => return Ok(refused(&error)),
        Landed::Kept(why) => return Err(fault(&why)),
    };
    let pending_revision = pending.revision.clone();
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
        // A card an earlier attempt already committed is not converted again: the completion
        // takes its commit and the outputs its committed extraction references.
        if let Some(committed) = replayed(ports, claimed, scope, (base, ordinal), &check).await? {
            take_committed(ports, scope, card.item_id, committed, Vec::new(), &mut done).await?;
        } else {
            let Some(converted) =
                convert_document(ports, claimed, scope, &local, &file_name, request.settings)
                    .await?
            else {
                done.cancelled = true;
                return Ok(done);
            };
            card.body.clone_from(&converted.assembled.markdown);
            card.appearance.extraction = extraction_of(&converted, None);
            let message = format!("Import {}: {}", card.title, outcome_word(&converted));
            let writes = Writes::Card {
                card: &card,
                since: &pending_revision,
                rule: Blocked::AnyChange,
            };
            match commit(
                ports,
                claimed,
                scope,
                (base, ordinal),
                message,
                writes,
                Arc::clone(&check),
            )
            .await?
            {
                Landed::Committed(committed) => {
                    let outputs = converted.outputs;
                    take_committed(ports, scope, card.item_id, committed, outputs, &mut done)
                        .await?;
                }
                Landed::Refused(error) => return Ok(refused(&error)),
                // The person's edit stands; this conversion is not written, and not retried.
                Landed::Kept(why) => done.warnings.push(Warning {
                    code: CARD_NOT_WRITTEN.to_owned(),
                    message: format!("the converted card was not written: {why}"),
                    location: Some(card.path.as_str().to_owned()),
                }),
            }
        }
        done.item_ids.push(card.item_id);
        // Between card commits: a job cancelled now converts no further source.
        if ordinal < total && cancelled_after(ports, claimed, ordinal, total).await? {
            done.cancelled = true;
            return Ok(done);
        }
    }
    Ok(done)
}

/// Take a card's commit into the completion: its revision and warnings, and its outputs. A
/// commit written now lists `converted`, the outputs of the conversion it wrote; a replayed one
/// (an earlier or a concurrent attempt wrote it) lists the outputs its committed extraction
/// references, which are that attempt's.
async fn take_committed(
    ports: &HandlerPorts,
    scope: &StorageScope,
    item_id: ItemId,
    committed: Committed,
    converted: Vec<Digest>,
    done: &mut Done,
) -> Result<(), ApiError> {
    if committed.replayed {
        done.outputs
            .extend(committed_outputs(ports, scope, &committed.revision, item_id).await?);
    } else {
        done.outputs.extend(converted);
    }
    done.revision = Some(committed.revision);
    done.warnings.extend(committed.warnings);
    Ok(())
}

/// Report progress after `done` of `total` cards, and say whether the job is now cancelled.
async fn cancelled_after(
    ports: &HandlerPorts,
    claimed: &ClaimedJob,
    done: u32,
    total: u32,
) -> Result<bool, ApiError> {
    let progress = percent(
        usize::try_from(done).unwrap_or(usize::MAX),
        usize::try_from(total).unwrap_or(usize::MAX),
    );
    let job = ports
        .records
        .update_progress(&claimed.lease, progress)
        .await?;
    Ok(job.state == JobState::Cancelled)
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
    // A redigest an earlier attempt already committed is not converted again.
    if let Some(committed) =
        replayed(ports, claimed, scope, (base_revision, 0), &ports.check).await?
    {
        return Ok(Done {
            outputs: committed_outputs(ports, scope, &committed.revision, item_id).await?,
            revision: Some(committed.revision),
            item_ids: vec![item_id],
            warnings: committed.warnings,
            ..Done::default()
        });
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
    let writes = Writes::Card {
        card: &card,
        since: base_revision,
        rule: Blocked::OriginalChanged,
    };
    let landed = commit(
        ports,
        claimed,
        scope,
        (base_revision, 0),
        format!("Redigest {}: {}", card.title, outcome_word(&converted)),
        writes,
        Arc::clone(&ports.check),
    )
    .await?;
    let refusal = |message: String| Done {
        refused: Some(ApiError::new(ErrorCode::Conflict, message)),
        ..Done::default()
    };
    Ok(match landed {
        Landed::Committed(committed) => Done {
            outputs: if committed.replayed {
                committed_outputs(ports, scope, &committed.revision, item_id).await?
            } else {
                converted.outputs
            },
            revision: Some(committed.revision),
            item_ids: vec![item_id],
            warnings: committed.warnings,
            ..Done::default()
        },
        Landed::Refused(error) => refusal(format!(
            "the redigest collides with what the workspace holds now: {}",
            error.message
        )),
        Landed::Kept(why) => refusal(format!("the redigest was not written: {why}")),
    })
}

/// Convert one source window by window and retain what it produced; `None` when the job was
/// cancelled on the way. A page count that itself ended the document (`PageCount::Failed`) is
/// recorded as the source's whole conversion, and no window is converted.
async fn convert_document(
    ports: &HandlerPorts,
    claimed: &ClaimedJob,
    scope: &StorageScope,
    local: &LocalSource,
    file_name: &str,
    settings: &ConversionSettings,
) -> Result<Option<Converted>, ApiError> {
    let limits = ports.converter.limits();
    // The page count runs in a child too, for up to its own time bound: it renews the lease
    // and stops for a cancellation like a window (review N4).
    let Some(page_count) = with_heartbeat(
        ports,
        claimed,
        0,
        ports.converter.page_count(local, file_name),
    )
    .await?
    else {
        return Ok(None);
    };
    let (page_count, counted) = match page_count {
        PageCount::Pages(pages) if pages > 0 => (Some(pages), None),
        PageCount::Pages(_) | PageCount::Whole => (None, None),
        PageCount::Failed(conversion) => (None, Some(*conversion)),
    };
    if let Some(conversion) = counted {
        let identity = conversion.converter.clone();
        let outcome = window_outcome(ports, scope, None, conversion).await?;
        return assembled(ports, scope, identity, None, vec![outcome])
            .await
            .map(Some);
    }
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
        outcomes.push(window_outcome(ports, scope, window, conversion).await?);
        let job = ports
            .records
            .update_progress(&claimed.lease, percent(index.saturating_add(1), total))
            .await?;
        if job.state == JobState::Cancelled {
            return Ok(None);
        }
    }
    let identity = identity.ok_or_else(|| fault("a conversion ran at least one window"))?;
    assembled(ports, scope, identity, page_count, outcomes)
        .await
        .map(Some)
}

/// The windows of one source assembled into its record, and the record retained.
async fn assembled(
    ports: &HandlerPorts,
    scope: &StorageScope,
    identity: ConverterIdentity,
    page_count: Option<u32>,
    outcomes: Vec<WindowOutcome>,
) -> Result<Converted, ApiError> {
    let assembled = assemble(identity.clone(), page_count, outcomes)?;
    let (record, outputs) = match &assembled.record {
        Some(record) => {
            let bytes = serde_json::to_vec(record).map_err(|error| {
                fault(&format!("a conversion record did not serialize: {error}"))
            })?;
            let digest = retain(ports, scope, bytes).await?;
            (Some(digest.clone()), outputs_of(record, digest))
        }
        None => (None, Vec::new()),
    };
    let converter = (assembled.outcome != ConversionOutcome::Unsupported).then_some(identity);
    Ok(Converted {
        assembled,
        converter,
        page_count,
        record,
        outputs,
    })
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
) -> Result<WindowOutcome, ApiError> {
    let converted = match conversion.document {
        Some(document) => Some(retain_window(ports, scope, document).await?),
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
) -> Result<ConvertedWindow, ApiError> {
    let export = retain(ports, scope, read_output(&document.structured).await?).await?;
    let mut assets = Vec::new();
    for asset in document.assets {
        let digest = retain(ports, scope, read_output(&asset.path).await?).await?;
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

/// The retained outputs of a conversion whose record is retained under `digest`: every window
/// export and asset the record references, then the record. A first conversion and a replay of
/// its commit name the same list, read from the same record.
fn outputs_of(record: &ConversionRecord, digest: Digest) -> Vec<Digest> {
    record
        .structured
        .iter()
        .map(|export| export.digest.clone())
        .chain(record.assets.iter().map(|asset| asset.digest.clone()))
        .chain(std::iter::once(digest))
        .collect()
}

/// The retained outputs a committed card references: those of the conversion record its
/// extraction names at `revision` (`outputs_of`), or none when it names no record.
async fn committed_outputs(
    ports: &HandlerPorts,
    scope: &StorageScope,
    revision: &Revision,
    item_id: ItemId,
) -> Result<Vec<Digest>, ApiError> {
    let document = ports.versions.show(scope, revision, item_id).await?;
    let Some(digest) = document
        .source
        .and_then(|appearance| appearance.extraction.digest)
    else {
        return Ok(Vec::new());
    };
    let local = ports.blobs.materialize(scope, &digest).await?;
    let record: ConversionRecord = serde_json::from_slice(&read_output(&local.path).await?)
        .map_err(|error| fault(&format!("a retained conversion record is not one: {error}")))?;
    Ok(outputs_of(&record, digest))
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

/// The `ordinal`-th commit of a job, on the current head.
///
/// Replayed when an earlier attempt already wrote it after `base` (`find_commit` on the
/// commit's own derived identity). Otherwise written on the head as read now: a card onto the
/// head's version of its item, or `Landed::Kept` when the item changed since its base in a way
/// the card's rule blocks. A head that moved before the write is read again, at most
/// `HEAD_RACES` times. A `Conflict` while the head stood still is the store refusing these
/// edits, a real collision: `Landed::Refused`.
async fn commit(
    ports: &HandlerPorts,
    claimed: &ClaimedJob,
    scope: &StorageScope,
    (base, ordinal): (&Revision, u32),
    message: String,
    writes: Writes<'_>,
    check: Arc<dyn CandidateCheck>,
) -> Result<Landed, ApiError> {
    if let Some(committed) = replayed(ports, claimed, scope, (base, ordinal), &check).await? {
        return Ok(Landed::Committed(committed));
    }
    for _race in 0..HEAD_RACES {
        let head = ports.versions.head(scope).await?;
        let edits = match &writes {
            Writes::Edits(edits) => edits.clone(),
            Writes::Card { card, since, rule } => {
                match onto_head(ports, scope, &head, card, since, *rule).await? {
                    Ok(card) => vec![TreeEdit::WriteSourceCard(Box::new(card))],
                    Err(why) => return Ok(Landed::Kept(why)),
                }
            }
        };
        let written = commit_on(
            ports,
            claimed,
            scope,
            (head.clone(), ordinal),
            message.clone(),
            edits,
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

/// The `ordinal`-th commit of the job when an earlier attempt already wrote it after `base`,
/// with the warnings its check returned then. Storage returns no warnings on a replay, so the
/// same check is run again over the replayed commit's tree (`checked_again`): the completion's
/// warnings do not depend on where an attempt died.
async fn replayed(
    ports: &HandlerPorts,
    claimed: &ClaimedJob,
    scope: &StorageScope,
    (base, ordinal): (&Revision, u32),
    check: &Arc<dyn CandidateCheck>,
) -> Result<Option<Committed>, ApiError> {
    let mutation_id = derive_commit_mutation_id(claimed.mutation_id, ordinal);
    let Some(revision) = ports.versions.find_commit(scope, mutation_id, base).await? else {
        return Ok(None);
    };
    let warnings = checked_again(ports, scope, &revision, Arc::clone(check)).await?;
    Ok(Some(Committed {
        revision,
        replayed: true,
        warnings,
    }))
}

/// Run `check` over the tree of `revision`, materialized from the store in a temporary
/// directory: every item file, each folder's maintained `index.md` and the root `log.md`,
/// which are the Markdown files of the bundle the check reads (storage's side records under
/// `.okf/` are not Markdown and no OKF check reads them). This is the candidate tree the
/// check saw when storage wrote that commit.
async fn checked_again(
    ports: &HandlerPorts,
    scope: &StorageScope,
    revision: &Revision,
    check: Arc<dyn CandidateCheck>,
) -> Result<Vec<Warning>, ApiError> {
    let staging = tempfile::tempdir()
        .map_err(|error| fault(&format!("a staging directory was refused: {error}")))?;
    let mut folders: Vec<Option<WorkspacePath>> = vec![None];
    while let Some(folder) = folders.pop() {
        let mut cursor = None;
        loop {
            let listing = ports
                .versions
                .list(
                    scope,
                    revision,
                    folder.as_ref(),
                    Page {
                        cursor: cursor.take(),
                        limit: LIST_PAGE,
                    },
                )
                .await?;
            for item in &listing.items {
                let bytes = ports
                    .versions
                    .read_file(scope, revision, &item.path)
                    .await?;
                stage(staging.path(), &item.path, &bytes)?;
            }
            folders.extend(listing.folders.into_iter().map(Some));
            match listing.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        let index = match &folder {
            Some(folder) => format!("{}/{MAINTAINED_INDEX}", folder.as_str()),
            None => MAINTAINED_INDEX.to_owned(),
        };
        stage_if_present(ports, scope, revision, staging.path(), index).await?;
    }
    stage_if_present(
        ports,
        scope,
        revision,
        staging.path(),
        CHANGE_LOG.to_owned(),
    )
    .await?;
    tokio::task::spawn_blocking(move || check.check(staging.path()))
        .await
        .map_err(|error| fault(&format!("the replayed commit's check stopped: {error}")))?
}

/// Stage the versioned file at `path` of `revision`, when the tree holds one.
async fn stage_if_present(
    ports: &HandlerPorts,
    scope: &StorageScope,
    revision: &Revision,
    root: &Path,
    path: String,
) -> Result<(), ApiError> {
    let path = WorkspacePath::try_from(path)
        .map_err(|error| fault(&format!("a maintained file has no workspace path: {error}")))?;
    match ports.versions.read_file(scope, revision, &path).await {
        Ok(bytes) => stage(root, &path, &bytes),
        Err(error) if error.code == ErrorCode::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Write one file of a staged tree; small files, written in place (the workspace tokio has no
/// `fs` feature).
fn stage(root: &Path, path: &WorkspacePath, bytes: &[u8]) -> Result<(), ApiError> {
    let file = root.join(path.as_str());
    let unwritable =
        |error: std::io::Error| fault(&format!("{} could not be staged: {error}", path.as_str()));
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder).map_err(unwritable)?;
    }
    std::fs::write(&file, bytes).map_err(unwritable)
}

/// `card` written onto the head's version of its item, or why it must not be written: the item
/// is gone at `head`, or changed since `since` in the way `rule` blocks. The head's path,
/// title, type and properties are kept; a redigest also keeps the head's appearance and its
/// supplied text, with the new extraction.
async fn onto_head(
    ports: &HandlerPorts,
    scope: &StorageScope,
    head: &Revision,
    card: &SourceCard,
    since: &Revision,
    rule: Blocked,
) -> Result<Result<SourceCard, String>, ApiError> {
    let now = match ports.versions.show(scope, head, card.item_id).await {
        Ok(now) => now,
        Err(error) if error.code == ErrorCode::NotFound => {
            return Ok(Err(format!(
                "{} was deleted after the job's base revision",
                card.path.as_str()
            )));
        }
        Err(error) => return Err(error),
    };
    let then = ports.versions.show(scope, since, card.item_id).await?;
    let mut written = card.clone();
    match rule {
        Blocked::AnyChange => {
            if !same_item(&now, &then) {
                return Ok(Err(format!(
                    "{} was changed after the import committed it as pending; the change is kept",
                    then.summary.path.as_str()
                )));
            }
        }
        Blocked::OriginalChanged => {
            let Some(appearance) = now.source.clone() else {
                return Ok(Err(format!(
                    "{} is no longer a source item",
                    now.summary.path.as_str()
                )));
            };
            let original = then.source.as_ref().map(|before| &before.object);
            if original != Some(&appearance.object) {
                return Ok(Err(format!(
                    "the original of {} changed after the job's base revision",
                    now.summary.path.as_str()
                )));
            }
            let supplied = appearance.extraction.supplied.clone();
            written.appearance = SourceAppearance {
                extraction: Extraction {
                    supplied,
                    ..card.appearance.extraction.clone()
                },
                ..appearance
            };
        }
    }
    written.path = now.summary.path.clone();
    written.title.clone_from(&now.summary.title);
    written.type_name.clone_from(&now.summary.type_name);
    written.properties = without_header(&now.properties);
    Ok(Ok(written))
}

/// Whether an item is as it was: the same place, name, type, status, content and appearance.
fn same_item(now: &ItemDocument, then: &ItemDocument) -> bool {
    let appearance = |document: &ItemDocument| {
        document
            .source
            .as_ref()
            .and_then(|appearance| serde_json::to_value(appearance).ok())
    };
    now.summary.path == then.summary.path
        && now.summary.title == then.summary.title
        && now.summary.type_name == then.summary.type_name
        && now.summary.status == then.summary.status
        && now.summary.archived == then.summary.archived
        && now.content_digest == then.content_digest
        && appearance(now) == appearance(then)
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
