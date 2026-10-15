//! History: Timeline, Changes and Who read at purge-checked revisions; Snapshot commits the
//! caller's own drafts in one commit and is blocked only by changes to a selected item; Rewind
//! restores the target's state in a new commit.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::{PageRequest, TextRange};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::events::{EventKind, ReceiptAudience};
use okf_jawn_contract::history::{
    BlameLine, BlameRequest, Commit, CommitRequest, DiffRequest, FileChange, FileChangeKind,
    LogRequest, LogResponse, RestoreRequest,
};
use okf_jawn_contract::identity::{At, IdempotencyKey, ItemId, PurgeId};
use okf_jawn_contract::item::{ItemDocument, ListItemsRequest};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::source::{SourceLocation, SourceLocator};
use okf_jawn_core::context::{Attempt, OperationContext};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::portable::draft_digest;
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::{CommitChanges, Provenance, TreeEdit, VersionStore};
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    ALICE, Built, CARD_BODY, NOTE, World, alice, context, converted, digest, draft_of, file_patch,
    file_text, instant, mutation, note, path, record, revision, scope, source_card,
};

const OTHER: u128 = 21;
const THIRD: u128 = 22;
const CARD: u128 = 10;
/// The properties a `filed` note holds besides its type: with it, three frontmatter keys, so
/// its body starts on file line 7.
const HEADER: [(&str, &str); 2] = [("author", "Ana"), ("tags", "plans")];
/// `HEADER` with another author: a change of properties.
const RESIGNED: [(&str, &str); 2] = [("author", "Ben"), ("tags", "plans")];

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
    let request = |cursor: Option<String>| -> Built<LogRequest> {
        Ok(LogRequest {
            workspace_id: scope()?.workspace_id,
            item_id: Some(item(NOTE)),
            at: At::Latest,
            page: PageRequest { cursor, limit: 20 },
        })
    };
    let caller = alice(OperationName::LogItems, None)?;
    let response = world.service.log_items(&caller, request(None)?).await?;
    assert_eq!(response.commits.len(), 1);
    // The continuation is this service's cursor, not the store's own.
    let next = some(response.next_cursor, "a continuation")?;
    assert_ne!(next, "1");
    let queries = world.versions.log_queries()?;
    let query = some(queries.first(), "the log query")?;
    assert_eq!(queries.len(), 1);
    assert_eq!(query.tip, head);
    assert_eq!(query.item_id, Some(item(NOTE)));
    assert_eq!(query.page.cursor, None);
    assert_eq!(query.page.limit, 20);
    // The store gets its own cursor back, and the same tip.
    world
        .service
        .log_items(&caller, request(Some(next))?)
        .await?;
    let queries = world.versions.log_queries()?;
    let query = some(queries.get(1), "the second log query")?;
    assert_eq!(query.tip, head);
    assert_eq!(query.page.cursor.as_deref(), Some("1"));
    assert_eq!(query.page.limit, 20);
    Ok(())
}

/// A Timeline of the workspace at `at`, `limit` commits a page.
fn timeline(at: At, cursor: Option<String>, limit: u16) -> Built<LogRequest> {
    Ok(LogRequest {
        workspace_id: scope()?.workspace_id,
        item_id: None,
        at,
        page: PageRequest { cursor, limit },
    })
}

/// A world whose head `a` has the history `3`, `2`, `1`, newest first, read two commits a page;
/// once the head is `b`, a Snapshot on top of `a`, its history is `b`, `3`, `2`, `1`.
fn paged_timeline() -> Built<World> {
    let world = World::new()?;
    let history = |fills: &[char]| -> Built<LogResponse> {
        Ok(LogResponse {
            commits: fills
                .iter()
                .map(|fill| wire_commit(*fill))
                .collect::<Built<_>>()?,
            next_cursor: None,
        })
    };
    world
        .versions
        .put_log(&revision('a')?, None, history(&['3', '2', '1'])?)?;
    world
        .versions
        .put_log(&revision('b')?, None, history(&['b', '3', '2', '1'])?)?;
    world.versions.page_log(2)?;
    Ok(world)
}

#[tokio::test]
async fn a_snapshot_between_two_pages_of_a_timeline_repeats_and_skips_no_commit() -> TestResult {
    let world = paged_timeline()?;
    let caller = alice(OperationName::LogItems, None)?;
    let first = world
        .service
        .log_items(&caller, timeline(At::Latest, None, 2)?)
        .await?;
    let cursor = some(first.next_cursor, "a continuation")?;
    // A Snapshot lands: `latest` is `b` now, whose second page starts one commit earlier.
    world.versions.set_head(revision('b')?)?;
    let second = world
        .service
        .log_items(&caller, timeline(At::Latest, Some(cursor), 2)?)
        .await?;
    assert_eq!(second.next_cursor, None);
    let listed: Vec<_> = first
        .commits
        .iter()
        .chain(&second.commits)
        .map(|commit| commit.revision.clone())
        .collect();
    assert_eq!(
        listed,
        vec![revision('3')?, revision('2')?, revision('1')?],
        "each commit of the Timeline once"
    );
    // Both pages walked back from the tip the first page resolved.
    let tips: Vec<_> = world
        .versions
        .log_queries()?
        .into_iter()
        .map(|query| (query.tip, query.page.cursor))
        .collect();
    assert_eq!(
        tips,
        vec![
            (revision('a')?, None),
            (revision('a')?, Some("2".to_owned()))
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_timeline_cursor_is_refused_unless_it_is_a_timelines_for_the_revision_named() -> TestResult
{
    let world = paged_timeline()?;
    let caller = alice(OperationName::LogItems, None)?;
    let first = world
        .service
        .log_items(&caller, timeline(At::Latest, None, 2)?)
        .await?;
    let cursor = some(first.next_cursor, "a continuation")?;
    let asked = world.versions.log_queries()?.len();
    let named = |fill: char| -> Built<At> {
        Ok(At::Revision {
            revision: revision(fill)?,
        })
    };
    // Not a cursor at all; the store's own cursor; JSON of another shape; a cursor with a field
    // this service never wrote; and a real cursor cut short.
    let shaped = |value: serde_json::Value| URL_SAFE_NO_PAD.encode(value.to_string());
    let tampered = [
        "not-a-cursor".to_owned(),
        "2".to_owned(),
        shaped(json!({})),
        shaped(json!({ "op": "log_items", "tip": "a".repeat(40), "next": "2", "limit": 500 })),
        shaped(json!({ "op": "log_items", "tip": "not a revision", "next": "2" })),
        // The right shape with no operation, and with another operation's name.
        shaped(json!({ "tip": "a".repeat(40), "next": "2" })),
        shaped(json!({ "op": "list_items", "tip": "a".repeat(40), "next": "2" })),
        cursor
            .chars()
            .take(cursor.len().saturating_sub(3))
            .collect(),
    ];
    for text in tampered {
        let refused = err_of(
            world
                .service
                .log_items(&caller, timeline(At::Latest, Some(text.clone()), 2)?)
                .await,
        )?;
        assert_eq!(refused.code, ErrorCode::InvalidInput, "{text}");
        assert_eq!(refused.field.as_deref(), Some("/page/cursor"), "{text}");
    }
    // A real cursor, with another revision named than the one it continues.
    world.versions.set_head(revision('b')?)?;
    let elsewhere = err_of(
        world
            .service
            .log_items(&caller, timeline(named('b')?, Some(cursor.clone()), 2)?)
            .await,
    )?;
    assert_eq!(elsewhere.code, ErrorCode::InvalidInput);
    assert_eq!(elsewhere.field.as_deref(), Some("/page/cursor"));
    assert_eq!(
        world.versions.log_queries()?.len(),
        asked,
        "no refused cursor reached the store"
    );
    // The Timeline's own cursor is no listing's cursor.
    let elsewhere = err_of(
        world
            .service
            .list_items(
                &alice(OperationName::ListItems, None)?,
                ListItemsRequest {
                    workspace_id: scope()?.workspace_id,
                    at: At::Latest,
                    folder: String::new(),
                    extraction: None,
                    page: PageRequest {
                        cursor: Some(cursor.clone()),
                        limit: 2,
                    },
                },
            )
            .await,
    )?;
    assert_eq!(elsewhere.code, ErrorCode::InvalidInput);
    assert_eq!(elsewhere.field.as_deref(), Some("/page/cursor"));
    assert_eq!(world.versions.list_queries()?.len(), 0);
    // The cursor is not signed: one a caller writes in the Timeline's shape, with its name, is
    // accepted, and reads what `at` naming that tip reads, from that position.
    let written = shaped(json!({ "op": "log_items", "tip": "a".repeat(40), "next": "2" }));
    let read = world
        .service
        .log_items(&caller, timeline(At::Latest, Some(written), 2)?)
        .await?;
    assert_eq!(read.commits.len(), 1);
    // Naming the revision it continues is the same read.
    let continued = world
        .service
        .log_items(&caller, timeline(named('a')?, Some(cursor), 2)?)
        .await?;
    let listed: Vec<_> = continued
        .commits
        .iter()
        .map(|commit| commit.revision.clone())
        .collect();
    assert_eq!(listed, vec![revision('1')?]);
    Ok(())
}

#[tokio::test]
async fn a_timeline_cursor_minted_before_a_purge_does_not_read_the_purged_revision() -> TestResult {
    let world = paged_timeline()?;
    let caller = alice(OperationName::LogItems, None)?;
    let first = world
        .service
        .log_items(&caller, timeline(At::Latest, None, 2)?)
        .await?;
    let cursor = some(first.next_cursor, "a continuation")?;
    let purge_id = purge(&world, 'a')?;
    let refused = err_of(
        world
            .service
            .log_items(&caller, timeline(At::Latest, Some(cursor), 2)?)
            .await,
    )?;
    assert_invalidated(&refused, purge_id);
    assert_eq!(world.versions.log_queries()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_timeline_of_three_pages_reads_every_page_at_the_tip_the_first_resolved() -> TestResult {
    // The cursor a continuation returns carries the first page's tip too: a Snapshot lands
    // after each page, and each later head has another history.
    let world = World::new()?;
    let history = |fills: &[char]| -> Built<LogResponse> {
        Ok(LogResponse {
            commits: fills
                .iter()
                .map(|fill| wire_commit(*fill))
                .collect::<Built<_>>()?,
            next_cursor: None,
        })
    };
    for (tip, fills) in [
        ('a', "54321"),
        ('b', "b54321"),
        ('c', "cb54321"),
        ('d', "dcb54321"),
    ] {
        let fills: Vec<char> = fills.chars().collect();
        world
            .versions
            .put_log(&revision(tip)?, None, history(&fills)?)?;
    }
    world.versions.page_log(2)?;
    let caller = alice(OperationName::LogItems, None)?;
    let mut listed = Vec::new();
    let mut cursor = None;
    for landing in ['b', 'c', 'd'] {
        let page = world
            .service
            .log_items(&caller, timeline(At::Latest, cursor.take(), 2)?)
            .await?;
        listed.extend(page.commits.into_iter().map(|commit| commit.revision));
        cursor = page.next_cursor;
        world.versions.set_head(revision(landing)?)?;
    }
    assert_eq!(cursor, None, "three pages");
    let expected: Vec<_> = "54321".chars().map(revision).collect::<Built<_>>()?;
    assert_eq!(listed, expected, "each commit once, over three pages");
    let asked: Vec<_> = world
        .versions
        .log_queries()?
        .into_iter()
        .map(|query| (query.tip, query.page.cursor))
        .collect();
    let tip = revision('a')?;
    assert_eq!(
        asked,
        vec![
            (tip.clone(), None),
            (tip.clone(), Some("2".to_owned())),
            (tip, Some("4".to_owned()))
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_timeline_continuation_that_names_its_revision_is_purge_checked() -> TestResult {
    // A caller that pins its reads to a revision: on a continuation `at` is not resolved again,
    // so the check of the cursor's tip is the only purge check of the revision it names.
    let world = paged_timeline()?;
    let caller = alice(OperationName::LogItems, None)?;
    let first = world
        .service
        .log_items(&caller, timeline(At::Latest, None, 2)?)
        .await?;
    let cursor = some(first.next_cursor, "a continuation")?;
    let purge_id = purge(&world, 'a')?;
    let named = At::Revision {
        revision: revision('a')?,
    };
    let refused = err_of(
        world
            .service
            .log_items(&caller, timeline(named, Some(cursor), 2)?)
            .await,
    )?;
    assert_invalidated(&refused, purge_id);
    assert_eq!(world.versions.log_queries()?.len(), 1);
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

fn patched(kind: FileChangeKind, old: Option<&str>, new: Option<&str>, patch: &str) -> FileChange {
    FileChange {
        patch: patch.to_owned(),
        ..change(kind, old, new)
    }
}

#[tokio::test]
async fn diff_items_records_a_receipt_citing_each_hunk_side_it_returned() -> TestResult {
    let world = World::new()?;
    note_at(&world, NOTE, "notes/plan.md", &['a', 'b'])?;
    // The note's file is `---`, `type: "Note"`, `---`, a blank line, then the body from file
    // line 5, so these hunks cover body lines 2 to 4 and 10 at `a`, and 2 to 5 at `b`.
    let changes = vec![
        patched(
            FileChangeKind::Modified,
            Some("notes/plan.md"),
            Some("notes/plan.md"),
            "--- a\n+++ b\n@@ -6,3 +6,4 @@\n x\n@@ -14 +15,0 @@\n-y\n",
        ),
        // A companion file holds no item, so nothing of it is cited.
        patched(
            FileChangeKind::Modified,
            Some(".okf/card.json"),
            Some(".okf/card.json"),
            "@@ -1 +1 @@\n",
        ),
    ];
    world
        .versions
        .put_diff(&revision('a')?, &revision('b')?, None, changes)?;
    let agent = context(
        OperationName::DiffItems,
        "agent",
        AccessRoute::McpDelegation,
        None,
    )?;
    world
        .service
        .diff_items(
            &agent,
            DiffRequest {
                workspace_id: scope()?.workspace_id,
                from: revision('a')?,
                to: revision('b')?,
                item_id: None,
            },
        )
        .await?;
    let receipts = world.records.receipts()?;
    let (under, receipt) = some(receipts.first(), "the diff's receipt")?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(*under, None);
    assert_eq!(receipt.operation_id, OperationName::DiffItems);
    assert_eq!(receipt.audience, ReceiptAudience::AgentContext);
    let cited: Vec<_> = receipt
        .sources
        .iter()
        .map(|source| {
            (
                source.item_id,
                source.revision.clone(),
                source.selection.clone(),
            )
        })
        .collect();
    assert_eq!(
        cited,
        vec![
            (item(NOTE), revision('a')?, lines(2, 4)),
            (item(NOTE), revision('a')?, lines(10, 10)),
            (item(NOTE), revision('b')?, lines(2, 5)),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn diff_items_cites_only_the_to_side_of_an_added_item() -> TestResult {
    let world = World::new()?;
    let added = filed(&world, NOTE, 'b', "notes/new.md", "a\nb\nc\n", &HEADER)?;
    world.versions.put_diff(
        &revision('a')?,
        &revision('b')?,
        None,
        vec![patched(
            FileChangeKind::Added,
            None,
            Some("notes/new.md"),
            &file_patch(None, Some(&added)),
        )],
    )?;
    world
        .service
        .diff_items(
            &alice(OperationName::DiffItems, None)?,
            DiffRequest {
                workspace_id: scope()?.workspace_id,
                from: revision('a')?,
                to: revision('b')?,
                item_id: None,
            },
        )
        .await?;
    let receipts = world.records.receipts()?;
    let (_, receipt) = some(receipts.first(), "the diff's receipt")?;
    assert_eq!(receipt.audience, ReceiptAudience::HumanDisplay);
    assert_eq!(receipt.sources.len(), 1);
    let only = some(receipt.sources.first(), "the added item's citation")?;
    assert_eq!(only.revision, revision('b')?);
    // The added file's one hunk shows every line, its frontmatter included.
    assert_eq!(only.selection, Selection::All);
    Ok(())
}

#[tokio::test]
async fn diff_items_cites_a_binary_change_whole() -> TestResult {
    let world = World::new()?;
    note_at(&world, NOTE, "notes/plan.md", &['a'])?;
    world.versions.put_diff(
        &revision('a')?,
        &revision('b')?,
        None,
        vec![FileChange {
            binary: true,
            patch: String::new(),
            ..change(FileChangeKind::Removed, Some("notes/plan.md"), None)
        }],
    )?;
    world
        .service
        .diff_items(
            &alice(OperationName::DiffItems, None)?,
            DiffRequest {
                workspace_id: scope()?.workspace_id,
                from: revision('a')?,
                to: revision('b')?,
                item_id: None,
            },
        )
        .await?;
    let receipts = world.records.receipts()?;
    let (_, receipt) = some(receipts.first(), "the diff's receipt")?;
    let only = some(receipt.sources.first(), "the citation")?;
    assert_eq!(receipt.sources.len(), 1);
    assert_eq!(only.selection, Selection::All);
    assert_eq!(only.revision, revision('a')?);
    Ok(())
}

/// A note `id` at `location`, readable at `at`, whose frontmatter holds its type and `extra`
/// properties, with `body`. Returned so a test can write the patch between two of them.
fn filed(
    world: &World,
    id: u128,
    at: char,
    location: &str,
    body: &str,
    extra: &[(&str, &str)],
) -> Built<ItemDocument> {
    let mut properties = serde_json::Map::new();
    properties.insert("type".to_owned(), json!("Note"));
    for (key, value) in extra {
        properties.insert((*key).to_owned(), json!(value));
    }
    let revision = revision(at)?;
    let document = note(
        id,
        &revision,
        location,
        body,
        serde_json::Value::Object(properties),
    )?;
    world.versions.put_document(&revision, document.clone())?;
    Ok(document)
}

/// A body of `count` numbered lines, those numbered in `edited` changed.
fn numbered(count: usize, edited: &[usize]) -> String {
    (1..=count).fold(String::new(), |mut body, line| {
        body.push_str("line ");
        body.push_str(&line.to_string());
        if edited.contains(&line) {
            body.push_str(", edited");
        }
        body.push('\n');
        body
    })
}

/// Run Changes from `a` to `b`, optionally of NOTE, and return what its receipt cites: the
/// item, the revision, the path and the selection of each source.
async fn cited_by_diff(
    world: &World,
    only: Option<u128>,
) -> Built<Vec<(ItemId, char, String, Selection)>> {
    world
        .service
        .diff_items(
            &alice(OperationName::DiffItems, None)?,
            DiffRequest {
                workspace_id: scope()?.workspace_id,
                from: revision('a')?,
                to: revision('b')?,
                item_id: only.map(item),
            },
        )
        .await?;
    let receipts = world.records.receipts()?;
    let (_, receipt) = some(receipts.first(), "the diff's receipt")?;
    let mut cited = Vec::new();
    for source in &receipt.sources {
        let side = if source.revision == revision('a')? {
            'a'
        } else {
            'b'
        };
        cited.push((
            source.item_id,
            side,
            source.path.as_str().to_owned(),
            source.selection.clone(),
        ));
    }
    Ok(cited)
}

/// The body lines `start` to `end`.
fn lines(start: u32, end: u32) -> Selection {
    Selection::Lines {
        range: TextRange { start, end },
    }
}

/// NOTE edited from `a` to `b` at `notes/plan.md`: `before` and `after` bodies and extra
/// properties, its patch numbered over the whole file as storage's libgit2 numbers it.
fn edited_between(
    world: &World,
    (before, before_extra): (String, &[(&str, &str)]),
    (after, after_extra): (String, &[(&str, &str)]),
) -> Built<()> {
    let old = filed(world, NOTE, 'a', "notes/plan.md", &before, before_extra)?;
    let new = filed(world, NOTE, 'b', "notes/plan.md", &after, after_extra)?;
    world.versions.put_diff(
        &revision('a')?,
        &revision('b')?,
        None,
        vec![patched(
            FileChangeKind::Modified,
            Some("notes/plan.md"),
            Some("notes/plan.md"),
            &file_patch(Some(&old), Some(&new)),
        )],
    )?;
    Ok(())
}

#[tokio::test]
async fn diff_items_cites_a_body_only_edit_by_the_body_lines_of_each_hunk() -> TestResult {
    let world = World::new()?;
    edited_between(
        &world,
        (numbered(20, &[]), HEADER.as_slice()),
        (numbered(20, &[6, 16]), HEADER.as_slice()),
    )?;
    // Two hunks, each the edited line with three lines of context: body lines 3 to 9 and 13
    // to 19, which the file numbers 6 lines later.
    let path = "notes/plan.md".to_owned();
    assert_eq!(
        cited_by_diff(&world, None).await?,
        vec![
            (item(NOTE), 'a', path.clone(), lines(3, 9)),
            (item(NOTE), 'a', path.clone(), lines(13, 19)),
            (item(NOTE), 'b', path.clone(), lines(3, 9)),
            (item(NOTE), 'b', path, lines(13, 19)),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn diff_items_cites_a_side_whole_when_its_properties_changed() -> TestResult {
    let world = World::new()?;
    let body = numbered(20, &[]);
    edited_between(
        &world,
        (body.clone(), HEADER.as_slice()),
        (body, RESIGNED.as_slice()),
    )?;
    let path = "notes/plan.md".to_owned();
    assert_eq!(
        cited_by_diff(&world, None).await?,
        vec![
            (item(NOTE), 'a', path.clone(), Selection::All),
            (item(NOTE), 'b', path, Selection::All),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn diff_items_cites_a_side_once_whole_when_both_its_properties_and_body_changed() -> TestResult
{
    let world = World::new()?;
    edited_between(
        &world,
        (numbered(20, &[]), HEADER.as_slice()),
        (numbered(20, &[16]), RESIGNED.as_slice()),
    )?;
    // The body hunk (lines 13 to 19) is cited by the side's All, not again as lines.
    let path = "notes/plan.md".to_owned();
    assert_eq!(
        cited_by_diff(&world, None).await?,
        vec![
            (item(NOTE), 'a', path.clone(), Selection::All),
            (item(NOTE), 'b', path, Selection::All),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn diff_items_cites_a_side_whole_when_a_hunk_returns_frontmatter_as_context() -> TestResult {
    let world = World::new()?;
    edited_between(
        &world,
        (numbered(20, &[]), HEADER.as_slice()),
        (numbered(20, &[2]), HEADER.as_slice()),
    )?;
    // Body line 2's context reaches the blank line and the closing delimiter: the patch shows
    // frontmatter, which no line selection names.
    let path = "notes/plan.md".to_owned();
    assert_eq!(
        cited_by_diff(&world, None).await?,
        vec![
            (item(NOTE), 'a', path.clone(), Selection::All),
            (item(NOTE), 'b', path, Selection::All),
        ]
    );
    Ok(())
}

/// NOTE moved from `notes/plan.md` at `a` to `archive/plan.md` at `b` and its body line 16
/// edited; the comparison is scripted for `only` (an unfiltered or an item-filtered request).
fn moved_and_edited(only: Option<u128>) -> Built<World> {
    let world = World::new()?;
    let old = filed(
        &world,
        NOTE,
        'a',
        "notes/plan.md",
        &numbered(20, &[]),
        &HEADER,
    )?;
    let new = filed(
        &world,
        NOTE,
        'b',
        "archive/plan.md",
        &numbered(20, &[16]),
        &HEADER,
    )?;
    world.versions.put_diff(
        &revision('a')?,
        &revision('b')?,
        only.map(item),
        vec![patched(
            FileChangeKind::Moved,
            Some("notes/plan.md"),
            Some("archive/plan.md"),
            &file_patch(Some(&old), Some(&new)),
        )],
    )?;
    Ok(world)
}

#[tokio::test]
async fn diff_items_cites_a_moved_item_at_its_old_path_and_its_new_path() -> TestResult {
    for only in [None, Some(NOTE)] {
        let world = moved_and_edited(only)?;
        assert_eq!(
            cited_by_diff(&world, only).await?,
            vec![
                (item(NOTE), 'a', "notes/plan.md".to_owned(), lines(13, 19)),
                (item(NOTE), 'b', "archive/plan.md".to_owned(), lines(13, 19)),
            ],
            "item filter {only:?}"
        );
        let queries = world.versions.diff_queries()?;
        let query = some(queries.first(), "the comparison")?;
        assert_eq!(query.item_id, only.map(item));
    }
    Ok(())
}

#[tokio::test]
async fn a_failed_diff_records_no_receipt() -> TestResult {
    let world = World::new()?;
    let purge_id = purge(&world, 'c')?;
    let refused = err_of(
        world
            .service
            .diff_items(
                &alice(OperationName::DiffItems, None)?,
                DiffRequest {
                    workspace_id: scope()?.workspace_id,
                    from: revision('c')?,
                    to: revision('a')?,
                    item_id: None,
                },
            )
            .await,
    )?;
    assert_invalidated(&refused, purge_id);
    assert_eq!(world.records.receipts()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn log_items_records_a_receipt_with_the_callers_audience_and_no_sources() -> TestResult {
    let world = World::new()?;
    world.versions.put_log(
        &revision('a')?,
        None,
        LogResponse {
            commits: vec![wire_commit('a')?],
            next_cursor: None,
        },
    )?;
    let agent = context(
        OperationName::LogItems,
        "agent",
        AccessRoute::McpDelegation,
        None,
    )?;
    world
        .service
        .log_items(
            &agent,
            LogRequest {
                workspace_id: scope()?.workspace_id,
                item_id: None,
                at: At::Latest,
                page: PageRequest {
                    cursor: None,
                    limit: 20,
                },
            },
        )
        .await?;
    let receipts = world.records.receipts()?;
    let (under, receipt) = some(receipts.first(), "the log's receipt")?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(*under, None);
    assert_eq!(receipt.operation_id, OperationName::LogItems);
    assert_eq!(receipt.audience, ReceiptAudience::AgentContext);
    assert!(receipt.sources.is_empty());
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
async fn commit_items_conflict_of_a_moved_and_rewritten_item_is_not_a_deletion() -> TestResult {
    let world = World::new()?;
    let base = revision('a')?;
    let head = revision('c')?;
    note_at(&world, NOTE, "notes/plan.md", &['a'])?;
    note_at(&world, NOTE, "archive/plan.md", &['c'])?;
    world.versions.set_head(head.clone())?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &base, "# Mine\n")?)?;
    // A move with a heavy rewrite, which rename detection reports as a removal and an addition:
    // the item still exists at the head.
    let moved = vec![
        change(FileChangeKind::Removed, Some("notes/plan.md"), None),
        change(FileChangeKind::Added, None, Some("archive/plan.md")),
    ];
    world
        .versions
        .put_diff(&base, &head, Some(item(NOTE)), moved.clone())?;
    world.versions.queue_revision(revision('d')?)?;
    let refused = err_of(
        world
            .service
            .commit_items(
                &alice(OperationName::CommitItems, Some(mutation(1)))?,
                snapshot(&[NOTE])?,
            )
            .await,
    )?;
    let Some(ErrorDetail::DraftConflict { items }) = refused.detail.as_deref() else {
        return Err(format!("not a draft conflict: {refused:?}").into());
    };
    let only = some(items.first(), "the moved item")?;
    assert_eq!(items.len(), 1);
    assert!(!only.deleted, "a moved item is not deleted: {only:?}");
    assert_eq!(only.changes, moved);
    assert_eq!(world.versions.commits()?.len(), 0);
    Ok(())
}
#[tokio::test]
async fn commit_items_conflict_of_a_replaced_item_is_a_deletion_whatever_sits_at_its_path()
-> TestResult {
    let world = World::new()?;
    let base = revision('a')?;
    let head = revision('c')?;
    note_at(&world, NOTE, "notes/plan.md", &['a'])?;
    // A different item was created at the drafted item's path.
    note_at(&world, OTHER, "notes/plan.md", &['c'])?;
    world.versions.set_head(head.clone())?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &base, "# Mine\n")?)?;
    let shown = vec![change(
        FileChangeKind::Modified,
        Some("notes/plan.md"),
        Some("notes/plan.md"),
    )];
    world
        .versions
        .put_diff(&base, &head, Some(item(NOTE)), shown.clone())?;
    let refused = err_of(
        world
            .service
            .commit_items(
                &alice(OperationName::CommitItems, Some(mutation(1)))?,
                snapshot(&[NOTE])?,
            )
            .await,
    )?;
    let Some(ErrorDetail::DraftConflict { items }) = refused.detail.as_deref() else {
        return Err(format!("not a draft conflict: {refused:?}").into());
    };
    let only = some(items.first(), "the replaced item")?;
    assert_eq!(items.len(), 1);
    assert!(only.deleted, "the item no longer exists: {only:?}");
    assert_eq!(only.changes, shown);
    assert_eq!(world.versions.commits()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn commit_items_conflict_of_a_moved_and_edited_item_is_not_a_deletion() -> TestResult {
    let world = World::new()?;
    let base = revision('a')?;
    let head = revision('c')?;
    note_at(&world, NOTE, "notes/plan.md", &['a'])?;
    note_at(&world, NOTE, "archive/plan.md", &['c'])?;
    world.versions.set_head(head.clone())?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &base, "# Mine\n")?)?;
    world.versions.put_diff(
        &base,
        &head,
        Some(item(NOTE)),
        vec![change(
            FileChangeKind::Moved,
            Some("notes/plan.md"),
            Some("archive/plan.md"),
        )],
    )?;
    let refused = err_of(
        world
            .service
            .commit_items(
                &alice(OperationName::CommitItems, Some(mutation(1)))?,
                snapshot(&[NOTE])?,
            )
            .await,
    )?;
    let Some(ErrorDetail::DraftConflict { items }) = refused.detail.as_deref() else {
        return Err(format!("not a draft conflict: {refused:?}").into());
    };
    let only = some(items.first(), "the moved item")?;
    assert!(!only.deleted, "the item still exists: {only:?}");
    Ok(())
}

#[tokio::test]
async fn commit_items_fails_on_a_store_fault_reading_the_item_at_the_head() -> TestResult {
    let world = World::new()?;
    let base = revision('a')?;
    let head = revision('c')?;
    note_at(&world, NOTE, "notes/plan.md", &['a', 'c'])?;
    world.versions.set_head(head.clone())?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &base, "# Mine\n")?)?;
    world.versions.put_diff(
        &base,
        &head,
        Some(item(NOTE)),
        vec![change(
            FileChangeKind::Modified,
            Some("notes/plan.md"),
            Some("notes/plan.md"),
        )],
    )?;
    world.versions.queue_revision(revision('d')?)?;
    // The read that decides `deleted` fails with something other than `NotFound`.
    world.versions.fail_next_show(ErrorCode::Unavailable)?;
    let failed = err_of(
        world
            .service
            .commit_items(
                &alice(OperationName::CommitItems, Some(mutation(1)))?,
                snapshot(&[NOTE])?,
            )
            .await,
    )?;
    assert_eq!(failed.code, ErrorCode::Unavailable, "{failed:?}");
    assert!(
        failed.detail.is_none(),
        "the fault is not folded into a conflict: {failed:?}"
    );
    assert_eq!(world.versions.commits()?.len(), 0);
    assert_eq!(world.drafts.held()?.len(), 1);
    Ok(())
}

/// NOTE at `a` and at each of `heads`, Alice's draft based on `a`, `d` queued, and an unrelated
/// commit landing before each of the next commits, one per revision of `heads`.
fn landing_mid_snapshot(heads: &[char]) -> Built<World> {
    let world = World::new()?;
    let mut fills = vec!['a'];
    fills.extend_from_slice(heads);
    note_at(&world, NOTE, "notes/plan.md", &fills)?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &revision('a')?, "# Mine\n")?)?;
    world.versions.queue_revision(revision('d')?)?;
    world.versions.land_before_next_commits(
        heads
            .iter()
            .map(|fill| revision(*fill))
            .collect::<Built<Vec<_>>>()?,
    )?;
    Ok(world)
}

#[tokio::test]
async fn commit_items_commits_on_the_new_head_when_an_unrelated_commit_lands_mid_snapshot()
-> TestResult {
    let world = landing_mid_snapshot(&['c'])?;
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
    assert_eq!(commits.len(), 1);
    assert_eq!(commit.expected_head, revision('c')?);
    // The item-level rule was evaluated again against the new head.
    let queries = world.versions.diff_queries()?;
    let query = some(queries.first(), "the item's comparison with the new head")?;
    assert_eq!(
        (&query.from, &query.to, query.item_id),
        (&revision('a')?, &revision('c')?, Some(item(NOTE)))
    );
    assert_eq!(world.drafts.held()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn commit_items_is_blocked_when_the_commit_landing_mid_snapshot_changed_its_item()
-> TestResult {
    let world = landing_mid_snapshot(&['c'])?;
    let edited = vec![change(
        FileChangeKind::Modified,
        Some("notes/plan.md"),
        Some("notes/plan.md"),
    )];
    world
        .versions
        .put_diff(&revision('a')?, &revision('c')?, Some(item(NOTE)), edited)?;
    let refused = err_of(
        world
            .service
            .commit_items(
                &alice(OperationName::CommitItems, Some(mutation(1)))?,
                snapshot(&[NOTE])?,
            )
            .await,
    )?;
    let Some(ErrorDetail::DraftConflict { items }) = refused.detail.as_deref() else {
        return Err(format!("not a draft conflict: {refused:?}").into());
    };
    let only = some(items.first(), "the changed item")?;
    assert_eq!(only.current_revision, revision('c')?);
    assert_eq!(world.versions.commits()?.len(), 0);
    assert_eq!(world.drafts.held()?.len(), 1);
    Ok(())
}

/// The reviewer's probe, kept: two unrelated commits land, and the Snapshot commits on the third
/// head it reads, so the bound is at least 3 heads.
#[tokio::test]
async fn rr_snapshot_commits_on_the_third_head() -> TestResult {
    let world = landing_mid_snapshot(&['c', 'e'])?;
    let result = world
        .service
        .commit_items(
            &alice(OperationName::CommitItems, Some(mutation(1)))?,
            snapshot(&[NOTE])?,
        )
        .await?;
    assert_eq!(result.revision, revision('d')?);
    let commits = world.versions.commits()?;
    assert_eq!(commits.len(), 1);
    assert_eq!(
        some(commits.first(), "commit")?.expected_head,
        revision('e')?
    );
    Ok(())
}
#[tokio::test]
async fn a_conflict_after_the_snapshot_committed_never_leads_to_a_second_commit() -> TestResult {
    let world = World::new()?;
    note_at(&world, NOTE, "notes/plan.md", &['a'])?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &revision('a')?, "# Mine\n")?)?;
    world.versions.queue_revision(revision('b')?)?;
    world.versions.queue_revision(revision('d')?)?;
    // The store commits `b`; indexing it then answers `Conflict`.
    world.search.conflict_next();
    let mut caller = alice(OperationName::CommitItems, Some(mutation(1)))?;
    let refused = err_of(
        world
            .service
            .commit_items(&caller, snapshot(&[NOTE])?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(
        refused.detail.is_none(),
        "not a draft conflict: {refused:?}"
    );
    let commits = world.versions.commits()?;
    assert_eq!(commits.len(), 1, "exactly one commit: {commits:?}");
    // A resumed attempt completes the commit it already wrote.
    caller.attempt = Attempt::Resumed;
    let retried = world
        .service
        .commit_items(&caller, snapshot(&[NOTE])?)
        .await?;
    assert_eq!(retried.revision, revision('b')?);
    assert_eq!(world.versions.commits()?.len(), 1);
    assert_eq!(world.search.indexed()?, vec![revision('b')?]);
    Ok(())
}
/// A world where Alice's draft of NOTE, whose body has no heading, commits as `b`, and the
/// staged tree the store checks is that commit's tree.
fn snapshot_with_lint() -> Built<World> {
    let world = World::new()?;
    note_at(&world, NOTE, "notes/plan.md", &['a'])?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &revision('a')?, "No heading.\n")?)?;
    world.versions.queue_revision(revision('b')?)?;
    let committed = note(
        NOTE,
        &revision('b')?,
        "notes/plan.md",
        "No heading.\n",
        json!({ "type": "Note" }),
    )?;
    world
        .versions
        .stage()
        .write("notes/plan.md", &file_text(&committed))?;
    Ok(world)
}

#[tokio::test]
async fn a_resumed_snapshot_answers_the_warnings_its_first_attempt_produced() -> TestResult {
    let mut caller = alice(OperationName::CommitItems, Some(mutation(1)))?;
    let uninterrupted = snapshot_with_lint()?;
    let first = uninterrupted
        .service
        .commit_items(&caller, snapshot(&[NOTE])?)
        .await?;
    assert!(
        first.warnings.iter().any(|warning| {
            warning.code.starts_with("okf_lint")
                && warning.location.as_deref() == Some("notes/plan.md")
        }),
        "{:?}",
        first.warnings
    );
    // The same Snapshot fails after its commit; the resumed attempt finds that commit.
    let world = snapshot_with_lint()?;
    world.search.fail_next();
    err_of(
        world
            .service
            .commit_items(&caller, snapshot(&[NOTE])?)
            .await,
    )?;
    // The tree a check would read now has its heading: a check run again would find no lint,
    // so only the warnings the store kept with the found commit can answer.
    world.versions.stage().write(
        "notes/plan.md",
        "---\ntype: Note\n---\n\n# Plan\n\nA heading now.\n",
    )?;
    caller.attempt = Attempt::Resumed;
    let resumed = world
        .service
        .commit_items(&caller, snapshot(&[NOTE])?)
        .await?;
    assert_eq!(world.versions.commits()?.len(), 1, "no second commit");
    assert_eq!(resumed.revision, revision('b')?);
    // `Warning` has no equality; its wire form is compared.
    assert_eq!(
        serde_json::to_value(&resumed.warnings)?,
        serde_json::to_value(&first.warnings)?
    );
    Ok(())
}

#[tokio::test]
async fn a_resumed_snapshot_the_store_kept_no_warnings_for_still_completes() -> TestResult {
    let world = snapshot_with_lint()?;
    world.search.fail_next();
    let mut caller = alice(OperationName::CommitItems, Some(mutation(1)))?;
    err_of(
        world
            .service
            .commit_items(&caller, snapshot(&[NOTE])?)
            .await,
    )?;
    // The store holds the commit and no warnings for it, as for a commit it wrote before it
    // kept them.
    world.versions.forget_warnings()?;
    caller.attempt = Attempt::Resumed;
    let resumed = world
        .service
        .commit_items(&caller, snapshot(&[NOTE])?)
        .await?;
    assert_eq!(resumed.revision, revision('b')?);
    assert_eq!(world.versions.commits()?.len(), 1, "no second commit");
    // The store's one warning, passed on as it is.
    let [only] = resumed.warnings.as_slice() else {
        return Err(format!("not one warning: {:?}", resumed.warnings).into());
    };
    assert_eq!(only.code, "replay_warnings_unavailable");
    assert_eq!(only.location, None);
    Ok(())
}

#[tokio::test]
async fn commit_items_gives_up_after_three_heads_moved_mid_snapshot() -> TestResult {
    let world = landing_mid_snapshot(&['c', 'e', 'f'])?;
    let refused = err_of(
        world
            .service
            .commit_items(
                &alice(OperationName::CommitItems, Some(mutation(1)))?,
                snapshot(&[NOTE])?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(refused.detail.is_none(), "{refused:?}");
    assert_eq!(world.versions.commits()?.len(), 0);
    assert_eq!(world.drafts.held()?.len(), 1);
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
        content_digest: draft_digest(body, &BTreeMap::new())?,
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

/// Alice's draft of `id` saved again with the committed body and type, and only an alias added,
/// digested as `save_draft` digests it.
fn aliased(id: u128, base: char) -> Built<DraftWrite> {
    let body = "# Mine\n";
    let properties = BTreeMap::from([
        ("type".to_owned(), json!("Note")),
        ("aliases".to_owned(), json!(["notes/old-plan.md"])),
    ]);
    Ok(DraftWrite {
        item_id: item(id),
        editor: ALICE.to_owned(),
        base_revision: revision(base)?,
        body: body.to_owned(),
        content_digest: draft_digest(body, &properties)?,
        properties,
    })
}

fn holds_aliased(world: &World, id: u128) -> Built<bool> {
    Ok(world
        .drafts
        .held()?
        .iter()
        .any(|kept| kept.draft.item_id == item(id) && kept.properties.contains_key("aliases")))
}

/// Review N1: a draft saved again between the Snapshot's read and its removal with only its
/// `aliases` changed is not the committed content, so it is kept.
#[tokio::test]
async fn a_snapshot_keeps_a_draft_saved_again_with_only_its_aliases_changed() -> TestResult {
    let world = resumable()?;
    world.drafts.save_before_next_discard(aliased(NOTE, 'a')?)?;
    let result = world
        .service
        .commit_items(
            &alice(OperationName::CommitItems, Some(mutation(1)))?,
            snapshot(&[NOTE, OTHER])?,
        )
        .await?;
    assert_eq!(result.revision, revision('b')?);
    assert!(holds_aliased(&world, NOTE)?, "{:?}", world.drafts.held()?);
    assert_eq!(
        world.drafts.held()?.len(),
        1,
        "OTHER's committed draft is removed"
    );
    Ok(())
}

/// Review N1: the same after a crash: a resumed Snapshot compares the item its commit holds with
/// the draft by the whole content, so a draft re-saved with only an alias added is kept.
#[tokio::test]
async fn a_resumed_snapshot_keeps_a_draft_saved_again_with_only_its_aliases_changed() -> TestResult
{
    let world = resumable()?;
    let caller = first_attempt_fails_after_its_commit(&world).await?;
    world
        .drafts
        .save(&scope()?, mutation(77), aliased(NOTE, 'b')?)
        .await?;
    let retried = world
        .service
        .commit_items(&caller, snapshot(&[NOTE, OTHER])?)
        .await?;
    assert_eq!(retried.revision, revision('b')?);
    assert_eq!(world.versions.commits()?.len(), 1, "no second commit");
    assert!(holds_aliased(&world, NOTE)?, "{:?}", world.drafts.held()?);
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
async fn a_store_fault_while_removing_a_draft_fails_the_snapshot() -> TestResult {
    // On the found-commit path.
    let world = resumable()?;
    let caller = first_attempt_fails_after_its_commit(&world).await?;
    world.drafts.fail_next_discard();
    let fault = err_of(
        world
            .service
            .commit_items(&caller, snapshot(&[NOTE, OTHER])?)
            .await,
    )?;
    assert_eq!(fault.code, ErrorCode::Internal);
    assert_eq!(
        world.records.receipts()?.len(),
        0,
        "the Snapshot did not complete"
    );
    // A later resume completes it.
    world
        .service
        .commit_items(&caller, snapshot(&[NOTE, OTHER])?)
        .await?;
    assert_eq!(world.drafts.held()?.len(), 0);
    assert_eq!(world.versions.commits()?.len(), 1);
    // On a first attempt.
    let world = resumable()?;
    world.drafts.fail_next_discard();
    let fault = err_of(
        world
            .service
            .commit_items(
                &alice(OperationName::CommitItems, Some(mutation(1)))?,
                snapshot(&[NOTE, OTHER])?,
            )
            .await,
    )?;
    assert_eq!(fault.code, ErrorCode::Internal);
    assert_eq!(world.records.receipts()?.len(), 0);
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
    assert!(
        matches!(
            commit.edits.as_slice(),
            [TreeEdit::RestoreItem { item_id, from }]
                if *item_id == item(NOTE) && *from == revision('a')?
        ),
        "not one item's restore: {:?}",
        commit.edits
    );
    // The item is back at its path at the target.
    let shown = world
        .versions
        .show(&scope()?, &revision('c')?, item(NOTE))
        .await?;
    assert_eq!(shown.summary.path.as_str(), "notes/plan.md");
    let events = world.events.appended()?;
    let (_, _, event) = some(events.first(), "the changed event")?;
    assert_eq!(event.item_id, Some(item(NOTE)));
    let receipts = world.records.receipts()?;
    let (_, receipt) = some(receipts.first(), "the rewind's receipt")?;
    assert_eq!(receipt.id, result.receipt_id);
    Ok(())
}

/// The version fake encodes the port's grouping of consecutive `RestoreItem` edits: two items
/// that swapped paths are rewound together, but not when another edit separates them.
#[tokio::test]
async fn the_version_fake_restores_consecutive_restore_items_as_one_group() -> TestResult {
    let restore = |id: u128| -> Built<TreeEdit> {
        Ok(TreeEdit::RestoreItem {
            item_id: item(id),
            from: revision('a')?,
        })
    };
    let changes = |edits: Vec<TreeEdit>| -> Built<CommitChanges> {
        Ok(CommitChanges {
            mutation_id: mutation(1),
            expected_head: revision('b')?,
            author: Provenance {
                subject: ALICE.to_owned(),
                route: AccessRoute::LocalOwner,
                client_id: None,
            },
            message: "Rewind".to_owned(),
            edits,
        })
    };
    let swapped = || -> Built<World> {
        let world = World::new()?;
        world.versions.set_head(revision('b')?)?;
        note_at(&world, NOTE, "notes/x.md", &['a'])?;
        note_at(&world, OTHER, "notes/y.md", &['a'])?;
        note_at(&world, NOTE, "notes/y.md", &['b'])?;
        note_at(&world, OTHER, "notes/x.md", &['b'])?;
        note_at(&world, THIRD, "notes/third.md", &['b'])?;
        world.versions.queue_revision(revision('c')?)?;
        Ok(world)
    };
    let world = swapped()?;
    world
        .versions
        .commit(
            &scope()?,
            changes(vec![restore(NOTE)?, restore(OTHER)?])?,
            world.service.conformance(),
        )
        .await?;
    let rewound = revision('c')?;
    let note = world.versions.show(&scope()?, &rewound, item(NOTE)).await?;
    let other = world
        .versions
        .show(&scope()?, &rewound, item(OTHER))
        .await?;
    assert_eq!(note.summary.path.as_str(), "notes/x.md");
    assert_eq!(other.summary.path.as_str(), "notes/y.md");
    // Separated by another edit, the first restore collides with the other item's current path.
    let world = swapped()?;
    let separated = err_of(
        world
            .versions
            .commit(
                &scope()?,
                changes(vec![
                    restore(NOTE)?,
                    TreeEdit::DeleteItem {
                        item_id: item(THIRD),
                    },
                    restore(OTHER)?,
                ])?,
                world.service.conformance(),
            )
            .await,
    )?;
    assert_eq!(separated.code, ErrorCode::Conflict);
    // An unknown `since` is the caller's error.
    let unknown = err_of(
        world
            .versions
            .find_commit(&scope()?, mutation(1), Some(&revision('e')?))
            .await,
    )?;
    assert_eq!(unknown.code, ErrorCode::NotFound);
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
async fn restore_items_restores_a_source_card_with_its_companion_files() -> TestResult {
    let world = World::new()?;
    let base = revision('b')?;
    let target = revision('a')?;
    // At the target the card shows the converter's text; a correction was recorded since.
    let card_at_target = source_card(
        CARD,
        &target,
        "sources/report-pdf.md",
        CARD_BODY,
        converted(&digest('b')?),
    )?;
    let mut corrected = converted(&digest('b')?);
    corrected.corrected = true;
    world
        .versions
        .put_document(&target, card_at_target.clone())?;
    world.versions.put_document(
        &base,
        source_card(
            CARD,
            &base,
            "sources/report-pdf.md",
            "# Corrected\n",
            corrected,
        )?,
    )?;
    world.versions.set_head(base.clone())?;
    world.versions.put_diff(
        &base,
        &target,
        Some(item(CARD)),
        vec![change(
            FileChangeKind::Modified,
            Some("sources/report-pdf.md"),
            Some("sources/report-pdf.md"),
        )],
    )?;
    world.versions.queue_revision(revision('c')?)?;
    world
        .service
        .restore_items(
            &alice(OperationName::RestoreItems, Some(mutation(1)))?,
            rewind('b', 'a', Some(CARD))?,
        )
        .await?;
    let commits = world.versions.commits()?;
    let commit = some(commits.first(), "the rewind")?;
    assert!(
        matches!(
            commit.edits.as_slice(),
            [TreeEdit::RestoreItem { item_id, from }]
                if *item_id == item(CARD) && *from == target
        ),
        "not one item's restore with its companions: {:?}",
        commit.edits
    );
    // The card comes back with the appearance it had at the target: no correction.
    let shown = world
        .versions
        .show(&scope()?, &revision('c')?, item(CARD))
        .await?;
    assert_eq!(shown.body, CARD_BODY);
    assert_eq!(
        serde_json::to_value(&shown.source)?,
        serde_json::to_value(&card_at_target.source)?
    );
    Ok(())
}

#[tokio::test]
async fn restore_items_refuses_to_restore_nothing_and_never_brings_back_or_overwrites_another_item()
-> TestResult {
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
    // The note moved away from `notes/new.md` to `notes/plan.md`, and another item held
    // `notes/plan.md` at the target: the note is restored by identity, so that item stays gone.
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
    world
        .service
        .restore_items(&caller()?, rewind('b', 'a', Some(NOTE))?)
        .await?;
    let restored = revision('c')?;
    let note = world
        .versions
        .show(&scope()?, &restored, item(NOTE))
        .await?;
    assert_eq!(note.summary.path.as_str(), "notes/new.md");
    let other = err_of(world.versions.show(&scope()?, &restored, item(OTHER)).await)?;
    assert_eq!(
        other.code,
        ErrorCode::NotFound,
        "the other item is not brought back"
    );
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

/// A world whose head `b` no longer holds NOTE: `a` holds it at `first`, `d` at `last`, and
/// `e`, whose parent is `d`, removed it. A Rewind of it commits `c`.
fn deleted_since(first: &str, last: &str) -> Built<World> {
    let world = World::new()?;
    note_at(&world, NOTE, first, &['a'])?;
    note_at(&world, NOTE, last, &['d'])?;
    world.versions.set_head(revision('b')?)?;
    world.versions.put_diff(
        &revision('b')?,
        &revision('a')?,
        Some(item(NOTE)),
        vec![change(FileChangeKind::Added, None, Some(first))],
    )?;
    // The item's history from the head, newest first: the commit that removed it.
    world.versions.put_log(
        &revision('b')?,
        Some(item(NOTE)),
        LogResponse {
            commits: vec![Commit {
                parents: vec![revision('d')?],
                ..wire_commit('e')?
            }],
            next_cursor: Some("1".to_owned()),
        },
    )?;
    world.versions.queue_revision(revision('c')?)?;
    Ok(world)
}

/// The edits of the one commit a Rewind of NOTE from `b` to `a` wrote in `world`.
async fn rewound(world: &World) -> Built<Vec<TreeEdit>> {
    world
        .service
        .restore_items(
            &alice(OperationName::RestoreItems, Some(mutation(1)))?,
            rewind('b', 'a', Some(NOTE))?,
        )
        .await?;
    let commits = world.versions.commits()?;
    assert_eq!(commits.len(), 1);
    Ok(some(commits.into_iter().next(), "the rewind")?.edits)
}

/// Whether `edits` are the one `RestoreItem` of NOTE from `a`.
fn one_restore(edits: &[TreeEdit]) -> Built<bool> {
    let target = revision('a')?;
    Ok(matches!(
        edits,
        [TreeEdit::RestoreItem { item_id, from }] if *item_id == item(NOTE) && *from == target
    ))
}

#[tokio::test]
async fn restoring_a_deleted_item_at_an_older_path_moves_it_there_from_its_last_path() -> TestResult
{
    // NOTE was at `notes/old.md` at the target, moved to `notes/new.md`, then deleted: the
    // links that pointed at it still name `notes/new.md`.
    let world = deleted_since("notes/old.md", "notes/new.md")?;
    let edits = rewound(&world).await?;
    let (last, target) = (revision('d')?, revision('a')?);
    // Back where it last was, moved to its path at the target (which rewrites the links that
    // point at it, as a move does), then restored from the target.
    assert!(
        matches!(
            edits.as_slice(),
            [
                TreeEdit::RestoreItem { item_id: put_back, from: held },
                TreeEdit::MoveItem { item_id: moved, destination },
                TreeEdit::RestoreItem { item_id: restored, from },
            ] if [put_back, moved, restored].iter().all(|id| **id == item(NOTE))
                && *held == last
                && destination.as_str() == "notes/old.md"
                && *from == target
        ),
        "{edits:?}"
    );
    let shown = world
        .versions
        .show(&scope()?, &revision('c')?, item(NOTE))
        .await?;
    assert_eq!(shown.summary.path.as_str(), "notes/old.md");
    // The last holder was asked of the item's own history from the base: its newest commit.
    let queries = world.versions.log_queries()?;
    let [query] = queries.as_slice() else {
        return Err(format!("not one history query: {queries:?}").into());
    };
    assert_eq!(query.tip, revision('b')?);
    assert_eq!(query.item_id, Some(item(NOTE)));
    assert_eq!((query.page.cursor.as_deref(), query.page.limit), (None, 1));
    Ok(())
}

#[tokio::test]
async fn a_rewind_of_a_deleted_item_reports_the_referrers_its_move_rewrote() -> TestResult {
    // NOTE was at `notes/old.md` at the target, moved to `notes/new.md`, then deleted. The
    // Rewind puts it back at `notes/new.md` and moves it to `notes/old.md` (R-I14); that
    // `MoveItem` rewrites the link in `notes/referrer.md`, which only the store can name.
    let world = deleted_since("notes/old.md", "notes/new.md")?;
    world
        .versions
        .put_referrer(item(NOTE), path("notes/referrer.md")?)?;
    let stage = world.versions.stage();
    for at in ["notes/referrer.md", "notes/bystander.md"] {
        stage.write(
            at,
            "---
type: Note
---

No heading.
",
        )?;
    }
    let result = world
        .service
        .restore_items(
            &alice(OperationName::RestoreItems, Some(mutation(1)))?,
            rewind('b', 'a', Some(NOTE))?,
        )
        .await?;
    let located: BTreeSet<_> = result
        .warnings
        .iter()
        .filter(|warning| warning.code.starts_with("okf_lint"))
        .filter_map(|warning| warning.location.as_deref())
        .collect();
    assert!(
        located.contains("notes/referrer.md"),
        "{:?}",
        result.warnings
    );
    assert!(
        !located.contains("notes/bystander.md"),
        "{:?}",
        result.warnings
    );
    Ok(())
}

#[tokio::test]
async fn a_rewind_moves_no_links_when_they_have_nothing_to_follow() -> TestResult {
    // The base still holds the item: the store moves it back itself, and no history is read.
    let world = World::new()?;
    note_at(&world, NOTE, "notes/old.md", &['a'])?;
    note_at(&world, NOTE, "notes/new.md", &['b'])?;
    world.versions.set_head(revision('b')?)?;
    world.versions.put_diff(
        &revision('b')?,
        &revision('a')?,
        Some(item(NOTE)),
        vec![change(
            FileChangeKind::Moved,
            Some("notes/new.md"),
            Some("notes/old.md"),
        )],
    )?;
    world.versions.queue_revision(revision('c')?)?;
    assert!(one_restore(&rewound(&world).await?)?, "still held");
    assert_eq!(world.versions.log_queries()?.len(), 0);

    // Deleted where the target holds it: its links already name the restored path.
    let world = deleted_since("notes/old.md", "notes/old.md")?;
    assert!(one_restore(&rewound(&world).await?)?, "the same path");

    // Deleted, and since then its last path was taken: by another item, by one whose path
    // differs only in letter case, or by a folder. The links that name it are that one's now.
    for holder in ["notes/new.md", "notes/New.md", "notes/new.md/inner.md"] {
        let world = deleted_since("notes/old.md", "notes/new.md")?;
        note_at(&world, OTHER, holder, &['b'])?;
        assert!(one_restore(&rewound(&world).await?)?, "{holder}");
    }

    // The history of the base never held the item, or its newest commit has no parent.
    for commits in [Vec::new(), vec![wire_commit('e')?]] {
        let world = deleted_since("notes/old.md", "notes/new.md")?;
        world.versions.put_log(
            &revision('f')?,
            Some(item(NOTE)),
            LogResponse {
                commits,
                next_cursor: None,
            },
        )?;
        world.versions.set_head(revision('f')?)?;
        world.versions.put_diff(
            &revision('f')?,
            &revision('a')?,
            Some(item(NOTE)),
            vec![change(FileChangeKind::Added, None, Some("notes/old.md"))],
        )?;
        world
            .service
            .restore_items(
                &alice(OperationName::RestoreItems, Some(mutation(1)))?,
                rewind('f', 'a', Some(NOTE))?,
            )
            .await?;
        let commits = world.versions.commits()?;
        let commit = some(commits.first(), "the rewind")?;
        assert!(one_restore(&commit.edits)?, "{:?}", commit.edits);
    }
    Ok(())
}

#[tokio::test]
async fn a_rewind_between_two_spellings_of_one_path_is_one_restore() -> TestResult {
    // `report.md` became `Report.md` (by way of another name, since the store refuses a move
    // that changes only letter case) and was deleted. To the store the two are one path: it
    // would refuse the move back, so the Rewind is the one restore, and it lands.
    let world = deleted_since("notes/report.md", "notes/Report.md")?;
    assert!(one_restore(&rewound(&world).await?)?);
    let shown = world
        .versions
        .show(&scope()?, &revision('c')?, item(NOTE))
        .await?;
    assert_eq!(shown.summary.path.as_str(), "notes/report.md");
    Ok(())
}

#[tokio::test]
async fn a_rewind_onto_a_path_another_item_holds_conflicts_whichever_edits_it_sends() -> TestResult
{
    // Another item holds the path at the target. Deleted since: the move of the three edits is
    // refused. Still held at the base: the one restore is. Nothing is written either way.
    let deleted = deleted_since("notes/old.md", "notes/new.md")?;
    let moved = World::new()?;
    note_at(&moved, NOTE, "notes/old.md", &['a'])?;
    note_at(&moved, NOTE, "notes/new.md", &['b'])?;
    moved.versions.set_head(revision('b')?)?;
    moved.versions.put_diff(
        &revision('b')?,
        &revision('a')?,
        Some(item(NOTE)),
        vec![change(
            FileChangeKind::Moved,
            Some("notes/new.md"),
            Some("notes/old.md"),
        )],
    )?;
    moved.versions.queue_revision(revision('c')?)?;
    for world in [deleted, moved] {
        note_at(&world, OTHER, "notes/old.md", &['b'])?;
        let refused = err_of(
            world
                .service
                .restore_items(
                    &alice(OperationName::RestoreItems, Some(mutation(1)))?,
                    rewind('b', 'a', Some(NOTE))?,
                )
                .await,
        )?;
        assert_eq!(refused.code, ErrorCode::Conflict, "{}", refused.message);
        assert_eq!(world.versions.commits()?.len(), 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_taker_of_the_last_path_on_a_later_page_of_its_folder_is_seen() -> TestResult {
    // The folder of the last path lists one entry a page; the item that took the path since is
    // on the second.
    let world = deleted_since("notes/old.md", "notes/new.md")?;
    note_at(&world, THIRD, "notes/first.md", &['b'])?;
    note_at(&world, OTHER, "notes/new.md", &['b'])?;
    world.versions.page_listings(1)?;
    assert!(one_restore(&rewound(&world).await?)?);
    let asked: Vec<_> = world
        .versions
        .list_queries()?
        .into_iter()
        .map(|(at, folder, page)| (at, folder, page.cursor))
        .collect();
    let (base, folder) = (revision('b')?, Some(path("notes")?));
    assert_eq!(
        asked,
        vec![
            (base.clone(), folder.clone(), None),
            (base, folder, Some("1".to_owned()))
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_store_fault_in_any_read_of_the_last_holder_lookup_fails_the_rewind() -> TestResult {
    // The reads after the first: the item at the target, its history, the item at its last
    // holder, the folder of its last path. A fault in any of them is the Rewind's error; it is
    // never read as "nothing to relink", which would restore the item and leave its links.
    type Inject = fn(&World, ErrorCode) -> Built<()>;
    let reads: [(&str, Inject); 4] = [
        ("show at the target", |world, code| {
            world.versions.fail_show_number(2, code)
        }),
        ("log", |world, code| world.versions.fail_next_log(code)),
        ("show at the last holder", |world, code| {
            world.versions.fail_show_number(3, code)
        }),
        ("list", |world, code| world.versions.fail_next_list(code)),
    ];
    for (read, inject) in reads {
        // `NotFound` from `log` or `list` is an unknown revision, not an absent item.
        let codes = if read.starts_with("show") {
            vec![ErrorCode::Unavailable]
        } else {
            vec![ErrorCode::Unavailable, ErrorCode::NotFound]
        };
        for code in codes {
            let world = deleted_since("notes/old.md", "notes/new.md")?;
            inject(&world, code.clone())?;
            let failed = err_of(
                world
                    .service
                    .restore_items(
                        &alice(OperationName::RestoreItems, Some(mutation(1)))?,
                        rewind('b', 'a', Some(NOTE))?,
                    )
                    .await,
            )?;
            assert_eq!(failed.code, code, "{read}");
            assert_eq!(failed.message, "the store failed", "{read}");
            assert_eq!(world.versions.commits()?.len(), 0, "{read}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_rewind_of_an_item_neither_revision_holds_is_left_to_the_store() -> TestResult {
    // The target does not hold the item either: that `NotFound` is tolerated, the one restore
    // is sent, and the refusal is the store's own.
    let world = World::new()?;
    world.versions.set_head(revision('b')?)?;
    world.versions.put_diff(
        &revision('b')?,
        &revision('a')?,
        Some(item(NOTE)),
        vec![change(FileChangeKind::Added, None, Some("notes/old.md"))],
    )?;
    world.versions.queue_revision(revision('c')?)?;
    let refused = err_of(
        world
            .service
            .restore_items(
                &alice(OperationName::RestoreItems, Some(mutation(1)))?,
                rewind('b', 'a', Some(NOTE))?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(refused.message, "no such item at either revision");
    assert_eq!(world.versions.log_queries()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn a_rewind_whose_newest_commits_parent_does_not_hold_the_item_is_one_restore() -> TestResult
{
    // The history names a parent that does not hold the item: that `NotFound` is tolerated.
    let world = deleted_since("notes/old.md", "notes/new.md")?;
    world.versions.set_head(revision('f')?)?;
    world.versions.put_diff(
        &revision('f')?,
        &revision('a')?,
        Some(item(NOTE)),
        vec![change(FileChangeKind::Added, None, Some("notes/old.md"))],
    )?;
    world.versions.put_log(
        &revision('f')?,
        Some(item(NOTE)),
        LogResponse {
            commits: vec![Commit {
                parents: vec![revision('e')?],
                ..wire_commit('f')?
            }],
            next_cursor: None,
        },
    )?;
    world
        .service
        .restore_items(
            &alice(OperationName::RestoreItems, Some(mutation(1)))?,
            rewind('f', 'a', Some(NOTE))?,
        )
        .await?;
    let commits = world.versions.commits()?;
    let commit = some(commits.first(), "the rewind")?;
    assert!(one_restore(&commit.edits)?, "{:?}", commit.edits);
    assert_eq!(
        world.versions.shows(),
        3,
        "the base, the target, the parent"
    );
    Ok(())
}

#[tokio::test]
async fn a_rewind_that_cannot_read_where_the_item_is_writes_nothing() -> TestResult {
    let world = deleted_since("notes/old.md", "notes/new.md")?;
    world.versions.fail_next_show(ErrorCode::Unavailable)?;
    let failed = err_of(
        world
            .service
            .restore_items(
                &alice(OperationName::RestoreItems, Some(mutation(1)))?,
                rewind('b', 'a', Some(NOTE))?,
            )
            .await,
    )?;
    assert_eq!(failed.code, ErrorCode::Unavailable);
    assert_eq!(world.versions.commits()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn restoring_a_moved_item_touches_both_its_paths_and_the_referrers_its_move_back_rewrote()
-> TestResult {
    let world = World::new()?;
    // The note was at `notes/plan.md` at the target and moved to `archive/plan.md` since; the
    // Rewind moves it back, as a `MoveItem` would, so both paths count, as a move's do, and so
    // does the file whose link the store rewrote for that move back (R-I12), which only the
    // store can name. A file nothing was done to stays unreported.
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
    world
        .versions
        .put_referrer(item(NOTE), path("notes/referrer.md")?)?;
    let stage = world.versions.stage();
    std::fs::create_dir_all(stage.0.join("archive"))?;
    for at in [
        "notes/plan.md",
        "archive/plan.md",
        "notes/referrer.md",
        "notes/bystander.md",
    ] {
        stage.write(at, "---\ntype: Note\n---\n\nNo heading.\n")?;
    }
    let result = world
        .service
        .restore_items(
            &alice(OperationName::RestoreItems, Some(mutation(1)))?,
            rewind('b', 'a', Some(NOTE))?,
        )
        .await?;
    let located: BTreeSet<_> = result
        .warnings
        .iter()
        .filter(|warning| warning.code.starts_with("okf_lint"))
        .filter_map(|warning| warning.location.as_deref())
        .collect();
    assert_eq!(
        located,
        BTreeSet::from(["archive/plan.md", "notes/plan.md", "notes/referrer.md"]),
        "{:?}",
        result.warnings
    );
    Ok(())
}

#[tokio::test]
async fn restore_items_reports_lint_only_for_the_files_it_restores() -> TestResult {
    let world = World::new()?;
    world.versions.set_head(revision('b')?)?;
    // The file moved from `notes/bare.md` to `notes/other.md` since the target: the Rewind
    // writes `notes/bare.md` back, and only that path counts as touched.
    world.versions.put_diff(
        &revision('b')?,
        &revision('a')?,
        None,
        vec![change(
            FileChangeKind::Moved,
            Some("notes/other.md"),
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
