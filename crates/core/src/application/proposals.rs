//! Proposals and their discussion: open, list and read a proposal, accept or decline it, and
//! comment on it (SPEC section 8; Stage 1b design section 5).
//!
//! Opening (`open_proposal`) writes a candidate commit retained off the accepted line
//! (`VersionStore::create_candidate`), then the proposal's row, then a `proposal_updated` event:
//!
//! - Before any port is called: a `create` change whose properties supply the application header
//!   is refused as `create_item` refuses it (`items::refuse_supplied_header`); a `create` of a
//!   View answers `not_implemented` until the View file layout lands (construction plan M1.6),
//!   and the contract's check has already refused a `create` of a source; a proposal of supplied
//!   text (`ProposalKind::SupplyExtraction`) is accepted only from an agent's delegated route
//!   (`invalid_input` on `/changes` otherwise: a person corrects text with `correct_digest`).
//! - The base is checked against the revision map first; it need not be the head.
//! - An `edit` change may leave the header out of its properties or echo the stored one, and is
//!   refused when it changes it (`items::refuse_header_change` against the item at the base).
//! - A supply change names a source whose conversion, at the base, is partial, failed or
//!   unsupported (`conflict` on its `item_id` otherwise; `invalid_input` there for an item that
//!   is no source), and its `based_on` is that source's digest at the base (`conflict` on its
//!   `based_on` otherwise).
//! - The candidate is judged by the edit check (`conformance::EditCheck`) over every path the
//!   changes write (a create's path; the path at the base of the item an edit, an archive, a move
//!   or a supply names; a move's destination) and the files the store reports it rewrote. A
//!   refusal stores nothing: no candidate reference, no row, no event. Lint and the validator's
//!   softer findings are the proposal's `warnings`.
//! - Identities derive from the mutation: the proposal's (`derive_proposal_id`), and the n-th
//!   created item's (`derive_item_id(mutation, n)`), so a resumed attempt names the same
//!   candidate reference and the same items. The candidate, the row and the event are each
//!   idempotent on the mutation, so a retry answers the proposal its first attempt stored.
//! - `content_digest` is SHA-256 of the canonical JSON of the changes, keys sorted
//!   (`mutations::request_digest`); a confirmation to accept binds it with `proposal_revision`.
//!
//! Reading (`list_proposals`, `get_proposal`) asks the store of the authorized workspace only, so
//! a proposal of another workspace is `not_found`. A proposal's candidate revision is readable by
//! anyone who may read the workspace (review N4): it is not on the accepted line, and a citation
//! of it names that revision, which a read with `at` naming it reads like any other.
//!
//! Accepting (`accept_proposal`) runs in this fixed order, every step idempotent on the
//! mutation:
//!
//! 1. A browser session is required (`forbidden` without one, before any port): a confirmation
//!    is a person's browser action. The proposal must be `open`: `declined` and `conflict` are
//!    refused with `conflict`, and so is an `accepted` one unless this mutation accepted it.
//!    `proposal_revision` must be the proposal's (`conflict` on `/proposal_revision`).
//! 2. The confirmation is consumed for this mutation (`accept_proposal`, the proposal, its
//!    revision and content digest, the caller's session and subject), before anything is
//!    promoted, so no promotion happens without a consumed confirmation.
//! 3. The candidate is promoted onto `expected_head`. The store's stale refusal, a `conflict`
//!    with `StaleBase { head }` (the head moved, or the candidate was not drafted on the head,
//!    which by contract ruling 5 is a stale base too), makes the proposal `conflict` (it needs
//!    reconciliation; a compare-and-set from `open`), appends a `proposal_updated` event naming
//!    it, and answers `conflict` with that `StaleBase`: on `/expected_head` when the head is not
//!    the request's `expected_head`, and with no field when it is, because then the request
//!    named the head and the proposal is what is stale. Any other refusal, the store's
//!    plain `conflict` of a candidate reference that is not the proposal's included, leaves the
//!    status as it was and is answered as given.
//! 4. The status becomes `accepted`, a compare-and-set from `open`. When that is refused and the
//!    proposal is `conflict`, it becomes `accepted` from `conflict` instead: this mutation's
//!    promotion landed (step 3 answered it, or a resumed attempt found it by the mutation), so
//!    the mark is false. It was set by a second acceptance in the window after this promotion
//!    landed and before this step, which the store refused as stale because this promotion had
//!    moved the head off the proposal's base.
//! 5. The new revision is indexed and a `changed` event appended (`shared::announce`); it names
//!    no item, because a proposal may change several.
//! 6. A `proposal_updated` event naming the proposal.
//! 7. A receipt; the answer carries the warnings stored with the proposal, the findings of the
//!    tree that was accepted (the promotion runs no check of its own).
//!
//! A resumed attempt first asks whether this mutation's promotion already landed
//! (`VersionStore::find_commit` after the proposal's base, on which a promotion sits): when it
//! did, steps 1 to 3 are behind it (the confirmation was consumed before it), so the attempt runs
//! steps 4 to 7 again, whatever the confirmation's expiry is by then. A resumed attempt that
//! finds the proposal `conflict` asks the store to write that mark again under its mutation,
//! which the store takes as a retry only from the mutation that wrote it: then this attempt's
//! first one marked it, so the attempt appends its event and answers its stale refusal again,
//! naming the head as it is now; otherwise the proposal is refused as any `conflict` one is.
//! Known limit, of a decline only: between a promotion that landed and step 4, another person may
//! decline the proposal, which is still `open`; the retry then answers the compare-and-set's
//! `conflict`, and the accepted content stays on the line under a proposal shown as declined.
//! (The decliner was already answered, so the decline is not overwritten.)
//!
//! `conflict` is terminal: reconciliation opens a new proposal on the current head. A proposal
//! in `conflict` cannot be accepted, declined or confirmed again; the one exception is the false
//! mark of step 4, which gives way to the acceptance whose promotion landed.
//!
//! Declining (`decline_proposal`) needs no confirmation (the contract has none). The proposal must
//! be `open`; the request's reason is recorded as a comment by the person who declines, under the
//! decline's own mutation, before the status becomes `declined` (a compare-and-set from `open`),
//! then a `proposal_updated` event; the answer is the declined proposal. The comment comes first
//! because it is the mark of this decline: an already `declined` proposal answers the stored
//! proposal when the comment of this mutation (`derive_comment_id`) is among its comments, and
//! `conflict` otherwise. The candidate reference stays: it is history. Known limit: a change of
//! the status by someone else between this decline's check and its compare-and-set (an accept
//! that lands then, or another person's decline: the store takes an identical change as a retry
//! only from the mutation that wrote it) leaves the reason comment on that proposal, and the
//! decline answers the compare-and-set's `conflict`.
//!
//! Comments (`add_comment`) are allowed on a proposal in any status. A comment's source is
//! checked as every citation is, in its own workspace (`sources::checked_citation`), and kept
//! with the comment; its identity derives from the mutation. Listing (`list_comments`) is
//! oldest first and paged by the store. A kept source is remapped through the revision map of its
//! workspace when it is read: to the rewritten revision when the cited item is still there,
//! otherwise the comment is marked invalidated by that purge. A source in another workspace is
//! returned only to a reader who may read that workspace, looked up through `AccessControl` on
//! every listing; to anyone else the comment is returned without it.

use std::collections::BTreeMap;
use std::sync::Arc;

use okf_jawn_contract::access::{AccessRoute, Permission};
use okf_jawn_contract::common::MutationResult;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::{
    MutationId, ProposalId, PurgeId, Revision, WorkspaceId, WorkspacePath,
};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_contract::proposal::{
    AcceptProposalRequest, AddCommentRequest, Change, Comment, DeclineProposalRequest,
    GetProposalRequest, ListCommentsRequest, ListCommentsResponse, ListProposalsRequest,
    ListProposalsResponse, OpenProposalRequest, Proposal, ProposalKind, ProposalStatus,
};
use okf_jawn_contract::review::{ConfirmationAction, ConfirmationTarget};
use okf_jawn_contract::source::SourceReference;

use super::ApplicationService;
use super::reviews::browser_session;
use super::shared::{
    announce, check_named, invalid, mutation_id, now, record_receipt, workspace_scope,
};
use super::sources::checked_citation;
use crate::access::authorize_workspace;
use crate::confirmations::ConfirmationConsume;
use crate::conformance::EditCheck;
use crate::context::{Attempt, OperationContext};
use crate::events::{EventScope, NewEvent};
use crate::items::{refuse_header_change, refuse_supplied_header};
use crate::mutations::request_digest;
use crate::proposals::ProposalFilter;
use crate::storage::{
    CandidateChanges, ChangeContext, Committed, Page, Promotion, Provenance, StorageScope,
    TreeEdit, derive_comment_id, derive_item_id, derive_proposal_id,
};

/// How many comments one page of the search for a decline's own comment asks for.
const COMMENT_SEARCH_PAGE: u16 = 200;

/// Open a proposal: a candidate commit off the accepted line and its row, then a
/// `proposal_updated` event. See the module header for every rule.
///
/// # Errors
/// Returns `InvalidInput` on `/changes/<n>/properties/okf_jawn` for a supplied or changed
/// application header, `NotImplemented` on `/changes/<n>/item_kind` for a View, `InvalidInput`
/// on `/changes` for supplied text from a route that is not an agent's, `InvalidInput` on
/// `/changes/<n>/item_id` for supplied text of an item that is no source, `Conflict` there for a
/// source that is not unprocessed and on `/changes/<n>/based_on` for a digest that is not the
/// source's, the typed `NotFound` of an invalidated base, the candidate check's refusal, or any
/// port error.
pub(super) async fn open_proposal(
    service: &ApplicationService,
    context: &OperationContext,
    request: OpenProposalRequest,
) -> Result<Proposal, ApiError> {
    let kind = ProposalKind::of(&request.changes)?;
    for (index, change) in request.changes.iter().enumerate() {
        if let Change::Create { properties, .. } = change {
            let pointer = format!("/changes/{index}");
            refuse_supplied_header(properties, &format!("{pointer}/properties"))?;
            change.check_kind(&pointer)?;
            if change.created_kind() == Some(ItemKind::View) {
                return Err(ApiError::new(
                    ErrorCode::NotImplemented,
                    "proposing a View is not implemented in this build",
                )
                .with_field(format!("{pointer}/item_kind")));
            }
        }
    }
    if kind == ProposalKind::SupplyExtraction
        && context.principal.route != AccessRoute::McpDelegation
    {
        return Err(invalid(
            "text for an unprocessed source is supplied by an agent through its delegated \
             connection; a person corrects the text with correct_digest",
            "/changes",
        ));
    }
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    check_named(service, &scope, &request.base_revision).await?;
    let proposal_id = derive_proposal_id(mutation);
    let proposer = Provenance::from_principal(&context.principal);
    let (edits, touched) =
        candidate_edits(service, &scope, &request, mutation, proposal_id, &proposer).await?;
    let ports = service.ports();
    let candidate = ports
        .versions
        .create_candidate(
            &scope,
            proposal_id,
            CandidateChanges {
                mutation_id: mutation,
                base: request.base_revision.clone(),
                author: proposer,
                message: format!("Proposal: {}", request.title),
                edits,
            },
            Arc::new(EditCheck::new(&touched, None)),
        )
        .await?;
    let content_digest = request_digest(&request.changes)?;
    let stored = ports
        .proposals
        .insert(
            &scope,
            mutation,
            Proposal {
                id: proposal_id,
                workspace_id: scope.workspace_id,
                base_revision: request.base_revision,
                proposal_revision: candidate.revision,
                content_digest,
                title: request.title,
                description: request.description,
                changes: request.changes,
                kind,
                status: ProposalStatus::Open,
                created_by: context.principal.subject.clone(),
                created_via: context.principal.route.clone(),
                client_id: context.principal.client_id.clone(),
                created_at: now(service)?,
                warnings: candidate.warnings,
            },
        )
        .await?;
    proposal_updated(service, &scope, mutation, stored.id).await?;
    Ok(stored)
}

/// The proposals of the workspace, newest first, optionally in one state.
///
/// # Errors
/// Returns any port error.
pub(super) async fn list_proposals(
    service: &ApplicationService,
    context: &OperationContext,
    request: ListProposalsRequest,
) -> Result<ListProposalsResponse, ApiError> {
    let scope = workspace_scope(context)?;
    service
        .ports()
        .proposals
        .list(
            &scope,
            ProposalFilter {
                status: request.status,
                page: request.page.into(),
            },
        )
        .await
}

/// One proposal with its exact content and base.
///
/// # Errors
/// Returns `NotFound` for a proposal the workspace does not hold, or any port error.
pub(super) async fn get_proposal(
    service: &ApplicationService,
    context: &OperationContext,
    request: GetProposalRequest,
) -> Result<Proposal, ApiError> {
    let scope = workspace_scope(context)?;
    service
        .ports()
        .proposals
        .get(&scope, request.proposal_id)
        .await
}

/// Accept exactly the confirmed proposal against the displayed head, in the order the module
/// header gives.
///
/// # Errors
/// Returns `Forbidden` without a browser session, `NotFound` for a proposal the workspace does
/// not hold, `Conflict` for a proposal that is not open (or accepted by another mutation), on
/// `/proposal_revision` for another revision, the confirmation store's refusal, `Conflict` with
/// `StaleBase` for a stale precondition (on `/expected_head` when the head moved), the store's
/// other refusals of the promotion as given, or any port error.
pub(super) async fn accept_proposal(
    service: &ApplicationService,
    context: &OperationContext,
    request: AcceptProposalRequest,
) -> Result<MutationResult, ApiError> {
    let session = browser_session(context, "accepting a proposal")?;
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    let ports = service.ports();
    let proposal = ports.proposals.get(&scope, request.proposal_id).await?;
    // A promotion sits on the proposal's base, so this mutation's lies after it when it landed.
    let landed = if context.attempt == Attempt::Resumed {
        ports
            .versions
            .find_commit(&scope, mutation, Some(&proposal.base_revision))
            .await?
    } else {
        None
    };
    if let Some(committed) = landed {
        return accepted(service, context, &scope, proposal, committed).await;
    }
    match proposal.status {
        ProposalStatus::Open => {}
        ProposalStatus::Accepted => return Err(conflict("this proposal was already accepted")),
        ProposalStatus::Declined => return Err(conflict("this proposal was declined")),
        ProposalStatus::Conflict if context.attempt == Attempt::Resumed => {
            return Err(
                stale_again(service, &scope, mutation, proposal, &request.expected_head).await,
            );
        }
        ProposalStatus::Conflict => return Err(needs_reconciliation()),
    }
    if request.proposal_revision != proposal.proposal_revision {
        return Err(
            conflict("the proposal shown is not the proposal's current revision")
                .with_field("/proposal_revision"),
        );
    }
    ports
        .confirmations
        .consume(
            &scope,
            mutation,
            ConfirmationConsume {
                confirmation_id: request.confirmation_id,
                action: ConfirmationAction::AcceptProposal,
                target: ConfirmationTarget::Proposal {
                    proposal_id: proposal.id,
                },
                revision: proposal.proposal_revision.clone(),
                content_digest: proposal.content_digest.clone(),
                session_id: session,
                subject: context.principal.subject.clone(),
            },
        )
        .await?;
    let promoted = ports
        .versions
        .promote_candidate(
            &scope,
            Promotion {
                mutation_id: mutation,
                proposal_id: proposal.id,
                expected_head: request.expected_head.clone(),
                candidate: proposal.proposal_revision.clone(),
                approver: Provenance::from_principal(&context.principal),
                message: format!("Accept proposal: {}", proposal.title),
            },
        )
        .await;
    match promoted {
        Ok(committed) => accepted(service, context, &scope, proposal, committed).await,
        Err(error) => match error.detail.as_deref() {
            Some(ErrorDetail::StaleBase { head }) if error.code == ErrorCode::Conflict => {
                let head = head.clone();
                Err(stale(
                    service,
                    &scope,
                    mutation,
                    proposal,
                    &request.expected_head,
                    head,
                )
                .await)
            }
            _ => Err(error),
        },
    }
}

/// Close an open proposal without applying it; its reason becomes a comment.
///
/// # Errors
/// Returns `NotFound` for a proposal the workspace does not hold, `Conflict` for a proposal that
/// is not open (or declined by another mutation), or any port error.
pub(super) async fn decline_proposal(
    service: &ApplicationService,
    context: &OperationContext,
    request: DeclineProposalRequest,
) -> Result<Proposal, ApiError> {
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    let ports = service.ports();
    let proposal = ports.proposals.get(&scope, request.proposal_id).await?;
    match proposal.status {
        ProposalStatus::Open => {}
        ProposalStatus::Declined
            if context.attempt == Attempt::Resumed
                && has_comment(service, &scope, proposal.id, &derive_comment_id(mutation))
                    .await? =>
        {
            proposal_updated(service, &scope, mutation, proposal.id).await?;
            return Ok(proposal);
        }
        ProposalStatus::Declined => return Err(conflict("this proposal was already declined")),
        ProposalStatus::Accepted => return Err(conflict("this proposal was accepted")),
        ProposalStatus::Conflict => {
            return Err(conflict(
                "this proposal needs reconciliation and cannot be declined as it stands",
            ));
        }
    }
    ports
        .proposals
        .add_comment(
            &scope,
            mutation,
            proposal.id,
            Comment {
                id: derive_comment_id(mutation),
                author: context.principal.subject.clone(),
                text: request.reason,
                created_at: now(service)?,
                invalidated_by: None,
                source: None,
            },
        )
        .await?;
    let declined = ports
        .proposals
        .update(
            &scope,
            mutation,
            Proposal {
                status: ProposalStatus::Declined,
                ..proposal
            },
            ProposalStatus::Open,
        )
        .await?;
    proposal_updated(service, &scope, mutation, declined.id).await?;
    Ok(declined)
}

/// Add a discussion comment, with an optional citation checked in its own workspace.
///
/// # Errors
/// Returns `NotFound` for a proposal the workspace does not hold, the refusals of
/// `sources::checked_citation` for the source, or any port error.
pub(super) async fn add_comment(
    service: &ApplicationService,
    context: &OperationContext,
    request: AddCommentRequest,
) -> Result<Comment, ApiError> {
    let scope = workspace_scope(context)?;
    let mutation = mutation_id(context)?;
    let ports = service.ports();
    ports.proposals.get(&scope, request.proposal_id).await?;
    let source = match request.source {
        None => None,
        Some(source) => {
            let cited = authorized_scope(context, source.workspace_id)?;
            Some(checked_citation(service, &cited, source).await?.0)
        }
    };
    let stored = ports
        .proposals
        .add_comment(
            &scope,
            mutation,
            request.proposal_id,
            Comment {
                id: derive_comment_id(mutation),
                author: context.principal.subject.clone(),
                text: request.text,
                created_at: now(service)?,
                invalidated_by: None,
                source,
            },
        )
        .await?;
    proposal_updated(service, &scope, mutation, request.proposal_id).await?;
    Ok(stored)
}

/// One page of a proposal's discussion, oldest first, each kept source remapped or marked
/// invalidated, and left out where the reader may not read its workspace.
///
/// # Errors
/// Returns `NotFound` for a proposal the workspace does not hold, or any port error, a grant
/// lookup's included, except the `Forbidden` and `NotFound` that mean the reader may not read a
/// source's workspace.
pub(super) async fn list_comments(
    service: &ApplicationService,
    context: &OperationContext,
    request: ListCommentsRequest,
) -> Result<ListCommentsResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let page = service
        .ports()
        .proposals
        .list_comments(&scope, request.proposal_id, request.page.into())
        .await?;
    let mut readable: BTreeMap<WorkspaceId, Option<StorageScope>> = BTreeMap::new();
    readable.insert(scope.workspace_id, Some(scope.clone()));
    let mut items = Vec::with_capacity(page.items.len());
    for mut comment in page.items {
        if let Some(source) = comment.source.take() {
            let cited = if let Some(found) = readable.get(&source.workspace_id) {
                found.clone()
            } else {
                let found = readable_scope(service, context, source.workspace_id).await?;
                readable.insert(source.workspace_id, found.clone());
                found
            };
            if let Some(cited) = cited {
                let (source, invalidated_by) = remapped(service, &cited, source).await?;
                comment.source = Some(source);
                if comment.invalidated_by.is_none() {
                    comment.invalidated_by = invalidated_by;
                }
            }
        }
        items.push(comment);
    }
    Ok(ListCommentsResponse {
        items,
        next_cursor: page.next_cursor,
    })
}

/// A stored citation as it reads now: to the revision a purge rewrote its revision to when the
/// cited item is there, and otherwise as stored with the purge that invalidated it. A citation no
/// purge touched is returned as stored.
///
/// # Errors
/// Returns any port error; a cited item absent at the rewritten revision is not one.
pub(super) async fn remapped(
    service: &ApplicationService,
    scope: &StorageScope,
    mut source: SourceReference,
) -> Result<(SourceReference, Option<PurgeId>), ApiError> {
    let ports = service.ports();
    let Some(mapping) = ports
        .records
        .revision_mapping(scope, &source.revision)
        .await?
    else {
        return Ok((source, None));
    };
    if let Some(replacement) = mapping.replacement {
        match ports
            .versions
            .show(scope, &replacement, source.item_id)
            .await
        {
            Ok(_) => {
                source.revision = replacement;
                return Ok((source, None));
            }
            Err(error) if error.code == ErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok((source, Some(mapping.purge_id)))
}

/// The edits of a proposal's changes on top of its base and the paths the changes write, after
/// the refusals that need the base (a changed header, a supply change's source and digest).
///
/// # Errors
/// As `open_proposal`, for those refusals, or any port error of reading the base.
async fn candidate_edits(
    service: &ApplicationService,
    scope: &StorageScope,
    request: &OpenProposalRequest,
    mutation: MutationId,
    proposal_id: ProposalId,
    proposer: &Provenance,
) -> Result<(Vec<TreeEdit>, Vec<WorkspacePath>), ApiError> {
    let versions = &service.ports().versions;
    let base = &request.base_revision;
    let mut edits = Vec::with_capacity(request.changes.len());
    let mut touched = Vec::new();
    let mut created: u32 = 0;
    for (index, change) in request.changes.iter().enumerate() {
        let pointer = format!("/changes/{index}");
        let new_item_id = derive_item_id(mutation, created);
        match change {
            Change::Create { path, .. } => {
                touched.push(path.clone());
                created = created.saturating_add(1);
            }
            Change::Edit {
                item_id,
                properties,
                ..
            } => {
                let stored = versions.show(scope, base, *item_id).await?;
                refuse_header_change(
                    &stored.properties,
                    properties,
                    &format!("{pointer}/properties"),
                )?;
                touched.push(stored.summary.path);
            }
            Change::Archive { item_id } => {
                touched.push(versions.show(scope, base, *item_id).await?.summary.path);
            }
            Change::Move {
                item_id,
                destination,
            } => {
                touched.push(versions.show(scope, base, *item_id).await?.summary.path);
                touched.push(destination.clone());
            }
            Change::SupplyExtraction {
                item_id, based_on, ..
            } => {
                let document = versions.show(scope, base, *item_id).await?;
                let Some(appearance) = document.source.as_ref() else {
                    return Err(invalid(
                        "text can be supplied only for a source",
                        &format!("{pointer}/item_id"),
                    ));
                };
                if !appearance.extraction.outcome.status().is_unprocessed() {
                    return Err(conflict(
                        "the source's conversion is complete or pending; only a partial, \
                         failed or unsupported source takes supplied text",
                    )
                    .with_field(format!("{pointer}/item_id")));
                }
                if based_on != &appearance.extraction.digest {
                    return Err(conflict(
                        "the source's digest is not the one the supplied text was based on",
                    )
                    .with_field(format!("{pointer}/based_on")));
                }
                touched.push(document.summary.path);
            }
        }
        edits.push(TreeEdit::from_change(
            change.clone(),
            &ChangeContext::for_change(change, new_item_id, proposal_id, proposer.clone()),
        ));
    }
    Ok((edits, touched))
}

/// Finish an acceptance whose promotion landed: the status, the index and the `changed` event,
/// the `proposal_updated` event and the receipt (steps 4 to 7 of the module header).
///
/// # Errors
/// Returns the compare-and-set's `Conflict` when the proposal is neither open nor marked
/// `conflict` (declined meanwhile, the header's known limit), or any port error.
async fn accepted(
    service: &ApplicationService,
    context: &OperationContext,
    scope: &StorageScope,
    proposal: Proposal,
    committed: Committed,
) -> Result<MutationResult, ApiError> {
    let mutation = mutation_id(context)?;
    let proposals = &service.ports().proposals;
    let id = proposal.id;
    let marked = proposals
        .update(
            scope,
            mutation,
            Proposal {
                status: ProposalStatus::Accepted,
                ..proposal
            },
            ProposalStatus::Open,
        )
        .await;
    let stored = match marked {
        Ok(stored) => stored,
        Err(refused) if refused.code == ErrorCode::Conflict => {
            // `committed` is this mutation's promotion, so a `conflict` mark set meanwhile (by an
            // acceptance the store refused because this promotion moved the head) is false.
            let now = proposals.get(scope, id).await?;
            if now.status != ProposalStatus::Conflict {
                return Err(refused);
            }
            proposals
                .update(
                    scope,
                    mutation,
                    Proposal {
                        status: ProposalStatus::Accepted,
                        ..now
                    },
                    ProposalStatus::Conflict,
                )
                .await?
        }
        Err(error) => return Err(error),
    };
    announce(service, scope.clone(), mutation, &committed.revision, None).await?;
    proposal_updated(service, scope, mutation, stored.id).await?;
    let receipt_id = record_receipt(service, context, scope, Some(mutation), Vec::new()).await?;
    Ok(MutationResult {
        revision: committed.revision,
        receipt_id,
        warnings: stored.warnings,
    })
}

/// The answer to a stale acceptance, after the proposal is marked `conflict` and a
/// `proposal_updated` event naming it is appended under the mutation: `Conflict` with
/// `StaleBase { head }`, on `/expected_head` when `head` is not the request's `expected_head`
/// (the head moved) and with no field when it is (the candidate was not drafted on the head the
/// request named); or the error of marking it or of the event, when one fails.
async fn stale(
    service: &ApplicationService,
    scope: &StorageScope,
    mutation: MutationId,
    proposal: Proposal,
    expected_head: &Revision,
    head: Revision,
) -> ApiError {
    let marked = service
        .ports()
        .proposals
        .update(
            scope,
            mutation,
            Proposal {
                status: ProposalStatus::Conflict,
                ..proposal
            },
            ProposalStatus::Open,
        )
        .await;
    let marked = match marked {
        Ok(marked) => marked,
        Err(error) => return error,
    };
    match proposal_updated(service, scope, mutation, marked.id).await {
        Ok(()) => stale_refusal(expected_head, head),
        Err(error) => error,
    }
}

/// A resumed acceptance of a proposal in `conflict`: when this mutation's first attempt marked it
/// (the store takes the same mark under the same mutation as a retry), the attempt appends its
/// `proposal_updated` event and answers its stale refusal again, naming the head as it is now;
/// when another mutation marked it, the refusal of any proposal in `conflict`; or the error of a
/// port, when one fails.
async fn stale_again(
    service: &ApplicationService,
    scope: &StorageScope,
    mutation: MutationId,
    proposal: Proposal,
    expected_head: &Revision,
) -> ApiError {
    let ports = service.ports();
    let marked = ports
        .proposals
        .update(scope, mutation, proposal, ProposalStatus::Open)
        .await;
    let marked = match marked {
        Ok(marked) => marked,
        Err(error) if error.code == ErrorCode::Conflict => return needs_reconciliation(),
        Err(error) => return error,
    };
    if let Err(error) = proposal_updated(service, scope, mutation, marked.id).await {
        return error;
    }
    match ports.versions.head(scope).await {
        Ok(head) => stale_refusal(expected_head, head),
        Err(error) => error,
    }
}

/// The refusal of a proposal in `conflict`: it is terminal, and reconciliation is a new proposal.
fn needs_reconciliation() -> ApiError {
    conflict(
        "this proposal needs reconciliation: it was drafted on a base that is no longer the head",
    )
}

/// `Conflict` with `StaleBase { head }`: on `/expected_head` when `head` is not the request's
/// `expected_head`, the request field that is stale; with no field when it is, because then the
/// request named the head and the proposal's base is what is stale.
fn stale_refusal(expected_head: &Revision, head: Revision) -> ApiError {
    let moved = head != *expected_head;
    let refusal = conflict(
        "the workspace head is not the one the proposal was drafted on; it needs reconciliation",
    )
    .with_detail(ErrorDetail::StaleBase { head });
    if moved {
        refusal.with_field("/expected_head")
    } else {
        refusal
    }
}

/// Whether the proposal's discussion holds the comment `id`, read page by page, oldest first.
///
/// # Errors
/// Returns any port error.
async fn has_comment(
    service: &ApplicationService,
    scope: &StorageScope,
    proposal: ProposalId,
    id: &str,
) -> Result<bool, ApiError> {
    let mut cursor = None;
    loop {
        let page = service
            .ports()
            .proposals
            .list_comments(
                scope,
                proposal,
                Page {
                    cursor,
                    limit: COMMENT_SEARCH_PAGE,
                },
            )
            .await?;
        if page.items.iter().any(|comment| comment.id == id) {
            return Ok(true);
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(false),
        }
    }
}

/// The storage scope of a workspace dispatch authorized for this request (a comment's source).
///
/// # Errors
/// Returns `Internal` when dispatch authorized no such workspace, which the request's targets
/// always name.
fn authorized_scope(
    context: &OperationContext,
    workspace: WorkspaceId,
) -> Result<StorageScope, ApiError> {
    context
        .grants
        .iter()
        .find(|grant| grant.workspace_id() == workspace)
        .map(|grant| grant.scope.clone())
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::Internal,
                "the cited workspace was not authorized for this request",
            )
        })
}

/// The scope of `workspace` when the reader may read it, looked up now through the access
/// port with the route and delegation rules dispatch applies; `None` when they may not.
///
/// # Errors
/// Returns `Internal` for a grant of another scope, or any error of the lookup except the
/// `Forbidden` and `NotFound` that mean the reader may not read the workspace.
async fn readable_scope(
    service: &ApplicationService,
    context: &OperationContext,
    workspace: WorkspaceId,
) -> Result<Option<StorageScope>, ApiError> {
    let principal = &context.principal;
    let raw = match service
        .ports()
        .access
        .authorize(principal, workspace, Permission::Read)
        .await
    {
        Ok(raw) => raw,
        Err(error) if matches!(error.code, ErrorCode::Forbidden | ErrorCode::NotFound) => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if raw.workspace_id() != workspace || raw.scope.tenant_id != principal.tenant_id {
        return Err(ApiError::new(
            ErrorCode::Internal,
            "the access adapter returned a grant for a different scope",
        ));
    }
    match authorize_workspace(principal, raw, Permission::Read) {
        Ok(grant) => Ok(Some(grant.scope)),
        Err(error) if error.code == ErrorCode::Forbidden => Ok(None),
        Err(error) => Err(error),
    }
}

/// Append the `proposal_updated` event naming `proposal` under the request's mutation.
///
/// # Errors
/// Returns any port error.
async fn proposal_updated(
    service: &ApplicationService,
    scope: &StorageScope,
    mutation: MutationId,
    proposal: ProposalId,
) -> Result<(), ApiError> {
    service
        .ports()
        .events
        .append(
            &EventScope::Workspace(scope.clone()),
            Some(mutation),
            NewEvent {
                kind: EventKind::ProposalUpdated,
                revision: None,
                item_id: None,
                job_id: None,
                connector_id: None,
                actor: None,
                operation: None,
                proposal_id: Some(proposal),
                count: None,
            },
        )
        .await?;
    Ok(())
}

/// A `Conflict` refusal.
fn conflict(message: &str) -> ApiError {
    ApiError::new(ErrorCode::Conflict, message)
}
