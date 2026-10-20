//! Durable mutation ledger: one write identity per (tenant, subject, client, operation, key).
//!
//! # Retention
//! A completed row is kept for 7 days, and so is a released row; the clock starts when the row
//! is completed or released; a terminally purged row keeps that existing retention clock.
//! A released attempt may have left effects, and those are safe to
//! leave because every creating store keys them by `MutationId`; the row itself is no longer
//! needed once the retry window has passed. A key reused after its row is dropped starts a new
//! mutation. Only a row whose lease expired without `complete` or `release` (its attempt
//! crashed, or could not reach the ledger) stays until it is reconciled, that is until a later
//! attempt resumes and completes/releases it, or a recorded workspace purge terminalizes it.
//! The 7-day TTL never drops an unreconciled row, or
//! crash protection has a hole. SPEC §8's "abandoned mutations stay until reconciled" means a
//! row whose lease expired without `complete` or `release`; `BeginOutcome::Abandoned` is also
//! returned for a released row, which does expire.
//!
//! A lease must expire. Its length is set by the storage implementation, which records it in
//! `crates/storage/AGENTS.md`; three exits other than a crash leave a live lease until then: a
//! dropped request future, a failed `release`, and a store that wrongly returns `Conflict`.
//!
//! # Resumed attempts
//! The ledger never looks into another store. Every store that creates a durable row takes the
//! `MutationId` and treats a repeated id as a no-op that returns the prior row. A handler is
//! therefore safe to run again under the same id: when `begin` reports `Abandoned`, dispatch
//! re-runs the handler with `Attempt::Resumed` and the same `MutationId`, and each store hands
//! back what the earlier attempt already wrote instead of writing it twice.
//!
//! # Leases
//! `begin` grants a lease, and every grant of the same mutation carries a different token. A
//! slow attempt whose lease expired and the resumed attempt that took the mutation over run
//! under one `MutationId` at the same time, so `complete` and `release` take the
//! [`MutationLease`] and act only for the current grant: the slow attempt's `complete` is
//! refused with `Conflict`, and its `release` changes nothing.
//!
//! # Failed attempts
//! A handler error releases the lease, and so does any failure after a successful handler
//! (the response cannot be serialized, the ledger body cannot be built, or `complete` fails
//! for a reason other than a lost lease). The row keeps its id and digest and is not
//! completed, so the same key and body may be sent again at once; that retry runs as a
//! resumed attempt. A recorded workspace purge instead terminalizes the attempt without
//! its digest or response; `begin` then refuses it as described below.
//!
//! # Workspace
//! Each row records the workspace its mutation writes, beside the key and never part of it:
//! `begin` records the workspace of the request's first target, the one the operation's table
//! permission names (dispatch passes `None` when that target is the tenant), so a comment
//! written in one workspace that cites another is recorded under the first; and `complete`
//! records the workspace a response names for a mutation authorized at the tenant that creates
//! or removes one (`RequestScope::RESPONSE_WORKSPACE`). A workspace purge reads that column,
//! never the response text: it marks completed and released rows of the purged workspace as
//! terminally purged, dropping their response and request digest while retaining their key
//! and identity. A retry is refused by `begin` before digest comparison, without another
//! handler run or a retention-clock write. No other workspace's row is touched.
//! The rows of `purge_workspace` and `purge_item` recorded under the
//! workspace are kept: their response is the purge record, identities and counts only, and a
//! retried purge replays it.
//!
//! `complete` and `release` check the recorded workspace purge inside their own transaction:
//! a late completion stores no response or digest, and a late release is purged as well.
//! Purged rows expire after seven days on their existing completion/release clock; refusing
//! a retry never extends it. The caller's random idempotency key stays to refuse the retry.
//!
//! Lifecycle ruling9 closes the earlier unattributed-row limit: no installation predates
//! records schema2. The schema5 upgrade also empties the request digests of already-emptied
//! rows in the one records copy taken immediately before upgrading. A pre-migration ledger
//! missing a column required for purge is a store fault, never silently skipped. These are
//! port guarantees; the lifecycle storage/schema implementation must supply them. The
//! `purge::rows` live/copy rewrite remains separate pending implementation work.

use std::collections::BTreeMap;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, IdempotencyKey, MutationId, TenantId, WorkspaceId};
use okf_jawn_contract::metadata::OperationName;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest as ShaDigest, Sha256};

use crate::ports::PortFuture;

/// Ledger key: one caller can never read another caller's stored response.
///
/// Subject and client are separate components: a connector acting for a subject is a
/// different caller from the subject, so it neither replays the subject's stored response nor
/// conflicts with the subject's key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MutationKey {
    /// Tenant boundary.
    pub tenant_id: TenantId,
    /// Authenticated subject.
    pub subject: String,
    /// Client the subject acts through (`Principal::client_id`); `None` for a direct caller.
    pub client_id: Option<String>,
    /// Canonical operation name.
    pub operation: OperationName,
    /// Caller-chosen retry identity.
    pub key: IdempotencyKey,
}

/// One granted lease on a mutation. `token` is different for every grant of the same mutation.
///
/// Two attempts can run under one `MutationId` at once: a slow first attempt whose lease
/// expired, and the resumed attempt that took the mutation over. Only the holder of the
/// current grant may complete or release it, so each grant is named by its own token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MutationLease {
    /// The durable write identity.
    pub mutation_id: MutationId,
    /// Changes every time the lease is granted; a stale holder cannot complete or release.
    pub token: u64,
}

/// Atomic insert-or-read outcome for one begin attempt.
#[derive(Debug, Clone)]
pub enum BeginOutcome {
    /// No prior row; execute the handler under this lease.
    New(MutationLease),
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
    /// after a handler error. The caller now holds a new lease and re-runs the handler under
    /// the same identity.
    Abandoned {
        /// New grant on the mutation whose rows may already exist in the stores the handler
        /// writes. Its token differs from every earlier grant's.
        lease: MutationLease,
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
    /// `digest` is [`request_digest`] of the typed request. `workspace` is the workspace the
    /// request writes, that of its first target, or `None` for a request authorized at the
    /// tenant (see the module header); it is recorded with a new row and is not part of the
    /// key. A row that already exists keeps what its first `begin` recorded: the same key and
    /// digest are the same request, so they name the same workspace.
    /// An existing terminally purged row is refused with `Conflict` naming the workspace
    /// and purge, before comparing its absent digest and without writing. The same refusal
    /// converts an expired leased or released row whose workspace has a recorded purge to
    /// purged, except for `purge_workspace` and `purge_item`; its retry never runs a handler.
    fn begin<'a>(
        &'a self,
        key: &'a MutationKey,
        digest: &'a Digest,
        workspace: Option<WorkspaceId>,
    ) -> PortFuture<'a, BeginOutcome>;

    /// Finish the current grant: retain its response for replay, or record purged without
    /// digest/response when its effective workspace has a recorded purge.
    ///
    /// `workspace` is the workspace the completed response names
    /// (`RequestScope::RESPONSE_WORKSPACE`), or `None`; it is recorded only when `begin`
    /// recorded none, so a request's own workspace is never replaced.
    ///
    /// Compare-and-set on the lease: fails with `ErrorCode::Conflict` when `lease` is no longer
    /// the current grant (it expired and another attempt holds the mutation). Nothing is
    /// stored in that case; the response of the attempt that holds the grant is the one kept.
    ///
    /// `Conflict` is returned for a lost lease and for nothing else; any other failure uses
    /// another code (`Unavailable` or `Internal`). Dispatch treats `Conflict` here as "another
    /// attempt holds this mutation" and does not release.
    fn complete(
        &self,
        lease: MutationLease,
        response: Value,
        workspace: Option<WorkspaceId>,
    ) -> PortFuture<'_, ()>;

    /// End the current lease after a failed attempt, preserving its identity.
    ///
    /// Ends the lease only if `lease` is the current grant; a stale lease is a no-op, and so is
    /// releasing a completed mutation. This is what keeps a row completed when `complete`
    /// committed and then reported an error. A recorded workspace purge writes purged instead;
    /// releasing an already purged row is a no-op. When no recorded workspace purge applies,
    /// after a release that took effect the next `begin` with the same key and digest returns
    /// `Abandoned` without waiting for the lease to expire.
    fn release(&self, lease: MutationLease) -> PortFuture<'_, ()>;
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
