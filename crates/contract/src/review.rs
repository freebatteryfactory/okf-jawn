//! Review evidence covers exact content, never all future edits.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// How recorded review evidence relates to current content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewCoverage {
    /// The recorded content digest still matches.
    Current,
    /// Content changed after this review.
    Changed,
    /// Imported metadata has no authenticated in-app review event.
    Imported,
    /// No covering review exists.
    Unreviewed,
}

/// A review action linked to the content that was actually displayed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Review {
    /// Durable review identity.
    pub id: crate::identity::ReviewId,
    /// Reviewed revision and range.
    pub source: crate::source::SourceReference,
    /// Digest of the exact reviewed text.
    pub content_digest: crate::identity::Digest,
    /// Server-established subject; not supplied in request.
    pub reviewer_subject: String,
    /// RFC 3339 action time.
    pub reviewed_at: String,
    /// Coverage relative to current content.
    pub coverage: ReviewCoverage,
}

/// Record an explicit review of a displayed revision, without approving its business claims.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateReviewRequest {
    /// Exact source shown.
    pub source: crate::source::SourceReference,
    /// Digest displayed in the review confirmation.
    pub content_digest: crate::identity::Digest,
    /// Session-bound confirmation issued for this revision.
    pub confirmation_id: String,
}

/// Read review evidence without rewriting it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListReviewsRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
}

/// Recorded reviews and current coverage.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListReviewsResponse {
    /// Reviews, including non-current ones.
    pub items: Vec<Review>,
}

/// Prepare explicit human confirmation bound to an action and immutable target.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateConfirmationRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// review or `accept_proposal`; not an arbitrary method.
    pub action: String,
    /// Item or proposal identity.
    pub target_id: String,
    /// Displayed content revision.
    pub revision: crate::identity::Revision,
    /// Displayed content digest.
    pub content_digest: crate::identity::Digest,
}

/// A short-lived challenge; issuance alone is not a review.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Confirmation {
    /// Opaque session-bound challenge.
    pub id: String,
    /// RFC 3339 expiration.
    pub expires_at: String,
    /// Bound revision.
    pub revision: crate::identity::Revision,
}
