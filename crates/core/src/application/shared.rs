//! Steps every handler of this service shares: the authorized scope, revision resolution with
//! the purge check, the commit path of a content write, and receipts.
//!
//! A content write is one path: the request's base revision is checked against the revision
//! map first (`reading::check_revision`), the edits are committed on top of it with the edit
//! check (`conformance::EditCheck`: OKF conformance refuses; the validator's warnings and OKF
//! lint about the files the write touched come back as warnings), the new revision is
//! indexed, a `changed` event is appended, and, for a `MutationResult`, a receipt is recorded. Every step after the commit is
//! idempotent on the mutation identity (the commit replays, indexing a revision again is a
//! no-op, the event and the receipt are unique on the mutation), so a failure at any step
//! fails the request and a retry under the same idempotency key completes it without a
//! second effect. A replayed commit's warnings are rebuilt by the same check over the
//! committed tree (`checked_again`), so the retry answers the warnings the first attempt
//! would have.

use std::path::PathBuf;
use std::sync::Arc;

use okf_jawn_contract::access::Principal;
use okf_jawn_contract::common::{MutationResult, Warning};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::{EventKind, Receipt, ReceiptAudience};
use okf_jawn_contract::identity::{
    At, ItemId, MutationId, ReceiptId, Revision, Timestamp, WorkspacePath,
};
use okf_jawn_contract::item::ItemDocument;
use okf_jawn_contract::source::SourceReference;
use uuid::Uuid;

use super::ApplicationService;
use crate::access::is_human_session;
use crate::conformance::EditCheck;
use crate::context::OperationContext;
use crate::events::{EventScope, NewEvent};
use crate::portable::item_content_digest;
use crate::reading::check_revision;
use crate::storage::{
    CandidateCheck, CommitChanges, Committed, Page, Provenance, StorageScope, TreeEdit,
};

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

/// A private directory the tree of a replayed commit is written to for its check; removed when
/// dropped.
struct Scratch(PathBuf);

/// How many entries of a folder are read at a time when a replayed commit's tree is read back.
const TREE_PAGE: u16 = 500;
/// The index the store maintains in every folder.
const MAINTAINED_INDEX: &str = "index.md";
/// The change log the store maintains at the root.
const CHANGE_LOG: &str = "log.md";

impl Scratch {
    /// A new, empty private directory under the system's temporary directory.
    fn create() -> Result<Self, ApiError> {
        let root = std::env::temp_dir().join(format!("okf-jawn-recheck-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).map_err(|error| scratch_error(&error))?;
        Ok(Self(root))
    }

    /// Write one file of the tree.
    fn write(&self, path: &WorkspacePath, bytes: &[u8]) -> Result<(), ApiError> {
        let target = self.0.join(path.as_str());
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| scratch_error(&error))?;
        }
        std::fs::write(&target, bytes).map_err(|error| scratch_error(&error))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a directory left under the temporary directory harms nothing.
        let _left = std::fs::remove_dir_all(&self.0);
    }
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
/// that retries on a moved head retries only this step, and announces once it succeeded.
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
    let mut committed = ports
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
            check.clone(),
        )
        .await?;
    if committed.replayed {
        committed.warnings = checked_again(service, &scope, &committed.revision, check).await?;
    }
    Ok(committed)
}

/// The warnings `check` gives over the committed tree of `revision`, read back from the store.
///
/// A store that replays a commit runs no check and returns no warnings, so the warnings the
/// first attempt's check produced were lost with that attempt. They are rebuilt by running the
/// same check, with the same touched set, over the tree the commit holds: every item file, each
/// folder's maintained `index.md` and the root `log.md`, the Markdown files of the bundle the
/// check reads (the store's side records under `.okf/` are not Markdown, and no OKF check reads
/// them). That is the candidate tree the check saw when the store wrote the commit, so a
/// replayed write answers the warnings its first attempt would have. Ingest rebuilds an
/// import's warnings the same way. The cleaner fix is R-I10, where the store keeps the warnings
/// with the commit; it is a later shared change.
///
/// The tree is written to a private directory under the system's temporary directory, because
/// a `CandidateCheck` reads a directory; it is removed when the check returns.
///
/// # Errors
/// Returns any port error of reading the tree, `Internal` when the directory cannot be
/// written, or the check's own error.
pub(super) async fn checked_again(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
    check: Arc<dyn CandidateCheck>,
) -> Result<Vec<Warning>, ApiError> {
    let versions = &service.ports().versions;
    let mut files: Vec<(WorkspacePath, Vec<u8>)> = Vec::new();
    let mut folders: Vec<Option<WorkspacePath>> = vec![None];
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
                        limit: TREE_PAGE,
                    },
                )
                .await?;
            for item in listing.items {
                let bytes = versions.read_file(scope, revision, &item.path).await?;
                files.push((item.path, bytes));
            }
            folders.extend(listing.folders.into_iter().map(Some));
            match listing.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        let index = folder.as_ref().map_or_else(
            || MAINTAINED_INDEX.to_owned(),
            |folder| format!("{}/{MAINTAINED_INDEX}", folder.as_str()),
        );
        files.extend(file_if_present(service, scope, revision, index).await?);
    }
    files.extend(file_if_present(service, scope, revision, CHANGE_LOG.to_owned()).await?);
    tokio::task::spawn_blocking(move || {
        let scratch = Scratch::create()?;
        for (path, bytes) in &files {
            scratch.write(path, bytes)?;
        }
        check.check(&scratch.0)
    })
    .await
    .map_err(|error| {
        ApiError::new(
            ErrorCode::Internal,
            format!("the replayed commit's check stopped: {error}"),
        )
    })?
}

/// The versioned file at `path` of `revision`, when the tree holds one.
async fn file_if_present(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
    path: String,
) -> Result<Option<(WorkspacePath, Vec<u8>)>, ApiError> {
    let path = WorkspacePath::try_from(path).map_err(|error| {
        ApiError::new(
            ErrorCode::Internal,
            format!("a maintained file has no workspace path: {error}"),
        )
    })?;
    match service
        .ports()
        .versions
        .read_file(scope, revision, &path)
        .await
    {
        Ok(bytes) => Ok(Some((path, bytes))),
        Err(error) if error.code == ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The `Internal` refusal of a scratch directory the system would not let us write.
fn scratch_error(error: &std::io::Error) -> ApiError {
    ApiError::new(
        ErrorCode::Internal,
        format!("the replayed commit's tree could not be written for its check: {error}"),
    )
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
