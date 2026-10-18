//! A saved View's bindings, read through the ports (core M1.6 design section 4.1): their
//! citations filled for `get_view` and `get_sources` (`cite`).
//!
//! Every function here takes the three ports it reads (`ViewPorts`), not the application
//! service, so ingest's View export handler reads a View exactly as the application does.
//!
//! A stored citation is remapped or left unlocated when read (verification.json,
//! `application-operations`): a revision a purge rewrote is followed to its replacement; a
//! revision a purge removed, or an item its revision does not hold, keeps the citation as stored
//! with no locations. Locations a citation cannot be given (a section its text no longer has, a
//! conversion record past the read bound) are left out the same way; any other port error fails
//! the read.

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::source::SourceReference;
use okf_jawn_contract::views::ViewDocument;

use crate::jobs::{RecordStore, RevisionMapping};
use crate::reading::citation_locations;
use crate::storage::{BlobStore, StorageScope, VersionStore};

/// The ports a View is read and resolved through.
#[derive(Clone, Copy)]
pub struct ViewPorts<'a> {
    /// Versioned content.
    pub versions: &'a dyn VersionStore,
    /// Retained bytes: conversion records and datasets.
    pub blobs: &'a dyn BlobStore,
    /// Revision maps and derived-object records.
    pub records: &'a dyn RecordStore,
}

/// Fill each binding's locations as a read answers a citation, following a revision a purge
/// rewrote (the module header).
///
/// # Errors
/// Returns any port error other than the ones the module header names as leaving a citation
/// unlocated.
pub async fn cite(
    ports: ViewPorts<'_>,
    scope: &StorageScope,
    view: &mut ViewDocument,
) -> Result<(), ApiError> {
    for binding in &mut view.bindings {
        read_cited(ports, scope, &mut binding.source).await?;
    }
    Ok(())
}

/// Read one citation: follow a rewritten revision, read the item there and fill the locations.
async fn read_cited(
    ports: ViewPorts<'_>,
    scope: &StorageScope,
    source: &mut SourceReference,
) -> Result<(), ApiError> {
    match ports
        .records
        .revision_mapping(scope, &source.revision)
        .await?
    {
        Some(RevisionMapping {
            replacement: Some(rewritten),
            ..
        }) => source.revision = rewritten,
        Some(RevisionMapping {
            replacement: None, ..
        }) => return Ok(()),
        None => {}
    }
    let document = match ports
        .versions
        .show(scope, &source.revision, source.item_id)
        .await
    {
        Ok(document) => document,
        Err(error) if error.code == ErrorCode::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    source.locations =
        unlocated_on_fault(citation_locations(ports.blobs, scope, &document, source, "").await)?;
    Ok(())
}

/// The locations computed, or none when the citation cannot be located (a section the text does
/// not have, a record past the read bound); any other error.
fn unlocated_on_fault<T: Default>(computed: Result<T, ApiError>) -> Result<T, ApiError> {
    match computed {
        Ok(locations) => Ok(locations),
        Err(error)
            if matches!(
                error.code,
                ErrorCode::NotFound | ErrorCode::InvalidInput | ErrorCode::TooLarge
            ) =>
        {
            Ok(T::default())
        }
        Err(error) => Err(error),
    }
}
