//! Proposal and comment persistence is separate from versioned content and review evidence.
//!
//! Inserting a proposal or a comment takes `MutationId`; a repeated id writes nothing and
//! returns the prior row. A status change is a compare-and-set on the stored status, and only
//! the mutation that wrote a change may write it again as a retry (`ProposalStore::update`).

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

/// One page of a proposal's discussion, oldest first.
#[derive(Debug, Clone)]
pub struct CommentPage {
    /// Comments on this page.
    pub items: Vec<Comment>,
    /// Continuation cursor, when more comments remain.
    pub next_cursor: Option<String>,
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
    /// Replace a proposal's status and retained fields after accept, decline, or conflict, only
    /// while its stored status is `expected`: a compare-and-set, so of two changes that race from
    /// the same status (an accept and a decline of one open proposal, or two declines of it) one
    /// is written and the other refused. The store keeps the mutation that wrote each change.
    ///
    /// Fails with `Conflict`, writing nothing, when the stored status is not `expected`, and
    /// with `NotFound` when the workspace has no such proposal. Writing again exactly the record
    /// that is stored (its status and every field) under the mutation that wrote it changes
    /// nothing and returns it, whatever `expected` says, so a retry of a change that was written
    /// succeeds. The same record under another mutation is a change like any other, refused when
    /// the stored status is not `expected`; so is a record whose writer the store does not keep
    /// (a workspace restored from an archive, which carries no retry records).
    fn update<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        proposal: Proposal,
        expected: ProposalStatus,
    ) -> PortFuture<'a, Proposal>;
    /// Append a discussion comment without certifying content.
    ///
    /// Unique on `mutation_id`: a repeated id inserts nothing and returns the prior comment.
    /// `NotFound`, writing nothing, when the workspace has no such proposal.
    fn add_comment<'a>(
        &'a self,
        scope: &'a StorageScope,
        mutation_id: MutationId,
        proposal: ProposalId,
        comment: Comment,
    ) -> PortFuture<'a, Comment>;
    /// List a proposal's discussion, oldest first; `NotFound` when the workspace has no such
    /// proposal.
    fn list_comments<'a>(
        &'a self,
        scope: &'a StorageScope,
        proposal: ProposalId,
        page: Page,
    ) -> PortFuture<'a, CommentPage>;
}
