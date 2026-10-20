//! The blob store, upload slots and connector credentials against the real data directory.
#![cfg(feature = "runtime")]

use okf_jawn_contract::access::{AccessRoute, Permission};
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{Digest, JobId, TenantId, UploadId};
use okf_jawn_contract::import::UploadListState;
use okf_jawn_core::credentials::{ConnectorIssue, CredentialStore, NewConnector, secret_hash};
use okf_jawn_core::storage::{BlobStore, ByteReader, Page, Provenance, StorageScope};
use okf_jawn_core::uploads::{NewUpload, UploadPage, UploadQuery, UploadStore};
use okf_jawn_storage::Storage;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use check::{TestResult, err_of, some};

/// The identity numbered `n`, of the type the context names.
macro_rules! uuid {
    ($n:expr) => {
        serde_json::from_value(json!(uuid_text($n)))
    };
}

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// How many rounds the batch race runs.
const RACE_ROUNDS: u32 = 300;

/// A fixed UUID spelling numbered `n`.
fn uuid_text(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

/// A number of round `round` of the batch race: `first` for round 0, then every second number,
/// so `numbered(n, round)` and `numbered(n + 1, round)` never meet across rounds.
fn numbered(first: u32, round: u32) -> u32 {
    first.saturating_add(round.saturating_mul(2))
}

fn scope(tenant: &str, workspace: u32) -> Fallible<StorageScope> {
    Ok(StorageScope {
        tenant_id: TenantId::try_from(tenant.to_owned())?,
        workspace_id: uuid!(workspace)?,
    })
}

fn body(bytes: &[u8]) -> ByteReader {
    Box::pin(std::io::Cursor::new(bytes.to_vec()))
}

fn sha256(bytes: &[u8]) -> Fallible<Digest> {
    let hash = Sha256::digest(bytes);
    Ok(Digest::try_from(format!("{hash:x}"))?)
}

async fn read_all(mut reader: ByteReader) -> Fallible<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await?;
    Ok(bytes)
}

#[tokio::test]
async fn blobs_are_content_addressed_within_a_tenant() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let blobs = storage.blobs();
    let local = scope("local", 1)?;
    let stored = blobs.put(&local, body(b"hello world"), 1024, None).await?;
    assert_eq!(stored.digest, sha256(b"hello world")?);
    assert_eq!(stored.size, 11);
    let again = blobs
        .put(&scope("local", 2)?, body(b"hello world"), 1024, None)
        .await?;
    assert_eq!(
        again, stored,
        "a second workspace of the tenant shares the object"
    );
    let read = blobs
        .open(&scope("local", 2)?, &stored.digest, 6, 100)
        .await?;
    assert_eq!((read.offset, read.length), (6, 5));
    assert_eq!(read_all(read.body).await?, b"world");
    let error = err_of(
        blobs
            .open(&scope("other", 1)?, &stored.digest, 0, 1)
            .await
            .map(|read| read.object),
    )?;
    assert_eq!(
        error.code,
        ErrorCode::NotFound,
        "objects are never shared across tenants"
    );
    let local_source = blobs.materialize(&local, &stored.digest).await?;
    assert_eq!(std::fs::read(&local_source.path)?, b"hello world");
    assert!(local_source.path.extension().is_none());
    Ok(())
}

#[tokio::test]
async fn a_blob_past_its_limit_or_with_another_digest_is_not_retained() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let blobs = storage.blobs();
    let local = scope("local", 1)?;
    let error = err_of(blobs.put(&local, body(b"too long"), 3, None).await)?;
    assert_eq!(error.code, ErrorCode::TooLarge);
    let error = err_of(
        blobs
            .put(&local, body(b"bytes"), 1024, Some(sha256(b"other")?))
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict);
    for digest in [sha256(b"too long")?, sha256(b"bytes")?] {
        let error = err_of(
            blobs
                .open(&local, &digest, 0, 1)
                .await
                .map(|read| read.object),
        )?;
        assert_eq!(error.code, ErrorCode::NotFound, "nothing was retained");
    }
    Ok(())
}

/// The detail of a completion refused for a mismatch: `expected` and `received` bytes, and
/// whether the digest matched.
fn mismatch(expected: &str, received: &str, digest_matches: Option<bool>) -> ErrorDetail {
    ErrorDetail::UploadMismatch {
        expected_bytes: expected.to_owned(),
        received_bytes: received.to_owned(),
        digest_matches,
    }
}

fn announced(size: u64) -> NewUpload {
    NewUpload {
        filename: "notes.txt".to_owned(),
        relative_path: String::new(),
        expected_size: size,
        expected_sha256: None,
        supplied_by: Provenance {
            subject: "owner".to_owned(),
            route: AccessRoute::LocalOwner,
            client_id: None,
        },
    }
}

#[tokio::test]
async fn one_transfer_writes_an_upload_at_a_time() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let local = scope("local", 1)?;
    let slot = uploads.create(&local, uuid!(5)?, announced(8)).await?;
    let (mut sender, receiver) = tokio::io::duplex(64);
    let first = {
        let uploads = storage.uploads();
        let local = local.clone();
        tokio::spawn(async move {
            uploads
                .put_content(&local, slot.id, Box::pin(receiver), 1024)
                .await
        })
    };
    sender.write_all(b"abcd").await?;
    // The first transfer is under way once its private file holds the first bytes.
    let incoming = directory
        .path()
        .join("uploads")
        .join(format!("{}.incoming", slot.id.0));
    let mut waited = 0_u32;
    while std::fs::metadata(&incoming).map_or(0, |metadata| metadata.len()) < 4 {
        assert!(waited < 500, "the first transfer never started");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        waited = waited.saturating_add(1);
    }
    let second = err_of(
        uploads
            .put_content(&local, slot.id, body(b"WXYZ"), 1024)
            .await,
    )?;
    assert_eq!(second.code, ErrorCode::Conflict, "{}", second.message);
    let finishing = err_of(
        uploads
            .complete(&local, slot.id, sha256(b"abcdefgh")?)
            .await,
    )?;
    assert_eq!(finishing.code, ErrorCode::Conflict, "{}", finishing.message);
    sender.write_all(b"efgh").await?;
    drop(sender);
    let written = first.await??;
    assert_eq!(written.received_bytes, 8);
    let complete = uploads
        .complete(&local, slot.id, sha256(b"abcdefgh")?)
        .await?;
    assert_eq!(
        some(complete.object, "the retained object")?.digest,
        sha256(b"abcdefgh")?,
        "the bytes of one transfer, never interleaved with another"
    );
    Ok(())
}

#[tokio::test]
async fn an_upload_is_not_completed_while_a_transfer_holds_it() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let local = scope("local", 1)?;
    let slot = uploads.create(&local, uuid!(5)?, announced(4)).await?;
    uploads
        .put_content(&local, slot.id, body(b"abcd"), 1024)
        .await?;
    // Every byte is received; a second transfer now holds the upload.
    let (sender, receiver) = tokio::io::duplex(64);
    let holding = {
        let uploads = storage.uploads();
        let local = local.clone();
        tokio::spawn(async move {
            uploads
                .put_content(&local, slot.id, Box::pin(receiver), 1024)
                .await
        })
    };
    let incoming = directory
        .path()
        .join("uploads")
        .join(format!("{}.incoming", slot.id.0));
    let mut waited = 0_u32;
    while !incoming.exists() {
        assert!(waited < 500, "the holding transfer never started");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        waited = waited.saturating_add(1);
    }
    let refused = err_of(uploads.complete(&local, slot.id, sha256(b"abcd")?).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict, "{}", refused.message);
    assert!(
        refused.message.contains("another transfer"),
        "refused for the transfer in progress: {}",
        refused.message
    );
    drop(sender);
    assert_eq!(holding.await??.received_bytes, 4);
    let complete = uploads.complete(&local, slot.id, sha256(b"abcd")?).await?;
    assert_eq!(some(complete.object, "the retained object")?.size, 4);
    Ok(())
}

#[tokio::test]
async fn a_resumed_transfer_counts_the_part_file_without_a_read_first() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let local = scope("local", 1)?;
    let slot = uploads.create(&local, uuid!(5)?, announced(9)).await?;
    uploads
        .put_content(&local, slot.id, body(b"abc"), 1024)
        .await?;
    let part = directory
        .path()
        .join("uploads")
        .join(format!("{}.part", slot.id.0));
    let mut file = std::fs::OpenOptions::new().append(true).open(&part)?;
    std::io::Write::write_all(&mut file, b"def")?;
    drop(file);
    // No `get` first: the transfer itself counts the six bytes, so four more pass the size.
    let over = err_of(
        uploads
            .put_content(&local, slot.id, body(b"ghij"), 1024)
            .await,
    )?;
    assert_eq!(over.code, ErrorCode::TooLarge, "{}", over.message);
    let resumed = uploads
        .put_content(&local, slot.id, body(b"ghi"), 1024)
        .await?;
    assert_eq!(resumed.received_bytes, 9);
    Ok(())
}

#[tokio::test]
async fn a_read_during_a_transfer_writes_no_count() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let local = scope("local", 1)?;
    let slot = uploads.create(&local, uuid!(5)?, announced(9)).await?;
    uploads
        .put_content(&local, slot.id, body(b"abc"), 1024)
        .await?;
    let (sender, receiver) = tokio::io::duplex(64);
    let holding = {
        let uploads = storage.uploads();
        let local = local.clone();
        tokio::spawn(async move {
            uploads
                .put_content(&local, slot.id, Box::pin(receiver), 1024)
                .await
        })
    };
    let uploads_dir = directory.path().join("uploads");
    let incoming = uploads_dir.join(format!("{}.incoming", slot.id.0));
    let mut waited = 0_u32;
    while !incoming.exists() {
        assert!(waited < 500, "the holding transfer never started");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        waited = waited.saturating_add(1);
    }
    // The part file grows as a transfer's append would, before that transfer records it.
    let part = uploads_dir.join(format!("{}.part", slot.id.0));
    let mut file = std::fs::OpenOptions::new().append(true).open(&part)?;
    std::io::Write::write_all(&mut file, b"def")?;
    drop(file);
    let seen = uploads.get(&local, slot.id).await?;
    assert_eq!(seen.received_bytes, 3, "the record as it is");
    let stored: i64 = rusqlite::Connection::open(storage.data().records_path())?.query_row(
        "SELECT received_bytes FROM uploads WHERE upload_id = ?1",
        [slot.id.0.to_string()],
        |row| row.get(0),
    )?;
    assert_eq!(stored, 3, "a read during a transfer writes no count");
    drop(sender);
    holding.await??;
    assert_eq!(
        uploads.get(&local, slot.id).await?.received_bytes,
        6,
        "with no transfer holding it, a read reconciles"
    );
    Ok(())
}

#[tokio::test]
async fn a_resumed_upload_counts_the_bytes_its_part_file_holds() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let local = scope("local", 1)?;
    let slot = uploads.create(&local, uuid!(5)?, announced(9)).await?;
    uploads
        .put_content(&local, slot.id, body(b"abc"), 1024)
        .await?;
    // A crash after the second chunk reached the part file and before its count was recorded.
    let part = directory
        .path()
        .join("uploads")
        .join(format!("{}.part", slot.id.0));
    let mut file = std::fs::OpenOptions::new().append(true).open(&part)?;
    std::io::Write::write_all(&mut file, b"def")?;
    drop(file);
    let resumed = uploads.get(&local, slot.id).await?;
    assert_eq!(
        resumed.received_bytes, 6,
        "the record is reconciled from the part file"
    );
    uploads
        .put_content(&local, slot.id, body(b"ghi"), 1024)
        .await?;
    let complete = uploads
        .complete(&local, slot.id, sha256(b"abcdefghi")?)
        .await?;
    assert_eq!(some(complete.object, "the retained object")?.size, 9);
    Ok(())
}

#[tokio::test]
async fn an_upload_is_received_verified_retained_and_consumed_once() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let local = scope("local", 1)?;
    let announce = NewUpload {
        filename: "Report.PDF".to_owned(),
        relative_path: "inbox/2026".to_owned(),
        expected_size: 10,
        expected_sha256: None,
        supplied_by: Provenance {
            subject: "owner".to_owned(),
            route: AccessRoute::LocalOwner,
            client_id: None,
        },
    };
    let slot = uploads.create(&local, uuid!(5)?, announce.clone()).await?;
    let repeated = uploads.create(&local, uuid!(5)?, announce).await?;
    assert_eq!(repeated.id, slot.id);
    assert_eq!(slot.filename, "Report.PDF");
    uploads
        .put_content(&local, slot.id, body(b"01234"), 1024)
        .await?;
    let error = err_of(
        uploads
            .put_content(&local, slot.id, body(b"567890"), 1024)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::TooLarge, "past the announced size");
    let partial = uploads.get(&local, slot.id).await?;
    assert_eq!(
        partial.received_bytes, 5,
        "the refused call appended nothing"
    );
    let error = err_of(
        uploads
            .complete(&local, slot.id, sha256(b"0123456789")?)
            .await,
    )?;
    assert_eq!(
        error.code,
        ErrorCode::Conflict,
        "five of ten bytes received"
    );
    // Ruling 5: the lengths are named, and the digest was not compared.
    assert_eq!(error.detail.as_deref(), Some(&mismatch("10", "5", None)));
    let error = err_of(uploads.consume(&local, slot.id, uuid!(7)?).await)?;
    assert_eq!(error.code, ErrorCode::InvalidInput);
    uploads
        .put_content(&local, slot.id, body(b"56789"), 1024)
        .await?;
    let error = err_of(uploads.complete(&local, slot.id, sha256(b"wrong")?).await)?;
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(
        error.detail.as_deref(),
        Some(&mismatch("10", "10", Some(false))),
        "every byte received, the digest differs"
    );
    let complete = uploads
        .complete(&local, slot.id, sha256(b"0123456789")?)
        .await?;
    let object = some(complete.object.clone(), "the retained object")?;
    assert_eq!(object.size, 10);
    let again = uploads
        .complete(&local, slot.id, sha256(b"0123456789")?)
        .await?;
    assert_eq!(again, complete);
    let other = err_of(uploads.complete(&local, slot.id, sha256(b"other")?).await)?;
    assert_eq!(other.code, ErrorCode::Conflict);
    assert_eq!(
        other.detail.as_deref(),
        Some(&mismatch("10", "10", Some(false))),
        "completed with another digest"
    );
    let read = storage.blobs().open(&local, &object.digest, 0, 10).await?;
    assert_eq!(read_all(read.body).await?, b"0123456789");
    let consumed = uploads.consume(&local, slot.id, uuid!(7)?).await?;
    assert_eq!(consumed.consumed_by, Some(uuid!(7)?));
    uploads.consume(&local, slot.id, uuid!(7)?).await?;
    let error = err_of(uploads.consume(&local, slot.id, uuid!(8)?).await)?;
    assert_eq!(error.code, ErrorCode::Conflict);
    let error = err_of(uploads.get(&scope("local", 2)?, slot.id).await)?;
    assert_eq!(error.code, ErrorCode::NotFound);
    Ok(())
}

/// Ruling 3: a workspace's slots are listed newest first, in each state (incomplete, waiting,
/// consumed) or all of them, a page at a time; another workspace's slots are not listed.
/// Ruling 5: a completion whose digest is not the one the slot announced is refused before any
/// byte is hashed, naming the lengths and that the digest does not match; the announced one
/// completes it.
#[tokio::test]
async fn a_completion_against_another_digest_than_announced_names_the_mismatch() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let local = scope("local", 1)?;
    let announce = NewUpload {
        expected_sha256: Some(sha256(b"0123")?),
        ..announced(4)
    };
    let slot = uploads.create(&local, uuid!(40)?, announce).await?;
    uploads
        .put_content(&local, slot.id, body(b"0123"), 1024)
        .await?;
    let error = err_of(uploads.complete(&local, slot.id, sha256(b"abcd")?).await)?;
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(
        error.detail.as_deref(),
        Some(&mismatch("4", "4", Some(false)))
    );
    let complete = uploads.complete(&local, slot.id, sha256(b"0123")?).await?;
    assert!(complete.object.is_some());
    Ok(())
}

#[tokio::test]
async fn uploads_are_listed_newest_first_by_state_and_paged() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let local = scope("local", 1)?;
    let open = uploads.create(&local, uuid!(30)?, announced(4)).await?.id;
    let waiting = complete_slot(&uploads, &local, 31, b"wait").await?;
    let consumed = complete_slot(&uploads, &local, 32, b"take").await?;
    uploads.consume(&local, consumed, uuid!(33)?).await?;
    complete_slot(&uploads, &scope("local", 2)?, 34, b"else").await?;
    let listed = |state: Option<UploadListState>, cursor: Option<String>, limit: u16| {
        let uploads = storage.uploads();
        let local = local.clone();
        async move {
            uploads
                .list(
                    &local,
                    UploadQuery {
                        state,
                        page: Page { cursor, limit },
                    },
                )
                .await
        }
    };
    let ids =
        |page: &UploadPage| -> Vec<UploadId> { page.items.iter().map(|slot| slot.id).collect() };
    let every = listed(None, None, 10).await?;
    assert_eq!(ids(&every), [consumed, waiting, open], "newest first");
    assert_eq!(every.next_cursor, None);
    for (state, expected) in [
        (UploadListState::Incomplete, open),
        (UploadListState::Waiting, waiting),
        (UploadListState::Consumed, consumed),
    ] {
        assert_eq!(
            ids(&listed(Some(state), None, 10).await?),
            [expected],
            "{state:?}"
        );
    }
    let first = listed(None, None, 2).await?;
    assert_eq!(ids(&first), [consumed, waiting]);
    let second = listed(None, first.next_cursor.clone(), 2).await?;
    assert_eq!(ids(&second), [open]);
    assert_eq!(second.next_cursor, None);
    let refused = err_of(listed(None, Some("not-a-cursor".to_owned()), 2).await)?;
    assert_eq!(refused.field.as_deref(), Some("/page/cursor"));
    Ok(())
}

/// The wire form of a slot carries what the slot recorded: its names, size, opening and taker.
#[tokio::test]
async fn a_slots_wire_form_carries_what_the_slot_recorded() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let local = scope("local", 1)?;
    let slot = complete_slot(&uploads, &local, 35, b"wire").await?;
    uploads.consume(&local, slot, uuid!(36)?).await?;
    let record = uploads.get(&local, slot).await?;
    let wire = record.to_wire(local.workspace_id);
    assert_eq!(
        wire.upload_path,
        format!(
            "/api/workspaces/{}/uploads/{}/content",
            local.workspace_id.0, slot.0
        )
    );
    assert!(wire.complete);
    assert_eq!(wire.received_bytes, "4");
    assert_eq!(wire.size.as_deref(), Some("4"));
    assert_eq!(wire.filename.as_deref(), Some("notes.txt"));
    assert_eq!(wire.relative_path.as_deref(), Some(""));
    assert_eq!(wire.created_at.as_ref(), Some(&record.created_at));
    assert_eq!(wire.consumed_by, Some(uuid!(36)?));
    Ok(())
}

/// A complete upload of `bytes` in `scope`, opened under mutation `mutation`.
async fn complete_slot(
    uploads: &dyn UploadStore,
    scope: &StorageScope,
    mutation: u32,
    bytes: &[u8],
) -> Fallible<UploadId> {
    let slot = uploads
        .create(
            scope,
            uuid!(mutation)?,
            announced(u64::try_from(bytes.len())?),
        )
        .await?;
    uploads
        .put_content(scope, slot.id, body(bytes), 1024)
        .await?;
    uploads.complete(scope, slot.id, sha256(bytes)?).await?;
    Ok(slot.id)
}

/// The job that consumed each of `slots`, as the store holds it now.
async fn consumers(
    uploads: &dyn UploadStore,
    scope: &StorageScope,
    slots: &[UploadId],
) -> Fallible<Vec<Option<JobId>>> {
    let mut found = Vec::with_capacity(slots.len());
    for slot in slots {
        found.push(uploads.get(scope, *slot).await?.consumed_by);
    }
    Ok(found)
}

/// R-I13: a batch is checked whole before any upload is marked. Each refusal is tried with the
/// upload that causes it listed last and listed first, so a batch that marks as it reads, or
/// that checks only its first upload, is caught either way.
#[tokio::test]
async fn a_refused_batch_of_uploads_marks_none_of_them() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let local = scope("local", 1)?;
    let (importer, other): (JobId, JobId) = (uuid!(7)?, uuid!(8)?);
    let free = complete_slot(&uploads, &local, 20, b"free").await?;
    let taken = complete_slot(&uploads, &local, 21, b"taken").await?;
    uploads.consume(&local, taken, other).await?;
    let open = uploads.create(&local, uuid!(22)?, announced(4)).await?.id;
    let elsewhere = complete_slot(&uploads, &scope("local", 2)?, 23, b"elsewhere").await?;
    let unknown: UploadId = uuid!(99)?;
    for (cause, code) in [
        (open, ErrorCode::InvalidInput),
        (taken, ErrorCode::Conflict),
        (unknown, ErrorCode::NotFound),
        (elsewhere, ErrorCode::NotFound),
    ] {
        for batch in [[free, cause], [cause, free]] {
            let error = err_of(uploads.consume_batch(&local, &batch, importer).await)?;
            assert_eq!(error.code, code, "{batch:?}: {}", error.message);
            assert_eq!(
                consumers(&uploads, &local, &[free, taken]).await?,
                [None, Some(other)],
                "{batch:?}: the refused batch marked nothing"
            );
        }
    }
    // The first upload that cannot be taken decides the refusal.
    let error = err_of(
        uploads
            .consume_batch(&local, &[open, taken], importer)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::InvalidInput, "{}", error.message);
    let error = err_of(
        uploads
            .consume_batch(&local, &[taken, open], importer)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    // The other workspace's upload was not marked from here either.
    assert_eq!(
        consumers(&uploads, &scope("local", 2)?, &[elsewhere]).await?,
        [None]
    );
    Ok(())
}

/// R-I13: a batch marks every upload it lists for its job, and a repeat by the same job changes
/// nothing and answers the same records.
#[tokio::test]
async fn a_batch_of_uploads_is_consumed_whole_and_a_repeat_changes_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let local = scope("local", 1)?;
    let (importer, other): (JobId, JobId) = (uuid!(7)?, uuid!(8)?);
    let first = complete_slot(&uploads, &local, 20, b"first").await?;
    let second = complete_slot(&uploads, &local, 21, b"second").await?;
    let third = complete_slot(&uploads, &local, 22, b"third").await?;
    assert_eq!(
        uploads.consume_batch(&local, &[], importer).await?,
        Vec::new(),
        "an empty batch"
    );
    let taken = uploads
        .consume_batch(&local, &[first, second], importer)
        .await?;
    assert_eq!(
        taken
            .iter()
            .map(|record| (record.id, record.consumed_by))
            .collect::<Vec<_>>(),
        [(first, Some(importer)), (second, Some(importer))],
        "the records, in the order given, as they now stand"
    );
    assert_eq!(
        consumers(&uploads, &local, &[first, second, third]).await?,
        [Some(importer), Some(importer), None]
    );
    assert_eq!(
        uploads
            .consume_batch(&local, &[first, second], importer)
            .await?,
        taken,
        "a repeat by the same job answers the same records"
    );
    // In the other order, and with an upload the job had not taken yet, listed twice.
    let more = uploads
        .consume_batch(&local, &[second, third, first, third], importer)
        .await?;
    assert_eq!(
        more.iter()
            .map(|record| (record.id, record.consumed_by))
            .collect::<Vec<_>>(),
        [second, third, first, third].map(|slot| (slot, Some(importer)))
    );
    assert_eq!(
        consumers(&uploads, &local, &[first, second, third]).await?,
        [Some(importer); 3]
    );
    // Another job is refused the whole of it, and a single consume still agrees.
    let error = err_of(uploads.consume_batch(&local, &[third], other).await)?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    let error = err_of(uploads.consume(&local, third, other).await)?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    assert_eq!(
        uploads.consume(&local, third, importer).await?.consumed_by,
        Some(importer)
    );
    Ok(())
}

/// R-I13, the ruling's race: two jobs consume the same two uploads at once, one listing them
/// [A, B] and the other [B, A]. In every round one job ends owning both and the other is
/// refused with `Conflict` owning neither; never one upload each. Both calls are released
/// together by a barrier, and exactly one refusal in a round shows both ran against the pair.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_batches_that_share_uploads_never_take_one_each() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let local = scope("local", 1)?;
    for round in 0..RACE_ROUNDS {
        let a = complete_slot(&uploads, &local, numbered(1000, round), b"a").await?;
        let b = complete_slot(&uploads, &local, numbered(1001, round), b"b").await?;
        let jobs: [JobId; 2] = [uuid!(numbered(5000, round))?, uuid!(numbered(5001, round))?];
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let mut racing = Vec::new();
        for (job, batch) in jobs.into_iter().zip([[a, b], [b, a]]) {
            let (uploads, local, barrier) = (storage.uploads(), local.clone(), barrier.clone());
            racing.push(tokio::spawn(async move {
                barrier.wait().await;
                uploads.consume_batch(&local, &batch, job).await
            }));
        }
        let mut winners = Vec::new();
        for (job, call) in jobs.into_iter().zip(racing) {
            match call.await? {
                Ok(records) => {
                    assert_eq!(records.len(), 2, "round {round}");
                    winners.push(job);
                }
                Err(error) => {
                    assert_eq!(error.code, ErrorCode::Conflict, "round {round}: {error}");
                }
            }
        }
        let [winner] = winners.as_slice() else {
            return Err(format!("round {round}: {} batches were taken", winners.len()).into());
        };
        assert_eq!(
            consumers(&uploads, &local, &[a, b]).await?,
            [Some(*winner); 2],
            "round {round}: one job owns both uploads"
        );
    }
    Ok(())
}

/// R-I13: a repeat by the same job with a different set. A superset that lists another job's
/// upload is refused whole, and the new upload it also listed stays free; a superset of free
/// uploads takes the new one and leaves the others; a subset changes nothing and releases
/// nothing. (Reviewer probe PO1, adopted.)
#[tokio::test]
async fn a_repeated_batch_with_a_different_set_takes_the_new_and_releases_none() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let local = scope("local", 1)?;
    let (importer, other): (JobId, JobId) = (uuid!(7)?, uuid!(8)?);
    let first = complete_slot(&uploads, &local, 20, b"a").await?;
    let second = complete_slot(&uploads, &local, 21, b"b").await?;
    let third = complete_slot(&uploads, &local, 22, b"c").await?;
    let theirs = complete_slot(&uploads, &local, 23, b"d").await?;
    let free = complete_slot(&uploads, &local, 24, b"e").await?;
    uploads.consume(&local, theirs, other).await?;
    uploads
        .consume_batch(&local, &[first, second], importer)
        .await?;
    // A superset holding another job's upload: refused whole, and the fifth stays free.
    let error = err_of(
        uploads
            .consume_batch(&local, &[first, second, free, theirs], importer)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    assert_eq!(
        consumers(&uploads, &local, &[first, second, third, theirs, free]).await?,
        [Some(importer), Some(importer), None, Some(other), None]
    );
    // A superset of free uploads: the new one is taken too.
    let more = uploads
        .consume_batch(&local, &[first, second, third], importer)
        .await?;
    assert_eq!(more.len(), 3);
    assert_eq!(
        consumers(&uploads, &local, &[first, second, third, theirs, free]).await?,
        [
            Some(importer),
            Some(importer),
            Some(importer),
            Some(other),
            None
        ]
    );
    // A subset: nothing changes, and nothing is released.
    let fewer = uploads.consume_batch(&local, &[first], importer).await?;
    assert_eq!(fewer.len(), 1);
    assert_eq!(
        consumers(&uploads, &local, &[first, second, third, theirs, free]).await?,
        [
            Some(importer),
            Some(importer),
            Some(importer),
            Some(other),
            None
        ]
    );
    Ok(())
}

/// R-I13: each refusal cause in the middle of a batch marks nothing before it or after it,
/// and one fresh upload listed twice gives two records and one mark. (Reviewer probe PO2,
/// adopted.)
#[tokio::test]
async fn a_cause_in_the_middle_of_a_batch_marks_none_and_an_upload_listed_twice_is_marked_once()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let uploads = storage.uploads();
    let local = scope("local", 1)?;
    let (importer, other): (JobId, JobId) = (uuid!(7)?, uuid!(8)?);
    let first = complete_slot(&uploads, &local, 20, b"first").await?;
    let last = complete_slot(&uploads, &local, 21, b"last").await?;
    let taken = complete_slot(&uploads, &local, 22, b"taken").await?;
    uploads.consume(&local, taken, other).await?;
    let open = uploads.create(&local, uuid!(23)?, announced(4)).await?.id;
    let unknown: UploadId = uuid!(99)?;
    for (cause, code) in [
        (open, ErrorCode::InvalidInput),
        (taken, ErrorCode::Conflict),
        (unknown, ErrorCode::NotFound),
    ] {
        let error = err_of(
            uploads
                .consume_batch(&local, &[first, cause, last], importer)
                .await,
        )?;
        assert_eq!(error.code, code, "{}", error.message);
        assert_eq!(
            consumers(&uploads, &local, &[first, last, taken]).await?,
            [None, None, Some(other)],
            "the refused batch marked nothing"
        );
    }
    let twice = uploads
        .consume_batch(&local, &[first, first], importer)
        .await?;
    assert_eq!(
        twice
            .iter()
            .map(|record| (record.id, record.consumed_by))
            .collect::<Vec<_>>(),
        [(first, Some(importer)), (first, Some(importer))]
    );
    assert_eq!(
        consumers(&uploads, &local, &[first, last]).await?,
        [Some(importer), None]
    );
    Ok(())
}

#[tokio::test]
async fn a_connector_secret_is_issued_once_and_kept_only_as_its_hash() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let credentials = storage.credentials();
    let identity = credentials.installation_identity().await?;
    assert_eq!(credentials.installation_identity().await?, identity);
    let request = NewConnector {
        label: "Claude".to_owned(),
        workspace_ids: vec![uuid!(1)?],
        allow_propose: true,
    };
    let ConnectorIssue::Issued(issued) = credentials
        .create_connector(uuid!(5)?, request.clone())
        .await?
    else {
        return Err("expected a new connector".into());
    };
    assert!(issued.secret.len() >= 64, "at least 32 random bytes");
    assert_eq!(
        issued.connector.permissions,
        vec![Permission::Read, Permission::Propose]
    );
    let ConnectorIssue::Existing(existing) =
        credentials.create_connector(uuid!(5)?, request).await?
    else {
        return Err("a repeated mutation must not issue a second secret".into());
    };
    assert_eq!(existing.connector_id, issued.connector.connector_id);
    let mut database = std::fs::read(storage.data().records_path())?;
    let wal = storage.data().root().join("records.sqlite-wal");
    if wal.exists() {
        database.extend(std::fs::read(&wal)?);
    }
    let plain = issued.secret.as_bytes();
    assert!(
        !database.windows(plain.len()).any(|window| window == plain),
        "the plaintext secret is never stored"
    );
    let found = some(
        credentials
            .lookup_connector(secret_hash(&issued.secret))
            .await?,
        "the connector by its secret",
    )?;
    assert_eq!(found.connector_id, issued.connector.connector_id);
    let rotated = credentials
        .rotate_connector_secret(issued.connector.connector_id)
        .await?;
    assert!(
        credentials
            .lookup_connector(secret_hash(&issued.secret))
            .await?
            .is_none()
    );
    assert!(
        credentials
            .lookup_connector(secret_hash(&rotated.secret))
            .await?
            .is_some()
    );
    credentials
        .revoke_connector(issued.connector.connector_id)
        .await?;
    assert!(
        credentials
            .lookup_connector(secret_hash(&rotated.secret))
            .await?
            .is_none()
    );
    assert_eq!(credentials.list_connectors(false).await?.len(), 0);
    assert_eq!(credentials.list_connectors(true).await?.len(), 1);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
