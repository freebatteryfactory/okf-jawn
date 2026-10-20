//! The offline commands against temporary data directories: `backup` writes a set that `restore
//! --replacement` brings back after the directory is deleted; both refuse while another holder
//! owns the directory's lock; `backup` refuses an output that is not empty or lies inside the
//! data directory, and `--tenant` in local mode; `restore` keeps storage's refusals. The
//! commands hold no behaviour of their own: they parse, check, open storage and call it.
#![cfg(feature = "runtime")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::identity::{MutationId, TenantId, WorkspaceId};
use okf_jawn_core::credentials::CredentialStore as _;
use okf_jawn_core::storage::{NewWorkspace, Provenance, WorkspaceCatalog as _};
use okf_jawn_server::config::AuthKind;
use okf_jawn_server::offline::{BackupArgs, RestoreArgs, backup, restore};
use okf_jawn_storage::Storage;

use check::{TestResult, err_of, some};

/// A temporary directory, removed when dropped (best effort).
struct Scratch(PathBuf);

static NEXT: AtomicU64 = AtomicU64::new(0);

impl Scratch {
    fn new(name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!(
            "okf-jawn-offline-{name}-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a leftover directory under the temporary directory harms nothing.
        let _removed = std::fs::remove_dir_all(&self.0);
    }
}

fn mutation(n: u32) -> Result<MutationId, serde_json::Error> {
    serde_json::from_value(serde_json::json!(format!(
        "00000000-0000-4000-8000-{n:012}"
    )))
}

/// Create a workspace of `tenant` in the data directory `data`, and its installation identity;
/// every store is dropped before this returns. Answers the workspace id and the identity.
async fn seed(
    data: &Path,
    tenant: &str,
    n: u32,
) -> Result<(WorkspaceId, String), Box<dyn std::error::Error>> {
    let storage = Storage::open(data)?;
    let identity = storage.credentials().installation_identity().await?;
    let created = storage
        .catalog()
        .create(
            &TenantId::try_from(tenant.to_owned())?,
            mutation(n)?,
            NewWorkspace {
                name: format!("Workspace {n}"),
                description: String::new(),
                creator: Provenance {
                    subject: identity.subject.clone(),
                    route: AccessRoute::LocalOwner,
                    client_id: None,
                },
            },
        )
        .await?;
    Ok((created.id, identity.subject))
}

fn names(paths: &[PathBuf], root: &Path) -> Vec<String> {
    let mut names: Vec<String> = paths
        .iter()
        .filter_map(|path| path.strip_prefix(root).ok())
        .map(|relative| {
            relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();
    names.sort();
    names
}

fn backup_args(output: &Path, tenant: Option<&str>) -> BackupArgs {
    BackupArgs {
        output: output.to_path_buf(),
        tenant: tenant.map(str::to_owned),
    }
}

#[tokio::test]
async fn a_local_backup_writes_every_archive_of_the_local_tenant() -> TestResult {
    let scratch = Scratch::new("local")?;
    let data = scratch.join("data");
    let (workspace, _) = seed(&data, "local", 1).await?;
    let output = scratch.join("set");
    let written = backup(AuthKind::Local, &data, &backup_args(&output, None)).await?;
    assert_eq!(
        names(&written, &output),
        vec![
            "installation.zip".to_owned(),
            "installation/local.zip".to_owned(),
            format!("workspaces/local/{}.zip", workspace.0),
        ]
    );
    assert!(written.iter().all(|path| path.is_file()));
    Ok(())
}

#[tokio::test]
async fn a_local_backup_of_a_directory_with_no_workspace_still_names_the_local_tenant() -> TestResult
{
    let scratch = Scratch::new("empty")?;
    let data = scratch.join("data");
    drop(Storage::open(&data)?);
    let output = scratch.join("set");
    let written = backup(AuthKind::Local, &data, &backup_args(&output, None)).await?;
    assert_eq!(
        names(&written, &output),
        vec![
            "installation.zip".to_owned(),
            "installation/local.zip".to_owned()
        ]
    );
    Ok(())
}

#[tokio::test]
async fn both_commands_refuse_while_another_holder_owns_the_lock() -> TestResult {
    let scratch = Scratch::new("locked")?;
    let data = scratch.join("data");
    seed(&data, "local", 1).await?;
    let first = scratch.join("first");
    let written = backup(AuthKind::Local, &data, &backup_args(&first, None)).await?;
    let installation = some(
        written
            .iter()
            .find(|path| path.ends_with("installation/local.zip")),
        "the local installation archive",
    )?
    .clone();
    let held = Storage::open(&data)?;
    let output = scratch.join("second");
    let refused = err_of(backup(AuthKind::Local, &data, &backup_args(&output, None)).await)?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert!(refused.message.contains("already owns the data directory"));
    assert!(!output.exists(), "the refused backup created its output");
    let fresh = scratch.join("fresh");
    let fresh_held = Storage::open(&fresh)?;
    let refused = err_of(
        restore(
            &fresh,
            &RestoreArgs {
                installation,
                workspaces: Vec::new(),
            },
        )
        .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    drop(fresh_held);
    drop(held);
    Ok(())
}

#[tokio::test]
async fn a_backup_refuses_an_output_that_holds_anything() -> TestResult {
    let scratch = Scratch::new("occupied")?;
    let data = scratch.join("data");
    seed(&data, "local", 1).await?;
    let output = scratch.join("set");
    std::fs::create_dir_all(&output)?;
    std::fs::write(output.join("older-archive.zip"), "older")?;
    let refused = err_of(backup(AuthKind::Local, &data, &backup_args(&output, None)).await)?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("--output"));
    assert_eq!(std::fs::read_dir(&output)?.count(), 1);
    // An existing empty directory is an output.
    let empty = scratch.join("empty");
    std::fs::create_dir_all(&empty)?;
    backup(AuthKind::Local, &data, &backup_args(&empty, None)).await?;
    // A file is not.
    let file = scratch.join("a-file");
    std::fs::write(&file, "")?;
    let refused = err_of(backup(AuthKind::Local, &data, &backup_args(&file, None)).await)?;
    assert_eq!(refused.field.as_deref(), Some("--output"));
    Ok(())
}

#[tokio::test]
async fn a_backup_refuses_an_output_inside_the_data_directory() -> TestResult {
    let scratch = Scratch::new("inside")?;
    let data = scratch.join("data");
    seed(&data, "local", 1).await?;
    for inside in [
        data.clone(),
        data.join("backups").join("mine"),
        data.join("new").join("deeper"),
    ] {
        let refused = err_of(backup(AuthKind::Local, &data, &backup_args(&inside, None)).await)?;
        assert_eq!(
            refused.code,
            ErrorCode::InvalidInput,
            "{}",
            inside.display()
        );
        assert_eq!(refused.field.as_deref(), Some("--output"));
    }
    // A sibling whose name only begins like the data directory's is outside it.
    let sibling = scratch.join("data-backup");
    backup(AuthKind::Local, &data, &backup_args(&sibling, None)).await?;
    Ok(())
}

/// Server M2 design 6.1 step 4: a backup must remain outside the original data directory.
/// A missing sibling followed by `..` must not hide that the output ultimately lies inside it.
/// The expected refusal and untouched records are literals from that acceptance rule.
#[tokio::test]
async fn a_backup_refuses_an_inside_output_reached_through_a_missing_sibling() -> TestResult {
    let scratch = Scratch::new("inside-through-parent")?;
    let data = scratch.join("data");
    seed(&data, "local", 1).await?;
    let records = data.join(okf_jawn_storage::data::RECORDS_FILE);
    let before = std::fs::read(&records)?;
    let missing = scratch.join("missing");
    let output = missing.join("..").join("data").join("backup");
    assert!(!missing.exists());
    assert!(!data.join("backup").exists());

    let refused = err_of(backup(AuthKind::Local, &data, &backup_args(&output, None)).await)?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("--output"));
    assert!(
        !missing.exists(),
        "a refusal must not create the missing sibling"
    );
    assert!(
        !data.join("backup").exists(),
        "a refusal must not write an internal backup"
    );
    assert_eq!(
        std::fs::read(&records)?,
        before,
        "a refusal must leave records unchanged"
    );
    Ok(())
}

/// SPEC12: a backup must outlive its data directory. A missing Windows data path is not
/// outside merely because an alternate Unicode case spelling precedes the output directory.
#[cfg(windows)]
#[tokio::test]
async fn a_backup_refuses_an_inside_unicode_case_output_for_a_missing_data_directory() -> TestResult
{
    let scratch = Scratch::new("unicode-case")?;
    let probe = scratch.join("probe-DÁTÁ");
    std::fs::create_dir(&probe)?;
    std::fs::write(probe.join("marker"), b"same directory")?;
    let alias = scratch.join("probe-dátá");
    if !alias.join("marker").is_file() {
        eprintln!("NOT EXERCISED: filesystem does not resolve this Unicode case alias");
        return Ok(());
    }
    assert_eq!(probe.canonicalize()?, alias.canonicalize()?);
    assert_eq!(std::fs::read(alias.join("marker"))?, b"same directory");
    eprintln!("VERIFIED: Unicode case alias resolves to the same directory");
    let data = scratch.join("DÁTÁ");
    let output = scratch.join("dátá").join("backup");
    assert!(
        !data.exists(),
        "the regression requires a missing data directory"
    );
    assert!(!output.exists());

    let refused = err_of(backup(AuthKind::Local, &data, &backup_args(&output, None)).await)?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("--output"));
    assert!(
        !output.exists() && !data.join("backup").exists(),
        "a refused backup must not create its output inside the actual data directory"
    );
    Ok(())
}

#[tokio::test]
async fn a_tenant_is_named_only_in_a_hosted_backup() -> TestResult {
    let scratch = Scratch::new("tenant")?;
    let data = scratch.join("data");
    let (a, _) = seed(&data, "org_a", 1).await?;
    seed(&data, "org_b", 2).await?;
    let refused = err_of(
        backup(
            AuthKind::Local,
            &data,
            &backup_args(&scratch.join("local"), Some("local")),
        )
        .await,
    )?;
    assert_eq!(refused.field.as_deref(), Some("--tenant"));
    let refused = err_of(
        backup(
            AuthKind::Hosted,
            &data,
            &backup_args(&scratch.join("bad"), Some("not a tenant")),
        )
        .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("--tenant"));
    let output = scratch.join("org-a");
    let written = backup(
        AuthKind::Hosted,
        &data,
        &backup_args(&output, Some("org_a")),
    )
    .await?;
    assert_eq!(
        names(&written, &output),
        vec![
            "installation.zip".to_owned(),
            "installation/org_a.zip".to_owned(),
            format!("workspaces/org_a/{}.zip", a.0),
        ]
    );
    // Without a tenant a hosted backup is every tenant's.
    let every = scratch.join("every");
    let written = backup(AuthKind::Hosted, &data, &backup_args(&every, None)).await?;
    assert_eq!(written.len(), 5);
    Ok(())
}

#[tokio::test]
async fn a_deleted_directory_is_replaced_from_its_backup() -> TestResult {
    let scratch = Scratch::new("replace")?;
    let data = scratch.join("data");
    let (workspace, owner) = seed(&data, "local", 1).await?;
    let output = scratch.join("set");
    let written = backup(AuthKind::Local, &data, &backup_args(&output, None)).await?;
    std::fs::remove_dir_all(&data)?;
    let installation = output.join("installation").join("local.zip");
    let archive = output
        .join("workspaces")
        .join("local")
        .join(format!("{}.zip", workspace.0));
    assert!(written.contains(&installation) && written.contains(&archive));
    let reports = restore(
        &data,
        &RestoreArgs {
            installation,
            workspaces: vec![archive],
        },
    )
    .await?;
    assert_eq!(reports.len(), 1);
    let storage = Storage::open(&data)?;
    let listed = storage.catalog().list(&TenantId::local(), true).await?;
    assert_eq!(
        listed.iter().map(|entry| entry.id).collect::<Vec<_>>(),
        vec![workspace]
    );
    assert_eq!(
        storage.credentials().installation_identity().await?.subject,
        owner
    );
    Ok(())
}

#[tokio::test]
async fn a_restore_keeps_storage_s_refusals_and_writes_nothing_then() -> TestResult {
    let scratch = Scratch::new("refusals")?;
    let data = scratch.join("data");
    let (a, _) = seed(&data, "org_a", 1).await?;
    let (b, _) = seed(&data, "org_b", 2).await?;
    let output = scratch.join("set");
    backup(AuthKind::Hosted, &data, &backup_args(&output, None)).await?;
    let installation_a = output.join("installation").join("org_a.zip");
    let archive_a = output
        .join("workspaces")
        .join("org_a")
        .join(format!("{}.zip", a.0));
    let archive_b = output
        .join("workspaces")
        .join("org_b")
        .join(format!("{}.zip", b.0));
    // Another tenant's workspace archive: refused before anything is written.
    let fresh = scratch.join("fresh");
    let refused = err_of(
        restore(
            &fresh,
            &RestoreArgs {
                installation: installation_a.clone(),
                workspaces: vec![archive_b],
            },
        )
        .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    {
        let storage = Storage::open(&fresh)?;
        let tenant = TenantId::try_from("org_a".to_owned())?;
        assert!(storage.catalog().list(&tenant, true).await?.is_empty());
    }
    // The installation-level archive restores no workspace.
    let refused = err_of(
        restore(
            &fresh,
            &RestoreArgs {
                installation: output.join("installation.zip"),
                workspaces: vec![archive_a.clone()],
            },
        )
        .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    // A workspace that exists, not restored from this archive.
    let refused = err_of(
        restore(
            &data,
            &RestoreArgs {
                installation: installation_a.clone(),
                workspaces: vec![archive_a],
            },
        )
        .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    // A file that is not an archive.
    let junk = scratch.join("junk.zip");
    std::fs::write(&junk, "not a zip")?;
    let refused = err_of(
        restore(
            &scratch.join("other"),
            &RestoreArgs {
                installation: junk,
                workspaces: Vec::new(),
            },
        )
        .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    // A path that names nothing is refused by name, before storage is opened.
    let untouched = scratch.join("untouched");
    let refused = err_of(
        restore(
            &untouched,
            &RestoreArgs {
                installation: scratch.join("missing.zip"),
                workspaces: Vec::new(),
            },
        )
        .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("--replacement"));
    assert!(!untouched.exists());
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
