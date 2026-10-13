//! Drafts belong to the caller: a save keeps its base unless it rebases on the head, refuses a
//! changed header, and listing and discarding see only the caller's own.

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::PageRequest;
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::extraction::TextOrigin;
use okf_jawn_contract::history::{CommitRequest, FileChange, FileChangeKind};
use okf_jawn_contract::identity::{At, IdempotencyKey, ItemId, PurgeId, Revision};
use okf_jawn_contract::item::{
    APP_HEADER_KEY, DiscardDraftRequest, GetItemRequest, ItemKind, ListDraftsRequest,
    SaveDraftRequest,
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

const CARD: u128 = 10;
const VIEW: u128 = 30;
const HEAD_TEXT: &str = "# Overview\nrewritten by a later re-extraction\n";

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

fn card_save(base: Revision, body: &str, properties: serde_json::Value) -> Built<SaveDraftRequest> {
    Ok(SaveDraftRequest {
        item_id: ItemId(Uuid::from_u128(CARD)),
        body: body.to_owned(),
        ..save(base, properties)?
    })
}

fn card_snapshot(items: &[u128]) -> Built<CommitRequest> {
    Ok(CommitRequest {
        item_ids: items
            .iter()
            .map(|id| ItemId(Uuid::from_u128(*id)))
            .collect(),
        ..snapshot_of_the_note()?
    })
}

/// A world with the note, a source card and a View, all at `a`, which is the head.
fn world_with_a_card_a_view_and_the_note() -> Built<World> {
    let world = World::new()?;
    stored_at(&world, 'a')?;
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
    let mut view = note(
        VIEW,
        &at,
        "views/board.md",
        "# Board\n",
        json!({ "type": "View" }),
    )?;
    view.summary.kind = ItemKind::View;
    world.versions.put_document(&at, view)?;
    world.versions.queue_revision(revision('c')?)?;
    Ok(world)
}

/// Replaces `today_a_draft_of_a_source_card_or_a_view_is_snapshotted_as_a_plain_edit` (commit
/// `d02f67a`), which pinned the old behaviour: a draft of a card or a View saved and committed
/// as a plain edit. This test and the ones after it pin the rule instead: a source's text is
/// not changed through a draft.
#[tokio::test]
async fn a_draft_of_a_source_card_may_not_change_its_text() -> TestResult {
    let world = world_with_a_card_a_view_and_the_note()?;
    let caller = alice(OperationName::SaveDraft, Some(mutation(1)))?;
    let refused = err_of(
        world
            .service
            .save_draft(
                &caller,
                card_save(
                    revision('a')?,
                    "# Overview\nedited\n",
                    json!({ "type": "Source" }),
                )?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/body"));
    assert!(
        refused.message.contains("correct_digest"),
        "{}",
        refused.message
    );
    // Both causes are named: the draft changed the text, or the card's text changed since the
    // draft's base (a caller that changed no text is told to take the current one).
    assert!(
        refused
            .message
            .contains("text changed since the draft's base: take the current text"),
        "{}",
        refused.message
    );
    assert!(world.drafts.saves()?.is_empty());
    // A body that differs in one character is refused whatever its line ends and final newline.
    for body in [
        CARD_BODY.replace("first", "firsT"),
        CARD_BODY.replace("first", "firsT").replace('\n', "\r\n"),
        format!("{CARD_BODY}x"),
    ] {
        let refused = err_of(
            world
                .service
                .save_draft(
                    &caller,
                    card_save(revision('a')?, &body, json!({ "type": "Source" }))?,
                )
                .await,
        )?;
        assert_eq!(refused.field.as_deref(), Some("/body"));
    }
    assert!(world.drafts.saves()?.is_empty());
    Ok(())
}

/// Storage's reader drops a body's final newline and turns `\r\n` into `\n`, so a client that
/// sends back the stored file's spelling sends the same text `show` gave it.
#[tokio::test]
async fn a_properties_only_draft_of_a_source_card_is_accepted_with_a_final_newline_or_crlf()
-> TestResult {
    let world = world_with_a_card_a_view_and_the_note()?;
    let at = revision('a')?;
    // The body as `show` gives it: no final newline.
    let shown = CARD_BODY.trim_end_matches('\n');
    world.versions.replace_document(
        &at,
        source_card(
            CARD,
            &at,
            "sources/report-pdf.md",
            shown,
            converted(&digest('b')?),
        )?,
    )?;
    let spellings = [
        ("a final newline", format!("{shown}\n")),
        ("CRLF line ends", shown.replace('\n', "\r\n")),
        ("both", format!("{}\r\n", shown.replace('\n', "\r\n"))),
    ];
    for (number, (name, body)) in (1..).zip(spellings) {
        let saved = world
            .service
            .save_draft(
                &alice(OperationName::SaveDraft, Some(mutation(number)))?,
                card_save(
                    at.clone(),
                    &body,
                    json!({ "type": "Source", "tags": ["quarterly"] }),
                )?,
            )
            .await;
        assert!(saved.is_ok(), "{name}: {saved:?}");
        // What is stored is the item's own body, never the request's spelling.
        let held = world.drafts.held()?;
        assert_eq!(some(held.first(), "the draft")?.body, shown, "{name}");
    }
    Ok(())
}

#[tokio::test]
async fn a_properties_only_draft_of_a_source_card_saves_and_commits_with_its_text_origin_intact()
-> TestResult {
    let world = world_with_a_card_a_view_and_the_note()?;
    let item_id = ItemId(Uuid::from_u128(CARD));
    let saved = world
        .service
        .save_draft(
            &alice(OperationName::SaveDraft, Some(mutation(1)))?,
            card_save(
                revision('a')?,
                CARD_BODY,
                json!({ "type": "Source", "tags": ["quarterly"] }),
            )?,
        )
        .await?;
    assert_eq!(saved.item_id, item_id);
    let held = world.drafts.held()?;
    assert_eq!(some(held.first(), "the draft")?.body, CARD_BODY);
    let landed = world
        .service
        .commit_items(
            &alice(OperationName::CommitItems, Some(mutation(2)))?,
            card_snapshot(&[CARD])?,
        )
        .await?;
    let commits = world.versions.commits()?;
    let commit = some(commits.last(), "the snapshot")?;
    assert!(
        matches!(
            commit.edits.as_slice(),
            [TreeEdit::EditItem { item_id: edited, body, properties, .. }]
                if *edited == item_id && body == CARD_BODY && properties.contains_key("tags")
        ),
        "{:?}",
        commit.edits
    );
    let shown = world
        .versions
        .show(&scope()?, &landed.revision, item_id)
        .await?;
    let appearance = some(shown.source, "the source appearance")?;
    assert_eq!(appearance.extraction.digest, Some(digest('b')?));
    assert_eq!(appearance.extraction.text_origin, TextOrigin::Converter);
    assert!(!appearance.extraction.corrected);
    Ok(())
}

#[tokio::test]
async fn a_source_draft_is_measured_against_its_base_not_the_head() -> TestResult {
    let world = world_with_a_card_a_view_and_the_note()?;
    // A later re-extraction changed the card's text at `b`, the head.
    let b = revision('b')?;
    world.versions.put_document(
        &b,
        source_card(
            CARD,
            &b,
            "sources/report-pdf.md",
            HEAD_TEXT,
            converted(&digest('b')?),
        )?,
    )?;
    world.versions.set_head(b.clone())?;
    let properties = || json!({ "type": "Source" });
    // The editor saw `a`: the text it kept is the text at `a`, and that is accepted.
    let first = world
        .service
        .save_draft(
            &alice(OperationName::SaveDraft, Some(mutation(1)))?,
            card_save(revision('a')?, CARD_BODY, properties())?,
        )
        .await?;
    assert_eq!(first.base_revision, revision('a')?);
    // Snapshotted now, the draft is measured against its base too: the card's file changed
    // between `a` and the head, so this is the draft conflict, not a refusal of the text.
    world.versions.put_diff(
        &revision('a')?,
        &b,
        Some(ItemId(Uuid::from_u128(CARD))),
        vec![FileChange {
            old_path: Some("sources/report-pdf.md".to_owned()),
            new_path: Some("sources/report-pdf.md".to_owned()),
            patch: "@@ -1 +1 @@\n-before\n+after\n".to_owned(),
            binary: false,
            kind: FileChangeKind::Modified,
        }],
    )?;
    let conflict = err_of(
        world
            .service
            .commit_items(
                &alice(OperationName::CommitItems, Some(mutation(5)))?,
                card_snapshot(&[CARD])?,
            )
            .await,
    )?;
    assert_eq!(conflict.code, ErrorCode::Conflict);
    assert!(
        matches!(
            conflict.detail.as_deref(),
            Some(ErrorDetail::DraftConflict { .. })
        ),
        "{conflict:?}"
    );
    assert!(world.versions.commits()?.is_empty());
    // The head's text is not the text at the draft's base: a later save that keeps the base
    // refuses it.
    let refused = err_of(
        world
            .service
            .save_draft(
                &alice(OperationName::SaveDraft, Some(mutation(2)))?,
                card_save(revision('a')?, HEAD_TEXT, properties())?,
            )
            .await,
    )?;
    assert_eq!(refused.field.as_deref(), Some("/body"));
    // A first save by another editor, based on `a`, is refused for the head's text as well.
    let other = context(
        OperationName::SaveDraft,
        "bob",
        AccessRoute::LocalOwner,
        Some(mutation(4)),
    )?;
    let refused = err_of(
        world
            .service
            .save_draft(&other, card_save(revision('a')?, HEAD_TEXT, properties())?)
            .await,
    )?;
    assert_eq!(refused.field.as_deref(), Some("/body"));
    // Naming the head rebases, and then the head's text is the one to keep.
    let rebased = world
        .service
        .save_draft(
            &alice(OperationName::SaveDraft, Some(mutation(3)))?,
            card_save(b.clone(), HEAD_TEXT, properties())?,
        )
        .await?;
    assert_eq!(rebased.base_revision, b);
    Ok(())
}

#[tokio::test]
async fn a_draft_of_a_view_is_refused_and_nothing_is_saved() -> TestResult {
    let world = world_with_a_card_a_view_and_the_note()?;
    let refused = err_of(
        world
            .service
            .save_draft(
                &alice(OperationName::SaveDraft, Some(mutation(1)))?,
                SaveDraftRequest {
                    item_id: ItemId(Uuid::from_u128(VIEW)),
                    body: "# Board\n".to_owned(),
                    ..save(revision('a')?, json!({ "type": "View" }))?
                },
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotImplemented);
    assert_eq!(refused.field.as_deref(), Some("/item_id"));
    assert!(world.drafts.saves()?.is_empty());
    Ok(())
}

/// A View's draft that already exists (saved before the rule) takes no further save either.
#[tokio::test]
async fn a_view_with_a_draft_that_predates_the_rule_takes_no_further_save() -> TestResult {
    let world = world_with_a_card_a_view_and_the_note()?;
    world
        .drafts
        .put_draft(draft_of(VIEW, ALICE, &revision('a')?, "# Not the text\n")?)?;
    let refused = err_of(
        world
            .service
            .save_draft(
                &alice(OperationName::SaveDraft, Some(mutation(1)))?,
                SaveDraftRequest {
                    item_id: ItemId(Uuid::from_u128(VIEW)),
                    body: "# Board, again\n".to_owned(),
                    ..save(revision('a')?, json!({ "type": "View" }))?
                },
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotImplemented);
    assert_eq!(refused.field.as_deref(), Some("/item_id"));
    assert!(world.drafts.saves()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_note_draft_saves_and_snapshots_as_before() -> TestResult {
    let world = world_with_a_card_a_view_and_the_note()?;
    world
        .service
        .save_draft(
            &alice(OperationName::SaveDraft, Some(mutation(1)))?,
            save(revision('a')?, json!({ "type": "Note" }))?,
        )
        .await?;
    world
        .service
        .commit_items(
            &alice(OperationName::CommitItems, Some(mutation(2)))?,
            snapshot_of_the_note()?,
        )
        .await?;
    let commits = world.versions.commits()?;
    let commit = some(commits.last(), "the snapshot")?;
    assert!(matches!(
        commit.edits.as_slice(),
        [TreeEdit::EditItem { body, .. }] if body == "# Plan, revised\n"
    ));
    Ok(())
}

#[tokio::test]
async fn a_snapshot_refuses_a_draft_that_predates_the_rules_and_commits_nothing() -> TestResult {
    let at = revision('a')?;
    // A card's draft with another text, and a View's draft, put into the store directly as an
    // archive restore would; each is selected beside the note's valid draft.
    for (id, selection, code, field) in [
        (CARD, [NOTE, CARD], ErrorCode::InvalidInput, "/body"),
        (VIEW, [NOTE, VIEW], ErrorCode::NotImplemented, "/item_id"),
    ] {
        let world = world_with_a_card_a_view_and_the_note()?;
        world
            .drafts
            .put_draft(draft_of(NOTE, ALICE, &at, "# Plan, revised\n")?)?;
        world
            .drafts
            .put_draft(draft_of(id, ALICE, &at, "# Not the text\n")?)?;
        let refused = err_of(
            world
                .service
                .commit_items(
                    &alice(OperationName::CommitItems, Some(mutation(2)))?,
                    card_snapshot(&selection)?,
                )
                .await,
        )?;
        assert_eq!(refused.code, code);
        assert_eq!(refused.field.as_deref(), Some(field));
        assert!(
            refused.message.contains(&Uuid::from_u128(id).to_string()),
            "{}",
            refused.message
        );
        assert!(world.versions.commits()?.is_empty());
        // Both drafts are still held: nothing of the Snapshot was committed or removed.
        assert_eq!(world.drafts.held()?.len(), 2);
    }
    Ok(())
}

#[tokio::test]
async fn a_refused_draft_can_be_discarded_and_get_item_still_shows_it() -> TestResult {
    let world = world_with_a_card_a_view_and_the_note()?;
    let at = revision('a')?;
    for id in [CARD, VIEW] {
        world
            .drafts
            .put_draft(draft_of(id, ALICE, &at, "# Not the text\n")?)?;
        let shown = world
            .service
            .get_item(
                &context(
                    OperationName::GetItem,
                    ALICE,
                    AccessRoute::BrowserSession,
                    None,
                )?,
                GetItemRequest {
                    workspace_id: scope()?.workspace_id,
                    item_id: ItemId(Uuid::from_u128(id)),
                    at: At::Latest,
                },
            )
            .await?;
        assert_eq!(
            shown.draft.as_ref().map(|draft| draft.body.as_str()),
            Some("# Not the text\n")
        );
        world
            .service
            .discard_draft(
                &alice(OperationName::DiscardDraft, Some(mutation(9)))?,
                DiscardDraftRequest {
                    workspace_id: scope()?.workspace_id,
                    item_id: ItemId(Uuid::from_u128(id)),
                    idempotency_key: IdempotencyKey(Uuid::from_u128(77)),
                },
            )
            .await?;
    }
    assert!(world.drafts.held()?.is_empty());
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

#[tokio::test]
async fn a_later_save_that_names_another_revision_keeps_the_drafts_base() -> TestResult {
    // The draft is based at `a`. Another editor changes the note at `b`, and an unrelated commit
    // makes `c` the head. A client that has refreshed to `b` saves again naming `b`, which is
    // neither the head nor the draft's base: the base stays `a`, so the Snapshot still conflicts
    // with the other editor's change instead of overwriting it.
    let world = World::new()?;
    for fill in ['a', 'b', 'c'] {
        stored_at(&world, fill)?;
    }
    world.versions.set_head(revision('c')?)?;
    world.versions.put_diff(
        &revision('a')?,
        &revision('c')?,
        Some(ItemId(Uuid::from_u128(NOTE))),
        vec![FileChange {
            old_path: Some("notes/plan.md".to_owned()),
            new_path: Some("notes/plan.md".to_owned()),
            patch: "@@ -1 +1 @@\n-before\n+after\n".to_owned(),
            binary: false,
            kind: FileChangeKind::Modified,
        }],
    )?;
    world.versions.queue_revision(revision('d')?)?;
    let first = world
        .service
        .save_draft(
            &alice(OperationName::SaveDraft, Some(mutation(1)))?,
            save(revision('a')?, json!({ "type": "Note" }))?,
        )
        .await?;
    assert_eq!(first.base_revision, revision('a')?);
    let later = world
        .service
        .save_draft(
            &alice(OperationName::SaveDraft, Some(mutation(2)))?,
            save(revision('b')?, json!({ "type": "Note" }))?,
        )
        .await?;
    assert_eq!(
        later.base_revision,
        revision('a')?,
        "a later save keeps the base"
    );
    let refused = err_of(
        world
            .service
            .commit_items(
                &alice(OperationName::CommitItems, Some(mutation(3)))?,
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
        .map(|item| (&item.draft_base, &item.current_revision))
        .collect();
    assert_eq!(listed, vec![(&revision('a')?, &revision('c')?)]);
    assert!(world.versions.commits()?.is_empty(), "nothing overwritten");
    Ok(())
}
