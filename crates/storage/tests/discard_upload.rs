//! Discard tombstone/replay vectors from lifecycle design section 4 and next brief section 2.
//! Real SQLite, upload files and tenant CAS; jobs are real records, not running handlers.
//! Initial nine behavior cases observed RED at the explicit discard compatibility stub.
//! The card-rooted and real post-tombstone unlink-failure cases below are added before their RED.
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use check::{TestResult, err_of, some};
use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::ConversionSettings;
use okf_jawn_contract::identity::{Digest, Revision, TenantId, UploadId, WorkspacePath};
use okf_jawn_contract::import::{CleanUpReport, JobState};
use okf_jawn_contract::purge::PurgeTarget;
use okf_jawn_core::jobs::{JobCompletion, JobScope, JobSpec, NewJob, NewPurge, RecordStore};
use okf_jawn_core::storage::{
    BlobStore, ByteReader, CandidateCheck, CommitChanges, NewWorkspace, Page, Provenance, Purger,
    SourceCard, StorageScope, TreeEdit, VersionStore, WorkspaceCatalog,
};
use okf_jawn_core::uploads::{NewUpload, UploadQuery, UploadRecord, UploadStore};
use okf_jawn_storage::Storage;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _, ReadBuf};

/// The same fixed identities/body/scope idiom as the neighboring object-store tests.
macro_rules! uuid {
    ($n:expr) => {
        serde_json::from_value(json!(uuid_text($n)))
    };
}

/// Signals only after actual body bytes were read, under the store's real upload writer claim.
struct StartedBody {
    reader: tokio::io::DuplexStream,
    started: Option<tokio::sync::oneshot::Sender<()>>,
}

/// Storage composition fixture: conformance policy is core's, not this adapter's.
struct Accept;

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

impl AsyncRead for StartedBody {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buffer.filled().len();
        match Pin::new(&mut this.reader).poll_read(context, buffer) {
            Poll::Ready(Ok(())) => {
                if buffer.filled().len() > before
                    && let Some(started) = this.started.take()
                    && started.send(()).is_err()
                {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "the discard test stopped observing its upload",
                    )));
                }
                Poll::Ready(Ok(()))
            }
            outcome => outcome,
        }
    }
}

impl CandidateCheck for Accept {
    fn check(&self, _root: &Path, _rewritten: &[WorkspacePath]) -> Result<Vec<Warning>, ApiError> {
        Ok(Vec::new())
    }
}

fn uuid_text(number: u32) -> String {
    format!("00000000-0000-4000-8000-{number:012}")
}

fn scope(tenant: &str, workspace: u32) -> Fallible<StorageScope> {
    Ok(StorageScope {
        tenant_id: TenantId::try_from(tenant.to_owned())?,
        workspace_id: uuid!(workspace)?,
    })
}

fn supplier() -> Provenance {
    Provenance {
        subject: "agent-ana".to_owned(),
        route: AccessRoute::McpDelegation,
        client_id: Some("import-client".to_owned()),
    }
}

fn body(bytes: &[u8]) -> ByteReader {
    Box::pin(std::io::Cursor::new(bytes.to_vec()))
}

fn sha256(bytes: &[u8]) -> Fallible<Digest> {
    let hash = Sha256::digest(bytes);
    Ok(Digest::try_from(format!("{hash:x}"))?)
}

fn announced(bytes: &[u8]) -> Fallible<NewUpload> {
    Ok(NewUpload {
        filename: "notes.txt".to_owned(),
        relative_path: "reports/October".to_owned(),
        expected_size: bytes.len().try_into()?,
        expected_sha256: Some(sha256(bytes)?),
        supplied_by: supplier(),
    })
}

fn part(directory: &Path, upload: UploadId, extension: &str) -> PathBuf {
    directory
        .join("uploads")
        .join(format!("{}.{extension}", upload.0))
}

async fn complete(
    storage: &Storage,
    scope: &StorageScope,
    mutation: u32,
) -> Fallible<UploadRecord> {
    let uploads = storage.uploads();
    let slot = uploads
        .create(scope, uuid!(mutation)?, announced(b"abcdefgh")?)
        .await?;
    uploads
        .put_content(scope, slot.id, body(b"abcdefgh"), 1024)
        .await?;
    Ok(uploads
        .complete(scope, slot.id, sha256(b"abcdefgh")?)
        .await?)
}

fn job(mutation: u32, spec: JobSpec) -> Fallible<NewJob> {
    Ok(NewJob {
        mutation_id: uuid!(mutation)?,
        initiator: supplier(),
        spec,
    })
}

/// Use a real durable claim, real collection pass and real completion, never a canned report.
/// Core's queue/handler integration is a separate connected journey, not simulated here.
async fn clean_up(storage: &Storage, tenant: &TenantId) -> Fallible<CleanUpReport> {
    let records = storage.records();
    let job = records
        .create_job(
            &JobScope::Tenant(tenant.clone()),
            job(100, JobSpec::CleanUp)?,
        )
        .await?;
    let claim = some(records.claim_job(job.id).await?, "the real cleanup claim")?;
    let report = storage.purger().clean_up(tenant, job.id).await?;
    let completed = records
        .complete_job(JobCompletion {
            lease: claim.lease,
            revision: None,
            item_ids: Vec::new(),
            artifact: None,
            restore: None,
            backup: None,
            clean_up: Some(report),
            outputs: Vec::new(),
            warnings: Vec::new(),
        })
        .await?;
    assert_eq!(completed.state, JobState::Succeeded);
    Ok(some(
        completed.clean_up,
        "the actual completed cleanup report",
    )?)
}

#[tokio::test]
async fn partial_discard_removes_files_and_reopens_with_the_exact_prior_record() -> TestResult {
    let directory = tempfile::tempdir()?;
    let local = scope("local", 1)?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let slot = uploads
        .create(&local, uuid!(10)?, announced(b"abcdefgh")?)
        .await?;
    uploads
        .put_content(&local, slot.id, body(b"abc"), 1024)
        .await?;
    let prior = uploads.get(&local, slot.id).await?;
    assert_eq!(prior.filename, "notes.txt");
    assert_eq!(prior.relative_path, "reports/October");
    assert_eq!(prior.expected_size, 8);
    assert_eq!(prior.expected_sha256, Some(sha256(b"abcdefgh")?));
    assert_eq!(prior.supplied_by, supplier());
    assert_eq!(prior.received_bytes, 3);
    assert_eq!(prior.object, None);
    assert_eq!(prior.consumed_by, None);
    let partial = part(directory.path(), slot.id, "part");
    let incoming = part(directory.path(), slot.id, "incoming");
    assert_eq!(std::fs::read(&partial)?, b"abc");
    // A crash-left private file, with no active transfer/writer claim.
    std::fs::write(&incoming, b"unappended")?;
    assert_eq!(uploads.discard(&local, slot.id, uuid!(11)?).await?, prior);
    assert!(!partial.exists());
    assert!(!incoming.exists());
    assert_eq!(
        err_of(uploads.get(&local, slot.id).await)?.code,
        ErrorCode::NotFound
    );
    let listed = uploads
        .list(
            &local,
            UploadQuery {
                state: None,
                page: Page {
                    cursor: None,
                    limit: 10,
                },
            },
        )
        .await?;
    assert_eq!(listed.items, [] as [UploadRecord; 0]);
    assert_eq!(listed.next_cursor, None);
    drop(uploads);
    drop(storage);

    // Arrange the post-commit/pre-unlink residue without claiming an actual process crash.
    std::fs::write(&partial, b"abc")?;
    std::fs::write(&incoming, b"unappended")?;
    let reopened = Storage::open(directory.path())?;
    let uploads = reopened.uploads();
    assert_eq!(uploads.discard(&local, slot.id, uuid!(11)?).await?, prior);
    assert!(!partial.exists());
    assert!(!incoming.exists());
    assert_eq!(uploads.discard(&local, slot.id, uuid!(11)?).await?, prior);
    assert_eq!(
        err_of(uploads.discard(&local, slot.id, uuid!(12)?).await)?.code,
        ErrorCode::NotFound
    );
    Ok(())
}

#[tokio::test]
async fn discarded_creation_cannot_resurrect_or_replay_as_another_scope_or_upload() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let local = scope("local", 1)?;
    let uploads = storage.uploads();
    let slot = uploads
        .create(&local, uuid!(10)?, announced(b"abcdefgh")?)
        .await?;
    let prior = uploads.discard(&local, slot.id, uuid!(11)?).await?;
    let other = uploads
        .create(&local, uuid!(20)?, announced(b"abcdefgh")?)
        .await?;
    assert_eq!(
        err_of(uploads.discard(&local, other.id, uuid!(11)?).await)?.code,
        ErrorCode::Conflict
    );
    assert_eq!(uploads.get(&local, other.id).await?, other);
    for foreign in [scope("other", 1)?, scope("local", 2)?] {
        // Design 4.1: NotFound when that workspace holds no such slot, never another scope's record.
        assert_eq!(
            err_of(uploads.discard(&foreign, slot.id, uuid!(11)?).await)?.code,
            ErrorCode::NotFound
        );
    }
    assert_eq!(
        err_of(
            uploads
                .create(&local, uuid!(10)?, announced(b"abcdefgh")?)
                .await
        )?
        .code,
        ErrorCode::Conflict
    );
    assert_eq!(uploads.discard(&local, slot.id, uuid!(11)?).await?, prior);
    assert_eq!(uploads.get(&local, other.id).await?, other);
    Ok(())
}

#[tokio::test]
async fn completed_discard_keeps_shared_cas_and_is_absent_from_every_upload_consumer() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let local = scope("local", 1)?;
    let removed = complete(&storage, &local, 10).await?;
    let survivor = complete(&storage, &local, 20).await?;
    let uploads = storage.uploads();
    assert_eq!(
        uploads.discard(&local, removed.id, uuid!(11)?).await?,
        removed
    );
    assert_eq!(uploads.get(&local, survivor.id).await?, survivor);
    let object = some(
        removed.object.clone(),
        "the completed upload's CAS identity",
    )?;
    assert_eq!(survivor.object, Some(object.clone()));
    let mut read = storage
        .blobs()
        .open(&local, &object.digest, 0, 1024)
        .await?;
    let mut bytes = Vec::new();
    read.body.read_to_end(&mut bytes).await?;
    assert_eq!(bytes, b"abcdefgh");
    assert_eq!(
        err_of(
            uploads
                .put_content(&local, removed.id, body(b"x"), 1024)
                .await
        )?
        .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        err_of(
            uploads
                .complete(&local, removed.id, sha256(b"abcdefgh")?)
                .await
        )?
        .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        err_of(uploads.consume(&local, removed.id, uuid!(90)?).await)?.code,
        ErrorCode::NotFound
    );
    // The first surviving input must not be marked when the second input was discarded.
    assert_eq!(
        err_of(
            uploads
                .consume_batch(&local, &[survivor.id, removed.id], uuid!(90)?)
                .await
        )?
        .code,
        ErrorCode::NotFound
    );
    assert_eq!(uploads.get(&local, survivor.id).await?, survivor);
    let report = clean_up(&storage, &local.tenant_id).await?;
    assert_eq!(report.objects_removed, "0");
    assert_eq!(report.objects_kept, "1");
    assert_eq!(report.upload_files_removed, 0);
    assert_eq!(
        storage
            .blobs()
            .open(&local, &object.digest, 0, 1024)
            .await?
            .object,
        object
    );
    Ok(())
}

/// V4.2's other side: discard drops only the slot's root, then the actual durable cleanup
/// removes its unshared CAS bytes. Discard itself must leave them available until that pass.
#[tokio::test]
async fn completed_discard_leaves_unshared_bytes_for_real_cleanup_to_remove() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let local = scope("local", 1)?;
    let slot = complete(&storage, &local, 10).await?;
    let object = some(slot.object.clone(), "the completed slot's object")?;
    assert_eq!(
        storage
            .uploads()
            .discard(&local, slot.id, uuid!(11)?)
            .await?,
        slot
    );
    assert_eq!(
        storage
            .blobs()
            .open(&local, &object.digest, 0, 1024)
            .await?
            .object,
        object
    );
    let report = clean_up(&storage, &local.tenant_id).await?;
    assert_eq!(report.objects_removed, "1");
    assert_eq!(report.objects_kept, "0");
    assert_eq!(report.upload_files_removed, 0);
    let absent = err_of(
        storage
            .blobs()
            .open(&local, &object.digest, 0, 1024)
            .await
            .map(|read| read.object),
    )?;
    assert_eq!(absent.code, ErrorCode::NotFound);
    Ok(())
}

#[tokio::test]
async fn consumed_upload_refuses_discard_without_changing_the_slot_or_object() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let local = scope("local", 1)?;
    let completed = complete(&storage, &local, 10).await?;
    let records = storage.records();
    let consuming = records
        .create_job(
            &JobScope::Workspace(local.clone()),
            job(30, JobSpec::RebuildIndex)?,
        )
        .await?;
    let uploads = storage.uploads();
    let prior = uploads.consume(&local, completed.id, consuming.id).await?;
    let refused = err_of(uploads.discard(&local, prior.id, uuid!(11)?).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(
        refused.message.contains(&consuming.id.0.to_string()),
        "{}",
        refused.message
    );
    assert_eq!(uploads.get(&local, prior.id).await?, prior);
    let object = some(prior.object, "consumed input still retains its CAS object")?;
    assert_eq!(
        storage
            .blobs()
            .open(&local, &object.digest, 0, 1024)
            .await?
            .object,
        object
    );
    Ok(())
}

#[tokio::test]
async fn second_import_input_is_reserved_while_queued_or_running_but_not_after_failure()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let local = scope("local", 1)?;
    let first = complete(&storage, &local, 10).await?;
    let second = complete(&storage, &local, 20).await?;
    let records = storage.records();
    let jobs = JobScope::Workspace(local.clone());
    let importing = records
        .create_job(
            &jobs,
            job(
                30,
                JobSpec::Import {
                    base_revision: Revision::try_from("a".repeat(40))?,
                    upload_ids: vec![first.id, second.id],
                    destination: None,
                    apply_naming_rules: true,
                    settings: ConversionSettings::default(),
                },
            )?,
        )
        .await?;
    assert_eq!(importing.state, JobState::Queued);
    let uploads = storage.uploads();
    let queued = err_of(uploads.discard(&local, second.id, uuid!(21)?).await)?;
    assert_eq!(queued.code, ErrorCode::Conflict);
    assert!(
        queued.message.contains(&importing.id.0.to_string()),
        "{}",
        queued.message
    );
    assert_eq!(uploads.get(&local, first.id).await?, first);
    assert_eq!(uploads.get(&local, second.id).await?, second);
    let running = some(
        records.claim_job(importing.id).await?,
        "the real queued import claim",
    )?;
    let refused = err_of(uploads.discard(&local, second.id, uuid!(21)?).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(
        refused.message.contains(&importing.id.0.to_string()),
        "{}",
        refused.message
    );
    assert_eq!(uploads.get(&local, second.id).await?, second);
    let failed = records
        .fail_job(
            running.lease,
            ApiError::new(
                ErrorCode::Unavailable,
                "conversion stopped before consuming inputs",
            ),
            true,
        )
        .await?;
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(
        uploads.discard(&local, second.id, uuid!(21)?).await?,
        second
    );
    assert_eq!(uploads.get(&local, first.id).await?, first);
    Ok(())
}

#[tokio::test]
async fn queued_restore_reserves_its_unconsumed_upload_without_changing_it() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let local = scope("local", 1)?;
    let slot = complete(&storage, &local, 10).await?;
    let object = some(slot.object.clone(), "completed restore input")?;
    // The real RecordStore persists the reservation; no archive parser/restore handler runs.
    let restoring = storage
        .records()
        .create_job(
            &JobScope::Workspace(local.clone()),
            job(
                30,
                JobSpec::RestoreWorkspace {
                    upload_id: slot.id,
                    archive: object.digest.clone(),
                    editors: vec!["ana".to_owned()],
                },
            )?,
        )
        .await?;
    assert_eq!(restoring.state, JobState::Queued);
    let refused = err_of(storage.uploads().discard(&local, slot.id, uuid!(11)?).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(
        refused.message.contains(&restoring.id.0.to_string()),
        "{}",
        refused.message
    );
    assert_eq!(storage.uploads().get(&local, slot.id).await?, slot);
    assert_eq!(
        storage
            .blobs()
            .open(&local, &object.digest, 0, 1024)
            .await?
            .object,
        object
    );
    Ok(())
}

/// V4.4: the real writer claim excludes discard while a controlled body remains in flight.
/// Release/join before assertions so even a wrong refusal leaves no blocked transfer task.
#[tokio::test]
async fn discard_refuses_a_transfer_mid_body_then_succeeds_after_it_ends() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let local = scope("local", 1)?;
    let uploads = storage.uploads();
    let slot = uploads
        .create(&local, uuid!(10)?, announced(b"abcdefgh")?)
        .await?;
    let (mut sender, reader) = tokio::io::duplex(64);
    sender.write_all(b"abcd").await?;
    let (started, observed) = tokio::sync::oneshot::channel();
    let transferring = {
        let uploads = storage.uploads();
        let local = local.clone();
        tokio::spawn(async move {
            uploads
                .put_content(
                    &local,
                    slot.id,
                    Box::pin(StartedBody {
                        reader,
                        started: Some(started),
                    }),
                    1024,
                )
                .await
        })
    };
    observed.await?;
    let refused = uploads.discard(&local, slot.id, uuid!(11)?).await;
    sender.write_all(b"efgh").await?;
    drop(sender);
    let transferred = transferring.await?;
    assert_eq!(err_of(refused)?.code, ErrorCode::Conflict);
    let transferred = transferred?;
    assert_eq!(transferred.received_bytes, 8);
    assert_eq!(
        uploads.discard(&local, slot.id, uuid!(11)?).await?,
        transferred
    );
    assert_eq!(
        err_of(
            uploads
                .complete(&local, slot.id, sha256(b"abcdefgh")?)
                .await
        )?
        .code,
        ErrorCode::NotFound
    );
    Ok(())
}

/// V4.5: recording a workspace purge refuses discard before it can change its row or files.
#[tokio::test]
async fn discard_refuses_a_recorded_purge_without_removing_the_slot_or_files() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let local = scope("local", 1)?;
    let uploads = storage.uploads();
    let slot = uploads
        .create(&local, uuid!(10)?, announced(b"abcdefgh")?)
        .await?;
    let partial = uploads
        .put_content(&local, slot.id, body(b"abc"), 1024)
        .await?;
    let purge = storage
        .records()
        .create_purge(
            &local.tenant_id,
            uuid!(30)?,
            NewPurge {
                id: uuid!(80)?,
                target: PurgeTarget::Workspace {
                    workspace_id: local.workspace_id,
                },
                initiator: supplier(),
            },
        )
        .await?;
    let refused = err_of(uploads.discard(&local, slot.id, uuid!(11)?).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(
        refused.message.contains(&purge.id.0.to_string()),
        "{}",
        refused.message
    );
    assert_eq!(uploads.get(&local, slot.id).await?, partial);
    assert_eq!(
        std::fs::read(part(directory.path(), slot.id, "part"))?,
        b"abc"
    );
    Ok(())
}

/// V4.2: a completed upload can leave while an accepted source card keeps the same object.
/// Exactly one card-rooted object survives the actual cleanup; no second upload roots it.
#[tokio::test]
async fn discard_safety_keeps_card_rooted_bytes_through_real_cleanup() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let tenant = TenantId::try_from("local".to_owned())?;
    let workspace = storage
        .catalog()
        .create(
            &tenant,
            uuid!(1)?,
            NewWorkspace {
                name: "Cards".to_owned(),
                description: String::new(),
                creator: supplier(),
            },
        )
        .await?;
    let local = StorageScope {
        tenant_id: tenant,
        workspace_id: workspace.id,
    };
    let slot = complete(&storage, &local, 10).await?;
    let object = some(slot.object.as_ref(), "the completed bytes")?;
    let versions = storage.versions();
    let committed = versions
        .commit(
            &local,
            CommitChanges {
                mutation_id: uuid!(2)?,
                expected_head: versions.head(&local).await?,
                author: supplier(),
                message: "Keep the source occurrence".to_owned(),
                edits: vec![TreeEdit::WriteSourceCard(Box::new(SourceCard {
                    item_id: uuid!(3)?,
                    path: WorkspacePath::try_from("sources/kept.md".to_owned())?,
                    title: "notes.txt".to_owned(),
                    type_name: "Source".to_owned(),
                    body: String::new(),
                    properties: BTreeMap::new(),
                    appearance: serde_json::from_value(json!({
                        "object": object.digest.as_str(),
                        "names": [{
                            "filename": "notes.txt", "folder": "reports/October",
                            "observed_at": "2026-10-19T12:00:00.000Z", "supplied_by": "agent-ana"
                        }],
                        "media_type": "text/plain", "size": "8", "metadata": {},
                        "extraction": {
                            "outcome": {"status": "unsupported"}, "text_origin": "none",
                            "corrected": false, "warnings": []
                        }
                    }))?,
                }))],
            },
            Arc::new(Accept),
        )
        .await?;
    assert_eq!(
        storage
            .uploads()
            .discard(&local, slot.id, uuid!(11)?)
            .await?,
        slot
    );
    let report = clean_up(&storage, &local.tenant_id).await?;
    assert_eq!(report.objects_removed, "0");
    assert_eq!(report.objects_kept, "1");
    assert_eq!(report.upload_files_removed, 0);
    assert_eq!(versions.head(&local).await?, committed.revision);
    let mut retained = storage
        .blobs()
        .open(&local, &object.digest, 0, 1024)
        .await?;
    let mut bytes = Vec::new();
    retained.body.read_to_end(&mut bytes).await?;
    assert_eq!(bytes, b"abcdefgh");
    Ok(())
}

/// Section4.4's post-transaction recovery: a real unlink failure occurs after the tombstone
/// commits, never success.
/// An empty directory at the private incoming-file path makes file removal fail on both platforms.
/// Repairing only that obstruction lets the same mutation answer the exact durable prior slot.
#[tokio::test]
async fn discard_safety_retries_after_a_real_post_tombstone_unlink_failure() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let local = scope("local", 1)?;
    let uploads = storage.uploads();
    let slot = uploads
        .create(&local, uuid!(10)?, announced(b"abcdefgh")?)
        .await?;
    let prior = uploads
        .put_content(&local, slot.id, body(b"abc"), 1024)
        .await?;
    let incoming = part(directory.path(), slot.id, "incoming");
    std::fs::create_dir(&incoming)?;
    let failed = err_of(uploads.discard(&local, slot.id, uuid!(11)?).await)?;
    assert_eq!(failed.code, ErrorCode::Unavailable);
    let connection = rusqlite::Connection::open(storage.data().records_path())?;
    let live: i64 = connection.query_row(
        "SELECT count(*) FROM uploads WHERE upload_id=?1",
        [slot.id.0.to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(live, 0);
    let tombstone: (String, String, i64) = connection.query_row(
        "SELECT mutation_id, created_by, received_bytes FROM discarded_uploads WHERE upload_id=?1",
        [slot.id.0.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(tombstone, (uuid_text(11), uuid_text(10), 3));
    assert_eq!(
        err_of(uploads.get(&local, slot.id).await)?.code,
        ErrorCode::NotFound
    );
    assert_eq!(
        err_of(
            uploads
                .create(&local, uuid!(10)?, announced(b"abcdefgh")?)
                .await
        )?
        .code,
        ErrorCode::Conflict
    );
    assert!(incoming.is_dir());
    // Only remove this test's empty obstruction; the product must finish its own part cleanup.
    std::fs::remove_dir(&incoming)?;
    assert_eq!(uploads.discard(&local, slot.id, uuid!(11)?).await?, prior);
    assert_eq!(uploads.discard(&local, slot.id, uuid!(11)?).await?, prior);
    assert!(!part(directory.path(), slot.id, "part").exists());
    assert!(!incoming.exists());
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
