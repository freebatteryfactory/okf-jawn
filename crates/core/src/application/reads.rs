//! `read_item`: one item at one resolved revision, in the representation asked for, within a
//! visible budget (SPEC section 7); and the sandbox capability that serves a hostile
//! representation from the sandbox origin.
//!
//! A read serves committed content only. It never reads a draft: the store's document carries
//! none, and nothing here asks the draft store.
//!
//! - The text is the item's body: a note's Markdown, or a source card's shown text (the
//!   correction, the supplied text or the converter's text). Its outline is the converter's
//!   when the shown text is the converter's, and pulldown-cmark's otherwise
//!   (`reading::markdown_outline`), so a section resolves the same way for both
//!   (`reading::section_lines`).
//! - A selection resolves to lines of that text. Pages, cells and a region are located through
//!   the conversion record's line map, which describes only converter text; on other text they
//!   are refused, never guessed.
//! - The budget cuts the text at a line end when it can, marks the response truncated, and
//!   issues a cursor bound to the item, the resolved revision, the view, the selection and the
//!   byte position. A continuation reads that same revision whatever `latest` is now.
//! - The citation is exact: the selection asked for when the whole of it was returned, else
//!   the lines actually returned, with the digest and the server's locations
//!   (`reading::cited_locations`). Every read records a receipt of what it returned.

use std::collections::BTreeSet;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use okf_jawn_contract::common::{CellRange, PageRange, TextRange, Warning};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{ConversionOutcome, TextOrigin};
use okf_jawn_contract::identity::{At, ItemId, Revision, Timestamp};
use okf_jawn_contract::item::ItemDocument;
use okf_jawn_contract::read::{
    AssetRole, CreateSandboxCapabilityRequest, MediaReference, OutlineEntry, ReadItemRequest,
    ReadItemResponse, ReadView, SandboxCapability, Selection,
};
use okf_jawn_contract::source::{
    CoordOrigin, PageRegion, SourceLocation, SourceLocator, SourceReference,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use super::ApplicationService;
use super::shared::{check_named, invalid, record_receipt, resolve, workspace_scope};
use super::sources::{record_of, served_media_type};
use crate::context::OperationContext;
use crate::conversion::{ConversionRecord, RetainedAsset};
use crate::reading::{LineIndex, cited_locations, location_page, markdown_outline, section_lines};
use crate::sandbox::{SandboxMint, token_hash};
use crate::storage::StorageScope;

/// What one read serves from.
struct Material {
    /// The committed document; its body is the text served.
    document: ItemDocument,
    /// The conversion record the source card's extraction names.
    record: Option<ConversionRecord>,
    /// The shown text is the converter's, so the record's line map and outline describe it.
    converter_text: bool,
    /// Lines of the body.
    lines: LineIndex,
    /// The outline of the body.
    outline: Vec<OutlineEntry>,
}

/// Where a continued read resumes; opaque to the caller (base64url of this JSON).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadCursor {
    item_id: ItemId,
    revision: Revision,
    view: ReadView,
    selection: Selection,
    offset: usize,
}

/// The lines a selection covers, and what the reader should know about them.
struct Selected {
    /// `None` when the selection covers no text.
    lines: Option<TextRange>,
    warnings: Vec<Warning>,
}

/// One budgeted block of the selected text.
#[derive(Default)]
struct Block {
    markdown: String,
    /// The lines the block touches; `None` for an empty block.
    lines: Option<TextRange>,
    /// Byte position of the next block within the selected text, when more remains.
    next: Option<usize>,
    /// The block is not the whole selected text.
    partial: bool,
}

/// The pages whose images a selection takes.
enum PageScope {
    /// Every page.
    Every,
    /// One inclusive range.
    Range(u32, u32),
    /// Exactly these pages (possibly none).
    Set(BTreeSet<u32>),
}

impl PageScope {
    /// Whether an image on `page` (unlocated: `None`) is in scope; only `Every` takes an
    /// unlocated image.
    fn covers(&self, page: Option<u32>) -> bool {
        match self {
            Self::Every => true,
            Self::Range(first, last) => page.is_some_and(|page| page >= *first && page <= *last),
            Self::Set(pages) => page.is_some_and(|page| pages.contains(&page)),
        }
    }
}

/// Read an item in one representation, within its budget, and record what was returned.
///
/// # Errors
/// Returns `InvalidInput` on `/cursor` for a cursor that does not continue this read, on
/// `/view` for an original or pages of an item with no original, on `/selection/...` for a
/// selection the text does not have; the typed `NotFound` of an invalidated revision; or any
/// port error.
pub(super) async fn read_item(
    service: &ApplicationService,
    context: &OperationContext,
    request: ReadItemRequest,
) -> Result<ReadItemResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let (revision, offset) = start(service, &scope, &request).await?;
    let material = load(service, &scope, &revision, request.item_id).await?;
    if matches!(request.view, ReadView::Original | ReadView::Pages)
        && material.document.source.is_none()
    {
        return Err(invalid(
            "this item has no original and no rendered pages; read its text",
            "/view",
        ));
    }
    let Selected {
        lines: span,
        mut warnings,
    } = select(&material, &request.selection)?;
    warnings.extend(extraction_warnings(&material));
    let block = if matches!(request.view, ReadView::Text | ReadView::Multimodal) {
        budget(&material, span.as_ref(), offset, request.max_bytes)?
    } else if offset == 0 {
        Block::default()
    } else {
        return Err(bad_cursor());
    };
    let cited = match (&block.lines, block.partial) {
        (Some(lines), true) => Selection::Lines {
            range: lines.clone(),
        },
        _ => request.selection.clone(),
    };
    let located = match (&cited, &span) {
        (Selection::Section { .. }, Some(lines)) => Selection::Lines {
            range: lines.clone(),
        },
        _ => cited.clone(),
    };
    let source = citation(&scope, &revision, &material, &request.view, cited, &located);
    let scope_pages = page_scope(&material, &request.selection, span.as_ref());
    // Images go with the first block of a read; a continuation returns only more text.
    let media = if offset == 0 {
        media(
            &scope,
            &revision,
            &material,
            &request.view,
            &scope_pages,
            request.max_images,
            &mut warnings,
        )
    } else {
        Vec::new()
    };
    let next_cursor = block
        .next
        .map(|offset| {
            encode_cursor(&ReadCursor {
                item_id: request.item_id,
                revision: revision.clone(),
                view: request.view.clone(),
                selection: request.selection.clone(),
                offset,
            })
        })
        .transpose()?;
    let mut returned = vec![source.clone()];
    returned.extend(media.iter().map(|reference| reference.source.clone()));
    let receipt_id = record_receipt(service, context, &scope, None, returned).await?;
    Ok(ReadItemResponse {
        source,
        view: request.view,
        markdown: block.markdown,
        outline: material.outline,
        media,
        warnings,
        truncated: next_cursor.is_some(),
        next_cursor,
        receipt_id,
        extraction: material
            .document
            .source
            .as_ref()
            .map(|source| source.extraction.summary()),
    })
}

/// Mint a short-lived capability on the sandbox origin for one object of one item revision.
///
/// Only the hash of the token reaches the store; the token itself appears only in the URL.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated revision, `NotFound` for an object that does
/// not belong to the item revision, or any port error.
pub(super) async fn create_sandbox_capability(
    service: &ApplicationService,
    context: &OperationContext,
    request: CreateSandboxCapabilityRequest,
) -> Result<SandboxCapability, ApiError> {
    let scope = workspace_scope(context)?;
    check_named(service, &scope, &request.revision).await?;
    let media_type = served_media_type(
        service,
        &scope,
        request.item_id,
        &request.revision,
        &request.object,
    )
    .await?;
    let config = service.config();
    let expires_at = expiry(service.now(), config.sandbox_ttl)?;
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    service
        .ports()
        .sandbox
        .mint(
            &scope,
            token_hash(&token),
            SandboxMint {
                item_id: request.item_id,
                revision: request.revision,
                object: request.object,
                media_type,
                expires_at: expires_at.clone(),
            },
        )
        .await?;
    Ok(SandboxCapability {
        url: format!("{}/sandbox/{token}", config.sandbox_origin),
        expires_at,
    })
}

/// The revision a read resolves, and where in the selected text it starts.
async fn start(
    service: &ApplicationService,
    scope: &StorageScope,
    request: &ReadItemRequest,
) -> Result<(Revision, usize), ApiError> {
    let Some(text) = request.cursor.as_deref() else {
        return Ok((resolve(service, scope, &request.at).await?, 0));
    };
    let cursor: ReadCursor = URL_SAFE_NO_PAD
        .decode(text)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(bad_cursor)?;
    let pinned_elsewhere =
        matches!(&request.at, At::Revision { revision } if revision != &cursor.revision);
    if cursor.item_id != request.item_id
        || cursor.view != request.view
        || cursor.selection != request.selection
        || pinned_elsewhere
    {
        return Err(bad_cursor());
    }
    check_named(service, scope, &cursor.revision).await?;
    Ok((cursor.revision, cursor.offset))
}

/// Read the committed document and, for a source card, the record its extraction names.
async fn load(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
    item: ItemId,
) -> Result<Material, ApiError> {
    let document = service.ports().versions.show(scope, revision, item).await?;
    let record = record_of(service, scope, document.source.as_ref()).await?;
    let converter_text = record.is_some()
        && document.source.as_ref().is_some_and(|source| {
            source.extraction.text_origin == TextOrigin::Converter && !source.extraction.corrected
        });
    let outline = match (&record, converter_text) {
        (Some(record), true) => record.outline.clone(),
        _ => markdown_outline(&document.body),
    };
    Ok(Material {
        lines: LineIndex::new(&document.body),
        document,
        record,
        converter_text,
        outline,
    })
}

/// The lines a selection covers in the material's text.
fn select(material: &Material, selection: &Selection) -> Result<Selected, ApiError> {
    let count = material.lines.count();
    match selection {
        Selection::All => Ok(Selected {
            lines: (count > 0).then_some(TextRange {
                start: 1,
                end: count,
            }),
            warnings: Vec::new(),
        }),
        Selection::Lines { range } => {
            if range.end < range.start {
                return Err(invalid(
                    "the last line comes before the first",
                    "/selection/range",
                ));
            }
            if range.start > count {
                return Err(invalid(
                    format!("the text has {count} lines"),
                    "/selection/range/start",
                ));
            }
            let end = range.end.min(count);
            let warnings = if end < range.end {
                vec![warning(
                    "selection_clamped",
                    format!("the text ends at line {count}"),
                )]
            } else {
                Vec::new()
            };
            Ok(Selected {
                lines: Some(TextRange {
                    start: range.start,
                    end,
                }),
                warnings,
            })
        }
        Selection::Section { heading } => Ok(Selected {
            lines: Some(section_lines(&material.outline, heading)?),
            warnings: Vec::new(),
        }),
        Selection::Pages { range } => {
            if range.end < range.start {
                return Err(invalid(
                    "the last page comes before the first",
                    "/selection/range",
                ));
            }
            located(material, |location| {
                location_page(location).is_some_and(|page| page >= range.start && page <= range.end)
            })
        }
        Selection::Cells { range } => located(material, |location| cells_overlap(range, location)),
        Selection::Region { region } => {
            located(material, |location| region_holds(region, location))
        }
    }
}

/// The lines whose recorded location `wanted` accepts, from the first to the last of them.
fn located(
    material: &Material,
    wanted: impl Fn(&SourceLocation) -> bool,
) -> Result<Selected, ApiError> {
    let Some(record) = material.record.as_ref().filter(|_| material.converter_text) else {
        return Err(invalid(
            "only converter text has located lines; select lines or a section",
            "/selection",
        ));
    };
    let mut matched = record
        .locations
        .iter()
        .filter(|entry| wanted(&entry.location));
    let Some(first) = matched.next() else {
        return Ok(Selected {
            lines: None,
            warnings: vec![warning(
                "nothing_located",
                "no converted text lies in the selection",
            )],
        });
    };
    let (start, end) = matched.fold(
        (first.lines.start, first.lines.end),
        |(start, end), entry| (start.min(entry.lines.start), end.max(entry.lines.end)),
    );
    Ok(Selected {
        lines: Some(TextRange {
            start,
            end: end.min(material.lines.count()).max(start),
        }),
        warnings: Vec::new(),
    })
}

/// Cut the selected text at `offset` to at most `max_bytes`, at a line end when one fits.
fn budget(
    material: &Material,
    span: Option<&TextRange>,
    offset: usize,
    max_bytes: u32,
) -> Result<Block, ApiError> {
    let Some(range) = span else {
        return if offset == 0 {
            Ok(Block::default())
        } else {
            Err(bad_cursor())
        };
    };
    let bytes = material.lines.span(range).ok_or_else(|| {
        ApiError::new(
            ErrorCode::Internal,
            "a resolved selection lies outside the text",
        )
    })?;
    let selected =
        material.document.body.get(bytes.clone()).ok_or_else(|| {
            ApiError::new(ErrorCode::Internal, "a line does not start a character")
        })?;
    let rest = selected.get(offset..).ok_or_else(bad_cursor)?;
    let max = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    let mut cut = if rest.len() <= max {
        rest.len()
    } else {
        let boundary = rest.floor_char_boundary(max);
        rest.get(..boundary)
            .and_then(|head| head.rfind('\n'))
            .map_or(boundary, |at| at.saturating_add(1))
    };
    if cut == 0 {
        // Always make progress: at least one character, whatever the budget.
        cut = rest.chars().next().map_or(0, char::len_utf8);
    }
    let markdown = rest.get(..cut).ok_or_else(bad_cursor)?.to_owned();
    let first = bytes.start.saturating_add(offset);
    let last = first.saturating_add(cut).saturating_sub(1);
    let next = offset.saturating_add(cut);
    Ok(Block {
        lines: (cut > 0).then(|| TextRange {
            start: material.lines.line_of(first),
            end: material.lines.line_of(last),
        }),
        markdown,
        next: (next < selected.len()).then_some(next),
        partial: offset > 0 || next < selected.len(),
    })
}

/// The exact citation of what the read returned.
fn citation(
    scope: &StorageScope,
    revision: &Revision,
    material: &Material,
    view: &ReadView,
    selection: Selection,
    located: &Selection,
) -> SourceReference {
    let source = material.document.source.as_ref();
    let original = matches!(view, ReadView::Original);
    let digest = if original {
        source.map(|source| source.object.clone())
    } else {
        source.and_then(|source| source.extraction.digest.clone())
    };
    let locations = match source {
        Some(source) if !original => {
            cited_locations(&source.extraction, material.record.as_ref(), located)
        }
        _ => Vec::new(),
    };
    SourceReference {
        workspace_id: scope.workspace_id,
        item_id: material.document.summary.id,
        path: material.document.summary.path.clone(),
        revision: revision.clone(),
        digest,
        selection,
        locations,
    }
}

/// The pages whose images the selection takes.
fn page_scope(material: &Material, selection: &Selection, span: Option<&TextRange>) -> PageScope {
    match selection {
        Selection::All => PageScope::Every,
        Selection::Pages { range } => PageScope::Range(range.start, range.end),
        Selection::Region { region } => PageScope::Range(region.page_no, region.page_no),
        Selection::Cells { .. } => PageScope::Set(BTreeSet::new()),
        Selection::Lines { .. } | Selection::Section { .. } => {
            let pages = match (material.record.as_ref(), span, material.converter_text) {
                (Some(record), Some(span), true) => record
                    .locations
                    .iter()
                    .filter(|entry| entry.lines.start <= span.end && entry.lines.end >= span.start)
                    .filter_map(|entry| location_page(&entry.location))
                    .collect(),
                _ => BTreeSet::new(),
            };
            PageScope::Set(pages)
        }
    }
}

/// The images a multimodal or pages read returns: pictures, or page renders, on the selected
/// pages, at most `max_images`, each with its own citation.
fn media(
    scope: &StorageScope,
    revision: &Revision,
    material: &Material,
    view: &ReadView,
    pages: &PageScope,
    max_images: u16,
    warnings: &mut Vec<Warning>,
) -> Vec<MediaReference> {
    let role = match view {
        ReadView::Multimodal => AssetRole::Picture,
        ReadView::Pages => AssetRole::PageImage,
        ReadView::Outline | ReadView::Text | ReadView::Original => return Vec::new(),
    };
    let chosen: Vec<&RetainedAsset> = material
        .record
        .as_ref()
        .map(|record| {
            record
                .assets
                .iter()
                .filter(|asset| asset.role == role && pages.covers(location_page(&asset.location)))
                .collect()
        })
        .unwrap_or_default();
    if chosen.is_empty() && role == AssetRole::PageImage {
        warnings.push(warning(
            "no_page_images",
            "no rendered page of the selection is retained",
        ));
    }
    let mut returned = Vec::new();
    let mut omitted = 0_u32;
    for asset in chosen {
        let Some(size) = asset.pixel_size.as_ref() else {
            warnings.push(warning(
                "image_size_unknown",
                format!("image {} has no recorded size", asset.digest.as_str()),
            ));
            continue;
        };
        if returned.len() >= usize::from(max_images) {
            omitted = omitted.saturating_add(1);
            continue;
        }
        let page = location_page(&asset.location);
        returned.push(MediaReference {
            object: asset.digest.clone(),
            media_type: asset.media_type.clone(),
            role: asset.role,
            source: SourceReference {
                workspace_id: scope.workspace_id,
                item_id: material.document.summary.id,
                path: material.document.summary.path.clone(),
                revision: revision.clone(),
                digest: Some(asset.digest.clone()),
                selection: page.map_or(Selection::All, |page| Selection::Pages {
                    range: PageRange {
                        start: page,
                        end: page,
                    },
                }),
                locations: vec![asset.location.clone()],
            },
            caption: asset.caption.clone(),
            width: size.width,
            height: size.height,
        });
    }
    if omitted > 0 {
        warnings.push(warning(
            "images_omitted",
            format!("{omitted} more images lie in the selection; raise max_images or narrow it"),
        ));
    }
    returned
}

/// What the reader should know about a source card's text before trusting it.
fn extraction_warnings(material: &Material) -> Vec<Warning> {
    let Some(extraction) = material
        .document
        .source
        .as_ref()
        .map(|source| &source.extraction)
    else {
        return Vec::new();
    };
    let mut warnings = Vec::new();
    if matches!(extraction.outcome, ConversionOutcome::Partial { .. }) {
        warnings.push(warning(
            "partial_extraction",
            "part of this source was not converted; its text is incomplete",
        ));
    }
    if extraction.text_origin == TextOrigin::None {
        warnings.push(warning(
            "no_text",
            "this source has no text: its conversion is pending, failed or unsupported",
        ));
    }
    warnings
}

/// Whether a recorded location is spreadsheet cells overlapping `wanted` on the same sheet.
fn cells_overlap(wanted: &CellRange, location: &SourceLocation) -> bool {
    let (SourceLocation::Direct {
        locator: SourceLocator::Cells { range },
    }
    | SourceLocation::Inferred {
        locator: SourceLocator::Cells { range },
    }) = location
    else {
        return false;
    };
    range.sheet == wanted.sheet
        && range.row_start <= wanted.row_end
        && range.row_end >= wanted.row_start
        && range.column_start <= wanted.column_end
        && range.column_end >= wanted.column_start
}

/// Whether a recorded location is a box on the same page lying inside `outer`.
fn region_holds(outer: &PageRegion, location: &SourceLocation) -> bool {
    let (SourceLocation::Direct {
        locator: SourceLocator::Region { region },
    }
    | SourceLocation::Inferred {
        locator: SourceLocator::Region { region },
    }) = location
    else {
        return false;
    };
    if region.page_no != outer.page_no {
        return false;
    }
    let (inner_left, inner_top, inner_right, inner_bottom) = top_left_box(region);
    let (left, top, right, bottom) = top_left_box(outer);
    inner_left >= left && inner_right <= right && inner_top >= top && inner_bottom <= bottom
}

/// A region's box as (left, top, right, bottom) measured from the page's top-left corner.
fn top_left_box(region: &PageRegion) -> (f64, f64, f64, f64) {
    let bbox = &region.bbox;
    let (first, second) = match bbox.coord_origin {
        CoordOrigin::TopLeft => (bbox.t, bbox.b),
        CoordOrigin::BottomLeft => (
            region.page_size.height - bbox.t,
            region.page_size.height - bbox.b,
        ),
    };
    (
        bbox.l.min(bbox.r),
        first.min(second),
        bbox.l.max(bbox.r),
        first.max(second),
    )
}

/// When a capability issued at `issued` with `ttl` stops resolving: exactly `issued + ttl`.
fn expiry(issued: OffsetDateTime, ttl: std::time::Duration) -> Result<Timestamp, ApiError> {
    let unrepresentable = || {
        ApiError::new(
            ErrorCode::Internal,
            "the sandbox expiry is not representable",
        )
    };
    let ttl = time::Duration::try_from(ttl).map_err(|_| unrepresentable())?;
    let instant = issued.checked_add(ttl).ok_or_else(unrepresentable)?;
    Timestamp::from_utc(instant).map_err(|_| unrepresentable())
}

fn encode_cursor(cursor: &ReadCursor) -> Result<String, ApiError> {
    serde_json::to_vec(cursor)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
}

fn bad_cursor() -> ApiError {
    invalid("the cursor does not continue this read", "/cursor")
}

fn warning(code: &str, message: impl Into<String>) -> Warning {
    Warning {
        code: code.to_owned(),
        message: message.into(),
        location: None,
    }
}
