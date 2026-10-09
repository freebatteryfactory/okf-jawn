//! Domain-facing operation ports and shared access checks.
//!
//! Adapters supply implementations; this crate never fabricates application success.

/// Capability checks independent of protocol hints.
pub mod access;
/// Production application service composed over injected ports.
pub mod application;
/// Session-bound confirmation challenges.
pub mod confirmations;
/// Per-request authorization context and grants.
pub mod context;
/// Connector credentials, sessions, and installation identity.
pub mod credentials;
/// JSON-boundary dispatch into typed application methods.
pub mod dispatch;
/// Per-editor draft persistence.
pub mod drafts;
/// The bound on text an error message quotes.
pub mod echo;
/// Resumable workspace notification log.
pub mod events;
/// Complete typed operation port for parallel implementation lanes.
pub mod ports;

/// Bounded conversion worker interface.
pub mod conversion;
/// The server-owned application header of item files.
pub mod items;
/// Durable job specifications, leases, reviews, receipts and artifact records.
pub mod jobs;
/// Durable mutation ledger.
pub mod mutations;
/// Proposal and comment persistence.
pub mod proposals;
/// Configured dependency readiness probe.
pub mod readiness;
/// Which retained objects a citation may open.
pub mod reading;
/// Sandbox-origin capability tokens.
pub mod sandbox;
/// Rebuildable search, link, and graph index interface.
pub mod search;
/// Blob, version and workspace-catalog ports with their core parameter types.
pub mod storage;
/// Schema validation of values read back from storage.
pub mod stored;
/// Authenticated upload occurrence slots.
pub mod uploads;
/// The producer of a View binding's materialized dataset.
pub mod views;
