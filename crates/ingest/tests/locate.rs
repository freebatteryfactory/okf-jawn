//! `ingest-locates-unlocated-items`: an item the export leaves unlocated is located through the
//! text layer by the rule of `qualification/docling/src/locate.rs`, or stays unlocated with its
//! reason; a location found this way is inferred, never direct, and is never a guessed box.
//!
//! The exports below are docling JSON exports in the library's shape. They are inputs for the
//! rule, so no model is loaded.
#![cfg(feature = "runtime")]

#[path = "../../../tests/support/check.rs"]
mod check;

mod locates_unlocated_items {
    use okf_jawn_contract::extraction::ExtractionWarning;
    use okf_jawn_contract::source::{
        CoordOrigin, PageSize, SourceLocation, SourceLocator, UnresolvedReason,
    };
    use okf_jawn_ingest::export::ItemKind;
    use okf_jawn_ingest::locate::{
        ItemLocation, LOCATE_LIMITS, Locations, LookupBasis, box_distance, is_page_region,
        locate_items,
    };
    use serde_json::{Value, json};

    use crate::check::{TestResult, some};

    fn prov(page: u64, left: f64, top: f64, right: f64, bottom: f64) -> Value {
        json!([{ "page_no": page, "bbox": { "l": left, "t": top, "r": right, "b": bottom, "coord_origin": "BOTTOMLEFT" } }])
    }

    /// A converted document: a picture and a table with captions on page 2, a code block on
    /// page 3 followed by a caption that hangs under the body, and a located paragraph.
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

    fn by<'a>(locations: &'a Locations, item: &str) -> Result<&'a ItemLocation, String> {
        some(
            locations
                .items
                .iter()
                .find(|location| location.item == item),
            item,
        )
    }

    /// The provenance word of each location, in export order.
    fn provenances(locations: &Locations) -> Vec<(&str, &'static str)> {
        locations
            .items
            .iter()
            .map(|location| {
                let word = match location.location {
                    SourceLocation::Direct { .. } => "direct",
                    SourceLocation::Inferred { .. } => "inferred",
                    SourceLocation::Unresolved { .. } => "unresolved",
                };
                (location.item.as_str(), word)
            })
            .collect()
    }

    fn reason(location: &ItemLocation) -> Option<&UnresolvedReason> {
        match &location.location {
            SourceLocation::Unresolved { reason } => Some(reason),
            SourceLocation::Direct { .. } | SourceLocation::Inferred { .. } => None,
        }
    }

    #[test]
    fn an_unlocated_item_takes_the_page_and_box_of_the_one_line_with_its_text_as_inferred()
    -> TestResult {
        let (export, layer) = (export(), text_layer());
        let locations = locate_items(&export, Ok(&layer));
        assert_eq!(
            provenances(&locations),
            [
                ("#/texts/0", "direct"),
                ("#/texts/1", "inferred"),
                ("#/texts/2", "inferred"),
                ("#/texts/3", "direct"),
                ("#/texts/4", "inferred"),
                ("#/tables/0", "direct"),
                ("#/pictures/0", "direct"),
            ]
        );
        assert_eq!(locations.text_layer, Ok(4));

        // A caption is searched on its parent's page, and takes the line's own box.
        let caption = by(&locations, "#/texts/2")?;
        let lookup = some(caption.lookup.as_ref(), "the lookup of the table caption")?;
        assert_eq!(
            (lookup.basis, lookup.pages.as_slice(), lookup.occurrences),
            (LookupBasis::Parent, &[2][..], 1)
        );
        let SourceLocation::Inferred {
            locator: SourceLocator::Region { region },
        } = &caption.location
        else {
            return Err(format!("expected an inferred region, got {:?}", caption.location).into());
        };
        assert_eq!(region.page_no, 2);
        assert_eq!(
            (region.bbox.l, region.bbox.t, region.bbox.r, region.bbox.b),
            (136.27, 512.02, 284.48, 504.28)
        );
        assert_eq!(region.bbox.coord_origin, CoordOrigin::BottomLeft);
        assert_eq!(
            region.page_size,
            PageSize {
                width: 612.0,
                height: 792.0
            }
        );

        // An item under the body is searched on the page of its nearest located earlier
        // sibling.
        let hanging = by(&locations, "#/texts/4")?;
        let lookup = some(hanging.lookup.as_ref(), "the lookup of the hanging caption")?;
        assert_eq!(
            (lookup.basis, lookup.pages.as_slice()),
            (LookupBasis::EarlierSibling, &[3][..])
        );
        assert_eq!(locations.page_of("#/texts/4"), Some(3));

        // An item the export located keeps the export's region and is not looked up.
        let paragraph = by(&locations, "#/texts/0")?;
        assert_eq!(paragraph.lookup, None);
        let SourceLocation::Direct {
            locator: SourceLocator::Region { region },
        } = &paragraph.location
        else {
            return Err(format!("expected a direct region, got {:?}", paragraph.location).into());
        };
        assert_eq!((region.page_no, region.bbox.t), (2, 700.0));

        // The warnings count what the text layer located.
        assert_eq!(
            locations.warnings(),
            [ExtractionWarning::InferredLocations { items: 3 }]
        );
        Ok(())
    }

    #[test]
    fn text_that_occurs_twice_on_the_page_stays_unlocated() -> TestResult {
        let (export, mut layer) = (export(), text_layer());
        some(
            layer.get_mut("texts").and_then(Value::as_array_mut),
            "the lines",
        )?
        .push(line(
            2,
            "Figure 1-2   Existing controls",
            &json!({ "l": 72.0, "t": 60.0, "r": 200.0, "b": 50.0, "coord_origin": "BOTTOMLEFT" }),
        ));
        let locations = locate_items(&export, Ok(&layer));
        let caption = by(&locations, "#/texts/1")?;
        assert_eq!(
            reason(caption),
            Some(&UnresolvedReason::AmbiguousMatch { occurrences: 2 })
        );
        assert_eq!(locations.page_of("#/texts/1"), None);
        assert_eq!(some(caption.lookup.as_ref(), "the lookup")?.occurrences, 2);
        // The other captions are still found.
        assert!(matches!(
            by(&locations, "#/texts/2")?.location,
            SourceLocation::Inferred { .. }
        ));
        assert_eq!(
            locations.warnings(),
            [
                ExtractionWarning::InferredLocations { items: 2 },
                ExtractionWarning::UnlocatedItems { items: 1 }
            ]
        );

        // The same text on another page is not a second occurrence: only the parent's page is
        // searched.
        let mut elsewhere = text_layer();
        some(
            elsewhere.get_mut("texts").and_then(Value::as_array_mut),
            "the lines",
        )?
        .push(line(3, "Figure 1-2   Existing controls", &good_box()));
        assert_eq!(
            locate_items(&export, Ok(&elsewhere)).page_of("#/texts/1"),
            Some(2)
        );
        Ok(())
    }

    #[test]
    fn text_that_is_absent_from_the_page_stays_unlocated() -> TestResult {
        let export = export();
        // Absent altogether, present only on another page, present only as part of a longer
        // line, and present with different spacing.
        for layer in [
            json!({ "texts": [] }),
            json!({ "texts": [line(3, "Figure 1-2   Existing controls", &good_box())] }),
            json!({ "texts": [line(2, "Figure 1-2   Existing controls and more", &good_box()), line(2, "Figure 1-2", &good_box())] }),
            json!({ "texts": [line(2, "Figure 1-2 Existing controls", &good_box())] }),
        ] {
            let locations = locate_items(&export, Ok(&layer));
            let caption = by(&locations, "#/texts/1")?;
            assert_eq!(reason(caption), Some(&UnresolvedReason::NoMatch), "{layer}");
            assert_eq!(some(caption.lookup.as_ref(), "the lookup")?.occurrences, 0);
        }
        // A text layer that could not be read locates nothing, and says so.
        let unread = locate_items(&export, Err("pdf: unreadable"));
        assert_eq!(unread.text_layer, Err("pdf: unreadable".to_owned()));
        assert_eq!(
            reason(by(&unread, "#/texts/1")?),
            Some(&UnresolvedReason::NoTextLayer)
        );
        assert!(matches!(
            by(&unread, "#/texts/0")?.location,
            SourceLocation::Direct { .. }
        ));
        Ok(())
    }

    #[test]
    fn a_match_whose_box_is_not_a_region_of_the_page_is_not_a_location() -> TestResult {
        let export = export();
        for bbox in [
            json!({ "l": 136.0, "t": 900.0, "r": 316.0, "b": 880.0, "coord_origin": "BOTTOMLEFT" }),
            json!({ "l": 136.0, "t": 100.0, "r": 136.0, "b": 90.0, "coord_origin": "BOTTOMLEFT" }),
            json!({ "l": 136.0, "t": 90.0, "r": 316.0, "b": 100.0, "coord_origin": "BOTTOMLEFT" }),
            json!({ "l": 136.0, "t": 100.0, "r": 316.0, "coord_origin": "BOTTOMLEFT" }),
            json!({ "l": 136.0, "t": 100.0, "r": 316.0, "b": 90.0 }),
        ] {
            let layer = json!({ "texts": [line(2, "Figure 1-2   Existing controls", &bbox)] });
            let locations = locate_items(&export, Ok(&layer));
            assert_eq!(
                reason(by(&locations, "#/texts/1")?),
                Some(&UnresolvedReason::MatchNotALocation),
                "{bbox}"
            );
        }
        // A top-left box is read with its own origin.
        let size = PageSize {
            width: 612.0,
            height: 792.0,
        };
        assert!(is_page_region(
            &json!({ "l": 10.0, "t": 20.0, "r": 30.0, "b": 40.0, "coord_origin": "TOPLEFT" }),
            &size
        ));
        Ok(())
    }

    #[test]
    fn an_item_with_nowhere_to_look_and_a_table_or_picture_stay_unlocated() -> TestResult {
        let mut export = export();
        // The hanging caption becomes the first child: no earlier sibling is located.
        some(
            export
                .pointer_mut("/body/children")
                .and_then(Value::as_array_mut),
            "children",
        )?
        .rotate_right(1);
        // A table and a picture the export does not locate have no text to look up.
        for pointer in ["/tables/0/prov", "/pictures/0/prov"] {
            *some(export.pointer_mut(pointer), pointer)? = json!([]);
        }
        let layer = text_layer();
        let locations = locate_items(&export, Ok(&layer));
        let hanging = by(&locations, "#/texts/4")?;
        assert_eq!(reason(hanging), Some(&UnresolvedReason::NoPageToSearch));
        let lookup = some(hanging.lookup.as_ref(), "the lookup")?;
        assert_eq!((lookup.basis, lookup.pages.len()), (LookupBasis::None, 0));
        for item in ["#/tables/0", "#/pictures/0"] {
            let location = by(&locations, item)?;
            assert_eq!(
                (reason(location), location.lookup.as_ref()),
                (Some(&UnresolvedReason::NoTextToMatch), None),
                "{item}"
            );
            assert_ne!(location.kind, ItemKind::Text);
        }
        // Their captions now have an unlocated parent and no located earlier sibling there.
        assert_eq!(
            reason(by(&locations, "#/texts/1")?),
            Some(&UnresolvedReason::NoPageToSearch)
        );
        Ok(())
    }

    #[test]
    fn each_lookup_records_its_distance_and_no_bound_drops_a_location() -> TestResult {
        let (export, layer) = (export(), text_layer());
        let locations = locate_items(&export, Ok(&layer));
        let measured = |item: &str| -> Result<(LookupBasis, Option<String>, Option<f64>), String> {
            let lookup = some(by(&locations, item)?.lookup.clone(), "the lookup")?;
            Ok((lookup.basis, lookup.reference, lookup.distance))
        };
        // The picture ends at 450 and its caption was found with its top at 100.55, below it.
        assert_eq!(
            measured("#/texts/1")?,
            (
                LookupBasis::Parent,
                Some("#/pictures/0".to_owned()),
                Some(349.45)
            )
        );
        assert_eq!(
            measured("#/texts/2")?,
            (
                LookupBasis::Parent,
                Some("#/tables/0".to_owned()),
                Some(104.28)
            )
        );
        assert_eq!(
            measured("#/texts/4")?,
            (
                LookupBasis::EarlierSibling,
                Some("#/texts/3".to_owned()),
                Some(34.83)
            )
        );
        // O1: no distance bound. An item found far from its parent is located all the same.
        assert!(matches!(
            by(&locations, "#/texts/1")?.location,
            SourceLocation::Inferred { .. }
        ));
        Ok(())
    }

    #[test]
    fn the_first_known_wrong_placement_is_located_as_the_limits_state() -> TestResult {
        for said in [
            "does not guarantee that the box is the item's own",
            "a running footer",
            "'Figure 1 (continued)'",
            "no threshold is applied",
        ] {
            assert!(LOCATE_LIMITS.contains(said), "{said}");
        }
        // A body item at the top of page 3 whose earlier sibling is on page 2, where the same
        // words stand once as a footer. It is located, on page 2, and marked inferred.
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
        assert!(matches!(item.location, SourceLocation::Inferred { .. }));
        assert_eq!(placed.page_of("#/texts/1"), Some(2));
        assert_eq!(
            some(item.lookup.as_ref(), "the lookup")?.distance,
            Some(654.55)
        );
        Ok(())
    }

    /// `fixtures/redp5110_sampled.export.json` is a real docling JSON export of
    /// `tests/fixtures/documents/corpus/redp5110_sampled.pdf`, written by a local docling run on
    /// 2026-10-06 (docling 1.93.5 with the font-run patch; `document.json` sha256
    /// `b822e52d…5dfe30`). Page and picture images and the table `grid` were removed. The rule
    /// reads none of them. The text layer is read live from the PDF by the pinned library's
    /// pure-Rust text path, which needs no model.
    #[test]
    fn the_corpus_tables_captions_are_located_on_the_pages_sources_json_declares() -> TestResult {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let export: Value = serde_json::from_slice(&std::fs::read(
            manifest.join("tests/fixtures/redp5110_sampled.export.json"),
        )?)?;
        let bytes = std::fs::read(
            manifest.join("../../tests/fixtures/documents/corpus/redp5110_sampled.pdf"),
        )?;
        let layer = docling::pdf_text_layer_pages(&bytes, "redp5110_sampled.pdf", None)?
            .export_to_json_value();
        let locations = locate_items(&export, Ok(&layer));

        // SOURCES.json: "the four table captions Table 2-1, 2-2, 3-1 and 3-2 are on pages 8, 9,
        // 11 and 12" (confirmed with xpdf pdftotext). The export leaves each without a box.
        for (caption, page) in [
            ("#/texts/88", 8),
            ("#/texts/100", 9),
            ("#/texts/124", 11),
            ("#/texts/148", 12),
        ] {
            let item = by(&locations, caption)?;
            assert!(
                matches!(item.location, SourceLocation::Inferred { .. }),
                "{caption}: {:?}",
                item.location
            );
            assert_eq!(locations.page_of(caption), Some(page), "{caption}");
        }
        // Over the whole document: an item with provenance is direct; every inferred item is on
        // a page its lookup searched, inside that page; nothing else has a page.
        for item in &locations.items {
            match (&item.location, &item.lookup) {
                (SourceLocation::Direct { .. }, None)
                | (SourceLocation::Unresolved { .. }, None | Some(_)) => {}
                (
                    SourceLocation::Inferred {
                        locator: SourceLocator::Region { region },
                    },
                    Some(lookup),
                ) => {
                    assert!(lookup.pages.contains(&region.page_no), "{}", item.item);
                    assert_eq!(lookup.occurrences, 1, "{}", item.item);
                    assert!(region.bbox.r > region.bbox.l, "{}", item.item);
                }
                (other, lookup) => {
                    return Err(format!("{}: {other:?} with {lookup:?}", item.item).into());
                }
            }
        }
        Ok(())
    }

    #[test]
    fn the_distance_between_two_boxes_is_the_gap_between_their_nearest_edges() -> TestResult {
        let bottom_left = |left: f64, top: f64, right: f64, bottom: f64| json!({ "l": left, "t": top, "r": right, "b": bottom, "coord_origin": "BOTTOMLEFT" });
        let anchor = bottom_left(100.0, 200.0, 300.0, 100.0);
        let distance = |other: &Value| box_distance(&anchor, other, 792.0);
        assert_eq!(
            distance(&bottom_left(150.0, 250.0, 250.0, 150.0)),
            Some(0.0)
        );
        assert_eq!(distance(&bottom_left(100.0, 95.5, 300.0, 80.0)), Some(4.5));
        assert_eq!(distance(&bottom_left(303.0, 96.0, 400.0, 50.0)), Some(5.0));
        let top_left =
            json!({ "l": 100.0, "t": 592.0, "r": 300.0, "b": 692.0, "coord_origin": "TOPLEFT" });
        assert_eq!(distance(&top_left), Some(0.0));
        assert_eq!(distance(&json!(null)), None);
        let apart = some(distance(&bottom_left(100.0, 95.5, 300.0, 80.0)), "the gap")?;
        assert!((apart - 4.5).abs() < 0.001, "{apart}");
        Ok(())
    }
}
