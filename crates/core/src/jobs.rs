//! Durable application work records are independent of queue delivery and telemetry.
//!
//! `RecordStore` is the durable source of job truth. `JobQueue` delivers wake-ups.
//! `JobHandler` executes claimed work; ingest owns the runtime adapter implementation.
//! A job is created from a core-owned `JobSpec`, never from the wire `Job`: the specification
//! carries the inputs, the initiator and the `MutationId` a handler needs to do the work again
//! after a crash. Every insert is unique on `MutationId` and a repeated id returns the prior row.
//!
//! A job belongs to a workspace or, for an installation backup and a purge, to the tenant
//! (`JobScope`); a tenant job outlives a purged workspace. `JobSpec::fits` holds the two
//! together. Purge records and the revision map are tenant and workspace records the same store
//! keeps (Stage 1b design sections 6 and 9.2); derived objects (a View's materialized dataset)
//! are recorded against the item revision they were made from (section 4).

use okf_jawn_contract::{
    common::{PageRange, Warning},
    error::{ApiError, ErrorCode},
    events::Receipt,
    identity::{
        ArtifactId, Digest, ItemId, JobId, MutationId, PurgeId, ReceiptId, Revision, TenantId,
        Timestamp, UploadId, WorkspaceId, WorkspacePath,
    },
    import::{Job, JobKind, ListJobsResponse},
    purge::{Purge, PurgeTarget},
    review::Review,
    workspace::{ArtifactScope, DownloadArtifact, RestoreReport},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::AccessControl;
use crate::conversion::ConversionSettings;
use crate::ports::PortFuture;
use crate::storage::{ObjectInfo, Page, Provenance, StorageScope};
use crate::stored::{ValidatorCell, decode_stored};

/// What a retained artifact is; the contract's, so the record and the wire agree.
pub use okf_jawn_contract::workspace::ArtifactKind;

/// Whose job or artifact a record is: the tenant's (an installation backup, a purge) or one
/// workspace's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobScope {
    /// Installation-level work; the wire `Job` has no `workspace_id`.
    Tenant(TenantId),
    /// Work within one workspace.
    Workspace(StorageScope),
}

/// What a job must do: the inputs of the request that started it, with selectors resolved.
///
/// The record store keeps the specification as written (`JobSpec::to_stored`) and hands it
/// back on every claim, decoded with `JobSpec::from_stored` so a value that does not meet the
/// schema is a store fault rather than a silently accepted job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JobSpec {
    /// Convert finalized uploads into source cards.
    Import {
        /// Revision the import is based on.
        base_revision: Revision,
        /// Completed upload slots to import.
        upload_ids: Vec<UploadId>,
        /// Destination folder; `None` is the workspace root.
        destination: Option<WorkspacePath>,
        /// Apply the workspace naming rules to the new cards.
        apply_naming_rules: bool,
        /// Conversion settings, resolved when the request was accepted.
        settings: ConversionSettings,
    },
    /// Convert one source again with explicit settings, keeping human corrections.
    Redigest {
        /// Source item to convert again.
        item_id: ItemId,
        /// Revision the new digest is based on.
        base_revision: Revision,
        /// Settings that identify the new digest.
        settings: ConversionSettings,
        /// The `not_converted` pages at `base_revision` when only those are converted again;
        /// `None` converts the whole document.
        pages: Option<Vec<PageRange>>,
    },
    /// Build a portable export of the workspace.
    ExportWorkspace {
        /// Resolved revision to export.
        revision: Revision,
        /// Include retained history in addition to the selected state.
        include_history: bool,
    },
    /// Back up content, retained objects and application records.
    BackupWorkspace,
    /// Restore content and application records from an uploaded workspace archive.
    ///
    /// Built with `restore_job_spec`.
    RestoreWorkspace {
        /// Upload slot holding the archive; consumed by this job.
        upload_id: UploadId,
        /// Digest of the archive, checked against the request's `sha256` at acceptance.
        archive: Digest,
        /// The subjects with `write` on the target workspace, read from `AccessControl` when
        /// the request was accepted, sorted and without repeats. The handler holds only its
        /// `ClaimedJob` and never consults `AccessControl`: it passes these to
        /// `Backups::restore_import`, which gives an archived draft back only to an editor
        /// among them.
        editors: Vec<String>,
    },
    /// Rebuild the derived search and link index at the current head.
    RebuildIndex,
    /// Export one View with its specification, sources and data table.
    ExportView {
        /// View item to export.
        item_id: ItemId,
        /// Resolved revision to export.
        revision: Revision,
    },
    /// Back up the installation's identities, grants, connector records without secrets,
    /// purge records and tenant events; no workspace content. Tenant-scoped.
    BackupInstallation,
    /// Remove a workspace and everything only it references. Tenant-scoped.
    PurgeWorkspace {
        /// The purge record this job carries out.
        purge_id: PurgeId,
        /// Workspace removed.
        workspace_id: WorkspaceId,
    },
    /// Remove one item's bytes, derivatives, index entries and history. Tenant-scoped.
    PurgeItem {
        /// The purge record this job carries out.
        purge_id: PurgeId,
        /// Its workspace.
        workspace_id: WorkspaceId,
        /// Item removed.
        item_id: ItemId,
    },
}

/// A job to register durably before it is enqueued or reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    /// Durable write identity of the request that starts the job; every commit and row the
    /// handler creates is written under it.
    pub mutation_id: MutationId,
    /// Who the job acts for.
    pub initiator: Provenance,
    /// What the job must do.
    pub spec: JobSpec,
}

/// Lease identity prevents a late worker from completing a newer attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobLease {
    /// Stable application work id.
    pub job_id: JobId,
    /// Unique claim token changed on every claim.
    pub token: String,
    /// Monotonic attempt number.
    pub attempt: u32,
    /// When the claim lapses unless `RecordStore::update_progress` renews it.
    pub expires_at: Timestamp,
}

/// A claimed job: everything `JobHandler::handle` needs, with nothing to look up elsewhere.
#[derive(Debug, Clone)]
pub struct ClaimedJob {
    /// Tenant, or tenant and workspace, the work belongs to.
    pub scope: JobScope,
    /// Compare-and-set claim identity.
    pub lease: JobLease,
    /// Durable job record as it stands at the claim.
    pub job: Job,
    /// Durable write identity the job was created under; the same on every attempt.
    pub mutation_id: MutationId,
    /// Who the job acts for.
    pub initiator: Provenance,
    /// What the job must do.
    pub spec: JobSpec,
}

/// A durable completion result, never inferred solely from a queue acknowledgment.
#[derive(Debug, Clone)]
pub struct JobCompletion {
    /// Compare-and-set claim identity.
    pub lease: JobLease,
    /// Committed content revision, when the job changed the workspace.
    pub revision: Option<Revision>,
    /// Items the job produced.
    pub item_ids: Vec<ItemId>,
    /// Artifact the job produced, already recorded with `RecordStore::record_artifact`.
    pub artifact: Option<ArtifactId>,
    /// What a restore did; present exactly for a `RestoreWorkspace` job.
    pub restore: Option<RestoreReport>,
    /// Retained output objects, kept as garbage-collection roots.
    pub outputs: Vec<Digest>,
    /// Non-fatal issues recorded during the work.
    pub warnings: Vec<Warning>,
}

/// An artifact a job produced, recorded once its bytes are retained in the blob store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArtifact {
    /// What the artifact is.
    pub kind: ArtifactKind,
    /// Identity and size of the retained bytes.
    pub object: ObjectInfo,
    /// Media type the download transport sends.
    pub media_type: String,
    /// Job that produced the artifact.
    pub created_by_job: JobId,
}

/// The non-rebuildable link from an artifact identity to retained bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRecord {
    /// Artifact identity, allocated by the store.
    pub id: ArtifactId,
    /// Whose artifact it is: the tenant's for an installation backup, a workspace's otherwise.
    pub scope: JobScope,
    /// What the artifact is.
    pub kind: ArtifactKind,
    /// Identity and size of the retained bytes; open them with `BlobStore::open`.
    pub object: ObjectInfo,
    /// Media type the download transport sends.
    pub media_type: String,
    /// Job that produced the artifact.
    pub created_by_job: JobId,
}

/// A purge to record; it holds no content of the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPurge {
    /// Identity, from `derive_purge_id` of the request's mutation id, so the tenant job's
    /// specification can name it.
    pub id: PurgeId,
    /// What is removed.
    pub target: PurgeTarget,
    /// The tenant administrator who asked.
    pub initiator: Provenance,
}

/// What became of a revision a purge rewrote or removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionMapping {
    /// The purge that rewrote or removed it.
    pub purge_id: PurgeId,
    /// The rewritten revision; `None` when the revision no longer exists.
    pub replacement: Option<Revision>,
}

/// What a derived object is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerivedKind {
    /// A View binding's materialized `Dataset`.
    Dataset,
}

/// An object the application derived from one item revision and retained; a
/// garbage-collection root, and an object `get_object` serves for that revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedObject {
    /// Item it was derived from.
    pub item_id: ItemId,
    /// Revision it was derived from.
    pub revision: Revision,
    /// Digest of the retained bytes.
    pub digest: Digest,
    /// What it is.
    pub kind: DerivedKind,
    /// Media type the object is served with.
    pub media_type: String,
}

/// SQLite-backed non-rebuildable application records.
pub trait RecordStore: Send + Sync {
    /// Register a queued job and allocate its identity.
    ///
    /// Unique on `job.mutation_id`: a repeated id inserts nothing and returns the prior job,
    /// whatever state it has reached. The returned `Job::kind` is `job.spec.kind()`, and a
    /// tenant job's `Job::workspace_id` is absent. Refuses with `Internal` a specification that
    /// does not fit the scope (`JobSpec::fits`).
    fn create_job<'a>(&'a self, scope: &'a JobScope, job: NewJob) -> PortFuture<'a, Job>;
    /// Read one durable job within its scope.
    fn get_job<'a>(&'a self, scope: &'a JobScope, job: JobId) -> PortFuture<'a, Job>;
    /// List durable jobs of one scope, newest first, with bounded pagination.
    fn list_jobs<'a>(&'a self, scope: &'a JobScope, page: Page)
    -> PortFuture<'a, ListJobsResponse>;
    /// Claim runnable work under a compare-and-set lease; `None` when it is not runnable.
    fn claim_job(&self, job: JobId) -> PortFuture<'_, Option<ClaimedJob>>;
    /// Record progress, 0 to 100, for the current claim, renew the lease to now plus the lease
    /// duration, and return the job as it now stands.
    ///
    /// It is the heartbeat: a handler converting a long window calls it at least every third
    /// of the lease. A handler reads the returned state: a cancelled job stops working.
    fn update_progress<'a>(&'a self, lease: &'a JobLease, progress: u8) -> PortFuture<'a, Job>;
    /// Commit completion only for the current unexpired claim.
    ///
    /// When `completion.artifact` is set, the returned and stored job shows
    /// `ArtifactRecord::download` of that record as its artifact.
    fn complete_job(&self, completion: JobCompletion) -> PortFuture<'_, Job>;
    /// Keep the failure and retry eligibility for the current claim.
    fn fail_job(&self, lease: JobLease, message: String, retryable: bool) -> PortFuture<'_, Job>;
    /// Cancel pending or running work while retaining recorded state.
    ///
    /// Unique on `mutation_id`: a repeated id makes no second transition and returns the job
    /// row as it stands.
    fn cancel_job<'a>(
        &'a self,
        scope: &'a JobScope,
        mutation_id: MutationId,
        job: JobId,
    ) -> PortFuture<'a, Job>;
    /// Queue a failed job again under the same durable identity.
    ///
    /// Unique on `mutation_id`: a repeated id does not queue the job a second time, even when
    /// the job has run and failed again since, and returns the job row as it stands.
    fn retry_job<'a>(
        &'a self,
        scope: &'a JobScope,
        mutation_id: MutationId,
        job: JobId,
    ) -> PortFuture<'a, Job>;
    /// Record the artifact a job produced and allocate its identity.
    ///
    /// The bytes are already retained in `BlobStore` under `artifact.object.digest`. Unique on
    /// `mutation_id`, the producing job's write identity: a repeated id records nothing and
    /// returns the prior record. Refuses with `Internal` a kind whose scope is not `scope`'s.
    fn record_artifact<'a>(
        &'a self,
        scope: &'a JobScope,
        mutation_id: MutationId,
        artifact: NewArtifact,
    ) -> PortFuture<'a, ArtifactRecord>;
    /// Read one artifact record within its scope; `NotFound` when the scope has no artifact
    /// with that identity.
    fn get_artifact<'a>(
        &'a self,
        scope: &'a JobScope,
        artifact: ArtifactId,
    ) -> PortFuture<'a, ArtifactRecord>;
    /// Enumerate unfinished records across tenants for queue reconciliation on restart.
    fn pending_jobs(&self) -> PortFuture<'_, Vec<(JobScope, JobId)>>;
    /// Release claims whose leases have expired so work can be reclaimed.
    fn expire_leases(&self) -> PortFuture<'_, u32>;
    /// Record a purge and, in the same transaction, its tenant job (`PurgeWorkspace` or
    /// `PurgeItem` from the target) under `mutation_id`.
    ///
    /// Unique on `purge.id`: a repeated id returns the prior purge. While a purge of the same
    /// target is not completed, any request for that target returns that unfinished purge and
    /// records nothing: that is how a failed purge is resumed.
    fn create_purge<'a>(
        &'a self,
        tenant: &'a TenantId,
        mutation_id: MutationId,
        purge: NewPurge,
    ) -> PortFuture<'a, Purge>;
    /// Read one purge; it outlives the workspace it removed.
    fn get_purge<'a>(&'a self, tenant: &'a TenantId, purge: PurgeId) -> PortFuture<'a, Purge>;
    /// Store the progress, report or failure of a purge and return it as stored.
    fn update_purge<'a>(&'a self, tenant: &'a TenantId, purge: Purge) -> PortFuture<'a, Purge>;
    /// What became of a revision a purge rewrote or removed; `None` when no purge touched it.
    /// Written by `Purger`.
    fn revision_mapping<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Option<RevisionMapping>>;
    /// Record an object derived from one item revision.
    ///
    /// Unique on (item, revision, digest): a repeated record inserts nothing and returns the
    /// prior one. Recorded objects are garbage-collection roots.
    fn record_derived_object<'a>(
        &'a self,
        scope: &'a StorageScope,
        object: DerivedObject,
    ) -> PortFuture<'a, DerivedObject>;
    /// The object recorded for this (item, revision, digest), if any.
    fn derived_object<'a>(
        &'a self,
        scope: &'a StorageScope,
        item: ItemId,
        revision: &'a Revision,
        digest: &'a Digest,
    ) -> PortFuture<'a, Option<DerivedObject>>;
    /// Record exact reviewed content after application-level confirmation.
    ///
    /// Unique on `mutation_id`: a repeated id inserts nothing and returns the prior review.
    fn insert_review<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        review: Review,
    ) -> PortFuture<'a, Review>;
    /// Every review recorded for an item, oldest first, with coverage as it was recorded.
    ///
    /// The application recomputes coverage against the revision it resolved.
    fn list_reviews<'a>(
        &'a self,
        scope: &'a StorageScope,
        item: ItemId,
    ) -> PortFuture<'a, Vec<Review>>;
    /// Persist what was returned, not merely a trace identifier.
    ///
    /// A write passes its `MutationId`: the receipt is then unique on it and a repeated id
    /// returns the prior receipt. A read passes `None` and always inserts.
    fn insert_receipt<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: Option<MutationId>,
        receipt: Receipt,
    ) -> PortFuture<'a, Receipt>;
    /// Read one durable receipt within workspace scope.
    fn get_receipt<'a>(
        &'a self,
        scope: &'a StorageScope,
        receipt: ReceiptId,
    ) -> PortFuture<'a, Receipt>;
}

/// Delivery adapter; job truth remains in `RecordStore`.
pub trait JobQueue: Send + Sync {
    /// Deliver an existing durable job identity, accepting possible duplicate delivery.
    fn enqueue(&self, scope: JobScope, job: JobId) -> PortFuture<'_, ()>;
}

/// Executes claimed durable work; ingest owns the Tokio + `RecordStore` runtime adapter.
pub trait JobHandler: Send + Sync {
    /// Run one claimed attempt; completion and failure are written through `RecordStore`.
    fn handle<'a>(&'a self, claimed: &'a ClaimedJob) -> PortFuture<'a, ()>;
}

impl JobScope {
    /// The tenant the record belongs to.
    #[must_use]
    pub const fn tenant(&self) -> &TenantId {
        match self {
            Self::Tenant(tenant) => tenant,
            Self::Workspace(scope) => &scope.tenant_id,
        }
    }

    /// The workspace, for a workspace record.
    #[must_use]
    pub const fn workspace(&self) -> Option<&WorkspaceId> {
        match self {
            Self::Tenant(_) => None,
            Self::Workspace(scope) => Some(&scope.workspace_id),
        }
    }

    /// Which artifact scope this is, so a kind's scope can be compared with it.
    #[must_use]
    pub const fn artifact_scope(&self) -> ArtifactScope {
        match self {
            Self::Tenant(_) => ArtifactScope::Tenant,
            Self::Workspace(_) => ArtifactScope::Workspace,
        }
    }
}

impl ArtifactRecord {
    /// The wire form shown on a job, with the path of the transport that serves its scope:
    /// `download_artifact` for a workspace artifact, `download_tenant_artifact` for a tenant one.
    #[must_use]
    pub fn download(&self) -> DownloadArtifact {
        DownloadArtifact {
            artifact_id: self.id,
            kind: self.kind,
            sha256: self.object.digest.clone(),
            size: self.object.size.to_string(),
            download_path: artifact_download_path(&self.scope, self.id),
        }
    }
}

impl JobSpec {
    /// The wire kind shown on the job.
    #[must_use]
    pub const fn kind(&self) -> JobKind {
        match self {
            Self::Import { .. } => JobKind::Import,
            Self::Redigest { .. } => JobKind::Redigest,
            Self::ExportWorkspace { .. } => JobKind::ExportWorkspace,
            Self::BackupWorkspace => JobKind::BackupWorkspace,
            Self::RestoreWorkspace { .. } => JobKind::RestoreWorkspace,
            Self::RebuildIndex => JobKind::RebuildIndex,
            Self::ExportView { .. } => JobKind::ExportView,
            Self::BackupInstallation => JobKind::BackupInstallation,
            Self::PurgeWorkspace { .. } => JobKind::PurgeWorkspace,
            Self::PurgeItem { .. } => JobKind::PurgeItem,
        }
    }

    /// The JSON the record store keeps.
    ///
    /// # Errors
    /// Returns `Internal` if the specification cannot be represented as JSON.
    pub fn to_stored(&self) -> Result<serde_json::Value, ApiError> {
        serde_json::to_value(self).map_err(|error| {
            ApiError::new(
                ErrorCode::Internal,
                format!("a job specification did not serialize: {error}"),
            )
        })
    }

    /// Decode a specification the record store kept, validated against its schema: a field
    /// beside a unit variant's tag, which serde alone accepts, is refused.
    ///
    /// # Errors
    /// Returns `Internal` naming the first violation when the stored value is not a job
    /// specification.
    pub fn from_stored(value: serde_json::Value) -> Result<Self, ApiError> {
        static SCHEMA: ValidatorCell = ValidatorCell::new();
        decode_stored(value, "a stored job specification", &SCHEMA)
    }

    /// Whether the specification belongs in `scope`: a tenant kind in the tenant scope, every
    /// other kind in a workspace scope.
    #[must_use]
    pub const fn fits(&self, scope: &JobScope) -> bool {
        self.kind().is_tenant() == matches!(scope, JobScope::Tenant(_))
    }
}

/// The tenant job specification that carries out a purge of `target`.
#[must_use]
pub fn purge_job_spec(target: &PurgeTarget, purge_id: PurgeId) -> JobSpec {
    match target {
        PurgeTarget::Workspace { workspace_id } => JobSpec::PurgeWorkspace {
            purge_id,
            workspace_id: *workspace_id,
        },
        PurgeTarget::Item {
            workspace_id,
            item_id,
        } => JobSpec::PurgeItem {
            purge_id,
            workspace_id: *workspace_id,
            item_id: *item_id,
        },
    }
}

/// The specification of a `restore_workspace` job accepted for the workspace in `scope`.
///
/// The editors are read from `access` now, when the request is accepted, because the handler
/// that later runs the job holds only its `ClaimedJob`; they are sorted and repeats removed, so
/// the specification does not depend on the adapter's order.
///
/// # Errors
/// Returns any error of `AccessControl::editors`.
pub async fn restore_job_spec(
    access: &dyn AccessControl,
    scope: &StorageScope,
    upload_id: UploadId,
    archive: Digest,
) -> Result<JobSpec, ApiError> {
    let mut editors = access.editors(scope).await?;
    editors.sort();
    editors.dedup();
    Ok(JobSpec::RestoreWorkspace {
        upload_id,
        archive,
        editors,
    })
}

/// The application-relative path of the transport that serves one artifact: `download_artifact`
/// (`/api/workspaces/{workspace_id}/artifacts/{artifact_id}`) for a workspace artifact,
/// `download_tenant_artifact` (`/api/artifacts/{artifact_id}`) for a tenant one.
///
/// Defined once so the record store, the application and the server route agree.
#[must_use]
pub fn artifact_download_path(scope: &JobScope, artifact: ArtifactId) -> String {
    match scope {
        JobScope::Workspace(scope) => {
            format!(
                "/api/workspaces/{}/artifacts/{}",
                scope.workspace_id.0, artifact.0
            )
        }
        JobScope::Tenant(_) => format!("/api/artifacts/{}", artifact.0),
    }
}
