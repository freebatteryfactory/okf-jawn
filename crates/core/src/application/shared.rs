//! Steps every handler of this service shares: the authorized scope, revision resolution with
//! the purge check, the commit path of a content write, and receipts.
//!
//! A paginated read that resolves `at` (`log_items`, `list_items`, `search_items`, `get_links`)
//! resolves it for its first page only. Its cursor is this service's, not the store's: it
//! carries the operation's name and the tip that page resolved beside the store's own position
//! (and nothing of the query; it is not signed), and every later page
//! reads at that tip (`resolve_page`, `next_page`), so a write that lands between two pages of
//! a read at `latest` cannot repeat or skip an entry. `read_item` has its own cursor, which
//! carries its revision the same way.
//!
//! A content write is one path: the request's base revision is checked against the revision
//! map first (`reading::check_revision`), the edits are committed on top of it with the edit
//! check (`conformance::EditCheck`: OKF conformance refuses; the validator's warnings and OKF
//! lint about the files the write touched come back as warnings), the new revision is
//! indexed, a `changed` event is appended, and, for a `MutationResult`, a receipt is recorded. Every step after the commit is
//! idempotent on the mutation identity (the commit replays, indexing a revision again is a
//! no-op, the event and the receipt are unique on the mutation), so a failure at any step
//! fails the request and a retry under the same idempotency key completes it without a
//! second effect. A replayed commit answers the warnings the store kept when the commit was
//! written (`VersionStore::commit`), so the retry answers the warnings its first attempt was
//! given; for a commit the store kept none for, the store answers one warning that says so,
//! and the landed write still succeeds. This service runs no check of its own on a replay and
//! writes no file.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use okf_jawn_contract::access::Principal;
use okf_jawn_contract::common::{MutationResult, PageRequest};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{EventKind, Receipt, ReceiptAudience};
use okf_jawn_contract::identity::{
    At, ItemId, MutationId, ReceiptId, Revision, Timestamp, WorkspacePath,
};
use okf_jawn_contract::item::ItemDocument;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::source::SourceReference;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ApplicationService;
use crate::access::is_human_session;
use crate::conformance::EditCheck;
use crate::context::OperationContext;
use crate::events::{EventScope, NewEvent};
use crate::portable::item_content_digest;
use crate::reading::check_revision;
use crate::storage::{CommitChanges, Committed, Page, Provenance, StorageScope, TreeEdit};

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
    /// The files the write touches, whose warnings it reports.
    pub(super) touched: Touched,
}

/// The files one write touches, for the warnings it reports (`conformance::EditCheck`).
#[derive(Default)]
pub(super) struct Touched {
    /// Paths it writes or creates.
    pub(super) paths: Vec<WorkspacePath>,
    /// Items whose path at the base it moves, changes or deletes; read after the purge check.
    pub(super) items: Vec<ItemId>,
    /// The type a type change redefines; its concepts count as touched.
    pub(super) of_type: Option<String>,
}

/// Where a paginated read that resolves `at` resumes; opaque to the caller (base64url of this
/// JSON). The store's own cursor is only a position in a listing, so it is carried with the
/// revision that listing was read at.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PageCursor {
    /// The operation the cursor continues, by its name: another operation's cursor is refused.
    op: String,
    /// The revision the first page resolved; every later page reads at it.
    tip: Revision,
    /// The store's own continuation within the listing at `tip`.
    next: String,
    /// For a folder listing only: whether its first page decided it is a current listing
    /// (`items::list_items`), so every later page hides or shows archived items as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current: Option<bool>,
}

/// The request field a page cursor is refused on.
const PAGE_CURSOR: &str = "/page/cursor";

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

/// The revision one page of a paginated read is read at, the page to ask the store for, and,
/// on a continuation of a folder listing, the "current" answer its cursor carries.
///
/// The first page (no cursor) resolves the selector once (`resolve`). A continuation reads at
/// the tip its cursor carries, whatever `latest` is now, so a write that lands between two pages
/// neither repeats nor skips an entry: the store's cursor is a position in the listing of one
/// revision, and means nothing in another's. The tip is checked against the revision map again,
/// so a continuation never reads a revision a purge invalidated meanwhile.
///
/// What a cursor is bound to, exactly: the operation that issued it (its name is in the
/// cursor) and the tip. It is not signed and not bound to the query. So a cursor a caller
/// writes in this shape with this operation's name is accepted, and reads what `at` naming
/// that tip would read, from a position the store checks; and a cursor given to another
/// query of the same operation (another item, folder, text or direction) reads that query at
/// the tip from the position. A written cursor can also set a folder listing's "current" answer
/// to `false`, so the head is listed with its archived items; that shows nothing an explicit
/// revision would not (`at` naming an earlier revision, `search_items` with `include_archived`
/// and `get_item` show the same items under the same permission), because hiding archived items
/// is how a current listing looks, not an access rule.
///
/// # Errors
/// Returns `InvalidInput` on `/page/cursor` for a cursor that is not base64url JSON of this
/// shape (another field included), one that names another operation, or one whose tip is not
/// the revision `at` names; the typed `NotFound` of an invalidated revision; or any port
/// error.
pub(super) async fn resolve_page(
    service: &ApplicationService,
    scope: &StorageScope,
    operation: OperationName,
    at: &At,
    page: PageRequest,
) -> Result<(Revision, Page, Option<bool>), ApiError> {
    let Some(text) = page.cursor.as_deref() else {
        return Ok((resolve(service, scope, at).await?, page.into(), None));
    };
    let cursor: PageCursor = URL_SAFE_NO_PAD
        .decode(text)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| invalid("the cursor is not one this operation issues", PAGE_CURSOR))?;
    if cursor.op != operation.as_str() {
        return Err(invalid(
            "the cursor was issued by another operation",
            PAGE_CURSOR,
        ));
    }
    if matches!(at, At::Revision { revision } if revision != &cursor.tip) {
        return Err(invalid(
            "the cursor continues a read of another revision than the one named",
            PAGE_CURSOR,
        ));
    }
    check_named(service, scope, &cursor.tip).await?;
    Ok((
        cursor.tip,
        Page {
            cursor: Some(cursor.next),
            limit: page.limit,
        },
        cursor.current,
    ))
}

/// The cursor of the page after one read of `operation` at `tip`: the store's continuation,
/// bound to both (`resolve_page` reads it back), with a folder listing's "current" answer when `current`
/// gives one. `None` when the store has no more.
///
/// # Errors
/// Returns `Internal` when the cursor cannot be serialized.
pub(super) fn next_page(
    operation: OperationName,
    tip: &Revision,
    next: Option<String>,
    current: Option<bool>,
) -> Result<Option<String>, ApiError> {
    next.map(|next| {
        serde_json::to_vec(&PageCursor {
            op: operation.as_str().to_owned(),
            tip: tip.clone(),
            next,
            current,
        })
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
    })
    .transpose()
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
    let item = write.item;
    let committed = store_commit(service, context, write).await?;
    announce(
        service,
        workspace_scope(context)?,
        mutation_id(context)?,
        &committed.revision,
        item,
    )
    .await?;
    Ok(committed)
}

/// Commit one write in the store only, without indexing or announcing it: the step whose
/// `Conflict` alone means the store refused the write (a moved head, a path collision). A writer
/// that retries on a moved head retries only this step, and announces once it succeeded. The
/// answer is the store's, a replay's kept warnings included, unchanged.
///
/// # Errors
/// Returns the purge check's refusal of the base, the store's refusal, or any port error of
/// reading the touched items.
pub(super) async fn store_commit(
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
    ports
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
        .await
}

/// Index a revision a write produced and append its `changed` event under the write's
/// mutation; both are idempotent on it, so a resumed attempt repeats them without a second
/// effect.
///
/// # Errors
/// Returns any port error.
pub(super) async fn announce(
    service: &ApplicationService,
    scope: StorageScope,
    mutation_id: MutationId,
    revision: &Revision,
    item: Option<ItemId>,
) -> Result<(), ApiError> {
    let ports = service.ports();
    ports
        .search
        .index_revision(&scope, revision.clone())
        .await?;
    ports
        .events
        .append(
            &EventScope::Workspace(scope),
            Some(mutation_id),
            NewEvent {
                kind: EventKind::Changed,
                revision: Some(revision.clone()),
                item_id: item,
                job_id: None,
                connector_id: None,
                actor: None,
                operation: None,
                proposal_id: None,
                count: None,
            },
        )
        .await?;
    Ok(())
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
        returned_at: now(service)?,
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

/// The service's current instant in the canonical spelling.
///
/// # Errors
/// Returns `Internal` when the clock lies outside what a `Timestamp` writes.
pub(super) fn now(service: &ApplicationService) -> Result<Timestamp, ApiError> {
    Timestamp::from_utc(service.now()).map_err(|error| ApiError::new(ErrorCode::Internal, error.0))
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
