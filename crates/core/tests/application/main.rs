//! `ApplicationService` handlers over fakes of their ports: what each operation of the item,
//! read, source, history, search, health, session, workspace, event and connector modules asks
//! of the ports, and what it answers.
//!
//! Handlers are called the way dispatch calls them, with the `OperationContext` dispatch
//! builds; dispatch's own rules (validation, authorization, the human-route refusal of
//! draft-bearing operations, the mutation ledger) are tested in `tests/dispatch.rs`, and its
//! archive and local-owner rules in `tests/archived.rs`.

mod connectors;
mod drafts;
mod events;
#[path = "../support/application.rs"]
mod fixture;
mod history;
mod items;
mod navigation;
mod reads;
mod service;
mod sources;
mod status;
#[path = "../support/tenancy.rs"]
mod tenancy;
mod workspaces;

#[path = "../../../../tests/support/check.rs"]
mod check;
