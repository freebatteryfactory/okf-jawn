//! Proposal and comment persistence is separate from versioned content and review evidence.
//!
//! Comment inserts take `MutationId` and enforce uniqueness.

use okf_jawn_contract::{
    identity::{MutationId, ProposalId},
    proposal::{
        Comment, GetProposalRequest, ListProposalsRequest, ListProposalsResponse, Proposal,
    },
};

use crate::ports::PortFuture;
use crate::storage::StorageScope;

/// Durable suggested change sets and discussion; storage owns the implementation.
pub trait ProposalStore: Send + Sync {
    /// Persist a new open proposal.
    fn insert<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        proposal: Proposal,
    ) -> PortFuture<'a, Proposal>;
    /// Read one proposal by identity.
    fn get<'a>(
        &'a self,
        scope: &'a StorageScope,
        request: GetProposalRequest,
    ) -> PortFuture<'a, Proposal>;
    /// List proposals with optional status filter and pagination.
    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        request: ListProposalsRequest,
    ) -> PortFuture<'a, ListProposalsResponse>;
    /// Replace proposal status and retained fields after accept, decline, or conflict.
    fn update<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal: Proposal,
    ) -> PortFuture<'a, Proposal>;
    /// Append a discussion comment without certifying content.
    ///
    /// Unique on `mutation_id`; a reused id returns the prior comment, never a duplicate.
    fn add_comment<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        proposal: ProposalId,
        comment: Comment,
    ) -> PortFuture<'a, Comment>;
    /// Look up a comment created under `mutation_id`, for abandoned-lease reconciliation.
    fn find_comment_by_mutation<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
    ) -> PortFuture<'a, Option<Comment>>;
}
