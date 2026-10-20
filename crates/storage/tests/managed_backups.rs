//! Lifecycle section 7: real SQLite/Git/object-store classification and the archive writer's
//! shared exclusion. Literal expected kinds and exclusions come from V7.2/V7.5, never a
//! classifier under test. No converter, bundle importer or live account is simulated here.
#![cfg(feature = "runtime")]

use std::io::Cursor;

use check::{TestResult, some};
use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::identity::{Digest, MutationId, Revision, TenantId};
use okf_jawn_contract::workspace::ArtifactKind;
use okf_jawn_core::jobs::{JobScope, JobSpec, NewArtifact, NewJob, RecordStore};
use okf_jawn_core::storage::{
    Backups, BlobStore, ManagedKind, NewWorkspace, ObjectInfo, Provenance, StorageScope,
    VersionStore, WorkspaceCatalog,
};
use okf_jawn_core::uploads::{NewUpload, UploadStore};
use okf_jawn_storage::Storage;
use uuid::Uuid;

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

fn mutation(number: u128) -> MutationId {
    MutationId(Uuid::from_u128(number))
}

fn author() -> Provenance {
    Provenance {
        subject: "ana".to_owned(),
        route: AccessRoute::LocalOwner,
        client_id: None,
    }
}

async fn workspace(
    storage: &Storage,
    number: u128,
    tenant: &str,
) -> Fallible<(StorageScope, Revision)> {
    let tenant_id = TenantId::try_from(tenant.to_owned())?;
    let created = storage
        .catalog()
        .create(
            &tenant_id,
            mutation(number),
            NewWorkspace {
                name: format!("Managed fixture {number}"),
                description: String::new(),
                creator: author(),
            },
        )
        .await?;
    Ok((
        StorageScope {
            tenant_id,
            workspace_id: created.id,
        },
        created.head,
    ))
}

async fn put(storage: &Storage, scope: &StorageScope, bytes: &[u8]) -> Fallible<ObjectInfo> {
    Ok(storage
        .blobs()
        .put(scope, Box::pin(Cursor::new(bytes.to_vec())), 1 << 20, None)
        .await?)
}

async fn artifact(
    storage: &Storage,
    scope: &StorageScope,
    number: u128,
    kind: ArtifactKind,
    object: ObjectInfo,
) -> TestResult {
    let records = storage.records();
    let installation = kind == ArtifactKind::InstallationBackup;
    let scope = if installation {
        JobScope::Tenant(scope.tenant_id.clone())
    } else {
        JobScope::Workspace(scope.clone())
    };
    let job = records
        .create_job(
            &scope,
            NewJob {
                mutation_id: mutation(number),
                initiator: author(),
                spec: if installation {
                    JobSpec::BackupInstallation
                } else {
                    JobSpec::BackupWorkspace
                },
            },
        )
        .await?;
    records
        .record_artifact(
            &scope,
            mutation(number.checked_add(1).ok_or("fixture identity overflow")?),
            NewArtifact {
                kind,
                object,
                media_type: "application/zip".to_owned(),
                created_by_job: job.id,
            },
        )
        .await?;
    Ok(())
}

/// Produce each managed kind through its real writer; no artifact registration is needed.
async fn written(
    storage: &Storage,
    scope: &StorageScope,
    head: &Revision,
) -> Fallible<Vec<(Digest, ManagedKind)>> {
    let backups = storage.backups();
    let workspace = backups
        .write_workspace_archive(scope, mutation(20))
        .await?
        .object
        .digest;
    let installation = backups
        .write_installation_archive(&scope.tenant_id, mutation(21))
        .await?
        .digest;
    let history = storage
        .versions()
        .write_history(scope, head, mutation(22))
        .await?
        .digest;
    Ok(vec![
        (workspace, ManagedKind::WorkspaceArchive),
        (installation, ManagedKind::InstallationArchive),
        (history, ManagedKind::WrittenHistory),
    ])
}

/// V7.2/V7.5: written histories and archives, including a workspace archive never recorded as
/// an artifact, are classified across the tenant's workspaces, without needing an artifact row.
#[tokio::test]
async fn managed_backups_classify_every_real_writer_without_artifact_registration() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (scope, head) = workspace(&storage, 1, "local").await?;
    let objects = written(&storage, &scope, &head).await?;
    for (digest, expected) in objects {
        assert_eq!(
            storage
                .backups()
                .managed_kind(&scope.tenant_id, &digest)
                .await?,
            Some(expected)
        );
    }
    Ok(())
}

/// V7.1/V7.5: a retained artifact's explicit backup kind is sufficient; export kinds stay
/// ordinary. These literal bytes test record classification only, not archive parsing.
#[tokio::test]
async fn managed_backups_classify_backup_artifact_records_but_not_portable_exports() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (scope, _) = workspace(&storage, 1, "local").await?;
    for (number, kind, bytes, expected) in [
        (
            30,
            ArtifactKind::WorkspaceBackup,
            b"workspace artifact".as_slice(),
            Some(ManagedKind::WorkspaceArchive),
        ),
        (
            40,
            ArtifactKind::InstallationBackup,
            b"installation artifact".as_slice(),
            Some(ManagedKind::InstallationArchive),
        ),
        (
            50,
            ArtifactKind::Export,
            b"portable export".as_slice(),
            None,
        ),
        (
            60,
            ArtifactKind::ViewExport,
            b"view export".as_slice(),
            None,
        ),
    ] {
        let object = put(&storage, &scope, bytes).await?;
        artifact(&storage, &scope, number, kind, object.clone()).await?;
        assert_eq!(
            storage
                .backups()
                .managed_kind(&scope.tenant_id, &object.digest)
                .await?,
            expected
        );
    }
    Ok(())
}

/// V7.5 and the tenant-only port contract: an unheld digest and an object managed by another
/// tenant have no managed kind here; a held ordinary object is likewise not a backup.
#[tokio::test]
async fn managed_backups_are_tenant_scoped_and_do_not_require_an_object_to_be_held() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (scope, head) = workspace(&storage, 1, "local").await?;
    let (other, _) = workspace(&storage, 2, "another-tenant").await?;
    let managed = storage
        .backups()
        .write_workspace_archive(&scope, mutation(20))
        .await?
        .object;
    let ordinary = put(&storage, &scope, b"ordinary retained original").await?;
    let unheld = Digest::try_from("f".repeat(64))?;
    assert_eq!(
        storage
            .backups()
            .managed_kind(&other.tenant_id, &managed.digest)
            .await?,
        None
    );
    assert_eq!(
        storage
            .backups()
            .managed_kind(&scope.tenant_id, &ordinary.digest)
            .await?,
        None
    );
    assert_eq!(
        storage
            .backups()
            .managed_kind(&scope.tenant_id, &unheld)
            .await?,
        None
    );
    assert_eq!(storage.versions().head(&scope).await?, head);
    Ok(())
}

/// Upload `object` as completed bytes in a different workspace of the same tenant.
async fn upload_object(
    storage: &Storage,
    scope: &StorageScope,
    number: u128,
    object: &ObjectInfo,
) -> TestResult {
    let local = storage.blobs().materialize(scope, &object.digest).await?;
    let bytes = std::fs::read(&local.path)?;
    let uploads = storage.uploads();
    let slot = uploads
        .create(
            scope,
            mutation(number),
            NewUpload {
                filename: format!("source-{number}.zip"),
                relative_path: String::new(),
                expected_size: object.size,
                expected_sha256: Some(object.digest.clone()),
                supplied_by: author(),
            },
        )
        .await?;
    uploads
        .put_content(scope, slot.id, Box::pin(Cursor::new(bytes)), 1 << 20)
        .await?;
    uploads
        .complete(scope, slot.id, object.digest.clone())
        .await?;
    Ok(())
}

/// V7.5: the same fixture states all three managed kinds. A backup of another workspace
/// excludes exactly their upload rows/objects, but keeps its uploaded portable export.
#[tokio::test]
async fn managed_backups_agree_with_the_workspace_archive_exclusion() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let (source, head) = workspace(&storage, 1, "local").await?;
    let (target, _) = workspace(&storage, 2, "local").await?;
    let objects = written(&storage, &source, &head).await?;
    for (number, (digest, _)) in (100_u128..).zip(&objects) {
        let local = storage.blobs().materialize(&source, digest).await?;
        upload_object(&storage, &target, number, &local.object).await?;
    }
    let export = put(&storage, &target, b"portable export stays in this archive").await?;
    artifact(&storage, &target, 200, ArtifactKind::Export, export.clone()).await?;
    upload_object(&storage, &target, 300, &export).await?;
    let archive = storage
        .backups()
        .write_workspace_archive(&target, mutation(400))
        .await?;
    assert_eq!(
        archive.rows_left_out, 3,
        "exactly the three managed uploads are omitted"
    );
    let local = storage
        .blobs()
        .materialize(&target, &archive.object.digest)
        .await?;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&local.path)?)?;
    let paths: Vec<String> = zip.file_names().map(str::to_owned).collect();
    assert!(paths.contains(&format!("objects/{}", export.digest.as_str())));
    for (digest, expected) in objects {
        assert!(!paths.contains(&format!("objects/{}", digest.as_str())));
        assert_eq!(
            storage
                .backups()
                .managed_kind(&target.tenant_id, &digest)
                .await?,
            Some(expected)
        );
    }
    let manifest: serde_json::Value = serde_json::from_reader(zip.by_name("manifest.json")?)?;
    assert_eq!(
        some(manifest.get("rows_left_out"), "the literal archive count")?,
        &serde_json::json!(3)
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
