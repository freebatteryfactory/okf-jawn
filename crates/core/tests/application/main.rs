//! `ApplicationService` handlers over fakes of their ports: what each operation of the item,
//! read, source, history, search, health, session and purge modules asks of the ports, and what
//! it answers.
//!
//! Handlers are called the way dispatch calls them, with the `OperationContext` dispatch
//! builds; dispatch's own rules (validation, authorization, the human-route refusal of
//! draft-bearing operations, the mutation ledger) are tested in `tests/dispatch.rs`.

mod drafts;
#[path = "../support/application.rs"]
mod fixture;
mod history;
mod items;
mod navigation;
#[path = "../support/purges.rs"]
mod purge_fixture;
mod purges;
mod reads;
mod service;
mod sources;
mod status;

#[path = "../../../../tests/support/check.rs"]
mod check;
