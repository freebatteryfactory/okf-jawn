//! Workspace purge against real Git, SQLite and the object store: the receipt of
//! `storage-purge-workspace` (`cargo test -p okf-jawn-storage --features runtime -- purge`).
//!
//! The target workspace's content carries the marker `zanzibar` (in a note's path and body, a
//! draft, original bytes, a conversion record's asset, retained archives and exports, the
//! warnings kept for a commit's replay (the fixture's check warns about each staged path that
//! carries the marker, as a lint finding quotes a path), an
//! upload's part file); nothing that survives carries it. After the purge no file anywhere
//! under the data directory may contain the marker, which reaches the SQLite files' free pages
//! and write-ahead logs, the index, the object store and the pre-migration backups.
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{EventKind, Receipt};
use okf_jawn_contract::identity::{
    ConfirmationId, Digest, ItemId, MutationId, PurgeId, Revision, TenantId, UploadId,
    WorkspacePath,
};
use okf_jawn_contract::import::{Job, JobState};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::proposal::{Comment, Proposal, ProposalStatus};
use okf_jawn_contract::purge::{PurgeReport, PurgeTarget};
use okf_jawn_contract::review::{ConfirmationAction, ConfirmationTarget};
use okf_jawn_contract::workspace::ArtifactKind;
use okf_jawn_core::confirmations::{ConfirmationConsume, ConfirmationCreate, ConfirmationStore};
use okf_jawn_core::credentials::{ConnectorIssue, CredentialStore, NewConnector};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::events::{EventLog, EventScope, NewEvent};
use okf_jawn_core::jobs::{
    ClaimedJob, DerivedKind, DerivedObject, JobCompletion, JobScope, JobSpec, NewArtifact, NewJob,
    NewPurge, RecordStore,
};
use okf_jawn_core::mutations::{BeginOutcome, MutationKey, MutationStore};
use okf_jawn_core::proposals::ProposalStore;
use okf_jawn_core::sandbox::SandboxCapabilityStore;
use okf_jawn_core::search::SearchIndex;
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

/// A ledger row as the table holds it: state, token, lease expiry, response and finish time.
type RowState = (String, i64, i64, Option<String>, Option<i64>);

/// Accepts every candidate, with one warning for each staged file whose path carries the marker,
/// quoting that path as a lint finding would. Storage keeps a commit's warnings for its replay
/// (`commit_warnings`), so every commit of the target's marked note leaves the marker there.
struct Accept;

/// Two workspaces of one tenant: the purge target and a survivor.
struct Fixture {
    storage: Storage,
    target: StorageScope,
    survivor: StorageScope,
    /// Every revision of the target, proposal candidate included.
    revisions: Vec<Revision>,
    /// An object both workspaces' cards reference.
    shared: Digest,
    /// Objects only the target references.
    only_target: Vec<Digest>,
    /// The target's workspace archive, which the survivor holds in an upload slot.
    archive: Digest,
}

/// What `seed_git` wrote.
struct Seeded {
    revisions: Vec<Revision>,
    shared: Digest,
    objects: Vec<Digest>,
}

/// The marker of the target's content.
const MARKER: &[u8] = b"zanzibar";

impl CandidateCheck for Accept {
    fn check(
        &self,
        root: &Path,
        _rewritten: &[okf_jawn_contract::identity::WorkspacePath],
    ) -> Result<Vec<Warning>, ApiError> {
        let mut marked = Vec::new();
        marked_paths(root, "", &mut marked)
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
        Ok(marked
            .into_iter()
            .map(|path| Warning {
                code: "test_marked_path".to_owned(),
                message: format!("{path} is named for a trip"),
                location: Some(path),
            })
            .collect())
    }
}

/// The files under `root`, as `/`-separated paths relative to it, whose path holds the marker.
fn marked_paths(root: &Path, prefix: &str, found: &mut Vec<String>) -> std::io::Result<()> {
    let marker = String::from_utf8_lossy(MARKER);
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if entry.file_type()?.is_dir() {
            marked_paths(&entry.path(), &path, found)?;
        } else if path.contains(marker.as_ref()) {
            found.push(path);
        }
    }
    Ok(())
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

fn tenant() -> Fallible<TenantId> {
    Ok(TenantId::try_from("local".to_owned())?)
}

fn note(item: ItemId, at: &str, body: &str) -> Fallible<TreeEdit> {
    Ok(TreeEdit::CreateItem {
        item_id: item,
        path: WorkspacePath::try_from(at.to_owned())?,
        title: Some("Plans".to_owned()),
        type_name: "Note".to_owned(),
        kind: ItemKind::Note,
        body: body.to_owned(),
        properties: BTreeMap::new(),
    })
}

/// A source card for `object` whose extraction names the conversion record `record`.
fn card(item: ItemId, at: &str, object: &Digest, record: Option<&Digest>) -> Fallible<TreeEdit> {
    let mut extraction = json!({
        "outcome": { "status": "unsupported" },
        "text_origin": "none",
        "corrected": false,
        "warnings": []
    });
    if let (Some(record), Some(fields)) = (record, extraction.as_object_mut()) {
        fields.insert("digest".to_owned(), json!(record.as_str()));
    }
    Ok(TreeEdit::WriteSourceCard(Box::new(SourceCard {
        item_id: item,
        path: WorkspacePath::try_from(at.to_owned())?,
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
            "extraction": extraction
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
    mutation: u32,
    name: &str,
) -> Fallible<(StorageScope, Revision)> {
    let created = storage
        .catalog()
        .create(
            &tenant()?,
            uuid!(mutation)?,
            NewWorkspace {
                name: name.to_owned(),
                description: String::new(),
                creator: person("ana"),
            },
        )
        .await?;
    Ok((
        StorageScope {
            tenant_id: tenant()?,
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

/// Record a retained artifact of `kind` over `bytes` in the workspace.
async fn artifact(
    storage: &Storage,
    scope: &StorageScope,
    mutation: u32,
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
                mutation_id: uuid!(mutation)?,
                initiator: person("ana"),
                spec: JobSpec::BackupWorkspace,
            },
        )
        .await?;
    storage
        .records()
        .record_artifact(
            &job_scope,
            uuid!(mutation.saturating_add(1))?,
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

/// Open an upload slot in `scope` and write `bytes` to it, completing it when asked.
async fn upload(
    storage: &Storage,
    scope: &StorageScope,
    mutation: u32,
    bytes: &[u8],
    complete: bool,
) -> Fallible<okf_jawn_contract::identity::UploadId> {
    let uploads = storage.uploads();
    let slot = uploads
        .create(
            scope,
            uuid!(mutation)?,
            NewUpload {
                filename: "archive.zip".to_owned(),
                relative_path: String::new(),
                expected_size: u64::try_from(bytes.len())?,
                expected_sha256: None,
                supplied_by: person("ana"),
            },
        )
        .await?;
    if !bytes.is_empty() {
        uploads
            .put_content(
                scope,
                slot.id,
                Box::pin(std::io::Cursor::new(bytes.to_vec())),
                1 << 20,
            )
            .await?;
    }
    if complete {
        let digest = Digest::try_from(sha256(bytes))?;
        uploads.complete(scope, slot.id, digest).await?;
    }
    Ok(slot.id)
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    use std::fmt::Write as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// The target's Git content (a note, two cards, a proposal candidate) and the survivor's
/// card that shares one of the target's objects.
async fn seed_git(
    storage: &Storage,
    target: &StorageScope,
    survivor: &StorageScope,
    heads: (Revision, Revision),
) -> Fallible<Seeded> {
    let (created, survivor_head) = heads;
    let original = put(storage, target, b"zanzibar original bytes").await?;
    let asset = put(storage, target, b"zanzibar page image").await?;
    let record = json!({ "assets": [{ "digest": asset.digest.as_str() }] }).to_string();
    let record = put(storage, target, record.as_bytes()).await?;
    let shared = put(storage, target, b"commonplace shared bytes").await?;
    let edits = vec![
        note(
            uuid!(10)?,
            "notes/zanzibar-plans.md",
            "Ferry to zanzibar at dawn.",
        )?,
        card(
            uuid!(11)?,
            "sources/scan.md",
            &original.digest,
            Some(&record.digest),
        )?,
        card(uuid!(12)?, "sources/common.md", &shared.digest, None)?,
    ];
    let seeded = commit(storage, target, 3, &created, edits).await?;
    let candidate = storage
        .versions()
        .create_candidate(
            target,
            uuid!(20)?,
            CandidateChanges {
                mutation_id: uuid!(4)?,
                base: seeded.clone(),
                author: person("ana"),
                message: "Propose".to_owned(),
                edits: vec![note(uuid!(13)?, "notes/other.md", "More zanzibar notes.")?],
            },
            Arc::new(Accept),
        )
        .await?;
    let shared_card = card(uuid!(14)?, "sources/common.md", &shared.digest, None)?;
    commit(storage, survivor, 5, &survivor_head, vec![shared_card]).await?;
    Ok(Seeded {
        revisions: vec![created, seeded, candidate],
        shared: shared.digest,
        objects: vec![original.digest, asset.digest, record.digest],
    })
}

/// A receipt of the survivor citing the target's note.
async fn cite(
    storage: &Storage,
    survivor: &StorageScope,
    target: &StorageScope,
    revision: &Revision,
) -> Fallible<()> {
    let receipt: Receipt = serde_json::from_value(json!({
        "id": uuid_text(40),
        "workspace_id": survivor.workspace_id,
        "operation_id": "search_items",
        "principal_subject": "ana",
        "route": "local_owner",
        "sources": [{
            "workspace_id": target.workspace_id,
            "item_id": uuid_text(10),
            "path": "notes/zanzibar-plans.md",
            "revision": revision.as_str(),
            "selection": { "kind": "all" }
        }],
        "returned_at": "2026-10-09T12:00:00.000Z",
        "audience": "human_display"
    }))?;
    storage
        .records()
        .insert_receipt(survivor, None, receipt)
        .await?;
    Ok(())
}

/// A connector that lists both workspaces.
async fn connect(
    storage: &Storage,
    workspaces: Vec<okf_jawn_contract::identity::WorkspaceId>,
) -> Fallible<()> {
    let issued = storage
        .credentials()
        .create_connector(
            uuid!(41)?,
            NewConnector {
                label: "Desk".to_owned(),
                workspace_ids: workspaces,
                allow_propose: false,
            },
        )
        .await?;
    match issued {
        ConnectorIssue::Issued(_) => Ok(()),
        ConnectorIssue::Existing(_) => Err("expected a new connector".into()),
    }
}

/// A completed mutation of the workspace whose stored response shows its name.
async fn remember(storage: &Storage, workspace: &StorageScope) -> Fallible<()> {
    let ledger = storage.mutations();
    let key = MutationKey {
        tenant_id: tenant()?,
        subject: "ana".to_owned(),
        client_id: None,
        operation: OperationName::UpdateWorkspace,
        key: uuid!(42)?,
    };
    let BeginOutcome::New(lease) = ledger
        .begin(
            &key,
            &Digest::try_from("a".repeat(64))?,
            Some(workspace.workspace_id),
        )
        .await?
    else {
        return Err("expected a new mutation".into());
    };
    let response = json!({ "name": "zanzibar trip" });
    ledger.complete(lease, response, None).await?;
    Ok(())
}

/// The target with everything a purge must reach, and a survivor that shares one object,
/// holds the target's archive in an upload slot and cites the target in a receipt.
async fn fixture(directory: &Path) -> Fallible<Fixture> {
    let storage = Storage::open(directory)?;
    let (target, created) = create(&storage, 1, "zanzibar trip").await?;
    let (survivor, survivor_head) = create(&storage, 2, "Garden").await?;
    let seeded = seed_git(&storage, &target, &survivor, (created, survivor_head)).await?;
    let base = some(seeded.revisions.get(1), "the seeded revision")?.clone();
    storage
        .drafts()
        .save(
            &target,
            uuid!(6)?,
            DraftWrite {
                item_id: uuid!(10)?,
                editor: "ana".to_owned(),
                base_revision: base.clone(),
                body: "Draft: zanzibar spice market".to_owned(),
                properties: BTreeMap::new(),
                content_digest: Digest::try_from("e".repeat(64))?,
            },
        )
        .await?;
    storage
        .search()
        .index_revision(&target, base.clone())
        .await?;
    let archive = b"zanzibar workspace archive";
    let archive = artifact(
        &storage,
        &target,
        30,
        ArtifactKind::WorkspaceBackup,
        archive,
    )
    .await?;
    let export = artifact(
        &storage,
        &target,
        32,
        ArtifactKind::Export,
        b"zanzibar export",
    )
    .await?;
    let history = storage
        .versions()
        .write_history(&target, &base, uuid!(7)?)
        .await?;
    upload(&storage, &target, 8, b"zanzibar upload part", false).await?;
    upload(&storage, &survivor, 9, b"zanzibar workspace archive", true).await?;
    cite(&storage, &survivor, &target, &base).await?;
    connect(&storage, vec![target.workspace_id, survivor.workspace_id]).await?;
    remember(&storage, &target).await?;
    let mut only_target = seeded.objects;
    only_target.extend([export, history.digest, archive.clone()]);
    Ok(Fixture {
        storage,
        target,
        survivor,
        revisions: seeded.revisions,
        shared: seeded.shared,
        only_target,
        archive,
    })
}

/// Copy a live database into the pre-migration directory as `migrate_database` does.
fn pre_migration_copy(data: &Path, file: &str, name: &str) -> Fallible<()> {
    let directory = data.join("backups").join("pre-migration");
    std::fs::create_dir_all(&directory)?;
    let target = directory.join(name);
    let connection = rusqlite::Connection::open(data.join(file))?;
    connection.execute("VACUUM INTO ?1", [some(target.to_str(), "a UTF-8 path")?])?;
    Ok(())
}

/// A pre-migration backup set `name` holding a workspace archive of each workspace, written by
/// the same writer an upgrade uses; the survivor's archive holds its receipt citing the target.
async fn pre_migration_set(data: &Path, fixture: &Fixture, name: &str) -> Fallible<()> {
    let directory = data.join("backups").join("pre-migration").join(name);
    let written = fixture
        .storage
        .backups()
        .write_backup_set(&directory, None)
        .await?;
    assert_eq!(
        written.len(),
        4,
        "two workspace archives, the tenant's installation archive and the installation-level one"
    );
    Ok(())
}

/// The bytes of one entry of the archive object `digest`.
fn archive_entry(data: &Path, digest: &Digest, name: &str) -> Fallible<Vec<u8>> {
    use std::io::Read as _;
    let path = data
        .join("objects")
        .join("local")
        .join(some(digest.as_str().get(..2), "a digest")?)
        .join(digest.as_str());
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let mut bytes = Vec::new();
    archive.by_name(name)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Every file under `directory` whose bytes contain the marker.
fn marked_files(directory: &Path) -> Fallible<Vec<String>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.file_name().is_some_and(|name| name == "okf-jawn.lock") {
            // Locked by this process on Windows, and empty.
            continue;
        }
        if path.is_dir() {
            found.extend(marked_files(&path)?);
        } else if std::fs::read(&path)?
            .windows(MARKER.len())
            .any(|window| window == MARKER)
        {
            found.push(path.display().to_string());
        }
    }
    Ok(found)
}

/// Record purge `purge` of `target`, as core does before it calls the purger.
async fn record_purge_of(storage: &Storage, target: &StorageScope, purge: PurgeId) -> Fallible<()> {
    storage
        .records()
        .create_purge(
            &tenant()?,
            uuid!(50)?,
            NewPurge {
                id: purge,
                target: PurgeTarget::Workspace {
                    workspace_id: target.workspace_id,
                },
                initiator: person("ana"),
            },
        )
        .await?;
    Ok(())
}

async fn record_purge(storage: &Storage, fixture: &Fixture, purge: PurgeId) -> Fallible<()> {
    storage
        .records()
        .create_purge(
            &tenant()?,
            uuid!(50)?,
            NewPurge {
                id: purge,
                target: PurgeTarget::Workspace {
                    workspace_id: fixture.target.workspace_id,
                },
                initiator: person("ana"),
            },
        )
        .await?;
    Ok(())
}

/// Whether the tenant's object store still holds `digest`.
async fn held(storage: &Storage, scope: &StorageScope, digest: &Digest) -> Fallible<bool> {
    match storage.blobs().open(scope, digest, 0, 1).await {
        Ok(_) => Ok(true),
        Err(error) if error.code == ErrorCode::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// What the purge removed and kept, and what it reported.
async fn assert_removed(fixture: &Fixture, report: &PurgeReport) -> TestResult {
    let storage = &fixture.storage;
    let repository = storage
        .data()
        .repositories_path()
        .join("local")
        .join(format!("{}.git", fixture.target.workspace_id.0));
    assert!(!repository.exists(), "the repository is deleted");
    assert_eq!(
        report.objects_removed,
        fixture.only_target.len().to_string()
    );
    assert_eq!(report.objects_kept_shared, "1");
    assert_eq!(
        report.backups_removed, 3,
        "its archive artifact, the index copy, the set's archive"
    );
    assert_eq!(
        report.backups_rewritten, 2,
        "the records copy, and the survivor's set archive that cites the target"
    );
    assert_eq!(report.exports_removed, 1);
    assert_eq!(report.drafts_removed, 1);
    assert_eq!(report.revisions_invalidated, 3);
    assert_eq!(report.citations_invalidated, 1);
    for digest in &fixture.only_target {
        assert!(
            !held(storage, &fixture.survivor, digest).await?,
            "{} is removed",
            digest.as_str()
        );
    }
    assert!(
        !held(storage, &fixture.survivor, &fixture.archive).await?,
        "an upload slot holding the target's archive is no surviving reference"
    );
    assert!(
        held(storage, &fixture.survivor, &fixture.shared).await?,
        "an object the survivor references stays"
    );
    let survivor_head = storage.versions().head(&fixture.survivor).await?;
    storage
        .versions()
        .show(&fixture.survivor, &survivor_head, uuid!(14)?)
        .await?;
    let error = err_of(storage.catalog().open(&fixture.target).await)?;
    assert_eq!(error.code, ErrorCode::NotFound);
    Ok(())
}

/// What the purge marked invalidated, scrubbed and left in place.
async fn assert_scrubbed(fixture: &Fixture, data: &Path, purge: PurgeId) -> TestResult {
    let storage = &fixture.storage;
    for revision in &fixture.revisions {
        let mapping = storage
            .records()
            .revision_mapping(&fixture.target, revision)
            .await?;
        let mapping = some(mapping, "the revision map entry")?;
        assert_eq!(mapping.purge_id, purge);
        assert!(
            mapping.replacement.is_none(),
            "nothing replaces a purged workspace"
        );
    }
    let receipt = storage
        .records()
        .get_receipt(&fixture.survivor, uuid!(40)?)
        .await?;
    assert_eq!(receipt.invalidated_by, Some(purge));
    let cited = some(receipt.sources.first(), "the cited source")?;
    assert_eq!(cited.path.as_str(), format!("purged/{}", uuid_text(10)));
    let connectors = storage.credentials().list_connectors(true).await?;
    let listed = some(connectors.first(), "the connector")?;
    assert_eq!(listed.workspace_ids, vec![fixture.survivor.workspace_id]);
    let kept = data
        .join("backups/pre-migration/set-1/workspaces/local")
        .join(format!("{}.zip", fixture.survivor.workspace_id.0));
    assert!(kept.is_file(), "the survivor's archive stays");
    let stored = storage.records().get_purge(&tenant()?, purge).await?;
    assert_eq!(
        stored.requested_by, "ana",
        "the purge record outlives the workspace"
    );
    Ok(())
}

#[tokio::test]
async fn purge_workspace_leaves_no_copy_the_application_manages() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    pre_migration_copy(directory.path(), "records.sqlite", "records-v1-1.sqlite")?;
    pre_migration_copy(directory.path(), "index.sqlite", "index-v1-1.sqlite")?;
    pre_migration_set(directory.path(), &fixture, "set-1").await?;
    assert!(
        !marked_files(directory.path())?.is_empty(),
        "the fixture writes the marker"
    );
    let purge: PurgeId = uuid!(60)?;
    record_purge(storage, &fixture, purge).await?;
    let mutation: MutationId = uuid!(61)?;
    let report = storage
        .purger()
        .purge_workspace(&tenant()?, mutation, purge, fixture.target.workspace_id)
        .await?;
    assert_eq!(
        marked_files(directory.path())?,
        Vec::<String>::new(),
        "no file under the data directory holds the target's content"
    );
    assert_removed(&fixture, &report).await?;
    assert_scrubbed(&fixture, directory.path(), purge).await?;
    let repeated = storage
        .purger()
        .purge_workspace(&tenant()?, mutation, purge, fixture.target.workspace_id)
        .await?;
    assert_eq!(
        repeated, report,
        "repeating a finished purge changes nothing"
    );
    Ok(())
}

/// Run a purge of the fixture's target to completion under `purge` (numbered 60).
async fn purge_target(fixture: &Fixture) -> Fallible<PurgeReport> {
    let purge: PurgeId = uuid!(60)?;
    record_purge(&fixture.storage, fixture, purge).await?;
    Ok(fixture
        .storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, purge, fixture.target.workspace_id)
        .await?)
}

/// The warnings kept for the commits of `workspace` in the records database at `records`: the
/// stored text of each row.
fn kept_warnings(records: &Path, workspace: &StorageScope) -> Fallible<Vec<String>> {
    let connection = rusqlite::Connection::open(records)?;
    let mut statement = connection.prepare(
        "SELECT warnings FROM commit_warnings WHERE tenant_id = ?1 AND workspace_id = ?2
         ORDER BY mutation_id",
    )?;
    let rows = statement
        .query_map(
            [
                workspace.tenant_id.as_str().to_owned(),
                workspace.workspace_id.0.to_string(),
            ],
            |row| row.get(0),
        )?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(rows)
}

/// R-I10: the warnings storage keeps for a commit's replay are rows of the workspace, and its
/// purge removes them from the live records and from a pre-migration copy; the survivor's stay,
/// and the survivor's commit still replays with its own. The target's row quotes the marked
/// path, so a table the purge skipped would leave the marker in both files.
#[tokio::test]
async fn purge_removes_the_warnings_kept_for_its_workspaces_commits() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let (target, survivor) = (&fixture.target, &fixture.survivor);
    pre_migration_copy(directory.path(), "records.sqlite", "records-v1-1.sqlite")?;
    let live = directory.path().join("records.sqlite");
    let copy = directory
        .path()
        .join("backups/pre-migration/records-v1-1.sqlite");
    let marker = String::from_utf8_lossy(MARKER).into_owned();
    for records in [&live, &copy] {
        let kept = kept_warnings(records, target)?;
        assert!(
            kept.iter().any(|row| row.contains(&marker)),
            "the fixture keeps a warning that quotes the marked path: {kept:?}"
        );
        assert_eq!(
            kept_warnings(records, survivor)?,
            ["[]"],
            "the survivor's one commit, which warned of nothing"
        );
    }
    purge_target(&fixture).await?;
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    for records in [&live, &copy] {
        assert_eq!(kept_warnings(records, target)?, Vec::<String>::new());
        assert_eq!(kept_warnings(records, survivor)?, ["[]"]);
    }
    // The survivor's commit (mutation 5 of the fixture) still replays, with what was kept.
    let head = fixture.storage.versions().head(survivor).await?;
    let replay = fixture
        .storage
        .versions()
        .find_commit(survivor, uuid!(5)?, None)
        .await?;
    let replay = some(replay, "the survivor's commit")?;
    assert_eq!(replay.revision, head);
    assert!(replay.warnings.is_empty(), "{:?}", replay.warnings);
    Ok(())
}

/// R-I10: a pre-migration records copy that holds nothing of the target but one kept warning,
/// which quotes the marker. The purge's "does this copy hold the workspace" reads the table as
/// its removal does, so the copy is rewritten and the marker is gone from every file.
/// (Reviewer probe PP1, adopted.)
#[tokio::test]
async fn purge_rewrites_a_copy_that_holds_only_a_kept_warning_of_its_workspace() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    // A records database of another, empty installation at this build's schema.
    let elsewhere = tempfile::tempdir()?;
    drop(Storage::open(elsewhere.path())?);
    pre_migration_copy(elsewhere.path(), "records.sqlite", "empty.sqlite")?;
    let copies = directory.path().join("backups").join("pre-migration");
    std::fs::create_dir_all(&copies)?;
    let copy = copies.join("records-v3-9.sqlite");
    std::fs::copy(
        elsewhere
            .path()
            .join("backups")
            .join("pre-migration")
            .join("empty.sqlite"),
        &copy,
    )?;
    let marker = String::from_utf8_lossy(MARKER).into_owned();
    rusqlite::Connection::open(&copy)?.execute(
        "INSERT INTO commit_warnings (tenant_id, workspace_id, mutation_id, commit_id, warnings)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        [
            fixture.target.tenant_id.as_str().to_owned(),
            fixture.target.workspace_id.0.to_string(),
            uuid_text(999),
            "c".repeat(40),
            format!("[{{\"code\":\"lint\",\"message\":\"{marker} trip.md has no heading\"}}]"),
        ],
    )?;
    assert_eq!(kept_warnings(&copy, &fixture.target)?.len(), 1);
    assert!(
        marked_files(directory.path())?.contains(&copy.display().to_string()),
        "the copy holds the marker before the purge"
    );
    purge_target(&fixture).await?;
    assert_eq!(
        kept_warnings(&copy, &fixture.target)?,
        Vec::<String>::new(),
        "the copy's one row of the workspace is removed"
    );
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    Ok(())
}

/// R-I10: a pre-migration records copy as records schema 2 left it, with no `commit_warnings`
/// table. The purge completes, no file keeps the marker, and the copy is still at schema 2.
/// (Reviewer probe PP2, adopted.)
#[tokio::test]
async fn purge_reaches_a_schema_2_copy_that_has_no_table_of_kept_warnings() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    pre_migration_copy(directory.path(), "records.sqlite", "records-v2-1.sqlite")?;
    let copy = directory
        .path()
        .join("backups")
        .join("pre-migration")
        .join("records-v2-1.sqlite");
    rusqlite::Connection::open(&copy)?
        .execute_batch("DROP TABLE commit_warnings; PRAGMA user_version = 2;")?;
    assert!(
        marked_files(directory.path())?.contains(&copy.display().to_string()),
        "the copy holds the marker before the purge"
    );
    purge_target(&fixture).await?;
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    let connection = rusqlite::Connection::open(&copy)?;
    let tables: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE name = 'commit_warnings'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(tables, 0, "the purge added no table to the copy");
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    assert_eq!(version, 2, "the copy is still at schema 2");
    Ok(())
}

/// Review probe R1: the backup handler failed (or its job was abandoned) after the archive was
/// written, so only `backup_archives` knows it.
#[tokio::test]
async fn purge_reaches_a_workspace_archive_no_artifact_records() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let archive = fixture
        .storage
        .backups()
        .write_workspace_archive(&fixture.target, uuid!(70)?)
        .await?;
    assert!(held(&fixture.storage, &fixture.survivor, &archive.digest).await?);
    let report = purge_target(&fixture).await?;
    assert!(
        !held(&fixture.storage, &fixture.survivor, &archive.digest).await?,
        "the unrecorded archive is removed"
    );
    assert_eq!(
        report.backups_removed, 2,
        "its recorded archive artifact and the unrecorded one"
    );
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    Ok(())
}

/// Review probe R3, widened: every field of a stored citation that can hold the target's text.
#[tokio::test]
async fn purge_scrubs_every_field_of_a_citation_that_can_hold_the_target() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let revision = some(fixture.revisions.get(1), "the seeded revision")?;
    let cell_range = json!({
        "sheet": "zanzibar sheet",
        "row_start": 1, "row_end": 2, "column_start": 1, "column_end": 2
    });
    let cited = |selection: serde_json::Value| {
        json!({
            "workspace_id": fixture.target.workspace_id,
            "item_id": uuid_text(10),
            "path": "notes/zanzibar-plans.md",
            "revision": revision.as_str(),
            "digest": "d".repeat(64),
            "selection": selection,
            "locations": [{
                "provenance": "direct",
                "locator": { "kind": "cells", "range": cell_range }
            }]
        })
    };
    let receipt: Receipt = serde_json::from_value(json!({
        "id": uuid_text(43),
        "workspace_id": fixture.survivor.workspace_id,
        "operation_id": "read_item",
        "principal_subject": "ana",
        "route": "local_owner",
        "sources": [
            cited(json!({ "kind": "section", "heading": "zanzibar ferry" })),
            cited(json!({ "kind": "cells", "range": cell_range }))
        ],
        "returned_at": "2026-10-09T12:00:00.000Z",
        "audience": "agent_context"
    }))?;
    fixture
        .storage
        .records()
        .insert_receipt(&fixture.survivor, None, receipt)
        .await?;
    let report = purge_target(&fixture).await?;
    assert_eq!(report.citations_invalidated, 3);
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    let stored = fixture
        .storage
        .records()
        .get_receipt(&fixture.survivor, uuid!(43)?)
        .await?;
    assert_eq!(stored.invalidated_by, Some(uuid!(60)?));
    assert_eq!(stored.sources.len(), 2);
    for source in &stored.sources {
        assert_eq!(source.path.as_str(), format!("purged/{}", uuid_text(10)));
        assert_eq!(source.selection, okf_jawn_contract::read::Selection::All);
        assert!(source.digest.is_none(), "the target's derived digest goes");
        assert!(source.locations.is_empty(), "its locations go");
        assert_eq!(source.revision, *revision, "the revision map resolves it");
    }
    Ok(())
}

/// Review probe R2: the survivor's own archive, taken before the purge, holds its receipt citing
/// the target. Restoring it must not bring the citation back.
#[tokio::test]
async fn purge_rewrites_another_workspaces_archive_that_cites_it() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let backups = fixture.storage.backups();
    let before = backups
        .write_workspace_archive(&fixture.survivor, uuid!(71)?)
        .await?;
    let report = purge_target(&fixture).await?;
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    assert_eq!(report.backups_rewritten, 1, "the survivor's archive");
    let after = backups
        .write_workspace_archive(&fixture.survivor, uuid!(71)?)
        .await?;
    assert_ne!(after.digest, before.digest, "the record names the rewrite");
    assert!(!held(&fixture.storage, &fixture.survivor, &before.digest).await?);
    let records = directory.path().join("rewritten.sqlite");
    std::fs::write(
        &records,
        archive_entry(directory.path(), &after.digest, "records.sqlite")?,
    )?;
    let receipt: String = rusqlite::Connection::open(&records)?.query_row(
        "SELECT record FROM receipts",
        [],
        |row| row.get(0),
    )?;
    let receipt: Receipt = serde_json::from_str(&receipt)?;
    assert_eq!(receipt.invalidated_by, Some(uuid!(60)?));
    let cited = some(receipt.sources.first(), "the archived citation")?;
    assert_eq!(cited.path.as_str(), format!("purged/{}", uuid_text(10)));
    Ok(())
}

/// Review probe R4: an upgrade after a failed attempt writes a pre-migration set that holds the
/// half-purged workspace; the resumed purge reaches it.
#[tokio::test]
async fn purge_reaches_a_pre_migration_set_written_after_a_failed_attempt() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    // A directory where the target's archive in a set would be: deleting it fails in step 2,
    // after the plan is recorded.
    let obstacle = directory
        .path()
        .join("backups/pre-migration/set-1/workspaces/local")
        .join(format!("{}.zip", fixture.target.workspace_id.0));
    std::fs::create_dir_all(obstacle.join("inside"))?;
    let purge: PurgeId = uuid!(60)?;
    record_purge(&fixture.storage, &fixture, purge).await?;
    let tenant = tenant()?;
    let purger = fixture.storage.purger();
    err_of(
        purger
            .purge_workspace(&tenant, uuid!(61)?, purge, fixture.target.workspace_id)
            .await,
    )?;
    std::fs::remove_dir_all(&obstacle)?;
    pre_migration_set(directory.path(), &fixture, "set-2").await?;
    let report = purger
        .purge_workspace(&tenant, uuid!(61)?, purge, fixture.target.workspace_id)
        .await?;
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    assert!(report.backups_removed >= 2, "{report:?}");
    Ok(())
}

/// A completed ledger row of `tenant` for `operation` under key `n`, recording `workspace`.
async fn completed_row(
    storage: &Storage,
    (tenant, operation, n): (&str, OperationName, u32),
    workspace: Option<okf_jawn_contract::identity::WorkspaceId>,
    response: serde_json::Value,
) -> Fallible<()> {
    let ledger = storage.mutations();
    let key = MutationKey {
        tenant_id: TenantId::try_from(tenant.to_owned())?,
        subject: "ana".to_owned(),
        client_id: None,
        operation,
        key: uuid!(n)?,
    };
    let BeginOutcome::New(lease) = ledger
        .begin(&key, &Digest::try_from("a".repeat(64))?, workspace)
        .await?
    else {
        return Err("expected a new mutation".into());
    };
    ledger.complete(lease, response, None).await?;
    Ok(())
}

/// Put a records database back as records schema 1 left it: without the tables schemas 2 and 3 add,
/// without the ledger's workspace column, and with the events table as it was before event keys.
fn as_records_schema_1(records: &Path) -> TestResult {
    let connection = rusqlite::Connection::open(records)?;
    connection.execute_batch(
        "DROP TABLE commit_warnings;
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

/// The ledger rows of the records database at `records`, in key order: tenant, operation,
/// state, and whether the row holds a response.
fn ledger_of(records: &Path) -> Fallible<Vec<(String, String, String, bool)>> {
    let connection = rusqlite::Connection::open(records)?;
    let mut statement = connection.prepare(
        "SELECT tenant_id, operation, state, response IS NOT NULL FROM mutations
         ORDER BY idempotency_key",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Round 1, item 2: a pre-migration copy from before records schema 2 has a ledger without the
/// workspace column, so no row of it can be attributed to a workspace. There the purge empties
/// every completed row of the tenant except a purge's; the fixture's own row (`remember`) holds
/// the target's name and not its identity, so nothing short of that reaches it.
#[tokio::test]
async fn purge_empties_the_ledger_of_a_copy_that_records_no_workspace() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let storage = &fixture.storage;
    let (target, survivor) = (fixture.target.workspace_id, fixture.survivor.workspace_id);
    let update = OperationName::UpdateWorkspace;
    let purge_record = json!({ "id": uuid_text(60), "state": "requested" });
    completed_row(
        storage,
        ("local", update, 43),
        Some(survivor),
        json!({ "name": "Garden" }),
    )
    .await?;
    completed_row(
        storage,
        ("elsewhere", update, 44),
        None,
        json!({ "name": "Orchard" }),
    )
    .await?;
    completed_row(
        storage,
        ("local", OperationName::PurgeWorkspace, 45),
        Some(target),
        purge_record,
    )
    .await?;
    pre_migration_copy(directory.path(), "records.sqlite", "records-v1-1.sqlite")?;
    let copy = directory
        .path()
        .join("backups/pre-migration/records-v1-1.sqlite");
    as_records_schema_1(&copy)?;
    purge_target(&fixture).await?;
    assert_eq!(
        marked_files(directory.path())?,
        Vec::<String>::new(),
        "no byte of the copy holds the target's name"
    );
    let row = |tenant: &str, operation: OperationName, state: &str, response: bool| {
        (
            tenant.to_owned(),
            operation.as_str().to_owned(),
            state.to_owned(),
            response,
        )
    };
    let purge = OperationName::PurgeWorkspace;
    // In the copy no row names a workspace: the tenant's rows are emptied, the survivor's too.
    assert_eq!(
        ledger_of(&copy)?,
        [
            row("local", update, "released", false),
            row("local", update, "released", false),
            row("elsewhere", update, "completed", true),
            row("local", purge, "completed", true),
        ]
    );
    // The live ledger records each row's workspace, so only the target's row is emptied.
    assert_eq!(
        ledger_of(&directory.path().join("records.sqlite"))?,
        [
            row("local", update, "released", false),
            row("local", update, "completed", true),
            row("elsewhere", update, "completed", true),
            row("local", purge, "completed", true),
        ]
    );
    Ok(())
}

/// The row under the idempotency key numbered `n`.
fn ledger_row(records: &Path, n: u32) -> Fallible<RowState> {
    let connection = rusqlite::Connection::open(records)?;
    Ok(connection.query_row(
        "SELECT state, token, lease_expires_ms, response, finished_ms FROM mutations
         WHERE idempotency_key = ?1",
        [uuid_text(n)],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?)
}

/// The key of a ledger row of `tenant` for `operation` numbered `n`.
fn ledger_key(tenant: &str, operation: OperationName, n: u32) -> Fallible<MutationKey> {
    Ok(MutationKey {
        tenant_id: TenantId::try_from(tenant.to_owned())?,
        subject: "ana".to_owned(),
        client_id: None,
        operation,
        key: uuid!(n)?,
    })
}

/// Round 2, B1: a row of the purged workspace that is still leased is an attempt in flight. The
/// purge leaves its state, lease and token alone, so its holder can still complete or release it
/// (SPEC section 8: a superseded attempt can neither complete nor release the mutation, and
/// this one was not superseded).
#[tokio::test]
async fn purge_leaves_a_leased_ledger_row_of_the_workspace_to_its_holder() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let (ledger, target) = (fixture.storage.mutations(), fixture.target.workspace_id);
    let digest = Digest::try_from("a".repeat(64))?;
    let update = OperationName::UpdateWorkspace;
    let mut leases = Vec::new();
    for n in [50, 51] {
        let BeginOutcome::New(lease) = ledger
            .begin(&ledger_key("local", update, n)?, &digest, Some(target))
            .await?
        else {
            return Err("expected a new mutation".into());
        };
        leases.push(lease);
    }
    let records = directory.path().join("records.sqlite");
    let (completing, releasing) = (ledger_row(&records, 50)?, ledger_row(&records, 51)?);
    assert_eq!(completing.0, "leased");
    purge_target(&fixture).await?;
    assert_eq!(ledger_row(&records, 50)?, completing, "state, lease, token");
    assert_eq!(ledger_row(&records, 51)?, releasing, "state, lease, token");
    let (first, second) = (
        *some(leases.first(), "the first lease")?,
        *some(leases.get(1), "the second lease")?,
    );
    ledger
        .complete(first, json!({ "name": "Late" }), None)
        .await?;
    ledger.release(second).await?;
    let done = ledger_row(&records, 50)?;
    assert_eq!((done.0.as_str(), done.3.is_some()), ("completed", true));
    assert_eq!(ledger_row(&records, 51)?.0, "released");
    Ok(())
}

/// Round 2, B1: a completed `purge_item` row recorded under the purged workspace keeps its
/// response, which is a purge record, as the `purge_workspace` row does.
#[tokio::test]
async fn purge_keeps_a_completed_purge_item_row_of_the_workspace() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    completed_row(
        &fixture.storage,
        ("local", OperationName::PurgeItem, 52),
        Some(fixture.target.workspace_id),
        json!({ "id": uuid_text(62), "state": "requested" }),
    )
    .await?;
    purge_target(&fixture).await?;
    let row = ledger_row(&directory.path().join("records.sqlite"), 52)?;
    assert_eq!(row.0, "completed");
    assert!(row.3.is_some(), "the purge record is kept");
    Ok(())
}

/// Round 2, B1: another tenant's completed row that names a workspace of the same id keeps its
/// response; a purge empties its own tenant's rows only.
#[tokio::test]
async fn purge_keeps_another_tenant_s_ledger_row_that_names_the_same_workspace_id() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    completed_row(
        &fixture.storage,
        ("elsewhere", OperationName::UpdateWorkspace, 53),
        Some(fixture.target.workspace_id),
        json!({ "name": "Orchard" }),
    )
    .await?;
    purge_target(&fixture).await?;
    let row = ledger_row(&directory.path().join("records.sqlite"), 53)?;
    assert_eq!(row.0, "completed");
    assert!(row.3.is_some(), "the other tenant's response is kept");
    Ok(())
}

/// Assert that `result` is the refusal of a write to a workspace purge 60 names.
fn refused<T: std::fmt::Debug>(result: Result<T, ApiError>, what: &str) -> TestResult {
    let error = err_of(result)?;
    assert_eq!(error.code, ErrorCode::Conflict, "{what}: {error}");
    assert!(
        error.message.contains(&uuid_text(60)),
        "{what}: the refusal names the purge: {error}"
    );
    Ok(())
}

/// The upload writes of the refusal test: each refused. `slot` is open and `complete` holds a
/// retained object no job consumed, so only the purge refuses a batch of it, and it stays
/// unconsumed.
async fn refuse_uploads(
    storage: &Storage,
    target: &StorageScope,
    slot: UploadId,
    complete: UploadId,
) -> TestResult {
    let uploads = storage.uploads();
    refused(
        uploads.consume_batch(target, &[complete], uuid!(70)?).await,
        "an upload batch",
    )?;
    let unconsumed: Option<String> = rusqlite::Connection::open(storage.data().records_path())?
        .query_row(
            "SELECT consumed_by FROM uploads WHERE upload_id = ?1",
            [complete.0.to_string()],
            |row| row.get(0),
        )?;
    assert_eq!(unconsumed, None, "the refused batch marked nothing");
    refused(
        uploads
            .create(
                target,
                uuid!(9)?,
                NewUpload {
                    filename: "a.zip".to_owned(),
                    relative_path: String::new(),
                    expected_size: 1,
                    expected_sha256: None,
                    supplied_by: person("ana"),
                },
            )
            .await,
        "an upload slot",
    )?;
    refused(
        uploads
            .put_content(target, slot, Box::pin(std::io::Cursor::new(Vec::new())), 1)
            .await,
        "upload content",
    )?;
    refused(
        uploads
            .complete(target, slot, Digest::try_from(sha256(b""))?)
            .await,
        "an upload completion",
    )?;
    Ok(())
}

/// The record writes of the refusal test: each refused.
async fn refuse_records(
    storage: &Storage,
    target: &StorageScope,
    head: &Revision,
    claimed: ClaimedJob,
    object: &ObjectInfo,
) -> TestResult {
    let records = storage.records();
    let job_scope = JobScope::Workspace(target.clone());
    refused(
        records
            .record_artifact(
                &job_scope,
                uuid!(31)?,
                NewArtifact {
                    kind: ArtifactKind::Export,
                    object: object.clone(),
                    media_type: "application/zip".to_owned(),
                    created_by_job: claimed.job.id,
                },
            )
            .await,
        "an artifact",
    )?;
    refused(
        records
            .complete_job(JobCompletion {
                lease: claimed.lease.clone(),
                revision: None,
                item_ids: Vec::new(),
                artifact: None,
                restore: None,
                outputs: vec![object.digest.clone()],
                warnings: Vec::new(),
            })
            .await,
        "a job completion",
    )?;
    refused(
        records
            .record_derived_object(
                target,
                DerivedObject {
                    item_id: uuid!(10)?,
                    revision: head.clone(),
                    digest: object.digest.clone(),
                    kind: DerivedKind::Dataset,
                    media_type: "text/csv".to_owned(),
                },
            )
            .await,
        "a derived object",
    )?;
    refused(
        storage
            .backups()
            .write_workspace_archive(target, uuid!(32)?)
            .await,
        "a workspace archive",
    )?;
    Ok(())
}

/// Review B6, window 1: from the moment the purge is recorded, storage refuses every write to
/// the workspace it names; the other workspace goes on.
#[tokio::test]
async fn purge_refuses_every_write_to_the_workspace_it_names() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, head) = create(&storage, 1, "Trip").await?;
    let (survivor, _) = create(&storage, 2, "Garden").await?;
    let job_scope = JobScope::Workspace(target.clone());
    let records = storage.records();
    let job = records
        .create_job(
            &job_scope,
            NewJob {
                mutation_id: uuid!(30)?,
                initiator: person("ana"),
                spec: JobSpec::BackupWorkspace,
            },
        )
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "the claimed job")?;
    let slot = upload(&storage, &target, 8, b"", false).await?;
    let complete = upload(&storage, &target, 7, b"an import's source", true).await?;
    let object = put(&storage, &target, b"before the purge").await?;
    let purge: PurgeId = uuid!(60)?;
    record_purge_of(&storage, &target, purge).await?;

    refused(
        storage
            .blobs()
            .put(
                &target,
                Box::pin(std::io::Cursor::new(b"after".to_vec())),
                1 << 20,
                None,
            )
            .await,
        "an object put",
    )?;
    refused(
        storage
            .versions()
            .commit(
                &target,
                CommitChanges {
                    mutation_id: uuid!(3)?,
                    expected_head: head.clone(),
                    author: person("ana"),
                    message: "Seed".to_owned(),
                    edits: vec![note(uuid!(10)?, "a.md", "x")?],
                },
                Arc::new(Accept),
            )
            .await,
        "a commit",
    )?;
    refused(
        storage
            .versions()
            .create_candidate(
                &target,
                uuid!(20)?,
                CandidateChanges {
                    mutation_id: uuid!(4)?,
                    base: head.clone(),
                    author: person("ana"),
                    message: "Propose".to_owned(),
                    edits: vec![note(uuid!(11)?, "b.md", "y")?],
                },
                Arc::new(Accept),
            )
            .await,
        "a candidate",
    )?;
    refused(
        storage
            .drafts()
            .save(
                &target,
                uuid!(6)?,
                DraftWrite {
                    item_id: uuid!(10)?,
                    editor: "ana".to_owned(),
                    base_revision: head.clone(),
                    body: "Draft".to_owned(),
                    properties: BTreeMap::new(),
                    content_digest: Digest::try_from("e".repeat(64))?,
                },
            )
            .await,
        "a draft",
    )?;
    refuse_uploads(&storage, &target, slot, complete).await?;
    refuse_records(&storage, &target, &head, claimed, &object).await?;
    put(&storage, &survivor, b"the other workspace goes on").await?;
    Ok(())
}

/// The SHA-256 digest of `bytes`.
fn digest_of(bytes: &[u8]) -> Fallible<Digest> {
    Ok(Digest::try_from(sha256(bytes))?)
}

/// Ruling 2: an upload completion that stored its bytes and whose record then failed abandons
/// its pin; the purge of the workspace deletes those bytes, which no record names.
#[tokio::test]
async fn purge_removes_the_bytes_of_an_upload_completion_whose_record_failed() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let bytes = b"zanzibar upload whose record failed";
    let slot = upload(&storage, &target, 8, bytes, false).await?;
    let records = rusqlite::Connection::open(directory.path().join("records.sqlite"))?;
    records.execute_batch(
        "CREATE TRIGGER stop_upload BEFORE UPDATE OF object_digest ON uploads
         BEGIN SELECT RAISE(ABORT, 'the record write failed'); END;",
    )?;
    let digest = digest_of(bytes)?;
    err_of(
        storage
            .uploads()
            .complete(&target, slot, digest.clone())
            .await,
    )?;
    records.execute_batch("DROP TRIGGER stop_upload;")?;
    drop(records);
    assert!(
        held(&storage, &target, &digest).await?,
        "the bytes were stored"
    );
    let purge: PurgeId = uuid!(60)?;
    record_purge_of(&storage, &target, purge).await?;
    storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, purge, target.workspace_id)
        .await?;
    assert!(
        !held(&storage, &target, &digest).await?,
        "the purge deletes bytes no record names"
    );
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    Ok(())
}

/// Ruling 2: the purge refusal of an upload completion is checked inside its record's
/// transaction. A trigger records the purge inside that transaction, as a purge recorded
/// after the completion's earlier checks and before its record commits.
#[tokio::test]
async fn purge_refuses_an_upload_record_inside_its_transaction() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let bytes = b"zanzibar upload recorded late";
    let slot = upload(&storage, &target, 8, bytes, false).await?;
    let records = rusqlite::Connection::open(directory.path().join("records.sqlite"))?;
    records.execute(
        &format!(
            "CREATE TRIGGER late_purge AFTER UPDATE OF object_digest ON uploads
             BEGIN INSERT INTO purges (purge_id, tenant_id, target_key, state, mutation_id,
                 record) VALUES ('{}', 'local', 'workspace/{}', 'running', '{}', '{{}}'); END;",
            uuid_text(60),
            target.workspace_id.0,
            uuid_text(50)
        ),
        [],
    )?;
    refused(
        storage
            .uploads()
            .complete(&target, slot, digest_of(bytes)?)
            .await,
        "an upload record",
    )?;
    records.execute_batch("DROP TRIGGER late_purge;")?;
    let stored = storage.uploads().get(&target, slot).await?;
    assert!(stored.object.is_none(), "the refused record did not land");
    Ok(())
}

/// Review B6, window 2, exactly: another workspace's import stores an object identical to one
/// of the target's before the objects step and lands its record after it. That object stays.
/// And an object the target itself stored without a record yet goes with the target.
#[tokio::test]
async fn purge_keeps_an_object_another_workspace_stores_before_and_references_after() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, head) = create(&storage, 1, "Trip").await?;
    let (survivor, survivor_head) = create(&storage, 2, "Garden").await?;
    let original = put(&storage, &target, b"zanzibar original bytes").await?;
    commit(
        &storage,
        &target,
        3,
        &head,
        vec![card(uuid!(11)?, "sources/scan.md", &original.digest, None)?],
    )
    .await?;
    let orphan = put(&storage, &target, b"zanzibar stored, never referenced").await?;
    // The survivor's import job stores the same bytes; its card lands only later.
    let imported = put(&storage, &survivor, b"zanzibar original bytes").await?;
    assert_eq!(imported.digest, original.digest);
    let purge: PurgeId = uuid!(60)?;
    record_purge_of(&storage, &target, purge).await?;

    let report = storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, purge, target.workspace_id)
        .await?;
    assert!(
        held(&storage, &survivor, &original.digest).await?,
        "the survivor's in-flight object stays"
    );
    assert!(
        !held(&storage, &survivor, &orphan.digest).await?,
        "the target's own unreferenced object goes"
    );
    assert_eq!(report.objects_kept_shared, "1");
    commit(
        &storage,
        &survivor,
        4,
        &survivor_head,
        vec![card(uuid!(12)?, "sources/scan.md", &imported.digest, None)?],
    )
    .await?;
    storage
        .blobs()
        .open(&survivor, &imported.digest, 0, 1)
        .await?;
    Ok(())
}

/// Ruling 3: a release frees a pin of its own workspace only. The survivor stores an object
/// first (its record lands later); the target stores the same bytes and commits the card
/// naming them, which releases the target's pin, not the survivor's. So the purge keeps it.
#[tokio::test]
async fn purge_keeps_an_object_whose_identical_pin_another_workspace_released() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, head) = create(&storage, 1, "Trip").await?;
    let (survivor, survivor_head) = create(&storage, 2, "Garden").await?;
    let bytes = b"commonplace bytes stored twice";
    let pending = put(&storage, &survivor, bytes).await?;
    let original = put(&storage, &target, bytes).await?;
    assert_eq!(pending.digest, original.digest);
    commit(
        &storage,
        &target,
        3,
        &head,
        vec![card(uuid!(11)?, "sources/scan.md", &original.digest, None)?],
    )
    .await?;
    let purge: PurgeId = uuid!(60)?;
    record_purge_of(&storage, &target, purge).await?;
    storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, purge, target.workspace_id)
        .await?;
    assert!(
        held(&storage, &survivor, &pending.digest).await?,
        "the survivor's pending object stays"
    );
    commit(
        &storage,
        &survivor,
        4,
        &survivor_head,
        vec![card(uuid!(12)?, "sources/scan.md", &pending.digest, None)?],
    )
    .await?;
    Ok(())
}

/// A proposal of `target` numbered `n` on `head`.
fn proposal(target: &StorageScope, n: u32, head: &Revision) -> Fallible<Proposal> {
    Ok(serde_json::from_value(json!({
        "id": uuid_text(n),
        "workspace_id": target.workspace_id,
        "base_revision": head,
        "proposal_revision": head,
        "content_digest": "d".repeat(64),
        "title": "zanzibar proposal",
        "description": "zanzibar reasons",
        "changes": [],
        "kind": "content",
        "status": "open",
        "created_by": "agent",
        "created_via": "mcp_delegation",
        "created_at": "2026-10-09T12:00:00.000Z"
    }))?)
}

/// A confirmation challenge for item 10 at `head`.
fn challenge(head: &Revision) -> Fallible<ConfirmationCreate> {
    Ok(ConfirmationCreate {
        action: ConfirmationAction::Review,
        target: ConfirmationTarget::Item {
            item_id: uuid!(10)?,
        },
        revision: head.clone(),
        content_digest: Digest::try_from("d".repeat(64))?,
        session_id: "session-1".to_owned(),
        subject: "ana".to_owned(),
        expires_at: serde_json::from_value(json!("2099-01-01T00:00:00.000Z"))?,
    })
}

/// Each write of a proposal, a comment and a confirmation to `target`: each refused.
async fn refuse_discussion(
    storage: &Storage,
    target: &StorageScope,
    head: &Revision,
    open: &Proposal,
    issued: ConfirmationId,
    when: &str,
) -> TestResult {
    let proposals = storage.proposals();
    refused(
        proposals
            .insert(target, uuid!(23)?, proposal(target, 24, head)?)
            .await,
        &format!("a proposal {when}"),
    )?;
    let mut changed = open.clone();
    changed.status = ProposalStatus::Declined;
    refused(
        proposals.update(target, changed).await,
        &format!("a proposal update {when}"),
    )?;
    let comment: Comment = serde_json::from_value(json!({
        "id": "c1",
        "author": "ana",
        "text": "zanzibar comment",
        "created_at": "2026-10-09T12:00:00.000Z"
    }))?;
    refused(
        proposals
            .add_comment(target, uuid!(25)?, open.id, comment)
            .await,
        &format!("a comment {when}"),
    )?;
    let confirmations = storage.confirmations();
    refused(
        confirmations
            .create(target, uuid!(26)?, challenge(head)?)
            .await,
        &format!("a confirmation {when}"),
    )?;
    let presented = ConfirmationConsume {
        confirmation_id: issued,
        action: ConfirmationAction::Review,
        target: ConfirmationTarget::Item {
            item_id: uuid!(10)?,
        },
        revision: head.clone(),
        content_digest: Digest::try_from("d".repeat(64))?,
        session_id: "session-1".to_owned(),
        subject: "ana".to_owned(),
    };
    refused(
        confirmations.consume(target, uuid!(27)?, presented).await,
        &format!("a confirmation's use {when}"),
    )?;
    Ok(())
}

/// Ruling 4: proposals, comments and confirmations carry the workspace's free text and come
/// from synchronous requests, not jobs; each is refused inside its own transaction from the
/// moment the purge is recorded, and after the purge completes.
#[tokio::test]
async fn purge_refuses_proposals_comments_and_confirmations_during_and_after() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, head) = create(&storage, 1, "Trip").await?;
    let open = storage
        .proposals()
        .insert(&target, uuid!(20)?, proposal(&target, 21, &head)?)
        .await?;
    let issued = storage
        .confirmations()
        .create(&target, uuid!(22)?, challenge(&head)?)
        .await?;
    let purge: PurgeId = uuid!(60)?;
    record_purge_of(&storage, &target, purge).await?;
    refuse_discussion(
        &storage,
        &target,
        &head,
        &open,
        issued.id,
        "during the purge",
    )
    .await?;
    storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, purge, target.workspace_id)
        .await?;
    refuse_discussion(
        &storage,
        &target,
        &head,
        &open,
        issued.id,
        "after the purge",
    )
    .await?;
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    Ok(())
}

/// Target rows of every kind that names an object (a dataset, a job output with its job's
/// control, a sandbox capability), each over an object only that row names; returns them.
async fn every_row_kind(fixture: &Fixture) -> Fallible<Vec<Digest>> {
    let (storage, target) = (&fixture.storage, &fixture.target);
    let revision = some(fixture.revisions.get(1), "the seeded revision")?.clone();
    let dataset = put(storage, target, b"zanzibar dataset").await?.digest;
    storage
        .records()
        .record_derived_object(
            target,
            DerivedObject {
                item_id: uuid!(11)?,
                revision: revision.clone(),
                digest: dataset.clone(),
                kind: DerivedKind::Dataset,
                media_type: "text/csv".to_owned(),
            },
        )
        .await?;
    let output = put(storage, target, b"zanzibar job output").await?.digest;
    let job_scope = JobScope::Workspace(target.clone());
    let records = storage.records();
    for (mutation, finish) in [(33_u32, true), (34, false)] {
        let job = records
            .create_job(
                &job_scope,
                NewJob {
                    mutation_id: uuid!(mutation)?,
                    initiator: person("ana"),
                    spec: JobSpec::BackupWorkspace,
                },
            )
            .await?;
        if finish {
            let claimed = some(records.claim_job(job.id).await?, "the claimed job")?;
            records
                .complete_job(JobCompletion {
                    lease: claimed.lease,
                    revision: None,
                    item_ids: Vec::new(),
                    artifact: None,
                    restore: None,
                    outputs: vec![output.clone()],
                    warnings: Vec::new(),
                })
                .await?;
        } else {
            records.cancel_job(&job_scope, uuid!(35)?, job.id).await?;
        }
    }
    let rendered = put(storage, target, b"zanzibar sandbox render")
        .await?
        .digest;
    storage
        .sandbox()
        .mint(
            target,
            [7; 32],
            okf_jawn_core::sandbox::SandboxMint {
                item_id: uuid!(11)?,
                revision,
                object: rendered.clone(),
                media_type: "text/html".to_owned(),
                expires_at: serde_json::from_value(json!("2099-01-01T00:00:00.000Z"))?,
            },
        )
        .await?;
    Ok(vec![dataset, output, rendered])
}

/// Review B7 (M01-M04, M13): every row kind that names an object is reached, and an asset the
/// survivor reaches only through its own conversion record stays.
#[tokio::test]
async fn purge_reaches_every_row_kind_and_keeps_a_survivors_converted_asset() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let (storage, survivor) = (&fixture.storage, &fixture.survivor);
    let only_rows = every_row_kind(&fixture).await?;
    // The asset: the target names it through a dataset row, the survivor only through the
    // conversion record its card names.
    let asset = put(storage, &fixture.target, b"commonplace page image")
        .await?
        .digest;
    storage
        .records()
        .record_derived_object(
            &fixture.target,
            DerivedObject {
                item_id: uuid!(11)?,
                revision: some(fixture.revisions.get(1), "the seeded revision")?.clone(),
                digest: asset.clone(),
                kind: DerivedKind::Dataset,
                media_type: "image/png".to_owned(),
            },
        )
        .await?;
    let original = put(storage, survivor, b"the survivor's own scan")
        .await?
        .digest;
    let record = json!({ "assets": [{ "digest": asset.as_str() }] }).to_string();
    let record = put(storage, survivor, record.as_bytes()).await?.digest;
    let head = storage.versions().head(survivor).await?;
    let converted = card(uuid!(15)?, "sources/own.md", &original, Some(&record))?;
    commit(storage, survivor, 16, &head, vec![converted]).await?;
    // After a restart no pin is held (a sandbox capability releases none), so each row kind
    // alone must name its object.
    let Fixture {
        storage,
        target,
        survivor,
        ..
    } = fixture;
    drop(storage);
    let storage = &Storage::open(directory.path())?;
    let survivor = &survivor;
    record_purge_of(storage, &target, uuid!(60)?).await?;
    storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, uuid!(60)?, target.workspace_id)
        .await?;
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    for digest in &only_rows {
        assert!(!held(storage, survivor, digest).await?, "{digest:?} goes");
    }
    assert!(
        held(storage, survivor, &asset).await?,
        "an asset the survivor's conversion record names stays"
    );
    Ok(())
}

/// Review B7 (M05): a row of the target that lands after the rows step (a write that passed the
/// refusal before the purge was recorded) is removed by the rows pass after the repository
/// goes. The purge is stopped at the index step, the row is written, the purge is resumed.
#[tokio::test]
async fn purge_removes_a_row_written_after_its_rows_step() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let index = rusqlite::Connection::open(directory.path().join("index.sqlite"))?;
    index.execute_batch(
        "CREATE TRIGGER stop_purge BEFORE DELETE ON index_items
         BEGIN SELECT RAISE(ABORT, 'stopped at the index step'); END;",
    )?;
    let purge: PurgeId = uuid!(60)?;
    record_purge(&fixture.storage, &fixture, purge).await?;
    let purger = fixture.storage.purger();
    let target = fixture.target.workspace_id;
    err_of(
        purger
            .purge_workspace(&tenant()?, uuid!(61)?, purge, target)
            .await,
    )?;
    {
        let records = rusqlite::Connection::open(directory.path().join("records.sqlite"))?;
        records.execute(
            "INSERT INTO receipts (receipt_id, tenant_id, workspace_id, mutation_id, record)
             VALUES (?1, 'local', ?2, NULL, '{\"late\": \"zanzibar\"}')",
            [uuid_text(44), target.0.to_string()],
        )?;
    }
    index.execute_batch("DROP TRIGGER stop_purge;")?;
    drop(index);
    purger
        .purge_workspace(&tenant()?, uuid!(61)?, purge, target)
        .await?;
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    Ok(())
}

#[tokio::test]
async fn purge_workspace_resumes_by_repeating_after_a_failure() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let (survivor, _) = create(&storage, 2, "Garden").await?;
    let export = artifact(
        &storage,
        &target,
        30,
        ArtifactKind::Export,
        b"zanzibar export",
    )
    .await?;
    let slot = upload(&storage, &target, 8, b"", false).await?;
    // A directory where the upload's part file would be: removing it fails, after the rows
    // step has removed the workspace's rows and before the purge can go on.
    let obstacle = directory
        .path()
        .join("uploads")
        .join(format!("{}.part", slot.0));
    std::fs::create_dir_all(obstacle.join("inside"))?;
    let purge: PurgeId = uuid!(60)?;
    let tenant = tenant()?;
    err_of(
        storage
            .purger()
            .purge_workspace(&tenant, uuid!(61)?, purge, target.workspace_id)
            .await,
    )?;
    let error = err_of(storage.catalog().open(&target).await)?;
    assert_eq!(
        error.code,
        ErrorCode::NotFound,
        "the rows step ran before the failure"
    );
    let error = err_of(
        storage
            .purger()
            .purge_workspace(&tenant, uuid!(61)?, purge, survivor.workspace_id)
            .await,
    )?;
    assert_eq!(
        error.code,
        ErrorCode::Conflict,
        "a purge identity resumes only the workspace it plans"
    );
    std::fs::remove_dir_all(&obstacle)?;
    let report = storage
        .purger()
        .purge_workspace(&tenant, uuid!(61)?, purge, target.workspace_id)
        .await?;
    assert_eq!(
        report.exports_removed, 1,
        "the report counts what the plan found"
    );
    assert_eq!(report.objects_removed, "1");
    let error = err_of(
        storage
            .blobs()
            .open(&survivor, &export, 0, 1)
            .await
            .map(|read| read.object),
    )?;
    assert_eq!(
        error.code,
        ErrorCode::NotFound,
        "the resumed purge removes what the plan found before the rows went"
    );
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    Ok(())
}

#[tokio::test]
async fn a_purged_workspace_identity_is_never_created_again() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let purge: PurgeId = uuid!(60)?;
    record_purge_of(&storage, &target, purge).await?;

    storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, purge, target.workspace_id)
        .await?;
    let error = err_of(create(&storage, 1, "Trip").await)?;
    assert!(
        error.to_string().contains("purged"),
        "a retried creation does not revive the purged identity: {error}"
    );
    Ok(())
}

#[tokio::test]
async fn item_purge_is_declared_and_not_built_yet() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let error = err_of(
        storage
            .purger()
            .purge_item(&target, uuid!(61)?, uuid!(60)?, uuid!(10)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotImplemented);
    Ok(())
}

/// Copy every file under `from` into `to`.
fn copy_tree(from: &Path, to: &Path) -> Fallible<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let path = entry?.path();
        let target = to.join(some(path.file_name(), "a file name")?);
        if path.is_dir() {
            copy_tree(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

/// Ruling 5: a managed backup that appears after the backups step (an upgrade between two
/// attempts writes a pre-migration set that still holds the workspace) is reached by the
/// backups pass the purge runs again before it completes. The resumed attempt starts after the
/// backups step, so only that last pass reaches the set.
#[tokio::test]
async fn purge_reaches_a_pre_migration_set_written_after_its_backups_step() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path()).await?;
    let elsewhere = tempfile::tempdir()?;
    let set = elsewhere.path().join("set-2");
    fixture
        .storage
        .backups()
        .write_backup_set(&set, None)
        .await?;
    let index = rusqlite::Connection::open(directory.path().join("index.sqlite"))?;
    index.execute_batch(
        "CREATE TRIGGER stop_purge BEFORE DELETE ON index_items
         BEGIN SELECT RAISE(ABORT, 'stopped at the index step'); END;",
    )?;
    let purge: PurgeId = uuid!(60)?;
    record_purge(&fixture.storage, &fixture, purge).await?;
    let purger = fixture.storage.purger();
    let target = fixture.target.workspace_id;
    err_of(
        purger
            .purge_workspace(&tenant()?, uuid!(61)?, purge, target)
            .await,
    )?;
    copy_tree(&set, &directory.path().join("backups/pre-migration/set-2"))?;
    index.execute_batch("DROP TRIGGER stop_purge;")?;
    drop(index);
    let report = purger
        .purge_workspace(&tenant()?, uuid!(61)?, purge, target)
        .await?;
    assert_eq!(
        marked_files(directory.path())?,
        Vec::<String>::new(),
        "the late set is reached: {report:?}"
    );
    Ok(())
}

/// Upload content arrives in no job: the purge refusal of its byte count is checked inside the
/// count's transaction, and a refused call removes the part file it appended to (it may have
/// recreated it after the purge removed the workspace's upload files). A trigger records the
/// purge inside that transaction, as a purge recorded while the body arrived.
#[tokio::test]
async fn purge_refuses_upload_content_inside_its_transaction_and_keeps_no_part() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let bytes = b"zanzibar content arriving late";
    let uploads = storage.uploads();
    let slot = uploads
        .create(
            &target,
            uuid!(8)?,
            NewUpload {
                filename: "late.bin".to_owned(),
                relative_path: String::new(),
                expected_size: u64::try_from(bytes.len())?,
                expected_sha256: None,
                supplied_by: person("ana"),
            },
        )
        .await?;
    let records = rusqlite::Connection::open(directory.path().join("records.sqlite"))?;
    records.execute(
        &format!(
            "CREATE TRIGGER late_purge AFTER UPDATE OF received_bytes ON uploads
             BEGIN INSERT INTO purges (purge_id, tenant_id, target_key, state, mutation_id,
                 record) VALUES ('{}', 'local', 'workspace/{}', 'running', '{}', '{{}}'); END;",
            uuid_text(60),
            target.workspace_id.0,
            uuid_text(50)
        ),
        [],
    )?;
    let refusal = uploads
        .put_content(
            &target,
            slot.id,
            Box::pin(std::io::Cursor::new(bytes.to_vec())),
            1 << 20,
        )
        .await;
    records.execute_batch("DROP TRIGGER late_purge;")?;
    drop(records);
    refused(refusal, "upload content")?;
    assert_eq!(
        marked_files(&directory.path().join("uploads"))?,
        Vec::<String>::new(),
        "a refused transfer keeps no part file"
    );
    Ok(())
}

/// An indexing that read the target's repository before the repository step and stored its
/// entries after the index step (core indexes right after a commit, in no job) is removed by
/// the index pass the repository step runs again. The purge is stopped at the index step, then
/// in the repository step after the repository went; the late entries are written then.
#[tokio::test]
async fn purge_removes_index_entries_stored_after_its_index_step() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, created) = create(&storage, 1, "Trip").await?;
    let revision = commit(
        &storage,
        &target,
        3,
        &created,
        vec![note(uuid!(10)?, "notes/plans.md", "Ferry to zanzibar.")?],
    )
    .await?;
    storage
        .search()
        .index_revision(&target, revision.clone())
        .await?;
    let index = rusqlite::Connection::open(directory.path().join("index.sqlite"))?;
    index.execute_batch(
        "CREATE TRIGGER stop_purge BEFORE DELETE ON index_items
         BEGIN SELECT RAISE(ABORT, 'stopped at the index step'); END;",
    )?;
    let purge: PurgeId = uuid!(60)?;
    record_purge_of(&storage, &target, purge).await?;
    let purger = storage.purger();
    let workspace = target.workspace_id;
    err_of(
        purger
            .purge_workspace(&tenant()?, uuid!(61)?, purge, workspace)
            .await,
    )?;
    index.execute_batch("DROP TRIGGER stop_purge;")?;
    let records = rusqlite::Connection::open(directory.path().join("records.sqlite"))?;
    records.execute(
        "INSERT INTO receipts (receipt_id, tenant_id, workspace_id, mutation_id, record)
         VALUES (?1, 'local', ?2, NULL, '{\"late\": \"receipt\"}')",
        [uuid_text(44), workspace.0.to_string()],
    )?;
    records.execute_batch(
        "CREATE TRIGGER stop_rows BEFORE DELETE ON receipts
         BEGIN SELECT RAISE(ABORT, 'stopped in the repository step'); END;",
    )?;
    err_of(
        purger
            .purge_workspace(&tenant()?, uuid!(61)?, purge, workspace)
            .await,
    )?;
    let repository = storage
        .data()
        .repositories_path()
        .join("local")
        .join(format!("{}.git", workspace.0));
    assert!(!repository.exists(), "stopped after the repository went");
    index.execute(
        "INSERT INTO index_items (tenant_id, workspace_id, revision, item_id, path, archived,
             extraction_status, summary)
         VALUES ('local', ?1, ?2, ?3, 'notes/plans.md', 0, NULL, '{\"title\": \"zanzibar\"}')",
        [
            workspace.0.to_string(),
            revision.as_str().to_owned(),
            uuid_text(10),
        ],
    )?;
    index.execute(
        "INSERT INTO index_text (rowid, title, description, body)
         VALUES (last_insert_rowid(), 'zanzibar', '', 'Ferry to zanzibar.')",
        [],
    )?;
    drop(index);
    records.execute_batch("DROP TRIGGER stop_rows;")?;
    drop(records);
    purger
        .purge_workspace(&tenant()?, uuid!(61)?, purge, workspace)
        .await?;
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    Ok(())
}

/// Re-review B1: an upload completion of the purged workspace after the purge completed is
/// refused with `Conflict` naming the purge, not answered `NotFound`.
#[tokio::test]
async fn purge_refuses_an_upload_completion_after_it_completes_naming_the_purge() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let bytes = b"late completion";
    let slot = upload(&storage, &target, 8, bytes, false).await?;
    let purge: PurgeId = uuid!(60)?;
    record_purge_of(&storage, &target, purge).await?;
    storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, purge, target.workspace_id)
        .await?;
    refused(
        storage
            .uploads()
            .complete(&target, slot, digest_of(bytes)?)
            .await,
        "an upload completion after the purge",
    )?;
    Ok(())
}

/// R-I13: a batch consumption of the purged workspace's uploads after the purge completed is
/// refused with `Conflict` naming the purge, not answered `NotFound` for the rows it removed.
#[tokio::test]
async fn purge_refuses_an_upload_batch_after_it_completes_naming_the_purge() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let slot = upload(&storage, &target, 8, b"an import's source", true).await?;
    let purge: PurgeId = uuid!(60)?;
    record_purge_of(&storage, &target, purge).await?;
    storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, purge, target.workspace_id)
        .await?;
    refused(
        storage
            .uploads()
            .consume_batch(&target, &[slot], uuid!(70)?)
            .await,
        "an upload batch after the purge",
    )?;
    Ok(())
}

/// Record purge `purge` of `target` under mutation `mutation`.
async fn record_purge_under(
    storage: &Storage,
    target: &StorageScope,
    purge: u32,
    mutation: u32,
) -> Fallible<()> {
    storage
        .records()
        .create_purge(
            &tenant()?,
            uuid!(mutation)?,
            NewPurge {
                id: uuid!(purge)?,
                target: PurgeTarget::Workspace {
                    workspace_id: target.workspace_id,
                },
                initiator: person("ana"),
            },
        )
        .await?;
    Ok(())
}

/// The pending puts the records database holds for `workspace`.
fn pending_puts(data: &Path, workspace: &StorageScope) -> Fallible<i64> {
    let records = rusqlite::Connection::open(data.join("records.sqlite"))?;
    Ok(records.query_row(
        "SELECT count(*) FROM pending_puts WHERE workspace_id = ?1",
        [workspace.workspace_id.0.to_string()],
        |row| row.get(0),
    )?)
}

/// Re-review B2 (the 25 h probe): a job of the target stored a derivative and failed, so no
/// record names it. The purge deletes the bytes its pending put names. The row's time plays no
/// part (rows have no lifetime): it is set a day back only to show that.
#[tokio::test]
async fn purge_deletes_the_bytes_of_a_put_whose_record_never_landed() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let derivative = put(
        &storage,
        &target,
        b"zanzibar page text of a failed conversion",
    )
    .await?;
    let records = rusqlite::Connection::open(directory.path().join("records.sqlite"))?;
    records.execute(
        "UPDATE pending_puts SET created_at = '2026-10-08T00:00:00.000Z'",
        [],
    )?;
    drop(records);
    let purge: PurgeId = uuid!(60)?;
    record_purge_of(&storage, &target, purge).await?;
    let report = storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, purge, target.workspace_id)
        .await?;
    assert!(
        !held(&storage, &target, &derivative.digest).await?,
        "a completed purge leaves none of the target's derivative bytes: {report:?}"
    );
    assert_eq!(
        pending_puts(directory.path(), &target)?,
        0,
        "its rows are cleared"
    );
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    Ok(())
}

/// Ruling B2: a write that retained its object and crashed before its record leaves its
/// pending put; after the restart, the purge of the workspace deletes the bytes.
#[tokio::test]
async fn purge_deletes_bytes_a_write_retained_just_before_a_crash() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (target, stored) = {
        let storage = Storage::open(directory.path())?;
        let (target, _) = create(&storage, 1, "Trip").await?;
        let stored = put(&storage, &target, b"zanzibar output of a crashed job").await?;
        // The process stops here: the job's completion never lands.
        (target, stored)
    };
    let storage = Storage::open(directory.path())?;
    assert!(held(&storage, &target, &stored.digest).await?);
    let purge: PurgeId = uuid!(60)?;
    record_purge_of(&storage, &target, purge).await?;
    storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, purge, target.workspace_id)
        .await?;
    assert!(
        !held(&storage, &target, &stored.digest).await?,
        "the crashed write's bytes go with the workspace"
    );
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    Ok(())
}

/// Ruling B2, the rule for leftover rows: a pending put that never gets a record is garbage of
/// its workspace. Until the next purge of that workspace it keeps an identical object of another
/// purge (counted as kept); that purge deletes the bytes and clears the rows.
#[tokio::test]
async fn a_leftover_pending_put_is_collected_by_the_next_purge_of_its_workspace() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, head) = create(&storage, 1, "Trip").await?;
    let (survivor, _) = create(&storage, 2, "Garden").await?;
    let bytes = b"commonplace bytes of a failed job";
    let original = put(&storage, &target, bytes).await?;
    commit(
        &storage,
        &target,
        3,
        &head,
        vec![card(uuid!(11)?, "sources/scan.md", &original.digest, None)?],
    )
    .await?;
    // The survivor's job stores the same bytes and fails: no record ever names them.
    put(&storage, &survivor, bytes).await?;
    record_purge_under(&storage, &target, 60, 50).await?;
    let report = storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, uuid!(60)?, target.workspace_id)
        .await?;
    assert!(
        held(&storage, &survivor, &original.digest).await?,
        "another workspace's pending put keeps the object"
    );
    assert_eq!(report.objects_kept_shared, "1");
    assert_eq!(pending_puts(directory.path(), &survivor)?, 1);
    record_purge_under(&storage, &survivor, 62, 52).await?;
    storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(63)?, uuid!(62)?, survivor.workspace_id)
        .await?;
    assert!(
        !held(&storage, &survivor, &original.digest).await?,
        "the next purge of its workspace collects the leftover"
    );
    assert_eq!(pending_puts(directory.path(), &survivor)?, 0);
    Ok(())
}

/// Ruling B2 (the re-review's `blobs.rs` note): a put whose retain fails keeps its pending put.
#[tokio::test]
async fn a_failed_retain_keeps_its_pending_put() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    err_of(
        storage
            .blobs()
            .put(
                &target,
                Box::pin(std::io::Cursor::new(b"zanzibar".to_vec())),
                1 << 20,
                Some(Digest::try_from("e".repeat(64))?),
            )
            .await,
    )?;
    assert_eq!(pending_puts(directory.path(), &target)?, 1);
    Ok(())
}

/// Ruling 3: a job of a workspace a purge names is refused inside its transaction, during the
/// purge and after it, so core drains only the jobs that already exist.
#[tokio::test]
async fn purge_refuses_a_new_job_of_the_workspace_it_names() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let (survivor, _) = create(&storage, 2, "Garden").await?;
    let purge: PurgeId = uuid!(60)?;
    record_purge_of(&storage, &target, purge).await?;
    let new_job = |n: u32| -> Fallible<NewJob> {
        Ok(NewJob {
            mutation_id: uuid!(n)?,
            initiator: person("ana"),
            spec: JobSpec::BackupWorkspace,
        })
    };
    let records = storage.records();
    let scope = JobScope::Workspace(target.clone());
    refused(
        records.create_job(&scope, new_job(30)?).await,
        "a job during the purge",
    )?;
    storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, purge, target.workspace_id)
        .await?;
    refused(
        records.create_job(&scope, new_job(31)?).await,
        "a job after the purge",
    )?;
    records
        .create_job(&JobScope::Workspace(survivor), new_job(32)?)
        .await?;
    Ok(())
}

/// A queued job of `scope` created under mutation `n`.
async fn queued_job(storage: &Storage, scope: &JobScope, n: u32) -> Fallible<Job> {
    Ok(storage
        .records()
        .create_job(
            scope,
            NewJob {
                mutation_id: uuid!(n)?,
                initiator: person("ana"),
                spec: JobSpec::BackupWorkspace,
            },
        )
        .await?)
}

/// Ruling 3: a claim of a job of a workspace a purge names is refused with the purge's
/// `Conflict` inside its transaction and leaves the job queued, still listed as pending, for
/// core to cancel.
#[tokio::test]
async fn purge_refuses_a_claim_of_a_job_of_the_workspace_it_names() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let (survivor, _) = create(&storage, 2, "Garden").await?;
    let scope = JobScope::Workspace(target.clone());
    let job = queued_job(&storage, &scope, 30).await?;
    let other = queued_job(&storage, &JobScope::Workspace(survivor), 31).await?;
    record_purge_under(&storage, &target, 60, 50).await?;
    let records = storage.records();
    refused(
        records.claim_job(job.id).await,
        "a claim after the purge is recorded",
    )?;
    assert_eq!(
        records.get_job(&scope, job.id).await?.state,
        JobState::Queued,
        "the refused claim leaves the job as it is"
    );
    assert!(
        records
            .pending_jobs()
            .await?
            .iter()
            .any(|(_, pending)| *pending == job.id),
        "it is still listed as pending"
    );
    let cancelled = records.cancel_job(&scope, uuid!(32)?, job.id).await?;
    assert_eq!(cancelled.state, JobState::Cancelled, "core can cancel it");
    some(
        records.claim_job(other.id).await?,
        "another workspace's job is claimed",
    )?;
    Ok(())
}

/// Round 4 re-review N2 (probe `rr4_an_expired_running_job_is_not_reclaimed_after_the_purge`):
/// a running job of a workspace a purge names whose lease expired is not reclaimed. The claim
/// is refused with the purge's `Conflict`, and the job stays running in its first attempt.
#[tokio::test]
async fn purge_refuses_a_reclaim_of_a_running_job_whose_lease_expired() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let scope = JobScope::Workspace(target.clone());
    let job = queued_job(&storage, &scope, 30).await?;
    let records = storage.records();
    some(records.claim_job(job.id).await?, "the first claim")?;
    rusqlite::Connection::open(directory.path().join("records.sqlite"))?.execute(
        "UPDATE jobs SET lease_expires_ms = 1 WHERE job_id = ?1",
        [job.id.0.to_string()],
    )?;
    record_purge_under(&storage, &target, 60, 50).await?;
    refused(records.claim_job(job.id).await, "a reclaim after the purge")?;
    let now = records.get_job(&scope, job.id).await?;
    assert_eq!(now.state, JobState::Running);
    assert_eq!(now.attempt, 1, "no second attempt");
    Ok(())
}

/// Round 4 re-review R3c: a claim of a running job under a live lease answers `None` before
/// the purge is recorded and the purge's `Conflict` after it (core's `claim_job` doc still says
/// `None`; request to the integration owner).
#[tokio::test]
async fn purge_refuses_a_claim_of_a_running_job_under_a_live_lease() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let scope = JobScope::Workspace(target.clone());
    let job = queued_job(&storage, &scope, 30).await?;
    let records = storage.records();
    some(records.claim_job(job.id).await?, "the first claim")?;
    assert!(
        records.claim_job(job.id).await?.is_none(),
        "before the purge a live claim answers None"
    );
    record_purge_under(&storage, &target, 60, 50).await?;
    refused(
        records.claim_job(job.id).await,
        "a live running job after the purge",
    )?;
    Ok(())
}

/// Port doc, "Events are not refused" (round 4 re-review probe
/// `rr4_an_event_after_the_purge_is_recorded_is_not_refused`): an event of a workspace a purge
/// names is stored after the purge is recorded, since an event carries identities only.
#[tokio::test]
async fn purge_does_not_refuse_an_event_of_the_workspace_it_names() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    record_purge_under(&storage, &target, 60, 50).await?;
    storage
        .events()
        .append(
            &EventScope::Workspace(target.clone()),
            None,
            NewEvent {
                kind: EventKind::JobUpdated,
                revision: None,
                item_id: None,
                job_id: None,
                connector_id: None,
                actor: None,
                operation: None,
            },
        )
        .await?;
    Ok(())
}

/// Ruling 3 (re-review probe `rr3_retry_of_a_failed_job_after_the_purge_is_recorded`): a
/// retry of a failed, retryable job of a workspace a purge names is refused with the purge's
/// `Conflict`, during the purge and after it, and the job stays failed.
#[tokio::test]
async fn purge_refuses_a_retry_of_a_failed_job_of_the_workspace_it_names() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let scope = JobScope::Workspace(target.clone());
    let job = queued_job(&storage, &scope, 30).await?;
    let records = storage.records();
    let claimed = some(records.claim_job(job.id).await?, "the claim")?;
    records
        .fail_job(claimed.lease, "disk full".to_owned(), true)
        .await?;
    record_purge_under(&storage, &target, 60, 50).await?;
    refused(
        records.retry_job(&scope, uuid!(31)?, job.id).await,
        "a retry during the purge",
    )?;
    assert_eq!(
        records.get_job(&scope, job.id).await?.state,
        JobState::Failed
    );
    storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, uuid!(60)?, target.workspace_id)
        .await?;
    refused(
        records.retry_job(&scope, uuid!(32)?, job.id).await,
        "a retry after the purge",
    )?;
    Ok(())
}

/// Ruling 3 under a real race: three loopers each retry, claim and fail their own job of the
/// target while the purge is recorded. Every retry is queued or refused naming the purge, none
/// is queued after one was refused, a refused retry records no control row, and no job is left
/// running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn purge_refuses_every_retry_once_recorded_while_retries_race_it() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let scope = JobScope::Workspace(target.clone());
    let queued_so_far = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let mut tasks = Vec::new();
    for lane in 0..3_u32 {
        let job = queued_job(&storage, &scope, 30 + lane).await?;
        let records = storage.records();
        let claimed = some(records.claim_job(job.id).await?, "the first claim")?;
        records
            .fail_job(claimed.lease, "busy".to_owned(), true)
            .await?;
        let scope = scope.clone();
        let queued_so_far = std::sync::Arc::clone(&queued_so_far);
        tasks.push(tokio::spawn(async move {
            // Each looper retries until a retry is refused, then tries 20 more.
            let mut outcomes: Vec<Result<(), ApiError>> = Vec::new();
            let (mut refusals, mut n) = (0, 0_u32);
            while refusals < 20 && n < 100_000 {
                let mutation = serde_json::from_value(json!(uuid_text(100_000 * (lane + 1) + n)))
                    .map_err(|error| error.to_string())?;
                n += 1;
                let retried = records
                    .retry_job(&scope, mutation, job.id)
                    .await
                    .map(|_| ());
                let queued = retried.is_ok();
                if queued {
                    queued_so_far.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                } else {
                    refusals += 1;
                }
                outcomes.push(retried);
                if !queued {
                    continue;
                }
                match records.claim_job(job.id).await {
                    Ok(Some(claimed)) => {
                        records
                            .fail_job(claimed.lease, "busy".to_owned(), true)
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    Ok(None) => return Err("a queued job was not claimable".to_owned()),
                    Err(error) => outcomes.push(Err(error)),
                }
            }
            Ok::<_, String>((job.id, outcomes))
        }));
    }
    // The purge is recorded once retries have been queued, whatever the machine's speed.
    let started = std::time::Instant::now();
    while queued_so_far.load(std::sync::atomic::Ordering::SeqCst) < 3 {
        if started.elapsed() > std::time::Duration::from_secs(60) {
            return Err("the loopers never queued a retry".into());
        }
        tokio::task::yield_now().await;
    }
    record_purge_under(&storage, &target, 60, 50).await?;
    let (mut queued, mut refused_count) = (0_i64, 0_i64);
    let records = storage.records();
    for task in tasks {
        let (job, outcomes) = task.await??;
        let mut seen_refusal = false;
        for outcome in outcomes {
            match outcome {
                Ok(()) => {
                    assert!(!seen_refusal, "a retry was queued after one was refused");
                    queued += 1;
                }
                Err(error) => {
                    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
                    assert!(error.message.contains(&uuid_text(60)), "{error}");
                    seen_refusal = true;
                    refused_count += 1;
                }
            }
        }
        let state = records.get_job(&scope, job).await?.state;
        assert!(
            matches!(state, JobState::Failed | JobState::Queued),
            "no job is left running: {state:?}"
        );
    }
    let controls: i64 = rusqlite::Connection::open(directory.path().join("records.sqlite"))?
        .query_row(
            "SELECT count(*) FROM job_controls WHERE action = 'retry'",
            [],
            |row| row.get(0),
        )?;
    eprintln!("retry race: queued {queued}, refused {refused_count}, controls {controls}");
    assert_eq!(controls, queued, "a refused retry records no control row");
    assert!(
        queued > 0 && refused_count >= 60,
        "a real race: {queued} queued"
    );
    Ok(())
}

/// Ruling B2: each record that references a freshly put object lands its pending put: a
/// commit, a job completion, a derived object, an artifact and an upload completion.
#[tokio::test]
async fn every_record_that_references_a_put_object_lands_its_pending_put() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (workspace, head) = create(&storage, 1, "Trip").await?;
    let original = put(&storage, &workspace, b"zanzibar original").await?;
    let revision = commit(
        &storage,
        &workspace,
        3,
        &head,
        vec![card(uuid!(11)?, "sources/scan.md", &original.digest, None)?],
    )
    .await?;
    assert_eq!(pending_puts(directory.path(), &workspace)?, 0, "a commit");
    let records = storage.records();
    let job_scope = JobScope::Workspace(workspace.clone());
    let job = records
        .create_job(
            &job_scope,
            NewJob {
                mutation_id: uuid!(30)?,
                initiator: person("ana"),
                spec: JobSpec::BackupWorkspace,
            },
        )
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "the claimed job")?;
    let output = put(&storage, &workspace, b"zanzibar output").await?;
    records
        .complete_job(JobCompletion {
            lease: claimed.lease,
            revision: None,
            item_ids: Vec::new(),
            artifact: None,
            restore: None,
            outputs: vec![output.digest],
            warnings: Vec::new(),
        })
        .await?;
    assert_eq!(
        pending_puts(directory.path(), &workspace)?,
        0,
        "a job completion"
    );
    let dataset = put(&storage, &workspace, b"zanzibar dataset").await?;
    records
        .record_derived_object(
            &workspace,
            DerivedObject {
                item_id: uuid!(11)?,
                revision,
                digest: dataset.digest,
                kind: DerivedKind::Dataset,
                media_type: "text/csv".to_owned(),
            },
        )
        .await?;
    assert_eq!(
        pending_puts(directory.path(), &workspace)?,
        0,
        "a derived object"
    );
    artifact(
        &storage,
        &workspace,
        32,
        ArtifactKind::Export,
        b"zanzibar export",
    )
    .await?;
    assert_eq!(
        pending_puts(directory.path(), &workspace)?,
        0,
        "an artifact"
    );
    upload(&storage, &workspace, 8, b"zanzibar upload", true).await?;
    assert_eq!(
        pending_puts(directory.path(), &workspace)?,
        0,
        "an upload completion"
    );
    Ok(())
}

/// Commit `edits` to `scope` on `head` under mutation `mutation`, returning whether it was
/// answered by an earlier commit.
async fn commit_replayed(
    storage: &Storage,
    scope: &StorageScope,
    mutation: u32,
    head: &Revision,
    edits: Vec<TreeEdit>,
) -> Fallible<bool> {
    Ok(storage
        .versions()
        .commit(
            scope,
            CommitChanges {
                mutation_id: uuid!(mutation)?,
                expected_head: head.clone(),
                author: person("ana"),
                message: "Convert".to_owned(),
                edits,
            },
            Arc::new(Accept),
        )
        .await?
        .replayed)
}

/// Purge `target` (purge 60, mutations 50 and 61) and return the report.
async fn purge_now(storage: &Storage, target: &StorageScope) -> Fallible<PurgeReport> {
    record_purge_under(storage, target, 60, 50).await?;
    Ok(storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, uuid!(60)?, target.workspace_id)
        .await?)
}

/// Re-review B3 (probe `rr3_replayed_artifact_keeps_the_pending_put_of_bytes_it_does_not_name`):
/// a repeated artifact mutation whose retry put different bytes is answered by the first record,
/// which does not name them, so their pending put stays and the purge deletes them.
#[tokio::test]
async fn purge_deletes_the_bytes_of_a_retry_a_replayed_artifact_does_not_name() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let scope = JobScope::Workspace(target.clone());
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
    let new = |object: ObjectInfo| NewArtifact {
        kind: ArtifactKind::Export,
        object,
        media_type: "application/zip".to_owned(),
        created_by_job: job.id,
    };
    let first = put(&storage, &target, b"zanzibar export attempt one").await?;
    records
        .record_artifact(&scope, uuid!(31)?, new(first.clone()))
        .await?;
    let second = put(&storage, &target, b"zanzibar export attempt two").await?;
    let replayed = records
        .record_artifact(&scope, uuid!(31)?, new(second.clone()))
        .await?;
    assert_eq!(
        replayed.object.digest, first.digest,
        "the first record answers"
    );
    assert_eq!(
        pending_puts(directory.path(), &target)?,
        1,
        "the retry's pending put stays"
    );
    let report = purge_now(&storage, &target).await?;
    assert!(
        !held(&storage, &target, &second.digest).await?,
        "a completed purge leaves none of the retry's bytes: {report:?}"
    );
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    Ok(())
}

/// Re-review B3 (probe `rr3_replayed_commit_keeps_the_pending_put_of_bytes_it_does_not_name`):
/// a repeated commit mutation whose retry's card names different bytes is answered by the
/// first commit, so the retry's pending put stays and the purge deletes the bytes.
#[tokio::test]
async fn purge_deletes_the_bytes_of_a_retry_a_replayed_commit_does_not_name() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, head) = create(&storage, 1, "Trip").await?;
    let first = put(&storage, &target, b"zanzibar conversion attempt one").await?;
    let card_of = |object: &Digest| card(uuid!(11)?, "sources/a.md", object, None);
    commit_replayed(&storage, &target, 40, &head, vec![card_of(&first.digest)?]).await?;
    let second = put(&storage, &target, b"zanzibar conversion attempt two").await?;
    let replayed =
        commit_replayed(&storage, &target, 40, &head, vec![card_of(&second.digest)?]).await?;
    assert!(replayed, "the repeated mutation is a replay");
    assert_eq!(
        pending_puts(directory.path(), &target)?,
        1,
        "the retry's pending put stays"
    );
    let report = purge_now(&storage, &target).await?;
    assert!(
        !held(&storage, &target, &second.digest).await?,
        "a completed purge leaves none of the retry's bytes: {report:?}"
    );
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    Ok(())
}

/// Ruling B3: a repeated candidate mutation answered by the retained candidate keeps the
/// pending put of the retry's different bytes, and the purge deletes them.
#[tokio::test]
async fn purge_deletes_the_bytes_of_a_retry_a_replayed_candidate_does_not_name() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, head) = create(&storage, 1, "Trip").await?;
    let candidate = |object: &Digest| -> Fallible<CandidateChanges> {
        Ok(CandidateChanges {
            mutation_id: uuid!(41)?,
            base: head.clone(),
            author: person("ana"),
            message: "Propose".to_owned(),
            edits: vec![card(uuid!(12)?, "sources/b.md", object, None)?],
        })
    };
    let first = put(&storage, &target, b"zanzibar proposed attempt one").await?;
    let retained = storage
        .versions()
        .create_candidate(
            &target,
            uuid!(20)?,
            candidate(&first.digest)?,
            Arc::new(Accept),
        )
        .await?;
    let second = put(&storage, &target, b"zanzibar proposed attempt two").await?;
    let answered = storage
        .versions()
        .create_candidate(
            &target,
            uuid!(20)?,
            candidate(&second.digest)?,
            Arc::new(Accept),
        )
        .await?;
    assert_eq!(answered, retained, "the retained candidate answers");
    assert_eq!(
        pending_puts(directory.path(), &target)?,
        1,
        "the retry's pending put stays"
    );
    let report = purge_now(&storage, &target).await?;
    assert!(
        !held(&storage, &target, &second.digest).await?,
        "a completed purge leaves none of the retry's bytes: {report:?}"
    );
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    Ok(())
}

/// Ruling B3: a replay lands the pending puts of the objects the stored commit does name, so
/// a retry that put the same bytes again leaves no row behind.
#[tokio::test]
async fn a_replayed_commit_lands_the_pending_put_of_bytes_it_names() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, head) = create(&storage, 1, "Trip").await?;
    let bytes = b"zanzibar deterministic conversion";
    let object = put(&storage, &target, bytes).await?;
    let edits = || -> Fallible<Vec<TreeEdit>> {
        Ok(vec![card(
            uuid!(11)?,
            "sources/a.md",
            &object.digest,
            None,
        )?])
    };
    commit_replayed(&storage, &target, 40, &head, edits()?).await?;
    put(&storage, &target, bytes).await?;
    assert_eq!(pending_puts(directory.path(), &target)?, 1);
    assert!(commit_replayed(&storage, &target, 40, &head, edits()?).await?);
    assert_eq!(
        pending_puts(directory.path(), &target)?,
        0,
        "the stored commit names the retry's object"
    );
    Ok(())
}

/// Ruling B3: a job completion is never replayed. A second completion under the same claim is
/// refused, so the pending put of an output only it names stays.
#[tokio::test]
async fn a_repeated_job_completion_lands_no_pending_put() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let records = storage.records();
    let job = records
        .create_job(
            &JobScope::Workspace(target.clone()),
            NewJob {
                mutation_id: uuid!(30)?,
                initiator: person("ana"),
                spec: JobSpec::BackupWorkspace,
            },
        )
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "the claimed job")?;
    let completion = |outputs: Vec<Digest>| JobCompletion {
        lease: claimed.lease.clone(),
        revision: None,
        item_ids: Vec::new(),
        artifact: None,
        restore: None,
        outputs,
        warnings: Vec::new(),
    };
    let first = put(&storage, &target, b"zanzibar output one").await?;
    records.complete_job(completion(vec![first.digest])).await?;
    let second = put(&storage, &target, b"zanzibar output two").await?;
    err_of(records.complete_job(completion(vec![second.digest])).await)?;
    assert_eq!(pending_puts(directory.path(), &target)?, 1);
    Ok(())
}

/// Ruling 4 (mutant X02): a candidate lands the pending puts of the objects its cards name.
#[tokio::test]
async fn a_candidate_lands_the_pending_puts_of_the_objects_its_cards_name() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (workspace, head) = create(&storage, 1, "Trip").await?;
    let object = put(&storage, &workspace, b"zanzibar proposed original").await?;
    assert_eq!(pending_puts(directory.path(), &workspace)?, 1);
    storage
        .versions()
        .create_candidate(
            &workspace,
            uuid!(20)?,
            CandidateChanges {
                mutation_id: uuid!(41)?,
                base: head,
                author: person("ana"),
                message: "Propose".to_owned(),
                edits: vec![card(uuid!(12)?, "sources/b.md", &object.digest, None)?],
            },
            Arc::new(Accept),
        )
        .await?;
    assert_eq!(pending_puts(directory.path(), &workspace)?, 0);
    Ok(())
}

/// Ruling 4 (mutant X09): a sandbox capability lands the pending put of the object it binds.
#[tokio::test]
async fn a_sandbox_capability_lands_the_pending_put_of_its_object() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (workspace, head) = create(&storage, 1, "Trip").await?;
    let rendered = put(&storage, &workspace, b"zanzibar sandbox render").await?;
    assert_eq!(pending_puts(directory.path(), &workspace)?, 1);
    storage
        .sandbox()
        .mint(
            &workspace,
            [7; 32],
            okf_jawn_core::sandbox::SandboxMint {
                item_id: uuid!(11)?,
                revision: head,
                object: rendered.digest,
                media_type: "text/html".to_owned(),
                expires_at: serde_json::from_value(json!("2099-01-01T00:00:00.000Z"))?,
            },
        )
        .await?;
    assert_eq!(pending_puts(directory.path(), &workspace)?, 0);
    Ok(())
}

/// Re-review B4 (probe `rr3_a_survivor_pending_put_of_the_targets_archive_keeps_it`): another
/// workspace put the bytes of the target's workspace archive and its record never landed. Its
/// pending put is no surviving reference of the archive, as its upload slot would not be, so
/// the purge deletes the archive.
#[tokio::test]
async fn purge_deletes_its_archive_another_workspaces_pending_put_names() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let (survivor, _) = create(&storage, 2, "Garden").await?;
    let bytes = b"zanzibar workspace archive";
    let archive = artifact(&storage, &target, 30, ArtifactKind::WorkspaceBackup, bytes).await?;
    put(&storage, &survivor, bytes).await?;
    assert_eq!(pending_puts(directory.path(), &survivor)?, 1);
    let report = purge_now(&storage, &target).await?;
    assert!(
        !held(&storage, &survivor, &archive).await?,
        "the target's archive goes with it: {report:?}"
    );
    assert_eq!(report.objects_kept_shared, "0", "{report:?}");
    assert_eq!(marked_files(directory.path())?, Vec::<String>::new());
    Ok(())
}

/// Every file in the object store of the data directory `data`.
fn stored_objects(data: &Path) -> Fallible<Vec<String>> {
    let mut found = Vec::new();
    let mut pending = vec![data.join("objects")];
    while let Some(directory) = pending.pop() {
        if !directory.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(path.display().to_string());
            }
        }
    }
    Ok(found)
}

/// Fail every insert into `table` of the records database of `data` until the returned
/// connection runs `DROP TRIGGER stop_record`: a record that never lands, as after a stop
/// between a write's bytes and its record.
fn stop_records_of(data: &Path, table: &str) -> Fallible<rusqlite::Connection> {
    let records = rusqlite::Connection::open(data.join("records.sqlite"))?;
    records.execute_batch(&format!(
        "CREATE TRIGGER stop_record BEFORE INSERT ON {table}
         BEGIN SELECT RAISE(ABORT, 'stopped before the record'); END;"
    ))?;
    Ok(records)
}

/// Purge `target` to completion under purge 60.
async fn purge_scope(storage: &Storage, target: &StorageScope) -> Fallible<PurgeReport> {
    let purge: PurgeId = uuid!(60)?;
    record_purge_of(storage, target, purge).await?;
    Ok(storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, purge, target.workspace_id)
        .await?)
}

/// PR #20 Codex P1 (`backup/mod.rs:218`): a workspace archive whose record never landed (the
/// process stopped between storing its bytes and recording them; here a trigger fails the
/// record) left a pending put, so the purge of the workspace deletes its bytes.
#[tokio::test]
async fn purge_deletes_a_workspace_archive_whose_record_never_landed() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let records = stop_records_of(directory.path(), "backup_archives")?;
    err_of(
        storage
            .backups()
            .write_workspace_archive(&target, uuid!(70)?)
            .await,
    )?;
    records.execute_batch("DROP TRIGGER stop_record;")?;
    drop(records);
    assert_eq!(
        stored_objects(directory.path())?.len(),
        1,
        "the archive's bytes were stored, and no record names them"
    );
    purge_scope(&storage, &target).await?;
    assert_eq!(
        stored_objects(directory.path())?,
        Vec::<String>::new(),
        "the purge deletes the unrecorded archive"
    );
    Ok(())
}

/// PR #20 Codex P1, the written-history form: a written history whose record never landed left
/// a pending put, so the purge of the workspace deletes its bytes.
#[tokio::test]
async fn purge_deletes_a_written_history_whose_record_never_landed() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, head) = create(&storage, 1, "Trip").await?;
    let records = stop_records_of(directory.path(), "history_exports")?;
    err_of(
        storage
            .versions()
            .write_history(&target, &head, uuid!(70)?)
            .await,
    )?;
    records.execute_batch("DROP TRIGGER stop_record;")?;
    drop(records);
    assert_eq!(
        stored_objects(directory.path())?.len(),
        1,
        "the history's bytes were stored, and no record names them"
    );
    purge_scope(&storage, &target).await?;
    assert_eq!(
        stored_objects(directory.path())?,
        Vec::<String>::new(),
        "the purge deletes the unrecorded history"
    );
    Ok(())
}

/// PR #20 Codex P1: a workspace archive's record and a written history's record each land the
/// pending put of the bytes they stored.
#[tokio::test]
async fn a_workspace_archive_and_a_written_history_land_their_pending_puts() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (target, head) = create(&storage, 1, "Trip").await?;
    storage
        .backups()
        .write_workspace_archive(&target, uuid!(70)?)
        .await?;
    assert_eq!(pending_puts(directory.path(), &target)?, 0, "an archive");
    storage
        .versions()
        .write_history(&target, &head, uuid!(71)?)
        .await?;
    assert_eq!(pending_puts(directory.path(), &target)?, 0, "a history");
    assert_eq!(stored_objects(directory.path())?.len(), 2);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
