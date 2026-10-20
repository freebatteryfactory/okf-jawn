//! Upload slots: opening one, completing it, reading one and listing them (SPEC section 5:
//! "Upload slots are authenticated, size-bounded and finalized against expected length/hash").
//!
//! `create_upload` refuses, before the store is asked, what no slot may hold: a `filename` that
//! gives no workspace path for its source card (`conventions::source_card_name`, then
//! `WorkspacePath::from_supplied`), as `invalid_input` on `/filename`, and a name over its byte
//! bound, as `too_large` on its field (the contract's `CreateUploadRequest::check_names`, the
//! same check dispatch runs as `check_rules`, run here again so the handler holds the rule
//! itself); then a `size` that is not a decimal `u64` (only the ASCII digits `0` to `9`, at
//! least one, no sign or space; leading zeros are read as written, so `0100` is a hundred), as
//! `invalid_input` on `/size`, and a size over the deployment's limit
//! (`ApplicationConfig::max_upload_bytes`, which `get_session` reports), as `too_large` on
//! `/size`. Only then is the slot opened (`UploadStore::create`), under the request's mutation,
//! with the caller as its supplier (`Provenance::from_principal`); the store opens one slot per
//! mutation, so a repeat answers the first slot. The names are kept exactly as supplied.
//!
//! `complete_upload`, `get_upload` and `list_uploads` are the store's: the answer is each slot's
//! wire form (`UploadRecord::to_wire`), and every refusal of the store is the answer as it gave
//! it (`complete_upload`'s `Conflict` with `ErrorDetail::UploadMismatch`; `list_uploads`'
//! `invalid_input` on `/page/cursor` for a cursor the store did not write). `list_uploads` hands
//! the store the request's state and page as they are.
//!
//! The bytes themselves arrive through the `upload_content` transport, not an operation.
//! A discard first removes its slot, then requests tenant cleanup under a mutation derived
//! from the discard's mutation and wakes that durable job. A failed wake fails the request;
//! its resumed attempt answers the same tombstone and wakes the same job.

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::import::{
    CompleteUploadRequest, CreateUploadRequest, DiscardUploadRequest, GetUploadRequest,
    ListUploadsRequest, ListUploadsResponse, Upload,
};

use super::ApplicationService;
use super::shared::{invalid, mutation_id, workspace_scope};
use crate::context::OperationContext;
use crate::jobs::{JobScope, JobSpec, NewJob};
use crate::storage::{Page, Provenance, derive_clean_up_mutation_id};
use crate::uploads::{NewUpload, UploadQuery};

/// The request field an upload's announced size is refused on.
const SIZE: &str = "/size";

/// Open an upload slot for one source occurrence before its bytes arrive.
///
/// # Errors
/// Returns `InvalidInput` on `/filename` for a name that gives its source card no workspace
/// path, `TooLarge` on `/filename` or `/relative_path` for a name over its byte bound,
/// `InvalidInput` on `/size` for a size that is not a decimal `u64`, `TooLarge` on `/size` for
/// one over the deployment's upload limit, or any port error.
pub(super) async fn create_upload(
    service: &ApplicationService,
    context: &OperationContext,
    request: CreateUploadRequest,
) -> Result<Upload, ApiError> {
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    request.check_names()?;
    let expected_size = announced_size(&request.size, service.config().max_upload_bytes)?;
    let record = service
        .ports()
        .uploads
        .create(
            &scope,
            mutation,
            NewUpload {
                filename: request.filename,
                relative_path: request.relative_path,
                expected_size,
                expected_sha256: request.sha256,
                supplied_by: Provenance::from_principal(&context.principal),
            },
        )
        .await?;
    Ok(record.to_wire(scope.workspace_id))
}

/// Verify a slot's bytes against its announced length and the request's digest, and retain
/// them.
///
/// # Errors
/// Returns the store's `Conflict` with `ErrorDetail::UploadMismatch` on a length or hash
/// mismatch, `NotFound` for a slot the workspace does not hold, or any other port error.
pub(super) async fn complete_upload(
    service: &ApplicationService,
    context: &OperationContext,
    request: CompleteUploadRequest,
) -> Result<Upload, ApiError> {
    let scope = workspace_scope(context)?;
    let record = service
        .ports()
        .uploads
        .complete(&scope, request.upload_id, request.sha256)
        .await?;
    Ok(record.to_wire(scope.workspace_id))
}

/// Discard one occurrence before requesting and waking its tenant's durable cleanup.
///
/// # Errors
/// Returns the upload store's refusal or any job creation or queue error; an unsuccessful
/// wake is never reported as a successful discard response.
pub(super) async fn discard_upload(
    service: &ApplicationService,
    context: &OperationContext,
    request: DiscardUploadRequest,
) -> Result<Upload, ApiError> {
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    let ports = service.ports();
    let record = ports
        .uploads
        .discard(&scope, request.upload_id, mutation)
        .await?;
    let jobs = JobScope::Tenant(scope.tenant_id);
    let job = ports
        .records
        .create_job(
            &jobs,
            NewJob {
                mutation_id: derive_clean_up_mutation_id(mutation),
                initiator: Provenance::from_principal(&context.principal),
                spec: JobSpec::CleanUp,
            },
        )
        .await?;
    ports.queue.enqueue(jobs, job.id).await?;
    Ok(record.to_wire(scope.workspace_id))
}

/// Read one slot of the workspace as it now stands.
///
/// # Errors
/// Returns `NotFound` for a slot the workspace does not hold, or any port error.
pub(super) async fn get_upload(
    service: &ApplicationService,
    context: &OperationContext,
    request: GetUploadRequest,
) -> Result<Upload, ApiError> {
    let scope = workspace_scope(context)?;
    let record = service
        .ports()
        .uploads
        .get(&scope, request.upload_id)
        .await?;
    Ok(record.to_wire(scope.workspace_id))
}

/// List the workspace's slots, newest first, in the request's state, a page at a time.
///
/// # Errors
/// Returns the store's `InvalidInput` on `/page/cursor` for a cursor it did not write, or any
/// port error.
pub(super) async fn list_uploads(
    service: &ApplicationService,
    context: &OperationContext,
    request: ListUploadsRequest,
) -> Result<ListUploadsResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let page = service
        .ports()
        .uploads
        .list(
            &scope,
            UploadQuery {
                state: request.state,
                page: Page::from(request.page),
            },
        )
        .await?;
    Ok(ListUploadsResponse {
        items: page
            .items
            .iter()
            .map(|record| record.to_wire(scope.workspace_id))
            .collect(),
        next_cursor: page.next_cursor,
    })
}

/// The byte count `size` announces, at most `limit`.
///
/// # Errors
/// Returns `InvalidInput` on `/size` for text that is not a decimal `u64` (anything but one or
/// more ASCII digits, or a value of 2^64 or more), and `TooLarge` on `/size` for a count over
/// `limit`.
fn announced_size(size: &str, limit: u64) -> Result<u64, ApiError> {
    let not_decimal = || invalid("the size must be a decimal byte count", SIZE);
    // `u64::from_str` alone also takes a leading `+`.
    if size.is_empty() || !size.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(not_decimal());
    }
    let bytes = size.parse::<u64>().map_err(|_| not_decimal())?;
    if bytes > limit {
        return Err(ApiError::new(
            ErrorCode::TooLarge,
            format!("the upload announces {bytes} bytes; this deployment takes at most {limit}"),
        )
        .with_field(SIZE));
    }
    Ok(bytes)
}
