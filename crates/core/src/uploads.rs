//! Upload slots track authenticated source occurrences before conversion begins.
//!
//! Opening an upload takes `MutationId` and enforces uniqueness.

use okf_jawn_contract::{
    identity::{Digest, MutationId, UploadId},
    import::Upload,
};

use crate::ports::PortFuture;
use crate::storage::{ByteReader, ObjectInfo, StorageScope};

/// Durable upload registration and completion; storage owns the implementation.
pub trait UploadStore: Send + Sync {
    /// Register an upload slot before authenticated binary bytes arrive.
    ///
    /// Unique on `mutation_id`; a reused id returns the prior row, never a second slot.
    fn create<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        upload: Upload,
    ) -> PortFuture<'a, Upload>;
    /// Look up an upload created under `mutation_id`, for abandoned-lease reconciliation.
    fn find_by_mutation<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
    ) -> PortFuture<'a, Option<Upload>>;
    /// Read one upload slot.
    fn get<'a>(&'a self, scope: &'a StorageScope, upload: UploadId) -> PortFuture<'a, Upload>;
    /// Append authenticated bytes into a preallocated upload, enforcing limits.
    fn put_content<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
        body: ByteReader,
        limit: u64,
    ) -> PortFuture<'a, Upload>;
    /// Verify expected length and digest, producing retained object identity when complete.
    fn complete<'a>(
        &'a self,
        scope: &'a StorageScope,
        upload: UploadId,
        sha256: Digest,
    ) -> PortFuture<'a, (Upload, ObjectInfo)>;
}
