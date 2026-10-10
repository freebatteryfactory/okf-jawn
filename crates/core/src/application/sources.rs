//! Citations and object reads: an item's supporting sources, and the bytes a citation may open.
//!
//! A source card's support is the card itself: its appearance and one whole-item citation of
//! its digest. A note's support is its OKF `sources` frontmatter (`reading::note_sources`): an
//! entry whose path names an item at the read revision becomes a whole-item citation of that
//! item, and every entry, cited or not, is declared in the file's order exactly as written (its
//! `id`, the key a footnote cites, and every other field), with the position of its citation or
//! why it cites nothing. A digest authorizes nothing: `get_object` serves an object only when
//! `reading::authorize_object` finds it among the cited item revision's objects, after the
//! cited revision passed the purge check. A citation's locations are the server's: a request's
//! are filled when omitted and must equal the computed ones when present, and a response's are
//! computed by the same rule (`computed_locations`).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, ItemId, Revision, WorkspacePath};
use okf_jawn_contract::item::ItemDocument;
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::source::{
    DeclaredOutcome, DeclaredSource, GetObjectRequest, GetObjectResponse, GetSourcesRequest,
    GetSourcesResponse, SourceAppearance, SourceLocation, SourceReference, UncitedReason,
};
use tokio::io::AsyncReadExt as _;

use super::ApplicationService;
use super::reads::shown_outline;
use super::shared::{check_named, invalid, resolve, workspace_scope};
use crate::context::OperationContext;
use crate::conversion::ConversionRecord;
use crate::reading::{
    NoteSource, ObjectRole, SourceTarget, authorize_object, cited_locations, fill_locations,
    note_sources, read_conversion_record, section_lines,
};
use crate::storage::{Page, StorageScope};

/// Most bytes one `get_object` block returns, 1 MiB; larger objects take the streaming route.
///
/// The wire schema puts no bound on `GetObjectRequest::length`, so this is the only bound on the
/// bytes one call reads into memory: a larger request is cut to it, not refused.
pub const MAX_OBJECT_BLOCK: u32 = 1_048_576;

/// Media type of a conversion record and of a docling structured export.
const JSON_MEDIA_TYPE: &str = "application/json";

/// Page size of the folder listings a path is looked up in.
const LOOKUP_PAGE: u16 = 200;

/// An item's supporting citations and, for a source card, its appearance; for a note, every
/// entry of its OKF `sources` declared in the file's order, exactly as written, with what it
/// resolved to.
///
/// A note's entry that names an item at the resolved revision is cited: the item's whole-item
/// citation is in `sources` and the entry's outcome is its position there. Two entries that
/// name one item share its one citation. Every other entry is declared uncited, with its reason.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated revision, or any port error.
pub(super) async fn get_sources(
    service: &ApplicationService,
    context: &OperationContext,
    request: GetSourcesRequest,
) -> Result<GetSourcesResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let revision = resolve(service, &scope, &request.at).await?;
    let ports = service.ports();
    let document = ports
        .versions
        .show(&scope, &revision, request.item_id)
        .await?;
    if let Some(appearance) = document.source.clone() {
        let record = record_of(service, &scope, Some(&appearance)).await?;
        return Ok(GetSourcesResponse {
            sources: vec![whole_citation(
                &scope,
                &revision,
                &document,
                record.as_ref(),
            )],
            revision,
            appearance: Some(appearance),
            declared: Vec::new(),
        });
    }
    let mut sources: Vec<SourceReference> = Vec::new();
    let mut declared = Vec::new();
    for NoteSource { entry, target } in note_sources(&document.properties, &document.summary.path) {
        let cited = match target {
            SourceTarget::Uncited(reason) => Err(reason),
            SourceTarget::Candidates(paths) => {
                first_item(service, &scope, &revision, &paths, request.item_id)
                    .await?
                    .ok_or(UncitedReason::NotFound)
            }
        };
        let outcome = match cited {
            Err(reason) => DeclaredOutcome::Uncited { reason },
            Ok(item) => {
                let index = if let Some(index) =
                    sources.iter().position(|citation| citation.item_id == item)
                {
                    index
                } else {
                    let document = ports.versions.show(&scope, &revision, item).await?;
                    let record = record_of(service, &scope, document.source.as_ref()).await?;
                    sources.push(whole_citation(
                        &scope,
                        &revision,
                        &document,
                        record.as_ref(),
                    ));
                    sources.len().saturating_sub(1)
                };
                DeclaredOutcome::Cited {
                    index: u32::try_from(index).map_err(|_| {
                        ApiError::new(ErrorCode::TooLarge, "the note declares too many sources")
                    })?,
                }
            }
        };
        declared.push(DeclaredSource { entry, outcome });
    }
    Ok(GetSourcesResponse {
        revision,
        sources,
        appearance: None,
        declared,
    })
}
/// Return a bounded block of an object the citation may open.
///
/// `length` defaults to, and is capped at, `MAX_OBJECT_BLOCK`; `has_more` says whether bytes
/// remain after the block. The citation's locations follow the citation rule
/// (`reading::fill_locations`): omitted, the server fills them; present, they must equal the
/// ones the server computes (`server_locations`), so a caller cannot supply a location.
///
/// # Errors
/// Returns `InvalidInput` on `/offset` for an offset that is not a decimal byte count or lies
/// past the object, `InvalidInput` on `/length` for zero, the typed `NotFound` of an
/// invalidated revision, `NotFound` for an object that does not belong to the cited item
/// revision, `InvalidInput` on `/source/locations` for supplied locations that are not the
/// computed ones, or any port error.
pub(super) async fn get_object(
    service: &ApplicationService,
    context: &OperationContext,
    request: GetObjectRequest,
) -> Result<GetObjectResponse, ApiError> {
    let offset = match request.offset.as_deref() {
        None => 0,
        Some(text) => text
            .parse::<u64>()
            .map_err(|_| invalid("the offset must be a decimal byte count", "/offset"))?,
    };
    let length = match request.length {
        Some(0) => return Err(invalid("ask for at least one byte", "/length")),
        Some(length) => length.min(MAX_OBJECT_BLOCK),
        None => MAX_OBJECT_BLOCK,
    };
    let scope = workspace_scope(context)?;
    let mut source = request.source;
    check_named(service, &scope, &source.revision).await?;
    let media_type = served_media_type(
        service,
        &scope,
        source.item_id,
        &source.revision,
        &request.object,
    )
    .await?;
    let computed = server_locations(service, &scope, &source).await?;
    fill_locations(&mut source, computed, "/source")?;
    let read = service
        .ports()
        .blobs
        .open(&scope, &request.object, offset, u64::from(length))
        .await?;
    let total = read.object.size;
    if offset > total {
        return Err(invalid(
            "the offset lies past the end of the object",
            "/offset",
        ));
    }
    let mut bytes = Vec::new();
    read.body
        .take(u64::from(length))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
    let read_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    Ok(GetObjectResponse {
        sha256: read.object.digest,
        offset: offset.to_string(),
        total_size: total.to_string(),
        media_type,
        data_base64: STANDARD.encode(&bytes),
        has_more: offset.saturating_add(read_len) < total,
    })
}

/// The media type an object of the cited item revision is served with, once
/// `reading::authorize_object` has found that it belongs to the revision.
///
/// The original's is the appearance's sniffed type; a conversion record and a structured
/// export are JSON; an asset's is the one its record names; a derived object's is the one
/// recorded with it.
///
/// # Errors
/// Returns `NotFound` when the object does not belong to the revision, or any port error.
pub(super) async fn served_media_type(
    service: &ApplicationService,
    scope: &StorageScope,
    item: ItemId,
    revision: &Revision,
    digest: &Digest,
) -> Result<String, ApiError> {
    let ports = service.ports();
    let role = authorize_object(
        ports.versions.as_ref(),
        ports.blobs.as_ref(),
        ports.records.as_ref(),
        scope,
        item,
        revision,
        digest,
    )
    .await?;
    let unrecorded = || {
        ApiError::new(
            ErrorCode::Internal,
            "an authorized object has no recorded media type",
        )
    };
    match role {
        ObjectRole::ConversionRecord | ObjectRole::StructuredExport => {
            Ok(JSON_MEDIA_TYPE.to_owned())
        }
        ObjectRole::Original => {
            let document = ports.versions.show(scope, revision, item).await?;
            document
                .source
                .map(|source| source.media_type)
                .ok_or_else(unrecorded)
        }
        ObjectRole::Asset(_) => {
            let document = ports.versions.show(scope, revision, item).await?;
            let record_digest = document
                .source
                .and_then(|source| source.extraction.digest)
                .ok_or_else(unrecorded)?;
            read_conversion_record(ports.blobs.as_ref(), scope, &record_digest)
                .await?
                .assets
                .into_iter()
                .find(|asset| &asset.digest == digest)
                .map(|asset| asset.media_type)
                .ok_or_else(unrecorded)
        }
        ObjectRole::Derived(_) => ports
            .records
            .derived_object(scope, item, revision, digest)
            .await?
            .map(|derived| derived.media_type)
            .ok_or_else(unrecorded),
    }
}

/// The citation of a whole item at one revision: for a source card, its conversion record's
/// digest with the locations of the whole record; for any other item, no digest and no
/// locations.
pub(super) fn whole_citation(
    scope: &StorageScope,
    revision: &Revision,
    document: &ItemDocument,
    record: Option<&ConversionRecord>,
) -> SourceReference {
    let extraction = document.source.as_ref().map(|source| &source.extraction);
    SourceReference {
        workspace_id: scope.workspace_id,
        item_id: document.summary.id,
        path: document.summary.path.clone(),
        revision: revision.clone(),
        digest: extraction.and_then(|extraction| extraction.digest.clone()),
        selection: Selection::All,
        locations: extraction.map_or_else(Vec::new, |extraction| {
            cited_locations(extraction, record, &Selection::All)
        }),
    }
}

/// The locations the server computes for a citation of `digest` with the selection `located`
/// (a section already resolved to its lines) of an item whose appearance is `appearance`
/// (Stage 1b design section 2: from item, revision, digest and selection).
///
/// - A citation of a source card's text, by its conversion record's digest (or by no digest
///   when the card has no record), locates the selection in the original
///   (`reading::cited_locations`).
/// - A citation of one of the record's retained images is that image's own location.
/// - Every other citation (a note, the original, a structured export, a derived object) has
///   none.
pub(super) fn computed_locations(
    appearance: Option<&SourceAppearance>,
    record: Option<&ConversionRecord>,
    digest: Option<&Digest>,
    located: &Selection,
) -> Vec<SourceLocation> {
    let Some(appearance) = appearance else {
        return Vec::new();
    };
    if digest == appearance.extraction.digest.as_ref() {
        return cited_locations(&appearance.extraction, record, located);
    }
    record
        .and_then(|record| {
            record
                .assets
                .iter()
                .find(|asset| Some(&asset.digest) == digest)
        })
        .map(|asset| vec![asset.location.clone()])
        .unwrap_or_default()
}

/// The locations the server computes for a citation a request carries, read through the ports:
/// the ones a response carries for the same citation (`computed_locations`).
///
/// The conversion record is read only when the citation is not of the original, and a section
/// is resolved to its lines through the outline of the shown text, as `read_item` resolves it.
///
/// # Errors
/// Returns `NotFound` on `/source/selection/heading` for a section the text does not have,
/// `InvalidInput` there for an ambiguous one, or any port error.
async fn server_locations(
    service: &ApplicationService,
    scope: &StorageScope,
    source: &SourceReference,
) -> Result<Vec<SourceLocation>, ApiError> {
    let document = service
        .ports()
        .versions
        .show(scope, &source.revision, source.item_id)
        .await?;
    let Some(appearance) = document.source.as_ref() else {
        return Ok(Vec::new());
    };
    if source.digest.as_ref() == Some(&appearance.object) {
        return Ok(Vec::new());
    }
    let record = record_of(service, scope, Some(appearance)).await?;
    let located = match &source.selection {
        Selection::Section { heading } if source.digest == appearance.extraction.digest => {
            let (_, outline) = shown_outline(&document, record.as_ref());
            Selection::Lines {
                range: section_lines(&outline, heading)
                    .map_err(|error| error.with_field("/source/selection/heading"))?,
            }
        }
        other => other.clone(),
    };
    Ok(computed_locations(
        Some(appearance),
        record.as_ref(),
        source.digest.as_ref(),
        &located,
    ))
}

/// The conversion record a source appearance names, if it names one.
///
/// # Errors
/// Returns any error of reading the record.
pub(super) async fn record_of(
    service: &ApplicationService,
    scope: &StorageScope,
    source: Option<&SourceAppearance>,
) -> Result<Option<ConversionRecord>, ApiError> {
    match source.and_then(|source| source.extraction.digest.as_ref()) {
        Some(digest) => Ok(Some(
            read_conversion_record(service.ports().blobs.as_ref(), scope, digest).await?,
        )),
        None => Ok(None),
    }
}

/// The first of `paths` that names an item at `revision` other than the citing note itself.
async fn first_item(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
    paths: &[WorkspacePath],
    citing: ItemId,
) -> Result<Option<ItemId>, ApiError> {
    for path in paths {
        if let Some(item) = item_at(service, scope, revision, path).await?
            && item != citing
        {
            return Ok(Some(item));
        }
    }
    Ok(None)
}

/// The item whose path is exactly `path` at `revision`, looked up in its folder's listing.
async fn item_at(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
    path: &WorkspacePath,
) -> Result<Option<ItemId>, ApiError> {
    let folder = match path.as_str().rsplit_once('/') {
        Some((parent, _)) => Some(
            WorkspacePath::try_from(parent.to_owned())
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.0))?,
        ),
        None => None,
    };
    let mut cursor = None;
    loop {
        let listed = service
            .ports()
            .versions
            .list(
                scope,
                revision,
                folder.as_ref(),
                Page {
                    cursor,
                    limit: LOOKUP_PAGE,
                },
            )
            .await;
        let listing = match listed {
            Ok(listing) => listing,
            // A folder that does not exist holds no item.
            Err(error) if error.code == ErrorCode::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if let Some(found) = listing.items.iter().find(|item| &item.path == path) {
            return Ok(Some(found.id));
        }
        match listing.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(None),
        }
    }
}
