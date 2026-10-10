//! Drafts belong to the caller: a save keeps its base unless it rebases on the head, refuses a
//! changed header, and listing and discarding see only the caller's own.

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::PageRequest;
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{IdempotencyKey, ItemId, PurgeId, Revision};
use okf_jawn_contract::item::{
    APP_HEADER_KEY, DiscardDraftRequest, ListDraftsRequest, SaveDraftRequest,
};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::portable::content_digest;
use okf_jawn_core::ports::Application;
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    ALICE, Built, NOTE, World, alice, context, draft_of, mutation, note, revision, scope,
    served_digest,
};

fn save(base: Revision, properties: serde_json::Value) -> Built<SaveDraftRequest> {
    Ok(SaveDraftRequest {
        workspace_id: scope()?.workspace_id,
        item_id: ItemId(Uuid::from_u128(NOTE)),
        base_revision: base,
        body: "# Plan, revised\n".to_owned(),
        properties: serde_json::from_value(properties)?,
        idempotency_key: IdempotencyKey(Uuid::from_u128(77)),
    })
}

fn header() -> serde_json::Value {
    json!({ "item_id": Uuid::from_u128(NOTE), "kind": "note" })
}

fn stored_at(world: &World, fill: char) -> Built<()> {
    let at = revision(fill)?;
    world.versions.put_document(
        &at,
        note(
            NOTE,
            &at,
            "notes/plan.md",
            "# Plan\n",
            json!({ "type": "Note", APP_HEADER_KEY: header() }),
        )?,
    )
}

#[tokio::test]
async fn a_first_save_bases_on_the_head_a_later_one_keeps_its_base_until_it_rebases() -> TestResult
{
    let world = World::new()?;
    stored_at(&world, 'a')?;
    stored_at(&world, 'b')?;
    let first = world
        .service
        .save_draft(
            &alice(OperationName::SaveDraft, Some(mutation(1)))?,
            save(revision('c')?, json!({ "type": "Note" }))?,
        )
        .await?;
    assert_eq!(first.base_revision, revision('a')?);
    assert_eq!(first.editor, ALICE);
    world.versions.set_head(revision('b')?)?;
    let kept = world
        .service
        .save_draft(
            &alice(OperationName::SaveDraft, Some(mutation(2)))?,
            save(revision('a')?, json!({ "type": "Note" }))?,
        )
        .await?;
    assert_eq!(kept.base_revision, revision('a')?);
    let rebased = world
        .service
        .save_draft(
            &alice(OperationName::SaveDraft, Some(mutation(3)))?,
            save(
                revision('b')?,
                json!({ "type": "Note", APP_HEADER_KEY: header() }),
            )?,
        )
        .await?;
    assert_eq!(rebased.base_revision, revision('b')?);
    let saves = world.drafts.saves()?;
    let (_, written) = some(saves.last(), "the rebase")?;
    assert_eq!(
        written.content_digest,
        content_digest(&written.body, &written.properties)?
    );
    assert!(world.versions.commits()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_draft_records_the_digest_of_its_own_body_and_properties() -> TestResult {
    let world = World::new()?;
    stored_at(&world, 'a')?;
    let at = revision('a')?;
    let properties = json!({ "type": "Note", "tags": ["draft"] });
    let request = save(at.clone(), properties.clone())?;
    // The item the draft would commit: the drafted body and properties.
    let drafted = note(NOTE, &at, "notes/plan.md", &request.body, properties)?;
    let stored = note(
        NOTE,
        &at,
        "notes/plan.md",
        "# Plan\n",
        json!({ "type": "Note" }),
    )?;
    let expected = served_digest(&drafted)?;
    assert_ne!(expected, served_digest(&stored)?);
    let answered = world
        .service
        .save_draft(
            &alice(OperationName::SaveDraft, Some(mutation(1)))?,
            request,
        )
        .await?;
    assert_eq!(answered.content_digest, expected);
    let writes = world.drafts.saves()?;
    let (_, written) = some(writes.first(), "the save")?;
    assert_eq!(written.content_digest, expected);
    Ok(())
}

#[tokio::test]
async fn a_save_checks_its_base_against_the_purge_map_first() -> TestResult {
    let world = World::new()?;
    stored_at(&world, 'a')?;
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
            .save_draft(
                &alice(OperationName::SaveDraft, Some(mutation(1)))?,
                save(revision('a')?, json!({ "type": "Note" }))?,
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
    assert_eq!(world.drafts.gets(), 0);
    assert!(world.drafts.saves()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_save_that_changes_the_header_is_refused_before_it_is_written() -> TestResult {
    let world = World::new()?;
    stored_at(&world, 'a')?;
    let refused = err_of(
        world
            .service
            .save_draft(
                &alice(OperationName::SaveDraft, Some(mutation(1)))?,
                save(
                    revision('a')?,
                    json!({ APP_HEADER_KEY: { "item_id": Uuid::from_u128(NOTE), "kind": "view" } }),
                )?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/properties/okf_jawn"));
    assert!(world.drafts.saves()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn draft_visibility_list_drafts_pages_through_only_the_callers_own() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    for item in [1, 2, 3] {
        world
            .drafts
            .put_draft(draft_of(item, ALICE, &at, "mine")?)?;
    }
    world.drafts.put_draft(draft_of(4, "bob", &at, "his")?)?;
    let request = |cursor: Option<&str>| -> Built<ListDraftsRequest> {
        Ok(ListDraftsRequest {
            workspace_id: scope()?.workspace_id,
            page: PageRequest {
                cursor: cursor.map(str::to_owned),
                limit: 2,
            },
        })
    };
    let caller = alice(OperationName::ListDrafts, None)?;
    let first = world.service.list_drafts(&caller, request(None)?).await?;
    assert_eq!(first.items.len(), 2);
    let next = some(first.next_cursor.clone(), "a continuation")?;
    let second = world
        .service
        .list_drafts(&caller, request(Some(&next))?)
        .await?;
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.next_cursor, None);
    assert!(
        first
            .items
            .iter()
            .chain(&second.items)
            .all(|draft| draft.editor == ALICE)
    );
    let refused = err_of(
        world
            .service
            .list_drafts(&caller, request(Some("not-a-cursor"))?)
            .await,
    )?;
    assert_eq!(refused.field.as_deref(), Some("/page/cursor"));
    Ok(())
}

#[tokio::test]
async fn draft_visibility_discard_draft_removes_only_the_callers_own() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.drafts.put_draft(draft_of(NOTE, "bob", &at, "his")?)?;
    let request = || -> Built<DiscardDraftRequest> {
        Ok(DiscardDraftRequest {
            workspace_id: scope()?.workspace_id,
            item_id: ItemId(Uuid::from_u128(NOTE)),
            idempotency_key: IdempotencyKey(Uuid::from_u128(77)),
        })
    };
    let refused = err_of(
        world
            .service
            .discard_draft(
                &alice(OperationName::DiscardDraft, Some(mutation(1)))?,
                request()?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    let bob = context(
        OperationName::DiscardDraft,
        "bob",
        AccessRoute::BrowserSession,
        Some(mutation(2)),
    )?;
    let removed = world.service.discard_draft(&bob, request()?).await?;
    assert_eq!(removed.editor, "bob");
    Ok(())
}
