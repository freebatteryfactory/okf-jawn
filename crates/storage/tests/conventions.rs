//! The version store's naming-rules ports against real Git repositories: the rules file read
//! with its text and the reader's error (`VersionStore::rules`), and the whole tree in one pass
//! (`VersionStore::tree`), and the old path a move records as an alias
//! (`TreeEdit::MoveItem::keep_alias`). Named `conventions_*`.
//! A preview/apply journey also runs the production application over the real adapters,
//! proving that an item can vacate the folder path another planned move needs.
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use git2::Repository;
use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::conventions::{
    ApplyNamesRequest, CollisionPolicy, DateOrder, ExtensionPolicy, LetterCase, NameSelection,
    NamingRule, NamingRules, PreviewNamesRequest,
};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{At, ItemId, JobId, Revision, TenantId, WorkspacePath};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::application::{ApplicationConfig, ApplicationService, Ports};
use okf_jawn_core::context::{Attempt, OperationContext, WorkspaceGrant};
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, Converter, ConverterLimits, PageCount,
};
use okf_jawn_core::jobs::{JobQueue, JobScope};
use okf_jawn_core::ports::{Application, PortFuture};
use okf_jawn_core::storage::{
    CandidateCheck, CommitChanges, LocalSource, NewWorkspace, Provenance, SourceCard, StorageScope,
    TreeEdit, VersionStore, WorkspaceCatalog,
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

/// Preview/apply names never starts a job or converts bytes; fail if the journey does either.
struct NoJobs;

/// A data directory with one blank workspace.
struct Workspace {
    directory: tempfile::TempDir,
    storage: Storage,
    scope: StorageScope,
    /// The catalog's first commit.
    initial: Revision,
}

impl CandidateCheck for Accept {
    fn check(&self, _root: &Path, _rewritten: &[WorkspacePath]) -> Result<Vec<Warning>, ApiError> {
        Ok(Vec::new())
    }
}

impl JobQueue for NoJobs {
    fn enqueue(&self, _scope: JobScope, _job: JobId) -> PortFuture<'_, ()> {
        not_used()
    }
}

impl Converter for NoJobs {
    fn page_count<'a>(
        &'a self,
        _source: &'a LocalSource,
        _file_name: &'a str,
    ) -> PortFuture<'a, PageCount> {
        not_used()
    }

    fn convert(&self, _input: ConversionInput) -> PortFuture<'_, Conversion> {
        not_used()
    }

    fn limits(&self) -> ConverterLimits {
        ConverterLimits {
            window_pages: 1,
            memory_limit_bytes: 1,
        }
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

    /// The production service over this workspace's durable adapters and conformance policy.
    fn service(&self) -> Fallible<ApplicationService> {
        let unused = Arc::new(NoJobs);
        Ok(ApplicationService::new(
            Ports {
                access: Arc::new(self.storage.access()),
                mutations: Arc::new(self.storage.mutations()),
                catalog: Arc::new(self.storage.catalog()),
                blobs: Arc::new(self.storage.blobs()),
                versions: Arc::new(self.storage.versions()),
                records: Arc::new(self.storage.records()),
                queue: unused.clone(),
                search: Arc::new(self.storage.search()),
                converter: unused,
                proposals: Arc::new(self.storage.proposals()),
                uploads: Arc::new(self.storage.uploads()),
                events: Arc::new(self.storage.events()),
                credentials: Arc::new(self.storage.credentials()),
                drafts: Arc::new(self.storage.drafts()),
                confirmations: Arc::new(self.storage.confirmations()),
                sandbox: Arc::new(self.storage.sandbox()),
                readiness: Arc::new(self.storage.readiness()),
            },
            ApplicationConfig::new(
                "https://sandbox.example.test".to_owned(),
                Duration::from_secs(300),
            ),
        )?)
    }

    /// A handler context with already-resolved read/write grants; authorization is not tested here.
    fn context(&self, operation: OperationName) -> OperationContext {
        OperationContext {
            principal: Principal {
                tenant_id: self.scope.tenant_id.clone(),
                subject: "ana".to_owned(),
                route: AccessRoute::LocalOwner,
                client_id: None,
                delegation: None,
            },
            session_id: Some("session".to_owned()),
            operation,
            tenant: None,
            grants: vec![WorkspaceGrant {
                scope: self.scope.clone(),
                permissions: vec![Permission::Read, Permission::Write],
            }],
            mutation: None,
            attempt: Attempt::First,
        }
    }

    /// Commit `edits` as mutation `mutation` on top of `expected`, accepting every candidate.
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

    /// The `aliases` property of item `item` at `revision`, as `show` answers it.
    async fn aliases(&self, revision: &Revision, item: u32) -> Fallible<Option<Value>> {
        Ok(self
            .storage
            .versions()
            .show(&self.scope, revision, uuid!(item)?)
            .await?
            .properties
            .get("aliases")
            .cloned())
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

    /// Commit `bytes` at `at` on main straight through `git2`, as no store edit can.
    fn raw(&self, mutation: u32, at: &str, bytes: &[u8]) -> Fallible<Revision> {
        let repository = self.repository()?;
        let parent = repository.find_commit(repository.refname_to_id("refs/heads/main")?)?;
        let mut update = git2::build::TreeUpdateBuilder::new();
        update.upsert(at, repository.blob(bytes)?, git2::FileMode::Blob);
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
}

/// A job/converter call would be outside this naming journey's purpose.
fn not_used<'a, T: Send + 'a>() -> PortFuture<'a, T> {
    Box::pin(async {
        Err(ApiError::new(
            ErrorCode::Internal,
            "naming never starts a job",
        ))
    })
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

/// A note numbered `item` at `at` with `properties` besides its type and title.
fn note(item: u32, at: &str, body: &str, properties: Value) -> Fallible<TreeEdit> {
    Ok(TreeEdit::CreateItem {
        item_id: uuid!(item)?,
        path: path(at)?,
        title: Some(format!("Note {item}")),
        type_name: "Note".to_owned(),
        kind: ItemKind::Note,
        body: body.to_owned(),
        properties: serde_json::from_value(properties)?,
    })
}

fn moved(item: u32, to: &str, keep_alias: bool) -> Fallible<TreeEdit> {
    Ok(TreeEdit::MoveItem {
        item_id: uuid!(item)?,
        destination: path(to)?,
        keep_alias,
    })
}

/// A source card numbered `item` at `at`, of an original named `original`.
fn card(item: u32, at: &str, original: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::WriteSourceCard(Box::new(SourceCard {
        item_id: uuid!(item)?,
        path: path(at)?,
        title: original.to_owned(),
        type_name: "source".to_owned(),
        body: "Converted.".to_owned(),
        properties: BTreeMap::new(),
        appearance: serde_json::from_value(json!({
            "object": "d".repeat(64),
            "names": [{
                "filename": original,
                "folder": "",
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

fn rule() -> NamingRule {
    NamingRule {
        glob: "**".to_owned(),
        type_name: None,
        case: LetterCase::Kebab,
        separator: "-".to_owned(),
        prefix: String::new(),
        suffix: None,
        date_order: DateOrder::Ymd,
        date_format: "[year]-[month]-[day]".to_owned(),
        strip_version_suffix: true,
        extension_policy: ExtensionPolicy::Preserve,
        alias_preservation: true,
        destination: Some("meetings/[year]".to_owned()),
        collision: CollisionPolicy::Suffix,
    }
}

// P0: `VersionStore::rules` answers the file's text and the reader's error.

#[tokio::test]
async fn conventions_rules_none_saved_is_none() -> TestResult {
    let workspace = Workspace::new().await?;
    let read = workspace
        .storage
        .versions()
        .rules(&workspace.scope, &workspace.initial)
        .await?;
    assert!(read.is_none());
    Ok(())
}

#[tokio::test]
async fn conventions_rules_saved_rules_read_back_with_their_text() -> TestResult {
    let workspace = Workspace::new().await?;
    let saved = NamingRules {
        schema_version: 1,
        rules: vec![rule()],
    };
    let head = workspace
        .commit(
            2,
            &workspace.initial,
            vec![TreeEdit::SetRules {
                rules: saved.clone(),
            }],
        )
        .await?;
    let read = some(
        workspace
            .storage
            .versions()
            .rules(&workspace.scope, &head)
            .await?,
        "the rules file",
    )?;
    assert!(read.text.contains("schema_version: 1"), "{}", read.text);
    let parsed = read
        .parsed
        .map_err(|error| format!("saved rules do not read: {error:?}"))?;
    assert_eq!(serde_json::to_value(parsed)?, serde_json::to_value(saved)?);
    Ok(())
}

#[tokio::test]
async fn conventions_rules_a_yaml_error_answers_the_text_and_its_line() -> TestResult {
    let workspace = Workspace::new().await?;
    let text = "schema_version: 1\ncolour: red\nrules: []\n";
    let head = workspace.raw(2, ".okf/rules.yaml", text.as_bytes())?;
    let read = some(
        workspace
            .storage
            .versions()
            .rules(&workspace.scope, &head)
            .await?,
        "the rules file",
    )?;
    assert_eq!(read.text, text);
    let error = err_of(read.parsed)?;
    assert!(error.message.contains("colour"), "{}", error.message);
    assert_eq!(error.line, Some(2));
    assert!(error.column.is_some());
    Ok(())
}

#[tokio::test]
async fn conventions_rules_bytes_that_are_not_utf8_never_read_as_rules() -> TestResult {
    let workspace = Workspace::new().await?;
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend_from_slice(b"\nschema_version: 1\nrules: []\n");
    let head = workspace.raw(2, ".okf/rules.yaml", &bytes)?;
    let read = some(
        workspace
            .storage
            .versions()
            .rules(&workspace.scope, &head)
            .await?,
        "the rules file",
    )?;
    assert!(read.text.starts_with('\u{fffd}'), "{:?}", read.text);
    assert!(read.text.ends_with("rules: []\n"));
    assert!(read.parsed.is_err());
    Ok(())
}

#[tokio::test]
async fn conventions_rules_an_unknown_revision_is_not_found() -> TestResult {
    let workspace = Workspace::new().await?;
    let unknown = Revision::try_from("e".repeat(40))?;
    let error = err_of(
        workspace
            .storage
            .versions()
            .rules(&workspace.scope, &unknown)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound);
    Ok(())
}

// P1: `VersionStore::tree` answers every item and folder in one pass.

#[tokio::test]
async fn conventions_tree_holds_every_item_and_folder_in_path_order() -> TestResult {
    let workspace = Workspace::new().await?;
    let head = workspace
        .commit(
            2,
            &workspace.initial,
            vec![
                note(10, "notes/b.md", "B.", json!({}))?,
                note(11, "a.md", "A.", json!({}))?,
                note(12, ".hidden/c.md", "C.", json!({}))?,
                note(13, "notes/deep/d.md", "D.", json!({}))?,
                card(14, "inbox/Report-pdf.md", "Report.pdf")?,
                TreeEdit::SetStatus {
                    item_id: uuid!(13)?,
                    status: None,
                    archived: Some(true),
                },
            ],
        )
        .await?;
    let tree = workspace
        .storage
        .versions()
        .tree(&workspace.scope, &head)
        .await?;
    let items: Vec<(&str, bool, Option<&str>)> = tree
        .items
        .iter()
        .map(|item| {
            (
                item.summary.path.as_str(),
                item.summary.archived,
                item.original_name.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        items,
        [
            (".hidden/c.md", false, None),
            ("a.md", false, None),
            ("inbox/Report-pdf.md", false, Some("Report.pdf")),
            ("notes/b.md", false, None),
            ("notes/deep/d.md", true, None),
        ]
    );
    let folders: Vec<&str> = tree.folders.iter().map(WorkspacePath::as_str).collect();
    for folder in [
        ".hidden",
        "inbox",
        "notes",
        "notes/deep",
        ".okf",
        ".okf/sources",
    ] {
        assert!(folders.contains(&folder), "{folder} in {folders:?}");
    }
    let mut sorted = folders.clone();
    sorted.sort_unstable();
    assert_eq!(folders, sorted);
    // The same summaries `list` gives of a folder.
    let listed = workspace
        .storage
        .versions()
        .list(
            &workspace.scope,
            &head,
            Some(&path("notes")?),
            okf_jawn_core::storage::Page {
                cursor: None,
                limit: 50,
            },
        )
        .await?;
    let listed_b = some(listed.items.first(), "notes/b.md listed")?;
    let tree_b = some(
        tree.items
            .iter()
            .find(|item| item.summary.path.as_str() == "notes/b.md"),
        "notes/b.md in the tree",
    )?;
    assert_eq!(
        serde_json::to_value(&tree_b.summary)?,
        serde_json::to_value(listed_b)?
    );
    Ok(())
}

#[tokio::test]
async fn conventions_tree_of_an_unknown_revision_is_not_found() -> TestResult {
    let workspace = Workspace::new().await?;
    let unknown = Revision::try_from("e".repeat(40))?;
    let error = err_of(
        workspace
            .storage
            .versions()
            .tree(&workspace.scope, &unknown)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound);
    Ok(())
}

// P2: `MoveItem::keep_alias` records the old path in `aliases` (design 4.11, A1 to A7).

async fn alias_after(start: Value, keep_alias: bool) -> Fallible<(Workspace, Revision)> {
    let workspace = Workspace::new().await?;
    let created = workspace
        .commit(
            2,
            &workspace.initial,
            vec![note(10, "notes/Old Name.md", "# Old\n", start)?],
        )
        .await?;
    let head = workspace
        .commit(
            3,
            &created,
            vec![moved(10, "notes/old-name.md", keep_alias)?],
        )
        .await?;
    Ok((workspace, head))
}

#[tokio::test]
async fn conventions_alias_a1_a_move_records_the_old_path() -> TestResult {
    let (workspace, head) = alias_after(json!({}), true).await?;
    assert_eq!(workspace.path_of(&head, 10).await?, "notes/old-name.md");
    assert_eq!(
        workspace.aliases(&head, 10).await?,
        Some(json!(["notes/Old Name.md"]))
    );
    Ok(())
}

#[tokio::test]
async fn conventions_alias_a2_a3_existing_aliases_are_kept_before_it() -> TestResult {
    for start in [json!({ "aliases": ["a.md"] }), json!({ "aliases": "a.md" })] {
        let (workspace, head) = alias_after(start.clone(), true).await?;
        assert_eq!(
            workspace.aliases(&head, 10).await?,
            Some(json!(["a.md", "notes/Old Name.md"])),
            "{start}"
        );
    }
    // The old path is not added twice.
    let (workspace, head) = alias_after(json!({ "aliases": ["notes/Old Name.md"] }), true).await?;
    assert_eq!(
        workspace.aliases(&head, 10).await?,
        Some(json!(["notes/Old Name.md"]))
    );
    Ok(())
}

#[tokio::test]
async fn conventions_alias_a4_aliases_that_are_not_paths_refuse_the_move() -> TestResult {
    let workspace = Workspace::new().await?;
    let created = workspace
        .commit(
            2,
            &workspace.initial,
            vec![note(
                10,
                "notes/Old Name.md",
                "# Old\n",
                json!({ "aliases": 5 }),
            )?],
        )
        .await?;
    let error = err_of(
        workspace
            .commit(3, &created, vec![moved(10, "notes/old-name.md", true)?])
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::InvalidInput, "{}", error.message);
    assert!(
        error.message.contains("notes/Old Name.md"),
        "{}",
        error.message
    );
    assert_eq!(
        workspace.storage.versions().head(&workspace.scope).await?,
        created
    );
    Ok(())
}

#[tokio::test]
async fn conventions_alias_a5_without_keep_alias_nothing_is_written() -> TestResult {
    let (workspace, head) = alias_after(json!({ "aliases": ["a.md"] }), false).await?;
    assert_eq!(workspace.aliases(&head, 10).await?, Some(json!(["a.md"])));
    let (workspace, head) = alias_after(json!({}), false).await?;
    assert_eq!(workspace.aliases(&head, 10).await?, None);
    Ok(())
}

#[tokio::test]
async fn conventions_alias_a6_a_case_only_rename_records_the_items_own_path() -> TestResult {
    let workspace = Workspace::new().await?;
    let created = workspace
        .commit(
            2,
            &workspace.initial,
            vec![note(10, "Plan.md", "# Plan\n", json!({}))?],
        )
        .await?;
    let head = workspace
        .commit(3, &created, vec![moved(10, "plan.md", true)?])
        .await?;
    assert_eq!(workspace.path_of(&head, 10).await?, "plan.md");
    assert_eq!(
        workspace.aliases(&head, 10).await?,
        Some(json!(["Plan.md"]))
    );
    Ok(())
}

#[tokio::test]
async fn conventions_alias_a7_a_moves_referrer_follows_it() -> TestResult {
    let workspace = Workspace::new().await?;
    let created = workspace
        .commit(
            2,
            &workspace.initial,
            vec![
                note(10, "notes/Plan_A.md", "# A\n", json!({}))?,
                note(11, "notes/B.md", "See [p](Plan_A.md).", json!({}))?,
            ],
        )
        .await?;
    let head = workspace
        .commit(3, &created, vec![moved(10, "notes/plan-a.md", false)?])
        .await?;
    let referrer = workspace
        .storage
        .versions()
        .show(&workspace.scope, &head, uuid!(11)?)
        .await?;
    assert_eq!(referrer.body.trim_end(), "See [p](plan-a.md).");
    assert_eq!(workspace.aliases(&head, 10).await?, None);
    Ok(())
}

#[tokio::test]
async fn conventions_alias_a_move_to_its_own_path_records_nothing() -> TestResult {
    let workspace = Workspace::new().await?;
    let created = workspace
        .commit(
            2,
            &workspace.initial,
            vec![note(10, "plan.md", "# Plan\n", json!({}))?],
        )
        .await?;
    let head = workspace
        .commit(3, &created, vec![moved(10, "plan.md", true)?])
        .await?;
    assert_eq!(workspace.aliases(&head, 10).await?, None);
    Ok(())
}

#[tokio::test]
async fn conventions_alias_only_the_hop_that_keeps_it_records_one() -> TestResult {
    // An apply's chain: the item goes to a temporary path recording its old one, then on to its
    // new path recording nothing.
    let workspace = Workspace::new().await?;
    let created = workspace
        .commit(
            2,
            &workspace.initial,
            vec![note(10, "a/x.md", "# X\n", json!({}))?],
        )
        .await?;
    let head = workspace
        .commit(
            3,
            &created,
            vec![
                moved(
                    10,
                    &format!("a/okf-jawn-renaming-{}.md", uuid_text(10)),
                    true,
                )?,
                moved(10, "b/x.md", false)?,
            ],
        )
        .await?;
    assert_eq!(workspace.path_of(&head, 10).await?, "b/x.md");
    assert_eq!(workspace.aliases(&head, 10).await?, Some(json!(["a/x.md"])));
    let item: ItemId = uuid!(10)?;
    let tree = workspace
        .storage
        .versions()
        .tree(&workspace.scope, &head)
        .await?;
    assert!(
        tree.items
            .iter()
            .all(|held| !held.summary.path.as_str().contains("okf-jawn-renaming-")),
        "{item:?}"
    );
    Ok(())
}

/// Two moving notes and their referrer: the second note vacates the first note's target folder.
async fn folder_dependency() -> Fallible<(Workspace, Revision, NamingRules)> {
    let workspace = Workspace::new().await?;
    let base = workspace
        .commit(
            2,
            &workspace.initial,
            vec![
                note(10, "a/child.md", "# Child\n", json!({}))?,
                note(11, "z.md", "# Parent\n", json!({}))?,
                note(
                    12,
                    "links.md",
                    "See [child](a/child.md) and [parent](z.md).\n",
                    json!({}),
                )?,
            ],
        )
        .await?;
    let mut child = rule();
    "a/*.md".clone_into(&mut child.glob);
    child.destination = Some("z.md".to_owned());
    child.date_format = String::new();
    child.strip_version_suffix = false;
    let mut parent = child.clone();
    "z.md".clone_into(&mut parent.glob);
    parent.destination = Some("elsewhere".to_owned());
    let rules = NamingRules {
        schema_version: 1,
        rules: vec![child, parent],
    };
    Ok((workspace, base, rules))
}

/// SPEC section 6: applying the accepted preview is one change, including link updates.
/// The final names below are literal rule results, not computed from the apply implementation:
/// `z.md` vacates its path, which the earlier-sorted child's destination needs as a folder.
#[tokio::test]
async fn conventions_apply_vacates_an_item_before_another_move_needs_its_path_as_a_folder()
-> TestResult {
    let (workspace, base, rules) = folder_dependency().await?;
    let service = workspace.service()?;
    let plan = service
        .preview_names(
            &workspace.context(OperationName::PreviewNames),
            PreviewNamesRequest {
                workspace_id: workspace.scope.workspace_id,
                at: At::Revision {
                    revision: base.clone(),
                },
                selection: NameSelection::Workspace,
                rules: rules.clone(),
            },
        )
        .await?;
    assert_eq!(
        plan.entries
            .iter()
            .map(|entry| (entry.from.as_str(), entry.to.as_str()))
            .collect::<Vec<_>>(),
        [("a/child.md", "z.md/child.md"), ("z.md", "elsewhere/z.md"),]
    );
    assert_eq!(plan.refused, 0);
    assert_eq!(
        workspace.storage.versions().head(&workspace.scope).await?,
        base
    );
    let mut context = workspace.context(OperationName::ApplyNames);
    context.mutation = Some(uuid!(3)?);
    let applied = service
        .apply_names(
            &context,
            ApplyNamesRequest {
                workspace_id: workspace.scope.workspace_id,
                plan,
                rules,
                idempotency_key: uuid!(3)?,
            },
        )
        .await?;
    let tree = workspace
        .storage
        .versions()
        .tree(&workspace.scope, &applied.revision)
        .await?;
    assert_eq!(
        tree.items
            .iter()
            .map(|item| (item.summary.id, item.summary.path.as_str()))
            .collect::<Vec<_>>(),
        [
            (uuid!(11)?, "elsewhere/z.md"),
            (uuid!(12)?, "links.md"),
            (uuid!(10)?, "z.md/child.md"),
        ]
    );
    assert_eq!(
        workspace.aliases(&applied.revision, 10).await?,
        Some(json!(["a/child.md"]))
    );
    assert_eq!(
        workspace.aliases(&applied.revision, 11).await?,
        Some(json!(["z.md"]))
    );
    let links = workspace
        .storage
        .versions()
        .show(&workspace.scope, &applied.revision, uuid!(12)?)
        .await?;
    assert_eq!(
        links.body.trim_end(),
        "See [child](z.md/child.md) and [parent](elsewhere/z.md)."
    );
    let repository = workspace.repository()?;
    let commit = repository.find_commit(git2::Oid::from_str(applied.revision.as_str())?)?;
    assert_eq!(commit.parent_count(), 1);
    assert_eq!(commit.parent_id(0)?.to_string(), base.as_str());
    assert_eq!(workspace.path_of(&base, 10).await?, "a/child.md");
    assert_eq!(workspace.path_of(&base, 11).await?, "z.md");
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
