//! `VersionStore::committed_files` against real Git repositories: every file of one revision's
//! tree, a page at a time, in byte order of path, each with its exact bytes and, for an item
//! file, what `show` answers of it at that revision (what a portable export writes).
//!
//! Files no store edit writes (a header-less note, a link) are written straight into the
//! repository with `git2`.
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use git2::{Oid, Repository, Tree};
use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{
    ConversionOutcome, ConversionSettings, ConverterIdentity, Extraction, TextOrigin,
};
use okf_jawn_contract::identity::{Digest, Revision, TenantId, WorkspacePath};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_contract::source::SourceAppearance;
use okf_jawn_core::storage::{
    CandidateChanges, CandidateCheck, CommitChanges, CommittedFile, FilePage, NewWorkspace,
    Provenance, SourceCard, StorageScope, TreeEdit, VersionStore, WorkspaceCatalog,
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

/// A data directory with one workspace seeded by `seeded`.
struct Seeded {
    storage: Storage,
    scope: StorageScope,
    /// The commit that wrote the seed.
    revision: Revision,
    /// Declared last: fields drop in declaration order, so the storage has closed its
    /// databases and its lock when the directory is removed (Windows cannot remove an open
    /// file, and `TempDir` would leave the directory behind).
    directory: tempfile::TempDir,
}

impl CandidateCheck for Accept {
    fn check(&self, _root: &Path, _rewritten: &[WorkspacePath]) -> Result<Vec<Warning>, ApiError> {
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

const fn all_of_it() -> FilePage {
    FilePage {
        after: None,
        limit: 500,
        max_bytes: u64::MAX,
    }
}

fn note(item: u32, at: &str, kind: ItemKind) -> Fallible<TreeEdit> {
    Ok(TreeEdit::CreateItem {
        item_id: uuid!(item)?,
        path: path(at)?,
        title: Some(format!("Title of {at}")),
        type_name: "Note".to_owned(),
        kind,
        body: format!("Body of {at}.\n"),
        properties: BTreeMap::from([("x_team".to_owned(), json!(7))]),
    })
}

/// A source card naming an original (`d…`) and a conversion record (`e…`).
fn card(item: u32, at: &str) -> Fallible<TreeEdit> {
    let mut appearance: SourceAppearance = serde_json::from_value(json!({
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
    }))?;
    appearance.extraction = Extraction {
        outcome: ConversionOutcome::Completed,
        converter: Some(ConverterIdentity {
            name: "docling".to_owned(),
            version: "2.3.0".to_owned(),
            packages: Vec::new(),
            settings: ConversionSettings::default(),
            models: Vec::new(),
            page_window: None,
        }),
        digest: Some(Digest::try_from("e".repeat(64))?),
        page_count: None,
        text_origin: TextOrigin::Converter,
        corrected: false,
        supplied: None,
        warnings: Vec::new(),
    };
    Ok(TreeEdit::WriteSourceCard(Box::new(SourceCard {
        item_id: uuid!(item)?,
        path: path(at)?,
        title: "scan.pdf".to_owned(),
        type_name: "source".to_owned(),
        body: "Converted text.\n".to_owned(),
        properties: BTreeMap::new(),
        appearance,
    })))
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

/// A workspace holding a note and a View in `notes/`, a source card in `sources/`, and a note
/// at the root: with their folder indexes, the change log, the card's appearance under `.okf/`.
async fn seeded() -> Fallible<Seeded> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (scope, initial) = workspace(&storage, 1).await?;
    let revision = storage
        .versions()
        .commit(
            &scope,
            CommitChanges {
                mutation_id: uuid!(2)?,
                expected_head: initial,
                author: person("ana"),
                message: "seed".to_owned(),
                edits: vec![
                    note(10, "notes/plan.md", ItemKind::Note)?,
                    note(11, "notes/chart.md", ItemKind::View)?,
                    card(12, "sources/scan-pdf.md")?,
                    note(13, "retired.md", ItemKind::Note)?,
                ],
            },
            Arc::new(Accept),
        )
        .await?
        .revision;
    Ok(Seeded {
        storage,
        scope,
        revision,
        directory,
    })
}

fn repository_path(directory: &Path, scope: &StorageScope) -> std::path::PathBuf {
    directory
        .join("repositories")
        .join(scope.tenant_id.as_str())
        .join(format!("{}.git", scope.workspace_id.0))
}

/// `tree` with an entry of `mode` holding `bytes` at `segments`, written as a new tree.
fn with_entry(
    repository: &Repository,
    tree: Option<&Tree<'_>>,
    segments: &[&str],
    bytes: &[u8],
    mode: i32,
) -> Fallible<Oid> {
    let mut builder = repository.treebuilder(tree)?;
    match segments {
        [name] => {
            builder.insert(name, repository.blob(bytes)?, mode)?;
        }
        [folder, rest @ ..] => {
            let child = tree
                .and_then(|tree| tree.get_name(folder))
                .map(|entry| repository.find_tree(entry.id()))
                .transpose()?;
            let written = with_entry(repository, child.as_ref(), rest, bytes, mode)?;
            builder.insert(folder, written, 0o040_000)?;
        }
        [] => return Err("an empty path".into()),
    }
    Ok(builder.write()?)
}

/// Commit one entry on top of the head straight into the workspace repository, bypassing the
/// store, and return the new head.
fn write_raw(seeded: &Seeded, at: &str, bytes: &[u8], mode: i32) -> Fallible<Revision> {
    let repository =
        Repository::open_bare(repository_path(seeded.directory.path(), &seeded.scope))?;
    let parent = repository.find_commit(repository.refname_to_id("refs/heads/main")?)?;
    let segments: Vec<&str> = at.split('/').collect();
    let written = with_entry(&repository, Some(&parent.tree()?), &segments, bytes, mode)?;
    let tree = repository.find_tree(written)?;
    let who = git2::Signature::now("raw", "raw@example.invalid")?;
    let commit = repository.commit(
        Some("refs/heads/main"),
        &who,
        &who,
        "raw entry",
        &tree,
        &[&parent],
    )?;
    Ok(Revision::try_from(commit.to_string())?)
}

/// Every file of the revision's tree, read with `git2`: path and bytes, in path order.
fn tree_files(seeded: &Seeded, revision: &Revision) -> Fallible<Vec<(String, Vec<u8>)>> {
    let repository =
        Repository::open_bare(repository_path(seeded.directory.path(), &seeded.scope))?;
    let tree = repository
        .find_commit(Oid::from_str(revision.as_str())?)?
        .tree()?;
    let mut found = Vec::new();
    tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
        if entry.kind() == Some(git2::ObjectType::Blob)
            && let (Ok(name), Ok(blob)) = (entry.name(), repository.find_blob(entry.id()))
        {
            found.push((format!("{root}{name}"), blob.content().to_vec()));
        }
        git2::TreeWalkResult::Ok
    })?;
    found.sort();
    Ok(found)
}

/// Every file `committed_files` answers at `revision`, every page read in turn with `page`.
async fn every_page(
    seeded: &Seeded,
    revision: &Revision,
    page: FilePage,
) -> Fallible<(Vec<CommittedFile>, usize)> {
    let mut files = Vec::new();
    let mut pages = 0_usize;
    let mut after = None;
    loop {
        let answered = seeded
            .storage
            .versions()
            .committed_files(
                &seeded.scope,
                revision,
                FilePage {
                    after: after.take(),
                    ..page.clone()
                },
            )
            .await?;
        pages = pages.saturating_add(1);
        assert!(!answered.files.is_empty(), "a page holds at least one file");
        files.extend(answered.files);
        match answered.next_after {
            Some(next) => after = Some(next),
            None => return Ok((files, pages)),
        }
    }
}

#[tokio::test]
async fn every_committed_file_is_answered_once_in_path_order_with_its_bytes() -> TestResult {
    let seeded = seeded().await?;
    let (files, pages) = every_page(&seeded, &seeded.revision, all_of_it()).await?;
    assert_eq!(pages, 1);
    let answered: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|file| (file.path.clone(), file.bytes.clone()))
        .collect();
    assert_eq!(answered, tree_files(&seeded, &seeded.revision)?);
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    for expected in [
        ".okf/sources/00000000-0000-4000-8000-000000000012.json",
        "index.md",
        "log.md",
        "notes/chart.md",
        "notes/index.md",
        "notes/plan.md",
        "retired.md",
        "sources/index.md",
        "sources/scan-pdf.md",
    ] {
        assert!(paths.contains(&expected), "{expected} in {paths:?}");
    }
    Ok(())
}

#[tokio::test]
async fn an_item_file_carries_what_show_answers_of_it() -> TestResult {
    let seeded = seeded().await?;
    let versions = seeded.storage.versions();
    let (files, _) = every_page(&seeded, &seeded.revision, all_of_it()).await?;
    let mut items = 0_usize;
    for file in &files {
        let Some(item) = &file.item else {
            // No file but an item file carries an item: indexes, the log and `.okf/` hold none.
            // OKF reads only the exact lower-case `md` extension; `X.MD` is a plain file.
            assert!(
                Path::new(&file.path)
                    .extension()
                    .is_none_or(|extension| extension != "md")
                    || file.path.ends_with("index.md")
                    || file.path == "log.md",
                "{} holds no item",
                file.path
            );
            continue;
        };
        items = items.saturating_add(1);
        let shown = versions
            .show(&seeded.scope, &seeded.revision, item.item_id)
            .await?;
        assert_eq!(shown.summary.path.as_str(), file.path);
        assert_eq!(item.content_digest, shown.content_digest, "{}", file.path);
        assert_eq!(item.kind, shown.summary.kind, "{}", file.path);
    }
    assert_eq!(items, 4);
    let card = some(
        files.iter().find(|file| file.path == "sources/scan-pdf.md"),
        "the card",
    )?;
    let named = some(card.item.as_ref(), "the card's item")?;
    let source = some(named.source.as_ref(), "the card's original")?;
    assert_eq!(source.digest.as_str(), "d".repeat(64));
    assert_eq!(source.original_name, "scan.pdf");
    assert_eq!(
        named
            .record
            .as_ref()
            .map(|digest| digest.as_str().to_owned()),
        Some("e".repeat(64))
    );
    let plan = some(
        files.iter().find(|file| file.path == "notes/plan.md"),
        "the note",
    )?;
    let note = some(plan.item.as_ref(), "the note's item")?;
    assert_eq!((note.source.as_ref(), note.record.as_ref()), (None, None));
    Ok(())
}

#[tokio::test]
async fn a_page_ends_at_its_limit_or_past_its_bytes_and_the_next_begins_after_it() -> TestResult {
    let seeded = seeded().await?;
    let (whole, _) = every_page(&seeded, &seeded.revision, all_of_it()).await?;
    let paths = |files: &[CommittedFile]| -> Vec<String> {
        files.iter().map(|file| file.path.clone()).collect()
    };
    // Two files a page.
    let (by_two, pages) = every_page(
        &seeded,
        &seeded.revision,
        FilePage {
            after: None,
            limit: 2,
            max_bytes: u64::MAX,
        },
    )
    .await?;
    assert_eq!(paths(&by_two), paths(&whole));
    assert_eq!(pages, whole.len().div_ceil(2));
    // One byte a page: every file alone on its page, however large.
    let (by_byte, pages) = every_page(
        &seeded,
        &seeded.revision,
        FilePage {
            after: None,
            limit: 500,
            max_bytes: 0,
        },
    )
    .await?;
    assert_eq!(paths(&by_byte), paths(&whole));
    assert_eq!(pages, whole.len());
    // A limit of 0 answers one file, not none.
    let first = seeded
        .storage
        .versions()
        .committed_files(
            &seeded.scope,
            &seeded.revision,
            FilePage {
                after: None,
                limit: 0,
                max_bytes: u64::MAX,
            },
        )
        .await?;
    assert_eq!(first.files.len(), 1);
    // `after` a path the tree does not hold: the files after it in path order.
    let after = seeded
        .storage
        .versions()
        .committed_files(
            &seeded.scope,
            &seeded.revision,
            FilePage {
                after: Some("notes/d".to_owned()),
                limit: 500,
                max_bytes: u64::MAX,
            },
        )
        .await?;
    let expected: Vec<String> = paths(&whole)
        .into_iter()
        .filter(|path| path.as_str() > "notes/d")
        .collect();
    assert_eq!(paths(&after.files), expected);
    assert_eq!(after.next_after, None);
    Ok(())
}

#[tokio::test]
async fn a_revision_off_the_accepted_line_or_unknown_is_not_found() -> TestResult {
    let seeded = seeded().await?;
    let versions = seeded.storage.versions();
    let candidate = versions
        .create_candidate(
            &seeded.scope,
            uuid!(40)?,
            CandidateChanges {
                mutation_id: uuid!(41)?,
                base: seeded.revision.clone(),
                author: person("bo"),
                message: "a proposal".to_owned(),
                edits: vec![note(42, "notes/proposed.md", ItemKind::Note)?],
            },
            Arc::new(Accept),
        )
        .await?
        .revision;
    for revision in [candidate, Revision::try_from("9".repeat(40))?] {
        let error = err_of(
            versions
                .committed_files(&seeded.scope, &revision, all_of_it())
                .await,
        )?;
        assert_eq!(error.code, ErrorCode::NotFound, "{revision:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_markdown_file_without_a_header_carries_no_item_and_a_link_is_a_store_fault() -> TestResult
{
    let seeded = seeded().await?;
    let versions = seeded.storage.versions();
    let plain = write_raw(&seeded, "notes/plain.md", b"# Plain\n", 0o100_644)?;
    let (files, _) = every_page(&seeded, &plain, all_of_it()).await?;
    let file = some(
        files.iter().find(|file| file.path == "notes/plain.md"),
        "the plain file",
    )?;
    assert_eq!(file.bytes, b"# Plain\n");
    assert!(file.item.is_none());
    let linked = write_raw(&seeded, "notes/link.md", b"plan.md", 0o120_000)?;
    let error = err_of(
        versions
            .committed_files(&seeded.scope, &linked, all_of_it())
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(error.message.contains("notes/link.md"), "{}", error.message);
    Ok(())
}

/// M2a design section6.2: properties read by core's portable YAML conversion must survive
/// the actual item writer and storage reader. The expected map is a literal, independent
/// of both conversions; the server adds only its own application header.
#[tokio::test]
async fn portable_json_properties_survive_a_real_item_write_and_read() -> TestResult {
    let seeded = seeded().await?;
    let supplied = okf_core::yaml::Value::parse(concat!(
        "type: Note\n",
        "title: Imported properties\n",
        "count: -3\n",
        "largest: 9223372036854775807\n",
        "ratio: 1.5\n",
        "nested: {enabled: true, values: [null, false, 'café']}\n",
        "verified: {by: 'human:outside', at: '2026-01-01T00:00:00Z'}\n",
    ))?;
    let converted = okf_jawn_core::portable::json_of(&supplied);
    let expected = json!({
        "type": "Note", "title": "Imported properties", "count": -3,
        "largest": 9_223_372_036_854_775_807_i64, "ratio": 1.5,
        "nested": {"enabled": true, "values": [null, false, "café"]},
        "verified": {"by": "human:outside", "at": "2026-01-01T00:00:00Z"}
    });
    assert_eq!(converted, expected);
    let versions = seeded.storage.versions();
    let item = uuid!(99)?;
    let committed = versions
        .commit(
            &seeded.scope,
            CommitChanges {
                mutation_id: uuid!(100)?,
                expected_head: seeded.revision.clone(),
                author: person("ana"),
                message: "Write portable properties".to_owned(),
                edits: vec![TreeEdit::CreateItem {
                    item_id: item,
                    path: path("notes/imported-properties.md")?,
                    title: Some("Imported properties".to_owned()),
                    type_name: "Note".to_owned(),
                    kind: ItemKind::Note,
                    body: "Original text.\n".to_owned(),
                    properties: serde_json::from_value(converted)?,
                }],
            },
            Arc::new(Accept),
        )
        .await?;
    let shown = versions
        .show(&seeded.scope, &committed.revision, item)
        .await?;
    assert_eq!(shown.body, "Original text.");
    assert_eq!(
        serde_json::to_value(okf_jawn_core::items::without_header(&shown.properties))?,
        expected
    );
    assert_eq!(shown.summary.id, item);
    assert_eq!(shown.summary.path.as_str(), "notes/imported-properties.md");
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
