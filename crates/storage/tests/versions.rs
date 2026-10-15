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
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{ItemId, MutationId, Revision, TenantId, WorkspacePath};
use okf_jawn_contract::item::{ItemKind, ItemStatus};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::mutations::{BeginOutcome, MutationKey, MutationStore};
use okf_jawn_core::storage::{
    BlobStore, CandidateChanges, CandidateCheck, CommitChanges, Committed, LogQuery, NewWorkspace,
    Page, Promotion, Provenance, REPLAY_WARNINGS_UNAVAILABLE, StorageScope, TreeEdit, VersionStore,
    WorkspaceArchive, WorkspaceCatalog, WorkspaceUpdate,
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

/// A stored shape of an item file the serializer does not write.
type Shape = fn(&str) -> String;

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

/// A check that, while a candidate is staged, sets the candidate's proposal reference to the head
/// as another write of the same proposal would (another handle on the same repository).
struct SetsTheReference {
    repository: std::path::PathBuf,
    reference: String,
}

/// A check that accepts every candidate with the warnings it was made with, and counts its runs.
struct Warns {
    warnings: Vec<Warning>,
    runs: AtomicUsize,
}

/// A check that moves the head as `Interloper` does and then accepts with its warnings: the
/// staged commit passes its check and cannot land.
struct WarnsAsTheHeadMoves {
    interloper: Interloper,
    warnings: Vec<Warning>,
}

/// The closing `---` of a stored file's frontmatter, followed by `\n`.
const CLOSING: &str = "\n---\n";

impl CandidateCheck for Counting {
    fn check(
        &self,
        root: &Path,
        _rewritten: &[okf_jawn_contract::identity::WorkspacePath],
    ) -> Result<Vec<okf_jawn_contract::common::Warning>, ApiError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        assert!(root.is_dir(), "the check reads a staged directory");
        Ok(Vec::new())
    }
}

impl CandidateCheck for Refusing {
    fn check(
        &self,
        _root: &Path,
        _rewritten: &[okf_jawn_contract::identity::WorkspacePath],
    ) -> Result<Vec<okf_jawn_contract::common::Warning>, ApiError> {
        Err(ApiError::new(ErrorCode::InvalidInput, "not conformant"))
    }
}

impl CandidateCheck for NamingStagedFile {
    fn check(
        &self,
        root: &Path,
        _rewritten: &[okf_jawn_contract::identity::WorkspacePath],
    ) -> Result<Vec<okf_jawn_contract::common::Warning>, ApiError> {
        Err(ApiError::new(
            ErrorCode::InvalidInput,
            format!("{}/notes/x.md is not conformant", root.display()),
        ))
    }
}

impl CandidateCheck for Interloper {
    fn check(
        &self,
        _root: &Path,
        _rewritten: &[okf_jawn_contract::identity::WorkspacePath],
    ) -> Result<Vec<okf_jawn_contract::common::Warning>, ApiError> {
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

impl CandidateCheck for SetsTheReference {
    fn check(
        &self,
        _root: &Path,
        _rewritten: &[okf_jawn_contract::identity::WorkspacePath],
    ) -> Result<Vec<Warning>, ApiError> {
        let fault = |error: git2::Error| ApiError::new(ErrorCode::Internal, error.to_string());
        let repository = git2::Repository::open_bare(&self.repository).map_err(fault)?;
        let head = repository.refname_to_id("refs/heads/main").map_err(fault)?;
        repository
            .reference(&self.reference, head, false, "another write")
            .map_err(fault)?;
        Ok(Vec::new())
    }
}

impl Warns {
    fn of(warnings: Vec<Warning>) -> Self {
        Self {
            warnings,
            runs: AtomicUsize::new(0),
        }
    }
}

impl CandidateCheck for Warns {
    fn check(
        &self,
        _root: &Path,
        _rewritten: &[okf_jawn_contract::identity::WorkspacePath],
    ) -> Result<Vec<Warning>, ApiError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(self.warnings.clone())
    }
}

impl CandidateCheck for WarnsAsTheHeadMoves {
    fn check(
        &self,
        root: &Path,
        _rewritten: &[okf_jawn_contract::identity::WorkspacePath],
    ) -> Result<Vec<Warning>, ApiError> {
        self.interloper.check(root, &[])?;
        Ok(self.warnings.clone())
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
    let BeginOutcome::New(lease) = ledger
        .begin(&key, &digest, Some(scope.workspace_id))
        .await?
    else {
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
    let BeginOutcome::Abandoned { lease: resumed } = ledger
        .begin(&key, &digest, Some(scope.workspace_id))
        .await?
    else {
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
        .find_commit(&scope, lease.mutation_id, Some(&initial))
        .await?
        .map(|found| found.revision);
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
    // Ruling 5: the refusal names the head the expected one was compared with.
    assert_eq!(
        error.detail.as_deref(),
        Some(&ErrorDetail::StaleBase {
            head: versions.head(&scope).await?
        })
    );
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
    drafts
        .discard(&scope, uuid!(40)?, item, "ana", None)
        .await?;
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

/// The case-folded refusals of `a_commit_is_staged_checked_and_refused_whole`, on a tree that
/// holds `notes/Plan.md`. A file whose path folds onto it collides (`Conflict`). Under
/// storage-paths ruling S2, a folder the tree spells another way (`Notes/plan.md`) is refused
/// first, as `InvalidInput` naming the tree's spelling; before the ruling it was `Conflict`.
async fn case_folded_paths_are_refused(
    versions: &okf_jawn_storage::GitVersions,
    scope: &StorageScope,
    created: &Revision,
) -> TestResult {
    let respelled = err_of(
        versions
            .commit(
                scope,
                changes(22, created, vec![note(uuid!(11)?, "Notes/plan.md", "B")?])?,
                Arc::new(Counting::default()),
            )
            .await,
    )?;
    assert_eq!(respelled.code, ErrorCode::InvalidInput, "{respelled}");
    assert!(
        respelled.message.contains("the folder is spelled notes/"),
        "{respelled}"
    );
    let collision = err_of(
        versions
            .commit(
                scope,
                changes(22, created, vec![note(uuid!(11)?, "notes/plan.md", "B")?])?,
                Arc::new(Counting::default()),
            )
            .await,
    )?;
    assert_eq!(
        collision.code,
        ErrorCode::Conflict,
        "a case-folded path collides"
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
    case_folded_paths_are_refused(&versions, &scope, &created).await?;
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
async fn an_integer_the_item_file_cannot_hold_exactly_is_refused_on_its_field() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let with = |properties: serde_json::Value| -> Fallible<TreeEdit> {
        Ok(TreeEdit::CreateItem {
            item_id: uuid!(10)?,
            path: path("n.md")?,
            title: None,
            type_name: "Note".to_owned(),
            kind: ItemKind::Note,
            body: "N.".to_owned(),
            properties: serde_json::from_value(properties)?,
        })
    };
    for (mutation, properties, field) in [
        (20, json!({ "count": u64::MAX }), "/properties/count"),
        (21, json!({ "list": [1, u64::MAX] }), "/properties/list/1"),
        (
            22,
            json!({ "a/b": { "n": 9_223_372_036_854_775_808_u64 } }),
            "/properties/a~1b/n",
        ),
    ] {
        let error = err_of(
            versions
                .commit(
                    &scope,
                    changes(mutation, &initial, vec![with(properties)?])?,
                    Arc::new(Counting::default()),
                )
                .await,
        )?;
        assert_eq!(error.code, ErrorCode::InvalidInput, "{}", error.message);
        assert_eq!(error.field.as_deref(), Some(field));
    }
    assert_eq!(
        versions.head(&scope).await?,
        initial,
        "nothing was changed silently"
    );
    let largest = versions
        .commit(
            &scope,
            changes(23, &initial, vec![with(json!({ "count": i64::MAX }))?])?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let shown = versions.show(&scope, &largest, uuid!(10)?).await?;
    assert_eq!(shown.properties.get("count"), Some(&json!(i64::MAX)));
    Ok(())
}

#[tokio::test]
async fn find_commit_since_a_revision_the_repository_never_held_is_not_found() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, _) = workspace(directory.path()).await?;
    let unknown = Revision::try_from("ab".repeat(20))?;
    let error = err_of(
        storage
            .versions()
            .find_commit(&scope, uuid!(20)?, Some(&unknown))
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound, "{}", error.message);
    Ok(())
}

#[tokio::test]
async fn an_expected_head_the_repository_never_held_conflicts() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let unknown = Revision::try_from("ab".repeat(20))?;
    let error = err_of(
        versions
            .commit(
                &scope,
                changes(20, &unknown, vec![note(uuid!(10)?, "a.md", "A.")?])?,
                Arc::new(Counting::default()),
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    // Review N5: a head the repository never held is not the head (`held_head`), so the
    // refusal names the head as a moved head's does, and leaves the field to the caller.
    let stale = ErrorDetail::StaleBase {
        head: initial.clone(),
    };
    assert_eq!(error.detail.as_deref(), Some(&stale));
    assert_eq!(error.field, None);
    let promoted = err_of(
        versions
            .promote_candidate(
                &scope,
                Promotion {
                    mutation_id: uuid!(21)?,
                    proposal_id: uuid!(50)?,
                    expected_head: unknown.clone(),
                    candidate: unknown,
                    approver: person("ana"),
                    message: "Accept".to_owned(),
                },
            )
            .await,
    )?;
    assert_eq!(promoted.code, ErrorCode::Conflict, "{}", promoted.message);
    assert_eq!(promoted.detail.as_deref(), Some(&stale));
    assert_eq!(promoted.field, None);
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

/// Ruling 19: a candidate is answered with its check's findings, kept with it, so a repeat of
/// the mutation answers the same findings (`replayed`) and runs no check.
#[tokio::test]
async fn a_candidate_answers_its_checks_findings_and_a_repeat_the_same() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let finding = Warning {
        code: "missing_description".to_owned(),
        message: "proposed.md has no description".to_owned(),
        location: Some("proposed.md".to_owned()),
    };
    let check = Arc::new(Warns {
        warnings: vec![finding.clone()],
        runs: AtomicUsize::new(0),
    });
    let changes = CandidateChanges {
        mutation_id: uuid!(53)?,
        base: initial,
        author: person("agent"),
        message: "Propose a note".to_owned(),
        edits: vec![note(uuid!(12)?, "proposed.md", "Proposed.")?],
    };
    let first = versions
        .create_candidate(&scope, uuid!(54)?, changes.clone(), check.clone())
        .await?;
    assert!(!first.replayed);
    assert_eq!(
        serde_json::to_value(&first.warnings)?,
        serde_json::to_value([&finding])?
    );
    let again = versions
        .create_candidate(&scope, uuid!(54)?, changes, check.clone())
        .await?;
    assert!(again.replayed);
    assert_eq!(again.revision, first.revision);
    assert_eq!(
        serde_json::to_value(&again.warnings)?,
        serde_json::to_value([&finding])?
    );
    assert_eq!(
        check.runs.load(Ordering::SeqCst),
        1,
        "a repeat runs no check"
    );
    Ok(())
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
        .await?
        .revision;
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
        .await?
        .revision;
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
    assert!(accepted.warnings.is_empty(), "no check runs at a promotion");
    let replay = versions
        .promote_candidate(&scope, promotion.clone())
        .await?;
    assert!(replay.replayed);
    assert_eq!(replay.revision, accepted.revision);
    // R-I10: a replayed promotion answers what its first call did, no warning, and not the
    // warning that none were kept; so does the promotion found by its mutation.
    assert!(replay.warnings.is_empty(), "{:?}", replay.warnings);
    let by_mutation = versions.find_commit(&scope, uuid!(52)?, None).await?;
    let by_mutation = some(by_mutation, "the promotion by its mutation")?;
    assert!(
        by_mutation.warnings.is_empty(),
        "{:?}",
        by_mutation.warnings
    );
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
    // Review N5: the refusal names the head the expected one was compared with.
    assert_eq!(
        stale.detail.as_deref(),
        Some(&ErrorDetail::StaleBase {
            head: accepted.revision
        })
    );
    assert_eq!(stale.field, None);
    Ok(())
}

/// Review N5: a candidate whose proposal's reference another write set while it was staged is
/// refused with a plain `Conflict`: the refusal is about no base, so it carries no `StaleBase`.
#[tokio::test]
async fn a_candidate_whose_reference_another_write_set_meanwhile_is_a_plain_conflict() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let proposal: okf_jawn_contract::identity::ProposalId = uuid!(60)?;
    let check = Arc::new(SetsTheReference {
        repository: repository_path(directory.path(), &scope),
        reference: format!("refs/okf-jawn/proposals/{}", proposal.0),
    });
    let refused = err_of(
        storage
            .versions()
            .create_candidate(
                &scope,
                proposal,
                CandidateChanges {
                    mutation_id: uuid!(61)?,
                    base: initial.clone(),
                    author: person("agent"),
                    message: "Propose a note".to_owned(),
                    edits: vec![note(uuid!(12)?, "proposed.md", "Proposed.")?],
                },
                check,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict, "{}", refused.message);
    assert_eq!(
        refused.message,
        "this proposal already has a candidate from another write"
    );
    assert_eq!(refused.detail, None);
    assert_eq!(storage.versions().head(&scope).await?, initial);
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
        versions
            .find_commit(&scope, uuid!(31)?, Some(&initial))
            .await?
            .map(|found| found.revision),
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
        versions
            .find_commit(&scope, uuid!(30)?, Some(&initial))
            .await?
            .map(|found| found.revision),
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
        .await?
        .revision;
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
async fn a_creation_that_crashed_before_its_record_is_adopted_on_retry() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let tenant = TenantId::try_from("local".to_owned())?;
    let creation = || NewWorkspace {
        name: "Research".to_owned(),
        description: String::new(),
        creator: person("ana"),
    };
    let first = storage
        .catalog()
        .create(&tenant, uuid!(1)?, creation())
        .await?;
    assert_eq!(
        first.id,
        okf_jawn_core::storage::derive_workspace_id(uuid!(1)?)
    );
    // The crash: the repository was written, the record never committed.
    rusqlite::Connection::open(storage.data().records_path())?.execute(
        "DELETE FROM workspaces WHERE mutation_id = ?1",
        [uuid_text(1)],
    )?;
    let retried = storage
        .catalog()
        .create(&tenant, uuid!(1)?, creation())
        .await?;
    assert_eq!(retried.id, first.id, "the retry keeps the identity");
    assert_eq!(
        retried.head, first.head,
        "the repository is adopted, not written again"
    );
    let repositories =
        std::fs::read_dir(directory.path().join("repositories").join("local"))?.count();
    assert_eq!(repositories, 1, "no unreachable repository is left");
    assert_eq!(storage.catalog().list(&tenant, true).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_creation_that_crashed_mid_repository_replaces_it_on_retry() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let tenant = TenantId::try_from("local".to_owned())?;
    let id = okf_jawn_core::storage::derive_workspace_id(uuid!(1)?);
    // The crash: a repository directory was begun, nothing valid in it, no record.
    let partial = directory
        .path()
        .join("repositories")
        .join("local")
        .join(format!("{}.git", id.0));
    std::fs::create_dir_all(partial.join("objects"))?;
    std::fs::write(partial.join("description"), "half written")?;
    let created = storage
        .catalog()
        .create(
            &tenant,
            uuid!(1)?,
            NewWorkspace {
                name: "Research".to_owned(),
                description: String::new(),
                creator: person("ana"),
            },
        )
        .await?;
    assert_eq!(created.id, id);
    let repository = git2::Repository::open_bare(&partial)?;
    assert_eq!(
        some(
            repository.find_reference("refs/heads/main")?.target(),
            "the head"
        )?
        .to_string(),
        created.head.as_str(),
        "the partial directory was replaced by a whole repository"
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

/// Ruling 5: a workspace update or archive whose expected head is not the head is refused with
/// `StaleBase` naming the head, leaves the field to the caller, and changes nothing.
#[tokio::test]
async fn a_workspace_update_or_archive_on_a_moved_head_names_the_head() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let moved = storage
        .versions()
        .commit(
            &scope,
            changes(20, &initial, vec![note(uuid!(10)?, "a.md", "A")?])?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let catalog = storage.catalog();
    let updated = err_of(
        catalog
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
            .await,
    )?;
    let archived = err_of(
        catalog
            .archive(
                &scope,
                uuid!(3)?,
                WorkspaceArchive {
                    expected_head: initial,
                    author: person("ana"),
                },
            )
            .await,
    )?;
    for error in [&updated, &archived] {
        assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
        assert_eq!(
            error.detail.as_deref(),
            Some(&ErrorDetail::StaleBase {
                head: moved.clone()
            })
        );
        assert_eq!(error.field, None, "the caller names its request's field");
    }
    let shown = catalog.open(&scope).await?;
    assert_eq!(shown.name, "Research");
    assert!(shown.archived_at.is_none());
    Ok(())
}

/// Commit `message` with mutation `mutation`'s trailer on top of the head straight into the
/// workspace repository, its committer time `age_seconds` before now, and return it.
fn commit_aged(
    directory: &Path,
    scope: &StorageScope,
    mutation: u32,
    age_seconds: i64,
) -> Fallible<Revision> {
    let repository = git2::Repository::open_bare(repository_path(directory, scope))?;
    let parent = repository.find_commit(repository.refname_to_id("refs/heads/main")?)?;
    let tree = parent.tree()?;
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
    )?;
    let who = git2::Signature::new(
        "raw",
        "raw@example.invalid",
        &git2::Time::new(now.saturating_sub(age_seconds), 0),
    )?;
    let message = format!(
        "an aged change\n\nOkf-Jawn-Mutation: {}\n",
        uuid_text(mutation)
    );
    let commit = repository.commit(
        Some("refs/heads/main"),
        &who,
        &who,
        &message,
        &tree,
        &[&parent],
    )?;
    Ok(Revision::try_from(commit.to_string())?)
}

#[tokio::test]
async fn find_commit_with_no_base_searches_the_accepted_line_back_from_the_head() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let first = versions
        .commit(
            &scope,
            changes(30, &initial, vec![note(uuid!(10)?, "a.md", "A.")?])?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    // A draft saved now is based on `base`, a revision after the commit asked about.
    let base = versions
        .commit(
            &scope,
            changes(31, &first, vec![note(uuid!(11)?, "b.md", "B.")?])?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let head = versions
        .commit(
            &scope,
            changes(32, &base, vec![note(uuid!(12)?, "c.md", "C.")?])?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    assert_eq!(
        versions
            .find_commit(&scope, uuid!(30)?, Some(&base))
            .await?
            .map(|found| found.revision),
        None,
        "searching after a later base cannot see it"
    );
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(30)?, None).await?),
        Some(first),
        "with no base, a commit older than the draft's base is found"
    );
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(32)?, None).await?),
        Some(head),
        "the head itself is examined"
    );
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(33)?, None).await?),
        None,
        "a mutation no commit carries"
    );
    Ok(())
}

#[tokio::test]
async fn find_commit_with_no_base_never_reads_a_proposal_candidate() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let proposal = uuid!(50)?;
    let candidate = versions
        .create_candidate(
            &scope,
            proposal,
            CandidateChanges {
                mutation_id: uuid!(51)?,
                base: initial.clone(),
                author: person("agent"),
                message: "Propose a note".to_owned(),
                edits: vec![note(uuid!(10)?, "proposed.md", "Proposed.")?],
            },
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(51)?, None).await?),
        None,
        "the candidate carries the trailer but is not on the accepted line"
    );
    let accepted = versions
        .promote_candidate(
            &scope,
            Promotion {
                mutation_id: uuid!(52)?,
                proposal_id: proposal,
                expected_head: initial,
                candidate,
                approver: person("ana"),
                message: "Accept the note".to_owned(),
            },
        )
        .await?;
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(51)?, None).await?),
        None,
        "promotion writes a new commit; the candidate's own stays off the line"
    );
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(52)?, None).await?),
        Some(accepted.revision)
    );
    Ok(())
}

#[tokio::test]
async fn find_commit_with_no_base_finds_a_commit_of_any_age_back_to_the_root() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let day = 24 * 60 * 60;
    // A lease-expired mutation stays until it is reconciled (SPEC section 8), so its commit
    // may be far older than the seven-day retention of completed mutations.
    let ancient = commit_aged(directory.path(), &scope, 50, 400 * day)?;
    let expired = commit_aged(directory.path(), &scope, 53, 8 * day)?;
    let recent = commit_aged(directory.path(), &scope, 52, 6 * day)?;
    let head = versions
        .commit(
            &scope,
            changes(51, &recent, vec![note(uuid!(10)?, "a.md", "A.")?])?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(51)?, None).await?),
        Some(head)
    );
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(52)?, None).await?),
        Some(recent)
    );
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(53)?, None).await?),
        Some(expired),
        "eight days old is found: nothing bounds the walk by time"
    );
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(50)?, None).await?),
        Some(ancient),
        "four hundred days old is found"
    );
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(1)?, None).await?),
        Some(initial),
        "the walk reaches the root: the workspace's creation commit"
    );
    Ok(())
}

/// A merge commit on main whose second parent is a candidate: `None` follows first parents
/// only, so the candidate's trailer is never found, while the merge's own trailer and a commit
/// behind its first parent are. The store never writes a merge; this is written with `git2`.
/// (Reviewer probe, adopted.)
#[tokio::test]
async fn find_commit_with_no_base_never_follows_a_merge_into_a_candidate() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let candidate = versions
        .create_candidate(
            &scope,
            uuid!(50)?,
            CandidateChanges {
                mutation_id: uuid!(51)?,
                base: initial.clone(),
                author: person("agent"),
                message: "Propose".to_owned(),
                edits: vec![note(uuid!(10)?, "proposed.md", "P.")?],
            },
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let head = versions
        .commit(
            &scope,
            changes(30, &initial, vec![note(uuid!(11)?, "a.md", "A.")?])?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let repository = git2::Repository::open_bare(repository_path(directory.path(), &scope))?;
    let first = repository.find_commit(git2::Oid::from_str(head.as_str())?)?;
    let second = repository.find_commit(git2::Oid::from_str(candidate.as_str())?)?;
    let who = git2::Signature::now("raw", "raw@example.invalid")?;
    let merge = repository.commit(
        Some("refs/heads/main"),
        &who,
        &who,
        &format!("merge\n\nOkf-Jawn-Mutation: {}\n", uuid_text(52)),
        &first.tree()?,
        &[&first, &second],
    )?;
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(51)?, None).await?),
        None,
        "the first-parent walk never reaches the candidate"
    );
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(52)?, None).await?),
        Some(Revision::try_from(merge.to_string())?)
    );
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(30)?, None).await?),
        Some(head)
    );
    Ok(())
}

/// A note at `at` whose frontmatter holds `extra` properties besides its type, title and
/// header, so its body starts on a later line the more it holds.
fn note_with(item: ItemId, at: &str, body: &str, extra: usize) -> Fallible<TreeEdit> {
    Ok(TreeEdit::CreateItem {
        item_id: item,
        path: path(at)?,
        title: Some(format!("Note at {at}")),
        type_name: "Note".to_owned(),
        kind: ItemKind::Note,
        body: body.to_owned(),
        properties: (0..extra)
            .map(|key| (format!("key{key}"), json!(format!("value {key}"))))
            .collect(),
    })
}

/// The committed text of the file at `at` at `revision`.
async fn file_lines(
    versions: &okf_jawn_storage::GitVersions,
    scope: &StorageScope,
    revision: &Revision,
    at: &str,
) -> Fallible<Vec<String>> {
    let bytes = versions.read_file(scope, revision, &path(at)?).await?;
    Ok(String::from_utf8(bytes)?
        .lines()
        .map(str::to_owned)
        .collect())
}

/// The new file's line number a unified `patch` gives the added line `text`, read from its
/// hunk headers and the lines that follow them.
fn added_at(patch: &str, text: &str) -> Fallible<u32> {
    let mut new_line = 0_u32;
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            let to = some(header.split_whitespace().nth(1), "the to side")?;
            let first_line = some(to.trim_start_matches('+').split(',').next(), "a start")?;
            new_line = first_line.parse()?;
        } else if line.starts_with('+') && !line.starts_with("+++") {
            if line.strip_prefix('+') == Some(text) {
                return Ok(new_line);
            }
            new_line = new_line.saturating_add(1);
        } else if line.starts_with(' ') {
            new_line = new_line.saturating_add(1);
        }
    }
    Err(format!("the patch adds no line {text:?}: {patch}").into())
}

#[tokio::test]
async fn body_start_is_the_file_line_after_the_frontmatter_of_any_length() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let files = [
        (10, "short.md", 0),
        (11, "middle.md", 2),
        (12, "long.md", 7),
    ];
    let mut edits = Vec::new();
    for (item, at, extra) in files {
        edits.push(note_with(uuid!(item)?, at, "First.\nSecond.", extra)?);
    }
    let revision = versions
        .commit(
            &scope,
            changes(20, &initial, edits)?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let mut starts = Vec::new();
    for (item, at, _) in files {
        let start = versions.body_start(&scope, &revision, uuid!(item)?).await?;
        let lines = file_lines(&versions, &scope, &revision, at).await?;
        let index = usize::try_from(start)?.saturating_sub(1);
        assert_eq!(
            lines.get(index).map(String::as_str),
            Some("First."),
            "{at}: {lines:?}"
        );
        assert_eq!(
            lines.get(index.saturating_sub(1)).map(String::as_str),
            Some(""),
            "{at}: the blank line after the frontmatter"
        );
        assert_eq!(
            lines.get(index.saturating_sub(2)).map(String::as_str),
            Some("---"),
            "{at}: the closing delimiter"
        );
        starts.push(start);
    }
    assert!(
        starts.windows(2).all(|pair| pair.first() < pair.get(1)),
        "a longer frontmatter starts the body later: {starts:?}"
    );
    let unknown = err_of(versions.body_start(&scope, &revision, uuid!(99)?).await)?;
    assert_eq!(unknown.code, ErrorCode::NotFound, "{}", unknown.message);
    Ok(())
}

#[tokio::test]
async fn body_start_maps_the_patch_and_blame_lines_to_the_same_body_line() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let item: ItemId = uuid!(10)?;
    let first = versions
        .commit(
            &scope,
            changes(
                20,
                &initial,
                vec![note_with(
                    item,
                    "plan.md",
                    "One.\nTwo.\nThree.\nFour.\nFive.",
                    3,
                )?],
            )?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let document = versions.show(&scope, &first, item).await?;
    let second = versions
        .commit(
            &scope,
            changes(
                21,
                &first,
                vec![TreeEdit::EditItem {
                    item_id: item,
                    body: "One.\nTwo.\nThree.\nFour, edited.\nFive.".to_owned(),
                    properties: document.properties,
                }],
            )?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let start = versions.body_start(&scope, &second, item).await?;
    // The patch's file line of the edited line, mapped with `body_start`, is body line 4.
    let diff = versions
        .diff(
            &scope,
            okf_jawn_core::storage::DiffQuery {
                from: first.clone(),
                to: second.clone(),
                item_id: Some(item),
            },
        )
        .await?;
    let patch = &some(diff.changes.first(), "the item's change")?.patch;
    let edited = added_at(patch, "Four, edited.")?;
    assert_eq!(edited.saturating_sub(start).saturating_add(1), 4, "{patch}");
    // Blame of each body line is libgit2's blame of the file line `body_start` maps it to.
    let blame = versions
        .blame(
            &scope,
            okf_jawn_core::storage::BlameQuery {
                revision: second.clone(),
                item_id: item,
                lines: okf_jawn_contract::common::TextRange { start: 1, end: 5 },
            },
        )
        .await?;
    let repository = git2::Repository::open_bare(repository_path(directory.path(), &scope))?;
    let mut options = git2::BlameOptions::new();
    options.newest_commit(git2::Oid::from_str(second.as_str())?);
    let raw = repository.blame_file(Path::new("plan.md"), Some(&mut options))?;
    for blamed in &blame.lines {
        let file_line = usize::try_from(blamed.line.saturating_add(start).saturating_sub(1))?;
        let hunk = some(raw.get_line(file_line), "the file line's blame")?;
        assert_eq!(
            blamed.commit.revision.as_str(),
            hunk.final_commit_id().to_string(),
            "body line {}",
            blamed.line
        );
    }
    let attributed: Vec<_> = blame
        .lines
        .iter()
        .map(|line| (line.line, line.commit.revision.clone()))
        .collect();
    assert_eq!(
        attributed,
        vec![
            (1, first.clone()),
            (2, first.clone()),
            (3, first.clone()),
            (4, second),
            (5, first),
        ]
    );
    Ok(())
}

/// Commit `bytes` at `at` on main straight into the workspace repository, as no store edit
/// can (an archive restore brings such files), and return the commit.
fn raw_write(
    directory: &Path,
    scope: &StorageScope,
    at: &str,
    bytes: &[u8],
    mutation: u32,
) -> Fallible<Revision> {
    let repository = git2::Repository::open_bare(repository_path(directory, scope))?;
    let parent = repository.find_commit(repository.refname_to_id("refs/heads/main")?)?;
    let blob = repository.blob(bytes)?;
    let mut update = git2::build::TreeUpdateBuilder::new();
    update.upsert(at, blob, git2::FileMode::Blob);
    let tree = repository.find_tree(update.create_updated(&repository, &parent.tree()?)?)?;
    let who = git2::Signature::now("raw", "raw@example.invalid")?;
    let commit = repository.commit(
        Some("refs/heads/main"),
        &who,
        &who,
        &format!("raw\n\nOkf-Jawn-Mutation: {}\n", uuid_text(mutation)),
        &tree,
        &[&parent],
    )?;
    Ok(Revision::try_from(commit.to_string())?)
}

/// Write a note through the store, then store its file again in `shape` and then once more in
/// `shape` with body line 2 edited, both by raw commits. `body_start` must name the stored
/// file's line holding the first body line, and blame of body line N must be libgit2's blame
/// of the stored line `start + N - 1`: line 2 from the edit, lines 1 and 3 from earlier.
async fn stored_shape_keeps_body_lines(shape: Shape) -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let item: ItemId = uuid!(10)?;
    let written = versions
        .commit(
            &scope,
            changes(
                20,
                &initial,
                vec![note_with(item, "plan.md", "One.\nTwo.\nThree.", 2)?],
            )?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let canonical = String::from_utf8(
        versions
            .read_file(&scope, &written, &path("plan.md")?)
            .await?,
    )?;
    let reshaped = shape(&canonical);
    assert_ne!(reshaped, canonical, "the shape changes the stored bytes");
    raw_write(directory.path(), &scope, "plan.md", reshaped.as_bytes(), 21)?;
    let edited = shape(&canonical.replacen("\nTwo.\n", "\nTwo, edited.\n", 1));
    let tip = raw_write(directory.path(), &scope, "plan.md", edited.as_bytes(), 22)?;
    let start = versions.body_start(&scope, &tip, item).await?;
    let stored: Vec<&str> = edited.lines().collect();
    let first = usize::try_from(start)?.saturating_sub(1);
    assert_eq!(
        stored.get(first).copied(),
        Some("One."),
        "body_start {start} names the first body line: {edited:?}"
    );
    let blame = versions
        .blame(
            &scope,
            okf_jawn_core::storage::BlameQuery {
                revision: tip.clone(),
                item_id: item,
                lines: okf_jawn_contract::common::TextRange { start: 1, end: 3 },
            },
        )
        .await?;
    let repository = git2::Repository::open_bare(repository_path(directory.path(), &scope))?;
    let mut options = git2::BlameOptions::new();
    options.newest_commit(git2::Oid::from_str(tip.as_str())?);
    let raw = repository.blame_file(Path::new("plan.md"), Some(&mut options))?;
    let texts: Vec<&str> = blame.lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(texts, vec!["One.", "Two, edited.", "Three."]);
    for blamed in &blame.lines {
        let file_line = usize::try_from(blamed.line.saturating_add(start).saturating_sub(1))?;
        let hunk = some(raw.get_line(file_line), "the stored line's blame")?;
        assert_eq!(
            blamed.commit.revision.as_str(),
            hunk.final_commit_id().to_string(),
            "body line {} is stored line {file_line}",
            blamed.line
        );
        assert_eq!(
            blamed.commit.revision == tip,
            blamed.line == 2,
            "only body line 2 comes from the edit"
        );
    }
    Ok(())
}

#[tokio::test]
async fn body_start_and_blame_count_a_stored_file_with_no_blank_line_after_its_frontmatter()
-> TestResult {
    stored_shape_keeps_body_lines(|text| text.replacen("\n---\n\n", CLOSING, 1)).await
}

#[tokio::test]
async fn body_start_and_blame_count_a_stored_file_with_a_comment_in_its_frontmatter() -> TestResult
{
    stored_shape_keeps_body_lines(|text| {
        text.replacen("---\n", "---\n# a comment the serializer drops\n", 1)
    })
    .await
}

#[tokio::test]
async fn body_start_and_blame_count_a_stored_file_with_crlf_line_endings() -> TestResult {
    stored_shape_keeps_body_lines(|text| text.replace('\n', "\r\n")).await
}

/// An empty body: `body_start` names the line where the body would begin (the line after the
/// closing delimiter), and blame has no body line to attribute.
#[tokio::test]
async fn body_start_of_a_stored_file_with_an_empty_body_is_the_line_after_its_frontmatter()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let item: ItemId = uuid!(10)?;
    let written = versions
        .commit(
            &scope,
            changes(20, &initial, vec![note_with(item, "plan.md", "One.", 2)?])?,
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    let canonical = String::from_utf8(
        versions
            .read_file(&scope, &written, &path("plan.md")?)
            .await?,
    )?;
    let closing = some(canonical.find(CLOSING), "the closing delimiter")?;
    let frontmatter_only = some(
        canonical.get(..closing.saturating_add(CLOSING.len())),
        "the frontmatter",
    )?;
    let tip = raw_write(
        directory.path(),
        &scope,
        "plan.md",
        frontmatter_only.as_bytes(),
        21,
    )?;
    assert_eq!(versions.show(&scope, &tip, item).await?.body, "");
    let start = versions.body_start(&scope, &tip, item).await?;
    let stored_lines = u32::try_from(frontmatter_only.lines().count())?;
    assert_eq!(
        start,
        stored_lines.saturating_add(1),
        "{frontmatter_only:?}"
    );
    let error = err_of(
        versions
            .blame(
                &scope,
                okf_jawn_core::storage::BlameQuery {
                    revision: tip,
                    item_id: item,
                    lines: okf_jawn_contract::common::TextRange { start: 1, end: 1 },
                },
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::InvalidInput, "{}", error.message);
    Ok(())
}

/// The revision of the commit `find_commit` answered, when it found one.
fn found(answer: Option<Committed>) -> Option<Revision> {
    answer.map(|found| found.revision)
}

/// A warning as a check would give it, about the file at `at`.
fn finding(code: &str, at: &str) -> Warning {
    Warning {
        code: code.to_owned(),
        message: format!("{at} could say more"),
        location: Some(at.to_owned()),
    }
}

/// `warnings` as the wire carries them: `Warning` has no equality.
fn wire(warnings: &[Warning]) -> Fallible<serde_json::Value> {
    Ok(serde_json::to_value(warnings)?)
}

/// The warnings in the row storage keeps for `mutation`'s commit in `scope`, when it keeps one.
fn kept_row(
    storage: &Storage,
    scope: &StorageScope,
    mutation: u32,
) -> Fallible<Option<serde_json::Value>> {
    let connection = rusqlite::Connection::open(storage.data().records_path())?;
    let mut statement = connection.prepare(
        "SELECT warnings FROM commit_warnings
         WHERE tenant_id = ?1 AND workspace_id = ?2 AND mutation_id = ?3",
    )?;
    let mut rows = statement.query_map(
        [
            scope.tenant_id.as_str().to_owned(),
            scope.workspace_id.0.to_string(),
            uuid_text(mutation),
        ],
        |row| row.get::<_, String>(0),
    )?;
    let text = rows.next().transpose()?;
    Ok(text.map(|text| serde_json::from_str(&text)).transpose()?)
}

/// A second workspace of `scope`'s tenant, created under mutation 2, and its head.
async fn second_workspace(
    storage: &Storage,
    scope: &StorageScope,
) -> Fallible<(StorageScope, Revision)> {
    let created = storage
        .catalog()
        .create(
            &scope.tenant_id,
            uuid!(2)?,
            NewWorkspace {
                name: "Other".to_owned(),
                description: String::new(),
                creator: person("ana"),
            },
        )
        .await?;
    Ok((
        StorageScope {
            tenant_id: scope.tenant_id.clone(),
            workspace_id: created.id,
        },
        created.head,
    ))
}

/// R-I10: a replay answers the warnings the first attempt's check gave, whichever call replays
/// it, and runs no check; and a commit whose check warned of nothing replays with no warning,
/// not with the unavailable one.
#[tokio::test]
async fn a_replayed_commit_answers_the_warnings_its_first_attempt_was_given() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let given = vec![finding("lint_a", "a.md"), finding("lint_b", "a.md")];
    let edits = || -> Fallible<_> { Ok(vec![note(uuid!(10)?, "a.md", "A")?]) };
    let first = versions
        .commit(
            &scope,
            changes(20, &initial, edits()?)?,
            Arc::new(Warns::of(given.clone())),
        )
        .await?;
    assert!(!first.replayed);
    assert_eq!(wire(&first.warnings)?, wire(&given)?);
    // The retry's own check would warn of something else, and never runs.
    let retry = Arc::new(Warns::of(vec![finding("other", "a.md")]));
    let again = versions
        .commit(&scope, changes(20, &initial, edits()?)?, retry.clone())
        .await?;
    assert!(again.replayed);
    assert_eq!(again.revision, first.revision);
    assert_eq!(
        retry.runs.load(Ordering::SeqCst),
        0,
        "a replay runs no check"
    );
    assert_eq!(wire(&again.warnings)?, wire(&given)?);
    for since in [Some(&initial), None] {
        let answer = versions.find_commit(&scope, uuid!(20)?, since).await?;
        let answer = some(answer, "the commit by its mutation")?;
        assert!(answer.replayed, "a found commit is answered as a replay");
        assert_eq!(answer.revision, first.revision);
        assert_eq!(wire(&answer.warnings)?, wire(&given)?);
    }
    // No warning is an answer too: it replays as no warning.
    let quiet =
        || -> Fallible<_> { changes(21, &first.revision, vec![note(uuid!(11)?, "b.md", "B")?]) };
    let silent = versions
        .commit(&scope, quiet()?, Arc::new(Counting::default()))
        .await?;
    assert!(silent.warnings.is_empty());
    let silent_again = versions
        .commit(&scope, quiet()?, Arc::new(Counting::default()))
        .await?;
    assert!(silent_again.replayed);
    assert_eq!(wire(&silent_again.warnings)?, wire(&[])?);
    Ok(())
}

/// R-I10: the warnings are kept by workspace as well as by mutation. Two workspaces each hold a
/// commit under the same mutation identity, and each replays with its own.
#[tokio::test]
async fn kept_warnings_belong_to_the_workspace_of_their_commit() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let (other, other_head) = second_workspace(&storage, &scope).await?;
    let versions = storage.versions();
    let ours = vec![finding("lint_a", "a.md")];
    let theirs = vec![finding("lint_c", "a.md")];
    for (workspace, head, given) in [(&scope, &initial, &ours), (&other, &other_head, &theirs)] {
        versions
            .commit(
                workspace,
                changes(20, head, vec![note(uuid!(10)?, "a.md", "A")?])?,
                Arc::new(Warns::of(given.clone())),
            )
            .await?;
    }
    for (workspace, expected) in [(&scope, &ours), (&other, &theirs)] {
        let answer = versions.find_commit(workspace, uuid!(20)?, None).await?;
        let answer = some(answer, "the commit of each workspace")?;
        assert_eq!(wire(&answer.warnings)?, wire(expected)?);
    }
    Ok(())
}

/// R-I10, a stop after the warnings were kept and before the commit landed: the row stands
/// without a commit, and the next attempt replaces it and commits. The check moves the head as a
/// concurrent writer would, so the staged commit is checked, its warnings are kept, and its
/// landing is refused.
#[tokio::test]
async fn warnings_kept_for_a_commit_that_did_not_land_are_replaced_by_the_next_attempt()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let stale = vec![finding("stale", "a.md")];
    let stranding = Arc::new(WarnsAsTheHeadMoves {
        interloper: Interloper {
            repository: repository_path(directory.path(), &scope),
            moved_to: std::sync::Mutex::new(None),
        },
        warnings: stale.clone(),
    });
    let edits = || -> Fallible<_> { Ok(vec![note(uuid!(10)?, "a.md", "A")?]) };
    let refused = err_of(
        versions
            .commit(&scope, changes(20, &initial, edits()?)?, stranding)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict, "{}", refused.message);
    let moved = versions.head(&scope).await?;
    assert_ne!(moved, initial, "the head moved under the commit");
    // Kept before the landing was tried: the row is there, and no commit carries the mutation.
    assert_eq!(
        kept_row(&storage, &scope, 20)?,
        Some(wire(&stale)?),
        "the warnings were kept before the commit could land"
    );
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(20)?, None).await?),
        None,
        "no commit landed"
    );
    // The next attempt checks again, on the head as it is now, and its warnings replace the row.
    let fresh = vec![finding("fresh", "a.md")];
    let landed = versions
        .commit(
            &scope,
            changes(20, &moved, edits()?)?,
            Arc::new(Warns::of(fresh.clone())),
        )
        .await?;
    assert!(!landed.replayed);
    assert_eq!(wire(&landed.warnings)?, wire(&fresh)?);
    assert_eq!(kept_row(&storage, &scope, 20)?, Some(wire(&fresh)?));
    let again = versions
        .commit(
            &scope,
            changes(20, &moved, edits()?)?,
            Arc::new(Counting::default()),
        )
        .await?;
    assert!(again.replayed);
    assert_eq!(again.revision, landed.revision);
    assert_eq!(wire(&again.warnings)?, wire(&fresh)?);
    Ok(())
}

/// Assert that `replay` is a replay answered with the one warning that the commit's warnings
/// are not available, whose message holds `why`.
fn assert_unavailable(replay: &Committed, why: &str) -> TestResult {
    assert!(replay.replayed);
    let [only] = replay.warnings.as_slice() else {
        return Err(format!("not one warning: {:?}", replay.warnings).into());
    };
    assert_eq!(only.code, REPLAY_WARNINGS_UNAVAILABLE);
    assert_eq!(only.code, "replay_warnings_unavailable");
    assert_eq!(only.location, None);
    assert!(only.message.contains(why), "{}", only.message);
    Ok(())
}

/// R-I10, a commit written before storage kept warnings: the data directory is put back as
/// records schema 2 left it and opened again, which migrates it. The commit is still there and
/// has no row, so a replay of it, by `commit` or by `find_commit`, answers the one warning
/// `replay_warnings_unavailable` and succeeds. So does a commit no check ran for (the
/// workspace's first), and a row that does not read as warnings.
#[tokio::test]
async fn a_replay_of_a_commit_without_kept_warnings_answers_the_one_unavailable_warning()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let edits = || -> Fallible<_> { Ok(vec![note(uuid!(10)?, "a.md", "A")?]) };
    let first = storage
        .versions()
        .commit(
            &scope,
            changes(20, &initial, edits()?)?,
            Arc::new(Warns::of(vec![finding("lint_a", "a.md")])),
        )
        .await?;
    let records = storage.data().records_path();
    drop(storage);
    rusqlite::Connection::open(&records)?.execute_batch(
        "DROP TABLE commit_warnings; ALTER TABLE jobs DROP COLUMN sealed;
             PRAGMA user_version = 2;",
    )?;
    let storage = Storage::open(directory.path())?;
    let versions = storage.versions();
    assert_eq!(
        kept_row(&storage, &scope, 20)?,
        None,
        "migrated, and no row"
    );
    let retry = Arc::new(Counting::default());
    let again = versions
        .commit(&scope, changes(20, &initial, edits()?)?, retry.clone())
        .await?;
    assert_eq!(again.revision, first.revision);
    assert_eq!(
        retry.runs.load(Ordering::SeqCst),
        0,
        "a replay runs no check"
    );
    assert_unavailable(&again, "kept none")?;
    let answer = versions.find_commit(&scope, uuid!(20)?, None).await?;
    assert_unavailable(&some(answer, "the commit")?, "kept none")?;
    // The workspace's first commit (its creation, mutation 1): no check ever ran for it.
    let created = versions.find_commit(&scope, uuid!(1)?, None).await?;
    let created = some(created, "the creation commit")?;
    assert_eq!(created.revision, initial);
    assert_unavailable(&created, "kept none")?;
    // A row that is not a list of warnings is no reason to fail a commit that landed.
    rusqlite::Connection::open(&records)?.execute(
        "INSERT INTO commit_warnings (tenant_id, workspace_id, mutation_id, commit_id, warnings)
         VALUES (?1, ?2, ?3, ?4, '{\"not\": \"a list\"}')",
        [
            scope.tenant_id.as_str().to_owned(),
            scope.workspace_id.0.to_string(),
            uuid_text(20),
            first.revision.as_str().to_owned(),
        ],
    )?;
    let unread = versions
        .commit(
            &scope,
            changes(20, &initial, edits()?)?,
            Arc::new(Counting::default()),
        )
        .await?;
    assert_eq!(unread.revision, first.revision);
    assert_unavailable(&unread, "could not be read")?;
    Ok(())
}

/// The commit the row storage keeps for `mutation` in `scope` names, when it keeps one.
fn kept_commit(storage: &Storage, scope: &StorageScope, mutation: u32) -> Fallible<Option<String>> {
    let connection = rusqlite::Connection::open(storage.data().records_path())?;
    let mut statement = connection.prepare(
        "SELECT commit_id FROM commit_warnings
         WHERE tenant_id = ?1 AND workspace_id = ?2 AND mutation_id = ?3",
    )?;
    let mut rows = statement.query_map(
        [
            scope.tenant_id.as_str().to_owned(),
            scope.workspace_id.0.to_string(),
            uuid_text(mutation),
        ],
        |row| row.get::<_, String>(0),
    )?;
    Ok(rows.next().transpose()?)
}

/// Review N1: a replay answers the warnings its commit was given, or says they are unavailable;
/// never another attempt's. A later attempt of a mutation misses the mutation's own commit (its
/// expected head is that commit), passes its check, keeps its warnings and then cannot land:
/// the row of the mutation is now that attempt's and names its commit. The one commit that
/// carries the mutation is the first, and it is answered with the one unavailable warning by
/// `find_commit` and by `commit`, not with the second attempt's warnings. (Reviewer probe PV6,
/// adopted with the ruled expectation.)
#[tokio::test]
async fn a_later_attempt_that_does_not_land_never_lends_its_warnings_to_the_landed_commit()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let first_given = vec![finding("first", "a.md")];
    let first_edits = || -> Fallible<_> { Ok(vec![note(uuid!(10)?, "a.md", "A")?]) };
    let first = versions
        .commit(
            &scope,
            changes(20, &initial, first_edits()?)?,
            Arc::new(Warns::of(first_given.clone())),
        )
        .await?;
    assert_eq!(
        kept_commit(&storage, &scope, 20)?.as_deref(),
        Some(first.revision.as_str()),
        "the row names the commit it was kept for"
    );
    let second_given = vec![finding("second", "b.md")];
    let stranding = Arc::new(WarnsAsTheHeadMoves {
        interloper: Interloper {
            repository: repository_path(directory.path(), &scope),
            moved_to: std::sync::Mutex::new(None),
        },
        warnings: second_given.clone(),
    });
    let refused = err_of(
        versions
            .commit(
                &scope,
                changes(20, &first.revision, vec![note(uuid!(11)?, "b.md", "B")?])?,
                stranding.clone(),
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict, "{}", refused.message);
    // The second attempt landed nothing: the head is the concurrent writer's commit, and the
    // row is the second attempt's, kept for a commit that is not on the line.
    let interloper = some(
        *stranding
            .interloper
            .moved_to
            .lock()
            .map_err(|_| "poisoned")?,
        "the concurrent commit",
    )?;
    assert_eq!(
        versions.head(&scope).await?.as_str(),
        interloper.to_string()
    );
    assert_eq!(kept_row(&storage, &scope, 20)?, Some(wire(&second_given)?));
    let stranded = some(kept_commit(&storage, &scope, 20)?, "the row's commit")?;
    assert_ne!(stranded, first.revision.as_str());
    assert_ne!(stranded, interloper.to_string());
    // The commit that carries the mutation is still the first, and it is not answered with
    // the second attempt's warnings.
    for since in [Some(&initial), None] {
        let answer = versions.find_commit(&scope, uuid!(20)?, since).await?;
        let answer = some(answer, "the first commit")?;
        assert_eq!(answer.revision, first.revision, "the first commit is found");
        assert_unavailable(&answer, "another attempt")?;
    }
    let retry = Arc::new(Counting::default());
    let again = versions
        .commit(
            &scope,
            changes(20, &initial, first_edits()?)?,
            retry.clone(),
        )
        .await?;
    assert_eq!(again.revision, first.revision);
    assert_eq!(
        retry.runs.load(Ordering::SeqCst),
        0,
        "a replay runs no check"
    );
    assert_unavailable(&again, "another attempt")?;
    Ok(())
}

/// Review N1, the rule by itself: the row of a mutation is answered only to the commit it
/// names. The row is pointed at another commit of the workspace (its first) behind the store,
/// and the commit that carries the mutation is then answered with the unavailable warning;
/// pointed back, it is answered with its warnings again.
#[tokio::test]
async fn a_row_kept_for_another_commit_is_not_answered() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let given = vec![finding("lint_a", "a.md")];
    let first = versions
        .commit(
            &scope,
            changes(20, &initial, vec![note(uuid!(10)?, "a.md", "A")?])?,
            Arc::new(Warns::of(given.clone())),
        )
        .await?;
    let point_at = |commit: &Revision| -> Fallible<usize> {
        Ok(
            rusqlite::Connection::open(storage.data().records_path())?.execute(
                "UPDATE commit_warnings SET commit_id = ?1 WHERE mutation_id = ?2",
                [commit.as_str().to_owned(), uuid_text(20)],
            )?,
        )
    };
    assert_eq!(point_at(&initial)?, 1, "the row now names another commit");
    let answer = versions.find_commit(&scope, uuid!(20)?, None).await?;
    let answer = some(answer, "the commit")?;
    assert_eq!(answer.revision, first.revision);
    assert_unavailable(&answer, "another attempt")?;
    assert_eq!(point_at(&first.revision)?, 1);
    let answer = versions.find_commit(&scope, uuid!(20)?, None).await?;
    assert_eq!(wire(&some(answer, "the commit")?.warnings)?, wire(&given)?);
    Ok(())
}

/// R-I10: the row is read by its tenant as well as its workspace and mutation. The commit's
/// own row is moved to another tenant behind the store (same workspace identity, same
/// mutation), so this tenant holds none, and the commit is answered as having none kept.
/// (Reviewer probe PV1, adopted.)
#[tokio::test]
async fn a_row_of_another_tenant_is_not_answered() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    versions
        .commit(
            &scope,
            changes(20, &initial, vec![note(uuid!(10)?, "a.md", "A")?])?,
            Arc::new(Warns::of(vec![finding("lint_a", "a.md")])),
        )
        .await?;
    let moved = rusqlite::Connection::open(storage.data().records_path())?.execute(
        "UPDATE commit_warnings SET tenant_id = 'elsewhere' WHERE mutation_id = ?1",
        [uuid_text(20)],
    )?;
    assert_eq!(moved, 1, "the row now belongs to another tenant");
    let answer = versions.find_commit(&scope, uuid!(20)?, None).await?;
    assert_unavailable(&some(answer, "the commit")?, "kept none")?;
    Ok(())
}

/// R-I10: the row is written before the commit lands, so a row that cannot be written stops
/// the commit. The table is dropped behind the store: the commit answers `Internal`, and the
/// head does not move. (Reviewer probe PV2, adopted.)
#[tokio::test]
async fn a_commit_whose_warnings_cannot_be_kept_does_not_land() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    rusqlite::Connection::open(storage.data().records_path())?
        .execute_batch("DROP TABLE commit_warnings;")?;
    let refused = err_of(
        versions
            .commit(
                &scope,
                changes(20, &initial, vec![note(uuid!(10)?, "a.md", "A")?])?,
                Arc::new(Warns::of(vec![finding("lint_a", "a.md")])),
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Internal, "{}", refused.message);
    assert_eq!(
        versions.head(&scope).await?,
        initial,
        "the head did not move"
    );
    assert_eq!(
        found(versions.find_commit(&scope, uuid!(20)?, None).await?),
        None,
        "no commit carries the mutation"
    );
    Ok(())
}

/// R-I10: the same for a promotion, whose empty answer is kept before its reference moves.
/// (Reviewer probe PV3, adopted.)
#[tokio::test]
async fn a_promotion_whose_answer_cannot_be_kept_does_not_land() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let versions = storage.versions();
    let proposal = uuid!(50)?;
    let candidate = versions
        .create_candidate(
            &scope,
            proposal,
            CandidateChanges {
                mutation_id: uuid!(51)?,
                base: initial.clone(),
                author: person("agent"),
                message: "Propose a note".to_owned(),
                edits: vec![note(uuid!(10)?, "proposed.md", "Proposed.")?],
            },
            Arc::new(Counting::default()),
        )
        .await?
        .revision;
    rusqlite::Connection::open(storage.data().records_path())?
        .execute_batch("DROP TABLE commit_warnings;")?;
    let refused = err_of(
        versions
            .promote_candidate(
                &scope,
                Promotion {
                    mutation_id: uuid!(52)?,
                    proposal_id: proposal,
                    expected_head: initial.clone(),
                    candidate,
                    approver: person("ana"),
                    message: "Accept the note".to_owned(),
                },
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Internal, "{}", refused.message);
    assert_eq!(
        versions.head(&scope).await?,
        initial,
        "the head did not move"
    );
    Ok(())
}

/// R-I10: two attempts of one mutation at once, each with a check that would warn of something
/// else, 60 rounds. One writes, the other replays it, both answer the writer's warnings, and
/// the row holds them. The two calls are serial under the workspace's write lock, so this
/// shows that their answers agree whichever goes first; it does not interleave them.
/// (Reviewer probe PV4, adopted.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_attempts_of_one_mutation_at_once_answer_one_commit_and_its_warnings() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, initial) = workspace(directory.path()).await?;
    let mut head = initial;
    for round in 0..60_u32 {
        let mutation = 1000_u32.saturating_add(round);
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let mut calls = Vec::new();
        for code in ["first", "second"] {
            let versions = storage.versions();
            let (scope, barrier) = (scope.clone(), barrier.clone());
            let given = vec![finding(code, "r.md")];
            let item: ItemId = uuid!(2000_u32.saturating_add(round))?;
            let change = changes(
                mutation,
                &head,
                vec![note(item, &format!("r{round}.md"), "R")?],
            )?;
            calls.push(tokio::spawn(async move {
                barrier.wait().await;
                versions
                    .commit(&scope, change, Arc::new(Warns::of(given)))
                    .await
            }));
        }
        let mut answers = Vec::new();
        for call in calls {
            answers.push(call.await??);
        }
        let writers: Vec<&Committed> = answers.iter().filter(|answer| !answer.replayed).collect();
        assert_eq!(writers.len(), 1, "round {round}: one attempt wrote");
        let writer = some(writers.first().copied(), "the writer")?;
        let first = some(answers.first(), "an answer")?;
        let second = some(answers.get(1), "an answer")?;
        assert_eq!(first.revision, second.revision, "round {round}: one commit");
        assert_eq!(
            wire(&first.warnings)?,
            wire(&second.warnings)?,
            "round {round}: both attempts answer the same warnings"
        );
        assert_eq!(
            kept_row(&storage, &scope, mutation)?,
            Some(wire(&writer.warnings)?),
            "round {round}: the row is the writer's"
        );
        assert_eq!(
            kept_commit(&storage, &scope, mutation)?.as_deref(),
            Some(writer.revision.as_str()),
            "round {round}: and names the writer's commit"
        );
        head = first.revision.clone();
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
