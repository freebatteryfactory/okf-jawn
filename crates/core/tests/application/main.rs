//! `ApplicationService` handlers over fakes of their ports: what each operation of the item,
//! read, source, history, search, health, session, proposal, review, purge and naming-rules
//! (conventions) modules asks of the ports, and what it answers.
//!
//! Handlers are called the way dispatch calls them, with the `OperationContext` dispatch
//! builds; dispatch's own rules (validation, authorization, the human-route refusal of
//! draft-bearing operations, the mutation ledger) are tested in `tests/dispatch.rs`.

#[path = "../support/candidates.rs"]
mod candidates;
#[path = "../support/confirmations.rs"]
mod confirmations;
mod conventions;
mod drafts;
#[path = "../support/application.rs"]
mod fixture;
mod history;
mod items;
mod navigation;
#[path = "../support/proposals.rs"]
mod proposal_store;
mod proposals;
#[path = "../support/purges.rs"]
mod purge_fixture;
mod purges;
mod reads;
#[path = "../support/reviews.rs"]
mod review_store;
mod reviews;
mod service;
mod sources;
mod status;

#[path = "../../../../tests/support/check.rs"]
mod check;
