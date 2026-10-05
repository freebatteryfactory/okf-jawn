//! Proposal and comment persistence is separate from versioned content and review evidence.

use okf_jawn_contract::{
    identity::ProposalId,
    proposal::{
        Comment, GetProposalRequest, ListProposalsRequest, ListProposalsResponse, Proposal,
    },
};

use crate::ports::PortFuture;
use crate::storage::StorageScope;

/// Durable suggested change sets and discussion; storage owns the implementation.
pub trait ProposalStore: Send + Sync {
    /// Persist a new open proposal.
    fn insert<'a>(&'a self, scope: &'a StorageScope, proposal: Proposal) -> PortFuture<'a, Proposal>;
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
    fn update<'a>(&'a self, scope: &'a StorageScope, proposal: Proposal) -> PortFuture<'a, Proposal>;
    /// Append a discussion comment without certifying content.
    fn add_comment<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal: ProposalId,
        comment: Comment,
    ) -> PortFuture<'a, Comment>;
}
