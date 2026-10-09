//! Storage implementation lane: Git content, immutable blobs, and durable application records.
//!
//! With the `runtime` feature, `Storage::open` takes a data directory and, in this order, takes
//! the single-writer lock, refuses data newer than this build without changing it, clears what
//! a crash left in staging, and migrates the records and index databases (backing each up first
//! when it is upgraded). Only then does any store exist. Each store implements one core port.
//! Without the feature the crate is empty, so code generation links no native library.

#[cfg(feature = "runtime")]
pub use git::GitVersions;
#[cfg(feature = "runtime")]
pub use storage::Storage;

#[cfg(feature = "runtime")]
#[macro_use]
mod macros;

#[cfg(feature = "runtime")]
pub mod access;
#[cfg(feature = "runtime")]
pub mod blobs;
#[cfg(feature = "runtime")]
pub mod catalog;
#[cfg(feature = "runtime")]
pub mod confirmations;
#[cfg(feature = "runtime")]
pub mod credentials;
#[cfg(feature = "runtime")]
pub mod data;
#[cfg(feature = "runtime")]
mod db;
#[cfg(feature = "runtime")]
pub mod drafts;
#[cfg(feature = "runtime")]
pub mod events;
#[cfg(feature = "runtime")]
pub mod format;
#[cfg(feature = "runtime")]
mod git;
#[cfg(feature = "runtime")]
pub mod mutations;
#[cfg(feature = "runtime")]
pub mod proposals;
#[cfg(feature = "runtime")]
pub mod readiness;
#[cfg(feature = "runtime")]
pub mod records;
#[cfg(feature = "runtime")]
pub mod sandbox;
#[cfg(feature = "runtime")]
pub mod schema;
#[cfg(feature = "runtime")]
mod seam;
#[cfg(feature = "runtime")]
pub mod search;
#[cfg(feature = "runtime")]
mod storage;
#[cfg(feature = "runtime")]
pub mod uploads;
