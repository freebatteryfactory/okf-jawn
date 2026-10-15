//! Workspace paths against real Git repositories, on the filesystem the tests run on: what a
//! note's file is, a rename that changes only letter case, a folder spelled two ways, the links
//! to files that a moved note keeps, and its links to itself, which follow it.
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
use okf_jawn_contract::item::{ItemKind, TypeDefinition};
use okf_jawn_contract::search::LinkDirection;
use okf_jawn_core::search::{GraphQuery, LinkQuery, SearchIndex, SearchQuery};
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

/// Paths to remove from a tree and files to write into it, for `Workspace::raw`.
type GitChanges = (Vec<String>, Vec<(String, Vec<u8>)>);

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

/// An item identity with letters in it, so the names the server writes for it have an
/// upper-case spelling.
const LETTERED: &str = "abcdef00-0000-4000-8000-000000000014";

/// The frontmatter of note 10 as `note` writes it, and of note 12.
const NOTE_10: &str = "---\ntype: Note\ntitle: Note 10\nokf_jawn:\n  item_id: \
                       00000000-0000-4000-8000-000000000010\n  kind: note\n---\n\n";
const NOTE_12: &str = "---\ntype: Note\ntitle: Note 12\nokf_jawn:\n  item_id: \
                       00000000-0000-4000-8000-000000000012\n  kind: note\n---\n\n";

/// A referrer of `Notes/Report.md` in three link styles, with a link in another spelling that
/// never pointed at it; and the referrer after the item is renamed to `Notes/report.md`.
const REFERRER: &str = "Read [a](Notes/Report.md), [b](./Notes/Report.md#sec), \
                        [c](/Notes/Report.md), [d](notes/report.md).";
const REFERRER_MOVED: &str = "Read [a](Notes/report.md), [b](Notes/report.md#sec), \
                              [c](/Notes/report.md), [d](notes/report.md).";

/// Bodies of `Notes/Report.md`, by link style, and the body its file holds after it is renamed
/// to `Notes/report.md` as a direct move writes it. Recorded on Linux (WSL Ubuntu 24.04,
/// okf-core 0.2.7), where `Report.md` and `report.md` are two files, from this branch with the
/// parking removed, so that the rename was one move from `Notes/Report` to `Notes/report`; each
/// file was printed whole and its body is kept here (the frontmatter is `NOTE_10`; the
/// referrer's is `REFERRER_MOVED`). The first four were recorded at ad6415c (`let from = if
/// false` in `move_item`), where a move was one `move_concept`; the last two with the file-links
/// workaround (`moved_body`), which leaves those four as they were but for the item's link to
/// itself in "dot slash".
const DIRECT_MOVES: [(&str, &str, &str); 6] = [
    (
        "above the root",
        "Up [a](../../outside.pdf) and [b](../../../x.md#t).",
        "Up [a](../outside.pdf) and [b](../x.md#t).",
    ),
    // okf-core 0.2.7 writes the item's link to itself as a link to its old name, `Report.md`,
    // which this value held until the self-link ruling: the link now follows the item to its
    // new name, as a direct move writes it too (`moved_body`).
    (
        "dot slash",
        "Here [p](./plan.md) and [me](./Report.md).",
        "Here [p](plan.md) and [me](report.md).",
    ),
    (
        "anchor",
        "Anchors [p](plan.md#part) and [s](#section).",
        "Anchors [p](plan.md#part) and [s](#section).",
    ),
    (
        "absolute",
        "Absolute [p](/Notes/plan.md) and [r](/ref.md#top).",
        "Absolute [p](/Notes/plan.md) and [r](/ref.md#top).",
    ),
    // okf-core 0.2.7 alone appends `.md` to a relative link to a file that is not Markdown on
    // every move, direct or not (review N8), which this value held until the file-links package:
    // `pics/p.png.md` and `../files/a.pdf.md`. Storage now writes the moved body again without
    // it (`moved_body`, the edit.rs header).
    (
        "not Markdown",
        "File ![i](pics/p.png) and [f](../files/a.pdf).",
        "File ![i](pics/p.png) and [f](../files/a.pdf).",
    ),
    // A link to a folder loses its `/` when it is rebased; read again by a second hop, `plan`
    // then names the note `Notes/plan.md` and would become `plan.md`.
    ("folder link", "Folder [d](plan/).", "Folder [d](plan)."),
];

/// The body of the note `notes/a.md` that the file-links tests move: relative links to an image
/// and a text file below its folder and a pdf beside it, a link to the note `notes/other.md`
/// with and without an anchor, and an absolute link.
const FILE_LINKS: &str = "See [Other](other.md) and [part](other.md#part).\n\n![p](pics/p.png)\n\
                          [a](../files/a.pdf)\n[s](sub/notes.txt)\n[abs](/files/a.pdf)";
/// The `sources[].resource` of that note: the image.
const FILE_LINKS_SOURCE: &str = "pics/p.png";
/// The files those links name, as the tree spells them.
const FILE_LINKS_NAMED: [&str; 3] = ["notes/pics/p.png", "files/a.pdf", "notes/sub/notes.txt"];
/// The body of `notes/other.md`, which links to the moved note at `{to}` and to the image.
const FILE_LINKS_REFERRER: &str = "Back to [A]({to}). Logo: ![p](pics/p.png)";
/// The body of `deep/x/far.md`, which links to two of those files and not to the moved note.
const FILE_LINKS_FAR: &str = "Far [f](../../files/a.pdf) and ![p](../../notes/pics/p.png).";
/// What the moves to `archive/` add to `FILE_LINKS` (`file_links_seeded`, `wider`): a link to
/// `notes/other.md` without `.md`, and a percent-encoded link to the note `notes/my note.md`.
const FILE_LINKS_WIDER: &str = "\n[o](other)\n[n](my%20note.md)";
/// `FILE_LINKS` and `FILE_LINKS_WIDER` after a move to `archive/a.md`: each link resolved from
/// the note's old folder, so `[o](other)` names `notes/other.md` and not the `archive/other.md`
/// beside the new file, and the percent-encoded link taken literally, as okf-core takes it, so it
/// is rebased as a path and keeps its spelling.
const FILE_LINKS_ARCHIVED: &str = "See [Other](../notes/other.md) and \
                                   [part](../notes/other.md#part).\n\n\
                                   ![p](../notes/pics/p.png)\n[a](../files/a.pdf)\n\
                                   [s](../notes/sub/notes.txt)\n[abs](/files/a.pdf)\n\
                                   [o](../notes/other.md)\n[n](../notes/my%20note.md)";

/// The body of the note `notes/a.md` that the self-link tests move: links to itself, relative
/// (beside it, with an anchor, through `./`, through its folder from above, without `.md`),
/// absolute with and without an anchor, and a pure anchor.
const SELF_LINKS: &str = "Me [m](a.md), [s](a.md#sec), [d](./a.md#sec), [up](../notes/a.md), \
                          [abs](/notes/a.md), [abs2](/notes/a.md#sec), [bare](a), [x](#sec).";

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

/// A note with `properties` besides its type and title.
fn note_with(
    item: u32,
    at: &str,
    body: &str,
    properties: BTreeMap<String, serde_json::Value>,
) -> Fallible<TreeEdit> {
    Ok(TreeEdit::CreateItem {
        item_id: uuid!(item)?,
        path: path(at)?,
        title: Some(format!("Note {item}")),
        type_name: "Note".to_owned(),
        kind: ItemKind::Note,
        body: body.to_owned(),
        properties,
    })
}

/// The first private name item `item` is parked under for `purpose` (`recase`, `rewind`).
fn parked_name(purpose: &str, item: u32) -> String {
    format!("okf-jawn-{purpose}-{}.md", uuid_text(item))
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

fn restore_paths(from: &Revision, paths: &[&str]) -> Fallible<TreeEdit> {
    Ok(TreeEdit::RestorePaths {
        from: from.clone(),
        paths: paths.iter().map(|at| path(at)).collect::<Fallible<_>>()?,
    })
}

fn deleted(item: u32) -> Fallible<TreeEdit> {
    Ok(TreeEdit::DeleteItem {
        item_id: uuid!(item)?,
    })
}

fn folder(at: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::CreateFolder { folder: path(at)? })
}

fn edited(item: u32, body: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::EditItem {
        item_id: uuid!(item)?,
        body: body.to_owned(),
        properties: BTreeMap::from([("type".to_owned(), json!("Note"))]),
    })
}

/// A source card at `at`.
fn card(item: u32, at: &str) -> Fallible<TreeEdit> {
    card_of(&uuid_text(item), at)
}

/// A type definition named `name`.
fn set_type(name: &str) -> TreeEdit {
    TreeEdit::SetType {
        definition: TypeDefinition {
            name: name.to_owned(),
            schema_version: 1,
            properties_schema: json!({ "type": "object" }),
            ui_schema: json!({}),
        },
    }
}

/// A source card at `at` for the item whose identity is spelled `id`.
fn card_of(id: &str, at: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::WriteSourceCard(Box::new(SourceCard {
        item_id: serde_json::from_value(json!(id))?,
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

/// `error` is the folder-spelling refusal: `InvalidInput` on `field`, naming `spelling`.
fn assert_spelled(error: &ApiError, spelling: &str, field: &str) {
    assert_eq!(error.code, ErrorCode::InvalidInput, "{}", error.message);
    assert_eq!(error.field.as_deref(), Some(field), "{}", error.message);
    assert!(
        error
            .message
            .contains(&format!("the folder is spelled {spelling}")),
        "{}",
        error.message
    );
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

/// A note at a path that does not end in `.md` in any letter case is refused as not Markdown,
/// and no lower-case name is offered for it: `x.txt` is not `x.tmd`.
#[tokio::test]
async fn a_note_not_ending_in_md_is_refused_as_not_markdown() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(2, &space.initial, vec![note(10, "a.md", "Ten.")?])
        .await?;
    for (mutation, at) in [(3, "x.txt"), (4, "Notes/y.tmd")] {
        let error = space
            .refused(mutation, &seeded, vec![note(11, at, "Eleven.")?])
            .await?;
        assert_eq!(error.code, ErrorCode::InvalidInput, "{}", error.message);
        assert_eq!(error.field.as_deref(), Some("/path"), "{}", error.message);
        assert_eq!(
            error.message,
            format!("{at} is not a Markdown file, so it cannot hold an item")
        );
    }
    Ok(())
}

/// An item under a folder whose name ends in `.md`, and one named `a.md.b.md`, each move as
/// themselves, and the links that point at them follow: only the last `.md` of a path is the
/// extension okf-core's concept leaves off.
#[tokio::test]
async fn an_item_with_md_inside_its_path_moves_as_itself() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "notes.md/a.md", "Ten.")?,
                note(11, "a.md.b.md", "Eleven.")?,
                note(
                    12,
                    "ref.md",
                    "See [ten](notes.md/a.md) and [eleven](a.md.b.md).",
                )?,
            ],
        )
        .await?;
    let moved_on = space
        .commit(
            3,
            &seeded,
            vec![moved(10, "notes.md/b.md")?, moved(11, "c.md")?],
        )
        .await?;
    assert_eq!(
        space.item_at(&moved_on, "notes.md/b.md").await?,
        Some(uuid!(10)?)
    );
    assert_eq!(space.item_at(&moved_on, "c.md").await?, Some(uuid!(11)?));
    assert_eq!(
        space.body(&moved_on, 12).await?,
        "See [ten](notes.md/b.md) and [eleven](c.md)."
    );
    Ok(())
}

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

/// An item whose file name ends in `.md.md` is okf-core's concept `a.md` (one `.md` comes off,
/// as `ConceptId::from_path` takes it): a move of it moves it and never the item at `a.md`, the
/// links that point at it follow it, and with no title it is titled `c.md`.
#[tokio::test]
async fn an_item_whose_name_ends_in_md_md_moves_as_itself() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "a.md", "Ten.")?,
                note(11, "a.md.md", "Eleven.")?,
                note(12, "ref.md", "See [eleven](a.md.md) and [ten](a.md).")?,
                TreeEdit::CreateItem {
                    item_id: uuid!(13)?,
                    path: path("c.md.md")?,
                    title: None,
                    type_name: "Note".to_owned(),
                    kind: ItemKind::Note,
                    body: "Thirteen.".to_owned(),
                    properties: BTreeMap::new(),
                },
            ],
        )
        .await?;
    let moved_on = space.commit(3, &seeded, vec![moved(11, "b.md")?]).await?;
    assert_eq!(space.item_at(&moved_on, "b.md").await?, Some(uuid!(11)?));
    assert_eq!(space.item_at(&moved_on, "a.md").await?, Some(uuid!(10)?));
    assert_eq!(space.file(&moved_on, "a.md")?, space.file(&seeded, "a.md")?);
    let referrer = space.body(&moved_on, 12).await?;
    assert!(
        referrer.contains("[eleven](b.md)") && referrer.contains("[ten](a.md)"),
        "{referrer}"
    );
    let untitled = space
        .storage
        .versions()
        .item_at_path(&space.scope, &moved_on, &path("c.md.md")?)
        .await?;
    assert_eq!(some(untitled, "c.md.md")?.title, "c.md");
    Ok(())
}

/// Search resolves the links of an item named `.md.md` or `x/.md.md` from the item's own folder:
/// one `.md` comes off its name, as okf-core's concept, so `y.md` from `x/.md.md` is `x/y.md`
/// and from `.md.md` is the root's `y.md`.
#[tokio::test]
async fn search_resolves_the_links_of_an_item_named_md_md_from_its_folder() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![
                note(13, ".md.md", "Root. See [y](y.md).")?,
                note(14, "x/.md.md", "Folder. See [y](y.md).")?,
                note(15, "x/y.md", "Folder y.")?,
                note(16, "y.md", "Root y.")?,
            ],
        )
        .await?;
    let search = space.storage.search();
    search.index_revision(&space.scope, seeded.clone()).await?;
    for (item, target) in [(13, 16), (14, 15)] {
        let found = search
            .links(
                &space.scope,
                LinkQuery {
                    revision: seeded.clone(),
                    item_id: uuid!(item)?,
                    direction: LinkDirection::Outgoing,
                    page: Page {
                        cursor: None,
                        limit: 20,
                    },
                },
            )
            .await?;
        let resolved: Vec<(String, Option<ItemId>)> = found
            .links
            .iter()
            .map(|link| (link.to_path.clone(), link.to))
            .collect();
        assert_eq!(resolved, [("y.md".to_owned(), Some(uuid!(target)?))]);
    }
    Ok(())
}

// S1: a rename that changes only letter case.

/// A note at `at` (item 10) that links to a sibling at `plan` (item 11), and a note at the root
/// (item 12) that links to it as `at`, are committed; item 10 is renamed to `to`, which differs
/// from `at` only in letter case. The rename is one move in one commit: the committed tree holds
/// only `to`, with the bytes `at` held (its one link is a sibling's, which okf-core leaves as it
/// is; `a_case_only_rename_writes_what_a_direct_move_writes` covers links it rewrites); the
/// referrer points at `to` and is the one file the check is told; no private path is in the
/// tree, in `rewritten` or in `log.md`.
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
        "a sibling link: the moved item's file is byte for byte what it was"
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

/// A rename by letter case writes the moved item's file, and its referrer's, byte for byte as a
/// direct move writes them on Linux (`DIRECT_MOVES`): a link that climbs above the bundle root,
/// `./` links (one of them to the item itself), anchors, absolute links, links to files that
/// are not Markdown and a link to a folder. The item's own links are rewritten on each hop, and
/// not idempotently, so only the first hop's bytes are a direct move's.
#[tokio::test]
async fn a_case_only_rename_writes_what_a_direct_move_writes() -> TestResult {
    for (style, body, direct) in DIRECT_MOVES {
        let space = Workspace::new().await?;
        let seeded = space
            .commit(
                2,
                &space.initial,
                vec![
                    note(10, "Notes/Report.md", body)?,
                    note(11, "Notes/plan.md", "Plan.")?,
                    note(12, "ref.md", REFERRER)?,
                ],
            )
            .await?;
        let renamed = space
            .commit(3, &seeded, vec![moved(10, "Notes/report.md")?])
            .await?;
        let item = some(space.file(&renamed, "Notes/report.md")?, "the item")?;
        assert_eq!(
            String::from_utf8(item)?,
            format!("{NOTE_10}{direct}\n"),
            "{style}"
        );
        let referrer = some(space.file(&renamed, "ref.md")?, "the referrer")?;
        assert_eq!(
            String::from_utf8(referrer)?,
            format!("{NOTE_12}{REFERRER_MOVED}\n"),
            "{style}"
        );
    }
    Ok(())
}

/// A referrer that a move rewrote and the same commit then renamed by letter case is not told
/// to the check at the spelling it no longer has, on any filesystem: the check is told only
/// paths the committed tree holds, as it spells them, each once.
#[tokio::test]
async fn a_referrer_renamed_by_case_after_a_move_is_told_only_as_the_tree_spells_it() -> TestResult
{
    for (edits, told) in [
        (vec![moved(10, "b.md")?, moved(12, "ref.md")?], vec![]),
        (
            vec![moved(10, "b.md")?, moved(12, "ref.md")?, moved(10, "c.md")?],
            vec!["ref.md".to_owned()],
        ),
    ] {
        let space = Workspace::new().await?;
        let seeded = space
            .commit(
                2,
                &space.initial,
                vec![
                    note(10, "a.md", "Ten.")?,
                    note(12, "Ref.md", "See [a](a.md).")?,
                ],
            )
            .await?;
        let recording = Arc::new(Recording::default());
        let committed = space
            .commit_with(3, &seeded, edits, recording.clone())
            .await?
            .revision;
        assert_eq!(recording.seen()?, [told]);
        let files = space.files(&committed)?;
        assert!(files.iter().any(|file| file == "ref.md"), "{files:?}");
        assert!(!files.iter().any(|file| file == "Ref.md"), "{files:?}");
    }
    Ok(())
}

/// A folder an earlier edit of the same commit emptied still takes its key: a note created at
/// `Report.md` after the only note of `report.md/` is deleted is refused as taken on every
/// system, so no later rename by letter case can meet a folder at its destination.
#[tokio::test]
async fn an_emptied_folder_in_another_spelling_still_takes_its_name() -> TestResult {
    let space = Workspace::new().await?;
    space
        .commit(
            2,
            &space.initial,
            vec![
                note(20, "report.md/x.md", "Twenty.")?,
                note(11, "b.md", "Eleven.")?,
            ],
        )
        .await?;
    let seeded = space.raw(900, &["report.md/index.md"], &[])?;
    let error = space
        .refused(
            3,
            &seeded,
            vec![
                deleted(20)?,
                note(10, "Report.md", "Ten.")?,
                moved(10, "report.md")?,
            ],
        )
        .await?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    assert_eq!(error.message, "Report.md is already taken");
    Ok(())
}

// B1: the private name an item is parked under is a free name.

/// Item 10 at `Notes/Report.md` with a referrer, and `taken` held by a person's note (item 11)
/// or, with `plain`, by a plain file committed straight through Git; item 10 is renamed to
/// `Notes/report.md`. The rename lands as one move past the taken name: `taken` keeps its
/// bytes, the referrer follows and is the one file the check is told, and no other private
/// name is left in the tree.
async fn renamed_past(taken: &str, plain: bool) -> TestResult {
    let space = Workspace::new().await?;
    let mut edits = vec![
        note(10, "Notes/Report.md", "The report.")?,
        note(12, "ref.md", "Read [it](Notes/Report.md).")?,
    ];
    if !plain {
        edits.push(note(11, taken, "A person's note named so.")?);
    }
    let mut seeded = space.commit(2, &space.initial, edits).await?;
    if plain {
        seeded = space.raw(900, &[], &[(taken, b"plain".to_vec())])?;
    }
    let held = some(space.file(&seeded, taken)?, "the taken name")?;
    let recording = Arc::new(Recording::default());
    let renamed = space
        .commit_with(
            3,
            &seeded,
            vec![moved(10, "Notes/report.md")?],
            recording.clone(),
        )
        .await?
        .revision;
    assert_eq!(
        space.item_at(&renamed, "Notes/report.md").await?,
        Some(uuid!(10)?)
    );
    assert_eq!(space.file(&renamed, taken)?, Some(held), "{taken}");
    let files = space.files(&renamed)?;
    assert!(
        !files
            .iter()
            .any(|file| file.contains(PARKED) && file != taken),
        "{files:?}"
    );
    assert!(
        !files.iter().any(|file| file == "Notes/Report.md"),
        "{files:?}"
    );
    assert_eq!(recording.seen()?, [vec!["ref.md".to_owned()]]);
    let referrer = space.body(&renamed, 12).await?;
    assert_eq!(referrer, "Read [it](Notes/report.md).");
    Ok(())
}

/// The first private name taken by a person's note, by a plain file, or by a note in another
/// spelling of it does not stop a rename that changes only letter case, and is left as it was.
#[tokio::test]
async fn a_case_only_rename_parks_past_a_taken_private_name() -> TestResult {
    let first = format!("Notes/{}", parked_name("recase", 10));
    renamed_past(&first, false).await?;
    renamed_past(&first, true).await?;
    let upper = format!(
        "Notes/{}.md",
        parked_name("recase", 10)
            .trim_end_matches(".md")
            .to_uppercase()
    );
    renamed_past(&upper, false).await
}

/// An item whose own name is its first private name, in either spelling, can be renamed by
/// letter case to the other spelling.
#[tokio::test]
async fn an_item_named_like_its_private_name_is_renamed_by_case() -> TestResult {
    let lower = parked_name("recase", 10);
    let upper = format!("{}.md", lower.trim_end_matches(".md").to_uppercase());
    for (from, to) in [(&lower, &upper), (&upper, &lower)] {
        let space = Workspace::new().await?;
        let seeded = space
            .commit(
                2,
                &space.initial,
                vec![
                    note(10, from, "Named like its private path.")?,
                    note(12, "ref.md", &format!("Read [it]({from})."))?,
                ],
            )
            .await?;
        let renamed = space.commit(3, &seeded, vec![moved(10, to)?]).await?;
        let files = space.files(&renamed)?;
        assert!(files.iter().any(|file| file == to), "{files:?}");
        assert!(!files.iter().any(|file| file == from), "{files:?}");
        assert!(
            !files.iter().any(|file| file.contains("-1.md")),
            "{files:?}"
        );
        assert_eq!(space.file(&renamed, to)?, space.file(&seeded, from)?);
        assert_eq!(space.body(&renamed, 12).await?, format!("Read [it]({to})."));
    }
    Ok(())
}

/// Item 10 at `Notes/Report.md`, `Notes/other.md` (item 12) linking to it, and a reference to
/// item 10's first private name while nothing holds that name, of one `style` alone: `cited`, a
/// `sources` entry of `Notes/cited.md`; `absolute`, an absolute link in `Notes/bystander.md`;
/// `far`, a relative link from another folder, `elsewhere/far.md`; otherwise relative links
/// beside it in `Notes/other.md` and `Notes/bystander.md`. A rename by letter case parks the
/// item elsewhere, so the broken reference stays as it was, only the link to the item follows
/// it, and the check is told only `Notes/other.md`.
async fn renamed_beside_a_broken_reference(style: &str) -> TestResult {
    let space = Workspace::new().await?;
    let ghost = parked_name("recase", 10);
    let only_real = "Only [real](Report.md).".to_owned();
    let (other, at, reference) = match style {
        "cited" => {
            let sources = BTreeMap::from([("sources".to_owned(), json!([{ "resource": ghost }]))]);
            (
                only_real,
                "Notes/cited.md",
                note_with(14, "Notes/cited.md", "Cites it.", sources)?,
            )
        }
        "absolute" => {
            let only = format!("Only [ghost](/Notes/{ghost}).");
            (
                only_real,
                "Notes/bystander.md",
                note(14, "Notes/bystander.md", &only)?,
            )
        }
        "far" => {
            let only = format!("Only [ghost](../Notes/{ghost}).");
            (
                only_real,
                "elsewhere/far.md",
                note(14, "elsewhere/far.md", &only)?,
            )
        }
        _ => {
            let only = format!("Only [ghost]({ghost}).");
            (
                format!("[ghost]({ghost}) and [real](Report.md)."),
                "Notes/bystander.md",
                note(14, "Notes/bystander.md", &only)?,
            )
        }
    };
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "Notes/Report.md", "The report.")?,
                note(12, "Notes/other.md", &other)?,
                reference,
            ],
        )
        .await?;
    let recording = Arc::new(Recording::default());
    let renamed = space
        .commit_with(
            3,
            &seeded,
            vec![moved(10, "Notes/report.md")?],
            recording.clone(),
        )
        .await?
        .revision;
    assert_eq!(
        space.body(&renamed, 12).await?,
        other.replace("](Report.md)", "](report.md)")
    );
    assert_eq!(space.file(&renamed, at)?, space.file(&seeded, at)?, "{at}");
    assert_eq!(recording.seen()?, [vec!["Notes/other.md".to_owned()]]);
    Ok(())
}

/// A link beside it, an absolute link, a link from another folder, or a `sources` entry, that
/// points at the first private name while nothing holds it is a broken reference that a rename
/// by letter case leaves alone.
#[tokio::test]
async fn a_link_to_the_private_name_is_not_rewritten_by_a_case_only_rename() -> TestResult {
    for style in ["beside", "absolute", "far", "cited"] {
        renamed_beside_a_broken_reference(style).await?;
    }
    Ok(())
}

/// Item 20 moved from `a/x.md` to `b/c/x.md`, then rewound. With `plain`, a plain file holds
/// its first private name `b/c/okf-jawn-rewind-<id>.md`; without, a note beside it links to
/// that name while nothing holds it. The Rewind parks the item at a free name: it lands, the
/// plain file or the link stays as it was, and a referrer of the item follows it back.
async fn rewound_past(plain: bool) -> TestResult {
    let space = Workspace::new().await?;
    let ghost = parked_name("rewind", 20);
    let first = format!("b/c/{ghost}");
    let early = space
        .commit(
            2,
            &space.initial,
            vec![
                note(20, "a/x.md", "Twenty.")?,
                note(21, "top.md", "Top [x](a/x.md).")?,
                note(22, "b/c/ghost.md", &format!("Only [ghost]({ghost})."))?,
            ],
        )
        .await?;
    let mut away = space
        .commit(3, &early, vec![moved(20, "b/c/x.md")?])
        .await?;
    if plain {
        away = space.raw(
            900,
            &["b/c/ghost.md"],
            &[(first.as_str(), b"plain".to_vec())],
        )?;
    }
    let back = space.commit(4, &away, vec![restore(20, &early)?]).await?;
    assert_eq!(space.item_at(&back, "a/x.md").await?, Some(uuid!(20)?));
    assert_eq!(space.body(&back, 21).await?, "Top [x](a/x.md).");
    for at in [first.as_str(), "b/c/ghost.md"] {
        assert_eq!(space.file(&back, at)?, space.file(&away, at)?, "{at}");
    }
    let files = space.files(&back)?;
    assert!(
        !files
            .iter()
            .any(|file| file.contains("okf-jawn-rewind-") && *file != first),
        "{files:?}"
    );
    Ok(())
}

/// A Rewind that moves an item back parks it beside its current path at a free name too, past
/// a plain file that holds the first private name and past a link that points at it.
#[tokio::test]
async fn a_rewind_parks_past_a_taken_private_name_and_leaves_links_to_it() -> TestResult {
    rewound_past(true).await?;
    rewound_past(false).await
}

// File links: a moved note keeps its links to files that are not Markdown.

/// The body of item 10 as `file_links_seeded` writes it: `FILE_LINKS`, and with `wider`
/// `FILE_LINKS_WIDER` after it.
fn file_links_body(wider: bool) -> String {
    if wider {
        format!("{FILE_LINKS}{FILE_LINKS_WIDER}")
    } else {
        FILE_LINKS.to_owned()
    }
}

/// Item 10 at `notes/a.md`, with a `sources` entry naming the image (`FILE_LINKS_SOURCE`) and
/// the body `file_links_body(wider)`; item 11 at `notes/other.md` (`FILE_LINKS_REFERRER`); item
/// 12 at `deep/x/far.md` (`FILE_LINKS_FAR`). With `wider`, also item 13 at `archive/other.md`,
/// a note beside the destination named like the one item 10 links to, and item 14 at
/// `notes/my note.md`, which item 10's percent-encoded link names. With `files`, the files the
/// links name (`FILE_LINKS_NAMED`) are committed beside them straight through Git; without, none
/// of them exists.
async fn file_links_seeded(space: &Workspace, files: bool, wider: bool) -> Fallible<Revision> {
    let sources = BTreeMap::from([(
        "sources".to_owned(),
        json!([{ "resource": FILE_LINKS_SOURCE }]),
    )]);
    let mut edits = vec![
        note_with(10, "notes/a.md", &file_links_body(wider), sources)?,
        note(
            11,
            "notes/other.md",
            &FILE_LINKS_REFERRER.replace("{to}", "a.md"),
        )?,
        note(12, "deep/x/far.md", FILE_LINKS_FAR)?,
    ];
    if wider {
        edits.push(note(13, "archive/other.md", "Another note named other.")?);
        edits.push(note(14, "notes/my note.md", "Mine.")?);
    }
    let seeded = space.commit(2, &space.initial, edits).await?;
    if !files {
        return Ok(seeded);
    }
    let written: Vec<(&str, Vec<u8>)> = FILE_LINKS_NAMED
        .iter()
        .map(|at| (*at, at.as_bytes().to_vec()))
        .collect();
    space.raw(900, &[], &written)
}

/// Item 10 (`file_links_seeded`, with and without the files its links name, and `wider`)
/// moved to `to`. Its file is the seeded one with the body `body` and okf-core's rebase of its
/// `sources` entry, `source`, whether the named files exist or not; item 11's link to it reads
/// `link` and its image link is untouched; item 12, which links only to files, keeps its bytes;
/// and the check is told only item 11's file. Answers the workspace and the revisions before
/// and after.
async fn file_links_moved(
    to: &str,
    body: &str,
    source: &str,
    link: &str,
    wider: bool,
) -> Fallible<Vec<(Workspace, Revision, Revision)>> {
    let mut spaces = Vec::new();
    for files in [true, false] {
        let space = Workspace::new().await?;
        let seeded = file_links_seeded(&space, files, wider).await?;
        let recording = Arc::new(Recording::default());
        let moved_on = space
            .commit_with(3, &seeded, vec![moved(10, to)?], recording.clone())
            .await?
            .revision;
        let before = String::from_utf8(some(space.file(&seeded, "notes/a.md")?, "item 10")?)?;
        let expected = before.replace(&file_links_body(wider), body).replace(
            &format!("resource: {FILE_LINKS_SOURCE}"),
            &format!("resource: {source}"),
        );
        let after = String::from_utf8(some(space.file(&moved_on, to)?, "item 10 moved")?)?;
        assert_eq!(after, expected, "{to}, files present: {files}");
        assert_eq!(
            space.body(&moved_on, 11).await?,
            FILE_LINKS_REFERRER.replace("{to}", link),
            "{to}, files present: {files}"
        );
        assert_eq!(
            space.file(&moved_on, "deep/x/far.md")?,
            space.file(&seeded, "deep/x/far.md")?,
            "{to}, files present: {files}"
        );
        assert_eq!(recording.seen()?, [vec!["notes/other.md".to_owned()]]);
        spaces.push((space, seeded, moved_on));
    }
    Ok(spaces)
}

/// A move within the note's folder leaves every link as it was: no `.md` is appended to a link
/// to an image or another file that is not Markdown.
#[tokio::test]
async fn file_links_survive_a_move_within_its_folder() -> TestResult {
    file_links_moved("notes/b.md", FILE_LINKS, FILE_LINKS_SOURCE, "b.md", false).await?;
    Ok(())
}

/// A move to another folder rebases every relative link and keeps each file's name: the image
/// and the text file below the note's old folder are reached from the new one, the pdf above
/// it keeps its path, the links to the note keep their anchor, and the absolute link is kept.
/// Every link is resolved from the note's old folder: a link to a note without `.md` names that
/// note, with `.md`, and not the note of the same name that the destination folder holds
/// (`archive/other.md`). A percent-encoded link to a note is read literally, as okf-core reads
/// it, so it is rebased as a path and keeps its encoding (`FILE_LINKS_ARCHIVED`).
#[tokio::test]
async fn file_links_survive_a_move_to_another_folder() -> TestResult {
    file_links_moved(
        "archive/a.md",
        FILE_LINKS_ARCHIVED,
        "../notes/pics/p.png",
        "../archive/a.md",
        true,
    )
    .await?;
    Ok(())
}

/// A move three folders deep rebases every relative link the same way.
#[tokio::test]
async fn file_links_survive_a_move_deeper() -> TestResult {
    file_links_moved(
        "archive/2026/q3/a.md",
        "See [Other](../../../notes/other.md) and [part](../../../notes/other.md#part).\n\n\
         ![p](../../../notes/pics/p.png)\n[a](../../../files/a.pdf)\n\
         [s](../../../notes/sub/notes.txt)\n[abs](/files/a.pdf)",
        "../../../notes/pics/p.png",
        "../archive/2026/q3/a.md",
        false,
    )
    .await?;
    Ok(())
}

/// A rename that changes only letter case, which goes by a private name beside the note, leaves
/// every link as it was.
#[tokio::test]
async fn file_links_survive_a_rename_by_letter_case() -> TestResult {
    file_links_moved("notes/A.md", FILE_LINKS, FILE_LINKS_SOURCE, "A.md", false).await?;
    Ok(())
}

/// A Rewind of the move to another folder puts item 10 back at `notes/a.md` with the bytes it
/// held there and item 11's link back to it, and a Rewind to the moved revision puts it back
/// at `archive/a.md` with the bytes it held there; item 12 keeps its bytes throughout. A Rewind
/// moves the item back with its links (`move_back`, through the same rewrite) and then writes
/// the earlier revision's bytes of the item itself, so this pins the outcome; the moved bytes
/// themselves are pinned by `file_links_survive_a_move_to_another_folder`.
#[tokio::test]
async fn file_links_survive_a_rewind_of_a_move() -> TestResult {
    let moves = file_links_moved(
        "archive/a.md",
        FILE_LINKS_ARCHIVED,
        "../notes/pics/p.png",
        "../archive/a.md",
        true,
    )
    .await?;
    for (space, seeded, moved_on) in moves {
        let back = space
            .commit(4, &moved_on, vec![restore(10, &seeded)?])
            .await?;
        let forth = space
            .commit(5, &back, vec![restore(10, &moved_on)?])
            .await?;
        for (revision, earlier, at) in [
            (&back, &seeded, "notes/a.md"),
            (&forth, &moved_on, "archive/a.md"),
        ] {
            assert_eq!(space.item_at(revision, at).await?, Some(uuid!(10)?));
            for file in [at, "notes/other.md", "deep/x/far.md"] {
                assert_eq!(
                    space.file(revision, file)?,
                    space.file(earlier, file)?,
                    "{file}"
                );
            }
        }
    }
    Ok(())
}

// Self links: a moved note's links to itself follow it.

/// Item 10 at `notes/a.md` with the body `SELF_LINKS`, and item 11 at `notes/other.md` linking
/// to it, with item 10 moved to `to`. Item 10's file is the seeded one with the body `body`,
/// byte for byte; item 11's link to it reads `link`; no private name is in the tree. Answers the
/// workspace and the revisions before and after.
async fn self_links_moved(
    to: &str,
    body: &str,
    link: &str,
) -> Fallible<(Workspace, Revision, Revision)> {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "notes/a.md", SELF_LINKS)?,
                note(11, "notes/other.md", "Back to [A](a.md).")?,
            ],
        )
        .await?;
    let moved_on = space.commit(3, &seeded, vec![moved(10, to)?]).await?;
    let before = String::from_utf8(some(space.file(&seeded, "notes/a.md")?, "item 10")?)?;
    let after = String::from_utf8(some(space.file(&moved_on, to)?, "item 10 moved")?)?;
    assert_eq!(after, before.replace(SELF_LINKS, body), "{to}");
    assert_eq!(
        space.body(&moved_on, 11).await?,
        format!("Back to [A]({link}).")
    );
    let files = space.files(&moved_on)?;
    assert!(
        !files.iter().any(|file| file.contains("okf-jawn-")),
        "{files:?}"
    );
    Ok((space, seeded, moved_on))
}

/// A move within the note's folder: every link to itself names its new name, the relative ones
/// beside it, the absolute ones from the root, each with its anchor; the pure anchor is kept.
#[tokio::test]
async fn self_links_follow_a_move_within_its_folder() -> TestResult {
    self_links_moved(
        "notes/b.md",
        "Me [m](b.md), [s](b.md#sec), [d](b.md#sec), [up](b.md), [abs](/notes/b.md), \
         [abs2](/notes/b.md#sec), [bare](b.md), [x](#sec).",
        "b.md",
    )
    .await?;
    Ok(())
}

/// A move to another folder: every relative link to itself names the new file beside it, and
/// every absolute one the new folder.
#[tokio::test]
async fn self_links_follow_a_move_to_another_folder() -> TestResult {
    self_links_moved(
        "archive/a.md",
        "Me [m](a.md), [s](a.md#sec), [d](a.md#sec), [up](a.md), [abs](/archive/a.md), \
         [abs2](/archive/a.md#sec), [bare](a.md), [x](#sec).",
        "../archive/a.md",
    )
    .await?;
    Ok(())
}

/// A rename that changes only letter case, which goes by a private name beside the note: every
/// link to itself names the destination, never the private name its first hop moved it to.
#[tokio::test]
async fn self_links_follow_a_rename_by_letter_case() -> TestResult {
    self_links_moved(
        "notes/A.md",
        "Me [m](A.md), [s](A.md#sec), [d](A.md#sec), [up](A.md), [abs](/notes/A.md), \
         [abs2](/notes/A.md#sec), [bare](A.md), [x](#sec).",
        "A.md",
    )
    .await?;
    Ok(())
}

/// A Rewind of the move to another folder, and of the rename by letter case, puts item 10 back
/// at `notes/a.md` with the bytes it held there (its links to `a.md`) and item 11's link back to
/// it; a Rewind to the moved revision puts the moved bytes back. As in
/// `file_links_survive_a_rewind_of_a_move`, the Rewind writes the earlier revision's bytes of
/// the item after it moves the item back, so this pins the outcome; the moved bytes are pinned
/// by the two moves it starts from.
#[tokio::test]
async fn self_links_follow_a_rewind_of_a_move() -> TestResult {
    for (to, body, link) in [
        (
            "archive/a.md",
            "Me [m](a.md), [s](a.md#sec), [d](a.md#sec), [up](a.md), [abs](/archive/a.md), \
             [abs2](/archive/a.md#sec), [bare](a.md), [x](#sec).",
            "../archive/a.md",
        ),
        (
            "notes/A.md",
            "Me [m](A.md), [s](A.md#sec), [d](A.md#sec), [up](A.md), [abs](/notes/A.md), \
             [abs2](/notes/A.md#sec), [bare](A.md), [x](#sec).",
            "A.md",
        ),
    ] {
        let (space, seeded, moved_on) = self_links_moved(to, body, link).await?;
        let back = space
            .commit(4, &moved_on, vec![restore(10, &seeded)?])
            .await?;
        let forth = space
            .commit(5, &back, vec![restore(10, &moved_on)?])
            .await?;
        for (revision, earlier, at) in [(&back, &seeded, "notes/a.md"), (&forth, &moved_on, to)] {
            assert_eq!(space.item_at(revision, at).await?, Some(uuid!(10)?));
            for file in [at, "notes/other.md"] {
                assert_eq!(
                    space.file(revision, file)?,
                    space.file(earlier, file)?,
                    "{to}: {file}"
                );
            }
            let files = space.files(revision)?;
            assert!(
                !files.iter().any(|file| file.contains("okf-jawn-")),
                "{files:?}"
            );
        }
    }
    Ok(())
}

// S2: a folder spelled two ways.

/// `Notes/a.md` (item 10), `Notes/Sub/c.md` (item 13) and `b.md` (item 11).
async fn notes_folder(space: &Workspace) -> Fallible<Revision> {
    Ok(space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "Notes/a.md", "Ten.")?,
                note(11, "b.md", "Eleven.")?,
                note(13, "Notes/Sub/c.md", "Thirteen.")?,
            ],
        )
        .await?)
}

/// No file of `files` is under a spelling of `Notes/` or `Notes/Sub/` other than those.
fn one_spelling(files: &[String]) {
    for file in files {
        let folder = file.split('/').next().unwrap_or_default();
        assert!(
            !folder.eq_ignore_ascii_case("notes") || folder == "Notes",
            "{files:?}"
        );
        let below = file.strip_prefix("Notes/").unwrap_or_default();
        let sub = below.split('/').next().unwrap_or_default();
        assert!(
            !below.contains('/') || !sub.eq_ignore_ascii_case("sub") || sub == "Sub",
            "{files:?}"
        );
    }
}

/// A note created, moved or written as a source card into a folder the tree spells another
/// way is refused naming the tree's spelling and the field; the same writes in the tree's
/// spelling land there.
#[tokio::test]
async fn a_note_written_into_a_folder_spelled_another_way_is_refused() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = notes_folder(&space).await?;
    let created = space
        .refused(3, &seeded, vec![note(12, "notes/new.md", "Twelve.")?])
        .await?;
    assert_spelled(&created, "Notes/", "/path");
    let nested = space
        .refused(4, &seeded, vec![note(12, "Notes/sub/new.md", "Twelve.")?])
        .await?;
    assert_spelled(&nested, "Notes/Sub/", "/path");
    let moving = space
        .refused(5, &seeded, vec![moved(11, "NOTES/b.md")?])
        .await?;
    assert_spelled(&moving, "Notes/", "/destination");
    let carded = space
        .refused(6, &seeded, vec![card(14, "notes/scan-pdf.md")?])
        .await?;
    assert_spelled(&carded, "Notes/", "/path");
    let landed = space
        .commit(
            7,
            &seeded,
            vec![
                note(12, "Notes/new.md", "Twelve.")?,
                note(15, "Notes/Sub/new.md", "Fifteen.")?,
                moved(11, "Notes/b.md")?,
                card(14, "Notes/scan-pdf.md")?,
            ],
        )
        .await?;
    let files = space.files(&landed)?;
    for at in [
        "Notes/new.md",
        "Notes/Sub/new.md",
        "Notes/b.md",
        "Notes/scan-pdf.md",
    ] {
        assert!(files.iter().any(|file| file == at), "{at}: {files:?}");
    }
    one_spelling(&files);
    Ok(())
}

/// A folder created inside, or as, a folder the tree spells another way is refused naming the
/// tree's spelling; the exact spelling of an existing folder is taken, as before; new folders in
/// the tree's spelling are created.
#[tokio::test]
async fn a_folder_created_in_or_as_a_folder_spelled_another_way_is_refused() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = notes_folder(&space).await?;
    for (mutation, at, spelling) in [
        (3, "notes/archive", "Notes/"),
        (4, "NOTES", "Notes/"),
        (5, "Notes/SUB", "Notes/Sub/"),
        (6, "Notes/sub/deep", "Notes/Sub/"),
    ] {
        let error = space.refused(mutation, &seeded, vec![folder(at)?]).await?;
        assert_spelled(&error, spelling, "/folder");
    }
    let taken = space.refused(7, &seeded, vec![folder("Notes")?]).await?;
    assert_conflict_naming(&taken, &["Notes", "already taken"]);
    let landed = space
        .commit(
            8,
            &seeded,
            vec![folder("Notes/archive")?, folder("Notes/Sub/deep")?],
        )
        .await?;
    let files = space.files(&landed)?;
    for at in ["Notes/archive/index.md", "Notes/Sub/deep/index.md"] {
        assert!(files.iter().any(|file| file == at), "{at}: {files:?}");
    }
    one_spelling(&files);
    Ok(())
}

/// Item 20 at `docs/a.md` and item 21 at `docs/b.md` (`early`), then item 20 moved to `x.md`
/// (`away`).
async fn docs_moved_away(space: &Workspace) -> Fallible<(Revision, Revision)> {
    let early = space
        .commit(
            2,
            &space.initial,
            vec![
                note(20, "docs/a.md", "Twenty.")?,
                note(21, "docs/b.md", "21.")?,
            ],
        )
        .await?;
    let away = space.commit(3, &early, vec![moved(20, "x.md")?]).await?;
    Ok((early, away))
}

/// The folder `docs/` of `away` renamed `Docs/` straight through Git (no edit renames a
/// folder yet), committed on top of it.
fn respell_docs(space: &Workspace, away: &Revision) -> Fallible<Revision> {
    let mut written = Vec::new();
    for name in ["b.md", "index.md"] {
        let bytes = some(space.file(away, &format!("docs/{name}"))?, name)?;
        written.push((format!("Docs/{name}"), bytes));
    }
    let written: Vec<(&str, Vec<u8>)> = written
        .iter()
        .map(|(at, bytes)| (at.as_str(), bytes.clone()))
        .collect();
    space.raw(900, &["docs/b.md", "docs/index.md"], &written)
}

/// A Rewind of an item to a path whose folder the tree now spells another way is refused
/// naming that spelling, whether the item moves back from where it is or is written back after
/// a delete; while the tree spells the folder as the earlier path does, the same Rewind lands.
#[tokio::test]
async fn a_rewind_into_a_folder_now_spelled_another_way_is_refused() -> TestResult {
    let space = Workspace::new().await?;
    let (early, away) = docs_moved_away(&space).await?;
    let back = space.commit(4, &away, vec![restore(20, &early)?]).await?;
    assert_eq!(space.item_at(&back, "docs/a.md").await?, Some(uuid!(20)?));
    let space = Workspace::new().await?;
    let (early, away) = docs_moved_away(&space).await?;
    let respelled = respell_docs(&space, &away)?;
    let moving_back = space
        .refused(4, &respelled, vec![restore(20, &early)?])
        .await?;
    assert_spelled(&moving_back, "Docs/", "/target");
    let gone = space.commit(5, &respelled, vec![deleted(20)?]).await?;
    let written_back = space.refused(6, &gone, vec![restore(20, &early)?]).await?;
    assert_spelled(&written_back, "Docs/", "/target");
    Ok(())
}

/// `RestorePaths` refuses a listed path whose folder the tree spells another way, naming the
/// listing's index, and a listed file the tree holds spelled another way (removing or writing
/// it would reach that file only on a filesystem that holds both spellings as one); while the
/// tree spells the folder as the listing does, the same restore lands.
#[tokio::test]
async fn restored_paths_spelled_another_way_are_refused() -> TestResult {
    let space = Workspace::new().await?;
    let (early, away) = docs_moved_away(&space).await?;
    let request = ["x.md", "docs/a.md"];
    let back = space
        .commit(4, &away, vec![restore_paths(&early, &request)?])
        .await?;
    assert_eq!(space.item_at(&back, "docs/a.md").await?, Some(uuid!(20)?));
    let space = Workspace::new().await?;
    let (early, away) = docs_moved_away(&space).await?;
    let respelled = respell_docs(&space, &away)?;
    let into = space
        .refused(4, &respelled, vec![restore_paths(&early, &request)?])
        .await?;
    assert_spelled(&into, "Docs/", "/paths/1");
    let held = space
        .commit(5, &respelled, vec![note(22, "Docs/A.md", "Twenty-two.")?])
        .await?;
    let removing = space
        .refused(6, &held, vec![restore_paths(&early, &["Docs/a.md"])?])
        .await?;
    assert_conflict_naming(&removing, &["Docs/a.md", "Docs/A.md"]);
    assert_eq!(removing.field.as_deref(), Some("/paths/0"));
    assert_eq!(space.item_at(&held, "Docs/A.md").await?, Some(uuid!(22)?));
    Ok(())
}

/// `RestorePaths` of a path below a folder the tree now holds as a file, in another spelling
/// or the same, at the root or below another folder, is refused alike on every system, before
/// anything below the file is read: `Conflict` on the listing's field, naming the path and the
/// file.
#[tokio::test]
async fn restored_paths_below_a_file_are_refused_naming_it() -> TestResult {
    for (folder, file) in [
        ("notes", "NOTES"),
        ("notes", "notes"),
        ("docs/notes", "docs/NOTES"),
    ] {
        let space = Workspace::new().await?;
        let restored = format!("{folder}/a.md");
        let early = space
            .commit(2, &space.initial, vec![note(10, &restored, "Ten.")?])
            .await?;
        let index = format!("{folder}/index.md");
        space.raw(900, &[restored.as_str(), index.as_str()], &[])?;
        let filed = space.raw(901, &[], &[(file, b"plain".to_vec())])?;
        let error = space
            .refused(
                3,
                &filed,
                vec![restore_paths(&early, &[restored.as_str()])?],
            )
            .await?;
        assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
        assert_eq!(
            error.field.as_deref(),
            Some("/paths/0"),
            "{}",
            error.message
        );
        assert_eq!(
            error.message,
            format!("{restored}: {file} is a file, not a folder")
        );
    }
    Ok(())
}

// The backstop: a tree that holds two spellings of one path.

/// `seeded` holds `Notes/a.md` (item 10) and `b.md` (item 11); the answer moves `b.md` to
/// `respelled` straight through Git, so the tree holds two spellings of one path.
fn two_spellings(space: &Workspace, seeded: &Revision, respelled: &str) -> Fallible<Revision> {
    let bytes = some(space.file(seeded, "b.md")?, "b.md")?;
    space.raw(900, &["b.md"], &[(respelled, bytes)])
}

/// A tree built in Git with two spellings of one folder (`Notes/` and `notes/`, as a
/// case-sensitive filesystem could leave it) refuses every commit, naming both, before a
/// case-insensitive filesystem could merge them; a whole-tree Rewind to a revision without
/// them is not refused, and repairs it.
#[tokio::test]
async fn a_tree_with_two_spellings_of_a_folder_refuses_every_commit_but_a_whole_rewind()
-> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "Notes/a.md", "Ten.")?,
                note(11, "b.md", "Eleven.")?,
            ],
        )
        .await?;
    let spelled = two_spellings(&space, &seeded, "notes/b.md")?;
    for (mutation, edit) in [
        (3, edited(10, "Changed.")?),
        (4, note(12, "c.md", "Twelve.")?),
        (5, edited(11, "Changed.")?),
    ] {
        let error = space.refused(mutation, &spelled, vec![edit]).await?;
        assert_conflict_naming(&error, &["notes", "Notes", "collides"]);
        // The refusal names both spellings and the repair.
        assert_eq!(
            error.message,
            "notes collides with Notes: the revision holds two spellings of one path, which \
             Windows and macOS keep as one; the workspace can be restored to a revision from \
             before they appeared"
        );
    }
    let repaired = space
        .commit(
            6,
            &spelled,
            vec![TreeEdit::RestoreWorkspace {
                from: seeded.clone(),
            }],
        )
        .await?;
    let files = space.files(&repaired)?;
    assert!(files.iter().any(|file| file == "b.md"), "{files:?}");
    one_spelling(&files);
    assert_eq!(
        space.file(&repaired, "Notes/a.md")?,
        space.file(&seeded, "Notes/a.md")?
    );
    Ok(())
}

/// Two spellings of a folder that are not the immediate folders of any file, `Notes/x/a.md`
/// beside `notes/y/b.md`, are refused like any other: every folder above a file counts.
#[tokio::test]
async fn a_tree_with_two_spellings_of_a_folder_two_levels_up_refuses_a_commit() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "Notes/x/a.md", "Ten.")?,
                note(11, "b.md", "Eleven.")?,
            ],
        )
        .await?;
    let spelled = two_spellings(&space, &seeded, "notes/y/b.md")?;
    let error = space
        .refused(3, &spelled, vec![edited(10, "Changed.")?])
        .await?;
    assert_conflict_naming(&error, &["notes collides with Notes"]);
    Ok(())
}

/// A whole-tree Rewind replaces the base only when it is the commit's first edit: then a base
/// holding two spellings is not staged and the edits after the Rewind land; when another edit
/// comes first, the base is staged and refused.
#[tokio::test]
async fn only_a_first_whole_tree_rewind_spares_a_base_with_two_spellings() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "Notes/a.md", "Ten.")?,
                note(11, "b.md", "Eleven.")?,
            ],
        )
        .await?;
    let spelled = two_spellings(&space, &seeded, "notes/b.md")?;
    let rewind = TreeEdit::RestoreWorkspace {
        from: seeded.clone(),
    };
    let error = space
        .refused(
            3,
            &spelled,
            vec![note(12, "c.md", "Twelve.")?, rewind.clone()],
        )
        .await?;
    assert_conflict_naming(&error, &["notes collides with Notes"]);
    let repaired = space
        .commit(4, &spelled, vec![rewind, note(12, "c.md", "Twelve.")?])
        .await?;
    let files = space.files(&repaired)?;
    assert!(files.iter().any(|file| file == "c.md"), "{files:?}");
    assert!(files.iter().any(|file| file == "b.md"), "{files:?}");
    one_spelling(&files);
    Ok(())
}

/// A tree built in Git with two spellings of one file (`a.md` and `A.md`, two items) refuses
/// every commit naming both, rather than letting a case-insensitive filesystem keep one of
/// them; a whole-tree Rewind repairs it.
#[tokio::test]
async fn a_tree_with_two_spellings_of_a_file_refuses_every_commit_but_a_whole_rewind() -> TestResult
{
    let space = Workspace::new().await?;
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![note(10, "a.md", "Ten.")?, note(11, "b.md", "Eleven.")?],
        )
        .await?;
    let spelled = two_spellings(&space, &seeded, "A.md")?;
    for (mutation, edit) in [(3, edited(10, "Changed.")?), (4, edited(11, "Changed.")?)] {
        let error = space.refused(mutation, &spelled, vec![edit]).await?;
        assert_conflict_naming(&error, &["a.md", "A.md", "collides"]);
    }
    let repaired = space
        .commit(
            5,
            &spelled,
            vec![TreeEdit::RestoreWorkspace {
                from: seeded.clone(),
            }],
        )
        .await?;
    assert_eq!(space.file(&repaired, "A.md")?, None);
    assert_eq!(space.body(&repaired, 10).await?, "Ten.");
    assert_eq!(space.body(&repaired, 11).await?, "Eleven.");
    Ok(())
}

/// A tree built in Git with two files whose names differ only in Unicode normalization (`café`
/// with a precomposed `é`, and `cafe` with a combining accent, as macOS merges them) refuses
/// every commit naming both, escaped so that they do not print alike, and saying that the names
/// differ in how their characters are encoded and that macOS keeps them as one (Windows keeps
/// them apart); two names that differ in letter case as well are said to differ in both. A
/// whole-tree Rewind repairs either.
#[tokio::test]
async fn a_tree_with_two_normalizations_of_a_name_refuses_every_commit() -> TestResult {
    for (composed, decomposed, clash, what) in [
        (
            "notes/caf\u{e9}.txt",
            "notes/cafe\u{301}.txt",
            "notes/caf\\u{e9}.txt collides with notes/cafe\\u{301}.txt",
            "two names that differ only in how their characters are encoded, which macOS keeps as \
             one",
        ),
        (
            "notes/Caf\u{e9}.txt",
            "notes/cafe\u{301}.txt",
            "notes/cafe\\u{301}.txt collides with notes/Caf\\u{e9}.txt",
            "two names that differ in letter case and in how their characters are encoded, which \
             macOS keeps as one",
        ),
    ] {
        let space = Workspace::new().await?;
        let seeded = space
            .commit(2, &space.initial, vec![note(10, "notes/a.md", "Ten.")?])
            .await?;
        let spelled = space.raw(
            900,
            &[],
            &[(composed, b"one".to_vec()), (decomposed, b"two".to_vec())],
        )?;
        for (mutation, edit) in [(3, edited(10, "Changed.")?), (4, set_type("Memo"))] {
            let error = space.refused(mutation, &spelled, vec![edit]).await?;
            assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
            assert_eq!(
                error.message,
                format!(
                    "{clash}: the revision holds {what}; the workspace can be restored to a \
                     revision from before they appeared"
                )
            );
        }
        let repaired = space
            .commit(
                5,
                &spelled,
                vec![TreeEdit::RestoreWorkspace {
                    from: seeded.clone(),
                }],
            )
            .await?;
        assert_eq!(space.files(&repaired)?, space.files(&seeded)?);
    }
    Ok(())
}

/// The Kelvin sign (U+212A) lower-cases to `k`, but NFC makes it `K`: a name holding it is not
/// in NFC, so beside `k` or `K` it is a pair that differs in how its characters are encoded,
/// which macOS keeps as one and NTFS keeps apart, never a pair Windows keeps as one. Beside `k`
/// the two differ in letter case too; beside `K`, only in encoding. Alone, as the `k` of
/// `.okf`, it spells a name the server writes in a way only macOS would merge with it. Each
/// refuses every commit naming both spellings escaped, and a whole-tree Rewind repairs it.
#[tokio::test]
async fn a_name_with_the_kelvin_sign_differs_in_encoding_not_only_in_letter_case() -> TestResult {
    let then = "the workspace can be restored to a revision from before";
    for (written, message) in [
        (
            ["notes/\u{212A}.txt", "notes/k.txt"],
            format!(
                "notes/\\u{{212a}}.txt collides with notes/k.txt: the revision holds two names \
                 that differ in letter case and in how their characters are encoded, which \
                 macOS keeps as one; {then} they appeared"
            ),
        ),
        (
            ["notes/\u{212A}.txt", "notes/K.txt"],
            format!(
                "notes/\\u{{212a}}.txt collides with notes/K.txt: the revision holds two names \
                 that differ only in how their characters are encoded, which macOS keeps as one; \
                 {then} they appeared"
            ),
        ),
        (
            [".o\u{212A}f/x.json", "notes/b.txt"],
            format!(
                ".o\\u{{212a}}f collides with the server's .okf: the revision spells a path the \
                 server writes another way, which macOS would merge with it; {then} \
                 .o\\u{{212a}}f appeared"
            ),
        ),
    ] {
        let space = Workspace::new().await?;
        let seeded = space
            .commit(2, &space.initial, vec![note(10, "notes/a.md", "Ten.")?])
            .await?;
        let [first, second] = written;
        let spelled = space.raw(
            900,
            &[],
            &[(first, b"one".to_vec()), (second, b"two".to_vec())],
        )?;
        let error = space
            .refused(3, &spelled, vec![edited(10, "Changed.")?])
            .await?;
        assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
        assert_eq!(error.message, message);
        let repaired = space
            .commit(
                4,
                &spelled,
                vec![TreeEdit::RestoreWorkspace {
                    from: seeded.clone(),
                }],
            )
            .await?;
        assert_eq!(space.files(&repaired)?, space.files(&seeded)?);
    }
    Ok(())
}

/// A whole-tree Rewind to a revision that holds two spellings of one path, from a head that
/// holds neither, is refused saying that the revision being restored holds them and that an
/// earlier one can be chosen, not that the workspace's own revision does.
#[tokio::test]
async fn a_rewind_to_a_revision_with_two_spellings_says_it_is_the_restored_one() -> TestResult {
    let space = Workspace::new().await?;
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "Notes/a.md", "Ten.")?,
                note(11, "b.md", "Eleven.")?,
            ],
        )
        .await?;
    let spelled = two_spellings(&space, &seeded, "notes/b.md")?;
    let clean = space
        .commit(
            3,
            &spelled,
            vec![TreeEdit::RestoreWorkspace {
                from: seeded.clone(),
            }],
        )
        .await?;
    let error = space
        .refused(
            4,
            &clean,
            vec![TreeEdit::RestoreWorkspace {
                from: spelled.clone(),
            }],
        )
        .await?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    assert_eq!(
        error.message,
        "notes collides with Notes: the revision being restored holds two spellings of one path, \
         which Windows and macOS keep as one; choose a revision from before they appeared"
    );
    Ok(())
}

// R1: every path the server writes has one spelling.

/// Respellings of the names the server writes, for a tree seeded with `Notes/a.md`, a card of
/// item `LETTERED` and a type: each is `[from, to, tree, server]` for `respelling`, with the
/// spelling the refusal names first and the server's spelling it names second.
fn server_name_cases() -> Vec<[String; 4]> {
    let upper = LETTERED.to_uppercase();
    let digest = "c".repeat(64);
    let upper_digest = digest.to_uppercase();
    let fixed = [
        [".okf/", ".OKF/", ".OKF", ".okf"],
        [
            ".okf/types.json",
            ".okf/Types.json",
            ".okf/Types.json",
            ".okf/types.json",
        ],
        [
            ".okf/rules.yaml",
            ".okf/Rules.yaml",
            ".okf/Rules.yaml",
            ".okf/rules.yaml",
        ],
        [
            ".okf/sources/",
            ".okf/Sources/",
            ".okf/Sources",
            ".okf/sources",
        ],
        [
            "Notes/index.md",
            "Notes/INDEX.md",
            "Notes/INDEX.md",
            "Notes/index.md",
        ],
        ["log.md", "LOG.md", "LOG.md", "log.md"],
    ];
    let mut cases: Vec<[String; 4]> = fixed.iter().map(|case| case.map(str::to_owned)).collect();
    cases.extend([
        [
            ".okf/supplied/".to_owned(),
            format!(".okf/Supplied/{LETTERED}.json"),
            ".okf/Supplied".to_owned(),
            ".okf/supplied".to_owned(),
        ],
        [
            ".okf/corrections/".to_owned(),
            format!(".okf/Corrections/{LETTERED}/{digest}.json"),
            ".okf/Corrections".to_owned(),
            ".okf/corrections".to_owned(),
        ],
        [
            format!(".okf/sources/{LETTERED}"),
            format!(".okf/sources/{upper}"),
            format!(".okf/sources/{upper}.json"),
            format!(".okf/sources/{LETTERED}.json"),
        ],
        [
            ".okf/supplied/".to_owned(),
            format!(".okf/supplied/{upper}.json"),
            format!(".okf/supplied/{upper}.json"),
            format!(".okf/supplied/{LETTERED}.json"),
        ],
        [
            ".okf/corrections/".to_owned(),
            format!(".okf/corrections/{upper}/{digest}.json"),
            format!(".okf/corrections/{upper}"),
            format!(".okf/corrections/{LETTERED}"),
        ],
        [
            ".okf/corrections/".to_owned(),
            format!(".okf/corrections/{LETTERED}/{upper_digest}.json"),
            format!(".okf/corrections/{LETTERED}/{upper_digest}.json"),
            format!(".okf/corrections/{LETTERED}/{digest}.json"),
        ],
    ]);
    cases
}

/// The removals and writes that respell `revision`'s paths: every file whose path starts with
/// `from` is moved to start with `to` instead, with its bytes; with no such file, the path `to`
/// is written with the bytes `{}`.
fn respelling(
    space: &Workspace,
    revision: &Revision,
    from: &str,
    to: &str,
) -> Fallible<GitChanges> {
    let mut removed = Vec::new();
    let mut written = Vec::new();
    for file in space.files(revision)? {
        if let Some(rest) = file.strip_prefix(from) {
            written.push((
                format!("{to}{rest}"),
                some(space.file(revision, &file)?, "a file")?,
            ));
            removed.push(file);
        }
    }
    if written.is_empty() {
        written.push((to.to_owned(), b"{}".to_vec()));
    }
    Ok((removed, written))
}

/// A revision that spells a name the server writes another way, written straight through Git
/// as an earlier build or an archive could have left it, refuses every commit on every system,
/// naming its spelling, the server's and the repair, before the server's write could land in
/// it (Windows, macOS) or beside it (Linux); a whole-tree Rewind to a revision without it lands.
/// Each fixed name of `SERVER_NAMES` is respelled, as a folder or as a file, and so are the
/// names the server makes of an item's identity and a digest.
#[tokio::test]
async fn a_revision_spelling_a_server_name_another_way_is_refused_naming_the_repair() -> TestResult
{
    let space = Workspace::new().await?;
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "Notes/a.md", "Ten.")?,
                card_of(LETTERED, "one.md")?,
                set_type("Memo"),
            ],
        )
        .await?;
    let mut head = seeded.clone();
    for (case, [from, to, tree, server]) in (0_u32..).zip(server_name_cases()) {
        let mutation = case.saturating_mul(10).saturating_add(100);
        let (removed, written) = respelling(&space, &head, &from, &to)?;
        let removed: Vec<&str> = removed.iter().map(String::as_str).collect();
        let written: Vec<(&str, Vec<u8>)> = written
            .iter()
            .map(|(at, bytes)| (at.as_str(), bytes.clone()))
            .collect();
        let respelled = space.raw(mutation, &removed, &written)?;
        for (offset, edit) in [(1, note(20, "z.md", "Twenty.")?), (2, set_type("Letter"))] {
            let error = space
                .refused(mutation.saturating_add(offset), &respelled, vec![edit])
                .await?;
            assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
            assert_eq!(
                error.message,
                format!(
                    "{tree} collides with the server's {server}: the revision spells a path the \
                     server writes another way, which Windows and macOS would merge with it; the \
                     workspace can be restored to a revision from before {tree} appeared"
                )
            );
        }
        head = space
            .commit(
                mutation.saturating_add(3),
                &respelled,
                vec![TreeEdit::RestoreWorkspace {
                    from: seeded.clone(),
                }],
            )
            .await?;
        assert_eq!(space.files(&head)?, space.files(&seeded)?, "{tree}");
    }
    Ok(())
}

/// A Rewind of an item to a revision whose corrections of it spell a digest's name another way
/// is refused naming the corrections' workspace path, the server's spelling, that the revision
/// being restored holds it, and the repair: the corrections folder is checked by its workspace
/// paths before it is written.
#[tokio::test]
async fn a_rewind_to_respelled_corrections_is_refused_naming_the_repair() -> TestResult {
    let space = Workspace::new().await?;
    space
        .commit(2, &space.initial, vec![card_of(LETTERED, "one.md")?])
        .await?;
    let digest = "c".repeat(64);
    let upper = digest.to_uppercase();
    let respelled = format!(".okf/corrections/{LETTERED}/{upper}.json");
    let early = space.raw(900, &[], &[(respelled.as_str(), b"{}".to_vec())])?;
    let clean = space.raw(901, &[respelled.as_str()], &[])?;
    let error = space
        .refused(
            3,
            &clean,
            vec![TreeEdit::RestoreItem {
                item_id: serde_json::from_value(json!(LETTERED))?,
                from: early,
            }],
        )
        .await?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    assert_eq!(
        error.message,
        format!(
            "{respelled} collides with the server's .okf/corrections/{LETTERED}/{digest}.json: \
             the revision being restored spells a path the server writes another way, which \
             Windows and macOS would merge with it; choose a revision from before {respelled} \
             appeared"
        )
    );
    Ok(())
}

/// A restore of paths that writes a name the server writes in another spelling, into a tree
/// that holds neither spelling, is refused naming both spellings: the staged copy is checked
/// for it after the edits, so no commit can hold it.
#[tokio::test]
async fn restored_paths_spelling_a_server_name_another_way_are_refused() -> TestResult {
    let space = Workspace::new().await?;
    space
        .commit(2, &space.initial, vec![note(10, "a.md", "Ten.")?])
        .await?;
    let early = space.raw(900, &[], &[(".OKF/x.json", b"{}".to_vec())])?;
    let clean = space.raw(901, &[".OKF/x.json"], &[])?;
    let error = space
        .refused(3, &clean, vec![restore_paths(&early, &[".OKF/x.json"])?])
        .await?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    assert_eq!(error.message, ".OKF collides with the server's .okf");
    Ok(())
}

/// A restore of paths that leaves a folder `newdir/INDEX.md/` beside a note of `newdir/`, a
/// folder the tree did not hold, is refused naming both spellings before the folder indexes are
/// regenerated, on every system: on a filesystem that holds `INDEX.md` and `index.md` as one the
/// new folder's index could not be written, and on one that holds them apart the commit would
/// hold both. The staged copy is checked once, after the edits (the review's R1-d probe).
#[tokio::test]
async fn restored_paths_leaving_a_respelled_index_are_refused_before_the_indexes() -> TestResult {
    let space = Workspace::new().await?;
    space
        .commit(2, &space.initial, vec![note(10, "a.md", "Ten.")?])
        .await?;
    let restored = ["newdir/a.md", "newdir/INDEX.md/x.json"];
    let early = space.raw(
        900,
        &[],
        &[
            ("newdir/a.md", b"A plain note.".to_vec()),
            ("newdir/INDEX.md/x.json", b"{}".to_vec()),
        ],
    )?;
    let clean = space.raw(901, &restored, &[])?;
    let error = space
        .refused(3, &clean, vec![restore_paths(&early, &restored)?])
        .await?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    assert_eq!(
        error.message,
        "newdir/INDEX.md collides with the server's newdir/index.md"
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
