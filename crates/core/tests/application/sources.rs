//! Sources and objects: a card cites itself, a note cites the items its OKF `sources` name, and
//! `get_object` serves a bounded block only of an object of the cited item revision.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{At, Digest, ItemId, PurgeId};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::read::{AssetRole, ReadItemRequest, ReadView, Selection};
use okf_jawn_contract::source::{
    DeclaredOutcome, DeclaredSource, GetObjectRequest, GetSourcesRequest, SourceLocation,
    SourceLocator, SourceReference, UncitedReason,
};
use okf_jawn_core::application::MAX_OBJECT_BLOCK;
use okf_jawn_core::jobs::{DerivedKind, DerivedObject, RevisionMapping};
use okf_jawn_core::ports::Application;
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    Built, CARD_BODY, NOTE, World, alice, converted, digest, image, note, path, record, revision,
    scope, source_card,
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
    let appearance = some(response.appearance.clone(), "the appearance")?;
    assert_eq!(appearance.object, digest('a')?);
    let cited = some(response.sources.first(), "the card's citation")?;
    assert_eq!(response.sources.len(), 1);
    assert_eq!(cited.item_id, ItemId(Uuid::from_u128(CARD)));
    assert_eq!(cited.digest, Some(digest('b')?));
    assert_eq!(cited.selection, Selection::All);
    assert_eq!(response.declared.len(), 0);
    assert!(serde_json::to_value(&response)?.get("declared").is_none());
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

/// Put a note at `notes/plan.md` declaring `sources`, beside the card `card_world` put at
/// `notes/report-pdf.md`; a path is found through `VersionStore::item_at_path`, never through
/// a folder listing (none is scripted).
fn note_declaring(world: &World, sources: &serde_json::Value) -> Built<()> {
    let at = revision('a')?;
    world.versions.put_document(
        &at,
        note(
            NOTE,
            &at,
            "notes/plan.md",
            "# Plan\n",
            json!({ "type": "Note", "sources": sources }),
        )?,
    )
}

#[tokio::test]
async fn a_note_declares_every_okf_source_as_written_and_cites_the_items_they_name() -> TestResult {
    let world = card_world()?;
    let entries = json!([
        { "id": "web", "resource": "https://example.test/report", "title": "Report" },
        { "id": "report", "resource": "report-pdf.md", "usage_count": 3, "author": "team:finance" },
        { "id": "all", "resource": "all queries in project X" },
        { "id": "bare", "title": "No resource" },
        { "id": "gone", "resource": "missing.md" },
        "just a string",
        { "id": "again", "resource": "report-pdf" }
    ]);
    note_declaring(&world, &entries)?;
    let response = world
        .service
        .get_sources(&alice(OperationName::GetSources, None)?, sources_of(NOTE)?)
        .await?;
    assert!(response.appearance.is_none());
    // Field preservation: every entry, in the file's order, exactly as written.
    let written: Vec<&serde_json::Value> = response
        .declared
        .iter()
        .map(|declared| &declared.entry)
        .collect();
    let expected: Vec<&serde_json::Value> =
        some(entries.as_array(), "the entries")?.iter().collect();
    assert_eq!(written, expected);
    let uncited = |reason| DeclaredOutcome::Uncited { reason };
    let outcomes: Vec<&DeclaredOutcome> = response
        .declared
        .iter()
        .map(|declared| &declared.outcome)
        .collect();
    assert_eq!(
        outcomes,
        vec![
            &uncited(UncitedReason::External),
            &DeclaredOutcome::Cited { index: 0 },
            &uncited(UncitedReason::Scope),
            &uncited(UncitedReason::Malformed),
            &uncited(UncitedReason::NotFound),
            &uncited(UncitedReason::Malformed),
            &DeclaredOutcome::Cited { index: 0 },
        ]
    );
    // The cited index is the citation's position in `sources`; two entries share one.
    assert_eq!(response.sources.len(), 1);
    let card = some(response.sources.first(), "the card's citation")?;
    assert_eq!(card.item_id, ItemId(Uuid::from_u128(CARD)));
    assert_eq!(card.digest, Some(digest('b')?));
    assert_eq!(card.locations.len(), 2);
    // On the wire.
    let wire = serde_json::to_value(&response)?;
    assert_eq!(
        wire.pointer("/declared/1"),
        Some(&json!({
            "entry": { "id": "report", "resource": "report-pdf.md", "usage_count": 3, "author": "team:finance" },
            "outcome": { "kind": "cited", "index": 0 }
        }))
    );
    for (at, reason) in [
        (0, "external"),
        (2, "scope"),
        (3, "malformed"),
        (4, "not_found"),
        (5, "malformed"),
    ] {
        assert_eq!(
            wire.pointer(&format!("/declared/{at}/outcome")),
            Some(&json!({ "kind": "uncited", "reason": reason })),
            "entry {at}"
        );
    }
    assert_eq!(
        wire.pointer("/declared/5/entry"),
        Some(&json!("just a string"))
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_path_lookup_fails_get_sources_rather_than_reading_as_no_item() -> TestResult {
    let world = card_world()?;
    note_declaring(
        &world,
        &json!([{ "id": "report", "resource": "report-pdf.md" }]),
    )?;
    // The store answers `NotFound`, which `item_at_path` means only for an unknown revision.
    world.versions.fail_next_lookup();
    let refused = err_of(
        world
            .service
            .get_sources(&alice(OperationName::GetSources, None)?, sources_of(NOTE)?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    Ok(())
}

#[tokio::test]
async fn a_bare_mapping_is_one_entry_and_a_scalar_sources_value_is_malformed() -> TestResult {
    let world = card_world()?;
    note_declaring(
        &world,
        &json!({ "id": "report", "resource": "report-pdf.md" }),
    )?;
    let caller = alice(OperationName::GetSources, None)?;
    let mapping = world
        .service
        .get_sources(&caller, sources_of(NOTE)?)
        .await?;
    assert_eq!(
        mapping.declared,
        vec![DeclaredSource {
            entry: json!({ "id": "report", "resource": "report-pdf.md" }),
            outcome: DeclaredOutcome::Cited { index: 0 },
        }]
    );
    let world = card_world()?;
    note_declaring(&world, &json!("see the report"))?;
    let scalar = world
        .service
        .get_sources(&caller, sources_of(NOTE)?)
        .await?;
    assert_eq!(
        scalar.declared,
        vec![DeclaredSource {
            entry: json!("see the report"),
            outcome: DeclaredOutcome::Uncited {
                reason: UncitedReason::Malformed
            },
        }]
    );
    assert_eq!(scalar.sources.len(), 0);
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
async fn get_object_fills_omitted_locations_and_refuses_supplied_ones_it_does_not_compute()
-> TestResult {
    let world = card_world()?;
    let caller = alice(OperationName::GetObject, None)?;
    let page = |page_no| SourceLocation::Direct {
        locator: SourceLocator::Page { page_no },
    };
    // Omitted: the server fills them, and the read goes ahead.
    let omitted = world
        .service
        .get_object(&caller, object_request(digest('a')?, None, Some(3))?)
        .await?;
    assert_eq!(STANDARD.decode(&omitted.data_base64)?, b"hel".to_vec());
    // Present and equal to what the server computes: the whole record locates pages 1 and 2.
    let mut matching = object_request(digest('a')?, None, Some(3))?;
    matching.source.locations = vec![page(1), page(2)];
    world.service.get_object(&caller, matching).await?;
    // A made-up location is refused, and the object is not opened.
    let original = digest('a')?;
    let opens_of_original = |world: &World| -> Built<usize> {
        Ok(world
            .blobs
            .opens()?
            .iter()
            .filter(|(opened, _, _)| *opened == original)
            .count())
    };
    let before = opens_of_original(&world)?;
    let mut made_up = object_request(digest('a')?, None, Some(3))?;
    made_up.source.locations = vec![page(99)];
    let refused = err_of(world.service.get_object(&caller, made_up).await)?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/source/locations"));
    assert_eq!(opens_of_original(&world)?, before);
    Ok(())
}

#[tokio::test]
async fn get_object_accepts_the_citations_read_item_returns() -> TestResult {
    let world = card_world()?;
    world.blobs.put_object(
        digest('b')?,
        serde_json::to_vec(&record(vec![image('7', AssetRole::Picture, 2)?])?)?,
    )?;
    world
        .blobs
        .put_object(digest('7')?, b"png bytes".to_vec())?;
    let reader = alice(OperationName::ReadItem, None)?;
    let read = |view, selection| -> Built<ReadItemRequest> {
        Ok(ReadItemRequest {
            workspace_id: scope()?.workspace_id,
            item_id: ItemId(Uuid::from_u128(CARD)),
            at: At::Latest,
            view,
            selection,
            max_bytes: 4096,
            max_images: 4,
            cursor: None,
        })
    };
    let section = world
        .service
        .read_item(
            &reader,
            read(
                ReadView::Text,
                Selection::Section {
                    heading: "Revenue".to_owned(),
                },
            )?,
        )
        .await?;
    let picture = world
        .service
        .read_item(&reader, read(ReadView::Multimodal, Selection::All)?)
        .await?;
    let media = some(picture.media.first(), "the picture")?;
    let caller = alice(OperationName::GetObject, None)?;
    for (source, object) in [
        (section.source.clone(), digest('a')?),
        (picture.source.clone(), digest('a')?),
        (media.source.clone(), digest('7')?),
    ] {
        assert_ne!(source.locations, Vec::<SourceLocation>::new());
        world
            .service
            .get_object(
                &caller,
                GetObjectRequest {
                    source,
                    object,
                    offset: None,
                    length: None,
                },
            )
            .await?;
    }
    Ok(())
}

#[tokio::test]
async fn get_object_reads_at_most_one_mebibyte_per_call() -> TestResult {
    assert_eq!(MAX_OBJECT_BLOCK, 1_048_576);
    let world = card_world()?;
    let size = usize::try_from(MAX_OBJECT_BLOCK)?
        .checked_add(10)
        .ok_or("size")?;
    world.blobs.put_object(digest('a')?, vec![7_u8; size])?;
    let caller = alice(OperationName::GetObject, None)?;
    let asked = MAX_OBJECT_BLOCK.checked_add(1).ok_or("length")?;
    for length in [Some(asked), Some(u32::MAX), None] {
        let block = world
            .service
            .get_object(&caller, object_request(digest('a')?, None, length)?)
            .await?;
        assert_eq!(
            STANDARD.decode(&block.data_base64)?.len(),
            usize::try_from(MAX_OBJECT_BLOCK)?
        );
        assert!(block.has_more);
        let opens = world.blobs.opens()?;
        let (opened, _, read) = some(opens.last(), "the open")?;
        assert_eq!(*opened, digest('a')?);
        assert_eq!(*read, u64::from(MAX_OBJECT_BLOCK));
    }
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
