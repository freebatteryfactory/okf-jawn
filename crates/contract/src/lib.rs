//! Shared wire contracts for the complete okf-jawn application surface.
//!
//! Rust declarations own vocabulary and structure. Runtime adapters own execution.
/// Authenticated principals and capabilities, never inferred from document labels.
pub mod access;
/// Diagnostics are actionable observations, never a blanket trust score.
pub mod attention;
/// Shared wire values, pagination, and bounded text locations.
pub mod common;
/// Human-readable naming conventions with preview and collision semantics.
pub mod conventions;
/// Structured transport-independent application errors.
pub mod error;
/// Durable retrieval records and live notifications have separate meanings.
pub mod events;
/// Liveness and dependency readiness do not claim product qualification.
pub mod health;
/// Git-backed snapshots, comparisons, attribution, and restore operations.
pub mod history;
/// Validated content and workspace identities.
pub mod identity;
/// Resumable import identities, conversion jobs, and durable progress.
pub mod import;
/// User-defined OKF types, editable notes, and source appearances.
pub mod item;
/// Operation names, aliases, hints and declared policy.
pub mod metadata;
/// Canonical operation declarations and their adapter projections.
pub mod operations;
/// Suggested changes are separate from both review and acceptance.
pub mod proposal;
/// Revision-bound progressive reading for people and multimodal agents.
pub mod read;
/// Review evidence covers exact content, never all future edits.
pub mod review;
/// Authorization targets, retry identity and schema-inexpressible rules of every request type.
pub mod scope;
/// Search and link navigation over retained, authorized workspace content.
pub mod search;
/// Content-addressed bytes and occurrence-specific provenance.
pub mod source;
/// Persistent visual artifacts bind presentation to identifiable source data.
pub mod views;
/// Create, open, list, and export user-organized workspaces.
pub mod workspace;

/// Streaming, discovery, and session transport declarations.
pub mod transport;
