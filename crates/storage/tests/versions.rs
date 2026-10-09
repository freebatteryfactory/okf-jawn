//! The workspace catalog and the version store against real Git repositories.
//!
//! `mutation_crash_reconcile_*` tests are the receipt of `mutation-crash-reconcile`;
//! `draft_per_editor_second_snapshot_conflicts_after_the_first` is the Snapshot half of
//! `draft-per-editor`.
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{ItemId, MutationId, Revision, TenantId, WorkspacePath};
use okf_jawn_contract::item::{ItemKind, ItemStatus};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::mutations::{BeginOutcome, MutationKey, MutationStore};
use okf_jawn_core::storage::{
    BlobStore, CandidateChanges, CandidateCheck, CommitChanges, LogQuery, NewWorkspace, Page,
    Promotion, Provenance, StorageScope, TreeEdit, VersionStore, WorkspaceArchive,
    WorkspaceCatalog, WorkspaceUpdate,
};
use okf_jawn_storage::Storage;
use serde_json::json;
use tokio::io::AsyncReadExt as _;

use check::{TestResult, err_of, some};

/// The identity numbered `n`, of the type the context names.
macro_rules! uuid {
    ($n:expr) => {
        serde_json::from_value(json!(uuid_text($n)))
    };
}

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// A check that accepts every candidate and counts how often it ran.
#[derive(Default)]
struct Counting {
    runs: AtomicUsize,
}

/// A check that refuses every candidate.
struct Refusing;

/// A check that refuses a candidate naming the file by its full staged path, as a conformance
/// check reporting where it found a fault would.
struct NamingStagedFile;

/// A check that, while the commit is staged, moves the workspace head as a concurrent writer
/// would (another handle on the same repository), and records where it moved it.
struct Interloper {
    repository: std::path::PathBuf,
    moved_to: std::sync::Mutex<Option<git2::Oid>>,
}

impl CandidateCheck for Counting {
    fn check(&self, root: &Path) -> Result<Vec<okf_jawn_contract::common::Warning>, ApiError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        assert!(root.is_dir(), "the check reads a staged directory");
        Ok(Vec::new())
    }
}

impl CandidateCheck for Refusing {
    fn check(&self, _root: &Path) -> Result<Vec<okf_jawn_contract::common::Warning>, ApiError> {
        Err(ApiError::new(ErrorCode::InvalidInput, "not conformant"))
    }
}

impl CandidateCheck for NamingStagedFile {
    fn check(&self, root: &Path) -> Result<Vec<okf_jawn_contract::common::Warning>, ApiError> {
        Err(ApiError::new(
            ErrorCode::InvalidInput,
            format!("{}/notes/x.md is not conformant", root.display()),
        ))
    }
}

impl CandidateCheck for Interloper {
    fn check(&self, _root: &Path) -> Result<Vec<okf_jawn_contract::common::Warning>, ApiError> {
        let fault = |error: git2::Error| ApiError::new(ErrorCode::Internal, error.to_string());
        let repository = git2::Repository::open_bare(&self.repository).map_err(fault)?;
        let head = repository.refname_to_id("refs/heads/main").map_err(fault)?;
        let parent = repository.find_commit(head).map_err(fault)?;
        let tree = parent.tree().map_err(fault)?;
        let who = git2::Signature::now("other", "other").map_err(fault)?;
        let moved = repository
            .commit(
                Some("refs/heads/main"),
                &who,
                &who,
                "a concurrent move",
                &tree,
                &[&parent],
            )
            .map_err(fault)?;
        if let Ok(mut slot) = self.moved_to.lock() {
            *slot = Some(moved);
        }
        Ok(Vec::new())
    }
}

/// A fixed UUID spelling numbered `n`.
fn uuid_text(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

fn person(subject: &str) -> Provenance {
    Provenance {
        subject: subject.to_owned(),
        route: AccessRoute::LocalOwner,
        client_id: None,
    }
}

fn path(text: &str) -> Fallible<WorkspacePath> {
    Ok(WorkspacePath::try_from(text.to_owned())?)
}

fn note(item: ItemId, at: &str, body: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::CreateItem {
        item_id: item,
        path: path(at)?,
        title: Some(format!("Note at {at}")),
        type_name: "Note".to_owned(),
        kind: ItemKind::Note,
        body: body.to_owned(),
        properties: BTreeMap::from([("description".to_owned(), json!("a note"))]),
    })
}

fn changes(mutation: u32, expected: &Revision, edits: Vec<TreeEdit>) -> Fallible<CommitChanges> {
    Ok(CommitChanges {
        mutation_id: uuid!(mutation)?,
        expected_head: expected.clone(),
        author: person("ana"),
        message: format!("change {mutation}"),
        edits,
    })
}

/// A data directory with one blank workspace.
async fn workspace(directory: &Path) -> Fallible<(Storage, StorageScope, Revision)> {
    let storage = Storage::open(directory)?;
    let tenant = TenantId::try_from("local".to_owned())?;
    let created = storage
        .catalog()
        .create(
            &tenant,
            uuid!(1)?,
            NewWorkspace {
                name: "Research".to_owned(),
                description: "Field notes".to_owned(),
                creator: person("ana"),
            },
        )
        .await?;
    let scope = StorageScope {
        tenant_id: tenant,
        workspace_id: created.id,
    };
    Ok((storage, scope, created.head))
}

/// The log, diff and blame of the moved item in `items_are_shown_listed_moved...`.
async fn history_follows_the_move(
    versions: &okf_jawn_storage::GitVersions,
    scope: &StorageScope,
    first: &Revision,
    moved: &Revision,
) -> TestResult {
    let history = versions
        .log(
            scope,
            LogQuery {
                tip: moved.clone(),
                item_id: Some(uuid!(11)?),
                page: Page {
                    cursor: None,
                    limit: 50,
                },
            },
        )
        .await?;
    assert_eq!(history.commits.len(), 2, "created, then moved");
    let diff = versions
        .diff(
            scope,
            okf_jawn_core::storage::DiffQuery {
                from: first.clone(),
                to: moved.clone(),
                item_id: Some(uuid!(11)?),
            },
        )
        .await?;
    assert_eq!(diff.changes.len(), 1);
    let blame = versions
        .blame(
            scope,
            okf_jawn_core::storage::BlameQuery {
                revision: moved.clone(),
                item_id: uuid!(11)?,
                lines: okf_jawn_contract::common::TextRange { start: 1, end: 1 },
            },
        )
        .await?;
    assert_eq!(
        some(blame.lines.first(), "a blamed line")?.text,
        "The plan."
    );
    Ok(())
}

/// How many files lie under `dir`, at any depth.
fn files_under(dir: &Path) -> std::io::Result<usize> {
    let mut count = 0_usize;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        count = count.saturating_add(if entry.file_type()?.is_dir() {
            files_under(&entry.path())?
        } else {
            1
        });
    }
    Ok(count)
}

fn staging_is_empty(storage: &Storage) -> Fallible<bool> {
    Ok(files_under(&storage.data().staging_path())? == 0)
}

#[tokio::test]
async fn mutation_crash_reconcile_retry_returns_the_first_commit_and_writes_no_second() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let ledger = storage.mutations();
    let versions = storage.versions();
    let key = MutationKey {
        tenant_id: scope.tenant_id.clone(),
        subject: "ana".to_owned(),
        client_id: None,
        operation: okf_jawn_contract::metadata::OperationName::CreateItem,
        key: uuid!(90)?,
    };
    let digest = okf_jawn_contract::identity::Digest::try_from("a".repeat(64))?;
    let BeginOutcome::New(lease) = ledger.begin(&key, &digest).await? else {
        return Err("expected a new mutation".into());
    };
    let item: ItemId = uuid!(10)?;
    let commit = |check: Arc<Counting>| -> Fallible<_> {
        let edits = vec![note(item, "notes/first.md", "Hello.")?];
        Ok((
            CommitChanges {
                mutation_id: lease.mutation_id,
                expected_head: initial.clone(),
                author: person("ana"),
                message: "Create the first note".to_owned(),
                edits,
            },
            check,
        ))
    };
    let first_check = Arc::new(Counting::default());
    let (first_changes, check) = commit(Arc::clone(&first_check))?;
    let first = versions.commit(&scope, first_changes, check).await?;
    assert!(!first.replayed);
    assert_eq!(first_check.runs.load(Ordering::SeqCst), 1);
    // The process dies here: `MutationStore::complete` is never written. The lease runs out.
    let connection = rusqlite::Connection::open(storage.data().records_path())?;
    connection.execute("UPDATE mutations SET lease_expires_ms = 0", [])?;
    let BeginOutcome::Abandoned { lease: resumed } = ledger.begin(&key, &digest).await? else {
        return Err("expected the crashed mutation to resume".into());
    };
    assert_eq!(resumed.mutation_id, lease.mutation_id);
    let retry_check = Arc::new(Counting::default());
    let (retry_changes, check) = commit(Arc::clone(&retry_check))?;
    let again = versions.commit(&scope, retry_changes, check).await?;
    assert!(
        again.replayed,
        "the retry finds the commit its first attempt wrote"
    );
    assert_eq!(again.revision, first.revision);
    assert_eq!(
        retry_check.runs.load(Ordering::SeqCst),
        0,
        "a replay runs no check"
    );
    assert_eq!(versions.head(&scope).await?, first.revision);
    let log = versions
        .log(
            &scope,
            LogQuery {
                tip: first.revision.clone(),
                item_id: None,
                page: Page {
                    cursor: None,
                    limit: 50,
                },
            },
        )
        .await?;
    assert_eq!(
        log.commits.len(),
        2,
        "the creation commit and one commit, no second"
    );
    let found = versions
        .find_commit(&scope, lease.mutation_id, &initial)
        .await?;
    assert_eq!(found, Some(first.revision));
    Ok(())
}

#[tokio::test]
async fn mutation_crash_reconcile_commit_carries_the_trailer_and_a_moved_head_conflicts()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let first = versions
        .commit(
            &scope,
            changes(20, &initial, vec![note(uuid!(10)?, "a.md", "A")?])?,
            Arc::new(Counting::default()),
        )
        .await?;
    let log = versions
        .log(
            &scope,
            LogQuery {
                tip: first.revision.clone(),
                item_id: None,
                page: Page {
                    cursor: None,
                    limit: 1,
                },
            },
        )
        .await?;
    let newest = some(log.commits.first(), "the newest commit")?;
    assert_eq!(
        newest.message, "change 20",
        "the trailer is not part of the shown message"
    );
    let repository = git2::Repository::open_bare(
        directory
            .path()
            .join("repositories")
            .join("local")
            .join(format!("{}.git", scope.workspace_id.0)),
    )?;
    let raw = repository.find_commit(git2::Oid::from_str(first.revision.as_str())?)?;
    let mutation: MutationId = uuid!(20)?;
    assert!(
        raw.message()?
            .contains(&format!("Okf-Jawn-Mutation: {}", mutation.0)),
        "{:?}",
        raw.message()
    );
    let error = err_of(
        versions
            .commit(
                &scope,
                changes(21, &initial, vec![note(uuid!(11)?, "b.md", "B")?])?,
                Arc::new(Counting::default()),
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict);
    assert!(
        staging_is_empty(&storage)?,
        "staging is removed after every call"
    );
    Ok(())
}

#[tokio::test]
async fn draft_per_editor_second_snapshot_conflicts_after_the_first() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let drafts = storage.drafts();
    let item: ItemId = uuid!(10)?;
    let base = versions
        .commit(
            &scope,
            changes(20, &initial, vec![note(item, "shared.md", "Original.")?])?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let document = versions.show(&scope, &base, item).await?;
    for (editor, mutation, body) in [("ana", 30, "Ana's text."), ("ben", 31, "Ben's text.")] {
        drafts
            .save(
                &scope,
                uuid!(mutation)?,
                DraftWrite {
                    item_id: item,
                    editor: editor.to_owned(),
                    base_revision: base.clone(),
                    body: body.to_owned(),
                    properties: document.properties.clone(),
                    content_digest: okf_jawn_contract::identity::Digest::try_from("c".repeat(64))?,
                },
            )
            .await?;
    }
    let ana = some(drafts.get(&scope, item, "ana").await?, "ana's draft")?;
    let snapshot = |mutation: u32, body: String| -> Fallible<CommitChanges> {
        changes(
            mutation,
            &base,
            vec![TreeEdit::EditItem {
                item_id: item,
                body,
                properties: document.properties.clone(),
            }],
        )
    };
    versions
        .commit(
            &scope,
            snapshot(40, ana.body)?,
            Arc::new(Counting::default()),
        )
        .await?;
    drafts.discard(&scope, uuid!(40)?, item, "ana").await?;
    let ben = some(drafts.get(&scope, item, "ben").await?, "ben's draft")?;
    let error = err_of(
        versions
            .commit(
                &scope,
                snapshot(41, ben.body)?,
                Arc::new(Counting::default()),
            )
            .await,
    )?;
    assert_eq!(
        error.code,
        ErrorCode::Conflict,
        "ben's snapshot is based on a moved head"
    );
    let kept = some(drafts.get(&scope, item, "ben").await?, "ben's draft")?;
    assert_eq!(
        kept.body, "Ben's text.",
        "a conflicting snapshot keeps the draft"
    );
    Ok(())
}

#[tokio::test]
async fn a_commit_is_staged_checked_and_refused_whole() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let error = err_of(
        versions
            .commit(
                &scope,
                changes(20, &initial, vec![note(uuid!(10)?, "a.md", "A")?])?,
                Arc::new(Refusing),
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(
        versions.head(&scope).await?,
        initial,
        "a refused check writes nothing"
    );
    let created = versions
        .commit(
            &scope,
            changes(21, &initial, vec![note(uuid!(10)?, "notes/Plan.md", "A")?])?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let collision = err_of(
        versions
            .commit(
                &scope,
                changes(22, &created, vec![note(uuid!(11)?, "Notes/plan.md", "B")?])?,
                Arc::new(Counting::default()),
            )
            .await,
    )?;
    assert_eq!(
        collision.code,
        ErrorCode::Conflict,
        "a case-folded path collides"
    );
    let mut supplied = BTreeMap::new();
    supplied.insert("okf_jawn".to_owned(), json!({ "item_id": uuid_text(12) }));
    let header = err_of(
        versions
            .commit(
                &scope,
                changes(
                    23,
                    &created,
                    vec![TreeEdit::CreateItem {
                        item_id: uuid!(12)?,
                        path: path("c.md")?,
                        title: None,
                        type_name: "Note".to_owned(),
                        kind: ItemKind::Note,
                        body: String::new(),
                        properties: supplied,
                    }],
                )?,
                Arc::new(Counting::default()),
            )
            .await,
    )?;
    assert_eq!(
        header.code,
        ErrorCode::InvalidInput,
        "a caller cannot supply the header"
    );
    let document = versions.show(&scope, &created, uuid!(10)?).await?;
    let mut changed = document.properties.clone();
    changed.insert("okf_jawn".to_owned(), json!({ "item_id": uuid_text(99) }));
    let edit = err_of(
        versions
            .commit(
                &scope,
                changes(
                    24,
                    &created,
                    vec![TreeEdit::EditItem {
                        item_id: uuid!(10)?,
                        body: "x".to_owned(),
                        properties: changed,
                    }],
                )?,
                Arc::new(Counting::default()),
            )
            .await,
    )?;
    assert_eq!(
        edit.code,
        ErrorCode::InvalidInput,
        "a caller cannot change the header"
    );
    assert_eq!(versions.head(&scope).await?, created);
    assert!(staging_is_empty(&storage)?);
    Ok(())
}

#[tokio::test]
async fn a_reserved_name_in_any_letter_case_is_refused_naming_the_path() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let reserved = [
        "notes/INDEX.md",
        "notes/index.MD",
        ".OKF/x.md",
        "Log.md",
        "index.md",
        "deep/er/LOG.md",
        "a/INDEX.MD/b.md",
        "LOG.md/x.md",
        "a/Index.md/c.md",
        ".okf/sources/x.md",
        "notes/.Okf/x.md",
    ];
    for (mutation, at) in (30..).zip(reserved) {
        let error = err_of(
            versions
                .commit(
                    &scope,
                    changes(mutation, &initial, vec![note(uuid!(10)?, at, "Mine.")?])?,
                    Arc::new(Counting::default()),
                )
                .await,
        )?;
        assert_eq!(
            error.code,
            ErrorCode::InvalidInput,
            "{at}: {}",
            error.message
        );
        assert_eq!(error.field.as_deref(), Some("/path"), "{at}");
        assert!(error.message.contains(at), "{at}: {}", error.message);
        assert!(
            !error.message.contains("staging"),
            "{at}: no server path leaks: {}",
            error.message
        );
        assert_eq!(
            versions.head(&scope).await?,
            initial,
            "{at}: nothing committed"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_shown_item_carries_the_content_digest_core_computes() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let first = versions
        .commit(
            &scope,
            changes(
                20,
                &initial,
                vec![
                    note(uuid!(10)?, "a.md", "One.")?,
                    note(uuid!(11)?, "b.md", "Two.")?,
                ],
            )?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let one = versions.show(&scope, &first, uuid!(10)?).await?;
    let two = versions.show(&scope, &first, uuid!(11)?).await?;
    assert_eq!(
        one.content_digest,
        okf_jawn_core::portable::item_content_digest(&one)?
    );
    assert_ne!(
        one.content_digest, two.content_digest,
        "other content has another digest"
    );
    Ok(())
}

#[tokio::test]
async fn a_refusal_never_names_the_server_staging_directory() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let error = err_of(
        storage
            .versions()
            .commit(
                &scope,
                changes(20, &initial, vec![note(uuid!(10)?, "notes/x.md", "X.")?])?,
                Arc::new(NamingStagedFile),
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.message, "notes/x.md is not conformant");
    Ok(())
}

#[tokio::test]
async fn a_move_or_folder_onto_a_reserved_name_is_refused_naming_the_path() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let created = versions
        .commit(
            &scope,
            changes(
                50,
                &initial,
                vec![note(uuid!(10)?, "notes/plan.md", "Mine.")?],
            )?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let moved = err_of(
        versions
            .commit(
                &scope,
                changes(
                    41,
                    &created,
                    vec![TreeEdit::MoveItem {
                        item_id: uuid!(10)?,
                        destination: path("notes/Index.md")?,
                    }],
                )?,
                Arc::new(Counting::default()),
            )
            .await,
    )?;
    assert_eq!(moved.code, ErrorCode::InvalidInput, "{}", moved.message);
    assert_eq!(moved.field.as_deref(), Some("/destination"));
    assert!(
        moved.message.contains("notes/Index.md"),
        "{}",
        moved.message
    );
    for (mutation, folder) in [
        (42, ".Okf/inner"),
        (43, "notes/INDEX.md"),
        (44, "a/Log.MD/inner"),
    ] {
        let refused = err_of(
            versions
                .commit(
                    &scope,
                    changes(
                        mutation,
                        &created,
                        vec![TreeEdit::CreateFolder {
                            folder: path(folder)?,
                        }],
                    )?,
                    Arc::new(Counting::default()),
                )
                .await,
        )?;
        assert_eq!(
            refused.code,
            ErrorCode::InvalidInput,
            "{folder}: {}",
            refused.message
        );
        assert_eq!(refused.field.as_deref(), Some("/folder"), "{folder}");
    }
    // A folder an item path needs, where a file already is: refused naming the request's path.
    let blocked = err_of(
        versions
            .commit(
                &scope,
                changes(
                    45,
                    &created,
                    vec![note(uuid!(11)?, "notes/PLAN.md/inner.md", "Below a file.")?],
                )?,
                Arc::new(Counting::default()),
            )
            .await,
    )?;
    assert_eq!(blocked.code, ErrorCode::Conflict, "{}", blocked.message);
    assert!(
        blocked.message.contains("notes/PLAN.md/inner.md") && !blocked.message.contains("staging"),
        "{}",
        blocked.message
    );
    assert_eq!(versions.head(&scope).await?, created);
    let kept = versions.show(&scope, &created, uuid!(10)?).await?;
    assert_eq!(kept.body, "Mine.", "the item's content is intact");
    Ok(())
}

#[tokio::test]
async fn items_are_shown_listed_moved_with_their_links_and_archived() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let first = versions
        .commit(
            &scope,
            changes(
                20,
                &initial,
                vec![
                    note(uuid!(10)?, "a.md", "See [the plan](b.md).")?,
                    note(uuid!(11)?, "b.md", "The plan.\n\n```vega-lite\n{}\n```")?,
                    TreeEdit::CreateItem {
                        item_id: uuid!(12)?,
                        path: path("chart.md")?,
                        title: None,
                        type_name: "Chart".to_owned(),
                        kind: ItemKind::View,
                        body: "No fence here.".to_owned(),
                        properties: BTreeMap::new(),
                    },
                ],
            )?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let shown = versions.show(&scope, &first, uuid!(11)?).await?;
    assert_eq!(
        shown.summary.kind,
        ItemKind::Note,
        "a fenced vega-lite example does not make a Note a View"
    );
    assert_eq!(shown.summary.status, ItemStatus::Stable);
    assert_eq!(shown.summary.description, "a note");
    assert_eq!(shown.properties.get("type"), Some(&json!("Note")));
    assert_eq!(
        shown.properties.get("okf_jawn"),
        Some(&json!({ "item_id": uuid_text(11), "kind": "note" }))
    );
    let view = versions.show(&scope, &first, uuid!(12)?).await?;
    assert_eq!(
        view.summary.kind,
        ItemKind::View,
        "the kind the creation names is the kind read back"
    );
    let listing = versions
        .list(
            &scope,
            &first,
            None,
            Page {
                cursor: None,
                limit: 50,
            },
        )
        .await?;
    assert_eq!(listing.items.len(), 3);
    let index = versions
        .read_file(&scope, &first, &path("index.md")?)
        .await?;
    assert!(!index.is_empty(), "the folder index is maintained");
    let moved = versions
        .commit(
            &scope,
            changes(
                21,
                &first,
                vec![
                    TreeEdit::MoveItem {
                        item_id: uuid!(11)?,
                        destination: path("plans/b.md")?,
                    },
                    TreeEdit::SetStatus {
                        item_id: uuid!(10)?,
                        status: Some(ItemStatus::Deprecated),
                        archived: Some(true),
                    },
                ],
            )?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let linking = versions.show(&scope, &moved, uuid!(10)?).await?;
    assert!(linking.body.contains("plans/b.md"), "{}", linking.body);
    assert!(linking.summary.archived);
    assert_eq!(linking.summary.status, ItemStatus::Deprecated);
    let plan = versions.show(&scope, &moved, uuid!(11)?).await?;
    assert_eq!(plan.summary.path.as_str(), "plans/b.md");
    history_follows_the_move(&storage.versions(), &scope, &first, &moved).await
}

#[tokio::test]
async fn a_candidate_is_retained_off_the_head_and_promoted_exactly() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let proposal = uuid!(50)?;
    let candidate_changes = CandidateChanges {
        mutation_id: uuid!(51)?,
        base: initial.clone(),
        author: person("agent"),
        message: "Propose a note".to_owned(),
        edits: vec![note(uuid!(10)?, "proposed.md", "Proposed.")?],
    };
    let candidate = versions
        .create_candidate(
            &scope,
            proposal,
            candidate_changes.clone(),
            Arc::new(Counting::default()),
        )
        .await?;
    assert_eq!(
        versions.head(&scope).await?,
        initial,
        "the head does not move"
    );
    let again = versions
        .create_candidate(
            &scope,
            proposal,
            candidate_changes,
            Arc::new(Counting::default()),
        )
        .await?;
    assert_eq!(again, candidate);
    let promotion = Promotion {
        mutation_id: uuid!(52)?,
        proposal_id: proposal,
        expected_head: initial.clone(),
        candidate: candidate.clone(),
        approver: person("ana"),
        message: "Accept the note".to_owned(),
    };
    let accepted = versions
        .promote_candidate(&scope, promotion.clone())
        .await?;
    assert!(!accepted.replayed);
    let replay = versions
        .promote_candidate(&scope, promotion.clone())
        .await?;
    assert!(replay.replayed);
    assert_eq!(replay.revision, accepted.revision);
    let shown = versions
        .show(&scope, &accepted.revision, uuid!(10)?)
        .await?;
    assert_eq!(shown.body, "Proposed.");
    let stale = err_of(
        versions
            .promote_candidate(
                &scope,
                Promotion {
                    mutation_id: uuid!(53)?,
                    ..promotion
                },
            )
            .await,
    )?;
    assert_eq!(
        stale.code,
        ErrorCode::Conflict,
        "the head moved since the confirmation"
    );
    Ok(())
}

fn repository_path(directory: &Path, scope: &StorageScope) -> std::path::PathBuf {
    directory
        .join("repositories")
        .join(scope.tenant_id.as_str())
        .join(format!("{}.git", scope.workspace_id.0))
}

#[tokio::test]
async fn a_concurrent_head_move_wins_and_the_commit_conflicts() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let interloper = Arc::new(Interloper {
        repository: repository_path(directory.path(), &scope),
        moved_to: std::sync::Mutex::new(None),
    });
    let error = err_of(
        versions
            .commit(
                &scope,
                changes(20, &initial, vec![note(uuid!(10)?, "a.md", "Mine.")?])?,
                Arc::clone(&interloper) as Arc<dyn CandidateCheck>,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    let moved = some(
        interloper.moved_to.lock().ok().and_then(|slot| *slot),
        "the concurrent move",
    )?;
    assert_eq!(
        versions.head(&scope).await?.as_str(),
        moved.to_string(),
        "the reference moves only from the head the commit was staged on"
    );
    Ok(())
}

#[tokio::test]
async fn git_writes_are_synced_before_a_commit_returns() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let path = repository_path(directory.path(), &scope);
    let setting = |path: &Path| -> Fallible<bool> {
        let repository = git2::Repository::open_bare(path)?;
        let local = repository.config()?.open_level(git2::ConfigLevel::Local)?;
        Ok(local.get_bool("core.fsyncObjectFiles")?)
    };
    // Read right after `catalog().create` (all `workspace` does), before any other store opens
    // the repository: the catalog's first commit and `refs/heads/main` were written synced.
    assert!(setting(&path)?, "a created repository syncs its writes");
    // A repository without the setting (as an older build left it) gets it before it writes.
    git2::Repository::open_bare(&path)?
        .config()?
        .open_level(git2::ConfigLevel::Local)?
        .remove("core.fsyncObjectFiles")?;
    let versions = storage.versions();
    versions.head(&scope).await?;
    storage.catalog().open(&scope).await?;
    assert!(
        !setting(&path).unwrap_or(false),
        "a read changes nothing, so only a write path can have set the setting"
    );
    storage
        .versions()
        .commit(
            &scope,
            changes(20, &initial, vec![note(uuid!(10)?, "a.md", "Mine.")?])?,
            Arc::new(Counting::default()),
        )
        .await?;
    assert!(setting(&path)?, "the setting is restored before the commit");
    Ok(())
}

#[tokio::test]
async fn a_quoted_trailer_line_in_a_message_body_replays_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let quoted = format!(
        "Explain an earlier change\n\nIt said:\nOkf-Jawn-Mutation: {}\nand more.",
        uuid_text(31)
    );
    let quoting = versions
        .commit(
            &scope,
            CommitChanges {
                message: quoted,
                ..changes(30, &initial, vec![note(uuid!(10)?, "a.md", "A.")?])?
            },
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    assert_eq!(
        versions.find_commit(&scope, uuid!(31)?, &initial).await?,
        None,
        "a quoted line is not the trailer of mutation 31"
    );
    let error = err_of(
        versions
            .commit(
                &scope,
                changes(31, &initial, vec![note(uuid!(11)?, "b.md", "B.")?])?,
                Arc::new(Counting::default()),
            )
            .await,
    )?;
    assert_eq!(
        error.code,
        ErrorCode::Conflict,
        "mutation 31 is not replayed as the quoting commit: {}",
        error.message
    );
    assert_eq!(
        versions.find_commit(&scope, uuid!(30)?, &initial).await?,
        Some(quoting.clone()),
        "the real trailer is still found"
    );
    let shown = versions
        .log(
            &scope,
            LogQuery {
                tip: quoting,
                item_id: None,
                page: Page {
                    cursor: None,
                    limit: 1,
                },
            },
        )
        .await?;
    let message = &some(shown.commits.first(), "the quoting commit")?.message;
    assert!(
        message.contains(&format!("Okf-Jawn-Mutation: {}", uuid_text(31))),
        "the body keeps the quoted line: {message}"
    );
    assert!(
        !message.contains(&uuid_text(30)),
        "the trailer itself is not shown: {message}"
    );
    Ok(())
}

/// Clone a bundle with stock `git` and open the clone with git2.
fn clone_bundle(bundle: &Path, into: &Path) -> Fallible<git2::Repository> {
    let output = std::process::Command::new("git")
        .arg("clone")
        .arg("--quiet")
        .arg(bundle)
        .arg(into)
        .output()
        .map_err(|error| {
            format!(
                "this test runs stock `git` to clone the history bundle, so `git` must be on \
                 PATH; starting it failed: {error}"
            )
        })?;
    assert!(
        output.status.success(),
        "stock git did not clone the bundle: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(git2::Repository::open(into)?)
}

#[tokio::test]
async fn history_is_a_bundle_of_the_accepted_line_that_stock_git_clones() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let first = versions
        .commit(
            &scope,
            changes(20, &initial, vec![note(uuid!(10)?, "a.md", "First.")?])?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let second = versions
        .commit(
            &scope,
            changes(21, &first, vec![note(uuid!(11)?, "b.md", "Second.")?])?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let candidate = versions
        .create_candidate(
            &scope,
            uuid!(50)?,
            CandidateChanges {
                mutation_id: uuid!(51)?,
                base: second.clone(),
                author: person("agent"),
                message: "Propose".to_owned(),
                edits: vec![note(uuid!(12)?, "proposed.md", "Proposed.")?],
            },
            Arc::new(Counting::default()),
        )
        .await?;
    let off_line = err_of(versions.write_history(&scope, &candidate, uuid!(60)?).await)?;
    assert_eq!(
        off_line.code,
        ErrorCode::NotFound,
        "a revision off the accepted line has no history to export"
    );

    let object = versions.write_history(&scope, &first, uuid!(61)?).await?;
    let retained = files_under(&storage.data().objects_path())?;
    let repeated = versions.write_history(&scope, &second, uuid!(61)?).await?;
    assert_eq!(
        repeated, object,
        "a repeated mutation id returns the object the first call wrote"
    );
    assert_eq!(
        files_under(&storage.data().objects_path())?,
        retained,
        "a repeated mutation id writes nothing"
    );
    let mut bytes = Vec::new();
    storage
        .blobs()
        .open(&scope, &object.digest, 0, object.size)
        .await?
        .body
        .read_to_end(&mut bytes)
        .await?;
    assert!(bytes.starts_with(b"# v2 git bundle\n"));
    let bundle = directory.path().join("history.bundle");
    std::fs::write(&bundle, &bytes)?;
    let clone = clone_bundle(&bundle, &directory.path().join("clone"))?;
    let cloned_head = some(clone.head()?.target(), "the cloned head")?;
    assert_eq!(cloned_head.to_string(), first.as_str());
    let parent = clone.find_commit(cloned_head)?.parent_id(0)?;
    assert_eq!(
        parent.to_string(),
        initial.as_str(),
        "the whole line, not one commit"
    );
    for absent in [&second, &candidate] {
        assert!(
            clone
                .find_commit(git2::Oid::from_str(absent.as_str())?)
                .is_err(),
            "{} is not on the line up to the revision",
            absent.as_str()
        );
    }
    let mut references = Vec::new();
    for reference in clone.references()? {
        references.push(reference?.name()?.to_owned());
    }
    assert!(
        references
            .iter()
            .all(|name| !name.contains("okf-jawn") && !name.contains("proposals")),
        "no proposal or candidate reference travels: {references:?}"
    );
    let workdir = some(clone.workdir(), "the clone's working tree")?;
    assert!(workdir.join("a.md").is_file());
    assert!(!workdir.join("b.md").exists());

    let whole = versions.write_history(&scope, &second, uuid!(62)?).await?;
    assert_ne!(
        whole, object,
        "another revision under another id is another bundle"
    );
    Ok(())
}

#[tokio::test]
async fn the_catalog_creates_once_lists_updates_and_archives() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let catalog = storage.catalog();
    let again = catalog
        .create(
            &scope.tenant_id,
            uuid!(1)?,
            NewWorkspace {
                name: "Other".to_owned(),
                description: String::new(),
                creator: person("ana"),
            },
        )
        .await?;
    assert_eq!(
        again.id, scope.workspace_id,
        "a repeated mutation creates nothing"
    );
    assert_eq!(again.name, "Research");
    assert_eq!(again.permissions.len(), 0);
    let updated = catalog
        .update(
            &scope,
            uuid!(2)?,
            WorkspaceUpdate {
                expected_head: initial.clone(),
                name: "Renamed".to_owned(),
                description: "New".to_owned(),
                author: person("ana"),
            },
        )
        .await?;
    assert_eq!(updated.name, "Renamed");
    let archived = catalog
        .archive(
            &scope,
            uuid!(3)?,
            WorkspaceArchive {
                expected_head: initial.clone(),
                author: person("ana"),
            },
        )
        .await?;
    assert!(archived.archived_at.is_some());
    assert_eq!(catalog.list(&scope.tenant_id, false).await?.len(), 0);
    assert_eq!(catalog.list(&scope.tenant_id, true).await?.len(), 1);
    let unarchived = catalog.unarchive(&scope, uuid!(4)?, person("ana")).await?;
    assert!(unarchived.archived_at.is_none());
    let repeated = catalog
        .archive(
            &scope,
            uuid!(3)?,
            WorkspaceArchive {
                expected_head: initial,
                author: person("ana"),
            },
        )
        .await?;
    assert!(
        repeated.archived_at.is_some(),
        "a repeated id returns the first result"
    );
    assert!(
        catalog.open(&scope).await?.archived_at.is_none(),
        "and changes nothing"
    );
    let missing = StorageScope {
        tenant_id: scope.tenant_id.clone(),
        workspace_id: uuid!(77)?,
    };
    let error = err_of(catalog.open(&missing).await)?;
    assert_eq!(error.code, ErrorCode::NotFound);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
