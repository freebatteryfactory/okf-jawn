//! Attention, the workspace's advisory observations (SPEC sections 1 and 11), and the index
//! rebuild job.
//!
//! `get_attention` resolves `at` once (a continuation reads the revision its first page
//! resolved, which its cursor carries: `shared::resolve_page`) and scans the workspace at that
//! revision in one pass. Every item of the tree at the revision is read (`VersionStore::list`
//! from the root, every page of every folder, then `VersionStore::show`), except archived ones:
//! an archived item is hidden from current listings, and none of its observations is shown. The
//! observations built, each from a signal the stores already hold:
//!
//! - `BrokenLink`: the item's outgoing links at the revision (`SearchIndex::links`, built on
//!   demand as `get_links` builds it, `navigation::indexed`) whose target the index resolved to
//!   no item (`Link::to` absent) and that name something inside the workspace: a link okf-core
//!   classifies as absolute or relative and resolves to at least one concept id
//!   (`okf_core::Link::resolve_all`). An external link, an anchor and a link to a folder name no
//!   item and are not broken. A link to a file that is not Markdown (`conformance::names_markdown`
//!   is false) is broken only when the revision holds no file at any reading of it
//!   (`VersionStore::read_file` answers `NotFound`), as the conformance check judges one. One row
//!   an item, listing each broken link with its body line; its selection is that line when the
//!   item has exactly one. Action `get_links`.
//! - `Stale`: okf-core's `trust::is_stale_on` of the item's `stale_after` property on the UTC day
//!   of the revision's own commit (`VersionStore::log` from the revision, its first entry), so a
//!   revision's answer is the same whenever it is asked. A `stale_after` that is not a string
//!   okf-core reads as an instant with a time and an offset is never stale, as okf-core rules.
//!   Action `get_item`.
//! - `Unreviewed`: an item whose content at the revision an agent wrote, with no recorded review
//!   whose coverage there is `current` (`reviews::reviews_at`, the coverage `list_reviews`
//!   answers). An agent wrote it when its OKF `generated.by` names an agent actor
//!   (`okf_core::Actor`, the `<producer>/<version>` form), when its shown text is an agent's
//!   accepted supplied text (`TextOrigin::SuppliedByAgent`), or when its content digest
//!   (`portable::item_content_digest`) is the one an accepted content proposal opened by an
//!   agent's delegated route (`AccessRoute::McpDelegation`) gave it, read at that proposal's
//!   candidate. Every accepted proposal of the workspace is listed for that, and each of its
//!   creations and edits read at its candidate once; a candidate the store no longer holds (its
//!   `NotFound`, after a purge) gives nothing, and every other error fails the request. A
//!   person's edit after an agent's leaves content no agent wrote alone, and is not counted.
//!   Action `create_review`.
//! - `Duplicate`: original bytes (`SourceAppearance::object`) under more than one item the scan
//!   reads; one row for each such item, naming the others. Action `get_item`.
//! - `MissingDescription`: an item whose `description` is empty or blank. Action `get_item`.
//! - `Unprocessed`: a source whose converter status is partial, failed or unsupported
//!   (`ExtractionStatus::is_unprocessed`), whether or not text was supplied; its selection is the
//!   pages not converted when they are one range. Action `redigest_item`.
//! - `ExtractionWarnings`: a completed source whose conversion recorded warnings. Action
//!   `read_item`.
//! - `InstructionLike`: the current local English rule set scans the shown body, including
//!   digest, corrected and supplied text. One row holds at most 32 original passages with the
//!   same citation locations as `read_item`, plus the count omitted. Action `read_item`.
//! - `UploadsWaiting`: one row for the workspace when any upload is complete and not imported
//!   (`UploadStore::list` in `waiting`, every page), with their count and total retained bytes.
//!   Action `list_uploads`.
//!
//! Remaining producers, said rather than hidden: `NewerSource`, `Invalidated` and
//! `UnparseableHeader` remain separate producer-backed work; this handler does not emit them.
//!
//! Order and pages: the rows are ordered by kind in the order `AttentionKind` declares them,
//! then by the item's path, then by its identity (a row about no item has an empty path and
//! sorts first in its kind). The cursor names the last row's (kind, path, item id); the next
//! page lists the rows after it in that order, at the same revision. So while the revision is
//! fixed pages neither repeat nor skip a row; a row that comes or goes between two pages for a
//! reason outside the revision (a review recorded, an upload imported) changes only itself. The
//! cursor is this service's (`shared::resolve_page`), unsigned, and bound to the operation and
//! the revision only.
//!
//! What it costs: every page scans the whole workspace again, so a page costs one `show` and one
//! link query per item at the revision, a listing per folder, a `read_file` per link to a file
//! that is not Markdown and has no item, a `list_reviews` per item an agent wrote, a read per
//! creation or edit of every accepted agent proposal, and every page of the waiting uploads: it
//! is O(items + accepted proposal changes + waiting uploads) per page, not O(items x
//! revisions). Nothing is cached between pages.
//!
//! An empty answer says only that these checks found nothing: SPEC section 11, "no clean scan
//! becomes a safe-for-AI certificate". The answer is a listing of summaries, as `list_items` is,
//! and records no receipt.
//!
//! `rebuild_index` queues `JobSpec::RebuildIndex` as every job-starting operation does
//! (`workspaces::start_job`): ingest runs it over `SearchIndex::rebuild`, which never touches jobs,
//! reviews or receipts. Dispatch lets it run while the workspace is archived (contract ruling
//! 25), and its table permission is `admin`.

use std::collections::BTreeMap;

use okf_core::{Actor, ActorKind, ConceptId, LinkKind};
use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::attention::{
    AttentionItem, AttentionKind, GetAttentionRequest, GetAttentionResponse, InstructionEvidence,
    InstructionFinding, RebuildIndexRequest,
};
use okf_jawn_contract::common::{PageRange, TextRange};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{
    ConversionOutcome, ExtractionStatus, ExtractionWarning, FailureReason, TextOrigin,
};
use okf_jawn_contract::identity::{Digest, ItemId, Revision, WorkspacePath};
use okf_jawn_contract::import::{Job, UploadListState};
use okf_jawn_contract::item::ItemDocument;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::proposal::{Change, ProposalKind, ProposalStatus};
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::review::ReviewCoverage;
use okf_jawn_contract::search::{Link, LinkDirection};
use serde::{Deserialize, Serialize};

use super::ApplicationService;
use super::navigation::indexed;
use super::reviews::reviews_at;
use super::shared::{invalid, next_page, resolve_page, workspace_scope};
use super::sources::{citation, record_of};
use super::workspaces::start_job;
use crate::conformance::names_markdown;
use crate::context::OperationContext;
use crate::instruction_like::{Detector, RULE_SET_VERSION};
use crate::jobs::{JobScope, JobSpec};
use crate::portable::item_content_digest;
use crate::proposals::ProposalFilter;
use crate::search::LinkQuery;
use crate::storage::{LogQuery, Page, StorageScope};
use crate::uploads::UploadQuery;

/// The accepted agent proposals' content: for each item, the content digests an agent's
/// proposal gave it, each with the proposer.
type AgentContent = BTreeMap<ItemId, Vec<(Digest, String)>>;

/// Where a row stands in the scan's order; a page's cursor carries the last row's, as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Key {
    /// The row's kind.
    kind: AttentionKind,
    /// The item's path at the revision; empty for a row about no item.
    path: String,
    /// The item; `None` for a row about no item.
    item: Option<ItemId>,
}

/// One observation and its place in the order.
struct Row {
    /// Its place.
    key: Key,
    /// The observation as the response lists it.
    item: AttentionItem,
}

/// Entries asked per page of a folder listing, a link listing, a proposal listing and the
/// waiting uploads; the stores may answer fewer.
const SCAN_PAGE: u16 = 200;

/// Most other paths a `Duplicate` row names; it counts the rest.
const DUPLICATES_NAMED: usize = 10;

/// The request field a cursor is refused on.
const PAGE_CURSOR: &str = "/page/cursor";

/// The workspace's advisory observations at one resolved revision, a page at a time.
///
/// # Errors
/// Returns `InvalidInput` on `/page/cursor` for a cursor this operation did not issue or that
/// continues another revision than `at` names; the typed `NotFound` of an invalidated revision;
/// the index's `Unavailable` for a revision outside the window an index is built for on demand;
/// or any port error.
pub(super) async fn get_attention(
    service: &ApplicationService,
    context: &OperationContext,
    request: GetAttentionRequest,
) -> Result<GetAttentionResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let (revision, page, _) = resolve_page(
        service,
        &scope,
        context.operation,
        &request.at,
        request.page,
    )
    .await?;
    let after = page
        .cursor
        .as_deref()
        .map(|text| {
            serde_json::from_str::<Key>(text)
                .map_err(|_| invalid("the cursor is not one this operation issues", PAGE_CURSOR))
        })
        .transpose()?;
    let mut rows = scan(service, &scope, &revision).await?;
    rows.sort_by(|left, right| order(&left.key).cmp(&order(&right.key)));
    let limit = usize::from(page.limit.max(1));
    let mut remaining = rows.into_iter().filter(|row| {
        after
            .as_ref()
            .is_none_or(|after| order(&row.key) > order(after))
    });
    let listed: Vec<Row> = remaining.by_ref().take(limit).collect();
    let next = match (remaining.next(), listed.last()) {
        (Some(_), Some(last)) => Some(
            serde_json::to_string(&last.key)
                .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?,
        ),
        _ => None,
    };
    Ok(GetAttentionResponse {
        next_cursor: next_page(context.operation, &revision, next, None)?,
        revision,
        items: listed.into_iter().map(|row| row.item).collect(),
    })
}

/// Queue a rebuild of the workspace's derived search and link index.
///
/// # Errors
/// Returns any port error.
pub(super) async fn rebuild_index(
    service: &ApplicationService,
    context: &OperationContext,
    _request: RebuildIndexRequest,
) -> Result<Job, ApiError> {
    let scope = workspace_scope(context)?;
    start_job(
        service,
        context,
        JobScope::Workspace(scope),
        JobSpec::RebuildIndex,
    )
    .await
}

/// Every observation of the module header at `revision`, unordered.
///
/// # Errors
/// Returns any port error.
async fn scan(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
) -> Result<Vec<Row>, ApiError> {
    let documents = documents_at(service, scope, revision).await?;
    let agents = agent_content(service, scope).await?;
    let detector = Detector::new()?;
    let mut day = None;
    let mut rows = Vec::new();
    let mut by_object: BTreeMap<Digest, Vec<&ItemDocument>> = BTreeMap::new();
    for document in &documents {
        rows.extend(instruction_like(service, scope, revision, document, &detector).await?);
        if let Some(message) = broken_links(service, scope, revision, document).await? {
            rows.push(message);
        }
        if let Some(message) = stale(service, scope, revision, document, &mut day).await? {
            rows.push(message);
        }
        if let Some(writer) = writer_of(document, &agents)? {
            let shown = item_content_digest(document)?;
            let covered = reviews_at(service, scope, document.summary.id, &shown)
                .await?
                .iter()
                .any(|review| review.coverage == ReviewCoverage::Current);
            if !covered {
                rows.push(row(
                    AttentionKind::Unreviewed,
                    document,
                    None,
                    format!(
                        "Written by an AI tool ({writer}) and not covered by a review of this \
                         content"
                    ),
                    OperationName::CreateReview,
                ));
            }
        }
        if document.summary.description.trim().is_empty() {
            rows.push(row(
                AttentionKind::MissingDescription,
                document,
                None,
                "No description: lists and search show nothing to say what this is".to_owned(),
                OperationName::GetItem,
            ));
        }
        if let Some(source) = &document.source {
            rows.extend(extraction(document));
            by_object
                .entry(source.object.clone())
                .or_default()
                .push(document);
        }
    }
    for group in by_object.values().filter(|group| group.len() > 1) {
        for document in group {
            rows.push(duplicate(document, group));
        }
    }
    rows.extend(uploads_waiting(service, scope).await?);
    Ok(rows)
}

/// Current advisory evidence over the unchanged shown body, located by the ordinary read path.
/// Only a matched source requires its conversion record; an empty scan emits no observation.
async fn instruction_like(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
    document: &ItemDocument,
    detector: &Detector,
) -> Result<Option<Row>, ApiError> {
    let scan = detector.scan(&document.body)?;
    if scan.findings.is_empty() {
        return Ok(None);
    }
    let record = record_of(service, scope, document.source.as_ref()).await?;
    let findings = scan
        .findings
        .into_iter()
        .map(|finding| InstructionFinding {
            rule_id: finding.rule,
            rule_set_version: RULE_SET_VERSION.to_owned(),
            passage: finding.passage,
            source: citation(
                scope,
                revision,
                document,
                record.as_ref(),
                Selection::Lines {
                    range: finding.lines,
                },
            ),
        })
        .collect();
    let mut observation = row(
        AttentionKind::InstructionLike,
        document,
        None,
        "This text may contain instructions aimed at an AI assistant. English-only rules; \
         findings are advisory and false positives are possible."
            .to_owned(),
        OperationName::ReadItem,
    );
    observation.item.instruction_like = Some(InstructionEvidence {
        language: "en".to_owned(),
        findings,
        omitted_findings: scan.omitted_findings,
    });
    Ok(Some(observation))
}

/// Every item of the tree at `revision` that is not archived, read whole.
///
/// # Errors
/// Returns any port error.
async fn documents_at(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
) -> Result<Vec<ItemDocument>, ApiError> {
    let versions = &service.ports().versions;
    let mut folders: Vec<Option<WorkspacePath>> = vec![None];
    let mut documents = Vec::new();
    while let Some(folder) = folders.pop() {
        let mut cursor = None;
        loop {
            let listing = versions
                .list(
                    scope,
                    revision,
                    folder.as_ref(),
                    Page {
                        cursor: cursor.take(),
                        limit: SCAN_PAGE,
                    },
                )
                .await?;
            for summary in listing.items.iter().filter(|summary| !summary.archived) {
                documents.push(versions.show(scope, revision, summary.id).await?);
            }
            folders.extend(listing.folders.into_iter().map(Some));
            match listing.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
    }
    Ok(documents)
}

/// The `BrokenLink` row of `document`, when any of its links leads nowhere.
///
/// # Errors
/// Returns any port error.
async fn broken_links(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
    document: &ItemDocument,
) -> Result<Option<Row>, ApiError> {
    let index = &service.ports().search;
    let mut broken: Vec<Link> = Vec::new();
    let mut cursor = None;
    loop {
        let query = LinkQuery {
            revision: revision.clone(),
            item_id: document.summary.id,
            direction: LinkDirection::Outgoing,
            page: Page {
                cursor: cursor.take(),
                limit: SCAN_PAGE,
            },
        };
        let page = indexed(service, scope, revision, || {
            index.links(scope, query.clone())
        })
        .await?;
        for link in page.links {
            if link.to.is_none()
                && leads_nowhere(service, scope, revision, &document.summary.path, &link).await?
            {
                broken.push(link);
            }
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    if broken.is_empty() {
        return Ok(None);
    }
    let selection = match broken.as_slice() {
        [only] => Some(Selection::Lines {
            range: TextRange {
                start: only.line,
                end: only.line,
            },
        }),
        _ => None,
    };
    let listed: Vec<String> = broken
        .iter()
        .map(|link| format!("line {}: {}", link.line, link.to_path))
        .collect();
    let message = if broken.len() == 1 {
        format!("A link leads nowhere ({})", listed.join("; "))
    } else {
        format!(
            "{} links lead nowhere ({})",
            broken.len(),
            listed.join("; ")
        )
    };
    Ok(Some(row(
        AttentionKind::BrokenLink,
        document,
        selection,
        message,
        OperationName::GetLinks,
    )))
}

/// Whether `link`, which the index resolved to no item, names something inside the workspace
/// that the revision does not hold (the module header's rule).
///
/// # Errors
/// Returns any error of reading a linked file other than its `NotFound`.
async fn leads_nowhere(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
    path: &WorkspacePath,
    link: &Link,
) -> Result<bool, ApiError> {
    let target = okf_core::Link {
        text: String::new(),
        target: link.to_path.clone(),
        kind: okf_core::Link::classify(&link.to_path),
    };
    if !matches!(target.kind, LinkKind::Absolute | LinkKind::Relative) {
        return Ok(false);
    }
    // One `.md` comes off, as the index takes the item's concept id.
    let Ok(source) = ConceptId::parse(path.as_str().strip_suffix(".md").unwrap_or(path.as_str()))
    else {
        return Ok(false);
    };
    let readings = target.resolve_all(&source);
    if readings.is_empty() {
        return Ok(false);
    }
    if names_markdown(&link.to_path) {
        return Ok(true);
    }
    for reading in readings {
        let Ok(file) = WorkspacePath::try_from(reading.segments().join("/")) else {
            continue;
        };
        match service
            .ports()
            .versions
            .read_file(scope, revision, &file)
            .await
        {
            Ok(_) => return Ok(false),
            Err(error) if error.code == ErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

/// The `Stale` row of `document`, judged on the UTC day of the revision's commit, which is read
/// once (`day`) and only when an item has a `stale_after` okf-core reads.
///
/// # Errors
/// Returns `Internal` when the revision has no commit to date it by, or any port error.
async fn stale(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
    document: &ItemDocument,
    day: &mut Option<okf_core::Date>,
) -> Result<Option<Row>, ApiError> {
    let Some(written) = document
        .properties
        .get("stale_after")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(None);
    };
    let Some(after) = okf_core::DateTime::parse(written) else {
        return Ok(None);
    };
    let today = match day {
        Some(today) => *today,
        None => {
            let found = revision_day(service, scope, revision).await?;
            *day = Some(found);
            found
        }
    };
    if !okf_core::trust::is_stale_on(Some(after), today) {
        return Ok(None);
    }
    Ok(Some(row(
        AttentionKind::Stale,
        document,
        None,
        format!(
            "Due for a check: it is to be checked again after {written}, which had passed on {today}, the day of this revision"
        ),
        OperationName::GetItem,
    )))
}

/// The UTC day of `revision`'s own commit.
///
/// # Errors
/// Returns `Internal` when the store answers no commit for the revision, or any port error.
async fn revision_day(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
) -> Result<okf_core::Date, ApiError> {
    let history = service
        .ports()
        .versions
        .log(
            scope,
            LogQuery {
                tip: revision.clone(),
                item_id: None,
                page: Page {
                    cursor: None,
                    limit: 1,
                },
            },
        )
        .await?;
    history
        .commits
        .first()
        .and_then(|commit| commit.committed_at.as_str().get(..10))
        .and_then(okf_core::Date::parse)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::Internal,
                "the store answered no commit to date the revision by",
            )
        })
}

/// Which agent wrote `document`'s content, as the first of the module header's signals that
/// names one names it.
///
/// # Errors
/// Returns `Internal` when the content cannot be digested.
fn writer_of(document: &ItemDocument, agents: &AgentContent) -> Result<Option<String>, ApiError> {
    let generated = document
        .properties
        .get("generated")
        .and_then(|generated| generated.get("by"))
        .and_then(serde_json::Value::as_str)
        .map(Actor::parse)
        .filter(|actor| actor.kind() == ActorKind::Agent);
    if let Some(actor) = generated {
        return Ok(Some(actor.as_str().to_owned()));
    }
    let supplied = document.source.as_ref().and_then(|source| {
        (source.extraction.text_origin == TextOrigin::SuppliedByAgent)
            .then_some(source.extraction.supplied.as_ref())
            .flatten()
    });
    if let Some(supplied) = supplied {
        return Ok(Some(supplied.supplied_by.clone()));
    }
    let Some(written) = agents.get(&document.summary.id) else {
        return Ok(None);
    };
    let content = item_content_digest(document)?;
    Ok(written
        .iter()
        .find(|(digest, _)| *digest == content)
        .map(|(_, who)| who.clone()))
}

/// The content digest each accepted content proposal of an agent's delegated route gave each
/// item it created or edited, read at the proposal's candidate.
///
/// # Errors
/// Returns any port error but the `NotFound` of a candidate the store no longer holds.
async fn agent_content(
    service: &ApplicationService,
    scope: &StorageScope,
) -> Result<AgentContent, ApiError> {
    let ports = service.ports();
    let mut written: AgentContent = BTreeMap::new();
    let mut cursor = None;
    loop {
        let page = ports
            .proposals
            .list(
                scope,
                ProposalFilter {
                    status: Some(ProposalStatus::Accepted),
                    page: Page {
                        cursor: cursor.take(),
                        limit: SCAN_PAGE,
                    },
                },
            )
            .await?;
        for proposal in page.items.iter().filter(|proposal| {
            proposal.created_via == AccessRoute::McpDelegation
                && proposal.kind == ProposalKind::Content
        }) {
            let candidate = &proposal.proposal_revision;
            for change in &proposal.changes {
                let item = match change {
                    Change::Create { path, .. } => {
                        match gone(ports.versions.item_at_path(scope, candidate, path).await)? {
                            Some(Some(found)) => found.id,
                            Some(None) | None => continue,
                        }
                    }
                    Change::Edit { item_id, .. } => *item_id,
                    Change::Move { .. }
                    | Change::Archive { .. }
                    | Change::SupplyExtraction { .. } => continue,
                };
                let Some(document) = gone(ports.versions.show(scope, candidate, item).await)?
                else {
                    continue;
                };
                written
                    .entry(item)
                    .or_default()
                    .push((item_content_digest(&document)?, proposal.created_by.clone()));
            }
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(written),
        }
    }
}

/// A read of a proposal's candidate: `None` for the store's `NotFound` (the candidate is gone),
/// every other error as it is.
///
/// # Errors
/// Returns every error of `read` but `NotFound`.
fn gone<T>(read: Result<T, ApiError>) -> Result<Option<T>, ApiError> {
    match read {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code == ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The `Unprocessed` or `ExtractionWarnings` row of a source, when it has one.
fn extraction(document: &ItemDocument) -> Option<Row> {
    let source = document.source.as_ref()?;
    let extraction = &source.extraction;
    if let Some((message, selection)) = unprocessed(&extraction.outcome) {
        return Some(row(
            AttentionKind::Unprocessed,
            document,
            selection,
            message,
            OperationName::RedigestItem,
        ));
    }
    if extraction.outcome.status() == ExtractionStatus::Completed && !extraction.warnings.is_empty()
    {
        let found: Vec<String> = extraction.warnings.iter().map(warning_words).collect();
        return Some(row(
            AttentionKind::ExtractionWarnings,
            document,
            None,
            format!("Converted with warnings: {}", found.join("; ")),
            OperationName::ReadItem,
        ));
    }
    None
}

/// What an unprocessed outcome (partial, failed or unsupported) says, and the pages not
/// converted when they are one range; `None` for an outcome that is not unprocessed.
fn unprocessed(outcome: &ConversionOutcome) -> Option<(String, Option<Selection>)> {
    match outcome {
        ConversionOutcome::Partial {
            coverage: Some(coverage),
            ..
        } if !coverage.not_converted.is_empty() => {
            let selection = match coverage.not_converted.as_slice() {
                [only] => Some(Selection::Pages {
                    range: only.clone(),
                }),
                _ => None,
            };
            let pages: Vec<String> = coverage.not_converted.iter().map(pages_words).collect();
            Some((
                format!(
                    "Converted in part: pages {} of {} were not converted; the original is kept",
                    pages.join(", "),
                    coverage.page_count
                ),
                selection,
            ))
        }
        ConversionOutcome::Partial { .. } => Some((
            "Converted in part: the converter recorded what is missing; the original is kept"
                .to_owned(),
            None,
        )),
        ConversionOutcome::Failed { reason, .. } => Some((
            format!(
                "Not converted: {}; the original is kept",
                failure_words(reason)
            ),
            None,
        )),
        ConversionOutcome::Unsupported => Some((
            "Not converted: no converter handles this format; the original is kept".to_owned(),
            None,
        )),
        ConversionOutcome::Pending | ConversionOutcome::Completed => None,
    }
}

/// A page range in words: `3` or `3-5`.
fn pages_words(range: &PageRange) -> String {
    if range.start == range.end {
        range.start.to_string()
    } else {
        format!("{}-{}", range.start, range.end)
    }
}

/// Why a conversion produced no document, in words.
fn failure_words(reason: &FailureReason) -> String {
    match reason {
        FailureReason::Damaged => "the file could not be read as what its name says".to_owned(),
        FailureReason::MemoryLimit { limit_bytes } => {
            format!("the converter reached its memory limit of {limit_bytes} bytes")
        }
        FailureReason::TimeLimit { limit_seconds } => {
            format!("the converter reached its time limit of {limit_seconds} seconds")
        }
        FailureReason::ConverterCrashed => "the converter stopped abnormally".to_owned(),
        FailureReason::ConverterError => "the converter reported an error".to_owned(),
    }
}

/// One conversion warning, in words.
fn warning_words(warning: &ExtractionWarning) -> String {
    match warning {
        ExtractionWarning::UndecodableGlyphs {
            pages,
            unlocated_glyphs,
        } => {
            let pages: Vec<String> = pages.iter().map(|page| page.page_no.to_string()).collect();
            let unlocated = if *unlocated_glyphs > 0 {
                format!(" and in {unlocated_glyphs} places with no page")
            } else {
                String::new()
            };
            format!(
                "text the file's fonts give no characters for, on pages {}{unlocated}",
                pages.join(", ")
            )
        }
        ExtractionWarning::InferredLocations { items } => {
            format!("{items} parts located by matching their text, not by the converter")
        }
        ExtractionWarning::UnlocatedItems { items } => {
            format!("{items} parts with no location in the original")
        }
        ExtractionWarning::CellValuesOnly { sheets } => format!(
            "only the stored cell values were read, not formulas or formats, in {}",
            sheets.join(", ")
        ),
    }
}

/// The `Duplicate` row of `document`, one of the items of `group`, which share original bytes.
fn duplicate(document: &ItemDocument, group: &[&ItemDocument]) -> Row {
    let mut others: Vec<&str> = group
        .iter()
        .filter(|other| other.summary.id != document.summary.id)
        .map(|other| other.summary.path.as_str())
        .collect();
    others.sort_unstable();
    let more = others.len().saturating_sub(DUPLICATES_NAMED);
    let named = others
        .iter()
        .take(DUPLICATES_NAMED)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    let rest = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    row(
        AttentionKind::Duplicate,
        document,
        None,
        format!("The same file also appears at {named}{rest}"),
        OperationName::GetItem,
    )
}

/// The workspace's `UploadsWaiting` row, when any upload is complete and not imported.
///
/// # Errors
/// Returns any port error.
async fn uploads_waiting(
    service: &ApplicationService,
    scope: &StorageScope,
) -> Result<Option<Row>, ApiError> {
    let mut count = 0_u64;
    let mut bytes = 0_u64;
    let mut cursor = None;
    loop {
        let page = service
            .ports()
            .uploads
            .list(
                scope,
                UploadQuery {
                    state: Some(UploadListState::Waiting),
                    page: Page {
                        cursor: cursor.take(),
                        limit: SCAN_PAGE,
                    },
                },
            )
            .await?;
        for upload in &page.items {
            count = count.saturating_add(1);
            bytes = bytes.saturating_add(upload.object.as_ref().map_or(0, |object| object.size));
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    if count == 0 {
        return Ok(None);
    }
    let message = if count == 1 {
        format!("1 uploaded file is complete and not imported: {bytes} bytes")
    } else {
        format!("{count} uploaded files are complete and not imported: {bytes} bytes in all")
    };
    Ok(Some(Row {
        key: Key {
            kind: AttentionKind::UploadsWaiting,
            path: String::new(),
            item: None,
        },
        item: AttentionItem {
            kind: AttentionKind::UploadsWaiting,
            item_id: None,
            item: None,
            selection: None,
            message,
            action: OperationName::ListUploads,
            instruction_like: None,
        },
    }))
}

/// A row about `document`.
fn row(
    kind: AttentionKind,
    document: &ItemDocument,
    selection: Option<Selection>,
    message: String,
    action: OperationName,
) -> Row {
    let summary = &document.summary;
    Row {
        key: Key {
            kind: kind.clone(),
            path: summary.path.as_str().to_owned(),
            item: Some(summary.id),
        },
        item: AttentionItem {
            kind,
            item_id: Some(summary.id),
            item: Some(summary.clone()),
            selection,
            message,
            action,
            instruction_like: None,
        },
    }
}

/// The order of a row: its kind's place in `AttentionKind`'s declaration, its path, its item.
fn order(key: &Key) -> (u8, &str, Option<ItemId>) {
    (rank(&key.kind), key.path.as_str(), key.item)
}

/// A kind's place in the order `AttentionKind` declares the kinds.
const fn rank(kind: &AttentionKind) -> u8 {
    match kind {
        AttentionKind::BrokenLink => 0,
        AttentionKind::Stale => 1,
        AttentionKind::Unreviewed => 2,
        AttentionKind::Duplicate => 3,
        AttentionKind::MissingDescription => 4,
        AttentionKind::Unprocessed => 5,
        AttentionKind::ExtractionWarnings => 6,
        AttentionKind::NewerSource => 7,
        AttentionKind::InstructionLike => 8,
        AttentionKind::Invalidated => 9,
        AttentionKind::UnparseableHeader => 10,
        AttentionKind::UploadsWaiting => 11,
    }
}
