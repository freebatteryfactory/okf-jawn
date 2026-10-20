//! The import and redigest handlers: the windowing loop over the `Converter`, the record
//! assembled and retained, and the source card committed (plan 3.2, Stage 1b design 9.1).
//!
//! Import:
//! 1. Each upload must be complete and taken by no other job, and its file name must give a
//!    card path; only once every upload of the request passes are they consumed by this job, as
//!    one batch the store takes whole or not at all (`UploadStore::consume_batch`), so a
//!    refused batch consumes none. An upload another job took refuses the job for good.
//! 2. Each card is named `conventions::source_card_name` of its file name, with the
//!    numbered-suffix rule (`conventions::numbered_source_card_name`: `report-pdf-2.md`, the
//!    number after the extension marker) against the destination folder at the base revision
//!    and against the other cards of the job. Both fit one path segment: the stem is
//!    shortened and the tail kept whole, so a long name is never refused for its length.
//!    With `apply_naming_rules`, each card is named by the workspace naming rules saved at the
//!    base revision instead, through core's one naming function
//!    (`okf_jawn_core::conventions::name_new_item`, which `preview_names` names through too),
//!    against every item and folder of the whole tree at the base (a destination can name any
//!    folder) and the job's own cards; a collision never fails the import (a `suffix` rule
//!    numbers the ruled name, any other falls back to the name without rules). No rules saved:
//!    the cards are named without them and the completion says so (`NAMING_RULES_ABSENT`).
//!    Rules that cannot be read or applied fail the job for good, `InvalidInput` on
//!    `/apply_naming_rules`, before any upload is consumed. The naming warnings join the
//!    completion's, located at the card's path.
//! 3. All cards are committed first with outcome `pending`, through `portable::ImportCheck`
//!    (the injected `CandidateCheck`, then OKF lint of the imported files) over every card's
//!    path. A commit's check reports what that commit wrote: each card's own commit (step 4)
//!    is checked over that card's path only, so an import of N cards completes with each
//!    commit's findings once (at most N from the pending commit and one card's from each card
//!    commit, for one finding a card), and no commit's kept warnings hold more than the
//!    pending commit's.
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
//! returned: the store kept them when the commit was written and gives them back with the
//! commit it finds (R-I10), so the completion does not depend on where an attempt died, and a
//! replay runs no check.
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
//! head's version of the card, keeping its path, title, properties and supplied text. Its check
//! is a card commit's (`card_check`): it reports the card it writes, at the head's path, not
//! the whole workspace.
//!
//! Every commit the job lands, written now or replayed (the pending commit, each card's commit,
//! the redigest's commit), is indexed and announced as `changed` under its own derived
//! mutation identity (`handler::announce_commit`), naming the card's item when it writes one.
//! An import that completes having produced items appends `imported` once, under the job's
//! mutation, with the last revision. Progress goes through `Attempt::report`, which announces
//! a value that changed.
//!
//! A job stops when `update_progress` shows it cancelled, and writes no completion: while the
//! pages are counted and while a window converts (it is called every `HandlerLimits::heartbeat`,
//! which also renews the lease, and dropping the work kills its child), between windows, and
//! between card commits. Cards already committed keep
//! the state they were committed with; a source not yet converted stays `pending`, with no job
//! left to finish it (review N10), until it is redigested.
//!
//! Construction limitations, stated rather than hidden:
//! - Redigest of only the unconverted pages (`pages: Some`) fails as `NotImplemented`: the
//!   retained record cannot give back the previous text of each window (request R-I5).
//! - The type name of a new source card is `SOURCE_CARD_TYPE`; no SPEC sentence fixes it.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use okf_jawn_contract::common::{PageRange, Warning};
use okf_jawn_contract::conventions::{NamingRules, source_card_name};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::extraction::{
    ConversionOutcome, ConversionSettings, ConverterIdentity, Extraction, SuppliedText, TextOrigin,
};
use okf_jawn_contract::identity::{Digest, ItemId, Revision, UploadId, WorkspacePath};
use okf_jawn_contract::import::JobState;
use okf_jawn_contract::item::ItemDocument;
use okf_jawn_contract::source::{SourceAppearance, SourceName};
use okf_jawn_core::conventions::{CompiledRules, Subject, TakenPaths, name_new_item};
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, ConversionRecord, ConvertedDocument, PageCount, RetainedAsset,
};
use okf_jawn_core::events::EventScope;
use okf_jawn_core::items::without_header;
use okf_jawn_core::jobs::ClaimedJob;
use okf_jawn_core::portable::ImportCheck;
use okf_jawn_core::storage::{
    CandidateCheck, CommitChanges, Committed, LocalSource, SourceCard, StorageScope, TreeEdit,
    derive_commit_mutation_id, derive_item_id,
};

use crate::handler::{Attempt, Done, HandlerPorts, Renewed, announce_commit, job_event, renewing};
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

/// What one commit writes, given the head it is written on, and so what its check reports.
enum Writes<'a> {
    /// These edits, whatever the head holds, checked by this check.
    Edits(Vec<TreeEdit>, Arc<dyn CandidateCheck>),
    /// This card, onto the head's version of its item, by `rule` against the item at `since`;
    /// checked over the card as written there (`card_check`).
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

/// The completion warning of an import that applies the naming rules when none are saved at its
/// base: the cards are named without them.
pub const NAMING_RULES_ABSENT: &str = "naming_rules_absent";

/// How many times one import commit reads the head again after it moved under the write,
/// before the attempt gives up as retryable.
const HEAD_RACES: u32 = 8;

/// Run an `Import` job.
pub(crate) async fn import(
    ports: &HandlerPorts,
    attempt: &Attempt<'_>,
    scope: &StorageScope,
    request: ImportRequest<'_>,
) -> Result<Done, ApiError> {
    // Conflict from the batch check (an upload another job took, or took meanwhile, or a purge
    // that names the workspace) refuses the job for good, like a collision: retrying it meets
    // the same refusal.
    let (cards, naming) = match pending_cards(ports, attempt.claimed, scope, &request).await {
        Ok(named) => named,
        Err(error) if error.code == ErrorCode::Conflict => return Ok(refused_as(error)),
        Err(error) => return Err(error),
    };
    // A commit's check reports what that commit wrote (review N2): the pending commit writes
    // every card, so its check is over every card's path; each card's own commit is checked
    // over that card's path only (`card_check`).
    let check: Arc<dyn CandidateCheck> = Arc::new(ImportCheck::new(
        Arc::clone(&ports.check),
        cards.iter().map(|(card, _)| card.path.clone()).collect(),
    ));
    let base = request.base_revision;
    let pending = match commit(
        ports,
        attempt,
        scope,
        (base, 0),
        format!("Import {} sources, pending conversion", cards.len()),
        Writes::Edits(
            cards
                .iter()
                .map(|(card, _)| TreeEdit::WriteSourceCard(Box::new(card.clone())))
                .collect(),
            check,
        ),
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
        warnings: naming,
        ..Done::default()
    };
    done.warnings.extend(pending.warnings);
    let total = u32::try_from(cards.len()).unwrap_or(u32::MAX);
    for ((mut card, local), ordinal) in cards.into_iter().zip(1_u32..) {
        let file_name = observed_name(&card);
        // A card an earlier attempt already committed is not converted again: the completion
        // takes its commit and the outputs its committed extraction references.
        let item = Some(card.item_id);
        let replay = replayed(ports, attempt, scope, (base, ordinal), item);
        if let Some(committed) = replay.await? {
            let resumed = (card.item_id, committed, Vec::new());
            take_committed(ports, attempt, scope, resumed, &mut done).await?;
        } else {
            let Some(converted) =
                convert_document(ports, attempt, scope, &local, &file_name, request.settings)
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
            match commit(ports, attempt, scope, (base, ordinal), message, writes).await? {
                Landed::Committed(committed) => {
                    let landed = (card.item_id, committed, converted.outputs);
                    take_committed(ports, attempt, scope, landed, &mut done).await?;
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
        if ordinal < total && cancelled_after(ports, attempt, ordinal, total).await? {
            done.cancelled = true;
            return Ok(done);
        }
    }
    announce_imported(ports, attempt.claimed, scope, &done).await?;
    Ok(done)
}

/// The check of a commit that writes `card` alone, an import's converted card or a redigest's:
/// the injected conformance check and OKF lint, reported for that card's file only
/// (`portable::ImportCheck`). `card` is the card as written onto the head, so its path is the
/// one the commit writes: an import's card at the path its pending commit gave it (a card a
/// person moved meanwhile is not written at all, `Blocked::AnyChange`), a redigest's at the
/// head's path of the item. Such a commit also writes the card's appearance record under
/// `.okf/`, the regenerated folder indexes and the log, none of them a concept, so the card is
/// all it is told. The workspace's other files are files of the candidate too, but this commit
/// does not write them: an import of N cards completes with each commit's findings once, not
/// with all N cards' on each of its N + 1 commits, and a redigest with the findings of its card,
/// not of the whole workspace.
fn card_check(ports: &HandlerPorts, card: &SourceCard) -> Arc<dyn CandidateCheck> {
    Arc::new(ImportCheck::new(
        Arc::clone(&ports.check),
        vec![card.path.clone()],
    ))
}

/// Append `imported` once, under the job's own mutation, with the last revision, when the
/// import produced items; a replay appends no second one.
async fn announce_imported(
    ports: &HandlerPorts,
    claimed: &ClaimedJob,
    scope: &StorageScope,
    done: &Done,
) -> Result<(), ApiError> {
    if done.item_ids.is_empty() {
        return Ok(());
    }
    let _event = ports
        .events
        .append(
            &EventScope::Workspace(scope.clone()),
            Some(claimed.mutation_id),
            job_event(EventKind::Imported, claimed, done.revision.clone(), None),
        )
        .await?;
    Ok(())
}

/// The file name a new card's original was observed under.
fn observed_name(card: &SourceCard) -> String {
    card.appearance
        .names
        .first()
        .map(|name| name.filename.clone())
        .unwrap_or_default()
}

/// Take a card's commit into the completion: its revision and warnings, and its outputs. A
/// commit written now lists `converted`, the outputs of the conversion it wrote; a replayed one
/// (an earlier or a concurrent attempt wrote it) lists the outputs its committed extraction
/// references, which are that attempt's.
async fn take_committed(
    ports: &HandlerPorts,
    attempt: &Attempt<'_>,
    scope: &StorageScope,
    (item_id, committed, converted): (ItemId, Committed, Vec<Digest>),
    done: &mut Done,
) -> Result<(), ApiError> {
    if committed.replayed {
        let outputs = committed_outputs(ports, attempt, scope, &committed.revision, item_id);
        done.outputs.extend(outputs.await?);
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
    attempt: &Attempt<'_>,
    done: u32,
    total: u32,
) -> Result<bool, ApiError> {
    let progress = percent(
        usize::try_from(done).unwrap_or(usize::MAX),
        usize::try_from(total).unwrap_or(usize::MAX),
    );
    let job = attempt.report(ports, progress).await?;
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

/// A job refused for good with `error` as it stands: its failure is written as not retryable.
fn refused_as(error: ApiError) -> Done {
    Done {
        refused: Some(error),
        ..Done::default()
    }
}

/// The cards of an import, each with outcome `pending`, beside its materialized original.
///
/// Every upload of the request is read and checked, and every card path derived, before any
/// upload is consumed. An incomplete upload refuses the batch (`InvalidInput`), so does one
/// another job consumed (`Conflict`), and so does a file name that gives no workspace path
/// (`InvalidInput`); each refusal names its upload as `/upload_ids/<index>`. Then no upload of
/// the batch is consumed, so another job may still take the good ones. Only then are they
/// consumed by this job, all in one call (`UploadStore::consume_batch`, one transaction: all of
/// them or none). Another job that consumed one of them between this check and that call is a
/// concurrent import of the same upload: the store refuses the whole batch with `Conflict` and
/// marks none, so two imports that share uploads never hold some of them each (R-I13).
///
/// The card paths are chosen before anything is consumed, with the naming rules when the
/// request applies them (`ruled_paths`), and answered beside the cards with the warnings naming
/// them gave.
async fn pending_cards(
    ports: &HandlerPorts,
    claimed: &ClaimedJob,
    scope: &StorageScope,
    request: &ImportRequest<'_>,
) -> Result<(Vec<(SourceCard, LocalSource)>, Vec<Warning>), ApiError> {
    let job = claimed.lease.job_id;
    let mut sources = Vec::new();
    for (index, upload_id) in request.upload_ids.iter().enumerate() {
        let field = format!("/upload_ids/{index}");
        let upload = ports.uploads.get(scope, *upload_id).await?;
        let object = upload.object.clone().ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidInput,
                "an upload to import is not complete",
            )
            .with_field(field.clone())
        })?;
        if let Some(kind) = ports
            .backups
            .managed_kind(&scope.tenant_id, &object.digest)
            .await?
        {
            use okf_jawn_core::storage::ManagedKind;
            let label = match kind {
                ManagedKind::WorkspaceArchive => "workspace archive",
                ManagedKind::InstallationArchive => "installation archive",
                ManagedKind::WrittenHistory => "written history",
            };
            return Err(ApiError::new(
                ErrorCode::InvalidInput,
                format!("upload {index} is a {label} this installation wrote; restore it instead of importing it"),
            ).with_field(field));
        }
        if upload.consumed_by.is_some_and(|consumer| consumer != job) {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "an upload to import was already taken by another job",
            )
            .with_field(field));
        }
        sources.push((upload, object));
    }
    let names: Vec<&str> = sources
        .iter()
        .map(|(upload, _object)| upload.filename.as_str())
        .collect();
    let (paths, naming) = if request.apply_naming_rules {
        ruled_paths(ports, scope, request, &names).await?
    } else {
        (
            unruled_paths(ports, scope, request, &names).await?,
            Vec::new(),
        )
    };
    let batch: Vec<UploadId> = sources.iter().map(|(upload, _object)| upload.id).collect();
    let _consumed = ports.uploads.consume_batch(scope, &batch, job).await?;
    let mut cards = Vec::new();
    for ((ordinal, (upload, object)), path) in (0_u32..).zip(&sources).zip(paths) {
        let local = ports.blobs.materialize(scope, &object.digest).await?;
        cards.push((
            SourceCard {
                item_id: derive_item_id(claimed.mutation_id, ordinal),
                path,
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
    Ok((cards, naming))
}

/// Name cards without saved rules using the same full-tree occupancy checks as ruled imports.
/// An item cannot become a destination folder, even when rules are disabled or absent.
async fn unruled_paths(
    ports: &HandlerPorts,
    scope: &StorageScope,
    request: &ImportRequest<'_>,
    names: &[&str],
) -> Result<Vec<WorkspacePath>, ApiError> {
    let rules = CompiledRules::compile(
        &NamingRules {
            schema_version: 1,
            rules: Vec::new(),
        },
        "",
    )?;
    let (paths, _warnings) = paths_by_rules(ports, scope, request, names, &rules).await?;
    Ok(paths)
}

/// The card paths of `names` under the naming rules saved at the import's base, and the
/// warnings naming them gave (the module header, step 2). Each card's path without rules is
/// its subject (`destination/source_card_name`), of type `SOURCE_CARD_TYPE`, with its file
/// name as the original's; the taken paths keep items distinct from folders and retain folder
/// spelling, over the whole tree at the base and every card named before it. An item cannot
/// become a destination folder: such a rule falls back before any upload is consumed.
///
/// # Errors
/// Returns `InvalidInput` on `/apply_naming_rules` for rules that do not read or that this
/// build cannot apply (their first problem), `InvalidInput` on `/upload_ids/<index>` for a file
/// name that gives no workspace path, on `/destination` when the unruled path itself cannot
/// use a folder (before any upload is consumed), or any port error.
async fn ruled_paths(
    ports: &HandlerPorts,
    scope: &StorageScope,
    request: &ImportRequest<'_>,
    names: &[&str],
) -> Result<(Vec<WorkspacePath>, Vec<Warning>), ApiError> {
    let base = request.base_revision;
    let Some(file) = ports.versions.rules(scope, base).await? else {
        let paths = unruled_paths(ports, scope, request, names).await?;
        return Ok((
            paths,
            vec![Warning {
                code: NAMING_RULES_ABSENT.to_owned(),
                message: "no naming rules are saved at the import's base, so the cards are \
                          named without them"
                    .to_owned(),
                location: None,
            }],
        ));
    };
    let unusable = |why: String| {
        ApiError::new(
            ErrorCode::InvalidInput,
            format!("the naming rules saved at the import's base cannot be applied: {why}"),
        )
        .with_field("/apply_naming_rules")
    };
    let saved = file.parsed.map_err(|error| unusable(error.message))?;
    let rules = CompiledRules::compile(&saved, "").map_err(|error| match error.field {
        Some(field) => unusable(format!("{field}: {}", error.message)),
        None => unusable(error.message),
    })?;
    paths_by_rules(ports, scope, request, names, &rules).await
}

/// Reserve each new card against all items and folders before consuming any upload.
async fn paths_by_rules(
    ports: &HandlerPorts,
    scope: &StorageScope,
    request: &ImportRequest<'_>,
    names: &[&str],
    rules: &CompiledRules,
) -> Result<(Vec<WorkspacePath>, Vec<Warning>), ApiError> {
    let tree = ports.versions.tree(scope, request.base_revision).await?;
    let mut taken = TakenPaths::new(&tree.items, &tree.folders);
    let mut paths = Vec::new();
    let mut warnings = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let field = format!("/upload_ids/{index}");
        let unruled = card_path(request.destination, &source_card_name(name), name, &field)?;
        let chosen = name_new_item(
            rules,
            &Subject {
                path: &unruled,
                type_name: SOURCE_CARD_TYPE,
                original_name: Some(name),
            },
            &mut taken,
        )?;
        paths.push(chosen.path);
        warnings.extend(chosen.warnings);
    }
    Ok((paths, warnings))
}

/// The workspace path of a card named `card` in `destination`, for the upload named `name`;
/// `InvalidInput` on `field` when it gives none.
fn card_path(
    destination: Option<&WorkspacePath>,
    card: &str,
    name: &str,
    field: &str,
) -> Result<WorkspacePath, ApiError> {
    let full = match destination {
        Some(folder) => format!("{}/{card}", folder.as_str()),
        None => card.to_owned(),
    };
    WorkspacePath::from_supplied(&full).map_err(|error| {
        ApiError::new(
            ErrorCode::InvalidInput,
            format!("the file name {name:?} gives no workspace path: {error}"),
        )
        .with_field(field)
    })
}

/// Run a `Redigest` job: convert the source again and replace its card.
pub(crate) async fn redigest(
    ports: &HandlerPorts,
    attempt: &Attempt<'_>,
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
        replayed(ports, attempt, scope, (base_revision, 0), Some(item_id)).await?
    {
        return Ok(Done {
            outputs: committed_outputs(ports, attempt, scope, &committed.revision, item_id).await?,
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
        convert_document(ports, attempt, scope, &local, &file_name, settings).await?
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
        attempt,
        scope,
        (base_revision, 0),
        format!("Redigest {}: {}", card.title, outcome_word(&converted)),
        writes,
    )
    .await?;
    let refusal = |message: String| Done {
        refused: Some(ApiError::new(ErrorCode::Conflict, message)),
        ..Done::default()
    };
    Ok(match landed {
        Landed::Committed(committed) => Done {
            outputs: if committed.replayed {
                committed_outputs(ports, attempt, scope, &committed.revision, item_id).await?
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
    attempt: &Attempt<'_>,
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
        attempt,
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
            with_heartbeat(ports, attempt, progress, ports.converter.convert(input)).await?
        else {
            return Ok(None);
        };
        if identity.is_none() {
            identity = Some(conversion.converter.clone());
        }
        outcomes.push(window_outcome(ports, scope, window, conversion).await?);
        let job = attempt
            .report(ports, percent(index.saturating_add(1), total))
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
    attempt: &Attempt<'_>,
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
                let job = attempt.report(ports, progress).await?;
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

/// The retained outputs a committed card references (`outputs_at`), read under the heartbeat.
async fn committed_outputs(
    ports: &HandlerPorts,
    attempt: &Attempt<'_>,
    scope: &StorageScope,
    revision: &Revision,
    item_id: ItemId,
) -> Result<Vec<Digest>, ApiError> {
    renewed(ports, attempt, outputs_at(ports, scope, revision, item_id)).await
}

/// Run a long storage read under the heartbeat (`handler::renewing`), so the lease stays live
/// however long it takes. Like the other long storage calls it is not dropped for a
/// cancellation: one seen meanwhile is acted on at the job's next checkpoint, which reads the
/// job's state again (a replay with no later checkpoint completes the work that landed).
async fn renewed<T>(
    ports: &HandlerPorts,
    attempt: &Attempt<'_>,
    work: impl Future<Output = Result<T, ApiError>>,
) -> Result<T, ApiError> {
    Ok(match renewing(ports, attempt, work).await? {
        Renewed::Done(value) | Renewed::Cancelled(value) => value,
    })
}

/// The retained outputs a committed card references: those of the conversion record its
/// extraction names at `revision` (`outputs_of`), or none when it names no record.
async fn outputs_at(
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
/// edits, a real collision: `Landed::Refused`. A commit that lands, either way, is indexed and
/// announced. A card is checked over the card as written onto the head read (`card_check`).
async fn commit(
    ports: &HandlerPorts,
    attempt: &Attempt<'_>,
    scope: &StorageScope,
    (base, ordinal): (&Revision, u32),
    message: String,
    writes: Writes<'_>,
) -> Result<Landed, ApiError> {
    let claimed = attempt.claimed;
    let item = written_item(&writes);
    if let Some(committed) = replayed(ports, attempt, scope, (base, ordinal), item).await? {
        return Ok(Landed::Committed(committed));
    }
    for _race in 0..HEAD_RACES {
        let head = ports.versions.head(scope).await?;
        let (edits, check) = match &writes {
            Writes::Edits(edits, check) => (edits.clone(), Arc::clone(check)),
            Writes::Card { card, since, rule } => {
                match onto_head(ports, scope, &head, card, since, *rule).await? {
                    Ok(card) => {
                        let check = card_check(ports, &card);
                        (vec![TreeEdit::WriteSourceCard(Box::new(card))], check)
                    }
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
            check,
        )
        .await;
        match written {
            Ok(committed) => {
                let mutation_id = derive_commit_mutation_id(claimed.mutation_id, ordinal);
                announce_commit(
                    ports,
                    claimed,
                    scope,
                    &committed.revision,
                    mutation_id,
                    item,
                )
                .await?;
                return Ok(Landed::Committed(committed));
            }
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
/// answered as the store answers a replay: with the warnings it kept when that attempt committed
/// (`VersionStore::find_commit`), so the completion's warnings do not depend on where an attempt
/// died. No check runs here. A replayed commit is indexed and announced again, naming `item`,
/// which has no second effect.
async fn replayed(
    ports: &HandlerPorts,
    attempt: &Attempt<'_>,
    scope: &StorageScope,
    (base, ordinal): (&Revision, u32),
    item: Option<ItemId>,
) -> Result<Option<Committed>, ApiError> {
    let claimed = attempt.claimed;
    let mutation_id = derive_commit_mutation_id(claimed.mutation_id, ordinal);
    let Some(found) = ports
        .versions
        .find_commit(scope, mutation_id, Some(base))
        .await?
    else {
        return Ok(None);
    };
    announce_commit(ports, claimed, scope, &found.revision, mutation_id, item).await?;
    Ok(Some(found))
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

/// The item a commit writes, when it writes exactly one card: what its `changed` event names.
fn written_item(writes: &Writes<'_>) -> Option<ItemId> {
    match writes {
        Writes::Card { card, .. } => Some(card.item_id),
        Writes::Edits(edits, _check) => match edits.as_slice() {
            [TreeEdit::WriteSourceCard(card)] => Some(card.item_id),
            _ => None,
        },
    }
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
