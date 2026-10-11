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
use std::sync::Arc;

use git2::Repository;
use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, ItemId, Revision, TenantId, WorkspacePath};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_core::storage::{
    CandidateCheck, CommitChanges, NewWorkspace, Provenance, SourceCard, StorageScope, TreeEdit,
    VersionStore, WorkspaceCatalog,
};
use okf_jawn_storage::Storage;
use serde_json::json;

use check::{TestResult, err_of};

/// The identity numbered `n`, of the type the context names.
macro_rules! uuid {
    ($n:expr) => {
        serde_json::from_value(json!(uuid_text($n)))
    };
}

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// Accepts every candidate.
struct Accept;

/// A data directory with one blank workspace.
struct Workspace {
    directory: tempfile::TempDir,
    storage: Storage,
    scope: StorageScope,
    /// The catalog's first commit.
    initial: Revision,
}

/// Where every test keeps its source card.
const CARD: &str = "sources/scan-pdf.md";

impl CandidateCheck for Accept {
    fn check(&self, _root: &Path) -> Result<Vec<Warning>, ApiError> {
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

    /// Commit `edits` as mutation `mutation` on top of `expected`.
    async fn commit(
        &self,
        mutation: u32,
        expected: &Revision,
        edits: Vec<TreeEdit>,
    ) -> Result<Revision, ApiError> {
        let changes = CommitChanges {
            mutation_id: serde_json::from_value(json!(uuid_text(mutation)))
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?,
            expected_head: expected.clone(),
            author: person("ana"),
            message: format!("change {mutation}"),
            edits,
        };
        Ok(self
            .storage
            .versions()
            .commit(&self.scope, changes, Arc::new(Accept))
            .await?
            .revision)
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

#[path = "../../../tests/support/check.rs"]
mod check;
