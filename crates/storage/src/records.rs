//! Durable application records (`RecordStore`) over SQLite: jobs, artifacts, purges, the
//! revision map, derived objects, reviews and receipts.
//!
//! Every insert that takes a `MutationId` is `INSERT ... ON CONFLICT (mutation_id) DO NOTHING`
//! followed by a read of the row that holds that id, so the unique column, not a lookup, makes
//! a repeated id a no-op that returns the prior row. A job's wire `Job` is kept as JSON with its
//! state, scope and lease in columns; the specification and initiator are kept exactly as
//! written and handed back unchanged on every claim. The job lease is `JOB_LEASE_SECONDS`.

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::Receipt;
use okf_jawn_contract::identity::{
    ArtifactId, Digest, ItemId, JobId, MutationId, PurgeId, ReceiptId, Revision, TenantId,
};
use okf_jawn_contract::import::{Job, JobState, ListJobsResponse};
use okf_jawn_contract::purge::{Purge, PurgeState, PurgeTarget};
use okf_jawn_contract::review::Review;
use okf_jawn_core::jobs::{
    ArtifactRecord, ClaimedJob, DerivedKind, DerivedObject, JobCompletion, JobLease, JobScope,
    JobSpec, NewArtifact, NewJob, NewPurge, RecordStore, RevisionMapping, purge_job_spec,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::{ObjectInfo, Page, Provenance, StorageScope};
use rusqlite::{OptionalExtension, Transaction, params};

use crate::db::{
    Db, conflict, cursor, internal, job_scope_key, json, not_found, page_limit, paged, scope_key,
    sql, to_i64, to_u64,
};
use crate::seam;

/// `RecordStore` over the records database.
#[derive(Debug, Clone)]
pub struct SqliteRecords {
    db: Db,
}

/// tenant, workspace, record, mutation, initiator, spec, lease expiry: what a claim reads.
type ClaimColumns = (
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    Option<i64>,
);

/// id, tenant, workspace, kind, digest, size, media type, job: one artifact row.
type ArtifactColumns = (
    String,
    String,
    Option<String>,
    String,
    String,
    i64,
    String,
    String,
);

/// The columns of a job row a lease operation needs.
struct LeasedRow {
    tenant_id: String,
    workspace_id: Option<String>,
    record: String,
    state: String,
    token: Option<String>,
    expires_ms: Option<i64>,
}

/// How long a job claim lasts unless `update_progress` renews it.
pub const JOB_LEASE_SECONDS: i64 = 300;

impl SqliteRecords {
    pub(crate) const fn new(db: Db) -> Self {
        Self { db }
    }
}

impl RecordStore for SqliteRecords {
    fn create_job<'a>(&'a self, scope: &'a JobScope, job: NewJob) -> PortFuture<'a, Job> {
        let scope = scope.clone();
        Box::pin(
            self.db
                .transaction(move |transaction| insert_job(transaction, &scope, &job)),
        )
    }

    fn get_job<'a>(&'a self, scope: &'a JobScope, job: JobId) -> PortFuture<'a, Job> {
        let (tenant, workspace) = job_scope_key(scope);
        Box::pin(self.db.call(move |connection| {
            let record: Option<String> = connection
                .query_row(
                    "SELECT record FROM jobs
                     WHERE job_id = ?1 AND tenant_id = ?2 AND workspace_id IS ?3",
                    params![job.0.to_string(), tenant, workspace],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| sql(&error))?;
            decode_job(&record.ok_or_else(|| not_found("job"))?)
        }))
    }

    fn list_jobs<'a>(
        &'a self,
        scope: &'a JobScope,
        page: Page,
    ) -> PortFuture<'a, ListJobsResponse> {
        let (tenant, workspace) = job_scope_key(scope);
        Box::pin(self.db.call(move |connection| {
            let after = cursor(page.cursor.as_deref())?;
            let limit = page_limit(page.limit);
            let mut statement = connection
                .prepare(
                    "SELECT rowid, record FROM jobs
                     WHERE tenant_id = ?1 AND workspace_id IS ?2 AND (?3 IS NULL OR rowid < ?3)
                     ORDER BY rowid DESC LIMIT ?4",
                )
                .map_err(|error| sql(&error))?;
            let rows = statement
                .query_map(
                    params![tenant, workspace, after, limit.saturating_add(1)],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(|error| sql(&error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| sql(&error))?;
            let (records, next_cursor) = paged(rows, limit);
            let items = records
                .iter()
                .map(|record| decode_job(record))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ListJobsResponse { items, next_cursor })
        }))
    }

    fn claim_job(&self, job: JobId) -> PortFuture<'_, Option<ClaimedJob>> {
        Box::pin(
            self.db
                .transaction(move |transaction| claim(transaction, job)),
        )
    }

    fn update_progress<'a>(&'a self, lease: &'a JobLease, progress: u8) -> PortFuture<'a, Job> {
        let lease = lease.clone();
        Box::pin(self.db.transaction(move |transaction| {
            let row = leased_row(transaction, lease.job_id)?;
            if row.token.as_deref() != Some(lease.token.as_str()) {
                return Err(lost_claim());
            }
            let mut job = decode_job(&row.record)?;
            if job.state == JobState::Cancelled {
                return Ok(job);
            }
            if job.state != JobState::Running {
                return Err(lost_claim());
            }
            job.progress = progress.min(100);
            let expires_ms = lease_expiry_ms()?;
            let expires_at = seam::later(JOB_LEASE_SECONDS)?;
            write_job(
                transaction,
                &job,
                params![expires_ms, expires_at.as_str()],
                "lease_expires_ms = ?3, lease_expires_at = ?4",
            )?;
            Ok(job)
        }))
    }

    fn complete_job(&self, completion: JobCompletion) -> PortFuture<'_, Job> {
        Box::pin(
            self.db
                .transaction(move |transaction| complete(transaction, &completion)),
        )
    }

    fn fail_job(&self, lease: JobLease, message: String, retryable: bool) -> PortFuture<'_, Job> {
        Box::pin(self.db.transaction(move |transaction| {
            let row = leased_row(transaction, lease.job_id)?;
            if row.token.as_deref() != Some(lease.token.as_str()) || row.state != "running" {
                return Err(lost_claim());
            }
            let mut job = decode_job(&row.record)?;
            job.state = JobState::Failed;
            let code = if retryable {
                ErrorCode::Unavailable
            } else {
                ErrorCode::Internal
            };
            job.error = Some(ApiError::new(code, message));
            write_job(
                transaction,
                &job,
                params![i64::from(retryable)],
                "retryable = ?3, lease_expires_ms = NULL",
            )?;
            Ok(job)
        }))
    }

    fn cancel_job<'a>(
        &'a self,
        scope: &'a JobScope,
        mutation_id: MutationId,
        job: JobId,
    ) -> PortFuture<'a, Job> {
        let scope = scope.clone();
        Box::pin(self.db.transaction(move |transaction| {
            control(transaction, &scope, mutation_id, job, "cancel")
        }))
    }

    fn retry_job<'a>(
        &'a self,
        scope: &'a JobScope,
        mutation_id: MutationId,
        job: JobId,
    ) -> PortFuture<'a, Job> {
        let scope = scope.clone();
        Box::pin(self.db.transaction(move |transaction| {
            control(transaction, &scope, mutation_id, job, "retry")
        }))
    }

    fn record_artifact<'a>(
        &'a self,
        scope: &'a JobScope,
        mutation_id: MutationId,
        artifact: NewArtifact,
    ) -> PortFuture<'a, ArtifactRecord> {
        let scope = scope.clone();
        Box::pin(self.db.transaction(move |transaction| {
            if artifact.kind.scope() != scope.artifact_scope() {
                return Err(internal(
                    "an artifact of this kind does not belong in this scope",
                ));
            }
            let (tenant, workspace) = job_scope_key(&scope);
            let artifact_id: ArtifactId = new_id!()?;
            transaction
                .execute(
                    "INSERT INTO artifacts (artifact_id, tenant_id, workspace_id, mutation_id,
                         kind, digest, size, media_type, job_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT (mutation_id) DO NOTHING",
                    params![
                        artifact_id.0.to_string(),
                        tenant,
                        workspace,
                        mutation_id.0.to_string(),
                        variant_text!(&artifact.kind)?,
                        artifact.object.digest.as_str(),
                        to_i64(artifact.object.size)?,
                        artifact.media_type,
                        artifact.created_by_job.0.to_string()
                    ],
                )
                .map_err(|error| sql(&error))?;
            read_artifact(transaction, "mutation_id", &mutation_id.0.to_string())?
                .ok_or_else(|| internal("an artifact row vanished after its insert"))
        }))
    }

    fn get_artifact<'a>(
        &'a self,
        scope: &'a JobScope,
        artifact: ArtifactId,
    ) -> PortFuture<'a, ArtifactRecord> {
        let scope = scope.clone();
        Box::pin(self.db.call(move |connection| {
            let transaction = connection.transaction().map_err(|error| sql(&error))?;
            let record = read_artifact(&transaction, "artifact_id", &artifact.0.to_string())?
                .filter(|record| record.scope == scope)
                .ok_or_else(|| not_found("artifact"))?;
            Ok(record)
        }))
    }

    fn pending_jobs(&self) -> PortFuture<'_, Vec<(JobScope, JobId)>> {
        Box::pin(self.db.call(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT tenant_id, workspace_id, job_id FROM jobs
                     WHERE state IN ('queued', 'running') ORDER BY rowid",
                )
                .map_err(|error| sql(&error))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|error| sql(&error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| sql(&error))?;
            rows.into_iter()
                .map(|(tenant, workspace, job)| {
                    Ok((
                        job_scope(&tenant, workspace.as_deref())?,
                        from_text!(job.as_str())?,
                    ))
                })
                .collect()
        }))
    }

    fn expire_leases(&self) -> PortFuture<'_, u32> {
        Box::pin(self.db.transaction(|transaction| {
            let released = transaction
                .execute(
                    "UPDATE jobs SET state = 'queued', lease_token = NULL, lease_expires_ms = NULL,
                         lease_expires_at = NULL, record = json_set(record, '$.state', 'queued')
                     WHERE state = 'running' AND lease_expires_ms <= ?1",
                    [seam::now_ms()?],
                )
                .map_err(|error| sql(&error))?;
            u32::try_from(released).map_err(|_| internal("too many expired leases to count"))
        }))
    }

    fn create_purge<'a>(
        &'a self,
        tenant: &'a TenantId,
        mutation_id: MutationId,
        purge: NewPurge,
    ) -> PortFuture<'a, Purge> {
        let tenant = tenant.clone();
        Box::pin(self.db.transaction(move |transaction| {
            insert_purge(transaction, &tenant, mutation_id, &purge)
        }))
    }

    fn get_purge<'a>(&'a self, tenant: &'a TenantId, purge: PurgeId) -> PortFuture<'a, Purge> {
        let tenant = tenant.as_str().to_owned();
        Box::pin(self.db.call(move |connection| {
            let record: Option<String> = connection
                .query_row(
                    "SELECT record FROM purges WHERE purge_id = ?1 AND tenant_id = ?2",
                    params![purge.0.to_string(), tenant],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| sql(&error))?;
            serde_json::from_str(&record.ok_or_else(|| not_found("purge"))?)
                .map_err(|error| json(&error))
        }))
    }

    fn update_purge<'a>(&'a self, tenant: &'a TenantId, purge: Purge) -> PortFuture<'a, Purge> {
        let tenant = tenant.as_str().to_owned();
        Box::pin(self.db.transaction(move |transaction| {
            let mut purge = purge;
            let stored: Option<String> = transaction
                .query_row(
                    "SELECT record FROM purges WHERE purge_id = ?1 AND tenant_id = ?2",
                    params![purge.id.0.to_string(), tenant],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| sql(&error))?;
            let stored: Purge = serde_json::from_str(&stored.ok_or_else(|| not_found("purge"))?)
                .map_err(|error| json(&error))?;
            // The store's clock stamps the first recorded completion; a repeat keeps it, and
            // a purge in any other state has none.
            purge.completed_at = if purge.state == PurgeState::Completed {
                match stored.completed_at {
                    Some(first) => Some(first),
                    None => Some(seam::now()?),
                }
            } else {
                None
            };
            let record = serde_json::to_string(&purge).map_err(|error| json(&error))?;
            let changed = transaction
                .execute(
                    "UPDATE purges SET state = ?1, record = ?2 WHERE purge_id = ?3 AND tenant_id = ?4",
                    params![variant_text!(&purge.state)?, record, purge.id.0.to_string(), tenant],
                )
                .map_err(|error| sql(&error))?;
            if changed == 0 {
                return Err(not_found("purge"));
            }
            Ok(purge)
        }))
    }

    fn revision_mapping<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: &'a Revision,
    ) -> PortFuture<'a, Option<RevisionMapping>> {
        let (tenant, workspace) = scope_key(scope);
        let revision = revision.as_str().to_owned();
        Box::pin(self.db.call(move |connection| {
            let found: Option<(String, Option<String>)> = connection
                .query_row(
                    "SELECT purge_id, replacement FROM revision_map
                     WHERE tenant_id = ?1 AND workspace_id = ?2 AND old_revision = ?3",
                    params![tenant, workspace, revision],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| sql(&error))?;
            found
                .map(|(purge, replacement)| {
                    Ok(RevisionMapping {
                        purge_id: from_text!(purge.as_str())?,
                        replacement: replacement
                            .map(|text| from_text!(text.as_str()))
                            .transpose()?,
                    })
                })
                .transpose()
        }))
    }

    fn record_derived_object<'a>(
        &'a self,
        scope: &'a StorageScope,
        object: DerivedObject,
    ) -> PortFuture<'a, DerivedObject> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.transaction(move |transaction| {
            transaction
                .execute(
                    "INSERT INTO derived_objects (tenant_id, workspace_id, item_id, revision,
                         digest, kind, media_type)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'dataset', ?6) ON CONFLICT DO NOTHING",
                    params![
                        tenant,
                        workspace,
                        object.item_id.0.to_string(),
                        object.revision.as_str(),
                        object.digest.as_str(),
                        object.media_type
                    ],
                )
                .map_err(|error| sql(&error))?;
            read_derived(
                transaction,
                &tenant,
                &workspace,
                object.item_id,
                &object.revision,
                &object.digest,
            )?
            .ok_or_else(|| internal("a derived object row vanished after its insert"))
        }))
    }

    fn derived_object<'a>(
        &'a self,
        scope: &'a StorageScope,
        item: ItemId,
        revision: &'a Revision,
        digest: &'a Digest,
    ) -> PortFuture<'a, Option<DerivedObject>> {
        let (tenant, workspace) = scope_key(scope);
        let revision = revision.clone();
        let digest = digest.clone();
        Box::pin(self.db.call(move |connection| {
            let transaction = connection.transaction().map_err(|error| sql(&error))?;
            read_derived(&transaction, &tenant, &workspace, item, &revision, &digest)
        }))
    }

    fn insert_review<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        review: Review,
    ) -> PortFuture<'a, Review> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.transaction(move |transaction| {
            let record = serde_json::to_string(&review).map_err(|error| json(&error))?;
            transaction
                .execute(
                    "INSERT INTO reviews (review_id, tenant_id, workspace_id, item_id,
                         mutation_id, record)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (mutation_id) DO NOTHING",
                    params![
                        review.id.0.to_string(),
                        tenant,
                        workspace,
                        review.source.item_id.0.to_string(),
                        mutation_id.0.to_string(),
                        record
                    ],
                )
                .map_err(|error| sql(&error))?;
            let stored: String = transaction
                .query_row(
                    "SELECT record FROM reviews WHERE mutation_id = ?1",
                    [mutation_id.0.to_string()],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            serde_json::from_str(&stored).map_err(|error| json(&error))
        }))
    }

    fn list_reviews<'a>(
        &'a self,
        scope: &'a StorageScope,
        item: ItemId,
    ) -> PortFuture<'a, Vec<Review>> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.call(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT record FROM reviews
                     WHERE tenant_id = ?1 AND workspace_id = ?2 AND item_id = ?3 ORDER BY rowid",
                )
                .map_err(|error| sql(&error))?;
            let records = statement
                .query_map(params![tenant, workspace, item.0.to_string()], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|error| sql(&error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| sql(&error))?;
            records
                .iter()
                .map(|record| serde_json::from_str(record).map_err(|error| json(&error)))
                .collect()
        }))
    }

    fn insert_receipt<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: Option<MutationId>,
        receipt: Receipt,
    ) -> PortFuture<'a, Receipt> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.transaction(move |transaction| {
            let record = serde_json::to_string(&receipt).map_err(|error| json(&error))?;
            let mutation = mutation_id.map(|id| id.0.to_string());
            transaction
                .execute(
                    "INSERT INTO receipts (receipt_id, tenant_id, workspace_id, mutation_id, record)
                     VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (mutation_id) DO NOTHING",
                    params![receipt.id.0.to_string(), tenant, workspace, mutation, record],
                )
                .map_err(|error| sql(&error))?;
            let Some(mutation) = mutation else {
                return Ok(receipt);
            };
            let stored: String = transaction
                .query_row(
                    "SELECT record FROM receipts WHERE mutation_id = ?1",
                    [mutation],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            serde_json::from_str(&stored).map_err(|error| json(&error))
        }))
    }

    fn get_receipt<'a>(
        &'a self,
        scope: &'a StorageScope,
        receipt: ReceiptId,
    ) -> PortFuture<'a, Receipt> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.db.call(move |connection| {
            let record: Option<String> = connection
                .query_row(
                    "SELECT record FROM receipts
                     WHERE receipt_id = ?1 AND tenant_id = ?2 AND workspace_id = ?3",
                    params![receipt.0.to_string(), tenant, workspace],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| sql(&error))?;
            serde_json::from_str(&record.ok_or_else(|| not_found("receipt"))?)
                .map_err(|error| json(&error))
        }))
    }
}

/// Insert a queued job, or return the job already created under its mutation id.
fn insert_job(
    transaction: &Transaction<'_>,
    scope: &JobScope,
    job: &NewJob,
) -> Result<Job, ApiError> {
    if !job.spec.fits(scope) {
        return Err(internal(
            "a job specification does not fit the scope it was created in",
        ));
    }
    let job_id: JobId = new_id!()?;
    let record = Job {
        id: job_id,
        workspace_id: scope.workspace().copied(),
        kind: job.spec.kind(),
        state: JobState::Queued,
        progress: 0,
        attempt: 0,
        warnings: Vec::new(),
        error: None,
        revision: None,
        item_ids: Vec::new(),
        artifact: None,
        restore: None,
    };
    let (tenant, workspace) = job_scope_key(scope);
    transaction
        .execute(
            "INSERT INTO jobs (job_id, tenant_id, workspace_id, mutation_id, initiator, spec,
                 record, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued') ON CONFLICT (mutation_id) DO NOTHING",
            params![
                job_id.0.to_string(),
                tenant,
                workspace,
                job.mutation_id.0.to_string(),
                serde_json::to_string(&job.initiator).map_err(|error| json(&error))?,
                job.spec.to_stored()?.to_string(),
                serde_json::to_string(&record).map_err(|error| json(&error))?
            ],
        )
        .map_err(|error| sql(&error))?;
    let stored: String = transaction
        .query_row(
            "SELECT record FROM jobs WHERE mutation_id = ?1",
            [job.mutation_id.0.to_string()],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    decode_job(&stored)
}

/// Claim a queued job, or a running one whose lease expired, under a fresh token.
fn claim(transaction: &Transaction<'_>, job_id: JobId) -> Result<Option<ClaimedJob>, ApiError> {
    let found: Option<ClaimColumns> = transaction
        .query_row(
            "SELECT tenant_id, workspace_id, record, mutation_id, initiator, spec,
                     lease_expires_ms
                 FROM jobs WHERE job_id = ?1 AND state IN ('queued', 'running')",
            [job_id.0.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((tenant, workspace, record, mutation, initiator, spec, expires_ms)) = found else {
        return Ok(None);
    };
    let mut job = decode_job(&record)?;
    let now = seam::now_ms()?;
    if job.state == JobState::Running && expires_ms.is_some_and(|expiry| expiry > now) {
        return Ok(None);
    }
    job.state = JobState::Running;
    job.attempt = job
        .attempt
        .checked_add(1)
        .ok_or_else(|| internal("a job's attempt count overflowed"))?;
    let token = seam::hex(&seam::random_bytes::<16>()?);
    let expires_at = seam::later(JOB_LEASE_SECONDS)?;
    write_job(
        transaction,
        &job,
        params![token, lease_expiry_ms()?, expires_at.as_str()],
        "lease_token = ?3, lease_expires_ms = ?4, lease_expires_at = ?5",
    )?;
    let initiator: Provenance = serde_json::from_str(&initiator).map_err(|error| json(&error))?;
    let spec = JobSpec::from_stored(serde_json::from_str(&spec).map_err(|error| json(&error))?)?;
    Ok(Some(ClaimedJob {
        scope: job_scope(&tenant, workspace.as_deref())?,
        lease: JobLease {
            job_id,
            token,
            attempt: job.attempt,
            expires_at,
        },
        job,
        mutation_id: from_text!(mutation.as_str())?,
        initiator,
        spec,
    }))
}

/// Record a completion for the current, unexpired claim.
fn complete(transaction: &Transaction<'_>, completion: &JobCompletion) -> Result<Job, ApiError> {
    let row = leased_row(transaction, completion.lease.job_id)?;
    let live = row
        .expires_ms
        .is_some_and(|expiry| seam::now_ms().is_ok_and(|now| expiry > now));
    if row.token.as_deref() != Some(completion.lease.token.as_str())
        || row.state != "running"
        || !live
    {
        return Err(lost_claim());
    }
    let scope = job_scope(&row.tenant_id, row.workspace_id.as_deref())?;
    let mut job = decode_job(&row.record)?;
    job.artifact = completion
        .artifact
        .map(|artifact| {
            read_artifact(transaction, "artifact_id", &artifact.0.to_string())?
                .filter(|record| record.scope == scope)
                .map(|record| record.download())
                .ok_or_else(|| {
                    internal("a completion names an artifact this job's scope does not hold")
                })
        })
        .transpose()?;
    job.state = JobState::Succeeded;
    job.progress = 100;
    job.revision.clone_from(&completion.revision);
    job.item_ids.clone_from(&completion.item_ids);
    job.restore.clone_from(&completion.restore);
    job.warnings.clone_from(&completion.warnings);
    job.error = None;
    write_job(transaction, &job, params![], "lease_expires_ms = NULL")?;
    for digest in &completion.outputs {
        transaction
            .execute(
                "INSERT INTO job_outputs (job_id, digest) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
                params![job.id.0.to_string(), digest.as_str()],
            )
            .map_err(|error| sql(&error))?;
    }
    Ok(job)
}

/// Cancel or retry a job once per mutation id.
fn control(
    transaction: &Transaction<'_>,
    scope: &JobScope,
    mutation_id: MutationId,
    job_id: JobId,
    action: &str,
) -> Result<Job, ApiError> {
    let (tenant, workspace) = job_scope_key(scope);
    let record: String = transaction
        .query_row(
            "SELECT record FROM jobs WHERE job_id = ?1 AND tenant_id = ?2 AND workspace_id IS ?3",
            params![job_id.0.to_string(), tenant, workspace],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?
        .ok_or_else(|| not_found("job"))?;
    let inserted = transaction
        .execute(
            "INSERT INTO job_controls (mutation_id, job_id, action) VALUES (?1, ?2, ?3)
             ON CONFLICT (mutation_id) DO NOTHING",
            params![mutation_id.0.to_string(), job_id.0.to_string(), action],
        )
        .map_err(|error| sql(&error))?;
    let mut job = decode_job(&record)?;
    if inserted == 0 {
        return Ok(job);
    }
    match (action, &job.state) {
        ("cancel", JobState::Queued | JobState::Running) => {
            job.state = JobState::Cancelled;
            write_job(transaction, &job, params![], "lease_expires_ms = NULL")?;
            Ok(job)
        }
        ("retry", JobState::Failed) => {
            let retryable: i64 = transaction
                .query_row(
                    "SELECT retryable FROM jobs WHERE job_id = ?1",
                    [job_id.0.to_string()],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            if retryable == 0 {
                return Err(conflict("this job's failure is not retryable"));
            }
            job.state = JobState::Queued;
            job.error = None;
            write_job(
                transaction,
                &job,
                params![],
                "lease_token = NULL, lease_expires_ms = NULL, lease_expires_at = NULL",
            )?;
            Ok(job)
        }
        _ => Err(conflict(format!(
            "a {} job cannot be {}",
            variant_text!(&job.state)?,
            if action == "cancel" {
                "cancelled"
            } else {
                "retried"
            }
        ))),
    }
}

/// Record a purge with its tenant job, or return the purge already recorded for its id or the
/// unfinished purge of the same target.
fn insert_purge(
    transaction: &Transaction<'_>,
    tenant: &TenantId,
    mutation_id: MutationId,
    purge: &NewPurge,
) -> Result<Purge, ApiError> {
    let target_key = match &purge.target {
        PurgeTarget::Workspace { workspace_id } => format!("workspace/{}", workspace_id.0),
        PurgeTarget::Item {
            workspace_id,
            item_id,
        } => format!("item/{}/{}", workspace_id.0, item_id.0),
    };
    let prior: Option<String> = transaction
        .query_row(
            "SELECT record FROM purges WHERE tenant_id = ?1
               AND (purge_id = ?2 OR (target_key = ?3 AND state <> 'completed'))
             ORDER BY purge_id = ?2 DESC LIMIT 1",
            params![tenant.as_str(), purge.id.0.to_string(), target_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    if let Some(prior) = prior {
        return serde_json::from_str(&prior).map_err(|error| json(&error));
    }
    let job = insert_job(
        transaction,
        &JobScope::Tenant(tenant.clone()),
        &NewJob {
            mutation_id,
            initiator: purge.initiator.clone(),
            spec: purge_job_spec(&purge.target, purge.id),
        },
    )?;
    let record = Purge {
        id: purge.id,
        target: purge.target.clone(),
        state: PurgeState::Requested,
        job_id: job.id,
        requested_by: purge.initiator.subject.clone(),
        requested_at: seam::now()?,
        completed_at: None,
        report: None,
        error: None,
    };
    transaction
        .execute(
            "INSERT INTO purges (purge_id, tenant_id, target_key, state, mutation_id, record)
             VALUES (?1, ?2, ?3, 'requested', ?4, ?5)",
            params![
                purge.id.0.to_string(),
                tenant.as_str(),
                target_key,
                mutation_id.0.to_string(),
                serde_json::to_string(&record).map_err(|error| json(&error))?
            ],
        )
        .map_err(|error| sql(&error))?;
    Ok(record)
}

fn leased_row(transaction: &Transaction<'_>, job_id: JobId) -> Result<LeasedRow, ApiError> {
    transaction
        .query_row(
            "SELECT tenant_id, workspace_id, record, state, lease_token, lease_expires_ms
             FROM jobs WHERE job_id = ?1",
            [job_id.0.to_string()],
            |row| {
                Ok(LeasedRow {
                    tenant_id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    record: row.get(2)?,
                    state: row.get(3)?,
                    token: row.get(4)?,
                    expires_ms: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|error| sql(&error))?
        .ok_or_else(|| not_found("job"))
}

/// Store `job` with its state column and the extra assignments in `set`, whose parameters
/// start at `?3`.
fn write_job(
    transaction: &Transaction<'_>,
    job: &Job,
    extra: &[&dyn rusqlite::ToSql],
    set: &str,
) -> Result<(), ApiError> {
    let record = serde_json::to_string(job).map_err(|error| json(&error))?;
    let state = variant_text!(&job.state)?;
    let id = job.id.0.to_string();
    let mut values: Vec<&dyn rusqlite::ToSql> = vec![&record, &state];
    values.extend_from_slice(extra);
    values.push(&id);
    let last = values.len();
    transaction
        .execute(
            &format!("UPDATE jobs SET record = ?1, state = ?2, {set} WHERE job_id = ?{last}"),
            values.as_slice(),
        )
        .map_err(|error| sql(&error))?;
    Ok(())
}

fn read_artifact(
    transaction: &Transaction<'_>,
    column: &str,
    value: &str,
) -> Result<Option<ArtifactRecord>, ApiError> {
    let found: Option<ArtifactColumns> = transaction
        .query_row(
            &format!(
                "SELECT artifact_id, tenant_id, workspace_id, kind, digest, size, media_type,
                         job_id
                     FROM artifacts WHERE {column} = ?1"
            ),
            [value],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()
        .map_err(|error| sql(&error))?;
    found
        .map(
            |(id, tenant, workspace, kind, digest, size, media_type, job)| {
                Ok(ArtifactRecord {
                    id: from_text!(id.as_str())?,
                    scope: job_scope(&tenant, workspace.as_deref())?,
                    kind: from_text!(kind.as_str())?,
                    object: ObjectInfo {
                        digest: from_text!(digest.as_str())?,
                        size: to_u64(size)?,
                    },
                    media_type,
                    created_by_job: from_text!(job.as_str())?,
                })
            },
        )
        .transpose()
}

fn read_derived(
    transaction: &Transaction<'_>,
    tenant: &str,
    workspace: &str,
    item: ItemId,
    revision: &Revision,
    digest: &Digest,
) -> Result<Option<DerivedObject>, ApiError> {
    let media_type: Option<String> = transaction
        .query_row(
            "SELECT media_type FROM derived_objects WHERE tenant_id = ?1 AND workspace_id = ?2
               AND item_id = ?3 AND revision = ?4 AND digest = ?5",
            params![
                tenant,
                workspace,
                item.0.to_string(),
                revision.as_str(),
                digest.as_str()
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    Ok(media_type.map(|media_type| DerivedObject {
        item_id: item,
        revision: revision.clone(),
        digest: digest.clone(),
        kind: DerivedKind::Dataset,
        media_type,
    }))
}

/// Rebuild a job or artifact scope from its key columns.
fn job_scope(tenant: &str, workspace: Option<&str>) -> Result<JobScope, ApiError> {
    let tenant_id: TenantId = from_text!(tenant)?;
    Ok(match workspace {
        None => JobScope::Tenant(tenant_id),
        Some(workspace) => JobScope::Workspace(StorageScope {
            tenant_id,
            workspace_id: from_text!(workspace)?,
        }),
    })
}

fn decode_job(record: &str) -> Result<Job, ApiError> {
    serde_json::from_str(record).map_err(|error| json(&error))
}

fn lease_expiry_ms() -> Result<i64, ApiError> {
    Ok(seam::now_ms()?.saturating_add(JOB_LEASE_SECONDS.saturating_mul(1000)))
}

fn lost_claim() -> ApiError {
    conflict("this claim is no longer the job's current lease")
}
