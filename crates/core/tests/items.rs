//! The application header is server-owned: no caller write changes it, and the server's own
//! card header reads back exactly.

use std::collections::BTreeMap;

use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::extraction::{ConversionOutcome, Extraction, TextOrigin};
use okf_jawn_contract::identity::{Digest, ItemId, Timestamp, WorkspacePath};
use okf_jawn_contract::item::{APP_HEADER_KEY, ItemKind, TypeDefinition};
use okf_jawn_contract::source::{SourceAppearance, SourceName};
use okf_jawn_core::items::{
    ApplicationHeader, SourceHeader, refuse_header_change, refuse_header_in_type,
    refuse_supplied_header, without_header,
};
use okf_jawn_core::storage::SourceCard;
use serde_json::json;
use uuid::Uuid;

use check::{TestResult, err_of};

fn digest(fill: char) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::try_from(fill.to_string().repeat(64))?)
}

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

fn card() -> Result<SourceCard, Box<dyn std::error::Error>> {
    Ok(SourceCard {
        item_id: ItemId(Uuid::from_u128(10)),
        path: WorkspacePath::try_from("sources/report-pdf.md".to_owned())?,
        title: "report.pdf".to_owned(),
        type_name: "source".to_owned(),
        body: String::new(),
        properties: BTreeMap::new(),
        appearance: SourceAppearance {
            object: digest('a')?,
            names: vec![SourceName {
                filename: "report.pdf".to_owned(),
                folder: "finance".to_owned(),
                observed_at: Timestamp::try_from("2026-10-08T12:00:00.000Z".to_owned())?,
                supplied_by: "user_1".to_owned(),
            }],
            media_type: "application/pdf".to_owned(),
            size: "2048".to_owned(),
            metadata: BTreeMap::new(),
            parent_item_id: None,
            supersedes: None,
            extraction: pending(),
        },
    })
}

fn stored_properties() -> Result<BTreeMap<String, serde_json::Value>, Box<dyn std::error::Error>> {
    let header = card()?.header(false)?.to_property()?;
    Ok(BTreeMap::from([
        (APP_HEADER_KEY.to_owned(), header),
        ("status".to_owned(), json!("stable")),
    ]))
}

#[test]
fn a_write_that_changes_the_application_header_is_refused() -> TestResult {
    let stored = stored_properties()?;
    let header = stored
        .get(APP_HEADER_KEY)
        .cloned()
        .ok_or("the stored item has a header")?;

    // create_item and Change::Create: the server assigns the header; supplying one is refused.
    let mut new_item = BTreeMap::from([("status".to_owned(), json!("draft"))]);
    refuse_supplied_header(&new_item, "/properties")?;
    new_item.insert(APP_HEADER_KEY.to_owned(), header.clone());
    let created = err_of(refuse_supplied_header(&new_item, "/properties"))?;
    assert_eq!(created.code, ErrorCode::InvalidInput);
    assert_eq!(created.field.as_deref(), Some("/properties/okf_jawn"));
    let proposed = err_of(refuse_supplied_header(&new_item, "/changes/0/properties"))?;
    assert_eq!(
        proposed.field.as_deref(),
        Some("/changes/0/properties/okf_jawn")
    );

    // save_draft and Change::Edit: leaving the header out or echoing it is accepted.
    let mut written = BTreeMap::from([("status".to_owned(), json!("deprecated"))]);
    refuse_header_change(&stored, &written, "/properties")?;
    written.insert(APP_HEADER_KEY.to_owned(), header.clone());
    refuse_header_change(&stored, &written, "/properties")?;

    // Any other value is refused: another item id, archived by a write, a forged digest.
    let mut changed = header.clone();
    let fields = changed.as_object_mut().ok_or("the header is a mapping")?;
    fields.insert("archived".to_owned(), json!(true));
    written.insert(APP_HEADER_KEY.to_owned(), changed);
    let edited = err_of(refuse_header_change(&stored, &written, "/properties"))?;
    assert_eq!(edited.code, ErrorCode::InvalidInput);
    assert_eq!(edited.field.as_deref(), Some("/properties/okf_jawn"));
    written.insert(
        APP_HEADER_KEY.to_owned(),
        json!({ "item_id": Uuid::from_u128(11), "kind": "source" }),
    );
    err_of(refuse_header_change(&stored, &written, "/properties"))?;
    // The rendering role is the header's too: a write cannot turn the card into a View.
    let mut viewed = header.clone();
    viewed
        .as_object_mut()
        .ok_or("the header is a mapping")?
        .insert("kind".to_owned(), json!("view"));
    written.insert(APP_HEADER_KEY.to_owned(), viewed);
    let rekinded = err_of(refuse_header_change(&stored, &written, "/properties"))?;
    assert_eq!(rekinded.field.as_deref(), Some("/properties/okf_jawn"));

    // A stored file without a header cannot gain one through a write.
    let headerless = BTreeMap::from([("status".to_owned(), json!("stable"))]);
    written.insert(APP_HEADER_KEY.to_owned(), header);
    err_of(refuse_header_change(&headerless, &written, "/properties"))?;

    // set_type: a type may not define the header.
    let mut definition = TypeDefinition {
        name: "note".to_owned(),
        schema_version: 1,
        properties_schema: json!({
            "type": "object",
            "properties": { "status": { "type": "string" } }
        }),
        ui_schema: json!({}),
    };
    refuse_header_in_type(&definition)?;
    definition.properties_schema = json!({
        "type": "object",
        "properties": { "okf_jawn": { "type": "object" } }
    });
    let typed = err_of(refuse_header_in_type(&definition))?;
    assert_eq!(
        typed.field.as_deref(),
        Some("/definition/properties_schema/properties/okf_jawn")
    );
    Ok(())
}

#[test]
fn a_card_header_reads_back_from_the_file_properties() -> TestResult {
    let card = card()?;
    let header = card.header(false)?;
    assert_eq!(header.item_id, card.item_id);
    assert_eq!(header.kind, ItemKind::Source);
    assert!(!header.archived);
    // A redigest replacing an archived source keeps it archived.
    let replaced = card.header(true)?;
    assert!(replaced.archived);
    assert_eq!(replaced.to_property()?.get("archived"), Some(&json!(true)));
    let source = header.source.clone().ok_or("a card names its original")?;
    assert_eq!(source.original_name, "report.pdf");
    assert_eq!(source.digest, card.appearance.object);
    assert_eq!(header.extraction, Some(pending()));

    let property = header.to_property()?;
    assert!(property.get("archived").is_none(), "{property}");
    let properties = BTreeMap::from([(APP_HEADER_KEY.to_owned(), property)]);
    assert_eq!(
        ApplicationHeader::from_properties(&properties)?,
        Some(header)
    );
    assert_eq!(ApplicationHeader::from_properties(&BTreeMap::new())?, None);
    Ok(())
}

#[test]
fn the_header_records_the_kind_and_a_headerless_import_is_never_a_view() -> TestResult {
    let item_id = ItemId(Uuid::from_u128(12));
    // A file imported without a header: a source card is a Source, anything else a Note,
    // whatever its body shows (a fenced vega-lite example does not make a View).
    let note = ApplicationHeader::assigned(item_id, None, None);
    assert_eq!(note.kind, ItemKind::Note);
    assert!(!note.archived);
    let source = SourceHeader {
        original_name: "report.pdf".to_owned(),
        digest: digest('a')?,
    };
    let card = ApplicationHeader::assigned(item_id, Some(source), Some(pending()));
    assert_eq!(card.kind, ItemKind::Source);
    assert_eq!(card.to_property()?.get("kind"), Some(&json!("source")));

    // The kind reads back as written, a View included.
    let view = ApplicationHeader {
        kind: ItemKind::View,
        ..note
    };
    let properties = BTreeMap::from([(APP_HEADER_KEY.to_owned(), view.to_property()?)]);
    assert_eq!(ApplicationHeader::from_properties(&properties)?, Some(view));

    // It is required: a stored header without it is unparseable, never defaulted.
    let kindless = BTreeMap::from([(
        APP_HEADER_KEY.to_owned(),
        json!({ "item_id": Uuid::from_u128(12) }),
    )]);
    let refused = err_of(ApplicationHeader::from_properties(&kindless))?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/properties/okf_jawn"));
    Ok(())
}

#[test]
fn a_card_without_an_observed_name_has_no_header() -> TestResult {
    let mut nameless = card()?;
    nameless.appearance.names.clear();
    let refused = err_of(nameless.header(false))?;
    assert_eq!(refused.code, ErrorCode::Internal);
    Ok(())
}

#[test]
fn an_unparseable_header_is_reported_on_its_field() -> TestResult {
    let properties = BTreeMap::from([(APP_HEADER_KEY.to_owned(), json!("not a mapping"))]);
    let refused = err_of(ApplicationHeader::from_properties(&properties))?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/properties/okf_jawn"));
    Ok(())
}

#[test]
fn a_type_cannot_reach_the_header_at_any_depth() -> TestResult {
    let typed = |schema: serde_json::Value| TypeDefinition {
        name: "note".to_owned(),
        schema_version: 1,
        properties_schema: schema,
        ui_schema: json!({}),
    };
    let nested = typed(json!({
        "allOf": [{ "properties": { "okf_jawn": { "const": 1 } } }]
    }));
    let refused = err_of(refuse_header_in_type(&nested))?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(
        refused.field.as_deref(),
        Some("/definition/properties_schema/allOf/0/properties/okf_jawn")
    );
    let defined = typed(json!({
        "$defs": { "a~b/c": { "properties": { "okf_jawn": {} } } }
    }));
    let refused = err_of(refuse_header_in_type(&defined))?;
    assert_eq!(
        refused.field.as_deref(),
        Some("/definition/properties_schema/$defs/a~0b~1c/properties/okf_jawn")
    );
    let required = typed(json!({ "type": "object", "required": ["okf_jawn"] }));
    err_of(refuse_header_in_type(&required))?;
    // Required only when another property is present: refused in both spellings, nested too.
    for keyword in ["dependentRequired", "dependencies"] {
        let conditional = typed(json!({
            "allOf": [{ keyword: { "status": ["title", "okf_jawn"] } }]
        }));
        let refused = err_of(refuse_header_in_type(&conditional))?;
        assert_eq!(
            refused.field.as_deref(),
            Some(
                format!("/definition/properties_schema/allOf/0/{keyword}/status/okf_jawn").as_str()
            )
        );
    }
    let unrelated = typed(json!({ "dependentRequired": { "status": ["title"] } }));
    refuse_header_in_type(&unrelated)?;
    let listed = typed(json!({ "items": [{ "required": ["okf_jawn"] }] }));
    let refused = err_of(refuse_header_in_type(&listed))?;
    assert_eq!(
        refused.field.as_deref(),
        Some("/definition/properties_schema/items/0/required/okf_jawn")
    );
    // Annotations and instance values are data, not subschemas: a header-shaped value there
    // declares nothing.
    let annotated = typed(json!({
        "type": "object",
        "examples": [{ "required": ["okf_jawn"] }],
        "default": { "properties": { "okf_jawn": {} } },
        "const": { "required": ["okf_jawn"] },
        "enum": [{ "properties": { "okf_jawn": 1 } }],
        "properties": { "status": { "type": "string", "examples": [{ "required": ["okf_jawn"] }] } }
    }));
    refuse_header_in_type(&annotated)?;
    // A strict type that does not name the header is accepted; the header is left out of what
    // it is evaluated against.
    let strict = typed(json!({
        "type": "object",
        "additionalProperties": false,
        "properties": { "status": { "type": "string" } }
    }));
    refuse_header_in_type(&strict)?;
    let stored = stored_properties()?;
    let evaluated = without_header(&stored);
    assert!(!evaluated.contains_key(APP_HEADER_KEY));
    assert_eq!(evaluated.get("status"), Some(&json!("stable")));
    Ok(())
}

#[test]
fn a_type_cannot_reference_anything_but_its_own_definitions() -> TestResult {
    let typed = |schema: serde_json::Value| TypeDefinition {
        name: "note".to_owned(),
        schema_version: 1,
        properties_schema: schema,
        ui_schema: json!({}),
    };
    // A reference to a header requirement kept where no keyword scan reaches it would make
    // every item fail; both are refused.
    for schema in [
        json!({ "$ref": "#/examples/0", "examples": [{ "required": ["okf_jawn"] }] }),
        json!({ "$ref": "#/x", "x": { "required": ["okf_jawn"] } }),
    ] {
        let refused = err_of(refuse_header_in_type(&typed(schema)))?;
        assert_eq!(refused.code, ErrorCode::InvalidInput);
        assert_eq!(
            refused.field.as_deref(),
            Some("/definition/properties_schema/$ref")
        );
        assert!(refused.message.contains("$defs or definitions"));
    }
    for reference in [
        "#/$defs/a/properties/b",
        "#foo",
        "other.json#/x",
        "https://example.com/s",
        "#/$defs/",
        "#/$defs/a~2",
        "#/definitions/a/b",
        "#/$defs/a%2",
        "#/$defs/%FF",
        "#/$defs/a%2Fx",
        "#/$defs/a%2Fexamples%2F0",
    ] {
        let refused = err_of(refuse_header_in_type(&typed(json!({ "$ref": reference }))))?;
        assert_eq!(
            refused.field.as_deref(),
            Some("/definition/properties_schema/$ref"),
            "{reference}"
        );
    }
    let refused = err_of(refuse_header_in_type(&typed(
        json!({ "$dynamicRef": "#meta" }),
    )))?;
    assert_eq!(
        refused.field.as_deref(),
        Some("/definition/properties_schema/$dynamicRef")
    );
    let refused = err_of(refuse_header_in_type(&typed(
        json!({ "$recursiveRef": "#/x" }),
    )))?;
    assert_eq!(
        refused.field.as_deref(),
        Some("/definition/properties_schema/$recursiveRef")
    );
    // A reference nested in an applicator is checked too.
    let nested = typed(json!({ "allOf": [{ "$ref": "#/x" }] }));
    let refused = err_of(refuse_header_in_type(&nested))?;
    assert_eq!(
        refused.field.as_deref(),
        Some("/definition/properties_schema/allOf/0/$ref")
    );
    // A property that is itself named `$ref` is a property, not a reference.
    refuse_header_in_type(&typed(
        json!({ "properties": { "$ref": { "type": "string" } } }),
    ))?;
    // References to the schema itself or to its own definitions are accepted.
    refuse_header_in_type(&typed(json!({
        "$ref": "#/$defs/a",
        "$defs": { "a": { "type": "object" } }
    })))?;
    refuse_header_in_type(&typed(json!({
        "$ref": "#/definitions/a~1b",
        "definitions": { "a/b": { "type": "object" } }
    })))?;
    refuse_header_in_type(&typed(json!({ "$ref": "#" })))?;
    // The definitions a reference reaches are scanned.
    let reached = typed(json!({
        "$ref": "#/$defs/a",
        "$defs": { "a": { "required": ["okf_jawn"] } }
    }));
    let refused = err_of(refuse_header_in_type(&reached))?;
    assert_eq!(
        refused.field.as_deref(),
        Some("/definition/properties_schema/$defs/a/required/okf_jawn")
    );
    Ok(())
}

#[test]
fn a_reference_is_judged_after_percent_decoding() -> TestResult {
    let typed = |schema: serde_json::Value| TypeDefinition {
        name: "note".to_owned(),
        schema_version: 1,
        properties_schema: schema,
        ui_schema: json!({}),
    };
    // The fragment is judged after percent-decoding, as the resolver reads it.
    refuse_header_in_type(&typed(json!({
        "$ref": "#/%24defs/a",
        "$defs": { "a": { "type": "object" } }
    })))?;
    refuse_header_in_type(&typed(json!({
        "$ref": "#/$defs/a~1x",
        "$defs": { "a/x": { "type": "object" } }
    })))?;
    // A percent-encoded `/` would make these several tokens reaching outside `$defs` entries.
    for schema in [
        json!({
            "$ref": "#/$defs/a%2Fexamples%2F0",
            "$defs": { "a": { "examples": [{ "required": ["okf_jawn"] }] } }
        }),
        json!({ "$ref": "#/$defs/a%2Fx", "$defs": { "a": { "x": { "required": ["okf_jawn"] } } } }),
    ] {
        let refused = err_of(refuse_header_in_type(&typed(schema)))?;
        assert_eq!(
            refused.field.as_deref(),
            Some("/definition/properties_schema/$ref")
        );
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
