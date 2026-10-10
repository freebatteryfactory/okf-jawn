//! Workspace and installation archives, restore and the pre-migration backup set, against real
//! Git, SQLite, the object store and `zip` (Stage 1b design section 8).
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::{Digest, ItemId, Revision, TenantId, WorkspacePath};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_contract::purge::{PurgeState, PurgeTarget};
use okf_jawn_contract::workspace::ArtifactKind;
use okf_jawn_core::credentials::{CredentialStore, NewConnector};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::events::{EventLog, EventScope, NewEvent};
use okf_jawn_core::jobs::{
    DerivedKind, DerivedObject, JobCompletion, JobScope, JobSpec, NewArtifact, NewJob, NewPurge,
    RecordStore,
};
use okf_jawn_core::storage::{
    Backups, BlobStore, CandidateChanges, CandidateCheck, CommitChanges, NewWorkspace, ObjectInfo,
    Provenance, Purger, SourceCard, StorageScope, TreeEdit, VersionStore, WorkspaceCatalog,
};
use okf_jawn_core::uploads::{NewUpload, UploadStore};
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

/// A workspace with a note, a source card over an original, a candidate, two drafts and a
/// retained export; and a second workspace whose content carries the marker `garden`.
struct Fixture {
    storage: Storage,
    workspace: StorageScope,
    other: StorageScope,
    head: Revision,
    candidate: Revision,
    original: Digest,
    export: Digest,
}

impl CandidateCheck for Accept {
    fn check(&self, _root: &Path) -> Result<Vec<Warning>, ApiError> {
        Ok(Vec::new())
    }
}

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

fn tenant(name: &str) -> Fallible<TenantId> {
    Ok(TenantId::try_from(name.to_owned())?)
}

fn note(item: ItemId, at: &str, body: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::CreateItem {
        item_id: item,
        path: WorkspacePath::try_from(at.to_owned())?,
        title: None,
        type_name: "Note".to_owned(),
        kind: ItemKind::Note,
        body: body.to_owned(),
        properties: BTreeMap::new(),
    })
}

fn card(item: ItemId, object: &Digest) -> Fallible<TreeEdit> {
    Ok(TreeEdit::WriteSourceCard(Box::new(SourceCard {
        item_id: item,
        path: WorkspacePath::try_from("sources/scan.md".to_owned())?,
        title: "scan.pdf".to_owned(),
        type_name: "Source".to_owned(),
        body: String::new(),
        properties: BTreeMap::new(),
        appearance: serde_json::from_value(json!({
            "object": object.as_str(),
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

async fn put(storage: &Storage, scope: &StorageScope, bytes: &[u8]) -> Fallible<ObjectInfo> {
    Ok(storage
        .blobs()
        .put(
            scope,
            Box::pin(std::io::Cursor::new(bytes.to_vec())),
            1 << 20,
            None,
        )
        .await?)
}

async fn create(
    storage: &Storage,
    tenant_name: &str,
    mutation: u32,
    name: &str,
) -> Fallible<(StorageScope, Revision)> {
    let created = storage
        .catalog()
        .create(
            &tenant(tenant_name)?,
            uuid!(mutation)?,
            NewWorkspace {
                name: name.to_owned(),
                description: "kept".to_owned(),
                creator: person("ana"),
            },
        )
        .await?;
    Ok((
        StorageScope {
            tenant_id: tenant(tenant_name)?,
            workspace_id: created.id,
        },
        created.head,
    ))
}

async fn commit(
    storage: &Storage,
    scope: &StorageScope,
    mutation: u32,
    head: &Revision,
    edits: Vec<TreeEdit>,
) -> Fallible<Revision> {
    Ok(storage
        .versions()
        .commit(
            scope,
            CommitChanges {
                mutation_id: uuid!(mutation)?,
                expected_head: head.clone(),
                author: person("ana"),
                message: "Seed".to_owned(),
                edits,
            },
            Arc::new(Accept),
        )
        .await?
        .revision)
}

async fn draft(
    storage: &Storage,
    scope: &StorageScope,
    mutation: u32,
    editor: &str,
    base: &Revision,
) -> Fallible<()> {
    storage
        .drafts()
        .save(
            scope,
            uuid!(mutation)?,
            DraftWrite {
                item_id: uuid!(10)?,
                editor: editor.to_owned(),
                base_revision: base.clone(),
                body: format!("Draft by {editor}"),
                properties: BTreeMap::new(),
                content_digest: Digest::try_from("e".repeat(64))?,
            },
        )
        .await?;
    Ok(())
}

async fn export(storage: &Storage, scope: &StorageScope) -> Fallible<Digest> {
    let object = put(storage, scope, b"an export archive").await?;
    let job_scope = JobScope::Workspace(scope.clone());
    let job = storage
        .records()
        .create_job(
            &job_scope,
            NewJob {
                mutation_id: uuid!(30)?,
                initiator: person("ana"),
                spec: JobSpec::BackupWorkspace,
            },
        )
        .await?;
    storage
        .records()
        .record_artifact(
            &job_scope,
            uuid!(31)?,
            NewArtifact {
                kind: ArtifactKind::Export,
                object: object.clone(),
                media_type: "application/zip".to_owned(),
                created_by_job: job.id,
            },
        )
        .await?;
    Ok(object.digest)
}

async fn fixture(directory: &Path) -> Fallible<Fixture> {
    let storage = Storage::open(directory)?;
    let (workspace, created) = create(&storage, "local", 1, "Research").await?;
    let (other, other_head) = create(&storage, "local", 2, "garden plans").await?;
    let original = put(&storage, &workspace, b"original scan bytes").await?;
    let edits = vec![
        note(uuid!(10)?, "notes/plan.md", "The harbor plan.")?,
        card(uuid!(11)?, &original.digest)?,
    ];
    let head = commit(&storage, &workspace, 3, &created, edits).await?;
    let candidate = storage
        .versions()
        .create_candidate(
            &workspace,
            uuid!(20)?,
            CandidateChanges {
                mutation_id: uuid!(4)?,
                base: head.clone(),
                author: person("bea"),
                message: "Propose".to_owned(),
                edits: vec![note(uuid!(12)?, "notes/idea.md", "An idea.")?],
            },
            Arc::new(Accept),
        )
        .await?;
    commit(
        &storage,
        &other,
        5,
        &other_head,
        vec![note(uuid!(13)?, "notes/garden.md", "garden beds")?],
    )
    .await?;
    draft(&storage, &workspace, 6, "ana", &head).await?;
    draft(&storage, &workspace, 7, "bea", &head).await?;
    let export = export(&storage, &workspace).await?;
    // The export uploaded back into the workspace: its slot names the export's object, which an
    // archive still never holds.
    let uploads = storage.uploads();
    let slot = uploads
        .create(
            &workspace,
            uuid!(32)?,
            NewUpload {
                filename: "export.zip".to_owned(),
                relative_path: String::new(),
                expected_size: 17,
                expected_sha256: None,
                supplied_by: person("ana"),
            },
        )
        .await?;
    uploads
        .put_content(
            &workspace,
            slot.id,
            Box::pin(std::io::Cursor::new(b"an export archive".to_vec())),
            1 << 20,
        )
        .await?;
    uploads
        .complete(&workspace, slot.id, export.clone())
        .await?;
    Ok(Fixture {
        storage,
        workspace,
        other,
        head,
        candidate,
        original: original.digest,
        export,
    })
}

/// The names of an archive's entries and the bytes of one of them.
fn entries(path: &Path) -> Fallible<Vec<String>> {
    let archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let mut names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    names.sort();
    Ok(names)
}

fn entry(path: &Path, name: &str, target: &Path) -> Fallible<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let mut bytes = Vec::new();
    archive.by_name(name)?.read_to_end(&mut bytes)?;
    std::fs::write(target, &bytes)?;
    Ok(bytes)
}

fn count(database: &Path, query: &str) -> Fallible<i64> {
    let connection = rusqlite::Connection::open(database)?;
    Ok(connection.query_row(query, [], |row| row.get(0))?)
}

fn contains(bytes: &[u8], marker: &[u8]) -> bool {
    bytes.windows(marker.len()).any(|window| window == marker)
}

/// A connector a replacement restore brought back is revoked: listed only with the revoked
/// ones, never live (review B7, M10).
async fn assert_connector_revoked(restored: &Storage) -> TestResult {
    let connectors = restored.credentials().list_connectors(true).await?;
    assert!(
        some(connectors.first(), "the connector")?
            .revoked_at
            .is_some(),
        "a connector comes back revoked: no secret is kept"
    );
    assert!(
        restored
            .credentials()
            .list_connectors(false)
            .await?
            .is_empty(),
        "a restored connector is not listed or authenticated as live"
    );
    Ok(())
}

/// A replacement restore into a directory that holds only the repository directory of the
/// archived workspace, with no record of it, is refused (review B7, M09).
async fn refuse_a_leftover_repository(
    installation: &Path,
    archives: &[std::path::PathBuf],
    workspace: &StorageScope,
) -> TestResult {
    let other = tempfile::tempdir()?;
    let leftover = Storage::open(other.path())?;
    std::fs::create_dir_all(
        leftover
            .data()
            .repositories_path()
            .join("local")
            .join(format!("{}.git", workspace.workspace_id.0)),
    )?;
    let error = err_of(
        leftover
            .backups()
            .restore_replacement(installation, archives)
            .await,
    )?;
    assert!(
        error.code == ErrorCode::Conflict && error.message.contains("already exists"),
        "a repository directory left in place is not replaced: {error}"
    );
    Ok(())
}

#[tokio::test]
async fn backup_a_workspace_archive_holds_its_history_drafts_and_objects_only() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let backups = fixture.storage.backups();
    let object = backups
        .write_workspace_archive(&fixture.workspace, uuid!(40)?)
        .await?;
    let repeated = backups
        .write_workspace_archive(&fixture.workspace, uuid!(40)?)
        .await?;
    assert_eq!(repeated, object, "a repeated mutation writes nothing new");
    let path = fixture
        .storage
        .data()
        .objects_path()
        .join("local")
        .join(object.digest.as_str().get(..2).unwrap_or_default())
        .join(object.digest.as_str());
    let names = entries(&path)?;
    assert!(names.contains(&format!("objects/{}", fixture.original.as_str())));
    assert!(
        !names.contains(&format!("objects/{}", fixture.export.as_str())),
        "an archive never holds a retained export"
    );
    for name in ["history.pack", "manifest.json", "records.sqlite"] {
        assert!(
            names.iter().any(|entry| entry == name),
            "{name} is in the archive"
        );
    }
    let records = directory.path().join("archived.sqlite");
    let bytes = entry(&path, "records.sqlite", &records)?;
    assert!(
        !contains(&bytes, b"garden"),
        "no byte of another workspace's records"
    );
    assert_eq!(
        count(&records, "SELECT count(*) FROM drafts")?,
        2,
        "drafts are included"
    );
    assert_eq!(count(&records, "SELECT count(*) FROM workspaces")?, 1);
    assert_eq!(count(&records, "SELECT count(*) FROM artifacts")?, 0);
    assert_eq!(count(&records, "SELECT count(*) FROM connectors")?, 0);
    let manifest = entry(
        &path,
        "manifest.json",
        &directory.path().join("manifest.json"),
    )?;
    let manifest: serde_json::Value = serde_json::from_slice(&manifest)?;
    assert_eq!(
        manifest
            .get("references")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(2),
        "the accepted head and the proposal candidate"
    );
    Ok(())
}

#[tokio::test]
async fn backup_an_installation_archive_holds_no_workspace_content() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    storage
        .credentials()
        .create_connector(
            uuid!(50)?,
            NewConnector {
                label: "Desk".to_owned(),
                workspace_ids: vec![fixture.workspace.workspace_id],
                allow_propose: false,
            },
        )
        .await?;
    let workspace_event = NewEvent {
        kind: EventKind::Changed,
        revision: Some(fixture.head.clone()),
        item_id: Some(uuid!(10)?),
        job_id: None,
        connector_id: None,
        actor: None,
        operation: None,
    };
    let tenant_event = NewEvent {
        kind: EventKind::SignedIn,
        revision: None,
        item_id: None,
        ..workspace_event.clone()
    };
    let events = storage.events();
    events
        .append(
            &EventScope::Workspace(fixture.workspace.clone()),
            None,
            workspace_event,
        )
        .await?;
    events
        .append(&EventScope::Tenant(tenant("local")?), None, tenant_event)
        .await?;
    let tenant = tenant("local")?;
    let backups = storage.backups();
    let object = backups
        .write_installation_archive(&tenant, uuid!(51)?)
        .await?;
    let mut read = backups
        .open_installation_archive(&tenant, &object.digest, 0, object.size)
        .await?;
    let mut bytes = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut read.body, &mut bytes).await?;
    for marker in [&b"harbor"[..], b"garden", b"Research", b"secret_hash"] {
        assert!(
            !contains(&bytes, marker),
            "the installation archive holds no workspace content"
        );
    }
    assert!(contains(&bytes, b"Desk"), "the connector is in it");
    let copy = directory.path().join("installation.zip");
    std::fs::write(&copy, &bytes)?;
    assert_eq!(entries(&copy)?, vec!["installation.json".to_owned()]);
    let manifest: serde_json::Value = serde_json::from_slice(&entry(
        &copy,
        "installation.json",
        &directory.path().join("i.json"),
    )?)?;
    assert_eq!(
        manifest
            .get("events")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(1),
        "the tenant event only, never a workspace's"
    );
    let error = err_of(
        backups
            .open_installation_archive(&tenant, &fixture.original, 0, 1)
            .await
            .map(|read| read.object),
    )?;
    assert_eq!(
        error.code,
        ErrorCode::NotFound,
        "only an installation archive is read back"
    );
    let workspace_archive = backups
        .write_workspace_archive(&fixture.workspace, uuid!(52)?)
        .await?;
    let error = err_of(
        backups
            .open_installation_archive(&tenant, &workspace_archive.digest, 0, 1)
            .await
            .map(|read| read.object),
    )?;
    assert_eq!(
        error.code,
        ErrorCode::NotFound,
        "a workspace archive is not read back as an installation archive"
    );
    Ok(())
}

#[tokio::test]
async fn backup_replacement_restore_after_deleting_the_data_directory() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    fixture
        .storage
        .credentials()
        .create_connector(
            uuid!(50)?,
            NewConnector {
                label: "Desk".to_owned(),
                workspace_ids: vec![fixture.workspace.workspace_id],
                allow_propose: false,
            },
        )
        .await?;
    let written = fixture
        .storage
        .backups()
        .write_backup_set(output.path(), None)
        .await?;
    assert_eq!(
        written.len(),
        3,
        "two workspace archives and one installation archive"
    );
    let Fixture {
        storage,
        workspace,
        head,
        candidate,
        original,
        ..
    } = fixture;
    drop(storage);
    std::fs::remove_dir_all(directory.path())?;

    let restored = Storage::open(directory.path())?;
    let installation = output.path().join("installation").join("local.zip");
    let archives = vec![
        output
            .path()
            .join("workspaces")
            .join("local")
            .join(format!("{}.zip", workspace.workspace_id.0)),
    ];
    let reports = restored
        .backups()
        .restore_replacement(&installation, &archives)
        .await?;
    let report = some(reports.first(), "the restore report")?;
    assert_eq!(
        (
            report.items,
            report.drafts_restored,
            report.drafts_unassigned
        ),
        (2, 2, 0)
    );
    let shown = restored.catalog().open(&workspace).await?;
    assert_eq!((shown.name.as_str(), &shown.head), ("Research", &head));
    let item = restored
        .versions()
        .show(&workspace, &head, uuid!(10)?)
        .await?;
    assert_eq!(item.body, "The harbor plan.");
    let draft = some(
        restored.drafts().get(&workspace, uuid!(10)?, "bea").await?,
        "bea's draft",
    )?;
    assert_eq!(draft.body, "Draft by bea");
    restored.blobs().open(&workspace, &original, 0, 1).await?;
    restored
        .versions()
        .show(&workspace, &candidate, uuid!(12)?)
        .await?;
    assert_connector_revoked(&restored).await?;
    let error = err_of(
        restored
            .backups()
            .restore_replacement(&installation, &archives)
            .await,
    )?;
    assert_eq!(
        error.code,
        ErrorCode::Conflict,
        "a workspace that exists is not restored again"
    );
    assert!(
        error.message.contains("already exists"),
        "refused before anything is written: {}",
        error.message
    );
    refuse_a_leftover_repository(&installation, &archives, &workspace).await?;
    Ok(())
}

/// Review B7 (M11, M12): an archive copies every object its records name (a dataset, a job
/// output), and never a written history, even where a job output names it.
#[tokio::test]
async fn backup_a_workspace_archive_holds_every_object_its_records_name() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let (storage, workspace) = (&fixture.storage, &fixture.workspace);
    let dataset = put(storage, workspace, b"a recorded dataset").await?.digest;
    storage
        .records()
        .record_derived_object(
            workspace,
            DerivedObject {
                item_id: uuid!(11)?,
                revision: fixture.head.clone(),
                digest: dataset.clone(),
                kind: DerivedKind::Dataset,
                media_type: "text/csv".to_owned(),
            },
        )
        .await?;
    let output = put(storage, workspace, b"a job output").await?.digest;
    let history = storage
        .versions()
        .write_history(workspace, &fixture.head, uuid!(33)?)
        .await?
        .digest;
    let records = storage.records();
    let job_scope = JobScope::Workspace(workspace.clone());
    let job = records
        .create_job(
            &job_scope,
            NewJob {
                mutation_id: uuid!(34)?,
                initiator: person("ana"),
                spec: JobSpec::BackupWorkspace,
            },
        )
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "the claimed job")?;
    records
        .complete_job(JobCompletion {
            lease: claimed.lease,
            revision: None,
            item_ids: Vec::new(),
            artifact: None,
            restore: None,
            outputs: vec![output.clone(), history.clone()],
            warnings: Vec::new(),
        })
        .await?;
    let archive = storage
        .backups()
        .write_workspace_archive(workspace, uuid!(40)?)
        .await?;
    let names = entries(
        &storage
            .data()
            .objects_path()
            .join("local")
            .join(archive.digest.as_str().get(..2).unwrap_or_default())
            .join(archive.digest.as_str()),
    )?;
    for digest in [&dataset, &output] {
        assert!(
            names.contains(&format!("objects/{}", digest.as_str())),
            "{digest:?} is copied"
        );
    }
    assert!(
        !names.contains(&format!("objects/{}", history.as_str())),
        "a written history never is"
    );
    Ok(())
}

#[tokio::test]
async fn backup_replacement_refuses_an_archive_of_a_purged_workspace() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    storage
        .backups()
        .write_backup_set(output.path(), None)
        .await?;
    let earlier = output
        .path()
        .join("workspaces")
        .join("local")
        .join(format!("{}.zip", fixture.other.workspace_id.0));
    let kept = directory.path().join("earlier.zip");
    std::fs::copy(&earlier, &kept)?;
    let tenant = tenant("local")?;
    let purge = storage
        .records()
        .create_purge(
            &tenant,
            uuid!(60)?,
            NewPurge {
                id: uuid!(61)?,
                target: PurgeTarget::Workspace {
                    workspace_id: fixture.other.workspace_id,
                },
                initiator: person("ana"),
            },
        )
        .await?;
    let report = storage
        .purger()
        .purge_workspace(&tenant, uuid!(62)?, purge.id, fixture.other.workspace_id)
        .await?;
    let mut completed = purge;
    completed.state = PurgeState::Completed;
    completed.report = Some(report);
    storage.records().update_purge(&tenant, completed).await?;
    let later = tempfile::tempdir()?;
    storage
        .backups()
        .write_backup_set(later.path(), None)
        .await?;
    drop(fixture);
    let fresh = tempfile::tempdir()?;
    let restored = Storage::open(fresh.path())?;
    let error = err_of(
        restored
            .backups()
            .restore_replacement(
                &later.path().join("installation").join("local.zip"),
                &[kept],
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    Ok(())
}

#[tokio::test]
async fn backup_an_import_refuses_an_archive_of_a_live_or_purged_workspace() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    let backups = storage.backups();
    let archive = backups
        .write_workspace_archive(&fixture.workspace, uuid!(40)?)
        .await?;
    let downloaded = std::fs::read(
        storage
            .data()
            .objects_path()
            .join("local")
            .join(archive.digest.as_str().get(..2).unwrap_or_default())
            .join(archive.digest.as_str()),
    )?;
    let (blank, _) = create(storage, "local", 70, "Blank").await?;
    let error = err_of(
        backups
            .restore_import(
                &blank,
                archive.digest.clone(),
                uuid!(71)?,
                vec!["ana".to_owned()],
            )
            .await,
    )?;
    assert_eq!(
        error.code,
        ErrorCode::Conflict,
        "the archived workspace still exists"
    );
    let tenant = tenant("local")?;
    storage
        .records()
        .create_purge(
            &tenant,
            uuid!(60)?,
            NewPurge {
                id: uuid!(61)?,
                target: PurgeTarget::Workspace {
                    workspace_id: fixture.workspace.workspace_id,
                },
                initiator: person("ana"),
            },
        )
        .await?;
    storage
        .purger()
        .purge_workspace(
            &tenant,
            uuid!(62)?,
            uuid!(61)?,
            fixture.workspace.workspace_id,
        )
        .await?;
    let error = err_of(
        backups
            .restore_import(
                &blank,
                archive.digest.clone(),
                uuid!(72)?,
                vec!["ana".to_owned()],
            )
            .await,
    )?;
    assert_eq!(
        error.code,
        ErrorCode::NotFound,
        "the purge removed the archive it wrote"
    );
    // A copy downloaded before the purge, uploaded again: the same bytes are refused.
    let uploaded = storage
        .blobs()
        .put(
            &blank,
            Box::pin(std::io::Cursor::new(downloaded)),
            1 << 30,
            None,
        )
        .await?;
    assert_eq!(uploaded.digest, archive.digest);
    let error = err_of(
        backups
            .restore_import(&blank, uploaded.digest, uuid!(73)?, vec!["ana".to_owned()])
            .await,
    )?;
    assert_eq!(
        error.code,
        ErrorCode::Conflict,
        "purged content never comes back"
    );
    Ok(())
}

#[tokio::test]
async fn backup_an_import_fills_a_blank_workspace_and_keeps_unknown_editors_drafts_in_the_archive()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    let backups = storage.backups();
    let archive = backups
        .write_workspace_archive(&fixture.workspace, uuid!(40)?)
        .await?;
    let (target, _) = create(storage, "elsewhere", 80, "Copy").await?;
    let bytes = std::fs::read(
        storage
            .data()
            .objects_path()
            .join("local")
            .join(archive.digest.as_str().get(..2).unwrap_or_default())
            .join(archive.digest.as_str()),
    )?;
    let uploaded = storage
        .blobs()
        .put(
            &target,
            Box::pin(std::io::Cursor::new(bytes)),
            1 << 30,
            None,
        )
        .await?;
    let (busy, busy_head) = create(storage, "elsewhere", 81, "Busy").await?;
    commit(
        storage,
        &busy,
        82,
        &busy_head,
        vec![note(uuid!(14)?, "a.md", "x")?],
    )
    .await?;
    let error = err_of(
        backups
            .restore_import(&busy, uploaded.digest.clone(), uuid!(83)?, Vec::new())
            .await,
    )?;
    assert_eq!(
        error.code,
        ErrorCode::Conflict,
        "only a blank workspace is filled"
    );
    let report = backups
        .restore_import(
            &target,
            uploaded.digest.clone(),
            uuid!(84)?,
            vec!["ana".to_owned()],
        )
        .await?;
    assert_eq!(
        (
            report.items,
            report.drafts_restored,
            report.drafts_unassigned
        ),
        (2, 1, 1)
    );
    let repeated = backups
        .restore_import(
            &target,
            uploaded.digest.clone(),
            uuid!(84)?,
            vec!["ana".to_owned()],
        )
        .await?;
    assert_eq!(repeated, report, "a repeated restore changes nothing");
    let item = storage
        .versions()
        .show(&target, &fixture.head, uuid!(10)?)
        .await?;
    assert_eq!(
        item.body, "The harbor plan.",
        "item identities and revisions are kept"
    );
    assert!(
        storage
            .drafts()
            .get(&target, uuid!(10)?, "ana")
            .await?
            .is_some()
    );
    assert!(
        storage
            .drafts()
            .get(&target, uuid!(10)?, "bea")
            .await?
            .is_none()
    );
    storage
        .blobs()
        .open(&target, &fixture.original, 0, 1)
        .await?;
    Ok(())
}

/// Review probe R5: a stop between the imported rows and the import's own record. The record
/// insert is made to fail (a trigger on `restores`, as a crash at that point); the stopped
/// import must leave no row, and the retry under the same mutation completes.
#[tokio::test]
async fn backup_an_import_stopped_before_its_record_leaves_nothing_and_completes_on_retry()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    let backups = storage.backups();
    let archive = backups
        .write_workspace_archive(&fixture.workspace, uuid!(40)?)
        .await?;
    let (target, _) = create(storage, "elsewhere", 80, "Copy").await?;
    let bytes = std::fs::read(
        storage
            .data()
            .objects_path()
            .join("local")
            .join(archive.digest.as_str().get(..2).unwrap_or_default())
            .join(archive.digest.as_str()),
    )?;
    let uploaded = storage
        .blobs()
        .put(
            &target,
            Box::pin(std::io::Cursor::new(bytes)),
            1 << 30,
            None,
        )
        .await?;
    let records = rusqlite::Connection::open(directory.path().join("records.sqlite"))?;
    records.execute_batch(
        "CREATE TRIGGER stop_restore BEFORE INSERT ON restores
         BEGIN SELECT RAISE(ABORT, 'stopped before the import record'); END;",
    )?;
    let stopped = err_of(
        backups
            .restore_import(
                &target,
                uploaded.digest.clone(),
                uuid!(84)?,
                vec!["ana".to_owned()],
            )
            .await,
    )?;
    assert!(
        storage
            .drafts()
            .get(&target, uuid!(10)?, "ana")
            .await?
            .is_none(),
        "no imported row outlives the stopped import: {stopped}"
    );
    records.execute_batch("DROP TRIGGER stop_restore;")?;
    let report = backups
        .restore_import(
            &target,
            uploaded.digest.clone(),
            uuid!(84)?,
            vec!["ana".to_owned()],
        )
        .await?;
    assert_eq!(
        (
            report.items,
            report.drafts_restored,
            report.drafts_unassigned
        ),
        (2, 1, 1)
    );
    let repeated = backups
        .restore_import(&target, uploaded.digest, uuid!(84)?, vec!["ana".to_owned()])
        .await?;
    assert_eq!(repeated, report, "the record replays the report");
    Ok(())
}

#[tokio::test]
async fn migrations_take_a_pre_migration_backup_set_before_an_upgrade() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let workspace = fixture.workspace.clone();
    drop(fixture);
    {
        // The directory as records schema 1 left it: schema 2 only adds these tables.
        let connection = rusqlite::Connection::open(directory.path().join("records.sqlite"))?;
        connection.execute_batch(
            "DROP TABLE purge_progress; DROP TABLE backup_archives; DROP TABLE restores;
             PRAGMA user_version = 1;",
        )?;
    }
    let storage = Storage::open(directory.path())?;
    let sets: Vec<_> = std::fs::read_dir(storage.data().pre_migration_path())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    let set = some(sets.first(), "the pre-migration backup set")?;
    assert!(set.join("installation").join("local.zip").is_file());
    let archive = set
        .join("workspaces")
        .join("local")
        .join(format!("{}.zip", workspace.workspace_id.0));
    assert!(
        entries(&archive)?
            .iter()
            .any(|name| name == "records.sqlite")
    );
    assert_eq!(
        count(
            &directory.path().join("records.sqlite"),
            "PRAGMA user_version"
        )?,
        2,
        "then the records are upgraded"
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
