//! A `RecordStore` for the review handlers: the fixture's `FakeRecords` for every method, with
//! reviews kept as `RecordStore::insert_review` and `list_reviews` document them. Not a
//! production adapter.
//!
//! Every other method is the fixture's, so a test still scripts revision maps and derived
//! objects there (`world.records`) and reads the receipts there. Where the review rows are kinder
//! than storage's: nothing is refused while a purge names the workspace, and a purge does not
//! scrub them (a test scripts an invalidated row with `put_review`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::Receipt;
use okf_jawn_contract::identity::{
    ArtifactId, Digest, ItemId, JobId, MutationId, PurgeId, ReceiptId, Revision, TenantId,
};
use okf_jawn_contract::import::{Job, ListJobsResponse};
use okf_jawn_contract::purge::Purge;
use okf_jawn_contract::review::Review;
use okf_jawn_core::jobs::{
    ArtifactRecord, ClaimedJob, DerivedObject, JobCompletion, JobLease, JobScope, NewArtifact,
    NewJob, NewPurge, RecordStore, RevisionMapping,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{Page, StorageScope};

use crate::fixture::{Built, FakeRecords};

/// `RecordStore` over the fixture's records, with review rows of its own.
pub struct ReviewRecords {
    inner: Arc<FakeRecords>,
    reviews: Mutex<Vec<(StorageScope, MutationId, Review)>>,
    fail_next_insert: AtomicBool,
    lists: Mutex<usize>,
}

impl ReviewRecords {
    /// Reviews over `inner`'s other records.
    #[must_use]
    pub fn new(inner: Arc<FakeRecords>) -> Self {
        Self {
            inner,
            reviews: Mutex::new(Vec::new()),
            fail_next_insert: AtomicBool::new(false),
            lists: Mutex::new(0),
        }
    }

    /// Hold `review` in `scope` as recorded by `mutation`, as it was stored (its coverage
    /// included), as a row recorded earlier or scrubbed by a purge would be.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn put_review(
        &self,
        scope: &StorageScope,
        mutation: MutationId,
        review: Review,
    ) -> Built<()> {
        lock(&self.reviews)?.push((scope.clone(), mutation, review));
        Ok(())
    }

    /// Every review held, oldest first, with the mutation that recorded it.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn reviews(&self) -> Built<Vec<(MutationId, Review)>> {
        Ok(lock(&self.reviews)?
            .iter()
            .map(|(_, mutation, review)| (*mutation, review.clone()))
            .collect())
    }

    /// Make the next `insert_review` fail with `Unavailable`, as a store fault would.
    pub fn fail_next_insert(&self) {
        self.fail_next_insert.store(true, Ordering::SeqCst);
    }

    /// How many times `list_reviews` was called.
    ///
    /// # Errors
    /// Returns when the lock is poisoned.
    pub fn lists(&self) -> Built<usize> {
        Ok(*lock(&self.lists)?)
    }
}

impl RecordStore for ReviewRecords {
    fn create_job<'a>(&'a self, scope: &'a JobScope, job: NewJob) -> PortFuture<'a, Job> {
        self.inner.create_job(scope, job)
    }
    fn get_job<'a>(&'a self, scope: &'a JobScope, job: JobId) -> PortFuture<'a, Job> {
        self.inner.get_job(scope, job)
    }
    fn list_jobs<'a>(
        &'a self,
        scope: &'a JobScope,
        page: Page,
    ) -> PortFuture<'a, ListJobsResponse> {
        self.inner.list_jobs(scope, page)
    }
    fn claim_job(&self, job: JobId) -> PortFuture<'_, Option<ClaimedJob>> {
        self.inner.claim_job(job)
    }
    fn update_progress<'a>(&'a self, lease: &'a JobLease, progress: u8) -> PortFuture<'a, Job> {
        self.inner.update_progress(lease, progress)
    }
    fn complete_job(&self, completion: JobCompletion) -> PortFuture<'_, Job> {
        self.inner.complete_job(completion)
    }
    fn fail_job(&self, lease: JobLease, error: ApiError, retryable: bool) -> PortFuture<'_, Job> {
        self.inner.fail_job(lease, error, retryable)
    }
    fn cancel_job<'a>(
        &'a self,
        scope: &'a JobScope,
        mutation_id: MutationId,
        job: JobId,
    ) -> PortFuture<'a, Job> {
        self.inner.cancel_job(scope, mutation_id, job)
    }
    fn retry_job<'a>(
        &'a self,
        scope: &'a JobScope,
        mutation_id: MutationId,
        job: JobId,
    ) -> PortFuture<'a, Job> {
        self.inner.retry_job(scope, mutation_id, job)
    }
    fn record_artifact<'a>(
        &'a self,
        scope: &'a JobScope,
        mutation_id: MutationId,
        artifact: NewArtifact,
    ) -> PortFuture<'a, ArtifactRecord> {
        self.inner.record_artifact(scope, mutation_id, artifact)
    }
    fn get_artifact<'a>(
        &'a self,
        scope: &'a JobScope,
        artifact: ArtifactId,
    ) -> PortFuture<'a, ArtifactRecord> {
        self.inner.get_artifact(scope, artifact)
    }
    fn pending_jobs(&self) -> PortFuture<'_, Vec<(JobScope, JobId)>> {
        self.inner.pending_jobs()
    }
    fn expire_leases(&self) -> PortFuture<'_, u32> {
        self.inner.expire_leases()
    }
    fn create_purge<'a>(
        &'a self,
        tenant: &'a TenantId,
        mutation_id: MutationId,
        purge: NewPurge,
    ) -> PortFuture<'a, Purge> {
        self.inner.create_purge(tenant, mutation_id, purge)
    }
    fn get_purge<'a>(&'a self, tenant: &'a TenantId, purge: PurgeId) -> PortFuture<'a, Purge> {
        self.inner.get_purge(tenant, purge)
    }
    fn update_purge<'a>(&'a self, tenant: &'a TenantId, purge: Purge) -> PortFuture<'a, Purge> {
        self.inner.update_purge(tenant, purge)
    }
    fn revision_mapping<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Option<RevisionMapping>> {
        self.inner.revision_mapping(scope, revision)
    }
    fn record_derived_object<'a>(
        &'a self,
        scope: &'a StorageScope,
        object: DerivedObject,
    ) -> PortFuture<'a, DerivedObject> {
        self.inner.record_derived_object(scope, object)
    }
    fn derived_object<'a>(
        &'a self,
        scope: &'a StorageScope,
        item: ItemId,
        revision: &'a Revision,
        digest: &'a Digest,
    ) -> PortFuture<'a, Option<DerivedObject>> {
        self.inner.derived_object(scope, item, revision, digest)
    }
    // "Record exact reviewed content after application-level confirmation. Unique on
    // `mutation_id`: a repeated id inserts nothing and returns the prior review."
    fn insert_review<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        review: Review,
    ) -> PortFuture<'a, Review> {
        Box::pin(async move {
            if self.fail_next_insert.swap(false, Ordering::SeqCst) {
                return Err(ApiError::new(ErrorCode::Unavailable, "the store failed"));
            }
            let mut reviews = lock_api(&self.reviews)?;
            if let Some((_, _, prior)) = reviews.iter().find(|(_, held, _)| *held == mutation_id) {
                return Ok(prior.clone());
            }
            reviews.push((scope.clone(), mutation_id, review.clone()));
            Ok(review)
        })
    }
    // "Every review recorded for an item, oldest first, with coverage as it was recorded. The
    // application recomputes coverage against the revision it resolved."
    fn list_reviews<'a>(
        &'a self,
        scope: &'a StorageScope,
        item: ItemId,
    ) -> PortFuture<'a, Vec<Review>> {
        Box::pin(async move {
            {
                let mut lists = lock_api(&self.lists)?;
                *lists = lists.saturating_add(1);
            }
            Ok(lock_api(&self.reviews)?
                .iter()
                .filter(|(held, _, review)| held == scope && review.source.item_id == item)
                .map(|(_, _, review)| review.clone())
                .collect())
        })
    }
    fn insert_receipt<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: Option<MutationId>,
        receipt: Receipt,
    ) -> PortFuture<'a, Receipt> {
        self.inner.insert_receipt(scope, mutation_id, receipt)
    }
    fn get_receipt<'a>(
        &'a self,
        scope: &'a StorageScope,
        receipt: ReceiptId,
    ) -> PortFuture<'a, Receipt> {
        self.inner.get_receipt(scope, receipt)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Built<MutexGuard<'_, T>> {
    mutex.lock().map_err(|_| "a fake's lock is poisoned".into())
}

fn lock_api<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, ApiError> {
    mutex
        .lock()
        .map_err(|_| ApiError::new(ErrorCode::Internal, "a fake's lock is poisoned"))
}
