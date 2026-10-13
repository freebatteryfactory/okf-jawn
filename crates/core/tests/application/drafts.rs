//! Drafts belong to the caller: a save keeps its base unless it rebases on the head, refuses a
//! changed header, and listing and discarding see only the caller's own.

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::PageRequest;
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::extraction::TextOrigin;
use okf_jawn_contract::history::{CommitRequest, FileChange, FileChangeKind};
use okf_jawn_contract::identity::{IdempotencyKey, ItemId, PurgeId, Revision};
use okf_jawn_contract::item::{
    APP_HEADER_KEY, DiscardDraftRequest, ItemKind, ListDraftsRequest, SaveDraftRequest,
};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::portable::content_digest;
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::{TreeEdit, VersionStore};
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    ALICE, Built, CARD_BODY, NOTE, World, alice, context, converted, digest, draft_of, mutation,
    note, revision, scope, served_digest, source_card,
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
    // The editor saw the head: the first save's base is the head, as it was before.
    let first = world
        .service
        .save_draft(
            &alice(OperationName::SaveDraft, Some(mutation(1)))?,
            save(revision('a')?, json!({ "type": "Note" }))?,
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

/// A world whose head is `b`, with the fixture note at `a` and at `b`: the editor's view is
/// still at `a`. `changed` says whether the commit between them changed the note itself.
fn behind_the_head(changed: bool) -> Built<World> {
    let world = World::new()?;
    stored_at(&world, 'a')?;
    stored_at(&world, 'b')?;
    world.versions.set_head(revision('b')?)?;
    if changed {
        world.versions.put_diff(
            &revision('a')?,
            &revision('b')?,
            Some(ItemId(Uuid::from_u128(NOTE))),
            vec![FileChange {
                old_path: Some("notes/plan.md".to_owned()),
                new_path: Some("notes/plan.md".to_owned()),
                patch: "@@ -1 +1 @@\n-before\n+after\n".to_owned(),
                binary: false,
                kind: FileChangeKind::Modified,
            }],
        )?;
    }
    world.versions.queue_revision(revision('c')?)?;
    Ok(world)
}

fn snapshot_of_the_note() -> Built<CommitRequest> {
    Ok(CommitRequest {
        workspace_id: scope()?.workspace_id,
        item_ids: vec![ItemId(Uuid::from_u128(NOTE))],
        message: "Snapshot".to_owned(),
        idempotency_key: IdempotencyKey(Uuid::from_u128(78)),
    })
}

#[tokio::test]
async fn a_first_save_records_the_revision_the_editor_saw_so_a_snapshot_conflicts_on_it()
-> TestResult {
    // The editor began from a view at `a`; another editor changed the same note at `b`.
    let world = behind_the_head(true)?;
    let first = world
        .service
        .save_draft(
            &alice(OperationName::SaveDraft, Some(mutation(1)))?,
            save(revision('a')?, json!({ "type": "Note" }))?,
        )
        .await?;
    assert_eq!(first.base_revision, revision('a')?);
    let refused = err_of(
        world
            .service
            .commit_items(
                &alice(OperationName::CommitItems, Some(mutation(2)))?,
                snapshot_of_the_note()?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    let Some(ErrorDetail::DraftConflict { items }) = refused.detail.as_deref() else {
        return Err(format!("not a draft conflict: {refused:?}").into());
    };
    let listed: Vec<_> = items
        .iter()
        .map(|item| (item.item_id, &item.draft_base, &item.current_revision))
        .collect();
    assert_eq!(
        listed,
        vec![(
            ItemId(Uuid::from_u128(NOTE)),
            &revision('a')?,
            &revision('b')?
        )]
    );
    assert!(world.versions.commits()?.is_empty(), "nothing overwritten");
    // The rebase still works: a save that names the head moves the base, and the Snapshot
    // lands.
    let rebased = world
        .service
        .save_draft(
            &alice(OperationName::SaveDraft, Some(mutation(3)))?,
            save(revision('b')?, json!({ "type": "Note" }))?,
        )
        .await?;
    assert_eq!(rebased.base_revision, revision('b')?);
    let landed = world
        .service
        .commit_items(
            &alice(OperationName::CommitItems, Some(mutation(4)))?,
            snapshot_of_the_note()?,
        )
        .await?;
    assert_eq!(landed.revision, revision('c')?);
    Ok(())
}

#[tokio::test]
async fn a_first_save_behind_an_unrelated_change_does_not_block_its_snapshot() -> TestResult {
    // The commit between the editor's view and the head changed another item.
    let world = behind_the_head(false)?;
    let first = world
        .service
        .save_draft(
            &alice(OperationName::SaveDraft, Some(mutation(1)))?,
            save(revision('a')?, json!({ "type": "Note" }))?,
        )
        .await?;
    assert_eq!(first.base_revision, revision('a')?);
    let landed = world
        .service
        .commit_items(
            &alice(OperationName::CommitItems, Some(mutation(2)))?,
            snapshot_of_the_note()?,
        )
        .await?;
    assert_eq!(landed.revision, revision('c')?);
    Ok(())
}

#[tokio::test]
async fn a_first_save_based_where_the_item_does_not_exist_is_refused_not_recorded() -> TestResult {
    // The note exists at the head `b` only.
    let world = World::new()?;
    stored_at(&world, 'b')?;
    world.versions.set_head(revision('b')?)?;
    let caller = alice(OperationName::SaveDraft, Some(mutation(1)))?;
    let request = || save(revision('a')?, json!({ "type": "Note" }));
    let refused = err_of(world.service.save_draft(&caller, request()?).await)?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(refused.field.as_deref(), Some("/base_revision"));
    // A store fault while reading the base is the store's error, not "no such item".
    world.versions.fail_show_number(2, ErrorCode::Unavailable)?;
    let failed = err_of(world.service.save_draft(&caller, request()?).await)?;
    assert_eq!(failed.code, ErrorCode::Unavailable);
    assert_eq!(failed.field, None);
    assert!(world.drafts.saves()?.is_empty());
    Ok(())
}

/// What happens today, pinned for the owner's question and not as a rule (sweep part 1, fix
/// round 1, item 7): a draft of a source card, and of a View, is saved and snapshotted exactly
/// as a note's is. The Snapshot is one `EditItem`: the card's shown text is replaced, no
/// correction is recorded, and the card still says its text is the converter's, uncorrected.
#[tokio::test]
async fn today_a_draft_of_a_source_card_or_a_view_is_snapshotted_as_a_plain_edit() -> TestResult {
    const CARD: u128 = 10;
    const VIEW: u128 = 30;
    for (id, kind, properties) in [
        (CARD, "Source", json!({ "type": "Source" })),
        (VIEW, "View", json!({ "type": "View" })),
    ] {
        let world = World::new()?;
        let head = revision('a')?;
        world.versions.put_document(
            &head,
            source_card(
                CARD,
                &head,
                "sources/report-pdf.md",
                CARD_BODY,
                converted(&digest('b')?),
            )?,
        )?;
        let mut view = note(
            VIEW,
            &head,
            "views/board.md",
            "# Board
",
            json!({ "type": "View" }),
        )?;
        view.summary.kind = ItemKind::View;
        world.versions.put_document(&head, view)?;
        world.versions.queue_revision(revision('c')?)?;
        let item_id = ItemId(Uuid::from_u128(id));
        // Saving is not refused for the kind.
        let saved = world
            .service
            .save_draft(
                &alice(OperationName::SaveDraft, Some(mutation(1)))?,
                SaveDraftRequest {
                    item_id,
                    ..save(head, properties)?
                },
            )
            .await?;
        assert_eq!(saved.item_id, item_id, "{kind}");
        // Nor is the Snapshot, and it is the edit a note's draft is.
        let landed = world
            .service
            .commit_items(
                &alice(OperationName::CommitItems, Some(mutation(2)))?,
                CommitRequest {
                    item_ids: vec![item_id],
                    ..snapshot_of_the_note()?
                },
            )
            .await?;
        let commits = world.versions.commits()?;
        let commit = some(commits.last(), "the snapshot")?;
        assert!(
            matches!(
                commit.edits.as_slice(),
                [TreeEdit::EditItem { item_id: edited, body, .. }]
                    if *edited == item_id && body == "# Plan, revised\n"
            ),
            "{kind}: {:?}",
            commit.edits
        );
        let shown = world
            .versions
            .show(&scope()?, &landed.revision, item_id)
            .await?;
        assert_eq!(shown.body, "# Plan, revised\n", "{kind}");
        if let Some(appearance) = shown.source {
            // The card still names the converter's record and calls the text the converter's.
            let extraction = appearance.extraction;
            assert_eq!(extraction.digest, Some(digest('b')?));
            assert_eq!(extraction.text_origin, TextOrigin::Converter);
            assert!(!extraction.corrected, "no correction was recorded");
        }
    }
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
