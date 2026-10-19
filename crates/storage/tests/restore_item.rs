//! `TreeEdit::RestoreItem` against real Git repositories: one item and every companion file
//! it owns set back to an earlier revision.
//!
//! The companions of a source card are its appearance (`.okf/sources/<id>.json`), its
//! accepted supplied text (`.okf/supplied/<id>.json`) and its corrections folder
//! (`.okf/corrections/<id>/`). Their presence and bytes are read straight from the commits
//! with `git2`, so the assertions name the exact files storage keeps.
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use git2::Repository;
use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, ItemId, Revision, TenantId, WorkspacePath};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_core::conformance::EditCheck;
use okf_jawn_core::storage::{
    CandidateChanges, CandidateCheck, CommitChanges, Committed, DiffQuery, LogQuery, NewWorkspace,
    Page, Provenance, SourceCard, StorageScope, TreeEdit, VersionStore, WorkspaceCatalog,
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

/// Refuses every candidate.
struct Refuses;

/// A check that records the rewritten paths each run was told (`CandidateCheck::check`) and
/// answers as the check it wraps.
struct Recording {
    told: Mutex<Vec<Vec<String>>>,
    inner: Arc<dyn CandidateCheck>,
}

/// A data directory with one blank workspace.
struct Workspace {
    storage: Storage,
    scope: StorageScope,
    /// The catalog's first commit.
    initial: Revision,
    /// Declared last: fields drop in declaration order, so the storage has closed its
    /// databases and its lock when the directory is removed (Windows cannot remove an open
    /// file, and `TempDir` would leave the directory behind).
    directory: tempfile::TempDir,
}

/// Where every test keeps its source card.
const CARD: &str = "sources/scan-pdf.md";

impl CandidateCheck for Accept {
    fn check(
        &self,
        _root: &Path,
        _rewritten: &[okf_jawn_contract::identity::WorkspacePath],
    ) -> Result<Vec<Warning>, ApiError> {
        Ok(Vec::new())
    }
}

impl CandidateCheck for Refuses {
    fn check(&self, _root: &Path, _rewritten: &[WorkspacePath]) -> Result<Vec<Warning>, ApiError> {
        Err(ApiError::new(ErrorCode::InvalidInput, "the check refuses"))
    }
}

impl Recording {
    fn over(inner: Arc<dyn CandidateCheck>) -> Arc<Self> {
        Arc::new(Self {
            told: Mutex::new(Vec::new()),
            inner,
        })
    }

    /// What each run of the check was told the store rewrote, in run order.
    fn seen(&self) -> Fallible<Vec<Vec<String>>> {
        Ok(self.told.lock().map_err(|_| "poisoned")?.clone())
    }
}

impl CandidateCheck for Recording {
    fn check(&self, root: &Path, rewritten: &[WorkspacePath]) -> Result<Vec<Warning>, ApiError> {
        self.told
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal, "poisoned"))?
            .push(
                rewritten
                    .iter()
                    .map(|path| path.as_str().to_owned())
                    .collect(),
            );
        self.inner.check(root, rewritten)
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

    /// Every file path of `revision` that starts with `prefix`.
    fn files_under(&self, revision: &Revision, prefix: &str) -> Fallible<Vec<String>> {
        let repository = self.repository()?;
        let tree = repository
            .find_commit(git2::Oid::from_str(revision.as_str())?)?
            .tree()?;
        let mut found = Vec::new();
        tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
            if entry.kind() == Some(git2::ObjectType::Blob)
                && let Ok(name) = entry.name()
            {
                let full = format!("{root}{name}");
                if full.starts_with(prefix) {
                    found.push(full);
                }
            }
            git2::TreeWalkResult::Ok
        })?;
        Ok(found)
    }

    /// Commit on main straight through `git2`, writing `bytes` at `at`, as no store edit can.
    fn raw_upsert(&self, at: &str, bytes: &[u8], mutation: u32) -> Fallible<Revision> {
        let repository = self.repository()?;
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

    /// Commit on main straight through `git2`, removing the file at `at`, as no store edit can.
    fn raw_remove(&self, at: &str, mutation: u32) -> Fallible<Revision> {
        let repository = self.repository()?;
        let parent = repository.find_commit(repository.refname_to_id("refs/heads/main")?)?;
        let mut update = git2::build::TreeUpdateBuilder::new();
        update.remove(at);
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

    /// The bytes of the file at `at` in `revision`, read straight from the repository.
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

    /// The identity of the item `revision` holds at `at`.
    async fn item_at(&self, revision: &Revision, at: &str) -> Fallible<Option<ItemId>> {
        Ok(self
            .storage
            .versions()
            .item_at_path(&self.scope, revision, &path(at)?)
            .await?
            .map(|summary| summary.id))
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

fn digest(fill: char) -> Fallible<Digest> {
    Ok(Digest::try_from(fill.to_string().repeat(64))?)
}

fn note(item: u32, at: &str, body: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::CreateItem {
        item_id: uuid!(item)?,
        path: path(at)?,
        title: Some(format!("Note at {at}")),
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

/// A source card at `sources/scan-pdf.md` whose appearance names `media_type` and whose
/// extraction shows the text of `shown`.
fn card(item: u32, media_type: &str, shown: char) -> Fallible<TreeEdit> {
    Ok(TreeEdit::WriteSourceCard(Box::new(SourceCard {
        item_id: uuid!(item)?,
        path: path("sources/scan-pdf.md")?,
        title: "scan.pdf".to_owned(),
        type_name: "Source".to_owned(),
        body: format!("Converted {shown}."),
        properties: BTreeMap::new(),
        appearance: serde_json::from_value(json!({
            "object": "d".repeat(64),
            "names": [{
                "filename": "scan.pdf",
                "folder": "sources",
                "observed_at": "2026-10-09T12:00:00.000Z",
                "supplied_by": "ana"
            }],
            "media_type": media_type,
            "size": "10",
            "metadata": {},
            "extraction": {
                "outcome": { "status": "unsupported" },
                "digest": shown.to_string().repeat(64),
                "text_origin": "none",
                "corrected": false,
                "warnings": []
            }
        }))?,
    })))
}

fn correction(item: u32, of: char, text: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::CorrectDigest {
        item_id: uuid!(item)?,
        digest: digest(of)?,
        corrected_markdown: text.to_owned(),
        adopts: None,
    })
}

fn supplied(item: u32, based_on: char) -> Fallible<TreeEdit> {
    Ok(TreeEdit::SupplyExtraction {
        item_id: uuid!(item)?,
        based_on: Some(digest(based_on)?),
        pages: Vec::new(),
        markdown: "Supplied text.".to_owned(),
        proposal_id: uuid!(60)?,
        supplier: person("agent"),
    })
}

fn appearance_file(item: u32) -> String {
    format!(".okf/sources/{}.json", uuid_text(item))
}

fn supplied_file(item: u32) -> String {
    format!(".okf/supplied/{}.json", uuid_text(item))
}

fn correction_file(item: u32, of: char) -> String {
    format!(
        "{}{}.json",
        corrections_folder(item),
        of.to_string().repeat(64)
    )
}

/// The corrections folder of `item`, with its trailing slash.
fn corrections_folder(item: u32) -> String {
    format!(".okf/corrections/{}/", uuid_text(item))
}

#[tokio::test]
async fn a_card_rewound_before_its_supplied_text_and_a_later_correction_loses_both() -> TestResult {
    let space = Workspace::new().await?;
    let early = space
        .commit(
            2,
            &space.initial,
            vec![
                card(12, "application/pdf", 'a')?,
                correction(12, 'a', "Corrected A.")?,
            ],
        )
        .await?;
    let late = space
        .commit(
            3,
            &early,
            vec![
                card(12, "application/x-pdf", 'b')?,
                supplied(12, 'b')?,
                correction(12, 'b', "Corrected B.")?,
            ],
        )
        .await?;
    for companion in [
        appearance_file(12),
        supplied_file(12),
        correction_file(12, 'a'),
        correction_file(12, 'b'),
    ] {
        assert!(
            space.file(&late, &companion)?.is_some(),
            "{companion} is held before the rewind"
        );
    }
    assert_ne!(
        space.file(&late, &appearance_file(12))?,
        space.file(&early, &appearance_file(12))?,
        "the appearance changed after `early`"
    );
    let rewound = space.commit(4, &late, vec![restore(12, &early)?]).await?;
    assert_eq!(space.file(&rewound, CARD)?, space.file(&early, CARD)?);
    assert_eq!(
        space.file(&rewound, &appearance_file(12))?,
        space.file(&early, &appearance_file(12))?,
        "the appearance is the old one"
    );
    assert_eq!(
        space.file(&rewound, &supplied_file(12))?,
        None,
        "supplied text absent at `from` is removed"
    );
    assert_eq!(
        space.file(&rewound, &correction_file(12, 'a'))?,
        space.file(&early, &correction_file(12, 'a'))?,
        "a correction recorded before `from` stays"
    );
    assert_eq!(
        space.file(&rewound, &correction_file(12, 'b'))?,
        None,
        "a correction recorded after `from` is removed"
    );
    let shown = space
        .storage
        .versions()
        .show(&space.scope, &rewound, uuid!(12)?)
        .await?;
    assert_eq!(shown.body, "Corrected A.");
    Ok(())
}

#[tokio::test]
async fn a_card_restored_forward_gets_every_companion_back() -> TestResult {
    let space = Workspace::new().await?;
    let early = space
        .commit(2, &space.initial, vec![card(12, "application/pdf", 'a')?])
        .await?;
    let late = space
        .commit(
            3,
            &early,
            vec![
                card(12, "application/x-pdf", 'b')?,
                supplied(12, 'b')?,
                correction(12, 'a', "Corrected A.")?,
                correction(12, 'b', "Corrected B.")?,
            ],
        )
        .await?;
    let rewound = space.commit(4, &late, vec![restore(12, &early)?]).await?;
    assert_eq!(space.file(&rewound, &supplied_file(12))?, None);
    assert_eq!(space.file(&rewound, &correction_file(12, 'a'))?, None);
    let forward = space.commit(5, &rewound, vec![restore(12, &late)?]).await?;
    for at in [
        CARD.to_owned(),
        appearance_file(12),
        supplied_file(12),
        correction_file(12, 'a'),
        correction_file(12, 'b'),
    ] {
        let restored = space.file(&forward, &at)?;
        assert!(restored.is_some(), "{at} is copied back");
        assert_eq!(restored, space.file(&late, &at)?, "{at} as `late` holds it");
    }
    Ok(())
}

/// The paths an item-limited comparison of `from` and `to` lists, each change's new path (its
/// old one when it removed the file).
async fn item_diff(
    space: &Workspace,
    item: u32,
    from: &Revision,
    to: &Revision,
) -> Fallible<Vec<String>> {
    let compared = space
        .storage
        .versions()
        .diff(
            &space.scope,
            DiffQuery {
                from: from.clone(),
                to: to.clone(),
                item_id: Some(uuid!(item)?),
            },
        )
        .await?;
    Ok(compared
        .changes
        .into_iter()
        .filter_map(|change| change.new_path.or(change.old_path))
        .collect())
}

/// The commits the item-filtered log from `tip` names, newest first.
async fn item_log(space: &Workspace, item: u32, tip: &Revision) -> Fallible<Vec<Revision>> {
    let log = space
        .storage
        .versions()
        .log(
            &space.scope,
            LogQuery {
                tip: tip.clone(),
                item_id: Some(uuid!(item)?),
                page: Page {
                    cursor: None,
                    limit: 50,
                },
            },
        )
        .await?;
    Ok(log
        .commits
        .into_iter()
        .map(|commit| commit.revision)
        .collect())
}

/// N1 (core-cli M1.2 re-review): a change that touches only an item's companion files is a
/// change of the item, as `RestoreItem` restores them with it: an appearance-only card write,
/// and, written through Git, a correction file and a supplied-text file alone. The
/// item-limited comparison lists exactly that file, and the item-filtered log names each
/// commit; another item's comparison and log do not. (A `CorrectDigest` is not one: it marks
/// the card's own file corrected too.)
#[tokio::test]
async fn a_change_of_only_an_items_companions_is_a_change_of_the_item() -> TestResult {
    let space = Workspace::new().await?;
    let carded = space
        .commit(
            2,
            &space.initial,
            vec![
                card(12, "application/pdf", 'a')?,
                note(10, "notes/n.md", "N.")?,
            ],
        )
        .await?;
    let shown = space
        .commit(3, &carded, vec![card(12, "application/x-pdf", 'a')?])
        .await?;
    let corrected = space.raw_upsert(
        &correction_file(12, 'b'),
        b"{\"corrected_markdown\": \"Corrected B.\"}",
        30,
    )?;
    let supplied = space.raw_upsert(&supplied_file(12), b"{\"markdown\": \"Supplied.\"}", 31)?;
    let unrelated = space
        .commit(4, &supplied, vec![note(11, "notes/o.md", "O.")?])
        .await?;
    for (from, to, only) in [
        (&carded, &shown, appearance_file(12)),
        (&shown, &corrected, correction_file(12, 'b')),
        (&corrected, &supplied, supplied_file(12)),
    ] {
        assert!(space.file(to, CARD)?.is_some());
        assert_eq!(
            space.file(to, CARD)?,
            space.file(from, CARD)?,
            "the card's own file is unchanged: {only}"
        );
        assert_eq!(item_diff(&space, 12, from, to).await?, vec![only]);
        assert_eq!(item_diff(&space, 10, from, to).await?, Vec::<String>::new());
    }
    assert_eq!(
        item_log(&space, 12, &unrelated).await?,
        vec![supplied, corrected, shown, carded.clone()]
    );
    assert_eq!(item_log(&space, 10, &unrelated).await?, vec![carded]);
    Ok(())
}

#[tokio::test]
async fn an_item_absent_at_from_is_removed_with_its_companions() -> TestResult {
    let space = Workspace::new().await?;
    let late = space
        .commit(
            2,
            &space.initial,
            vec![
                card(12, "application/pdf", 'a')?,
                supplied(12, 'a')?,
                correction(12, 'a', "Corrected A.")?,
            ],
        )
        .await?;
    let rewound = space
        .commit(3, &late, vec![restore(12, &space.initial)?])
        .await?;
    for at in [
        CARD.to_owned(),
        appearance_file(12),
        supplied_file(12),
        correction_file(12, 'a'),
    ] {
        assert_eq!(space.file(&rewound, &at)?, None, "{at} is removed");
    }
    let error = err_of(
        space
            .storage
            .versions()
            .show(&space.scope, &rewound, uuid!(12)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound);
    Ok(())
}

#[tokio::test]
async fn a_moved_note_returns_to_its_old_path_and_the_new_path_is_gone() -> TestResult {
    let space = Workspace::new().await?;
    let early = space
        .commit(
            2,
            &space.initial,
            vec![note(10, "notes/a.md", "Original.")?],
        )
        .await?;
    let moved = space
        .commit(
            3,
            &early,
            vec![
                TreeEdit::MoveItem {
                    item_id: uuid!(10)?,
                    destination: path("archive/a.md")?,
                },
                TreeEdit::EditItem {
                    item_id: uuid!(10)?,
                    body: "Rewritten.".to_owned(),
                    properties: BTreeMap::new(),
                },
            ],
        )
        .await?;
    let rewound = space.commit(4, &moved, vec![restore(10, &early)?]).await?;
    assert_eq!(
        space.item_at(&rewound, "notes/a.md").await?,
        Some(uuid!(10)?)
    );
    assert_eq!(
        space.file(&rewound, "notes/a.md")?,
        space.file(&early, "notes/a.md")?
    );
    assert_eq!(
        space.file(&rewound, "archive/a.md")?,
        None,
        "the moved file is gone"
    );
    Ok(())
}

#[tokio::test]
async fn restoring_over_another_items_path_conflicts_and_writes_nothing() -> TestResult {
    let space = Workspace::new().await?;
    let early = space
        .commit(2, &space.initial, vec![note(10, "a.md", "Ten.")?])
        .await?;
    let taken = space
        .commit(
            3,
            &early,
            vec![
                TreeEdit::MoveItem {
                    item_id: uuid!(10)?,
                    destination: path("b.md")?,
                },
                note(11, "a.md", "Eleven.")?,
            ],
        )
        .await?;
    let error = err_of(space.commit(4, &taken, vec![restore(10, &early)?]).await)?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    assert_eq!(
        space.storage.versions().head(&space.scope).await?,
        taken,
        "nothing is written"
    );
    Ok(())
}

#[tokio::test]
async fn an_item_absent_at_both_revisions_is_not_found() -> TestResult {
    let space = Workspace::new().await?;
    let head = space
        .commit(2, &space.initial, vec![note(10, "a.md", "Ten.")?])
        .await?;
    let error = err_of(
        space
            .commit(3, &head, vec![restore(99, &space.initial)?])
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound, "{}", error.message);
    assert_eq!(space.storage.versions().head(&space.scope).await?, head);
    Ok(())
}

#[tokio::test]
async fn an_archived_item_restored_from_before_its_archiving_is_not_a_header_change() -> TestResult
{
    let space = Workspace::new().await?;
    let early = space
        .commit(2, &space.initial, vec![note(10, "a.md", "Ten.")?])
        .await?;
    let archived = space
        .commit(
            3,
            &early,
            vec![TreeEdit::SetStatus {
                item_id: uuid!(10)?,
                status: None,
                archived: Some(true),
            }],
        )
        .await?;
    let rewound = space
        .commit(4, &archived, vec![restore(10, &early)?])
        .await?;
    let shown = space
        .storage
        .versions()
        .show(&space.scope, &rewound, uuid!(10)?)
        .await?;
    assert!(
        !shown.summary.archived,
        "the server's restore writes the old header"
    );
    Ok(())
}

#[tokio::test]
async fn two_items_that_traded_paths_are_restored_in_one_commit() -> TestResult {
    let space = Workspace::new().await?;
    let early = space
        .commit(
            2,
            &space.initial,
            vec![note(10, "x.md", "Ten.")?, note(11, "y.md", "Eleven.")?],
        )
        .await?;
    let traded = space
        .commit(
            3,
            &early,
            vec![
                TreeEdit::MoveItem {
                    item_id: uuid!(10)?,
                    destination: path("parked.md")?,
                },
                TreeEdit::MoveItem {
                    item_id: uuid!(11)?,
                    destination: path("x.md")?,
                },
                TreeEdit::MoveItem {
                    item_id: uuid!(10)?,
                    destination: path("y.md")?,
                },
                TreeEdit::EditItem {
                    item_id: uuid!(11)?,
                    body: "Eleven, rewritten.".to_owned(),
                    properties: BTreeMap::new(),
                },
            ],
        )
        .await?;
    let rewound = space
        .commit(4, &traded, vec![restore(10, &early)?, restore(11, &early)?])
        .await?;
    assert_eq!(space.item_at(&rewound, "x.md").await?, Some(uuid!(10)?));
    assert_eq!(space.item_at(&rewound, "y.md").await?, Some(uuid!(11)?));
    for at in ["x.md", "y.md"] {
        assert_eq!(space.file(&rewound, at)?, space.file(&early, at)?, "{at}");
    }
    assert_eq!(space.file(&rewound, "parked.md")?, None);
    let shown = space
        .storage
        .versions()
        .show(&space.scope, &rewound, uuid!(11)?)
        .await?;
    assert_eq!(shown.body, "Eleven.");
    Ok(())
}

#[tokio::test]
async fn a_deleted_card_leaves_no_companion_files() -> TestResult {
    let space = Workspace::new().await?;
    let carded = space
        .commit(
            2,
            &space.initial,
            vec![
                card(12, "application/pdf", 'a')?,
                supplied(12, 'a')?,
                correction(12, 'a', "Corrected A.")?,
            ],
        )
        .await?;
    let deleted = space
        .commit(
            3,
            &carded,
            vec![TreeEdit::DeleteItem {
                item_id: uuid!(12)?,
            }],
        )
        .await?;
    for at in [CARD.to_owned(), appearance_file(12), supplied_file(12)] {
        assert!(
            space.file(&carded, &at)?.is_some(),
            "{at} before the delete"
        );
        assert_eq!(space.file(&deleted, &at)?, None, "{at} after the delete");
    }
    assert_eq!(
        space.files_under(&deleted, &corrections_folder(12))?,
        Vec::<String>::new(),
        "the corrections folder goes with the item"
    );
    Ok(())
}

/// Core M1.2 storage-side re-review N1 (mutant 9): a delete clears the whole corrections folder,
/// files nested below its top level included, not only its top-level files.
#[tokio::test]
async fn a_deleted_card_leaves_no_nested_correction_file() -> TestResult {
    let space = Workspace::new().await?;
    space
        .commit(
            2,
            &space.initial,
            vec![
                card(12, "application/pdf", 'a')?,
                correction(12, 'a', "Corrected A.")?,
            ],
        )
        .await?;
    let folder = corrections_folder(12);
    let nested = format!("{folder}nested/deep/x.json");
    space.raw_upsert(&nested, b"{\"nested\":true}", 40)?;
    let held = space.raw_upsert(&format!("{folder}nested/y.json"), b"{}", 41)?;
    let before = space.files_under(&held, &folder)?;
    assert_eq!(before.len(), 3, "{before:?}");
    let deleted = space
        .commit(
            3,
            &held,
            vec![TreeEdit::DeleteItem {
                item_id: uuid!(12)?,
            }],
        )
        .await?;
    assert_eq!(
        space.files_under(&deleted, &folder)?,
        Vec::<String>::new(),
        "every file under the corrections folder goes with the item"
    );
    assert_eq!(space.file(&deleted, &nested)?, None);
    Ok(())
}

#[tokio::test]
async fn a_card_restored_to_the_revision_that_deleted_it_leaves_no_companion_files() -> TestResult {
    let space = Workspace::new().await?;
    let carded = space
        .commit(
            2,
            &space.initial,
            vec![
                card(12, "application/pdf", 'a')?,
                supplied(12, 'a')?,
                correction(12, 'a', "Corrected A.")?,
            ],
        )
        .await?;
    let deleted = space
        .commit(
            3,
            &carded,
            vec![TreeEdit::DeleteItem {
                item_id: uuid!(12)?,
            }],
        )
        .await?;
    let undeleted = space
        .commit(4, &deleted, vec![restore(12, &carded)?])
        .await?;
    assert!(space.file(&undeleted, CARD)?.is_some());
    assert!(space.file(&undeleted, &appearance_file(12))?.is_some());
    let rewound = space
        .commit(5, &undeleted, vec![restore(12, &deleted)?])
        .await?;
    for at in [CARD.to_owned(), appearance_file(12), supplied_file(12)] {
        assert_eq!(space.file(&rewound, &at)?, None, "{at}");
    }
    assert_eq!(
        space.files_under(&rewound, &corrections_folder(12))?,
        Vec::<String>::new()
    );
    Ok(())
}

// Reviewer probes, adopted as regressions: grouping order, companions in odd shapes.

#[tokio::test]
async fn an_item_restored_onto_the_path_another_restored_item_vacates_in_either_order() -> TestResult
{
    for (first, second) in [(10_u32, 11_u32), (11, 10)] {
        let space = Workspace::new().await?;
        let early = space
            .commit(
                2,
                &space.initial,
                vec![note(10, "p.md", "A.")?, note(11, "q.md", "C.")?],
            )
            .await?;
        let late = space
            .commit(3, &early, vec![moved(10, "r.md")?, moved(11, "p.md")?])
            .await?;
        let rewound = space
            .commit(
                4,
                &late,
                vec![restore(first, &early)?, restore(second, &early)?],
            )
            .await?;
        assert_eq!(space.item_at(&rewound, "p.md").await?, Some(uuid!(10)?));
        assert_eq!(space.item_at(&rewound, "q.md").await?, Some(uuid!(11)?));
        assert_eq!(space.file(&rewound, "r.md")?, None);
    }
    Ok(())
}

#[tokio::test]
async fn an_item_restored_onto_the_path_an_item_absent_at_from_vacates_in_either_order()
-> TestResult {
    for (first, second) in [(10_u32, 12_u32), (12, 10)] {
        let space = Workspace::new().await?;
        let early = space
            .commit(2, &space.initial, vec![note(10, "p.md", "A.")?])
            .await?;
        let late = space
            .commit(3, &early, vec![moved(10, "r.md")?, note(12, "p.md", "D.")?])
            .await?;
        let rewound = space
            .commit(
                4,
                &late,
                vec![restore(first, &early)?, restore(second, &early)?],
            )
            .await?;
        assert_eq!(space.item_at(&rewound, "p.md").await?, Some(uuid!(10)?));
        assert_eq!(space.file(&rewound, "r.md")?, None);
        let error = err_of(
            space
                .storage
                .versions()
                .show(&space.scope, &rewound, uuid!(12)?)
                .await,
        )?;
        assert_eq!(error.code, ErrorCode::NotFound);
    }
    Ok(())
}

#[tokio::test]
async fn a_restore_then_an_edit_of_the_same_item_apply_in_order() -> TestResult {
    let space = Workspace::new().await?;
    let early = space
        .commit(
            2,
            &space.initial,
            vec![note(10, "notes/a.md", "Original.")?],
        )
        .await?;
    let late = space
        .commit(3, &early, vec![moved(10, "other/a.md")?])
        .await?;
    let edited = space
        .commit(
            4,
            &late,
            vec![
                restore(10, &early)?,
                TreeEdit::EditItem {
                    item_id: uuid!(10)?,
                    body: "Edited after restore.".to_owned(),
                    properties: BTreeMap::from([("title".to_owned(), json!("Note at notes/a.md"))]),
                },
            ],
        )
        .await?;
    assert_eq!(
        space.item_at(&edited, "notes/a.md").await?,
        Some(uuid!(10)?)
    );
    assert_eq!(space.file(&edited, "other/a.md")?, None);
    let shown = space
        .storage
        .versions()
        .show(&space.scope, &edited, uuid!(10)?)
        .await?;
    assert_eq!(shown.body.trim(), "Edited after restore.");
    Ok(())
}

#[tokio::test]
async fn restores_separated_by_another_edit_are_not_one_group() -> TestResult {
    let space = Workspace::new().await?;
    let early = space
        .commit(
            2,
            &space.initial,
            vec![note(10, "x.md", "Ten.")?, note(11, "y.md", "Eleven.")?],
        )
        .await?;
    let traded = space
        .commit(
            3,
            &early,
            vec![
                moved(10, "parked.md")?,
                moved(11, "x.md")?,
                moved(10, "y.md")?,
            ],
        )
        .await?;
    let error = err_of(
        space
            .commit(
                4,
                &traded,
                vec![
                    restore(10, &early)?,
                    TreeEdit::CreateFolder {
                        folder: path("between")?,
                    },
                    restore(11, &early)?,
                ],
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    Ok(())
}

#[tokio::test]
async fn nested_files_in_a_corrections_folder_follow_the_restore_both_ways() -> TestResult {
    let space = Workspace::new().await?;
    let early = space
        .commit(
            2,
            &space.initial,
            vec![
                card(12, "application/pdf", 'a')?,
                correction(12, 'a', "Corrected A.")?,
            ],
        )
        .await?;
    let folder = corrections_folder(12);
    space.raw_upsert(
        &format!("{folder}nested/deep/x.json"),
        b"{\"nested\":true}",
        40,
    )?;
    let late = space.raw_upsert(&format!("{folder}zz.json"), b"{}", 41)?;
    let before = space.files_under(&late, &folder)?;
    assert_eq!(before.len(), 3, "{before:?}");
    let rewound = space.commit(4, &late, vec![restore(12, &early)?]).await?;
    assert_eq!(
        space.files_under(&rewound, &folder)?,
        space.files_under(&early, &folder)?,
        "exactly the corrections `from` holds"
    );
    let forward = space.commit(5, &rewound, vec![restore(12, &late)?]).await?;
    assert_eq!(space.files_under(&forward, &folder)?, before);
    for at in &before {
        assert_eq!(space.file(&forward, at)?, space.file(&late, at)?, "{at}");
    }
    Ok(())
}

#[tokio::test]
async fn an_item_listed_twice_ends_as_its_last_listing() -> TestResult {
    let space = Workspace::new().await?;
    let early = space
        .commit(2, &space.initial, vec![card(12, "application/pdf", 'a')?])
        .await?;
    let late = space
        .commit(
            3,
            &early,
            vec![card(12, "application/x-pdf", 'b')?, supplied(12, 'b')?],
        )
        .await?;
    let head = space
        .commit(4, &late, vec![note(10, "n.md", "N.")?])
        .await?;
    let as_late = space
        .commit(5, &head, vec![restore(12, &early)?, restore(12, &late)?])
        .await?;
    assert!(space.file(&as_late, &supplied_file(12))?.is_some());
    for at in [supplied_file(12), appearance_file(12)] {
        assert_eq!(space.file(&as_late, &at)?, space.file(&late, &at)?, "{at}");
    }
    let as_early = space
        .commit(6, &as_late, vec![restore(12, &late)?, restore(12, &early)?])
        .await?;
    assert_eq!(space.file(&as_early, &supplied_file(12))?, None);
    assert_eq!(
        space.file(&as_early, &appearance_file(12))?,
        space.file(&early, &appearance_file(12))?
    );
    Ok(())
}

/// S7 (fix5 re-review): an item listed twice in one restore run, held at a third path now and
/// at a different path at each listing's revision, ends at its last listing's path with that
/// revision's bytes, and a link that pointed at it follows it there: the move back goes to the
/// last listing's path, in either order of the two listings.
#[tokio::test]
async fn a_mover_listed_twice_ends_at_its_last_listings_path_with_its_links() -> TestResult {
    let space = Workspace::new().await?;
    let first = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "one/a.md", "First.")?,
                note(11, "ref.md", "See [it](one/a.md).")?,
            ],
        )
        .await?;
    let second = space
        .commit(
            3,
            &first,
            vec![
                moved(10, "two/a.md")?,
                TreeEdit::EditItem {
                    item_id: uuid!(10)?,
                    body: "Second.".to_owned(),
                    properties: BTreeMap::new(),
                },
            ],
        )
        .await?;
    let now = space
        .commit(4, &second, vec![moved(10, "three/a.md")?])
        .await?;
    let mut head = now;
    for (mutation, listings, (ends, held)) in [
        (5, [&first, &second], ("two/a.md", &second)),
        (6, [&second, &first], ("one/a.md", &first)),
    ] {
        let [earlier, last] = listings;
        head = space
            .commit(
                mutation,
                &head,
                vec![restore(10, earlier)?, restore(10, last)?],
            )
            .await?;
        assert_eq!(
            space.item_at(&head, ends).await?,
            Some(uuid!(10)?),
            "{ends}"
        );
        assert_eq!(space.file(&head, ends)?, space.file(held, ends)?, "{ends}");
        for other in ["one/a.md", "two/a.md", "three/a.md"] {
            if other != ends {
                assert_eq!(space.file(&head, other)?, None, "{other}");
            }
        }
        let referrer = body_of(&space, &head, 11).await?;
        assert_eq!(referrer, format!("See [it]({ends})."), "{ends}");
    }
    Ok(())
}

#[tokio::test]
async fn one_item_of_a_group_absent_at_both_revisions_writes_nothing() -> TestResult {
    let space = Workspace::new().await?;
    let early = space
        .commit(2, &space.initial, vec![note(10, "x.md", "Ten.")?])
        .await?;
    let late = space.commit(3, &early, vec![moved(10, "y.md")?]).await?;
    let error = err_of(
        space
            .commit(4, &late, vec![restore(10, &early)?, restore(99, &early)?])
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound, "{}", error.message);
    assert_eq!(space.storage.versions().head(&space.scope).await?, late);
    Ok(())
}

#[tokio::test]
async fn restoring_onto_a_path_that_differs_only_in_letter_case_conflicts() -> TestResult {
    let space = Workspace::new().await?;
    let early = space
        .commit(2, &space.initial, vec![note(10, "Topic.md", "Ten.")?])
        .await?;
    let late = space
        .commit(
            3,
            &early,
            vec![moved(10, "z.md")?, note(11, "topic.md", "Eleven.")?],
        )
        .await?;
    let error = err_of(space.commit(4, &late, vec![restore(10, &early)?]).await)?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    Ok(())
}

/// The body of item `item` at `revision`.
async fn body_of(space: &Workspace, revision: &Revision, item: u32) -> Fallible<String> {
    Ok(space
        .storage
        .versions()
        .show(&space.scope, revision, uuid!(item)?)
        .await?
        .body)
}

#[tokio::test]
async fn a_rewind_to_an_earlier_path_moves_the_links_that_point_at_the_item() -> TestResult {
    let space = Workspace::new().await?;
    let early = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "notes/a.md", "Original.")?,
                note(11, "ref.md", "See [the note](notes/a.md).")?,
            ],
        )
        .await?;
    let moved = space
        .commit(3, &early, vec![moved(10, "archive/a.md")?])
        .await?;
    let referring = body_of(&space, &moved, 11).await?;
    assert!(referring.contains("archive/a.md"), "{referring}");
    let rewound = space.commit(4, &moved, vec![restore(10, &early)?]).await?;
    assert_eq!(
        space.item_at(&rewound, "notes/a.md").await?,
        Some(uuid!(10)?)
    );
    assert_eq!(
        space.file(&rewound, "notes/a.md")?,
        space.file(&early, "notes/a.md")?
    );
    assert_eq!(space.file(&rewound, "archive/a.md")?, None);
    let referring = body_of(&space, &rewound, 11).await?;
    assert!(
        referring.contains("](notes/a.md)") && !referring.contains("archive/a.md"),
        "the link follows the item back: {referring}"
    );
    Ok(())
}

#[tokio::test]
async fn links_to_items_that_traded_paths_follow_each_item_back() -> TestResult {
    let space = Workspace::new().await?;
    let early = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "x.md", "Ten.")?,
                note(11, "y.md", "Eleven.")?,
                note(12, "ref.md", "[ten](x.md) and [eleven](y.md)")?,
            ],
        )
        .await?;
    let traded = space
        .commit(
            3,
            &early,
            vec![
                moved(10, "parked.md")?,
                moved(11, "x.md")?,
                moved(10, "y.md")?,
            ],
        )
        .await?;
    assert_eq!(
        body_of(&space, &traded, 12).await?,
        "[ten](y.md) and [eleven](x.md)"
    );
    let rewound = space
        .commit(4, &traded, vec![restore(10, &early)?, restore(11, &early)?])
        .await?;
    assert_eq!(space.item_at(&rewound, "x.md").await?, Some(uuid!(10)?));
    assert_eq!(space.item_at(&rewound, "y.md").await?, Some(uuid!(11)?));
    assert_eq!(
        body_of(&space, &rewound, 12).await?,
        "[ten](x.md) and [eleven](y.md)"
    );
    let left: Vec<String> = space
        .files_under(&rewound, "")?
        .into_iter()
        .filter(|file| file.contains("rewind"))
        .collect();
    assert!(left.is_empty(), "no private path is left: {left:?}");
    Ok(())
}

#[tokio::test]
async fn a_deleted_item_is_restored_without_rewriting_any_link() -> TestResult {
    let space = Workspace::new().await?;
    let early = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "notes/a.md", "Original.")?,
                note(11, "ref.md", "See [the note](notes/a.md).")?,
            ],
        )
        .await?;
    let deleted = space
        .commit(
            3,
            &early,
            vec![TreeEdit::DeleteItem {
                item_id: uuid!(10)?,
            }],
        )
        .await?;
    let rewound = space
        .commit(4, &deleted, vec![restore(10, &early)?])
        .await?;
    assert_eq!(
        space.item_at(&rewound, "notes/a.md").await?,
        Some(uuid!(10)?)
    );
    assert_eq!(
        space.file(&rewound, "ref.md")?,
        space.file(&deleted, "ref.md")?,
        "the referrer is untouched"
    );
    Ok(())
}

/// A note without a heading (which OKF lint flags, rule L1) whose body links to `target`.
fn linking(item: u32, at: &str, target: &str) -> Fallible<TreeEdit> {
    note(item, at, &format!("See [the plan]({target})."))
}

/// The paths OKF lint (`okf_lint_*`) located findings at, in order, each once.
fn linted(warnings: &[Warning]) -> Vec<&str> {
    let mut located: Vec<&str> = warnings
        .iter()
        .filter(|warning| warning.code.starts_with("okf_lint"))
        .filter_map(|warning| warning.location.as_deref())
        .collect();
    located.sort_unstable();
    located.dedup();
    located
}

/// The production edit check of a write that touched `touched`, as core builds it.
fn edit_check(touched: &[&str]) -> Fallible<Arc<dyn CandidateCheck>> {
    let touched = touched
        .iter()
        .map(|at| path(at))
        .collect::<Fallible<Vec<_>>>()?;
    Ok(Arc::new(EditCheck::new(&touched, None)))
}

/// A workspace whose plan (item 10, `notes/plan.md`) is linked from `notes/a-ref.md` (item 11)
/// and `refs/b-ref.md` (item 12); `notes/bystander.md` (item 13) links nowhere. The three
/// other notes have no heading, so OKF lint flags each. Returns the revision that holds them.
async fn linked_plan(workspace: &Workspace) -> Fallible<Revision> {
    Ok(workspace
        .commit(
            20,
            &workspace.initial,
            vec![
                note(10, "notes/plan.md", "# Plan\n\nThe plan.")?,
                linking(11, "notes/a-ref.md", "plan.md")?,
                linking(12, "refs/b-ref.md", "../notes/plan.md")?,
                note(13, "notes/bystander.md", "Nothing to see.")?,
            ],
        )
        .await?)
}

/// R-I12: a move tells the check the files whose links it rewrote, in path order, each once,
/// without the moved item's own two paths; and the production edit check, which the caller
/// built from those two paths alone, then reports the lint of those files and of no other.
#[tokio::test]
async fn a_move_tells_the_check_the_referrers_it_rewrote_and_their_lint_is_reported() -> TestResult
{
    let workspace = Workspace::new().await?;
    let seeded = linked_plan(&workspace).await?;
    let recording = Recording::over(edit_check(&["notes/plan.md", "archive/plan.md"])?);
    let moved_plan = workspace
        .commit_with(
            21,
            &seeded,
            vec![moved(10, "archive/plan.md")?],
            recording.clone(),
        )
        .await?;
    assert_eq!(
        recording.seen()?,
        [vec![
            "notes/a-ref.md".to_owned(),
            "refs/b-ref.md".to_owned()
        ]],
        "the check ran once, told the two referrers"
    );
    // The links did follow the plan: these are files the store rewrote.
    let rewritten = workspace.file(&moved_plan.revision, "notes/a-ref.md")?;
    assert!(
        String::from_utf8(rewritten.unwrap_or_default())?.contains("](../archive/plan.md)"),
        "the referrer's link was rewritten"
    );
    let lint = linted(&moved_plan.warnings);
    assert!(
        lint.contains(&"notes/a-ref.md") && lint.contains(&"refs/b-ref.md"),
        "the referrers' lint is reported: {:?}",
        moved_plan.warnings
    );
    assert!(
        !lint.contains(&"notes/bystander.md"),
        "a file nothing was done to is not: {:?}",
        moved_plan.warnings
    );
    Ok(())
}

/// R-I12: a commit that moves nothing tells the check nothing was rewritten: a new note, and
/// a move of the plan to the path it holds.
#[tokio::test]
async fn a_commit_that_moves_nothing_tells_the_check_no_rewritten_path() -> TestResult {
    let workspace = Workspace::new().await?;
    let seeded = linked_plan(&workspace).await?;
    let recording = Recording::over(edit_check(&["notes/new.md"])?);
    let edited = workspace
        .commit_with(
            21,
            &seeded,
            vec![
                note(14, "notes/new.md", "# New\n\nA new note.")?,
                moved(10, "notes/plan.md")?,
            ],
            recording.clone(),
        )
        .await?;
    assert_eq!(recording.seen()?, [Vec::<String>::new()]);
    let lint = linted(&edited.warnings);
    assert!(
        !lint.contains(&"notes/a-ref.md") && !lint.contains(&"refs/b-ref.md"),
        "no referrer was touched: {:?}",
        edited.warnings
    );
    Ok(())
}

/// R-I12: a referrer the same commit removes after the move is not reported: it is no file of
/// the candidate.
#[tokio::test]
async fn a_referrer_a_later_edit_of_the_commit_removed_is_not_reported() -> TestResult {
    let workspace = Workspace::new().await?;
    let seeded = linked_plan(&workspace).await?;
    let recording = Recording::over(Arc::new(Accept));
    workspace
        .commit_with(
            21,
            &seeded,
            vec![
                moved(10, "archive/plan.md")?,
                TreeEdit::DeleteItem {
                    item_id: uuid!(11)?,
                },
            ],
            recording.clone(),
        )
        .await?;
    assert_eq!(recording.seen()?, [vec!["refs/b-ref.md".to_owned()]]);
    Ok(())
}

/// R-I12, a Rewind's move back: the item moved since the target, so `RestoreItem` moves it
/// back and rewrites the links that followed it. The check is told those files (and not the
/// private path the item is parked at on the way), and their lint is reported.
#[tokio::test]
async fn a_rewind_that_moves_an_item_back_tells_the_check_the_referrers_it_rewrote() -> TestResult {
    let workspace = Workspace::new().await?;
    let seeded = linked_plan(&workspace).await?;
    let moved_away = workspace
        .commit(21, &seeded, vec![moved(10, "archive/plan.md")?])
        .await?;
    let recording = Recording::over(edit_check(&["notes/plan.md", "archive/plan.md"])?);
    let rewound = workspace
        .commit_with(
            22,
            &moved_away,
            vec![restore(10, &seeded)?],
            recording.clone(),
        )
        .await?;
    assert_eq!(
        recording.seen()?,
        [vec![
            "notes/a-ref.md".to_owned(),
            "refs/b-ref.md".to_owned()
        ]]
    );
    assert_eq!(
        workspace.file(&rewound.revision, "notes/a-ref.md")?,
        workspace.file(&seeded, "notes/a-ref.md")?,
        "the referrer points at the restored path again"
    );
    let lint = linted(&rewound.warnings);
    assert!(
        lint.contains(&"notes/a-ref.md") && lint.contains(&"refs/b-ref.md"),
        "the referrers' lint is reported: {:?}",
        rewound.warnings
    );
    assert!(
        !lint.contains(&"notes/bystander.md"),
        "a file nothing was done to is not: {:?}",
        rewound.warnings
    );
    Ok(())
}

/// R-I12 with R-I14: the Rewind of an item deleted since, as core composes it (put back at its
/// last path, moved to its path at the target, restored from the target). The `MoveItem` in
/// the middle rewrites the referrers, and the check is told them.
#[tokio::test]
async fn a_rewind_of_a_deleted_item_tells_the_check_the_referrers_its_move_rewrote() -> TestResult {
    let workspace = Workspace::new().await?;
    let seeded = linked_plan(&workspace).await?;
    let moved_away = workspace
        .commit(21, &seeded, vec![moved(10, "archive/plan.md")?])
        .await?;
    let deleted = workspace
        .commit(
            22,
            &moved_away,
            vec![TreeEdit::DeleteItem {
                item_id: uuid!(10)?,
            }],
        )
        .await?;
    let recording = Recording::over(edit_check(&["notes/plan.md"])?);
    let rewound = workspace
        .commit_with(
            23,
            &deleted,
            vec![
                restore(10, &moved_away)?,
                moved(10, "notes/plan.md")?,
                restore(10, &seeded)?,
            ],
            recording.clone(),
        )
        .await?;
    assert_eq!(
        recording.seen()?,
        [vec![
            "notes/a-ref.md".to_owned(),
            "refs/b-ref.md".to_owned()
        ]]
    );
    assert_eq!(
        workspace.file(&rewound.revision, "notes/a-ref.md")?,
        workspace.file(&seeded, "notes/a-ref.md")?,
        "the referrer points at the restored path again"
    );
    let lint = linted(&rewound.warnings);
    assert!(
        lint.contains(&"notes/a-ref.md") && lint.contains(&"refs/b-ref.md"),
        "the referrers' lint is reported: {:?}",
        rewound.warnings
    );
    assert!(
        !lint.contains(&"notes/bystander.md"),
        "a file nothing was done to is not: {:?}",
        rewound.warnings
    );
    Ok(())
}

/// R-I12, the one rewritten file that is left out: a Markdown file that is no item, at a path
/// that is no workspace path (not in Unicode NFC), which only a commit made outside the store
/// can hold. The move rewrites its link like any referrer's, and succeeds; the check is told
/// the referrers that have a workspace path, and not this one.
#[tokio::test]
async fn a_rewritten_file_whose_path_is_no_workspace_path_is_left_out_of_what_the_check_is_told()
-> TestResult {
    let workspace = Workspace::new().await?;
    linked_plan(&workspace).await?;
    // `e` followed by a combining acute accent: the decomposed spelling of `é`.
    let decomposed = "notes/cafe\u{301}.md";
    assert!(path(decomposed).is_err(), "not a workspace path");
    let planted = workspace.raw_upsert(
        decomposed,
        b"---\ntype: Note\ntitle: Cafe\n---\n\n# Cafe\n\nSee [the plan](plan.md).\n",
        30,
    )?;
    let recording = Recording::over(Arc::new(Accept));
    let moved_plan = workspace
        .commit_with(
            21,
            &planted,
            vec![moved(10, "archive/plan.md")?],
            recording.clone(),
        )
        .await?;
    let rewritten = workspace.file(&moved_plan.revision, decomposed)?;
    assert!(
        String::from_utf8(rewritten.unwrap_or_default())?.contains("](../archive/plan.md)"),
        "the file's link was rewritten like any referrer's"
    );
    assert_eq!(
        recording.seen()?,
        [vec![
            "notes/a-ref.md".to_owned(),
            "refs/b-ref.md".to_owned()
        ]],
        "told without the file that has no workspace path"
    );
    Ok(())
}

/// The plan's workspace (`linked_plan`) with a second linked item: `notes/other.md` (item 14),
/// linked from `notes/c-ref.md` (item 15), which has no heading either. Returns the revision
/// that holds both.
async fn two_linked_items(workspace: &Workspace) -> Fallible<Revision> {
    let seeded = linked_plan(workspace).await?;
    Ok(workspace
        .commit(
            21,
            &seeded,
            vec![
                note(14, "notes/other.md", "# Other\n\nThe other.")?,
                linking(15, "notes/c-ref.md", "other.md")?,
            ],
        )
        .await?)
}

/// R-I12, two moves in one commit, each with its own referrers: the check is told the referrers
/// of both, in path order, each once, and the lint of both sets is reported. (Reviewer probe
/// PR1, adopted.)
#[tokio::test]
async fn two_moves_in_one_commit_tell_the_check_both_sets_of_referrers() -> TestResult {
    let workspace = Workspace::new().await?;
    let seeded = two_linked_items(&workspace).await?;
    let recording = Recording::over(edit_check(&[
        "notes/plan.md",
        "archive/plan.md",
        "notes/other.md",
        "archive/other.md",
    ])?);
    let moved_both = workspace
        .commit_with(
            22,
            &seeded,
            vec![
                moved(10, "archive/plan.md")?,
                moved(14, "archive/other.md")?,
            ],
            recording.clone(),
        )
        .await?;
    assert_eq!(
        recording.seen()?,
        [vec![
            "notes/a-ref.md".to_owned(),
            "notes/c-ref.md".to_owned(),
            "refs/b-ref.md".to_owned()
        ]]
    );
    let lint = linted(&moved_both.warnings);
    assert!(
        lint.contains(&"notes/a-ref.md")
            && lint.contains(&"notes/c-ref.md")
            && lint.contains(&"refs/b-ref.md"),
        "the lint of both moves' referrers is reported: {:?}",
        moved_both.warnings
    );
    assert!(
        !lint.contains(&"notes/bystander.md"),
        "a file nothing was done to is not: {:?}",
        moved_both.warnings
    );
    Ok(())
}

/// R-I12, a Rewind of two items that each moved since the target, in one commit (one run of
/// `RestoreItem` edits, as core sends for several selected items). Each has its own referrer,
/// and the check is told both. (Reviewer probe PR7, adopted.)
#[tokio::test]
async fn a_rewind_of_two_moved_items_tells_the_check_both_sets_of_referrers() -> TestResult {
    let workspace = Workspace::new().await?;
    let seeded = two_linked_items(&workspace).await?;
    let moved_away = workspace
        .commit(
            22,
            &seeded,
            vec![
                moved(10, "archive/plan.md")?,
                moved(14, "archive/other.md")?,
            ],
        )
        .await?;
    let recording = Recording::over(Arc::new(Accept));
    workspace
        .commit_with(
            23,
            &moved_away,
            vec![restore(10, &seeded)?, restore(14, &seeded)?],
            recording.clone(),
        )
        .await?;
    assert_eq!(
        recording.seen()?,
        [vec![
            "notes/a-ref.md".to_owned(),
            "notes/c-ref.md".to_owned(),
            "refs/b-ref.md".to_owned()
        ]]
    );
    Ok(())
}

/// R-I12, a move inside a proposal candidate: the candidate's check is told the referrers, as
/// a commit's is. (Reviewer probe PR2, adopted.)
#[tokio::test]
async fn a_move_inside_a_proposal_candidate_tells_the_check_the_referrers() -> TestResult {
    let workspace = Workspace::new().await?;
    let seeded = linked_plan(&workspace).await?;
    let recording = Recording::over(edit_check(&["notes/plan.md", "archive/plan.md"])?);
    let candidate = workspace
        .storage
        .versions()
        .create_candidate(
            &workspace.scope,
            uuid!(50)?,
            CandidateChanges {
                mutation_id: uuid!(51)?,
                base: seeded.clone(),
                author: person("agent"),
                message: "Propose a move".to_owned(),
                edits: vec![moved(10, "archive/plan.md")?],
            },
            recording.clone(),
        )
        .await?
        .revision;
    assert_ne!(candidate, seeded);
    assert_eq!(
        recording.seen()?,
        [vec![
            "notes/a-ref.md".to_owned(),
            "refs/b-ref.md".to_owned()
        ]]
    );
    Ok(())
}

/// R-I12, review B2: the check is told each referrer as the workspace path the tree spells,
/// capital letters included and with forward slashes, because the edit check compares it as an
/// exact string with the location of a lint finding. A referrer in a nested, capitalised
/// folder is told so, and its lint is reported. (Reviewer probe PR3, adopted.)
#[tokio::test]
async fn a_mixed_case_referrer_is_told_as_the_tree_spells_it() -> TestResult {
    let workspace = Workspace::new().await?;
    let seeded = workspace
        .commit(
            20,
            &workspace.initial,
            vec![
                note(10, "Notes/Plan.md", "# Plan\n\nThe plan.")?,
                linking(12, "Refs/Deep/B-Ref.md", "../../Notes/Plan.md")?,
            ],
        )
        .await?;
    let recording = Recording::over(edit_check(&["Notes/Plan.md", "Archive/Plan.md"])?);
    let moved_plan = workspace
        .commit_with(
            21,
            &seeded,
            vec![moved(10, "Archive/Plan.md")?],
            recording.clone(),
        )
        .await?;
    assert_eq!(recording.seen()?, [vec!["Refs/Deep/B-Ref.md".to_owned()]]);
    assert!(
        linted(&moved_plan.warnings).contains(&"Refs/Deep/B-Ref.md"),
        "the referrer's lint is reported at the path the check was told: {:?}",
        moved_plan.warnings
    );
    Ok(())
}

/// R-I12, a referrer the first move rewrote and a later edit of the same commit moved away:
/// its old path is no file of the candidate and is not told; its new path is the caller's own
/// to name. (Reviewer probe PR4, adopted.)
#[tokio::test]
async fn a_referrer_rewritten_and_then_moved_is_not_told_at_its_old_path() -> TestResult {
    let workspace = Workspace::new().await?;
    let seeded = linked_plan(&workspace).await?;
    let recording = Recording::over(Arc::new(Accept));
    workspace
        .commit_with(
            21,
            &seeded,
            vec![
                moved(10, "archive/plan.md")?,
                moved(11, "archive/a-ref.md")?,
            ],
            recording.clone(),
        )
        .await?;
    assert_eq!(recording.seen()?, [vec!["refs/b-ref.md".to_owned()]]);
    Ok(())
}

/// R-I12, a file rewritten and then rewritten back is named: an item moved away and back in
/// one commit. Each referrer was rewritten twice and ends with the bytes it began with, and
/// the check is still told both: `rewritten` names the files the store rewrote while it
/// applied the commit's edits, not the files whose bytes differ at the end. (Reviewer probe
/// PR6, adopted; review note N10.)
#[tokio::test]
async fn a_move_away_and_back_in_one_commit_tells_the_referrers_it_rewrote_twice() -> TestResult {
    let workspace = Workspace::new().await?;
    let seeded = linked_plan(&workspace).await?;
    let recording = Recording::over(Arc::new(Accept));
    let there_and_back = workspace
        .commit_with(
            21,
            &seeded,
            vec![moved(10, "archive/plan.md")?, moved(10, "notes/plan.md")?],
            recording.clone(),
        )
        .await?;
    for referrer in ["notes/a-ref.md", "refs/b-ref.md"] {
        let before = workspace.file(&seeded, referrer)?;
        assert!(before.is_some(), "{referrer} is a file of the workspace");
        assert_eq!(
            workspace.file(&there_and_back.revision, referrer)?,
            before,
            "{referrer} ends as it began"
        );
    }
    assert_eq!(
        recording.seen()?,
        [vec![
            "notes/a-ref.md".to_owned(),
            "refs/b-ref.md".to_owned()
        ]],
        "told the referrers although their bytes did not change"
    );
    Ok(())
}

// The sweep-1 reviewer's probes r1 to r11 (sweep 2a): the edits core composes for a Rewind of
// an item deleted since its target (R-I14: put back from the last revision that held it, moved
// to its path at the target, restored from the target), applied by real storage. Core finds the
// last holder as the first parent of the newest commit the item-filtered log names
// (`last_holder`, asserted in each test); the edits are written out as core composes them,
// since a storage test cannot run core's application.

/// `DeleteItem` of `item`.
fn deleted(item: u32) -> Fallible<TreeEdit> {
    Ok(TreeEdit::DeleteItem {
        item_id: uuid!(item)?,
    })
}

/// `EditItem` of `item` to `body`, with no properties beyond its header.
fn edited(item: u32, body: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::EditItem {
        item_id: uuid!(item)?,
        body: body.to_owned(),
        properties: BTreeMap::new(),
    })
}

/// The last revision before `base` that held `item`, as core finds it: the first parent of the
/// newest commit the item-filtered log names, from `base`.
async fn last_holder(space: &Workspace, base: &Revision, item: u32) -> Fallible<Revision> {
    let newest = space
        .storage
        .versions()
        .log(
            &space.scope,
            LogQuery {
                tip: base.clone(),
                item_id: Some(uuid!(item)?),
                page: Page {
                    cursor: None,
                    limit: 1,
                },
            },
        )
        .await?;
    let commit = some(
        newest.commits.into_iter().next(),
        "the newest commit that changed the item",
    )?;
    Ok(some(commit.parents.into_iter().next(), "its first parent")?)
}

/// The three edits of a Rewind of `item`, deleted since: put back from `last`, moved to `to`
/// (its path at `target`), restored from `target`.
fn rewound_deleted(
    item: u32,
    last: &Revision,
    to: &str,
    target: &Revision,
) -> Fallible<Vec<TreeEdit>> {
    Ok(vec![
        restore(item, last)?,
        moved(item, to)?,
        restore(item, target)?,
    ])
}

/// The files of `revision` whose path holds a private name (`okf-jawn-`).
fn private_names(space: &Workspace, revision: &Revision) -> Fallible<Vec<String>> {
    Ok(space
        .files_under(revision, "")?
        .into_iter()
        .filter(|file| file.contains("okf-jawn-"))
        .collect())
}

/// Probe r1: an item moved twice and then deleted, with an unrelated commit since. The delete is
/// the newest commit that changed it, and its parent held it last; the three edits put it back
/// at its first path with the target's bytes, its referrer's bytes are the target's again (the
/// move rewrote the link back), and no later path and no private name is left.
#[tokio::test]
async fn r1_an_item_moved_twice_then_deleted_is_rewound_with_its_referrer() -> TestResult {
    let space = Workspace::new().await?;
    let c2 = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "a.md", "# The item\n\nOne.\n")?,
                note(11, "notes/ref.md", "See [it](../a.md#the-item).\n")?,
            ],
        )
        .await?;
    let c3 = space.commit(3, &c2, vec![moved(10, "b.md")?]).await?;
    let c4 = space.commit(4, &c3, vec![moved(10, "deep/c.md")?]).await?;
    let c5 = space.commit(5, &c4, vec![deleted(10)?]).await?;
    let c6 = space
        .commit(6, &c5, vec![note(12, "other.md", "Unrelated.\n")?])
        .await?;
    assert_eq!(last_holder(&space, &c6, 10).await?, c4);
    let c7 = space
        .commit(7, &c6, rewound_deleted(10, &c4, "a.md", &c2)?)
        .await?;
    for at in ["a.md", "notes/ref.md"] {
        assert!(space.file(&c2, at)?.is_some(), "{at}");
        assert_eq!(space.file(&c7, at)?, space.file(&c2, at)?, "{at}");
    }
    for gone in ["b.md", "deep/c.md"] {
        assert_eq!(space.file(&c7, gone)?, None, "{gone}");
    }
    assert_eq!(private_names(&space, &c7)?, Vec::<String>::new());
    Ok(())
}

/// Probe r1 (its r3 part): when the target is the last revision that held the deleted item, the
/// Rewind is one `RestoreItem` from it, and the links are right as they are: the item and its
/// referrer come back with the target's bytes.
#[tokio::test]
async fn r1_a_rewind_to_the_revision_that_held_the_item_last_is_one_restore() -> TestResult {
    let space = Workspace::new().await?;
    let d2 = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "a.md", "# The item\n\nOne.\n")?,
                note(11, "notes/ref.md", "See [it](../a.md#the-item).\n")?,
            ],
        )
        .await?;
    let d3 = space.commit(3, &d2, vec![moved(10, "deep/c.md")?]).await?;
    let d4 = space.commit(4, &d3, vec![deleted(10)?]).await?;
    assert_eq!(last_holder(&space, &d4, 10).await?, d3);
    let d5 = space.commit(5, &d4, vec![restore(10, &d3)?]).await?;
    for at in ["deep/c.md", "notes/ref.md"] {
        assert!(space.file(&d3, at)?.is_some(), "{at}");
        assert_eq!(space.file(&d5, at)?, space.file(&d3, at)?, "{at}");
    }
    Ok(())
}

/// Probe r1: an item deleted, rewound, then moved and deleted again. The newest commit that
/// changed it is the second delete, and the three edits from its parent put the referrer back
/// as the target holds it, with no file at the second last path.
#[tokio::test]
async fn r1_an_item_deleted_rewound_and_deleted_again_is_rewound_from_its_last_delete() -> TestResult
{
    let space = Workspace::new().await?;
    let d2 = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "a.md", "# The item\n\nOne.\n")?,
                note(11, "notes/ref.md", "See [it](../a.md#the-item).\n")?,
            ],
        )
        .await?;
    let d3 = space.commit(3, &d2, vec![moved(10, "deep/c.md")?]).await?;
    let d4 = space.commit(4, &d3, vec![deleted(10)?]).await?;
    let d5 = space.commit(5, &d4, vec![restore(10, &d3)?]).await?;
    let d6 = space.commit(6, &d5, vec![moved(10, "third.md")?]).await?;
    let d7 = space.commit(7, &d6, vec![deleted(10)?]).await?;
    assert_eq!(last_holder(&space, &d7, 10).await?, d6);
    let d8 = space
        .commit(8, &d7, rewound_deleted(10, &d6, "a.md", &d2)?)
        .await?;
    for at in ["a.md", "notes/ref.md"] {
        assert_eq!(space.file(&d8, at)?, space.file(&d2, at)?, "{at}");
    }
    assert_eq!(space.file(&d8, "third.md")?, None);
    assert_eq!(private_names(&space, &d8)?, Vec::<String>::new());
    Ok(())
}

/// Probe r2: an item deleted in a commit that also deleted the other item of its old folder and
/// edited its referrer. The three edits put it back in that folder with the target's bytes, and
/// the referrer keeps its later edit, with its link pointing at the item again.
#[tokio::test]
async fn r2_an_item_deleted_with_its_folder_in_a_shared_commit_is_rewound() -> TestResult {
    let space = Workspace::new().await?;
    let c2 = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "box/x.md", "# X\n")?,
                note(12, "box/y.md", "# Y\n")?,
                note(11, "ref.md", "[x](box/x.md) and [y](box/y.md).\n")?,
            ],
        )
        .await?;
    let c3 = space
        .commit(3, &c2, vec![moved(10, "crate/x2.md")?])
        .await?;
    let c4 = space
        .commit(
            4,
            &c3,
            vec![
                deleted(10)?,
                deleted(12)?,
                edited(11, "Edited. [x](crate/x2.md) and [y](box/y.md).\n")?,
            ],
        )
        .await?;
    assert_eq!(last_holder(&space, &c4, 10).await?, c3);
    let c5 = space
        .commit(5, &c4, rewound_deleted(10, &c3, "box/x.md", &c2)?)
        .await?;
    let referrer = body_of(&space, &c5, 11).await?;
    assert!(referrer.contains("[x](box/x.md)"), "{referrer}");
    assert!(referrer.contains("Edited."), "{referrer}");
    assert_eq!(space.file(&c5, "box/x.md")?, space.file(&c2, "box/x.md")?);
    assert_eq!(space.file(&c5, "crate/x2.md")?, None);
    Ok(())
}

/// Probe r4: referrers moved, edited and deleted since the item was. The moved referrer's link
/// is written from its new folder, the edited one keeps its edit with the link to the item's
/// restored path, and the deleted one stays deleted.
#[tokio::test]
async fn r4_referrers_moved_edited_and_deleted_since_follow_the_rewound_item() -> TestResult {
    let space = Workspace::new().await?;
    let c2 = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "old.md", "# The item\n")?,
                note(11, "notes/moved.md", "Moved [it](../old.md).\n")?,
                note(12, "notes/edited.md", "Edited [it](../old.md).\n")?,
                note(13, "notes/gone.md", "Gone [it](../old.md).\n")?,
            ],
        )
        .await?;
    let c3 = space.commit(3, &c2, vec![moved(10, "new.md")?]).await?;
    let c4 = space.commit(4, &c3, vec![deleted(10)?]).await?;
    let c5 = space
        .commit(
            5,
            &c4,
            vec![
                moved(11, "elsewhere/deep/moved.md")?,
                edited(12, "Changed since. [it](../new.md).\n")?,
                deleted(13)?,
            ],
        )
        .await?;
    assert_eq!(last_holder(&space, &c5, 10).await?, c3);
    let c6 = space
        .commit(6, &c5, rewound_deleted(10, &c3, "old.md", &c2)?)
        .await?;
    let moved_referrer = body_of(&space, &c6, 11).await?;
    let edited_referrer = body_of(&space, &c6, 12).await?;
    assert!(
        moved_referrer.contains("](../../old.md)"),
        "{moved_referrer}"
    );
    assert!(
        edited_referrer.contains("Changed since. [it](../old.md)"),
        "{edited_referrer}"
    );
    assert_eq!(space.file(&c6, "notes/gone.md")?, None);
    assert_eq!(space.file(&c6, "old.md")?, space.file(&c2, "old.md")?);
    Ok(())
}

/// Probe r5: an item that links to itself (beside it with an anchor, through `./`, and a pure
/// anchor) comes back from the three edits with the target's bytes, nothing left at its last
/// path.
#[tokio::test]
async fn r5_an_item_that_links_to_itself_is_rewound_with_its_own_bytes() -> TestResult {
    let space = Workspace::new().await?;
    let c2 = space
        .commit(
            2,
            &space.initial,
            vec![note(
                10,
                "old.md",
                "# Me\n\n[self](old.md#me), [dot](./old.md) and [bare](#me).\n",
            )?],
        )
        .await?;
    let c3 = space.commit(3, &c2, vec![moved(10, "sub/new.md")?]).await?;
    let c4 = space.commit(4, &c3, vec![deleted(10)?]).await?;
    assert_eq!(last_holder(&space, &c4, 10).await?, c3);
    let c5 = space
        .commit(5, &c4, rewound_deleted(10, &c3, "old.md", &c2)?)
        .await?;
    assert_eq!(space.file(&c5, "old.md")?, space.file(&c2, "old.md")?);
    assert_eq!(space.file(&c5, "sub/new.md")?, None);
    assert_eq!(private_names(&space, &c5)?, Vec::<String>::new());
    Ok(())
}

/// Probe r6: a referrer's links in every form (inline, with a fragment, in angle brackets, with
/// a title, reference style, an image) after the three edits.
#[tokio::test]
async fn r6_every_link_form_of_a_referrer_follows_the_rewound_item() -> TestResult {
    let forms = "Inline [a](../old.md).\n\
                 Fragment [b](../old.md#the-item).\n\
                 Angle [c](<../old.md>).\n\
                 Titled [d](../old.md \"The title\").\n\
                 Reference [e][target].\n\
                 Image ![f](../old.md).\n\
                 \n\
                 [target]: ../old.md\n";
    let space = Workspace::new().await?;
    let c2 = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "old.md", "# The item\n")?,
                note(11, "notes/ref.md", forms)?,
            ],
        )
        .await?;
    let c3 = space.commit(3, &c2, vec![moved(10, "new.md")?]).await?;
    let c4 = space.commit(4, &c3, vec![deleted(10)?]).await?;
    assert_eq!(last_holder(&space, &c4, 10).await?, c3);
    let c5 = space
        .commit(5, &c4, rewound_deleted(10, &c3, "old.md", &c2)?)
        .await?;
    let after = body_of(&space, &c5, 11).await?;
    assert!(after.contains("Inline [a](../old.md)"), "{after}");
    assert!(
        after.contains("Fragment [b](../old.md#the-item)"),
        "{after}"
    );
    Ok(())
}

/// Probe r7: a source card with a correction, moved, then given supplied text, then deleted. The
/// three edits put the card, its appearance and its correction back as the target holds them,
/// with no supplied text (absent at the target) and nothing at the later path; the referrer's
/// link points at the card again.
#[tokio::test]
async fn r7_a_card_with_companions_is_rewound_with_them() -> TestResult {
    let space = Workspace::new().await?;
    let c2 = space
        .commit(
            2,
            &space.initial,
            vec![
                card(20, "application/pdf", 'a')?,
                note(11, "ref.md", "[scan](sources/scan-pdf.md).\n")?,
            ],
        )
        .await?;
    let c3 = space
        .commit(3, &c2, vec![correction(20, 'a', "Fixed A.")?])
        .await?;
    let c4 = space
        .commit(4, &c3, vec![moved(20, "sources/renamed.md")?])
        .await?;
    let c5 = space.commit(5, &c4, vec![supplied(20, 'a')?]).await?;
    let c6 = space.commit(6, &c5, vec![deleted(20)?]).await?;
    assert_eq!(space.file(&c6, &appearance_file(20))?, None);
    assert_eq!(last_holder(&space, &c6, 20).await?, c5);
    let c7 = space
        .commit(7, &c6, rewound_deleted(20, &c5, CARD, &c3)?)
        .await?;
    for file in [
        CARD.to_owned(),
        appearance_file(20),
        correction_file(20, 'a'),
    ] {
        assert!(space.file(&c3, &file)?.is_some(), "{file}");
        assert_eq!(space.file(&c7, &file)?, space.file(&c3, &file)?, "{file}");
    }
    assert_eq!(space.file(&c7, &supplied_file(20))?, None);
    assert_eq!(space.file(&c7, "sources/renamed.md")?, None);
    let referrer = body_of(&space, &c7, 11).await?;
    assert!(
        referrer.contains("[scan](sources/scan-pdf.md)"),
        "{referrer}"
    );
    Ok(())
}

/// `old.md` (item 10) with a referrer, moved to `new.md`, then deleted: the three revisions.
async fn moved_then_deleted(space: &Workspace) -> Fallible<(Revision, Revision, Revision)> {
    let at_old = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "old.md", "# The item\n\nFirst text.\n")?,
                note(11, "notes/ref.md", "See [it](../old.md#the-item).\n")?,
            ],
        )
        .await?;
    let at_new = space.commit(3, &at_old, vec![moved(10, "new.md")?]).await?;
    let gone = space.commit(4, &at_new, vec![deleted(10)?]).await?;
    Ok((at_old, at_new, gone))
}

/// Probe r8, its first step failing: another item took the last path since, so putting the item
/// back there is `Conflict`, and nothing is written.
#[tokio::test]
async fn r8_a_rewind_whose_last_path_was_taken_since_writes_nothing() -> TestResult {
    let space = Workspace::new().await?;
    let (at_old, at_new, gone) = moved_then_deleted(&space).await?;
    let base = space
        .commit(5, &gone, vec![note(12, "new.md", "# Another\n")?])
        .await?;
    let error = err_of(
        space
            .commit(6, &base, rewound_deleted(10, &at_new, "old.md", &at_old)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    assert_eq!(space.storage.versions().head(&space.scope).await?, base);
    Ok(())
}

/// Probe r8, its second step failing: another item holds the target's path now, so the move is
/// `Conflict`, as one plain `RestoreItem` is, and nothing is written.
#[tokio::test]
async fn r8_a_rewind_onto_a_path_another_item_holds_writes_nothing() -> TestResult {
    let space = Workspace::new().await?;
    let (at_old, at_new, gone) = moved_then_deleted(&space).await?;
    let base = space
        .commit(5, &gone, vec![note(12, "old.md", "# Another\n")?])
        .await?;
    for (mutation, edits) in [
        (6, rewound_deleted(10, &at_new, "old.md", &at_old)?),
        (7, vec![restore(10, &at_old)?]),
    ] {
        let error = err_of(space.commit(mutation, &base, edits).await)?;
        assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
        assert_eq!(space.storage.versions().head(&space.scope).await?, base);
    }
    Ok(())
}

/// Probe r8, its third step failing: the target is a revision the repository does not hold, so
/// the restore from it is refused, and nothing is written.
#[tokio::test]
async fn r8_a_rewind_to_a_revision_the_repository_lacks_writes_nothing() -> TestResult {
    let space = Workspace::new().await?;
    let (_, at_new, gone) = moved_then_deleted(&space).await?;
    let unknown = Revision::try_from("f".repeat(40))?;
    let error = err_of(
        space
            .commit(6, &gone, rewound_deleted(10, &at_new, "old.md", &unknown)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound, "{}", error.message);
    assert_eq!(space.storage.versions().head(&space.scope).await?, gone);
    Ok(())
}

/// Probe r8, the check refusing the composed tree: nothing is written; the same mutation then
/// accepted lands once, and a third call replays it.
#[tokio::test]
async fn r8_a_refused_rewind_lands_once_when_accepted_and_then_replays() -> TestResult {
    let space = Workspace::new().await?;
    let (at_old, at_new, gone) = moved_then_deleted(&space).await?;
    let edits = rewound_deleted(10, &at_new, "old.md", &at_old)?;
    let refused = err_of(
        space
            .commit_with(6, &gone, edits.clone(), Arc::new(Refuses))
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput, "{}", refused.message);
    assert_eq!(space.storage.versions().head(&space.scope).await?, gone);
    let landed = space
        .commit_with(6, &gone, edits.clone(), Arc::new(Accept))
        .await?;
    assert!(!landed.replayed);
    let replayed = space.commit_with(6, &gone, edits, Arc::new(Accept)).await?;
    assert!(replayed.replayed);
    assert_eq!(replayed.revision, landed.revision);
    assert_eq!(
        space.storage.versions().head(&space.scope).await?,
        landed.revision
    );
    Ok(())
}

/// Probe r9 (core's B1, fixed in core): an item renamed in letter case only and then deleted.
/// Its last path is its target path in another spelling, so core sends one `RestoreItem` from
/// the target, which puts it back at the target's spelling with the target's bytes.
#[tokio::test]
async fn r9_an_item_renamed_in_letter_case_only_and_then_deleted_is_rewound() -> TestResult {
    let space = Workspace::new().await?;
    let c2 = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "report.md", "# The report\n")?,
                note(11, "ref.md", "[r](report.md).\n")?,
            ],
        )
        .await?;
    let c3 = space.commit(3, &c2, vec![moved(10, "Report.md")?]).await?;
    let c4 = space.commit(4, &c3, vec![deleted(10)?]).await?;
    assert_eq!(last_holder(&space, &c4, 10).await?, c3);
    let c5 = space.commit(5, &c4, vec![restore(10, &c2)?]).await?;
    assert_eq!(space.item_at(&c5, "report.md").await?, Some(uuid!(10)?));
    assert_eq!(space.file(&c5, "report.md")?, space.file(&c2, "report.md")?);
    let files = space.files_under(&c5, "")?;
    assert!(!files.iter().any(|file| file == "Report.md"), "{files:?}");
    Ok(())
}

/// Probe r10: a file where a folder of the last path is needed. Plain edits cannot reach it (the
/// folder outlives its item through its `index.md`), so the tree is changed through Git: the
/// folder's index removed and a file written at the folder's name. Putting the item back at its
/// last path is then `Conflict` naming it, and nothing is written (sweep 2a, item 22: storage
/// refuses the case core's `taken` does not see).
#[tokio::test]
async fn r10_a_file_where_a_folder_of_the_last_path_is_needed_refuses_the_rewind() -> TestResult {
    let space = Workspace::new().await?;
    let c2 = space
        .commit(2, &space.initial, vec![note(10, "old.md", "# The item\n")?])
        .await?;
    let c3 = space
        .commit(3, &c2, vec![moved(10, "docs.md/inner.md")?])
        .await?;
    let c4 = space.commit(4, &c3, vec![deleted(10)?]).await?;
    let refused = err_of(
        space
            .commit(5, &c4, vec![note(12, "docs.md", "# A file now\n")?])
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict, "{}", refused.message);
    space.raw_remove("docs.md/index.md", 30)?;
    let blocked = space.raw_upsert(
        "docs.md",
        b"---\ntype: Note\ntitle: A file now\n---\n\n# A file now\n",
        31,
    )?;
    let error = err_of(
        space
            .commit(6, &blocked, rewound_deleted(10, &c3, "old.md", &c2)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    assert!(
        error.message.contains("docs.md/inner.md"),
        "{}",
        error.message
    );
    assert_eq!(space.storage.versions().head(&space.scope).await?, blocked);
    Ok(())
}

/// Probe r11: a folder of another letter case holds the last name. After an item at
/// `Notes/new.md` is deleted, the folder outlives it through its `index.md`, so a note at
/// `notes/new.md` is refused as a folder spelled another way (PR #34), and the state the probe
/// went on to rewind is not reachable this way.
#[tokio::test]
async fn r11_a_note_under_a_folder_of_another_case_that_held_the_last_name_is_refused() -> TestResult
{
    let space = Workspace::new().await?;
    let c2 = space
        .commit(2, &space.initial, vec![note(10, "old.md", "# The item\n")?])
        .await?;
    let c3 = space
        .commit(3, &c2, vec![moved(10, "Notes/new.md")?])
        .await?;
    let c4 = space.commit(4, &c3, vec![deleted(10)?]).await?;
    let error = err_of(
        space
            .commit(5, &c4, vec![note(12, "notes/new.md", "# Another\n")?])
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::InvalidInput, "{}", error.message);
    assert!(error.message.contains("Notes"), "{}", error.message);
    assert_eq!(space.storage.versions().head(&space.scope).await?, c4);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
