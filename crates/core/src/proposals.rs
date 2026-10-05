//! Proposal and comment persistence is separate from versioned content and review evidence.
//!
//! Inserting a proposal or a comment takes `MutationId`; a repeated id writes nothing and
//! returns the prior row.

use okf_jawn_contract::{
    identity::{MutationId, ProposalId},
    proposal::{Comment, ListProposalsResponse, Proposal, ProposalStatus},
};

use crate::ports::PortFuture;
use crate::storage::{Page, StorageScope};

/// Which proposals of a workspace to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalFilter {
    /// Only proposals in this state; `None` lists every state.
    pub status: Option<ProposalStatus>,
    /// Bounded page.
    pub page: Page,
}

/// Durable suggested change sets and discussion; storage owns the implementation.
pub trait ProposalStore: Send + Sync {
    /// Persist a new open proposal.
    ///
    /// Unique on `mutation_id`: a repeated id inserts nothing and returns the prior proposal.
    fn insert<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        proposal: Proposal,
    ) -> PortFuture<'a, Proposal>;
    /// Read one proposal by identity; `NotFound` when the workspace has no such proposal.
    fn get<'a>(&'a self, scope: &'a StorageScope, proposal: ProposalId)
    -> PortFuture<'a, Proposal>;
    /// List proposals, newest first.
    fn list<'a>(
        &'a self,
        scope: &'a StorageScope,
        filter: ProposalFilter,
    ) -> PortFuture<'a, ListProposalsResponse>;
    /// Replace proposal status and retained fields after accept, decline, or conflict.
    ///
    /// Writing the same state twice changes nothing.
    fn update<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal: Proposal,
    ) -> PortFuture<'a, Proposal>;
    /// Append a discussion comment without certifying content.
    ///
    /// Unique on `mutation_id`: a repeated id inserts nothing and returns the prior comment.
    fn add_comment<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        proposal: ProposalId,
        comment: Comment,
    ) -> PortFuture<'a, Comment>;
}
