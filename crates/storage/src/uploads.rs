//! Upload slots (`UploadStore`) over SQLite, with received bytes in `<data>/uploads`.
//!
//! A slot is unique on its `MutationId`. Bytes arrive in one or more `put_content` calls; each
//! call is received completely into a private file before it is appended, so a call that would
//! pass the limit or the announced size appends nothing. `complete` retains the bytes in the
//! blob store only when their length and SHA-256 match; `consume` marks a complete upload as the
//! input of exactly one job.
//!
//! One transfer writes an upload at a time: a concurrent `put_content` or `complete` on the same
//! upload is refused as `Conflict`, never interleaved into the same `.incoming` or `.part` file.
//! `received_bytes` is reconciled from the `.part` file's length whenever the upload is read for
//! a resume (`get`, `put_content`, `complete`), always under the upload's claim, so a crash
//! between the append and the record update never makes a resent chunk duplicate bytes. A `get`
//! while a transfer holds the claim reads the record as it is and writes nothing.

use std::collections::HashSet;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, JobId, MutationId, UploadId};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{ByteReader, ObjectInfo, StorageScope};
use okf_jawn_core::uploads::{NewUpload, UploadRecord, UploadStore};
use rusqlite::{Connection, OptionalExtension, params};
use tokio::io::AsyncReadExt as _;

use crate::blobs::{LocalBlobs, file_reader};
use crate::data::io_error;
use crate::db::{Db, conflict, json, not_found, scope_key, sql, to_i64, to_u64};
use crate::seam;

/// `UploadStore` over the records database and the blob store.
#[derive(Debug, Clone)]
pub struct SqliteUploads {
    db: Db,
    blobs: LocalBlobs,
    directory: PathBuf,
    writers: UploadWriters,
}

/// The uploads a transfer is writing now; shared by every `SqliteUploads` of one `Storage`.
#[derive(Debug, Clone, Default)]
pub(crate) struct UploadWriters {
    busy: Arc<Mutex<HashSet<UploadId>>>,
}

/// One transfer's claim on an upload, released when dropped.
struct Writing {
    writers: UploadWriters,
    upload: UploadId,
}

impl UploadWriters {
    /// Claim `upload` for one transfer; a second concurrent transfer is refused, never
    /// interleaved into the same files.
    fn claim(&self, upload: UploadId) -> Result<Writing, ApiError> {
        let mut busy = self.busy.lock().map_err(|_| {
            ApiError::new(ErrorCode::Internal, "the upload writer table was poisoned")
        })?;
        if !busy.insert(upload) {
            return Err(conflict(
                "another transfer to this upload is in progress; resume it after that one ends",
            ));
        }
        Ok(Writing {
            writers: self.clone(),
            upload,
        })
    }
}

impl Drop for Writing {
    fn drop(&mut self) {
        // A poisoned table is already reported to every later claim.
        if let Ok(mut busy) = self.writers.busy.lock() {
            busy.remove(&self.upload);
        }
    }
}

impl SqliteUploads {
    pub(crate) const fn new(
        db: Db,
        blobs: LocalBlobs,
        directory: PathBuf,
        writers: UploadWriters,
    ) -> Self {
        Self {
            db,
            blobs,
            directory,
            writers,
        }
    }

    /// The bytes durably in the `.part` file of an unfinished upload.
    fn part_length(&self, upload: UploadId) -> Result<u64, ApiError> {
        let part = self.part(upload);
        match std::fs::metadata(&part) {
            Ok(metadata) => Ok(metadata.len()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(io_error("inspect", &part, &error)),
        }
    }

    /// See `UploadStore::get`: the upload reconciled from its `.part` file when no transfer
    /// holds it, and as recorded when one does. A transfer in progress appends to `.part`
    /// before it records the count, so writing the count from the file then would race it; the
    /// transfer reconciles under its own claim instead.
    async fn look(&self, scope: &StorageScope, upload: UploadId) -> Result<UploadRecord, ApiError> {
        match self.writers.claim(upload) {
            Ok(_reading) => self.reconciled(scope, upload).await,
            Err(error) if error.code == ErrorCode::Conflict => self.record(scope, upload).await,
            Err(error) => Err(error),
        }
    }

    /// The upload with `received_bytes` reconciled from its `.part` file; only under the
    /// upload's claim.
    ///
    /// The `.part` file is synced before `received_bytes` is updated, so after a crash between
    /// the two the file holds more than the record says; those bytes were fully received and
    /// checked against the limit, so the record is brought up to the file. A resuming client
    /// then sends from the true offset and nothing is duplicated.
    async fn reconciled(
        &self,
        scope: &StorageScope,
        upload: UploadId,
    ) -> Result<UploadRecord, ApiError> {
        let record = self.record(scope, upload).await?;
        if record.object.is_some() {
            return Ok(record);
        }
        let durable = self.part_length(upload)?;
        if durable == record.received_bytes {
            return Ok(record);
        }
        let (tenant, workspace) = scope_key(scope);
        self.db
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "UPDATE uploads SET received_bytes = ?1
                         WHERE upload_id = ?2 AND object_digest IS NULL",
                        params![to_i64(durable)?, upload.0.to_string()],
                    )
                    .map_err(|error| sql(&error))?;
                read(transaction, &tenant, &workspace, upload)?.ok_or_else(|| not_found("upload"))
            })
            .await
    }

    fn part(&self, upload: UploadId) -> PathBuf {
        self.directory.join(format!("{}.part", upload.0))
    }

    async fn record(
        &self,
        scope: &StorageScope,
        upload: UploadId,
    ) -> Result<UploadRecord, ApiError> {
        let (tenant, workspace) = scope_key(scope);
        self.db
            .call(move |connection| {
                read(connection, &tenant, &workspace, upload)?.ok_or_else(|| not_found("upload"))
            })
            .await
    }

    async fn append(
        &self,
        scope: &StorageScope,
        upload: UploadId,
        mut body: ByteReader,
        limit: u64,
    ) -> Result<UploadRecord, ApiError> {
        let _writing = self.writers.claim(upload)?;
        let record = self.reconciled(scope, upload).await?;
        if record.object.is_some() {
            return Err(conflict("this upload is already complete"));
        }
        let ceiling = limit.min(record.expected_size);
        std::fs::create_dir_all(&self.directory)
            .map_err(|error| io_error("create", &self.directory, &error))?;
        let incoming = self.directory.join(format!("{}.incoming", upload.0));
        let mut file = std::fs::File::create(&incoming)
            .map_err(|error| io_error("create", &incoming, &error))?;
        let mut total = record.received_bytes;
        let mut chunk = vec![0_u8; 1 << 20];
        loop {
            let read = body.read(&mut chunk).await.map_err(|error| {
                ApiError::new(
                    ErrorCode::Unavailable,
                    format!("the body could not be read: {error}"),
                )
            })?;
            if read == 0 {
                break;
            }
            total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
            if total > ceiling {
                drop(file);
                let _ = std::fs::remove_file(&incoming);
                return Err(ApiError::new(
                    ErrorCode::TooLarge,
                    "the upload would exceed its announced size or the limit; nothing was appended",
                ));
            }
            file.write_all(chunk.get(..read).unwrap_or_default())
                .map_err(|error| io_error("write", &incoming, &error))?;
        }
        drop(file);
        append_file(&incoming, &self.part(upload))?;
        let _ = std::fs::remove_file(&incoming);
        let (tenant, workspace) = scope_key(scope);
        self.db
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "UPDATE uploads SET received_bytes = ?1 WHERE upload_id = ?2",
                        params![to_i64(total)?, upload.0.to_string()],
                    )
                    .map_err(|error| sql(&error))?;
                read(transaction, &tenant, &workspace, upload)?.ok_or_else(|| not_found("upload"))
            })
            .await
    }

    async fn finish(
        &self,
        scope: &StorageScope,
        upload: UploadId,
        sha256: Digest,
    ) -> Result<UploadRecord, ApiError> {
        let _writing = self.writers.claim(upload)?;
        let record = self.reconciled(scope, upload).await?;
        if let Some(object) = &record.object {
            if object.digest == sha256 {
                return Ok(record);
            }
            return Err(conflict("this upload was completed with another digest"));
        }
        if record.received_bytes != record.expected_size {
            return Err(conflict(format!(
                "received {} bytes of the {} announced",
                record.received_bytes, record.expected_size
            )));
        }
        if record
            .expected_sha256
            .as_ref()
            .is_some_and(|announced| *announced != sha256)
        {
            return Err(conflict(
                "the digest differs from the one announced for this upload",
            ));
        }
        let part = self.part(upload);
        let body = if record.expected_size == 0 {
            file_reader(None, 0, 0)
        } else {
            file_reader(Some(part.clone()), 0, record.expected_size)
        };
        let object = self
            .blobs
            .put_for(&scope.tenant_id, body, record.expected_size, Some(sha256))
            .await?;
        let (tenant, workspace) = scope_key(scope);
        let stored = self
            .db
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "UPDATE uploads SET object_digest = ?1, object_size = ?2
                         WHERE upload_id = ?3",
                        params![
                            object.digest.as_str(),
                            to_i64(object.size)?,
                            upload.0.to_string()
                        ],
                    )
                    .map_err(|error| sql(&error))?;
                read(transaction, &tenant, &workspace, upload)?.ok_or_else(|| not_found("upload"))
            })
            .await?;
        let _ = std::fs::remove_file(&part);
        Ok(stored)
    }
}

impl UploadStore for SqliteUploads {
    fn create<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        upload: NewUpload,
    ) -> PortFuture<'a, UploadRecord> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.transaction(move |transaction| {
            let id: UploadId = new_id!()?;
            transaction
                .execute(
                    "INSERT INTO uploads (upload_id, tenant_id, workspace_id, mutation_id, filename,
                         relative_path, expected_size, expected_sha256, supplied_by, created_at,
                         received_bytes)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)
                     ON CONFLICT (mutation_id) DO NOTHING",
                    params![
                        id.0.to_string(),
                        tenant,
                        workspace,
                        mutation_id.0.to_string(),
                        upload.filename,
                        upload.relative_path,
                        to_i64(upload.expected_size)?,
                        upload.expected_sha256.as_ref().map(Digest::as_str),
                        serde_json::to_string(&upload.supplied_by).map_err(|error| json(&error))?,
                        seam::now()?.as_str()
                    ],
                )
                .map_err(|error| sql(&error))?;
            let stored: String = transaction
                .query_row(
                    "SELECT upload_id FROM uploads WHERE mutation_id = ?1",
                    [mutation_id.0.to_string()],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            read(
                transaction,
                &tenant,
                &workspace,
                from_text!(stored.as_str())?,
            )?
            .ok_or_else(|| not_found("upload"))
        }))
    }

    fn get<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
    ) -> PortFuture<'a, UploadRecord> {
        Box::pin(self.look(scope, upload))
    }

    fn put_content<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
        body: ByteReader,
        limit: u64,
    ) -> PortFuture<'a, UploadRecord> {
        Box::pin(self.append(scope, upload, body, limit))
    }

    fn complete<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
        sha256: Digest,
    ) -> PortFuture<'a, UploadRecord> {
        Box::pin(self.finish(scope, upload, sha256))
    }

    fn consume<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
        job: JobId,
    ) -> PortFuture<'a, UploadRecord> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.transaction(move |transaction| {
            let record = read(transaction, &tenant, &workspace, upload)?
                .ok_or_else(|| not_found("upload"))?;
            if record.object.is_none() {
                return Err(ApiError::new(
                    ErrorCode::InvalidInput,
                    "an upload is consumed only once it is complete",
                ));
            }
            match record.consumed_by {
                Some(consumer) if consumer == job => Ok(record),
                Some(_) => Err(conflict("this upload was already taken by another job")),
                None => {
                    transaction
                        .execute(
                            "UPDATE uploads SET consumed_by = ?1 WHERE upload_id = ?2",
                            params![job.0.to_string(), upload.0.to_string()],
                        )
                        .map_err(|error| sql(&error))?;
                    Ok(UploadRecord {
                        consumed_by: Some(job),
                        ..record
                    })
                }
            }
        }))
    }
}

fn read(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
    upload: UploadId,
) -> Result<Option<UploadRecord>, ApiError> {
    let found = connection
        .query_row(
            "SELECT upload_id, filename, relative_path, expected_size, expected_sha256,
                 supplied_by, created_at, received_bytes, object_digest, object_size, consumed_by
             FROM uploads WHERE upload_id = ?1 AND tenant_id = ?2 AND workspace_id = ?3",
            params![upload.0.to_string(), tenant, workspace],
            |row| {
                Ok((
                    (
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ),
                    (
                        row.get::<_, String>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<i64>>(9)?,
                        row.get::<_, Option<String>>(10)?,
                    ),
                ))
            },
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((
        (id, filename, relative_path, expected_size, expected_sha256, supplied_by),
        (created_at, received, object_digest, object_size, consumed_by),
    )) = found
    else {
        return Ok(None);
    };
    let object = match (object_digest, object_size) {
        (Some(digest), Some(size)) => Some(ObjectInfo {
            digest: from_text!(digest.as_str())?,
            size: to_u64(size)?,
        }),
        _ => None,
    };
    Ok(Some(UploadRecord {
        id: from_text!(id.as_str())?,
        filename,
        relative_path,
        expected_size: to_u64(expected_size)?,
        expected_sha256: expected_sha256
            .map(|digest| from_text!(digest.as_str()))
            .transpose()?,
        supplied_by: serde_json::from_str(&supplied_by).map_err(|error| json(&error))?,
        created_at: from_text!(created_at.as_str())?,
        received_bytes: to_u64(received)?,
        object,
        consumed_by: consumed_by
            .map(|job| from_text!(job.as_str()))
            .transpose()?,
    }))
}

/// Append the whole of `from` to `to`, creating `to` when absent, and sync it.
fn append_file(from: &std::path::Path, to: &std::path::Path) -> Result<(), ApiError> {
    let mut source = std::fs::File::open(from).map_err(|error| io_error("read", from, &error))?;
    let mut target = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(to)
        .map_err(|error| io_error("open", to, &error))?;
    let mut chunk = vec![0_u8; 1 << 20];
    loop {
        let read = source
            .read(&mut chunk)
            .map_err(|error| io_error("read", from, &error))?;
        if read == 0 {
            break;
        }
        target
            .write_all(chunk.get(..read).unwrap_or_default())
            .map_err(|error| io_error("append to", to, &error))?;
    }
    target
        .sync_all()
        .map_err(|error| io_error("sync", to, &error))
}
