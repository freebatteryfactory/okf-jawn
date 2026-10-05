//! Durable mutation ledger: one write identity per (tenant, subject, operation, key).
//!
//! # Retention
//! Completed mutations are retained for 7 days; a key reused after that starts a new mutation.
//! A mutation that began and never completed stays until a later attempt completes it; the
//! 7-day TTL never drops it, or crash protection has a hole.
//!
//! # Resumed attempts
//! The ledger never looks into another store. Every store that creates a durable row takes the
//! `MutationId` and treats a repeated id as a no-op that returns the prior row. A handler is
//! therefore safe to run again under the same id: when `begin` reports `Abandoned`, dispatch
//! re-runs the handler with `Attempt::Resumed` and the same `MutationId`, and each store hands
//! back what the earlier attempt already wrote instead of writing it twice.
//!
//! # Failed handlers
//! A handler error releases the lease. The row keeps its id and digest and is not completed,
//! so the same key and body may be sent again at once; that retry runs as a resumed attempt.

use std::collections::BTreeMap;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, IdempotencyKey, MutationId, TenantId};
use okf_jawn_contract::metadata::OperationName;
use serde::Serialize;
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
    /// Same key and digest already completed; return the stored response (or `AlreadyIssued`).
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
    /// An earlier attempt ended without completing: its lease expired, or it was released
    /// after a handler error. The caller now holds the lease and re-runs the handler under
    /// the same identity.
    Abandoned {
        /// Mutation whose rows may already exist in the stores the handler writes.
        mutation_id: MutationId,
    },
}

/// Completed response body retained by the ledger.
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
    /// `digest` is [`request_digest`] of the typed request.
    fn begin<'a>(
        &'a self,
        key: &'a MutationKey,
        digest: &'a Digest,
    ) -> PortFuture<'a, BeginOutcome>;

    /// Mark the mutation completed and retain the response for replay.
    fn complete(&self, mutation_id: MutationId, response: Value) -> PortFuture<'_, ()>;

    /// End the lease after a handler error. The row keeps its id; the same key may begin again.
    ///
    /// The next `begin` with the same key and digest returns `Abandoned` without waiting for
    /// the lease to expire. Releasing a completed mutation changes nothing.
    fn release(&self, mutation_id: MutationId) -> PortFuture<'_, ()>;
}

/// SHA-256 over the typed request re-serialized with object keys sorted; independent of
/// `serde_json`'s `preserve_order` feature.
///
/// # Errors
/// Returns `Internal` when the request cannot be serialized.
pub fn request_digest<T: Serialize>(request: &T) -> Result<Digest, ApiError> {
    let value = serde_json::to_value(request).map_err(|error| internal(&error))?;
    let bytes = serde_json::to_vec(&canonical(value)).map_err(|error| internal(&error))?;
    let hash = Sha256::digest(bytes);
    Digest::try_from(format!("{hash:x}")).map_err(|error| internal(&error))
}

/// Rebuild `value` with the keys of every object in byte order, at every depth.
///
/// Collecting through a `BTreeMap` fixes the order whether `serde_json::Map` keeps insertion
/// order (`preserve_order`) or is itself a `BTreeMap`.
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> = map
                .into_iter()
                .map(|(key, child)| (key, canonical(child)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        scalar => scalar,
    }
}

fn internal(error: &dyn std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, error.to_string())
}
