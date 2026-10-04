//! Domain-facing operation ports and shared access checks.
//!
//! Adapters supply implementations; this crate never fabricates application success.

/// Capability checks independent of protocol hints.
pub mod access;
/// Production application service composed over injected ports.
pub mod application;
/// JSON-boundary dispatch into typed application methods.
pub mod dispatch;
/// Complete typed operation port for parallel implementation lanes.
pub mod ports;

/// Bounded conversion worker interface.
pub mod conversion;
/// Durable job and review record interfaces.
pub mod jobs;
/// Rebuildable search, link, and graph index interface.
pub mod search;
/// Scoped byte and version persistence interfaces.
pub mod storage;
