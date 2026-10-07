//! Upload slots track authenticated source occurrences before conversion begins.
//!
//! A slot records what the caller supplied (filename, relative folder, expected length and
//! hash) so the import that follows preserves the occurrence. Opening a slot takes
//! `MutationId`; a repeated id opens nothing and returns the prior slot.

use okf_jawn_contract::identity::{Digest, MutationId, UploadId};

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
    /// RFC 3339 time the slot was opened.
    pub created_at: String,
    /// Bytes durably received so far.
    pub received_bytes: u64,
    /// Retained object identity; present exactly when the upload is complete.
    pub object: Option<ObjectInfo>,
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
}
