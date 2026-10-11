//! Reading the docling JSON export (`DoclingDocument::export_to_json_value`).
//!
//! The export is the docling wire schema: `texts`, `tables` and `pictures` arrays whose items
//! carry `self_ref`, `parent`, `children`, `label`, `text` and `prov` (one entry per page
//! region, with `page_no` and a `bbox`), and a `pages` map holding each page's `size`. These
//! helpers read it as JSON, so the location and glyph rules are pure functions that tests run
//! without models.

use std::collections::BTreeSet;

use okf_jawn_contract::source::{BoundingBox, CoordOrigin, PageRegion, PageSize};
use serde_json::Value;

/// What an exported body item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// An item of `texts`: a paragraph, heading, caption, list item, code block.
    Text,
    /// An item of `tables`.
    Table,
    /// An item of `pictures`.
    Picture,
}

/// The item arrays of an export, with the kind each holds.
const ITEM_ARRAYS: [(&str, ItemKind); 3] = [
    ("texts", ItemKind::Text),
    ("tables", ItemKind::Table),
    ("pictures", ItemKind::Picture),
];

/// Every text item, table and picture of an export with its kind, in export order.
pub fn body_items(export: &Value) -> impl Iterator<Item = (ItemKind, &Value)> {
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
#[must_use]
pub fn resolve<'a>(export: &'a Value, reference: &str) -> Option<&'a Value> {
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
#[must_use]
pub fn provenance(item: &Value) -> &[Value] {
    item.get("prov")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The pages an exported item's provenance names, ascending and each once.
#[must_use]
pub fn pages_of(item: &Value) -> Vec<u32> {
    provenance(item)
        .iter()
        .filter_map(page_no)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The one-based page of a provenance entry.
#[must_use]
pub fn page_no(entry: &Value) -> Option<u32> {
    entry
        .get("page_no")
        .and_then(Value::as_u64)
        .and_then(|page| u32::try_from(page).ok())
        .filter(|page| *page > 0)
}

/// The size of a page of the converted document.
#[must_use]
pub fn page_size(export: &Value, page: u32) -> Option<PageSize> {
    let entry = export
        .get("pages")?
        .as_object()?
        .values()
        .find(|entry| page_no(entry) == Some(page))?;
    let size = entry.get("size")?;
    Some(PageSize {
        width: size.get("width")?.as_f64()?,
        height: size.get("height")?.as_f64()?,
    })
}

/// A docling `bbox` as the contract's box, when it is four finite numbers with a known origin.
#[must_use]
pub fn bounding_box(bbox: &Value) -> Option<BoundingBox> {
    let side = |name: &str| {
        bbox.get(name)
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
    };
    let coord_origin = match bbox.get("coord_origin").and_then(Value::as_str)? {
        "BOTTOMLEFT" => CoordOrigin::BottomLeft,
        "TOPLEFT" => CoordOrigin::TopLeft,
        _ => return None,
    };
    Some(BoundingBox {
        l: side("l")?,
        t: side("t")?,
        r: side("r")?,
        b: side("b")?,
        coord_origin,
    })
}

/// The region a provenance entry names, when its box and its page's size are both known.
#[must_use]
pub fn region(export: &Value, entry: &Value) -> Option<PageRegion> {
    let page = page_no(entry)?;
    Some(PageRegion {
        page_no: page,
        bbox: bounding_box(entry.get("bbox")?)?,
        page_size: page_size(export, page)?,
    })
}
