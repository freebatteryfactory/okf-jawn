//! The search index, the event logs, the local access control and readiness over a real
//! workspace.
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use okf_jawn_contract::access::{AccessRoute, Permission, Principal};
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::extraction::ExtractionFilter;
use okf_jawn_contract::identity::{ItemId, Revision, TenantId, WorkspacePath};
use okf_jawn_contract::item::{ItemKind, ItemStatus};
use okf_jawn_contract::purge::{PurgeState, PurgeTarget};
use okf_jawn_contract::search::LinkDirection;
use okf_jawn_core::access::AccessControl;
use okf_jawn_core::credentials::{ConnectorIssue, CredentialStore, NewConnector};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::events::{EventLog, EventQuery, EventScope, NewEvent};
use okf_jawn_core::jobs::{JobScope, JobSpec, NewJob, NewPurge, RecordStore};
use okf_jawn_core::readiness::ReadinessProbe;
use okf_jawn_core::search::{GraphQuery, LinkQuery, SearchIndex, SearchQuery};
use okf_jawn_core::storage::{
    CandidateCheck, CommitChanges, NewWorkspace, Page, Provenance, SourceCard, StorageScope,
    TreeEdit, VersionStore, WorkspaceCatalog,
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

fn note(item: u32, at: &str, body: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::CreateItem {
        item_id: uuid!(item)?,
        path: WorkspacePath::try_from(at.to_owned())?,
        title: Some(format!("Title {item}")),
        type_name: "Note".to_owned(),
        kind: ItemKind::Note,
        body: body.to_owned(),
        properties: BTreeMap::new(),
    })
}

fn card(item: u32) -> Fallible<TreeEdit> {
    Ok(TreeEdit::WriteSourceCard(Box::new(SourceCard {
        item_id: uuid!(item)?,
        path: WorkspacePath::try_from("sources/scan-pdf.md".to_owned())?,
        title: "scan.pdf".to_owned(),
        type_name: "Source".to_owned(),
        body: String::new(),
        properties: BTreeMap::new(),
        appearance: serde_json::from_value(json!({
            "object": "d".repeat(64),
            "names": [{
                "filename": "scan.pdf",
                "folder": "",
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

/// A workspace with three notes linking to one another and an unsupported source.
async fn populated(directory: &Path) -> Fallible<(Storage, StorageScope, Revision)> {
    let storage = Storage::open(directory)?;
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
    let scope = StorageScope {
        tenant_id: tenant,
        workspace_id: created.id,
    };
    let committed = storage
        .versions()
        .commit(
            &scope,
            CommitChanges {
                mutation_id: uuid!(2)?,
                expected_head: created.head,
                author: person("ana"),
                message: "Seed".to_owned(),
                edits: vec![
                    note(
                        10,
                        "notes/harbor.md",
                        "The harbor dredging plan.\n\nSee [budget](budget.md).",
                    )?,
                    note(11, "notes/budget.md", "The budget for dredging.")?,
                    note(12, "other/garden.md", "Tomatoes and basil.")?,
                    card(13)?,
                ],
            },
            Arc::new(Accept),
        )
        .await?;
    Ok((storage, scope, committed.revision))
}

fn query(revision: &Revision, text: &str) -> SearchQuery {
    SearchQuery {
        revision: revision.clone(),
        text: text.to_owned(),
        folder: None,
        include_archived: false,
        extraction: None,
        page: Page {
            cursor: None,
            limit: 20,
        },
    }
}

fn owner(subject: &str, route: AccessRoute, client: Option<String>) -> Fallible<Principal> {
    Ok(Principal {
        subject: subject.to_owned(),
        tenant_id: TenantId::try_from("local".to_owned())?,
        route,
        client_id: client,
        delegation: None,
    })
}

#[tokio::test]
async fn search_reads_committed_trees_and_never_drafts() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, revision) = populated(directory.path()).await?;
    let search = storage.search();
    let error = err_of(search.search(&scope, query(&revision, "dredging")).await)?;
    assert_eq!(
        error.code,
        ErrorCode::Unavailable,
        "an unindexed revision is not searched"
    );
    storage
        .drafts()
        .save(
            &scope,
            uuid!(30)?,
            DraftWrite {
                item_id: uuid!(12)?,
                editor: "ana".to_owned(),
                base_revision: revision.clone(),
                body: "zeppelin".to_owned(),
                properties: BTreeMap::new(),
                content_digest: okf_jawn_contract::identity::Digest::try_from("e".repeat(64))?,
            },
        )
        .await?;
    search.index_revision(&scope, revision.clone()).await?;
    search.index_revision(&scope, revision.clone()).await?;
    let found = search.search(&scope, query(&revision, "dredging")).await?;
    assert_eq!(found.hits.len(), 2);
    let none = search.search(&scope, query(&revision, "zeppelin")).await?;
    assert_eq!(none.hits.len(), 0, "a draft is never indexed");
    let mut in_folder = query(&revision, "dredging");
    in_folder.folder = Some(WorkspacePath::try_from("notes".to_owned())?);
    in_folder.page.limit = 1;
    let first = search.search(&scope, in_folder.clone()).await?;
    assert_eq!(first.hits.len(), 1);
    in_folder.page.cursor = first.next_cursor;
    assert_eq!(search.search(&scope, in_folder).await?.hits.len(), 1);
    let hostile = search
        .search(&scope, query(&revision, "\" OR 1=1 --"))
        .await?;
    assert_eq!(hostile.hits.len(), 0, "the text is matched, never executed");
    let mut unprocessed = query(&revision, "");
    unprocessed.extraction = Some(ExtractionFilter::Unprocessed);
    let sources = search.search(&scope, unprocessed).await?;
    assert_eq!(sources.hits.len(), 1);
    let error = err_of(search.search(&scope, query(&revision, "  ")).await)?;
    assert_eq!(error.code, ErrorCode::InvalidInput);
    Ok(())
}

#[tokio::test]
async fn a_deprecated_item_is_always_searchable() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, revision) = populated(directory.path()).await?;
    let deprecated = storage
        .versions()
        .commit(
            &scope,
            CommitChanges {
                mutation_id: uuid!(3)?,
                expected_head: revision,
                author: person("ana"),
                message: "Deprecate the budget".to_owned(),
                edits: vec![TreeEdit::SetStatus {
                    item_id: uuid!(11)?,
                    status: Some(ItemStatus::Deprecated),
                    archived: None,
                }],
            },
            Arc::new(Accept),
        )
        .await?
        .revision;
    let search = storage.search();
    search.index_revision(&scope, deprecated.clone()).await?;
    let found = search.search(&scope, query(&deprecated, "budget")).await?;
    let budget: ItemId = uuid!(11)?;
    assert!(
        found.hits.iter().any(|hit| hit.source.item_id == budget),
        "the deprecated item is found"
    );
    let shown = storage
        .versions()
        .show(&scope, &deprecated, uuid!(11)?)
        .await?;
    assert_eq!(
        shown.summary.status,
        ItemStatus::Deprecated,
        "the hit is the deprecated item"
    );
    Ok(())
}

#[tokio::test]
async fn links_and_the_graph_come_from_the_indexed_revision() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, revision) = populated(directory.path()).await?;
    let search = storage.search();
    search.index_revision(&scope, revision.clone()).await?;
    let link_query = |item: u32, direction: LinkDirection| -> Fallible<LinkQuery> {
        Ok(LinkQuery {
            revision: revision.clone(),
            item_id: uuid!(item)?,
            direction,
            page: Page {
                cursor: None,
                limit: 20,
            },
        })
    };
    let outgoing = search
        .links(&scope, link_query(10, LinkDirection::Outgoing)?)
        .await?;
    let link = some(outgoing.links.first(), "the harbor note's link")?;
    assert_eq!(link.to, Some(uuid!(11)?));
    assert_eq!(link.line, 3);
    let incoming = search
        .links(&scope, link_query(11, LinkDirection::Incoming)?)
        .await?;
    assert_eq!(incoming.links.len(), 1);
    let graph = search
        .graph(
            &scope,
            GraphQuery {
                revision: revision.clone(),
                folder: Some(WorkspacePath::try_from("notes".to_owned())?),
                max_nodes: 10,
            },
        )
        .await?;
    assert_eq!(
        (graph.nodes.len(), graph.edges.len(), graph.truncated),
        (2, 1, false)
    );
    let cut = search
        .graph(
            &scope,
            GraphQuery {
                revision,
                folder: None,
                max_nodes: 1,
            },
        )
        .await?;
    assert!(cut.truncated);
    Ok(())
}

#[tokio::test]
async fn a_graph_node_of_a_source_keeps_its_media_type() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, revision) = populated(directory.path()).await?;
    let search = storage.search();
    search.index_revision(&scope, revision.clone()).await?;
    let graph = search
        .graph(
            &scope,
            GraphQuery {
                revision,
                folder: Some(WorkspacePath::try_from("sources".to_owned())?),
                max_nodes: 10,
            },
        )
        .await?;
    let card: ItemId = uuid!(13)?;
    let node = some(
        graph.nodes.iter().find(|node| node.id == card),
        "the source card's node",
    )?;
    assert_eq!(
        node.media_type.as_deref(),
        Some("application/pdf"),
        "the media type of the committed source appearance, as a folder listing shows"
    );
    Ok(())
}

#[tokio::test]
async fn rebuilding_the_index_never_touches_jobs_reviews_or_receipts() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, revision) = populated(directory.path()).await?;
    let records = storage.records();
    records
        .create_job(
            &JobScope::Workspace(scope.clone()),
            NewJob {
                mutation_id: uuid!(40)?,
                initiator: person("ana"),
                spec: JobSpec::RebuildIndex,
            },
        )
        .await?;
    let records_file = storage.data().records_path();
    let count = |table: &str| -> Fallible<i64> {
        let connection = rusqlite::Connection::open(&records_file)?;
        Ok(
            connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })?,
        )
    };
    let before = (count("jobs")?, count("reviews")?, count("receipts")?);
    let search = storage.search();
    search.index_revision(&scope, revision.clone()).await?;
    search.rebuild(&scope, revision.clone()).await?;
    assert_eq!(
        (count("jobs")?, count("reviews")?, count("receipts")?),
        before
    );
    let found = search.search(&scope, query(&revision, "tomatoes")).await?;
    assert_eq!(found.hits.len(), 1, "the rebuilt index answers");
    Ok(())
}

#[tokio::test]
async fn an_event_for_a_workspace_the_tenant_does_not_hold_goes_to_the_tenant_log() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, revision) = populated(directory.path()).await?;
    let events = storage.events();
    let changed = NewEvent {
        kind: EventKind::Changed,
        revision: Some(revision),
        item_id: None,
        job_id: None,
        connector_id: None,
        actor: None,
        operation: None,
    };
    let held = events
        .append(
            &EventScope::Workspace(scope.clone()),
            Some(uuid!(50)?),
            changed.clone(),
        )
        .await?;
    assert_eq!(held.workspace_id, Some(scope.workspace_id));
    let again = events
        .append(
            &EventScope::Workspace(scope.clone()),
            Some(uuid!(50)?),
            changed.clone(),
        )
        .await?;
    assert_eq!(
        again.id, held.id,
        "an identical event under one mutation is appended once"
    );
    let never = StorageScope {
        tenant_id: scope.tenant_id.clone(),
        workspace_id: uuid!(77)?,
    };
    let refused = principal_event()?;
    let recorded = events
        .append(&EventScope::Workspace(never.clone()), None, refused.clone())
        .await?;
    assert_eq!(
        recorded.workspace_id, None,
        "a never-created workspace gets no log"
    );
    let records = storage.records();
    let purge = records
        .create_purge(
            &scope.tenant_id,
            uuid!(60)?,
            NewPurge {
                id: uuid!(61)?,
                target: PurgeTarget::Workspace {
                    workspace_id: scope.workspace_id,
                },
                initiator: person("ana"),
            },
        )
        .await?;
    let mut completed = purge;
    completed.state = PurgeState::Completed;
    records.update_purge(&scope.tenant_id, completed).await?;
    let after = events
        .append(&EventScope::Workspace(scope.clone()), None, refused)
        .await?;
    assert_eq!(
        after.workspace_id, None,
        "a purged workspace's log is never revived"
    );
    let page = |after: Option<String>| EventQuery {
        after,
        page: Page {
            cursor: None,
            limit: 50,
        },
    };
    let tenant_log = events
        .list(&EventScope::Tenant(scope.tenant_id.clone()), page(None))
        .await?;
    assert_eq!(tenant_log.events.len(), 2);
    let workspace_log = events
        .list(&EventScope::Workspace(scope.clone()), page(None))
        .await?;
    assert_eq!(workspace_log.events.len(), 1);
    let unknown_log = events
        .list(&EventScope::Workspace(never), page(None))
        .await?;
    assert_eq!(
        unknown_log.events.len(),
        0,
        "no rows were created for that workspace"
    );
    let rest = events
        .list(
            &EventScope::Tenant(scope.tenant_id),
            page(tenant_log.events.first().map(|event| event.id.clone())),
        )
        .await?;
    assert_eq!(rest.events.len(), 1);
    Ok(())
}

fn principal_event() -> Fallible<NewEvent> {
    Ok(NewEvent::permission_denied(
        &owner("mallory", AccessRoute::BrowserSession, None)?,
        okf_jawn_contract::metadata::OperationName::CreateItem,
    ))
}

#[tokio::test]
async fn local_access_grants_the_owner_connectors_and_creators() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, _) = populated(directory.path()).await?;
    let access = storage.access();
    let identity = storage.credentials().installation_identity().await?;
    let local_owner = owner(&identity.subject, AccessRoute::LocalOwner, None)?;
    let grant = access
        .authorize(&local_owner, scope.workspace_id, Permission::Admin)
        .await?;
    assert_eq!(grant.permissions.len(), 6);
    let tenant = access
        .authorize_tenant(&local_owner, Permission::Admin)
        .await?;
    assert!(tenant.allows(Permission::Admin));
    let ConnectorIssue::Issued(issued) = storage
        .credentials()
        .create_connector(
            uuid!(70)?,
            NewConnector {
                label: "agent".to_owned(),
                workspace_ids: vec![scope.workspace_id],
                allow_propose: false,
            },
        )
        .await?
    else {
        return Err("expected a new connector".into());
    };
    let agent = owner(
        &identity.subject,
        AccessRoute::McpDelegation,
        Some(issued.connector.connector_id.0.to_string()),
    )?;
    let delegated = access
        .authorize(&agent, scope.workspace_id, Permission::Read)
        .await?;
    assert_eq!(delegated.permissions, vec![Permission::Read]);
    assert_eq!(
        access
            .authorize_tenant(&agent, Permission::Admin)
            .await?
            .permissions
            .len(),
        0
    );
    let stranger = owner("someone", AccessRoute::BrowserSession, None)?;
    let nothing = access
        .authorize(&stranger, scope.workspace_id, Permission::Read)
        .await?;
    assert_eq!(nothing.permissions.len(), 0);
    access.grant_creator(&stranger, scope.workspace_id).await?;
    assert_eq!(access.grants(&stranger).await?.len(), 1);
    let missing: ItemId = uuid!(99)?;
    let unknown = access
        .authorize(
            &local_owner,
            okf_jawn_contract::identity::WorkspaceId(missing.0),
            Permission::Read,
        )
        .await?;
    assert!(
        unknown.permissions.is_empty(),
        "no grant on a workspace the tenant does not hold"
    );
    Ok(())
}

#[tokio::test]
async fn a_connector_is_granted_only_on_the_workspaces_it_lists() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, _) = populated(directory.path()).await?;
    let other = storage
        .catalog()
        .create(
            &scope.tenant_id,
            uuid!(80)?,
            NewWorkspace {
                name: "Private".to_owned(),
                description: String::new(),
                creator: person("ana"),
            },
        )
        .await?;
    let ConnectorIssue::Issued(issued) = storage
        .credentials()
        .create_connector(
            uuid!(70)?,
            NewConnector {
                label: "agent".to_owned(),
                workspace_ids: vec![scope.workspace_id],
                allow_propose: true,
            },
        )
        .await?
    else {
        return Err("expected a new connector".into());
    };
    let identity = storage.credentials().installation_identity().await?;
    let agent = owner(
        &identity.subject,
        AccessRoute::McpDelegation,
        Some(issued.connector.connector_id.0.to_string()),
    )?;
    let access = storage.access();
    let listed = access
        .authorize(&agent, scope.workspace_id, Permission::Read)
        .await?;
    assert_eq!(
        listed.permissions,
        vec![Permission::Read, Permission::Propose]
    );
    let unlisted = access.authorize(&agent, other.id, Permission::Read).await?;
    assert!(
        unlisted.permissions.is_empty(),
        "the owner's connector holds nothing on a workspace it does not list"
    );
    let grants = access.grants(&agent).await?;
    assert_eq!(
        grants
            .iter()
            .map(|grant| grant.scope.workspace_id)
            .collect::<Vec<_>>(),
        vec![scope.workspace_id]
    );
    Ok(())
}

#[tokio::test]
async fn local_editors_are_the_owner_and_every_grant_with_write_sorted_once() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (storage, scope, _) = populated(directory.path()).await?;
    let access = storage.access();
    let identity = storage.credentials().installation_identity().await?;
    assert_eq!(
        access.editors(&scope).await?,
        vec![identity.subject.clone()]
    );
    // `zed` and `amy` create (all permissions); the owner's own grant repeats the owner.
    for subject in ["zed", "amy"] {
        access
            .grant_creator(
                &owner(subject, AccessRoute::BrowserSession, None)?,
                scope.workspace_id,
            )
            .await?;
    }
    access
        .grant_creator(
            &owner(&identity.subject, AccessRoute::LocalOwner, None)?,
            scope.workspace_id,
        )
        .await?;
    // A grant without `write`, recorded directly: the adapter has no call that makes one.
    rusqlite::Connection::open(storage.data().records_path())?.execute(
        "INSERT INTO grants (tenant_id, workspace_id, subject, permissions)
         VALUES (?1, ?2, 'reader', '[\"read\",\"propose\"]')",
        rusqlite::params![scope.tenant_id.as_str(), scope.workspace_id.0.to_string()],
    )?;
    storage
        .credentials()
        .create_connector(
            uuid!(70)?,
            NewConnector {
                label: "agent".to_owned(),
                workspace_ids: vec![scope.workspace_id],
                allow_propose: true,
            },
        )
        .await?;
    let mut expected = vec!["amy".to_owned(), identity.subject.clone(), "zed".to_owned()];
    expected.sort();
    assert_eq!(access.editors(&scope).await?, expected);
    let unheld = StorageScope {
        tenant_id: scope.tenant_id.clone(),
        workspace_id: uuid!(77)?,
    };
    assert!(
        access.editors(&unheld).await?.is_empty(),
        "a workspace the tenant does not hold has no editor"
    );
    Ok(())
}

#[tokio::test]
async fn readiness_reports_each_store() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let report = storage.readiness().probe().await?;
    assert!(report.ready);
    assert_eq!(report.dependencies.len(), 4);
    Ok(())
}

/// Refuse new files in `directory` through its access-control list, leaving its permission
/// bits (the read-only attribute) showing it writable.
#[cfg(windows)]
fn refuse_writes(directory: &Path) -> TestResult {
    let status = std::process::Command::new("icacls")
        .arg(directory)
        .args(["/deny", "*S-1-1-0:(W)"])
        .output()?
        .status;
    assert!(status.success(), "icacls could not deny writes");
    assert!(
        !std::fs::metadata(directory)?.permissions().readonly(),
        "the permission bits still show the directory writable"
    );
    Ok(())
}

#[cfg(windows)]
fn allow_writes(directory: &Path) -> TestResult {
    let status = std::process::Command::new("icacls")
        .arg(directory)
        .args(["/remove:d", "*S-1-1-0"])
        .output()?
        .status;
    assert!(status.success(), "icacls could not lift the denial");
    Ok(())
}

/// Refuse new files in `directory` through its mode (the test runs as an unprivileged user).
#[cfg(unix)]
fn refuse_writes(directory: &Path) -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o555))?;
    Ok(())
}

#[cfg(unix)]
fn allow_writes(directory: &Path) -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

/// Probe files left in `directory`.
fn probe_files(directory: &Path) -> Fallible<usize> {
    let mut found = 0_usize;
    for entry in std::fs::read_dir(directory)? {
        if entry?
            .file_name()
            .to_string_lossy()
            .starts_with(".okf-jawn-ready")
        {
            found = found.saturating_add(1);
        }
    }
    Ok(found)
}

#[tokio::test]
async fn readiness_reports_a_database_at_another_schema_version_as_not_ready() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    assert!(storage.readiness().probe().await?.ready);
    rusqlite::Connection::open(storage.data().index_path())?.pragma_update(
        None,
        "user_version",
        99,
    )?;
    let report = storage.readiness().probe().await?;
    let index = some(
        report
            .dependencies
            .iter()
            .find(|dependency| dependency.name == "index"),
        "the index status",
    )?;
    assert!(!index.ready, "{}", index.message);
    assert!(
        index.message.contains("schema version"),
        "{}",
        index.message
    );
    assert!(!report.ready);
    Ok(())
}

#[tokio::test]
async fn readiness_reports_a_directory_it_cannot_write_as_not_ready() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let objects = storage.data().objects_path();
    let repositories = storage.data().repositories_path();
    refuse_writes(&objects)?;
    let refused = storage.readiness().probe().await;
    allow_writes(&objects)?;
    let refused = refused?;
    assert!(!refused.ready, "an unwritable blob directory is not ready");
    let blob = some(
        refused
            .dependencies
            .iter()
            .find(|dependency| dependency.name == "objects"),
        "the objects status",
    )?;
    assert!(!blob.ready, "{}", blob.message);
    let git = some(
        refused
            .dependencies
            .iter()
            .find(|dependency| dependency.name == "repositories"),
        "the repositories status",
    )?;
    assert!(git.ready, "{}", git.message);
    assert_eq!(
        (probe_files(&objects)?, probe_files(&repositories)?),
        (0, 0),
        "no probe file is left behind"
    );
    assert!(
        storage.readiness().probe().await?.ready,
        "writable again, so ready again"
    );
    assert_eq!(
        (probe_files(&objects)?, probe_files(&repositories)?),
        (0, 0)
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
