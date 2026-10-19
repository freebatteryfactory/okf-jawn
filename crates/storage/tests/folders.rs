//! A folder as a whole against real Git repositories, on the filesystem the tests run on: the
//! store's `TreeEdit::MoveFolder` (every link resolving after, the files that are not items, a
//! rename by letter case, the refusals that need the tree, what the check is told) and
//! `TreeEdit::DeleteFolder` (every item with its companion files, the other files, the
//! refusals), and `move_folder` and `delete_folder` through `ApplicationService` composed over
//! the same data directory and driven through core's dispatch, as `purge_ledger_service.rs`
//! drives it: a folder of items with links in and out, a rename that changes only letter case,
//! a conflict from a change under the folder and none from a change elsewhere, and a delete
//! whose items Rewind brings back with their history.
//!
//! Windows and macOS hold `Notes/` and `notes/` as one folder; Linux and Git hold two. Every
//! assertion reads the committed tree straight from Git, so each holds on both kinds of
//! filesystem. A tree no edit can build (a plain file, a Markdown file without the application
//! header, a folder spelled another way) is committed straight through `git2`.
//!
//! Authorization is held open (`OpenAccess`), as in `purge_ledger_service.rs`: what these tests
//! check is what the service and the store do with a folder, whatever authorization lets
//! through. No job runs here (`NoQueue` and `NoConverter` refuse every call).
#![cfg(feature = "runtime")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use git2::Repository;
use okf_core::{Document, Link, LinkKind, LinkRewriteAction, rewrite_markdown_links};
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{JobId, Revision, TenantId, WorkspaceId, WorkspacePath};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::application::{ApplicationConfig, ApplicationService, Ports};
use okf_jawn_core::context::{TenantGrant, WorkspaceGrant};
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, Converter, ConverterLimits, PageCount,
};
use okf_jawn_core::dispatch::{Caller, DispatchPorts, dispatch};
use okf_jawn_core::jobs::{JobQueue, JobScope};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    CandidateCheck, CommitChanges, Committed, DiffQuery, FOLDER_OTHER_FILES, LocalSource,
    NewWorkspace, Provenance, SourceCard, StorageScope, TreeEdit, VersionStore, WorkspaceCatalog,
};
use okf_jawn_storage::Storage;
use serde_json::{Value, json};

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

/// `ApplicationService` over a workspace's data directory, called through dispatch.
struct Service {
    workspace: Workspace,
    service: ApplicationService,
}

/// Grants every permission on every workspace and on the tenant.
struct OpenAccess;

/// No job is queued by the operations these tests call.
struct NoQueue;

/// No conversion runs in these tests.
struct NoConverter;

const EVERY: [Permission; 6] = [
    Permission::Read,
    Permission::Write,
    Permission::Propose,
    Permission::Approve,
    Permission::Review,
    Permission::Admin,
];
/// The bytes of the plain files the link tests move, which are not UTF-8.
const PICTURE: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0x00, 0xff, 0xfe];
const REPORT: &[u8] = b"%PDF-1.4 not text \xff";
/// The private folder a rename of a folder by letter case parks it under first.
const PARKED: &str = "okf-jawn-recase-folder";
/// The bodies `linked_folder`'s notes hold once `notes/` has moved to `archive/notes/`: every
/// link names its target where it now is, in the form a move of one note writes.
const LINKED_MOVED: [(u32, &str, &str); 4] = [
    (
        10,
        "archive/notes/a.md",
        "A to [b](b.md), [c](sub/c.md#part), [abs](/archive/notes/b.md), out \
         [e](../../elsewhere/e.md), ![p](pics/p.png), [self](a.md).",
    ),
    (
        11,
        "archive/notes/b.md",
        "B to [a](a.md) and [c](sub/c.md).",
    ),
    (
        12,
        "archive/notes/sub/c.md",
        "C to [a](../a.md) and [pdf](../files/r.pdf).",
    ),
    (
        14,
        "elsewhere/e.md",
        "E to [a](../archive/notes/a.md), [c](/archive/notes/sub/c.md), \
         ![p](../archive/notes/pics/p.png), [r](../archive/notes/files/r.pdf).",
    ),
];

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
                    creator: person(),
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
    ) -> Result<Committed, ApiError> {
        self.commit_with(mutation, expected, edits, Arc::new(Accept))
            .await
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
            author: person(),
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
            self.head().await?,
            *head,
            "a refusal commits nothing: {error}"
        );
        Ok(error)
    }

    async fn head(&self) -> Result<Revision, ApiError> {
        self.storage.versions().head(&self.scope).await
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

    /// Every file path of `revision`, read straight from Git, in path order.
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
        found.sort();
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

    /// The parents of the commit `revision`.
    fn parents(&self, revision: &Revision) -> Fallible<Vec<Revision>> {
        let repository = self.repository()?;
        let commit = repository.find_commit(git2::Oid::from_str(revision.as_str())?)?;
        commit
            .parent_ids()
            .map(|id| Ok(Revision::try_from(id.to_string())?))
            .collect()
    }

    /// Commit on main straight through `git2`, as no store edit can: write each of `written`.
    fn raw(&self, mutation: u32, written: &[(&str, &[u8])]) -> Fallible<Revision> {
        let repository = self.repository()?;
        let parent = repository.find_commit(repository.refname_to_id("refs/heads/main")?)?;
        let mut update = git2::build::TreeUpdateBuilder::new();
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

    /// The body of item `item` at `revision`, without its last line break.
    async fn body(&self, revision: &Revision, item: u32) -> Fallible<String> {
        Ok(self
            .storage
            .versions()
            .show(&self.scope, revision, uuid!(item)?)
            .await?
            .body
            .trim_end()
            .to_owned())
    }

    /// The path of item `item` at `revision`.
    async fn path_of(&self, revision: &Revision, item: u32) -> Fallible<String> {
        Ok(self
            .storage
            .versions()
            .show(&self.scope, revision, uuid!(item)?)
            .await?
            .summary
            .path
            .as_str()
            .to_owned())
    }

    /// Every reference of a Markdown file of `revision` (a body link, or a `sources` resource)
    /// that resolves into the tree to no file there, as `file: target`. A link to a concept may
    /// be written with or without `.md` (`Link::resolve` takes one off).
    fn unresolved(&self, revision: &Revision) -> Fallible<Vec<String>> {
        let files: BTreeSet<String> = self.files(revision)?.into_iter().collect();
        let mut broken = Vec::new();
        for file in &files {
            let Some(stem) = file.strip_suffix(".md") else {
                continue;
            };
            if file
                .rsplit('/')
                .next()
                .is_some_and(|name| name == "index.md" || name == "log.md")
            {
                continue;
            }
            let text = String::from_utf8(some(self.file(revision, file)?, "the file")?)?;
            let document = Document::parse(&text)?;
            let id = okf_core::ConceptId::parse(stem)?;
            let mut links = Vec::new();
            rewrite_markdown_links(&document.body, |link, _| {
                links.push(link.clone());
                LinkRewriteAction::Keep
            });
            if let Some(okf_core::Value::Sequence(sources)) = document.frontmatter.get("sources") {
                for source in sources {
                    if let okf_core::Value::Mapping(entry) = source
                        && let Some(okf_core::Value::String(resource)) = entry.get("resource")
                    {
                        links.push(Link {
                            text: String::new(),
                            kind: Link::classify(resource),
                            target: resource.clone(),
                        });
                    }
                }
            }
            for link in links {
                if !matches!(link.kind, LinkKind::Relative | LinkKind::Absolute) {
                    continue;
                }
                let Some(target) = link.resolve(&id) else {
                    continue;
                };
                let target = target.to_string();
                if !files.contains(&format!("{target}.md")) && !files.contains(&target) {
                    broken.push(format!("{file}: {}", link.target));
                }
            }
        }
        Ok(broken)
    }
}

impl Service {
    async fn new() -> Fallible<Self> {
        let workspace = Workspace::new().await?;
        let storage = &workspace.storage;
        let ports = Ports {
            access: Arc::new(OpenAccess),
            mutations: Arc::new(storage.mutations()),
            catalog: Arc::new(storage.catalog()),
            blobs: Arc::new(storage.blobs()),
            versions: Arc::new(storage.versions()),
            records: Arc::new(storage.records()),
            queue: Arc::new(NoQueue),
            search: Arc::new(storage.search()),
            converter: Arc::new(NoConverter),
            proposals: Arc::new(storage.proposals()),
            uploads: Arc::new(storage.uploads()),
            events: Arc::new(storage.events()),
            credentials: Arc::new(storage.credentials()),
            drafts: Arc::new(storage.drafts()),
            confirmations: Arc::new(storage.confirmations()),
            sandbox: Arc::new(storage.sandbox()),
            readiness: Arc::new(storage.readiness()),
        };
        let config = ApplicationConfig::new(
            "https://sandbox.example.test".to_owned(),
            Duration::from_secs(300),
        );
        Ok(Self {
            service: ApplicationService::new(ports, config)?,
            workspace,
        })
    }

    /// Dispatch `operation` as the installation owner in a browser session, awaited inline as a
    /// binding does.
    async fn call(&self, operation: &str, input: Value) -> Result<Value, ApiError> {
        let principal = Principal {
            subject: "ana".to_owned(),
            tenant_id: self.workspace.scope.tenant_id.clone(),
            route: AccessRoute::LocalOwner,
            client_id: None,
            delegation: None,
        };
        let mutations = self.workspace.storage.mutations();
        let events = self.workspace.storage.events();
        let ports = DispatchPorts {
            access: &OpenAccess,
            mutations: &mutations,
            events: &events,
        };
        let caller = Caller {
            principal: &principal,
            session_id: Some("session"),
        };
        dispatch(&self.service, &ports, &caller, operation, input).await
    }

    /// Move `folder` to `destination` through the service, on `base`, under key `key`.
    async fn move_folder(
        &self,
        base: &Revision,
        folder: &str,
        destination: &str,
        key: u32,
    ) -> Result<Value, ApiError> {
        self.call(
            "move_folder",
            json!({
                "workspace_id": self.workspace.scope.workspace_id,
                "base_revision": base,
                "folder": folder,
                "destination": destination,
                "idempotency_key": uuid_text(key),
            }),
        )
        .await
    }
}

impl AccessControl for OpenAccess {
    fn authorize<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
        _permission: Permission,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move { Ok(open_grant(principal, workspace)) })
    }

    fn authorize_tenant<'a>(
        &'a self,
        principal: &'a Principal,
        _permission: Permission,
    ) -> PortFuture<'a, TenantGrant> {
        Box::pin(async move {
            Ok(TenantGrant {
                tenant_id: principal.tenant_id.clone(),
                permissions: EVERY.to_vec(),
            })
        })
    }

    fn grants<'a>(&'a self, _principal: &'a Principal) -> PortFuture<'a, Vec<WorkspaceGrant>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn grant_creator<'a>(
        &'a self,
        principal: &'a Principal,
        workspace: WorkspaceId,
    ) -> PortFuture<'a, WorkspaceGrant> {
        Box::pin(async move { Ok(open_grant(principal, workspace)) })
    }

    fn editors<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Vec<String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

impl JobQueue for NoQueue {
    fn enqueue(&self, _scope: JobScope, _job: JobId) -> PortFuture<'_, ()> {
        Box::pin(async {
            Err(ApiError::new(
                ErrorCode::Internal,
                "no job is queued in these tests",
            ))
        })
    }
}

impl Converter for NoConverter {
    fn page_count<'a>(
        &'a self,
        _source: &'a LocalSource,
        _file_name: &'a str,
    ) -> PortFuture<'a, PageCount> {
        Box::pin(async { Err(no_conversion()) })
    }

    fn convert(&self, _input: ConversionInput) -> PortFuture<'_, Conversion> {
        Box::pin(async { Err(no_conversion()) })
    }

    fn limits(&self) -> ConverterLimits {
        ConverterLimits {
            window_pages: 1,
            memory_limit_bytes: 1,
        }
    }
}

fn open_grant(principal: &Principal, workspace: WorkspaceId) -> WorkspaceGrant {
    WorkspaceGrant {
        scope: StorageScope {
            tenant_id: principal.tenant_id.clone(),
            workspace_id: workspace,
        },
        permissions: EVERY.to_vec(),
    }
}

fn no_conversion() -> ApiError {
    ApiError::new(ErrorCode::Internal, "no conversion runs in these tests")
}

/// A fixed UUID spelling numbered `n`.
fn uuid_text(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

fn person() -> Provenance {
    Provenance {
        subject: "ana".to_owned(),
        route: AccessRoute::LocalOwner,
        client_id: None,
    }
}

fn path(text: &str) -> Fallible<WorkspacePath> {
    Ok(WorkspacePath::try_from(text.to_owned())?)
}

fn note(item: u32, at: &str, body: &str) -> Fallible<TreeEdit> {
    note_with(item, at, body, BTreeMap::new())
}

/// A note with `properties` besides its type and title.
fn note_with(
    item: u32,
    at: &str,
    body: &str,
    properties: BTreeMap<String, Value>,
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

fn moved_folder(folder: &str, destination: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::MoveFolder {
        folder: path(folder)?,
        destination: path(destination)?,
    })
}

/// The `FOLDER_OTHER_FILES` warnings among `warnings`, by message.
fn other_files(warnings: &[Warning]) -> Vec<(String, Option<String>)> {
    warnings
        .iter()
        .filter(|warning| warning.code == FOLDER_OTHER_FILES)
        .map(|warning| (warning.message.clone(), warning.location.clone()))
        .collect()
}

/// The workspace the link tests move `notes/` out of: four items under it (`a`, `b`, `sub/c`
/// and the archived `old`) that link to each other, to `elsewhere/e.md` and to two plain files
/// under the folder; `elsewhere/e.md`, which links into the folder in every style; and the two
/// plain files, written straight through Git. Answers the head.
async fn linked_folder(space: &Workspace) -> Fallible<Revision> {
    let seeded = space
        .commit(
            2,
            &space.initial,
            vec![
                note(
                    10,
                    "notes/a.md",
                    "A to [b](b.md), [c](sub/c.md#part), [abs](/notes/b.md), out \
                     [e](../elsewhere/e.md), ![p](pics/p.png), [self](a.md).",
                )?,
                note(11, "notes/b.md", "B to [a](a.md) and [c](./sub/c.md).")?,
                note(
                    12,
                    "notes/sub/c.md",
                    "C to [a](../a.md) and [pdf](../files/r.pdf).",
                )?,
                note(13, "notes/old.md", "Old.")?,
                note(
                    14,
                    "elsewhere/e.md",
                    "E to [a](../notes/a.md), [c](/notes/sub/c.md), ![p](../notes/pics/p.png), \
                     [r](../notes/files/r.pdf).",
                )?,
            ],
        )
        .await?
        .revision;
    space
        .commit(
            3,
            &seeded,
            vec![TreeEdit::SetStatus {
                item_id: uuid!(13)?,
                status: None,
                archived: Some(true),
            }],
        )
        .await?;
    space.raw(
        4,
        &[("notes/pics/p.png", PICTURE), ("notes/files/r.pdf", REPORT)],
    )
}

// The store's edit.

/// F1: every item and file under the folder moves, every link in and out resolves after, each
/// in the form a move of one note writes it; the plain files keep their bytes; the archived
/// item moves and stays archived; nothing is left at the old path.
#[tokio::test]
async fn move_folder_moves_everything_under_it_and_every_link_still_resolves() -> TestResult {
    let space = Workspace::new().await?;
    let base = linked_folder(&space).await?;
    assert_eq!(
        space.unresolved(&base)?,
        Vec::<String>::new(),
        "the seed resolves"
    );
    let moved = space
        .commit(5, &base, vec![moved_folder("notes", "archive/notes")?])
        .await?
        .revision;
    for (item, at, body) in LINKED_MOVED {
        assert_eq!(space.path_of(&moved, item).await?, at, "{item}");
        assert_eq!(space.body(&moved, item).await?, body, "{item}");
    }
    assert_eq!(space.unresolved(&moved)?, Vec::<String>::new());
    let old = space
        .storage
        .versions()
        .show(&space.scope, &moved, uuid!(13)?)
        .await?
        .summary;
    assert_eq!(old.path.as_str(), "archive/notes/old.md");
    assert!(old.archived, "an archived item moves archived");
    assert_eq!(
        space.file(&moved, "archive/notes/pics/p.png")?.as_deref(),
        Some(PICTURE)
    );
    assert_eq!(
        space.file(&moved, "archive/notes/files/r.pdf")?.as_deref(),
        Some(REPORT)
    );
    let files = space.files(&moved)?;
    assert!(
        files.iter().all(|file| !file.starts_with("notes/")),
        "{files:?}"
    );
    assert!(
        files.contains(&"archive/notes/index.md".to_owned()),
        "{files:?}"
    );
    assert!(
        files.contains(&"archive/notes/sub/index.md".to_owned()),
        "{files:?}"
    );
    assert_eq!(space.parents(&moved)?, vec![base], "one commit");
    Ok(())
}

/// The check is told every file whose links the folder's moves rewrote, a moved file at its new
/// path, and not a moved file nothing rewrote (`old.md`).
#[tokio::test]
async fn move_folder_tells_the_check_every_file_its_moves_rewrote() -> TestResult {
    let space = Workspace::new().await?;
    let base = linked_folder(&space).await?;
    let recording = Arc::new(Recording::default());
    space
        .commit_with(
            5,
            &base,
            vec![moved_folder("notes", "archive/notes")?],
            recording.clone(),
        )
        .await?;
    assert_eq!(
        recording.seen()?,
        vec![vec![
            "archive/notes/a.md".to_owned(),
            "archive/notes/b.md".to_owned(),
            "archive/notes/sub/c.md".to_owned(),
            "elsewhere/e.md".to_owned(),
        ]]
    );
    Ok(())
}

/// F6: one warning says how many files that are not items moved: a plain file, a `X.MD` and a
/// Markdown file without the application header count, the folder indexes the store
/// regenerates do not; a folder of items alone moves with no such warning. A Markdown file
/// without the header moves with its links, as okf-core moves any concept.
#[tokio::test]
async fn move_folder_counts_the_files_that_are_not_items_in_one_warning() -> TestResult {
    let space = Workspace::new().await?;
    space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "notes/a.md", "A.")?,
                note(11, "notes/sub/b.md", "B.")?,
                note(12, "only/c.md", "C.")?,
                note(13, "top.md", "Top [p](notes/plain.md).")?,
            ],
        )
        .await?;
    let head = space.raw(
        3,
        &[
            ("notes/p.png", PICTURE),
            ("notes/sub/X.MD", b"# Plain\n"),
            ("notes/plain.md", b"# Plain\n\nUp [t](../top.md).\n"),
        ],
    )?;
    let moved = space
        .commit(4, &head, vec![moved_folder("notes", "archive/notes")?])
        .await?;
    assert_eq!(
        other_files(&moved.warnings),
        vec![(
            "3 files that are not items moved with the folder to archive/notes".to_owned(),
            Some("archive/notes".to_owned())
        )]
    );
    assert_eq!(
        space
            .file(&moved.revision, "archive/notes/plain.md")?
            .as_deref(),
        Some(b"# Plain\n\nUp [t](../../top.md).\n".as_slice())
    );
    assert_eq!(
        space.body(&moved.revision, 13).await?,
        "Top [p](archive/notes/plain.md)."
    );
    let alone = space
        .commit(5, &moved.revision, vec![moved_folder("only", "elsewhere")?])
        .await?;
    assert_eq!(other_files(&alone.warnings), Vec::new());
    Ok(())
}

/// The frontmatter paths okf-core rebases when it moves a concept follow a plain file the folder
/// move carried, from inside the folder and from outside it.
#[tokio::test]
async fn move_folder_rewrites_frontmatter_paths_to_the_files_it_moved() -> TestResult {
    let space = Workspace::new().await?;
    let sources = |resource: &str| {
        BTreeMap::from([(
            "sources".to_owned(),
            json!([{ "resource": resource, "kind": "file" }]),
        )])
    };
    space
        .commit(
            2,
            &space.initial,
            vec![
                note_with(10, "notes/a.md", "A.", sources("pics/p.png"))?,
                note_with(11, "elsewhere/e.md", "E.", sources("/notes/pics/p.png"))?,
                note_with(12, "elsewhere/f.md", "F.", sources("../notes/pics/p.png"))?,
            ],
        )
        .await?;
    let head = space.raw(3, &[("notes/pics/p.png", PICTURE)])?;
    let moved = space
        .commit(4, &head, vec![moved_folder("notes", "archive/notes")?])
        .await?
        .revision;
    for (item, resource) in [
        (10, "pics/p.png"),
        (11, "/archive/notes/pics/p.png"),
        (12, "../archive/notes/pics/p.png"),
    ] {
        let properties = space
            .storage
            .versions()
            .show(&space.scope, &moved, uuid!(item)?)
            .await?
            .properties;
        assert_eq!(
            properties.get("sources"),
            Some(&json!([{ "resource": resource, "kind": "file" }])),
            "{item}"
        );
    }
    assert_eq!(space.unresolved(&moved)?, Vec::<String>::new());
    Ok(())
}

/// F2: a rename that changes only letter case (`Notes/` to `notes/`) lands as one move: the
/// committed tree spells only the new folder, every link follows, and the private folder it
/// passed through is in no file.
#[tokio::test]
async fn move_folder_renames_a_folder_by_letter_case() -> TestResult {
    let space = Workspace::new().await?;
    space
        .commit(
            2,
            &space.initial,
            vec![
                note(
                    10,
                    "Notes/a.md",
                    "A to [b](b.md), [abs](/Notes/b.md) and ![p](p.png).",
                )?,
                note(11, "Notes/b.md", "B to [a](./a.md#top).")?,
                note(12, "top.md", "Top [a](Notes/a.md) and [b](./Notes/b.md#x).")?,
            ],
        )
        .await?;
    let base = space.raw(3, &[("Notes/p.png", PICTURE)])?;
    let renamed = space
        .commit(4, &base, vec![moved_folder("Notes", "notes")?])
        .await?;
    let files = space.files(&renamed.revision)?;
    assert!(
        files
            .iter()
            .all(|file| !file.starts_with("Notes/") && !file.contains(PARKED)),
        "{files:?}"
    );
    for (item, at, body) in [
        (
            10,
            "notes/a.md",
            "A to [b](b.md), [abs](/notes/b.md) and ![p](p.png).",
        ),
        (11, "notes/b.md", "B to [a](a.md#top)."),
        (12, "top.md", "Top [a](notes/a.md) and [b](notes/b.md#x)."),
    ] {
        assert_eq!(space.path_of(&renamed.revision, item).await?, at, "{item}");
        assert_eq!(space.body(&renamed.revision, item).await?, body, "{item}");
    }
    assert_eq!(
        space.file(&renamed.revision, "notes/p.png")?.as_deref(),
        Some(PICTURE)
    );
    assert_eq!(space.unresolved(&renamed.revision)?, Vec::<String>::new());
    for file in &files {
        let bytes = some(space.file(&renamed.revision, file)?, "the file")?;
        assert!(
            !String::from_utf8_lossy(&bytes).contains(PARKED),
            "{file} names the private folder"
        );
    }
    assert_eq!(space.parents(&renamed.revision)?, vec![base]);
    Ok(())
}

/// A rename by letter case parks past a private folder the tree already holds and past one a
/// link of the tree resolves into: the folder named like the private one is untouched, and the
/// link into the second, which the second hop would otherwise take for its own, keeps its text.
#[tokio::test]
async fn move_folder_parks_past_a_taken_private_folder_and_a_linked_one() -> TestResult {
    let space = Workspace::new().await?;
    space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "Notes/a.md", "A.")?,
                note(11, &format!("{PARKED}/x.md"), "Parked by hand.")?,
                note(12, "top.md", &format!("Top [ghost]({PARKED}-1/a.md)."))?,
            ],
        )
        .await?;
    let base = space.head().await?;
    let renamed = space
        .commit(3, &base, vec![moved_folder("Notes", "notes")?])
        .await?
        .revision;
    assert_eq!(space.path_of(&renamed, 10).await?, "notes/a.md");
    assert_eq!(space.path_of(&renamed, 11).await?, format!("{PARKED}/x.md"));
    assert_eq!(space.body(&renamed, 11).await?, "Parked by hand.");
    assert_eq!(
        space.body(&renamed, 12).await?,
        format!("Top [ghost]({PARKED}-1/a.md).")
    );
    Ok(())
}

/// F5, F7 at the store: what the edit refuses on its own, each before anything is written.
#[tokio::test]
async fn move_folder_refuses_what_the_tree_does_not_allow_and_writes_nothing() -> TestResult {
    let space = Workspace::new().await?;
    space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "notes/a.md", "A.")?,
                note(11, "elsewhere/e.md", "E.")?,
                note(12, "top.md", "Top.")?,
                note(13, "Spelled/s.md", "S.")?,
            ],
        )
        .await?;
    let head = space.head().await?;
    let cases = [
        (".okf", "moved", ErrorCode::InvalidInput, "/folder"),
        (
            "notes/index.md",
            "moved",
            ErrorCode::InvalidInput,
            "/folder",
        ),
        ("missing", "moved", ErrorCode::NotFound, "/folder"),
        ("top.md", "moved", ErrorCode::NotFound, "/folder"),
        ("spelled", "moved", ErrorCode::InvalidInput, "/folder"),
        (
            "notes",
            ".OKF/notes",
            ErrorCode::InvalidInput,
            "/destination",
        ),
        ("notes", "notes", ErrorCode::InvalidInput, "/destination"),
        (
            "notes",
            "notes/sub",
            ErrorCode::InvalidInput,
            "/destination",
        ),
        (
            "notes",
            "NOTES/sub",
            ErrorCode::InvalidInput,
            "/destination",
        ),
        (
            "notes",
            "spelled/notes",
            ErrorCode::InvalidInput,
            "/destination",
        ),
        ("notes", "elsewhere", ErrorCode::Conflict, "/destination"),
        ("notes", "Elsewhere", ErrorCode::Conflict, "/destination"),
        ("notes", "TOP.MD", ErrorCode::Conflict, "/destination"),
        ("notes", "top.md/inner", ErrorCode::Conflict, "/destination"),
    ];
    for (number, (folder, destination, code, field)) in (20..).zip(cases) {
        let case = format!("{folder} to {destination}");
        let refused = space
            .refused(number, &head, vec![moved_folder(folder, destination)?])
            .await?;
        assert_eq!(refused.code, code, "{case}: {}", refused.message);
        assert_eq!(refused.field.as_deref(), Some(field), "{case}");
    }
    let spelled = space
        .refused(40, &head, vec![moved_folder("spelled", "moved")?])
        .await?;
    assert!(
        spelled.message.contains("the folder is spelled Spelled/"),
        "{}",
        spelled.message
    );
    Ok(())
}

// Through the service.

/// F1, F6 through the service: one commit, the items counted (the archived one too), the
/// files that are not items in one warning, every link resolving after, and a draft of a moved
/// item still with it.
#[tokio::test]
async fn move_folder_through_the_service_moves_a_folder_of_linked_items() -> TestResult {
    let service = Service::new().await?;
    let space = &service.workspace;
    let base = linked_folder(space).await?;
    service
        .call(
            "save_draft",
            json!({
                "workspace_id": space.scope.workspace_id,
                "item_id": uuid_text(10),
                "base_revision": base,
                "body": "A draft.",
                "properties": { "type": "Note", "title": "Note 10" },
                "idempotency_key": uuid_text(50),
            }),
        )
        .await?;
    let response = service
        .move_folder(&base, "notes", "archive/notes", 51)
        .await?;
    assert_eq!(response.get("items"), Some(&json!(4)), "{response}");
    let revision: Revision =
        serde_json::from_value(some(response.get("revision"), "the revision")?.clone())?;
    assert_eq!(space.head().await?, revision);
    assert_eq!(space.parents(&revision)?, vec![base.clone()], "one commit");
    let warnings: Vec<Warning> =
        serde_json::from_value(some(response.get("warnings"), "the warnings")?.clone())?;
    assert_eq!(
        other_files(&warnings),
        vec![(
            "2 files that are not items moved with the folder to archive/notes".to_owned(),
            Some("archive/notes".to_owned())
        )]
    );
    for (item, at, body) in LINKED_MOVED {
        assert_eq!(space.path_of(&revision, item).await?, at, "{item}");
        assert_eq!(space.body(&revision, item).await?, body, "{item}");
    }
    assert_eq!(space.unresolved(&revision)?, Vec::<String>::new());
    let read = service
        .call(
            "get_item",
            json!({
                "workspace_id": space.scope.workspace_id,
                "item_id": uuid_text(10),
                "at": { "kind": "latest" },
            }),
        )
        .await?;
    assert_eq!(
        read.pointer("/summary/path"),
        Some(&json!("archive/notes/a.md"))
    );
    assert_eq!(
        read.pointer("/draft/body"),
        Some(&json!("A draft.")),
        "{read}"
    );
    // The same request again answers the first response: one commit.
    let again = service
        .move_folder(&base, "notes", "archive/notes", 51)
        .await?;
    assert_eq!(again, response);
    assert_eq!(space.head().await?, revision);
    Ok(())
}

/// F2 through the service: `Notes/` renamed `notes/` in one commit, every link following.
#[tokio::test]
async fn move_folder_through_the_service_renames_a_folder_by_letter_case() -> TestResult {
    let service = Service::new().await?;
    let space = &service.workspace;
    let base = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "Notes/a.md", "# A\n\nTo [b](b.md).")?,
                note(11, "Notes/b.md", "# B\n\nTo [a](/Notes/a.md).")?,
                note(12, "top.md", "# Top\n\nTo [a](Notes/a.md).")?,
            ],
        )
        .await?
        .revision;
    let response = service.move_folder(&base, "Notes", "notes", 50).await?;
    assert_eq!(response.get("items"), Some(&json!(2)), "{response}");
    let revision: Revision =
        serde_json::from_value(some(response.get("revision"), "the revision")?.clone())?;
    assert_eq!(space.parents(&revision)?, vec![base]);
    for (item, at, body) in [
        (10, "notes/a.md", "# A\n\nTo [b](b.md)."),
        (11, "notes/b.md", "# B\n\nTo [a](/notes/a.md)."),
        (12, "top.md", "# Top\n\nTo [a](notes/a.md)."),
    ] {
        assert_eq!(space.path_of(&revision, item).await?, at, "{item}");
        assert_eq!(space.body(&revision, item).await?, body, "{item}");
    }
    let files = space.files(&revision)?;
    assert!(
        files
            .iter()
            .all(|file| !file.starts_with("Notes/") && !file.contains(PARKED)),
        "{files:?}"
    );
    let log = String::from_utf8(some(space.file(&revision, "log.md")?, "the log")?)?;
    assert_eq!(
        log.matches("Move folder Notes to notes").count(),
        1,
        "{log}"
    );
    assert!(!log.contains(PARKED), "{log}");
    Ok(())
}

/// F4 through the service: a change under the folder since the base conflicts on
/// `/base_revision`, naming the head, and commits nothing.
#[tokio::test]
async fn move_folder_through_the_service_conflicts_on_a_change_under_the_folder() -> TestResult {
    let service = Service::new().await?;
    let space = &service.workspace;
    let base = space
        .commit(2, &space.initial, vec![note(10, "notes/a.md", "# A")?])
        .await?
        .revision;
    let head = space
        .commit(3, &base, vec![note(11, "notes/late.md", "# Late")?])
        .await?
        .revision;
    let refused = err_of(service.move_folder(&base, "notes", "archive", 50).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict, "{}", refused.message);
    assert_eq!(refused.field.as_deref(), Some("/base_revision"));
    assert_eq!(
        refused.detail.as_deref(),
        Some(&ErrorDetail::StaleBase { head: head.clone() })
    );
    assert_eq!(space.head().await?, head, "nothing committed");
    Ok(())
}

/// F4 through the service: a change elsewhere since the base does not block the move, which is
/// committed on the head, and the link that change made into the folder follows the move.
#[tokio::test]
async fn move_folder_through_the_service_commits_past_a_change_elsewhere() -> TestResult {
    let service = Service::new().await?;
    let space = &service.workspace;
    let base = space
        .commit(2, &space.initial, vec![note(10, "notes/a.md", "# A")?])
        .await?
        .revision;
    let head = space
        .commit(
            3,
            &base,
            vec![note(
                11,
                "elsewhere/late.md",
                "# Late\n\nTo [a](../notes/a.md).",
            )?],
        )
        .await?
        .revision;
    let response = service.move_folder(&base, "notes", "archive", 50).await?;
    let revision: Revision =
        serde_json::from_value(some(response.get("revision"), "the revision")?.clone())?;
    assert_eq!(
        space.parents(&revision)?,
        vec![head],
        "committed on the head"
    );
    assert_eq!(space.path_of(&revision, 10).await?, "archive/a.md");
    assert_eq!(
        space.body(&revision, 11).await?,
        "# Late\n\nTo [a](../archive/a.md)."
    );
    Ok(())
}

/// A source card at `at` whose extraction digest is `a`×64, with an appearance the store keeps
/// under `.okf/sources/`.
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

/// F3: every item under the folder goes, each with its companion files (a card's appearance
/// its accepted supplied text and its corrections), then every other file and the folder;
/// what is outside stays; the
/// files that are not items are counted in one warning; the base still holds everything.
#[tokio::test]
async fn delete_folder_removes_everything_under_it_with_the_companions() -> TestResult {
    let space = Workspace::new().await?;
    space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "notes/a.md", "A.")?,
                card(11, "notes/sub/scan.md")?,
                TreeEdit::CorrectDigest {
                    item_id: uuid!(11)?,
                    digest: serde_json::from_value(json!("a".repeat(64)))?,
                    corrected_markdown: "Corrected.".to_owned(),
                    adopts: None,
                },
                TreeEdit::SupplyExtraction {
                    item_id: uuid!(11)?,
                    based_on: Some(serde_json::from_value(json!("a".repeat(64)))?),
                    pages: Vec::new(),
                    markdown: "Accepted supplied text.".to_owned(),
                    proposal_id: uuid!(14)?,
                    supplier: person(),
                },
                note(12, "elsewhere/e.md", "E to [a](../notes/a.md).")?,
                note(13, "notes-old/k.md", "Kept.")?,
            ],
        )
        .await?;
    let base = space.raw(3, &[("notes/p.png", PICTURE)])?;
    let card_files: Vec<String> = space
        .files(&base)?
        .into_iter()
        .filter(|file| file.contains(&uuid_text(11)))
        .collect();
    assert_eq!(
        card_files,
        vec![
            format!(".okf/corrections/{}/{}.json", uuid_text(11), "a".repeat(64)),
            format!(".okf/sources/{}.json", uuid_text(11)),
            format!(".okf/supplied/{}.json", uuid_text(11)),
        ],
        "all three companion kinds are present before deletion"
    );
    let deleted = space
        .commit(
            4,
            &base,
            vec![TreeEdit::DeleteFolder {
                folder: path("notes")?,
            }],
        )
        .await?;
    let files = space.files(&deleted.revision)?;
    assert!(
        files
            .iter()
            .all(|file| !file.starts_with("notes/") && !file.contains(&uuid_text(11))),
        "{files:?}"
    );
    assert!(files.contains(&"elsewhere/e.md".to_owned()), "{files:?}");
    assert!(files.contains(&"notes-old/k.md".to_owned()), "{files:?}");
    assert_eq!(
        other_files(&deleted.warnings),
        vec![(
            "1 file that is not an item was deleted with the folder notes".to_owned(),
            Some("notes".to_owned())
        )]
    );
    for file in card_files {
        assert!(space.file(&base, &file)?.is_some(), "the base keeps {file}");
    }
    assert_eq!(space.body(&base, 10).await?, "A.");
    Ok(())
}

/// F3, F7 at the store: a reserved folder or one spelled another way is `InvalidInput` on
/// `/folder`; a folder the tree does not hold, or a file, `NotFound` on `/folder`; each
/// before anything is written.
#[tokio::test]
async fn delete_folder_refuses_what_the_tree_does_not_allow_and_writes_nothing() -> TestResult {
    let space = Workspace::new().await?;
    space
        .commit(
            2,
            &space.initial,
            vec![note(10, "Notes/a.md", "A.")?, note(11, "top.md", "Top.")?],
        )
        .await?;
    let head = space.head().await?;
    let cases = [
        (".okf", ErrorCode::InvalidInput),
        ("Notes/INDEX.md", ErrorCode::InvalidInput),
        ("notes", ErrorCode::InvalidInput),
        ("missing", ErrorCode::NotFound),
        ("top.md", ErrorCode::NotFound),
    ];
    for (number, (folder, code)) in (20..).zip(cases) {
        let refused = space
            .refused(
                number,
                &head,
                vec![TreeEdit::DeleteFolder {
                    folder: path(folder)?,
                }],
            )
            .await?;
        assert_eq!(refused.code, code, "{folder}: {}", refused.message);
        assert_eq!(refused.field.as_deref(), Some("/folder"), "{folder}");
    }
    Ok(())
}

/// F3 through the service: one commit, the items counted (the archived one too), a draft of a
/// deleted item kept as `delete_item` keeps one, Rewind bringing each item back with its
/// history, and a folder that does not exist `not_found` on `/folder`.
#[tokio::test]
async fn delete_folder_through_the_service_and_rewind_brings_the_items_back() -> TestResult {
    let service = Service::new().await?;
    let space = &service.workspace;
    let created = space
        .commit(
            2,
            &space.initial,
            vec![
                note(10, "notes/a.md", "# A\n\nKept in history.")?,
                note(11, "notes/sub/b.md", "# B")?,
            ],
        )
        .await?
        .revision;
    let base = space
        .commit(
            3,
            &created,
            vec![TreeEdit::SetStatus {
                item_id: uuid!(11)?,
                status: None,
                archived: Some(true),
            }],
        )
        .await?
        .revision;
    let workspace = space.scope.workspace_id;
    service
        .call(
            "save_draft",
            json!({
                "workspace_id": workspace, "item_id": uuid_text(10), "base_revision": base,
                "body": "A draft.", "properties": { "type": "Note", "title": "Note 10" },
                "idempotency_key": uuid_text(50),
            }),
        )
        .await?;
    let response = service
        .call(
            "delete_folder",
            json!({
                "workspace_id": workspace, "base_revision": base, "folder": "notes",
                "idempotency_key": uuid_text(51),
            }),
        )
        .await?;
    assert_eq!(response.get("items"), Some(&json!(2)), "{response}");
    let deleted: Revision =
        serde_json::from_value(some(response.get("revision"), "the revision")?.clone())?;
    assert_eq!(space.parents(&deleted)?, vec![base.clone()]);
    assert!(
        space
            .files(&deleted)?
            .iter()
            .all(|file| !file.starts_with("notes/")),
        "nothing under notes/"
    );
    let drafts = service
        .call(
            "list_drafts",
            json!({ "workspace_id": workspace, "page": { "limit": 20 } }),
        )
        .await?;
    assert_eq!(
        drafts.pointer("/items/0/draft/item_id"),
        Some(&json!(uuid_text(10))),
        "{drafts}"
    );
    assert_eq!(
        drafts.pointer("/items/0/item"),
        None,
        "the item is gone: {drafts}"
    );
    let mut head = deleted;
    for (number, item) in [(52, 10), (53, 11)] {
        let restored = service
            .call(
                "restore_items",
                json!({
                    "workspace_id": workspace, "base_revision": head, "target": base,
                    "item_id": uuid_text(item), "message": "Bring it back",
                    "idempotency_key": uuid_text(number),
                }),
            )
            .await?;
        head = serde_json::from_value(some(restored.get("revision"), "the revision")?.clone())?;
    }
    assert_eq!(space.path_of(&head, 10).await?, "notes/a.md");
    assert_eq!(space.body(&head, 10).await?, "# A\n\nKept in history.");
    let back = space
        .storage
        .versions()
        .show(&space.scope, &head, uuid!(11)?)
        .await?
        .summary;
    assert_eq!(back.path.as_str(), "notes/sub/b.md");
    assert!(back.archived, "it comes back as it was, archived");
    let history = service
        .call(
            "log_items",
            json!({
                "workspace_id": workspace, "item_id": uuid_text(10),
                "at": { "kind": "latest" }, "page": { "limit": 20 },
            }),
        )
        .await?;
    let revisions: Vec<Value> = some(history.get("commits"), "the commits")?
        .as_array()
        .map(|commits| {
            commits
                .iter()
                .filter_map(|commit| commit.get("revision").cloned())
                .collect()
        })
        .unwrap_or_default();
    assert!(revisions.contains(&json!(created)), "{history}");
    let missing = err_of(
        service
            .call(
                "delete_folder",
                json!({
                    "workspace_id": workspace, "base_revision": head, "folder": "gone",
                    "idempotency_key": uuid_text(54),
                }),
            )
            .await,
    )?;
    assert_eq!(missing.code, ErrorCode::NotFound, "{}", missing.message);
    assert_eq!(missing.field.as_deref(), Some("/folder"));
    Ok(())
}

/// F4: changing only a source occurrence's versioned metadata changes its item. Both folder
/// operations must reject the stale base, preserving that newer commit. The raw tree writer
/// and an explicit diff check prove that no visible folder file changed in this setup.
#[tokio::test]
async fn folder_operations_conflict_on_a_contained_sources_metadata_only_change() -> TestResult {
    for operation in ["move_folder", "delete_folder"] {
        let service = Service::new().await?;
        let space = &service.workspace;
        let base = space
            .commit(2, &space.initial, vec![card(10, "notes/source.md")?])
            .await?
            .revision;
        let companion = format!(".okf/sources/{}.json", uuid_text(10));
        let before = some(space.file(&base, &companion)?, "the source metadata")?;
        let mut metadata: Value = serde_json::from_slice(&before)?;
        some(
            metadata.get_mut("metadata").and_then(Value::as_object_mut),
            "the source's metadata map",
        )?
        .insert(
            "operator_note".to_owned(),
            json!("Newer occurrence metadata."),
        );
        let changed = serde_json::to_vec(&metadata)?;
        let head = space.raw(3, &[(companion.as_str(), changed.as_slice())])?;
        let diff = space
            .storage
            .versions()
            .diff(
                &space.scope,
                DiffQuery {
                    from: base.clone(),
                    to: head.clone(),
                    item_id: None,
                },
            )
            .await?;
        assert_eq!(diff.changes.len(), 1);
        assert_eq!(
            diff.changes
                .first()
                .and_then(|change| change.new_path.as_deref()),
            Some(companion.as_str())
        );
        assert_eq!(
            space.file(&base, "notes/source.md")?,
            space.file(&head, "notes/source.md")?,
            "the source card itself is unchanged"
        );
        let mut request = json!({
            "workspace_id": space.scope.workspace_id,
            "base_revision": base,
            "folder": "notes",
            "idempotency_key": uuid_text(50),
        });
        if operation == "move_folder" {
            some(request.as_object_mut(), "the request")?
                .insert("destination".to_owned(), json!("archive"));
        }
        let error = err_of(service.call(operation, request).await)?;
        assert_eq!(error.code, ErrorCode::Conflict, "{operation}: {error:?}");
        assert_eq!(error.field.as_deref(), Some("/base_revision"));
        assert_eq!(
            error.detail.as_deref(),
            Some(&ErrorDetail::StaleBase { head: head.clone() })
        );
        assert_eq!(space.head().await?, head, "{operation} writes nothing");
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
