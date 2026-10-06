//! Locate what a PDF conversion leaves without a location, through the page text layer.
//!
//! The library pairs a caption with its picture or table by the caption's region box and then
//! drops the box, so the exported caption has `prov: []` (docling-pdf 1.93.6 `assemble.rs`
//! 2998-3000 and 3096-3098; a code block's caption is pushed unlocated at 3232-3236). The
//! library's own text-layer document (`docling::pdf_text_layer_pages`) has one located text
//! item per line of the page, so an unlocated item whose text is one such line can be found
//! again by its text.
//!
//! The rule, for a text item the export gives no provenance:
//!
//! 1. Where to look. The pages of its parent, when the parent is itself located (a caption's
//!    picture or table). Otherwise (the parent is the body or a group) the last page of the
//!    nearest earlier sibling, in the parent's order of children, that the export located.
//! 2. What counts. The item's text must equal the text of a text-layer item on those pages,
//!    character for character, and must do so exactly once. The item then takes that page and
//!    that box, provided the box has area and lies inside the page.
//! 3. Otherwise the item stays unlocated, and says why. A box is never guessed.
//!
//! What the rule cannot find: text that wraps over several text-layer lines; text that occurs
//! twice on the page; an item on a page that has no text layer (a scan read by OCR); an item
//! that starts a page when it hangs under the body (its earlier sibling is on the page
//! before); an item with no located earlier sibling. A table or a picture has no text to look
//! up and stays unlocated when the export does not locate it.
//!
//! What the rule does not guarantee is that the box it gives is the item's own. Two cases put
//! an item in the wrong place and pass, and the receipt says so (`LOCATE_LIMITS`): an item
//! under the body that starts a page is searched on its earlier sibling's page, so the same
//! words standing once on that earlier page (a running footer) are taken for it; and when the
//! page sets the item's text differently (a caption continued as 'Figure 1 (continued)') while
//! another line of that page is exactly the item's text, that line is taken. The page is chosen
//! by the order of the siblings, not by where the item is, and nothing holds the box found to
//! the box of the parent. So each lookup records `distance`: how far the box found lies from
//! the box of the item whose page was searched (`reference`). It is a measurement with no
//! threshold; a rule that uses it (a box near the parent, or below the earlier sibling) is for
//! whoever carries this module into the product to decide.
//!
//! Everything here is a pure function over the JSON exports, so it is tested without models.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

/// Where one item of the converted document is, and which source said so.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct ItemLocation {
    /// The box on `page_no`, as the source gave it (`l`, `t`, `r`, `b`, `coord_origin`).
    pub(crate) bbox: Option<Value>,
    /// The item's `self_ref`.
    pub(crate) item: String,
    /// `text`, `table` or `picture`.
    pub(crate) kind: &'static str,
    /// The item's label in the export.
    pub(crate) label: String,
    /// Which source located the item.
    pub(crate) located_by: LocatedBy,
    /// What the text-layer lookup did; null for an item the export located.
    pub(crate) lookup: Option<Lookup>,
    /// The page the item is on; null when it is not located.
    pub(crate) page_no: Option<u64>,
}

/// Which source gave an item its page and box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocatedBy {
    /// The converted document's own provenance.
    Export,
    /// The library's text-layer document, by the rule of this module.
    TextLayer,
    /// Nothing: the item has no location.
    None,
}

/// The locations of every item of one converted PDF.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Locations {
    /// One entry per text item, table and picture, in export order.
    pub(crate) items: Vec<ItemLocation>,
    /// What the rule does not guarantee, in words.
    pub(crate) limits: &'static str,
    /// The rule in words.
    pub(crate) rule: &'static str,
    /// What was read of the text layer.
    pub(crate) text_layer: TextLayerRead,
}

/// What the text-layer lookup did for one item the export left unlocated.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Lookup {
    /// How the pages were chosen: `parent`, `earlier_sibling` or `none`.
    pub(crate) basis: &'static str,
    /// The shortest distance, in page units, between the box found and the box `reference`
    /// has on the same page; 0 when they touch or overlap. A measurement: no threshold is
    /// applied. Null when the item is not located or `reference` has no box on that page.
    pub(crate) distance: Option<f64>,
    /// How many text-layer items on those pages have exactly the item's text.
    pub(crate) occurrences: usize,
    /// The pages that were searched.
    pub(crate) pages: Vec<u64>,
    /// Why the item is not located; null when it is.
    pub(crate) reason: Option<String>,
    /// The `self_ref` of the item whose pages were searched: the parent, or the earlier
    /// sibling. Null when there was no page to search.
    pub(crate) reference: Option<String>,
}

/// What `docling::pdf_text_layer_pages` returned for the fixture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TextLayerRead {
    /// The library's error, when it returned one.
    pub(crate) error: Option<String>,
    /// Located text items in the text-layer document.
    pub(crate) items: usize,
    /// The entry point that was called.
    pub(crate) source: &'static str,
}

/// One located line of the text layer.
struct Line<'a> {
    bbox: &'a Value,
    page_no: u64,
    text: &'a str,
}

/// The item arrays of an export that are judged, with the kind each holds.
const ITEM_ARRAYS: [(&str, &str); 3] = [
    ("texts", "text"),
    ("tables", "table"),
    ("pictures", "picture"),
];

/// What the rule does not guarantee, as the receipt states it; `lib/document.mjs`
/// `TEXT_LAYER_LIMITS` is the same sentence and a test holds the two equal.
pub(crate) const LOCATE_LIMITS: &str = "The rule does not guarantee that the box is the item's own. An item under the body that starts a page is searched on the page of its earlier sibling, so the same words standing once on that earlier page (a running footer) are taken for it; and when the page sets the item's text differently (a caption continued as 'Figure 1 (continued)') while another line of that page is exactly the item's text, that line is taken. Nothing holds the box found to the box of the parent: for each item the lookup records the distance between the box found and the box of the item whose page was searched (lookup.distance, in page units, 0 when they touch or overlap) as a measurement, and no threshold is applied";

/// The rule, as the receipt states it.
pub(crate) const LOCATE_RULE: &str = "an item the export gives no provenance is looked up in the library's text-layer document (docling::pdf_text_layer_pages): on the pages of its parent when the parent is located, otherwise on the last page of the nearest earlier sibling the export located; it is located only when exactly one text-layer item on those pages has exactly its text and that item's box has area and lies inside the page; otherwise it stays unlocated";

/// The export rounds coordinates to two decimals (docling-core json.rs:667).
const ROUNDING: f64 = 0.011;

/// The entry point the text layer is read with.
pub(crate) const TEXT_LAYER_SOURCE: &str = "docling::pdf_text_layer_pages";

/// Every text item, table and picture of an export with its kind, in export order.
pub(crate) fn body_items(export: &Value) -> impl Iterator<Item = (&'static str, &Value)> {
    ITEM_ARRAYS.into_iter().flat_map(move |(array, kind)| {
        export
            .get(array)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(move |item| (kind, item))
    })
}

/// The value a `#/...` reference of the export points at.
fn resolve<'a>(export: &'a Value, reference: &str) -> Option<&'a Value> {
    let path = reference.strip_prefix("#/")?;
    match path.split_once('/') {
        Some((array, index)) => export
            .get(array)?
            .as_array()?
            .get(index.parse::<usize>().ok()?),
        None => export.get(path),
    }
}

/// The provenance entries of an exported item.
fn provenance(item: &Value) -> &[Value] {
    item.get("prov")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The pages an exported item's provenance names, ascending and each once.
fn pages_of(item: &Value) -> Vec<u64> {
    provenance(item)
        .iter()
        .filter_map(|entry| entry.get("page_no").and_then(Value::as_u64))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The pages to search for an unlocated item, how they were chosen, and the item they are
/// the pages of: the parent, or the earlier sibling.
fn lookup_pages<'a>(
    export: &'a Value,
    item: &Value,
) -> (&'static str, Vec<u64>, Option<&'a Value>) {
    let parent = item
        .get("parent")
        .and_then(|parent| parent.get("$ref"))
        .and_then(Value::as_str)
        .and_then(|reference| resolve(export, reference));
    let Some(parent) = parent else {
        return ("none", Vec::new(), None);
    };
    let own = pages_of(parent);
    if !own.is_empty() {
        return ("parent", own, Some(parent));
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
        .map_or(&[][..], |at| children.get(..at).unwrap_or_default());
    let found = earlier
        .iter()
        .rev()
        .filter_map(|sibling| resolve(export, sibling))
        .find_map(|sibling| pages_of(sibling).last().map(|page| (*page, sibling)));
    match found {
        Some((page, sibling)) => ("earlier_sibling", vec![page], Some(sibling)),
        None => ("none", Vec::new(), None),
    }
}

/// The left, bottom, right and top of a box, measured from the bottom-left corner of a page
/// of this height.
fn edges(bbox: &Value, height: f64) -> Option<(f64, f64, f64, f64)> {
    let side = |name: &str| {
        bbox.get(name)
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
    };
    let (left, top, right, bottom) = (side("l")?, side("t")?, side("r")?, side("b")?);
    match bbox.get("coord_origin").and_then(Value::as_str) {
        Some("BOTTOMLEFT") => Some((left, bottom, right, top)),
        Some("TOPLEFT") => Some((left, height - bottom, right, height - top)),
        _ => None,
    }
}

/// The shortest distance between two boxes of one page of this height, in page units and
/// rounded as the export rounds; 0 when they touch or overlap. `None` when either is not a
/// box.
pub(crate) fn box_distance(first: &Value, second: &Value, height: f64) -> Option<f64> {
    let (left, bottom, right, top) = edges(first, height)?;
    let (other_left, other_bottom, other_right, other_top) = edges(second, height)?;
    let across = (other_left - right).max(left - other_right).max(0.0);
    let along = (other_bottom - top).max(bottom - other_top).max(0.0);
    Some((across.hypot(along) * 100.0).round() / 100.0)
}

/// How far the box found on `page_no` lies from the box `reference` has on that page.
fn distance_to(
    export: &Value,
    reference: Option<&Value>,
    page_no: u64,
    found: &Value,
) -> Option<f64> {
    let own = provenance(reference?)
        .iter()
        .find(|entry| entry.get("page_no").and_then(Value::as_u64) == Some(page_no))?
        .get("bbox")?;
    let (_, height) = page_size(export, page_no)?;
    box_distance(found, own, height)
}

/// The size of a page of the converted document.
fn page_size(export: &Value, page_no: u64) -> Option<(f64, f64)> {
    let page = export
        .get("pages")?
        .as_object()?
        .values()
        .find(|page| page.get("page_no").and_then(Value::as_u64) == Some(page_no))?;
    let size = page.get("size")?;
    Some((size.get("width")?.as_f64()?, size.get("height")?.as_f64()?))
}

/// Why `bbox` does not locate a region of a page of this size, or `None` when it does.
pub(crate) fn bbox_problem(bbox: &Value, (width, height): (f64, f64)) -> Option<String> {
    let side = |name: &str| {
        bbox.get(name)
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
    };
    let (Some(left), Some(top), Some(right), Some(bottom)) =
        (side("l"), side("t"), side("r"), side("b"))
    else {
        return Some("the box is not four numbers".to_owned());
    };
    let (low, high) = match bbox.get("coord_origin").and_then(Value::as_str) {
        Some("BOTTOMLEFT") => (bottom, top),
        Some("TOPLEFT") => (top, bottom),
        other => return Some(format!("the box has the unknown coord_origin {other:?}")),
    };
    if left < -ROUNDING || right > width + ROUNDING || low < -ROUNDING || high > height + ROUNDING {
        return Some(format!("the box lies outside the {width} x {height} page"));
    }
    if right - left <= 0.0 || high - low <= 0.0 {
        return Some("the box has no area".to_owned());
    }
    None
}

/// The located lines of a text-layer document.
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
                page_no: entry.get("page_no")?.as_u64()?,
                text: item.get("text")?.as_str()?,
            })
        })
        .collect()
}

/// Look one unlocated text item up in the text layer.
fn look_up(
    export: &Value,
    item: &Value,
    lines: Result<&[Line<'_>], &str>,
) -> (Lookup, Option<(u64, Value)>) {
    let (basis, pages, searched) = lookup_pages(export, item);
    let reference = searched
        .and_then(|other| other.get("self_ref"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let unlocated = |occurrences: usize, reason: String| Lookup {
        basis,
        distance: None,
        occurrences,
        pages: pages.clone(),
        reason: Some(reason),
        reference: reference.clone(),
    };
    let lines = match lines {
        Ok(lines) => lines,
        Err(error) => {
            return (
                unlocated(0, format!("the text layer was not read: {error}")),
                None,
            );
        }
    };
    let text = item.get("text").and_then(Value::as_str).unwrap_or_default();
    if text.trim().is_empty() {
        return (
            unlocated(0, "the item has no text to look up".to_owned()),
            None,
        );
    }
    if pages.is_empty() {
        return (
            unlocated(
                0,
                "no page to search: neither its parent nor an earlier sibling is located"
                    .to_owned(),
            ),
            None,
        );
    }
    let matches: Vec<&Line<'_>> = lines
        .iter()
        .filter(|line| pages.contains(&line.page_no) && line.text == text)
        .collect();
    let (Some(line), 1) = (matches.first(), matches.len()) else {
        let said = if matches.is_empty() {
            format!("no text-layer item on page {pages:?} has exactly this text")
        } else {
            format!(
                "{} text-layer items on page {pages:?} have exactly this text",
                matches.len()
            )
        };
        return (unlocated(matches.len(), said), None);
    };
    let problem = match page_size(export, line.page_no) {
        Some(size) => bbox_problem(line.bbox, size),
        None => Some(format!(
            "page {} is not a page of the converted document",
            line.page_no
        )),
    };
    if let Some(problem) = problem {
        return (
            unlocated(1, format!("the one match is not a location: {problem}")),
            None,
        );
    }
    let found = Lookup {
        basis,
        distance: distance_to(export, searched, line.page_no, line.bbox),
        occurrences: 1,
        pages,
        reason: None,
        reference,
    };
    (found, Some((line.page_no, line.bbox.clone())))
}

/// Where every text item, table and picture of a converted PDF is.
///
/// `export` is the converted document's JSON export. `text_layer` is the JSON export of what
/// `docling::pdf_text_layer_pages` returned for the same bytes, or the library's error text.
/// An item with provenance is located by the export and is not looked up. Whether an exported
/// box is itself sound is judged by the orchestrator, as before.
pub(crate) fn locate_items(export: &Value, text_layer: Result<&Value, &str>) -> Locations {
    let lines = text_layer.map(text_layer_lines);
    let read = TextLayerRead {
        error: text_layer.err().map(str::to_owned),
        items: lines.as_ref().map_or(0, Vec::len),
        source: TEXT_LAYER_SOURCE,
    };
    let items = body_items(export)
        .map(|(kind, item)| {
            let text = |name: &str| {
                item.get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            let located = |located_by, lookup, place: Option<(u64, Value)>| {
                let (page_no, bbox) =
                    place.map_or((None, None), |(page, bbox)| (Some(page), Some(bbox)));
                ItemLocation {
                    bbox,
                    item: text("self_ref"),
                    kind,
                    label: text("label"),
                    located_by,
                    lookup,
                    page_no,
                }
            };
            if let Some(first) = provenance(item).first() {
                let place = first
                    .get("page_no")
                    .and_then(Value::as_u64)
                    .zip(first.get("bbox").cloned());
                return located(LocatedBy::Export, None, place);
            }
            if kind != "text" {
                return located(LocatedBy::None, None, None);
            }
            let (lookup, place) = look_up(export, item, lines.as_deref().map_err(|error| *error));
            let by = if place.is_some() {
                LocatedBy::TextLayer
            } else {
                LocatedBy::None
            };
            located(by, Some(lookup), place)
        })
        .collect();
    Locations {
        items,
        limits: LOCATE_LIMITS,
        rule: LOCATE_RULE,
        text_layer: read,
    }
}

#[cfg(test)]
mod tests {
    use super::{LOCATE_LIMITS, LocatedBy, Locations, bbox_problem, box_distance, locate_items};
    use serde_json::{Value, json};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The value of an option that must be present; `what` names it in the failure.
    fn some<T>(value: Option<T>, what: &str) -> Result<T, String> {
        value.ok_or_else(|| format!("missing {what}"))
    }

    fn prov(page: u64, left: f64, top: f64, right: f64, bottom: f64) -> Value {
        json!([{ "page_no": page, "bbox": { "l": left, "t": top, "r": right, "b": bottom, "coord_origin": "BOTTOMLEFT" } }])
    }

    /// A converted document: a picture and a table with captions on page 2, a code block on page 3
    /// followed by a caption that hangs under the body, and a located paragraph.
    fn export() -> Value {
        json!({
            "pages": {
                "2": { "page_no": 2, "size": { "width": 612.0, "height": 792.0 } },
                "3": { "page_no": 3, "size": { "width": 612.0, "height": 792.0 } },
            },
            "body": { "self_ref": "#/body", "children": [
                { "$ref": "#/texts/0" }, { "$ref": "#/pictures/0" }, { "$ref": "#/tables/0" },
                { "$ref": "#/texts/3" }, { "$ref": "#/texts/4" },
            ] },
            "texts": [
                { "self_ref": "#/texts/0", "label": "text", "text": "A paragraph.", "parent": { "$ref": "#/body" }, "prov": prov(2, 72.0, 700.0, 300.0, 690.0) },
                { "self_ref": "#/texts/1", "label": "caption", "text": "Figure 1-2   Existing controls", "parent": { "$ref": "#/pictures/0" }, "prov": [] },
                { "self_ref": "#/texts/2", "label": "caption", "text": "Table 2-1   FUNCTION_USAGE view", "parent": { "$ref": "#/tables/0" }, "prov": [] },
                { "self_ref": "#/texts/3", "label": "code", "text": "CREATE MASK", "parent": { "$ref": "#/body" }, "prov": prov(3, 72.0, 500.0, 400.0, 420.0) },
                { "self_ref": "#/texts/4", "label": "text", "text": "Example 3-9   Creating a mask", "parent": { "$ref": "#/body" }, "prov": [] },
            ],
            "tables": [{ "self_ref": "#/tables/0", "label": "table", "parent": { "$ref": "#/body" }, "prov": prov(2, 72.0, 400.0, 500.0, 300.0), "data": { "table_cells": [] } }],
            "pictures": [{ "self_ref": "#/pictures/0", "label": "picture", "parent": { "$ref": "#/body" }, "prov": prov(2, 72.0, 650.0, 500.0, 450.0) }],
        })
    }

    fn line(page: u64, text: &str, bbox: &Value) -> Value {
        json!({ "label": "text", "text": text, "prov": [{ "page_no": page, "bbox": bbox }] })
    }

    fn good_box() -> Value {
        json!({ "l": 136.27, "t": 100.55, "r": 316.76, "b": 91.27, "coord_origin": "BOTTOMLEFT" })
    }

    /// The text layer of the document above: each caption once, on its own page.
    fn text_layer() -> Value {
        json!({ "texts": [
            line(2, "A paragraph.", &good_box()),
            line(2, "Figure 1-2   Existing controls", &good_box()),
            line(2, "Table 2-1   FUNCTION_USAGE view", &json!({ "l": 136.27, "t": 512.02, "r": 284.48, "b": 504.28, "coord_origin": "BOTTOMLEFT" })),
            line(3, "Example 3-9   Creating a mask", &json!({ "l": 136.27, "t": 385.17, "r": 351.42, "b": 377.44, "coord_origin": "BOTTOMLEFT" })),
        ] })
    }

    fn by<'a>(locations: &'a Locations, item: &str) -> Result<&'a super::ItemLocation, String> {
        some(
            locations
                .items
                .iter()
                .find(|location| location.item == item),
            item,
        )
    }

    fn sources(locations: &Locations) -> Vec<(&str, LocatedBy)> {
        locations
            .items
            .iter()
            .map(|location| (location.item.as_str(), location.located_by))
            .collect()
    }

    #[test]
    fn an_unlocated_item_takes_the_page_and_box_of_the_one_line_with_its_text() -> TestResult {
        let (export, layer) = (export(), text_layer());
        let locations = locate_items(&export, Ok(&layer));
        assert_eq!(
            sources(&locations),
            [
                ("#/texts/0", LocatedBy::Export),
                ("#/texts/1", LocatedBy::TextLayer),
                ("#/texts/2", LocatedBy::TextLayer),
                ("#/texts/3", LocatedBy::Export),
                ("#/texts/4", LocatedBy::TextLayer),
                ("#/tables/0", LocatedBy::Export),
                ("#/pictures/0", LocatedBy::Export),
            ]
        );
        assert_eq!(locations.text_layer.items, 4);
        assert_eq!(locations.text_layer.error, None);

        // A caption is searched on its parent's page; an item under the body on the page of
        // the nearest earlier sibling the export located.
        let caption = by(&locations, "#/texts/2")?;
        let lookup = some(caption.lookup.as_ref(), "the lookup of the table caption")?;
        assert_eq!(
            (
                lookup.basis,
                lookup.pages.as_slice(),
                lookup.occurrences,
                lookup.reason.as_deref()
            ),
            ("parent", &[2][..], 1, None)
        );
        assert_eq!(caption.page_no, Some(2));
        let hanging = by(&locations, "#/texts/4")?;
        let lookup = some(hanging.lookup.as_ref(), "the lookup of the hanging caption")?;
        assert_eq!(
            (lookup.basis, lookup.pages.as_slice()),
            ("earlier_sibling", &[3][..])
        );
        assert_eq!(hanging.page_no, Some(3));

        // A located box lies inside the page and has area.
        for location in locations
            .items
            .iter()
            .filter(|location| location.located_by == LocatedBy::TextLayer)
        {
            let bbox = some(location.bbox.as_ref(), "a located box")?;
            assert_eq!(
                bbox_problem(bbox, (612.0, 792.0)),
                None,
                "{}",
                location.item
            );
            let side = |name: &str| some(bbox.get(name).and_then(Value::as_f64), name);
            assert!(side("r")? > side("l")? && side("t")? > side("b")?, "{bbox}");
            assert!(
                side("l")? >= 0.0
                    && side("r")? <= 612.0
                    && side("b")? >= 0.0
                    && side("t")? <= 792.0,
                "{bbox}"
            );
        }
        // An item the export located keeps the export's page and box and is not looked up.
        let paragraph = by(&locations, "#/texts/0")?;
        assert_eq!(paragraph.lookup, None);
        assert_eq!(
            paragraph.bbox,
            some(export.pointer("/texts/0/prov/0/bbox"), "the exported box")?
                .clone()
                .into()
        );
        Ok(())
    }

    #[test]
    fn text_that_occurs_twice_on_the_page_is_not_located() -> TestResult {
        let (export, mut layer) = (export(), text_layer());
        let lines = some(
            layer.get_mut("texts").and_then(Value::as_array_mut),
            "the lines",
        )?;
        lines.push(line(
            2,
            "Figure 1-2   Existing controls",
            &json!({ "l": 72.0, "t": 60.0, "r": 200.0, "b": 50.0, "coord_origin": "BOTTOMLEFT" }),
        ));
        let locations = locate_items(&export, Ok(&layer));
        let caption = by(&locations, "#/texts/1")?;
        assert_eq!(
            (caption.located_by, caption.page_no, caption.bbox.as_ref()),
            (LocatedBy::None, None, None)
        );
        let lookup = some(caption.lookup.as_ref(), "the lookup")?;
        assert_eq!(lookup.occurrences, 2);
        assert_eq!(
            lookup.reason.as_deref(),
            Some("2 text-layer items on page [2] have exactly this text")
        );
        // The other captions are still found.
        assert_eq!(
            by(&locations, "#/texts/2")?.located_by,
            LocatedBy::TextLayer
        );

        // The same text on another page is not a second occurrence: only the parent's page is searched.
        let mut elsewhere = text_layer();
        let lines = some(
            elsewhere.get_mut("texts").and_then(Value::as_array_mut),
            "the lines",
        )?;
        lines.push(line(3, "Figure 1-2   Existing controls", &good_box()));
        assert_eq!(
            by(&locate_items(&export, Ok(&elsewhere)), "#/texts/1")?.page_no,
            Some(2)
        );
        Ok(())
    }

    #[test]
    fn text_that_is_absent_from_the_page_is_not_located() -> TestResult {
        let export = export();
        // Absent altogether, present only on another page, and present only as part of a longer line.
        for layer in [
            json!({ "texts": [] }),
            json!({ "texts": [line(3, "Figure 1-2   Existing controls", &good_box())] }),
            json!({ "texts": [line(2, "Figure 1-2   Existing controls and more", &good_box()), line(2, "Figure 1-2", &good_box())] }),
            json!({ "texts": [line(2, "Figure 1-2 Existing controls", &good_box())] }),
        ] {
            let locations = locate_items(&export, Ok(&layer));
            let caption = by(&locations, "#/texts/1")?;
            assert_eq!(
                (caption.located_by, caption.page_no, caption.bbox.as_ref()),
                (LocatedBy::None, None, None),
                "{layer}"
            );
            let lookup = some(caption.lookup.as_ref(), "the lookup")?;
            assert_eq!(lookup.occurrences, 0);
            assert_eq!(
                lookup.reason.as_deref(),
                Some("no text-layer item on page [2] has exactly this text")
            );
        }
        // A text layer that could not be read locates nothing, and says so.
        let unread = locate_items(&export, Err("pdf: unreadable"));
        assert_eq!(unread.text_layer.error.as_deref(), Some("pdf: unreadable"));
        let caption = by(&unread, "#/texts/1")?;
        assert_eq!(caption.located_by, LocatedBy::None);
        assert_eq!(
            some(caption.lookup.as_ref(), "the lookup")?
                .reason
                .as_deref(),
            Some("the text layer was not read: pdf: unreadable")
        );
        assert_eq!(by(&unread, "#/texts/0")?.located_by, LocatedBy::Export);
        Ok(())
    }

    #[test]
    fn a_match_whose_box_is_not_a_region_of_the_page_is_not_a_location() -> TestResult {
        let export = export();
        for (bbox, reason) in [
            (
                json!({ "l": 136.0, "t": 900.0, "r": 316.0, "b": 880.0, "coord_origin": "BOTTOMLEFT" }),
                "the one match is not a location: the box lies outside the 612 x 792 page",
            ),
            (
                json!({ "l": 136.0, "t": 100.0, "r": 136.0, "b": 90.0, "coord_origin": "BOTTOMLEFT" }),
                "the one match is not a location: the box has no area",
            ),
            (
                json!({ "l": 136.0, "t": 90.0, "r": 316.0, "b": 100.0, "coord_origin": "BOTTOMLEFT" }),
                "the one match is not a location: the box has no area",
            ),
            (
                json!({ "l": 136.0, "t": 100.0, "r": 316.0, "coord_origin": "BOTTOMLEFT" }),
                "the one match is not a location: the box is not four numbers",
            ),
            (
                json!({ "l": 136.0, "t": 100.0, "r": 316.0, "b": 90.0 }),
                "the one match is not a location: the box has the unknown coord_origin None",
            ),
        ] {
            let layer = json!({ "texts": [line(2, "Figure 1-2   Existing controls", &bbox)] });
            let locations = locate_items(&export, Ok(&layer));
            let caption = by(&locations, "#/texts/1")?;
            assert_eq!(
                (caption.located_by, caption.page_no, caption.bbox.as_ref()),
                (LocatedBy::None, None, None)
            );
            assert_eq!(
                some(caption.lookup.as_ref(), "the lookup")?
                    .reason
                    .as_deref(),
                Some(reason)
            );
        }
        // A top-left box is read with its own origin.
        assert_eq!(
            bbox_problem(
                &json!({ "l": 10.0, "t": 20.0, "r": 30.0, "b": 40.0, "coord_origin": "TOPLEFT" }),
                (612.0, 792.0)
            ),
            None
        );
        Ok(())
    }

    #[test]
    fn an_item_with_nowhere_to_look_and_a_table_or_picture_stay_unlocated() -> TestResult {
        let mut export = export();
        // The hanging caption becomes the first child: no earlier sibling is located.
        let children = some(
            export
                .pointer_mut("/body/children")
                .and_then(Value::as_array_mut),
            "children",
        )?;
        children.rotate_right(1);
        // A table and a picture the export does not locate have no text to look up.
        for pointer in ["/tables/0/prov", "/pictures/0/prov"] {
            *some(export.pointer_mut(pointer), pointer)? = json!([]);
        }
        let layer = text_layer();
        let locations = locate_items(&export, Ok(&layer));
        let hanging = by(&locations, "#/texts/4")?;
        let lookup = some(hanging.lookup.as_ref(), "the lookup")?;
        assert_eq!(
            (hanging.located_by, lookup.basis, lookup.pages.len()),
            (LocatedBy::None, "none", 0)
        );
        assert_eq!(
            lookup.reason.as_deref(),
            Some("no page to search: neither its parent nor an earlier sibling is located")
        );
        for item in ["#/tables/0", "#/pictures/0"] {
            let location = by(&locations, item)?;
            assert_eq!(
                (
                    location.located_by,
                    location.lookup.as_ref(),
                    location.page_no
                ),
                (LocatedBy::None, None, None),
                "{item}"
            );
        }
        // Their captions now have an unlocated parent, and fall back to the earlier sibling of the parent's children: none.
        assert_eq!(by(&locations, "#/texts/1")?.located_by, LocatedBy::None);
        Ok(())
    }

    #[test]
    fn the_locations_serialize_with_the_three_source_words() -> TestResult {
        let (export, layer) = (export(), text_layer());
        let value = serde_json::to_value(locate_items(&export, Ok(&layer)))?;
        let words: Vec<&str> = some(value.get("items").and_then(Value::as_array), "items")?
            .iter()
            .filter_map(|item| item.get("located_by").and_then(Value::as_str))
            .collect();
        assert_eq!(
            words,
            [
                "export",
                "text_layer",
                "text_layer",
                "export",
                "text_layer",
                "export",
                "export"
            ]
        );
        assert_eq!(serde_json::to_value(LocatedBy::None)?, "none");
        assert_eq!(
            value.pointer("/text_layer/source"),
            Some(&json!("docling::pdf_text_layer_pages"))
        );
        Ok(())
    }

    #[test]
    fn each_lookup_records_how_far_the_box_found_lies_from_the_item_whose_page_was_searched()
    -> TestResult {
        let (export, layer) = (export(), text_layer());
        let locations = locate_items(&export, Ok(&layer));
        let measured = |item: &str| -> Result<(Value, Value, Value), Box<dyn std::error::Error>> {
            let lookup =
                serde_json::to_value(some(by(&locations, item)?.lookup.as_ref(), "the lookup")?)?;
            let field = |name: &str| some(lookup.get(name).cloned(), name);
            Ok((field("basis")?, field("reference")?, field("distance")?))
        };
        // The picture ends at 450 and its caption was found with its top at 100.55, below it.
        assert_eq!(
            measured("#/texts/1")?,
            (json!("parent"), json!("#/pictures/0"), json!(349.45))
        );
        // The table starts at 400 and its caption was found with its bottom at 504.28, above it.
        assert_eq!(
            measured("#/texts/2")?,
            (json!("parent"), json!("#/tables/0"), json!(104.28))
        );
        // An item under the body is measured against the earlier sibling whose page was searched.
        assert_eq!(
            measured("#/texts/4")?,
            (json!("earlier_sibling"), json!("#/texts/3"), json!(34.83))
        );

        // An item that is not located has no distance, and still says whose page was searched.
        let mut twice = text_layer();
        some(
            twice.get_mut("texts").and_then(Value::as_array_mut),
            "the lines",
        )?
        .push(line(2, "Figure 1-2   Existing controls", &good_box()));
        let unlocated = locate_items(&export, Ok(&twice));
        let lookup = some(by(&unlocated, "#/texts/1")?.lookup.as_ref(), "the lookup")?;
        assert_eq!(
            (lookup.distance, lookup.reference.as_deref()),
            (None, Some("#/pictures/0"))
        );
        // With no page to search there is nothing to measure against.
        let mut alone = export.clone();
        some(
            alone
                .pointer_mut("/body/children")
                .and_then(Value::as_array_mut),
            "children",
        )?
        .rotate_right(1);
        let lookup = locate_items(&alone, Ok(&layer));
        let lookup = some(by(&lookup, "#/texts/4")?.lookup.as_ref(), "the lookup")?;
        assert_eq!((lookup.distance, lookup.reference.as_deref()), (None, None));

        // The distance is a measurement: an item found far from its parent is located all the same.
        assert_eq!(
            by(&locations, "#/texts/1")?.located_by,
            LocatedBy::TextLayer
        );
        Ok(())
    }

    #[test]
    fn the_distance_between_two_boxes_is_the_gap_between_their_nearest_edges() -> TestResult {
        let bottom_left = |left: f64, top: f64, right: f64, bottom: f64| json!({ "l": left, "t": top, "r": right, "b": bottom, "coord_origin": "BOTTOMLEFT" });
        let distance = |first: &Value, second: &Value| {
            serde_json::to_value(box_distance(first, second, 792.0))
        };
        let anchor = bottom_left(100.0, 200.0, 300.0, 100.0);
        // Overlapping and touching boxes are at distance 0.
        assert_eq!(
            distance(&anchor, &bottom_left(150.0, 250.0, 250.0, 150.0))?,
            json!(0.0)
        );
        assert_eq!(
            distance(&anchor, &bottom_left(300.0, 200.0, 400.0, 100.0))?,
            json!(0.0)
        );
        // Straight below, straight to the right, and across a corner (3 and 4 make 5).
        assert_eq!(
            distance(&anchor, &bottom_left(100.0, 95.5, 300.0, 80.0))?,
            json!(4.5)
        );
        assert_eq!(
            distance(&anchor, &bottom_left(312.25, 200.0, 400.0, 100.0))?,
            json!(12.25)
        );
        assert_eq!(
            distance(&anchor, &bottom_left(303.0, 96.0, 400.0, 50.0))?,
            json!(5.0)
        );
        // The order of the two boxes does not matter.
        assert_eq!(
            distance(&bottom_left(303.0, 96.0, 400.0, 50.0), &anchor)?,
            json!(5.0)
        );
        // A top-left box is read from the top of the page: 592 to 692 down a 792 page is 100 to 200 up it.
        let top_left =
            json!({ "l": 100.0, "t": 592.0, "r": 300.0, "b": 692.0, "coord_origin": "TOPLEFT" });
        assert_eq!(distance(&anchor, &top_left)?, json!(0.0));
        assert_eq!(
            distance(&top_left, &bottom_left(100.0, 95.5, 300.0, 80.0))?,
            json!(4.5)
        );
        // What is not a box has no distance.
        for broken in [
            json!({ "l": 1.0, "t": 2.0, "r": 3.0, "coord_origin": "BOTTOMLEFT" }),
            json!({ "l": 1.0, "t": 2.0, "r": 3.0, "b": 0.0 }),
            json!(null),
        ] {
            assert_eq!(distance(&anchor, &broken)?, json!(null), "{broken}");
        }
        Ok(())
    }

    #[test]
    fn the_locations_state_what_the_rule_does_not_guarantee() -> TestResult {
        let (export, layer) = (export(), text_layer());
        let locations = locate_items(&export, Ok(&layer));
        assert_eq!(locations.limits, LOCATE_LIMITS);
        let value = serde_json::to_value(&locations)?;
        assert_eq!(value.get("limits"), Some(&json!(LOCATE_LIMITS)));
        for said in [
            "does not guarantee that the box is the item's own",
            "starts a page is searched on the page of its earlier sibling",
            "a running footer",
            "'Figure 1 (continued)'",
            "no threshold is applied",
        ] {
            assert!(LOCATE_LIMITS.contains(said), "{said}");
        }

        // The first case, as a document: a body item at the top of page 3 whose earlier sibling is
        // on page 2, where the same words stand once as a footer. It is located, on page 2.
        let wrong = json!({
            "pages": {
                "2": { "page_no": 2, "size": { "width": 612.0, "height": 792.0 } },
                "3": { "page_no": 3, "size": { "width": 612.0, "height": 792.0 } },
            },
            "body": { "self_ref": "#/body", "children": [{ "$ref": "#/texts/0" }, { "$ref": "#/texts/1" }] },
            "texts": [
                { "self_ref": "#/texts/0", "label": "text", "text": "A paragraph.", "parent": { "$ref": "#/body" }, "prov": prov(2, 72.0, 700.0, 300.0, 690.0) },
                { "self_ref": "#/texts/1", "label": "text", "text": "Row and column access control", "parent": { "$ref": "#/body" }, "prov": [] },
            ],
        });
        let footer =
            json!({ "l": 345.0, "t": 37.0, "r": 560.0, "b": 27.0, "coord_origin": "BOTTOMLEFT" });
        let layer = json!({ "texts": [line(2, "Row and column access control", &footer)] });
        let placed = locate_items(&wrong, Ok(&layer));
        let item = by(&placed, "#/texts/1")?;
        assert_eq!(
            (item.located_by, item.page_no),
            (LocatedBy::TextLayer, Some(2))
        );
        // What the measurement shows of it: the footer lies 654.55 units from the sibling.
        let lookup = serde_json::to_value(some(item.lookup.as_ref(), "the lookup")?)?;
        assert_eq!(lookup.get("distance"), Some(&json!(654.55)));
        Ok(())
    }
}
