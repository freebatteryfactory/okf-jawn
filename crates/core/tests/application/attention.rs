//! Attention over the fixture's fakes, with upload slots, proposals and review rows (ruling R4),
//! and the index rebuild job over the tenancy fakes (ruling R5).
//!
//! Each kind built is shown with the cases that must not raise it: a broken link and the links
//! that are not (external, an anchor, a folder, a file the revision holds); `Stale` on the day of
//! the revision's commit, never by today; `Unreviewed` from each signal of agent authorship and
//! not for a person's content or covered content; `Duplicate` among visible items only;
//! `MissingDescription`; `Unprocessed` and `ExtractionWarnings` by the converter's verdict;
//! `UploadsWaiting` once for the workspace. Then the scan as a whole: archived items left out,
//! every folder and page read, the index built on demand, the order and the cursor.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use okf_jawn_contract::attention::{
    AttentionItem, AttentionKind, GetAttentionRequest, GetAttentionResponse, RebuildIndexRequest,
};
use okf_jawn_contract::common::{PageRange, PageRequest, TextRange};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{
    ConversionOutcome, Extraction, ExtractionWarning, FailureReason, PageCoverage, SuppliedText,
    TextOrigin,
};
use okf_jawn_contract::history::{Commit, LogResponse};
use okf_jawn_contract::identity::{
    At, Digest, IdempotencyKey, ItemId, JobId, ProposalId, ReviewId, UploadId,
};
use okf_jawn_contract::import::JobKind;
use okf_jawn_contract::item::ItemDocument;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::review::{Review, ReviewCoverage};
use okf_jawn_contract::search::Link;
use okf_jawn_contract::source::SourceReference;
use okf_jawn_core::application::ApplicationService;
use okf_jawn_core::jobs::{JobScope, JobSpec};
use okf_jawn_core::portable::item_content_digest;
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::ObjectInfo;
use okf_jawn_core::uploads::UploadRecord;
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    Built, World, alice, config, digest, instant, mutation, note, revision, scope, source_card,
};
use crate::proposal_store::FakeProposals;
use crate::review_store::ReviewRecords;
use crate::tenancy::{FakeUploads, Tenancy, uploader};

/// The fixture's fakes with upload slots, proposals and review rows, and the service over them.
struct Scan {
    /// Versions, the index, blobs and revision maps.
    world: World,
    /// Upload slots.
    uploads: Arc<FakeUploads>,
    /// Proposals.
    proposals: Arc<FakeProposals>,
    /// Review rows.
    reviews: Arc<ReviewRecords>,
    /// The service under test.
    service: ApplicationService,
}

/// A description that raises no `MissingDescription`.
const DESCRIBED: &str = "What this is.";

/// A fresh scan world whose head `a` is indexed and was committed on 2026-10-09.
fn scan_world() -> Built<Scan> {
    let world = World::new()?;
    let uploads = Arc::new(FakeUploads::default());
    let proposals = Arc::new(FakeProposals::default());
    let reviews = Arc::new(ReviewRecords::new(Arc::clone(&world.records)));
    let mut ports = world.service.ports().clone();
    ports.uploads = uploads.clone();
    ports.proposals = proposals.clone();
    ports.records = reviews.clone();
    let service = ApplicationService::new(ports, config())?;
    let head = revision('a')?;
    world.search.mark_indexed(&head)?;
    world.versions.put_log(
        &head,
        None,
        LogResponse {
            commits: vec![Commit {
                revision: head.clone(),
                parents: Vec::new(),
                message: "m".to_owned(),
                author: "alice".to_owned(),
                committed_at: instant()?,
            }],
            next_cursor: None,
        },
    )?;
    Ok(Scan {
        world,
        uploads,
        proposals,
        reviews,
        service,
    })
}

fn item(number: u128) -> ItemId {
    ItemId(Uuid::from_u128(number))
}

/// A described note numbered `number` at `a`.
fn described(number: u128, location: &str, properties: serde_json::Value) -> Built<ItemDocument> {
    let mut document = note(number, &revision('a')?, location, "text", properties)?;
    DESCRIBED.clone_into(&mut document.summary.description);
    Ok(document)
}

/// A described source card numbered `number` at `a` with `extraction`, whose original is the
/// object `object` names.
fn card(number: u128, location: &str, extraction: Extraction, object: char) -> Built<ItemDocument> {
    let mut document = source_card(number, &revision('a')?, location, "text", extraction)?;
    DESCRIBED.clone_into(&mut document.summary.description);
    if let Some(source) = document.source.as_mut() {
        source.object = digest(object)?;
    }
    Ok(document)
}

/// An extraction with `outcome` and `warnings`, the converter's text shown.
fn extraction(outcome: ConversionOutcome, warnings: Vec<ExtractionWarning>) -> Built<Extraction> {
    Ok(Extraction {
        outcome,
        converter: None,
        digest: Some(digest('c')?),
        page_count: Some(8),
        text_origin: TextOrigin::Converter,
        corrected: false,
        supplied: None,
        warnings,
    })
}

fn put(scan: &Scan, document: &ItemDocument) -> Built<()> {
    scan.world
        .versions
        .put_document(&revision('a')?, document.clone())
}

/// An outgoing link of `from` at `a`, to the item numbered `to` when the index resolved one.
fn link(scan: &Scan, from: &ItemDocument, to_path: &str, to: Option<u128>, line: u32) -> Built<()> {
    scan.world.search.put_link(
        &revision('a')?,
        Link {
            from: from.summary.id,
            from_path: from.summary.path.clone(),
            to_path: to_path.to_owned(),
            to: to.map(item),
            label: "l".to_owned(),
            line,
        },
    )
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, error.to_string())
}

async fn attention_at(
    scan: &Scan,
    at: At,
    limit: u16,
    cursor: Option<String>,
) -> Result<GetAttentionResponse, ApiError> {
    let context = alice(OperationName::GetAttention, None).map_err(internal)?;
    let workspace_id = scope().map_err(internal)?.workspace_id;
    scan.service
        .get_attention(
            &context,
            GetAttentionRequest {
                workspace_id,
                at,
                page: PageRequest { cursor, limit },
            },
        )
        .await
}

/// Every row at the head, on one page.
async fn every_row(scan: &Scan) -> Built<Vec<AttentionItem>> {
    let response = attention_at(scan, At::Latest, 200, None).await?;
    assert_eq!(response.revision, revision('a')?);
    assert_eq!(response.next_cursor, None, "one page");
    Ok(response.items)
}

/// The rows of `kind`.
fn of_kind(rows: &[AttentionItem], kind: &AttentionKind) -> Vec<AttentionItem> {
    rows.iter()
        .filter(|row| row.kind == *kind)
        .cloned()
        .collect()
}

/// The items the rows of `kind` name, in order.
fn named(rows: &[AttentionItem], kind: &AttentionKind) -> Vec<Option<ItemId>> {
    of_kind(rows, kind).iter().map(|row| row.item_id).collect()
}

/// The only row of `kind`.
fn only(rows: &[AttentionItem], kind: &AttentionKind) -> Built<AttentionItem> {
    let found = of_kind(rows, kind);
    let [row] = found.as_slice() else {
        return Err(format!("one {kind:?} row, not {}", found.len()).into());
    };
    Ok(row.clone())
}

/// A review of `document` recorded under `number`'s mutation, of the content `content`.
fn review(scan: &Scan, number: u128, document: &ItemDocument, content: Digest) -> Built<()> {
    scan.reviews.put_review(
        &scope()?,
        mutation(number),
        Review {
            id: ReviewId(Uuid::from_u128(number)),
            source: SourceReference {
                workspace_id: scope()?.workspace_id,
                item_id: document.summary.id,
                path: document.summary.path.clone(),
                revision: revision('a')?,
                digest: None,
                selection: Selection::All,
                locations: Vec::new(),
            },
            content_digest: content,
            reviewer_subject: "alice".to_owned(),
            reviewed_at: instant()?,
            coverage: ReviewCoverage::Current,
        },
    )
}

/// An accepted content proposal opened on `route`, whose candidate is `candidate`.
fn accepted(
    scan: &Scan,
    number: u128,
    route: &str,
    candidate: char,
    changes: serde_json::Value,
) -> Built<()> {
    let held = serde_json::from_value(json!({
        "id": ProposalId(Uuid::from_u128(number)),
        "workspace_id": scope()?.workspace_id,
        "base_revision": revision('a')?,
        "proposal_revision": revision(candidate)?,
        "content_digest": digest('d')?,
        "title": "t",
        "description": "d",
        "changes": changes,
        "kind": "content",
        "status": "accepted",
        "created_by": format!("agent-{number}"),
        "created_via": route,
        "created_at": "2026-10-09T12:00:00.000Z"
    }))?;
    scan.proposals.put(&scope()?, mutation(number), held)?;
    Ok(())
}

/// A waiting, incomplete or consumed upload numbered `number`.
fn upload(scan: &Scan, number: u128, complete: bool, consumed: bool) -> Built<()> {
    scan.uploads.put(
        &scope()?,
        UploadRecord {
            id: UploadId(Uuid::from_u128(number)),
            filename: format!("upload-{number}.pdf"),
            relative_path: String::new(),
            expected_size: 10,
            expected_sha256: None,
            supplied_by: uploader(),
            created_at: instant()?,
            received_bytes: 10,
            object: complete
                .then(|| -> Built<ObjectInfo> {
                    Ok(ObjectInfo {
                        digest: digest('c')?,
                        size: 10,
                    })
                })
                .transpose()?,
            consumed_by: consumed.then_some(JobId(Uuid::from_u128(5))),
        },
    )
}

#[tokio::test]
async fn attention_a_link_that_leads_nowhere_is_one_row_an_item_with_each_line() -> TestResult {
    let scan = scan_world()?;
    let plan = described(50, "notes/plan.md", json!({}))?;
    let single = described(51, "notes/single.md", json!({}))?;
    let clean = described(52, "notes/clean.md", json!({}))?;
    let chart = described(53, "notes/pics/chart.png", json!({}))?;
    for document in [&plan, &single, &clean, &chart] {
        put(&scan, document)?;
    }
    link(&scan, &plan, "gone.md", None, 3)?;
    link(&scan, &plan, "https://example.com/a", None, 4)?;
    link(&scan, &plan, "mailto:someone@example.com", None, 5)?;
    link(&scan, &plan, "#section", None, 6)?;
    link(&scan, &plan, "other/", None, 7)?;
    link(&scan, &plan, "clean.md", Some(52), 8)?;
    link(&scan, &plan, "pics/chart.png", None, 9)?;
    link(&scan, &plan, "pics/missing.png", None, 10)?;
    link(&scan, &plan, "/notes/absent", None, 11)?;
    link(&scan, &single, "../nowhere.md", None, 7)?;
    let rows = every_row(&scan).await?;
    assert_eq!(
        named(&rows, &AttentionKind::BrokenLink),
        vec![Some(plan.summary.id), Some(single.summary.id)]
    );
    let broken = of_kind(&rows, &AttentionKind::BrokenLink);
    let first = some(broken.first(), "the plan's row")?;
    for named_line in [
        "line 3: gone.md",
        "line 10: pics/missing.png",
        "line 11: /notes/absent",
    ] {
        assert!(first.message.contains(named_line), "{}", first.message);
    }
    for not_broken in [
        "example.com",
        "mailto",
        "#section",
        "other/",
        "clean.md",
        "chart.png",
    ] {
        assert!(!first.message.contains(not_broken), "{}", first.message);
    }
    assert_eq!(first.selection, None, "three places");
    assert_eq!(first.action, OperationName::GetLinks);
    assert_eq!(
        first.item.as_ref().map(|summary| summary.id),
        Some(plan.summary.id)
    );
    let second = some(broken.get(1), "the single row")?;
    assert_eq!(
        second.selection,
        Some(Selection::Lines {
            range: TextRange { start: 7, end: 7 }
        })
    );
    Ok(())
}

/// `trust::is_stale_on` on the revision's day: midnight UTC of 2026-10-09, the fixture
/// commit's day, so an instant later that day is not yet stale, and today (later) is never the
/// measure. A date-only, unreadable or non-string `stale_after` is never stale.
#[tokio::test]
async fn attention_stale_is_judged_on_the_day_of_the_revisions_commit() -> TestResult {
    let scan = scan_world()?;
    for (number, location, value) in [
        (60, "a/before.md", json!("2026-10-08T00:00:00Z")),
        (61, "a/midnight.md", json!("2026-10-09T00:00:00Z")),
        (62, "a/later-that-day.md", json!("2026-10-09T00:00:01Z")),
        (63, "a/date-only.md", json!("2026-10-01")),
        (64, "a/number.md", json!(20_261_001)),
        (65, "a/words.md", json!("soon")),
        (66, "a/offset.md", json!("2026-10-09T01:00:00+02:00")),
    ] {
        put(
            &scan,
            &described(number, location, json!({ "stale_after": value }))?,
        )?;
    }
    put(&scan, &described(67, "a/never.md", json!({}))?)?;
    let rows = every_row(&scan).await?;
    assert_eq!(
        named(&rows, &AttentionKind::Stale),
        vec![Some(item(60)), Some(item(61)), Some(item(66))]
    );
    let stale = of_kind(&rows, &AttentionKind::Stale);
    let row = some(stale.first(), "the first stale row")?;
    assert!(
        row.message.contains("2026-10-08T00:00:00Z"),
        "{}",
        row.message
    );
    assert!(row.message.contains("2026-10-09"), "{}", row.message);
    assert_eq!(row.action, OperationName::GetItem);
    Ok(())
}

/// The revision's day is read only when an item has a `stale_after` okf-core reads.
#[tokio::test]
async fn attention_stale_reads_the_history_only_when_an_item_needs_it() -> TestResult {
    let scan = scan_world()?;
    put(&scan, &described(68, "a/plain.md", json!({}))?)?;
    every_row(&scan).await?;
    assert_eq!(scan.world.versions.log_queries()?.len(), 0);
    put(
        &scan,
        &described(
            69,
            "a/dated.md",
            json!({ "stale_after": "2026-10-08T00:00:00Z" }),
        )?,
    )?;
    every_row(&scan).await?;
    assert_eq!(scan.world.versions.log_queries()?.len(), 1, "once a scan");
    Ok(())
}

/// `Unreviewed` from OKF `generated.by`: an agent actor, not a person or a process; a review
/// whose coverage is current covers it, one of other content does not.
#[tokio::test]
async fn attention_unreviewed_content_an_agent_generated() -> TestResult {
    let scan = scan_world()?;
    let generated = |by: &str| json!({ "generated": { "by": by, "at": "2026-10-01T00:00:00Z" } });
    let by_agent = described(
        70,
        "g/agent.md",
        generated("reference_agent/gemini-2.5-pro"),
    )?;
    let by_person = described(71, "g/person.md", generated("human:walter"))?;
    let by_process = described(72, "g/process.md", generated("process:nightly"))?;
    let reviewed = described(73, "g/reviewed.md", generated("reference_agent/v1"))?;
    let reviewed_before = described(74, "g/reviewed-before.md", generated("reference_agent/v1"))?;
    for document in [
        &by_agent,
        &by_person,
        &by_process,
        &reviewed,
        &reviewed_before,
    ] {
        put(&scan, document)?;
    }
    review(&scan, 700, &reviewed, item_content_digest(&reviewed)?)?;
    review(&scan, 701, &reviewed_before, digest('9')?)?;
    let rows = every_row(&scan).await?;
    assert_eq!(
        named(&rows, &AttentionKind::Unreviewed),
        vec![Some(by_agent.summary.id), Some(reviewed_before.summary.id)]
    );
    let row = some(
        of_kind(&rows, &AttentionKind::Unreviewed).first().cloned(),
        "a row",
    )?;
    assert!(
        row.message.contains("reference_agent/gemini-2.5-pro"),
        "{}",
        row.message
    );
    assert_eq!(row.action, OperationName::CreateReview);
    Ok(())
}

/// `Unreviewed` from an agent's accepted supplied text shown on a source.
#[tokio::test]
async fn attention_unreviewed_supplied_text_shown_on_a_source() -> TestResult {
    let scan = scan_world()?;
    let mut shown = extraction(ConversionOutcome::Unsupported, Vec::new())?;
    shown.text_origin = TextOrigin::SuppliedByAgent;
    shown.supplied = Some(SuppliedText {
        proposal_id: ProposalId(Uuid::from_u128(1)),
        supplied_by: "agent-7".to_owned(),
        client_id: None,
        approved_by: "alice".to_owned(),
        approved_at: instant()?,
        content_digest: digest('7')?,
        pages: Vec::new(),
        kept_by: None,
    });
    let supplied = card(75, "s/supplied-pdf.md", shown, '1')?;
    let converted = card(
        76,
        "s/converted-pdf.md",
        extraction(ConversionOutcome::Completed, Vec::new())?,
        '2',
    )?;
    put(&scan, &supplied)?;
    put(&scan, &converted)?;
    let rows = every_row(&scan).await?;
    let row = only(&rows, &AttentionKind::Unreviewed)?;
    assert_eq!(row.item_id, Some(supplied.summary.id));
    assert!(row.message.contains("agent-7"), "{}", row.message);
    Ok(())
}

/// `Unreviewed` from an accepted content proposal of an agent's delegated route: the content
/// its candidate gave an item it created or edited, while the item still holds it. A person's
/// proposal, a person's edit after the agent's, and a candidate the store no longer holds give
/// nothing.
#[tokio::test]
async fn attention_unreviewed_content_an_agents_accepted_proposal_wrote() -> TestResult {
    let scan = scan_world()?;
    let versions = &scan.world.versions;
    let candidate = revision('e')?;
    let edited = described(80, "p/edited.md", json!({}))?;
    let created = described(81, "p/created.md", json!({}))?;
    let changed_since = described(82, "p/changed-since.md", json!({}))?;
    let by_person = described(83, "p/by-person.md", json!({}))?;
    for document in [&edited, &created, &changed_since, &by_person] {
        put(&scan, document)?;
        let mut at_candidate = document.clone();
        at_candidate.summary.revision = candidate.clone();
        if document.summary.id == changed_since.summary.id {
            "the agent's words".clone_into(&mut at_candidate.body);
        }
        versions.put_document(&candidate, at_candidate)?;
    }
    accepted(
        &scan,
        800,
        "mcp_delegation",
        'e',
        json!([
            { "kind": "edit", "item_id": edited.summary.id, "body": "text", "properties": {} },
            { "kind": "create", "path": "p/created.md", "type_name": "Note", "body": "text",
              "properties": {} },
            { "kind": "edit", "item_id": changed_since.summary.id, "body": "the agent's words",
              "properties": {} },
            { "kind": "move", "item_id": by_person.summary.id, "destination": "p/moved.md" },
        ]),
    )?;
    accepted(
        &scan,
        801,
        "local_owner",
        'e',
        json!([{ "kind": "edit", "item_id": by_person.summary.id, "body": "text",
                 "properties": {} }]),
    )?;
    // A candidate the store no longer holds (revision `f` is unknown to it).
    accepted(
        &scan,
        802,
        "mcp_delegation",
        'f',
        json!([
            { "kind": "edit", "item_id": by_person.summary.id, "body": "text",
              "properties": {} },
            { "kind": "create", "path": "p/by-person.md", "type_name": "Note", "body": "text",
              "properties": {} },
        ]),
    )?;
    let rows = every_row(&scan).await?;
    assert_eq!(
        named(&rows, &AttentionKind::Unreviewed),
        vec![Some(created.summary.id), Some(edited.summary.id)]
    );
    assert!(
        of_kind(&rows, &AttentionKind::Unreviewed)
            .iter()
            .all(|row| row.message.contains("agent-800")),
        "{rows:?}"
    );
    Ok(())
}

/// Every page of the accepted proposals is read: the agent's, the oldest, comes after a full
/// page of a person's.
#[tokio::test]
async fn attention_every_page_of_the_accepted_proposals_is_read() -> TestResult {
    let scan = scan_world()?;
    let document = described(85, "p/oldest.md", json!({}))?;
    put(&scan, &document)?;
    scan.world
        .versions
        .put_document(&revision('e')?, document.clone())?;
    let edit = json!([{ "kind": "edit", "item_id": document.summary.id, "body": "text",
                        "properties": {} }]);
    accepted(&scan, 820, "mcp_delegation", 'e', edit.clone())?;
    for number in 821..=1020 {
        accepted(&scan, number, "local_owner", 'e', edit.clone())?;
    }
    let row = only(&every_row(&scan).await?, &AttentionKind::Unreviewed)?;
    assert!(row.message.contains("agent-820"), "{}", row.message);
    Ok(())
}

/// Only the `NotFound` of a gone candidate is passed over; any other failure of reading a
/// candidate fails the request.
#[tokio::test]
async fn attention_a_candidate_that_cannot_be_read_fails_the_request() -> TestResult {
    let scan = scan_world()?;
    let document = described(84, "p/one.md", json!({}))?;
    put(&scan, &document)?;
    scan.world
        .versions
        .put_document(&revision('e')?, document.clone())?;
    accepted(
        &scan,
        810,
        "mcp_delegation",
        'e',
        json!([{ "kind": "edit", "item_id": document.summary.id, "body": "text",
                 "properties": {} }]),
    )?;
    // The scan shows the one item of the head, then the candidate.
    scan.world
        .versions
        .fail_show_number(2, ErrorCode::Unavailable)?;
    let refused = err_of(attention_at(&scan, At::Latest, 200, None).await)?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    Ok(())
}

#[tokio::test]
async fn attention_duplicate_bytes_under_more_than_one_visible_item() -> TestResult {
    let scan = scan_world()?;
    let completed = || extraction(ConversionOutcome::Completed, Vec::new());
    let first = card(90, "d/first-pdf.md", completed()?, '1')?;
    let second = card(91, "d/second-pdf.md", completed()?, '1')?;
    let mut archived = card(92, "d/archived-pdf.md", completed()?, '2')?;
    archived.summary.archived = true;
    let alone = card(93, "d/alone-pdf.md", completed()?, '2')?;
    for document in [&first, &second, &archived, &alone] {
        put(&scan, document)?;
    }
    let rows = every_row(&scan).await?;
    assert_eq!(
        named(&rows, &AttentionKind::Duplicate),
        vec![Some(first.summary.id), Some(second.summary.id)]
    );
    let duplicates = of_kind(&rows, &AttentionKind::Duplicate);
    let row = some(duplicates.first(), "the first row")?;
    assert!(row.message.contains("d/second-pdf.md"), "{}", row.message);
    assert!(!row.message.contains("d/first-pdf.md"), "{}", row.message);
    assert_eq!(row.action, OperationName::GetItem);
    Ok(())
}

#[tokio::test]
async fn attention_an_empty_or_blank_description_is_missing() -> TestResult {
    let scan = scan_world()?;
    let mut blank = described(100, "m/blank.md", json!({}))?;
    "   ".clone_into(&mut blank.summary.description);
    let empty = note(101, &revision('a')?, "m/empty.md", "text", json!({}))?;
    let fine = described(102, "m/fine.md", json!({}))?;
    for document in [&blank, &empty, &fine] {
        put(&scan, document)?;
    }
    let rows = every_row(&scan).await?;
    assert_eq!(
        named(&rows, &AttentionKind::MissingDescription),
        vec![Some(blank.summary.id), Some(empty.summary.id)]
    );
    Ok(())
}

fn partial(not_converted: Vec<PageRange>) -> ConversionOutcome {
    ConversionOutcome::Partial {
        coverage: Some(PageCoverage {
            page_count: 8,
            converted: Vec::new(),
            partly_extracted: Vec::new(),
            not_converted,
        }),
        issues: Vec::new(),
    }
}

/// `Unprocessed` for a partial, failed or unsupported source, supplied text or not; its pages
/// as the selection when they are one range. `ExtractionWarnings` for a completed source with
/// warnings only.
#[tokio::test]
async fn attention_unprocessed_and_extraction_warnings_follow_the_converters_verdict() -> TestResult
{
    let scan = scan_world()?;
    let warned = vec![ExtractionWarning::UnlocatedItems { items: 2 }];
    let mut supplied_failed = extraction(
        ConversionOutcome::Failed {
            reason: FailureReason::Damaged,
            issues: Vec::new(),
        },
        Vec::new(),
    )?;
    supplied_failed.text_origin = TextOrigin::SuppliedByAgent;
    let cases = [
        (
            110,
            "u/a-one-range-pdf.md",
            extraction(partial(vec![PageRange { start: 3, end: 4 }]), Vec::new())?,
        ),
        (
            111,
            "u/b-two-ranges-pdf.md",
            extraction(
                partial(vec![
                    PageRange { start: 3, end: 4 },
                    PageRange { start: 7, end: 7 },
                ]),
                Vec::new(),
            )?,
        ),
        (
            112,
            "u/c-failed-pdf.md",
            extraction(
                ConversionOutcome::Failed {
                    reason: FailureReason::MemoryLimit {
                        limit_bytes: "1024".to_owned(),
                    },
                    issues: Vec::new(),
                },
                Vec::new(),
            )?,
        ),
        (
            113,
            "u/d-unsupported-pdf.md",
            extraction(ConversionOutcome::Unsupported, Vec::new())?,
        ),
        (
            114,
            "u/e-pending-pdf.md",
            extraction(ConversionOutcome::Pending, Vec::new())?,
        ),
        (
            115,
            "u/f-warned-pdf.md",
            extraction(ConversionOutcome::Completed, warned.clone())?,
        ),
        (
            116,
            "u/g-clean-pdf.md",
            extraction(ConversionOutcome::Completed, Vec::new())?,
        ),
        (
            117,
            "u/h-partial-warned-pdf.md",
            extraction(partial(vec![PageRange { start: 1, end: 1 }]), warned)?,
        ),
        (
            118,
            "u/i-partial-whole-pdf.md",
            extraction(
                ConversionOutcome::Partial {
                    coverage: None,
                    issues: Vec::new(),
                },
                Vec::new(),
            )?,
        ),
        (119, "u/j-supplied-failed-pdf.md", supplied_failed),
    ];
    let mut object = 'a';
    for (number, location, extraction) in cases {
        object = if object == 'a' { 'b' } else { 'a' };
        put(&scan, &card(number, location, extraction, object)?)?;
    }
    let rows = every_row(&scan).await?;
    assert_eq!(
        named(&rows, &AttentionKind::Unprocessed),
        [110, 111, 112, 113, 117, 118, 119]
            .into_iter()
            .map(|number| Some(item(number)))
            .collect::<Vec<_>>()
    );
    let unprocessed = of_kind(&rows, &AttentionKind::Unprocessed);
    let one_range = some(unprocessed.first(), "the one-range row")?;
    assert_eq!(
        one_range.selection,
        Some(Selection::Pages {
            range: PageRange { start: 3, end: 4 }
        })
    );
    assert!(one_range.message.contains("3-4"), "{}", one_range.message);
    assert_eq!(one_range.action, OperationName::RedigestItem);
    let two_ranges = some(unprocessed.get(1), "the two-range row")?;
    assert_eq!(two_ranges.selection, None);
    assert!(
        two_ranges.message.contains("3-4, 7"),
        "{}",
        two_ranges.message
    );
    let warning = only(&rows, &AttentionKind::ExtractionWarnings)?;
    assert_eq!(warning.item_id, Some(item(115)));
    assert_eq!(warning.action, OperationName::ReadItem);
    Ok(())
}

#[tokio::test]
async fn attention_uploads_waiting_is_one_row_with_their_count_and_size() -> TestResult {
    let scan = scan_world()?;
    assert_eq!(
        of_kind(&every_row(&scan).await?, &AttentionKind::UploadsWaiting).len(),
        0
    );
    upload(&scan, 1, true, false)?;
    upload(&scan, 2, true, false)?;
    upload(&scan, 3, true, false)?;
    upload(&scan, 4, false, false)?;
    upload(&scan, 5, true, true)?;
    let rows = every_row(&scan).await?;
    let row = only(&rows, &AttentionKind::UploadsWaiting)?;
    assert!(row.message.contains("3 uploaded files"), "{}", row.message);
    assert!(row.message.contains("30 bytes"), "{}", row.message);
    assert_eq!((row.item_id, &row.selection), (None, &None));
    assert!(row.item.is_none(), "about no item");
    assert_eq!(row.action, OperationName::ListUploads);
    Ok(())
}

/// Every page of the waiting uploads is read: more than one store page of them.
#[tokio::test]
async fn attention_uploads_waiting_counts_every_page() -> TestResult {
    let scan = scan_world()?;
    for number in 1..=201 {
        upload(&scan, number, true, false)?;
    }
    let row = only(&every_row(&scan).await?, &AttentionKind::UploadsWaiting)?;
    assert!(
        row.message.contains("201 uploaded files"),
        "{}",
        row.message
    );
    assert!(row.message.contains("2010 bytes"), "{}", row.message);
    Ok(())
}

/// Archived items are left out of every observation; every folder of the tree is read, across
/// pages.
#[tokio::test]
async fn attention_scans_every_folder_and_page_and_leaves_archived_items_out() -> TestResult {
    let scan = scan_world()?;
    scan.world.versions.page_listings(1)?;
    let mut archived = note(120, &revision('a')?, "x/archived.md", "text", json!({}))?;
    archived.summary.archived = true;
    put(&scan, &archived)?;
    link(&scan, &archived, "gone.md", None, 1)?;
    for (number, location) in [
        (121, "top.md"),
        (122, "x/y/deep.md"),
        (123, "x/y/z/deeper.md"),
        (124, "w/side.md"),
    ] {
        put(
            &scan,
            &note(number, &revision('a')?, location, "text", json!({}))?,
        )?;
    }
    let rows = every_row(&scan).await?;
    assert_eq!(
        named(&rows, &AttentionKind::MissingDescription),
        vec![
            Some(item(121)),
            Some(item(124)),
            Some(item(122)),
            Some(item(123))
        ]
    );
    assert!(
        rows.iter().all(|row| row.item_id != Some(item(120))),
        "{rows:?}"
    );
    Ok(())
}

/// A revision the index has not built is built on demand, as `get_links` builds one.
#[tokio::test]
async fn attention_builds_the_index_of_a_revision_it_lacks() -> TestResult {
    let world = World::new()?;
    let mut ports = world.service.ports().clone();
    ports.uploads = Arc::new(FakeUploads::default());
    ports.proposals = Arc::new(FakeProposals::default());
    ports.records = Arc::new(ReviewRecords::new(Arc::clone(&world.records)));
    let service = ApplicationService::new(ports, config())?;
    world
        .versions
        .put_document(&revision('a')?, described(130, "n/one.md", json!({}))?)?;
    let response = service
        .get_attention(
            &alice(OperationName::GetAttention, None)?,
            GetAttentionRequest {
                workspace_id: scope()?.workspace_id,
                at: At::Latest,
                page: PageRequest {
                    cursor: None,
                    limit: 10,
                },
            },
        )
        .await?;
    assert_eq!(response.items.len(), 0, "{:?}", response.items);
    assert_eq!(world.search.indexed()?, vec![revision('a')?]);
    Ok(())
}

/// The order: kind as `AttentionKind` declares them, then path, then item; a row about no item
/// first in its kind. Pages of two follow the cursor through every row once, at the cursor's
/// revision even when the head moved meanwhile.
#[tokio::test]
async fn attention_rows_are_ordered_and_paged_by_kind_path_and_item() -> TestResult {
    let scan = scan_world()?;
    let b = note(140, &revision('a')?, "b.md", "text", json!({}))?;
    let a = note(141, &revision('a')?, "a.md", "text", json!({}))?;
    let source = card(
        142,
        "c/r-pdf.md",
        extraction(ConversionOutcome::Unsupported, Vec::new())?,
        '3',
    )?;
    for document in [&b, &a, &source] {
        put(&scan, document)?;
    }
    link(&scan, &b, "gone.md", None, 1)?;
    upload(&scan, 1, true, false)?;
    let expected = vec![
        (AttentionKind::BrokenLink, Some(item(140))),
        (AttentionKind::MissingDescription, Some(item(141))),
        (AttentionKind::MissingDescription, Some(item(140))),
        (AttentionKind::Unprocessed, Some(item(142))),
        (AttentionKind::UploadsWaiting, None),
    ];
    let whole: Vec<(AttentionKind, Option<ItemId>)> = every_row(&scan)
        .await?
        .into_iter()
        .map(|row| (row.kind, row.item_id))
        .collect();
    assert_eq!(whole, expected);
    let first = attention_at(&scan, At::Latest, 2, None).await?;
    let cursor = some(first.next_cursor.clone(), "a cursor")?;
    // A write lands at a new head between the pages; the pages read the first one's revision.
    scan.world.versions.set_head(revision('b')?)?;
    scan.world.versions.put_document(
        &revision('b')?,
        note(143, &revision('b')?, "0.md", "text", json!({}))?,
    )?;
    let mut paged: Vec<(AttentionKind, Option<ItemId>)> = first
        .items
        .into_iter()
        .map(|row| (row.kind, row.item_id))
        .collect();
    let mut next = Some(cursor);
    while let Some(cursor) = next {
        let page = attention_at(&scan, At::Latest, 2, Some(cursor)).await?;
        assert_eq!(page.revision, revision('a')?);
        assert!(page.items.len() <= 2);
        paged.extend(page.items.into_iter().map(|row| (row.kind, row.item_id)));
        next = page.next_cursor;
    }
    assert_eq!(paged, expected, "no row repeated or skipped");
    Ok(())
}

/// A cursor this operation did not issue is `invalid_input` on `/page/cursor`: another
/// operation's, one that is not a position of this scan, and one of another revision than `at`.
#[tokio::test]
async fn attention_a_cursor_it_did_not_issue_is_refused() -> TestResult {
    let scan = scan_world()?;
    for number in [150, 151, 152] {
        put(
            &scan,
            &note(
                number,
                &revision('a')?,
                &format!("n{number}.md"),
                "t",
                json!({}),
            )?,
        )?;
    }
    let first = attention_at(&scan, At::Latest, 1, None).await?;
    let cursor = some(first.next_cursor, "a cursor")?;
    let encode = |value: serde_json::Value| URL_SAFE_NO_PAD.encode(value.to_string());
    let tip = revision('a')?;
    for forged in [
        encode(json!({ "op": "list_items", "tip": tip, "next": "{}" })),
        encode(json!({ "op": "get_attention", "tip": tip, "next": "not a position" })),
        encode(json!({ "op": "get_attention", "tip": tip,
                       "next": "{\"kind\":\"stale\",\"path\":\"\",\"item\":null,\"x\":1}" })),
        "not base64 at all".to_owned(),
    ] {
        let refused = err_of(attention_at(&scan, At::Latest, 1, Some(forged)).await)?;
        assert_eq!(refused.code, ErrorCode::InvalidInput, "{}", refused.message);
        assert_eq!(refused.field.as_deref(), Some("/page/cursor"));
    }
    let refused = err_of(
        attention_at(
            &scan,
            At::Revision {
                revision: revision('b')?,
            },
            1,
            Some(cursor),
        )
        .await,
    )?;
    assert_eq!(refused.field.as_deref(), Some("/page/cursor"));
    Ok(())
}

/// Nothing found is an empty answer, with no cursor; it is no certificate (the contract's
/// sentence), and nothing else is claimed.
#[tokio::test]
async fn attention_nothing_found_is_an_empty_answer() -> TestResult {
    let scan = scan_world()?;
    put(&scan, &described(160, "n/fine.md", json!({}))?)?;
    let response = attention_at(&scan, At::Latest, 50, None).await?;
    assert_eq!((response.items.len(), response.next_cursor), (0, None));
    Ok(())
}

fn rebuild() -> Built<RebuildIndexRequest> {
    Ok(RebuildIndexRequest {
        workspace_id: scope()?.workspace_id,
        idempotency_key: serde_json::from_value::<IdempotencyKey>(json!(
            "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        ))?,
    })
}

/// Ruling R5: `rebuild_index` queues `JobSpec::RebuildIndex` and wakes the worker, once per
/// mutation.
#[tokio::test]
async fn attention_rebuild_index_queues_a_rebuild_job_and_wakes_the_worker() -> TestResult {
    let tenancy = Tenancy::new()?;
    let context = alice(OperationName::RebuildIndex, Some(mutation(170)))?;
    let job = tenancy.service.rebuild_index(&context, rebuild()?).await?;
    assert_eq!(job.kind, JobKind::RebuildIndex);
    let created = tenancy.jobs.created()?;
    let [(held_in, new_job, _)] = created.as_slice() else {
        return Err("one job".into());
    };
    assert_eq!(*held_in, JobScope::Workspace(scope()?));
    assert_eq!(
        (new_job.mutation_id, &new_job.spec),
        (mutation(170), &JobSpec::RebuildIndex)
    );
    let again = tenancy.service.rebuild_index(&context, rebuild()?).await?;
    assert_eq!(again.id, job.id);
    assert_eq!(tenancy.jobs.created()?.len(), 1);
    assert_eq!(
        tenancy.queue.woken()?,
        vec![
            (JobScope::Workspace(scope()?), job.id),
            (JobScope::Workspace(scope()?), job.id)
        ]
    );
    Ok(())
}
