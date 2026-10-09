//! Steps every handler of this service shares: the authorized scope, revision resolution with
//! the purge check, the commit path of a content write, and receipts.
//!
//! A content write is one path: the request's base revision is checked against the revision
//! map first (`reading::check_revision`), the edits are committed on top of it with the edit
//! check (`conformance::EditCheck`: OKF conformance refuses, OKF lint of the files the write
//! touched comes back as warnings), the new revision is indexed, a `changed` event is appended,
//! and, for a `MutationResult`, a receipt is recorded. Every step after the commit is
//! idempotent on the mutation identity (the commit replays, indexing a revision again is a
//! no-op, the event and the receipt are unique on the mutation), so a failure at any step
//! fails the request and a retry under the same idempotency key completes it without a
//! second effect.

use std::sync::Arc;

use okf_jawn_contract::access::Principal;
use okf_jawn_contract::common::MutationResult;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{EventKind, Receipt, ReceiptAudience};
use okf_jawn_contract::identity::{
    At, ItemId, MutationId, ReceiptId, Revision, Timestamp, WorkspacePath,
};
use okf_jawn_contract::item::ItemDocument;
use okf_jawn_contract::source::SourceReference;
use time::OffsetDateTime;
use uuid::Uuid;

use super::ApplicationService;
use crate::access::is_human_session;
use crate::conformance::EditCheck;
use crate::context::OperationContext;
use crate::events::{EventScope, NewEvent};
use crate::portable::item_content_digest;
use crate::reading::check_revision;
use crate::storage::{CommitChanges, Committed, Provenance, StorageScope, TreeEdit};

/// One content write of a handler.
pub(super) struct Write {
    /// The revision the request says it is based on; a moved head conflicts.
    pub(super) base: Revision,
    /// Commit message, without the trailer.
    pub(super) message: String,
    /// Edits applied in order.
    pub(super) edits: Vec<TreeEdit>,
    /// The item the write changes, for the `changed` event.
    pub(super) item: Option<ItemId>,
    /// The files the write touches, whose lint findings it reports.
    pub(super) touched: Touched,
}

/// The files one write touches, for the lint it reports (`conformance::EditCheck`).
#[derive(Default)]
pub(super) struct Touched {
    /// Paths it writes or creates.
    pub(super) paths: Vec<WorkspacePath>,
    /// Items whose path at the base it moves, changes or deletes; read after the purge check.
    pub(super) items: Vec<ItemId>,
    /// The type a type change redefines; its concepts count as touched.
    pub(super) of_type: Option<String>,
}

/// The storage scope of the operation's one authorized workspace.
///
/// # Errors
/// Returns `Internal` when dispatch authorized no workspace, which only a request without a
/// workspace target could cause.
pub(super) fn workspace_scope(context: &OperationContext) -> Result<StorageScope, ApiError> {
    context
        .primary_workspace()
        .map(|grant| grant.scope.clone())
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::Internal,
                "the operation was authorized for no workspace",
            )
        })
}

/// The durable write identity dispatch began for this mutation.
///
/// # Errors
/// Returns `Internal` when the request carried no idempotency key, which every write request
/// declares.
pub(super) fn mutation_id(context: &OperationContext) -> Result<MutationId, ApiError> {
    context.mutation.ok_or_else(|| {
        ApiError::new(
            ErrorCode::Internal,
            "a write runs only under a mutation identity",
        )
    })
}

/// Resolve a selector once: `latest` is the accepted head now; a named revision is checked
/// against the revision map a purge wrote before anything is read at it.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated revision, or any port error.
pub(super) async fn resolve(
    service: &ApplicationService,
    scope: &StorageScope,
    at: &At,
) -> Result<Revision, ApiError> {
    match at {
        At::Latest => service.ports().versions.head(scope).await,
        At::Revision { revision } => {
            check_revision(service.ports().records.as_ref(), scope, revision).await?;
            Ok(revision.clone())
        }
    }
}

/// Check a revision the request names (a base revision, a cited revision) against the
/// revision map before it is used.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated revision, or any port error.
pub(super) async fn check_named(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
) -> Result<(), ApiError> {
    check_revision(service.ports().records.as_ref(), scope, revision).await
}

/// Commit one write and index and announce the revision it produced.
///
/// # Errors
/// Returns the purge check's refusal of the base, the store's refusal (`Conflict` for a moved
/// head, the check's refusal of the candidate, a path collision, a header change), or any
/// later port error.
pub(super) async fn commit(
    service: &ApplicationService,
    context: &OperationContext,
    write: Write,
) -> Result<Committed, ApiError> {
    let scope = workspace_scope(context)?;
    let mutation_id = mutation_id(context)?;
    check_named(service, &scope, &write.base).await?;
    let ports = service.ports();
    let mut touched = write.touched.paths;
    for item in write.touched.items {
        touched.push(
            ports
                .versions
                .show(&scope, &write.base, item)
                .await?
                .summary
                .path,
        );
    }
    let check = Arc::new(EditCheck::new(&touched, write.touched.of_type));
    let committed = ports
        .versions
        .commit(
            &scope,
            CommitChanges {
                mutation_id,
                expected_head: write.base,
                author: Provenance::from_principal(&context.principal),
                message: write.message,
                edits: write.edits,
            },
            check,
        )
        .await?;
    ports
        .search
        .index_revision(&scope, committed.revision.clone())
        .await?;
    ports
        .events
        .append(
            &EventScope::Workspace(scope),
            Some(mutation_id),
            NewEvent {
                kind: EventKind::Changed,
                revision: Some(committed.revision.clone()),
                item_id: write.item,
                job_id: None,
                connector_id: None,
                actor: None,
                operation: None,
            },
        )
        .await?;
    Ok(committed)
}

/// Commit one write and answer with its revision, its receipt and the check's warnings.
///
/// # Errors
/// Returns any error of `commit` or of recording the receipt.
pub(super) async fn commit_with_receipt(
    service: &ApplicationService,
    context: &OperationContext,
    write: Write,
) -> Result<MutationResult, ApiError> {
    let committed = commit(service, context, write).await?;
    let scope = workspace_scope(context)?;
    let receipt_id = record_receipt(
        service,
        context,
        &scope,
        Some(mutation_id(context)?),
        Vec::new(),
    )
    .await?;
    Ok(MutationResult {
        revision: committed.revision,
        receipt_id,
        warnings: committed.warnings,
    })
}

/// Record what this operation returned and to whom, and give its identity.
///
/// A write passes its mutation, so a resumed attempt gets the first attempt's receipt back.
///
/// # Errors
/// Returns `Internal` when the clock is unrepresentable, or any error of the record store.
pub(super) async fn record_receipt(
    service: &ApplicationService,
    context: &OperationContext,
    scope: &StorageScope,
    mutation: Option<MutationId>,
    sources: Vec<SourceReference>,
) -> Result<ReceiptId, ApiError> {
    let receipt = Receipt {
        id: ReceiptId(Uuid::new_v4()),
        workspace_id: scope.workspace_id,
        operation_id: context.operation,
        principal_subject: context.principal.subject.clone(),
        route: context.principal.route.clone(),
        sources,
        returned_at: now()?,
        trace_id: None,
        audience: audience(&context.principal),
        invalidated_by: None,
    };
    let stored = service
        .ports()
        .records
        .insert_receipt(scope, mutation, receipt)
        .await?;
    Ok(stored.id)
}

/// Where returned content goes: a human session displays it to a person; every other route
/// (an agent's delegation, a service identity) takes it into a program's context.
pub(super) const fn audience(principal: &Principal) -> ReceiptAudience {
    if is_human_session(principal) {
        ReceiptAudience::HumanDisplay
    } else {
        ReceiptAudience::AgentContext
    }
}

/// The current instant in the canonical spelling.
///
/// # Errors
/// Returns `Internal` when the clock lies outside what a `Timestamp` writes.
pub(super) fn now() -> Result<Timestamp, ApiError> {
    Timestamp::from_utc(OffsetDateTime::now_utc())
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.0))
}

/// The document with the server's content digest, whatever the store put there.
///
/// # Errors
/// Returns `Internal` when the content cannot be serialized.
pub(super) fn with_content_digest(mut document: ItemDocument) -> Result<ItemDocument, ApiError> {
    document.content_digest = item_content_digest(&document)?;
    Ok(document)
}

/// An `InvalidInput` refusal of the field at `field`.
pub(super) fn invalid(message: impl Into<String>, field: &str) -> ApiError {
    ApiError::new(ErrorCode::InvalidInput, message).with_field(field)
}
