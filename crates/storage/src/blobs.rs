//! Content-addressed bytes (`BlobStore`) on `object_store`'s local filesystem store.
//!
//! An object's key is `<tenant>/<first two digest characters>/<digest>`: shared inside one
//! tenant, never across tenants, and never keyed by workspace. `put` streams the body into a
//! private file under staging while hashing it, refuses it past the limit or on a digest
//! mismatch without retaining anything, and writes it through the store's multipart upload
//! (a temporary file renamed into place), with fsync. Reads stream the selected interval from
//! the retained file on the blocking pool, so a large object is never held in memory.

use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use object_store::local::LocalFileSystem;
use object_store::path::Path as ObjectPath;
use object_store::{GetOptions, ObjectStore, PutMultipartOptions, PutPayload};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, TenantId};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    BlobStore, ByteReader, LocalSource, ObjectInfo, ObjectRead, StorageScope,
};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt as _, ReadBuf};
use tokio::sync::mpsc;

use crate::data::{DataDir, io_error};
use crate::seam;

/// `BlobStore` over the data directory's object store.
#[derive(Debug, Clone)]
pub struct LocalBlobs {
    store: Arc<LocalFileSystem>,
    incoming: PathBuf,
    /// The data directory's single-writer lock, held while any copy of this value lives.
    _lock: Arc<DataDir>,
}

/// An `AsyncRead` over chunks a blocking reader sends; an error is passed on, never hidden as
/// an early end of stream.
struct ChunkReader {
    receiver: mpsc::Receiver<std::io::Result<Vec<u8>>>,
    current: Vec<u8>,
    position: usize,
}

/// Bytes read from a body or a file at a time.
const CHUNK: usize = 1 << 20;

impl LocalBlobs {
    /// Open the object store of the locked `data` directory; bodies stage under its staging.
    pub(crate) fn open(data: Arc<DataDir>) -> Result<Self, ApiError> {
        let store = LocalFileSystem::new_with_prefix(data.objects_path())
            .map_err(|error| store_error(&error))?
            .with_fsync(true);
        Ok(Self {
            store: Arc::new(store),
            incoming: data.staging_path().join("incoming"),
            _lock: data,
        })
    }

    /// Store a bounded stream in `tenant`'s objects; see `BlobStore::put`.
    pub(crate) async fn put_for(
        &self,
        tenant: &TenantId,
        body: ByteReader,
        limit: u64,
        expected: Option<Digest>,
    ) -> Result<ObjectInfo, ApiError> {
        std::fs::create_dir_all(&self.incoming)
            .map_err(|error| io_error("create", &self.incoming, &error))?;
        let staged = self.incoming.join(seam::hex(&seam::random_bytes::<16>()?));
        let received = receive(body, &staged, limit).await;
        let result = match received {
            Ok((digest, size)) => self.retain(tenant, &staged, digest, size, expected).await,
            Err(error) => Err(error),
        };
        // The staged copy is never referenced; a failure to remove it is cleared at startup.
        let _ = std::fs::remove_file(&staged);
        result
    }

    /// Open `tenant`'s object; see `BlobStore::open`.
    pub(crate) async fn open_for(
        &self,
        tenant: &TenantId,
        digest: &Digest,
        offset: u64,
        length: u64,
    ) -> Result<ObjectRead, ApiError> {
        let location = location(tenant, digest);
        let size = self.size(&location).await?;
        let start = offset.min(size);
        let length = length.min(size.saturating_sub(start));
        let path = self
            .store
            .path_to_filesystem(&location)
            .map_err(|error| store_error(&error))?;
        Ok(ObjectRead {
            object: ObjectInfo {
                digest: digest.clone(),
                size,
            },
            offset: start,
            length,
            body: file_reader(Some(path), start, length),
        })
    }

    /// Whether `tenant` holds the object.
    pub(crate) async fn contains(
        &self,
        tenant: &TenantId,
        digest: &Digest,
    ) -> Result<bool, ApiError> {
        match self.size(&location(tenant, digest)).await {
            Ok(_) => Ok(true),
            Err(error) if error.code == ErrorCode::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn size(&self, location: &ObjectPath) -> Result<u64, ApiError> {
        self.store
            .get_opts(location, GetOptions::new().with_head(true))
            .await
            .map(|result| result.meta.size)
            .map_err(|error| store_error(&error))
    }

    /// Check the digest, then move the staged bytes into the store unless already retained.
    async fn retain(
        &self,
        tenant: &TenantId,
        staged: &std::path::Path,
        digest: Digest,
        size: u64,
        expected: Option<Digest>,
    ) -> Result<ObjectInfo, ApiError> {
        if expected
            .as_ref()
            .is_some_and(|expected| *expected != digest)
        {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "the bytes do not have the expected SHA-256 digest; nothing was retained",
            ));
        }
        let location = location(tenant, &digest);
        if self.contains(tenant, &digest).await? {
            return Ok(ObjectInfo { digest, size });
        }
        let mut upload = self
            .store
            .put_multipart_opts(&location, PutMultipartOptions::default())
            .await
            .map_err(|error| store_error(&error))?;
        let mut file =
            std::fs::File::open(staged).map_err(|error| io_error("read", staged, &error))?;
        loop {
            let mut chunk = vec![0_u8; CHUNK];
            let read = file
                .read(&mut chunk)
                .map_err(|error| io_error("read", staged, &error))?;
            if read == 0 {
                break;
            }
            chunk.truncate(read);
            if let Err(error) = upload.put_part(PutPayload::from(Bytes::from(chunk))).await {
                let _ = upload.abort().await;
                return Err(store_error(&error));
            }
        }
        upload
            .complete()
            .await
            .map_err(|error| store_error(&error))?;
        Ok(ObjectInfo { digest, size })
    }
}

impl BlobStore for LocalBlobs {
    fn put<'a>(
        &'a self,
        scope: &'a StorageScope,
        body: ByteReader,
        limit: u64,
        expected: Option<Digest>,
    ) -> PortFuture<'a, ObjectInfo> {
        Box::pin(self.put_for(&scope.tenant_id, body, limit, expected))
    }

    fn open<'a>(
        &'a self,
        scope: &'a StorageScope,
        digest: &'a Digest,
        offset: u64,
        length: u64,
    ) -> PortFuture<'a, ObjectRead> {
        Box::pin(self.open_for(&scope.tenant_id, digest, offset, length))
    }

    fn materialize<'a>(
        &'a self,
        scope: &'a StorageScope,
        digest: &'a Digest,
    ) -> PortFuture<'a, LocalSource> {
        Box::pin(async move {
            let location = location(&scope.tenant_id, digest);
            let size = self.size(&location).await?;
            let path = self
                .store
                .path_to_filesystem(&location)
                .map_err(|error| store_error(&error))?;
            let checked = path.clone();
            let actual = tokio::task::spawn_blocking(move || hash_file(&checked))
                .await
                .map_err(|error| {
                    ApiError::new(
                        ErrorCode::Internal,
                        format!("a hashing task stopped: {error}"),
                    )
                })??;
            if actual != *digest {
                return Err(ApiError::new(
                    ErrorCode::Internal,
                    "a retained object no longer has the digest it is stored under",
                ));
            }
            Ok(LocalSource {
                path,
                object: ObjectInfo {
                    digest: digest.clone(),
                    size,
                },
            })
        })
    }
}

impl AsyncRead for ChunkReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        loop {
            if let Some(rest) = self.current.get(self.position..)
                && !rest.is_empty()
            {
                let count = rest.len().min(buffer.remaining());
                if let Some(part) = rest.get(..count) {
                    buffer.put_slice(part);
                }
                self.position = self.position.saturating_add(count);
                return Poll::Ready(Ok(()));
            }
            match self.receiver.poll_recv(context) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.current = chunk;
                    self.position = 0;
                }
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(error)),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Read the body into `staged`, hashing as it goes; refuse it past `limit`.
async fn receive(
    mut body: ByteReader,
    staged: &std::path::Path,
    limit: u64,
) -> Result<(Digest, u64), ApiError> {
    let mut file =
        std::fs::File::create(staged).map_err(|error| io_error("create", staged, &error))?;
    let mut hasher = Sha256::new();
    let mut size: u64 = 0;
    let mut chunk = vec![0_u8; CHUNK];
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
        size = size.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if size > limit {
            return Err(ApiError::new(
                ErrorCode::TooLarge,
                format!("the bytes exceed the limit of {limit}; nothing was retained"),
            ));
        }
        let part = chunk.get(..read).unwrap_or_default();
        hasher.update(part);
        file.write_all(part)
            .map_err(|error| io_error("write", staged, &error))?;
    }
    file.sync_all()
        .map_err(|error| io_error("sync", staged, &error))?;
    let digest = Digest::try_from(seam::hex(&hasher.finalize()))
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
    Ok((digest, size))
}

/// Send `length` bytes of `path` from `start` as chunks; an error is the last item sent.
fn stream_file(
    path: &std::path::Path,
    start: u64,
    length: u64,
    sender: &mpsc::Sender<std::io::Result<Vec<u8>>>,
) {
    let opened = std::fs::File::open(path).and_then(|mut file| {
        file.seek(SeekFrom::Start(start))?;
        Ok(file)
    });
    let mut file = match opened {
        Ok(file) => file.take(length),
        Err(error) => {
            let _ = sender.blocking_send(Err(error));
            return;
        }
    };
    loop {
        let mut chunk = vec![0_u8; CHUNK];
        match file.read(&mut chunk) {
            Ok(0) => return,
            Ok(read) => {
                chunk.truncate(read);
                if sender.blocking_send(Ok(chunk)).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.blocking_send(Err(error));
                return;
            }
        }
    }
}

/// A bounded reader of length bytes of path from start, streamed from the blocking pool;
/// None reads nothing.
pub(crate) fn file_reader(path: Option<PathBuf>, start: u64, length: u64) -> ByteReader {
    let (sender, receiver) = mpsc::channel(4);
    if let Some(path) = path {
        tokio::task::spawn_blocking(move || stream_file(&path, start, length, &sender));
    }
    Box::pin(ChunkReader {
        receiver,
        current: Vec::new(),
        position: 0,
    })
}

/// SHA-256 of a whole file.
pub(crate) fn hash_file(path: &std::path::Path) -> Result<Digest, ApiError> {
    let mut file = std::fs::File::open(path).map_err(|error| io_error("read", path, &error))?;
    let mut hasher = Sha256::new();
    let mut chunk = vec![0_u8; CHUNK];
    loop {
        let read = file
            .read(&mut chunk)
            .map_err(|error| io_error("read", path, &error))?;
        if read == 0 {
            break;
        }
        hasher.update(chunk.get(..read).unwrap_or_default());
    }
    Digest::try_from(seam::hex(&hasher.finalize()))
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
}

fn location(tenant: &TenantId, digest: &Digest) -> ObjectPath {
    let shard: String = digest.as_str().chars().take(2).collect();
    ObjectPath::from(format!("{}/{shard}/{}", tenant.as_str(), digest.as_str()))
}

fn store_error(error: &object_store::Error) -> ApiError {
    match error {
        object_store::Error::NotFound { .. } => {
            ApiError::new(ErrorCode::NotFound, "No object with that digest here")
        }
        other => ApiError::new(
            ErrorCode::Unavailable,
            format!("the object store failed: {other}"),
        ),
    }
}
