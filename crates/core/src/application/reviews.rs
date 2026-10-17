//! Confirmations and reviews: a person's revision-bound confirmation, the review it allows, and
//! review evidence read back with its coverage (SPEC section 8; Stage 1b design sections 6, 8
//! and 15).
//!
//! A confirmation is a person's explicit browser action (SPEC section 8: "Human confirmation
//! requires an explicit revision-bound browser action and application checks"), so issuing one
//! (`create_confirmation`), and consuming one (`create_review` here, `accept_proposal` in
//! `proposals`), is `forbidden` to a caller without a browser session, before any port is
//! called. The identity a request acts for is never review evidence: the reviewer is the
//! authenticated subject, never a value from the request.
//!
//! Issuing (`create_confirmation`) binds what the person was shown. A review confirms an item
//! and an acceptance a proposal (`invalid_input` on `/target` for any other pairing). For an
//! item, the revision is checked against the revision map first and the item's content digest
//! there (`portable::item_content_digest`) must be the request's (`conflict` on
//! `/content_digest`). For a proposal, it must be `open`, and the request's revision and digest
//! must be its `proposal_revision` and `content_digest` (`conflict` on `/revision` and
//! `/content_digest`). A confirmation expires `CONFIRMATION_TTL` after it is issued, and the
//! store makes it unique on the mutation.
//!
//! Recording (`create_review`) covers a whole item: the source's selection must be `all`
//! (`invalid_input` on `/source/selection`), because the confirmation binds the whole item's
//! digest and an export writes only whole-item reviews as OKF `verified`
//! (`portable::exported_verified`). In order: the source is checked as every citation is
//! (`sources::checked_citation`: the revision map first, the item, its path, its digest, the
//! locations filled); the item's content digest at that revision must be the request's
//! (`conflict` on `/content_digest`); the confirmation is consumed for this mutation (`review`,
//! the item, that revision and digest, the caller's session and subject); the review is recorded
//! with the subject as reviewer, now as its time and coverage `current`; then a `reviewed` event
//! naming the item. The review's identity derives from the mutation (`derive_review_id`), so a
//! resumed attempt that finds it among the item's reviews has consumed its confirmation already
//! and only records (again, idempotently) and announces, whatever the confirmation's expiry is
//! by then.
//!
//! Reading (`list_reviews`) resolves `at` once and returns every recorded review of the item.
//! Each kept source is remapped through the revision map, or its review is `invalidated` by the
//! purge (`proposals::remapped`); then its coverage is recomputed against the item's content
//! digest at the resolved revision (`portable::coverage_at`: `invalidated` and `imported` are
//! never promoted, and a review never extends to later content). `imported` lists the claims
//! the item's file makes in its OKF `verified` at that revision
//! (`portable::imported_verification`); each one's coverage is `imported`
//! (`portable::claim_coverage`), and none is a review.

use std::time::Duration;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::Timestamp;
use okf_jawn_contract::proposal::ProposalStatus;
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::review::{
    Confirmation, ConfirmationAction, ConfirmationTarget, CreateConfirmationRequest,
    CreateReviewRequest, ListReviewsRequest, ListReviewsResponse, Review, ReviewCoverage,
};

use super::ApplicationService;
use super::proposals::remapped;
use super::shared::{check_named, invalid, mutation_id, now, resolve, workspace_scope};
use super::sources::checked_citation;
use crate::confirmations::{ConfirmationConsume, ConfirmationCreate};
use crate::context::{Attempt, OperationContext};
use crate::events::{EventScope, NewEvent};
use crate::portable::{coverage_at, imported_verification, item_content_digest};
use crate::storage::derive_review_id;

/// How long a confirmation can be consumed after it is issued: ten minutes. Long enough for a
/// person to read what the confirmation shows and press the button; short enough that a
/// confirmation left in a forgotten tab cannot be used later.
pub const CONFIRMATION_TTL: Duration = Duration::from_secs(600);

/// Issue a confirmation bound to the action, target, revision and digest the person was shown.
///
/// # Errors
/// Returns `Forbidden` without a browser session, `InvalidInput` on `/target` for an action and
/// target that do not go together, the typed `NotFound` of an invalidated revision, `NotFound`
/// for an item or proposal the workspace does not hold, `Conflict` for content that is not what
/// was shown or a proposal that is not open, or any port error.
pub(super) async fn create_confirmation(
    service: &ApplicationService,
    context: &OperationContext,
    request: CreateConfirmationRequest,
) -> Result<Confirmation, ApiError> {
    let session = browser_session(context, "a confirmation")?;
    match (&request.action, &request.target) {
        (ConfirmationAction::Review, ConfirmationTarget::Item { .. })
        | (ConfirmationAction::AcceptProposal, ConfirmationTarget::Proposal { .. }) => {}
        _ => {
            return Err(invalid(
                "a review confirms an item and an acceptance confirms a proposal",
                "/target",
            ));
        }
    }
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    let ports = service.ports();
    match &request.target {
        ConfirmationTarget::Item { item_id } => {
            check_named(service, &scope, &request.revision).await?;
            let document = ports
                .versions
                .show(&scope, &request.revision, *item_id)
                .await?;
            if item_content_digest(&document)? != request.content_digest {
                return Err(
                    not_shown("the item's content at this revision").with_field("/content_digest")
                );
            }
        }
        ConfirmationTarget::Proposal { proposal_id } => {
            let proposal = ports.proposals.get(&scope, *proposal_id).await?;
            if proposal.status != ProposalStatus::Open {
                return Err(ApiError::new(
                    ErrorCode::Conflict,
                    "only an open proposal can be confirmed for acceptance",
                )
                .with_field("/target"));
            }
            if proposal.proposal_revision != request.revision {
                return Err(not_shown("the proposal's revision").with_field("/revision"));
            }
            if proposal.content_digest != request.content_digest {
                return Err(not_shown("the proposal's content").with_field("/content_digest"));
            }
        }
    }
    let expires_at = expiry(service)?;
    ports
        .confirmations
        .create(
            &scope,
            mutation,
            ConfirmationCreate {
                action: request.action,
                target: request.target,
                revision: request.revision,
                content_digest: request.content_digest,
                session_id: session,
                subject: context.principal.subject.clone(),
                expires_at,
            },
        )
        .await
}

/// Record a person's confirmed review of a whole item at one revision.
///
/// # Errors
/// Returns `Forbidden` without a browser session, `InvalidInput` on `/source/selection` for a
/// partial selection, the refusals of `sources::checked_citation`, `Conflict` on
/// `/content_digest` for content that is not the item's at that revision, the confirmation
/// store's refusal, or any port error.
pub(super) async fn create_review(
    service: &ApplicationService,
    context: &OperationContext,
    request: CreateReviewRequest,
) -> Result<Review, ApiError> {
    let session = browser_session(context, "a review")?;
    if request.source.selection != Selection::All {
        return Err(invalid(
            "a review covers a whole item: its selection is all",
            "/source/selection",
        ));
    }
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    let ports = service.ports();
    let (source, document) = checked_citation(service, &scope, request.source).await?;
    if item_content_digest(&document)? != request.content_digest {
        return Err(not_shown("the item's content at this revision").with_field("/content_digest"));
    }
    let id = derive_review_id(mutation);
    let recorded = context.attempt == Attempt::Resumed
        && ports
            .records
            .list_reviews(&scope, source.item_id)
            .await?
            .iter()
            .any(|review| review.id == id);
    if !recorded {
        ports
            .confirmations
            .consume(
                &scope,
                mutation,
                ConfirmationConsume {
                    confirmation_id: request.confirmation_id,
                    action: ConfirmationAction::Review,
                    target: ConfirmationTarget::Item {
                        item_id: source.item_id,
                    },
                    revision: source.revision.clone(),
                    content_digest: request.content_digest.clone(),
                    session_id: session,
                    subject: context.principal.subject.clone(),
                },
            )
            .await?;
    }
    let item = source.item_id;
    let review = ports
        .records
        .insert_review(
            &scope,
            mutation,
            Review {
                id,
                source,
                content_digest: request.content_digest,
                reviewer_subject: context.principal.subject.clone(),
                reviewed_at: now(service)?,
                coverage: ReviewCoverage::Current,
            },
        )
        .await?;
    ports
        .events
        .append(
            &EventScope::Workspace(scope),
            Some(mutation),
            NewEvent {
                kind: EventKind::Reviewed,
                revision: None,
                item_id: Some(item),
                job_id: None,
                connector_id: None,
                actor: None,
                operation: None,
                proposal_id: None,
                count: None,
            },
        )
        .await?;
    Ok(review)
}

/// Every recorded review of an item with its coverage at one resolved revision, and the claims
/// its file makes there.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated revision, `NotFound` for an item the revision
/// does not hold, or any port error.
pub(super) async fn list_reviews(
    service: &ApplicationService,
    context: &OperationContext,
    request: ListReviewsRequest,
) -> Result<ListReviewsResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let revision = resolve(service, &scope, &request.at).await?;
    let ports = service.ports();
    let document = ports
        .versions
        .show(&scope, &revision, request.item_id)
        .await?;
    let shown = item_content_digest(&document)?;
    let mut items = Vec::new();
    for mut review in ports.records.list_reviews(&scope, request.item_id).await? {
        let (source, invalidated_by) = remapped(service, &scope, review.source).await?;
        review.source = source;
        if invalidated_by.is_some() {
            review.coverage = ReviewCoverage::Invalidated;
        }
        review.coverage = coverage_at(&review, &shown);
        items.push(review);
    }
    Ok(ListReviewsResponse {
        items,
        imported: imported_verification(&document.properties),
    })
}

/// The browser session a confirmation is bound to.
///
/// # Errors
/// Returns `Forbidden` naming `what` when the request came in no browser session: a bearer or
/// connector caller cannot confirm (SPEC section 8).
pub(super) fn browser_session(context: &OperationContext, what: &str) -> Result<String, ApiError> {
    context.session_id.clone().ok_or_else(|| {
        ApiError::new(
            ErrorCode::Forbidden,
            format!("{what} needs a person's confirmation in a browser session"),
        )
    })
}

/// When a confirmation issued now expires.
///
/// # Errors
/// Returns `Internal` when that instant is not representable.
fn expiry(service: &ApplicationService) -> Result<Timestamp, ApiError> {
    let unrepresentable = || {
        ApiError::new(
            ErrorCode::Internal,
            "the confirmation expiry is not representable",
        )
    };
    let ttl = time::Duration::try_from(CONFIRMATION_TTL).map_err(|_| unrepresentable())?;
    let instant = service.now().checked_add(ttl).ok_or_else(unrepresentable)?;
    Timestamp::from_utc(instant).map_err(|_| unrepresentable())
}

/// The `Conflict` of content that is not what the person was shown.
fn not_shown(what: &str) -> ApiError {
    ApiError::new(
        ErrorCode::Conflict,
        format!("{what} is not what the confirmation was shown"),
    )
}
