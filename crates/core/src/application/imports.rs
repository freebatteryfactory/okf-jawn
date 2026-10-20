//! Imports, conversion again and corrections (SPEC section 5): `start_import`, `redigest_item`
//! and `correct_digest`. The first two record a job and wake the worker
//! (`workspaces::start_job`), answered with the queued `Job` (status 202); ingest runs the job.
//!
//! `start_import` checks, before its job is queued, in this order, so a refused request queues
//! nothing:
//! - the request's own rules, the ones dispatch runs as `check_rules` (between one and
//!   `MAX_IMPORT_UPLOADS` uploads, each listed once), run here again so the handler holds them;
//!   and supplied settings within their bounds (`ConversionSettings::check`).
//! - the destination: empty is the workspace root (`None`); anything else must parse as a
//!   `WorkspacePath` (`invalid_input` on `/destination`), and need not exist at the base.
//! - the base: a revision of the workspace, checked against the revision map
//!   (`shared::check_named`) and then held by the store, which answers `NotFound` for a revision
//!   it does not hold. A head that moved since does not refuse the import (contract ruling 26):
//!   card names are chosen at the base, and a card path taken since fails the job.
//! - the destination against the complete base tree, including hidden folders: a folder
//!   with the same collision key as the destination's folder at that level but another spelling
//!   is refused as `invalid_input` on `/destination`, naming the spelling the base holds, so the
//!   cards never land in a second spelling of an existing folder. A level the base does not hold
//!   ends the walk: the rest is new. An item at any segment, including a collision-key variant,
//!   is refused before a job is created. The store's commit still refuses such a collision
//!   whatever lands meanwhile.
//! - every upload, in the request's order: held by the workspace (the store's `NotFound`, named
//!   on `/upload_ids/<i>`), complete (`invalid_input` on `/upload_ids/<i>`), and not taken by a
//!   job (`conflict` on `/upload_ids/<i>`, naming the job that took it).
//!
//! Not refused at acceptance: an upload that a queued or running job names but has not consumed
//! yet. The job consumes its uploads in one transaction, all of them or none
//! (`UploadStore::consume_batch`), so a second import of the same uploads fails its job at run
//! time, and nothing is imported twice.
//!
//! Settings: absent ones are the deployment's (`ApplicationConfig::conversion_defaults`),
//! resolved into the job's specification now, so a retry of the job converts the same way.
//! `apply_naming_rules` is passed on as asked; ingest uses core's naming engine for card names.
//!
//! A resumed attempt (`Attempt::Resumed`) runs the same checks and is answered the job its first
//! attempt created (`RecordStore::create_job` is unique on the mutation). That job may have taken
//! the uploads meanwhile: `RecordStore::job_mutation` reads the consuming job's immutable
//! creating mutation within this workspace. Matching it recognizes this request's own job
//! even before its first commit or after it failed. Every later upload is still checked; no
//! ownership match returns early or creates a job before all acceptance checks pass.
//!
//! `redigest_item` follows the import's rule for its base (contract ruling 26, the integration
//! owner's ruling R3 for core M1.5): the base must be a revision of the workspace (the revision
//! map, then the store, through `VersionStore::show` of the item there), and a head that moved
//! since does not refuse; ingest writes onto the head's card unless the item was deleted or its
//! original changed since the base (ingest ruling R-I8). The item must be a source at the base
//! (`invalid_input` on `/item_id`), and the settings within their bounds. With
//! `unconverted_only`, core computes the job's `pages`: the `not_converted` pages of the card's
//! conversion record at the base (`reading::read_conversion_record` of its extraction digest). A
//! card with no record, or whose record names no page that was not converted, is `invalid_input`
//! on `/unconverted_only`; settings other than the record's converter settings (the current
//! digest's) are `invalid_input` on `/settings`. Stated, not hidden: ingest's merge of the newly
//! converted pages into a new digest is ingest M2b, so until it lands such a job fails with
//! ingest's `NotImplemented` (request R-I5).
//!
//! `correct_digest` writes one commit (`TreeEdit::CorrectDigest`) through
//! `shared::commit_with_receipt`, with the base as the expected head: a base that is not the head
//! is `Conflict` with `ErrorDetail::StaleBase` on `/base_revision`. Before it, `digest` must be
//! the item's digest at the base, the conversion record its extraction names
//! (`invalid_input` on `/digest` otherwise, a note or a card with no record included). With
//! `adopts`, the proposal must be accepted, supply text for this item
//! (`ProposalKind::SupplyExtraction`), and hold the same text as `corrected_markdown`, compared
//! by SHA-256 digest; otherwise `conflict` on `/adopts` (a proposal the workspace does not hold
//! is the store's `NotFound`, named on `/adopts`). The correction is kept beside the generated
//! extraction (storage's `TreeEdit::CorrectDigest`), and a card replaced by a later conversion
//! keeps it (`TreeEdit::WriteSourceCard`): re-extraction does not erase a correction. A resumed
//! attempt runs the same checks at the same base and is answered the first attempt's commit and
//! receipt (the commit replays on the mutation).

use std::collections::BTreeMap;

use okf_jawn_contract::common::{MutationResult, PageRange};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{ConversionOutcome, ConversionSettings};
use okf_jawn_contract::identity::{
    Digest, ItemId, MutationId, ProposalId, Revision, UploadId, WorkspacePath, collision_key_of,
};
use okf_jawn_contract::import::{CorrectDigestRequest, Job, RedigestRequest, StartImportRequest};
use okf_jawn_contract::proposal::{Change, ProposalKind, ProposalStatus};
use okf_jawn_contract::scope::RequestScope as _;
use okf_jawn_contract::source::SourceAppearance;
use sha2::{Digest as _, Sha256};

use super::ApplicationService;
use super::shared::{
    Touched, Write, check_named, commit_with_receipt, invalid, mutation_id, workspace_scope,
};
use super::sources::record_of;
use super::workspaces::start_job;
use crate::context::{Attempt, OperationContext};
use crate::jobs::{JobScope, JobSpec};
use crate::storage::{Page, StorageScope, TreeEdit};

/// The request field a destination is refused on.
const DESTINATION: &str = "/destination";

/// The request field a proposal to adopt is refused on.
const ADOPTS: &str = "/adopts";

/// Queue the import of completed uploads into the destination folder, once every check of the
/// module header passes.
///
/// # Errors
/// Returns `TooLarge` or `InvalidInput` on `/upload_ids` for a list the contract refuses,
/// `InvalidInput` on `/settings/page_image_dpi` for settings out of bounds, on `/destination`
/// for a destination that is not a workspace path or names a folder of the base spelled
/// otherwise, and on `/upload_ids/<i>` for an incomplete upload; the typed `NotFound` of an
/// invalidated base, the store's `NotFound` of a base it does not hold or of an upload (on
/// `/upload_ids/<i>`); `Conflict` on `/upload_ids/<i>` for an upload a job took; or any port
/// error.
pub(super) async fn start_import(
    service: &ApplicationService,
    context: &OperationContext,
    request: StartImportRequest,
) -> Result<Job, ApiError> {
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    request.check_uploads()?;
    if let Some(settings) = &request.settings {
        settings.check()?;
    }
    let destination = destination_of(&request.destination)?;
    let base = request.base_revision;
    check_named(service, &scope, &base).await?;
    check_destination(service, &scope, &base, destination.as_ref()).await?;
    check_uploads(service, context, &scope, mutation, &request.upload_ids).await?;
    let settings = request
        .settings
        .unwrap_or_else(|| service.config().conversion_defaults.clone());
    start_job(
        service,
        context,
        JobScope::Workspace(scope),
        JobSpec::Import {
            base_revision: base,
            upload_ids: request.upload_ids,
            destination,
            apply_naming_rules: request.apply_naming_rules,
            settings,
        },
    )
    .await
}

/// Queue a conversion of one source again with the request's settings, of the whole source or
/// of the pages its conversion record names as not converted.
///
/// # Errors
/// Returns `InvalidInput` on `/settings/page_image_dpi` for settings out of bounds, on `/item_id`
/// for an item that is not a source at the base, on `/unconverted_only` for a source with no
/// unconverted page, and on `/settings` for settings other than the current digest's with
/// `unconverted_only`; the typed `NotFound` of an invalidated base, the store's `NotFound` of an
/// item or base it does not hold; or any port error.
pub(super) async fn redigest_item(
    service: &ApplicationService,
    context: &OperationContext,
    request: RedigestRequest,
) -> Result<Job, ApiError> {
    let scope = workspace_scope(context)?;
    request.settings.check()?;
    check_named(service, &scope, &request.base_revision).await?;
    let document = service
        .ports()
        .versions
        .show(&scope, &request.base_revision, request.item_id)
        .await?;
    let Some(source) = document.source.as_ref() else {
        return Err(invalid(
            "only a source is converted; this item has no original",
            "/item_id",
        ));
    };
    let pages = if request.unconverted_only {
        Some(unconverted_pages(service, &scope, source, &request.settings).await?)
    } else {
        None
    };
    start_job(
        service,
        context,
        JobScope::Workspace(scope),
        JobSpec::Redigest {
            item_id: request.item_id,
            base_revision: request.base_revision,
            settings: request.settings,
            pages,
        },
    )
    .await
}

/// Record a correction of one digest of a source beside its generated extraction, in one
/// commit on the base, or keep the accepted supplied text of a proposal over it.
///
/// # Errors
/// Returns `TooLarge` on `/corrected_markdown` for a text over the bound; `InvalidInput` on
/// `/digest` for a digest that is not the item's at the base; `Conflict` on `/adopts` for a
/// proposal that is not accepted, supplies no text for this item or holds another text, and the
/// store's `NotFound` of a proposal there; the typed `NotFound` of an invalidated base; the
/// store's `NotFound` of an item or a base it does not hold; `Conflict` with
/// `ErrorDetail::StaleBase` on `/base_revision` when the head moved; or any port error.
pub(super) async fn correct_digest(
    service: &ApplicationService,
    context: &OperationContext,
    request: CorrectDigestRequest,
) -> Result<MutationResult, ApiError> {
    request.check_rules()?;
    let scope = workspace_scope(context)?;
    check_named(service, &scope, &request.base_revision).await?;
    let document = service
        .ports()
        .versions
        .show(&scope, &request.base_revision, request.item_id)
        .await?;
    let current = document
        .source
        .as_ref()
        .and_then(|source| source.extraction.digest.as_ref());
    if current != Some(&request.digest) {
        return Err(invalid(
            "the digest is not this item's at the base revision",
            "/digest",
        ));
    }
    if let Some(proposal) = request.adopts {
        check_adopted(
            service,
            &scope,
            proposal,
            request.item_id,
            &request.corrected_markdown,
        )
        .await?;
    }
    let message = if request.adopts.is_some() {
        "Keep the supplied text"
    } else {
        "Correct the text"
    };
    commit_with_receipt(
        service,
        context,
        Write {
            base: request.base_revision,
            message: message.to_owned(),
            edits: vec![TreeEdit::CorrectDigest {
                item_id: request.item_id,
                digest: request.digest,
                corrected_markdown: request.corrected_markdown,
                adopts: request.adopts,
            }],
            item: Some(request.item_id),
            touched: Touched {
                items: vec![request.item_id],
                ..Touched::default()
            },
        },
    )
    .await
}

/// The destination folder a request names: `None` for the root (an empty string).
///
/// # Errors
/// Returns `InvalidInput` on `/destination` for text that is not a workspace path.
fn destination_of(destination: &str) -> Result<Option<WorkspacePath>, ApiError> {
    if destination.is_empty() {
        return Ok(None);
    }
    WorkspacePath::try_from(destination.to_owned())
        .map(Some)
        .map_err(|error| invalid(error.0, DESTINATION))
}

/// Refuse a base the store does not hold, and a destination that names a folder of the base
/// spelled otherwise, at any level, using the complete tree rather than the visible listing.
///
/// # Errors
/// Returns `InvalidInput` on `/destination` naming the base's spelling, the store's `NotFound`
/// for a revision it does not hold, or any port error.
async fn check_destination(
    service: &ApplicationService,
    scope: &StorageScope,
    base: &Revision,
    destination: Option<&WorkspacePath>,
) -> Result<(), ApiError> {
    let Some(destination) = destination else {
        // The store answers `NotFound` for a revision it does not hold.
        service
            .ports()
            .versions
            .list(
                scope,
                base,
                None,
                Page {
                    cursor: None,
                    limit: 1,
                },
            )
            .await?;
        return Ok(());
    };
    // Ordinary listings deliberately omit dot-prefixed folders. Build collision-key indexes
    // once from the complete tree, so a hidden item or folder is equally occupied.
    let tree = service.ports().versions.tree(scope, base).await?;
    let items: BTreeMap<_, _> = tree
        .items
        .iter()
        .map(|item| (item.summary.path.collision_key(), &item.summary.path))
        .collect();
    let folders: BTreeMap<_, _> = tree
        .folders
        .iter()
        .map(|folder| (folder.collision_key(), folder))
        .collect();
    let mut wanted = String::new();
    for segment in destination.as_str().split('/') {
        if !wanted.is_empty() {
            wanted.push('/');
        }
        wanted.push_str(segment);
        let key = collision_key_of(&wanted);
        if let Some(item) = items.get(&key) {
            return Err(invalid(
                format!(
                    "the destination names the item {} as a folder",
                    item.as_str()
                ),
                DESTINATION,
            ));
        }
        match folders.get(&key) {
            None => return Ok(()),
            Some(held) if held.as_str() == wanted => {}
            Some(held) => {
                return Err(invalid(
                    format!(
                        "the destination names the folder {} spelled {wanted}; use the folder's \
                         own spelling",
                        held.as_str()
                    ),
                    DESTINATION,
                ));
            }
        }
    }
    Ok(())
}

/// Refuse, at the first upload in the request's order that fails, an upload the workspace does
/// not hold, one that is not complete, and one a job took (unless, on a resumed attempt, this
/// request's own job took them: see the module header).
///
/// # Errors
/// Returns the store's `NotFound` on `/upload_ids/<i>`, `InvalidInput` there for an incomplete
/// upload, `Conflict` there naming the job that took one, or any other port error unchanged.
async fn check_uploads(
    service: &ApplicationService,
    context: &OperationContext,
    scope: &StorageScope,
    mutation: MutationId,
    uploads: &[UploadId],
) -> Result<(), ApiError> {
    let store = &service.ports().uploads;
    let jobs = JobScope::Workspace(scope.clone());
    for (index, upload) in uploads.iter().enumerate() {
        let field = format!("/upload_ids/{index}");
        let record = store.get(scope, *upload).await.map_err(|error| {
            if error.code == ErrorCode::NotFound {
                error.with_field(field.clone())
            } else {
                error
            }
        })?;
        if record.object.is_none() {
            return Err(invalid("the upload is not complete", &field));
        }
        if let Some(job) = record.consumed_by {
            if context.attempt == Attempt::Resumed
                && service.ports().records.job_mutation(&jobs, job).await? == mutation
            {
                continue;
            }
            return Err(ApiError::new(
                ErrorCode::Conflict,
                format!("the upload was already taken by job {}", job.0),
            )
            .with_field(field));
        }
    }
    Ok(())
}

/// The pages a redigest of only the unconverted pages converts: the `not_converted` pages of the
/// source's conversion record, converted with the record's own settings.
///
/// # Errors
/// Returns `InvalidInput` on `/unconverted_only` for a source with no record or none of whose
/// pages are unconverted, on `/settings` for settings other than the record's, or any error of
/// reading the record.
async fn unconverted_pages(
    service: &ApplicationService,
    scope: &StorageScope,
    source: &SourceAppearance,
    settings: &ConversionSettings,
) -> Result<Vec<PageRange>, ApiError> {
    let Some(record) = record_of(service, scope, Some(source)).await? else {
        return Err(invalid(
            "the source has no conversion whose unconverted pages could be converted alone; \
             convert the whole source",
            "/unconverted_only",
        ));
    };
    let pages = match &record.outcome {
        ConversionOutcome::Partial {
            coverage: Some(coverage),
            ..
        } => coverage.not_converted.clone(),
        _ => Vec::new(),
    };
    if pages.is_empty() {
        return Err(invalid(
            "the source's conversion names no page that was not converted",
            "/unconverted_only",
        ));
    }
    if record.converter.settings != *settings {
        return Err(invalid(
            "converting only the unconverted pages takes the current digest's settings",
            "/settings",
        ));
    }
    Ok(pages)
}

/// Refuse a proposal to adopt that is not accepted, supplies no text for `item`, or holds
/// another text than `corrected`, compared by SHA-256 digest.
///
/// # Errors
/// Returns `Conflict` on `/adopts` for each of those, the store's `NotFound` there for a
/// proposal the workspace does not hold, or any other port error unchanged.
async fn check_adopted(
    service: &ApplicationService,
    scope: &StorageScope,
    adopted: ProposalId,
    item: ItemId,
    corrected: &str,
) -> Result<(), ApiError> {
    let proposal = service
        .ports()
        .proposals
        .get(scope, adopted)
        .await
        .map_err(|error| {
            if error.code == ErrorCode::NotFound {
                error.with_field(ADOPTS)
            } else {
                error
            }
        })?;
    let refused = |message: &str| ApiError::new(ErrorCode::Conflict, message).with_field(ADOPTS);
    if proposal.status != ProposalStatus::Accepted {
        return Err(refused("only an accepted proposal's text can be kept"));
    }
    let supplied = proposal.changes.iter().find_map(|change| match change {
        Change::SupplyExtraction {
            item_id, markdown, ..
        } if *item_id == item => Some(markdown),
        _ => None,
    });
    let Some(supplied) = supplied.filter(|_| proposal.kind == ProposalKind::SupplyExtraction)
    else {
        return Err(refused("the proposal supplies no text for this item"));
    };
    if text_digest(supplied)? != text_digest(corrected)? {
        return Err(refused(
            "the corrected text is not the text the proposal supplied",
        ));
    }
    Ok(())
}

/// The SHA-256 of `text`'s UTF-8 bytes.
///
/// # Errors
/// Returns `Internal` when the hex spelling is not a digest, which SHA-256 never gives.
fn text_digest(text: &str) -> Result<Digest, ApiError> {
    let hash = Sha256::digest(text.as_bytes());
    Digest::try_from(format!("{hash:x}"))
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
}
