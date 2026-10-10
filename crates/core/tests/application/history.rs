//! History: Timeline, Changes and Who read at purge-checked revisions; Snapshot commits the
//! caller's own drafts in one commit and is blocked only by changes to a selected item; Rewind
//! restores the target's state in a new commit.

use std::collections::BTreeMap;

use okf_jawn_contract::common::{PageRequest, TextRange};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::history::{
    BlameLine, BlameRequest, Commit, CommitRequest, DiffRequest, FileChange, FileChangeKind,
    LogRequest, LogResponse, RestoreRequest,
};
use okf_jawn_contract::identity::{At, IdempotencyKey, ItemId, PurgeId, WorkspacePath};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::source::{SourceLocation, SourceLocator};
use okf_jawn_core::context::{Attempt, OperationContext};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::portable::content_digest;
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::TreeEdit;
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    ALICE, Built, CARD_BODY, NOTE, World, alice, context, converted, digest, draft_of, instant,
    mutation, note, record, revision, scope, source_card,
};

const OTHER: u128 = 21;
const THIRD: u128 = 22;
const CARD: u128 = 10;

fn item(id: u128) -> ItemId {
    ItemId(Uuid::from_u128(id))
}

fn key() -> IdempotencyKey {
    IdempotencyKey(Uuid::from_u128(88))
}

fn change(kind: FileChangeKind, old: Option<&str>, new: Option<&str>) -> FileChange {
    FileChange {
        old_path: old.map(str::to_owned),
        new_path: new.map(str::to_owned),
        patch: "@@ -1 +1 @@\n-before\n+after\n".to_owned(),
        binary: false,
        kind,
    }
}

fn wire_commit(fill: char) -> Built<Commit> {
    Ok(Commit {
        revision: revision(fill)?,
        parents: Vec::new(),
        message: "Snapshot".to_owned(),
        author: ALICE.to_owned(),
        committed_at: instant()?,
    })
}

fn purge(world: &World, fill: char) -> Built<PurgeId> {
    let purge_id = PurgeId(Uuid::from_u128(5));
    world.records.put_mapping(
        revision(fill)?,
        RevisionMapping {
            purge_id,
            replacement: None,
        },
    )?;
    Ok(purge_id)
}

fn assert_invalidated(refused: &ApiError, purge_id: PurgeId) {
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(
        refused.detail.as_deref(),
        Some(&ErrorDetail::Invalidated {
            purge_id,
            replacement: None
        })
    );
}

/// Put a note `id` at `location` at each revision named by `fills`.
fn note_at(world: &World, id: u128, location: &str, fills: &[char]) -> Built<()> {
    for fill in fills {
        let at = revision(*fill)?;
        world.versions.put_document(
            &at,
            note(id, &at, location, "# Plan\n", json!({ "type": "Note" }))?,
        )?;
    }
    Ok(())
}

fn snapshot(items: &[u128]) -> Built<CommitRequest> {
    Ok(CommitRequest {
        workspace_id: scope()?.workspace_id,
        item_ids: items.iter().map(|id| item(*id)).collect(),
        message: "Snapshot".to_owned(),
        idempotency_key: key(),
    })
}

fn rewind(base: char, target: char, only: Option<u128>) -> Built<RestoreRequest> {
    Ok(RestoreRequest {
        workspace_id: scope()?.workspace_id,
        base_revision: revision(base)?,
        target: revision(target)?,
        item_id: only.map(item),
        message: "Rewind".to_owned(),
        idempotency_key: key(),
    })
}

fn blame_request(id: u128, at: At, start: u32, end: u32) -> Built<BlameRequest> {
    Ok(BlameRequest {
        workspace_id: scope()?.workspace_id,
        item_id: item(id),
        at,
        lines: TextRange { start, end },
    })
}

fn blamed(line: u32, text: &str) -> Built<BlameLine> {
    Ok(BlameLine {
        line,
        text: text.to_owned(),
        commit: wire_commit('a')?,
    })
}

#[tokio::test]
async fn log_items_walks_back_from_the_head_resolved_once_for_one_item() -> TestResult {
    let world = World::new()?;
    let head = revision('a')?;
    world.versions.put_log(
        &head,
        Some(item(NOTE)),
        LogResponse {
            commits: vec![wire_commit('a')?],
            next_cursor: Some("1".to_owned()),
        },
    )?;
    let response = world
        .service
        .log_items(
            &alice(OperationName::LogItems, None)?,
            LogRequest {
                workspace_id: scope()?.workspace_id,
                item_id: Some(item(NOTE)),
                at: At::Latest,
                page: PageRequest {
                    cursor: Some("0".to_owned()),
                    limit: 20,
                },
            },
        )
        .await?;
    assert_eq!(response.commits.len(), 1);
    assert_eq!(response.next_cursor.as_deref(), Some("1"));
    let queries = world.versions.log_queries()?;
    let query = some(queries.first(), "the log query")?;
    assert_eq!(queries.len(), 1);
    assert_eq!(query.tip, head);
    assert_eq!(query.item_id, Some(item(NOTE)));
    assert_eq!(query.page.cursor.as_deref(), Some("0"));
    assert_eq!(query.page.limit, 20);
    Ok(())
}

#[tokio::test]
async fn diff_items_compares_the_two_named_revisions_of_one_item() -> TestResult {
    let world = World::new()?;
    let changes = vec![change(
        FileChangeKind::Modified,
        Some("notes/plan.md"),
        Some("notes/plan.md"),
    )];
    world.versions.put_diff(
        &revision('a')?,
        &revision('b')?,
        Some(item(NOTE)),
        changes.clone(),
    )?;
    let response = world
        .service
        .diff_items(
            &alice(OperationName::DiffItems, None)?,
            DiffRequest {
                workspace_id: scope()?.workspace_id,
                from: revision('a')?,
                to: revision('b')?,
                item_id: Some(item(NOTE)),
            },
        )
        .await?;
    assert_eq!(response.from, revision('a')?);
    assert_eq!(response.to, revision('b')?);
    assert_eq!(response.changes, changes);
    let queries = world.versions.diff_queries()?;
    let query = some(queries.first(), "the diff query")?;
    assert_eq!(
        (&query.from, &query.to, query.item_id),
        (&revision('a')?, &revision('b')?, Some(item(NOTE)))
    );
    Ok(())
}

#[tokio::test]
async fn history_ranges_are_purge_checked_before_anything_is_read() -> TestResult {
    let world = World::new()?;
    let purge_id = purge(&world, 'c')?;
    let workspace_id = scope()?.workspace_id;
    let log = err_of(
        world
            .service
            .log_items(
                &alice(OperationName::LogItems, None)?,
                LogRequest {
                    workspace_id,
                    item_id: None,
                    at: At::Revision {
                        revision: revision('c')?,
                    },
                    page: PageRequest {
                        cursor: None,
                        limit: 20,
                    },
                },
            )
            .await,
    )?;
    assert_invalidated(&log, purge_id);
    for (from, to) in [('c', 'a'), ('a', 'c')] {
        let diff = err_of(
            world
                .service
                .diff_items(
                    &alice(OperationName::DiffItems, None)?,
                    DiffRequest {
                        workspace_id,
                        from: revision(from)?,
                        to: revision(to)?,
                        item_id: None,
                    },
                )
                .await,
        )?;
        assert_invalidated(&diff, purge_id);
    }
    let blame = err_of(
        world
            .service
            .blame_item(
                &alice(OperationName::BlameItem, None)?,
                blame_request(
                    NOTE,
                    At::Revision {
                        revision: revision('c')?,
                    },
                    1,
                    1,
                )?,
            )
            .await,
    )?;
    assert_invalidated(&blame, purge_id);
    assert_eq!(world.versions.log_queries()?.len(), 0);
    assert_eq!(world.versions.diff_queries()?.len(), 0);
    assert_eq!(world.versions.blame_queries()?.len(), 0);
    assert_eq!(world.versions.shows(), 0);
    Ok(())
}

#[tokio::test]
async fn blame_item_cites_the_attributed_lines_of_the_shown_text_and_records_a_receipt()
-> TestResult {
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
        .put_object(digest('b')?, serde_json::to_vec(&record(Vec::new())?)?)?;
    world.versions.put_blame(
        &at,
        item(CARD),
        vec![blamed(2, "first")?, blamed(3, "second")?],
    )?;
    let response = world
        .service
        .blame_item(
            &alice(OperationName::BlameItem, None)?,
            blame_request(CARD, At::Latest, 2, 3)?,
        )
        .await?;
    assert_eq!(response.lines.len(), 2);
    // The card's lines are cited by its conversion record's digest and located on page 1,
    // whatever the store put in its own citation.
    assert_eq!(response.source.digest, Some(digest('b')?));
    assert_eq!(response.source.revision, at);
    assert_eq!(
        response.source.selection,
        Selection::Lines {
            range: TextRange { start: 2, end: 3 }
        }
    );
    assert_eq!(
        response.source.locations,
        vec![SourceLocation::Direct {
            locator: SourceLocator::Page { page_no: 1 }
        }]
    );
    let queries = world.versions.blame_queries()?;
    let query = some(queries.first(), "the blame query")?;
    assert_eq!(query.revision, at);
    assert_eq!(query.lines, TextRange { start: 2, end: 3 });
    let receipts = world.records.receipts()?;
    let (under, receipt) = some(receipts.first(), "the blame's receipt")?;
    assert_eq!(*under, None);
    assert_eq!(receipt.operation_id, OperationName::BlameItem);
    let recorded = some(receipt.sources.first(), "the recorded citation")?;
    assert_eq!(
        serde_json::to_value(recorded)?,
        serde_json::to_value(&response.source)?
    );
    Ok(())
}

#[tokio::test]
async fn blame_item_refuses_a_range_that_ends_before_it_starts() -> TestResult {
    let world = World::new()?;
    let refused = err_of(
        world
            .service
            .blame_item(
                &alice(OperationName::BlameItem, None)?,
                blame_request(NOTE, At::Latest, 3, 2)?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/lines/end"));
    assert_eq!(world.versions.blame_queries()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn draft_visibility_commit_items_snapshots_only_the_callers_own_drafts_in_one_commit()
-> TestResult {
    let world = World::new()?;
    let base = revision('a')?;
    note_at(&world, NOTE, "notes/plan.md", &['a'])?;
    note_at(&world, OTHER, "notes/other.md", &['a'])?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &base, "# Alice's plan\n")?)?;
    world
        .drafts
        .put_draft(draft_of(OTHER, ALICE, &base, "# Alice's other\n")?)?;
    world
        .drafts
        .put_draft(draft_of(NOTE, "bob", &base, "# Bob's plan\n")?)?;
    world.versions.queue_revision(revision('b')?)?;
    let caller = alice(OperationName::CommitItems, Some(mutation(1)))?;
    let result = world
        .service
        .commit_items(&caller, snapshot(&[NOTE, OTHER])?)
        .await?;
    assert_eq!(result.revision, revision('b')?);
    let commits = world.versions.commits()?;
    let commit = some(commits.first(), "the snapshot")?;
    assert_eq!(commits.len(), 1);
    assert_eq!(commit.expected_head, base);
    assert_eq!(commit.message, "Snapshot");
    assert!(matches!(
        commit.edits.as_slice(),
        [
            TreeEdit::EditItem { item_id: first, body: first_body, .. },
            TreeEdit::EditItem { item_id: second, body: second_body, .. },
        ] if *first == item(NOTE) && first_body == "# Alice's plan\n"
            && *second == item(OTHER) && second_body == "# Alice's other\n"
    ));
    // Alice's snapshotted drafts are removed; Bob's stays.
    let held = world.drafts.held()?;
    assert_eq!(held.len(), 1);
    let kept = some(held.first(), "Bob's draft")?;
    assert_eq!(kept.draft.editor, "bob");
    assert_eq!(world.search.indexed()?, vec![revision('b')?]);
    let events = world.events.appended()?;
    let (_, under, event) = some(events.first(), "the changed event")?;
    assert_eq!(*under, Some(mutation(1)));
    assert_eq!(event.kind, EventKind::Changed);
    assert_eq!(event.item_id, None);
    let receipts = world.records.receipts()?;
    let (under, receipt) = some(receipts.first(), "the snapshot's receipt")?;
    assert_eq!(*under, Some(mutation(1)));
    assert_eq!(receipt.id, result.receipt_id);
    Ok(())
}

#[tokio::test]
async fn commit_items_is_not_blocked_by_head_movement_that_did_not_touch_a_selected_item()
-> TestResult {
    let world = World::new()?;
    let base = revision('a')?;
    let head = revision('c')?;
    note_at(&world, NOTE, "notes/plan.md", &['a', 'c'])?;
    world.versions.set_head(head.clone())?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &base, "# Alice's plan\n")?)?;
    world.versions.queue_revision(revision('d')?)?;
    let result = world
        .service
        .commit_items(
            &alice(OperationName::CommitItems, Some(mutation(1)))?,
            snapshot(&[NOTE])?,
        )
        .await?;
    assert_eq!(result.revision, revision('d')?);
    let commits = world.versions.commits()?;
    let commit = some(commits.first(), "the snapshot")?;
    assert_eq!(commit.expected_head, head);
    // The item's own comparison from its draft's base to the head was asked, and listed nothing.
    let queries = world.versions.diff_queries()?;
    let query = some(queries.first(), "the item's comparison")?;
    assert_eq!(
        (&query.from, &query.to, query.item_id),
        (&base, &head, Some(item(NOTE)))
    );
    let events = world.events.appended()?;
    let (_, _, event) = some(events.first(), "the changed event")?;
    assert_eq!(event.item_id, Some(item(NOTE)));
    Ok(())
}

#[tokio::test]
async fn commit_items_conflict_lists_every_changed_or_deleted_item_and_commits_nothing()
-> TestResult {
    let world = World::new()?;
    let base = revision('a')?;
    let head = revision('c')?;
    note_at(&world, NOTE, "notes/plan.md", &['a', 'c'])?;
    note_at(&world, OTHER, "notes/other.md", &['a'])?;
    note_at(&world, THIRD, "notes/third.md", &['a', 'c'])?;
    world.versions.set_head(head.clone())?;
    for id in [NOTE, OTHER, THIRD] {
        world
            .drafts
            .put_draft(draft_of(id, ALICE, &base, "# Mine\n")?)?;
    }
    let edited = vec![change(
        FileChangeKind::Modified,
        Some("notes/plan.md"),
        Some("notes/plan.md"),
    )];
    let removed = vec![change(
        FileChangeKind::Removed,
        Some("notes/other.md"),
        None,
    )];
    world
        .versions
        .put_diff(&base, &head, Some(item(NOTE)), edited.clone())?;
    world
        .versions
        .put_diff(&base, &head, Some(item(OTHER)), removed.clone())?;
    world.versions.queue_revision(revision('d')?)?;
    let refused = err_of(
        world
            .service
            .commit_items(
                &alice(OperationName::CommitItems, Some(mutation(1)))?,
                snapshot(&[NOTE, OTHER, THIRD])?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    let Some(ErrorDetail::DraftConflict { items }) = refused.detail.as_deref() else {
        return Err(format!("not a draft conflict: {refused:?}").into());
    };
    assert_eq!(items.len(), 2, "{items:?}");
    let first = some(items.first(), "the edited item")?;
    assert_eq!(first.item_id, item(NOTE));
    assert_eq!(first.draft_base, base);
    assert_eq!(first.current_revision, head);
    assert!(!first.deleted);
    assert_eq!(first.changes, edited);
    let second = some(items.get(1), "the deleted item")?;
    assert_eq!(second.item_id, item(OTHER));
    assert!(second.deleted);
    assert_eq!(second.changes, removed);
    assert_eq!(world.versions.commits()?.len(), 0);
    assert_eq!(world.drafts.held()?.len(), 3);
    assert_eq!(world.records.receipts()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn commit_items_checks_every_draft_base_against_the_purge_map_first() -> TestResult {
    let world = World::new()?;
    note_at(&world, NOTE, "notes/plan.md", &['a'])?;
    note_at(&world, OTHER, "notes/other.md", &['a'])?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &revision('a')?, "# Mine\n")?)?;
    world
        .drafts
        .put_draft(draft_of(OTHER, ALICE, &revision('c')?, "# Mine\n")?)?;
    let purge_id = purge(&world, 'c')?;
    world.versions.queue_revision(revision('b')?)?;
    let refused = err_of(
        world
            .service
            .commit_items(
                &alice(OperationName::CommitItems, Some(mutation(1)))?,
                snapshot(&[NOTE, OTHER])?,
            )
            .await,
    )?;
    assert_invalidated(&refused, purge_id);
    assert_eq!(world.versions.diff_queries()?.len(), 0);
    assert_eq!(world.versions.commits()?.len(), 0);
    assert_eq!(world.versions.shows(), 0);
    Ok(())
}

#[tokio::test]
async fn commit_items_refuses_a_missing_draft_and_an_item_selected_twice() -> TestResult {
    let world = World::new()?;
    note_at(&world, NOTE, "notes/plan.md", &['a'])?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &revision('a')?, "# Mine\n")?)?;
    world.versions.queue_revision(revision('b')?)?;
    let caller = alice(OperationName::CommitItems, Some(mutation(1)))?;
    let missing = err_of(
        world
            .service
            .commit_items(&caller, snapshot(&[NOTE, OTHER])?)
            .await,
    )?;
    assert_eq!(missing.code, ErrorCode::NotFound);
    assert_eq!(missing.field.as_deref(), Some("/item_ids/1"));
    let twice = err_of(
        world
            .service
            .commit_items(&caller, snapshot(&[NOTE, NOTE])?)
            .await,
    )?;
    assert_eq!(twice.code, ErrorCode::InvalidInput);
    assert_eq!(twice.field.as_deref(), Some("/item_ids/1"));
    let none = err_of(world.service.commit_items(&caller, snapshot(&[])?).await)?;
    assert_eq!(none.code, ErrorCode::InvalidInput);
    assert_eq!(none.field.as_deref(), Some("/item_ids"));
    assert_eq!(world.versions.commits()?.len(), 0);
    assert_eq!(world.drafts.held()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_resumed_snapshot_completes_the_commit_its_first_attempt_wrote() -> TestResult {
    let world = World::new()?;
    let base = revision('a')?;
    note_at(&world, NOTE, "notes/plan.md", &['a'])?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &base, "# Mine\n")?)?;
    world.versions.queue_revision(revision('b')?)?;
    world.search.fail_next();
    let mut caller = alice(OperationName::CommitItems, Some(mutation(1)))?;
    let failed = err_of(
        world
            .service
            .commit_items(&caller, snapshot(&[NOTE])?)
            .await,
    )?;
    assert_eq!(failed.code, ErrorCode::Internal);
    assert_eq!(world.versions.commits()?.len(), 1);
    // Someone else commits before the retry, so the head is no longer the snapshot's.
    world.versions.set_head(revision('c')?)?;
    caller.attempt = Attempt::Resumed;
    let retried = world
        .service
        .commit_items(&caller, snapshot(&[NOTE])?)
        .await?;
    assert_eq!(retried.revision, revision('b')?);
    assert_eq!(world.versions.commits()?.len(), 1, "no second commit");
    assert_eq!(world.drafts.held()?.len(), 0);
    assert_eq!(world.search.indexed()?, vec![revision('b')?]);
    assert_eq!(world.events.appended()?.len(), 1);
    let receipts = world.records.receipts()?;
    let (under, receipt) = some(receipts.first(), "the snapshot's receipt")?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(*under, Some(mutation(1)));
    assert_eq!(receipt.id, retried.receipt_id);
    Ok(())
}

#[tokio::test]
async fn a_resumed_snapshot_recovers_the_drafts_its_first_attempt_removed() -> TestResult {
    let world = World::new()?;
    let base = revision('a')?;
    note_at(&world, NOTE, "notes/plan.md", &['a'])?;
    note_at(&world, OTHER, "notes/other.md", &['a'])?;
    for id in [NOTE, OTHER] {
        world
            .drafts
            .put_draft(draft_of(id, ALICE, &base, "# Mine\n")?)?;
    }
    world.versions.queue_revision(revision('b')?)?;
    world.records.fail_next_receipt();
    let mut caller = alice(OperationName::CommitItems, Some(mutation(1)))?;
    let failed = err_of(
        world
            .service
            .commit_items(&caller, snapshot(&[NOTE, OTHER])?)
            .await,
    )?;
    assert_eq!(failed.code, ErrorCode::Internal);
    assert_eq!(world.drafts.held()?.len(), 0, "removed after the commit");
    caller.attempt = Attempt::Resumed;
    let retried = world
        .service
        .commit_items(&caller, snapshot(&[NOTE, OTHER])?)
        .await?;
    assert_eq!(retried.revision, revision('b')?);
    assert_eq!(world.versions.commits()?.len(), 1);
    assert_eq!(world.records.receipts()?.len(), 1);
    Ok(())
}

/// NOTE and OTHER at `a` and `b`, Alice's draft of each based on `a`, and the revisions `b`
/// then `d` queued: the reviewer's setup for a Snapshot resumed after its first attempt
/// committed `b` and failed before removing any draft.
fn resumable() -> Built<World> {
    let world = World::new()?;
    note_at(&world, NOTE, "notes/plan.md", &['a', 'b'])?;
    note_at(&world, OTHER, "notes/other.md", &['a', 'b'])?;
    for id in [NOTE, OTHER] {
        world
            .drafts
            .put_draft(draft_of(id, ALICE, &revision('a')?, "# Mine\n")?)?;
    }
    world.versions.queue_revision(revision('b')?)?;
    world.versions.queue_revision(revision('d')?)?;
    Ok(world)
}

/// A newer draft of `id` by Alice, based on `b` and never committed.
fn newer(id: u128) -> Built<DraftWrite> {
    let body = "# Newer, never committed\n";
    Ok(DraftWrite {
        item_id: item(id),
        editor: ALICE.to_owned(),
        base_revision: revision('b')?,
        body: body.to_owned(),
        properties: BTreeMap::new(),
        content_digest: content_digest(body, &BTreeMap::new())?,
    })
}

/// Run the first attempt of a Snapshot of NOTE and OTHER, which commits `b` and fails before
/// removing a draft; answer the caller of the resumed attempt.
async fn first_attempt_fails_after_its_commit(world: &World) -> Built<OperationContext> {
    world.search.fail_next();
    let mut caller = alice(OperationName::CommitItems, Some(mutation(1)))?;
    let failed = err_of(
        world
            .service
            .commit_items(&caller, snapshot(&[NOTE, OTHER])?)
            .await,
    )?;
    assert_eq!(failed.code, ErrorCode::Internal);
    assert_eq!(world.versions.commits()?.len(), 1);
    assert_eq!(world.drafts.held()?.len(), 2, "nothing removed yet");
    caller.attempt = Attempt::Resumed;
    Ok(caller)
}

fn holds_newer(world: &World, id: u128) -> Built<bool> {
    Ok(world
        .drafts
        .held()?
        .iter()
        .any(|kept| kept.draft.item_id == item(id) && kept.body == "# Newer, never committed\n"))
}

#[tokio::test]
async fn a_resumed_snapshot_keeps_a_newer_draft_of_a_later_item() -> TestResult {
    let world = resumable()?;
    let caller = first_attempt_fails_after_its_commit(&world).await?;
    world
        .drafts
        .save(&scope()?, mutation(77), newer(OTHER)?)
        .await?;
    let retried = world
        .service
        .commit_items(&caller, snapshot(&[NOTE, OTHER])?)
        .await?;
    assert_eq!(retried.revision, revision('b')?);
    assert!(holds_newer(&world, OTHER)?, "{:?}", world.drafts.held()?);
    // The committed draft of NOTE is removed.
    assert_eq!(world.drafts.held()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_resumed_snapshot_finds_its_commit_when_the_first_draft_is_newer() -> TestResult {
    let world = resumable()?;
    // What storage answers: commit `b` changed OTHER between `a` and `b`.
    world.versions.put_diff(
        &revision('a')?,
        &revision('b')?,
        Some(item(OTHER)),
        vec![change(
            FileChangeKind::Modified,
            Some("notes/other.md"),
            Some("notes/other.md"),
        )],
    )?;
    let caller = first_attempt_fails_after_its_commit(&world).await?;
    world
        .drafts
        .save(&scope()?, mutation(77), newer(NOTE)?)
        .await?;
    let retried = world
        .service
        .commit_items(&caller, snapshot(&[NOTE, OTHER])?)
        .await?;
    assert_eq!(retried.revision, revision('b')?);
    assert_eq!(world.versions.commits()?.len(), 1);
    assert!(holds_newer(&world, NOTE)?, "{:?}", world.drafts.held()?);
    Ok(())
}

#[tokio::test]
async fn a_resumed_snapshot_whose_drafts_were_all_saved_again_writes_no_second_commit() -> TestResult
{
    let world = resumable()?;
    let caller = first_attempt_fails_after_its_commit(&world).await?;
    for id in [NOTE, OTHER] {
        world
            .drafts
            .save(&scope()?, mutation(77), newer(id)?)
            .await?;
    }
    let retried = world
        .service
        .commit_items(&caller, snapshot(&[NOTE, OTHER])?)
        .await?;
    assert_eq!(
        retried.revision,
        revision('b')?,
        "answers with the first commit"
    );
    let commits = world.versions.commits()?;
    assert_eq!(commits.len(), 1, "no second commit under the mutation");
    assert!(holds_newer(&world, NOTE)? && holds_newer(&world, OTHER)?);
    Ok(())
}

#[tokio::test]
async fn a_snapshot_keeps_a_draft_saved_again_between_its_read_and_its_removal() -> TestResult {
    let world = resumable()?;
    world.drafts.save_before_next_discard(newer(NOTE)?)?;
    let result = world
        .service
        .commit_items(
            &alice(OperationName::CommitItems, Some(mutation(1)))?,
            snapshot(&[NOTE, OTHER])?,
        )
        .await?;
    assert_eq!(result.revision, revision('b')?);
    assert!(holds_newer(&world, NOTE)?, "{:?}", world.drafts.held()?);
    assert_eq!(
        world.drafts.held()?.len(),
        1,
        "OTHER's committed draft is removed"
    );
    Ok(())
}

#[tokio::test]
async fn a_resumed_snapshot_completes_when_the_editor_discarded_a_draft_meanwhile() -> TestResult {
    let world = resumable()?;
    let caller = first_attempt_fails_after_its_commit(&world).await?;
    world
        .drafts
        .discard(&scope()?, mutation(78), item(NOTE), ALICE, None)
        .await?;
    let retried = world
        .service
        .commit_items(&caller, snapshot(&[NOTE, OTHER])?)
        .await?;
    assert_eq!(retried.revision, revision('b')?);
    assert_eq!(world.drafts.held()?.len(), 0);
    assert_eq!(world.records.receipts()?.len(), 1);
    Ok(())
}
#[tokio::test]
async fn commit_items_reports_lint_only_for_the_items_it_snapshots() -> TestResult {
    let world = World::new()?;
    let base = revision('a')?;
    note_at(&world, NOTE, "notes/bare.md", &['a'])?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &base, "No heading.\n")?)?;
    world.versions.queue_revision(revision('b')?)?;
    let stage = world.versions.stage();
    stage.write("notes/bare.md", "---\ntype: Note\n---\n\nNo heading.\n")?;
    stage.write("notes/other.md", "---\ntype: Note\n---\n\nNo heading.\n")?;
    let result = world
        .service
        .commit_items(
            &alice(OperationName::CommitItems, Some(mutation(1)))?,
            snapshot(&[NOTE])?,
        )
        .await?;
    let lint: Vec<_> = result
        .warnings
        .iter()
        .filter(|warning| warning.code.starts_with("okf_lint"))
        .collect();
    assert!(
        lint.iter()
            .any(|warning| warning.location.as_deref() == Some("notes/bare.md")),
        "{lint:?}"
    );
    assert!(
        lint.iter()
            .all(|warning| warning.location.as_deref() == Some("notes/bare.md")),
        "{lint:?}"
    );
    Ok(())
}

#[tokio::test]
async fn restore_items_restores_one_item_from_the_target_in_a_new_commit() -> TestResult {
    let world = World::new()?;
    // The note was at `notes/plan.md` at the target and moved to `archive/plan.md` since.
    note_at(&world, NOTE, "notes/plan.md", &['a'])?;
    note_at(&world, NOTE, "archive/plan.md", &['b'])?;
    world.versions.set_head(revision('b')?)?;
    world.versions.put_diff(
        &revision('b')?,
        &revision('a')?,
        Some(item(NOTE)),
        vec![change(
            FileChangeKind::Moved,
            Some("archive/plan.md"),
            Some("notes/plan.md"),
        )],
    )?;
    world.versions.queue_revision(revision('c')?)?;
    let result = world
        .service
        .restore_items(
            &alice(OperationName::RestoreItems, Some(mutation(1)))?,
            rewind('b', 'a', Some(NOTE))?,
        )
        .await?;
    assert_eq!(result.revision, revision('c')?);
    let commits = world.versions.commits()?;
    let commit = some(commits.first(), "the rewind")?;
    assert_eq!(commits.len(), 1);
    assert_eq!(commit.expected_head, revision('b')?);
    assert_eq!(commit.message, "Rewind");
    let restored: Vec<&str> = match commit.edits.as_slice() {
        [TreeEdit::RestorePaths { from, paths }] if *from == revision('a')? => {
            paths.iter().map(WorkspacePath::as_str).collect()
        }
        other => return Err(format!("not one item's restore: {other:?}").into()),
    };
    assert_eq!(restored, vec!["archive/plan.md", "notes/plan.md"]);
    let events = world.events.appended()?;
    let (_, _, event) = some(events.first(), "the changed event")?;
    assert_eq!(event.item_id, Some(item(NOTE)));
    let receipts = world.records.receipts()?;
    let (_, receipt) = some(receipts.first(), "the rewind's receipt")?;
    assert_eq!(receipt.id, result.receipt_id);
    Ok(())
}

#[tokio::test]
async fn restore_items_restores_the_whole_workspace_in_a_new_commit_on_its_base() -> TestResult {
    let world = World::new()?;
    world.versions.set_head(revision('b')?)?;
    world.versions.put_diff(
        &revision('b')?,
        &revision('a')?,
        None,
        vec![change(
            FileChangeKind::Modified,
            Some("notes/plan.md"),
            Some("notes/plan.md"),
        )],
    )?;
    world.versions.queue_revision(revision('c')?)?;
    let result = world
        .service
        .restore_items(
            &alice(OperationName::RestoreItems, Some(mutation(1)))?,
            rewind('b', 'a', None)?,
        )
        .await?;
    assert_eq!(result.revision, revision('c')?);
    let commits = world.versions.commits()?;
    let commit = some(commits.first(), "the rewind")?;
    assert_eq!(commit.expected_head, revision('b')?);
    assert!(matches!(
        commit.edits.as_slice(),
        [TreeEdit::RestoreWorkspace { from }] if *from == revision('a')?
    ));
    Ok(())
}

#[tokio::test]
async fn restore_items_checks_its_base_and_target_against_the_purge_map_first() -> TestResult {
    for (base, target) in [('c', 'a'), ('b', 'c')] {
        let world = World::new()?;
        world.versions.set_head(revision('b')?)?;
        let purge_id = purge(&world, 'c')?;
        world.versions.queue_revision(revision('d')?)?;
        let refused = err_of(
            world
                .service
                .restore_items(
                    &alice(OperationName::RestoreItems, Some(mutation(1)))?,
                    rewind(base, target, Some(NOTE))?,
                )
                .await,
        )?;
        assert_invalidated(&refused, purge_id);
        assert_eq!(world.versions.diff_queries()?.len(), 0, "{base}..{target}");
        assert_eq!(world.versions.commits()?.len(), 0);
    }
    Ok(())
}

#[tokio::test]
async fn restore_items_refuses_to_restore_nothing_or_another_items_file() -> TestResult {
    let caller = || alice(OperationName::RestoreItems, Some(mutation(1)));
    // Nothing differs.
    let world = World::new()?;
    world.versions.set_head(revision('b')?)?;
    note_at(&world, NOTE, "notes/plan.md", &['a', 'b'])?;
    let nothing = err_of(
        world
            .service
            .restore_items(&caller()?, rewind('b', 'a', Some(NOTE))?)
            .await,
    )?;
    assert_eq!(nothing.code, ErrorCode::InvalidInput);
    assert_eq!(nothing.field.as_deref(), Some("/target"));
    // The note moved away from `notes/plan.md`, and another item held that path at the target:
    // restoring the old path would bring that item back.
    let world = World::new()?;
    world.versions.set_head(revision('b')?)?;
    note_at(&world, NOTE, "notes/new.md", &['a'])?;
    note_at(&world, NOTE, "notes/plan.md", &['b'])?;
    note_at(&world, OTHER, "notes/plan.md", &['a'])?;
    world.versions.put_diff(
        &revision('b')?,
        &revision('a')?,
        Some(item(NOTE)),
        vec![change(
            FileChangeKind::Moved,
            Some("notes/plan.md"),
            Some("notes/new.md"),
        )],
    )?;
    world.versions.queue_revision(revision('c')?)?;
    let back = err_of(
        world
            .service
            .restore_items(&caller()?, rewind('b', 'a', Some(NOTE))?)
            .await,
    )?;
    assert_eq!(back.code, ErrorCode::Conflict);
    assert_eq!(back.field.as_deref(), Some("/item_id"));
    assert_eq!(world.versions.commits()?.len(), 0);
    // Another item holds the note's target path at the base: restoring it would write over it.
    let world = World::new()?;
    world.versions.set_head(revision('b')?)?;
    note_at(&world, NOTE, "notes/plan.md", &['a'])?;
    note_at(&world, NOTE, "archive/plan.md", &['b'])?;
    note_at(&world, OTHER, "notes/plan.md", &['b'])?;
    world.versions.put_diff(
        &revision('b')?,
        &revision('a')?,
        Some(item(NOTE)),
        vec![change(
            FileChangeKind::Moved,
            Some("archive/plan.md"),
            Some("notes/plan.md"),
        )],
    )?;
    world.versions.queue_revision(revision('c')?)?;
    let over = err_of(
        world
            .service
            .restore_items(&caller()?, rewind('b', 'a', Some(NOTE))?)
            .await,
    )?;
    assert_eq!(over.code, ErrorCode::Conflict);
    assert_eq!(world.versions.commits()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn restore_items_reports_lint_only_for_the_files_it_restores() -> TestResult {
    let world = World::new()?;
    world.versions.set_head(revision('b')?)?;
    world.versions.put_diff(
        &revision('b')?,
        &revision('a')?,
        None,
        vec![change(
            FileChangeKind::Modified,
            Some("notes/bare.md"),
            Some("notes/bare.md"),
        )],
    )?;
    world.versions.queue_revision(revision('c')?)?;
    let stage = world.versions.stage();
    stage.write("notes/bare.md", "---\ntype: Note\n---\n\nNo heading.\n")?;
    stage.write("notes/other.md", "---\ntype: Note\n---\n\nNo heading.\n")?;
    let result = world
        .service
        .restore_items(
            &context(
                OperationName::RestoreItems,
                ALICE,
                okf_jawn_contract::access::AccessRoute::LocalOwner,
                Some(mutation(1)),
            )?,
            rewind('b', 'a', None)?,
        )
        .await?;
    let lint: Vec<_> = result
        .warnings
        .iter()
        .filter(|warning| warning.code.starts_with("okf_lint"))
        .collect();
    assert!(
        lint.iter()
            .any(|warning| warning.location.as_deref() == Some("notes/bare.md")),
        "{lint:?}"
    );
    assert!(
        lint.iter()
            .all(|warning| warning.location.as_deref() == Some("notes/bare.md")),
        "{lint:?}"
    );
    Ok(())
}
