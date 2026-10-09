//! A job and its artifacts belong to a tenant or a workspace, never to both, and a purge's
//! job names the purge it carries out.

use okf_jawn_contract::common::PageRange;
use okf_jawn_contract::identity::{
    ArtifactId, Digest, ItemId, JobId, MutationId, PurgeId, Revision, TenantId, UploadId,
    WorkspaceId,
};
use okf_jawn_contract::import::JobKind;
use okf_jawn_contract::purge::PurgeTarget;
use okf_jawn_contract::workspace::{ArtifactKind, ArtifactScope};
use okf_jawn_core::conversion::ConversionSettings;
use okf_jawn_core::jobs::{
    ArtifactRecord, JobScope, JobSpec, artifact_download_path, purge_job_spec,
};
use okf_jawn_core::storage::{ObjectInfo, StorageScope, derive_purge_id};
use uuid::Uuid;

use check::{TestResult, err_of};

fn tenant() -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::try_from("local".to_owned())?)
}

fn workspace_scope() -> Result<JobScope, Box<dyn std::error::Error>> {
    Ok(JobScope::Workspace(StorageScope {
        tenant_id: tenant()?,
        workspace_id: WorkspaceId(Uuid::from_u128(1)),
    }))
}

fn digest(fill: char) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::try_from(fill.to_string().repeat(64))?)
}

fn every_spec() -> Result<Vec<JobSpec>, Box<dyn std::error::Error>> {
    let revision = Revision::try_from("a".repeat(40))?;
    Ok(vec![
        JobSpec::Import {
            base_revision: revision.clone(),
            upload_ids: vec![UploadId(Uuid::from_u128(9))],
            destination: None,
            apply_naming_rules: false,
            settings: ConversionSettings::default(),
        },
        JobSpec::Redigest {
            item_id: ItemId(Uuid::from_u128(10)),
            base_revision: revision.clone(),
            settings: ConversionSettings::default(),
            pages: None,
        },
        JobSpec::ExportWorkspace {
            revision: revision.clone(),
            include_history: true,
        },
        JobSpec::BackupWorkspace,
        JobSpec::RestoreWorkspace {
            upload_id: UploadId(Uuid::from_u128(12)),
            archive: digest('c')?,
        },
        JobSpec::RebuildIndex,
        JobSpec::ExportView {
            item_id: ItemId(Uuid::from_u128(10)),
            revision,
        },
        JobSpec::BackupInstallation,
        JobSpec::PurgeWorkspace {
            purge_id: PurgeId(Uuid::from_u128(13)),
            workspace_id: WorkspaceId(Uuid::from_u128(1)),
        },
        JobSpec::PurgeItem {
            purge_id: PurgeId(Uuid::from_u128(14)),
            workspace_id: WorkspaceId(Uuid::from_u128(1)),
            item_id: ItemId(Uuid::from_u128(10)),
        },
    ])
}

#[test]
fn a_tenant_kind_fits_only_the_tenant_scope() -> TestResult {
    let tenant_scope = JobScope::Tenant(tenant()?);
    let workspace = workspace_scope()?;
    let specs = every_spec()?;
    assert_eq!(specs.len(), 10);
    for spec in &specs {
        let tenant_kind = matches!(
            spec.kind(),
            JobKind::BackupInstallation | JobKind::PurgeWorkspace | JobKind::PurgeItem
        );
        assert_eq!(spec.fits(&tenant_scope), tenant_kind, "{spec:?}");
        assert_eq!(spec.fits(&workspace), !tenant_kind, "{spec:?}");
    }
    Ok(())
}

#[test]
fn every_new_spec_survives_storage_with_its_kind() -> TestResult {
    for spec in every_spec()? {
        let stored = serde_json::to_value(&spec)?;
        let kind = serde_json::to_value(spec.kind())?;
        assert_eq!(stored.get("kind"), Some(&kind), "{stored}");
        let loaded: JobSpec = serde_json::from_value(stored)?;
        assert_eq!(loaded, spec);
    }
    let redigest = JobSpec::Redigest {
        item_id: ItemId(Uuid::from_u128(10)),
        base_revision: Revision::try_from("a".repeat(40))?,
        settings: ConversionSettings::default(),
        pages: Some(vec![PageRange { start: 5, end: 8 }]),
    };
    let loaded: JobSpec = serde_json::from_value(serde_json::to_value(&redigest)?)?;
    assert_eq!(loaded, redigest);
    Ok(())
}

#[test]
fn a_restore_names_its_upload_and_archive_not_an_artifact() -> TestResult {
    let old_shape = serde_json::json!({
        "kind": "restore_workspace",
        "artifact_id": Uuid::from_u128(12),
        "sha256": "c".repeat(64),
    });
    err_of(serde_json::from_value::<JobSpec>(old_shape))?;
    Ok(())
}

#[test]
fn a_purge_job_names_its_purge_and_its_target() -> TestResult {
    let mutation = MutationId(Uuid::from_u128(5));
    let purge_id = derive_purge_id(mutation);
    assert_eq!(purge_id, derive_purge_id(mutation));
    assert_ne!(purge_id, derive_purge_id(MutationId(Uuid::from_u128(6))));
    let workspace_id = WorkspaceId(Uuid::from_u128(1));
    let item_id = ItemId(Uuid::from_u128(10));
    assert_eq!(
        purge_job_spec(&PurgeTarget::Workspace { workspace_id }, purge_id),
        JobSpec::PurgeWorkspace {
            purge_id,
            workspace_id
        }
    );
    let item = purge_job_spec(
        &PurgeTarget::Item {
            workspace_id,
            item_id,
        },
        purge_id,
    );
    assert_eq!(
        item,
        JobSpec::PurgeItem {
            purge_id,
            workspace_id,
            item_id
        }
    );
    assert!(item.fits(&JobScope::Tenant(tenant()?)));
    Ok(())
}

#[test]
fn an_artifact_download_path_follows_its_scope() -> TestResult {
    let artifact = ArtifactId(Uuid::from_u128(12));
    let workspace = workspace_scope()?;
    assert_eq!(
        artifact_download_path(&workspace, artifact),
        format!(
            "/api/workspaces/{}/artifacts/{}",
            Uuid::from_u128(1),
            Uuid::from_u128(12)
        )
    );
    let tenant_scope = JobScope::Tenant(tenant()?);
    assert_eq!(
        artifact_download_path(&tenant_scope, artifact),
        format!("/api/artifacts/{}", Uuid::from_u128(12))
    );
    let backup = ArtifactRecord {
        id: artifact,
        scope: tenant_scope.clone(),
        kind: ArtifactKind::InstallationBackup,
        object: ObjectInfo {
            digest: digest('d')?,
            size: 10,
        },
        media_type: "application/zip".to_owned(),
        created_by_job: JobId(Uuid::from_u128(7)),
    };
    assert_eq!(backup.kind.scope(), tenant_scope.artifact_scope());
    assert_eq!(tenant_scope.artifact_scope(), ArtifactScope::Tenant);
    assert_eq!(
        backup.download().download_path,
        "/api/artifacts/".to_owned() + &Uuid::from_u128(12).to_string()
    );
    assert_eq!(tenant_scope.workspace(), None);
    assert_eq!(tenant_scope.tenant(), &tenant()?);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
