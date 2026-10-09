//! The application header is server-owned: no caller write changes it, and the server's own
//! card header reads back exactly.

use std::collections::BTreeMap;

use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::extraction::{ConversionOutcome, Extraction, TextOrigin};
use okf_jawn_contract::identity::{Digest, ItemId, Timestamp, WorkspacePath};
use okf_jawn_contract::item::{APP_HEADER_KEY, TypeDefinition};
use okf_jawn_contract::source::{SourceAppearance, SourceName};
use okf_jawn_core::items::{
    ApplicationHeader, refuse_header_change, refuse_header_in_type, refuse_supplied_header,
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
    let header = card()?.header(false).to_property()?;
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
        json!({ "item_id": Uuid::from_u128(11) }),
    );
    err_of(refuse_header_change(&stored, &written, "/properties"))?;

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
    let header = card.header(false);
    assert_eq!(header.item_id, card.item_id);
    assert!(!header.archived);
    // A redigest replacing an archived source keeps it archived.
    let replaced = card.header(true);
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
fn an_unparseable_header_is_reported_on_its_field() -> TestResult {
    let properties = BTreeMap::from([(APP_HEADER_KEY.to_owned(), json!("not a mapping"))]);
    let refused = err_of(ApplicationHeader::from_properties(&properties))?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/properties/okf_jawn"));
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
