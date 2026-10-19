//! Upload slots over the tenancy fakes. `create_upload` refuses, before the store is called, a
//! `size` that is not a decimal `u64` (`invalid_input`) or is over the deployment's upload limit
//! (`too_large`), both on `/size`, and a `filename` that gives its source card no workspace path
//! (`invalid_input` on `/filename`); it then opens the slot under the request's mutation with
//! the caller's provenance, and a repeat answers the first slot. `complete_upload`, `get_upload`
//! and `list_uploads` answer the store's slots in their wire form, with the store's refusals
//! unchanged.

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::PageRequest;
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{Digest, IdempotencyKey, JobId, UploadId, WorkspaceId};
use okf_jawn_contract::import::{
    CompleteUploadRequest, CreateUploadRequest, GetUploadRequest, ListUploadsRequest,
    ListUploadsResponse, MAX_UPLOAD_FILENAME_BYTES, Upload, UploadListState,
};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::application::ApplicationConfig;
use okf_jawn_core::context::{Attempt, OperationContext};
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::{ObjectInfo, Page, Provenance, StorageScope};
use okf_jawn_core::uploads::{UploadQuery, UploadRecord, UploadStore};
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{ALICE, Built, config, context, digest, instant, mutation, scope};
use crate::tenancy::{Tenancy, sha256_of, uploader};

/// The bytes the tests upload: ten of them.
const BYTES: &[u8] = b"0123456789";

fn key() -> Built<IdempotencyKey> {
    Ok(serde_json::from_value(json!(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    ))?)
}

/// Alice's local session running `operation`, under `number`'s mutation when it has one.
fn alice(operation: OperationName, number: Option<u128>) -> Built<OperationContext> {
    context(
        operation,
        ALICE,
        AccessRoute::LocalOwner,
        number.map(mutation),
    )
}

/// A slot of `size` bytes for `filename` in the fixture workspace, from the folder `2026/q3`.
fn create(filename: &str, size: &str) -> Built<CreateUploadRequest> {
    Ok(CreateUploadRequest {
        workspace_id: scope()?.workspace_id,
        filename: filename.to_owned(),
        relative_path: "2026/q3".to_owned(),
        size: size.to_owned(),
        sha256: None,
        idempotency_key: key()?,
    })
}

/// Open a ten-byte slot under `number`'s mutation through the service.
async fn opened(tenancy: &Tenancy, number: u128) -> Built<Upload> {
    Ok(tenancy
        .service
        .create_upload(
            &alice(OperationName::CreateUpload, Some(number))?,
            create("report.pdf", "10")?,
        )
        .await?)
}

fn complete(upload: UploadId, sha256: Digest) -> Built<CompleteUploadRequest> {
    Ok(CompleteUploadRequest {
        workspace_id: scope()?.workspace_id,
        upload_id: upload,
        sha256,
        idempotency_key: key()?,
    })
}

fn listing(
    state: Option<UploadListState>,
    limit: u16,
    cursor: Option<String>,
) -> Built<ListUploadsRequest> {
    Ok(ListUploadsRequest {
        workspace_id: scope()?.workspace_id,
        state,
        page: PageRequest { cursor, limit },
    })
}

fn ids(response: &ListUploadsResponse) -> Vec<UploadId> {
    response.items.iter().map(|item| item.id).collect()
}

/// A slot as the store holds one after `received` bytes, completed or taken by `consumed_by`.
fn record(
    number: u128,
    received: u64,
    complete: bool,
    consumed_by: Option<JobId>,
) -> Built<UploadRecord> {
    Ok(UploadRecord {
        id: UploadId(Uuid::from_u128(number)),
        filename: format!("upload-{number}.pdf"),
        relative_path: String::new(),
        expected_size: 10,
        expected_sha256: None,
        supplied_by: uploader(),
        created_at: instant()?,
        received_bytes: received,
        object: if complete {
            Some(ObjectInfo {
                digest: digest('c')?,
                size: 10,
            })
        } else {
            None
        },
        consumed_by,
    })
}

/// The wire form of `response` and of `record` in the fixture workspace agree, field by field.
fn assert_wire(response: &Upload, record: &UploadRecord) -> TestResult {
    assert_eq!(
        serde_json::to_value(response)?,
        serde_json::to_value(record.to_wire(scope()?.workspace_id))?
    );
    Ok(())
}

#[tokio::test]
async fn create_upload_opens_a_slot_with_the_names_as_supplied_and_the_callers_provenance()
-> TestResult {
    let tenancy = Tenancy::new()?;
    let mut caller = context(
        OperationName::CreateUpload,
        "bob",
        AccessRoute::BrowserSession,
        Some(mutation(31)),
    )?;
    caller.principal.client_id = Some("workspace-ui".to_owned());
    let mut request = create("Q3 Results.XLSX", "4096")?;
    request.relative_path = "Finance/2026 Q3/".to_owned();
    request.sha256 = Some(digest('e')?);
    let answered = tenancy.service.create_upload(&caller, request).await?;
    let slots = tenancy.uploads.slots(&scope()?)?;
    let [slot] = slots.as_slice() else {
        return Err(format!("not one slot: {slots:?}").into());
    };
    assert_eq!(slot.filename, "Q3 Results.XLSX");
    assert_eq!(slot.relative_path, "Finance/2026 Q3/", "preserved exactly");
    assert_eq!(slot.expected_size, 4096);
    assert_eq!(slot.expected_sha256, Some(digest('e')?));
    assert_eq!(
        slot.supplied_by,
        Provenance {
            subject: "bob".to_owned(),
            route: AccessRoute::BrowserSession,
            client_id: Some("workspace-ui".to_owned()),
        }
    );
    assert_wire(&answered, slot)?;
    assert_eq!(answered.size.as_deref(), Some("4096"));
    assert_eq!(answered.received_bytes, "0");
    assert!(!answered.complete);
    Ok(())
}

/// The slot is opened under the request's mutation: a resumed attempt of the same mutation is
/// answered the first slot by the store, and another mutation opens another.
#[tokio::test]
async fn a_repeated_create_upload_under_its_mutation_answers_the_first_slot() -> TestResult {
    let tenancy = Tenancy::new()?;
    let first = opened(&tenancy, 31).await?;
    let mut resumed = alice(OperationName::CreateUpload, Some(31))?;
    resumed.attempt = Attempt::Resumed;
    let again = tenancy
        .service
        .create_upload(&resumed, create("report.pdf", "10")?)
        .await?;
    assert_eq!(again.id, first.id);
    assert_eq!(tenancy.uploads.slots(&scope()?)?.len(), 1, "one slot");
    let other = opened(&tenancy, 32).await?;
    assert_ne!(other.id, first.id);
    assert_eq!(tenancy.uploads.slots(&scope()?)?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn create_upload_refuses_a_size_that_is_not_a_decimal_u64_on_size_before_the_store()
-> TestResult {
    let tenancy = Tenancy::new()?;
    // A sign, a space, an exponent, a point, a hexadecimal or non-ASCII digit, and 2^64.
    for size in [
        "",
        "+5",
        "-1",
        " 5",
        "5 ",
        "1e3",
        "5.0",
        "0x10",
        "\u{663}",
        "18446744073709551616",
    ] {
        let refused = err_of(
            tenancy
                .service
                .create_upload(
                    &alice(OperationName::CreateUpload, Some(31))?,
                    create("report.pdf", size)?,
                )
                .await,
        )?;
        assert_eq!(refused.code, ErrorCode::InvalidInput, "{size:?}");
        assert_eq!(refused.field.as_deref(), Some("/size"), "{size:?}");
    }
    assert_eq!(tenancy.uploads.creates()?, 0, "the store was never asked");
    Ok(())
}

/// The limit is the deployment's (`ApplicationConfig::max_upload_bytes`), pinned on both sides.
#[tokio::test]
async fn create_upload_refuses_a_size_over_the_deployments_limit_as_too_large_on_size() -> TestResult
{
    let tenancy = Tenancy::with_config(ApplicationConfig {
        max_upload_bytes: 100,
        ..config()
    })?;
    // The limit minus one, the limit (also with a leading zero), one over, and 2^64 - 1.
    for (number, size, announced) in [
        (31, "0", Some("0")),
        (32, "99", Some("99")),
        (33, "100", Some("100")),
        (34, "0100", Some("100")),
        (35, "101", None),
        (36, "18446744073709551615", None),
    ] {
        let outcome = tenancy
            .service
            .create_upload(
                &alice(OperationName::CreateUpload, Some(number))?,
                create("report.pdf", size)?,
            )
            .await;
        if let Some(announced) = announced {
            assert_eq!(outcome?.size.as_deref(), Some(announced), "{size}");
        } else {
            let refused = err_of(outcome)?;
            assert_eq!(refused.code, ErrorCode::TooLarge, "{size}");
            assert_eq!(refused.field.as_deref(), Some("/size"), "{size}");
        }
    }
    assert_eq!(
        tenancy.uploads.creates()?,
        4,
        "only the accepted sizes reach the store"
    );
    Ok(())
}

#[tokio::test]
async fn create_upload_refuses_a_file_name_that_gives_its_card_no_path_on_filename() -> TestResult {
    let tenancy = Tenancy::new()?;
    let long = "a".repeat(MAX_UPLOAD_FILENAME_BYTES.saturating_add(1));
    for (name, code) in [
        ("a:b.pdf", ErrorCode::InvalidInput),
        ("CON", ErrorCode::InvalidInput),
        ("notes?.txt", ErrorCode::InvalidInput),
        (long.as_str(), ErrorCode::TooLarge),
    ] {
        let refused = err_of(
            tenancy
                .service
                .create_upload(
                    &alice(OperationName::CreateUpload, Some(31))?,
                    create(name, "10")?,
                )
                .await,
        )?;
        assert_eq!(refused.code, code, "{name}");
        assert_eq!(refused.field.as_deref(), Some("/filename"), "{name}");
    }
    assert_eq!(tenancy.uploads.creates()?, 0, "the store was never asked");
    // A device name with an extension gives its card `CON-pdf.md`, a fine path.
    let slot = tenancy
        .service
        .create_upload(
            &alice(OperationName::CreateUpload, Some(32))?,
            create("CON.pdf", "10")?,
        )
        .await?;
    assert_eq!(slot.filename.as_deref(), Some("CON.pdf"));
    Ok(())
}

#[tokio::test]
async fn complete_upload_completes_the_slot_through_the_store_and_answers_it() -> TestResult {
    let tenancy = Tenancy::new()?;
    let slot = opened(&tenancy, 31).await?;
    tenancy.uploads.receive(&scope()?, slot.id, BYTES)?;
    let caller = alice(OperationName::CompleteUpload, Some(33))?;
    let completed = tenancy
        .service
        .complete_upload(&caller, complete(slot.id, sha256_of(BYTES)?)?)
        .await?;
    assert!(completed.complete);
    assert_eq!(completed.received_bytes, "10");
    let held = tenancy.uploads.held(&scope()?, slot.id)?;
    assert_eq!(
        held.object.as_ref().map(|object| &object.digest),
        Some(&sha256_of(BYTES)?)
    );
    assert_wire(&completed, &held)?;
    // Completing it again with the same digest answers the same slot.
    let again = tenancy
        .service
        .complete_upload(&caller, complete(slot.id, sha256_of(BYTES)?)?)
        .await?;
    assert_wire(&again, &held)?;
    Ok(())
}

/// The store's `Conflict` with `UploadMismatch` is the answer as the store gave it, for a short
/// upload, a digest that is not the bytes', and another digest on a complete slot.
#[tokio::test]
async fn complete_upload_passes_the_stores_upload_mismatch_through_unchanged() -> TestResult {
    let tenancy = Tenancy::new()?;
    let short = opened(&tenancy, 31).await?;
    tenancy.uploads.receive(&scope()?, short.id, b"0123")?;
    let other = opened(&tenancy, 32).await?;
    tenancy.uploads.receive(&scope()?, other.id, BYTES)?;
    let done = opened(&tenancy, 33).await?;
    tenancy.uploads.receive(&scope()?, done.id, BYTES)?;
    let caller = alice(OperationName::CompleteUpload, Some(34))?;
    tenancy
        .service
        .complete_upload(&caller, complete(done.id, sha256_of(BYTES)?)?)
        .await?;
    for (upload, sha256, received, digest_matches) in [
        (short.id, sha256_of(b"0123")?, "4", None),
        (other.id, digest('e')?, "10", Some(false)),
        (done.id, digest('e')?, "10", Some(false)),
    ] {
        let refused = err_of(
            tenancy
                .service
                .complete_upload(&caller, complete(upload, sha256.clone())?)
                .await,
        )?;
        let direct = err_of(tenancy.uploads.complete(&scope()?, upload, sha256).await)?;
        assert_eq!(
            serde_json::to_value(&refused)?,
            serde_json::to_value(&direct)?,
            "unchanged"
        );
        assert_eq!(refused.code, ErrorCode::Conflict);
        assert_eq!(
            refused.detail.as_deref(),
            Some(&ErrorDetail::UploadMismatch {
                expected_bytes: "10".to_owned(),
                received_bytes: received.to_owned(),
                digest_matches,
            })
        );
    }
    assert!(tenancy.uploads.held(&scope()?, short.id)?.object.is_none());
    assert!(tenancy.uploads.held(&scope()?, other.id)?.object.is_none());
    Ok(())
}

#[tokio::test]
async fn get_upload_reads_one_slot_of_the_workspace_as_it_stands() -> TestResult {
    let tenancy = Tenancy::new()?;
    let slot = opened(&tenancy, 31).await?;
    tenancy.uploads.receive(&scope()?, slot.id, b"0123")?;
    let read = tenancy
        .service
        .get_upload(
            &alice(OperationName::GetUpload, None)?,
            GetUploadRequest {
                workspace_id: scope()?.workspace_id,
                upload_id: slot.id,
            },
        )
        .await?;
    assert_eq!(read.received_bytes, "4", "as it stands now");
    assert_wire(&read, &tenancy.uploads.held(&scope()?, slot.id)?)?;
    // A slot of another workspace is not this workspace's.
    let elsewhere = StorageScope {
        tenant_id: scope()?.tenant_id,
        workspace_id: WorkspaceId(Uuid::from_u128(2)),
    };
    tenancy
        .uploads
        .put(&elsewhere, record(77, 0, false, None)?)?;
    let refused = err_of(
        tenancy
            .service
            .get_upload(
                &alice(OperationName::GetUpload, None)?,
                GetUploadRequest {
                    workspace_id: scope()?.workspace_id,
                    upload_id: UploadId(Uuid::from_u128(77)),
                },
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    Ok(())
}

/// The request's state and page reach the store exactly, and the store's pages are the answer:
/// newest first, continued by its cursor.
#[tokio::test]
async fn list_uploads_maps_state_and_page_exactly_and_pages_newest_first() -> TestResult {
    let tenancy = Tenancy::new()?;
    let first = opened(&tenancy, 31).await?;
    let second = opened(&tenancy, 32).await?;
    let third = opened(&tenancy, 33).await?;
    let reader = alice(OperationName::ListUploads, None)?;
    let page = tenancy
        .service
        .list_uploads(&reader, listing(None, 2, None)?)
        .await?;
    assert_eq!(ids(&page), vec![third.id, second.id]);
    let cursor = some(page.next_cursor.clone(), "the cursor")?;
    let rest = tenancy
        .service
        .list_uploads(&reader, listing(None, 2, Some(cursor.clone()))?)
        .await?;
    assert_eq!(ids(&rest), vec![first.id]);
    assert_eq!(rest.next_cursor, None);
    let waiting = tenancy
        .service
        .list_uploads(
            &reader,
            listing(Some(UploadListState::Waiting), 7, Some(cursor.clone()))?,
        )
        .await?;
    assert!(waiting.items.is_empty(), "no slot is complete");
    let query = |state, limit, cursor| UploadQuery {
        state,
        page: Page { cursor, limit },
    };
    let workspace = scope()?;
    assert_eq!(
        tenancy.uploads.queries()?,
        vec![
            (workspace.clone(), query(None, 2, None)),
            (workspace.clone(), query(None, 2, Some(cursor.clone()))),
            (
                workspace,
                query(Some(UploadListState::Waiting), 7, Some(cursor))
            ),
        ]
    );
    for (item, held) in page.items.iter().zip([&third, &second]) {
        assert_wire(item, &tenancy.uploads.held(&scope()?, held.id)?)?;
    }
    Ok(())
}

#[tokio::test]
async fn list_uploads_lists_the_slots_of_each_state() -> TestResult {
    let tenancy = Tenancy::new()?;
    let workspace = scope()?;
    let job = JobId(Uuid::from_u128(90));
    tenancy
        .uploads
        .put(&workspace, record(71, 4, false, None)?)?;
    tenancy
        .uploads
        .put(&workspace, record(72, 10, true, None)?)?;
    tenancy
        .uploads
        .put(&workspace, record(73, 10, true, Some(job))?)?;
    let reader = alice(OperationName::ListUploads, None)?;
    let id = |number: u128| UploadId(Uuid::from_u128(number));
    for (state, expected) in [
        (None, vec![id(73), id(72), id(71)]),
        (Some(UploadListState::Incomplete), vec![id(71)]),
        (Some(UploadListState::Waiting), vec![id(72)]),
        (Some(UploadListState::Consumed), vec![id(73)]),
    ] {
        let listed = tenancy
            .service
            .list_uploads(&reader, listing(state, 50, None)?)
            .await?;
        assert_eq!(ids(&listed), expected, "{state:?}");
    }
    let consumed = tenancy
        .service
        .list_uploads(&reader, listing(Some(UploadListState::Consumed), 50, None)?)
        .await?;
    let taken = some(consumed.items.first(), "the consumed slot")?;
    assert_eq!(taken.consumed_by, Some(job));
    assert!(taken.complete);
    Ok(())
}

#[tokio::test]
async fn list_uploads_answers_the_stores_refusal_of_a_cursor_it_did_not_write() -> TestResult {
    let tenancy = Tenancy::new()?;
    let refused = err_of(
        tenancy
            .service
            .list_uploads(
                &alice(OperationName::ListUploads, None)?,
                listing(None, 10, Some("not-a-position".to_owned()))?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/page/cursor"));
    Ok(())
}
