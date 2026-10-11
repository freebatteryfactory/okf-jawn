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

/// Backup round-2 rulings 2 and 4 (N4b, RM): the same replacement restore run again accepts the
/// workspace this archive already restored in full; once the workspace has moved on (a new
/// commit), it is refused as existing, and its history is kept.
async fn rerun_accepts_then_refuses_a_moved_workspace(
    restored: &Storage,
    installation: &Path,
    archives: &[std::path::PathBuf],
    workspace: &StorageScope,
    head: &Revision,
) -> TestResult {
    restored
        .backups()
        .restore_replacement(installation, archives)
        .await?;
    let moved = commit(
        restored,
        workspace,
        90,
        head,
        vec![note(uuid!(15)?, "notes/later.md", "Later.")?],
    )
    .await?;
    let error = err_of(
        restored
            .backups()
            .restore_replacement(installation, archives)
            .await,
    )?;
    assert_eq!(
        error.code,
        ErrorCode::Conflict,
        "a workspace that exists with other content is not restored again"
    );
    assert!(
        error.message.contains("already exists"),
        "refused before anything is written: {}",
        error.message
    );
    assert_eq!(
        restored.versions().head(workspace).await?,
        moved,
        "the live workspace's history is never deleted"
    );
    Ok(())
}

/// PR #20 finding (Codex P1): after a replacement restore the retained export is back, its
/// artifact record and its bytes.
async fn assert_export_restored(
    restored: &Storage,
    workspace: &StorageScope,
    export: &Digest,
    data: &Path,
) -> TestResult {
    restored.blobs().open(workspace, export, 0, 1).await?;
    assert_eq!(
        count(
            &data.join("records.sqlite"),
            &format!(
                "SELECT count(*) FROM artifacts WHERE kind = 'export' AND digest = '{}'",
                export.as_str()
            )
        )?,
        1,
        "the export's artifact record comes back"
    );
    Ok(())
}

/// A replacement restore into a directory that holds only the repository directory of the
/// archived workspace, with no record of it (what a restore that stopped inside it leaves),
/// replaces it and restores the workspace (backup re-review ruling 4; it was refused before).
async fn a_leftover_repository_is_replaced(
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
    leftover
        .backups()
        .restore_replacement(installation, archives)
        .await?;
    leftover.catalog().open(workspace).await?;
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
    // PR #20 finding (Codex P1, snapshot.rs:53), owner ruling: a retained export is an
    // application record with retained bytes, so the archive keeps both (SPEC section 12).
    assert!(
        names.contains(&format!("objects/{}", fixture.export.as_str())),
        "an archive holds the workspace's retained export"
    );
    assert!(
        !names.contains(&format!("objects/{}", object.digest.as_str())),
        "an archive never holds a backup archive"
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
    assert_eq!(
        count(&records, "SELECT count(*) FROM artifacts")?,
        1,
        "the export's artifact record is kept"
    );
    assert_eq!(
        count(
            &records,
            "SELECT count(*) FROM artifacts WHERE kind = 'workspace_backup'"
        )?,
        0,
        "no workspace archive record"
    );
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
        4,
        "two workspace archives, the tenant's installation archive and the installation-level one"
    );
    let Fixture {
        storage,
        workspace,
        head,
        candidate,
        original,
        export,
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
    assert_export_restored(&restored, &workspace, &export, directory.path()).await?;
    restored
        .versions()
        .show(&workspace, &candidate, uuid!(12)?)
        .await?;
    assert_connector_revoked(&restored).await?;
    rerun_accepts_then_refuses_a_moved_workspace(
        &restored,
        &installation,
        &archives,
        &workspace,
        &head,
    )
    .await?;
    a_leftover_repository_is_replaced(&installation, &archives, &workspace).await?;
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

/// PR #20 finding (Codex P1, `backup/mod.rs:469`): an installation with no workspace and no purge
/// still gets the installation archive of the requested tenant, and a replacement restore from
/// it brings the connector back (revoked, since no secret is kept).
#[tokio::test]
async fn backup_an_empty_installation_still_writes_its_installation_archive() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    {
        let storage = Storage::open(directory.path())?;
        storage
            .credentials()
            .create_connector(
                uuid!(50)?,
                NewConnector {
                    label: "Desk".to_owned(),
                    workspace_ids: Vec::new(),
                    allow_propose: false,
                },
            )
            .await?;
        let written = storage
            .backups()
            .write_backup_set(output.path(), Some(&tenant("local")?))
            .await?;
        let installation = output.path().join("installation").join("local.zip");
        assert_eq!(
            written,
            vec![installation, output.path().join("installation.zip")],
            "the tenant's installation archive and the installation-level one"
        );
    }
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    restored
        .backups()
        .restore_replacement(&output.path().join("installation").join("local.zip"), &[])
        .await?;
    assert_connector_revoked(&restored).await
}

/// PR #20 finding (Codex P2, `restore.rs:542`): an import into another installation rewrites the
/// workspace identity only in structured fields. A proposal's `workspace_id` becomes the
/// target's; its description and a comment's text that quote the source workspace's UUID keep
/// it.
#[tokio::test]
async fn backup_an_import_rewrites_only_structured_workspace_identity_fields() -> TestResult {
    use okf_jawn_core::proposals::ProposalStore;
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    let source = fixture.workspace.workspace_id.0.to_string();
    let proposal: okf_jawn_contract::proposal::Proposal = serde_json::from_value(json!({
        "id": uuid_text(24),
        "workspace_id": source,
        "base_revision": fixture.head,
        "proposal_revision": fixture.head,
        "content_digest": "d".repeat(64),
        "title": "Move the plan",
        "description": format!("copied from workspace {source}"),
        "changes": [],
        "kind": "content",
        "status": "open",
        "created_by": "agent",
        "created_via": "mcp_delegation",
        "created_at": "2026-10-09T12:00:00.000Z"
    }))?;
    let proposals = storage.proposals();
    proposals
        .insert(&fixture.workspace, uuid!(23)?, proposal.clone())
        .await?;
    let comment: okf_jawn_contract::proposal::Comment = serde_json::from_value(json!({
        "id": "c1",
        "author": "ana",
        "text": format!("see {source} for the original"),
        "created_at": "2026-10-09T12:00:00.000Z"
    }))?;
    proposals
        .add_comment(&fixture.workspace, uuid!(25)?, proposal.id, comment)
        .await?;
    // Another installation, as SPEC section 12 imports: a second data directory.
    let archive = storage
        .backups()
        .write_workspace_archive(&fixture.workspace, uuid!(40)?)
        .await?;
    let bytes = std::fs::read(
        storage
            .data()
            .objects_path()
            .join("local")
            .join(archive.digest.as_str().get(..2).unwrap_or_default())
            .join(archive.digest.as_str()),
    )?;
    let elsewhere = tempfile::tempdir()?;
    let other = Storage::open(elsewhere.path())?;
    let (target, _) = create(&other, "local", 80, "Copy").await?;
    let uploaded = other
        .blobs()
        .put(
            &target,
            Box::pin(std::io::Cursor::new(bytes)),
            1 << 30,
            None,
        )
        .await?;
    other
        .backups()
        .restore_import(&target, uploaded.digest, uuid!(84)?, vec!["ana".to_owned()])
        .await?;
    let proposals = other.proposals();
    let imported = proposals.get(&target, proposal.id).await?;
    assert_eq!(
        imported.workspace_id, target.workspace_id,
        "the structured field is rewritten"
    );
    assert_eq!(
        imported.description,
        format!("copied from workspace {source}"),
        "user-authored text keeps the identity it quotes"
    );
    let comments = proposals
        .list_comments(
            &target,
            proposal.id,
            okf_jawn_core::storage::Page {
                cursor: None,
                limit: 10,
            },
        )
        .await?;
    let text = &some(comments.items.first(), "the imported comment")?.text;
    assert_eq!(text, &format!("see {source} for the original"));
    Ok(())
}

/// PR #20 finding (`CodeRabbit`, `backup/mod.rs:595`): a replacement restore run again (after a
/// later workspace archive failed, say) does not append the archived tenant events twice.
#[tokio::test]
async fn backup_a_replacement_restore_run_again_restores_each_tenant_event_once() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    {
        let storage = Storage::open(directory.path())?;
        storage
            .events()
            .append(
                &EventScope::Tenant(tenant("local")?),
                None,
                NewEvent {
                    kind: EventKind::SignedIn,
                    revision: None,
                    item_id: None,
                    job_id: None,
                    connector_id: None,
                    actor: None,
                    operation: None,
                },
            )
            .await?;
        storage
            .backups()
            .write_backup_set(output.path(), Some(&tenant("local")?))
            .await?;
    }
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let installation = output.path().join("installation").join("local.zip");
    for _ in 0..2 {
        restored
            .backups()
            .restore_replacement(&installation, &[])
            .await?;
    }
    assert_eq!(
        count(
            &directory.path().join("records.sqlite"),
            "SELECT count(*) FROM events WHERE workspace_id IS NULL"
        )?,
        1,
        "the tenant event is restored once"
    );
    Ok(())
}

/// Record a retained artifact of `kind` over `bytes` in `scope` (job under mutation `n`, the
/// artifact under `n + 1`).
async fn retained(
    storage: &Storage,
    scope: &StorageScope,
    n: u32,
    kind: ArtifactKind,
    bytes: &[u8],
) -> Fallible<Digest> {
    let object = put(storage, scope, bytes).await?;
    let job_scope = JobScope::Workspace(scope.clone());
    let job = storage
        .records()
        .create_job(
            &job_scope,
            NewJob {
                mutation_id: uuid!(n)?,
                initiator: person("ana"),
                spec: JobSpec::BackupWorkspace,
            },
        )
        .await?;
    storage
        .records()
        .record_artifact(
            &job_scope,
            uuid!(n.saturating_add(1))?,
            NewArtifact {
                kind,
                object: object.clone(),
                media_type: "application/zip".to_owned(),
                created_by_job: job.id,
            },
        )
        .await?;
    Ok(object.digest)
}

/// Every object file under `data/objects` whose bytes contain `marker`.
fn objects_holding(data: &Path, marker: &[u8]) -> Fallible<Vec<std::path::PathBuf>> {
    let mut pending = vec![data.join("objects")];
    let mut found = Vec::new();
    while let Some(directory) = pending.pop() {
        if !directory.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if contains(&std::fs::read(&path)?, marker) {
                found.push(path);
            }
        }
    }
    Ok(found)
}

/// Backup re-review B1 (probe `rr_concurrent_same_mutation_archives_with_draft_churn`): two
/// archives under one mutation while drafts autosave. The second re-reads the record under the
/// lock and stores nothing, so one archive object exists and the purge leaves none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backup_two_archives_under_one_mutation_store_one_object_a_purge_removes() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (workspace, created) = create(&storage, "local", 1, "zanzibar-marker-workspace").await?;
    let item: ItemId = uuid!(10)?;
    let head = commit(
        &storage,
        &workspace,
        3,
        &created,
        vec![note(item, "notes/plan.md", "plan")?],
    )
    .await?;
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let churn = {
        let (drafts, scope, stop, head) = (
            storage.drafts(),
            workspace.clone(),
            Arc::clone(&stop),
            head.clone(),
        );
        tokio::spawn(async move {
            let mut n = 1000_u32;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                n += 1;
                let mutation = serde_json::from_value(json!(uuid_text(n)))
                    .map_err(|error| error.to_string())?;
                drafts
                    .save(
                        &scope,
                        mutation,
                        DraftWrite {
                            item_id: item,
                            editor: "ana".to_owned(),
                            base_revision: head.clone(),
                            body: format!("autosave {n}"),
                            properties: BTreeMap::new(),
                            content_digest: Digest::try_from("e".repeat(64))
                                .map_err(|error| error.to_string())?,
                        },
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                tokio::task::yield_now().await;
            }
            Ok::<u32, String>(n)
        })
    };
    let mutation = uuid!(40)?;
    let first = {
        let (backups, scope) = (storage.backups(), workspace.clone());
        tokio::spawn(async move { backups.write_workspace_archive(&scope, mutation).await })
    };
    let second = {
        let (backups, scope) = (storage.backups(), workspace.clone());
        tokio::spawn(async move { backups.write_workspace_archive(&scope, mutation).await })
    };
    let (a, b) = (first.await??, second.await??);
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    churn.await??;
    assert_eq!(a, b, "both answer the recorded archive");
    assert_eq!(
        objects_holding(directory.path(), b"zanzibar-marker-workspace")?.len(),
        1,
        "one archive object is stored"
    );
    storage
        .records()
        .create_purge(
            &workspace.tenant_id,
            uuid!(50)?,
            NewPurge {
                id: uuid!(60)?,
                target: PurgeTarget::Workspace {
                    workspace_id: workspace.workspace_id,
                },
                initiator: person("ana"),
            },
        )
        .await?;
    storage
        .purger()
        .purge_workspace(
            &workspace.tenant_id,
            uuid!(61)?,
            uuid!(60)?,
            workspace.workspace_id,
        )
        .await?;
    assert_eq!(
        objects_holding(directory.path(), b"zanzibar-marker-workspace")?,
        Vec::<std::path::PathBuf>::new(),
        "a purge leaves no stored archive of the workspace"
    );
    Ok(())
}

/// Backup re-review R1 (probe `rr_export_without_upload_comes_back_after_replacement`): an
/// export that only its artifact names (no upload slot) comes back, record and bytes.
#[tokio::test]
async fn backup_an_export_only_its_artifact_names_comes_back_after_replacement() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let (workspace, export) = {
        let storage = Storage::open(directory.path())?;
        let (workspace, _) = create(&storage, "local", 1, "Research").await?;
        let export = retained(
            &storage,
            &workspace,
            30,
            ArtifactKind::Export,
            b"an export only the artifact names",
        )
        .await?;
        storage
            .backups()
            .write_backup_set(output.path(), Some(&tenant("local")?))
            .await?;
        (workspace, export)
    };
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let archive = output
        .path()
        .join("workspaces")
        .join("local")
        .join(format!("{}.zip", workspace.workspace_id.0));
    restored
        .backups()
        .restore_replacement(
            &output.path().join("installation").join("local.zip"),
            &[archive],
        )
        .await?;
    assert_export_restored(&restored, &workspace, &export, directory.path()).await
}

/// Backup re-review R3: a workspace archive never holds a `workspace_backup` artifact record of
/// the workspace, while it keeps the export's.
#[tokio::test]
async fn backup_a_workspace_archive_keeps_no_workspace_backup_record() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    retained(
        storage,
        &fixture.workspace,
        34,
        ArtifactKind::WorkspaceBackup,
        b"an earlier archive",
    )
    .await?;
    let archive = storage
        .backups()
        .write_workspace_archive(&fixture.workspace, uuid!(40)?)
        .await?;
    let path = storage
        .data()
        .objects_path()
        .join("local")
        .join(archive.digest.as_str().get(..2).unwrap_or_default())
        .join(archive.digest.as_str());
    let records = directory.path().join("archived.sqlite");
    entry(&path, "records.sqlite", &records)?;
    assert_eq!(
        count(
            &records,
            "SELECT count(*) FROM artifacts WHERE kind = 'workspace_backup'"
        )?,
        0
    );
    assert_eq!(
        count(
            &records,
            "SELECT count(*) FROM artifacts WHERE kind = 'export'"
        )?,
        1
    );
    Ok(())
}

/// Backup re-review R6 and R6b: an import into a workspace whose head is blank but which holds
/// a reference the archive does not hold at that commit is refused, whether the reference has a
/// name of its own or the archive's candidate name at another commit.
#[tokio::test]
async fn backup_an_import_refuses_a_blank_head_with_a_foreign_reference() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    let candidate_name = some(
        other_references(&repository_of(storage, &fixture.workspace)?)?
            .first()
            .map(|(name, _)| name.clone()),
        "the archive's candidate reference",
    )?;
    for (n, name) in [
        (80_u32, "refs/heads/other".to_owned()),
        (81, candidate_name),
    ] {
        let (target, blank) = create(storage, "elsewhere", n, "Copy").await?;
        let uploaded = archive_into(storage, &fixture.workspace, &target).await?;
        repository_of(storage, &target)?.reference(
            &name,
            git2::Oid::from_str(blank.as_str())?,
            true,
            "a foreign reference",
        )?;
        let error = err_of(
            storage
                .backups()
                .restore_import(
                    &target,
                    uploaded,
                    uuid!(n.saturating_add(4))?,
                    vec!["ana".to_owned()],
                )
                .await,
        )?;
        assert_eq!(error.code, ErrorCode::Conflict, "{name}: {error}");
    }
    Ok(())
}

/// Backup re-review R7: an unfiltered backup set writes the installation archive of a tenant
/// that only a tenant event names.
#[tokio::test]
async fn backup_an_unfiltered_set_writes_the_archive_of_a_tenant_only_an_event_names() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    storage
        .events()
        .append(
            &EventScope::Tenant(tenant("local")?),
            None,
            NewEvent {
                kind: EventKind::SignedIn,
                revision: None,
                item_id: None,
                job_id: None,
                connector_id: None,
                actor: None,
                operation: None,
            },
        )
        .await?;
    let written = storage
        .backups()
        .write_backup_set(output.path(), None)
        .await?;
    assert_eq!(
        written,
        vec![
            output.path().join("installation").join("local.zip"),
            output.path().join("installation.zip")
        ]
    );
    Ok(())
}

/// Backup round-2 ruling 1 (B2): a tenant event is keyed by its own identity (the source
/// installation and its sequence there), so a later archive of the same installation restores
/// only its new event and the first one is not repeated.
#[tokio::test]
async fn backup_a_later_archive_of_one_installation_restores_each_event_once() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (earlier, later) = (tempfile::tempdir()?, tempfile::tempdir()?);
    {
        let storage = Storage::open(directory.path())?;
        let event = || NewEvent {
            kind: EventKind::SignedIn,
            revision: None,
            item_id: None,
            job_id: None,
            connector_id: None,
            actor: None,
            operation: None,
        };
        let scope = EventScope::Tenant(tenant("local")?);
        storage.events().append(&scope, None, event()).await?;
        storage
            .backups()
            .write_backup_set(earlier.path(), Some(&tenant("local")?))
            .await?;
        storage.events().append(&scope, None, event()).await?;
        storage
            .backups()
            .write_backup_set(later.path(), Some(&tenant("local")?))
            .await?;
    }
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    for set in [earlier.path(), later.path()] {
        restored
            .backups()
            .restore_replacement(&set.join("installation").join("local.zip"), &[])
            .await?;
    }
    assert_eq!(
        count(
            &directory.path().join("records.sqlite"),
            "SELECT count(*) FROM events WHERE workspace_id IS NULL"
        )?,
        2,
        "the first archive's event once and the second's new one"
    );
    Ok(())
}

/// Backup re-review ruling 4: a replacement restore that stopped inside a workspace (a trigger
/// fails its rows after its repository was written) completes when run again, with no manual
/// removal of the repository it left.
#[tokio::test]
async fn backup_a_replacement_restore_stopped_inside_a_workspace_completes_on_retry() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let workspace = {
        let fixture = fixture(directory.path()).await?;
        fixture
            .storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        fixture.workspace.clone()
    };
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
    let records = rusqlite::Connection::open(directory.path().join("records.sqlite"))?;
    records.execute_batch(
        "CREATE TRIGGER stop_restore BEFORE INSERT ON drafts
         BEGIN SELECT RAISE(ABORT, 'stopped inside the workspace'); END;",
    )?;
    err_of(
        restored
            .backups()
            .restore_replacement(&installation, &archives)
            .await,
    )?;
    assert!(
        restored
            .data()
            .repositories_path()
            .join("local")
            .join(format!("{}.git", workspace.workspace_id.0))
            .exists(),
        "the stopped restore left its repository"
    );
    records.execute_batch("DROP TRIGGER stop_restore;")?;
    restored
        .backups()
        .restore_replacement(&installation, &archives)
        .await?;
    restored.catalog().open(&workspace).await?;
    Ok(())
}

/// The names directly under `directory`, sorted.
fn entries_of(directory: &Path) -> Fallible<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    names.sort();
    Ok(names)
}

/// Coordinator decision (ruling 2): an installation with only an identity and a connector, and
/// no tenant anywhere, backs up through the pre-migration path into the installation-level
/// archive, and a replacement restore from it brings both back.
#[tokio::test]
async fn backup_a_tenantless_installation_survives_the_pre_migration_set() -> TestResult {
    let directory = tempfile::tempdir()?;
    let subject = {
        let storage = Storage::open(directory.path())?;
        let subject = storage.credentials().installation_identity().await?.subject;
        storage
            .credentials()
            .create_connector(
                uuid!(50)?,
                NewConnector {
                    label: "Desk".to_owned(),
                    workspace_ids: Vec::new(),
                    allow_propose: false,
                },
            )
            .await?;
        subject
    };
    {
        // The directory as records schema 1 left it: opening it takes a pre-migration set.
        let connection = rusqlite::Connection::open(directory.path().join("records.sqlite"))?;
        connection.execute_batch(
            "DROP TABLE purge_progress; DROP TABLE backup_archives; DROP TABLE restores;
             DROP TABLE pending_puts; PRAGMA user_version = 1;",
        )?;
    }
    let level = {
        let storage = Storage::open(directory.path())?;
        let sets: Vec<_> = std::fs::read_dir(storage.data().pre_migration_path())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        let set = some(sets.first(), "the pre-migration set")?.clone();
        let level = set.join("installation.zip");
        assert!(
            level.is_file(),
            "the installation-level archive is in the set: {set:?} holds {:?}",
            entries_of(&set)?
        );
        let copy = tempfile::NamedTempFile::new()?;
        std::fs::copy(&level, copy.path())?;
        copy
    };
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    restored
        .backups()
        .restore_replacement(level.path(), &[])
        .await?;
    assert_eq!(
        restored
            .credentials()
            .installation_identity()
            .await?
            .subject,
        subject,
        "the installation identity comes back"
    );
    assert_connector_revoked(&restored).await
}

/// A tenant-level sign-in event.
fn sign_in() -> NewEvent {
    NewEvent {
        kind: EventKind::SignedIn,
        revision: None,
        item_id: None,
        job_id: None,
        connector_id: None,
        actor: None,
        operation: None,
    }
}

/// Backup round-2 B2 (probe `rr2_two_sets_of_an_unchanged_installation_restore_each_event_once`):
/// two backup sets of an unchanged installation, taken seconds apart (so the zip files differ),
/// restore its one event once.
#[tokio::test]
async fn backup_two_sets_of_an_unchanged_installation_restore_each_event_once() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (earlier, later) = (tempfile::tempdir()?, tempfile::tempdir()?);
    {
        let storage = Storage::open(directory.path())?;
        storage.credentials().installation_identity().await?;
        storage
            .events()
            .append(&EventScope::Tenant(tenant("local")?), None, sign_in())
            .await?;
        storage
            .backups()
            .write_backup_set(earlier.path(), None)
            .await?;
        // Zip entries carry a two-second timestamp: a later set is another file.
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        storage
            .backups()
            .write_backup_set(later.path(), None)
            .await?;
    }
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    for set in [earlier.path(), later.path()] {
        restored
            .backups()
            .restore_replacement(&set.join("installation").join("local.zip"), &[])
            .await?;
    }
    assert_eq!(
        count(
            &directory.path().join("records.sqlite"),
            "SELECT count(*) FROM events WHERE workspace_id IS NULL"
        )?,
        1,
        "the one event of an unchanged installation is restored once"
    );
    Ok(())
}

/// Backup round-2 ruling 2 (N4b, the reviewer's probe): a replacement restore of two
/// workspaces that stopped inside the second completes when the same command is run again; the
/// first, already restored in full, is accepted.
#[tokio::test]
async fn backup_a_replacement_restore_run_again_with_every_archive_completes() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let (workspace, other) = {
        let fixture = fixture(directory.path()).await?;
        fixture
            .storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        (fixture.workspace.clone(), fixture.other.clone())
    };
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let installation = output.path().join("installation").join("local.zip");
    let path = |scope: &StorageScope| {
        output
            .path()
            .join("workspaces")
            .join("local")
            .join(format!("{}.zip", scope.workspace_id.0))
    };
    let records = rusqlite::Connection::open(directory.path().join("records.sqlite"))?;
    records.execute_batch(&format!(
        "CREATE TRIGGER stop_restore BEFORE INSERT ON workspaces WHEN NEW.workspace_id = '{}'
         BEGIN SELECT RAISE(ABORT, 'stopped inside the second workspace'); END;",
        other.workspace_id.0
    ))?;
    err_of(
        restored
            .backups()
            .restore_replacement(&installation, &[path(&workspace), path(&other)])
            .await,
    )?;
    records.execute_batch("DROP TRIGGER stop_restore;")?;
    let reports = restored
        .backups()
        .restore_replacement(&installation, &[path(&workspace), path(&other)])
        .await?;
    assert_eq!(reports.len(), 2);
    restored.catalog().open(&workspace).await?;
    restored.catalog().open(&other).await?;
    Ok(())
}

/// Backup round-2 ruling 3 (N7): a replacement restore refuses a workspace that any recorded
/// purge names, an unfinished one included (here the purge is recorded and stopped before it
/// removed anything), with `Conflict` naming the purge.
#[tokio::test]
async fn backup_a_replacement_restore_refuses_a_workspace_an_unfinished_purge_names() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let workspace = {
        let fixture = fixture(directory.path()).await?;
        fixture
            .storage
            .records()
            .create_purge(
                &tenant("local")?,
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
        fixture
            .storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        fixture.workspace.clone()
    };
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let error = err_of(
        restored
            .backups()
            .restore_replacement(
                &output.path().join("installation").join("local.zip"),
                &[output
                    .path()
                    .join("workspaces")
                    .join("local")
                    .join(format!("{}.zip", workspace.workspace_id.0))],
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert!(
        error.message.contains(&uuid_text(61)),
        "names the purge: {error}"
    );
    Ok(())
}

/// Backup round-2 RW (probe `rr2_level_archive_with_workspaces_is_refused_before_anything_is_written`):
/// the installation-level archive given with workspaces is refused before anything is written.
#[tokio::test]
async fn backup_the_level_archive_with_workspaces_is_refused_before_anything_is_written()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let workspace = {
        let fixture = fixture(directory.path()).await?;
        fixture
            .storage
            .credentials()
            .installation_identity()
            .await?;
        fixture
            .storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        fixture.workspace.clone()
    };
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let archive = output
        .path()
        .join("workspaces")
        .join("local")
        .join(format!("{}.zip", workspace.workspace_id.0));
    let error = err_of(
        restored
            .backups()
            .restore_replacement(&output.path().join("installation.zip"), &[archive])
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::InvalidInput, "{error}");
    let records = directory.path().join("records.sqlite");
    assert_eq!(
        count(&records, "SELECT count(*) FROM installation")?,
        0,
        "no identity written"
    );
    assert_eq!(
        count(&records, "SELECT count(*) FROM workspaces")?,
        0,
        "no workspace written"
    );
    Ok(())
}

/// Backup round-2 R7s (probe `rr2_an_unfiltered_set_writes_the_archive_of_a_tenant_only_a_job_names`):
/// tenant discovery reads the schema, so a tenant only a tenant job names gets its archive.
#[tokio::test]
async fn backup_an_unfiltered_set_writes_the_archive_of_a_tenant_only_a_job_names() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    storage
        .records()
        .create_job(
            &JobScope::Tenant(tenant("other")?),
            NewJob {
                mutation_id: uuid!(30)?,
                initiator: person("ana"),
                spec: JobSpec::BackupInstallation,
            },
        )
        .await?;
    let written = storage
        .backups()
        .write_backup_set(output.path(), None)
        .await?;
    assert!(
        written.contains(&output.path().join("installation").join("other.zip")),
        "{written:?}"
    );
    Ok(())
}

/// Write a workspace archive of `source` under mutation 40 and put its bytes into `target`, as
/// an upload of it would; returns the digest to import.
async fn archive_into(
    storage: &Storage,
    source: &StorageScope,
    target: &StorageScope,
) -> Fallible<Digest> {
    let archive = storage
        .backups()
        .write_workspace_archive(source, uuid!(40)?)
        .await?;
    let bytes = std::fs::read(
        storage
            .data()
            .objects_path()
            .join(source.tenant_id.as_str())
            .join(archive.digest.as_str().get(..2).unwrap_or_default())
            .join(archive.digest.as_str()),
    )?;
    Ok(storage
        .blobs()
        .put(target, Box::pin(std::io::Cursor::new(bytes)), 1 << 30, None)
        .await?
        .digest)
}

/// The bare repository of `scope`.
fn repository_of(storage: &Storage, scope: &StorageScope) -> Fallible<git2::Repository> {
    Ok(git2::Repository::open_bare(
        storage
            .data()
            .repositories_path()
            .join(scope.tenant_id.as_str())
            .join(format!("{}.git", scope.workspace_id.0)),
    )?)
}

/// The accepted head of `repository` and every commit on its first-parent line whose trailer
/// names mutation `n`.
fn carrying(repository: &git2::Repository, n: u32) -> Fallible<(git2::Oid, Vec<git2::Oid>)> {
    let trailer = format!("Okf-Jawn-Mutation: {}", uuid_text(n));
    let head = repository.refname_to_id("refs/heads/main")?;
    let mut found = Vec::new();
    let mut next = Some(head);
    while let Some(oid) = next {
        let commit = repository.find_commit(oid)?;
        if commit
            .message()
            .is_ok_and(|message| message.contains(&trailer))
        {
            found.push(oid);
        }
        next = commit.parent_ids().next();
    }
    Ok((head, found))
}

/// Assert that the head of `target` is the one commit carrying mutation 84: its only parent is
/// the archived head and its tree is the archived head's.
fn assert_one_restore_commit(
    storage: &Storage,
    target: &StorageScope,
    archived: &Revision,
) -> TestResult {
    let repository = repository_of(storage, target)?;
    let (head, found) = carrying(&repository, 84)?;
    assert_eq!(
        found,
        vec![head],
        "one commit carries the import's mutation, at the head"
    );
    let commit = repository.find_commit(head)?;
    let archived = repository.find_commit(git2::Oid::from_str(archived.as_str())?)?;
    assert_eq!(commit.parent_ids().collect::<Vec<_>>(), vec![archived.id()]);
    assert_eq!(commit.tree_id(), archived.tree_id(), "no content change");
    Ok(())
}

/// Take-main re-review F1 (probe `rr_an_import_leaves_a_commit_carrying_its_mutation`): the
/// import's revision is found by its mutation, as ingest's restore handler looks for it, and a
/// repeated import writes no second commit.
#[tokio::test]
async fn backup_an_import_leaves_a_commit_carrying_its_mutation() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    let backups = storage.backups();
    let (target, _) = create(storage, "elsewhere", 80, "Copy").await?;
    let uploaded = archive_into(storage, &fixture.workspace, &target).await?;
    let report = backups
        .restore_import(
            &target,
            uploaded.clone(),
            uuid!(84)?,
            vec!["ana".to_owned()],
        )
        .await?;
    let head = storage.versions().head(&target).await?;
    let found = storage
        .versions()
        .find_commit(&target, uuid!(84)?, None)
        .await?;
    assert_eq!(
        found,
        Some(head.clone()),
        "the restore's revision is found by its mutation"
    );
    assert_one_restore_commit(storage, &target, &fixture.head)?;
    let repeated = backups
        .restore_import(&target, uploaded, uuid!(84)?, vec!["ana".to_owned()])
        .await?;
    assert_eq!(repeated, report);
    assert_eq!(
        storage.versions().head(&target).await?,
        head,
        "nothing is written again"
    );
    assert_one_restore_commit(storage, &target, &fixture.head)?;
    Ok(())
}

/// F1, a stop after the history is installed and before the restore commit (the archive's
/// references are set straight into the target repository, as the first attempt left them):
/// the resumed import writes the one commit.
#[tokio::test]
async fn backup_an_import_stopped_before_its_commit_writes_it_once_on_retry() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    let (target, _) = create(storage, "elsewhere", 80, "Copy").await?;
    let uploaded = archive_into(storage, &fixture.workspace, &target).await?;
    let source = storage
        .data()
        .repositories_path()
        .join("local")
        .join(format!("{}.git", fixture.workspace.workspace_id.0));
    let repository = repository_of(storage, &target)?;
    repository
        .remote_anonymous(some(source.to_str(), "a UTF-8 repository path")?)?
        .fetch(&["+refs/*:refs/*"], None, None)?;
    assert_eq!(
        repository.refname_to_id("refs/heads/main")?.to_string(),
        fixture.head.as_str(),
        "the history is installed and no commit carries the import"
    );
    storage
        .backups()
        .restore_import(&target, uploaded, uuid!(84)?, vec!["ana".to_owned()])
        .await?;
    assert_one_restore_commit(storage, &target, &fixture.head)?;
    Ok(())
}

/// F1, a stop after the restore commit and before the import's rows and record (a trigger on
/// `restores` fails the rows' transaction): the resumed import finds the commit at the head and
/// writes no second one.
#[tokio::test]
async fn backup_an_import_stopped_after_its_commit_writes_no_second_one_on_retry() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    let backups = storage.backups();
    let (target, _) = create(storage, "elsewhere", 80, "Copy").await?;
    let uploaded = archive_into(storage, &fixture.workspace, &target).await?;
    let records = rusqlite::Connection::open(directory.path().join("records.sqlite"))?;
    records.execute_batch(
        "CREATE TRIGGER stop_restore BEFORE INSERT ON restores
         BEGIN SELECT RAISE(ABORT, 'stopped before the import record'); END;",
    )?;
    err_of(
        backups
            .restore_import(
                &target,
                uploaded.clone(),
                uuid!(84)?,
                vec!["ana".to_owned()],
            )
            .await,
    )?;
    assert_one_restore_commit(storage, &target, &fixture.head)?;
    let stopped = storage.versions().head(&target).await?;
    records.execute_batch("DROP TRIGGER stop_restore;")?;
    backups
        .restore_import(&target, uploaded, uuid!(84)?, vec!["ana".to_owned()])
        .await?;
    assert_eq!(storage.versions().head(&target).await?, stopped);
    assert_one_restore_commit(storage, &target, &fixture.head)?;
    Ok(())
}

/// Every reference of `repository` but the accepted head, with its commit, sorted.
fn other_references(repository: &git2::Repository) -> Fallible<Vec<(String, String)>> {
    let mut found = Vec::new();
    for reference in repository.references()? {
        let reference = reference?;
        let name = reference.name().unwrap_or_default().to_owned();
        if name != "refs/heads/main" {
            found.push((
                name,
                reference
                    .target()
                    .map(|oid| oid.to_string())
                    .unwrap_or_default(),
            ));
        }
    }
    found.sort();
    Ok(found)
}

/// Fetch the references `refspec` names from `source`'s repository into `target`'s, as a
/// restore that stopped while setting references left them.
fn fetch_into(
    storage: &Storage,
    source: &StorageScope,
    target: &StorageScope,
    refspec: &str,
) -> TestResult {
    let path = storage
        .data()
        .repositories_path()
        .join(source.tenant_id.as_str())
        .join(format!("{}.git", source.workspace_id.0));
    repository_of(storage, target)?
        .remote_anonymous(some(path.to_str(), "a UTF-8 repository path")?)?
        .fetch(&[refspec], None, None)?;
    Ok(())
}

/// PR #20 finding (`CodeRabbit`, `restore.rs:311`): an import that stopped after it set the accepted
/// head and before a candidate reference sets the candidate when resumed.
#[tokio::test]
async fn backup_an_import_stopped_after_main_before_a_candidate_reference_sets_it_on_retry()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    let (target, _) = create(storage, "elsewhere", 80, "Copy").await?;
    let uploaded = archive_into(storage, &fixture.workspace, &target).await?;
    // The pack is indexed (every object fetched under a scratch namespace), the accepted head
    // set, and no other reference yet.
    fetch_into(
        storage,
        &fixture.workspace,
        &target,
        "+refs/*:refs/fetched/*",
    )?;
    {
        let repository = repository_of(storage, &target)?;
        repository.reference(
            "refs/heads/main",
            git2::Oid::from_str(fixture.head.as_str())?,
            true,
            "a restore stopped after main",
        )?;
        let fetched: Vec<String> = repository
            .references_glob("refs/fetched/*")?
            .filter_map(|reference| reference.ok()?.name().ok().map(str::to_owned))
            .collect();
        for name in fetched {
            repository.find_reference(&name)?.delete()?;
        }
    }
    let source = other_references(&repository_of(storage, &fixture.workspace)?)?;
    assert_ne!(
        source,
        Vec::new(),
        "the archive holds a candidate reference"
    );
    assert_eq!(
        other_references(&repository_of(storage, &target)?)?,
        Vec::new()
    );
    storage
        .backups()
        .restore_import(&target, uploaded, uuid!(84)?, vec!["ana".to_owned()])
        .await?;
    assert_eq!(
        other_references(&repository_of(storage, &target)?)?,
        source,
        "the candidate reference is set"
    );
    assert_one_restore_commit(storage, &target, &fixture.head)?;
    Ok(())
}

/// PR #20 finding (`CodeRabbit`, `restore.rs:311`): an import that stopped after it set a candidate
/// reference and before the accepted head is not refused as a non-blank workspace when resumed.
#[tokio::test]
async fn backup_an_import_stopped_after_a_candidate_reference_before_main_completes_on_retry()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    let (target, blank) = create(storage, "elsewhere", 80, "Copy").await?;
    let uploaded = archive_into(storage, &fixture.workspace, &target).await?;
    fetch_into(
        storage,
        &fixture.workspace,
        &target,
        "+refs/okf-jawn/*:refs/okf-jawn/*",
    )?;
    let repository = repository_of(storage, &target)?;
    assert_eq!(
        repository.refname_to_id("refs/heads/main")?.to_string(),
        blank.as_str(),
        "the accepted head is still the blank one"
    );
    assert_ne!(
        other_references(&repository)?,
        Vec::new(),
        "a candidate reference is set"
    );
    storage
        .backups()
        .restore_import(&target, uploaded, uuid!(84)?, vec!["ana".to_owned()])
        .await?;
    assert_one_restore_commit(storage, &target, &fixture.head)?;
    Ok(())
}

/// F1, the purge rule: an import into a workspace a purge names is refused under the
/// workspace's lock before any Git write. The archive holds no object, so no object put
/// refuses first.
#[tokio::test]
async fn backup_an_import_into_a_purged_workspace_writes_no_history() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (source, source_head) = create(&storage, "local", 1, "Notes").await?;
    commit(
        &storage,
        &source,
        3,
        &source_head,
        vec![note(uuid!(10)?, "notes/plan.md", "The harbor plan.")?],
    )
    .await?;
    let (target, blank) = create(&storage, "elsewhere", 80, "Copy").await?;
    let uploaded = archive_into(&storage, &source, &target).await?;
    storage
        .records()
        .create_purge(
            &tenant("elsewhere")?,
            uuid!(60)?,
            NewPurge {
                id: uuid!(61)?,
                target: PurgeTarget::Workspace {
                    workspace_id: target.workspace_id,
                },
                initiator: person("ana"),
            },
        )
        .await?;
    let error = err_of(
        storage
            .backups()
            .restore_import(&target, uploaded, uuid!(84)?, vec!["ana".to_owned()])
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert!(
        error.message.contains(&uuid_text(61)),
        "names the purge: {error}"
    );
    let (head, found) = carrying(&repository_of(&storage, &target)?, 84)?;
    assert_eq!(
        head.to_string(),
        blank.as_str(),
        "the workspace is as it was"
    );
    assert_eq!(
        found,
        Vec::<git2::Oid>::new(),
        "no commit carries the import"
    );
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
             DROP TABLE pending_puts; PRAGMA user_version = 1;",
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
