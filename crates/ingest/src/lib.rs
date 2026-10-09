//! Ingestion: bounded document conversion and occurrence-preserving imports.
//!
//! Everything here is behind the `runtime` feature, which links the conversion library. Code
//! generation builds the crate without it and gets an empty library. No converter or queue is
//! replaced by a success stub.
//!
//! - `export`, `locate` and `glyphs` are pure rules over the docling JSON export: where each
//!   item is (`ingest-locates-unlocated-items`) and which text a font could not decode
//!   (`ingest-flags-undecodable-text`).
//! - `outline` and `record` assemble the whole-document `ConversionRecord` from its windows.
//! - `cap` is the per-platform memory cap of the conversion child
//!   (`converter-worker-memory-ceiling`).
//! - `protocol`, `child` and `converter` are the docling adapter: the request and reply between
//!   supervisor and child, the child's body, and the `Converter` that runs it under the cap.
//! - `runtime` is the Tokio + `RecordStore` job runtime, and `handler` runs every `JobSpec`.

#[cfg(feature = "runtime")]
pub mod cap;
#[cfg(feature = "runtime")]
pub mod child;
#[cfg(feature = "runtime")]
pub mod converter;
#[cfg(feature = "runtime")]
pub mod export;
#[cfg(feature = "runtime")]
pub mod glyphs;
#[cfg(feature = "runtime")]
pub mod handler;
#[cfg(feature = "runtime")]
pub mod locate;
#[cfg(feature = "runtime")]
pub mod outline;
#[cfg(feature = "runtime")]
pub mod protocol;
#[cfg(feature = "runtime")]
pub mod record;
#[cfg(feature = "runtime")]
pub mod runtime;
