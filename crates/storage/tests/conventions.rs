//! The version store's naming-rules ports against real Git repositories: the rules file read
//! with its text and the reader's error (`VersionStore::rules`). Named `conventions_*`.
#![cfg(feature = "runtime")]

use std::path::Path;
use std::sync::Arc;

use git2::Repository;
use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::conventions::{
    CollisionPolicy, DateOrder, ExtensionPolicy, LetterCase, NamingRule, NamingRules,
};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Revision, TenantId, WorkspacePath};
use okf_jawn_core::storage::{
    CandidateCheck, CommitChanges, NewWorkspace, Provenance, StorageScope, TreeEdit, VersionStore,
    WorkspaceCatalog,
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

#[path = "../../../tests/support/check.rs"]
mod check;
