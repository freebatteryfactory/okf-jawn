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
use okf_jawn_contract::extraction::ConversionSettings;
use okf_jawn_contract::identity::{Digest, ItemId, Revision, TenantId, UploadId, WorkspacePath};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_contract::purge::{PurgeState, PurgeTarget};
use okf_jawn_contract::workspace::ArtifactKind;
use okf_jawn_core::credentials::{CredentialStore, NewConnector};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::events::{EventLog, EventScope, NewEvent};
use okf_jawn_core::jobs::{
    ClaimedJob, DerivedKind, DerivedObject, JobCompletion, JobScope, JobSpec, NewArtifact, NewJob,
    NewPurge, RecordStore,
};
use okf_jawn_core::storage::{
    Backups, BlobStore, CandidateChanges, CandidateCheck, CommitChanges, NewWorkspace, ObjectInfo,
    Provenance, Purger, REPLAY_WARNINGS_UNAVAILABLE, SourceCard, StorageScope, TreeEdit,
    VersionStore, WorkspaceCatalog,
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

/// What a replacement restore returned.
type Restored = Result<Vec<okf_jawn_contract::workspace::RestoreReport>, ApiError>;

/// A zip archive being built in memory.
type ZipBuild = zip::ZipWriter<std::io::Cursor<Vec<u8>>>;

/// A Git pack and the references it was built from (`rr9_pack_of`).
type Rr9Pack = (Vec<u8>, Vec<(String, String)>);

/// What damages a workspace's repository through Git for a crafted archive, giving the objects
/// to add to the archive (`rr9_set_with_history`).
type Rr9Damage<'a> = dyn Fn(&Storage, &StorageScope) -> Fallible<Vec<Vec<u8>>> + 'a;

/// What picks the object a crafted pack leaves out (`rr9_set_with_history`).
type Rr9Omit<'a> = dyn Fn(&git2::Repository) -> Fallible<Option<git2::Oid>> + 'a;

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

/// A zip local file header's signature.
const LOCAL: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
/// A zip central directory record's signature.
const CENTRAL: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
/// This build's `drafts` columns, without the table's key.
const RR5_COLUMNS: &str = "tenant_id TEXT NOT NULL, workspace_id TEXT NOT NULL, \
    item_id TEXT NOT NULL, editor TEXT NOT NULL, saved_at TEXT NOT NULL, \
    record TEXT NOT NULL, body TEXT NOT NULL, properties TEXT NOT NULL";
/// This build's `drafts` key.
const RR5_KEY: &str = "PRIMARY KEY (tenant_id, workspace_id, item_id, editor)";

impl CandidateCheck for Accept {
    fn check(
        &self,
        _root: &Path,
        _rewritten: &[okf_jawn_contract::identity::WorkspacePath],
    ) -> Result<Vec<Warning>, ApiError> {
        Ok(Vec::new())
    }
}

/// The codes of `warnings`, in order.
fn codes(warnings: &[Warning]) -> Vec<&str> {
    warnings
        .iter()
        .map(|warning| warning.code.as_str())
        .collect()
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
        .await?
        .revision;
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
        .await?
        .object;
    let repeated = backups
        .write_workspace_archive(&fixture.workspace, uuid!(40)?)
        .await?
        .object;
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
    // R-I10: the warnings kept for a commit's replay are this installation's retry records.
    assert!(
        count(
            &fixture.storage.data().records_path(),
            "SELECT count(*) FROM commit_warnings"
        )? > 0,
        "the live records keep a row for the workspace's commit"
    );
    assert_eq!(
        count(&records, "SELECT count(*) FROM commit_warnings")?,
        0,
        "an archive carries none of them"
    );
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
        ..sign_in()
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
        .await?
        .object;
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
            backup: None,
            outputs: vec![output.clone(), history.clone()],
            warnings: Vec::new(),
        })
        .await?;
    let archive = storage
        .backups()
        .write_workspace_archive(workspace, uuid!(40)?)
        .await?
        .object;
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
        .await?
        .object;
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
        .await?
        .object;
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
        .await?
        .object;
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
        .await?
        .object;
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
                    proposal_id: None,
                    count: None,
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
        .await?
        .object;
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
                proposal_id: None,
                count: None,
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
            proposal_id: None,
            count: None,
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
    // The directory as records schema 1 left it: opening it takes a pre-migration set.
    as_records_schema_1(&directory.path().join("records.sqlite"))?;
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
        proposal_id: None,
        count: None,
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

/// Backup round-2 ruling 1 (B2): two different installations' events never collide: restoring
/// the installation archives of two installations, each with one tenant event, keeps both.
#[tokio::test]
async fn backup_two_installations_events_never_collide() -> TestResult {
    let (first, second, output_a, output_b) = (
        tempfile::tempdir()?,
        tempfile::tempdir()?,
        tempfile::tempdir()?,
        tempfile::tempdir()?,
    );
    for (data, output) in [(&first, &output_a), (&second, &output_b)] {
        let storage = Storage::open(data.path())?;
        storage.credentials().installation_identity().await?;
        storage
            .events()
            .append(&EventScope::Tenant(tenant("local")?), None, sign_in())
            .await?;
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
    }
    let directory = tempfile::tempdir()?;
    let restored = Storage::open(directory.path())?;
    for output in [&output_a, &output_b] {
        restored
            .backups()
            .restore_replacement(&output.path().join("installation").join("local.zip"), &[])
            .await?;
    }
    assert_eq!(
        count(
            &directory.path().join("records.sqlite"),
            "SELECT count(*) FROM events WHERE workspace_id IS NULL"
        )?,
        2,
        "each installation's event is kept"
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
    // A workspace with notes only: its archive holds no object, so no object put (which the
    // restored purge record refuses too) stands in for the refusal under test.
    let workspace = {
        let storage = Storage::open(directory.path())?;
        let (workspace, head) = create(&storage, "local", 1, "Notes").await?;
        commit(
            &storage,
            &workspace,
            3,
            &head,
            vec![note(uuid!(10)?, "notes/plan.md", "The plan.")?],
        )
        .await?;
        storage
            .records()
            .create_purge(
                &tenant("local")?,
                uuid!(60)?,
                NewPurge {
                    id: uuid!(61)?,
                    target: PurgeTarget::Workspace {
                        workspace_id: workspace.workspace_id,
                    },
                    initiator: person("ana"),
                },
            )
            .await?;
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        workspace
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
        .await?
        .object;
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
    let found = some(
        storage
            .versions()
            .find_commit(&target, uuid!(84)?, None)
            .await?,
        "the restore's commit",
    )?;
    assert_eq!(
        found.revision, head,
        "the restore's revision is found by its mutation"
    );
    // R-I10: no check ran for the restore's commit, and the archive brought no kept warnings.
    // So the restore's commit, and a commit of the archived history (the fixture's, mutation
    // 3), answer the one warning that says so, where the source workspace still answers what
    // it kept: no warning.
    assert_eq!(codes(&found.warnings), [REPLAY_WARNINGS_UNAVAILABLE]);
    let versions = storage.versions();
    let archived = versions.find_commit(&target, uuid!(3)?, None).await?;
    let archived = some(archived, "the archived commit, restored")?;
    assert_eq!(codes(&archived.warnings), [REPLAY_WARNINGS_UNAVAILABLE]);
    let source = versions
        .find_commit(&fixture.workspace, uuid!(3)?, None)
        .await?;
    let source = some(source, "the commit where it was written")?;
    assert_eq!(codes(&source.warnings), Vec::<&str>::new());
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
    as_records_schema_1(&directory.path().join("records.sqlite"))?;
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
        4,
        "then the records are upgraded"
    );
    Ok(())
}

/// Put a records database back as records schema 1 left it: without the tables schemas 2 and 3
/// add and the job column schema 4 adds, without the ledger's workspace column, and with the
/// events table as it was before event keys.
fn as_records_schema_1(records: &Path) -> TestResult {
    let connection = rusqlite::Connection::open(records)?;
    connection.execute_batch(
        "ALTER TABLE jobs DROP COLUMN sealed; DROP TABLE commit_warnings;
         DROP TABLE purge_progress; DROP TABLE backup_archives; DROP TABLE restores;
         DROP TABLE pending_puts; DROP TABLE replacement_restores;
         DROP INDEX mutations_by_workspace; ALTER TABLE mutations DROP COLUMN workspace_id;
         CREATE TABLE unkeyed_events (
             seq INTEGER PRIMARY KEY AUTOINCREMENT,
             tenant_id TEXT NOT NULL,
             workspace_id TEXT,
             replay_key TEXT UNIQUE,
             record TEXT NOT NULL
         ) STRICT;
         INSERT INTO unkeyed_events SELECT seq, tenant_id, workspace_id, replay_key, record
             FROM events;
         DROP TABLE events;
         ALTER TABLE unkeyed_events RENAME TO events;
         CREATE INDEX events_by_log ON events (tenant_id, workspace_id, seq);
         PRAGMA user_version = 1;",
    )?;
    Ok(())
}

/// The tenant `local`'s tenant-level events in `records`: (event key, record), by sequence.
fn tenant_events(records: &Path) -> Fallible<Vec<(String, String)>> {
    let connection = rusqlite::Connection::open(records)?;
    let mut statement = connection.prepare(
        "SELECT event_key, record FROM events WHERE tenant_id = 'local' AND workspace_id IS NULL
         ORDER BY seq",
    )?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Each tenant event of `rows` is kept once: `expected` distinct records, none repeated.
fn assert_each_event_once(rows: &[(String, String)], expected: usize) {
    let distinct: std::collections::BTreeSet<&String> =
        rows.iter().map(|(_, record)| record).collect();
    assert_eq!(distinct.len(), expected, "no event is lost: {rows:?}");
    assert_eq!(rows.len(), expected, "no event is repeated: {rows:?}");
}

/// The archives of a backup set for `workspace`: the tenant's installation archive and the
/// workspace's archive.
fn set_of(output: &Path, workspace: &StorageScope) -> (std::path::PathBuf, std::path::PathBuf) {
    (
        output
            .join("installation")
            .join(format!("{}.zip", workspace.tenant_id.as_str())),
        output
            .join("workspaces")
            .join(workspace.tenant_id.as_str())
            .join(format!("{}.zip", workspace.workspace_id.0)),
    )
}

/// Save ana's draft of item 10 in `scope` again with `body`, under mutation `n`.
async fn redraft(
    storage: &Storage,
    scope: &StorageScope,
    n: u32,
    base: &Revision,
    body: &str,
) -> TestResult {
    storage
        .drafts()
        .save(
            scope,
            uuid!(n)?,
            DraftWrite {
                item_id: uuid!(10)?,
                editor: "ana".to_owned(),
                base_revision: base.clone(),
                body: body.to_owned(),
                properties: BTreeMap::new(),
                content_digest: Digest::try_from("f".repeat(64))?,
            },
        )
        .await?;
    Ok(())
}

/// Ana's draft bodies in the records database of `data`.
fn draft_bodies(data: &Path) -> Fallible<Vec<String>> {
    let connection = rusqlite::Connection::open(data.join("records.sqlite"))?;
    let mut statement = connection.prepare("SELECT body FROM drafts WHERE editor = 'ana'")?;
    let bodies = statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(bodies)
}

/// Backup round-3 B3 (the reviewer's probe
/// `rr3_a_set_of_a_restored_installation_after_the_original_set_keeps_every_event`): a replacement
/// restore renumbers the tenant events it brings back (another tenant's event took a sequence of
/// the shared log in the source), so a key from the sequence would name different events on the
/// two sides. Each event keeps its own key: the original set and a set of the restored
/// installation, restored into one directory, keep each of the three events once.
#[tokio::test]
async fn backup_a_set_of_a_restored_installation_after_the_original_set_keeps_every_event()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let (earlier, later) = (tempfile::tempdir()?, tempfile::tempdir()?);
    let pause = || tokio::time::sleep(std::time::Duration::from_millis(30));
    {
        let storage = Storage::open(directory.path())?;
        storage.credentials().installation_identity().await?;
        let events = storage.events();
        events
            .append(&EventScope::Tenant(tenant("local")?), None, sign_in())
            .await?;
        pause().await;
        // Another tenant's event takes the next sequence of the shared log.
        events
            .append(&EventScope::Tenant(tenant("other")?), None, sign_in())
            .await?;
        pause().await;
        events
            .append(&EventScope::Tenant(tenant("local")?), None, sign_in())
            .await?;
        storage
            .backups()
            .write_backup_set(earlier.path(), None)
            .await?;
    }
    std::fs::remove_dir_all(directory.path())?;
    {
        let restored = Storage::open(directory.path())?;
        restored
            .backups()
            .restore_replacement(&earlier.path().join("installation").join("local.zip"), &[])
            .await?;
        pause().await;
        restored
            .events()
            .append(&EventScope::Tenant(tenant("local")?), None, sign_in())
            .await?;
        restored
            .backups()
            .write_backup_set(later.path(), None)
            .await?;
    }
    let target = tempfile::tempdir()?;
    let fresh = Storage::open(target.path())?;
    for set in [earlier.path(), later.path()] {
        fresh
            .backups()
            .restore_replacement(&set.join("installation").join("local.zip"), &[])
            .await?;
    }
    assert_each_event_once(&tenant_events(&target.path().join("records.sqlite"))?, 3);
    Ok(())
}

/// Backup round-3 B3, ruling 1: an event's key is carried verbatim, never derived from its
/// position. Two installations restored from one set each go on with an event of their own (at
/// the same position of their tenant logs); their later sets, restored into one directory, keep
/// the shared event once and both later events.
#[tokio::test]
async fn backup_two_installations_restored_from_one_set_keep_each_ones_later_events() -> TestResult
{
    let (first, second) = (tempfile::tempdir()?, tempfile::tempdir()?);
    let (original, later_first, later_second) = (
        tempfile::tempdir()?,
        tempfile::tempdir()?,
        tempfile::tempdir()?,
    );
    {
        let storage = Storage::open(first.path())?;
        storage.credentials().installation_identity().await?;
        storage
            .events()
            .append(&EventScope::Tenant(tenant("local")?), None, sign_in())
            .await?;
        storage
            .backups()
            .write_backup_set(original.path(), None)
            .await?;
    }
    Storage::open(second.path())?
        .backups()
        .restore_replacement(&original.path().join("installation").join("local.zip"), &[])
        .await?;
    for (data, output) in [(&first, &later_first), (&second, &later_second)] {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let storage = Storage::open(data.path())?;
        storage
            .events()
            .append(&EventScope::Tenant(tenant("local")?), None, sign_in())
            .await?;
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
    }
    let target = tempfile::tempdir()?;
    let fresh = Storage::open(target.path())?;
    for output in [&later_first, &later_second] {
        fresh
            .backups()
            .restore_replacement(&output.path().join("installation").join("local.zip"), &[])
            .await?;
    }
    assert_each_event_once(&tenant_events(&target.path().join("records.sqlite"))?, 3);
    Ok(())
}

/// Backup round-3 B3: the events of a records database older than event keys get theirs from
/// the migration, as an archive without keys keys them. A backup set written before the upgrade
/// (without keys) restored over the upgraded directory repeats no event, for either tenant.
#[tokio::test]
async fn backup_a_pre_migration_set_restored_over_the_upgraded_directory_repeats_no_event()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let records = directory.path().join("records.sqlite");
    {
        let storage = Storage::open(directory.path())?;
        storage.credentials().installation_identity().await?;
        for name in ["local", "other", "local"] {
            storage
                .events()
                .append(&EventScope::Tenant(tenant(name)?), None, sign_in())
                .await?;
        }
    }
    as_records_schema_1(&records)?;
    let storage = Storage::open(directory.path())?;
    let sets: Vec<_> = std::fs::read_dir(storage.data().pre_migration_path())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    let set = some(sets.first(), "the pre-migration set")?.clone();
    let query = "SELECT count(DISTINCT event_key) FROM events";
    assert_eq!(count(&records, query)?, 3, "every event has its own key");
    for name in ["local", "other"] {
        storage
            .backups()
            .restore_replacement(&set.join("installation").join(format!("{name}.zip")), &[])
            .await?;
    }
    assert_eq!(
        count(&records, "SELECT count(*) FROM events")?,
        3,
        "no event is repeated"
    );
    Ok(())
}

/// Backup round-3 B4 (the reviewer's probe
/// `rr3_a_rerun_refuses_a_restored_workspace_a_purge_now_names`): a workspace a replacement
/// restore completed, of which a purge is then recorded (requested, not run): the same restore
/// run again is refused with `Conflict` naming the purge, not accepted as already restored.
#[tokio::test]
async fn backup_a_rerun_refuses_a_restored_workspace_a_purge_now_names() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let workspace = {
        let storage = Storage::open(directory.path())?;
        let (workspace, head) = create(&storage, "local", 1, "Notes").await?;
        commit(
            &storage,
            &workspace,
            3,
            &head,
            vec![note(uuid!(10)?, "notes/plan.md", "The plan.")?],
        )
        .await?;
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        workspace
    };
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let (installation, archive) = set_of(output.path(), &workspace);
    let archives = vec![archive];
    restored
        .backups()
        .restore_replacement(&installation, &archives)
        .await?;
    restored
        .records()
        .create_purge(
            &tenant("local")?,
            uuid!(60)?,
            NewPurge {
                id: uuid!(61)?,
                target: PurgeTarget::Workspace {
                    workspace_id: workspace.workspace_id,
                },
                initiator: person("ana"),
            },
        )
        .await?;
    let error = err_of(
        restored
            .backups()
            .restore_replacement(&installation, &archives)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert!(
        error.message.contains(&uuid_text(61)),
        "names the purge: {error}"
    );
    Ok(())
}

/// Backup round-3 B5 (the reviewer's probe
/// `rr3_a_live_workspace_with_other_drafts_is_not_accepted_as_restored`): a live workspace whose
/// head is still the archive's but whose draft changed after the set was never restored from
/// that archive, so a replacement restore over it is refused as existing; the live draft is
/// kept, and the tenant event, already here under its own key, is not repeated.
#[tokio::test]
async fn backup_a_live_workspace_with_other_drafts_is_not_accepted_as_restored() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    storage.credentials().installation_identity().await?;
    let (workspace, created) = create(&storage, "local", 1, "Notes").await?;
    let head = commit(
        &storage,
        &workspace,
        3,
        &created,
        vec![note(uuid!(10)?, "notes/plan.md", "The plan.")?],
    )
    .await?;
    draft(&storage, &workspace, 6, "ana", &head).await?;
    storage
        .events()
        .append(&EventScope::Tenant(tenant("local")?), None, sign_in())
        .await?;
    storage
        .backups()
        .write_backup_set(output.path(), None)
        .await?;
    redraft(&storage, &workspace, 7, &head, "Changed after the set").await?;
    let records = directory.path().join("records.sqlite");
    let tenant_level = "SELECT count(*) FROM events WHERE workspace_id IS NULL";
    let before = count(&records, tenant_level)?;
    let (installation, archive) = set_of(output.path(), &workspace);
    let error = err_of(
        storage
            .backups()
            .restore_replacement(&installation, &[archive])
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert!(error.message.contains("already exists"), "{error}");
    assert_eq!(
        draft_bodies(directory.path())?,
        vec!["Changed after the set".to_owned()],
        "the live draft is kept"
    );
    assert_eq!(
        count(&records, tenant_level)?,
        before,
        "the tenant event is not repeated"
    );
    Ok(())
}

/// Backup round-3 B5, ruling 3: a workspace a replacement restore restored from one archive is
/// not taken as restored by another archive of it (a later set, whose draft differs, with the
/// same head): it is refused as existing and keeps the first archive's draft.
#[tokio::test]
async fn backup_a_workspace_restored_from_another_archive_is_not_accepted_as_restored() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let (earlier, later) = (tempfile::tempdir()?, tempfile::tempdir()?);
    let workspace = {
        let storage = Storage::open(directory.path())?;
        let (workspace, created) = create(&storage, "local", 1, "Notes").await?;
        let head = commit(
            &storage,
            &workspace,
            3,
            &created,
            vec![note(uuid!(10)?, "notes/plan.md", "The plan.")?],
        )
        .await?;
        draft(&storage, &workspace, 6, "ana", &head).await?;
        storage
            .backups()
            .write_backup_set(earlier.path(), None)
            .await?;
        redraft(&storage, &workspace, 7, &head, "Changed after the set").await?;
        storage
            .backups()
            .write_backup_set(later.path(), None)
            .await?;
        workspace
    };
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let (installation, archive) = set_of(earlier.path(), &workspace);
    restored
        .backups()
        .restore_replacement(&installation, &[archive])
        .await?;
    let (installation, archive) = set_of(later.path(), &workspace);
    let error = err_of(
        restored
            .backups()
            .restore_replacement(&installation, &[archive])
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert!(error.message.contains("already exists"), "{error}");
    assert_eq!(
        draft_bodies(directory.path())?,
        vec!["Draft by ana".to_owned()],
        "the first archive's draft is kept"
    );
    Ok(())
}

/// Backup round-3 `RMp`: the purge refusal of a replacement restore over a live directory leaves
/// the live workspace as it was: its moved history, its record and its draft.
#[tokio::test]
async fn backup_the_purge_refusal_leaves_a_live_workspace_intact() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (workspace, created) = create(&storage, "local", 1, "Notes").await?;
    let head = commit(
        &storage,
        &workspace,
        3,
        &created,
        vec![note(uuid!(10)?, "notes/plan.md", "The plan.")?],
    )
    .await?;
    draft(&storage, &workspace, 6, "ana", &head).await?;
    storage
        .backups()
        .write_backup_set(output.path(), None)
        .await?;
    let moved = commit(
        &storage,
        &workspace,
        4,
        &head,
        vec![note(uuid!(15)?, "notes/later.md", "Later.")?],
    )
    .await?;
    storage
        .records()
        .create_purge(
            &tenant("local")?,
            uuid!(60)?,
            NewPurge {
                id: uuid!(61)?,
                target: PurgeTarget::Workspace {
                    workspace_id: workspace.workspace_id,
                },
                initiator: person("ana"),
            },
        )
        .await?;
    let (installation, archive) = set_of(output.path(), &workspace);
    let error = err_of(
        storage
            .backups()
            .restore_replacement(&installation, &[archive])
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert!(
        error.message.contains(&uuid_text(61)),
        "names the purge: {error}"
    );
    assert_eq!(
        repository_of(&storage, &workspace)?
            .refname_to_id("refs/heads/main")?
            .to_string(),
        moved.as_str(),
        "the live workspace's history is kept"
    );
    let records = directory.path().join("records.sqlite");
    assert_eq!(
        count(
            &records,
            &format!(
                "SELECT count(*) FROM workspaces WHERE workspace_id = '{}'",
                workspace.workspace_id.0
            )
        )?,
        1,
        "its record is kept"
    );
    assert_eq!(
        draft_bodies(directory.path())?,
        vec!["Draft by ana".to_owned()],
        "its draft is kept"
    );
    Ok(())
}

/// Backup round-3 RT: a replacement restore refuses a workspace archive of another tenant than
/// the installation archive's, before anything of it is written.
#[tokio::test]
async fn backup_a_replacement_restore_refuses_another_tenants_workspace_archive() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let foreign = {
        let storage = Storage::open(directory.path())?;
        create(&storage, "local", 1, "Notes").await?;
        let (foreign, head) = create(&storage, "other", 2, "Theirs").await?;
        commit(
            &storage,
            &foreign,
            3,
            &head,
            vec![note(uuid!(10)?, "notes/plan.md", "Their plan.")?],
        )
        .await?;
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        foreign
    };
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let (_, archive) = set_of(output.path(), &foreign);
    let error = err_of(
        restored
            .backups()
            .restore_replacement(
                &output.path().join("installation").join("local.zip"),
                &[archive],
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::InvalidInput, "{error}");
    assert!(error.message.contains("another tenant"), "{error}");
    assert_eq!(
        count(
            &directory.path().join("records.sqlite"),
            "SELECT count(*) FROM workspaces"
        )?,
        0,
        "no workspace written"
    );
    assert!(
        !restored
            .data()
            .repositories_path()
            .join("local")
            .join(format!("{}.git", foreign.workspace_id.0))
            .exists(),
        "no repository written"
    );
    Ok(())
}

/// Backup round-4 re-review MMIGPART (the reviewer's probe
/// `rr4_c_migration_positions_ignore_workspace_events`): workspace events interleaved with the
/// tenant events of a schema-1 directory do not shift the positions the migration keys tenant
/// events by, so the pre-migration set restored over the upgraded directory repeats nothing.
#[tokio::test]
async fn backup_migration_positions_ignore_workspace_events() -> TestResult {
    let directory = tempfile::tempdir()?;
    let records = directory.path().join("records.sqlite");
    {
        let storage = Storage::open(directory.path())?;
        storage.credentials().installation_identity().await?;
        let (workspace, _) = create(&storage, "local", 1, "Notes").await?;
        let in_workspace = EventScope::Workspace(workspace.clone());
        for scope in [
            EventScope::Tenant(tenant("local")?),
            in_workspace.clone(),
            EventScope::Tenant(tenant("local")?),
            EventScope::Tenant(tenant("other")?),
            in_workspace,
            EventScope::Tenant(tenant("local")?),
        ] {
            storage.events().append(&scope, None, sign_in()).await?;
        }
    }
    as_records_schema_1(&records)?;
    let storage = Storage::open(directory.path())?;
    let sets: Vec<_> = std::fs::read_dir(storage.data().pre_migration_path())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    let set = some(sets.first(), "the pre-migration set")?.clone();
    let tenant_level = "SELECT count(*) FROM events WHERE workspace_id IS NULL";
    assert_eq!(count(&records, tenant_level)?, 4);
    for name in ["local", "other"] {
        storage
            .backups()
            .restore_replacement(&set.join("installation").join(format!("{name}.zip")), &[])
            .await?;
    }
    assert_eq!(
        count(&records, tenant_level)?,
        4,
        "no tenant event is repeated: {:?}",
        tenant_events(&records)?
    );
    Ok(())
}

/// Backup round-4 re-review MORD: a replacement restore brings the tenant log back in its
/// original order (the archive holds it oldest first).
#[tokio::test]
async fn backup_restored_tenant_events_keep_their_order() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let original = {
        let storage = Storage::open(directory.path())?;
        storage.credentials().installation_identity().await?;
        for _ in 0..3 {
            // Each event's instant (in milliseconds) is its own, so the records differ.
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            storage
                .events()
                .append(&EventScope::Tenant(tenant("local")?), None, sign_in())
                .await?;
        }
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        tenant_events(&directory.path().join("records.sqlite"))?
    };
    let distinct: std::collections::BTreeSet<&String> =
        original.iter().map(|(_, record)| record).collect();
    assert_eq!(distinct.len(), 3, "three distinct events: {original:?}");
    let target = tempfile::tempdir()?;
    Storage::open(target.path())?
        .backups()
        .restore_replacement(&output.path().join("installation").join("local.zip"), &[])
        .await?;
    assert_eq!(
        tenant_events(&target.path().join("records.sqlite"))?,
        original,
        "the restored log is the original, in its order"
    );
    Ok(())
}

/// PR #20 Codex P2 (`snapshot.rs:143`): a workspace archive leaves out the workspace's own
/// backup artifacts, and the finished backup job it keeps loses that artifact's download
/// descriptor with it. After a replacement restore the job is finished with no artifact, not
/// one that advertises a download answering not found.
#[tokio::test]
async fn backup_a_restored_backup_job_advertises_no_download_it_lost() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let (workspace, job) = {
        let storage = Storage::open(directory.path())?;
        let (workspace, _) = create(&storage, "local", 1, "Notes").await?;
        let scope = JobScope::Workspace(workspace.clone());
        let records = storage.records();
        let job = records
            .create_job(
                &scope,
                NewJob {
                    mutation_id: uuid!(30)?,
                    initiator: person("ana"),
                    spec: JobSpec::BackupWorkspace,
                },
            )
            .await?;
        let claimed = some(records.claim_job(job.id).await?, "the claimed job")?;
        let archive = storage
            .backups()
            .write_workspace_archive(&workspace, uuid!(31)?)
            .await?
            .object;
        let artifact = records
            .record_artifact(
                &scope,
                uuid!(32)?,
                NewArtifact {
                    kind: ArtifactKind::WorkspaceBackup,
                    object: archive,
                    media_type: "application/zip".to_owned(),
                    created_by_job: job.id,
                },
            )
            .await?;
        let completed = records
            .complete_job(JobCompletion {
                lease: claimed.lease,
                revision: None,
                item_ids: Vec::new(),
                artifact: Some(artifact.id),
                restore: None,
                backup: None,
                outputs: Vec::new(),
                warnings: Vec::new(),
            })
            .await?;
        assert!(
            completed.artifact.is_some(),
            "the live job names its backup"
        );
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        (workspace, job.id)
    };
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let (installation, archive) = set_of(output.path(), &workspace);
    restored
        .backups()
        .restore_replacement(&installation, &[archive])
        .await?;
    let restored_job = restored
        .records()
        .get_job(&JobScope::Workspace(workspace.clone()), job)
        .await?;
    assert_eq!(
        restored_job.state,
        okf_jawn_contract::import::JobState::Succeeded
    );
    assert!(
        restored_job.artifact.is_none(),
        "no download the restored workspace cannot serve: {:?}",
        restored_job.artifact
    );
    Ok(())
}

/// PR #20 Codex P1 (`backup/mod.rs:164`): the search index starts empty in a replaced data
/// directory; a replacement restore indexes each restored workspace's head, so search there
/// answers at once, without a rebuild.
#[tokio::test]
async fn backup_a_replacement_restore_indexes_the_restored_head() -> TestResult {
    use okf_jawn_core::search::{SearchIndex, SearchQuery};
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let (workspace, head) = {
        let storage = Storage::open(directory.path())?;
        let (workspace, created) = create(&storage, "local", 1, "Notes").await?;
        let head = commit(
            &storage,
            &workspace,
            3,
            &created,
            vec![note(uuid!(10)?, "notes/plan.md", "The harbor plan.")?],
        )
        .await?;
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        (workspace, head)
    };
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let (installation, archive) = set_of(output.path(), &workspace);
    restored
        .backups()
        .restore_replacement(&installation, &[archive])
        .await?;
    let found = restored
        .search()
        .search(
            &workspace,
            SearchQuery {
                revision: head,
                text: "harbor".to_owned(),
                folder: None,
                include_archived: false,
                extraction: None,
                page: okf_jawn_core::storage::Page {
                    cursor: None,
                    limit: 20,
                },
            },
        )
        .await?;
    assert_eq!(found.hits.len(), 1, "the restored note is found");
    Ok(())
}

/// The file of the object `digest` of tenant `local` in the data directory `data`.
fn object_file(data: &Path, digest: &Digest) -> Fallible<std::path::PathBuf> {
    Ok(data
        .join("objects")
        .join("local")
        .join(some(digest.as_str().get(..2), "a digest")?)
        .join(digest.as_str()))
}

/// PR #20 Codex P1 (`archive.rs:169`): an archive must be independently recoverable (SPEC
/// section 12), so a workspace archive whose rows name an object the store no longer holds
/// fails, naming the digest and the row, and records nothing: here a source card's original,
/// then an export artifact.
#[tokio::test]
async fn backup_an_archive_missing_an_object_a_row_names_fails_and_records_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let records = directory.path().join("records.sqlite");
    for (n, digest, row) in [
        (70_u32, &fixture.original, "a source card's original"),
        (71, &fixture.export, "an artifact of kind export"),
    ] {
        let file = object_file(directory.path(), digest)?;
        let bytes = std::fs::read(&file)?;
        std::fs::remove_file(&file)?;
        let error = err_of(
            fixture
                .storage
                .backups()
                .write_workspace_archive(&fixture.workspace, uuid!(n)?)
                .await,
        )?;
        assert_eq!(error.code, ErrorCode::Internal, "{error}");
        assert!(
            error.message.contains(digest.as_str()) && error.message.contains(row),
            "names the digest and the row: {error}"
        );
        assert_eq!(
            count(&records, "SELECT count(*) FROM backup_archives")?,
            0,
            "no archive is recorded"
        );
        std::fs::write(&file, bytes)?;
    }
    Ok(())
}

/// The bytes of a workspace archive of `source`, written under mutation 40.
async fn archive_bytes(storage: &Storage, source: &StorageScope) -> Fallible<Vec<u8>> {
    let archive = storage
        .backups()
        .write_workspace_archive(source, uuid!(40)?)
        .await?
        .object;
    Ok(std::fs::read(object_file(
        storage.data().root(),
        &archive.digest,
    )?)?)
}

/// The archive `bytes` rebuilt entry by entry: each entry compressed with `method(name)` and
/// holding `replace(name)` when that gives bytes, then the `extra` entries, stored.
fn rebuilt(
    bytes: &[u8],
    method: &dyn Fn(&str) -> zip::CompressionMethod,
    replace: &dyn Fn(&str) -> Option<Vec<u8>>,
    extra: &[(&str, &[u8])],
) -> Fallible<Vec<u8>> {
    use std::io::Write as _;
    let mut source = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec()))?;
    let mut target = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for index in 0..source.len() {
        let mut entry = source.by_index(index)?;
        let name = entry.name().to_owned();
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        let content = replace(&name).unwrap_or(content);
        target.start_file(
            name.as_str(),
            zip::write::SimpleFileOptions::default().compression_method(method(&name)),
        )?;
        target.write_all(&content)?;
    }
    for (name, content) in extra {
        target.start_file(
            *name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )?;
        target.write_all(content)?;
    }
    Ok(target.finish()?.into_inner())
}

/// Set the decompressed size the entry `name` of the archive `bytes` declares, in its local
/// header and its central directory record, to `size`.
fn declare_size(bytes: &mut [u8], name: &str, size: u32) -> TestResult {
    // (signature, offset of the name length, offset of the name, offset of the size)
    let headers: [([u8; 4], usize, usize, usize); 2] = [
        ([0x50, 0x4b, 0x03, 0x04], 26, 30, 22),
        ([0x50, 0x4b, 0x01, 0x02], 28, 46, 24),
    ];
    let mut patched = 0_u32;
    for (signature, length_at, name_at, size_at) in headers {
        for position in 0..bytes.len() {
            if bytes.get(position..position.saturating_add(4)) != Some(&signature[..]) {
                continue;
            }
            let at = |offset: usize| position.saturating_add(offset);
            let length = some(
                bytes.get(at(length_at)..at(length_at.saturating_add(2))),
                "a length",
            )?;
            let length = usize::from(u16::from_le_bytes(length.try_into()?));
            if bytes.get(at(name_at)..at(name_at.saturating_add(length))) != Some(name.as_bytes()) {
                continue;
            }
            some(
                bytes.get_mut(at(size_at)..at(size_at.saturating_add(4))),
                "the size field",
            )?
            .copy_from_slice(&size.to_le_bytes());
            patched = patched.saturating_add(1);
        }
    }
    assert_eq!(patched, 2, "the local header and the central record");
    Ok(())
}

/// `length` bytes that do not compress (a xorshift stream).
fn incompressible(length: usize) -> Vec<u8> {
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes().first().copied().unwrap_or_default()
        })
        .collect()
}

/// Import the archive `bytes` into a blank workspace of another tenant, as an upload of it
/// would; returns the target and the result.
async fn import_crafted(
    storage: &Storage,
    bytes: Vec<u8>,
) -> Fallible<(
    StorageScope,
    Revision,
    Result<okf_jawn_contract::workspace::RestoreReport, ApiError>,
)> {
    let (target, blank) = create(storage, "elsewhere", 80, "Copy").await?;
    let uploaded = storage
        .blobs()
        .put(
            &target,
            Box::pin(std::io::Cursor::new(bytes)),
            1 << 30,
            None,
        )
        .await?
        .digest;
    let result = storage
        .backups()
        .restore_import(&target, uploaded, uuid!(84)?, vec!["ana".to_owned()])
        .await;
    Ok((target, blank, result))
}

/// A notes-only workspace of tenant `local`, with ana's draft.
async fn notes_workspace(storage: &Storage) -> Fallible<StorageScope> {
    let (workspace, created) = create(storage, "local", 1, "Notes").await?;
    let head = commit(
        storage,
        &workspace,
        3,
        &created,
        vec![note(uuid!(10)?, "notes/plan.md", "The plan.")?],
    )
    .await?;
    draft(storage, &workspace, 6, "ana", &head).await?;
    Ok(workspace)
}

/// Assert that an import was refused as `InvalidInput` mentioning `what`, and wrote nothing:
/// the target's head is still blank and no import is recorded.
async fn assert_import_refused(
    storage: &Storage,
    (target, blank, result): (
        StorageScope,
        Revision,
        Result<okf_jawn_contract::workspace::RestoreReport, ApiError>,
    ),
    what: &str,
) -> TestResult {
    let error = err_of(result)?;
    assert_eq!(error.code, ErrorCode::InvalidInput, "{error}");
    assert!(error.message.contains(what), "{error}");
    assert_eq!(
        storage.versions().head(&target).await?,
        blank,
        "no history written"
    );
    assert_eq!(
        count(
            &storage.data().root().join("records.sqlite"),
            "SELECT count(*) FROM restores"
        )?,
        0,
        "no import recorded"
    );
    Ok(())
}

/// Point `reference` of `scope`'s repository at a new commit on its target whose tree also holds
/// the file `written`, straight through Git, as no store edit can (an earlier build, a
/// case-sensitive filesystem or a crafted archive can leave such a tree); answers the commit.
fn respelled_at(
    storage: &Storage,
    scope: &StorageScope,
    reference: &str,
    written: &str,
) -> Fallible<git2::Oid> {
    let repository = repository_of(storage, scope)?;
    let parent = repository.find_commit(repository.refname_to_id(reference)?)?;
    let mut update = git2::build::TreeUpdateBuilder::new();
    update.upsert(
        written,
        repository.blob(b"respelled")?,
        git2::FileMode::Blob,
    );
    let tree = repository.find_tree(update.create_updated(&repository, &parent.tree()?)?)?;
    let who = git2::Signature::now("raw", "raw@example.invalid")?;
    Ok(repository.commit(
        Some(reference),
        &who,
        &who,
        &format!("raw\n\nOkf-Jawn-Mutation: {}\n", uuid_text(90)),
        &tree,
        &[&parent],
    )?)
}

/// Storage-paths re-review note (c), ruling B-R2: an import refuses, before anything is
/// written, an archive one of whose references names a tree the store would refuse to stage:
/// two spellings of one path, or a path the server writes spelled another way. The refusal
/// names both spellings and the reference: the head (`refs/heads/main`), and a proposal's
/// candidate. Each archive is written by the store's own archive writer from a workspace whose
/// reference was moved to such a tree through Git.
#[tokio::test]
async fn backup_an_import_refuses_an_archive_whose_reference_holds_a_respelling() -> TestResult {
    let main = "refs/heads/main";
    for (written, what) in [
        (
            "Notes/b.md",
            "notes collides with Notes: its refs/heads/main holds two spellings of one path, \
             which Windows and macOS keep as one",
        ),
        (
            ".OKF/x.json",
            ".OKF collides with the server's .okf: its refs/heads/main spells a path the server \
             writes another way, which Windows and macOS would merge with it",
        ),
    ] {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(directory.path())?;
        let source = notes_workspace(&storage).await?;
        respelled_at(&storage, &source, main, written)?;
        let bytes = archive_bytes(&storage, &source).await?;
        let imported = import_crafted(&storage, bytes).await?;
        assert_import_refused(
            &storage,
            imported,
            &format!("the archive cannot be restored: {what}"),
        )
        .await?;
    }
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    let candidate = some(
        other_references(&repository_of(storage, &fixture.workspace)?)?
            .first()
            .map(|(name, _)| name.clone()),
        "the archive's candidate reference",
    )?;
    respelled_at(storage, &fixture.workspace, &candidate, "Notes/x.md")?;
    let bytes = archive_bytes(storage, &fixture.workspace).await?;
    let imported = import_crafted(storage, bytes).await?;
    assert_import_refused(
        storage,
        imported,
        &format!(
            "the archive cannot be restored: notes collides with Notes: its {candidate} holds two \
             spellings of one path, which Windows and macOS keep as one"
        ),
    )
    .await
}

/// Contract ruling 33 (PR #34's Codex pair): an import refuses, before anything is written, an
/// archive whose head holds two names that differ only in Unicode normalization inside a
/// segment Windows refuses even in NFC (`café?.txt`, precomposed and with a combining accent):
/// they share a collision key (`identity::collision_key_of`), so the import's check names both,
/// escaped, and the reference.
#[tokio::test]
async fn backup_an_import_refuses_an_archive_holding_two_normalizations_of_a_name() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let main = "refs/heads/main";
    respelled_at(&storage, &source, main, "notes/caf\u{e9}?.txt")?;
    respelled_at(&storage, &source, main, "notes/cafe\u{301}?.txt")?;
    let bytes = archive_bytes(&storage, &source).await?;
    let imported = import_crafted(&storage, bytes).await?;
    assert_import_refused(
        &storage,
        imported,
        "the archive cannot be restored: notes/caf\\u{e9}?.txt collides with \
         notes/cafe\\u{301}?.txt: its refs/heads/main holds two names that differ in how their \
         characters are encoded, which macOS keeps as one",
    )
    .await
}

/// Ruling B-R2 refuses an import only: a replacement restore puts back a workspace whose head
/// holds two spellings as the installation held it, since the archive writer wrote that head
/// and a refusal would make the set unrestorable; the workspace then refuses a commit as it
/// did before the backup, naming the repair, and the repair it names (a whole-tree Rewind to a
/// revision from before the respelling) lands, after which a commit lands again.
#[tokio::test]
async fn backup_a_replacement_restore_puts_back_a_head_holding_a_respelling() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let workspace = notes_workspace(&storage).await?;
    let clean = storage.versions().head(&workspace).await?;
    let respelled = respelled_at(&storage, &workspace, "refs/heads/main", "Notes/b.md")?;
    let respelled = Revision::try_from(respelled.to_string())?;
    storage
        .backups()
        .write_backup_set(output.path(), None)
        .await?;
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
    restored
        .backups()
        .restore_replacement(&installation, &archives)
        .await?;
    assert_eq!(restored.versions().head(&workspace).await?, respelled);
    let error = err_of(
        commit(
            &restored,
            &workspace,
            92,
            &respelled,
            vec![note(uuid!(14)?, "c.md", "Fourteen.")?],
        )
        .await,
    )?;
    let error = some(error.downcast_ref::<ApiError>(), "the store's refusal")?;
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert_eq!(
        error.message,
        "notes collides with Notes: the revision holds two spellings of one path, which Windows \
         and macOS keep as one; the workspace can be restored to a revision from before they \
         appeared"
    );
    assert_eq!(restored.versions().head(&workspace).await?, respelled);
    let repaired = commit(
        &restored,
        &workspace,
        93,
        &respelled,
        vec![TreeEdit::RestoreWorkspace {
            from: clean.clone(),
        }],
    )
    .await?;
    let landed = commit(
        &restored,
        &workspace,
        94,
        &repaired,
        vec![note(uuid!(14)?, "c.md", "Fourteen.")?],
    )
    .await?;
    assert_eq!(restored.versions().head(&workspace).await?, landed);
    let repository = repository_of(&restored, &workspace)?;
    let tree = repository
        .find_commit(git2::Oid::from_str(landed.as_str())?)?
        .tree()?;
    assert!(tree.get_path(Path::new("c.md")).is_ok(), "the note landed");
    assert!(
        tree.get_path(Path::new("Notes")).is_err(),
        "the respelling is gone"
    );
    Ok(())
}

/// Ruling B-R2's other side: an archive whose respelling is only in an earlier revision, behind
/// a head repaired by a whole-tree Rewind, imports with that head. Only a Rewind to the earlier
/// revision meets it, and is refused then (`Holder::Restored`).
#[tokio::test]
async fn backup_an_import_takes_an_archive_with_a_respelling_only_in_its_history() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let clean = storage.versions().head(&source).await?;
    let respelled = respelled_at(&storage, &source, "refs/heads/main", "Notes/b.md")?;
    let repaired = commit(
        &storage,
        &source,
        91,
        &Revision::try_from(respelled.to_string())?,
        vec![TreeEdit::RestoreWorkspace {
            from: clean.clone(),
        }],
    )
    .await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let (target, _, result) = import_crafted(&storage, bytes).await?;
    result?;
    let repository = repository_of(&storage, &target)?;
    let head = repository.find_commit(repository.refname_to_id("refs/heads/main")?)?;
    let source_repository = repository_of(&storage, &source)?;
    let archived = source_repository.find_commit(git2::Oid::from_str(repaired.as_str())?)?;
    assert_eq!(
        head.tree_id(),
        archived.tree_id(),
        "the repaired head's tree"
    );
    Ok(())
}

/// PR #20 Codex P2 (`archive.rs:145`): an uploaded archive whose manifest decompresses past the
/// 64 MiB cap (a small deflated entry; the archive carries 10 MiB of incompressible padding, so
/// the cap and not the expansion bound refuses it) is refused before anything is written. The
/// manifest is a real one, padded with whitespace, so without the cap the import would succeed.
#[tokio::test]
async fn backup_an_archive_whose_manifest_expands_past_its_cap_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let padding = incompressible(10 << 20);
    let crafted = rebuilt(
        &bytes,
        &|name| {
            if name == "manifest.json" {
                zip::CompressionMethod::Deflated
            } else {
                zip::CompressionMethod::Stored
            }
        },
        &|name| {
            (name == "manifest.json").then(|| {
                let mut manifest = entry_bytes_of(&bytes, "manifest.json").unwrap_or_default();
                manifest.resize(65 << 20, b' ');
                manifest
            })
        },
        &[("padding", &padding)],
    )?;
    let outcome = import_crafted(&storage, crafted).await?;
    assert_import_refused(&storage, outcome, "manifest.json").await
}

/// The bytes of the entry `name` of the archive `bytes`.
fn entry_bytes_of(bytes: &[u8], name: &str) -> Fallible<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec()))?;
    let mut content = Vec::new();
    archive.by_name(name)?.read_to_end(&mut content)?;
    Ok(content)
}

/// PR #20 Codex P2 (`archive.rs:145`): an entry whose decompressed size is more than it
/// declares (a deflated `records.sqlite` declaring 1 KiB) is refused before anything is
/// written.
#[tokio::test]
async fn backup_an_archive_entry_that_lies_about_its_size_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let mut crafted = rebuilt(
        &bytes,
        &|name| {
            if name == "records.sqlite" {
                zip::CompressionMethod::Deflated
            } else {
                zip::CompressionMethod::Stored
            }
        },
        &|_| None,
        &[],
    )?;
    declare_size(&mut crafted, "records.sqlite", 1024)?;
    let outcome = import_crafted(&storage, crafted).await?;
    assert_import_refused(&storage, outcome, "records.sqlite").await
}

/// PR #20 Codex P2 (`archive.rs:145`): an entry compressed with a method this build does not
/// read (bzip2; only stored and deflated are) is refused before anything is written.
#[tokio::test]
async fn backup_an_archive_entry_of_an_unknown_compression_method_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let crafted = rebuilt(
        &bytes,
        &|name| {
            if name == "records.sqlite" {
                zip::CompressionMethod::Bzip2
            } else {
                zip::CompressionMethod::Stored
            }
        },
        &|_| None,
        &[],
    )?;
    let outcome = import_crafted(&storage, crafted).await?;
    assert_import_refused(&storage, outcome, "compression method").await
}

/// PR #20 Codex P2 (`restore.rs:680`): a replacement restore refuses, before anything is
/// written, an archive whose records hold a row of another workspace than its manifest (a
/// crafted copy of ana's draft under another workspace's identity).
#[tokio::test]
async fn backup_a_replacement_restore_refuses_an_archive_with_a_row_of_another_workspace()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let workspace = {
        let storage = Storage::open(directory.path())?;
        let workspace = notes_workspace(&storage).await?;
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        workspace
    };
    let (installation, archive) = set_of(output.path(), &workspace);
    let bytes = std::fs::read(&archive)?;
    let staged = tempfile::tempdir()?;
    let records = staged.path().join("records.sqlite");
    std::fs::write(&records, entry_bytes_of(&bytes, "records.sqlite")?)?;
    rusqlite::Connection::open(&records)?.execute(
        "INSERT INTO drafts SELECT tenant_id, ?1, item_id, editor, saved_at, record, body,
             properties FROM drafts",
        [uuid_text(99)],
    )?;
    let forged = std::fs::read(&records)?;
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|name| (name == "records.sqlite").then(|| forged.clone()),
        &[],
    )?;
    std::fs::write(&archive, crafted)?;
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let error = err_of(
        restored
            .backups()
            .restore_replacement(&installation, &[archive])
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::InvalidInput, "{error}");
    assert!(error.message.contains("drafts"), "{error}");
    let live = directory.path().join("records.sqlite");
    for table in ["workspaces", "drafts"] {
        assert_eq!(
            count(&live, &format!("SELECT count(*) FROM {table}"))?,
            0,
            "no {table} row written"
        );
    }
    assert!(
        !restored
            .data()
            .repositories_path()
            .join("local")
            .join(format!("{}.git", workspace.workspace_id.0))
            .exists(),
        "no repository written"
    );
    Ok(())
}

// The PR #20 pass-4 security re-review's probes (lane-storage-m2-pass4-rereview.md), adopted.

fn rr4_converter() -> serde_json::Value {
    json!({
        "name": "docling",
        "version": "2.3.0",
        "packages": [],
        "settings": {
            "ocr": "auto",
            "table_structure": true,
            "page_images": true,
            "page_image_dpi": 144
        },
        "models": [{ "stage": "layout", "file": "layout.onnx", "size": "1", "sha256": "7".repeat(64) }]
    })
}

/// A source card over `object` whose completed extraction names the conversion record `record`.
fn rr4_converted_card(
    item: ItemId,
    at: &str,
    object: &Digest,
    record: &Digest,
) -> Fallible<TreeEdit> {
    Ok(TreeEdit::WriteSourceCard(Box::new(SourceCard {
        item_id: item,
        path: WorkspacePath::try_from(at.to_owned())?,
        title: "report.pdf".to_owned(),
        type_name: "Source".to_owned(),
        body: "Converted text.".to_owned(),
        properties: BTreeMap::new(),
        appearance: serde_json::from_value(json!({
            "object": object.as_str(),
            "names": [{
                "filename": "report.pdf",
                "folder": "",
                "observed_at": "2026-10-09T12:00:00.000Z",
                "supplied_by": "ana"
            }],
            "media_type": "application/pdf",
            "size": "10",
            "metadata": {},
            "extraction": {
                "outcome": { "status": "completed" },
                "converter": rr4_converter(),
                "digest": record.as_str(),
                "page_count": 1,
                "text_origin": "converter",
                "corrected": false,
                "warnings": []
            }
        }))?,
    })))
}

/// A conversion record naming `structured` and `asset`, with `locations` line locations (so a
/// large document's record is large); its markdown hash and model hash name no object.
fn rr4_record(structured: &Digest, asset: &Digest, locations: usize) -> Vec<u8> {
    use std::fmt::Write as _;
    let mut lines = String::with_capacity(locations.saturating_mul(48));
    for line in 0..locations {
        if line > 0 {
            lines.push(',');
        }
        let _ = write!(
            lines,
            "{{\"lines\":{{\"start\":{line},\"end\":{line}}},\"page_no\":1}}"
        );
    }
    format!(
        "{{\"converter\":{},\"outcome\":{{\"status\":\"completed\"}},\"page_count\":1,\
         \"structured\":[{{\"digest\":\"{}\"}}],\"markdown\":\"{}\",\"locations\":[{lines}],\
         \"outline\":[],\"tables\":[],\"assets\":[{{\"digest\":\"{}\",\"media_type\":\"image/png\",\
         \"role\":\"page_render\",\"location\":{{\"page_no\":1}}}}],\"warnings\":[]}}",
        rr4_converter(),
        structured.as_str(),
        "8".repeat(64),
        asset.as_str()
    )
    .into_bytes()
}

async fn rr4_put_large(
    storage: &Storage,
    scope: &StorageScope,
    bytes: Vec<u8>,
) -> Fallible<Digest> {
    Ok(storage
        .blobs()
        .put(scope, Box::pin(std::io::Cursor::new(bytes)), 1 << 30, None)
        .await?
        .digest)
}

/// A completed upload of `workspace` (mutation 62); returns its object.
async fn rr4_completed_upload(storage: &Storage, workspace: &StorageScope) -> Fallible<Digest> {
    let body = b"rr4 upload body".to_vec();
    let upload = put(storage, workspace, &body).await?.digest;
    let uploads = storage.uploads();
    let slot = uploads
        .create(
            workspace,
            uuid!(62)?,
            NewUpload {
                filename: "notes.txt".to_owned(),
                relative_path: String::new(),
                expected_size: 15,
                expected_sha256: None,
                supplied_by: person("ana"),
            },
        )
        .await?;
    uploads
        .put_content(
            workspace,
            slot.id,
            Box::pin(std::io::Cursor::new(body)),
            1 << 20,
        )
        .await?;
    uploads.complete(workspace, slot.id, upload.clone()).await?;
    Ok(upload)
}

/// Q1: every structurally named object kind the implementer did not test is required, and a
/// 64-hex string that names no object (markdown hash, model hash, draft content digest) is not.
#[tokio::test]
async fn rr4_missing_objects_of_every_named_kind_fail_the_archive() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let (storage, workspace) = (&fixture.storage, &fixture.workspace);
    let source = put(storage, workspace, b"rr4 converted original")
        .await?
        .digest;
    let structured = put(storage, workspace, b"{\"rr4\": \"docling export\"}")
        .await?
        .digest;
    let asset = put(storage, workspace, b"rr4 page render").await?.digest;
    let record = put(storage, workspace, &rr4_record(&structured, &asset, 3))
        .await?
        .digest;
    commit(
        storage,
        workspace,
        60,
        &fixture.head,
        vec![rr4_converted_card(
            uuid!(14)?,
            "sources/report.md",
            &source,
            &record,
        )?],
    )
    .await?;
    let dataset = put(storage, workspace, b"rr4 dataset").await?.digest;
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
    let output = put(storage, workspace, b"rr4 job output").await?.digest;
    let job_scope = JobScope::Workspace(workspace.clone());
    let job = storage
        .records()
        .create_job(
            &job_scope,
            NewJob {
                mutation_id: uuid!(61)?,
                initiator: person("ana"),
                spec: JobSpec::BackupWorkspace,
            },
        )
        .await?;
    let claimed = some(storage.records().claim_job(job.id).await?, "the claim")?;
    storage
        .records()
        .complete_job(JobCompletion {
            lease: claimed.lease,
            revision: None,
            item_ids: Vec::new(),
            artifact: None,
            restore: None,
            backup: None,
            outputs: vec![output.clone()],
            warnings: Vec::new(),
        })
        .await?;
    let upload = rr4_completed_upload(storage, workspace).await?;
    // Baseline: with everything held (and unheld non-object hashes named), the archive is written.
    storage
        .backups()
        .write_workspace_archive(workspace, uuid!(63)?)
        .await?;
    for (n, digest, row) in [
        (64_u32, &structured, "structured export"),
        (65, &asset, "retained asset"),
        (66, &record, "conversion record"),
        (67, &source, "original"),
        (68, &dataset, "derived dataset"),
        (69, &output, "job output"),
        (70, &upload, "completed upload"),
    ] {
        let file = object_file(directory.path(), digest)?;
        let bytes = std::fs::read(&file)?;
        std::fs::remove_file(&file)?;
        let result = storage
            .backups()
            .write_workspace_archive(workspace, uuid!(n)?)
            .await;
        std::fs::write(&file, bytes)?;
        let error = err_of(result)?;
        assert_eq!(error.code, ErrorCode::Internal, "{row}: {error}");
        assert!(
            error.message.contains(digest.as_str()) && error.message.contains(row),
            "{row}: {error}"
        );
    }
    Ok(())
}

/// Q1: a conversion record larger than 64 MiB (a large document's line locations) still names
/// its structured export and retained asset: the archive must hold them, or fail.
#[tokio::test]
async fn rr4_a_large_conversion_record_keeps_what_it_names() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (workspace, created) = create(&storage, "local", 1, "Large").await?;
    let source = put(&storage, &workspace, b"rr4 large original")
        .await?
        .digest;
    let structured = put(&storage, &workspace, b"{\"rr4\": \"large export\"}")
        .await?
        .digest;
    let asset = put(&storage, &workspace, b"rr4 large render").await?.digest;
    let bytes = rr4_record(&structured, &asset, 1_500_000);
    let size = bytes.len();
    assert!(size > 64 << 20, "the record is {size} bytes");
    let record = rr4_put_large(&storage, &workspace, bytes).await?;
    commit(
        &storage,
        &workspace,
        3,
        &created,
        vec![rr4_converted_card(
            uuid!(14)?,
            "sources/large.md",
            &source,
            &record,
        )?],
    )
    .await?;
    match storage
        .backups()
        .write_workspace_archive(&workspace, uuid!(40)?)
        .await
    {
        Err(error) => {
            // This build copies everything a record names: the archive is written, not refused.
            return Err(format!("the archive was refused: {error}").into());
        }
        Ok(archive) => {
            let names = entries(&object_file(directory.path(), &archive.object.digest)?)?;
            assert!(
                names.contains(&format!("objects/{}", record.as_str())),
                "the record itself is held"
            );
            for (digest, what) in [(&structured, "structured export"), (&asset, "asset")] {
                assert!(
                    names.contains(&format!("objects/{}", digest.as_str())),
                    "the archive was written without the {what} its {size}-byte conversion \
                     record names ({}): not independently recoverable",
                    digest.as_str()
                );
            }
        }
    }
    Ok(())
}

/// Every entry of `bytes` stored, then whatever `extra` writes.
fn rr4_with(bytes: &[u8], extra: &dyn Fn(&mut ZipBuild) -> TestResult) -> Fallible<Vec<u8>> {
    use std::io::Write as _;
    let mut source = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec()))?;
    let mut target = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for index in 0..source.len() {
        let mut entry = source.by_index(index)?;
        let name = entry.name().to_owned();
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        target.start_file(
            name.as_str(),
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )?;
        target.write_all(&content)?;
    }
    extra(&mut target)?;
    Ok(target.finish()?.into_inner())
}

/// A deflated entry `name` of `kibibytes` KiB of zeros, written in 64 KiB chunks.
fn rr4_zeros(
    writer: &mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>,
    name: &str,
    kibibytes: usize,
) -> TestResult {
    use std::io::Write as _;
    writer.start_file(
        name,
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated),
    )?;
    let chunk = vec![0_u8; 64 << 10];
    for _ in 0..kibibytes / 64 {
        writer.write_all(&chunk)?;
    }
    Ok(())
}

/// Like `declare_size`, but only in the headers whose signature is in `which`; returns the count.
fn rr4_declare(bytes: &mut [u8], name: &str, size: u32, which: &[[u8; 4]]) -> Fallible<u32> {
    let headers: [([u8; 4], usize, usize, usize); 2] = [
        ([0x50, 0x4b, 0x03, 0x04], 26, 30, 22),
        ([0x50, 0x4b, 0x01, 0x02], 28, 46, 24),
    ];
    let mut patched = 0_u32;
    for (signature, length_at, name_at, size_at) in headers {
        if !which.contains(&signature) {
            continue;
        }
        for position in 0..bytes.len() {
            if bytes.get(position..position.saturating_add(4)) != Some(&signature[..]) {
                continue;
            }
            let at = |offset: usize| position.saturating_add(offset);
            let Some(length) = bytes.get(at(length_at)..at(length_at.saturating_add(2))) else {
                continue;
            };
            let length = usize::from(u16::from_le_bytes(length.try_into()?));
            if bytes.get(at(name_at)..at(name_at.saturating_add(length))) != Some(name.as_bytes()) {
                continue;
            }
            some(
                bytes.get_mut(at(size_at)..at(size_at.saturating_add(4))),
                "the size field",
            )?
            .copy_from_slice(&size.to_le_bytes());
            patched = patched.saturating_add(1);
        }
    }
    Ok(patched)
}

/// Rename every entry named `from` to `to` (the same length), in local and central headers.
fn rr4_rename(bytes: &mut [u8], from: &str, to: &str) -> Fallible<u32> {
    assert_eq!(from.len(), to.len());
    let mut renamed = 0_u32;
    for position in 0..bytes.len() {
        if bytes.get(position..position.saturating_add(from.len())) == Some(from.as_bytes()) {
            some(
                bytes.get_mut(position..position.saturating_add(to.len())),
                "the name",
            )?
            .copy_from_slice(to.as_bytes());
            renamed = renamed.saturating_add(1);
        }
    }
    Ok(renamed)
}

/// Q2: a deflate bomb in a non-manifest entry that declares its 256 MiB honestly.
#[tokio::test]
async fn rr4_zip_an_honest_bomb_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let crafted = rr4_with(&bytes, &|zip| rr4_zeros(zip, "padding", 256 << 10))?;
    assert!(
        crafted.len() < 4 << 20,
        "the bomb is {} bytes",
        crafted.len()
    );
    let outcome = import_crafted(&storage, crafted).await?;
    assert_import_refused(&storage, outcome, "padding").await
}

/// Q2: the same bomb declaring 1 KiB in both headers.
#[tokio::test]
async fn rr4_zip_a_lying_bomb_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let mut crafted = rr4_with(&bytes, &|zip| rr4_zeros(zip, "padding", 256 << 10))?;
    assert_eq!(
        rr4_declare(&mut crafted, "padding", 1024, &[LOCAL, CENTRAL])?,
        2
    );
    let outcome = import_crafted(&storage, crafted).await?;
    assert_import_refused(&storage, outcome, "holds a different amount").await
}

/// Q2: many small deflated entries, each honest, summing past the total bound.
#[tokio::test]
async fn rr4_zip_many_small_entries_past_the_total_are_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let crafted = rr4_with(&bytes, &|zip| {
        for index in 0..2000 {
            rr4_zeros(zip, &format!("pad/{index}"), 64)?;
        }
        Ok(())
    })?;
    eprintln!("rr4 many small entries: {} bytes", crafted.len());
    let outcome = import_crafted(&storage, crafted).await?;
    assert_import_refused(&storage, outcome, "in all").await
}

/// Q2: a stored entry that declares less, or more, than it holds.
#[tokio::test]
async fn rr4_zip_a_stored_entry_that_lies_is_refused() -> TestResult {
    for delta in [-1_i64, 1] {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(directory.path())?;
        let source = notes_workspace(&storage).await?;
        let bytes = archive_bytes(&storage, &source).await?;
        let real = i64::try_from(entry_bytes_of(&bytes, "records.sqlite")?.len())?;
        let mut crafted = rr4_with(&bytes, &|_| Ok(()))?;
        let lie = u32::try_from(real.saturating_add(delta.saturating_mul(4096)))?;
        assert_eq!(
            rr4_declare(&mut crafted, "records.sqlite", lie, &[LOCAL, CENTRAL])?,
            2
        );
        let outcome = import_crafted(&storage, crafted).await?;
        assert_import_refused(&storage, outcome, "records.sqlite").await?;
    }
    Ok(())
}

/// Q2: the central directory lies and the local header does not (and the reverse).
#[tokio::test]
async fn rr4_zip_a_central_directory_lie_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let deflated = rebuilt(
        &bytes,
        &|name| {
            if name == "records.sqlite" {
                zip::CompressionMethod::Deflated
            } else {
                zip::CompressionMethod::Stored
            }
        },
        &|_| None,
        &[],
    )?;
    let mut central = deflated.clone();
    assert_eq!(
        rr4_declare(&mut central, "records.sqlite", 1024, &[CENTRAL])?,
        1
    );
    let outcome = import_crafted(&storage, central).await?;
    assert_import_refused(&storage, outcome, "records.sqlite").await?;
    // The local header alone lying: zip reads sizes from the central directory, so the archive is
    // read by what it really holds (recorded, not asserted either way).
    let mut local = deflated;
    assert_eq!(
        rr4_declare(&mut local, "records.sqlite", 1024, &[LOCAL])?,
        1
    );
    let (_, _, result) = import_crafted(&storage, local).await?;
    eprintln!(
        "rr4 local-header-only lie: {:?}",
        result.map(|_| "imported")
    );
    Ok(())
}

/// Q2: entries whose names climb out of any directory are never written anywhere.
#[tokio::test]
async fn rr4_zip_climbing_entry_names_write_nothing() -> TestResult {
    use std::io::Write as _;
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let crafted = rr4_with(&bytes, &|zip| {
        for name in [
            "../../rr4-escape-a",
            "objects/../../../rr4-escape-b",
            "/rr4-escape-c",
        ] {
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )?;
            zip.write_all(b"escaped")?;
        }
        Ok(())
    })?;
    let (target, _, result) = import_crafted(&storage, crafted).await?;
    eprintln!(
        "rr4 climbing names: {:?}",
        result.as_ref().map(|_| "imported")
    );
    let mut stack = vec![directory.path().to_path_buf()];
    if let Some(parent) = directory.path().parent() {
        for name in ["rr4-escape-a", "rr4-escape-b", "rr4-escape-c"] {
            assert!(!parent.join(name).exists(), "{name} escaped");
        }
    }
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(&path)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(
                !name.starts_with("rr4-escape"),
                "{} written",
                entry.path().display()
            );
            if entry.file_type()?.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    let _ = target;
    Ok(())
}

/// The records entry of `bytes` changed by `sql` (`WS99` stands for another workspace id).
fn rr4_forged_records(bytes: &[u8], sql: &str) -> Fallible<Vec<u8>> {
    let staged = tempfile::tempdir()?;
    let records = staged.path().join("records.sqlite");
    std::fs::write(&records, entry_bytes_of(bytes, "records.sqlite")?)?;
    {
        let connection = rusqlite::Connection::open(&records)?;
        connection.execute_batch(&format!(
            "PRAGMA foreign_keys = OFF; {}",
            sql.replace("WS99", &uuid_text(99))
        ))?;
    }
    Ok(std::fs::read(&records)?)
}

/// Q2: duplicate entry names. Two `records.sqlite` entries: whichever zip keeps (the last) is the
/// one both checked and read; the other is invisible.
#[tokio::test]
async fn rr4_zip_duplicate_entry_names_read_one_checked_entry() -> TestResult {
    use std::io::Write as _;
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let forged = rr4_forged_records(
        &bytes,
        "INSERT INTO drafts SELECT tenant_id, 'WS99', item_id, editor, saved_at, record, body,
             properties FROM drafts",
    )?;
    // The forged copy last: it is the one read, so the foreign row is refused.
    let mut last = rr4_with(&bytes, &|zip| {
        zip.start_file(
            "records.sqlitX",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )?;
        zip.write_all(&forged)?;
        Ok(())
    })?;
    assert_eq!(
        rr4_rename(&mut last, "records.sqlitX", "records.sqlite")?,
        2
    );
    assert_eq!(
        zip::ZipArchive::new(std::io::Cursor::new(last.clone()))?.len(),
        3,
        "four entries, one name twice"
    );
    let outcome = import_crafted(&storage, last).await?;
    assert_import_refused(&storage, outcome, "drafts").await?;
    Ok(())
}

/// A backup set of a notes workspace whose workspace archive's records are changed by `sql`,
/// restored by replacement over the deleted directory.
async fn rr4_replace_forged(
    sql: &str,
) -> Fallible<(tempfile::TempDir, tempfile::TempDir, StorageScope, Restored)> {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let workspace = {
        let storage = Storage::open(directory.path())?;
        let workspace = notes_workspace(&storage).await?;
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        workspace
    };
    let (installation, archive) = set_of(output.path(), &workspace);
    let bytes = std::fs::read(&archive)?;
    let forged = rr4_forged_records(&bytes, sql)?;
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|name| (name == "records.sqlite").then(|| forged.clone()),
        &[],
    )?;
    std::fs::write(&archive, crafted)?;
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let result = restored
        .backups()
        .restore_replacement(&installation, &[archive])
        .await;
    drop(restored);
    Ok((directory, output, workspace, result))
}

/// Assert a forged replacement was refused as `InvalidInput` naming `table`, with no row of the
/// other workspace written.
fn rr4_assert_refused(directory: &Path, result: Restored, table: &str) -> TestResult {
    let live = directory.join("records.sqlite");
    let foreign = count(
        &live,
        &format!(
            "SELECT count(*) FROM {table} WHERE {}",
            if table == "job_outputs" {
                "1".to_owned()
            } else {
                format!("workspace_id = '{}'", uuid_text(99))
            }
        ),
    )?;
    match result {
        Ok(reports) => Err(format!(
            "the forged archive was restored ({reports:?}); {foreign} {table} rows of the other \
             workspace are now live"
        )
        .into()),
        Err(error) => {
            assert_eq!(error.code, ErrorCode::InvalidInput, "{error}");
            assert!(error.message.contains(table), "{error}");
            assert_eq!(foreign, 0, "no foreign {table} row written");
            Ok(())
        }
    }
}

/// Q3: the control: a plain foreign draft is refused (the implementer's case).
#[tokio::test]
async fn rr4_foreign_plain_draft_is_refused_by_replacement() -> TestResult {
    let (directory, _output, _, result) = rr4_replace_forged(
        "INSERT INTO drafts SELECT tenant_id, 'WS99', item_id, editor, saved_at, record, body,
             properties FROM drafts",
    )
    .await?;
    rr4_assert_refused(directory.path(), result, "drafts")
}

/// Q3: the same foreign draft, with the archive's column spelled `WORKSPACE_ID`.
#[tokio::test]
async fn rr4_foreign_draft_under_an_upper_case_column_is_refused_by_replacement() -> TestResult {
    let (directory, _output, _, result) = rr4_replace_forged(
        "ALTER TABLE drafts RENAME COLUMN workspace_id TO WORKSPACE_ID;
         INSERT INTO drafts SELECT tenant_id, 'WS99', item_id, editor, saved_at, record, body,
             properties FROM drafts",
    )
    .await?;
    rr4_assert_refused(directory.path(), result, "drafts")
}

/// Q3: the foreign draft served by a view named `drafts`.
#[tokio::test]
async fn rr4_foreign_draft_through_a_view_is_refused_by_replacement() -> TestResult {
    let (directory, _output, _, result) = rr4_replace_forged(
        "ALTER TABLE drafts RENAME TO drafts_kept;
         CREATE VIEW drafts AS
             SELECT tenant_id, workspace_id, item_id, editor, saved_at, record, body, properties
             FROM drafts_kept
             UNION ALL
             SELECT tenant_id, 'WS99', item_id, editor, saved_at, record, body, properties
             FROM drafts_kept",
    )
    .await?;
    rr4_assert_refused(directory.path(), result, "drafts")
}

/// Q3: the foreign workspace as a generated column (not listed by `pragma_table_info`).
#[tokio::test]
async fn rr4_foreign_draft_as_a_generated_column_is_refused_by_replacement() -> TestResult {
    let (directory, _output, _, result) = rr4_replace_forged(
        "CREATE TABLE drafts_new (tenant_id TEXT NOT NULL, item_id TEXT NOT NULL,
             editor TEXT NOT NULL, saved_at TEXT NOT NULL, record TEXT NOT NULL,
             body TEXT NOT NULL, properties TEXT NOT NULL,
             workspace_id TEXT GENERATED ALWAYS AS ('WS99') VIRTUAL);
         INSERT INTO drafts_new (tenant_id, item_id, editor, saved_at, record, body, properties)
             SELECT tenant_id, item_id, editor, saved_at, record, body, properties FROM drafts;
         DROP TABLE drafts;
         ALTER TABLE drafts_new RENAME TO drafts",
    )
    .await?;
    rr4_assert_refused(directory.path(), result, "drafts")
}

/// Q3: a job output whose job the archive does not hold (implementer claim, untested).
#[tokio::test]
async fn rr4_absent_job_output_is_refused_by_replacement_before_any_write() -> TestResult {
    let (directory, _output, workspace, result) = rr4_replace_forged(&format!(
        "INSERT INTO job_outputs (job_id, digest) VALUES ('{}', '{}')",
        uuid_text(98),
        "9".repeat(64)
    ))
    .await?;
    let repository = directory
        .path()
        .join("repositories")
        .join("local")
        .join(format!("{}.git", workspace.workspace_id.0));
    let error = err_of(result)?;
    assert_eq!(error.code, ErrorCode::InvalidInput, "{error}");
    assert!(error.message.contains("job_outputs"), "{error}");
    assert!(!repository.exists(), "no repository written");
    Ok(())
}

/// Q3: the same, with the archive's column spelled `JOB_ID`.
#[tokio::test]
async fn rr4_absent_job_output_under_an_upper_case_column_is_refused_before_any_write() -> TestResult
{
    let (directory, _output, workspace, result) = rr4_replace_forged(&format!(
        "ALTER TABLE job_outputs RENAME COLUMN job_id TO JOB_ID;
         INSERT INTO job_outputs VALUES ('{}', '{}')",
        uuid_text(98),
        "9".repeat(64)
    ))
    .await?;
    let repository = directory
        .path()
        .join("repositories")
        .join("local")
        .join(format!("{}.git", workspace.workspace_id.0));
    let written = repository.exists();
    let error = err_of(result)?;
    assert!(
        error.code == ErrorCode::InvalidInput && !written,
        "refused as {:?} ({}), repository written: {written}",
        error.code,
        error.message
    );
    Ok(())
}

/// Q3: an import refuses a plain foreign draft and an absent job's output too.
#[tokio::test]
async fn rr4_an_import_refuses_foreign_and_jobless_rows() -> TestResult {
    for (sql, table) in [
        (
            "INSERT INTO drafts SELECT tenant_id, 'WS99', item_id, editor, saved_at, record,
                 body, properties FROM drafts"
                .to_owned(),
            "drafts",
        ),
        (
            format!(
                "INSERT INTO job_outputs (job_id, digest) VALUES ('{}', '{}')",
                uuid_text(98),
                "9".repeat(64)
            ),
            "job_outputs",
        ),
    ] {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(directory.path())?;
        let source = notes_workspace(&storage).await?;
        let bytes = archive_bytes(&storage, &source).await?;
        let forged = rr4_forged_records(&bytes, &sql)?;
        let crafted = rebuilt(
            &bytes,
            &|_| zip::CompressionMethod::Stored,
            &|name| (name == "records.sqlite").then(|| forged.clone()),
            &[],
        )?;
        let outcome = import_crafted(&storage, crafted).await?;
        assert_import_refused(&storage, outcome, table).await?;
    }
    Ok(())
}

/// Q3: a notes workspace with one finished job (and its output, or its cancel control)
/// round-trips by replacement over the deleted directory.
async fn rr4_job_round_trip(control: bool, output: bool) -> TestResult {
    let directory = tempfile::tempdir()?;
    let backup = tempfile::tempdir()?;
    let workspace = {
        let storage = Storage::open(directory.path())?;
        let workspace = notes_workspace(&storage).await?;
        let records = storage.records();
        let job_scope = JobScope::Workspace(workspace.clone());
        let job = records
            .create_job(
                &job_scope,
                NewJob {
                    mutation_id: uuid!(61)?,
                    initiator: person("ana"),
                    spec: JobSpec::BackupWorkspace,
                },
            )
            .await?;
        if control {
            records.cancel_job(&job_scope, uuid!(62)?, job.id).await?;
        } else {
            let claimed = some(records.claim_job(job.id).await?, "the claim")?;
            let outputs = if output {
                vec![put(&storage, &workspace, b"rr4 job output").await?.digest]
            } else {
                Vec::new()
            };
            records
                .complete_job(JobCompletion {
                    lease: claimed.lease,
                    revision: None,
                    item_ids: Vec::new(),
                    artifact: None,
                    restore: None,
                    backup: None,
                    outputs,
                    warnings: Vec::new(),
                })
                .await?;
        }
        storage
            .backups()
            .write_backup_set(backup.path(), None)
            .await?;
        workspace
    };
    let (installation, archive) = set_of(backup.path(), &workspace);
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    restored
        .backups()
        .restore_replacement(&installation, &[archive])
        .await?;
    Ok(())
}

#[tokio::test]
async fn rr4_rt_a_finished_job_without_output_or_control() -> TestResult {
    rr4_job_round_trip(false, false).await
}

#[tokio::test]
async fn rr4_rt_a_finished_job_with_an_output() -> TestResult {
    rr4_job_round_trip(false, true).await
}

#[tokio::test]
async fn rr4_rt_a_cancelled_job_with_its_control() -> TestResult {
    rr4_job_round_trip(true, false).await
}

/// Q3: the real backup journey. A workspace backed up once by the application's backup job
/// (which, as `ingest/src/handler.rs:419` does, completes with its archive as the job's output)
/// is backed up again in a set and restored by replacement over the deleted directory.
#[tokio::test]
async fn rr4_rt_a_workspace_the_app_backed_up_once_restores_by_replacement() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let workspace = {
        let storage = Storage::open(directory.path())?;
        let workspace = notes_workspace(&storage).await?;
        let scope = JobScope::Workspace(workspace.clone());
        let records = storage.records();
        let job = records
            .create_job(
                &scope,
                NewJob {
                    mutation_id: uuid!(30)?,
                    initiator: person("ana"),
                    spec: JobSpec::BackupWorkspace,
                },
            )
            .await?;
        let claimed = some(records.claim_job(job.id).await?, "the claimed job")?;
        let archive = storage
            .backups()
            .write_workspace_archive(&workspace, claimed.mutation_id)
            .await?
            .object;
        let artifact = records
            .record_artifact(
                &scope,
                claimed.mutation_id,
                NewArtifact {
                    kind: ArtifactKind::WorkspaceBackup,
                    object: archive.clone(),
                    media_type: "application/zip".to_owned(),
                    created_by_job: job.id,
                },
            )
            .await?;
        records
            .complete_job(JobCompletion {
                lease: claimed.lease,
                revision: None,
                item_ids: Vec::new(),
                artifact: Some(artifact.id),
                restore: None,
                backup: None,
                outputs: vec![archive.digest],
                warnings: Vec::new(),
            })
            .await?;
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        workspace
    };
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let (installation, archive) = set_of(output.path(), &workspace);
    restored
        .backups()
        .restore_replacement(&installation, &[archive])
        .await?;
    Ok(())
}

/// Pass-4 re-review MTENANT: a row of another TENANT under the archive's own workspace id is
/// refused before anything is written.
#[tokio::test]
async fn backup_a_replacement_restore_refuses_a_row_of_another_tenant() -> TestResult {
    let (directory, _output, _, result) = rr4_replace_forged(
        "INSERT INTO drafts SELECT 'other', workspace_id, item_id, editor, saved_at, record, body,
             properties FROM drafts",
    )
    .await?;
    let error = err_of(result)?;
    assert_eq!(error.code, ErrorCode::InvalidInput, "{error}");
    assert!(error.message.contains("drafts"), "{error}");
    let live = directory.path().join("records.sqlite");
    for table in ["workspaces", "drafts"] {
        assert_eq!(
            count(&live, &format!("SELECT count(*) FROM {table}"))?,
            0,
            "no {table} row written"
        );
    }
    Ok(())
}

/// Pass-4 re-review B1: an archive whose table is not this build's (here `drafts.body` made a
/// generated column, its rows all of the archive's own tenant and workspace) is refused before
/// any row is read or written: the schema, not only the rows, must be this build's.
#[tokio::test]
async fn backup_a_replacement_restore_refuses_an_archive_table_with_a_generated_column()
-> TestResult {
    let (directory, _output, _, result) = rr4_replace_forged(
        "CREATE TABLE drafts_new (
             tenant_id TEXT NOT NULL, workspace_id TEXT NOT NULL, item_id TEXT NOT NULL,
             editor TEXT NOT NULL, saved_at TEXT NOT NULL, record TEXT NOT NULL,
             body TEXT GENERATED ALWAYS AS ('forged') VIRTUAL, properties TEXT NOT NULL,
             PRIMARY KEY (tenant_id, workspace_id, item_id, editor)) STRICT;
         INSERT INTO drafts_new (tenant_id, workspace_id, item_id, editor, saved_at, record,
             properties)
             SELECT tenant_id, workspace_id, item_id, editor, saved_at, record, properties
             FROM drafts;
         DROP TABLE drafts;
         ALTER TABLE drafts_new RENAME TO drafts;",
    )
    .await?;
    let error = err_of(result)?;
    assert_eq!(error.code, ErrorCode::InvalidInput, "{error}");
    assert!(error.message.contains("drafts"), "{error}");
    assert_eq!(
        count(
            &directory.path().join("records.sqlite"),
            "SELECT count(*) FROM drafts"
        )?,
        0,
        "no draft written"
    );
    Ok(())
}

// The PR #20 pass-4b re-review's probes (lane-storage-m2-pass4b-rereview.md), adopted, and the
// tests of its rulings.

/// SQL that takes the quotes `ALTER TABLE ... RENAME` puts around the table `name` off its
/// stored statement (`CREATE TABLE "name"` becomes `CREATE TABLE name`), as this build writes it.
fn rr5_unquoted(name: &str) -> String {
    format!(
        "PRAGMA writable_schema = ON;
         UPDATE sqlite_schema
             SET sql = replace(sql, 'CREATE TABLE \"{name}\"', 'CREATE TABLE {name}')
             WHERE name = '{name}';
         PRAGMA writable_schema = OFF;"
    )
}

/// The archive `drafts` table rebuilt as `definition`, its rows copied by `copy`. The rename
/// stores the statement as `CREATE TABLE "drafts" ...`; the quotes are taken off (`rr5_unquoted`),
/// so the stored statement is this build's, up to whitespace, but for what `definition` changes:
/// the same rebuild with nothing changed restores
/// (`rr5_control_a_rebuild_with_no_change_restores`).
fn rr5_drafts_as(definition: &str, copy: &str) -> String {
    format!(
        "CREATE TABLE drafts_new {definition};
         INSERT INTO drafts_new {copy};
         DROP TABLE drafts;
         ALTER TABLE drafts_new RENAME TO drafts;
         {}",
        rr5_unquoted("drafts")
    )
}

/// This build's `drafts` statement after `CREATE TABLE drafts`, its `columns` replaced and with
/// `tail` after the closing parenthesis, token for token as the live schema states it.
fn rr5_definition(columns: &str, tail: &str) -> String {
    format!("( {columns}, {RR5_KEY} ){tail}")
}

/// Each schema variant of `drafts` that is not this build's, each changing exactly one
/// property, with its rows all of the archive's own tenant and workspace.
fn rr5_variants() -> Vec<(&'static str, String)> {
    let all = "SELECT * FROM drafts";
    vec![
        (
            "collation",
            rr5_drafts_as(
                &rr5_definition(
                    &RR5_COLUMNS.replace(
                        "workspace_id TEXT NOT NULL",
                        "workspace_id TEXT NOT NULL COLLATE NOCASE",
                    ),
                    " STRICT",
                ),
                all,
            ),
        ),
        (
            "not strict",
            rr5_drafts_as(&rr5_definition(RR5_COLUMNS, ""), all),
        ),
        (
            "without rowid",
            rr5_drafts_as(&rr5_definition(RR5_COLUMNS, " STRICT, WITHOUT ROWID"), all),
        ),
        (
            "stored generated",
            rr5_drafts_as(
                &rr5_definition(
                    &RR5_COLUMNS.replace(
                        "body TEXT NOT NULL",
                        "body TEXT GENERATED ALWAYS AS ('forged') STORED",
                    ),
                    " STRICT",
                ),
                "(tenant_id, workspace_id, item_id, editor, saved_at, record, properties)
                 SELECT tenant_id, workspace_id, item_id, editor, saved_at, record, properties
                 FROM drafts",
            ),
        ),
        (
            "extra column",
            rr5_drafts_as(
                &rr5_definition(&format!("{RR5_COLUMNS}, extra TEXT"), " STRICT"),
                "SELECT *, NULL FROM drafts",
            ),
        ),
        (
            "missing column",
            rr5_drafts_as(
                &rr5_definition(
                    &RR5_COLUMNS.replace(", properties TEXT NOT NULL", ""),
                    " STRICT",
                ),
                "SELECT tenant_id, workspace_id, item_id, editor, saved_at, record, body
                 FROM drafts",
            ),
        ),
        (
            "other type",
            rr5_drafts_as(
                &rr5_definition(
                    &RR5_COLUMNS.replace("saved_at TEXT NOT NULL", "saved_at ANY NOT NULL"),
                    " STRICT",
                ),
                all,
            ),
        ),
        (
            "other default",
            rr5_drafts_as(
                &rr5_definition(
                    &RR5_COLUMNS.replace("body TEXT NOT NULL", "body TEXT NOT NULL DEFAULT ''"),
                    " STRICT",
                ),
                all,
            ),
        ),
        (
            "other case table",
            format!(
                "ALTER TABLE drafts RENAME TO drafts_tmp; ALTER TABLE drafts_tmp RENAME TO Drafts;
                 {}",
                rr5_unquoted("Drafts")
            ),
        ),
        (
            "check",
            rr5_drafts_as(
                &rr5_definition(
                    &format!("{RR5_COLUMNS}, CHECK (length(body) >= 0)"),
                    " STRICT",
                ),
                all,
            ),
        ),
        (
            "extra index",
            "CREATE INDEX drafts_by_editor ON drafts (editor);".to_owned(),
        ),
    ]
}

/// Assert a forged replacement was refused as `InvalidInput` before any workspace row or
/// repository was written.
fn rr5_refused_before_writing(
    directory: &Path,
    workspace: &StorageScope,
    result: Restored,
    what: &str,
) -> TestResult {
    let repository = directory
        .join("repositories")
        .join("local")
        .join(format!("{}.git", workspace.workspace_id.0));
    let written = repository.exists();
    let rows = count(
        &directory.join("records.sqlite"),
        "SELECT count(*) FROM workspaces",
    )?;
    match result {
        Ok(reports) => Err(format!("{what}: restored ({reports:?})").into()),
        Err(error) => {
            assert!(
                error.code == ErrorCode::InvalidInput && !written && rows == 0,
                "{what}: refused as {:?} ({}), repository written: {written}, workspace rows: \
                 {rows}",
                error.code,
                error.message
            );
            Ok(())
        }
    }
}

/// B1-R: the schema variant `name` is refused by a replacement before anything is written.
async fn rr5_replacement_refuses(name: &str) -> TestResult {
    let variants = rr5_variants();
    let sql = &some(variants.iter().find(|(n, _)| *n == name), "the variant")?.1;
    let (directory, _output, workspace, result) = rr4_replace_forged(sql).await?;
    rr5_refused_before_writing(directory.path(), &workspace, result, name)
}

/// B1-R: the schema variant `name` is refused by an import before anything is written.
async fn rr5_import_refuses(name: &str) -> TestResult {
    let variants = rr5_variants();
    let sql = &some(variants.iter().find(|(n, _)| *n == name), "the variant")?.1;
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let forged = rr4_forged_records(&bytes, sql)?;
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|n| (n == "records.sqlite").then(|| forged.clone()),
        &[],
    )?;
    let outcome = import_crafted(&storage, crafted).await?;
    assert_import_refused(&storage, outcome, "drafts").await
}

#[tokio::test]
async fn rr5_replace_collation() -> TestResult {
    rr5_replacement_refuses("collation").await
}

#[tokio::test]
async fn rr5_replace_not_strict() -> TestResult {
    rr5_replacement_refuses("not strict").await
}

#[tokio::test]
async fn rr5_replace_without_rowid() -> TestResult {
    rr5_replacement_refuses("without rowid").await
}

#[tokio::test]
async fn rr5_replace_stored_generated() -> TestResult {
    rr5_replacement_refuses("stored generated").await
}

#[tokio::test]
async fn rr5_replace_extra_column() -> TestResult {
    rr5_replacement_refuses("extra column").await
}

#[tokio::test]
async fn rr5_replace_missing_column() -> TestResult {
    rr5_replacement_refuses("missing column").await
}

#[tokio::test]
async fn rr5_replace_other_type() -> TestResult {
    rr5_replacement_refuses("other type").await
}

#[tokio::test]
async fn rr5_replace_other_default() -> TestResult {
    rr5_replacement_refuses("other default").await
}

#[tokio::test]
async fn rr5_replace_other_case_table() -> TestResult {
    rr5_replacement_refuses("other case table").await
}

#[tokio::test]
async fn rr5_replace_check() -> TestResult {
    rr5_replacement_refuses("check").await
}

#[tokio::test]
async fn rr5_replace_extra_index() -> TestResult {
    rr5_replacement_refuses("extra index").await
}

#[tokio::test]
async fn rr5_import_extra_index() -> TestResult {
    rr5_import_refuses("extra index").await
}

#[tokio::test]
async fn rr5_import_collation() -> TestResult {
    rr5_import_refuses("collation").await
}

#[tokio::test]
async fn rr5_import_not_strict() -> TestResult {
    rr5_import_refuses("not strict").await
}

#[tokio::test]
async fn rr5_import_without_rowid() -> TestResult {
    rr5_import_refuses("without rowid").await
}

#[tokio::test]
async fn rr5_import_stored_generated() -> TestResult {
    rr5_import_refuses("stored generated").await
}

#[tokio::test]
async fn rr5_import_extra_column() -> TestResult {
    rr5_import_refuses("extra column").await
}

#[tokio::test]
async fn rr5_import_missing_column() -> TestResult {
    rr5_import_refuses("missing column").await
}

#[tokio::test]
async fn rr5_import_other_type() -> TestResult {
    rr5_import_refuses("other type").await
}

#[tokio::test]
async fn rr5_import_other_default() -> TestResult {
    rr5_import_refuses("other default").await
}

#[tokio::test]
async fn rr5_import_other_case_table() -> TestResult {
    rr5_import_refuses("other case table").await
}

/// B1-R: a trigger in the archive database never fires (nothing writes to the archive after the
/// check), so an archive carrying one restores as any other: the schema comparison covers
/// tables and their indexes, not triggers.
#[tokio::test]
async fn rr5_a_trigger_in_the_archive_changes_nothing() -> TestResult {
    let (directory, _output, workspace, result) = rr4_replace_forged(
        "CREATE TRIGGER drafts_touch AFTER INSERT ON drafts BEGIN SELECT 1; END;",
    )
    .await?;
    result?;
    assert_eq!(
        count(
            &directory.path().join("records.sqlite"),
            &format!(
                "SELECT count(*) FROM drafts WHERE workspace_id = '{}'",
                workspace.workspace_id.0
            )
        )?,
        1
    );
    Ok(())
}

/// A backup set of a notes workspace with one cancelled job (and its cancel control), its
/// workspace archive's records changed by `sql`, restored by replacement over the deleted
/// directory.
async fn rr5_replace_job_forged(
    sql: &str,
) -> Fallible<(tempfile::TempDir, tempfile::TempDir, StorageScope, Restored)> {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let workspace = {
        let storage = Storage::open(directory.path())?;
        let workspace = notes_workspace(&storage).await?;
        let records = storage.records();
        let job_scope = JobScope::Workspace(workspace.clone());
        let job = records
            .create_job(
                &job_scope,
                NewJob {
                    mutation_id: uuid!(61)?,
                    initiator: person("ana"),
                    spec: JobSpec::BackupWorkspace,
                },
            )
            .await?;
        records.cancel_job(&job_scope, uuid!(62)?, job.id).await?;
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        workspace
    };
    let (installation, archive) = set_of(output.path(), &workspace);
    let bytes = std::fs::read(&archive)?;
    let forged = rr4_forged_records(&bytes, sql)?;
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|name| (name == "records.sqlite").then(|| forged.clone()),
        &[],
    )?;
    std::fs::write(&archive, crafted)?;
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let result = restored
        .backups()
        .restore_replacement(&installation, &[archive])
        .await;
    drop(restored);
    Ok((directory, output, workspace, result))
}

/// B1-R: `job_controls` rebuilt with a case-insensitive `job_id`, its control pointing at the
/// upper-cased id of its own job. Before this round the job-keyed row check compared under the
/// archive column's collation and passed, and only the live foreign key caught it, after the
/// objects and the repository were written.
#[tokio::test]
async fn rr5_a_case_insensitive_control_column_is_refused_before_any_write() -> TestResult {
    let (directory, _output, workspace, result) = rr5_replace_job_forged(
        "CREATE TABLE job_controls_new (
             mutation_id TEXT PRIMARY KEY,
             job_id TEXT NOT NULL COLLATE NOCASE REFERENCES jobs (job_id),
             action TEXT NOT NULL CHECK (action IN ('cancel', 'retry'))) STRICT;
         INSERT INTO job_controls_new SELECT mutation_id, upper(job_id), action FROM job_controls;
         DROP TABLE job_controls;
         ALTER TABLE job_controls_new RENAME TO job_controls;",
    )
    .await?;
    if let Err(error) = &result {
        assert!(error.message.contains("job_controls"), "{error}");
    }
    rr5_refused_before_writing(
        directory.path(),
        &workspace,
        result,
        "job_controls collation",
    )
}

/// B1-R: a non-STRICT archive table holding a value of another type than its column is refused
/// before anything is written, and never reported as a collision.
#[tokio::test]
async fn rr5_a_wrongly_typed_value_in_a_loose_table_is_refused_before_any_write() -> TestResult {
    let sql = format!(
        "{} UPDATE drafts SET body = CAST(body AS BLOB);",
        rr5_drafts_as(
            &format!("({RR5_COLUMNS}, {RR5_KEY})"),
            "SELECT * FROM drafts"
        )
    );
    let (directory, _output, workspace, result) = rr4_replace_forged(&sql).await?;
    rr5_refused_before_writing(directory.path(), &workspace, result, "loose drafts")
}

/// B1-R: records that fail `PRAGMA integrity_check` are refused before anything is written.
/// The `events_by_log` index is left empty under this build's statement (built partial, then
/// its statement rewritten), so the table and its index disagree while every table and index
/// statement is this build's, and every row is the archive's own.
#[tokio::test]
async fn backup_a_restore_refuses_damaged_records_before_any_write() -> TestResult {
    let (directory, _output, workspace, result) = rr4_replace_forged(
        "INSERT INTO events (tenant_id, workspace_id, event_key, record)
             SELECT tenant_id, workspace_id, 'rr5-damaged', '{}' FROM workspaces;
         DROP INDEX events_by_log;
         CREATE INDEX events_by_log ON events (tenant_id, workspace_id, seq) WHERE seq < 0;
         PRAGMA writable_schema = ON;
         UPDATE sqlite_schema
             SET sql = 'CREATE INDEX events_by_log ON events (tenant_id, workspace_id, seq)'
             WHERE name = 'events_by_log';
         PRAGMA writable_schema = OFF;",
    )
    .await?;
    if let Err(error) = &result {
        assert!(error.message.contains("damaged"), "{error}");
    }
    rr5_refused_before_writing(directory.path(), &workspace, result, "damaged records")
}

/// B1-R: a constraint the live database still enforces at the copy is the archive's, so
/// `InvalidInput`, never a collision: here the second archive of a set gives its workspace the
/// creation mutation of the first, which the same restore already wrote. N-3: the failed run
/// leaves what `restore_replacement` says it leaves (the installation's rows, the first
/// workspace in full, and of the second its history but none of its rows), and a run again
/// with the good set completes.
#[tokio::test]
async fn backup_a_constraint_failed_at_the_copy_is_invalid_input() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let (first, second, identity) = {
        let storage = Storage::open(directory.path())?;
        let identity = storage.credentials().installation_identity().await?;
        let first = notes_workspace(&storage).await?;
        let (second, created) = create(&storage, "local", 2, "Second").await?;
        // The second holds an object, so what its failed copy leaves of it can be seen.
        let original = put(&storage, &second, b"n3 the second workspace's original").await?;
        commit(
            &storage,
            &second,
            3,
            &created,
            vec![card(uuid!(15)?, &original.digest)?],
        )
        .await?;
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        (first, second, identity)
    };
    let (installation, first_archive) = set_of(output.path(), &first);
    let (_, second_archive) = set_of(output.path(), &second);
    let bytes = std::fs::read(&second_archive)?;
    let good_second = output.path().join("good-second.zip");
    std::fs::write(&good_second, &bytes)?;
    let records = rr4_forged_records(
        &bytes,
        &format!("UPDATE workspaces SET mutation_id = '{}'", uuid_text(1)),
    )?;
    std::fs::write(
        &second_archive,
        rebuilt(
            &bytes,
            &|_| zip::CompressionMethod::Stored,
            &|name| (name == "records.sqlite").then(|| records.clone()),
            &[],
        )?,
    )?;
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let error = err_of(
        restored
            .backups()
            .restore_replacement(&installation, &[first_archive.clone(), second_archive])
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::InvalidInput, "{error}");
    assert!(
        error.message.contains("workspaces") && error.message.contains("constraint"),
        "{error}"
    );
    // What the failed run left: the installation's rows; the first workspace in full, its rows
    // with the record of the restore that wrote them; of the second, its objects as pending
    // puts and its history, no row.
    let live = directory.path().join("records.sqlite");
    assert_eq!(
        restored
            .credentials()
            .installation_identity()
            .await?
            .subject,
        identity.subject
    );
    for (workspace, table, rows) in [
        (&first, "workspaces", 1),
        (&first, "replacement_restores", 1),
        (&first, "drafts", 1),
        (&second, "workspaces", 0),
        (&second, "replacement_restores", 0),
        (&second, "pending_puts", 1),
    ] {
        assert_eq!(rows_of(&live, workspace, table)?, rows, "{table}");
    }
    assert!(
        restored
            .data()
            .repositories_path()
            .join("local")
            .join(format!("{}.git", second.workspace_id.0))
            .is_dir(),
        "the second workspace's history was written before its rows failed"
    );
    // A run again with the good set completes: the first is accepted as it is, the second is
    // restored over the history the failed run left.
    let reports = restored
        .backups()
        .restore_replacement(&installation, &[first_archive, good_second])
        .await?;
    assert_eq!(reports.len(), 2);
    for workspace in [&first, &second] {
        assert_eq!(rows_of(&live, workspace, "workspaces")?, 1);
        assert_eq!(rows_of(&live, workspace, "replacement_restores")?, 1);
        restored.catalog().open(workspace).await?;
    }
    Ok(())
}

/// How many rows of `table` in the records database `live` name `workspace`.
fn rows_of(live: &Path, workspace: &StorageScope, table: &str) -> Fallible<i64> {
    count(
        live,
        &format!(
            "SELECT count(*) FROM {table} WHERE workspace_id = '{}'",
            workspace.workspace_id.0
        ),
    )
}

/// B1-R: a workspace archive whose records are at records schema 1 (a pre-migration set's)
/// still restores by replacement after the schema comparison: this build's migrations bring it
/// to exactly this build's tables and indexes.
#[tokio::test]
async fn backup_a_records_schema_1_archive_restores_by_replacement() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let workspace = fixture.workspace.clone();
    drop(fixture);
    as_records_schema_1(&directory.path().join("records.sqlite"))?;
    let (installation, archive) = {
        let storage = Storage::open(directory.path())?;
        let sets: Vec<_> = std::fs::read_dir(storage.data().pre_migration_path())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        let (installation, archive) =
            set_of(some(sets.first(), "the pre-migration set")?, &workspace);
        let copies = (
            output.path().join("installation.zip"),
            output.path().join("workspace.zip"),
        );
        std::fs::copy(installation, &copies.0)?;
        std::fs::copy(archive, &copies.1)?;
        copies
    };
    let staged = output.path().join("records.sqlite");
    std::fs::write(
        &staged,
        entry_bytes_of(&std::fs::read(&archive)?, "records.sqlite")?,
    )?;
    assert_eq!(
        count(&staged, "PRAGMA user_version")?,
        1,
        "a schema-1 archive"
    );
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    restored
        .backups()
        .restore_replacement(&installation, &[archive])
        .await?;
    let live = directory.path().join("records.sqlite");
    assert_eq!(count(&live, "SELECT count(*) FROM workspaces")?, 1);
    assert!(
        count(&live, "SELECT count(*) FROM drafts")? > 0,
        "its drafts are restored"
    );
    Ok(())
}

/// Records schema 3 and older archives: a backup set written from a data directory at records
/// schema 2 restores on this build, by replacement and by import. The live records are put
/// back as schema 2 left them (schema 3 only adds the table of kept warnings), and opening the
/// directory writes the pre-migration set from that schema-2 database before it migrates. That
/// set's workspace archive holds records at `user_version` 2 with no `commit_warnings` table.
/// It restores by replacement into an emptied directory, where a restored commit replays with
/// the unavailable warning, and by import into a blank workspace.
///
/// The limit of this test: the archive is written by this build's writer from a schema-2
/// database, not by a build that stopped at schema 2. It needs no committed fixture.
/// (Reviewer probe PB1, adopted.)
#[tokio::test]
async fn backup_a_records_schema_2_backup_set_restores_by_replacement_and_by_import() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let workspace = fixture.workspace.clone();
    drop(fixture);
    rusqlite::Connection::open(directory.path().join("records.sqlite"))?.execute_batch(
        "DROP TABLE commit_warnings; ALTER TABLE jobs DROP COLUMN sealed;
             PRAGMA user_version = 2;",
    )?;
    let (installation, archive) = {
        let storage = Storage::open(directory.path())?;
        let sets: Vec<_> = std::fs::read_dir(storage.data().pre_migration_path())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        let (installation, archive) =
            set_of(some(sets.first(), "the pre-migration set")?, &workspace);
        let copies = (
            output.path().join("installation.zip"),
            output.path().join("workspace.zip"),
        );
        std::fs::copy(installation, &copies.0)?;
        std::fs::copy(archive, &copies.1)?;
        copies
    };
    let staged = output.path().join("records.sqlite");
    std::fs::write(
        &staged,
        entry_bytes_of(&std::fs::read(&archive)?, "records.sqlite")?,
    )?;
    assert_eq!(
        count(&staged, "PRAGMA user_version")?,
        2,
        "a schema-2 archive"
    );
    assert_eq!(
        count(
            &staged,
            "SELECT count(*) FROM sqlite_schema WHERE name = 'commit_warnings'"
        )?,
        0,
        "with no table of kept warnings"
    );
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    restored
        .backups()
        .restore_replacement(&installation, std::slice::from_ref(&archive))
        .await?;
    let live = directory.path().join("records.sqlite");
    assert_eq!(count(&live, "PRAGMA user_version")?, 4);
    assert_eq!(count(&live, "SELECT count(*) FROM workspaces")?, 1);
    assert!(
        count(&live, "SELECT count(*) FROM drafts")? > 0,
        "its drafts are restored"
    );
    assert_eq!(count(&live, "SELECT count(*) FROM commit_warnings")?, 0);
    // A commit of the restored history (the fixture's, mutation 3) has no kept warnings.
    let versions = restored.versions();
    let archived = versions.find_commit(&workspace, uuid!(3)?, None).await?;
    let archived = some(archived, "a commit of the restored history")?;
    assert_eq!(codes(&archived.warnings), [REPLAY_WARNINGS_UNAVAILABLE]);
    // By import, into a blank workspace of another tenant of the restored installation.
    let (target, _) = create(&restored, "elsewhere", 80, "Copy").await?;
    let uploaded = restored
        .blobs()
        .put(
            &target,
            Box::pin(std::io::Cursor::new(std::fs::read(&archive)?)),
            1 << 30,
            None,
        )
        .await?
        .digest;
    restored
        .backups()
        .restore_import(&target, uploaded, uuid!(84)?, vec!["ana".to_owned()])
        .await?;
    let imported = versions.find_commit(&target, uuid!(84)?, None).await?;
    let imported = some(imported, "the import's commit")?;
    assert_eq!(imported.revision, versions.head(&target).await?);
    Ok(())
}

/// Gap 2: a `records.sqlite` that is not a database is refused as invalid input.
#[tokio::test]
async fn rr5_a_records_entry_that_is_not_a_database_is_invalid_input() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|name| (name == "records.sqlite").then(|| b"not a database at all".repeat(256)),
        &[],
    )?;
    let outcome = import_crafted(&storage, crafted).await?;
    assert_import_refused(&storage, outcome, "records database").await
}

/// Gap 2: records at a newer schema than this build's do not migrate, and are refused as
/// invalid input before anything is written.
#[tokio::test]
async fn backup_an_archive_whose_records_do_not_migrate_is_invalid_input() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let forged = rr4_forged_records(&bytes, "PRAGMA user_version = 99;")?;
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|name| (name == "records.sqlite").then(|| forged.clone()),
        &[],
    )?;
    let outcome = import_crafted(&storage, crafted).await?;
    assert_import_refused(&storage, outcome, "migrate").await
}

/// Gap 2: an archive whose records entry is an empty file (which SQLite reads as an empty
/// database, and this build migrates to its schema) holds no workspace, and is refused before
/// anything is written.
#[tokio::test]
async fn backup_an_archive_whose_records_hold_no_workspace_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|name| (name == "records.sqlite").then(Vec::new),
        &[],
    )?;
    let outcome = import_crafted(&storage, crafted).await?;
    assert_import_refused(&storage, outcome, "workspaces").await
}

/// B4: an `objects/` entry whose name is not a digest is refused before anything is written.
#[tokio::test]
async fn rr5_an_object_entry_named_otherwise_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|_| None,
        &[("objects/not-a-digest", b"rr5 stray")],
    )?;
    let outcome = import_crafted(&storage, crafted).await?;
    assert_import_refused(&storage, outcome, "objects/").await
}

/// B4: an `objects/<digest>` entry that no row or card names is stored (its digest checked),
/// and stays as the target's pending put, which a purge of that workspace collects.
#[tokio::test]
async fn rr5_an_unneeded_object_entry_is_stored_and_left_pending() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let stray = b"rr5 stray object";
    let digest = {
        use sha2::Digest as _;
        use std::fmt::Write as _;
        let hash = sha2::Sha256::digest(stray);
        Digest::try_from(hash.iter().fold(String::new(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        }))?
    };
    let name = format!("objects/{}", digest.as_str());
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|_| None,
        &[(name.as_str(), stray)],
    )?;
    let (target, _, result) = import_crafted(&storage, crafted).await?;
    result?;
    assert!(
        storage.blobs().open(&target, &digest, 0, 1).await.is_ok(),
        "stored"
    );
    let pending = count(
        &directory.path().join("records.sqlite"),
        &format!(
            "SELECT count(*) FROM pending_puts WHERE workspace_id = '{}' AND digest = '{}'",
            target.workspace_id.0,
            digest.as_str()
        ),
    )?;
    assert_eq!(pending, 1, "left as the target's pending put");
    Ok(())
}

/// Gap 3: an `objects/<digest>` entry whose bytes are not that digest is refused as invalid
/// input when the archive is opened, before anything is written or stored.
#[tokio::test]
async fn rr5_an_object_entry_whose_bytes_are_not_its_digest_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let claimed = Digest::try_from("a".repeat(64))?;
    let name = format!("objects/{}", claimed.as_str());
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|_| None,
        &[(name.as_str(), b"rr5 bytes of another digest")],
    )?;
    let outcome = import_crafted(&storage, crafted).await?;
    let stored = storage
        .blobs()
        .open(&outcome.0, &claimed, 0, 1)
        .await
        .is_ok();
    assert!(!stored, "nothing stored under the claimed digest");
    assert_import_refused(&storage, outcome, "digest").await
}

/// Gap 1: a replacement checks every workspace archive before it writes the installation
/// archive's rows, so a set whose second archive is refused writes nothing at all: no tenant
/// event, no workspace and no repository. The good set then restores in full.
#[tokio::test]
async fn backup_a_replacement_set_with_a_refused_archive_writes_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let (first, second) = {
        let storage = Storage::open(directory.path())?;
        let first = notes_workspace(&storage).await?;
        let (second, _) = create(&storage, "local", 2, "Second").await?;
        storage
            .events()
            .append(&EventScope::Tenant(tenant("local")?), None, sign_in())
            .await?;
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        (first, second)
    };
    let (installation, first_archive) = set_of(output.path(), &first);
    let (_, second_archive) = set_of(output.path(), &second);
    let good = std::fs::read(&second_archive)?;
    let records = rr4_forged_records(
        &good,
        "INSERT INTO grants VALUES ('local', 'WS99', 'mallory', '[\"read\"]')",
    )?;
    std::fs::write(
        &second_archive,
        rebuilt(
            &good,
            &|_| zip::CompressionMethod::Stored,
            &|name| (name == "records.sqlite").then(|| records.clone()),
            &[],
        )?,
    )?;
    std::fs::remove_dir_all(directory.path())?;
    let archives = [first_archive, second_archive.clone()];
    {
        let restored = Storage::open(directory.path())?;
        let error = err_of(
            restored
                .backups()
                .restore_replacement(&installation, &archives)
                .await,
        )?;
        assert_eq!(error.code, ErrorCode::InvalidInput, "{error}");
        assert!(error.message.contains("grants"), "{error}");
    }
    let live = directory.path().join("records.sqlite");
    for table in ["events", "workspaces"] {
        assert_eq!(
            count(&live, &format!("SELECT count(*) FROM {table}"))?,
            0,
            "no {table} row written"
        );
    }
    assert!(
        !directory
            .path()
            .join("repositories")
            .join("local")
            .join(format!("{}.git", first.workspace_id.0))
            .exists(),
        "the first workspace's repository is not written"
    );
    std::fs::write(&second_archive, &good)?;
    let restored = Storage::open(directory.path())?;
    let reports = restored
        .backups()
        .restore_replacement(&installation, &archives)
        .await?;
    assert_eq!(reports.len(), 2);
    assert_eq!(count(&live, "SELECT count(*) FROM workspaces")?, 2);
    assert_eq!(count(&live, "SELECT count(*) FROM events")?, 1);
    Ok(())
}

/// B3: a purge of another workspace keeps the structured export and asset that a surviving
/// workspace's conversion record over 64 MiB names, even when the purged workspace named them
/// too (`references::record_tokens` past `RECORD_LIMIT`, read for survivors).
#[tokio::test]
async fn rr5_a_purge_keeps_what_a_survivors_large_record_names() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (survivor, created) = create(&storage, "local", 1, "Large").await?;
    let (target, target_head) = create(&storage, "local", 2, "Purged").await?;
    let source = put(&storage, &survivor, b"rr5 large original")
        .await?
        .digest;
    let structured = put(&storage, &target, b"{\"rr5\": \"large export\"}")
        .await?
        .digest;
    let asset = put(&storage, &target, b"rr5 large render").await?.digest;
    let bytes = rr4_record(&structured, &asset, 1_500_000);
    assert!(
        bytes.len() > 64 << 20,
        "the record is {} bytes",
        bytes.len()
    );
    let record = rr4_put_large(&storage, &survivor, bytes).await?;
    commit(
        &storage,
        &survivor,
        3,
        &created,
        vec![rr4_converted_card(
            uuid!(14)?,
            "sources/large.md",
            &source,
            &record,
        )?],
    )
    .await?;
    // The target names the same two objects, as cards over them.
    put(&storage, &target, b"{\"rr5\": \"large export\"}").await?;
    put(&storage, &target, b"rr5 large render").await?;
    commit(
        &storage,
        &target,
        4,
        &target_head,
        vec![card(uuid!(15)?, &structured)?, {
            let TreeEdit::WriteSourceCard(mut second) = card(uuid!(16)?, &asset)? else {
                return Err("a card".into());
            };
            second.path = WorkspacePath::try_from("sources/render.md".to_owned())?;
            TreeEdit::WriteSourceCard(second)
        }],
    )
    .await?;
    rr5_purge(&storage, &target).await?;
    for (digest, what) in [(&structured, "structured export"), (&asset, "asset")] {
        assert!(
            object_file(directory.path(), digest)?.is_file(),
            "the purge deleted the {what} a surviving large record names"
        );
    }
    Ok(())
}

/// Purge the workspace `target` (purge 61 under mutations 60 and 62).
async fn rr5_purge(storage: &Storage, target: &StorageScope) -> TestResult {
    let tenant = target.tenant_id.clone();
    let purge = storage
        .records()
        .create_purge(
            &tenant,
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
    storage
        .purger()
        .purge_workspace(&tenant, uuid!(62)?, purge.id, target.workspace_id)
        .await?;
    Ok(())
}

/// B3 bounded reading: an object over 64 MiB is read as a conversion record only when a source
/// card names it as one. A surviving workspace's large JSON upload that no card names, whose
/// `assets` and `structured` lists name two objects of the purged workspace, keeps neither:
/// the purge deletes both.
#[tokio::test]
async fn backup_a_large_upload_is_never_read_as_a_conversion_record() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (survivor, _) = create(&storage, "local", 1, "Large").await?;
    let (target, _) = create(&storage, "local", 2, "Purged").await?;
    let structured = put(&storage, &target, b"{\"rr5\": \"export an upload names\"}")
        .await?
        .digest;
    let asset = put(&storage, &target, b"rr5 render an upload names")
        .await?
        .digest;
    let bytes = rr4_record(&structured, &asset, 1_500_000);
    assert!(
        bytes.len() > 64 << 20,
        "the upload is {} bytes",
        bytes.len()
    );
    // The survivor's pending put, which a purge reads as a surviving reference; no card names it.
    rr4_put_large(&storage, &survivor, bytes).await?;
    rr5_purge(&storage, &target).await?;
    for (digest, what) in [(&structured, "structured export"), (&asset, "asset")] {
        assert!(
            !object_file(directory.path(), digest)?.is_file(),
            "the {what} was kept because a large upload, read as a record, names it"
        );
    }
    Ok(())
}

/// A completed upload of `workspace` holding `body` (mutation 63); returns its object.
async fn rr6_uploaded(
    storage: &Storage,
    workspace: &StorageScope,
    body: &[u8],
) -> Fallible<Digest> {
    let object = put(storage, workspace, body).await?.digest;
    let uploads = storage.uploads();
    let slot = uploads
        .create(
            workspace,
            uuid!(63)?,
            NewUpload {
                filename: "notes.json".to_owned(),
                relative_path: String::new(),
                expected_size: u64::try_from(body.len())?,
                expected_sha256: None,
                supplied_by: person("ana"),
            },
        )
        .await?;
    uploads
        .put_content(
            workspace,
            slot.id,
            Box::pin(std::io::Cursor::new(body.to_vec())),
            1 << 20,
        )
        .await?;
    uploads.complete(workspace, slot.id, object.clone()).await?;
    Ok(object)
}

/// B1-R round 2 (MSMALLUP): a small JSON upload is never read as a conversion record. A member
/// of workspace A uploads a few bytes of JSON holding the digest of an object only workspace B
/// holds (the original of B's card): A's workspace archive does not copy that object, and a
/// purge of B deletes it.
#[tokio::test]
async fn backup_a_small_upload_naming_another_workspaces_object_keeps_and_copies_nothing()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (uploader, _) = create(&storage, "local", 1, "Uploader").await?;
    let (target, head) = create(&storage, "local", 2, "Target").await?;
    let hidden = put(&storage, &target, b"rr6 an original only the target holds")
        .await?
        .digest;
    commit(
        &storage,
        &target,
        3,
        &head,
        vec![card(uuid!(15)?, &hidden)?],
    )
    .await?;
    let text = format!(
        "{{\"structured\": [{{\"digest\": \"{0}\"}}], \"assets\": [{{\"digest\": \"{0}\"}}], \
         \"note\": \"{0}\"}}",
        hidden.as_str()
    );
    rr6_uploaded(&storage, &uploader, text.as_bytes()).await?;
    let archive = storage
        .backups()
        .write_workspace_archive(&uploader, uuid!(40)?)
        .await?
        .object;
    let names = entries(&object_file(directory.path(), &archive.digest)?)?;
    assert!(
        !names.contains(&format!("objects/{}", hidden.as_str())),
        "the uploader's archive copied another workspace's object an upload names"
    );
    rr5_purge(&storage, &target).await?;
    assert!(
        !object_file(directory.path(), &hidden)?.is_file(),
        "the purged workspace's original survived because an upload names it"
    );
    Ok(())
}

/// A workspace of tenant `local` in the window of a conversion job (mutation 30): its original,
/// structured export, asset and a small conversion record are put, and its card naming the
/// record is committed, but the job has not completed, so its export and asset are still the
/// workspace's pending puts and no job output names them. Returns the workspace, the claimed
/// job, and (original, export, asset, record).
async fn rr6_conversion_before_its_completion(
    storage: &Storage,
) -> Fallible<(StorageScope, ClaimedJob, [Digest; 4])> {
    let (workspace, created) = create(storage, "local", 1, "Converted").await?;
    let records = storage.records();
    let job = records
        .create_job(
            &JobScope::Workspace(workspace.clone()),
            NewJob {
                mutation_id: uuid!(30)?,
                initiator: person("ana"),
                spec: JobSpec::Import {
                    base_revision: created.clone(),
                    upload_ids: Vec::new(),
                    destination: None,
                    apply_naming_rules: false,
                    settings: ConversionSettings::default(),
                },
            },
        )
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "the claimed job")?;
    let original = put(storage, &workspace, b"rr6 converted original")
        .await?
        .digest;
    let export = put(storage, &workspace, b"{\"rr6\": \"docling export\"}")
        .await?
        .digest;
    let asset = put(storage, &workspace, b"rr6 page render").await?.digest;
    let record = put(storage, &workspace, &rr4_record(&export, &asset, 3))
        .await?
        .digest;
    commit(
        storage,
        &workspace,
        31,
        &created,
        vec![rr4_converted_card(
            uuid!(14)?,
            "sources/report.md",
            &original,
            &record,
        )?],
    )
    .await?;
    Ok((workspace, claimed, [original, export, asset, record]))
}

/// The pending puts of `workspace` in the records at `data` that name one of `digests`.
fn rr6_pending(data: &Path, workspace: &StorageScope, digests: &[&Digest]) -> Fallible<i64> {
    let names = digests
        .iter()
        .map(|digest| format!("'{}'", digest.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    count(
        &data.join("records.sqlite"),
        &format!(
            "SELECT count(*) FROM pending_puts WHERE workspace_id = '{}' AND digest IN ({names})",
            workspace.workspace_id.0
        ),
    )
}

/// B1-R round 2 (MWINDOW): a conversion whose card is committed but whose job has not completed
/// (ingest lists its outputs only after the card's commit lands, and the completion then names
/// each as a job output) is backed up, restored by replacement and by import, then completed.
/// The archive holds the export and asset its card-named record names; each restore lands them
/// with the workspace; a purge of another workspace naming the asset keeps it for the imported
/// one; and the completion then names all three outputs and lands them.
#[tokio::test]
async fn backup_a_conversion_before_its_completion_restores_with_its_exports_and_assets()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (workspace, claimed, [_, export, asset, record]) =
        rr6_conversion_before_its_completion(&storage).await?;
    let named = [&export, &asset, &record];
    storage
        .backups()
        .write_backup_set(output.path(), None)
        .await?;
    let (installation, archive) = set_of(output.path(), &workspace);
    let held = entries(&archive)?;
    for digest in named {
        assert!(held.contains(&format!("objects/{}", digest.as_str())));
    }
    // By replacement, over a fresh data directory.
    let replaced = tempfile::tempdir()?;
    Storage::open(replaced.path())?
        .backups()
        .restore_replacement(&installation, std::slice::from_ref(&archive))
        .await?;
    for digest in named {
        assert!(object_file(replaced.path(), digest)?.is_file());
    }
    assert_eq!(
        rr6_pending(replaced.path(), &workspace, &named)?,
        0,
        "landed"
    );
    // By import, into a blank workspace of another tenant.
    let (target, _, imported) = import_crafted(&storage, std::fs::read(&archive)?).await?;
    imported?;
    for digest in named {
        assert!(storage.blobs().open(&target, digest, 0, 1).await.is_ok());
    }
    assert_eq!(rr6_pending(directory.path(), &target, &named)?, 0, "landed");
    // A purge of another workspace of that tenant whose card names the asset keeps it.
    let (other, other_head) = create(&storage, "elsewhere", 90, "Other").await?;
    put(&storage, &other, b"rr6 page render").await?;
    commit(
        &storage,
        &other,
        91,
        &other_head,
        vec![card(uuid!(16)?, &asset)?],
    )
    .await?;
    rr5_purge(&storage, &other).await?;
    assert!(
        storage.blobs().open(&target, &asset, 0, 1).await.is_ok(),
        "kept"
    );
    // Then the job completes: its outputs are named and landed.
    storage
        .records()
        .complete_job(JobCompletion {
            lease: claimed.lease,
            revision: None,
            item_ids: Vec::new(),
            artifact: None,
            restore: None,
            backup: None,
            outputs: vec![export.clone(), asset.clone(), record.clone()],
            warnings: Vec::new(),
        })
        .await?;
    let live = directory.path().join("records.sqlite");
    assert_eq!(count(&live, "SELECT count(*) FROM job_outputs")?, 3);
    assert_eq!(rr6_pending(directory.path(), &workspace, &named)?, 0);
    Ok(())
}

// The PR #20 B1-R re-review's probes (lane-storage-m2-b1r-rereview.md, `rr7_`), adopted, and the
// tests of its round-3 rulings.

/// The records entry of the archive `bytes`, with each of `steps` run on a fresh connection,
/// foreign keys off (`WS99` stands for another workspace id). A fresh connection per step reads
/// the schema a `writable_schema` step left on disk.
fn rr7_forged_steps(bytes: &[u8], steps: &[&str]) -> Fallible<Vec<u8>> {
    let staged = tempfile::tempdir()?;
    let records = staged.path().join("records.sqlite");
    std::fs::write(&records, entry_bytes_of(bytes, "records.sqlite")?)?;
    for step in steps {
        let connection = rusqlite::Connection::open(&records)?;
        connection.execute_batch(&format!(
            "PRAGMA foreign_keys = OFF; {}",
            step.replace("WS99", &uuid_text(99))
        ))?;
    }
    Ok(std::fs::read(&records)?)
}

/// The stored statement of the schema object `name` in the records at `records`.
fn rr7_statement(records: &Path, name: &str) -> Fallible<String> {
    let connection = rusqlite::Connection::open(records)?;
    Ok(connection.query_row(
        "SELECT sql FROM sqlite_schema WHERE name = ?1",
        [name],
        |row| row.get(0),
    )?)
}

/// The records entry of the archive `bytes` with the stored statement of `name` rewritten in
/// place (`writable_schema`): every `from` replaced by `to`, nothing else. Refuses a rewrite that
/// would change nothing, so a probe never passes vacuously.
fn rr7_rewritten(bytes: &[u8], name: &str, from: &str, to: &str) -> Fallible<Vec<u8>> {
    let staged = tempfile::tempdir()?;
    let records = staged.path().join("records.sqlite");
    std::fs::write(&records, entry_bytes_of(bytes, "records.sqlite")?)?;
    let before = rr7_statement(&records, name)?;
    if !before.contains(from) {
        return Err(format!("{name}'s statement holds no {from:?}: {before}").into());
    }
    {
        let connection = rusqlite::Connection::open(&records)?;
        connection.execute_batch("PRAGMA writable_schema = ON")?;
        connection.execute(
            "UPDATE sqlite_schema SET sql = replace(sql, ?1, ?2) WHERE name = ?3",
            rusqlite::params![from, to, name],
        )?;
        connection.execute_batch("PRAGMA writable_schema = OFF")?;
    }
    let after = rr7_statement(&records, name)?;
    if after == before {
        return Err(format!("{name}'s statement did not change").into());
    }
    Ok(std::fs::read(&records)?)
}

/// The records entry of the archive `bytes` copied, row for row, into a new database whose text
/// encoding is UTF-16le, built by this build's own migrations: the same statements, the same
/// rows.
fn rr7_utf16(bytes: &[u8]) -> Fallible<Vec<u8>> {
    let staged = tempfile::tempdir()?;
    let source = staged.path().join("source.sqlite");
    std::fs::write(&source, entry_bytes_of(bytes, "records.sqlite")?)?;
    let target = staged.path().join("utf16.sqlite");
    {
        let from = rusqlite::Connection::open(&source)?;
        let mut to = rusqlite::Connection::open(&target)?;
        to.execute_batch("PRAGMA encoding = 'UTF-16le'")?;
        okf_jawn_storage::schema::RECORDS.to_latest(&mut to)?;
        to.execute_batch("PRAGMA foreign_keys = OFF")?;
        let tables: Vec<String> = {
            let mut statement = from.prepare(
                "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )?;
            statement
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?
        };
        for table in tables {
            let mut select = from.prepare(&format!("SELECT * FROM \"{table}\""))?;
            let width = select.column_count();
            let rows: Vec<Vec<rusqlite::types::Value>> = select
                .query_map([], |row| {
                    (0..width)
                        .map(|column| row.get::<_, rusqlite::types::Value>(column))
                        .collect()
                })?
                .collect::<Result<_, _>>()?;
            let marks = (1..=width)
                .map(|n| format!("?{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            for row in rows {
                to.execute(
                    &format!("INSERT INTO \"{table}\" VALUES ({marks})"),
                    rusqlite::params_from_iter(row),
                )?;
            }
        }
        let encoding: String = to.query_row("PRAGMA encoding", [], |row| row.get(0))?;
        assert_eq!(encoding, "UTF-16le");
        let integrity: String = to.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        assert_eq!(integrity, "ok");
        for name in ["drafts", "events", "job_controls"] {
            assert_eq!(
                rr7_statement(&target, name)?,
                rr7_statement(&source, name)?,
                "{name}: the same statement"
            );
        }
    }
    Ok(std::fs::read(&target)?)
}

/// A backup set of a notes workspace (with a cancelled job and its control when `job`) whose
/// workspace archive's records are replaced by `forge(archive bytes)`, restored by replacement
/// over the deleted directory.
async fn rr7_replace(
    job: bool,
    forge: &dyn Fn(&[u8]) -> Fallible<Vec<u8>>,
) -> Fallible<(tempfile::TempDir, tempfile::TempDir, StorageScope, Restored)> {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let workspace = {
        let storage = Storage::open(directory.path())?;
        let workspace = notes_workspace(&storage).await?;
        if job {
            let records = storage.records();
            let job_scope = JobScope::Workspace(workspace.clone());
            let created = records
                .create_job(
                    &job_scope,
                    NewJob {
                        mutation_id: uuid!(61)?,
                        initiator: person("ana"),
                        spec: JobSpec::BackupWorkspace,
                    },
                )
                .await?;
            records
                .cancel_job(&job_scope, uuid!(62)?, created.id)
                .await?;
        }
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        workspace
    };
    let (installation, archive) = set_of(output.path(), &workspace);
    let bytes = std::fs::read(&archive)?;
    let forged = forge(&bytes)?;
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|name| (name == "records.sqlite").then(|| forged.clone()),
        &[],
    )?;
    std::fs::write(&archive, crafted)?;
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let result = restored
        .backups()
        .restore_replacement(&installation, &[archive])
        .await;
    drop(restored);
    Ok((directory, output, workspace, result))
}

/// A forged replacement is refused as `InvalidInput` naming `table`, before any workspace row
/// or repository was written.
async fn rr7_replace_refused(
    job: bool,
    forge: &dyn Fn(&[u8]) -> Fallible<Vec<u8>>,
    table: &str,
) -> TestResult {
    let (directory, _output, workspace, result) = rr7_replace(job, forge).await?;
    if let Err(error) = &result {
        assert!(error.message.contains(table), "{table}: {error}");
    }
    rr5_refused_before_writing(directory.path(), &workspace, result, table)
}

/// A forged replacement restores as any other: the workspace and its draft are live.
async fn rr7_replace_restores(forge: &dyn Fn(&[u8]) -> Fallible<Vec<u8>>) -> TestResult {
    let (directory, _output, workspace, result) = rr7_replace(false, forge).await?;
    result?;
    assert_eq!(
        count(
            &directory.path().join("records.sqlite"),
            &format!(
                "SELECT count(*) FROM drafts WHERE workspace_id = '{}'",
                workspace.workspace_id.0
            )
        )?,
        1
    );
    Ok(())
}

/// A notes workspace's archive whose records are replaced by `forge(archive bytes)`, imported
/// into a blank workspace of another tenant: refused as `InvalidInput` naming `table`, nothing
/// written.
async fn rr7_import_refused(forge: &dyn Fn(&[u8]) -> Fallible<Vec<u8>>, table: &str) -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let forged = forge(&bytes)?;
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|name| (name == "records.sqlite").then(|| forged.clone()),
        &[],
    )?;
    let outcome = import_crafted(&storage, crafted).await?;
    assert_import_refused(&storage, outcome, table).await
}

/// An import of a notes workspace's archive whose records are `forge(archive bytes)`: refused as
/// `InvalidInput` with the target's head still blank, naming what was written otherwise.
async fn rr7_import_reported(forge: &dyn Fn(&[u8]) -> Fallible<Vec<u8>>) -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let forged = forge(&bytes)?;
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|name| (name == "records.sqlite").then(|| forged.clone()),
        &[],
    )?;
    let (target, blank, result) = import_crafted(&storage, crafted).await?;
    let moved = storage.versions().head(&target).await? != blank;
    let pending = count(
        &directory.path().join("records.sqlite"),
        &format!(
            "SELECT count(*) FROM pending_puts WHERE workspace_id = '{}'",
            target.workspace_id.0
        ),
    )?;
    match result {
        Ok(report) => Err(format!("imported: {report:?}").into()),
        Err(error) => {
            // The one pending put is the crafted archive this test uploaded, not the restore's.
            assert!(
                error.code == ErrorCode::InvalidInput && !moved && pending == 1,
                "refused as {:?} ({}); the target's head moved: {moved}; its pending puts: \
                 {pending}",
                error.code,
                error.message
            );
            Ok(())
        }
    }
}

// N-4: each property changed alone, in place, every other byte of the statement this build's.

/// R7SPACED: a statement that differs from this build's only in whitespace is this build's.
#[tokio::test]
async fn rr7_control_a_whitespace_only_rewrite_restores() -> TestResult {
    rr7_replace_restores(&|bytes| rr7_rewritten(bytes, "drafts", "\n", "\n \t  ")).await
}

#[tokio::test]
async fn rr7_replace_column_collation_alone() -> TestResult {
    rr7_replace_refused(
        false,
        &|bytes| {
            rr7_rewritten(
                bytes,
                "drafts",
                "workspace_id TEXT NOT NULL,",
                "workspace_id TEXT NOT NULL COLLATE NOCASE,",
            )
        },
        "drafts",
    )
    .await
}

#[tokio::test]
async fn rr7_import_column_collation_alone() -> TestResult {
    rr7_import_refused(
        &|bytes| {
            rr7_rewritten(
                bytes,
                "drafts",
                "workspace_id TEXT NOT NULL,",
                "workspace_id TEXT NOT NULL COLLATE NOCASE,",
            )
        },
        "drafts",
    )
    .await
}

#[tokio::test]
async fn rr7_replace_strict_dropped_alone() -> TestResult {
    rr7_replace_refused(
        false,
        &|bytes| rr7_rewritten(bytes, "drafts", ") STRICT", ")"),
        "drafts",
    )
    .await
}

#[tokio::test]
async fn rr7_import_strict_dropped_alone() -> TestResult {
    rr7_import_refused(
        &|bytes| rr7_rewritten(bytes, "drafts", ") STRICT", ")"),
        "drafts",
    )
    .await
}

#[tokio::test]
async fn rr7_replace_check_added_alone() -> TestResult {
    rr7_replace_refused(
        false,
        &|bytes| {
            rr7_rewritten(
                bytes,
                "drafts",
                "body TEXT NOT NULL,",
                "body TEXT NOT NULL CHECK (length(body) >= 0),",
            )
        },
        "drafts",
    )
    .await
}

#[tokio::test]
async fn rr7_replace_check_removed_alone() -> TestResult {
    rr7_replace_refused(
        true,
        &|bytes| {
            rr7_rewritten(
                bytes,
                "job_controls",
                " CHECK (action IN ('cancel', 'retry'))",
                "",
            )
        },
        "job_controls",
    )
    .await
}

#[tokio::test]
async fn rr7_import_check_removed_alone() -> TestResult {
    rr7_import_refused(
        &|bytes| {
            rr7_rewritten(
                bytes,
                "job_controls",
                " CHECK (action IN ('cancel', 'retry'))",
                "",
            )
        },
        "job_controls",
    )
    .await
}

#[tokio::test]
async fn rr7_replace_references_removed_alone() -> TestResult {
    rr7_replace_refused(
        true,
        &|bytes| rr7_rewritten(bytes, "job_outputs", " REFERENCES jobs (job_id)", ""),
        "job_outputs",
    )
    .await
}

#[tokio::test]
async fn rr7_import_references_removed_alone() -> TestResult {
    rr7_import_refused(
        &|bytes| rr7_rewritten(bytes, "job_outputs", " REFERENCES jobs (job_id)", ""),
        "job_outputs",
    )
    .await
}

#[tokio::test]
async fn rr7_replace_index_collation_alone() -> TestResult {
    rr7_replace_refused(
        false,
        &|bytes| {
            rr7_rewritten(
                bytes,
                "events_by_log",
                "(tenant_id,",
                "(tenant_id COLLATE NOCASE,",
            )
        },
        "events",
    )
    .await
}

#[tokio::test]
async fn rr7_import_index_collation_alone() -> TestResult {
    rr7_import_refused(
        &|bytes| {
            rr7_rewritten(
                bytes,
                "events_by_log",
                "(tenant_id,",
                "(tenant_id COLLATE NOCASE,",
            )
        },
        "events",
    )
    .await
}

#[tokio::test]
async fn rr7_replace_partial_index_alone() -> TestResult {
    rr7_replace_refused(
        false,
        &|bytes| {
            rr7_rewritten(
                bytes,
                "jobs_by_state",
                "ON jobs (state)",
                "ON jobs (state) WHERE state <> 'rr7'",
            )
        },
        "jobs",
    )
    .await
}

#[tokio::test]
async fn rr7_replace_missing_index() -> TestResult {
    rr7_replace_refused(
        false,
        &|bytes| rr7_forged_steps(bytes, &["DROP INDEX reviews_by_item;"]),
        "reviews",
    )
    .await
}

#[tokio::test]
async fn rr7_import_missing_index() -> TestResult {
    rr7_import_refused(
        &|bytes| rr7_forged_steps(bytes, &["DROP INDEX reviews_by_item;"]),
        "reviews",
    )
    .await
}

#[tokio::test]
async fn rr7_replace_keyword_case_game() -> TestResult {
    rr7_replace_refused(
        false,
        &|bytes| rr7_rewritten(bytes, "drafts", "NOT NULL", "not null"),
        "drafts",
    )
    .await
}

#[tokio::test]
async fn rr7_replace_quoted_table_name() -> TestResult {
    rr7_replace_refused(
        false,
        &|bytes| {
            rr7_rewritten(
                bytes,
                "drafts",
                "CREATE TABLE drafts",
                "CREATE TABLE \"drafts\"",
            )
        },
        "drafts",
    )
    .await
}

#[tokio::test]
async fn rr7_import_bracketed_table_name() -> TestResult {
    rr7_import_refused(
        &|bytes| {
            rr7_rewritten(
                bytes,
                "drafts",
                "CREATE TABLE drafts",
                "CREATE TABLE [drafts]",
            )
        },
        "drafts",
    )
    .await
}

/// An extra table and a view under other names: never read, so the restore goes ahead, and the
/// extra table does not appear in the live database.
#[tokio::test]
async fn rr7_an_extra_table_and_view_are_never_read() -> TestResult {
    let (directory, _output, _workspace, result) =
        rr7_replace(false, &|bytes| {
            rr7_forged_steps(
            bytes,
            &["CREATE TABLE rr7_extra (workspace_id TEXT); INSERT INTO rr7_extra VALUES ('WS99');
               CREATE VIEW rr7_view AS SELECT tenant_id, 'WS99' AS workspace_id FROM drafts;"],
        )
        })
        .await?;
    result?;
    assert_eq!(
        count(
            &directory.path().join("records.sqlite"),
            "SELECT count(*) FROM sqlite_schema WHERE name IN ('rr7_extra', 'rr7_view')"
        )?,
        0
    );
    Ok(())
}

/// N-4 control: a rename rebuild with nothing changed is refused. SQLite stores the renamed
/// table's statement as `CREATE TABLE "drafts" (...`, and a quoted name is not whitespace, so
/// the statement is not this build's text: the in-place quoted name's case
/// (`rr7_replace_quoted_table_name`). No archive this build writes is renamed after it is built;
/// the one rename of the schema (`keyed_events` to `events`, schema 2) runs in the migrations
/// every records database goes through, live and archived alike, so both store the same text.
#[tokio::test]
async fn rr7_control_a_rename_rebuild_with_no_change_is_refused() -> TestResult {
    rr7_replace_refused(
        false,
        &|bytes| {
            rr7_forged_steps(
                bytes,
                &[&format!(
                    "CREATE TABLE drafts_new {};
                     INSERT INTO drafts_new SELECT * FROM drafts;
                     DROP TABLE drafts;
                     ALTER TABLE drafts_new RENAME TO drafts;",
                    rr5_definition(RR5_COLUMNS, " STRICT")
                )],
            )
        },
        "drafts",
    )
    .await
}

/// N-4 control: the same rebuild with the rename's quotes taken off (`rr5_drafts_as`) is this
/// build's statement up to whitespace, and restores; so each `rr5_` variant differs from this
/// build's schema in its one property only.
#[tokio::test]
async fn rr5_control_a_rebuild_with_no_change_restores() -> TestResult {
    rr7_replace_restores(&|bytes| {
        rr7_forged_steps(
            bytes,
            &[&rr5_drafts_as(
                &rr5_definition(RR5_COLUMNS, " STRICT"),
                "SELECT * FROM drafts",
            )],
        )
    })
    .await
}

/// N-1: records in UTF-16, with this build's statements and rows, intact and migrated, are
/// refused before anything is written: the copy attaches them to the UTF-8 live database, which
/// SQLite refuses for another encoding.
#[tokio::test]
async fn rr7_replace_utf16_records() -> TestResult {
    rr7_replace_refused(false, &rr7_utf16, "UTF-8").await
}

#[tokio::test]
async fn rr7_import_utf16_records() -> TestResult {
    rr7_import_reported(&rr7_utf16).await
}

/// `drafts` made loose, a draft's body stored as a BLOB, then its statement put back byte for
/// byte: a STRICT table holding a value of another type, every statement this build's.
#[tokio::test]
async fn rr7_a_blob_in_a_strict_table_under_its_own_statement_is_refused_before_any_write()
-> TestResult {
    rr7_replace_refused(
        false,
        &|bytes| {
            let forged = rr7_forged_steps(
                bytes,
                &[
                    "PRAGMA writable_schema = ON;
                     UPDATE sqlite_schema SET sql = replace(sql, ') STRICT', ')') WHERE name = 'drafts';
                     PRAGMA writable_schema = OFF;",
                    "UPDATE drafts SET body = CAST(body AS BLOB);",
                    "PRAGMA writable_schema = ON;
                     UPDATE sqlite_schema SET sql = sql || ' STRICT' WHERE name = 'drafts';
                     PRAGMA writable_schema = OFF;",
                ],
            )?;
            let staged = tempfile::tempdir()?;
            let path = staged.path().join("f.sqlite");
            std::fs::write(&path, &forged)?;
            let original = staged.path().join("o.sqlite");
            std::fs::write(&original, entry_bytes_of(bytes, "records.sqlite")?)?;
            assert_eq!(
                rr7_statement(&path, "drafts")?,
                rr7_statement(&original, "drafts")?
            );
            assert_eq!(
                count(&path, "SELECT count(*) FROM drafts WHERE typeof(body) = 'blob'")?,
                1
            );
            Ok(forged)
        },
        "drafts",
    )
    .await
}

/// `jobs` made loose, a job of the archive's own workspace with no id added, a control of a job
/// the archive does not hold added, then `jobs`' statement put back byte for byte. The job-keyed
/// row check reads `job_id NOT IN (SELECT job_id FROM jobs)`, which a NULL id makes unknown.
#[tokio::test]
async fn rr7_a_job_without_an_id_is_refused_before_any_write() -> TestResult {
    rr7_replace_refused(
        true,
        &|bytes| {
            rr7_forged_steps(
                bytes,
                &[
                    "PRAGMA writable_schema = ON;
                     UPDATE sqlite_schema SET sql = replace(sql, ') STRICT', ')') WHERE name = 'jobs';
                     PRAGMA writable_schema = OFF;",
                    "INSERT INTO jobs (job_id, tenant_id, workspace_id, mutation_id, initiator, spec,
                         record, state)
                         SELECT NULL, tenant_id, workspace_id, 'rr7-null-job', '{}', '{}', '{}',
                         'cancelled' FROM workspaces;
                     INSERT INTO job_controls VALUES ('rr7-control', 'rr7-elsewhere-job', 'cancel');",
                    "PRAGMA writable_schema = ON;
                     UPDATE sqlite_schema SET sql = sql || ' STRICT' WHERE name = 'jobs';
                     PRAGMA writable_schema = OFF;",
                ],
            )
        },
        "jobs",
    )
    .await
}

/// N-2: a draft whose stored `record` is not JSON (an import rewrites the workspace identity
/// inside it) is refused before anything is written.
#[tokio::test]
async fn rr7_an_import_of_a_draft_whose_record_is_not_json_is_refused_before_any_write()
-> TestResult {
    rr7_import_reported(&|bytes| {
        rr7_forged_steps(bytes, &["UPDATE drafts SET record = 'rr7 not json';"])
    })
    .await
}

/// N-2, by replacement: the application reads every `record` as JSON, so one that is not is
/// refused before anything is written.
#[tokio::test]
async fn backup_a_replacement_refuses_a_record_that_is_not_json() -> TestResult {
    rr7_replace_refused(
        false,
        &|bytes| rr7_forged_steps(bytes, &["UPDATE drafts SET record = 'rr7 not json';"]),
        "drafts",
    )
    .await
}

/// R7QUICK: an index whose entry count matches its table but whose content does not. The event
/// row is the archive's own while `events_by_log` is built, then the index is made partial (so
/// an update leaves its entry alone), the row is moved to another workspace, and the statement
/// is put back. `quick_check` counts entries; only `integrity_check` compares their content. A
/// row check that counts through the covering index would see the old workspace.
#[tokio::test]
async fn rr7_an_index_that_lies_about_its_rows_is_refused_before_any_write() -> TestResult {
    let (directory, _output, workspace, result) = rr7_replace(false, &|bytes| {
        rr7_forged_steps(
            bytes,
            &[
                "INSERT INTO events (tenant_id, workspace_id, event_key, record)
                     SELECT tenant_id, workspace_id, 'rr7-lying-index', '{}' FROM workspaces;",
                "PRAGMA writable_schema = ON;
                 UPDATE sqlite_schema SET sql = sql || ' WHERE seq < 0'
                     WHERE name = 'events_by_log';
                 PRAGMA writable_schema = OFF;",
                "UPDATE events SET workspace_id = 'WS99' WHERE event_key = 'rr7-lying-index';",
                "PRAGMA writable_schema = ON;
                 UPDATE sqlite_schema SET sql = replace(sql, ' WHERE seq < 0', '')
                     WHERE name = 'events_by_log';
                 PRAGMA writable_schema = OFF;",
            ],
        )
    })
    .await?;
    let foreign = count(
        &directory.path().join("records.sqlite"),
        &format!(
            "SELECT count(*) FROM events WHERE workspace_id = '{}'",
            uuid_text(99)
        ),
    )?;
    if let Err(error) = &result {
        assert!(error.message.contains("damaged"), "{error}");
    }
    assert_eq!(foreign, 0, "an event of another workspace was restored");
    rr5_refused_before_writing(directory.path(), &workspace, result, "lying index")
}

// B-1: a reference to a stored object is read structurally, never as a token of free text.

/// A conversion record naming `structured` and `asset`, with `locations` line locations, whose
/// one converted table has one cell of text `cell` (a document's own words).
fn rr7_record_with_cell(
    structured: &Digest,
    asset: &Digest,
    cell: &str,
    locations: usize,
) -> Fallible<Vec<u8>> {
    let table = format!(
        "\"tables\":[{{\"lines\":{{\"start\":1,\"end\":1}},\"location\":{{\"page_no\":1}},\
         \"num_rows\":1,\"num_cols\":1,\"cells\":[{{\"row\":0,\"column\":0,\"row_span\":1,\
         \"column_span\":1,\"text\":\"{cell}\",\"column_header\":false,\"row_header\":false}}]}}]"
    );
    let text = String::from_utf8(rr4_record(structured, asset, locations))?;
    if !text.contains("\"tables\":[]") {
        return Err("the record has no empty tables list".into());
    }
    Ok(text.replacen("\"tables\":[]", &table, 1).into_bytes())
}

/// Workspace B holds an original no other workspace holds (`hidden`); workspace A converts a
/// document of its own whose one table cell reads as `hidden`'s digest (its record, of
/// `locations` locations, named by A's card). Returns (storage dir, storage, A, B, hidden,
/// record size).
async fn rr7_a_record_quoting(
    locations: usize,
) -> Fallible<(
    tempfile::TempDir,
    Storage,
    StorageScope,
    StorageScope,
    Digest,
    usize,
)> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (a, a_head) = create(&storage, "local", 1, "A").await?;
    let (b, b_head) = create(&storage, "local", 2, "B").await?;
    let hidden = put(&storage, &b, b"rr7 an original only workspace B holds")
        .await?
        .digest;
    commit(&storage, &b, 3, &b_head, vec![card(uuid!(15)?, &hidden)?]).await?;
    let source = put(&storage, &a, b"rr7 A's spreadsheet").await?.digest;
    let export = put(&storage, &a, b"{\"rr7\": \"A's docling export\"}")
        .await?
        .digest;
    let asset = put(&storage, &a, b"rr7 A's page render").await?.digest;
    let bytes = rr7_record_with_cell(&export, &asset, hidden.as_str(), locations)?;
    let size = bytes.len();
    let record = rr4_put_large(&storage, &a, bytes).await?;
    commit(
        &storage,
        &a,
        4,
        &a_head,
        vec![rr4_converted_card(
            uuid!(14)?,
            "sources/sheet.md",
            &source,
            &record,
        )?],
    )
    .await?;
    Ok((directory, storage, a, b, hidden, size))
}

/// Whether A's archive copies `hidden`, then whether B's purge keeps it.
async fn rr7_copied_and_kept(
    directory: &Path,
    storage: &Storage,
    a: &StorageScope,
    b: &StorageScope,
    hidden: &Digest,
) -> Fallible<(bool, bool)> {
    let archive = storage
        .backups()
        .write_workspace_archive(a, uuid!(40)?)
        .await?
        .object;
    let copied = entries(&object_file(directory, &archive.digest)?)?
        .contains(&format!("objects/{}", hidden.as_str()));
    rr5_purge(storage, b).await?;
    let kept = object_file(directory, hidden)?.is_file();
    Ok((copied, kept))
}

/// B-1: a small conversion record is read structurally, as a large one is: a table cell quoting
/// B's digest neither copies B's original into A's archive nor keeps it past B's purge.
#[tokio::test]
async fn rr7_a_small_records_text_copies_and_keeps_nothing() -> TestResult {
    let (directory, storage, a, b, hidden, size) = rr7_a_record_quoting(3).await?;
    let (copied, kept) = rr7_copied_and_kept(directory.path(), &storage, &a, &b, &hidden).await?;
    assert!(
        !copied && !kept,
        "a {size}-byte record whose table cell quotes B's digest: copied into A's archive: \
         {copied}; kept past B's purge: {kept}"
    );
    Ok(())
}

/// The same record past 64 MiB: its cell names nothing either. The control for the probe above.
#[tokio::test]
async fn rr7_control_a_large_records_text_copies_and_keeps_nothing() -> TestResult {
    let (directory, storage, a, b, hidden, size) = rr7_a_record_quoting(1_500_000).await?;
    assert!(size > 64 << 20, "the record is {size} bytes");
    let (copied, kept) = rr7_copied_and_kept(directory.path(), &storage, &a, &b, &hidden).await?;
    assert!(
        !copied && !kept,
        "a {size}-byte record: copied {copied}, kept {kept}"
    );
    Ok(())
}

/// A source card over `object` at `at`, whose supplied file name is `filename`.
fn rr7_card_named(item: ItemId, at: &str, object: &Digest, filename: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::WriteSourceCard(Box::new(SourceCard {
        item_id: item,
        path: WorkspacePath::try_from(at.to_owned())?,
        title: filename.to_owned(),
        type_name: "Source".to_owned(),
        body: String::new(),
        properties: BTreeMap::new(),
        appearance: serde_json::from_value(json!({
            "object": object.as_str(),
            "names": [{
                "filename": filename,
                "folder": "",
                "observed_at": "2026-10-09T12:00:00.000Z",
                "supplied_by": "ana"
            }],
            "media_type": "text/csv",
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

/// B-1, through a card: A uploads a file of its own named after B's digest. The card records
/// the supplied name, which names no object, so A's archive does not copy B's original and B's
/// purge deletes it.
#[tokio::test]
async fn rr7_a_cards_file_name_copies_and_keeps_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (a, a_head) = create(&storage, "local", 1, "A").await?;
    let (b, b_head) = create(&storage, "local", 2, "B").await?;
    let hidden = put(&storage, &b, b"rr7 an original only workspace B holds")
        .await?
        .digest;
    commit(&storage, &b, 3, &b_head, vec![card(uuid!(15)?, &hidden)?]).await?;
    let own = put(&storage, &a, b"rr7 A's own small file").await?.digest;
    commit(
        &storage,
        &a,
        4,
        &a_head,
        vec![rr7_card_named(
            uuid!(14)?,
            "sources/named.md",
            &own,
            &format!("{}.csv", hidden.as_str()),
        )?],
    )
    .await?;
    let (copied, kept) = rr7_copied_and_kept(directory.path(), &storage, &a, &b, &hidden).await?;
    assert!(
        !copied && !kept,
        "A's card whose file name is B's digest: copied into A's archive: {copied}; kept past \
         B's purge: {kept}"
    );
    Ok(())
}

/// B-1, when a commit lands its pending puts: a card lands only the puts of the objects it names
/// structurally. Bytes a workspace put and named only in a card's file name keep their pending
/// put, so a purge of the workspace deletes them: a token never lands a put that no record would
/// then name (which no purge would reach).
#[tokio::test]
async fn backup_a_put_a_card_names_only_in_its_text_goes_with_its_workspace() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (a, head) = create(&storage, "local", 1, "A").await?;
    let own = put(&storage, &a, b"rr8 A's original").await?.digest;
    let typed = put(
        &storage,
        &a,
        b"rr8 bytes A put and named only in a file name",
    )
    .await?
    .digest;
    commit(
        &storage,
        &a,
        3,
        &head,
        vec![rr7_card_named(
            uuid!(14)?,
            "sources/named.md",
            &own,
            &format!("{}.csv", typed.as_str()),
        )?],
    )
    .await?;
    assert_eq!(
        rr6_pending(directory.path(), &a, &[&own, &typed])?,
        1,
        "only the original's put landed"
    );
    rr5_purge(&storage, &a).await?;
    for (digest, what) in [(&own, "the original"), (&typed, "the named-in-text bytes")] {
        assert!(
            !object_file(directory.path(), digest)?.is_file(),
            "the purge left {what}"
        );
    }
    Ok(())
}

/// Commit on `scope`'s accepted head, straight through Git, the head's tree with the file `path`
/// holding `bytes`: a file only a damaged store (or a crafted archive) holds.
fn rr8_commit_raw_file(
    storage: &Storage,
    scope: &StorageScope,
    path: &str,
    bytes: &[u8],
) -> TestResult {
    let repository = repository_of(storage, scope)?;
    let head = repository
        .find_reference("refs/heads/main")?
        .peel_to_commit()?;
    let blob = repository.blob(bytes)?;
    let tree = git2::build::TreeUpdateBuilder::new()
        .upsert(path, blob, git2::FileMode::Blob)
        .create_updated(&repository, &head.tree()?)?;
    let tree = repository.find_tree(tree)?;
    let who = git2::Signature::now("rr8", "rr8")?;
    repository.commit(
        Some("refs/heads/main"),
        &who,
        &who,
        "rr8 a file written past the store",
        &tree,
        &[&head],
    )?;
    Ok(())
}

/// Purge the workspace `target` as `rr5_purge` does (purge 61 under mutations 60 and 62), and
/// give back what the purger answered.
async fn rr8_purge(
    storage: &Storage,
    target: &StorageScope,
) -> Fallible<Result<okf_jawn_contract::purge::PurgeReport, ApiError>> {
    let tenant = target.tenant_id.clone();
    let purge = storage
        .records()
        .create_purge(
            &tenant,
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
    Ok(storage
        .purger()
        .purge_workspace(&tenant, uuid!(62)?, purge.id, target.workspace_id)
        .await)
}

/// B-1: a source appearance that does not read as one (here its `object` is not a digest) is
/// damage (only the application writes them). A backup of its workspace fails naming it, and so
/// does a purge of another workspace, which reads it as a survivor's and names the damaged
/// workspace too (round 4, N-10): neither guesses what it names, from tokens or otherwise.
#[tokio::test]
async fn backup_a_source_appearance_that_does_not_read_fails_a_backup_and_a_purge() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (damaged, _) = create(&storage, "local", 1, "Damaged").await?;
    let (other, _) = create(&storage, "local", 2, "Other").await?;
    let path = format!(".okf/sources/{}.json", uuid_text(15));
    rr8_commit_raw_file(
        &storage,
        &damaged,
        &path,
        b"{\"object\": \"not a digest\", \"names\": []}",
    )?;
    let error = err_of(
        storage
            .backups()
            .write_workspace_archive(&damaged, uuid!(40)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Internal, "{error}");
    assert!(error.message.contains(&path), "{error}");
    let error = err_of(rr8_purge(&storage, &other).await?)?;
    assert_eq!(error.code, ErrorCode::Internal, "{error}");
    assert!(
        error.message.contains(&path)
            && error.message.contains(&damaged.workspace_id.0.to_string()),
        "names the path and the damaged workspace: {error}"
    );
    Ok(())
}

/// B-1: a conversion record a card names that does not read as one is damage (only ingest
/// writes them). A backup fails naming it, and so does a purge of another workspace, which
/// would otherwise not know what the record's lists name past the damage.
#[tokio::test]
async fn backup_a_card_named_record_that_does_not_read_fails_a_backup_and_a_purge() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (damaged, head) = create(&storage, "local", 1, "Damaged").await?;
    let (other, _) = create(&storage, "local", 2, "Other").await?;
    let original = put(&storage, &damaged, b"rr8 a converted original")
        .await?
        .digest;
    let record = put(&storage, &damaged, b"{\"structured\": [{\"digest\": \"cut")
        .await?
        .digest;
    commit(
        &storage,
        &damaged,
        3,
        &head,
        vec![rr4_converted_card(
            uuid!(14)?,
            "sources/report.md",
            &original,
            &record,
        )?],
    )
    .await?;
    let error = err_of(
        storage
            .backups()
            .write_workspace_archive(&damaged, uuid!(40)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Internal, "{error}");
    assert!(
        error.message.contains(record.as_str()) && error.message.contains("does not read"),
        "{error}"
    );
    let error = err_of(rr8_purge(&storage, &other).await?)?;
    assert_eq!(error.code, ErrorCode::Internal, "{error}");
    assert!(
        error.message.contains(record.as_str())
            && error.message.contains(&damaged.workspace_id.0.to_string()),
        "names the record and the damaged workspace: {error}"
    );
    Ok(())
}

/// The names of the objects of tenant `tenant` the store at `data` holds.
fn rr8_stored(data: &Path, tenant: &str) -> Fallible<std::collections::BTreeSet<String>> {
    let mut found = std::collections::BTreeSet::new();
    let root = data.join("objects").join(tenant);
    for prefix in std::fs::read_dir(&root)? {
        let prefix = prefix?.path();
        if prefix.is_dir() {
            for object in std::fs::read_dir(&prefix)? {
                found.insert(object?.file_name().to_string_lossy().into_owned());
            }
        }
    }
    Ok(found)
}

/// The `objects/` entries of the workspace archive object `digest` in the store at `data`.
fn rr8_archived(data: &Path, digest: &Digest) -> Fallible<std::collections::BTreeSet<String>> {
    Ok(entries(&object_file(data, digest)?)?
        .into_iter()
        .filter_map(|name| name.strip_prefix("objects/").map(str::to_owned))
        .collect())
}

/// A conversion record as ingest writes it (core `ConversionRecord`, read back through its
/// type): one structured export, a page render and a picture whose caption holds no hex, and a
/// table whose one cell is plain text.
fn rr8_record(export: &Digest, render: &Digest, picture: &Digest) -> Fallible<Vec<u8>> {
    let page = json!({ "provenance": "direct", "locator": { "kind": "page", "page_no": 1 } });
    let record: okf_jawn_core::conversion::ConversionRecord = serde_json::from_value(json!({
        "converter": rr4_converter(),
        "outcome": { "status": "completed" },
        "page_count": 1,
        "structured": [{ "digest": export.as_str() }],
        "markdown": "8".repeat(64),
        "locations": [{ "lines": { "start": 1, "end": 2 }, "location": page }],
        "outline": [],
        "tables": [{
            "lines": { "start": 2, "end": 2 },
            "location": page,
            "num_rows": 1,
            "num_cols": 1,
            "cells": [{
                "row": 0, "column": 0, "row_span": 1, "column_span": 1,
                "text": "harbour depth", "column_header": false, "row_header": false
            }]
        }],
        "assets": [
            {
                "digest": render.as_str(),
                "media_type": "image/png",
                "role": "page_image",
                "location": page
            },
            {
                "digest": picture.as_str(),
                "media_type": "image/png",
                "role": "picture",
                "location": page,
                "caption": "A map of the harbour"
            }
        ],
        "warnings": []
    }))?;
    Ok(serde_json::to_vec(&record)?)
}

/// A converted source in `rich` (created at `created`) as an import job writes it: the job is
/// claimed, the original, structured export, page render, captioned picture and conversion
/// record are put, the card naming the record is committed, and the job completes with its
/// outputs. Then a correction and supplied text, which name the record only as a key. Returns
/// the card's item, the card's revision and (original, record, export, render, picture).
async fn rr8_converted(
    storage: &Storage,
    rich: &StorageScope,
    created: &Revision,
) -> Fallible<(ItemId, Revision, [Digest; 5])> {
    let records = storage.records();
    let import = records
        .create_job(
            &JobScope::Workspace(rich.clone()),
            NewJob {
                mutation_id: uuid!(30)?,
                initiator: person("ana"),
                spec: JobSpec::Import {
                    base_revision: created.clone(),
                    upload_ids: Vec::new(),
                    destination: None,
                    apply_naming_rules: false,
                    settings: ConversionSettings::default(),
                },
            },
        )
        .await?;
    let claimed = some(records.claim_job(import.id).await?, "the claimed import")?;
    let original = put(storage, rich, b"rr8 rich original").await?.digest;
    let export = put(storage, rich, b"{\"rr8\": \"docling export\"}")
        .await?
        .digest;
    let render = put(storage, rich, b"rr8 page render").await?.digest;
    let picture = put(storage, rich, b"rr8 harbour picture").await?.digest;
    let record = put(storage, rich, &rr8_record(&export, &render, &picture)?)
        .await?
        .digest;
    let item: ItemId = uuid!(14)?;
    let revision = commit(
        storage,
        rich,
        31,
        created,
        vec![rr4_converted_card(
            item,
            "sources/report.md",
            &original,
            &record,
        )?],
    )
    .await?;
    records
        .complete_job(JobCompletion {
            lease: claimed.lease,
            revision: Some(revision.clone()),
            item_ids: vec![item],
            artifact: None,
            restore: None,
            backup: None,
            outputs: vec![
                export.clone(),
                render.clone(),
                picture.clone(),
                record.clone(),
            ],
            warnings: Vec::new(),
        })
        .await?;
    let corrected = commit(
        storage,
        rich,
        32,
        &revision,
        vec![TreeEdit::CorrectDigest {
            item_id: item,
            digest: record.clone(),
            corrected_markdown: "Corrected harbour text.".to_owned(),
            adopts: None,
        }],
    )
    .await?;
    commit(
        storage,
        rich,
        33,
        &corrected,
        vec![TreeEdit::SupplyExtraction {
            item_id: item,
            based_on: Some(record.clone()),
            pages: Vec::new(),
            markdown: "Supplied harbour text.".to_owned(),
            proposal_id: uuid!(34)?,
            supplier: person("agent"),
        }],
    )
    .await?;
    Ok((item, revision, [original, record, export, render, picture]))
}

/// The records of `rich` that name objects, as the application writes them: an export artifact,
/// a derived dataset of `item` at `revision`, and a completed upload. Returns (export artifact,
/// dataset, upload).
async fn rr8_rows(
    storage: &Storage,
    rich: &StorageScope,
    item: ItemId,
    revision: &Revision,
) -> Fallible<[Digest; 3]> {
    let records = storage.records();
    let scope = JobScope::Workspace(rich.clone());
    let exported = put(storage, rich, b"rr8 an export archive").await?;
    let job = records
        .create_job(
            &scope,
            NewJob {
                mutation_id: uuid!(35)?,
                initiator: person("ana"),
                spec: JobSpec::BackupWorkspace,
            },
        )
        .await?;
    records
        .record_artifact(
            &scope,
            uuid!(36)?,
            NewArtifact {
                kind: ArtifactKind::Export,
                object: exported.clone(),
                media_type: "application/zip".to_owned(),
                created_by_job: job.id,
            },
        )
        .await?;
    let dataset = put(storage, rich, b"rr8 harbour dataset").await?.digest;
    records
        .record_derived_object(
            rich,
            DerivedObject {
                item_id: item,
                revision: revision.clone(),
                digest: dataset.clone(),
                kind: DerivedKind::Dataset,
                media_type: "text/csv".to_owned(),
            },
        )
        .await?;
    let upload = rr6_uploaded(storage, rich, b"rr8 an uploaded note").await?;
    Ok([exported.digest, dataset, upload])
}

/// B-1 false-delete guard. A workspace built through the store calls the application makes (an
/// import job's original, conversion record, structured export, page render and captioned
/// picture, then its completion; a correction and supplied text; an export artifact; a derived
/// dataset; a completed upload) references structurally every object the store holds for it:
/// its archive, which copies exactly that set, holds every one, so no object is reached only by
/// a token.
///
/// Round 4 (N-15): the workspace is built in another tenant and its archive imported into
/// `local`, the way a survivor comes to hold a conversion record whose exports and assets no
/// `job_outputs` row names (an import copies no job row), so only following the record keeps
/// them. A purge of another `local` workspace that names each of the eight objects as its own
/// originals deletes none.
#[tokio::test]
async fn backup_every_object_a_rich_workspace_uses_is_structural_and_survives_another_purge()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (rich, created) = create(&storage, "elsewhere", 1, "Rich").await?;
    let (item, revision, converted) = rr8_converted(&storage, &rich, &created).await?;
    let rows = rr8_rows(&storage, &rich, item, &revision).await?;
    // The archive copies exactly the structural set; every object the store holds is in it.
    let archive = storage
        .backups()
        .write_workspace_archive(&rich, uuid!(40)?)
        .await?
        .object;
    let archived = rr9_archived(directory.path(), "elsewhere", &archive.digest)?;
    let mut stored = rr8_stored(directory.path(), "elsewhere")?;
    stored.remove(archive.digest.as_str());
    assert_eq!(
        archived, stored,
        "every object the store holds is named structurally"
    );
    for digest in converted.iter().chain(&rows) {
        assert!(archived.contains(digest.as_str()), "{digest:?} is archived");
    }
    assert_eq!(
        archived.len(),
        8,
        "the five converted objects and the three rows'"
    );
    // Imported into `local`, the workspace holds the eight objects through its history and its
    // rows alone: no job row came with it, so the record's export, render and picture are kept
    // only by following the record its card names.
    let (restored, _) = create(&storage, "local", 2, "Restored").await?;
    let archive_bytes = std::fs::read(rr9_object_file(
        directory.path(),
        "elsewhere",
        &archive.digest,
    )?)?;
    let uploaded = storage
        .blobs()
        .put(
            &restored,
            Box::pin(std::io::Cursor::new(archive_bytes)),
            1 << 30,
            None,
        )
        .await?
        .digest;
    storage
        .backups()
        .restore_import(&restored, uploaded, uuid!(84)?, vec!["ana".to_owned()])
        .await?;
    let live = directory.path().join("records.sqlite");
    assert_eq!(
        count(
            &live,
            &format!(
                "SELECT count(*) FROM job_outputs JOIN jobs USING (job_id)
                 WHERE jobs.workspace_id = '{}'",
                restored.workspace_id.0
            ),
        )?,
        0,
        "an import copies no job row, so no output names the record's exports or assets"
    );
    for name in &archived {
        assert!(
            object_file(directory.path(), &Digest::try_from(name.clone())?)?.is_file(),
            "{name} was put back by the import"
        );
    }
    // Another workspace of `local` names every one of them, as its own originals, and is
    // purged.
    let (other, other_head) = create(&storage, "local", 3, "Other").await?;
    let mut cards = Vec::new();
    for (n, name) in (100_u32..).zip(&archived) {
        let digest = Digest::try_from(name.clone())?;
        put(
            &storage,
            &other,
            &std::fs::read(object_file(directory.path(), &digest)?)?,
        )
        .await?;
        cards.push(rr7_card_named(
            uuid!(n)?,
            &format!("sources/copy-{n}.md"),
            &digest,
            "copy.bin",
        )?);
    }
    commit(&storage, &other, 50, &other_head, cards).await?;
    rr5_purge(&storage, &other).await?;
    for name in &archived {
        assert!(
            object_file(directory.path(), &Digest::try_from(name.clone())?)?.is_file(),
            "another workspace's purge deleted {name}, which the restored workspace uses"
        );
    }
    Ok(())
}

// B-2: a restore refuses, before any write, an archive whose history or rows name an object it
// does not hold.

/// The entry names of the archive `bytes`.
fn entries_of_bytes(bytes: &[u8]) -> Fallible<Vec<String>> {
    let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec()))?;
    Ok(archive.file_names().map(str::to_owned).collect())
}

/// The archive `bytes` without its entry `name`.
fn rr7_without(bytes: &[u8], name: &str) -> Fallible<Vec<u8>> {
    use std::io::Write as _;
    let mut source = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec()))?;
    let mut target = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let mut dropped = 0_u32;
    for index in 0..source.len() {
        let mut entry = source.by_index(index)?;
        let entry_name = entry.name().to_owned();
        if entry_name == name {
            dropped = dropped.saturating_add(1);
            continue;
        }
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        target.start_file(
            entry_name.as_str(),
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )?;
        target.write_all(&content)?;
    }
    assert_eq!(dropped, 1, "{name} dropped");
    Ok(target.finish()?.into_inner())
}

/// B-2: an uploaded archive whose card names, as its original, an object the archive does not
/// hold but another workspace of the importing tenant does (its digest is all the uploader
/// knows) is refused before anything is written. Otherwise the imported card would name the
/// other workspace's bytes, the target's own archive would copy them, and that workspace's
/// purge would keep them.
#[tokio::test]
async fn rr7_an_import_whose_card_names_an_object_it_does_not_hold_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (victim, victim_head) = create(&storage, "local", 2, "Victim").await?;
    let secret = b"rr7 bytes only the victim workspace holds";
    let hidden = put(&storage, &victim, secret).await?.digest;
    commit(
        &storage,
        &victim,
        3,
        &victim_head,
        vec![card(uuid!(15)?, &hidden)?],
    )
    .await?;
    // An archive whose card names the same digest; its copy of the bytes is then left out.
    let (source, source_head) = create(&storage, "staging", 4, "Source").await?;
    put(&storage, &source, secret).await?;
    commit(
        &storage,
        &source,
        5,
        &source_head,
        vec![card(uuid!(16)?, &hidden)?],
    )
    .await?;
    let written = storage
        .backups()
        .write_workspace_archive(&source, uuid!(40)?)
        .await?
        .object;
    let bytes = std::fs::read(
        directory
            .path()
            .join("objects")
            .join("staging")
            .join(some(written.digest.as_str().get(..2), "a digest")?)
            .join(written.digest.as_str()),
    )?;
    assert!(
        entries_of_bytes(&bytes)?.contains(&format!("objects/{}", hidden.as_str())),
        "the written archive holds the bytes its card names"
    );
    let crafted = rr7_without(&bytes, &format!("objects/{}", hidden.as_str()))?;
    let (target, blank) = create(&storage, "local", 81, "Blank").await?;
    let uploaded = storage
        .blobs()
        .put(
            &target,
            Box::pin(std::io::Cursor::new(crafted)),
            1 << 30,
            None,
        )
        .await?
        .digest;
    let result = storage
        .backups()
        .restore_import(&target, uploaded, uuid!(84)?, vec!["ana".to_owned()])
        .await;
    match result {
        Err(error) => {
            assert_eq!(error.code, ErrorCode::InvalidInput, "{error}");
            assert!(
                error.message.contains(hidden.as_str()) && error.message.contains("original"),
                "names the digest and what names it: {error}"
            );
            assert_eq!(storage.versions().head(&target).await?, blank);
            Ok(())
        }
        Ok(report) => {
            let archive = storage
                .backups()
                .write_workspace_archive(&target, uuid!(85)?)
                .await?
                .object;
            let copied = entries(&object_file(directory.path(), &archive.digest)?)?
                .contains(&format!("objects/{}", hidden.as_str()));
            rr5_purge(&storage, &victim).await?;
            let kept = object_file(directory.path(), &hidden)?.is_file();
            Err(format!(
                "imported ({report:?}) although the archive does not hold {}; the target's own \
                 archive then holds the victim's bytes: {copied}; the victim's purge kept them: \
                 {kept}",
                hidden.as_str()
            )
            .into())
        }
    }
}

/// B-2: an uploaded archive whose card-named conversion record names an asset the archive does
/// not hold is refused before anything is written, naming the asset.
#[tokio::test]
async fn backup_an_import_whose_record_names_an_asset_it_does_not_hold_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (source, head) = create(&storage, "local", 1, "Converted").await?;
    let original = put(&storage, &source, b"rr8 converted original")
        .await?
        .digest;
    let export = put(&storage, &source, b"{\"rr8\": \"export\"}")
        .await?
        .digest;
    let asset = put(&storage, &source, b"rr8 page render").await?.digest;
    let record = put(&storage, &source, &rr4_record(&export, &asset, 3))
        .await?
        .digest;
    commit(
        &storage,
        &source,
        3,
        &head,
        vec![rr4_converted_card(
            uuid!(14)?,
            "sources/report.md",
            &original,
            &record,
        )?],
    )
    .await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let crafted = rr7_without(&bytes, &format!("objects/{}", asset.as_str()))?;
    let outcome = import_crafted(&storage, crafted).await?;
    assert_import_refused(&storage, outcome, asset.as_str()).await
}

/// B-2, rows: an import refuses an archive whose derived dataset (a row it copies) names an
/// object the archive does not hold; a replacement refuses one whose job output does (it copies
/// job rows too). Both before anything is written.
#[tokio::test]
async fn backup_a_restore_refuses_a_row_naming_an_object_the_archive_does_not_hold() -> TestResult {
    let absent = "f".repeat(64);
    let dataset = format!(
        "INSERT INTO derived_objects SELECT tenant_id, workspace_id, '{}', '{}', '{absent}',
             'dataset', 'text/csv' FROM workspaces;",
        uuid_text(10),
        "a".repeat(40)
    );
    rr7_import_refused(
        &|bytes| rr7_forged_steps(bytes, &[dataset.as_str()]),
        &absent,
    )
    .await?;
    let output = format!("INSERT INTO job_outputs SELECT job_id, '{absent}' FROM jobs;");
    rr7_replace_refused(
        true,
        &|bytes| rr7_forged_steps(bytes, &[output.as_str()]),
        "job output",
    )
    .await
}

/// B-2: an import copies no job row, so it does not require the objects job outputs name, and
/// imports an archive whose job output names one it does not hold; the target gets no job row.
#[tokio::test]
async fn backup_an_import_does_not_require_what_job_outputs_it_does_not_copy_name() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let job_scope = JobScope::Workspace(source.clone());
    let job = storage
        .records()
        .create_job(
            &job_scope,
            NewJob {
                mutation_id: uuid!(61)?,
                initiator: person("ana"),
                spec: JobSpec::BackupWorkspace,
            },
        )
        .await?;
    storage
        .records()
        .cancel_job(&job_scope, uuid!(62)?, job.id)
        .await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let forged = rr7_forged_steps(
        &bytes,
        &[&format!(
            "INSERT INTO job_outputs SELECT job_id, '{}' FROM jobs;",
            "f".repeat(64)
        )],
    )?;
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|name| (name == "records.sqlite").then(|| forged.clone()),
        &[],
    )?;
    let (target, _, result) = import_crafted(&storage, crafted).await?;
    result?;
    assert_eq!(
        count(
            &directory.path().join("records.sqlite"),
            &format!(
                "SELECT count(*) FROM jobs WHERE workspace_id = '{}'",
                target.workspace_id.0
            )
        )?,
        0,
        "no job row imported"
    );
    Ok(())
}

/// B-2, the writer: a backup is never archived, so a card whose original is a workspace
/// archive's bytes (an archive imported back as a document) would make the archive
/// unrestorable. The backup fails, naming the item and the digest, and writes nothing: no
/// archive object and no record, by itself or in a set.
#[tokio::test]
async fn backup_a_card_whose_original_is_a_backup_fails_the_backup_and_writes_nothing() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (workspace, created) = create(&storage, "local", 1, "Notes").await?;
    let backup = storage
        .backups()
        .write_workspace_archive(&workspace, uuid!(40)?)
        .await?
        .object;
    commit(
        &storage,
        &workspace,
        3,
        &created,
        vec![card(uuid!(15)?, &backup.digest)?],
    )
    .await?;
    let stored = rr8_stored(directory.path(), "local")?;
    let error = err_of(
        storage
            .backups()
            .write_workspace_archive(&workspace, uuid!(41)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Internal, "{error}");
    assert!(
        error.message.contains(&uuid_text(15)) && error.message.contains(backup.digest.as_str()),
        "names the item and the digest: {error}"
    );
    assert_eq!(
        count(
            &directory.path().join("records.sqlite"),
            "SELECT count(*) FROM backup_archives"
        )?,
        1,
        "no second archive recorded"
    );
    assert_eq!(
        rr8_stored(directory.path(), "local")?,
        stored,
        "no archive object stored"
    );
    let error = err_of(
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Internal, "{error}");
    assert!(error.message.contains(backup.digest.as_str()), "{error}");
    Ok(())
}

/// Q4 (passing probe, adopted): a record no card ever named (a write that died before its card)
/// is not followed. For a survivor its asset and record stay as that write's pending puts, so
/// another workspace's purge keeps them even when it names the asset too; for the target, its
/// own purge deletes both.
#[tokio::test]
async fn rr7_a_record_no_card_names_is_kept_or_collected_as_its_pending_puts() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (survivor, _) = create(&storage, "local", 1, "Survivor").await?;
    let (target, head) = create(&storage, "local", 2, "Target").await?;
    let asset = put(&storage, &survivor, b"rr7 a survivor's unlanded asset")
        .await?
        .digest;
    let record = put(&storage, &survivor, &rr4_record(&asset, &asset, 1))
        .await?
        .digest;
    let own_asset = put(&storage, &target, b"rr7 the target's unlanded asset")
        .await?
        .digest;
    let own_record = put(&storage, &target, &rr4_record(&own_asset, &own_asset, 1))
        .await?
        .digest;
    put(&storage, &target, b"rr7 a survivor's unlanded asset").await?;
    commit(&storage, &target, 3, &head, vec![card(uuid!(15)?, &asset)?]).await?;
    rr5_purge(&storage, &target).await?;
    for (digest, what, expected) in [
        (&asset, "the survivor's asset", true),
        (&record, "the survivor's record", true),
        (&own_asset, "the target's asset", false),
        (&own_record, "the target's record", false),
    ] {
        assert_eq!(
            object_file(directory.path(), digest)?.is_file(),
            expected,
            "{what}"
        );
    }
    Ok(())
}

/// Q4 (passing probe, adopted): a few bytes of JSON another workspace is still putting (no
/// card, no completed upload) naming the target's original keep nothing past its purge.
#[tokio::test]
async fn rr7_a_small_pending_json_naming_another_workspaces_object_keeps_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (uploader, _) = create(&storage, "local", 1, "Uploader").await?;
    let (target, head) = create(&storage, "local", 2, "Target").await?;
    let hidden = put(&storage, &target, b"rr7 an original only the target holds")
        .await?
        .digest;
    commit(
        &storage,
        &target,
        3,
        &head,
        vec![card(uuid!(15)?, &hidden)?],
    )
    .await?;
    let text = format!(
        "{{\"structured\": [{{\"digest\": \"{0}\"}}], \"assets\": [{{\"digest\": \"{0}\"}}]}}",
        hidden.as_str()
    );
    put(&storage, &uploader, text.as_bytes()).await?;
    rr5_purge(&storage, &target).await?;
    assert!(!object_file(directory.path(), &hidden)?.is_file());
    Ok(())
}

/// A completed upload slot of `workspace` under mutation `mutation` holding `body`, and the
/// object it holds.
async fn n6_upload(
    storage: &Storage,
    workspace: &StorageScope,
    mutation: u32,
    body: &[u8],
) -> Fallible<(UploadId, Digest)> {
    let object = put(storage, workspace, body).await?.digest;
    let uploads = storage.uploads();
    let slot = uploads
        .create(
            workspace,
            uuid!(mutation)?,
            NewUpload {
                filename: "document.pdf".to_owned(),
                relative_path: String::new(),
                expected_size: u64::try_from(body.len())?,
                expected_sha256: None,
                supplied_by: person("ana"),
            },
        )
        .await?;
    uploads
        .put_content(
            workspace,
            slot.id,
            Box::pin(std::io::Cursor::new(body.to_vec())),
            1 << 20,
        )
        .await?;
    uploads.complete(workspace, slot.id, object.clone()).await?;
    Ok((slot.id, object))
}

/// An import job of `workspace` under mutation `mutation`, claimed, that took `upload` as its
/// input; answers the job and its claim.
async fn n6_import_taking(
    storage: &Storage,
    workspace: &StorageScope,
    mutation: u32,
    upload: UploadId,
) -> Fallible<ClaimedJob> {
    let records = storage.records();
    let job = records
        .create_job(
            &JobScope::Workspace(workspace.clone()),
            NewJob {
                mutation_id: uuid!(mutation)?,
                initiator: person("ana"),
                spec: JobSpec::Import {
                    base_revision: storage.versions().head(workspace).await?,
                    upload_ids: vec![upload],
                    destination: None,
                    apply_naming_rules: false,
                    settings: ConversionSettings::default(),
                },
            },
        )
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "the claimed import")?;
    storage
        .uploads()
        .consume_batch(workspace, &[upload], job.id)
        .await?;
    Ok(claimed)
}

/// Complete the claimed import `claimed` with no output.
async fn n6_complete(storage: &Storage, claimed: ClaimedJob) -> TestResult {
    storage
        .records()
        .complete_job(JobCompletion {
            lease: claimed.lease,
            revision: None,
            item_ids: Vec::new(),
            artifact: None,
            restore: None,
            backup: None,
            outputs: Vec::new(),
            warnings: Vec::new(),
        })
        .await?;
    Ok(())
}

/// N-6 (b1r re-review): a backup taken while an import runs, after the import took its upload
/// and before its card landed, drops the running job (a restored one would run again), and so
/// writes the upload back as completed and unconsumed: restored by replacement, a new import
/// takes the upload and lands its card. An upload a finished import took stays taken by it.
#[tokio::test]
async fn backup_an_upload_a_running_import_took_is_restored_free_for_the_next_import() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let (workspace, (taken, object), (done, finished)) = {
        let storage = Storage::open(directory.path())?;
        let workspace = notes_workspace(&storage).await?;
        let (done, _) =
            n6_upload(&storage, &workspace, 40, b"n6 a finished import's input").await?;
        let finished = n6_import_taking(&storage, &workspace, 41, done).await?;
        let finished_job = finished.job.id;
        n6_complete(&storage, finished).await?;
        let taken = n6_upload(&storage, &workspace, 42, b"n6 a running import's input").await?;
        n6_import_taking(&storage, &workspace, 43, taken.0).await?;
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        (workspace, taken, (done, finished_job))
    };
    let (installation, archive) = set_of(output.path(), &workspace);
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    restored
        .backups()
        .restore_replacement(&installation, &[archive])
        .await?;
    let uploads = restored.uploads();
    assert_eq!(uploads.get(&workspace, taken).await?.consumed_by, None);
    assert_eq!(
        uploads.get(&workspace, done).await?.consumed_by,
        Some(finished),
        "a finished import keeps its input"
    );
    let again = n6_import_taking(&restored, &workspace, 44, taken).await?;
    let head = restored.versions().head(&workspace).await?;
    commit(
        &restored,
        &workspace,
        45,
        &head,
        vec![card(uuid!(15)?, &object)?],
    )
    .await?;
    n6_complete(&restored, again).await?;
    Ok(())
}

// B1-R round 4 (`rr9_`, the round 3 re-review's probes): a card's two references are read by
// path, a workspace's own damage never stops its purge, a survivor's damage names the
// workspace, and the restore refusals the review probed are committed tests.

/// The object `digest` of tenant `tenant` in the store at `data`.
fn rr9_object_file(data: &Path, tenant: &str, digest: &Digest) -> Fallible<std::path::PathBuf> {
    Ok(data
        .join("objects")
        .join(tenant)
        .join(some(digest.as_str().get(..2), "a digest")?)
        .join(digest.as_str()))
}

/// The `objects/` entries of the workspace archive object `digest` of tenant `tenant`.
fn rr9_archived(
    data: &Path,
    tenant: &str,
    digest: &Digest,
) -> Fallible<std::collections::BTreeSet<String>> {
    Ok(entries(&rr9_object_file(data, tenant, digest)?)?
        .into_iter()
        .filter_map(|name| name.strip_prefix("objects/").map(str::to_owned))
        .collect())
}

/// The one text `query` answers in `database`.
fn rr9_text(database: &Path, query: &str) -> Fallible<String> {
    let connection = rusqlite::Connection::open(database)?;
    Ok(connection.query_row(query, [], |row| row.get(0))?)
}

/// The pack of every commit on every reference of `repository`, and those references, as the
/// archive writer builds them (`archive::write_pack`). With `omit`, every object the walk
/// reaches is inserted one by one but that one, so the pack lacks it.
fn rr9_pack_of(repository: &git2::Repository, omit: Option<git2::Oid>) -> Fallible<Rr9Pack> {
    let mut walk = repository.revwalk()?;
    let mut references = Vec::new();
    for reference in repository.references()? {
        let reference = reference?;
        if let (Ok(name), Some(target)) = (reference.name(), reference.target())
            && repository.find_commit(target).is_ok()
        {
            walk.push(target)?;
            references.push((name.to_owned(), target.to_string()));
        }
    }
    references.sort();
    let mut pack = repository.packbuilder()?;
    match omit {
        None => pack.insert_walk(&mut walk)?,
        Some(omitted) => {
            for commit in walk {
                let commit = repository.find_commit(commit?)?;
                let mut objects = vec![commit.id(), commit.tree_id()];
                commit
                    .tree()?
                    .walk(git2::TreeWalkMode::PreOrder, |_, entry| {
                        objects.push(entry.id());
                        git2::TreeWalkResult::Ok
                    })?;
                for object in objects {
                    if object != omitted {
                        pack.insert_object(object, None)?;
                    }
                }
            }
        }
    }
    let mut buffer = git2::Buf::new();
    pack.write_buf(&mut buffer)?;
    Ok((buffer.to_vec(), references))
}

/// The archive `bytes` with its manifest changed by `change`.
fn rr9_with_manifest(bytes: &[u8], change: &dyn Fn(&mut serde_json::Value)) -> Fallible<Vec<u8>> {
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&entry_bytes_of(bytes, "manifest.json")?)?;
    change(&mut manifest);
    let text = serde_json::to_vec(&manifest)?;
    rebuilt(
        bytes,
        &|_| zip::CompressionMethod::Stored,
        &|name| (name == "manifest.json").then(|| text.clone()),
        &[],
    )
}

/// A backup set of tenant `local` with one notes workspace. After the set is written, `damage`
/// changes the workspace's repository through Git and gives the objects to add as
/// `objects/<digest>` entries; the workspace archive's `history.pack` is then rebuilt from that
/// repository, without the object `omit` names, and its manifest references with it. The data
/// directory is left as it is. Returns (data directory, output directory, workspace,
/// installation archive, workspace archive).
async fn rr9_set_with_history(
    damage: &Rr9Damage<'_>,
    omit: &Rr9Omit<'_>,
) -> Fallible<(
    tempfile::TempDir,
    tempfile::TempDir,
    StorageScope,
    std::path::PathBuf,
    std::path::PathBuf,
)> {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let workspace = notes_workspace(&storage).await?;
    storage
        .backups()
        .write_backup_set(output.path(), None)
        .await?;
    let extra = damage(&storage, &workspace)?;
    let repository = repository_of(&storage, &workspace)?;
    let (pack, references) = rr9_pack_of(&repository, omit(&repository)?)?;
    let (installation, archive) = set_of(output.path(), &workspace);
    let bytes = std::fs::read(&archive)?;
    let named: Vec<(String, Vec<u8>)> = extra
        .into_iter()
        .map(|object| {
            let digest = digest_of_bytes(&object)?;
            Ok((format!("objects/{}", digest.as_str()), object))
        })
        .collect::<Fallible<_>>()?;
    let added: Vec<(&str, &[u8])> = named
        .iter()
        .map(|(name, object)| (name.as_str(), object.as_slice()))
        .collect();
    let crafted = rebuilt(
        &bytes,
        &|_| zip::CompressionMethod::Stored,
        &|name| (name == "history.pack").then(|| pack.clone()),
        &added,
    )?;
    let crafted = rr9_with_manifest(&crafted, &|manifest| {
        if let Some(fields) = manifest.as_object_mut() {
            fields.insert("references".to_owned(), json!(references));
        }
    })?;
    std::fs::write(&archive, crafted)?;
    drop(storage);
    Ok((directory, output, workspace, installation, archive))
}

/// Q3(a): the crafted set restored by replacement over the deleted directory, and its archive
/// imported into a blank workspace of another tenant; each must be refused as invalid input
/// naming `what`, with nothing written.
async fn rr9_both_refuse(damage: &Rr9Damage<'_>, omit: &Rr9Omit<'_>, what: &str) -> TestResult {
    let (directory, _output, workspace, installation, archive) =
        rr9_set_with_history(damage, omit).await?;
    // Import first, into another tenant of the same data directory.
    {
        let storage = Storage::open(directory.path())?;
        let outcome = import_crafted(&storage, std::fs::read(&archive)?).await?;
        assert_import_refused(&storage, outcome, what).await?;
    }
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let result = restored
        .backups()
        .restore_replacement(&installation, &[archive])
        .await;
    drop(restored);
    if let Err(error) = &result {
        assert!(error.message.contains(what), "{error}");
    }
    rr5_refused_before_writing(directory.path(), &workspace, result, what)
}

/// Q3(a): an archive whose history holds a source appearance that does not read as one: not
/// JSON, an `object` that is not a digest, or an `extraction.digest` that is not one. If a
/// restore took it, every later purge of the tenant would stop on it.
#[tokio::test]
async fn rr9_a_restore_refuses_an_appearance_that_does_not_read() -> TestResult {
    let path = format!(".okf/sources/{}.json", uuid_text(15));
    let digest = "a".repeat(64);
    for bytes in [
        b"not json at all".to_vec(),
        b"{\"object\": \"not a digest\"}".to_vec(),
        format!("{{\"object\": \"{digest}\", \"extraction\": {{\"digest\": \"cut\"}}}}")
            .into_bytes(),
    ] {
        rr9_both_refuse(
            &|storage, workspace| {
                rr8_commit_raw_file(storage, workspace, &path, &bytes)?;
                Ok(Vec::new())
            },
            &|_| Ok(None),
            &path,
        )
        .await?;
    }
    Ok(())
}

/// Q3(a): an archive whose `.okf/sources` holds a folder, not appearance files.
#[tokio::test]
async fn rr9_a_restore_refuses_a_folder_among_the_appearances() -> TestResult {
    rr9_both_refuse(
        &|storage, workspace| {
            rr8_commit_raw_file(storage, workspace, ".okf/sources/nested/x.json", b"{}")?;
            Ok(Vec::new())
        },
        &|_| Ok(None),
        ".okf/sources/nested",
    )
    .await
}

/// Q3(a): an archive whose card names, as its conversion record, an object the archive holds
/// but that does not read as a record.
#[tokio::test]
async fn rr9_a_restore_refuses_a_card_named_record_that_does_not_read() -> TestResult {
    let original = b"rr9 a converted original".to_vec();
    let record = b"{\"structured\": [{\"digest\": \"cut".to_vec();
    let record_digest = digest_of_bytes(&record)?;
    rr9_both_refuse(
        &|storage, workspace| {
            let original_digest = digest_of_bytes(&original)?;
            let path = format!(".okf/sources/{}.json", uuid_text(14));
            let TreeEdit::WriteSourceCard(card) = rr4_converted_card(
                uuid!(14)?,
                "sources/report.md",
                &original_digest,
                &record_digest,
            )?
            else {
                return Err("a card".into());
            };
            rr8_commit_raw_file(
                storage,
                workspace,
                &path,
                &serde_json::to_vec(&card.appearance)?,
            )?;
            Ok(vec![original.clone(), record.clone()])
        },
        &|_| Ok(None),
        record_digest.as_str(),
    )
    .await
}

/// N-14: a pack that lacks an object a commit of its history needs (the parent of its head, the
/// head's tree, an appearance blob) is refused as invalid input naming the object, before any
/// write, in both modes: it is the archive's fault, not the version store's.
#[tokio::test]
async fn rr9_a_restore_refuses_a_pack_lacking_an_object_its_history_needs() -> TestResult {
    let original = b"rr9 an original a valid card names".to_vec();
    let path = format!(".okf/sources/{}.json", uuid_text(15));
    let omissions: [&Rr9Omit<'_>; 3] = [
        &|repository| Ok(Some(rr9_head(repository)?.parent(0)?.id())),
        &|repository| Ok(Some(rr9_head(repository)?.tree_id())),
        &|repository| {
            Ok(Some(
                rr9_head(repository)?
                    .tree()?
                    .get_path(Path::new(&path))?
                    .id(),
            ))
        },
    ];
    for omission in omissions {
        rr9_both_refuse(
            &|storage, workspace| {
                rr8_commit_raw_file(
                    storage,
                    workspace,
                    &path,
                    &rr9_appearance(&digest_of_bytes(&original)?, &json!({}))?,
                )?;
                Ok(vec![original.clone()])
            },
            omission,
            "the history does not hold",
        )
        .await?;
    }
    Ok(())
}

/// The commit `refs/heads/main` of `repository` names.
fn rr9_head(repository: &git2::Repository) -> Fallible<git2::Commit<'_>> {
    Ok(repository
        .find_reference("refs/heads/main")?
        .peel_to_commit()?)
}

/// The SHA-256 digest of `bytes`.
fn digest_of_bytes(bytes: &[u8]) -> Fallible<Digest> {
    use sha2::Digest as _;
    use std::fmt::Write as _;
    let hash = sha2::Sha256::digest(bytes);
    Ok(Digest::try_from(hash.iter().fold(
        String::new(),
        |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        },
    ))?)
}

/// The JSON of a valid source appearance over `object` (`card`'s), with `extra` fields merged in
/// at its top level.
fn rr9_appearance(object: &Digest, extra: &serde_json::Value) -> Fallible<Vec<u8>> {
    let mut appearance = json!({
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
    });
    if let (Some(fields), Some(more)) = (appearance.as_object_mut(), extra.as_object()) {
        for (key, value) in more {
            fields.insert(key.clone(), value.clone());
        }
    }
    Ok(serde_json::to_vec(&appearance)?)
}

/// Q3(b): a survivor's appearance that is valid except for one field this build does not know
/// (as a later contract might add, or an earlier one had). Read by path, it still names its
/// original, and another workspace's purge completes and keeps it. Read through the contract's
/// `deny_unknown_fields` type, it is "damaged", and the purge stops at its objects step, after
/// the target's rows and repository are gone.
#[tokio::test]
async fn rr9_an_appearance_with_an_unknown_field_does_not_stop_purges() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (survivor, _) = create(&storage, "local", 1, "Survivor").await?;
    let (target, target_head) = create(&storage, "local", 2, "Target").await?;
    let bytes = b"rr9 an original both workspaces hold";
    let shared = put(&storage, &survivor, bytes).await?.digest;
    rr8_commit_raw_file(
        &storage,
        &survivor,
        &format!(".okf/sources/{}.json", uuid_text(15)),
        &rr9_appearance(
            &shared,
            &json!({ "rr9_later_field": "a field a later contract adds" }),
        )?,
    )?;
    put(&storage, &target, bytes).await?;
    commit(
        &storage,
        &target,
        3,
        &target_head,
        vec![card(uuid!(16)?, &shared)?],
    )
    .await?;
    let outcome = rr8_purge(&storage, &target).await?;
    let live = directory.path().join("records.sqlite");
    let rows = count(
        &live,
        &format!(
            "SELECT count(*) FROM workspaces WHERE workspace_id = '{}'",
            target.workspace_id.0
        ),
    )?;
    let repository = directory
        .path()
        .join("repositories")
        .join("local")
        .join(format!("{}.git", target.workspace_id.0))
        .exists();
    match outcome {
        Ok(_) => {
            assert!(
                object_file(directory.path(), &shared)?.is_file(),
                "the survivor's original was deleted"
            );
            Ok(())
        }
        Err(error) => Err(format!(
            "another workspace's purge failed as {:?} ({}), because a survivor's appearance \
             carries one unknown field; the target's workspace rows left: {rows}, its repository \
             left: {repository}",
            error.code, error.message
        )
        .into()),
    }
}

/// R3-1: a survivor's card as a later contract might write it: a field this build does not know
/// at the top and under `extraction`, an unknown variant in an unrelated enum
/// (`outcome.status`) and a changed type (`size`), over objects the target put. Its `object` and
/// `extraction.digest` are read by path, so the target's purge keeps the original, the record
/// and the record's export and asset (nothing else of the survivor names them); the survivor's
/// backup copies exactly those four; and that archive imports into another tenant, which puts
/// the four back.
#[tokio::test]
async fn rr9_a_card_of_a_later_contract_is_read_by_purge_backup_and_restore() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (survivor, _) = create(&storage, "local", 1, "Survivor").await?;
    let (target, _) = create(&storage, "local", 2, "Target").await?;
    let original = put(&storage, &target, b"rr9 a later contract's original")
        .await?
        .digest;
    let export = put(&storage, &target, b"{\"rr9\": \"a later export\"}")
        .await?
        .digest;
    let asset = put(&storage, &target, b"rr9 a later render").await?.digest;
    let record = put(&storage, &target, &rr4_record(&export, &asset, 2))
        .await?
        .digest;
    let later = rr9_appearance(
        &original,
        &json!({
            "rr9_later_field": { "nested": [1, 2, 3] },
            "size": 10,
            "extraction": {
                "outcome": { "status": "rr9_later_status", "rr9_detail": "x" },
                "digest": record.as_str(),
                "text_origin": "none",
                "rr9_later_extraction_field": true
            }
        }),
    )?;
    rr8_commit_raw_file(
        &storage,
        &survivor,
        &format!(".okf/sources/{}.json", uuid_text(15)),
        &later,
    )?;
    let four = [&original, &record, &export, &asset];
    // Purge: the target's puts survive only through the survivor's card, read by path.
    rr8_purge(&storage, &target).await??;
    for digest in four {
        assert!(
            object_file(directory.path(), digest)?.is_file(),
            "the target's purge deleted {digest:?}, which a later contract's card names"
        );
    }
    // Backup: the archive copies exactly what the card and its record name.
    let archive = storage
        .backups()
        .write_workspace_archive(&survivor, uuid!(40)?)
        .await?
        .object;
    let archived = rr8_archived(directory.path(), &archive.digest)?;
    let expected: std::collections::BTreeSet<String> = four
        .iter()
        .map(|digest| digest.as_str().to_owned())
        .collect();
    assert_eq!(
        archived, expected,
        "the archive holds the four named objects"
    );
    // Restore: the archive imports, its check having read the card, and puts the four back.
    let bytes = std::fs::read(object_file(directory.path(), &archive.digest)?)?;
    let (restored, _, imported) = import_crafted(&storage, bytes).await?;
    imported?;
    for digest in four {
        assert!(
            rr9_object_file(directory.path(), restored.tenant_id.as_str(), digest)?.is_file(),
            "the import did not put {digest:?} back"
        );
    }
    Ok(())
}

/// N-9, N-10: a workspace holds an appearance whose `object` is not a digest and a card-named
/// conversion record that does not read. Another workspace's purge stops at its objects step,
/// `Internal`, naming the damaged workspace and the path (its own rows are gone by then, as
/// before; the message says where to look). The damaged workspace's own purge completes, and its
/// plan records the path and the record digest it could not read. The other's purge, repeated,
/// then completes: purging the damaged workspace is the in-product repair.
#[tokio::test]
async fn rr9_a_damaged_workspace_purges_itself_and_frees_the_tenants_purges() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (damaged, head) = create(&storage, "local", 1, "Damaged").await?;
    let (other, _) = create(&storage, "local", 2, "Other").await?;
    let original = put(&storage, &damaged, b"rr9 a converted original")
        .await?
        .digest;
    let record = put(&storage, &damaged, b"{\"structured\": [{\"digest\": \"cut")
        .await?
        .digest;
    commit(
        &storage,
        &damaged,
        3,
        &head,
        vec![rr4_converted_card(
            uuid!(14)?,
            "sources/report.md",
            &original,
            &record,
        )?],
    )
    .await?;
    let path = format!(".okf/sources/{}.json", uuid_text(15));
    rr8_commit_raw_file(&storage, &damaged, &path, b"{\"object\": \"not a digest\"}")?;
    // Another workspace's purge, recorded under its own ids, stops at its objects step, naming
    // the damaged workspace and the path.
    let tenant = other.tenant_id.clone();
    let purge = storage
        .records()
        .create_purge(
            &tenant,
            uuid!(70)?,
            NewPurge {
                id: uuid!(71)?,
                target: PurgeTarget::Workspace {
                    workspace_id: other.workspace_id,
                },
                initiator: person("ana"),
            },
        )
        .await?;
    let error = err_of(
        storage
            .purger()
            .purge_workspace(&tenant, uuid!(72)?, purge.id, other.workspace_id)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Internal, "{error}");
    assert!(
        error.message.contains(&damaged.workspace_id.0.to_string())
            && error.message.contains(&path),
        "names the damaged workspace and the path: {error}"
    );
    let live = directory.path().join("records.sqlite");
    assert_eq!(
        count(
            &live,
            &format!(
                "SELECT count(*) FROM workspaces WHERE workspace_id = '{}'",
                other.workspace_id.0
            )
        )?,
        0,
        "the other workspace's rows are already gone"
    );
    // The damaged workspace's own purge completes, recording what it could not read.
    rr8_purge(&storage, &damaged).await??;
    let unread: serde_json::Value = serde_json::from_str(&rr9_text(
        &live,
        &format!(
            "SELECT json_extract(plan, '$.unread') FROM purge_progress WHERE purge_id = '{}'",
            uuid_text(61)
        ),
    )?)?;
    let unread = some(unread.as_array(), "the plan's unread list")?;
    let mentions = |what: &str| {
        unread
            .iter()
            .any(|entry| entry.as_str().is_some_and(|text| text.contains(what)))
    };
    assert!(
        unread.len() == 2 && mentions(&path) && mentions(record.as_str()),
        "the plan records the appearance's path and the record's digest: {unread:?}"
    );
    assert!(
        !directory
            .path()
            .join("repositories")
            .join("local")
            .join(format!("{}.git", damaged.workspace_id.0))
            .exists(),
        "the damaged workspace's repository is gone"
    );
    // The other's purge, repeated, completes now that nothing of the tenant is damaged.
    storage
        .purger()
        .purge_workspace(&tenant, uuid!(72)?, purge.id, other.workspace_id)
        .await?;
    Ok(())
}

/// Q1: digest-shaped text in an appearance's other fields (embedded metadata, nested, and the
/// converter's model hash) names nothing: A's archive does not copy B's original, and B's purge
/// deletes it.
#[tokio::test]
async fn rr9_metadata_and_model_hashes_copy_and_keep_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (a, a_head) = create(&storage, "local", 1, "A").await?;
    let (b, b_head) = create(&storage, "local", 2, "B").await?;
    let hidden = put(&storage, &b, b"rr9 an original only workspace B holds")
        .await?
        .digest;
    commit(&storage, &b, 3, &b_head, vec![card(uuid!(15)?, &hidden)?]).await?;
    let source = put(&storage, &a, b"rr9 A's document").await?.digest;
    let export = put(&storage, &a, b"{\"rr9\": \"A's export\"}")
        .await?
        .digest;
    let asset = put(&storage, &a, b"rr9 A's render").await?.digest;
    let record = put(&storage, &a, &rr4_record(&export, &asset, 3))
        .await?
        .digest;
    let TreeEdit::WriteSourceCard(mut card) =
        rr4_converted_card(uuid!(14)?, "sources/doc.md", &source, &record)?
    else {
        return Err("a card".into());
    };
    let mut appearance = serde_json::to_value(&card.appearance)?;
    if let Some(fields) = appearance.as_object_mut() {
        fields.insert(
            "metadata".to_owned(),
            json!({ "Title": hidden.as_str(), "xmp": { "deep": [hidden.as_str()] } }),
        );
    }
    if let Some(models) = appearance
        .pointer_mut("/extraction/converter/models")
        .and_then(serde_json::Value::as_array_mut)
    {
        for model in models.iter_mut() {
            if let Some(fields) = model.as_object_mut() {
                fields.insert("sha256".to_owned(), json!(hidden.as_str()));
            }
        }
    }
    card.appearance = serde_json::from_value(appearance)?;
    commit(
        &storage,
        &a,
        4,
        &a_head,
        vec![TreeEdit::WriteSourceCard(card)],
    )
    .await?;
    let (copied, kept) = rr7_copied_and_kept(directory.path(), &storage, &a, &b, &hidden).await?;
    assert!(
        !copied && !kept,
        "digest text in A's metadata and model hash: copied {copied}, kept {kept}"
    );
    Ok(())
}

/// Q5: a manifest that names a commit the archive's history does not hold is refused as invalid
/// input naming the commit, before anything is written, in both modes.
#[tokio::test]
async fn rr9_a_manifest_commit_the_pack_lacks_is_refused() -> TestResult {
    let ghost = "0123456789abcdef0123456789abcdef01234567";
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let workspace = {
        let storage = Storage::open(directory.path())?;
        let workspace = notes_workspace(&storage).await?;
        storage
            .backups()
            .write_backup_set(output.path(), None)
            .await?;
        workspace
    };
    let (installation, archive) = set_of(output.path(), &workspace);
    let crafted = rr9_with_manifest(&std::fs::read(&archive)?, &|manifest| {
        if let Some(references) = manifest
            .get_mut("references")
            .and_then(serde_json::Value::as_array_mut)
        {
            references.push(json!(["refs/heads/rr9-ghost", ghost]));
        }
    })?;
    std::fs::write(&archive, &crafted)?;
    {
        let storage = Storage::open(directory.path())?;
        let outcome = import_crafted(&storage, crafted).await?;
        assert_import_refused(&storage, outcome, ghost).await?;
    }
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    let result = restored
        .backups()
        .restore_replacement(&installation, &[archive])
        .await;
    drop(restored);
    if let Err(error) = &result {
        assert!(error.message.contains(ghost), "{error}");
    }
    rr5_refused_before_writing(directory.path(), &workspace, result, "a ghost commit")
}

/// N-11: a manifest reference whose name Git refuses, on a commit the pack holds, is refused as
/// invalid input before anything is written: the target's head has not moved and its only
/// pending put is the uploaded archive.
#[tokio::test]
async fn rr9_a_manifest_reference_git_cannot_name_is_refused_before_any_write() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let source = notes_workspace(&storage).await?;
    let bytes = archive_bytes(&storage, &source).await?;
    let crafted = rr9_with_manifest(&bytes, &|manifest| {
        let main = manifest
            .get("references")
            .and_then(serde_json::Value::as_array)
            .and_then(|references| {
                references.iter().find(|reference| {
                    reference.get(0).and_then(serde_json::Value::as_str) == Some("refs/heads/main")
                })
            })
            .and_then(|reference| reference.get(1))
            .cloned();
        if let (Some(main), Some(references)) = (
            main,
            manifest
                .get_mut("references")
                .and_then(serde_json::Value::as_array_mut),
        ) {
            references.push(json!(["refs/heads/rr9..bad", main]));
        }
    })?;
    let (target, blank, result) = import_crafted(&storage, crafted).await?;
    let moved = storage.versions().head(&target).await? != blank;
    let pending = count(
        &directory.path().join("records.sqlite"),
        &format!(
            "SELECT count(*) FROM pending_puts WHERE workspace_id = '{}'",
            target.workspace_id.0
        ),
    )?;
    match result {
        Ok(report) => Err(format!("imported: {report:?}").into()),
        Err(error) => {
            assert!(
                error.code == ErrorCode::InvalidInput
                    && error.message.contains("rr9..bad")
                    && !moved
                    && pending == 1,
                "refused as {:?} ({}); the target's head moved: {moved}; its pending puts: \
                 {pending} (1 is the uploaded archive)",
                error.code,
                error.message
            );
            Ok(())
        }
    }
}

/// A notes workspace in the data directory `directory` whose backup job's output is the archive
/// it wrote, recorded as its `WorkspaceBackup` artifact, and which holds that archive in a
/// completed upload slot too, beside an ordinary upload; then a backup set of the directory
/// written into `output`. Returns the workspace and the first archive's digest.
async fn rr9_workspace_holding_its_own_backup(
    directory: &Path,
    output: &Path,
) -> Fallible<(StorageScope, Digest)> {
    let storage = Storage::open(directory)?;
    let workspace = notes_workspace(&storage).await?;
    let scope = JobScope::Workspace(workspace.clone());
    let records = storage.records();
    let job = records
        .create_job(
            &scope,
            NewJob {
                mutation_id: uuid!(30)?,
                initiator: person("ana"),
                spec: JobSpec::BackupWorkspace,
            },
        )
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "the claimed job")?;
    let archive = storage
        .backups()
        .write_workspace_archive(&workspace, claimed.mutation_id)
        .await?
        .object;
    let artifact = records
        .record_artifact(
            &scope,
            claimed.mutation_id,
            NewArtifact {
                kind: ArtifactKind::WorkspaceBackup,
                object: archive.clone(),
                media_type: "application/zip".to_owned(),
                created_by_job: job.id,
            },
        )
        .await?;
    records
        .complete_job(JobCompletion {
            lease: claimed.lease,
            revision: None,
            item_ids: Vec::new(),
            artifact: Some(artifact.id),
            restore: None,
            backup: None,
            outputs: vec![archive.digest.clone()],
            warnings: Vec::new(),
        })
        .await?;
    // The archive's bytes uploaded back into the workspace, then an ordinary upload.
    let archive_bytes = std::fs::read(object_file(directory, &archive.digest)?)?;
    let uploads = storage.uploads();
    let slot = uploads
        .create(
            &workspace,
            uuid!(41)?,
            NewUpload {
                filename: "backup.zip".to_owned(),
                relative_path: String::new(),
                expected_size: u64::try_from(archive_bytes.len())?,
                expected_sha256: None,
                supplied_by: person("ana"),
            },
        )
        .await?;
    uploads
        .put_content(
            &workspace,
            slot.id,
            Box::pin(std::io::Cursor::new(archive_bytes)),
            1 << 30,
        )
        .await?;
    uploads
        .complete(&workspace, slot.id, archive.digest.clone())
        .await?;
    rr6_uploaded(&storage, &workspace, b"rr9 an ordinary upload").await?;
    storage.backups().write_backup_set(output, None).await?;
    Ok((workspace, archive.digest))
}

/// Q4, N-13: a workspace whose backup job's output is the archive it wrote, recorded as its
/// artifact, and which holds that archive in a completed upload slot too (beside an ordinary
/// upload). Its next backup set leaves out the three rows that name the archive, counts them in
/// the manifest's `rows_left_out`, and keeps the rest; the set restores by replacement and its
/// archive by import, and the restored workspace keeps the job (finished, no output, no
/// download) and the ordinary upload.
#[tokio::test]
async fn rr9_a_workspace_holding_its_own_backup_restores_without_the_rows_naming_it() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let (workspace, first) =
        rr9_workspace_holding_its_own_backup(directory.path(), output.path()).await?;
    let (installation, archive) = set_of(output.path(), &workspace);
    let bytes = std::fs::read(&archive)?;
    assert!(
        !entries(&archive)?.contains(&format!("objects/{}", first.as_str())),
        "the set's archive holds no earlier archive"
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&entry_bytes_of(&bytes, "manifest.json")?)?;
    assert_eq!(
        manifest
            .get("rows_left_out")
            .and_then(serde_json::Value::as_u64),
        Some(3),
        "the backup artifact, the job's output and the upload slot naming the archive are \
         counted: {manifest}"
    );
    let staged = output.path().join("records.sqlite");
    std::fs::write(&staged, entry_bytes_of(&bytes, "records.sqlite")?)?;
    assert_eq!(
        count(&staged, "SELECT count(*) FROM uploads")?,
        1,
        "one upload kept"
    );
    assert_eq!(
        count(&staged, "SELECT count(*) FROM job_outputs")?,
        0,
        "no output kept"
    );
    assert_eq!(
        count(&staged, "SELECT count(*) FROM jobs")?,
        1,
        "the job kept"
    );
    // By import, into another tenant of the same directory.
    {
        let storage = Storage::open(directory.path())?;
        let (_, _, imported) = import_crafted(&storage, bytes).await?;
        imported?;
    }
    // By replacement, over the deleted directory.
    std::fs::remove_dir_all(directory.path())?;
    let restored = Storage::open(directory.path())?;
    restored
        .backups()
        .restore_replacement(&installation, &[archive])
        .await?;
    let live = directory.path().join("records.sqlite");
    assert_eq!(count(&live, "SELECT count(*) FROM uploads")?, 1);
    assert_eq!(count(&live, "SELECT count(*) FROM jobs")?, 1);
    assert_eq!(count(&live, "SELECT count(*) FROM job_outputs")?, 0);
    assert_eq!(
        count(
            &live,
            "SELECT count(*) FROM jobs WHERE json_extract(record, '$.artifact') IS NOT NULL"
        )?,
        0,
        "the restored job advertises no download"
    );
    Ok(())
}

// Round 4 re-review (`rr10_`, b1r4): B4-1, a pack lacking a blob only its history or its head
// needs, and what a refused import leaves working; N-20, the `.okf` and `sources` trees; N-21,
// the count against other workspaces' rows; N-22, a survivor whose repository does not open.

/// `Rr9Damage`: edit `notes/plan.md` once more through Git, so the note's first blob is needed
/// by the history but not by the head.
fn rr10_edit_the_note(storage: &Storage, workspace: &StorageScope) -> Fallible<Vec<Vec<u8>>> {
    let repository = repository_of(storage, workspace)?;
    let head = rr9_head(&repository)?;
    let entry = head.tree()?.get_path(Path::new("notes/plan.md"))?;
    let text = String::from_utf8(repository.find_blob(entry.id())?.content().to_vec())?;
    let edited = text.replace("The plan.", "The plan, edited.");
    assert!(edited != text, "the note was edited");
    drop(head);
    drop(repository);
    rr8_commit_raw_file(storage, workspace, "notes/plan.md", edited.as_bytes())?;
    Ok(Vec::new())
}

/// `Rr9Omit`: the blob of `notes/plan.md` on the parent of the head of `repository`, which
/// only the history needs once `rr10_edit_the_note` ran.
fn rr10_old_note(repository: &git2::Repository) -> Fallible<Option<git2::Oid>> {
    Ok(Some(
        rr9_head(repository)?
            .parent(0)?
            .tree()?
            .get_path(Path::new("notes/plan.md"))?
            .id(),
    ))
}

/// `Rr9Omit`: the blob of `notes/plan.md` on the head of `repository`.
fn rr10_head_note(repository: &git2::Repository) -> Fallible<Option<git2::Oid>> {
    Ok(Some(
        rr9_head(repository)?
            .tree()?
            .get_path(Path::new("notes/plan.md"))?
            .id(),
    ))
}

/// B4-1: an archive whose pack lacks a blob only its history needs (an earlier version of a
/// note, on no appearance tree) is refused as invalid input before any write, in both modes, as
/// N-14 refuses a pack lacking a parent, a tree or an appearance blob: the archive writer could
/// not pack the restored workspace again.
#[tokio::test]
async fn rr10_a_pack_lacking_a_blob_only_its_history_needs_is_refused() -> TestResult {
    rr9_both_refuse(
        &rr10_edit_the_note,
        &rr10_old_note,
        "the history does not hold",
    )
    .await
}

/// B4-1: a pack lacking a blob its head needs (the note itself) is refused as invalid input
/// before any write, in both modes, rather than failing as the version store's after the pack
/// is indexed and the head moved.
#[tokio::test]
async fn rr10_a_pack_lacking_a_blob_its_head_needs_is_refused_before_any_write() -> TestResult {
    rr9_both_refuse(
        &|_, _| Ok(Vec::new()),
        &rr10_head_note,
        "the history does not hold",
    )
    .await
}

/// B4-1, the reviewer's characterisation turned around: a workspace Admin of tenant `elsewhere`
/// imports an archive whose pack lacks a blob (`omit`, after `damage`), and the import is
/// refused. Then every later backup still runs: the target workspace's own archive, the
/// installation's backup set, and, once the records need an upgrade, the pre-migration set, so
/// the installation opens and migrates for every tenant (SPEC sections 2 and 12).
async fn rr10_refused_import_leaves_the_backups_working(
    damage: &Rr9Damage<'_>,
    omit: &Rr9Omit<'_>,
) -> TestResult {
    let (directory, _output, _workspace, _installation, archive) =
        rr9_set_with_history(damage, omit).await?;
    {
        let storage = Storage::open(directory.path())?;
        let (target, blank, result) = import_crafted(&storage, std::fs::read(&archive)?).await?;
        assert_import_refused(
            &storage,
            (target.clone(), blank, result),
            "the history does not hold",
        )
        .await?;
        storage
            .backups()
            .write_workspace_archive(&target, uuid!(95)?)
            .await?;
        let everything = tempfile::tempdir()?;
        storage
            .backups()
            .write_backup_set(everything.path(), None)
            .await?;
        assert!(
            set_of(everything.path(), &target).1.is_file(),
            "the installation's set holds the target's archive"
        );
    }
    let records = directory.path().join("records.sqlite");
    as_records_schema_1(&records)?;
    let storage = Storage::open(directory.path())?;
    let sets = std::fs::read_dir(storage.data().pre_migration_path())?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .count();
    assert_eq!(sets, 1, "the pre-migration backup set was written");
    assert_eq!(
        count(&records, "PRAGMA user_version")?,
        4,
        "then the records are upgraded"
    );
    Ok(())
}

/// B4-1: after the refused import of a pack lacking a blob only its history needs, every later
/// backup set and the pre-migration backup succeed.
#[tokio::test]
async fn rr10_a_refused_import_lacking_an_old_blob_leaves_the_installations_backups_working()
-> TestResult {
    rr10_refused_import_leaves_the_backups_working(&rr10_edit_the_note, &rr10_old_note).await
}

/// B4-1: after the refused import of a pack lacking a blob its head needs, every later backup
/// set and the pre-migration backup succeed.
#[tokio::test]
async fn rr10_a_refused_import_lacking_a_head_blob_leaves_the_installations_backups_working()
-> TestResult {
    rr10_refused_import_leaves_the_backups_working(&|_, _| Ok(Vec::new()), &rr10_head_note).await
}

/// N-20: a pack that lacks the tree at `tree` of its head (under which a valid appearance sits)
/// is refused as invalid input naming `what`, before any write, in both modes.
async fn rr10_a_pack_lacking_a_tree_is_refused(tree: &str, what: &str) -> TestResult {
    let original = b"rr10 an original a valid card names".to_vec();
    let path = format!(".okf/sources/{}.json", uuid_text(15));
    rr9_both_refuse(
        &|storage, workspace| {
            rr8_commit_raw_file(
                storage,
                workspace,
                &path,
                &rr9_appearance(&digest_of_bytes(&original)?, &json!({}))?,
            )?;
            Ok(vec![original.clone()])
        },
        &|repository| {
            Ok(Some(
                rr9_head(repository)?
                    .tree()?
                    .get_path(Path::new(tree))?
                    .id(),
            ))
        },
        what,
    )
    .await
}

/// N-20: a pack lacking the `.okf` tree of a commit is refused naming that folder, told apart
/// from a commit with no sources folder.
#[tokio::test]
async fn rr10_a_pack_lacking_the_okf_tree_is_refused_naming_it() -> TestResult {
    rr10_a_pack_lacking_a_tree_is_refused(".okf", "the folder .okf (").await
}

/// N-20: a pack lacking the `.okf/sources` tree of a commit is refused naming that folder.
#[tokio::test]
async fn rr10_a_pack_lacking_the_sources_tree_is_refused_naming_it() -> TestResult {
    rr10_a_pack_lacking_a_tree_is_refused(".okf/sources", "the folder .okf/sources (").await
}

/// Ruling 14: a workspace archive answers the rows it left out, the count its manifest holds,
/// on its first write and on a repeat of its mutation (read back from the stored archive), and
/// a restore of it reports the same count, also when the restore is repeated.
#[tokio::test]
async fn an_archive_answers_and_its_restore_reports_the_rows_it_left_out() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let (workspace, _first) =
        rr9_workspace_holding_its_own_backup(directory.path(), output.path()).await?;
    let storage = Storage::open(directory.path())?;
    let backups = storage.backups();
    let written = backups
        .write_workspace_archive(&workspace, uuid!(95)?)
        .await?;
    assert_eq!(
        written.rows_left_out, 3,
        "the artifact, the output and the upload"
    );
    let bytes = std::fs::read(object_file(directory.path(), &written.object.digest)?)?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&entry_bytes_of(&bytes, "manifest.json")?)?;
    assert_eq!(
        manifest
            .get("rows_left_out")
            .and_then(serde_json::Value::as_u64),
        Some(3)
    );
    let repeated = backups
        .write_workspace_archive(&workspace, uuid!(95)?)
        .await?;
    assert_eq!(
        repeated, written,
        "a repeat answers the archive and its count"
    );
    let (target, _) = create(&storage, "elsewhere", 96, "Copy").await?;
    let uploaded = storage
        .blobs()
        .put(
            &target,
            Box::pin(std::io::Cursor::new(bytes)),
            1 << 30,
            None,
        )
        .await?;
    let report = backups
        .restore_import(&target, uploaded.digest.clone(), uuid!(97)?, Vec::new())
        .await?;
    assert_eq!(report.rows_left_out, Some(3));
    let again = backups
        .restore_import(&target, uploaded.digest, uuid!(97)?, Vec::new())
        .await?;
    assert_eq!(again, report);
    Ok(())
}

/// N-21: `rows_left_out` is the workspace's own count. A second workspace of the tenant holding
/// its own backup artifact and job output adds nothing to the first's count (3), and its own
/// archive counts its two rows.
#[tokio::test]
async fn rr10_rows_left_out_counts_only_the_workspaces_own_rows() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let second = {
        let storage = Storage::open(directory.path())?;
        let (second, _) = create(&storage, "local", 2, "Second").await?;
        let scope = JobScope::Workspace(second.clone());
        let records = storage.records();
        let job = records
            .create_job(
                &scope,
                NewJob {
                    mutation_id: uuid!(90)?,
                    initiator: person("ana"),
                    spec: JobSpec::BackupWorkspace,
                },
            )
            .await?;
        let claimed = some(records.claim_job(job.id).await?, "the claimed job")?;
        let archive = storage
            .backups()
            .write_workspace_archive(&second, claimed.mutation_id)
            .await?
            .object;
        let artifact = records
            .record_artifact(
                &scope,
                claimed.mutation_id,
                NewArtifact {
                    kind: ArtifactKind::WorkspaceBackup,
                    object: archive.clone(),
                    media_type: "application/zip".to_owned(),
                    created_by_job: job.id,
                },
            )
            .await?;
        records
            .complete_job(JobCompletion {
                lease: claimed.lease,
                revision: None,
                item_ids: Vec::new(),
                artifact: Some(artifact.id),
                restore: None,
                backup: None,
                outputs: vec![archive.digest.clone()],
                warnings: Vec::new(),
            })
            .await?;
        second
    };
    let (workspace, _) =
        rr9_workspace_holding_its_own_backup(directory.path(), output.path()).await?;
    let counted = |scope: &StorageScope| -> Fallible<Option<u64>> {
        let (_, archive) = set_of(output.path(), scope);
        let manifest: serde_json::Value =
            serde_json::from_slice(&entry_bytes_of(&std::fs::read(&archive)?, "manifest.json")?)?;
        Ok(manifest
            .get("rows_left_out")
            .and_then(serde_json::Value::as_u64))
    };
    let (first, other) = (counted(&workspace)?, counted(&second)?);
    assert!(
        first == Some(3) && other == Some(2),
        "the first workspace's count {first:?} (3 expected), the second's {other:?} (2 expected)"
    );
    Ok(())
}

/// N-22: another workspace's purge, once `break_repository` has broken the survivor's
/// repository directory, fails naming the survivor as its damage does (N-10), "in the workspace
/// <id>", so an operator knows where to look; the failure stays the version store's. The id
/// alone would not do: libgit2's own message for a repository that does not open carries the
/// directory path, which holds the id.
async fn rr10_a_broken_survivor_fails_the_purge_naming_it(
    break_repository: &dyn Fn(&Path) -> TestResult,
) -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (survivor, _) = create(&storage, "local", 1, "Survivor").await?;
    let (target, _) = create(&storage, "local", 2, "Target").await?;
    break_repository(
        &storage
            .data()
            .repositories_path()
            .join("local")
            .join(format!("{}.git", survivor.workspace_id.0)),
    )?;
    let error = err_of(rr8_purge(&storage, &target).await?)?;
    assert_eq!(error.code, ErrorCode::Unavailable, "{error}");
    assert!(
        error
            .message
            .contains(&format!("in the workspace {}", survivor.workspace_id.0)),
        "names the survivor as damage does: {error}"
    );
    Ok(())
}

/// N-22: a survivor whose repository does not open (its `HEAD` is gone) is named.
#[tokio::test]
async fn rr10_a_survivor_whose_repository_does_not_open_fails_the_purge_naming_it() -> TestResult {
    rr10_a_broken_survivor_fails_the_purge_naming_it(&|repository| {
        Ok(std::fs::remove_file(repository.join("HEAD"))?)
    })
    .await
}

/// Git errors name no machine path (sweep 2a, ruling 10): libgit2's message for a survivor's
/// repository that does not open carries the repository's absolute path; the purge's failure
/// names it relative to the data directory, with libgit2's class and code, and holds no part of
/// the data directory's absolute path.
#[tokio::test]
async fn a_survivor_that_does_not_open_is_named_without_a_machine_path() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (survivor, _) = create(&storage, "local", 1, "Survivor").await?;
    let (target, _) = create(&storage, "local", 2, "Target").await?;
    std::fs::remove_file(
        storage
            .data()
            .repositories_path()
            .join("local")
            .join(format!("{}.git", survivor.workspace_id.0))
            .join("HEAD"),
    )?;
    let error = err_of(rr8_purge(&storage, &target).await?)?;
    assert_eq!(error.code, ErrorCode::Unavailable, "{error}");
    let relative = format!("repositories/local/{}.git", survivor.workspace_id.0);
    assert!(error.message.contains(&relative), "{error}");
    assert!(error.message.contains("(Repository, NotFound)"), "{error}");
    let root = directory.path().to_string_lossy();
    let forward = root.replace('\\', "/");
    for part in [root.as_ref(), forward.as_str()]
        .into_iter()
        .chain(forward.split('/').filter(|part| !part.is_empty()))
    {
        assert!(!error.message.contains(part), "{part:?} is in {error}");
    }
    Ok(())
}

/// N-22: a survivor whose repository opens but whose references do not list (its `packed-refs`
/// does not parse) is named too.
#[tokio::test]
async fn rr10_a_survivor_whose_references_do_not_list_fails_the_purge_naming_it() -> TestResult {
    rr10_a_broken_survivor_fails_the_purge_naming_it(&|repository| {
        Ok(std::fs::write(
            repository.join("packed-refs"),
            b"not a packed references file",
        )?)
    })
    .await
}

#[path = "../../../tests/support/check.rs"]
mod check;
