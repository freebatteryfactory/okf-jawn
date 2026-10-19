//! `ApplicationService` handlers over fakes of their ports: what each operation of the item,
//! read, source, history, search, health, session, proposal, review, purge, naming-rules
//! (conventions), workspace, event, connector, upload and job-control modules asks of the
//! ports, and what it answers.
//!
//! Handlers are called the way dispatch calls them, with the `OperationContext` dispatch
//! builds; dispatch's own rules (validation, authorization, the human-route refusal of
//! draft-bearing operations, the mutation ledger) are tested in `tests/dispatch.rs`, and its
//! archive and local-owner rules in `tests/archived.rs`. Where a rule belongs to the two
//! together (a handler's `Forbidden` that dispatch records, the archive rule in front of a
//! handler), `job_control` dispatches to the service over the same fakes, with the fixture
//! ledger (`support`).

#[path = "../support/candidates.rs"]
mod candidates;
#[path = "../support/confirmations.rs"]
mod confirmations;
mod connectors;
mod conventions;
mod drafts;
mod events;
#[path = "../support/application.rs"]
mod fixture;
mod history;
mod items;
mod job_control;
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
#[path = "../support/mod.rs"]
mod support;
#[path = "../support/tenancy.rs"]
mod tenancy;
mod uploads;
mod views;
mod workspaces;

#[path = "../../../../tests/support/check.rs"]
mod check;
