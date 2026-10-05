//! Session-bound human confirmation challenges; consume records which MutationId used it.
//!
//! A retry of the same MutationId after a crash may re-consume successfully; a different
//! MutationId still fails as already used.

use okf_jawn_contract::identity::{ConfirmationId, Digest, MutationId, Revision};
use okf_jawn_contract::review::{Confirmation, ConfirmationAction, ConfirmationTarget};

use crate::ports::PortFuture;
use crate::storage::StorageScope;

/// Fields required to create a confirmation challenge.
#[derive(Debug, Clone)]
pub struct ConfirmationCreate {
    /// Confirmed action.
    pub action: ConfirmationAction,
    /// Item or proposal being confirmed.
    pub target: ConfirmationTarget,
    /// Displayed content revision.
    pub revision: Revision,
    /// Displayed content digest.
    pub content_digest: Digest,
    /// Browser session the confirmation is bound to.
    pub session_id: String,
    /// Subject that may consume the confirmation.
    pub subject: String,
    /// RFC 3339 expiry.
    pub expires_at: String,
}

/// Atomic consume checks; success records `mutation_id` as the consumer.
#[derive(Debug, Clone)]
pub struct ConfirmationConsume {
    /// Challenge to consume.
    pub confirmation_id: ConfirmationId,
    /// Expected action.
    pub action: ConfirmationAction,
    /// Expected target.
    pub target: ConfirmationTarget,
    /// Expected revision.
    pub revision: Revision,
    /// Expected digest.
    pub content_digest: Digest,
    /// Session presenting the confirmation.
    pub session_id: String,
    /// Subject presenting the confirmation.
    pub subject: String,
}

/// Confirmation issuance and single-use consume; storage owns the implementation.
pub trait ConfirmationStore: Send + Sync {
    /// Create a session-bound challenge.
    ///
    /// Unique on `mutation_id`; a reused id returns the prior row, never a duplicate.
    fn create<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        create: ConfirmationCreate,
    ) -> PortFuture<'a, Confirmation>;
    /// Look up a confirmation created under `mutation_id`.
    fn find_by_mutation<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
    ) -> PortFuture<'a, Option<Confirmation>>;
    /// Atomically consume a confirmation for `mutation_id`.
    ///
    /// Checks subject, session, action, target, revision, digest and expiry. Records which
    /// `MutationId` consumed it. Re-consume with the same `MutationId` succeeds; a different
    /// `MutationId` fails as already used.
    fn consume<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        consume: ConfirmationConsume,
    ) -> PortFuture<'a, Confirmation>;
}
