//! Imports, conversion again and corrections over the tenancy fakes, with a proposal store.
//!
//! `start_import` (ruling R1): the destination parsed (empty is the root) and checked against the
//! folders of the base by collision key at every level; the base a revision of the workspace
//! (the revision map, then the store), a moved head not refusing; every upload held, complete
//! and not taken, in order, before anything is queued; an upload a job names but has not
//! consumed not refused; absent settings resolved from the deployment's defaults into the job; a
//! resumed attempt answered its own job. `redigest_item` (R3): the base and the item checked, the
//! unconverted pages taken from the conversion record at the base with the record's settings.
//! `correct_digest` (R2): the digest the item's at the base, one commit on the base with a
//! receipt, a stale base a conflict on `/base_revision`, and `adopts` held to an accepted supply
//! of the same text. That a later redigest keeps a correction is storage's (`TreeEdit`
//! `WriteSourceCard` keeps corrections), proven against the real store in
//! `crates/storage/tests/imports_attention_service.rs`.

use std::sync::Arc;

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::{MutationResult, PageRange};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::extraction::{
    ConversionOutcome, ConversionSettings, Extraction, OcrPolicy, PageCoverage, TextOrigin,
};
use okf_jawn_contract::identity::{
    Digest, IdempotencyKey, ItemId, JobId, ProposalId, PurgeId, UploadId,
};
use okf_jawn_contract::import::{
    CorrectDigestRequest, Job, JobKind, JobState, RedigestRequest, StartImportRequest,
};
use okf_jawn_contract::item::MAX_ITEM_TEXT_BYTES;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::application::{ApplicationConfig, ApplicationService};
use okf_jawn_core::conformance::OkfConformance;
use okf_jawn_core::context::{Attempt, OperationContext};
use okf_jawn_core::jobs::{JobScope, JobSpec, RevisionMapping};
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::{
    CommitChanges, ObjectInfo, Provenance, TreeEdit, VersionStore, derive_commit_mutation_id,
};
use okf_jawn_core::uploads::UploadRecord;
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    ALICE, Built, CARD_BODY, config, context, digest, instant, mutation, note, path, record,
    revision, scope, source_card,
};
use crate::proposal_store::FakeProposals;
use crate::tenancy::{Tenancy, uploader};

/// The tenancy fakes with a proposal store, and the service composed over both.
struct Imports {
    /// Jobs, the queue, uploads, the log and the fixture world.
    tenancy: Tenancy,
    /// Proposals a correction may adopt.
    proposals: Arc<FakeProposals>,
    /// The service under test.
    service: ApplicationService,
}

/// The source card the redigest and correction tests read.
const CARD: u128 = 30;

fn imports() -> Built<Imports> {
    imports_with(config())
}

fn imports_with(config: ApplicationConfig) -> Built<Imports> {
    let tenancy = Tenancy::with_config(config.clone())?;
    let proposals = Arc::new(FakeProposals::default());
    let mut ports = tenancy.service.ports().clone();
    ports.proposals = proposals.clone();
    let service = ApplicationService::new(ports, config)?;
    Ok(Imports {
        tenancy,
        proposals,
        service,
    })
}

fn key() -> Built<IdempotencyKey> {
    Ok(serde_json::from_value(json!(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    ))?)
}

/// Alice's local session running `operation` under `number`'s mutation.
fn alice(operation: OperationName, number: u128) -> Built<OperationContext> {
    context(
        operation,
        ALICE,
        AccessRoute::LocalOwner,
        Some(mutation(number)),
    )
}

/// The same context as a resumed attempt of its mutation.
fn resumed(mut context: OperationContext) -> OperationContext {
    context.attempt = Attempt::Resumed;
    context
}

fn upload_id(number: u128) -> UploadId {
    UploadId(Uuid::from_u128(number))
}

/// Hold an upload numbered `number` in the fixture workspace: complete or not, taken or not.
fn held(
    tenancy: &Tenancy,
    number: u128,
    complete: bool,
    consumed_by: Option<JobId>,
) -> Built<UploadId> {
    tenancy.uploads.put(
        &scope()?,
        UploadRecord {
            id: upload_id(number),
            filename: format!("upload-{number}.pdf"),
            relative_path: String::new(),
            expected_size: 10,
            expected_sha256: None,
            supplied_by: uploader(),
            created_at: instant()?,
            received_bytes: if complete { 10 } else { 4 },
            object: complete
                .then(|| -> Built<ObjectInfo> {
                    Ok(ObjectInfo {
                        digest: digest('c')?,
                        size: 10,
                    })
                })
                .transpose()?,
            consumed_by,
        },
    )?;
    Ok(upload_id(number))
}

/// An import of `uploads` into `destination` on the fixture head `a`.
fn import(uploads: Vec<UploadId>, destination: &str) -> Built<StartImportRequest> {
    Ok(StartImportRequest {
        workspace_id: scope()?.workspace_id,
        base_revision: revision('a')?,
        upload_ids: uploads,
        destination: destination.to_owned(),
        idempotency_key: key()?,
        apply_naming_rules: false,
        settings: None,
    })
}

/// Run `start_import` as Alice under `number`'s mutation.
async fn start(
    imports: &Imports,
    number: u128,
    request: StartImportRequest,
) -> Result<Job, ApiError> {
    let context = alice(OperationName::StartImport, number).map_err(internal)?;
    imports.service.start_import(&context, request).await
}

/// The specification of the only job created so far.
fn only_spec(tenancy: &Tenancy) -> Built<JobSpec> {
    let created = tenancy.jobs.created()?;
    let [(held_in, new_job, _)] = created.as_slice() else {
        return Err(format!("one job was to be created, not {}", created.len()).into());
    };
    assert_eq!(*held_in, JobScope::Workspace(scope()?));
    Ok(new_job.spec.clone())
}

/// Nothing was created and nothing woken.
fn assert_nothing_queued(tenancy: &Tenancy) -> TestResult {
    assert_eq!(tenancy.jobs.created()?.len(), 0, "no job");
    assert_eq!(tenancy.queue.woken()?, Vec::new(), "no wake-up");
    Ok(())
}

fn assert_refused(error: &ApiError, code: ErrorCode, field: &str) {
    assert_eq!(error.code, code, "{}", error.message);
    assert_eq!(error.field.as_deref(), Some(field), "{}", error.message);
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, error.to_string())
}

#[tokio::test]
async fn imports_an_accepted_import_records_its_job_with_the_resolved_specification() -> TestResult
{
    let imports = imports()?;
    let first = held(&imports.tenancy, 1, true, None)?;
    let second = held(&imports.tenancy, 2, true, None)?;
    let job = start(&imports, 10, import(vec![first, second], "")?).await?;
    assert_eq!((job.kind, job.state), (JobKind::Import, JobState::Queued));
    let created = imports.tenancy.jobs.created()?;
    let [(_, new_job, wire)] = created.as_slice() else {
        return Err("one job was to be created".into());
    };
    assert_eq!(wire.id, job.id);
    assert_eq!(new_job.mutation_id, mutation(10));
    assert_eq!(
        new_job.initiator,
        Provenance {
            subject: ALICE.to_owned(),
            route: AccessRoute::LocalOwner,
            client_id: None,
        }
    );
    assert_eq!(
        new_job.spec,
        JobSpec::Import {
            base_revision: revision('a')?,
            upload_ids: vec![first, second],
            destination: None,
            apply_naming_rules: false,
            settings: ConversionSettings::default(),
        }
    );
    assert_eq!(
        imports.tenancy.queue.woken()?,
        vec![(JobScope::Workspace(scope()?), job.id)]
    );
    Ok(())
}

/// Ruling R1 resolves absent settings from the deployment. Those settings must satisfy the
/// contract's literal 36..=144 DPI bounds before any service can accept imports with them.
#[test]
fn imports_deployment_conversion_defaults_are_checked_before_acceptance() -> TestResult {
    let bad = ApplicationConfig {
        conversion_defaults: ConversionSettings {
            page_image_dpi: 145,
            ..ConversionSettings::default()
        },
        ..config()
    };
    let refused = err_of(bad.check())?;
    assert_refused(
        &refused,
        ErrorCode::InvalidInput,
        "/settings/page_image_dpi",
    );
    Ok(())
}

/// Ruling R1's settings: absent ones are the deployment's, resolved into the job at acceptance;
/// supplied ones are kept as given, and checked.
#[tokio::test]
async fn imports_absent_settings_are_the_deployments_defaults_resolved_into_the_job() -> TestResult
{
    let defaults = ConversionSettings {
        ocr: OcrPolicy::Skip,
        page_image_dpi: 72,
        ..ConversionSettings::default()
    };
    let imports = imports_with(ApplicationConfig {
        conversion_defaults: defaults.clone(),
        ..config()
    })?;
    let upload = held(&imports.tenancy, 1, true, None)?;
    start(&imports, 10, import(vec![upload], "")?).await?;
    let JobSpec::Import { settings, .. } = only_spec(&imports.tenancy)? else {
        return Err("an import job".into());
    };
    assert_eq!(settings, defaults);
    let given = ConversionSettings {
        ocr: OcrPolicy::ForceFullPage,
        ..ConversionSettings::default()
    };
    let mut request = import(vec![upload], "")?;
    request.settings = Some(given.clone());
    start(&imports, 11, request).await?;
    let created = imports.tenancy.jobs.created()?;
    let spec = &some(created.get(1), "the second job")?.1.spec;
    assert!(
        matches!(spec, JobSpec::Import { settings, .. } if *settings == given),
        "{spec:?}"
    );
    let mut out_of_bounds = import(vec![upload], "")?;
    out_of_bounds.settings = Some(ConversionSettings {
        page_image_dpi: 145,
        ..ConversionSettings::default()
    });
    let refused = err_of(start(&imports, 12, out_of_bounds).await)?;
    assert_refused(
        &refused,
        ErrorCode::InvalidInput,
        "/settings/page_image_dpi",
    );
    assert_eq!(imports.tenancy.jobs.created()?.len(), 2, "no third job");
    Ok(())
}

#[tokio::test]
async fn imports_the_destination_is_a_workspace_path_that_need_not_exist() -> TestResult {
    let imports = imports()?;
    let upload = held(&imports.tenancy, 1, true, None)?;
    for (number, written) in [
        (20, "a//b"),
        (21, "/reports"),
        (22, "reports/../secret"),
        (23, "reports/con"),
        (24, "reports\\2026"),
        (25, "reports."),
    ] {
        let refused = err_of(start(&imports, number, import(vec![upload], written)?).await)?;
        assert_refused(&refused, ErrorCode::InvalidInput, "/destination");
    }
    assert_nothing_queued(&imports.tenancy)?;
    start(&imports, 26, import(vec![upload], "reports/2026")?).await?;
    let JobSpec::Import { destination, .. } = only_spec(&imports.tenancy)? else {
        return Err("an import job".into());
    };
    assert_eq!(destination, Some(path("reports/2026")?));
    Ok(())
}

/// Ruling R1's destination: a folder of the base that the destination names with another
/// spelling (same collision key) is refused before the job is queued, naming the base's
/// spelling, at every level of the destination, whichever page of the listing holds it.
#[tokio::test]
async fn imports_a_destination_spelled_otherwise_than_a_folder_of_the_base_is_refused() -> TestResult
{
    let imports = imports()?;
    let versions = &imports.tenancy.world.versions;
    let base = revision('a')?;
    for (id, location) in [
        (40, "Reports/Q3/plan.md"),
        (41, "a/one.md"),
        (42, "b/two.md"),
        (43, "zeta/three.md"),
    ] {
        versions.put_document(&base, note(id, &base, location, "text", json!({}))?)?;
    }
    // One entry a page, so the folders are read across pages.
    versions.page_listings(1)?;
    let upload = held(&imports.tenancy, 1, true, None)?;
    for (number, written, held_spelling) in [
        (30, "reports", "Reports"),
        (31, "REPORTS/Q3", "Reports"),
        (32, "Reports/q3", "Reports/Q3"),
        (33, "Reports/q3/new", "Reports/Q3"),
        (34, "Zeta", "zeta"),
    ] {
        let refused = err_of(start(&imports, number, import(vec![upload], written)?).await)?;
        assert_refused(&refused, ErrorCode::InvalidInput, "/destination");
        assert!(
            refused.message.contains(held_spelling),
            "{written}: {}",
            refused.message
        );
    }
    assert_nothing_queued(&imports.tenancy)?;
    for (number, written) in [(35, "Reports/Q3/new"), (36, "zeta"), (37, "Reports/q4")] {
        start(&imports, number, import(vec![upload], written)?).await?;
    }
    assert_eq!(imports.tenancy.jobs.created()?.len(), 3);
    Ok(())
}

/// SPEC section 3's item/folder distinction: an existing item cannot become an import
/// destination, whatever its case or depth, and refusing it creates no job. A real folder
/// ending in .md and a mere string-prefix neighbour are valid destinations.
#[tokio::test]
async fn imports_a_destination_or_ancestor_that_is_an_item_is_refused_before_job_creation()
-> TestResult {
    let imports = imports()?;
    let versions = &imports.tenancy.world.versions;
    let base = revision('a')?;
    for (id, location) in [
        (40, "archive.md"),
        (41, "notes/report.md"),
        (42, "notes/folder.md/kept.md"),
    ] {
        versions.put_document(&base, note(id, &base, location, "text", json!({}))?)?;
    }
    versions.page_listings(1)?;
    let upload = held(&imports.tenancy, 1, true, None)?;
    for (number, written, occupied) in [
        (130, "archive.md", "archive.md"),
        (131, "ARCHIVE.md/nested", "archive.md"),
        (132, "notes/report.md", "notes/report.md"),
        (133, "notes/REPORT.md/new/deep", "notes/report.md"),
    ] {
        let refused = err_of(start(&imports, number, import(vec![upload], written)?).await)?;
        assert_refused(&refused, ErrorCode::InvalidInput, "/destination");
        assert!(
            refused.message.contains(occupied),
            "{written}: {}",
            refused.message
        );
        assert_nothing_queued(&imports.tenancy)?;
    }
    for (number, written) in [(134, "notes/folder.md/new"), (135, "archive.md-new")] {
        start(&imports, number, import(vec![upload], written)?).await?;
    }
    assert_eq!(imports.tenancy.jobs.created()?.len(), 2);
    Ok(())
}

/// Ruling R1's base: the revision map first (`check_named`), then the store; a head that moved
/// since does not refuse (contract ruling 26).
#[tokio::test]
async fn imports_the_base_must_be_a_revision_of_the_workspace_and_a_moved_head_does_not_refuse()
-> TestResult {
    let imports = imports()?;
    let upload = held(&imports.tenancy, 1, true, None)?;
    let mut unknown = import(vec![upload], "")?;
    unknown.base_revision = revision('f')?;
    let refused = err_of(start(&imports, 50, unknown).await)?;
    assert_eq!(refused.code, ErrorCode::NotFound, "{}", refused.message);
    // A revision the store holds and a purge removed is the revision map's typed refusal.
    let purged = revision('c')?;
    imports
        .tenancy
        .world
        .versions
        .put_document(&purged, note(44, &purged, "notes/a.md", "a", json!({}))?)?;
    imports.tenancy.world.records.put_mapping(
        purged.clone(),
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(9)),
            replacement: None,
        },
    )?;
    let mut invalidated = import(vec![upload], "")?;
    invalidated.base_revision = purged;
    let refused = err_of(start(&imports, 51, invalidated).await)?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert!(
        matches!(
            refused.detail.as_deref(),
            Some(ErrorDetail::Invalidated { .. })
        ),
        "{refused:?}"
    );
    assert_nothing_queued(&imports.tenancy)?;
    imports.tenancy.world.versions.set_head(revision('b')?)?;
    start(&imports, 52, import(vec![upload], "")?).await?;
    assert!(matches!(
        only_spec(&imports.tenancy)?,
        JobSpec::Import { base_revision, .. } if base_revision == revision('a')?
    ));
    Ok(())
}

/// Ruling R1's uploads: each held, complete and not taken, checked in order before the job is
/// queued; the first that fails is named by its place in the request.
#[tokio::test]
async fn imports_every_upload_must_be_held_complete_and_not_taken() -> TestResult {
    let imports = imports()?;
    let tenancy = &imports.tenancy;
    let fine = held(tenancy, 1, true, None)?;
    let incomplete = held(tenancy, 2, false, None)?;
    let taker = JobId(Uuid::from_u128(77));
    let taken = held(tenancy, 3, true, Some(taker))?;
    let refused = err_of(start(&imports, 60, import(vec![fine, upload_id(99)], "")?).await)?;
    assert_refused(&refused, ErrorCode::NotFound, "/upload_ids/1");
    let refused = err_of(start(&imports, 61, import(vec![fine, incomplete], "")?).await)?;
    assert_refused(&refused, ErrorCode::InvalidInput, "/upload_ids/1");
    let refused = err_of(start(&imports, 62, import(vec![fine, taken], "")?).await)?;
    assert_refused(&refused, ErrorCode::Conflict, "/upload_ids/1");
    assert!(
        refused.message.contains(&taker.0.to_string()),
        "{}",
        refused.message
    );
    // The first upload that fails, in the request's order.
    let refused = err_of(start(&imports, 63, import(vec![taken, incomplete], "")?).await)?;
    assert_refused(&refused, ErrorCode::Conflict, "/upload_ids/0");
    let refused = err_of(start(&imports, 64, import(vec![incomplete, taken], "")?).await)?;
    assert_refused(&refused, ErrorCode::InvalidInput, "/upload_ids/0");
    assert_nothing_queued(tenancy)?;
    Ok(())
}

/// The request's own rules (`check_rules`), held by the handler too.
#[tokio::test]
async fn imports_the_list_of_uploads_is_checked_by_the_handler_too() -> TestResult {
    let imports = imports()?;
    let upload = held(&imports.tenancy, 1, true, None)?;
    let refused = err_of(start(&imports, 70, import(Vec::new(), "")?).await)?;
    assert_refused(&refused, ErrorCode::InvalidInput, "/upload_ids");
    let refused = err_of(start(&imports, 71, import(vec![upload, upload], "")?).await)?;
    assert_refused(&refused, ErrorCode::InvalidInput, "/upload_ids/1");
    assert_nothing_queued(&imports.tenancy)?;
    Ok(())
}

/// Not refused at acceptance: an upload another queued job names but has not consumed. The job
/// consumes its uploads all or none, so the second import fails at run time instead.
#[tokio::test]
async fn imports_an_upload_a_queued_job_names_but_has_not_taken_is_not_refused() -> TestResult {
    let imports = imports()?;
    let upload = held(&imports.tenancy, 1, true, None)?;
    let first = start(&imports, 80, import(vec![upload], "")?).await?;
    let second = start(&imports, 81, import(vec![upload], "")?).await?;
    assert_ne!(first.id, second.id);
    assert_eq!(imports.tenancy.jobs.created()?.len(), 2);
    Ok(())
}

/// A repeat under the mutation is answered the first job, and wakes the worker again; a failed
/// wake-up fails the request, and its retry answers the same job.
#[tokio::test]
async fn imports_a_repeat_answers_the_first_job_and_a_failed_wake_up_fails_the_request()
-> TestResult {
    let imports = imports()?;
    let upload = held(&imports.tenancy, 1, true, None)?;
    imports.tenancy.queue.fail_next();
    let refused = err_of(start(&imports, 90, import(vec![upload], "")?).await)?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    let context = resumed(alice(OperationName::StartImport, 90)?);
    let job = imports
        .service
        .start_import(&context, import(vec![upload], "")?)
        .await?;
    assert_eq!(imports.tenancy.jobs.created()?.len(), 1, "one job");
    assert_eq!(
        imports.tenancy.queue.woken()?,
        vec![(JobScope::Workspace(scope()?), job.id)]
    );
    Ok(())
}

/// SPEC section 5: "a retried request returns the job it already created", even after that
/// job consumed its uploads but before it wrote the pending-card commit. A foreign mutation
/// cannot use that job as ownership proof and must create no orphan job.
#[tokio::test]
async fn imports_a_resumed_attempt_whose_own_job_took_the_uploads_is_answered_that_job()
-> TestResult {
    let imports = imports()?;
    let tenancy = &imports.tenancy;
    let first = held(tenancy, 1, true, None)?;
    let second = held(tenancy, 2, true, None)?;
    let job = start(&imports, 100, import(vec![first, second], "")?).await?;
    // The job takes both, as its handler's `consume_batch` does.
    held(tenancy, 1, true, Some(job.id))?;
    held(tenancy, 2, true, Some(job.id))?;
    let again =
        || -> Built<OperationContext> { Ok(resumed(alice(OperationName::StartImport, 100)?)) };
    let before_commit = imports
        .service
        .start_import(&again()?, import(vec![first, second], "")?)
        .await?;
    assert_eq!(before_commit.id, job.id);
    assert_eq!(
        tenancy.jobs.created()?.len(),
        1,
        "no second job before the commit"
    );
    // The job's pending cards land after the base, under the job's first commit identity.
    let versions = &tenancy.world.versions;
    versions.queue_revision(revision('b')?)?;
    versions
        .commit(
            &scope()?,
            CommitChanges {
                mutation_id: derive_commit_mutation_id(mutation(100), 0),
                expected_head: revision('a')?,
                author: uploader(),
                message: "Import".to_owned(),
                edits: Vec::new(),
            },
            Arc::new(OkfConformance),
        )
        .await?;
    let answered = imports
        .service
        .start_import(&again()?, import(vec![first, second], "")?)
        .await?;
    assert_eq!(answered.id, job.id);
    assert_eq!(tenancy.jobs.created()?.len(), 1, "no second job");
    // Another request's first attempt is refused, whatever this one's job wrote.
    let refused = err_of(start(&imports, 101, import(vec![second], "")?).await)?;
    assert_refused(&refused, ErrorCode::Conflict, "/upload_ids/0");
    // And so is another resumed request, whose own commit never landed.
    let refused = err_of(
        imports
            .service
            .start_import(
                &resumed(alice(OperationName::StartImport, 102)?),
                import(vec![second], "")?,
            )
            .await,
    )?;
    assert_refused(&refused, ErrorCode::Conflict, "/upload_ids/0");
    assert_eq!(
        tenancy.jobs.created()?.len(),
        1,
        "foreign retries create no orphan"
    );
    Ok(())
}

/// Every upload is checked, including after one is recognized as this resumed request's.
/// A partially inconsistent/removed later slot is never hidden by an early successful return.
#[tokio::test]
async fn imports_a_resumed_own_consumption_does_not_skip_later_upload_checks() -> TestResult {
    let imports = imports()?;
    let tenancy = &imports.tenancy;
    let first = held(tenancy, 1, true, None)?;
    let second = held(tenancy, 2, true, None)?;
    let job = start(&imports, 140, import(vec![first, second], "")?).await?;
    held(tenancy, 1, true, Some(job.id))?;
    held(tenancy, 2, false, None)?;
    let refused = err_of(
        imports
            .service
            .start_import(
                &resumed(alice(OperationName::StartImport, 140)?),
                import(vec![first, second], "")?,
            )
            .await,
    )?;
    assert_refused(&refused, ErrorCode::InvalidInput, "/upload_ids/1");
    assert_eq!(tenancy.jobs.created()?.len(), 1);
    assert_eq!(
        tenancy.queue.woken()?.len(),
        1,
        "no new wake-up for the refusal"
    );
    Ok(())
}

fn card_id() -> ItemId {
    ItemId(Uuid::from_u128(CARD))
}

/// A partly converted card at `a` whose record (digest `c`) leaves pages 3-4 and 7 of 8 not
/// converted; the record is held in the blob store.
fn partial_card(imports: &Imports) -> Built<()> {
    let coverage = PageCoverage {
        page_count: 8,
        converted: vec![
            PageRange { start: 1, end: 2 },
            PageRange { start: 5, end: 6 },
            PageRange { start: 8, end: 8 },
        ],
        partly_extracted: Vec::new(),
        not_converted: vec![
            PageRange { start: 3, end: 4 },
            PageRange { start: 7, end: 7 },
        ],
    };
    let outcome = ConversionOutcome::Partial {
        coverage: Some(coverage),
        issues: Vec::new(),
    };
    put_card(imports, outcome)
}

/// A card at `a` with `outcome`, recorded under digest `c` in the blob store.
fn put_card(imports: &Imports, outcome: ConversionOutcome) -> Built<()> {
    let base = revision('a')?;
    let extraction = Extraction {
        outcome: outcome.clone(),
        converter: None,
        digest: Some(digest('c')?),
        page_count: Some(8),
        text_origin: TextOrigin::Converter,
        corrected: false,
        supplied: None,
        warnings: Vec::new(),
    };
    imports.tenancy.world.versions.put_document(
        &base,
        source_card(CARD, &base, "files/report-pdf.md", CARD_BODY, extraction)?,
    )?;
    let mut conversion = record(Vec::new())?;
    conversion.outcome = outcome;
    conversion.page_count = Some(8);
    imports
        .tenancy
        .world
        .blobs
        .put_object(digest('c')?, serde_json::to_vec(&conversion)?)?;
    Ok(())
}

fn redigest(unconverted_only: bool) -> Built<RedigestRequest> {
    Ok(RedigestRequest {
        workspace_id: scope()?.workspace_id,
        item_id: card_id(),
        base_revision: revision('a')?,
        settings: ConversionSettings::default(),
        unconverted_only,
        idempotency_key: key()?,
    })
}

async fn convert_again(
    imports: &Imports,
    number: u128,
    request: RedigestRequest,
) -> Result<Job, ApiError> {
    let context = alice(OperationName::RedigestItem, number).map_err(internal)?;
    imports.service.redigest_item(&context, request).await
}

#[tokio::test]
async fn imports_a_redigest_queues_the_whole_source_again_with_the_requests_settings() -> TestResult
{
    let imports = imports()?;
    partial_card(&imports)?;
    imports.tenancy.world.versions.set_head(revision('b')?)?;
    let mut request = redigest(false)?;
    request.settings.ocr = OcrPolicy::ForceFullPage;
    let job = convert_again(&imports, 200, request).await?;
    assert_eq!(job.kind, JobKind::Redigest);
    assert_eq!(
        only_spec(&imports.tenancy)?,
        JobSpec::Redigest {
            item_id: card_id(),
            base_revision: revision('a')?,
            settings: ConversionSettings {
                ocr: OcrPolicy::ForceFullPage,
                ..ConversionSettings::default()
            },
            pages: None,
        }
    );
    assert_eq!(
        imports.tenancy.queue.woken()?,
        vec![(JobScope::Workspace(scope()?), job.id)]
    );
    Ok(())
}

/// Ruling R3: core computes `pages` from the `not_converted` pages of the record at the base.
#[tokio::test]
async fn imports_a_redigest_of_the_unconverted_pages_takes_them_from_the_record() -> TestResult {
    let imports = imports()?;
    partial_card(&imports)?;
    convert_again(&imports, 210, redigest(true)?).await?;
    let JobSpec::Redigest { pages, .. } = only_spec(&imports.tenancy)? else {
        return Err("a redigest job".into());
    };
    assert_eq!(
        pages,
        Some(vec![
            PageRange { start: 3, end: 4 },
            PageRange { start: 7, end: 7 }
        ])
    );
    Ok(())
}

#[tokio::test]
async fn imports_a_redigest_of_the_unconverted_pages_needs_some_and_the_records_settings()
-> TestResult {
    let imports = imports()?;
    partial_card(&imports)?;
    let mut other = redigest(true)?;
    other.settings.table_structure = false;
    let refused = err_of(convert_again(&imports, 220, other).await)?;
    assert_refused(&refused, ErrorCode::InvalidInput, "/settings");
    // A completed conversion has no page that was not converted.
    let complete = imports()?;
    put_card(&complete, ConversionOutcome::Completed)?;
    let refused = err_of(convert_again(&complete, 221, redigest(true)?).await)?;
    assert_refused(&refused, ErrorCode::InvalidInput, "/unconverted_only");
    convert_again(&complete, 222, redigest(false)?).await?;
    // A failed card names no record.
    let failed = imports()?;
    let base = revision('a')?;
    let extraction = Extraction {
        outcome: ConversionOutcome::Unsupported,
        converter: None,
        digest: None,
        page_count: None,
        text_origin: TextOrigin::None,
        corrected: false,
        supplied: None,
        warnings: Vec::new(),
    };
    failed.tenancy.world.versions.put_document(
        &base,
        source_card(CARD, &base, "files/report-pdf.md", "", extraction)?,
    )?;
    let refused = err_of(convert_again(&failed, 223, redigest(true)?).await)?;
    assert_refused(&refused, ErrorCode::InvalidInput, "/unconverted_only");
    assert_nothing_queued(&imports.tenancy)?;
    assert_nothing_queued(&failed.tenancy)?;
    Ok(())
}

#[tokio::test]
async fn imports_a_redigest_needs_a_source_at_a_revision_of_the_workspace() -> TestResult {
    let imports = imports()?;
    let base = revision('a')?;
    imports
        .tenancy
        .world
        .versions
        .put_document(&base, note(31, &base, "notes/plan.md", "text", json!({}))?)?;
    let mut of_a_note = redigest(false)?;
    of_a_note.item_id = ItemId(Uuid::from_u128(31));
    let refused = err_of(convert_again(&imports, 230, of_a_note).await)?;
    assert_refused(&refused, ErrorCode::InvalidInput, "/item_id");
    let refused = err_of(convert_again(&imports, 231, redigest(false)?).await)?;
    assert_eq!(refused.code, ErrorCode::NotFound, "no card at the base");
    partial_card(&imports)?;
    let mut unknown = redigest(false)?;
    unknown.base_revision = revision('f')?;
    let refused = err_of(convert_again(&imports, 232, unknown).await)?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    let mut out_of_bounds = redigest(false)?;
    out_of_bounds.settings.page_image_dpi = 35;
    let refused = err_of(convert_again(&imports, 233, out_of_bounds).await)?;
    assert_refused(
        &refused,
        ErrorCode::InvalidInput,
        "/settings/page_image_dpi",
    );
    assert_nothing_queued(&imports.tenancy)?;
    Ok(())
}

fn correction(text: &str, adopts: Option<ProposalId>) -> Built<CorrectDigestRequest> {
    Ok(CorrectDigestRequest {
        workspace_id: scope()?.workspace_id,
        item_id: card_id(),
        base_revision: revision('a')?,
        digest: digest('c')?,
        corrected_markdown: text.to_owned(),
        adopts,
        idempotency_key: key()?,
    })
}

async fn correct(
    imports: &Imports,
    number: u128,
    request: CorrectDigestRequest,
) -> Result<MutationResult, ApiError> {
    let context = alice(OperationName::CorrectDigest, number).map_err(internal)?;
    imports.service.correct_digest(&context, request).await
}

/// The correction edits the fake's commits hold, in order: (digest, text, adopted proposal).
fn corrections(imports: &Imports) -> Built<Vec<(Digest, String, Option<ProposalId>)>> {
    Ok(imports
        .tenancy
        .world
        .versions
        .commits()?
        .iter()
        .flat_map(|commit| commit.edits.iter())
        .filter_map(|edit| match edit {
            TreeEdit::CorrectDigest {
                digest,
                corrected_markdown,
                adopts,
                ..
            } => Some((digest.clone(), corrected_markdown.clone(), *adopts)),
            _ => None,
        })
        .collect())
}

/// Ruling R2: one commit of `TreeEdit::CorrectDigest` on the base, with a receipt; a resumed
/// attempt is answered the first commit and receipt.
#[tokio::test]
async fn imports_a_correction_is_one_commit_on_the_base_with_a_receipt() -> TestResult {
    let imports = imports()?;
    partial_card(&imports)?;
    imports
        .tenancy
        .world
        .versions
        .queue_revision(revision('b')?)?;
    let result = correct(&imports, 300, correction("# Fixed\n", None)?).await?;
    assert_eq!(result.revision, revision('b')?);
    let commits = imports.tenancy.world.versions.commits()?;
    let [commit] = commits.as_slice() else {
        return Err("one commit".into());
    };
    assert_eq!(commit.expected_head, revision('a')?);
    assert_eq!(commit.mutation_id, mutation(300));
    assert!(matches!(
        commit.edits.as_slice(),
        [TreeEdit::CorrectDigest { item_id, .. }] if *item_id == card_id()
    ));
    assert_eq!(
        corrections(&imports)?,
        vec![(digest('c')?, "# Fixed\n".to_owned(), None)]
    );
    let receipts = imports.tenancy.world.records.receipts()?;
    assert!(
        receipts.iter().any(
            |(under, receipt)| *under == Some(mutation(300)) && receipt.id == result.receipt_id
        ),
        "the receipt under the mutation"
    );
    assert_eq!(
        imports.tenancy.world.search.indexed()?,
        vec![revision('b')?]
    );
    let again = imports
        .service
        .correct_digest(
            &resumed(alice(OperationName::CorrectDigest, 300)?),
            correction("# Fixed\n", None)?,
        )
        .await?;
    assert_eq!(
        (again.revision, again.receipt_id),
        (result.revision, result.receipt_id)
    );
    assert_eq!(
        imports.tenancy.world.versions.commits()?.len(),
        1,
        "no second commit"
    );
    Ok(())
}

#[tokio::test]
async fn imports_a_correction_names_the_items_digest_at_the_base() -> TestResult {
    let imports = imports()?;
    partial_card(&imports)?;
    let mut other = correction("text", None)?;
    other.digest = digest('d')?;
    let refused = err_of(correct(&imports, 310, other).await)?;
    assert_refused(&refused, ErrorCode::InvalidInput, "/digest");
    let base = revision('a')?;
    imports
        .tenancy
        .world
        .versions
        .put_document(&base, note(31, &base, "notes/plan.md", "text", json!({}))?)?;
    let mut of_a_note = correction("text", None)?;
    of_a_note.item_id = ItemId(Uuid::from_u128(31));
    let refused = err_of(correct(&imports, 311, of_a_note).await)?;
    assert_refused(&refused, ErrorCode::InvalidInput, "/digest");
    let mut too_long = correction("", None)?;
    too_long.corrected_markdown = "x".repeat(MAX_ITEM_TEXT_BYTES.saturating_add(1));
    let refused = err_of(correct(&imports, 312, too_long).await)?;
    assert_refused(&refused, ErrorCode::TooLarge, "/corrected_markdown");
    assert_eq!(imports.tenancy.world.versions.commits()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn imports_a_correction_on_a_moved_head_is_a_conflict_on_the_base() -> TestResult {
    let imports = imports()?;
    partial_card(&imports)?;
    imports.tenancy.world.versions.set_head(revision('b')?)?;
    let refused = err_of(correct(&imports, 320, correction("text", None)?).await)?;
    assert_refused(&refused, ErrorCode::Conflict, "/base_revision");
    assert!(
        matches!(
            refused.detail.as_deref(),
            Some(ErrorDetail::StaleBase { head }) if *head == revision('b')?
        ),
        "{refused:?}"
    );
    Ok(())
}

/// A proposal held in the fixture workspace in `status`, of `kind`, opened by an agent's
/// delegated route, with `changes`.
fn proposal(
    imports: &Imports,
    number: u128,
    status: &str,
    kind: &str,
    changes: serde_json::Value,
) -> Built<ProposalId> {
    let id = ProposalId(Uuid::from_u128(number));
    let held = serde_json::from_value(json!({
        "id": id,
        "workspace_id": scope()?.workspace_id,
        "base_revision": revision('a')?,
        "proposal_revision": revision('e')?,
        "content_digest": digest('d')?,
        "title": "t",
        "description": "d",
        "changes": changes,
        "kind": kind,
        "status": status,
        "created_by": "agent",
        "created_via": "mcp_delegation",
        "created_at": "2026-10-09T12:00:00.000Z"
    }))?;
    imports.proposals.put(&scope()?, mutation(number), held)?;
    Ok(id)
}

fn supplied(item: ItemId, text: &str) -> serde_json::Value {
    json!([{ "kind": "supply_extraction", "item_id": item, "markdown": text }])
}

/// Ruling R2's `adopts`: the proposal accepted, a supply of this item, the same text by digest;
/// otherwise `conflict` on `/adopts`, and a proposal the workspace does not hold `NotFound`
/// there. Nothing is committed for a refusal.
#[tokio::test]
async fn imports_keeping_supplied_text_needs_an_accepted_supply_of_this_item_with_that_text()
-> TestResult {
    let imports = imports()?;
    partial_card(&imports)?;
    let text = "# The agent's text\n";
    let kept = proposal(
        &imports,
        400,
        "accepted",
        "supply_extraction",
        supplied(card_id(), text),
    )?;
    let open = proposal(
        &imports,
        401,
        "open",
        "supply_extraction",
        supplied(card_id(), text),
    )?;
    let elsewhere = proposal(
        &imports,
        402,
        "accepted",
        "supply_extraction",
        supplied(ItemId(Uuid::from_u128(31)), text),
    )?;
    let content = proposal(
        &imports,
        403,
        "accepted",
        "content",
        json!([{ "kind": "edit", "item_id": card_id(), "body": text, "properties": {} }]),
    )?;
    for (number, adopted, written) in [
        (410, open, text),
        (411, elsewhere, text),
        (412, content, text),
        (413, kept, "# Another text\n"),
    ] {
        let refused = err_of(correct(&imports, number, correction(written, Some(adopted))?).await)?;
        assert_refused(&refused, ErrorCode::Conflict, "/adopts");
    }
    let unknown = ProposalId(Uuid::from_u128(499));
    let refused = err_of(correct(&imports, 414, correction(text, Some(unknown))?).await)?;
    assert_refused(&refused, ErrorCode::NotFound, "/adopts");
    assert_eq!(imports.tenancy.world.versions.commits()?.len(), 0);
    imports
        .tenancy
        .world
        .versions
        .queue_revision(revision('b')?)?;
    correct(&imports, 415, correction(text, Some(kept))?).await?;
    assert_eq!(
        corrections(&imports)?,
        vec![(digest('c')?, text.to_owned(), Some(kept))]
    );
    Ok(())
}
