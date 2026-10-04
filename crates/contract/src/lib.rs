//! Shared wire contracts for the complete okf-jawn application surface.
//!
//! Rust declarations own vocabulary and structure. Runtime adapters own execution.
/// Shared wire values, pagination, and bounded text locations.
pub mod common;
/// Structured transport-independent application errors.
pub mod error;
/// Authenticated principals and capabilities, never inferred from document labels.
pub mod access;
/// Create, open, list, and export user-organized workspaces.
pub mod workspace;
/// User-defined OKF types, editable notes, and source appearances.
pub mod item;
/// Content-addressed bytes and occurrence-specific provenance.
pub mod source;
/// Revision-bound progressive reading for people and multimodal agents.
pub mod read;
/// Search and link navigation over retained, authorized workspace content.
pub mod search;
/// Git-backed snapshots, comparisons, attribution, and restore operations.
pub mod history;
/// Suggested changes are separate from both review and acceptance.
pub mod proposal;
/// Review evidence covers exact content, never all future edits.
pub mod review;
/// Resumable import identities, conversion jobs, and durable progress.
pub mod import;
/// Human-readable naming conventions with preview and collision semantics.
pub mod conventions;
/// Diagnostics are actionable observations, never a blanket trust score.
pub mod attention;
/// Persistent visual artifacts bind presentation to identifiable source data.
pub mod views;
/// Durable retrieval records and live notifications have separate meanings.
pub mod events;
/// Liveness and dependency readiness do not claim product qualification.
pub mod health;
/// Validated content and workspace identities.
pub mod identity;
/// Canonical operation declarations and their adapter projections.
pub mod operations;
/// Operation names, labels, and declared policy.
pub mod metadata;

/// Streaming, discovery, and session transport declarations.
pub mod transport;

/// Human-facing command aliases; canonical operations remain unchanged.
pub mod labels;
