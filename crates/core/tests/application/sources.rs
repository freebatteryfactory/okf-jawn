//! Sources and objects: a card cites itself, a note cites the items its OKF `sources` name, and
//! `get_object` serves a bounded block only of an object of the cited item revision.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{At, Digest, ItemId, PurgeId};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::source::{
    GetObjectRequest, GetSourcesRequest, SourceLocation, SourceLocator, SourceReference,
};
use okf_jawn_core::jobs::{DerivedKind, DerivedObject, RevisionMapping};
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::FolderListing;
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    Built, CARD_BODY, NOTE, World, alice, converted, digest, note, path, record, revision, scope,
    source_card,
};

const CARD: u128 = 10;

fn sources_of(item: u128) -> Built<GetSourcesRequest> {
    Ok(GetSourcesRequest {
        workspace_id: scope()?.workspace_id,
        item_id: ItemId(Uuid::from_u128(item)),
        at: At::Latest,
    })
}

fn citation(digest_of: Option<Digest>) -> Built<SourceReference> {
    Ok(SourceReference {
        workspace_id: scope()?.workspace_id,
        item_id: ItemId(Uuid::from_u128(CARD)),
        path: path("notes/report-pdf.md")?,
        revision: revision('a')?,
        digest: digest_of,
        selection: Selection::All,
        locations: Vec::new(),
    })
}

fn object_request(
    object: Digest,
    offset: Option<&str>,
    length: Option<u32>,
) -> Built<GetObjectRequest> {
    Ok(GetObjectRequest {
        source: citation(Some(digest('b')?))?,
        object,
        offset: offset.map(str::to_owned),
        length,
    })
}

/// A world whose head holds a converted card at `notes/report-pdf.md`, its record, and its
/// original `digest('a')` with the bytes `hello world`.
fn card_world() -> Built<World> {
    let world = World::new()?;
    let at = revision('a')?;
    world.versions.put_document(
        &at,
        source_card(
            CARD,
            &at,
            "notes/report-pdf.md",
            CARD_BODY,
            converted(&digest('b')?),
        )?,
    )?;
    world
        .blobs
        .put_object(digest('b')?, serde_json::to_vec(&record(Vec::new())?)?)?;
    world
        .blobs
        .put_object(digest('a')?, b"hello world".to_vec())?;
    Ok(world)
}

#[tokio::test]
async fn a_source_card_is_supported_by_its_appearance_and_one_whole_citation() -> TestResult {
    let world = card_world()?;
    let response = world
        .service
        .get_sources(&alice(OperationName::GetSources, None)?, sources_of(CARD)?)
        .await?;
    assert_eq!(response.revision, revision('a')?);
    let appearance = some(response.appearance, "the appearance")?;
    assert_eq!(appearance.object, digest('a')?);
    let cited = some(response.sources.first(), "the card's citation")?;
    assert_eq!(response.sources.len(), 1);
    assert_eq!(cited.item_id, ItemId(Uuid::from_u128(CARD)));
    assert_eq!(cited.digest, Some(digest('b')?));
    assert_eq!(cited.selection, Selection::All);
    assert_eq!(
        cited.locations,
        vec![
            SourceLocation::Direct {
                locator: SourceLocator::Page { page_no: 1 }
            },
            SourceLocation::Direct {
                locator: SourceLocator::Page { page_no: 2 }
            }
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_note_cites_the_items_its_okf_sources_name() -> TestResult {
    let world = card_world()?;
    let at = revision('a')?;
    world.versions.put_document(
        &at,
        note(
            NOTE,
            &at,
            "notes/plan.md",
            "# Plan\n",
            json!({
                "type": "Note",
                "sources": [
                    { "id": "external", "resource": "https://example.test/report" },
                    { "id": "report", "resource": "report-pdf.md" },
                    { "id": "scope", "resource": "all queries in project X" },
                    { "id": "missing", "resource": "missing.md" }
                ]
            }),
        )?,
    )?;
    let listed = source_card(
        CARD,
        &at,
        "notes/report-pdf.md",
        CARD_BODY,
        converted(&digest('b')?),
    )?;
    world.versions.put_listing(
        &at,
        Some(path("notes")?),
        FolderListing {
            items: vec![listed.summary],
            folders: Vec::new(),
            next_cursor: None,
        },
    )?;
    let response = world
        .service
        .get_sources(&alice(OperationName::GetSources, None)?, sources_of(NOTE)?)
        .await?;
    assert!(response.appearance.is_none());
    let cited: Vec<ItemId> = response
        .sources
        .iter()
        .map(|source| source.item_id)
        .collect();
    assert_eq!(cited, vec![ItemId(Uuid::from_u128(CARD))]);
    let card = some(response.sources.first(), "the card's citation")?;
    assert_eq!(card.digest, Some(digest('b')?));
    assert_eq!(card.locations.len(), 2);
    Ok(())
}

#[tokio::test]
async fn get_object_serves_a_bounded_block_of_an_object_of_the_cited_revision() -> TestResult {
    let world = card_world()?;
    let caller = alice(OperationName::GetObject, None)?;
    let block = world
        .service
        .get_object(&caller, object_request(digest('a')?, Some("6"), Some(3))?)
        .await?;
    assert_eq!(STANDARD.decode(&block.data_base64)?, b"wor".to_vec());
    assert_eq!(block.offset, "6");
    assert_eq!(block.total_size, "11");
    assert!(block.has_more);
    assert_eq!(block.media_type, "application/pdf");
    assert_eq!(block.sha256, digest('a')?);
    let record = world
        .service
        .get_object(&caller, object_request(digest('b')?, None, None)?)
        .await?;
    assert_eq!(record.media_type, "application/json");
    assert!(!record.has_more);
    let past = err_of(
        world
            .service
            .get_object(&caller, object_request(digest('a')?, Some("12"), None)?)
            .await,
    )?;
    assert_eq!(past.field.as_deref(), Some("/offset"));
    Ok(())
}

#[tokio::test]
async fn get_object_never_serves_an_object_by_its_digest_alone() -> TestResult {
    let world = card_world()?;
    world.blobs.put_object(digest('f')?, b"a backup".to_vec())?;
    let refused = err_of(
        world
            .service
            .get_object(
                &alice(OperationName::GetObject, None)?,
                object_request(digest('f')?, None, None)?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    let backup = digest('f')?;
    assert!(
        world
            .blobs
            .opens()?
            .iter()
            .all(|(opened, _, _)| *opened != backup)
    );
    Ok(())
}

#[tokio::test]
async fn get_object_checks_the_cited_revision_against_the_purge_map_first() -> TestResult {
    let world = card_world()?;
    let purge_id = PurgeId(Uuid::from_u128(5));
    world.records.put_mapping(
        revision('a')?,
        RevisionMapping {
            purge_id,
            replacement: None,
        },
    )?;
    let refused = err_of(
        world
            .service
            .get_object(
                &alice(OperationName::GetObject, None)?,
                object_request(digest('a')?, None, None)?,
            )
            .await,
    )?;
    assert_eq!(
        refused.detail.as_deref(),
        Some(&ErrorDetail::Invalidated {
            purge_id,
            replacement: None
        })
    );
    assert_eq!(world.versions.shows(), 0);
    assert_eq!(world.blobs.opens()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn get_object_serves_a_derived_object_only_for_the_revision_it_was_recorded_for() -> TestResult
{
    let world = card_world()?;
    world
        .blobs
        .put_object(digest('6')?, b"{\"rows\":[]}".to_vec())?;
    world.records.put_derived(DerivedObject {
        item_id: ItemId(Uuid::from_u128(CARD)),
        revision: revision('a')?,
        digest: digest('6')?,
        kind: DerivedKind::Dataset,
        media_type: "application/vnd.okf-jawn.dataset+json".to_owned(),
    })?;
    let caller = alice(OperationName::GetObject, None)?;
    let dataset = world
        .service
        .get_object(&caller, object_request(digest('6')?, None, None)?)
        .await?;
    assert_eq!(dataset.media_type, "application/vnd.okf-jawn.dataset+json");
    let at = revision('7')?;
    world.versions.put_document(
        &at,
        source_card(
            CARD,
            &at,
            "notes/report-pdf.md",
            CARD_BODY,
            converted(&digest('b')?),
        )?,
    )?;
    let mut elsewhere = object_request(digest('6')?, None, None)?;
    elsewhere.source.revision = at;
    let refused = err_of(world.service.get_object(&caller, elsewhere).await)?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    Ok(())
}
