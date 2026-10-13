//! Upload slots track authenticated source occurrences before conversion begins.
//!
//! A slot records what the caller supplied (filename, relative folder, expected length and
//! hash) so the import that follows preserves the occurrence. Opening a slot takes
//! `MutationId`; a repeated id opens nothing and returns the prior slot.

use okf_jawn_contract::identity::{Digest, JobId, MutationId, Timestamp, UploadId};

use crate::ports::PortFuture;
use crate::storage::{ByteReader, ObjectInfo, Provenance, StorageScope};

/// One source occurrence about to be uploaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUpload {
    /// Original filename, preserved exactly.
    pub filename: String,
    /// Supplied folder context, preserved exactly; empty when none was supplied.
    pub relative_path: String,
    /// Byte count the caller announced.
    pub expected_size: u64,
    /// Content hash the caller announced, when it knew one.
    pub expected_sha256: Option<Digest>,
    /// Who supplies the bytes.
    pub supplied_by: Provenance,
}

/// A durable upload slot and how far it has come.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadRecord {
    /// Slot identity, allocated by the store.
    pub id: UploadId,
    /// Original filename.
    pub filename: String,
    /// Supplied folder context.
    pub relative_path: String,
    /// Byte count the caller announced.
    pub expected_size: u64,
    /// Content hash the caller announced, when it knew one.
    pub expected_sha256: Option<Digest>,
    /// Who supplies the bytes.
    pub supplied_by: Provenance,
    /// When the slot was opened, in the canonical UTC spelling.
    pub created_at: Timestamp,
    /// Bytes durably received so far.
    pub received_bytes: u64,
    /// Retained object identity; present exactly when the upload is complete.
    pub object: Option<ObjectInfo>,
    /// The job that took the upload as its input (an import or a restore); an upload is
    /// consumed once.
    pub consumed_by: Option<JobId>,
}

/// Durable upload registration and completion; storage owns the implementation.
pub trait UploadStore: Send + Sync {
    /// Open a slot before authenticated binary bytes arrive.
    ///
    /// Unique on `mutation_id`: a repeated id opens nothing and returns the prior slot.
    fn create<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        upload: NewUpload,
    ) -> PortFuture<'a, UploadRecord>;
    /// Read one slot; `NotFound` when the workspace has no such slot.
    fn get<'a>(&'a self, scope: &'a StorageScope, upload: UploadId)
    -> PortFuture<'a, UploadRecord>;
    /// Append authenticated bytes to a slot.
    ///
    /// Fails with `TooLarge` when the slot would exceed `limit` or its announced size.
    fn put_content<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
        body: ByteReader,
        limit: u64,
    ) -> PortFuture<'a, UploadRecord>;
    /// Verify the received bytes against the announced length and `sha256`, and retain them.
    ///
    /// Fails with `Conflict` on a length or hash mismatch and retains nothing. Completing a
    /// complete slot with the same `sha256` changes nothing and returns the same record.
    fn complete<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
        sha256: Digest,
    ) -> PortFuture<'a, UploadRecord>;
    /// Mark a complete upload as the input of `job`.
    ///
    /// Consuming it again for the same job changes nothing and returns the same record; a
    /// second consumer is refused with `Conflict`, and an incomplete upload with
    /// `InvalidInput`.
    ///
    /// Refused with `Conflict` naming the purge, before the upload is read, when a purge names
    /// the workspace (`storage::Purger`), in the same transaction as the read and the mark:
    /// the upload is not marked, and an upload the purge already removed is refused this way,
    /// not answered `NotFound`. A caller's retry meets the same refusal, so the job that
    /// consumes (a restore) is refused for good.
    fn consume<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
        job: JobId,
    ) -> PortFuture<'a, UploadRecord>;
    /// Mark the complete uploads of `uploads` as the input of `job` in one transaction: all of
    /// them, or none.
    ///
    /// The uploads are checked in the order given before any is marked, and the first that
    /// cannot be taken refuses the batch, which then marks none: `NotFound` for one the
    /// workspace does not hold, `InvalidInput` for an incomplete one, `Conflict` for one another
    /// job consumed. An upload `job` already consumed stays as it is, so repeating the batch
    /// changes nothing and returns the same records. An upload listed twice is the same upload
    /// at each place it is listed. Returns the records in the order given; an empty batch marks
    /// nothing and returns none.
    ///
    /// Two jobs whose batches share uploads, listed in whatever order, do not end holding some
    /// of them each: one batch takes every upload it lists, and the other is refused with
    /// `Conflict` having marked none. An import consumes its uploads with this, not with one
    /// `consume` for each.
    ///
    /// Refused with `Conflict` naming the purge, before any upload is read, when a purge names
    /// the workspace (`storage::Purger`).
    fn consume_batch<'a>(
        &'a self,
        scope: &'a StorageScope,
        uploads: &'a [UploadId],
        job: JobId,
    ) -> PortFuture<'a, Vec<UploadRecord>>;
}
