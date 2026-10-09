//! Minimal fakes of `VersionStore`, `BlobStore` and `RecordStore` for the functions that
//! compose ports (`authorize_object`, `check_revision`, `retain_dataset`); not production
//! adapters.
//!
//! Each fake answers only the methods those functions call. Every other method returns
//! `Internal` ("not used by this test"), so a function that starts calling one fails loudly
//! instead of being answered with an invented value.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use okf_jawn_contract::{
    conventions::NamingRules,
    error::{ApiError, ErrorCode},
    events::Receipt,
    history::{BlameResponse, DiffResponse, LogResponse},
    identity::{
        ArtifactId, Digest, ItemId, JobId, MutationId, ProposalId, PurgeId, ReceiptId, Revision,
        TenantId, WorkspacePath,
    },
    import::{Job, ListJobsResponse},
    item::{ItemDocument, TypeDefinition},
    purge::Purge,
    review::Review,
};
use okf_jawn_core::jobs::{
    ArtifactRecord, ClaimedJob, DerivedObject, JobCompletion, JobLease, JobScope, NewArtifact,
    NewJob, NewPurge, RecordStore, RevisionMapping,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{
    BlameQuery, BlobStore, ByteReader, CandidateChanges, CandidateCheck, CommitChanges, Committed,
    DiffQuery, FolderListing, LocalSource, LogQuery, ObjectInfo, ObjectRead, Page, Promotion,
    StorageScope, VersionStore,
};
use tokio::io::AsyncReadExt;

/// `VersionStore` whose `show` answers one document whatever is asked.
pub struct FakeVersions {
    /// The document `show` returns.
    pub document: ItemDocument,
}

/// What the fake blob store does when a test opens an object.
pub enum Open {
    /// The object's bytes, reported at their real size.
    Serve(Vec<u8>),
    /// The store fails.
    Fail,
    /// The store reports an object past what core reads, with an empty body.
    Oversized(u64),
}

/// One `BlobStore::put` as the fake saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Put {
    /// The bytes read from the body.
    pub bytes: Vec<u8>,
    /// The limit the caller passed.
    pub limit: u64,
    /// The digest the caller expected.
    pub expected: Option<Digest>,
}

/// `BlobStore` that does one scripted thing on `open` and records every `put`.
pub struct FakeBlobs {
    open: Open,
    opens: AtomicUsize,
    puts: Mutex<Vec<Put>>,
}

/// `RecordStore` that answers derived-object lookups and revision mappings from a script, and
/// keeps what is recorded.
///
/// `derived_object` matches on the digest alone, whatever item and revision are asked, so the
/// item and revision rule under test is core's, not the fake's.
pub struct FakeRecords {
    derived: Vec<DerivedObject>,
    mapping: Option<RevisionMapping>,
    recorded: Mutex<Vec<DerivedObject>>,
}

impl VersionStore for FakeVersions {
    fn head<'a>(&'a self, _scope: &'a StorageScope) -> PortFuture<'a, Revision> {
        unused()
    }
    fn list<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _folder: Option<&'a WorkspacePath>,
        _page: Page,
    ) -> PortFuture<'a, FolderListing> {
        unused()
    }
    fn show<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _item: ItemId,
    ) -> PortFuture<'a, ItemDocument> {
        let document = self.document.clone();
        Box::pin(async move { Ok(document) })
    }
    fn read_file<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _path: &'a WorkspacePath,
    ) -> PortFuture<'a, Vec<u8>> {
        unused()
    }
    fn rules<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
    ) -> PortFuture<'a, Option<NamingRules>> {
        unused()
    }
    fn types<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
    ) -> PortFuture<'a, Vec<TypeDefinition>> {
        unused()
    }
    fn correction<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
        _item: ItemId,
        _digest: &'a Digest,
    ) -> PortFuture<'a, Option<String>> {
        unused()
    }
    fn commit<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _changes: CommitChanges,
        _check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Committed> {
        unused()
    }
    fn find_commit<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _since: &'a Revision,
    ) -> PortFuture<'a, Option<Revision>> {
        unused()
    }
    fn create_candidate<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _proposal_id: ProposalId,
        _changes: CandidateChanges,
        _check: Arc<dyn CandidateCheck>,
    ) -> PortFuture<'a, Revision> {
        unused()
    }
    fn promote_candidate<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _promotion: Promotion,
    ) -> PortFuture<'a, Committed> {
        unused()
    }
    fn log<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: LogQuery,
    ) -> PortFuture<'a, LogResponse> {
        unused()
    }
    fn diff<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: DiffQuery,
    ) -> PortFuture<'a, DiffResponse> {
        unused()
    }
    fn blame<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _query: BlameQuery,
    ) -> PortFuture<'a, BlameResponse> {
        unused()
    }
}

impl FakeBlobs {
    /// A store whose `open` does `open`.
    #[must_use]
    pub const fn new(open: Open) -> Self {
        Self {
            open,
            opens: AtomicUsize::new(0),
            puts: Mutex::new(Vec::new()),
        }
    }

    /// How many times `open` was called.
    pub fn opens(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }

    /// Every `put`, in order.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn puts(&self) -> Result<Vec<Put>, ApiError> {
        self.puts
            .lock()
            .map(|puts| puts.clone())
            .map_err(|_| poisoned())
    }
}

impl BlobStore for FakeBlobs {
    fn put<'a>(
        &'a self,
        _scope: &'a StorageScope,
        mut body: ByteReader,
        limit: u64,
        expected: Option<Digest>,
    ) -> PortFuture<'a, ObjectInfo> {
        Box::pin(async move {
            let mut bytes = Vec::new();
            body.read_to_end(&mut bytes)
                .await
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
            let digest = expected.clone().ok_or_else(not_used)?;
            let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            self.puts.lock().map_err(|_| poisoned())?.push(Put {
                bytes,
                limit,
                expected,
            });
            Ok(ObjectInfo { digest, size })
        })
    }
    fn open<'a>(
        &'a self,
        _scope: &'a StorageScope,
        digest: &'a Digest,
        offset: u64,
        length: u64,
    ) -> PortFuture<'a, ObjectRead> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            match &self.open {
                Open::Serve(bytes) => Ok(ObjectRead {
                    object: ObjectInfo {
                        digest: digest.clone(),
                        size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                    },
                    offset,
                    length,
                    body: Box::pin(std::io::Cursor::new(bytes.clone())),
                }),
                Open::Fail => Err(ApiError::new(
                    ErrorCode::Internal,
                    "the blob store is unreadable",
                )),
                Open::Oversized(size) => Ok(ObjectRead {
                    object: ObjectInfo {
                        digest: digest.clone(),
                        size: *size,
                    },
                    offset,
                    length,
                    body: Box::pin(std::io::Cursor::new(Vec::new())),
                }),
            }
        })
    }
    fn materialize<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _digest: &'a Digest,
    ) -> PortFuture<'a, LocalSource> {
        unused()
    }
}

impl FakeRecords {
    /// A store holding `derived` and mapping every revision to `mapping`.
    #[must_use]
    pub const fn new(derived: Vec<DerivedObject>, mapping: Option<RevisionMapping>) -> Self {
        Self {
            derived,
            mapping,
            recorded: Mutex::new(Vec::new()),
        }
    }

    /// Everything `record_derived_object` stored, in order, without repeats.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn recorded(&self) -> Result<Vec<DerivedObject>, ApiError> {
        self.recorded
            .lock()
            .map(|recorded| recorded.clone())
            .map_err(|_| poisoned())
    }
}

impl RecordStore for FakeRecords {
    fn create_job<'a>(&'a self, _scope: &'a JobScope, _job: NewJob) -> PortFuture<'a, Job> {
        unused()
    }
    fn get_job<'a>(&'a self, _scope: &'a JobScope, _job: JobId) -> PortFuture<'a, Job> {
        unused()
    }
    fn list_jobs<'a>(
        &'a self,
        _scope: &'a JobScope,
        _page: Page,
    ) -> PortFuture<'a, ListJobsResponse> {
        unused()
    }
    fn claim_job(&self, _job: JobId) -> PortFuture<'_, Option<ClaimedJob>> {
        unused()
    }
    fn update_progress<'a>(&'a self, _lease: &'a JobLease, _progress: u8) -> PortFuture<'a, Job> {
        unused()
    }
    fn complete_job(&self, _completion: JobCompletion) -> PortFuture<'_, Job> {
        unused()
    }
    fn fail_job(
        &self,
        _lease: JobLease,
        _message: String,
        _retryable: bool,
    ) -> PortFuture<'_, Job> {
        unused()
    }
    fn cancel_job<'a>(
        &'a self,
        _scope: &'a JobScope,
        _mutation_id: MutationId,
        _job: JobId,
    ) -> PortFuture<'a, Job> {
        unused()
    }
    fn retry_job<'a>(
        &'a self,
        _scope: &'a JobScope,
        _mutation_id: MutationId,
        _job: JobId,
    ) -> PortFuture<'a, Job> {
        unused()
    }
    fn record_artifact<'a>(
        &'a self,
        _scope: &'a JobScope,
        _mutation_id: MutationId,
        _artifact: NewArtifact,
    ) -> PortFuture<'a, ArtifactRecord> {
        unused()
    }
    fn get_artifact<'a>(
        &'a self,
        _scope: &'a JobScope,
        _artifact: ArtifactId,
    ) -> PortFuture<'a, ArtifactRecord> {
        unused()
    }
    fn pending_jobs(&self) -> PortFuture<'_, Vec<(JobScope, JobId)>> {
        unused()
    }
    fn expire_leases(&self) -> PortFuture<'_, u32> {
        unused()
    }
    fn create_purge<'a>(
        &'a self,
        _tenant: &'a TenantId,
        _mutation_id: MutationId,
        _purge: NewPurge,
    ) -> PortFuture<'a, Purge> {
        unused()
    }
    fn get_purge<'a>(&'a self, _tenant: &'a TenantId, _purge: PurgeId) -> PortFuture<'a, Purge> {
        unused()
    }
    fn update_purge<'a>(&'a self, _tenant: &'a TenantId, _purge: Purge) -> PortFuture<'a, Purge> {
        unused()
    }
    fn revision_mapping<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _revision: &'a Revision,
    ) -> PortFuture<'a, Option<RevisionMapping>> {
        let mapping = self.mapping.clone();
        Box::pin(async move { Ok(mapping) })
    }
    fn record_derived_object<'a>(
        &'a self,
        _scope: &'a StorageScope,
        object: DerivedObject,
    ) -> PortFuture<'a, DerivedObject> {
        Box::pin(async move {
            let mut recorded = self.recorded.lock().map_err(|_| poisoned())?;
            if !recorded.contains(&object) {
                recorded.push(object.clone());
            }
            Ok(object)
        })
    }
    fn derived_object<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _item: ItemId,
        _revision: &'a Revision,
        digest: &'a Digest,
    ) -> PortFuture<'a, Option<DerivedObject>> {
        let found = self
            .derived
            .iter()
            .find(|object| &object.digest == digest)
            .cloned();
        Box::pin(async move { Ok(found) })
    }
    fn insert_review<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: MutationId,
        _review: Review,
    ) -> PortFuture<'a, Review> {
        unused()
    }
    fn list_reviews<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _item: ItemId,
    ) -> PortFuture<'a, Vec<Review>> {
        unused()
    }
    fn insert_receipt<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _mutation_id: Option<MutationId>,
        _receipt: Receipt,
    ) -> PortFuture<'a, Receipt> {
        unused()
    }
    fn get_receipt<'a>(
        &'a self,
        _scope: &'a StorageScope,
        _receipt: ReceiptId,
    ) -> PortFuture<'a, Receipt> {
        unused()
    }
}

/// The error of a method a test does not use.
fn not_used() -> ApiError {
    ApiError::new(ErrorCode::Internal, "not used by this test")
}

/// A future that answers `not_used`.
fn unused<'a, T: Send + 'a>() -> PortFuture<'a, T> {
    Box::pin(async { Err(not_used()) })
}

/// The error of a log lock another test thread poisoned.
fn poisoned() -> ApiError {
    ApiError::new(ErrorCode::Internal, "a fake's log is poisoned")
}
