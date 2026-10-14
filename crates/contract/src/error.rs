//! Structured transport-independent application errors.
//!
//! `detail` carries the typed outcome a client can act on; adapters map `conflict`,
//! `already_issued` and `in_progress` to HTTP 409 with this body. `detail` is boxed so that
//! `Result<_, ApiError>` stays small.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Stable failure classifications; HTTP and MCP adapters preserve these.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Input violates the declared contract.
    InvalidInput,
    /// No acceptable identity was established.
    Unauthenticated,
    /// The principal lacks the required capability.
    Forbidden,
    /// The resource is absent or not visible to this principal.
    NotFound,
    /// A precondition, revision, draft-base, or idempotency-key conflict occurred.
    Conflict,
    /// A secret-bearing write already completed; replaying it never returns the secret again.
    AlreadyIssued,
    /// Another attempt with the same idempotency key holds a live lease; retry later.
    InProgress,
    /// A configured resource limit was exceeded.
    TooLarge,
    /// The requested representation or format is unsupported.
    Unsupported,
    /// A configured integration is unavailable.
    Unavailable,
    /// The operation was cancelled.
    Cancelled,
    /// An unexpected server failure occurred without exposing secrets.
    Internal,
    /// The operation is declared but this build does not implement it; never a success.
    NotImplemented,
}

/// Typed failure context the UI and agents can act on without parsing messages.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ErrorDetail {
    /// The connector was already issued under this idempotency key; its secret is not replayed.
    AlreadyIssued {
        /// Connector created by the original request.
        connector_id: crate::identity::ConnectorId,
    },
    /// An identical request is executing under a live lease.
    InProgress {
        /// Mutation holding the lease.
        mutation_id: crate::identity::MutationId,
        /// Whole seconds the caller should wait before retrying the same request.
        retry_after: u32,
    },
    /// One or more snapshotted items changed or were deleted after their drafts' bases.
    DraftConflict {
        /// Every conflicting item, never only the first.
        #[schemars(length(min = 1))]
        items: Vec<DraftConflictItem>,
    },
    /// The idempotency key was reused with a different request body.
    IdempotencyConflict {
        /// Operation that first used the key.
        operation: crate::metadata::OperationName,
    },
    /// The named revision was removed or rewritten by a purge; the code is `not_found`.
    Invalidated {
        /// The purge that removed or rewrote it.
        purge_id: crate::identity::PurgeId,
        /// After an item purge, the rewritten revision to re-pin to; absent after a
        /// workspace purge.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replacement: Option<crate::identity::Revision>,
    },
    /// An upload's received bytes do not match what was announced for it: their length, or
    /// their SHA-256. The code is `conflict`, and nothing was retained.
    UploadMismatch {
        /// The byte count announced when the slot was opened, as a decimal string.
        expected_bytes: String,
        /// The byte count the slot holds, as a decimal string.
        received_bytes: String,
        /// Whether the received bytes have the digest asked for; absent when the length
        /// already differs, so the digest was not compared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        digest_matches: Option<bool>,
    },
    /// The request's base revision (or expected head) is not the workspace's head; `field`
    /// names that request field. The code is `conflict`.
    StaleBase {
        /// The head the request was compared with.
        head: crate::identity::Revision,
    },
    /// The workspace is archived, and this operation is not one an archived workspace allows
    /// (`operations::ALLOWED_WHILE_ARCHIVED`). The code is `conflict`.
    WorkspaceArchived {
        /// When the workspace was archived.
        archived_at: crate::identity::Timestamp,
    },
}

/// One snapshotted item whose committed content moved after its draft's base.
///
/// The editor resolves it per item, with `changes` as the diff: keep mine is `save_draft` with
/// `base_revision` = `current_revision`, then `commit_items`; take theirs is `discard_draft`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DraftConflictItem {
    /// Item whose draft no longer applies cleanly.
    pub item_id: crate::identity::ItemId,
    /// Revision the draft was based on.
    pub draft_base: crate::identity::Revision,
    /// Head revision at which the item was found changed or deleted.
    pub current_revision: crate::identity::Revision,
    /// The item no longer exists at `current_revision`.
    pub deleted: bool,
    /// The item's committed changes from `draft_base` to `current_revision`.
    pub changes: Vec<crate::history::FileChange>,
}

/// A safe error response, not an internal backtrace.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiError {
    /// Stable code for programmatic handling.
    pub code: ErrorCode,
    /// Safe explanation for the caller.
    pub message: String,
    /// Input field needing correction. A JSON Pointer into the request (`/base_revision`,
    /// `/changes/2/body`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Correlation identifier, not a secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Typed context for conflict, replay, and in-progress outcomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Box<ErrorDetail>>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ApiError {}
impl ApiError {
    /// Construct a safe error without source-code details or credentials.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            field: None,
            request_id: None,
            detail: None,
        }
    }

    /// Attach typed context the caller can act on.
    #[must_use]
    pub fn with_detail(mut self, detail: ErrorDetail) -> Self {
        self.detail = Some(Box::new(detail));
        self
    }

    /// Name the input field that needs correction.
    #[must_use]
    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }
}
