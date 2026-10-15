//! Confirmations and reviews: a confirmation is issued only in a browser session and only for
//! what the person was shown; a review covers a whole item at one revision, consumes its
//! confirmation and names the authenticated reviewer; listing recomputes each review's coverage
//! at the resolved revision and never extends it to later content.

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::TextRange;
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::{At, IdempotencyKey, ItemId, PurgeId, Revision, Timestamp};
use okf_jawn_contract::item::ItemDocument;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::proposal::{Change, OpenProposalRequest, Proposal, ProposalStatus};
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::review::{
    Confirmation, ConfirmationAction, ConfirmationTarget, CreateConfirmationRequest,
    CreateReviewRequest, ImportedClaim, ListReviewsRequest, Review, ReviewCoverage,
};
use okf_jawn_contract::source::SourceReference;
use okf_jawn_core::application::CONFIRMATION_TTL;
use okf_jawn_core::context::OperationContext;
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::ports::Application;
use okf_jawn_core::proposals::ProposalStore;
use okf_jawn_core::storage::derive_review_id;
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    ALICE, Built, NOTE, alice, context, mutation, note, path, revision, scope, served_digest,
};
use crate::proposal_store::ProposalWorld;
use crate::proposals::{SESSION, in_session, plan, proposal_world, resumed, same};

/// The mutation that issues the fixture confirmation.
const CONFIRMING: u128 = 600;

/// The mutation that records the fixture review.
const REVIEWING: u128 = 650;

fn key() -> IdempotencyKey {
    IdempotencyKey(Uuid::from_u128(78))
}

/// The whole plan note at `at`, as `read_item` cites it.
fn whole_plan(at: char) -> Built<SourceReference> {
    Ok(SourceReference {
        workspace_id: scope()?.workspace_id,
        item_id: ItemId(Uuid::from_u128(NOTE)),
        path: path("notes/plan.md")?,
        revision: revision(at)?,
        digest: None,
        selection: Selection::All,
        locations: Vec::new(),
    })
}

fn confirm_item_request(at: char, document: &ItemDocument) -> Built<CreateConfirmationRequest> {
    Ok(CreateConfirmationRequest {
        workspace_id: scope()?.workspace_id,
        action: ConfirmationAction::Review,
        target: ConfirmationTarget::Item {
            item_id: ItemId(Uuid::from_u128(NOTE)),
        },
        revision: revision(at)?,
        content_digest: served_digest(document)?,
        idempotency_key: key(),
    })
}

fn confirming(number: u128) -> Built<OperationContext> {
    Ok(in_session(alice(
        OperationName::CreateConfirmation,
        Some(mutation(number)),
    )?))
}

/// Confirm Alice's review of the plan note at `a`.
async fn confirmed(world: &ProposalWorld) -> Built<Confirmation> {
    Ok(world
        .service
        .create_confirmation(
            &confirming(CONFIRMING)?,
            confirm_item_request('a', &plan(&revision('a')?)?)?,
        )
        .await?)
}

fn reviewing(number: u128) -> Built<OperationContext> {
    Ok(in_session(alice(
        OperationName::CreateReview,
        Some(mutation(number)),
    )?))
}

fn review_request(confirmation: &Confirmation) -> Built<CreateReviewRequest> {
    Ok(CreateReviewRequest {
        source: whole_plan('a')?,
        content_digest: served_digest(&plan(&revision('a')?)?)?,
        confirmation_id: confirmation.id,
        idempotency_key: key(),
    })
}

#[test]
fn a_confirmation_lasts_ten_minutes() {
    assert_eq!(CONFIRMATION_TTL, std::time::Duration::from_secs(600));
}

#[tokio::test]
async fn a_confirmation_binds_what_the_person_was_shown_in_their_session() -> TestResult {
    let world = proposal_world()?;
    let confirmation = confirmed(&world).await?;
    assert_eq!(confirmation.revision, revision('a')?);
    // Issued at noon, for ten minutes.
    assert_eq!(confirmation.expires_at.as_str(), "2026-10-09T12:10:00.000Z");
    let issued = world.confirmations.issued()?;
    let (id, held, consumed) = some(issued.first(), "the challenge")?;
    assert_eq!(*id, confirmation.id);
    assert_eq!(held.action, ConfirmationAction::Review);
    assert_eq!(
        held.target,
        ConfirmationTarget::Item {
            item_id: ItemId(Uuid::from_u128(NOTE))
        }
    );
    assert_eq!(held.revision, revision('a')?);
    assert_eq!(held.content_digest, served_digest(&plan(&revision('a')?)?)?);
    assert_eq!(held.session_id, SESSION);
    assert_eq!(held.subject, ALICE);
    assert_eq!(*consumed, None);
    // Unique on the mutation: a retry answers the same challenge.
    let again = world
        .service
        .create_confirmation(
            &resumed(confirming(CONFIRMING)?),
            confirm_item_request('a', &plan(&revision('a')?)?)?,
        )
        .await?;
    assert_eq!(again.id, confirmation.id);
    assert_eq!(world.confirmations.issued()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_confirmation_needs_a_browser_session() -> TestResult {
    let world = proposal_world()?;
    for route in [
        AccessRoute::McpDelegation,
        AccessRoute::Service,
        AccessRoute::LocalOwner,
    ] {
        let error = err_of(
            world
                .service
                .create_confirmation(
                    &context(
                        OperationName::CreateConfirmation,
                        ALICE,
                        route,
                        Some(mutation(CONFIRMING)),
                    )?,
                    confirm_item_request('a', &plan(&revision('a')?)?)?,
                )
                .await,
        )?;
        assert_eq!(error.code, ErrorCode::Forbidden);
    }
    assert!(world.confirmations.issued()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_confirmation_of_an_item_refuses_content_that_was_not_shown() -> TestResult {
    let world = proposal_world()?;
    let mut changed = plan(&revision('a')?)?;
    changed.body = "# Plan, as someone else saw it\n".to_owned();
    let error = err_of(
        world
            .service
            .create_confirmation(
                &confirming(CONFIRMING)?,
                confirm_item_request('a', &changed)?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(error.field.as_deref(), Some("/content_digest"));
    // A revision a purge removed is refused before anything is read at it.
    let purge_id = PurgeId(Uuid::from_u128(9));
    world.world.records.put_mapping(
        revision('a')?,
        RevisionMapping {
            purge_id,
            replacement: None,
        },
    )?;
    let error = err_of(
        world
            .service
            .create_confirmation(
                &confirming(CONFIRMING)?,
                confirm_item_request('a', &plan(&revision('a')?)?)?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound);
    assert!(matches!(
        error.detail.as_deref(),
        Some(ErrorDetail::Invalidated { .. })
    ));
    assert!(world.confirmations.issued()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn an_action_confirms_only_its_own_kind_of_target() -> TestResult {
    let world = proposal_world()?;
    let mut review_of_a_proposal = confirm_item_request('a', &plan(&revision('a')?)?)?;
    review_of_a_proposal.target = ConfirmationTarget::Proposal {
        proposal_id: okf_jawn_contract::identity::ProposalId(Uuid::from_u128(5)),
    };
    let mut acceptance_of_an_item = confirm_item_request('a', &plan(&revision('a')?)?)?;
    acceptance_of_an_item.action = ConfirmationAction::AcceptProposal;
    for request in [review_of_a_proposal, acceptance_of_an_item] {
        let error = err_of(
            world
                .service
                .create_confirmation(&confirming(CONFIRMING)?, request)
                .await,
        )?;
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("/target"));
    }
    assert!(world.confirmations.issued()?.is_empty());
    Ok(())
}

/// Open a proposal of one new note as Alice.
async fn one_proposal(world: &ProposalWorld) -> Built<Proposal> {
    Ok(world
        .service
        .open_proposal(
            &alice(OperationName::OpenProposal, Some(mutation(500)))?,
            OpenProposalRequest {
                workspace_id: scope()?.workspace_id,
                base_revision: revision('a')?,
                title: "Add".to_owned(),
                description: "Why".to_owned(),
                changes: vec![Change::Create {
                    path: path("notes/new.md")?,
                    type_name: "Note".to_owned(),
                    body: "# New\n".to_owned(),
                    properties: std::collections::BTreeMap::new(),
                    item_kind: None,
                }],
                idempotency_key: key(),
            },
        )
        .await?)
}

fn confirm_proposal_request(
    proposal: &Proposal,
    revision: Revision,
    content_digest: okf_jawn_contract::identity::Digest,
) -> Built<CreateConfirmationRequest> {
    Ok(CreateConfirmationRequest {
        workspace_id: scope()?.workspace_id,
        action: ConfirmationAction::AcceptProposal,
        target: ConfirmationTarget::Proposal {
            proposal_id: proposal.id,
        },
        revision,
        content_digest,
        idempotency_key: key(),
    })
}

#[tokio::test]
async fn a_confirmation_of_a_proposal_binds_its_revision_and_digest_while_it_is_open() -> TestResult
{
    let world = proposal_world()?;
    let proposal = one_proposal(&world).await?;
    let wrong_revision =
        confirm_proposal_request(&proposal, revision('a')?, proposal.content_digest.clone())?;
    let wrong_digest = confirm_proposal_request(
        &proposal,
        proposal.proposal_revision.clone(),
        crate::fixture::digest('9')?,
    )?;
    for (request, field) in [
        (wrong_revision, "/revision"),
        (wrong_digest, "/content_digest"),
    ] {
        let error = err_of(
            world
                .service
                .create_confirmation(&confirming(CONFIRMING)?, request)
                .await,
        )?;
        assert_eq!(error.code, ErrorCode::Conflict);
        assert_eq!(error.field.as_deref(), Some(field));
    }
    assert!(world.confirmations.issued()?.is_empty());
    let shown = confirm_proposal_request(
        &proposal,
        proposal.proposal_revision.clone(),
        proposal.content_digest.clone(),
    )?;
    let confirmation = world
        .service
        .create_confirmation(&confirming(CONFIRMING)?, shown.clone())
        .await?;
    assert_eq!(confirmation.revision, proposal.proposal_revision);
    // A proposal that is no longer open cannot be confirmed.
    world
        .proposals
        .update(
            &scope()?,
            Proposal {
                status: ProposalStatus::Declined,
                ..proposal
            },
            ProposalStatus::Open,
        )
        .await?;
    let error = err_of(
        world
            .service
            .create_confirmation(&confirming(CONFIRMING.saturating_add(1))?, shown)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(error.field.as_deref(), Some("/target"));
    Ok(())
}

#[tokio::test]
async fn a_review_records_the_whole_item_shown_for_the_authenticated_reviewer() -> TestResult {
    let world = proposal_world()?;
    let confirmation = confirmed(&world).await?;
    let review = world
        .service
        .create_review(&reviewing(REVIEWING)?, review_request(&confirmation)?)
        .await?;
    assert_eq!(review.id, derive_review_id(mutation(REVIEWING)));
    assert_eq!(same(&review.source)?, same(&whole_plan('a')?)?);
    assert_eq!(
        review.content_digest,
        served_digest(&plan(&revision('a')?)?)?
    );
    assert_eq!(review.reviewer_subject, ALICE);
    assert_eq!(review.reviewed_at.as_str(), "2026-10-09T12:00:00.000Z");
    assert_eq!(review.coverage, ReviewCoverage::Current);
    let held = world.records.reviews()?;
    assert_eq!(
        held.iter()
            .map(|(under, kept)| (*under, kept.id))
            .collect::<Vec<_>>(),
        [(mutation(REVIEWING), review.id)]
    );
    let issued = world.confirmations.issued()?;
    assert_eq!(
        some(issued.first(), "the confirmation")?.2,
        Some(mutation(REVIEWING))
    );
    let reviewed: Vec<_> = world
        .world
        .events
        .appended()?
        .into_iter()
        .filter(|(_, _, event)| event.kind == EventKind::Reviewed)
        .collect();
    let (_, under, event) = some(reviewed.first(), "the reviewed event")?;
    assert_eq!(reviewed.len(), 1);
    assert_eq!(*under, Some(mutation(REVIEWING)));
    assert_eq!(event.item_id, Some(ItemId(Uuid::from_u128(NOTE))));
    Ok(())
}

#[tokio::test]
async fn the_reviewer_is_the_authenticated_subject_who_holds_the_confirmation() -> TestResult {
    let world = proposal_world()?;
    // Carol confirms and reviews: the review is hers, whoever the confirmation was shown to.
    let carol = |operation, number| -> Built<OperationContext> {
        Ok(in_session(context(
            operation,
            "carol",
            AccessRoute::BrowserSession,
            Some(mutation(number)),
        )?))
    };
    let confirmation = world
        .service
        .create_confirmation(
            &carol(OperationName::CreateConfirmation, CONFIRMING)?,
            confirm_item_request('a', &plan(&revision('a')?)?)?,
        )
        .await?;
    // Alice cannot present Carol's confirmation.
    let error = err_of(
        world
            .service
            .create_review(&reviewing(REVIEWING)?, review_request(&confirmation)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert!(world.records.reviews()?.is_empty());
    let review = world
        .service
        .create_review(
            &carol(OperationName::CreateReview, REVIEWING)?,
            review_request(&confirmation)?,
        )
        .await?;
    assert_eq!(review.reviewer_subject, "carol");
    Ok(())
}

#[tokio::test]
async fn a_review_covers_a_whole_item_only() -> TestResult {
    let world = proposal_world()?;
    let confirmation = confirmed(&world).await?;
    let mut partial = review_request(&confirmation)?;
    partial.source.selection = Selection::Lines {
        range: TextRange { start: 1, end: 1 },
    };
    let error = err_of(
        world
            .service
            .create_review(&reviewing(REVIEWING)?, partial)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("/source/selection"));
    assert_eq!(world.confirmations.consumes(), 0);
    assert!(world.records.reviews()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_review_refuses_content_that_is_not_the_items_at_that_revision() -> TestResult {
    let world = proposal_world()?;
    let confirmation = confirmed(&world).await?;
    let mut request = review_request(&confirmation)?;
    request.content_digest = crate::fixture::digest('9')?;
    let error = err_of(
        world
            .service
            .create_review(&reviewing(REVIEWING)?, request)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(error.field.as_deref(), Some("/content_digest"));
    assert_eq!(world.confirmations.consumes(), 0);
    assert!(world.records.reviews()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_review_needs_a_browser_session_before_anything_is_read() -> TestResult {
    let world = proposal_world()?;
    let confirmation = confirmed(&world).await?;
    let shows = world.world.versions.shows();
    let error = err_of(
        world
            .service
            .create_review(
                &alice(OperationName::CreateReview, Some(mutation(REVIEWING)))?,
                review_request(&confirmation)?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(world.world.versions.shows(), shows);
    assert_eq!(world.confirmations.consumes(), 0);
    assert!(world.records.reviews()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_resumed_review_records_once_and_finishes_after_its_confirmation_expired() -> TestResult {
    // A failure before the review was recorded: the retry consumes again and records.
    let world = proposal_world()?;
    let confirmation = confirmed(&world).await?;
    world.records.fail_next_insert();
    err_of(
        world
            .service
            .create_review(&reviewing(REVIEWING)?, review_request(&confirmation)?)
            .await,
    )?;
    let review = world
        .service
        .create_review(
            &resumed(reviewing(REVIEWING)?),
            review_request(&confirmation)?,
        )
        .await?;
    assert_eq!(review.id, derive_review_id(mutation(REVIEWING)));
    assert_eq!(world.records.reviews()?.len(), 1);
    // A failure after it was recorded, retried once the confirmation expired: the review its
    // first attempt recorded is found, so the confirmation is not consumed again.
    let world = proposal_world()?;
    let confirmation = confirmed(&world).await?;
    world.events.fail_next(EventKind::Reviewed)?;
    err_of(
        world
            .service
            .create_review(&reviewing(REVIEWING)?, review_request(&confirmation)?)
            .await,
    )?;
    world
        .confirmations
        .set_now(Timestamp::try_from("2026-10-09T13:00:00.000Z".to_owned())?)?;
    let consumes = world.confirmations.consumes();
    let review = world
        .service
        .create_review(
            &resumed(reviewing(REVIEWING)?),
            review_request(&confirmation)?,
        )
        .await?;
    assert_eq!(world.confirmations.consumes(), consumes);
    assert_eq!(review.reviewed_at.as_str(), "2026-10-09T12:00:00.000Z");
    assert_eq!(world.records.reviews()?.len(), 1);
    let reviewed = world
        .world
        .events
        .appended()?
        .into_iter()
        .filter(|(_, _, event)| event.kind == EventKind::Reviewed)
        .count();
    assert_eq!(reviewed, 1);
    // A first attempt never skips the confirmation, even when its review id were found.
    let error = err_of(
        world
            .service
            .create_review(
                &reviewing(REVIEWING.saturating_add(1))?,
                review_request(&confirmation)?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict);
    Ok(())
}

fn list_request(at: At) -> Built<ListReviewsRequest> {
    Ok(ListReviewsRequest {
        workspace_id: scope()?.workspace_id,
        item_id: ItemId(Uuid::from_u128(NOTE)),
        at,
    })
}

async fn listed(
    world: &ProposalWorld,
    at: At,
) -> Built<okf_jawn_contract::review::ListReviewsResponse> {
    Ok(world
        .service
        .list_reviews(&alice(OperationName::ListReviews, None)?, list_request(at)?)
        .await?)
}

#[tokio::test]
async fn a_review_never_extends_to_later_content() -> TestResult {
    let world = proposal_world()?;
    let confirmation = confirmed(&world).await?;
    world
        .service
        .create_review(&reviewing(REVIEWING)?, review_request(&confirmation)?)
        .await?;
    // The note is edited after the review: the head `b` holds other content.
    let mut edited = plan(&revision('b')?)?;
    edited.body = "# Plan, edited later\n".to_owned();
    world.world.versions.put_document(&revision('b')?, edited)?;
    world.world.versions.set_head(revision('b')?)?;
    let latest = listed(&world, At::Latest).await?;
    let review = some(latest.items.first(), "the review")?;
    assert_eq!(latest.items.len(), 1);
    assert_eq!(review.coverage, ReviewCoverage::Changed);
    // At the reviewed revision it covers what it covered.
    let reviewed = listed(
        &world,
        At::Revision {
            revision: revision('a')?,
        },
    )
    .await?;
    assert_eq!(
        some(reviewed.items.first(), "the review")?.coverage,
        ReviewCoverage::Current
    );
    Ok(())
}

/// A review of the plan note at `at` with the server's digest of it, recorded with `coverage`.
fn recorded(id: u128, at: char, coverage: ReviewCoverage) -> Built<Review> {
    Ok(Review {
        id: okf_jawn_contract::identity::ReviewId(Uuid::from_u128(id)),
        source: whole_plan(at)?,
        content_digest: served_digest(&plan(&revision(at)?)?)?,
        reviewer_subject: ALICE.to_owned(),
        reviewed_at: Timestamp::try_from("2026-10-01T12:00:00.000Z".to_owned())?,
        coverage,
    })
}

#[tokio::test]
async fn an_invalidated_review_stays_invalidated_and_a_purged_citation_is_remapped() -> TestResult {
    let world = proposal_world()?;
    let purge_id = PurgeId(Uuid::from_u128(9));
    // Stored invalidated (a purge scrubbed it): never promoted, though the digest matches.
    world.records.put_review(
        &scope()?,
        mutation(1),
        recorded(1, 'a', ReviewCoverage::Invalidated)?,
    )?;
    // A review at `e`, a revision a purge rewrote to `a`, where the item still is: remapped.
    world.records.put_review(
        &scope()?,
        mutation(2),
        recorded(2, 'e', ReviewCoverage::Current)?,
    )?;
    world.world.records.put_mapping(
        revision('e')?,
        RevisionMapping {
            purge_id,
            replacement: Some(revision('a')?),
        },
    )?;
    // A review at `f`, a revision a purge removed: invalidated.
    world.records.put_review(
        &scope()?,
        mutation(3),
        recorded(3, 'f', ReviewCoverage::Current)?,
    )?;
    world.world.records.put_mapping(
        revision('f')?,
        RevisionMapping {
            purge_id,
            replacement: None,
        },
    )?;
    let response = listed(&world, At::Latest).await?;
    let found: Vec<(u128, Revision, ReviewCoverage)> = response
        .items
        .iter()
        .map(|review| {
            (
                review.id.0.as_u128(),
                review.source.revision.clone(),
                review.coverage.clone(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (1, revision('a')?, ReviewCoverage::Invalidated),
            (2, revision('a')?, ReviewCoverage::Current),
            (3, revision('f')?, ReviewCoverage::Invalidated),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn the_items_imported_claims_are_listed_and_are_no_reviews() -> TestResult {
    let world = proposal_world()?;
    let at = revision('a')?;
    let claimed = note(
        NOTE,
        &at,
        "notes/plan.md",
        "# Plan\n",
        json!({
            "type": "Note",
            "verified": [{ "by": "human:ann", "at": "2026-01-01" }, { "by": "agent:x" }]
        }),
    )?;
    world.world.versions.replace_document(&at, claimed)?;
    let response = listed(&world, At::Latest).await?;
    assert!(response.items.is_empty());
    assert_eq!(
        response.imported,
        [
            ImportedClaim {
                by: Some("human:ann".to_owned()),
                at: Some("2026-01-01".to_owned())
            },
            ImportedClaim {
                by: Some("agent:x".to_owned()),
                at: None
            },
        ]
    );
    assert!(
        world.records.reviews()?.is_empty(),
        "no review was recorded"
    );
    Ok(())
}

#[tokio::test]
async fn reviews_are_listed_at_a_revision_resolved_once_and_checked_against_purges() -> TestResult {
    let world = proposal_world()?;
    let purge_id = PurgeId(Uuid::from_u128(9));
    world.world.records.put_mapping(
        revision('a')?,
        RevisionMapping {
            purge_id,
            replacement: None,
        },
    )?;
    let error = err_of(
        world
            .service
            .list_reviews(
                &alice(OperationName::ListReviews, None)?,
                list_request(At::Revision {
                    revision: revision('a')?,
                })?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(world.records.lists()?, 0, "nothing is read at it");
    Ok(())
}
