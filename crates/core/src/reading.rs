//! Which retained objects a citation may open (Stage 1b design section 9.5).
//!
//! A digest authorizes nothing by itself: `get_object` serves an object only when it belongs to
//! the cited item revision. For a source card that is its original, its conversion record, the
//! record's structured window exports and its retained assets; for any item, an object derived
//! from that revision and recorded for it (a View binding's dataset). Nothing else is served,
//! so a backup or export archive's digest, named with an otherwise valid citation of the
//! workspace, is `NotFound` exactly as an unknown digest is.
//!
//! The same module holds the other checks a read makes before it answers: a named revision is
//! checked against the revision map a purge wrote (`check_revision`), and a citation's
//! locations are computed from the conversion record (`cited_locations`) and filled or
//! compared (`fill_locations`); a saved View keeps none (`strip_locations`).

use tokio::io::AsyncReadExt as _;

use okf_jawn_contract::{
    common::TextRange,
    error::{ApiError, ErrorCode, ErrorDetail},
    extraction::{Extraction, TextOrigin},
    identity::{Digest, ItemId, Revision},
    read::{AssetRole, Selection},
    source::{SourceAppearance, SourceLocation, SourceLocator, SourceReference, UnresolvedReason},
    views::ViewDocument,
};

use crate::conversion::ConversionRecord;
use crate::jobs::{DerivedKind, DerivedObject, RecordStore, RevisionMapping};
use crate::storage::{BlobStore, StorageScope, VersionStore};
use crate::stored::{ValidatorCell, decode_stored};

/// What an object is to the item revision that may serve it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectRole {
    /// The source card's retained original.
    Original,
    /// The conversion record of the card's digest.
    ConversionRecord,
    /// One window's docling export, named by the record.
    StructuredExport,
    /// A page render or picture, named by the record.
    Asset(AssetRole),
    /// An object derived from this revision and recorded for it.
    Derived(DerivedKind),
}

/// The objects of one item revision, as the caller read them.
#[derive(Debug, Clone, Copy)]
pub struct RevisionObjects<'a> {
    /// The cited item.
    pub item_id: ItemId,
    /// The cited revision.
    pub revision: &'a Revision,
    /// The source appearance at that revision, for a source card.
    pub source: Option<&'a SourceAppearance>,
    /// The conversion record the appearance's extraction names, when it names one.
    pub record: Option<&'a ConversionRecord>,
    /// What `RecordStore::derived_object` returned for (item, revision, digest).
    pub derived: Option<&'a DerivedObject>,
}

/// Most bytes of a conversion record core reads into memory.
pub const MAX_CONVERSION_RECORD_BYTES: u64 = 64 * 1024 * 1024;

/// What `digest` is to the cited item revision.
///
/// # Errors
/// Returns `NotFound` when the digest belongs to none of the revision's objects.
pub fn object_role(objects: &RevisionObjects<'_>, digest: &Digest) -> Result<ObjectRole, ApiError> {
    if let Some(source) = objects.source {
        if &source.object == digest {
            return Ok(ObjectRole::Original);
        }
        if source.extraction.digest.as_ref() == Some(digest) {
            return Ok(ObjectRole::ConversionRecord);
        }
    }
    if let Some(record) = objects.record {
        if record
            .structured
            .iter()
            .any(|export| &export.digest == digest)
        {
            return Ok(ObjectRole::StructuredExport);
        }
        if let Some(asset) = record.assets.iter().find(|asset| &asset.digest == digest) {
            return Ok(ObjectRole::Asset(asset.role));
        }
    }
    if let Some(derived) = objects.derived {
        // The record is trusted only for the exact (item, revision, digest) it was made for.
        if derived.item_id == objects.item_id
            && &derived.revision == objects.revision
            && &derived.digest == digest
        {
            return Ok(ObjectRole::Derived(derived.kind));
        }
    }
    Err(not_found())
}

/// Read the cited item revision's objects through the ports and decide what `digest` is.
///
/// The conversion record is read from the blob store only when the digest is neither the
/// original nor the record itself.
///
/// # Errors
/// Returns `NotFound` when the digest does not belong to the revision, and any port error.
pub async fn authorize_object(
    versions: &dyn VersionStore,
    blobs: &dyn BlobStore,
    records: &dyn RecordStore,
    scope: &StorageScope,
    item: ItemId,
    revision: &Revision,
    digest: &Digest,
) -> Result<ObjectRole, ApiError> {
    let document = versions.show(scope, revision, item).await?;
    let source = document.source.as_ref();
    let direct = RevisionObjects {
        item_id: item,
        revision,
        source,
        record: None,
        derived: None,
    };
    if let Ok(role) = object_role(&direct, digest) {
        return Ok(role);
    }
    // A recorded derived object is decided before the conversion record is read, so a missing,
    // oversized or faulty record cannot hide a dataset of the revision.
    let derived = records
        .derived_object(scope, item, revision, digest)
        .await?;
    let with_derived = RevisionObjects {
        derived: derived.as_ref(),
        ..direct
    };
    if let Ok(role) = object_role(&with_derived, digest) {
        return Ok(role);
    }
    let record = match source.and_then(|source| source.extraction.digest.as_ref()) {
        Some(record_digest) => Some(read_conversion_record(blobs, scope, record_digest).await?),
        None => None,
    };
    object_role(
        &RevisionObjects {
            record: record.as_ref(),
            ..with_derived
        },
        digest,
    )
}

/// Read and decode the conversion record a digest names.
///
/// # Errors
/// Returns `NotFound` when absent, `TooLarge` past `MAX_CONVERSION_RECORD_BYTES`, and
/// `Internal` when the stored bytes are not a conversion record.
pub async fn read_conversion_record(
    blobs: &dyn BlobStore,
    scope: &StorageScope,
    digest: &Digest,
) -> Result<ConversionRecord, ApiError> {
    let mut read = blobs
        .open(scope, digest, 0, MAX_CONVERSION_RECORD_BYTES)
        .await?;
    if read.object.size > MAX_CONVERSION_RECORD_BYTES {
        return Err(ApiError::new(
            ErrorCode::TooLarge,
            "the conversion record is larger than core reads",
        ));
    }
    let mut bytes = Vec::new();
    read.body
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
    decode_conversion_record(&bytes)
}

/// Decode stored conversion-record bytes, validated against the record's schema.
///
/// # Errors
/// Returns `Internal` when the bytes are not JSON or not a conversion record.
pub fn decode_conversion_record(bytes: &[u8]) -> Result<ConversionRecord, ApiError> {
    static SCHEMA: ValidatorCell = ValidatorCell::new();
    let value = serde_json::from_slice(bytes).map_err(|error| {
        ApiError::new(
            ErrorCode::Internal,
            format!("a stored conversion record is not JSON: {error}"),
        )
    })?;
    decode_stored(value, "a stored conversion record", &SCHEMA)
}

/// Refuse a revision a purge removed or rewrote, as a typed `NotFound` naming the purge and,
/// after an item purge, the revision to re-pin to. Checked before anything is read at it.
///
/// # Errors
/// Returns that `NotFound`, or any error of `records`.
pub async fn check_revision(
    records: &dyn RecordStore,
    scope: &StorageScope,
    revision: &Revision,
) -> Result<(), ApiError> {
    match records.revision_mapping(scope, revision).await? {
        None => Ok(()),
        Some(mapping) => Err(invalidated(&mapping)),
    }
}

/// The error a read of an invalidated revision returns.
#[must_use]
pub fn invalidated(mapping: &RevisionMapping) -> ApiError {
    ApiError::new(
        ErrorCode::NotFound,
        "The revision was removed or rewritten by a purge",
    )
    .with_detail(ErrorDetail::Invalidated {
        purge_id: mapping.purge_id,
        replacement: mapping.replacement.clone(),
    })
}

/// Where the cited selection of a digest lies in the original, one entry per located item it
/// covers, in reading order.
///
/// Converter text is located from the record's line map: a line selection takes the entries
/// whose lines overlap it, a page selection the entries on those pages, the whole source every
/// entry. A cell or region selection is its own direct location. A correction or supplied text
/// has no place in the original and is one unresolved entry saying which. A section selection
/// is resolved to its lines first (`section_lines`) and located as those lines; passed here
/// unresolved it has no entry.
#[must_use]
pub fn cited_locations(
    extraction: &Extraction,
    record: Option<&ConversionRecord>,
    selection: &Selection,
) -> Vec<SourceLocation> {
    if extraction.corrected {
        return vec![SourceLocation::Unresolved {
            reason: UnresolvedReason::CorrectedText,
        }];
    }
    if extraction.text_origin == TextOrigin::SuppliedByAgent {
        return vec![SourceLocation::Unresolved {
            reason: UnresolvedReason::SuppliedText,
        }];
    }
    match selection {
        Selection::Cells { range } => {
            return vec![SourceLocation::Direct {
                locator: SourceLocator::Cells {
                    range: range.clone(),
                },
            }];
        }
        Selection::Region { region } => {
            return vec![SourceLocation::Direct {
                locator: SourceLocator::Region {
                    region: region.clone(),
                },
            }];
        }
        Selection::All
        | Selection::Lines { .. }
        | Selection::Pages { .. }
        | Selection::Section { .. } => {}
    }
    let Some(record) = record else {
        return Vec::new();
    };
    record
        .locations
        .iter()
        .filter(|entry| match selection {
            Selection::Lines { range } => {
                entry.lines.start <= range.end && entry.lines.end >= range.start
            }
            Selection::Pages { range } => location_page(&entry.location)
                .is_some_and(|page| page >= range.start && page <= range.end),
            Selection::All => true,
            Selection::Section { .. } | Selection::Cells { .. } | Selection::Region { .. } => false,
        })
        .map(|entry| entry.location.clone())
        .collect()
}

/// Apply the citation rule to one reference: an omitted list is filled with `computed`; a
/// present list must equal it.
///
/// `at` is the JSON Pointer of the reference in the request, such as `/bindings/0/source`.
///
/// # Errors
/// Returns `InvalidInput` on `{at}/locations` when the caller supplied locations that are
/// not the computed ones.
pub fn fill_locations(
    reference: &mut SourceReference,
    computed: Vec<SourceLocation>,
    at: &str,
) -> Result<(), ApiError> {
    if reference.locations.is_empty() {
        reference.locations = computed;
        return Ok(());
    }
    if reference.locations == computed {
        return Ok(());
    }
    Err(ApiError::new(
        ErrorCode::InvalidInput,
        "locations are computed by the server; omit them or send the computed ones",
    )
    .with_field(format!("{at}/locations")))
}

/// Remove every binding's locations before a View is saved; the server fills them again when
/// it returns the View.
pub fn strip_locations(view: &mut ViewDocument) {
    for binding in &mut view.bindings {
        binding.source.locations.clear();
    }
}

/// The page a direct or inferred location lies on.
pub(crate) const fn location_page(location: &SourceLocation) -> Option<u32> {
    match location {
        SourceLocation::Direct { locator } | SourceLocation::Inferred { locator } => {
            match locator {
                SourceLocator::Page { page_no } => Some(*page_no),
                SourceLocator::Region { region } => Some(region.page_no),
                SourceLocator::Cells { .. } => None,
            }
        }
        SourceLocation::Unresolved { .. } => None,
    }
}

/// The lines of the section a heading opens in `markdown`: from the heading line to the line
/// before the next heading of the same or a higher level, or to the end. Headings are ATX
/// headings (`#` to `######`) indented by at most three spaces; lines inside fenced code blocks
/// (closed by the `CommonMark` rule) and indented code are not headings.
///
/// # Errors
/// Returns `NotFound` on `/selection/heading` when no heading has exactly this text, and
/// `InvalidInput` there when several do, since the citation would not say which.
pub fn section_lines(markdown: &str, heading: &str) -> Result<TextRange, ApiError> {
    // The open fence's character and run length (`CommonMark` 4.5): only a run of the same
    // character at least as long, with nothing after it, closes it.
    let mut fence: Option<(u8, usize)> = None;
    let mut headings: Vec<(u32, usize, &str)> = Vec::new();
    let mut last = 0_u32;
    for (index, line) in markdown.lines().enumerate() {
        let number = u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1);
        last = number;
        let content = line.trim_start_matches(' ');
        // Four columns of indentation (a tab after at most three spaces reaches them) make the
        // line indented code or fence content: never a fence marker or a heading.
        if line.len().saturating_sub(content.len()) > 3 || content.starts_with('\t') {
            continue;
        }
        let run = fence_run(content);
        if let Some((open_char, open_len)) = fence {
            let closes = run.is_some_and(|(mark, len)| {
                mark == open_char
                    && len >= open_len
                    && content
                        .get(len..)
                        .is_some_and(|rest| rest.trim().is_empty())
            });
            if closes {
                fence = None;
            }
            continue;
        }
        if let Some((mark, len)) = run {
            let info = content.get(len..).unwrap_or_default();
            if mark == b'~' || !info.contains('`') {
                fence = run;
                continue;
            }
        }
        if let Some((level, text)) = atx_heading(content) {
            headings.push((number, level, text));
        }
    }
    let matching: Vec<usize> = headings
        .iter()
        .enumerate()
        .filter(|(_, (_, _, text))| *text == heading.trim())
        .map(|(position, _)| position)
        .collect();
    let position = match matching.as_slice() {
        [one] => *one,
        [] => {
            return Err(
                ApiError::new(ErrorCode::NotFound, "No heading has exactly this text")
                    .with_field("/selection/heading"),
            );
        }
        several => {
            return Err(ApiError::new(
                ErrorCode::InvalidInput,
                format!(
                    "{} headings have exactly this text; cite lines instead",
                    several.len()
                ),
            )
            .with_field("/selection/heading"));
        }
    };
    let (start, level, _) = headings.get(position).copied().ok_or_else(not_found)?;
    let end = headings
        .iter()
        .skip(position.saturating_add(1))
        .find(|(_, next_level, _)| *next_level <= level)
        .map_or(last, |(next, _, _)| next.saturating_sub(1));
    Ok(TextRange { start, end })
}

/// The level and text of an ATX heading line, without its closing `#` run.
fn atx_heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|byte| *byte == b'#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = line.get(level..)?;
    if !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim_end();
    Some((level, text))
}

/// The character and length of the fence run a line opens with: three or more backticks or
/// tildes.
fn fence_run(content: &str) -> Option<(u8, usize)> {
    let first = *content.as_bytes().first()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let len = content.bytes().take_while(|byte| *byte == first).count();
    (len >= 3).then_some((first, len))
}

fn not_found() -> ApiError {
    ApiError::new(
        ErrorCode::NotFound,
        "No object with that digest belongs to the cited item revision",
    )
}
