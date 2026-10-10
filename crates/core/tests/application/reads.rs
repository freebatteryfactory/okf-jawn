//! `read_item` serves committed text only, cites exactly what it returned, keeps a budget with
//! a continuation pinned to its revision, locates pages only in converter text, and records a
//! receipt; a sandbox capability is minted for an object of the item revision only.

use std::fmt::Write as _;

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::{PageRange, TextRange};
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::events::ReceiptAudience;
use okf_jawn_contract::identity::Timestamp;
use okf_jawn_contract::identity::{At, Digest, ItemId, PurgeId};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::read::{
    AssetRole, CreateSandboxCapabilityRequest, ReadItemRequest, ReadItemResponse, ReadView,
    Selection,
};
use okf_jawn_contract::source::{SourceLocation, SourceLocator, UnresolvedReason};
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::ports::Application;
use okf_jawn_core::reading::markdown_outline;
use okf_jawn_core::sandbox::token_hash;
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    ALICE, Built, CARD_BODY, NOTE, SANDBOX_ORIGIN, World, alice, context, converted, digest,
    draft_of, image, note, record, revision, scope, source_card,
};

const CARD: u128 = 10;

fn read(item: u128, view: ReadView, selection: Selection) -> Built<ReadItemRequest> {
    Ok(ReadItemRequest {
        workspace_id: scope()?.workspace_id,
        item_id: ItemId(Uuid::from_u128(item)),
        at: At::Latest,
        view,
        selection,
        max_bytes: 4096,
        max_images: 4,
        cursor: None,
    })
}

/// Forty numbered lines, long enough for a 256-byte budget to need several blocks.
fn forty_lines() -> Built<String> {
    let mut body = String::new();
    for line in 1..=40 {
        writeln!(body, "line {line:02} of the plan")?;
    }
    Ok(body)
}

async fn read_as_alice(world: &World, request: ReadItemRequest) -> Built<ReadItemResponse> {
    Ok(world
        .service
        .read_item(&alice(OperationName::ReadItem, None)?, request)
        .await?)
}

/// A world whose head holds a converted source card with `assets`, and its record.
fn card_world(assets: Vec<okf_jawn_core::conversion::RetainedAsset>) -> Built<World> {
    let world = World::new()?;
    let at = revision('a')?;
    world.versions.put_document(
        &at,
        source_card(
            CARD,
            &at,
            "sources/report-pdf.md",
            CARD_BODY,
            converted(&digest('b')?),
        )?,
    )?;
    world
        .blobs
        .put_object(digest('b')?, serde_json::to_vec(&record(assets)?)?)?;
    Ok(world)
}

#[tokio::test]
async fn draft_visibility_read_item_never_returns_a_draft() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.versions.put_document(
        &at,
        note(
            NOTE,
            &at,
            "notes/plan.md",
            "# Plan\ncommitted\n",
            json!({ "type": "Note" }),
        )?,
    )?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &at, "# Plan\nmy unsaved words\n")?)?;
    let response = read_as_alice(&world, read(NOTE, ReadView::Text, Selection::All)?).await?;
    assert_eq!(response.markdown, "# Plan\ncommitted\n");
    assert_eq!(world.drafts.gets(), 0);
    Ok(())
}

#[tokio::test]
async fn a_whole_read_cites_its_selection_and_records_a_receipt() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    let body = "# Plan\nintro\n## Goals\none\n";
    world.versions.put_document(
        &at,
        note(NOTE, &at, "notes/plan.md", body, json!({ "type": "Note" }))?,
    )?;
    let agent = context(
        OperationName::ReadItem,
        "agent",
        AccessRoute::McpDelegation,
        None,
    )?;
    let response = world
        .service
        .read_item(&agent, read(NOTE, ReadView::Text, Selection::All)?)
        .await?;
    assert_eq!(response.markdown, body);
    assert!(!response.truncated);
    assert_eq!(response.next_cursor, None);
    assert_eq!(response.source.selection, Selection::All);
    assert_eq!(response.source.revision, at);
    assert_eq!(response.source.digest, None);
    assert_eq!(response.outline, markdown_outline(body));
    let receipts = world.records.receipts()?;
    let (under, receipt) = some(receipts.first(), "the read's receipt")?;
    assert_eq!(*under, None);
    assert_eq!(receipt.id, response.receipt_id);
    assert_eq!(receipt.audience, ReceiptAudience::AgentContext);
    assert_eq!(receipt.sources.len(), 1);
    assert_eq!(
        receipt.sources.first().map(|source| &source.selection),
        Some(&Selection::All)
    );
    Ok(())
}

#[tokio::test]
async fn a_budgeted_read_cuts_at_a_line_and_continues_on_the_same_revision() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    let mut body = String::new();
    for line in 1..=40 {
        writeln!(body, "line {line:02} of the plan")?;
    }
    world.versions.put_document(
        &at,
        note(NOTE, &at, "notes/plan.md", &body, json!({ "type": "Note" }))?,
    )?;
    let mut request = read(NOTE, ReadView::Text, Selection::All)?;
    request.max_bytes = 256;
    let first = read_as_alice(&world, request.clone()).await?;
    assert!(first.truncated);
    assert!(first.markdown.len() <= 256 && first.markdown.ends_with('\n'));
    let lines = u32::try_from(first.markdown.lines().count())?;
    assert_eq!(
        first.source.selection,
        Selection::Lines {
            range: TextRange {
                start: 1,
                end: lines
            }
        }
    );
    // The head moves to a revision with no such item; the continuation still reads `a`.
    world.versions.set_head(revision('b')?)?;
    let mut text = first.markdown.clone();
    let mut cursor = first.next_cursor.clone();
    let mut rounds = 0_u32;
    while let Some(next) = cursor {
        rounds = rounds.saturating_add(1);
        let mut again = request.clone();
        again.cursor = Some(next);
        let page = read_as_alice(&world, again).await?;
        assert_eq!(page.source.revision, at);
        text.push_str(&page.markdown);
        cursor = page.next_cursor;
        assert!(rounds < 10, "the read must end");
    }
    assert_eq!(text, body);
    Ok(())
}

#[tokio::test]
async fn a_cursor_continues_only_its_own_read() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    let mut body = String::new();
    for line in 1..=40 {
        writeln!(body, "line {line:02}")?;
    }
    world.versions.put_document(
        &at,
        note(NOTE, &at, "notes/plan.md", &body, json!({ "type": "Note" }))?,
    )?;
    let mut request = read(NOTE, ReadView::Text, Selection::All)?;
    request.max_bytes = 256;
    let first = read_as_alice(&world, request.clone()).await?;
    let cursor = some(first.next_cursor, "a continuation")?;
    let mut other = request.clone();
    // A longer selection, so only the cursor's own check can refuse it.
    other.selection = Selection::Lines {
        range: TextRange { start: 2, end: 40 },
    };
    other.cursor = Some(cursor.clone());
    let refused = err_of(read_as_alice(&world, other).await)?;
    assert!(refused.to_string().contains("cursor"), "{refused}");
    let mut pinned = request;
    pinned.at = At::Revision {
        revision: revision('c')?,
    };
    pinned.cursor = Some(cursor);
    let elsewhere = err_of(read_as_alice(&world, pinned).await)?;
    assert!(elsewhere.to_string().contains("cursor"), "{elsewhere}");
    Ok(())
}

#[tokio::test]
async fn a_cursor_minted_before_a_purge_does_not_read_the_purged_revision() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.versions.put_document(
        &at,
        note(
            NOTE,
            &at,
            "notes/plan.md",
            &forty_lines()?,
            json!({ "type": "Note" }),
        )?,
    )?;
    let mut request = read(NOTE, ReadView::Text, Selection::All)?;
    request.max_bytes = 256;
    let first = read_as_alice(&world, request.clone()).await?;
    request.cursor = Some(some(first.next_cursor, "a continuation")?);
    let purge_id = PurgeId(Uuid::from_u128(5));
    world.records.put_mapping(
        at,
        RevisionMapping {
            purge_id,
            replacement: None,
        },
    )?;
    let shows = world.versions.shows();
    let refused = err_of(
        world
            .service
            .read_item(&alice(OperationName::ReadItem, None)?, request)
            .await,
    )?;
    assert_eq!(
        refused.detail.as_deref(),
        Some(&ErrorDetail::Invalidated {
            purge_id,
            replacement: None
        })
    );
    assert_eq!(world.versions.shows(), shows, "nothing is read at it");
    assert_eq!(world.records.receipts()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_sandbox_capability_checks_its_revision_against_the_purge_map_first() -> TestResult {
    let world = card_world(Vec::new())?;
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
            .create_sandbox_capability(
                &alice(OperationName::CreateSandboxCapability, None)?,
                CreateSandboxCapabilityRequest {
                    workspace_id: scope()?.workspace_id,
                    item_id: ItemId(Uuid::from_u128(CARD)),
                    revision: revision('a')?,
                    object: digest('a')?,
                },
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
    assert!(world.sandbox.mints()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_section_of_a_note_is_read_through_its_parsed_outline() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    let body = "# Plan\nintro\n## Goals\none\ntwo\n## Risks\nthree\n";
    world.versions.put_document(
        &at,
        note(NOTE, &at, "notes/plan.md", body, json!({ "type": "Note" }))?,
    )?;
    let section = Selection::Section {
        heading: "Goals".to_owned(),
    };
    let response = read_as_alice(&world, read(NOTE, ReadView::Text, section.clone())?).await?;
    assert_eq!(response.markdown, "## Goals\none\ntwo\n");
    assert_eq!(response.source.selection, section);
    assert_eq!(response.source.locations.len(), 0);
    let pages = err_of(
        read_as_alice(
            &world,
            read(
                NOTE,
                ReadView::Text,
                Selection::Pages {
                    range: PageRange { start: 1, end: 1 },
                },
            )?,
        )
        .await,
    )?;
    assert!(pages.to_string().contains("converter text"), "{pages}");
    Ok(())
}

#[tokio::test]
async fn converter_text_is_read_and_located_through_its_record() -> TestResult {
    let world = card_world(Vec::new())?;
    let section = read_as_alice(
        &world,
        read(
            CARD,
            ReadView::Text,
            Selection::Section {
                heading: "Revenue".to_owned(),
            },
        )?,
    )
    .await?;
    assert_eq!(
        section.markdown,
        "# Revenue\n| Quarter | Revenue |\n|---|---|\n| Q1 | 1200.5 |\n"
    );
    assert_eq!(section.source.digest, Some(digest('b')?));
    assert_eq!(
        section.source.locations,
        vec![SourceLocation::Direct {
            locator: SourceLocator::Page { page_no: 2 }
        }]
    );
    let page = read_as_alice(
        &world,
        read(
            CARD,
            ReadView::Text,
            Selection::Pages {
                range: PageRange { start: 1, end: 1 },
            },
        )?,
    )
    .await?;
    assert_eq!(page.markdown, "# Overview\nfirst\nsecond\n");
    assert_eq!(
        page.source.selection,
        Selection::Pages {
            range: PageRange { start: 1, end: 1 }
        }
    );
    Ok(())
}

#[tokio::test]
async fn a_corrected_text_has_no_pages_and_cites_no_place_in_the_original() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    let mut extraction = converted(&digest('b')?);
    extraction.corrected = true;
    world.versions.put_document(
        &at,
        source_card(
            CARD,
            &at,
            "sources/report-pdf.md",
            "# Corrected\ntext\n",
            extraction,
        )?,
    )?;
    world
        .blobs
        .put_object(digest('b')?, serde_json::to_vec(&record(Vec::new())?)?)?;
    let pages = err_of(
        read_as_alice(
            &world,
            read(
                CARD,
                ReadView::Text,
                Selection::Pages {
                    range: PageRange { start: 1, end: 2 },
                },
            )?,
        )
        .await,
    )?;
    assert!(pages.to_string().contains("converter text"), "{pages}");
    let whole = read_as_alice(&world, read(CARD, ReadView::Text, Selection::All)?).await?;
    assert_eq!(whole.markdown, "# Corrected\ntext\n");
    assert_eq!(
        whole.outline,
        markdown_outline("# Corrected\ntext\n"),
        "a correction is outlined by the parser, not by the converter's record"
    );
    assert_eq!(
        whole.source.locations,
        vec![SourceLocation::Unresolved {
            reason: UnresolvedReason::CorrectedText
        }]
    );
    Ok(())
}

#[tokio::test]
async fn the_pages_view_returns_the_selected_page_renders_within_max_images() -> TestResult {
    let world = card_world(vec![
        image('d', AssetRole::PageImage, 1)?,
        image('f', AssetRole::PageImage, 2)?,
        image('7', AssetRole::Picture, 2)?,
    ])?;
    let mut request = read(
        CARD,
        ReadView::Pages,
        Selection::Pages {
            range: PageRange { start: 2, end: 2 },
        },
    )?;
    let response = read_as_alice(&world, request.clone()).await?;
    let objects: Vec<&Digest> = response.media.iter().map(|media| &media.object).collect();
    assert_eq!(objects, vec![&digest('f')?]);
    assert_eq!(response.markdown, "");
    let media = some(response.media.first(), "the page render")?;
    assert_eq!(media.source.digest, Some(digest('f')?));
    request.selection = Selection::All;
    request.max_images = 1;
    let capped = read_as_alice(&world, request).await?;
    assert_eq!(capped.media.len(), 1);
    assert!(
        capped
            .warnings
            .iter()
            .any(|warning| warning.code == "images_omitted")
    );
    let multimodal =
        read_as_alice(&world, read(CARD, ReadView::Multimodal, Selection::All)?).await?;
    let pictures: Vec<&Digest> = multimodal.media.iter().map(|media| &media.object).collect();
    assert_eq!(pictures, vec![&digest('7')?]);
    assert_eq!(multimodal.markdown, CARD_BODY);
    Ok(())
}

#[tokio::test]
async fn a_continued_multimodal_read_returns_its_images_once() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    let mut body = CARD_BODY.to_owned();
    for line in 1..=40 {
        writeln!(body, "| Q{line} | {line}00.0 |")?;
    }
    world.versions.put_document(
        &at,
        source_card(
            CARD,
            &at,
            "sources/report-pdf.md",
            &body,
            converted(&digest('b')?),
        )?,
    )?;
    world.blobs.put_object(
        digest('b')?,
        serde_json::to_vec(&record(vec![image('7', AssetRole::Picture, 2)?])?)?,
    )?;
    let mut request = read(CARD, ReadView::Multimodal, Selection::All)?;
    request.max_bytes = 256;
    let first = read_as_alice(&world, request.clone()).await?;
    assert_eq!(first.media.len(), 1);
    request.cursor = Some(some(first.next_cursor, "a continuation")?);
    let next = read_as_alice(&world, request).await?;
    assert_eq!(next.media.len(), 0);
    Ok(())
}

#[tokio::test]
async fn the_original_view_cites_the_original_and_a_note_has_none() -> TestResult {
    let world = card_world(Vec::new())?;
    let original = read_as_alice(&world, read(CARD, ReadView::Original, Selection::All)?).await?;
    assert_eq!(original.source.digest, Some(digest('a')?));
    assert_eq!(original.markdown, "");
    let at = revision('a')?;
    world.versions.put_document(
        &at,
        note(NOTE, &at, "notes/plan.md", "x\n", json!({ "type": "Note" }))?,
    )?;
    let refused =
        err_of(read_as_alice(&world, read(NOTE, ReadView::Original, Selection::All)?).await)?;
    assert!(refused.to_string().contains("no original"), "{refused}");
    Ok(())
}

#[tokio::test]
async fn a_sandbox_capability_binds_a_hashed_token_to_one_object_of_the_revision() -> TestResult {
    let world = card_world(Vec::new())?;
    let request = |object: Digest| -> Built<CreateSandboxCapabilityRequest> {
        Ok(CreateSandboxCapabilityRequest {
            workspace_id: scope()?.workspace_id,
            item_id: ItemId(Uuid::from_u128(CARD)),
            revision: revision('a')?,
            object,
        })
    };
    let caller = alice(OperationName::CreateSandboxCapability, None)?;
    let before = Timestamp::from_utc(OffsetDateTime::now_utc())?;
    let capability = world
        .service
        .create_sandbox_capability(&caller, request(digest('a')?)?)
        .await?;
    let token = some(
        capability
            .url
            .strip_prefix(&format!("{SANDBOX_ORIGIN}/sandbox/")),
        "a sandbox URL",
    )?;
    assert_eq!(token.len(), 64);
    assert!(capability.expires_at > before);
    let mints = world.sandbox.mints()?;
    let (hash, mint) = some(mints.first(), "the mint")?;
    assert_eq!(*hash, token_hash(token));
    assert_eq!(mint.object, digest('a')?);
    assert_eq!(mint.media_type, "application/pdf");
    assert_eq!(mint.expires_at, capability.expires_at);
    let refused = err_of(
        world
            .service
            .create_sandbox_capability(&caller, request(digest('9')?)?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(world.sandbox.mints()?.len(), 1);
    Ok(())
}
