//! Upload slots track authenticated source occurrences before conversion begins.

use okf_jawn_contract::{
    identity::{Digest, UploadId},
    import::Upload,
};

use crate::ports::PortFuture;
use crate::storage::{ByteReader, ObjectInfo, StorageScope};

/// Durable upload registration and completion; storage owns the implementation.
pub trait UploadStore: Send + Sync {
    /// Register an upload slot before authenticated binary bytes arrive.
    fn create<'a>(&'a self, scope: &'a StorageScope, upload: Upload) -> PortFuture<'a, Upload>;
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
