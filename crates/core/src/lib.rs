//! Domain-facing operation ports and shared access checks.
//!
//! Adapters supply implementations; this crate never fabricates application success.

/// Capability checks independent of protocol hints.
pub mod access;
/// JSON-boundary dispatch into typed application methods.
pub mod dispatch;
/// Complete typed operation port for parallel implementation lanes.
pub mod ports;

/// Scoped byte and version persistence interfaces.
pub mod storage;
/// Durable job and review record interfaces.
pub mod jobs;
/// Bounded conversion worker interface.
pub mod conversion;
