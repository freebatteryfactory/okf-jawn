//! Locate what a PDF conversion leaves without a location, through the page text layer.
//!
//! The library pairs a caption with its picture or table by the caption's region box and
//! then drops the box, so the exported caption has `prov: []`. The library's own text-layer
//! document (`docling::pdf_text_layer_pages`) has one located text item per line of the page,
//! so an unlocated item whose text is one such line can be found again by its text. This is
//! the rule of `qualification/docling/src/locate.rs`, which the Docling qualification runs on
//! the corpus, carried into the product:
//!
//! 1. Where to look. The pages of the item's parent, when the parent is itself located (a
//!    caption's picture or table). Otherwise (the parent is the body or a group) the last page
//!    of the nearest earlier sibling, in the parent's order of children, that the export
//!    located.
//! 2. What counts. The item's text must equal the text of a text-layer item on those pages,
//!    character for character, and must do so exactly once. The item then takes that page and
//!    that box, provided the box has area and lies inside the page.
//! 3. Otherwise the item stays unlocated, and says why. A box is never guessed, and never the
//!    parent's.
//!
//! A location found this way is `SourceLocation::Inferred`, a different variant from the
//! converter's own `Direct` location, so no consumer can show it with the certainty of a direct
//! one. Owner decision O1 (construction plan section 6): the product applies no distance bound.
//! Each lookup still records `distance`, how far the box found lies from the box of the item
//! whose page was searched, as a measurement. `LOCATE_LIMITS` states what the rule does not
//! guarantee.

use std::collections::HashMap;

use okf_jawn_contract::extraction::ExtractionWarning;
use okf_jawn_contract::source::{
    PageRegion, PageSize, SourceLocation, SourceLocator, UnresolvedReason,
};
use serde_json::Value;

use crate::export::{
    ItemKind, body_items, bounding_box, page_no, page_size, pages_of, provenance, region, resolve,
};

/// How the pages of a lookup were chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupBasis {
    /// The pages of the item's located parent.
    Parent,
    /// The last page of the nearest located earlier sibling.
    EarlierSibling,
    /// Neither: there was no page to search.
    None,
}

/// Where one item of the converted document is, and which source said so.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemLocation {
    /// The item's `self_ref`.
    pub item: String,
    /// Text, table or picture.
    pub kind: ItemKind,
    /// The location and its provenance: `Direct` from the export, `Inferred` from the text
    /// layer, `Unresolved` with the reason otherwise.
    pub location: SourceLocation,
    /// What the text-layer lookup did; `None` for an item the export located, and for a table
    /// or picture, which has no text to look up.
    pub lookup: Option<Lookup>,
}

/// The locations of every item of one converted PDF.
#[derive(Debug, Clone, PartialEq)]
pub struct Locations {
    /// One entry per text item, table and picture, in export order.
    pub items: Vec<ItemLocation>,
    /// How many located lines the text layer held, or the library's error when it was not read.
    pub text_layer: Result<usize, String>,
}

/// What the text-layer lookup did for one item the export left unlocated.
#[derive(Debug, Clone, PartialEq)]
pub struct Lookup {
    /// How the pages were chosen.
    pub basis: LookupBasis,
    /// The shortest distance, in page units, between the box found and the box `reference`
    /// has on the same page; 0 when they touch or overlap. A measurement: no bound is applied
    /// (O1). `None` when the item is not located or `reference` has no box on that page.
    pub distance: Option<f64>,
    /// How many text-layer items on those pages have exactly the item's text.
    pub occurrences: u32,
    /// The pages that were searched.
    pub pages: Vec<u32>,
    /// The `self_ref` of the item whose pages were searched; `None` when there was none.
    pub reference: Option<String>,
}

/// One located line of the text layer.
struct Line<'a> {
    bbox: &'a Value,
    page_no: u32,
    text: &'a str,
}

/// What the rule does not guarantee; the same sentence as the qualification's `LOCATE_LIMITS`.
pub const LOCATE_LIMITS: &str = "The rule does not guarantee that the box is the item's own. An item under the body that starts a page is searched on the page of its earlier sibling, so the same words standing once on that earlier page (a running footer) are taken for it; and when the page sets the item's text differently (a caption continued as 'Figure 1 (continued)') while another line of that page is exactly the item's text, that line is taken. Nothing holds the box found to the box of the parent: for each item the lookup records the distance between the box found and the box of the item whose page was searched (lookup.distance, in page units, 0 when they touch or overlap) as a measurement, and no threshold is applied";

/// The rule, in words.
pub const LOCATE_RULE: &str = "an item the export gives no provenance is looked up in the library's text-layer document (docling::pdf_text_layer_pages): on the pages of its parent when the parent is located, otherwise on the last page of the nearest earlier sibling the export located; it is located only when exactly one text-layer item on those pages has exactly its text and that item's box has area and lies inside the page; otherwise it stays unlocated";

/// The export rounds coordinates to two decimals (docling-core `json.rs`).
const ROUNDING: f64 = 0.011;

impl Locations {
    /// The page an item is located on, by whichever source located it.
    #[must_use]
    pub fn page_of(&self, item: &str) -> Option<u32> {
        self.items
            .iter()
            .find(|location| location.item == item)
            .and_then(|location| location_page(&location.location))
    }

    /// The location of one item, by `self_ref`.
    #[must_use]
    pub fn location_of(&self, item: &str) -> Option<&SourceLocation> {
        self.items
            .iter()
            .find(|location| location.item == item)
            .map(|location| &location.location)
    }

    /// The warnings a conversion records about locations: how many items the text layer
    /// located, and how many have no location.
    #[must_use]
    pub fn warnings(&self) -> Vec<ExtractionWarning> {
        let count = |wanted: fn(&SourceLocation) -> bool| {
            u32::try_from(
                self.items
                    .iter()
                    .filter(|location| wanted(&location.location))
                    .count(),
            )
            .unwrap_or(u32::MAX)
        };
        let inferred = count(|location| matches!(location, SourceLocation::Inferred { .. }));
        let unlocated = count(|location| matches!(location, SourceLocation::Unresolved { .. }));
        let mut warnings = Vec::new();
        if inferred > 0 {
            warnings.push(ExtractionWarning::InferredLocations { items: inferred });
        }
        if unlocated > 0 {
            warnings.push(ExtractionWarning::UnlocatedItems { items: unlocated });
        }
        warnings
    }
}

/// Where every text item, table and picture of a converted PDF is.
///
/// `export` is the converted document's JSON export. `text_layer` is the JSON export of what
/// `docling::pdf_text_layer_pages` returned for the same bytes, or the library's error text.
/// An item with provenance is located by the export and is not looked up.
#[must_use]
pub fn locate_items(export: &Value, text_layer: Result<&Value, &str>) -> Locations {
    let lines = text_layer.map(text_layer_lines);
    let read = match &lines {
        Ok(lines) => Ok(lines.len()),
        Err(error) => Err((*error).to_owned()),
    };
    let lines = lines.as_deref().map_err(|error| *error);
    let items = body_items(export)
        .map(|(kind, item)| {
            let reference = item
                .get("self_ref")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let (location, lookup) = if let Some(first) = provenance(item).first() {
                (direct(export, first), None)
            } else if kind == ItemKind::Text {
                let (location, lookup) = look_up(export, item, lines);
                (location, Some(lookup))
            } else {
                (unresolved(UnresolvedReason::NoTextToMatch), None)
            };
            ItemLocation {
                item: reference,
                kind,
                location,
                lookup,
            }
        })
        .collect();
    Locations {
        items,
        text_layer: read,
    }
}

/// The page a location names, when it names one.
#[must_use]
pub fn location_page(location: &SourceLocation) -> Option<u32> {
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

/// The shortest distance between two boxes of one page of this height, in page units and
/// rounded as the export rounds; 0 when they touch or overlap. `None` when either is not a box.
#[must_use]
pub fn box_distance(first: &Value, second: &Value, height: f64) -> Option<f64> {
    let (left, bottom, right, top) = edges(first, height)?;
    let (other_left, other_bottom, other_right, other_top) = edges(second, height)?;
    let across = (other_left - right).max(left - other_right).max(0.0);
    let along = (other_bottom - top).max(bottom - other_top).max(0.0);
    Some((across.hypot(along) * 100.0).round() / 100.0)
}

/// Whether `bbox` locates a region of a page of this size: four numbers with a known origin,
/// inside the page, with area.
#[must_use]
pub fn is_page_region(bbox: &Value, size: &PageSize) -> bool {
    let Some(parsed) = bounding_box(bbox) else {
        return false;
    };
    let (low, high) = match parsed.coord_origin {
        okf_jawn_contract::source::CoordOrigin::BottomLeft => (parsed.b, parsed.t),
        okf_jawn_contract::source::CoordOrigin::TopLeft => (parsed.t, parsed.b),
    };
    let inside = parsed.l >= -ROUNDING
        && parsed.r <= size.width + ROUNDING
        && low >= -ROUNDING
        && high <= size.height + ROUNDING;
    inside && parsed.r - parsed.l > 0.0 && high - low > 0.0
}

/// The converter's own location of an item from its first provenance entry: the region when
/// its box and page size are known, the page otherwise.
fn direct(export: &Value, entry: &Value) -> SourceLocation {
    if let Some(region) = region(export, entry) {
        return SourceLocation::Direct {
            locator: SourceLocator::Region { region },
        };
    }
    match page_no(entry) {
        Some(page_no) => SourceLocation::Direct {
            locator: SourceLocator::Page { page_no },
        },
        None => unresolved(UnresolvedReason::NotLocatedByConverter),
    }
}

/// The left, bottom, right and top of a box, measured from the bottom-left corner of a page
/// of this height.
fn edges(bbox: &Value, height: f64) -> Option<(f64, f64, f64, f64)> {
    let parsed = bounding_box(bbox)?;
    match parsed.coord_origin {
        okf_jawn_contract::source::CoordOrigin::BottomLeft => {
            Some((parsed.l, parsed.b, parsed.r, parsed.t))
        }
        okf_jawn_contract::source::CoordOrigin::TopLeft => {
            Some((parsed.l, height - parsed.b, parsed.r, height - parsed.t))
        }
    }
}

/// How far the box found on `page` lies from the box `reference` has on that page.
fn distance_to(export: &Value, reference: Option<&Value>, page: u32, found: &Value) -> Option<f64> {
    let own = provenance(reference?)
        .iter()
        .find(|entry| page_no(entry) == Some(page))?
        .get("bbox")?;
    let size = page_size(export, page)?;
    box_distance(found, own, size.height)
}

/// Look one unlocated text item up in the text layer.
fn look_up(
    export: &Value,
    item: &Value,
    lines: Result<&[Line<'_>], &str>,
) -> (SourceLocation, Lookup) {
    let (basis, pages, searched) = lookup_pages(export, item);
    let reference = searched
        .and_then(|other| other.get("self_ref"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let lookup = |occurrences: u32, distance: Option<f64>| Lookup {
        basis,
        distance,
        occurrences,
        pages: pages.clone(),
        reference: reference.clone(),
    };
    let Ok(lines) = lines else {
        return (unresolved(UnresolvedReason::NoTextLayer), lookup(0, None));
    };
    let text = item.get("text").and_then(Value::as_str).unwrap_or_default();
    if text.trim().is_empty() {
        return (unresolved(UnresolvedReason::NoTextToMatch), lookup(0, None));
    }
    if pages.is_empty() {
        return (
            unresolved(UnresolvedReason::NoPageToSearch),
            lookup(0, None),
        );
    }
    let matches: Vec<&Line<'_>> = lines
        .iter()
        .filter(|line| pages.contains(&line.page_no) && line.text == text)
        .collect();
    let occurrences = u32::try_from(matches.len()).unwrap_or(u32::MAX);
    let (Some(line), 1) = (matches.first(), matches.len()) else {
        let reason = if matches.is_empty() {
            UnresolvedReason::NoMatch
        } else {
            UnresolvedReason::AmbiguousMatch { occurrences }
        };
        return (unresolved(reason), lookup(occurrences, None));
    };
    let found = page_size(export, line.page_no)
        .filter(|size| is_page_region(line.bbox, size))
        .zip(bounding_box(line.bbox));
    let Some((size, bbox)) = found else {
        return (
            unresolved(UnresolvedReason::MatchNotALocation),
            lookup(occurrences, None),
        );
    };
    let distance = distance_to(export, searched, line.page_no, line.bbox);
    let location = SourceLocation::Inferred {
        locator: SourceLocator::Region {
            region: PageRegion {
                page_no: line.page_no,
                bbox,
                page_size: size,
            },
        },
    };
    (location, lookup(occurrences, distance))
}

/// The pages to search for an unlocated item, how they were chosen, and the item they are the
/// pages of: the parent, or the earlier sibling.
fn lookup_pages<'a>(export: &'a Value, item: &Value) -> (LookupBasis, Vec<u32>, Option<&'a Value>) {
    let parent = item
        .get("parent")
        .and_then(|parent| parent.get("$ref"))
        .and_then(Value::as_str)
        .and_then(|reference| resolve(export, reference));
    let Some(parent) = parent else {
        return (LookupBasis::None, Vec::new(), None);
    };
    let own = pages_of(parent);
    if !own.is_empty() {
        return (LookupBasis::Parent, own, Some(parent));
    }
    let reference = item.get("self_ref").and_then(Value::as_str);
    let children: Vec<&str> = parent
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|child| child.get("$ref").and_then(Value::as_str))
        .collect();
    let earlier = children
        .iter()
        .position(|child| Some(*child) == reference)
        .and_then(|at| children.get(..at))
        .unwrap_or_default();
    let found = earlier
        .iter()
        .rev()
        .filter_map(|sibling| resolve(export, sibling))
        .find_map(|sibling| pages_of(sibling).last().map(|page| (*page, sibling)));
    match found {
        Some((page, sibling)) => (LookupBasis::EarlierSibling, vec![page], Some(sibling)),
        None => (LookupBasis::None, Vec::new(), None),
    }
}

/// The located lines of a text-layer document, keyed by nothing: in document order.
fn text_layer_lines(text_layer: &Value) -> Vec<Line<'_>> {
    text_layer
        .get("texts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let entry = provenance(item).first()?;
            Some(Line {
                bbox: entry.get("bbox")?,
                page_no: page_no(entry)?,
                text: item.get("text")?.as_str()?,
            })
        })
        .collect()
}

/// An unresolved location with its reason.
const fn unresolved(reason: UnresolvedReason) -> SourceLocation {
    SourceLocation::Unresolved { reason }
}

/// The page each item is on, by `self_ref`, for the glyph rule.
#[must_use]
pub fn pages_by_item(locations: &Locations) -> HashMap<String, u32> {
    locations
        .items
        .iter()
        .filter_map(|location| {
            location_page(&location.location).map(|page| (location.item.clone(), page))
        })
        .collect()
}
