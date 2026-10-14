//! Workspace paths against real Git repositories, on the filesystem the tests run on: what a
//! note's file is, a rename that changes only letter case, and a folder spelled two ways.
//!
//! Windows and macOS treat `Report.md` and `report.md` as one file; Linux and Git treat them as
//! two. Storage edits a staged copy of the tree on the disk it runs on, so every assertion here
//! holds on both kinds of filesystem: the committed tree read straight from Git, the bytes, the
//! links and the refusals. A tree no edit can build (a note file ending in `.MD`, two spellings
//! of one folder) is committed straight through `git2`, as a case-sensitive filesystem or an
//! earlier build could have left it.
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use git2::Repository;
use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::history::FileChangeKind;
use okf_jawn_contract::identity::{ItemId, Revision, TenantId, WorkspacePath};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_core::search::{GraphQuery, SearchIndex, SearchQuery};
use okf_jawn_core::storage::{
    CandidateCheck, CommitChanges, Committed, DiffQuery, LogQuery, NewWorkspace, Page, Provenance,
    SourceCard, StorageScope, TreeEdit, VersionStore, WorkspaceCatalog,
};
use okf_jawn_storage::Storage;
use serde_json::json;

use check::{TestResult, err_of, some};

/// The identity numbered `n`, of the type the context names.
macro_rules! uuid {
    ($n:expr) => {
        serde_json::from_value(json!(uuid_text($n)))
    };
}

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// Accepts every candidate.
struct Accept;

/// A check that records the rewritten paths each run was told (`CandidateCheck::check`) and
/// accepts.
#[derive(Default)]
struct Recording {
    told: Mutex<Vec<Vec<String>>>,
}

/// A data directory with one blank workspace.
struct Workspace {
    directory: tempfile::TempDir,
    storage: Storage,
    scope: StorageScope,
    /// The catalog's first commit.
    initial: Revision,
}

/// The private name a case-only rename parks its item under, in any folder.
const PARKED: &str = "okf-jawn-recase-";

impl CandidateCheck for Accept {
    fn check(&self, _root: &Path, _rewritten: &[WorkspacePath]) -> Result<Vec<Warning>, ApiError> {
        Ok(Vec::new())
    }
}

impl Recording {
    /// What each run of the check was told the store rewrote, in run order.
    fn seen(&self) -> Fallible<Vec<Vec<String>>> {
        Ok(self.told.lock().map_err(|_| "poisoned")?.clone())
    }
}

impl CandidateCheck for Recording {
    fn check(&self, _root: &Path, rewritten: &[WorkspacePath]) -> Result<Vec<Warning>, ApiError> {
        self.told
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))?
            .push(
                rewritten
                    .iter()
                    .map(|path| path.as_str().to_owned())
                    .collect(),
            );
        Ok(Vec::new())
    }
}

impl Workspace {
    async fn new() -> Fallible<Self> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(directory.path())?;
        let tenant = TenantId::try_from("local".to_owned())?;
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
        Ok(Self {
            directory,
            storage,
            scope: StorageScope {
                tenant_id: tenant,
                workspace_id: created.id,
            },
            initial: created.head,
        })
    }

    /// Commit `edits` as mutation `mutation` on top of `expected`, accepting every candidate.
    async fn commit(
        &self,
        mutation: u32,
        expected: &Revision,
        edits: Vec<TreeEdit>,
    ) -> Result<Revision, ApiError> {
        Ok(self
            .commit_with(mutation, expected, edits, Arc::new(Accept))
            .await?
            .revision)
    }

    /// Commit `edits` as mutation `mutation` on top of `expected` under `check`.
    async fn commit_with(
        &self,
        mutation: u32,
        expected: &Revision,
        edits: Vec<TreeEdit>,
        check: Arc<dyn CandidateCheck>,
    ) -> Result<Committed, ApiError> {
        let changes = CommitChanges {
            mutation_id: serde_json::from_value(json!(uuid_text(mutation)))
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?,
            expected_head: expected.clone(),
            author: person("ana"),
            message: format!("change {mutation}"),
            edits,
        };
        self.storage
            .versions()
            .commit(&self.scope, changes, check)
            .await
    }

    /// The refusal of a commit of `edits` on the head `head`, which stays the head.
    async fn refused(
        &self,
        mutation: u32,
        head: &Revision,
        edits: Vec<TreeEdit>,
    ) -> Fallible<ApiError> {
        let error = err_of(self.commit(mutation, head, edits).await)?;
        assert_eq!(
            self.storage.versions().head(&self.scope).await?,
            *head,
            "a refusal commits nothing: {error}"
        );
        Ok(error)
    }

    /// The workspace's bare repository.
    fn repository(&self) -> Fallible<Repository> {
        Ok(Repository::open_bare(
            self.directory
                .path()
                .join("repositories")
                .join(self.scope.tenant_id.as_str())
                .join(format!("{}.git", self.scope.workspace_id.0)),
        )?)
    }

    /// Every file path of `revision`, read straight from Git.
    fn files(&self, revision: &Revision) -> Fallible<Vec<String>> {
        let repository = self.repository()?;
        let tree = repository
            .find_commit(git2::Oid::from_str(revision.as_str())?)?
            .tree()?;
        let mut found = Vec::new();
        tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
            if entry.kind() == Some(git2::ObjectType::Blob)
                && let Ok(name) = entry.name()
            {
                found.push(format!("{root}{name}"));
            }
            git2::TreeWalkResult::Ok
        })?;
        Ok(found)
    }

    /// The bytes of the file at `at` in `revision`, read straight from Git.
    fn file(&self, revision: &Revision, at: &str) -> Fallible<Option<Vec<u8>>> {
        let repository = self.repository()?;
        let tree = repository
            .find_commit(git2::Oid::from_str(revision.as_str())?)?
            .tree()?;
        match tree.get_path(Path::new(at)) {
            Ok(entry) => Ok(Some(repository.find_blob(entry.id())?.content().to_vec())),
            Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Commit on main straight through `git2`, as no store edit can: remove each of `removed`,
    /// then write each of `written`. Git trees are case-sensitive on every system.
    fn raw(
        &self,
        mutation: u32,
        removed: &[&str],
        written: &[(&str, Vec<u8>)],
    ) -> Fallible<Revision> {
        let repository = self.repository()?;
        let parent = repository.find_commit(repository.refname_to_id("refs/heads/main")?)?;
        let mut update = git2::build::TreeUpdateBuilder::new();
        for at in removed {
            update.remove(at);
        }
        for (at, bytes) in written {
            update.upsert(at, repository.blob(bytes)?, git2::FileMode::Blob);
        }
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

    /// The identity of the item `revision` holds at `at`.
    async fn item_at(&self, revision: &Revision, at: &str) -> Fallible<Option<ItemId>> {
        Ok(self
            .storage
            .versions()
            .item_at_path(&self.scope, revision, &path(at)?)
            .await?
            .map(|summary| summary.id))
    }

    /// The body of item `item` at `revision`.
    async fn body(&self, revision: &Revision, item: u32) -> Fallible<String> {
        Ok(self
            .storage
            .versions()
            .show(&self.scope, revision, uuid!(item)?)
            .await?
            .body)
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

fn note(item: u32, at: &str, body: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::CreateItem {
        item_id: uuid!(item)?,
        path: path(at)?,
        title: Some(format!("Note {item}")),
        type_name: "Note".to_owned(),
        kind: ItemKind::Note,
        body: body.to_owned(),
        properties: BTreeMap::new(),
    })
}

fn moved(item: u32, to: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::MoveItem {
        item_id: uuid!(item)?,
        destination: path(to)?,
    })
}

fn restore(item: u32, from: &Revision) -> Fallible<TreeEdit> {
    Ok(TreeEdit::RestoreItem {
        item_id: uuid!(item)?,
        from: from.clone(),
    })
}

/// A source card at `at`.
fn card(item: u32, at: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::WriteSourceCard(Box::new(SourceCard {
        item_id: uuid!(item)?,
        path: path(at)?,
        title: "scan.pdf".to_owned(),
        type_name: "Source".to_owned(),
        body: "Converted.".to_owned(),
        properties: BTreeMap::new(),
        appearance: serde_json::from_value(json!({
            "object": "d".repeat(64),
            "names": [{
                "filename": "scan.pdf",
                "folder": "sources",
                "observed_at": "2026-10-09T12:00:00.000Z",
                "supplied_by": "ana"
            }],
            "media_type": "application/pdf",
            "size": "10",
            "metadata": {},
            "extraction": {
                "outcome": { "status": "unsupported" },
                "digest": "a".repeat(64),
                "text_origin": "none",
                "corrected": false,
                "warnings": []
            }
        }))?,
    })))
}

/// `error` is a `Conflict` that names every one of `names`.
fn assert_conflict_naming(error: &ApiError, names: &[&str]) {
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    for name in names {
        assert!(error.message.contains(name), "{name}: {}", error.message);
    }
}

/// `error` refuses a note file not ending in lower-case `.md`: `InvalidInput` on `field`,
/// naming the lower-case name `named`.
fn assert_lower_case(error: &ApiError, named: &str, field: &str) {
    assert_eq!(error.code, ErrorCode::InvalidInput, "{}", error.message);
    assert_eq!(error.field.as_deref(), Some(field), "{}", error.message);
    assert!(
        error.message.contains(&format!(
            "a note's file ends in `.md` in lower case: name it {named}"
        )),
        "{}",
        error.message
    );
}

// S3: a note's file ends in `.md`, in lower case.

/// A note created, moved or written as a source card at a path ending in `.MD`, `.Md` or
/// `.mD` is refused naming the lower-case name, and nothing is committed.
#[tokio::test]
async fn a_note_ending_in_upper_case_md_is_refused_naming_the_lower_case_name() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(2, &space.initial, vec![note(10, "a.md", "Ten.")?])
        .await?;
    for (mutations, at, named) in [
        ([3, 4, 5], "Notes/X.MD", "Notes/X.md"),
        ([6, 7, 8], "y.Md", "y.md"),
        ([9, 10, 11], "z.mD", "z.md"),
    ] {
        let [create, move_to, write_card] = mutations;
        let created = space
            .refused(create, &seeded, vec![note(11, at, "Eleven.")?])
            .await?;
        assert_lower_case(&created, named, "/path");
        let moving = space
            .refused(move_to, &seeded, vec![moved(10, at)?])
            .await?;
        assert_lower_case(&moving, named, "/destination");
        let carded = space
            .refused(write_card, &seeded, vec![card(12, at)?])
            .await?;
        assert_lower_case(&carded, named, "/path");
    }
    Ok(())
}

/// A note file ending in `.MD` that carries an application header, committed straight through
/// Git, is a plain file: no listing, path lookup or identity lookup answers it as an item, it
/// reads back as a file, the next commit keeps its bytes, and no note may take its path.
#[tokio::test]
async fn an_upper_case_md_file_in_a_tree_is_a_plain_file_that_commits_keep() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(2, &space.initial, vec![note(10, "x.md", "Ten.")?])
        .await?;
    let bytes = some(space.file(&seeded, "x.md")?, "the note's file")?;
    let plain = space.raw(900, &["x.md"], &[("X.MD", bytes.clone())])?;
    let versions = space.storage.versions();
    let page = Page {
        cursor: None,
        limit: 50,
    };
    let listing = versions.list(&space.scope, &plain, None, page).await?;
    assert!(listing.items.is_empty(), "{:?}", listing.items);
    assert_eq!(space.item_at(&plain, "X.MD").await?, None);
    let shown = err_of(versions.show(&space.scope, &plain, uuid!(10)?).await)?;
    assert_eq!(shown.code, ErrorCode::NotFound, "{}", shown.message);
    assert_eq!(
        versions
            .read_file(&space.scope, &plain, &path("X.MD")?)
            .await?,
        bytes
    );
    let next = space
        .commit(3, &plain, vec![note(11, "y.md", "Eleven.")?])
        .await?;
    assert_eq!(space.file(&next, "X.MD")?, Some(bytes.clone()));
    let taken = space
        .refused(4, &next, vec![note(12, "x.md", "Twelve.")?])
        .await?;
    assert_conflict_naming(&taken, &["x.md", "already taken"]);
    assert_eq!(space.file(&next, "X.MD")?, Some(bytes));
    Ok(())
}

/// Search indexes no file ending in `.MD` as an item: its words find nothing and the graph of
/// the revision holds only the lower-case notes.
#[tokio::test]
async fn search_does_not_index_an_upper_case_md_file_as_an_item() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "x.md", "A zeppelin over the harbor.")?,
                note(11, "y.md", "Eleven.")?,
            ],
        )
        .await?;
    let bytes = some(space.file(&seeded, "x.md")?, "the note's file")?;
    let plain = space.raw(900, &["x.md"], &[("X.MD", bytes)])?;
    let search = space.storage.search();
    search.index_revision(&space.scope, plain.clone()).await?;
    let found = search
        .search(
            &space.scope,
            SearchQuery {
                revision: plain.clone(),
                text: "zeppelin".to_owned(),
                folder: None,
                include_archived: true,
                extraction: None,
                page: Page {
                    cursor: None,
                    limit: 20,
                },
            },
        )
        .await?;
    assert!(found.hits.is_empty(), "{:?}", found.hits);
    let graph = search
        .graph(
            &space.scope,
            GraphQuery {
                revision: plain,
                folder: None,
                max_nodes: 50,
            },
        )
        .await?;
    let nodes: Vec<ItemId> = graph.nodes.iter().map(|node| node.id).collect();
    assert_eq!(nodes, vec![uuid!(11)?]);
    Ok(())
}

// S1: a rename that changes only letter case.

/// A note at `at` (item 10) that links to a sibling at `plan` (item 11), and a note at the root
/// (item 12) that links to it as `at`, are committed; item 10 is renamed to `to`, which differs
/// from `at` only in letter case. The rename is one move in one commit: the committed tree holds
/// only `to`, with the bytes `at` held; the referrer points at `to` and is the one file the
/// check is told; no private path is in the tree, in `rewritten` or in `log.md`.
async fn renamed_by_case_only(at: &str, plan: &str, to: &str) -> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, at, "The report.\n\nSee [the plan](plan.md).")?,
                note(11, plan, "The plan.")?,
                note(12, "ref.md", &format!("Read [the report]({at})."))?,
            ],
        )
        .await?;
    let recording = Arc::new(Recording::default());
    let renamed = space
        .commit_with(3, &seeded, vec![moved(10, to)?], recording.clone())
        .await?
        .revision;
    let files = space.files(&renamed)?;
    assert!(files.iter().any(|file| file == to), "{files:?}");
    assert!(!files.iter().any(|file| file == at), "{files:?}");
    assert!(!files.iter().any(|file| file.contains(PARKED)), "{files:?}");
    assert_eq!(space.item_at(&renamed, to).await?, Some(uuid!(10)?));
    assert_eq!(
        space.file(&renamed, to)?,
        space.file(&seeded, at)?,
        "the moved item's file is byte for byte what it was"
    );
    let referrer = space.body(&renamed, 12).await?;
    assert!(referrer.contains(&format!("]({to})")), "{referrer}");
    assert_eq!(recording.seen()?, [vec!["ref.md".to_owned()]]);
    let log = String::from_utf8(some(space.file(&renamed, "log.md")?, "log.md")?)?;
    assert_eq!(log.matches("change 3").count(), 1, "{log}");
    assert!(!log.contains(PARKED), "{log}");
    one_move_in_history(&space, &seeded, &renamed, at, to).await
}

/// The history of item 10 from `before` to `after` is one commit with one moved file.
async fn one_move_in_history(
    space: &Workspace,
    before: &Revision,
    after: &Revision,
    at: &str,
    to: &str,
) -> TestResult {
    let versions = space.storage.versions();
    let history = versions
        .log(
            &space.scope,
            LogQuery {
                tip: after.clone(),
                item_id: Some(uuid!(10)?),
                page: Page {
                    cursor: None,
                    limit: 10,
                },
            },
        )
        .await?;
    assert_eq!(history.commits.len(), 2, "created, then renamed once");
    let compared = versions
        .diff(
            &space.scope,
            DiffQuery {
                from: before.clone(),
                to: after.clone(),
                item_id: Some(uuid!(10)?),
            },
        )
        .await?;
    let change = some(compared.changes.first(), "the rename")?;
    assert_eq!(compared.changes.len(), 1, "{:?}", compared.changes);
    assert_eq!(change.kind, FileChangeKind::Moved);
    assert_eq!(change.old_path.as_deref(), Some(at));
    assert_eq!(change.new_path.as_deref(), Some(to));
    Ok(())
}

#[tokio::test]
async fn a_rename_that_changes_only_letter_case_at_the_top_level_is_one_move() -> TestResult {
    renamed_by_case_only("Report.md", "plan.md", "report.md").await
}

#[tokio::test]
async fn a_rename_that_changes_only_letter_case_in_a_folder_is_one_move() -> TestResult {
    renamed_by_case_only("Notes/Report.md", "Notes/plan.md", "Notes/report.md").await
}

/// The Rewind of a case-only rename puts the earlier spelling back, with its links (`move_back`
/// parks the item beside itself first, as the rename did).
#[tokio::test]
async fn a_rewind_of_a_case_only_rename_puts_the_earlier_spelling_back() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "Notes/Report.md", "The report.")?,
                note(12, "ref.md", "Read [the report](Notes/Report.md).")?,
            ],
        )
        .await?;
    let renamed = space
        .commit(3, &seeded, vec![moved(10, "Notes/report.md")?])
        .await?;
    let rewound = space
        .commit(4, &renamed, vec![restore(10, &seeded)?])
        .await?;
    let files = space.files(&rewound)?;
    assert!(
        files.iter().any(|file| file == "Notes/Report.md"),
        "{files:?}"
    );
    assert!(
        !files
            .iter()
            .any(|file| file == "Notes/report.md" || file.contains("okf-jawn-")),
        "{files:?}"
    );
    assert_eq!(
        space.file(&rewound, "Notes/Report.md")?,
        space.file(&seeded, "Notes/Report.md")?
    );
    let referrer = space.body(&rewound, 12).await?;
    assert!(referrer.contains("](Notes/Report.md)"), "{referrer}");
    Ok(())
}

/// A move onto another item's path, or a plain file's, in another letter case is refused as
/// taken, as before: only the moved item's own file is not counted.
#[tokio::test]
async fn a_move_onto_another_files_path_in_another_case_is_still_refused() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![note(10, "a.md", "Ten.")?, note(11, "b.md", "Eleven.")?],
        )
        .await?;
    let plain = space.raw(900, &[], &[("C.MD", b"plain".to_vec())])?;
    for (mutation, to) in [(3, "B.md"), (4, "c.md")] {
        let error = space
            .refused(mutation, &plain, vec![moved(10, to)?])
            .await?;
        assert_conflict_naming(&error, &[to, "already taken"]);
    }
    assert_eq!(space.body(&plain, 11).await?, "Eleven.");
    assert_eq!(space.file(&plain, "C.MD")?, Some(b"plain".to_vec()));
    assert_eq!(space.file(&plain, "a.md")?, space.file(&seeded, "a.md")?);
    Ok(())
}

/// The moved item's own file is not counted only for the path it already holds in another
/// case: as a folder above the destination, it still takes that path.
#[tokio::test]
async fn a_move_below_the_items_own_file_is_still_refused_as_taken() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(2, &space.initial, vec![note(10, "a.md", "Ten.")?])
        .await?;
    let error = space
        .refused(3, &seeded, vec![moved(10, "a.md/inner.md")?])
        .await?;
    assert_conflict_naming(&error, &["a.md/inner.md", "already taken"]);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
