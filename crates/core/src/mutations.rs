//! Durable mutation ledger: one write identity per (tenant, subject, operation, key).
//!
//! # Retention
//! Completed mutations are retained for 7 days; a key reused after that starts a new mutation.
//! Abandoned mutations stay until reconciled (completed or explicitly failed); they are never
//! dropped by the 7-day TTL, or crash protection has a hole.
//!
//! # Reconciliation
//! Reconciliation is not Git-only. Every store that creates a durable row takes `MutationId`
//! and enforces uniqueness. On `Abandoned`, dispatch looks up the id in the relevant store(s);
//! if a row exists the handler is not re-run.

use okf_jawn_contract::error::ApiError;
use okf_jawn_contract::identity::{Digest, IdempotencyKey, MutationId, TenantId};
use okf_jawn_contract::metadata::OperationName;
use serde_json::Value;
use sha2::{Digest as ShaDigest, Sha256};

use crate::ports::PortFuture;

/// Ledger key: one caller can never read another caller's stored response.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MutationKey {
    /// Tenant boundary.
    pub tenant_id: TenantId,
    /// Authenticated subject.
    pub subject: String,
    /// Canonical operation name.
    pub operation: OperationName,
    /// Caller-chosen retry identity.
    pub key: IdempotencyKey,
}

/// Atomic insert-or-read outcome for one begin attempt.
#[derive(Debug, Clone)]
pub enum BeginOutcome {
    /// No prior row; execute the handler under this identity.
    New(MutationId),
    /// Same key and digest already completed; return the stored response (or AlreadyIssued).
    Replay(StoredResponse),
    /// Same key, different digest.
    Conflict {
        /// Operation that first used the key.
        operation: OperationName,
    },
    /// Another attempt holds a live lease.
    InProgress {
        /// Mutation holding the lease.
        mutation_id: MutationId,
        /// Whole seconds before the caller should retry.
        retry_after: u32,
    },
    /// The lease expired without completion; the caller holds the lease and must reconcile.
    Abandoned {
        /// Mutation whose effect may already exist in a creating store.
        mutation_id: MutationId,
    },
}

/// Completed or recorded response body retained by the ledger.
#[derive(Debug, Clone)]
pub struct StoredResponse {
    /// Mutation that produced this response.
    pub mutation_id: MutationId,
    /// Serialized response body. Secret-bearing operations store only non-secret fields.
    pub body: Value,
}

/// Idempotency ledger owned by storage; adapters enforce uniqueness and leases.
pub trait MutationStore: Send + Sync {
    /// Atomic insert-or-read with a lease.
    ///
    /// `digest` is SHA-256 of the canonical request JSON.
    fn begin<'a>(
        &'a self,
        key: &'a MutationKey,
        digest: &'a Digest,
    ) -> PortFuture<'a, BeginOutcome>;

    /// Record that a durable effect was produced before `complete` (crash window).
    fn record_effect<'a>(
        &'a self,
        mutation_id: MutationId,
        effect: Value,
    ) -> PortFuture<'a, ()>;

    /// Mark the mutation completed and retain the response for replay.
    fn complete<'a>(
        &'a self,
        mutation_id: MutationId,
        response: Value,
    ) -> PortFuture<'a, ()>;

    /// Look up a mutation by id when reconciling an abandoned lease.
    fn find<'a>(
        &'a self,
        mutation_id: MutationId,
    ) -> PortFuture<'a, Option<StoredResponse>>;
}

/// Look up whether an abandoned mutation already produced its durable effect outside the ledger.
///
/// Production wires VersionStore::find_mutation, CredentialStore, comments, uploads,
/// confirmations, and drafts. Test fixtures implement this directly.
pub trait AbandonedEffects: Send + Sync {
    /// Return a response body when the effect already exists; `None` means the handler may run.
    fn lookup<'a>(
        &'a self,
        operation: OperationName,
        mutation_id: MutationId,
    ) -> PortFuture<'a, Option<Value>>;
}

/// SHA-256 digest of canonical JSON bytes for an idempotency begin.
///
/// # Errors
/// Returns `Internal` when serialization fails or the digest is not valid hex.
pub fn request_digest(value: &Value) -> Result<Digest, ApiError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        okf_jawn_contract::error::ApiError::new(
            okf_jawn_contract::error::ErrorCode::Internal,
            error.to_string(),
        )
    })?;
    let hash = Sha256::digest(bytes);
    let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    Digest::try_from(hex).map_err(|error| {
        ApiError::new(
            okf_jawn_contract::error::ErrorCode::Internal,
            error.to_string(),
        )
    })
}
