//! Workspace purge against real Git, SQLite and the object store: the receipt of
//! `storage-purge-workspace` (`cargo test -p okf-jawn-storage --features runtime -- purge`).
//!
//! The target workspace's content carries the marker `zanzibar` (in a note's path and body, a
//! draft, original bytes, a conversion record's asset, retained archives and exports, an
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
use okf_jawn_contract::events::Receipt;
use okf_jawn_contract::identity::{
    Digest, ItemId, MutationId, PurgeId, Revision, TenantId, UploadId, WorkspacePath,
};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::purge::{PurgeReport, PurgeTarget};
use okf_jawn_contract::workspace::ArtifactKind;
use okf_jawn_core::credentials::{ConnectorIssue, CredentialStore, NewConnector};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::jobs::{
    ClaimedJob, DerivedKind, DerivedObject, JobCompletion, JobScope, JobSpec, NewArtifact, NewJob,
    NewPurge, RecordStore,
};
use okf_jawn_core::mutations::{BeginOutcome, MutationKey, MutationStore};
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

/// Accepts every candidate.
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

/// A completed mutation whose stored response names the workspace and shows its name.
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
        .begin(&key, &Digest::try_from("a".repeat(64))?)
        .await?
    else {
        return Err("expected a new mutation".into());
    };
    let response = json!({ "id": workspace.workspace_id, "name": "zanzibar trip" });
    ledger.complete(lease, response).await?;
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
    assert_eq!(written.len(), 3, "two workspace archives, one installation");
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

/// The upload writes of the refusal test: each refused.
async fn refuse_uploads(storage: &Storage, target: &StorageScope, slot: UploadId) -> TestResult {
    let uploads = storage.uploads();
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
    refuse_uploads(&storage, &target, slot).await?;
    refuse_records(&storage, &target, &head, claimed, &object).await?;
    put(&storage, &survivor, b"the other workspace goes on").await?;
    Ok(())
}

/// The SHA-256 digest of `bytes`.
fn digest_of(bytes: &[u8]) -> Fallible<Digest> {
    Ok(Digest::try_from(sha256(bytes))?)
}

/// Re-review probe Q3 (window 1, an upload completion in flight): the completion passed its
/// first refusal before the purge was recorded and is still reading the part file when the
/// purge runs. An upload completion is no job, so no drain covers it. Whichever ends first,
/// the completion is refused and no byte of the target remains.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn purge_leaves_nothing_of_an_upload_completion_in_flight() -> TestResult {
    let directory = tempfile::tempdir()?;
    let data = directory.path().to_path_buf();
    let storage = Storage::open(&data)?;
    let (target, _) = create(&storage, 1, "Trip").await?;
    let (_survivor, _) = create(&storage, 2, "Garden").await?;
    let bytes: Vec<u8> = MARKER.repeat(8 << 20);
    let uploads = storage.uploads();
    let slot = uploads
        .create(
            &target,
            uuid!(8)?,
            NewUpload {
                filename: "big.bin".to_owned(),
                relative_path: String::new(),
                expected_size: u64::try_from(bytes.len())?,
                expected_sha256: None,
                supplied_by: person("ana"),
            },
        )
        .await?;
    uploads
        .put_content(
            &target,
            slot.id,
            Box::pin(std::io::Cursor::new(bytes.clone())),
            1 << 28,
        )
        .await?;
    let digest = digest_of(&bytes)?;
    drop(bytes);
    let incoming = data.join("staging").join("incoming");
    let finishing = {
        let uploads = storage.uploads();
        let target = target.clone();
        let id = slot.id;
        tokio::spawn(async move { uploads.complete(&target, id, digest).await })
    };
    // The completion is past its first refusal once its staged copy exists.
    let started = std::time::Instant::now();
    while !incoming.exists() || std::fs::read_dir(&incoming)?.next().is_none() {
        if started.elapsed() > std::time::Duration::from_secs(30) || finishing.is_finished() {
            return Err("the completion never started staging".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    let purge: PurgeId = uuid!(60)?;
    record_purge_of(&storage, &target, purge).await?;
    let report = storage
        .purger()
        .purge_workspace(&tenant()?, uuid!(61)?, purge, target.workspace_id)
        .await?;
    let finished_before = finishing.is_finished();
    let completion = finishing.await?;
    eprintln!("Q3: completion finished before the purge: {finished_before}; report {report:?}");
    refused(completion, "the completion in flight")?;
    assert_eq!(
        marked_files(&data)?,
        Vec::<String>::new(),
        "a completed purge leaves none of the target's upload bytes"
    );
    Ok(())
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

#[path = "../../../tests/support/check.rs"]
mod check;
