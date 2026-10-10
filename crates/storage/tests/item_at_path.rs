//! `VersionStore::item_at_path` against real Git repositories: the item at exactly one path of
//! one revision, looked up directly in that commit's tree and summarized exactly as `list` of
//! its folder summarizes it.
//!
//! Files `list` would not report (a folder `index.md` carrying a header, a copy under `.okf/`,
//! a header-less note, a broken sibling) are written straight into the repository with `git2`,
//! as no store edit can write them.
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use git2::{Oid, Repository, Tree};
use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Revision, TenantId, WorkspacePath};
use okf_jawn_contract::item::{ItemKind, ItemStatus, ItemSummary};
use okf_jawn_core::storage::{
    CandidateCheck, CommitChanges, NewWorkspace, Page, Provenance, SourceCard, StorageScope,
    TreeEdit, VersionStore, WorkspaceCatalog,
};
use okf_jawn_storage::{GitVersions, Storage};
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

/// A data directory with one workspace seeded by `seed`.
struct Seeded {
    directory: tempfile::TempDir,
    storage: Storage,
    scope: StorageScope,
    /// The commit that wrote the seed.
    revision: Revision,
}

impl CandidateCheck for Accept {
    fn check(&self, _root: &Path) -> Result<Vec<Warning>, ApiError> {
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

const fn page() -> Page {
    Page {
        cursor: None,
        limit: 500,
    }
}

fn note(item: u32, at: &str, title: Option<&str>, kind: ItemKind) -> Fallible<TreeEdit> {
    Ok(TreeEdit::CreateItem {
        item_id: uuid!(item)?,
        path: path(at)?,
        title: title.map(str::to_owned),
        type_name: "Note".to_owned(),
        kind,
        body: format!("Body of {at}."),
        properties: BTreeMap::from([("description".to_owned(), json!(format!("about {at}")))]),
    })
}

/// A source card whose summary carries a media type and an extraction.
fn card(item: u32, at: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::WriteSourceCard(Box::new(SourceCard {
        item_id: uuid!(item)?,
        path: path(at)?,
        title: "scan.pdf".to_owned(),
        type_name: "Source".to_owned(),
        body: String::new(),
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
                "text_origin": "none",
                "corrected": false,
                "warnings": []
            }
        }))?,
    })))
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

/// Create the workspace numbered `n` in `storage`, returning its scope and first head.
async fn workspace(storage: &Storage, n: u32) -> Fallible<(StorageScope, Revision)> {
    let tenant = TenantId::try_from("local".to_owned())?;
    let created = storage
        .catalog()
        .create(
            &tenant,
            uuid!(n)?,
            NewWorkspace {
                name: format!("Workspace {n}"),
                description: String::new(),
                creator: person("ana"),
            },
        )
        .await?;
    let scope = StorageScope {
        tenant_id: tenant,
        workspace_id: created.id,
    };
    Ok((scope, created.head))
}

/// A workspace holding a note and a View (titled from its file stem) in `notes/`, a source
/// card in `sources/`, an archived deprecated note at the root, and a folder named `deck.md`.
async fn seeded() -> Fallible<Seeded> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (scope, initial) = workspace(&storage, 1).await?;
    let revision = storage
        .versions()
        .commit(
            &scope,
            changes(
                2,
                &initial,
                vec![
                    note(10, "notes/plan.md", Some("The plan"), ItemKind::Note)?,
                    note(11, "notes/chart.md", None, ItemKind::View)?,
                    card(12, "sources/scan-pdf.md")?,
                    note(13, "retired.md", Some("Retired"), ItemKind::Note)?,
                    TreeEdit::SetStatus {
                        item_id: uuid!(13)?,
                        status: Some(ItemStatus::Deprecated),
                        archived: Some(true),
                    },
                    TreeEdit::CreateFolder {
                        folder: path("deck.md")?,
                    },
                ],
            )?,
            Arc::new(Accept),
        )
        .await?
        .revision;
    Ok(Seeded {
        directory,
        storage,
        scope,
        revision,
    })
}

fn repository_path(directory: &Path, scope: &StorageScope) -> std::path::PathBuf {
    directory
        .join("repositories")
        .join(scope.tenant_id.as_str())
        .join(format!("{}.git", scope.workspace_id.0))
}

/// `tree` with the file `bytes` at `segments`, written as a new tree.
fn with_file(
    repository: &Repository,
    tree: Option<&Tree<'_>>,
    segments: &[&str],
    bytes: &[u8],
) -> Fallible<Oid> {
    let mut builder = repository.treebuilder(tree)?;
    match segments {
        [name] => {
            builder.insert(name, repository.blob(bytes)?, 0o100_644)?;
        }
        [folder, rest @ ..] => {
            let child = tree
                .and_then(|tree| tree.get_name(folder))
                .map(|entry| repository.find_tree(entry.id()))
                .transpose()?;
            let written = with_file(repository, child.as_ref(), rest, bytes)?;
            builder.insert(folder, written, 0o040_000)?;
        }
        [] => return Err("an empty path".into()),
    }
    Ok(builder.write()?)
}

/// Commit `files` on top of the head straight into the workspace repository, bypassing the
/// store, and return the new head.
fn write_raw(seeded: &Seeded, files: &[(&str, &[u8])]) -> Fallible<Revision> {
    let repository =
        Repository::open_bare(repository_path(seeded.directory.path(), &seeded.scope))?;
    let parent = repository.find_commit(repository.refname_to_id("refs/heads/main")?)?;
    let mut tree = parent.tree()?;
    for (at, bytes) in files {
        let segments: Vec<&str> = at.split('/').collect();
        let written = with_file(&repository, Some(&tree), &segments, bytes)?;
        tree = repository.find_tree(written)?;
    }
    let who = git2::Signature::now("raw", "raw@example.invalid")?;
    let commit = repository.commit(
        Some("refs/heads/main"),
        &who,
        &who,
        "raw files",
        &tree,
        &[&parent],
    )?;
    Ok(Revision::try_from(commit.to_string())?)
}

/// Every item `list` reports in `folder` at `revision`.
async fn listed(
    versions: &GitVersions,
    scope: &StorageScope,
    revision: &Revision,
    folder: Option<&str>,
) -> Fallible<Vec<ItemSummary>> {
    let folder = folder.map(path).transpose()?;
    Ok(versions
        .list(scope, revision, folder.as_ref(), page())
        .await?
        .items)
}

async fn lookup(
    versions: &GitVersions,
    scope: &StorageScope,
    revision: &Revision,
    at: &str,
) -> Fallible<Option<ItemSummary>> {
    Ok(versions.item_at_path(scope, revision, &path(at)?).await?)
}

#[tokio::test]
async fn an_item_is_summarized_exactly_as_the_listing_of_its_folder_summarizes_it() -> TestResult {
    let seeded = seeded().await?;
    let versions = seeded.storage.versions();
    let mut compared = 0_usize;
    for folder in [None, Some("notes"), Some("sources")] {
        for entry in listed(&versions, &seeded.scope, &seeded.revision, folder).await? {
            let found = some(
                lookup(
                    &versions,
                    &seeded.scope,
                    &seeded.revision,
                    entry.path.as_str(),
                )
                .await?,
                "the listed item at its path",
            )?;
            assert_eq!(
                serde_json::to_value(&found)?,
                serde_json::to_value(&entry)?,
                "{} differs from its listing",
                entry.path.as_str()
            );
            compared = compared.saturating_add(1);
        }
    }
    assert_eq!(
        compared, 4,
        "the note, the View, the card and the retired note"
    );
    let card = some(
        lookup(
            &versions,
            &seeded.scope,
            &seeded.revision,
            "sources/scan-pdf.md",
        )
        .await?,
        "the card",
    )?;
    assert_eq!(card.media_type.as_deref(), Some("application/pdf"));
    assert!(
        card.extraction.is_some(),
        "the card summarizes its extraction"
    );
    let view = some(
        lookup(&versions, &seeded.scope, &seeded.revision, "notes/chart.md").await?,
        "the View",
    )?;
    assert_eq!((view.title.as_str(), view.kind), ("chart", ItemKind::View));
    let retired = some(
        lookup(&versions, &seeded.scope, &seeded.revision, "retired.md").await?,
        "the retired note",
    )?;
    assert!(retired.archived);
    assert_eq!(retired.status, ItemStatus::Deprecated);
    assert_eq!(retired.revision, seeded.revision);
    Ok(())
}

#[tokio::test]
async fn a_path_that_holds_no_item_answers_none() -> TestResult {
    let seeded = seeded().await?;
    let versions = seeded.storage.versions();
    for (at, what) in [
        ("notes/absent.md", "nothing at the path"),
        ("nowhere/plan.md", "a missing intermediate folder"),
        (
            "notes/plan.md/inner.md",
            "an intermediate segment that is a file",
        ),
        (
            "deck.md",
            "a folder entry, even one named like a Markdown file",
        ),
        ("notes", "a folder"),
        ("notes/index.md", "a folder index"),
        ("index.md", "the root index"),
        ("log.md", "the change log"),
    ] {
        assert!(
            lookup(&versions, &seeded.scope, &seeded.revision, at)
                .await?
                .is_none(),
            "{at}: {what}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_file_the_listing_would_not_report_answers_none() -> TestResult {
    let seeded = seeded().await?;
    let versions = seeded.storage.versions();
    let item = versions
        .read_file(&seeded.scope, &seeded.revision, &path("notes/plan.md")?)
        .await?;
    let raw = write_raw(
        &seeded,
        &[
            (".okf/stray.md", &item),
            ("old/index.md", &item),
            ("notes/LOG.md", &item),
            ("notes/copy.txt", &item),
            ("notes/readme.md", b"# Read me\n\nNo application header.\n"),
        ],
    )?;
    for (at, folder) in [
        (".okf/stray.md", Some(".okf")),
        ("old/index.md", Some("old")),
        ("notes/LOG.md", Some("notes")),
        ("notes/copy.txt", Some("notes")),
        ("notes/readme.md", Some("notes")),
    ] {
        let reported = listed(&versions, &seeded.scope, &raw, folder)
            .await?
            .iter()
            .any(|entry| entry.path.as_str() == at);
        assert!(!reported, "{at}: the listing does not report it");
        assert!(
            lookup(&versions, &seeded.scope, &raw, at).await?.is_none(),
            "{at}: so the lookup answers none"
        );
    }
    assert!(
        lookup(&versions, &seeded.scope, &raw, "notes/plan.md")
            .await?
            .is_some(),
        "the original is still an item"
    );
    Ok(())
}

#[tokio::test]
async fn only_a_revision_this_workspace_does_not_hold_is_not_found() -> TestResult {
    let seeded = seeded().await?;
    let versions = seeded.storage.versions();
    let unknown = Revision::try_from("ab".repeat(20))?;
    for at in ["notes/plan.md", "notes/absent.md", "notes/index.md"] {
        let error = err_of(
            versions
                .item_at_path(&seeded.scope, &unknown, &path(at)?)
                .await,
        )?;
        assert_eq!(error.code, ErrorCode::NotFound, "{at}: {}", error.message);
    }
    let (other, other_initial) = workspace(&seeded.storage, 3).await?;
    let elsewhere = versions
        .commit(
            &other,
            changes(
                4,
                &other_initial,
                vec![note(
                    20,
                    "notes/plan.md",
                    Some("Elsewhere"),
                    ItemKind::Note,
                )?],
            )?,
            Arc::new(Accept),
        )
        .await?
        .revision;
    assert!(
        lookup(&versions, &other, &elsewhere, "notes/plan.md")
            .await?
            .is_some()
    );
    let error = err_of(
        versions
            .item_at_path(&seeded.scope, &elsewhere, &path("notes/plan.md")?)
            .await,
    )?;
    assert_eq!(
        error.code,
        ErrorCode::NotFound,
        "a revision of another workspace is resolved in this one's repository only"
    );
    Ok(())
}

#[tokio::test]
async fn a_moved_item_answers_at_the_revision_asked_not_the_head() -> TestResult {
    let seeded = seeded().await?;
    let versions = seeded.storage.versions();
    let moved = versions
        .commit(
            &seeded.scope,
            changes(
                5,
                &seeded.revision,
                vec![TreeEdit::MoveItem {
                    item_id: uuid!(10)?,
                    destination: path("plans/plan.md")?,
                }],
            )?,
            Arc::new(Accept),
        )
        .await?
        .revision;
    assert_eq!(versions.head(&seeded.scope).await?, moved);
    let before = some(
        lookup(&versions, &seeded.scope, &seeded.revision, "notes/plan.md").await?,
        "the old path at the older revision",
    )?;
    assert_eq!(before.id, uuid!(10)?);
    assert_eq!(before.revision, seeded.revision);
    assert!(
        lookup(&versions, &seeded.scope, &seeded.revision, "plans/plan.md")
            .await?
            .is_none(),
        "the new path holds nothing at the older revision"
    );
    assert!(
        lookup(&versions, &seeded.scope, &moved, "notes/plan.md")
            .await?
            .is_none(),
        "the old path holds nothing at the head"
    );
    let after = some(
        lookup(&versions, &seeded.scope, &moved, "plans/plan.md").await?,
        "the new path at the head",
    )?;
    assert_eq!((after.id, after.revision), (uuid!(10)?, moved));
    Ok(())
}

#[tokio::test]
async fn the_lookup_reads_only_the_path_it_names() -> TestResult {
    let seeded = seeded().await?;
    let versions = seeded.storage.versions();
    let not_utf8: &[u8] = &[0xff, 0xfe, 0xfd];
    let raw = write_raw(
        &seeded,
        &[
            ("notes/broken.md", not_utf8),
            ("far/away/broken.md", not_utf8),
        ],
    )?;
    // Listing a folder reads every item file of the tree, so a broken sibling fails it ...
    let listing = err_of(
        versions
            .list(&seeded.scope, &raw, Some(&path("notes")?), page())
            .await,
    )?;
    assert_eq!(listing.code, ErrorCode::Internal, "{}", listing.message);
    // ... while the lookup touches only the entries on its own path.
    let found = some(
        lookup(&versions, &seeded.scope, &raw, "notes/plan.md").await?,
        "the item beside a broken sibling",
    )?;
    let mut expected = some(
        listed(&versions, &seeded.scope, &seeded.revision, Some("notes"))
            .await?
            .into_iter()
            .find(|entry| entry.path.as_str() == "notes/plan.md"),
        "the item in the clean listing",
    )?;
    expected.revision = raw;
    assert_eq!(
        serde_json::to_value(&found)?,
        serde_json::to_value(&expected)?
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
