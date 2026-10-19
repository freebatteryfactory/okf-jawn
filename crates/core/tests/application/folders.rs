//! Folders as a whole over the fakes: `move_folder` and `delete_folder` are each one commit of
//! one edit (`TreeEdit::MoveFolder`, `TreeEdit::DeleteFolder`) that counts the folder's items
//! at the base, archived ones included; the paths alone are refused before any port; and "a
//! folder changed since `base_revision` conflicts": the edit is committed on the base, and when
//! the head moved, on the head only when the store's comparison shows nothing under the folder
//! (or a move's destination) changed.
//!
//! The fake applies no folder edit and rewrites no link, so what the store does under the
//! folder (every link resolving after, a case-only rename, the refusals that need the tree) is
//! proven against real storage (`crates/storage/tests/folders.rs`). The fake's `diff` answers
//! an unscripted comparison with no change, so every test of the comparison scripts it.

use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::events::{EventKind, ReceiptAudience};
use okf_jawn_contract::history::{FileChange, FileChangeKind};
use okf_jawn_contract::identity::{IdempotencyKey, PurgeId, Revision};
use okf_jawn_contract::item::{DeleteFolderRequest, FolderResult, MoveFolderRequest};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::TreeEdit;
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    ALICE, Built, World, alice, converted, digest, file_text, mutation, note, path, revision,
    scope, source_card,
};

fn key() -> IdempotencyKey {
    IdempotencyKey(Uuid::from_u128(78))
}

/// The base `a` holds `notes/a.md`, the archived `notes/sub/b.md`, and two items that only look
/// as if they were in `notes/`: `notes-old/c.md` and the top-level `top.md`.
fn folder_world() -> Built<World> {
    let world = World::new()?;
    let at = revision('a')?;
    for (id, location, archived) in [
        (21, "notes/a.md", false),
        (22, "notes/sub/b.md", true),
        (23, "notes-old/c.md", false),
        (24, "top.md", false),
    ] {
        let mut document = note(id, &at, location, "# Note\n", json!({ "type": "Note" }))?;
        document.summary.archived = archived;
        world.versions.put_document(&at, document)?;
    }
    Ok(world)
}

fn move_request(folder: &str, destination: &str) -> Built<MoveFolderRequest> {
    Ok(MoveFolderRequest {
        workspace_id: scope()?.workspace_id,
        base_revision: revision('a')?,
        folder: path(folder)?,
        destination: path(destination)?,
        idempotency_key: key(),
    })
}

/// Move a folder as Alice under mutation `number`; a context that cannot be built is answered as
/// an `Internal` error, so the store's and the service's refusals keep their codes.
async fn moved(
    world: &World,
    number: u128,
    request: MoveFolderRequest,
) -> Result<FolderResult, ApiError> {
    let context = alice(OperationName::MoveFolder, Some(mutation(number)))
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
    world.service.move_folder(&context, request).await
}

fn change(kind: FileChangeKind, old: Option<&str>, new: Option<&str>) -> FileChange {
    FileChange {
        old_path: old.map(str::to_owned),
        new_path: new.map(str::to_owned),
        patch: String::new(),
        binary: false,
        kind,
    }
}

/// The world with its head moved from the base `a` to `c`, the comparison of the two scripted
/// as `changes`, and `d` the revision a commit on `c` produces.
fn moved_head(changes: Vec<FileChange>) -> Built<World> {
    let world = folder_world()?;
    world.versions.set_head(revision('c')?)?;
    world
        .versions
        .put_diff(&revision('a')?, &revision('c')?, None, changes)?;
    world.versions.queue_revision(revision('d')?)?;
    Ok(world)
}

/// The refusal a folder changed since the base must be: `Conflict` with `StaleBase` naming
/// the head it was compared with, on `/base_revision`, and nothing committed.
fn assert_stale(world: &World, error: &ApiError, head: &Revision, case: &str) -> TestResult {
    assert_eq!(error.code, ErrorCode::Conflict, "{case}: {error:?}");
    assert_eq!(error.field.as_deref(), Some("/base_revision"), "{case}");
    assert_eq!(
        error.detail.as_deref(),
        Some(&ErrorDetail::StaleBase { head: head.clone() }),
        "{case}"
    );
    assert!(
        world.versions.commits()?.is_empty(),
        "{case}: nothing committed"
    );
    assert!(world.records.receipts()?.is_empty(), "{case}: no receipt");
    Ok(())
}

/// F1, F6: one commit of one edit on the base, which is the head, so nothing is compared; the
/// items under `notes/` are counted, the archived one included, and an item under
/// `notes-old/` or at the top is not; the revision is indexed, announced and receipted.
#[tokio::test]
async fn move_folder_commits_one_edit_on_its_base_and_counts_the_items_archived_ones_too()
-> TestResult {
    let world = folder_world()?;
    world.versions.queue_revision(revision('b')?)?;
    let result = moved(&world, 1, move_request("notes", "archive/notes")?).await?;
    assert_eq!(result.revision, revision('b')?);
    assert_eq!(
        result.items, 2,
        "notes/a.md and the archived notes/sub/b.md"
    );
    let commits = world.versions.commits()?;
    let [commit] = commits.as_slice() else {
        return Err(format!("not one commit: {}", commits.len()).into());
    };
    assert_eq!(commit.expected_head, revision('a')?);
    assert_eq!(commit.message, "Move folder notes to archive/notes");
    assert!(
        matches!(
            commit.edits.as_slice(),
            [TreeEdit::MoveFolder { folder, destination }]
                if folder.as_str() == "notes" && destination.as_str() == "archive/notes"
        ),
        "{:?}",
        commit.edits
    );
    assert!(
        world.versions.diff_queries()?.is_empty(),
        "the head is the base: nothing is compared"
    );
    assert_eq!(world.search.indexed()?, vec![revision('b')?]);
    let events = world.events.appended()?;
    let (_, under, event) = some(events.first(), "the changed event")?;
    assert_eq!(*under, Some(mutation(1)));
    assert_eq!(event.kind, EventKind::Changed);
    assert_eq!(event.revision, Some(revision('b')?));
    assert_eq!(event.item_id, None);
    let receipts = world.records.receipts()?;
    let [(receipt_under, receipt)] = receipts.as_slice() else {
        return Err(format!("not one receipt: {}", receipts.len()).into());
    };
    assert_eq!(*receipt_under, Some(mutation(1)));
    assert_eq!(receipt.id, result.receipt_id);
    assert_eq!(receipt.operation_id, OperationName::MoveFolder);
    assert_eq!(receipt.principal_subject, ALICE);
    assert_eq!(receipt.audience, ReceiptAudience::HumanDisplay);
    assert!(receipt.sources.is_empty());
    Ok(())
}

/// A destination that only begins with the folder's name is not inside it, and a rename that
/// changes only letter case is not the folder itself (F2): both reach the store.
#[tokio::test]
async fn move_folder_passes_a_sibling_name_and_a_case_only_rename_to_the_store() -> TestResult {
    for (destination, items) in [("notes2", 2), ("Notes", 2), ("notes-old/inner", 2)] {
        let world = folder_world()?;
        world.versions.queue_revision(revision('b')?)?;
        let result = moved(&world, 1, move_request("notes", destination)?).await?;
        assert_eq!(result.items, items, "{destination}");
        let commits = world.versions.commits()?;
        assert!(
            matches!(
                commits.first().map(|commit| commit.edits.as_slice()),
                Some([TreeEdit::MoveFolder { destination: sent, .. }]) if sent.as_str() == destination
            ),
            "{destination}"
        );
    }
    Ok(())
}

/// F5, F7: the paths alone are refused before any port is read: a reserved folder (`.okf/` and
/// below, a reserved name in any letter case) on `/folder`; a reserved destination, the folder
/// itself, or a path inside it (in any letter case) on `/destination`.
#[tokio::test]
async fn move_folder_refuses_reserved_paths_the_folder_itself_and_its_inside_before_any_port()
-> TestResult {
    let cases = [
        (".okf", "elsewhere", "/folder"),
        (".OKF/sources", "elsewhere", "/folder"),
        ("notes/INDEX.md", "elsewhere", "/folder"),
        ("notes/log.md", "elsewhere", "/folder"),
        ("notes", ".okf/notes", "/destination"),
        ("notes", "archive/Index.md", "/destination"),
        ("notes", "notes", "/destination"),
        ("notes", "notes/deeper", "/destination"),
        ("notes", "Notes/deeper/still", "/destination"),
    ];
    for (folder, destination, field) in cases {
        let world = folder_world()?;
        world.versions.queue_revision(revision('b')?)?;
        let case = format!("{folder} to {destination}");
        let refused = err_of(moved(&world, 1, move_request(folder, destination)?).await)?;
        assert_eq!(refused.code, ErrorCode::InvalidInput, "{case}: {refused:?}");
        assert_eq!(refused.field.as_deref(), Some(field), "{case}");
        assert_eq!(world.versions.trees(), 0, "{case}: nothing read");
        assert!(world.versions.commits()?.is_empty(), "{case}");
    }
    Ok(())
}

/// F7: the root cannot be named at all: a folder is a `WorkspacePath`, which is never empty.
#[test]
fn move_folder_cannot_name_the_root() -> TestResult {
    for root in ["", ".", "/"] {
        let request = json!({
            "workspace_id": scope()?.workspace_id, "base_revision": revision('a')?,
            "folder": root, "destination": "elsewhere", "idempotency_key": key()
        });
        assert!(
            serde_json::from_value::<MoveFolderRequest>(request).is_err(),
            "{root:?}"
        );
    }
    Ok(())
}

/// A base a purge invalidated is refused before the tree is read at it.
#[tokio::test]
async fn move_folder_refuses_a_purged_base_before_reading_it() -> TestResult {
    let world = folder_world()?;
    world.records.put_mapping(
        revision('a')?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(5)),
            replacement: None,
        },
    )?;
    let refused = err_of(moved(&world, 1, move_request("notes", "archive")?).await)?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(world.versions.trees(), 0);
    assert!(world.versions.commits()?.is_empty());
    Ok(())
}

/// An item whose path under the destination would pass the workspace path limit is refused on
/// `/destination` before anything is committed.
#[tokio::test]
async fn move_folder_refuses_a_destination_that_makes_an_items_path_too_long() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    let deep = format!("n/{}f.md", format!("{}/", "x".repeat(200)).repeat(20));
    assert!(deep.len() < 4096, "the item's own path is a workspace path");
    world.versions.put_document(
        &at,
        note(21, &at, &deep, "# Deep\n", json!({ "type": "Note" }))?,
    )?;
    world.versions.queue_revision(revision('b')?)?;
    let refused = err_of(moved(&world, 1, move_request("n", &"d".repeat(100))?).await)?;
    assert_eq!(refused.code, ErrorCode::InvalidInput, "{refused:?}");
    assert_eq!(refused.field.as_deref(), Some("/destination"));
    assert!(world.versions.commits()?.is_empty());
    Ok(())
}

/// The check reports the lint of the moved items at their new paths, and nothing about a file
/// the move did not touch.
#[tokio::test]
async fn move_folder_reports_the_lint_of_its_items_at_their_new_paths() -> TestResult {
    let world = folder_world()?;
    world.versions.queue_revision(revision('b')?)?;
    let stage = world.versions.stage();
    let unheaded = note(
        21,
        &revision('b')?,
        "archive/notes/a.md",
        "No heading.\n",
        json!({ "type": "Note" }),
    )?;
    for at in ["archive/notes", "elsewhere"] {
        std::fs::create_dir_all(stage.0.join(at))?;
    }
    stage.write("archive/notes/a.md", &file_text(&unheaded))?;
    stage.write("elsewhere/x.md", &file_text(&unheaded))?;
    let result = moved(&world, 1, move_request("notes", "archive/notes")?).await?;
    let located: Vec<&str> = result
        .warnings
        .iter()
        .filter(|warning| warning.code.starts_with("okf_lint"))
        .filter_map(|warning| warning.location.as_deref())
        .collect();
    assert!(
        located.contains(&"archive/notes/a.md"),
        "{:?}",
        result.warnings
    );
    assert!(
        !located.contains(&"elsewhere/x.md"),
        "{:?}",
        result.warnings
    );
    Ok(())
}

/// F4: the head moved, and nothing under the folder or the destination changed (a change
/// beside them, under names that only begin like theirs, the root's log): the same edit is
/// committed on the head.
#[tokio::test]
async fn move_folder_commits_on_the_head_when_nothing_under_it_changed() -> TestResult {
    let world = moved_head(vec![
        change(
            FileChangeKind::Modified,
            Some("other/x.md"),
            Some("other/x.md"),
        ),
        change(FileChangeKind::Added, None, Some("notes-old/new.md")),
        change(FileChangeKind::Added, None, Some("archive2/new.md")),
        change(
            FileChangeKind::Moved,
            Some("notesx.md"),
            Some("archived.md"),
        ),
        change(FileChangeKind::Modified, Some("log.md"), Some("log.md")),
    ])?;
    let result = moved(&world, 1, move_request("notes", "archive")?).await?;
    assert_eq!(result.revision, revision('d')?);
    assert_eq!(result.items, 2);
    let commits = world.versions.commits()?;
    let [commit] = commits.as_slice() else {
        return Err(format!("not one commit: {}", commits.len()).into());
    };
    assert_eq!(
        commit.expected_head,
        revision('c')?,
        "committed on the head"
    );
    let queries = world.versions.diff_queries()?;
    let [query] = queries.as_slice() else {
        return Err(format!("not one comparison: {}", queries.len()).into());
    };
    assert_eq!(
        (&query.from, &query.to, query.item_id),
        (&revision('a')?, &revision('c')?, None)
    );
    Ok(())
}

/// F4: a change at or under the folder or the destination since the base, at either end of
/// the change and in any letter case, conflicts on `/base_revision`, naming the head.
#[tokio::test]
async fn move_folder_conflicts_when_the_folder_or_the_destination_changed() -> TestResult {
    let cases = [
        (
            "an item added",
            change(FileChangeKind::Added, None, Some("notes/new.md")),
        ),
        (
            "an item removed",
            change(FileChangeKind::Removed, Some("notes/a.md"), None),
        ),
        (
            "an item edited",
            change(
                FileChangeKind::Modified,
                Some("notes/sub/b.md"),
                Some("notes/sub/b.md"),
            ),
        ),
        (
            "an item moved out",
            change(
                FileChangeKind::Moved,
                Some("notes/a.md"),
                Some("elsewhere/a.md"),
            ),
        ),
        (
            "an item moved in",
            change(
                FileChangeKind::Moved,
                Some("elsewhere/x.md"),
                Some("notes/x.md"),
            ),
        ),
        (
            "its index rewritten",
            change(
                FileChangeKind::Modified,
                Some("notes/index.md"),
                Some("notes/index.md"),
            ),
        ),
        (
            "another spelling",
            change(FileChangeKind::Added, None, Some("NOTES/new.md")),
        ),
        (
            "the destination filled",
            change(FileChangeKind::Added, None, Some("archive/new.md")),
        ),
        (
            "a file at the destination",
            change(FileChangeKind::Added, None, Some("archive")),
        ),
        (
            "a file at the folder",
            change(FileChangeKind::Added, None, Some("notes")),
        ),
    ];
    for (case, changed) in cases {
        let world = moved_head(vec![
            change(
                FileChangeKind::Modified,
                Some("other/x.md"),
                Some("other/x.md"),
            ),
            changed,
        ])?;
        let refused = err_of(moved(&world, 1, move_request("notes", "archive")?).await)?;
        assert_stale(&world, &refused, &revision('c')?, case)?;
    }
    Ok(())
}

/// A comparison the store cannot make because it no longer holds the base cannot show that
/// nothing changed, so it conflicts; any other failure of the comparison is the store's and
/// is answered as it is.
#[tokio::test]
async fn move_folder_conflicts_without_a_comparison_and_passes_a_store_fault_on() -> TestResult {
    let world = moved_head(Vec::new())?;
    world.versions.fail_next_diff(ErrorCode::NotFound)?;
    let refused = err_of(moved(&world, 1, move_request("notes", "archive")?).await)?;
    assert_stale(&world, &refused, &revision('c')?, "no comparison")?;
    let world = moved_head(Vec::new())?;
    world.versions.fail_next_diff(ErrorCode::Unavailable)?;
    let failed = err_of(moved(&world, 1, move_request("notes", "archive")?).await)?;
    assert_eq!(failed.code, ErrorCode::Unavailable);
    assert!(world.versions.commits()?.is_empty());
    Ok(())
}

/// The comparison and the commit on the head are not atomic: a write that lands between them
/// moves the head again, and the store's own head check refuses the commit, which is answered
/// on `/base_revision` naming the newer head.
#[tokio::test]
async fn move_folder_conflicts_when_the_head_moves_again_after_the_comparison() -> TestResult {
    let world = folder_world()?;
    world
        .versions
        .put_diff(&revision('a')?, &revision('c')?, None, Vec::new())?;
    world
        .versions
        .land_before_next_commits(vec![revision('c')?, revision('e')?])?;
    world.versions.queue_revision(revision('d')?)?;
    let refused = err_of(moved(&world, 1, move_request("notes", "archive")?).await)?;
    assert_stale(&world, &refused, &revision('e')?, "a second write")?;
    assert_eq!(world.versions.diff_queries()?.len(), 1);
    Ok(())
}

/// A retry after a failure past the commit repeats the first attempt: committed on the base,
/// or on the head it compared, the commit lies after the base, so the store replays it; the
/// retry compares nothing again and answers the same revision, receipt and count.
#[tokio::test]
async fn move_folder_retried_after_its_commit_answers_the_first_attempt() -> TestResult {
    for head_moved in [false, true] {
        let world = if head_moved {
            moved_head(Vec::new())?
        } else {
            let world = folder_world()?;
            world.versions.queue_revision(revision('d')?)?;
            world
        };
        world.search.fail_next();
        let request = || move_request("notes", "archive");
        let failed = err_of(moved(&world, 1, request()?).await)?;
        assert_eq!(failed.code, ErrorCode::Internal, "{head_moved}");
        let compared = world.versions.diff_queries()?.len();
        let retried = moved(&world, 1, request()?).await?;
        assert_eq!(retried.revision, revision('d')?, "{head_moved}");
        assert_eq!(retried.items, 2, "{head_moved}");
        assert_eq!(
            world.versions.commits()?.len(),
            1,
            "{head_moved}: no second commit"
        );
        assert_eq!(
            world.versions.diff_queries()?.len(),
            compared,
            "{head_moved}: no second comparison"
        );
        let again = moved(&world, 1, request()?).await?;
        assert_eq!(again.receipt_id, retried.receipt_id, "{head_moved}");
        assert_eq!(
            world.records.receipts()?.len(),
            1,
            "{head_moved}: one receipt"
        );
    }
    Ok(())
}
fn delete_request(folder: &str) -> Built<DeleteFolderRequest> {
    Ok(DeleteFolderRequest {
        workspace_id: scope()?.workspace_id,
        base_revision: revision('a')?,
        folder: path(folder)?,
        idempotency_key: key(),
    })
}

/// Delete a folder as Alice under mutation `number`, as `moved` moves one.
async fn deleted(
    world: &World,
    number: u128,
    request: DeleteFolderRequest,
) -> Result<FolderResult, ApiError> {
    let context = alice(OperationName::DeleteFolder, Some(mutation(number)))
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
    world.service.delete_folder(&context, request).await
}

/// F3, F6: one commit of one edit on the base; the items under `notes/` are counted, the
/// archived one included; nothing is compared while the head is the base; a receipt is kept.
#[tokio::test]
async fn delete_folder_commits_one_edit_on_its_base_and_counts_the_items_archived_ones_too()
-> TestResult {
    let world = folder_world()?;
    world.versions.queue_revision(revision('b')?)?;
    let result = deleted(&world, 1, delete_request("notes")?).await?;
    assert_eq!(result.revision, revision('b')?);
    assert_eq!(
        result.items, 2,
        "notes/a.md and the archived notes/sub/b.md"
    );
    let commits = world.versions.commits()?;
    let [commit] = commits.as_slice() else {
        return Err(format!("not one commit: {}", commits.len()).into());
    };
    assert_eq!(commit.expected_head, revision('a')?);
    assert_eq!(commit.message, "Delete folder notes");
    assert!(
        matches!(
            commit.edits.as_slice(),
            [TreeEdit::DeleteFolder { folder }] if folder.as_str() == "notes"
        ),
        "{:?}",
        commit.edits
    );
    assert!(world.versions.diff_queries()?.is_empty());
    assert_eq!(world.search.indexed()?, vec![revision('b')?]);
    let receipts = world.records.receipts()?;
    let [(under, receipt)] = receipts.as_slice() else {
        return Err(format!("not one receipt: {}", receipts.len()).into());
    };
    assert_eq!(*under, Some(mutation(1)));
    assert_eq!(receipt.id, result.receipt_id);
    assert_eq!(receipt.operation_id, OperationName::DeleteFolder);
    Ok(())
}

/// F7: a reserved folder (`.okf/` and below, a reserved name in any letter case) is refused on
/// `/folder` before any port is read.
#[tokio::test]
async fn delete_folder_refuses_a_reserved_folder_before_any_port() -> TestResult {
    for folder in [".okf", ".Okf/corrections", "notes/Log.md", "INDEX.MD"] {
        let world = folder_world()?;
        let refused = err_of(deleted(&world, 1, delete_request(folder)?).await)?;
        assert_eq!(
            refused.code,
            ErrorCode::InvalidInput,
            "{folder}: {refused:?}"
        );
        assert_eq!(refused.field.as_deref(), Some("/folder"), "{folder}");
        assert_eq!(world.versions.trees(), 0, "{folder}: nothing read");
        assert!(world.versions.commits()?.is_empty(), "{folder}");
    }
    Ok(())
}

/// F4: the head moved; a change under the folder conflicts, and a change anywhere else (a
/// delete has no destination, so `archive/` is elsewhere too) is committed on the head.
#[tokio::test]
async fn delete_folder_conflicts_on_a_change_under_it_and_commits_past_one_elsewhere() -> TestResult
{
    let world = moved_head(vec![change(
        FileChangeKind::Modified,
        Some("notes/sub/b.md"),
        Some("notes/sub/b.md"),
    )])?;
    let refused = err_of(deleted(&world, 1, delete_request("notes")?).await)?;
    assert_stale(&world, &refused, &revision('c')?, "under the folder")?;
    let world = moved_head(vec![
        change(FileChangeKind::Added, None, Some("archive/new.md")),
        change(FileChangeKind::Added, None, Some("notes-old/new.md")),
    ])?;
    let result = deleted(&world, 1, delete_request("notes")?).await?;
    assert_eq!(result.revision, revision('d')?);
    let commits = world.versions.commits()?;
    assert_eq!(
        commits.first().map(|commit| &commit.expected_head),
        Some(&revision('c')?)
    );
    Ok(())
}

/// A retry after a failure past the commit answers the first attempt, compares nothing again
/// and writes no second commit.
#[tokio::test]
async fn delete_folder_retried_after_its_commit_answers_the_first_attempt() -> TestResult {
    let world = moved_head(Vec::new())?;
    world.search.fail_next();
    err_of(deleted(&world, 1, delete_request("notes")?).await)?;
    let retried = deleted(&world, 1, delete_request("notes")?).await?;
    assert_eq!(retried.revision, revision('d')?);
    assert_eq!(retried.items, 2);
    assert_eq!(world.versions.commits()?.len(), 1);
    assert_eq!(world.versions.diff_queries()?.len(), 1);
    Ok(())
}

/// F4 and the request's "a folder changed since base conflicts": an item's versioned source,
/// supplied text and corrections are part of that item even outside its visible folder. A
/// companion of an item elsewhere must not block either folder operation.
#[tokio::test]
async fn folder_operations_check_companion_changes_of_contained_items_only() -> TestResult {
    for delete in [false, true] {
        for inside in [false, true] {
            let item = if inside { 25 } else { 26 };
            let id = Uuid::from_u128(item);
            for companion in [
                format!(".okf/sources/{id}.json"),
                format!(".okf/supplied/{id}.json"),
                format!(".okf/corrections/{id}/{}.json", digest('b')?.as_str()),
            ] {
                let world = moved_head(vec![change(
                    FileChangeKind::Modified,
                    Some(&companion),
                    Some(&companion),
                )])?;
                for (source, location) in [(25, "notes/source.md"), (26, "elsewhere/source.md")] {
                    world.versions.put_document(
                        &revision('a')?,
                        source_card(
                            source,
                            &revision('a')?,
                            location,
                            "Unchanged converter text.",
                            converted(&digest('b')?),
                        )?,
                    )?;
                }
                let outcome = if delete {
                    deleted(&world, 1, delete_request("notes")?).await
                } else {
                    moved(&world, 1, move_request("notes", "archive")?).await
                };
                if inside {
                    let error = err_of(outcome)?;
                    assert_stale(&world, &error, &revision('c')?, &companion)?;
                } else {
                    assert_eq!(outcome?.revision, revision('d')?, "{companion}");
                    assert_eq!(world.versions.commits()?.len(), 1, "{companion}");
                }
                let queries = world.versions.diff_queries()?;
                let [query] = queries.as_slice() else {
                    return Err(format!("not one comparison for {companion}: {queries:?}").into());
                };
                assert_eq!(query.from, revision('a')?);
                assert_eq!(query.to, revision('c')?);
                assert_eq!(
                    query.item_id, None,
                    "one workspace comparison, not one per item"
                );
            }
        }
    }
    Ok(())
}
