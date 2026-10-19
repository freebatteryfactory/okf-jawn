//! A saved or presented View's bindings, read through the ports (core M1.6 and M1b design
//! sections 4.1 and 5): their citations filled for `get_view` and `get_sources` (`cite`), and the
//! resolution a present, a resolve or an export makes (`resolve`).
//!
//! Every function here takes the three ports it reads (`ViewPorts`), not the application
//! service, so ingest's View export handler reads and resolves a View exactly as the
//! application does.
//!
//! A stored citation is remapped or left unlocated when read (verification.json,
//! `application-operations`): a revision a purge rewrote is followed to its replacement; a
//! revision a purge removed, or an item its revision does not hold, keeps the citation as stored
//! with no locations. Locations a citation cannot be given (a section its text no longer has, a
//! conversion record past the read bound) are left out the same way; any other port error fails
//! the read.
//!
//! The resolution reads each binding at its own revision (`Inputs::AsCited`: a candidate, a
//! pinned View, a live View not refreshed) or at the workspace head read once (`Inputs::AtHead`:
//! a live View refreshed). Then the dataset of each binding a drawable chart or a table reads is
//! produced from the source's conversion record (`views::materialize`) and retained and recorded
//! against the binding's item and revision (`views::retain_dataset`). A binding that cannot be
//! read, or whose dataset cannot be produced, is that binding's failure, and every chart that
//! reads it fails alone with that reason: `Invalidated` for a revision a purge removed,
//! `DatasetUnavailable` for an item gone from its revision or the head, a citation of no
//! source's converted text, or a source whose shown text is not the converter's (decision O2:
//! `materialize`'s own sentence says why), `TooLarge` past a bound. No port error is turned
//! into a chart's failure except the conversion record's read bound (`TooLarge`).
//! A live refresh preserves the cited representation: the current original or source text,
//! or an exact retained object still authorized in the same role. An object with no provable
//! current counterpart leaves its old citation intact and is explicitly unavailable; neither
//! an asset's position nor its locator proves that a different object is its successor.
//!
//! Warnings (each located at `/bindings/<i>`): `newer_source` when a binding read as cited names
//! an item whose content at the head differs from the cited revision's (a source card's original,
//! extraction state or shown content; any other item's content digest: SPEC section 10, "Newer source data is an
//! observation"); `source_removed` when the item is not at the head; `dataset_unavailable` for a
//! binding a chart or a table reads whose dataset could not be produced, with the reason (a
//! table has no chart status to carry it); `unknown_unit` for a unit naming no column of the
//! dataset; and, for a refreshed live View, one `live_resolution` naming the head and the
//! instant, since a review never covers live data.

use std::collections::BTreeMap;

use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, Revision, Timestamp};
use okf_jawn_contract::item::ItemDocument;
use okf_jawn_contract::source::SourceReference;
use okf_jawn_contract::views::{
    ChartFailure, ChartResult, ChartStatus, Dataset, ViewBinding, ViewDocument,
};

use super::validate::{ChartCheck, Checked};
use super::{BindingFailure, materialize, retain_dataset};
use crate::jobs::{RecordStore, RevisionMapping};
use crate::portable::item_content_digest;
use crate::reading::{ObjectRole, authorize_object, citation_locations, read_conversion_record};
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

/// Which revision each binding of a resolution is read at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inputs {
    /// Each binding at its own revision.
    AsCited,
    /// Each binding at the workspace head: a live View refreshed.
    AtHead,
}

/// A View's bindings resolved for a present, a resolve or an export; the caller adds the
/// instant and the receipt.
#[derive(Debug, Clone)]
pub struct Resolution {
    /// The View as validated, each binding's locations filled and none materialized; a refreshed
    /// live View's bindings as saved.
    pub view: ViewDocument,
    /// The bindings as read (at the head for a refreshed live View), each with its dataset's
    /// digest when one was produced.
    pub resolved_bindings: Vec<ViewBinding>,
    /// One per `view.chart_refs()`, in that order.
    pub charts: Vec<ChartResult>,
    /// Freshness and dataset notes (the module header).
    pub warnings: Vec<Warning>,
}

/// What reading one citation found.
enum Cited {
    /// The cited item at the cited (or rewritten) revision.
    Read(Box<ItemDocument>),
    /// A purge removed the cited revision.
    Removed,
    /// The revision does not hold the item.
    Missing,
    /// The cited representation has no provably equivalent current representation.
    Unavailable(BindingFailure),
}

/// What a resolution found besides the bindings: each binding's failure, and the warnings.
#[derive(Default)]
struct Found {
    failures: BTreeMap<String, BindingFailure>,
    warnings: Vec<Warning>,
}

impl Found {
    /// The document a binding was read from, or its failure noted: `Invalidated` for a removed
    /// revision, `DatasetUnavailable` for an item its revision (or the head) does not hold.
    fn settle(
        &mut self,
        read: Cited,
        name: &str,
        inputs: Inputs,
        at: &str,
    ) -> Option<Box<ItemDocument>> {
        match read {
            Cited::Read(document) => Some(document),
            Cited::Removed => {
                self.failures.insert(name.to_owned(), invalidated());
                None
            }
            Cited::Missing => {
                if inputs == Inputs::AtHead {
                    self.warnings.push(warning(
                        "source_removed",
                        format!("binding {name}: the cited item is not at the head"),
                        at,
                    ));
                }
                self.failures
                    .insert(name.to_owned(), unavailable(missing(inputs)));
                None
            }
            Cited::Unavailable(failure) => {
                self.warnings.push(warning(
                    "source_unavailable",
                    format!("binding {name}: {}", failure.message),
                    at,
                ));
                self.failures.insert(name.to_owned(), failure);
                None
            }
        }
    }
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

/// Resolve the bindings of `view`, which `checked` is the verdict of (`views::validate`), at
/// `inputs`; `head` is the workspace head read once at `as_of` (the module header).
///
/// # Errors
/// Returns any port error of reading a citation, a conversion record (but its read bound) or the
/// head, or of retaining and recording a dataset.
pub async fn resolve(
    ports: ViewPorts<'_>,
    scope: &StorageScope,
    mut view: ViewDocument,
    checked: &Checked,
    inputs: Inputs,
    head: &Revision,
    as_of: &Timestamp,
) -> Result<Resolution, ApiError> {
    let needed = checked.needed();
    let mut found = Found::default();
    let mut resolved = Vec::with_capacity(view.bindings.len());
    for (index, saved) in view.bindings.iter_mut().enumerate() {
        let at = format!("/bindings/{index}");
        let cited = read_cited(ports, scope, &mut saved.source).await?;
        let mut binding = saved.clone();
        let read = match inputs {
            Inputs::AsCited => {
                if let Cited::Read(document) = &cited {
                    found
                        .warnings
                        .extend(freshness(ports, scope, &binding, document, head, &at).await?);
                }
                cited
            }
            Inputs::AtHead => at_head(ports, scope, &mut binding.source, &cited, head).await?,
        };
        if let Some(document) = found.settle(read, &binding.name, inputs, &at)
            && needed.contains(&binding.name)
        {
            dataset(ports, scope, &mut binding, &document, &at, &mut found).await?;
        }
        if needed.contains(&binding.name)
            && let Some(failure) = found.failures.get(&binding.name)
        {
            let message = format!("binding {}: {}", binding.name, failure.message);
            found
                .warnings
                .push(warning("dataset_unavailable", message, &at));
        }
        resolved.push(binding);
    }
    let Found {
        failures,
        mut warnings,
    } = found;
    if inputs == Inputs::AtHead {
        warnings.push(Warning {
            code: "live_resolution".to_owned(),
            message: format!(
                "resolved at the current head {} at {}; a review does not cover live data",
                head.as_str(),
                as_of.as_str()
            ),
            location: None,
        });
    }
    let charts = view
        .chart_refs()
        .into_iter()
        .zip(&checked.charts)
        .map(|(chart, check)| ChartResult {
            chart,
            status: status(check, &failures),
        })
        .collect();
    Ok(Resolution {
        view,
        resolved_bindings: resolved,
        charts,
        warnings,
    })
}

/// Read one citation: follow a rewritten revision, read the item there and fill the locations.
async fn read_cited(
    ports: ViewPorts<'_>,
    scope: &StorageScope,
    source: &mut SourceReference,
) -> Result<Cited, ApiError> {
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
        }) => return Ok(Cited::Removed),
        None => {}
    }
    let document = match ports
        .versions
        .show(scope, &source.revision, source.item_id)
        .await
    {
        Ok(document) => document,
        Err(error) if error.code == ErrorCode::NotFound => return Ok(Cited::Missing),
        Err(error) => return Err(error),
    };
    source.locations =
        unlocated_on_fault(citation_locations(ports.blobs, scope, &document, source, "").await)?;
    Ok(Cited::Read(Box::new(document)))
}

/// Re-point a live binding only after its represented input has been identified and mapped.
/// An unavailable mapping leaves the old citation intact, rather than claiming it is current.
async fn at_head(
    ports: ViewPorts<'_>,
    scope: &StorageScope,
    source: &mut SourceReference,
    cited: &Cited,
    head: &Revision,
) -> Result<Cited, ApiError> {
    let document = match ports.versions.show(scope, head, source.item_id).await {
        Ok(document) => document,
        Err(error) if error.code == ErrorCode::NotFound => return Ok(Cited::Missing),
        Err(error) => return Err(error),
    };
    let cited = match cited {
        Cited::Read(cited) => cited,
        Cited::Removed => return Ok(Cited::Removed),
        Cited::Missing | Cited::Unavailable(_) => {
            return Ok(Cited::Unavailable(unavailable(
                "the cited representation cannot be identified to refresh it".to_owned(),
            )));
        }
    };
    let digest = match refreshed_digest(ports, scope, source, cited, &document, head).await {
        Ok(digest) => digest,
        Err(error) if error.code == ErrorCode::NotFound => {
            return Ok(Cited::Unavailable(unavailable(
                "the cited representation has no equivalent at the current head".to_owned(),
            )));
        }
        Err(error) if error.code == ErrorCode::TooLarge => {
            return Ok(Cited::Unavailable(BindingFailure {
                reason: ChartFailure::TooLarge,
                message: error.message,
            }));
        }
        Err(error) => return Err(error),
    };
    source.revision = head.clone();
    source.path = document.summary.path.clone();
    source.digest = digest;
    source.locations =
        unlocated_on_fault(citation_locations(ports.blobs, scope, &document, source, "").await)?;
    Ok(Cited::Read(Box::new(document)))
}

/// Refresh the same represented input: original, converted text (including previously pending
/// text with no digest), or the same retained object still authorized in the same role.
async fn refreshed_digest(
    ports: ViewPorts<'_>,
    scope: &StorageScope,
    source: &SourceReference,
    cited: &ItemDocument,
    current: &ItemDocument,
    head: &Revision,
) -> Result<Option<Digest>, ApiError> {
    let no_equivalent = || ApiError::new(ErrorCode::NotFound, "no equivalent representation");
    let Some(digest) = source.digest.as_ref() else {
        return match (&cited.source, &current.source) {
            (Some(_), Some(current)) => Ok(current.extraction.digest.clone()),
            (None, None) => Ok(None),
            _ => Err(no_equivalent()),
        };
    };
    let role = authorize_object(
        ports.versions,
        ports.blobs,
        ports.records,
        scope,
        source.item_id,
        &source.revision,
        digest,
    )
    .await?;
    match role {
        ObjectRole::Original => current
            .source
            .as_ref()
            .map(|appearance| Some(appearance.object.clone()))
            .ok_or_else(no_equivalent),
        ObjectRole::ConversionRecord => current
            .source
            .as_ref()
            .map(|appearance| appearance.extraction.digest.clone())
            .ok_or_else(no_equivalent),
        ObjectRole::Asset(_) | ObjectRole::StructuredExport | ObjectRole::Derived(_) => {
            let current_role = authorize_object(
                ports.versions,
                ports.blobs,
                ports.records,
                scope,
                source.item_id,
                head,
                digest,
            )
            .await?;
            if current_role == role {
                Ok(Some(digest.clone()))
            } else {
                Err(no_equivalent())
            }
        }
    }
}

/// The freshness warning of a binding read as cited: its item gone from the head, or changed
/// there (the module header).
async fn freshness(
    ports: ViewPorts<'_>,
    scope: &StorageScope,
    binding: &ViewBinding,
    cited: &ItemDocument,
    head: &Revision,
    at: &str,
) -> Result<Option<Warning>, ApiError> {
    if &binding.source.revision == head {
        return Ok(None);
    }
    let current = match ports
        .versions
        .show(scope, head, binding.source.item_id)
        .await
    {
        Ok(document) => document,
        Err(error) if error.code == ErrorCode::NotFound => {
            return Ok(Some(warning(
                "source_removed",
                format!(
                    "binding {}: the cited item is not at the head",
                    binding.name
                ),
                at,
            )));
        }
        Err(error) => return Err(error),
    };
    // A correction changes shown content/state while retaining the conversion record's digest.
    let source_changed = match (&cited.source, &current.source) {
        (Some(then), Some(now)) => then.object != now.object || then.extraction != now.extraction,
        (None, None) => false,
        _ => true,
    };
    let changed = source_changed || item_content_digest(cited)? != item_content_digest(&current)?;
    Ok(changed.then(|| {
        warning(
            "newer_source",
            format!(
                "binding {}: the cited item has newer content at the head {}",
                binding.name,
                head.as_str()
            ),
            at,
        )
    }))
}

/// Produce, retain and record the dataset of `binding` (read from `document`), naming its
/// digest; or note why it has none.
async fn dataset(
    ports: ViewPorts<'_>,
    scope: &StorageScope,
    binding: &mut ViewBinding,
    document: &ItemDocument,
    at: &str,
    found: &mut Found,
) -> Result<(), ApiError> {
    match produce(ports, scope, binding, document).await? {
        Ok(dataset) => {
            found.warnings.extend(unknown_units(binding, &dataset, at));
            let object =
                retain_dataset(ports.blobs, ports.records, scope, binding, &dataset).await?;
            binding.materialized = Some(object.digest);
        }
        Err(failure) => {
            found.failures.insert(binding.name.clone(), failure);
        }
    }
    Ok(())
}

/// The dataset of `binding`, from the conversion record of the source text it cites.
async fn produce(
    ports: ViewPorts<'_>,
    scope: &StorageScope,
    binding: &ViewBinding,
    document: &ItemDocument,
) -> Result<Result<Dataset, BindingFailure>, ApiError> {
    let Some(appearance) = document.source.as_ref() else {
        return Ok(Err(unavailable(format!(
            "a dataset is read from a source's converted tables; {} is not a source",
            binding.source.path.as_str()
        ))));
    };
    let Some(record) = appearance.extraction.digest.as_ref() else {
        return Ok(Err(unavailable(
            "the source has no converted text to read a dataset from".to_owned(),
        )));
    };
    if binding.source.digest.as_ref() != Some(record) {
        return Ok(Err(unavailable(
            "the binding does not cite the source's converted text, which a dataset is read from"
                .to_owned(),
        )));
    }
    let record = match read_conversion_record(ports.blobs, scope, record).await {
        Ok(record) => record,
        Err(error) if error.code == ErrorCode::TooLarge => {
            return Ok(Err(BindingFailure {
                reason: ChartFailure::TooLarge,
                message: error.message,
            }));
        }
        Err(error) => return Err(error),
    };
    Ok(materialize(
        binding,
        &record,
        &appearance.extraction,
        &appearance.extraction.warnings,
    ))
}

/// One chart's status: its static refusal, the first failure of a binding it reads (sorted), or
/// ready.
fn status(check: &ChartCheck, failures: &BTreeMap<String, BindingFailure>) -> ChartStatus {
    match check {
        ChartCheck::Refused {
            reason,
            binding,
            message,
            ..
        } => ChartStatus::Failed {
            reason: *reason,
            binding: binding.clone(),
            message: message.clone(),
        },
        ChartCheck::Drawable { bindings } => match bindings
            .iter()
            .find_map(|name| failures.get(name).map(|failure| (name, failure)))
        {
            Some((name, failure)) => ChartStatus::Failed {
                reason: failure.reason,
                binding: Some(name.clone()),
                message: failure.message.clone(),
            },
            None => ChartStatus::Ready {
                bindings: bindings.clone(),
            },
        },
    }
}

/// A warning for each unit naming no column of the dataset.
fn unknown_units(binding: &ViewBinding, dataset: &Dataset, at: &str) -> Vec<Warning> {
    binding
        .units
        .keys()
        .filter(|unit| !dataset.columns.iter().any(|column| &column.name == *unit))
        .map(|unit| {
            warning(
                "unknown_unit",
                format!(
                    "binding {}: the unit of `{unit}` names no column of its dataset",
                    binding.name
                ),
                at,
            )
        })
        .collect()
}

/// Why a binding read at `inputs` has no item.
fn missing(inputs: Inputs) -> String {
    match inputs {
        Inputs::AsCited => "the cited revision does not hold the cited item".to_owned(),
        Inputs::AtHead => "the cited item is not in the workspace's current head".to_owned(),
    }
}

fn invalidated() -> BindingFailure {
    BindingFailure {
        reason: ChartFailure::Invalidated,
        message: "the cited revision was removed by a purge".to_owned(),
    }
}

fn unavailable(message: String) -> BindingFailure {
    BindingFailure {
        reason: ChartFailure::DatasetUnavailable,
        message,
    }
}

fn warning(code: &str, message: String, at: &str) -> Warning {
    Warning {
        code: code.to_owned(),
        message,
        location: Some(at.to_owned()),
    }
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
