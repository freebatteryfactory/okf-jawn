//! Proposals: opening one writes a checked candidate, its row and an event, idempotently on the
//! mutation; accepting consumes a confirmation, promotes, marks, indexes, announces and records,
//! in that order, and a resumed attempt finishes a landed promotion; declining records its
//! reason as a comment; comments keep a checked citation and are listed with each citation
//! remapped, invalidated or left out for a reader who may not read it.

use std::collections::BTreeMap;

use okf_jawn_contract::access::{AccessRoute, Permission};
use okf_jawn_contract::common::PageRequest;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::extraction::{ConversionOutcome, Extraction, TextOrigin};
use okf_jawn_contract::identity::{
    IdempotencyKey, ItemId, MutationId, ProposalId, PurgeId, Revision, Timestamp, WorkspaceId,
};
use okf_jawn_contract::item::{APP_HEADER_KEY, ItemKind};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::proposal::{
    AcceptProposalRequest, AddCommentRequest, Change, DeclineProposalRequest, GetProposalRequest,
    ListCommentsRequest, ListProposalsRequest, OpenProposalRequest, Proposal, ProposalKind,
    ProposalStatus,
};
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::review::{
    Confirmation, ConfirmationAction, ConfirmationTarget, CreateConfirmationRequest,
};
use okf_jawn_contract::source::{SourceLocation, SourceReference, UnresolvedReason};
use okf_jawn_core::context::{Attempt, OperationContext, WorkspaceGrant};
use okf_jawn_core::events::NewEvent;
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::mutations::request_digest;
use okf_jawn_core::ports::Application;
use okf_jawn_core::proposals::ProposalStore;
use okf_jawn_core::storage::{
    Provenance, StorageScope, TreeEdit, derive_comment_id, derive_item_id, derive_proposal_id,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    ALICE, Built, NOTE, alice, context, digest, mutation, note, path, revision, scope, source_card,
};
use crate::proposal_store::{ProposalCall, ProposalWorld};

/// Each step of an acceptance a first attempt may fail at.
#[derive(Debug, Clone, Copy)]
enum Step {
    Promote,
    Status,
    Index,
    Changed,
    Announced,
    Receipt,
}

/// The browser session every confirming context carries.
pub const SESSION: &str = "session-1";

/// The mutation that opens the fixture proposal.
const OPENED: u128 = 500;

/// A source card in the fixture tree.
const CARD: u128 = 30;

/// The other workspace a citation may lie in.
const ELSEWHERE: u128 = 2;

/// The mutation that accepts the fixture proposal.
const ACCEPTING: u128 = 700;

/// The mutation that declines the fixture proposal.
const DECLINING: u128 = 900;

/// The mutation that comments on the fixture proposal.
const COMMENTING: u128 = 1000;

/// The mutation under which a test sets a proposal's status in the store itself.
const SETTING: u128 = 90;

/// A proposed View in the fixture tree.
const PROPOSED_VIEW: u128 = 45;

fn key() -> IdempotencyKey {
    IdempotencyKey(Uuid::from_u128(77))
}

/// `context` in Alice's browser session.
pub fn in_session(mut context: OperationContext) -> OperationContext {
    context.session_id = Some(SESSION.to_owned());
    context
}

/// `context` as a resumed attempt of its mutation.
pub fn resumed(mut context: OperationContext) -> OperationContext {
    context.attempt = Attempt::Resumed;
    context
}

/// The fixture note at `notes/plan.md` at `at`.
pub fn plan(at: &Revision) -> Built<okf_jawn_contract::item::ItemDocument> {
    note(
        NOTE,
        at,
        "notes/plan.md",
        "# Plan\n",
        json!({ "type": "Note", "title": "Plan", "description": "The plan." }),
    )
}

/// A world whose head `a` holds the plan note, with the candidate revision `c` and the promoted
/// revision `d` queued.
pub fn proposal_world() -> Built<ProposalWorld> {
    let world = ProposalWorld::new()?;
    world
        .world
        .versions
        .put_document(&revision('a')?, plan(&revision('a')?)?)?;
    world.versions.queue(revision('c')?)?;
    world.versions.queue(revision('d')?)?;
    Ok(world)
}

/// A `create` of a note at `location` without the recommended title and description.
fn create(location: &str) -> Built<Change> {
    Ok(Change::Create {
        path: path(location)?,
        type_name: "Note".to_owned(),
        body: "# New\n".to_owned(),
        properties: BTreeMap::new(),
        item_kind: None,
    })
}

fn open_request(changes: Vec<Change>) -> Built<OpenProposalRequest> {
    Ok(OpenProposalRequest {
        workspace_id: scope()?.workspace_id,
        base_revision: revision('a')?,
        title: "Add a note".to_owned(),
        description: "It was missing.".to_owned(),
        changes,
        idempotency_key: key(),
    })
}

/// Open a proposal of `changes` as Alice under `number`.
async fn open(world: &ProposalWorld, number: u128, changes: Vec<Change>) -> Built<Proposal> {
    Ok(world
        .service
        .open_proposal(
            &alice(OperationName::OpenProposal, Some(mutation(number)))?,
            open_request(changes)?,
        )
        .await?)
}

/// `value` as JSON, to compare wire values that have no equality of their own.
pub fn same<T: serde::Serialize>(value: &T) -> Built<Value> {
    Ok(serde_json::to_value(value)?)
}

/// The events appended, without their scope.
fn events(world: &ProposalWorld) -> Built<Vec<(Option<MutationId>, NewEvent)>> {
    Ok(world
        .world
        .events
        .appended()?
        .into_iter()
        .map(|(_, mutation, event)| (mutation, event))
        .collect())
}

/// The `proposal_updated` events naming `proposal`, by the mutation that appended them.
fn proposal_events(world: &ProposalWorld, proposal: ProposalId) -> Built<Vec<Option<MutationId>>> {
    Ok(events(world)?
        .into_iter()
        .filter(|(_, event)| {
            event.kind == EventKind::ProposalUpdated && event.proposal_id == Some(proposal)
        })
        .map(|(mutation, _)| mutation)
        .collect())
}

/// Nothing of a proposal was stored: no candidate, no row, no event.
fn nothing_stored(world: &ProposalWorld) -> TestResult {
    assert!(world.versions.candidates()?.is_empty());
    assert!(world.proposals.rows()?.is_empty());
    assert_eq!(events(world)?, Vec::new());
    Ok(())
}

#[tokio::test]
async fn opening_a_proposal_writes_its_checked_candidate_its_row_and_an_event() -> TestResult {
    let world = proposal_world()?;
    let changes = vec![
        create("notes/new.md")?,
        Change::Edit {
            item_id: ItemId(Uuid::from_u128(NOTE)),
            body: "# Plan, edited\n".to_owned(),
            properties: serde_json::from_value(
                json!({ "type": "Note", "title": "Plan", "description": "The plan." }),
            )?,
        },
        Change::Move {
            item_id: ItemId(Uuid::from_u128(NOTE)),
            destination: path("archive/plan.md")?,
        },
        create("notes/second.md")?,
    ];
    let proposal = open(&world, OPENED, changes.clone()).await?;
    let id = derive_proposal_id(mutation(OPENED));
    assert_eq!(proposal.id, id);
    assert_eq!(proposal.workspace_id, scope()?.workspace_id);
    assert_eq!(proposal.base_revision, revision('a')?);
    assert_eq!(proposal.proposal_revision, revision('c')?);
    // R1: the digest of the canonical JSON of the changes.
    assert_eq!(proposal.content_digest, request_digest(&changes)?);
    assert_eq!(proposal.kind, ProposalKind::Content);
    assert_eq!(proposal.status, ProposalStatus::Open);
    assert_eq!(proposal.created_by, ALICE);
    assert_eq!(proposal.created_via, AccessRoute::LocalOwner);
    assert_eq!(proposal.client_id, None);
    assert_eq!(proposal.created_at.as_str(), "2026-10-09T12:00:00.000Z");
    assert_eq!(proposal.changes, changes);
    // The candidate: drafted on the base by Alice, each change as its edit, created items named
    // from the mutation in create order.
    let candidates = world.versions.candidates()?;
    let candidate = some(candidates.first(), "the candidate")?;
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidate.mutation_id, mutation(OPENED));
    assert_eq!(candidate.base, revision('a')?);
    assert_eq!(
        candidate.author,
        Provenance {
            subject: ALICE.to_owned(),
            route: AccessRoute::LocalOwner,
            client_id: None
        }
    );
    let created: Vec<(ItemId, ItemKind)> = candidate
        .edits
        .iter()
        .filter_map(|edit| match edit {
            TreeEdit::CreateItem { item_id, kind, .. } => Some((*item_id, kind.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        created,
        [
            (derive_item_id(mutation(OPENED), 0), ItemKind::Note),
            (derive_item_id(mutation(OPENED), 1), ItemKind::Note)
        ]
    );
    assert!(matches!(
        candidate.edits.get(2),
        Some(TreeEdit::MoveItem { destination, .. }) if destination.as_str() == "archive/plan.md"
    ));
    assert_eq!(world.versions.reference(id)?, Some(revision('c')?));
    // The check's findings about the files the changes write are the proposal's warnings: the
    // new notes lack their recommended fields.
    assert!(
        proposal
            .warnings
            .iter()
            .any(|warning| warning.location.as_deref() == Some("notes/new.md")),
        "{:?}",
        proposal.warnings
    );
    assert!(
        proposal
            .warnings
            .iter()
            .any(|warning| warning.location.as_deref() == Some("notes/second.md")),
        "{:?}",
        proposal.warnings
    );
    assert_eq!(world.proposals.rows()?.len(), 1);
    assert_eq!(proposal_events(&world, id)?, [Some(mutation(OPENED))]);
    Ok(())
}

#[tokio::test]
async fn the_findings_about_an_edited_item_are_the_proposals_warnings() -> TestResult {
    let world = proposal_world()?;
    // The edit drops the recommended description: the check finds it, at the item's path.
    let proposal = open(
        &world,
        OPENED,
        vec![Change::Edit {
            item_id: ItemId(Uuid::from_u128(NOTE)),
            body: "# Plan\n".to_owned(),
            properties: serde_json::from_value(json!({ "type": "Note", "title": "Plan" }))?,
        }],
    )
    .await?;
    assert!(
        proposal
            .warnings
            .iter()
            .any(|warning| warning.location.as_deref() == Some("notes/plan.md")),
        "{:?}",
        proposal.warnings
    );
    Ok(())
}

#[tokio::test]
async fn a_proposal_is_drafted_on_its_base_even_when_the_head_moved() -> TestResult {
    let world = proposal_world()?;
    world.world.versions.set_head(revision('e')?)?;
    let proposal = open(&world, OPENED, vec![create("notes/new.md")?]).await?;
    assert_eq!(proposal.base_revision, revision('a')?);
    assert_eq!(
        some(world.versions.candidates()?.first(), "the candidate")?.base,
        revision('a')?
    );
    Ok(())
}

#[tokio::test]
async fn a_proposal_on_a_purged_base_is_not_found_and_stores_nothing() -> TestResult {
    let world = proposal_world()?;
    let purge_id = PurgeId(Uuid::from_u128(9));
    world.world.records.put_mapping(
        revision('a')?,
        RevisionMapping {
            purge_id,
            replacement: None,
        },
    )?;
    let error = err_of(open(&world, OPENED, vec![create("notes/new.md")?]).await)?;
    let error = some(error.downcast_ref::<ApiError>(), "an ApiError")?;
    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(
        error.detail.as_deref(),
        Some(&ErrorDetail::Invalidated {
            purge_id,
            replacement: None
        })
    );
    nothing_stored(&world)
}

/// The refusal of opening `changes` from `context`: its code and field, with nothing stored and
/// no check run.
async fn refused_open(
    world: &ProposalWorld,
    context: &OperationContext,
    changes: Vec<Change>,
) -> Built<(ErrorCode, Option<String>)> {
    let error = err_of(
        world
            .service
            .open_proposal(context, open_request(changes)?)
            .await,
    )?;
    nothing_stored(world)?;
    assert_eq!(world.versions.checks()?, 0);
    Ok((error.code, error.field))
}

#[tokio::test]
async fn a_created_note_that_supplies_the_application_header_is_refused_before_any_port()
-> TestResult {
    let world = proposal_world()?;
    let Change::Create {
        path,
        type_name,
        body,
        ..
    } = create("notes/new.md")?
    else {
        return Err("a create change".into());
    };
    let supplied = Change::Create {
        path,
        type_name,
        body,
        properties: BTreeMap::from([(APP_HEADER_KEY.to_owned(), json!({ "archived": true }))]),
        item_kind: None,
    };
    let shows = world.world.versions.shows();
    let refused = refused_open(
        &world,
        &alice(OperationName::OpenProposal, Some(mutation(OPENED)))?,
        vec![create("notes/first.md")?, supplied],
    )
    .await?;
    assert_eq!(
        refused,
        (
            ErrorCode::InvalidInput,
            Some("/changes/1/properties/okf_jawn".to_owned())
        )
    );
    assert_eq!(world.world.versions.shows(), shows, "no port was read");
    Ok(())
}

/// The plan note at `a` carrying an application header.
fn plan_with_header(world: &ProposalWorld) -> Built<serde_json::Map<String, Value>> {
    let at = revision('a')?;
    let mut stored = plan(&at)?;
    stored
        .properties
        .insert(APP_HEADER_KEY.to_owned(), json!({ "id": "kept" }));
    world.world.versions.replace_document(&at, stored.clone())?;
    Ok(stored.properties.into_iter().collect())
}

fn edit(properties: serde_json::Map<String, Value>) -> Change {
    Change::Edit {
        item_id: ItemId(Uuid::from_u128(NOTE)),
        body: "# Plan, edited\n".to_owned(),
        properties: properties.into_iter().collect(),
    }
}

#[tokio::test]
async fn an_edit_may_echo_the_stored_header_and_is_refused_when_it_changes_it() -> TestResult {
    let world = proposal_world()?;
    let stored = plan_with_header(&world)?;
    let mut changed = stored.clone();
    changed.insert(APP_HEADER_KEY.to_owned(), json!({ "id": "forged" }));
    let refused = refused_open(
        &world,
        &alice(OperationName::OpenProposal, Some(mutation(OPENED)))?,
        vec![edit(changed)],
    )
    .await?;
    assert_eq!(
        refused,
        (
            ErrorCode::InvalidInput,
            Some("/changes/0/properties/okf_jawn".to_owned())
        )
    );
    // Echoing the stored header unchanged is accepted.
    let proposal = open(&world, OPENED.saturating_add(1), vec![edit(stored)]).await?;
    assert_eq!(proposal.status, ProposalStatus::Open);
    Ok(())
}

#[tokio::test]
async fn a_proposed_view_is_checked_as_a_view_and_a_proposed_source_is_refused() -> TestResult {
    let world = proposal_world()?;
    let with_kind = |kind| -> Built<Change> {
        let Change::Create {
            path,
            type_name,
            body,
            properties,
            ..
        } = create("notes/new.md")?
        else {
            return Err("a create change".into());
        };
        Ok(Change::Create {
            path,
            type_name,
            body,
            properties,
            item_kind: Some(kind),
        })
    };
    let context = alice(OperationName::OpenProposal, Some(mutation(OPENED)))?;
    // A note's create named a View is checked as a View (core M1.6): it has no title.
    assert_eq!(
        refused_open(&world, &context, vec![with_kind(ItemKind::View)?]).await?,
        (
            ErrorCode::InvalidInput,
            Some("/changes/0/properties/title".to_owned())
        )
    );
    assert_eq!(
        refused_open(&world, &context, vec![with_kind(ItemKind::Source)?]).await?,
        (
            ErrorCode::InvalidInput,
            Some("/changes/0/item_kind".to_owned())
        )
    );
    // A note named explicitly is a note.
    let proposal = open(&world, OPENED, vec![with_kind(ItemKind::Note)?]).await?;
    assert_eq!(proposal.status, ProposalStatus::Open);
    Ok(())
}

/// A View over the plan note at `revision(fill)`: an excerpt of it and its source line.
fn plan_view_body(fill: char) -> Built<String> {
    let block = json!({
        "schema_version": 1,
        "mode": "pinned",
        "grammar": "json_render",
        "bindings": [{
            "name": "plan",
            "source": {
                "item_id": ItemId(Uuid::from_u128(NOTE)),
                "path": "notes/plan.md",
                "revision": revision(fill)?,
                "selection": { "kind": "all" }
            },
            "units": {},
            "transforms": []
        }],
        "spec": {
            "root": "root",
            "elements": {
                "root": { "type": "Stack", "props": {}, "children": ["excerpt", "sources"] },
                "excerpt": { "type": "SourceExcerpt", "props": { "binding": "plan" }, "children": [] },
                "sources": { "type": "SourceList", "props": { "bindings": ["plan"] }, "children": [] }
            }
        }
    });
    Ok(format!(
        "The plan, as written.\n\n```json okf-jawn-view\n{}\n```\n",
        serde_json::to_string_pretty(&block)?
    ))
}

/// A `create` of a View at `views/plan.md` holding `body`.
fn view_create(body: String) -> Built<Change> {
    Ok(Change::Create {
        path: path("views/plan.md")?,
        type_name: "Dashboard".to_owned(),
        body,
        properties: serde_json::from_value(json!({ "title": "The plan" }))?,
        item_kind: Some(ItemKind::View),
    })
}

/// The proposal world with the plan note held at `b` too.
fn view_proposal_world() -> Built<ProposalWorld> {
    let world = proposal_world()?;
    world
        .world
        .versions
        .put_document(&revision('b')?, plan(&revision('b')?)?)?;
    Ok(world)
}

#[tokio::test]
async fn a_proposed_view_that_validates_is_opened_with_its_view_created() -> TestResult {
    let world = view_proposal_world()?;
    let proposal = open(&world, OPENED, vec![view_create(plan_view_body('b')?)?]).await?;
    assert_eq!(proposal.status, ProposalStatus::Open);
    let candidates = world.versions.candidates()?;
    assert!(matches!(
        some(candidates.first(), "the candidate")?.edits.as_slice(),
        [TreeEdit::CreateItem {
            kind: ItemKind::View,
            ..
        }]
    ));
    // A binding citing an item its revision does not hold stores nothing.
    let without = proposal_world()?;
    let context = alice(OperationName::OpenProposal, Some(mutation(OPENED)))?;
    assert_eq!(
        refused_open(&without, &context, vec![view_create(plan_view_body('b')?)?]).await?,
        (ErrorCode::NotFound, Some("/changes/0/body".to_owned()))
    );
    nothing_stored(&without)?;
    Ok(())
}

#[tokio::test]
async fn an_edit_of_a_view_is_checked_as_a_view() -> TestResult {
    let world = proposal_world()?;
    let at = revision('a')?;
    let mut view = note(
        PROPOSED_VIEW,
        &at,
        "views/plan.md",
        &plan_view_body('a')?,
        json!({ "type": "Dashboard", "title": "The plan" }),
    )?;
    view.summary.kind = ItemKind::View;
    world.world.versions.put_document(&at, view)?;
    let context = alice(OperationName::OpenProposal, Some(mutation(OPENED)))?;
    let edit = Change::Edit {
        item_id: ItemId(Uuid::from_u128(PROPOSED_VIEW)),
        body: "# Not a View\n".to_owned(),
        properties: serde_json::from_value(json!({ "type": "Dashboard", "title": "The plan" }))?,
    };
    assert_eq!(
        refused_open(&world, &context, vec![edit]).await?,
        (ErrorCode::InvalidInput, Some("/changes/0/body".to_owned()))
    );
    nothing_stored(&world)?;
    Ok(())
}

#[tokio::test]
async fn accepting_checks_a_proposed_view_again_before_the_confirmation_is_used() -> TestResult {
    let world = view_proposal_world()?;
    let proposal = open(&world, OPENED, vec![view_create(plan_view_body('b')?)?]).await?;
    let confirmation = confirm(&world, &proposal, 600).await?;
    // The View cites the plan at `b`, which a purge removed after the proposal was opened.
    world.world.records.put_mapping(
        revision('b')?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(9)),
            replacement: None,
        },
    )?;
    let refused = err_of(
        world
            .service
            .accept_proposal(
                &accepting(ACCEPTING)?,
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert_eq!(refused.field, None);
    assert!(
        refused.message.contains("/changes/0/body"),
        "{}",
        refused.message
    );
    // Nothing was used or promoted: the confirmation is unconsumed, the proposal still open.
    assert_eq!(
        some(world.confirmations.issued()?.first(), "the confirmation")?.2,
        None
    );
    assert!(world.versions.promotions()?.is_empty());
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Open);
    Ok(())
}

#[tokio::test]
async fn a_resumed_acceptance_whose_promotion_landed_is_not_refused_by_the_view_check() -> TestResult
{
    let world = view_proposal_world()?;
    let proposal = open(&world, OPENED, vec![view_create(plan_view_body('b')?)?]).await?;
    let confirmation = confirm(&world, &proposal, 600).await?;
    let first = world
        .service
        .accept_proposal(
            &accepting(ACCEPTING)?,
            accept_request(&proposal, &confirmation, 'a')?,
        )
        .await?;
    world.world.records.put_mapping(
        revision('b')?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(9)),
            replacement: None,
        },
    )?;
    let again = world
        .service
        .accept_proposal(
            &resumed(accepting(ACCEPTING)?),
            accept_request(&proposal, &confirmation, 'a')?,
        )
        .await?;
    assert_eq!(again.revision, first.revision);
    assert_eq!(world.versions.promotions()?.len(), 1);
    Ok(())
}

/// A source card in the base tree whose conversion is `outcome`, with the record digest `e`.
fn card_in_base(world: &ProposalWorld, outcome: ConversionOutcome) -> TestResult {
    let at = revision('a')?;
    world.world.versions.put_document(
        &at,
        source_card(
            CARD,
            &at,
            "sources/report-pdf.md",
            "",
            Extraction {
                outcome,
                converter: None,
                digest: Some(digest('e')?),
                page_count: Some(2),
                text_origin: TextOrigin::Converter,
                corrected: false,
                supplied: None,
                warnings: Vec::new(),
            },
        )?,
    )?;
    Ok(())
}

fn supply(item: u128, based_on: Option<char>) -> Built<Change> {
    Ok(Change::SupplyExtraction {
        item_id: ItemId(Uuid::from_u128(item)),
        based_on: based_on.map(digest).transpose()?,
        pages: Vec::new(),
        markdown: "# The whole text\n".to_owned(),
    })
}

fn partial() -> ConversionOutcome {
    ConversionOutcome::Partial {
        coverage: None,
        issues: Vec::new(),
    }
}

fn agent(number: u128) -> Built<OperationContext> {
    context(
        OperationName::OpenProposal,
        "agent",
        AccessRoute::McpDelegation,
        Some(mutation(number)),
    )
}

#[tokio::test]
async fn supplied_text_is_accepted_only_from_an_agents_delegated_route() -> TestResult {
    let world = proposal_world()?;
    card_in_base(&world, partial())?;
    for route in [
        AccessRoute::LocalOwner,
        AccessRoute::BrowserSession,
        AccessRoute::Service,
    ] {
        let person = context(
            OperationName::OpenProposal,
            ALICE,
            route,
            Some(mutation(OPENED)),
        )?;
        assert_eq!(
            refused_open(&world, &person, vec![supply(CARD, Some('e'))?]).await?,
            (ErrorCode::InvalidInput, Some("/changes".to_owned()))
        );
    }
    let proposal = world
        .service
        .open_proposal(
            &agent(OPENED)?,
            open_request(vec![supply(CARD, Some('e'))?])?,
        )
        .await?;
    assert_eq!(proposal.kind, ProposalKind::SupplyExtraction);
    assert_eq!(proposal.created_via, AccessRoute::McpDelegation);
    assert_eq!(proposal.created_by, "agent");
    let candidate = some(world.versions.candidates()?.first().cloned(), "a candidate")?;
    assert!(matches!(
        candidate.edits.first(),
        Some(TreeEdit::SupplyExtraction { proposal_id, supplier, .. })
            if *proposal_id == proposal.id && supplier.subject == "agent"
    ));
    Ok(())
}

#[tokio::test]
async fn supplied_text_needs_a_source_that_is_unprocessed_at_the_base() -> TestResult {
    let world = proposal_world()?;
    card_in_base(&world, ConversionOutcome::Completed)?;
    assert_eq!(
        refused_open(&world, &agent(OPENED)?, vec![supply(CARD, Some('e'))?]).await?,
        (ErrorCode::Conflict, Some("/changes/0/item_id".to_owned()))
    );
    assert_eq!(
        refused_open(&world, &agent(OPENED)?, vec![supply(NOTE, None)?]).await?,
        (
            ErrorCode::InvalidInput,
            Some("/changes/0/item_id".to_owned())
        )
    );
    for outcome in [
        ConversionOutcome::Unsupported,
        partial(),
        ConversionOutcome::Failed {
            reason: okf_jawn_contract::extraction::FailureReason::Damaged,
            issues: Vec::new(),
        },
    ] {
        let world = proposal_world()?;
        card_in_base(&world, outcome)?;
        let proposal = world
            .service
            .open_proposal(
                &agent(OPENED)?,
                open_request(vec![supply(CARD, Some('e'))?])?,
            )
            .await?;
        assert_eq!(proposal.status, ProposalStatus::Open);
    }
    Ok(())
}

#[tokio::test]
async fn supplied_text_must_be_based_on_the_sources_digest() -> TestResult {
    let world = proposal_world()?;
    card_in_base(&world, partial())?;
    for based_on in [Some('f'), None] {
        assert_eq!(
            refused_open(&world, &agent(OPENED)?, vec![supply(CARD, based_on)?]).await?,
            (ErrorCode::Conflict, Some("/changes/0/based_on".to_owned()))
        );
    }
    Ok(())
}

#[tokio::test]
async fn candidate_check_refuses_a_non_conformant_proposal_and_stores_nothing() -> TestResult {
    let world = proposal_world()?;
    // A note with an empty `type` is not conformant OKF (okf-validator: "`type` must be a
    // non-empty scalar"); the production check refuses its candidate.
    let untyped = Change::Create {
        path: path("notes/untyped.md")?,
        type_name: String::new(),
        body: "# Untyped\n".to_owned(),
        properties: BTreeMap::new(),
        item_kind: None,
    };
    let error = err_of(
        world
            .service
            .open_proposal(
                &alice(OperationName::OpenProposal, Some(mutation(OPENED)))?,
                open_request(vec![untyped])?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::InvalidInput, "{error}");
    assert!(
        error.message.contains("non-conformant OKF bundle")
            && error.message.contains("notes/untyped.md"),
        "{error}"
    );
    assert_eq!(
        world.versions.checks()?,
        1,
        "the check ran on the candidate"
    );
    assert_eq!(
        world
            .versions
            .reference(derive_proposal_id(mutation(OPENED)))?,
        None
    );
    nothing_stored(&world)
}

#[tokio::test]
async fn a_resumed_open_answers_the_first_proposal_and_writes_nothing_twice() -> TestResult {
    for failing in [Some(ProposalCall::Insert), None] {
        let world = proposal_world()?;
        let changes = vec![create("notes/new.md")?];
        let context = alice(OperationName::OpenProposal, Some(mutation(OPENED)))?;
        match failing {
            Some(call) => world.proposals.fail_next(call)?,
            None => world.events.fail_next(EventKind::ProposalUpdated)?,
        }
        err_of(
            world
                .service
                .open_proposal(&context, open_request(changes.clone())?)
                .await,
        )?;
        let proposal = world
            .service
            .open_proposal(&resumed(context.clone()), open_request(changes.clone())?)
            .await?;
        let again = world
            .service
            .open_proposal(&resumed(context), open_request(changes)?)
            .await?;
        assert_eq!(world.versions.candidates()?.len(), 1, "{failing:?}");
        assert_eq!(world.versions.checks()?, 1, "{failing:?}");
        assert_eq!(world.proposals.rows()?.len(), 1, "{failing:?}");
        assert_eq!(proposal.proposal_revision, revision('c')?);
        assert_eq!(again.proposal_revision, revision('c')?);
        assert_eq!(same(&again.warnings)?, same(&proposal.warnings)?);
        assert_eq!(
            proposal_events(&world, proposal.id)?,
            [Some(mutation(OPENED))],
            "{failing:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn proposals_are_listed_newest_first_by_state_and_read_by_identity_in_their_workspace()
-> TestResult {
    let world = proposal_world()?;
    world.versions.queue(revision('e')?)?;
    let first = open(&world, OPENED, vec![create("notes/one.md")?]).await?;
    let second = open(
        &world,
        OPENED.saturating_add(1),
        vec![create("notes/two.md")?],
    )
    .await?;
    world
        .proposals
        .update(
            &scope()?,
            mutation(SETTING),
            Proposal {
                status: ProposalStatus::Declined,
                ..second.clone()
            },
            ProposalStatus::Open,
        )
        .await?;
    let reader = alice(OperationName::ListProposals, None)?;
    let listed = |status, cursor: Option<String>, limit| ListProposalsRequest {
        workspace_id: WorkspaceId(Uuid::from_u128(1)),
        status,
        page: PageRequest { cursor, limit },
    };
    let all = world
        .service
        .list_proposals(&reader, listed(None, None, 1))
        .await?;
    assert_eq!(
        all.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        [second.id]
    );
    let rest = world
        .service
        .list_proposals(&reader, listed(None, all.next_cursor, 10))
        .await?;
    assert_eq!(
        rest.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        [first.id]
    );
    let open = world
        .service
        .list_proposals(&reader, listed(Some(ProposalStatus::Open), None, 10))
        .await?;
    assert_eq!(
        open.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        [first.id]
    );
    let read = world
        .service
        .get_proposal(
            &alice(OperationName::GetProposal, None)?,
            GetProposalRequest {
                workspace_id: scope()?.workspace_id,
                proposal_id: first.id,
            },
        )
        .await?;
    assert_eq!(read.proposal_revision, first.proposal_revision);
    // In another workspace the proposal does not exist.
    let mut elsewhere = alice(OperationName::GetProposal, None)?;
    elsewhere.grants = vec![grant_on(ELSEWHERE, vec![Permission::Read])?];
    let error = err_of(
        world
            .service
            .get_proposal(
                &elsewhere,
                GetProposalRequest {
                    workspace_id: WorkspaceId(Uuid::from_u128(ELSEWHERE)),
                    proposal_id: first.id,
                },
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound);
    Ok(())
}

/// A grant of `permissions` on workspace `number` of the fixture tenant.
pub fn grant_on(number: u128, permissions: Vec<Permission>) -> Built<WorkspaceGrant> {
    Ok(WorkspaceGrant {
        scope: StorageScope {
            tenant_id: scope()?.tenant_id,
            workspace_id: WorkspaceId(Uuid::from_u128(number)),
        },
        permissions,
    })
}

/// Open the fixture proposal (a new note on `a`, candidate `c`) and confirm its acceptance in
/// Alice's session under `confirming`.
async fn opened_and_confirmed(world: &ProposalWorld) -> Built<(Proposal, Confirmation)> {
    let proposal = open(world, OPENED, vec![create("notes/new.md")?]).await?;
    let confirmation = confirm(world, &proposal, 600).await?;
    Ok((proposal, confirmation))
}

/// Confirm the acceptance of `proposal` in Alice's session under `number`.
async fn confirm(world: &ProposalWorld, proposal: &Proposal, number: u128) -> Built<Confirmation> {
    Ok(world
        .service
        .create_confirmation(
            &in_session(alice(
                OperationName::CreateConfirmation,
                Some(mutation(number)),
            )?),
            CreateConfirmationRequest {
                workspace_id: scope()?.workspace_id,
                action: ConfirmationAction::AcceptProposal,
                target: ConfirmationTarget::Proposal {
                    proposal_id: proposal.id,
                },
                revision: proposal.proposal_revision.clone(),
                content_digest: proposal.content_digest.clone(),
                idempotency_key: key(),
            },
        )
        .await?)
}

fn accepting(number: u128) -> Built<OperationContext> {
    Ok(in_session(alice(
        OperationName::AcceptProposal,
        Some(mutation(number)),
    )?))
}

fn accept_request(
    proposal: &Proposal,
    confirmation: &Confirmation,
    head: char,
) -> Built<AcceptProposalRequest> {
    Ok(AcceptProposalRequest {
        workspace_id: scope()?.workspace_id,
        proposal_id: proposal.id,
        expected_head: revision(head)?,
        proposal_revision: proposal.proposal_revision.clone(),
        confirmation_id: confirmation.id,
        idempotency_key: key(),
    })
}

/// The stored status of `proposal`.
fn status(world: &ProposalWorld, proposal: ProposalId) -> Built<ProposalStatus> {
    Ok(some(
        world
            .proposals
            .rows()?
            .into_iter()
            .find(|row| row.id == proposal),
        "the proposal",
    )?
    .status)
}

#[tokio::test]
async fn accepting_consumes_promotes_marks_indexes_announces_and_records_in_order() -> TestResult {
    let world = proposal_world()?;
    let (proposal, confirmation) = opened_and_confirmed(&world).await?;
    let result = world
        .service
        .accept_proposal(
            &accepting(ACCEPTING)?,
            accept_request(&proposal, &confirmation, 'a')?,
        )
        .await?;
    assert_eq!(result.revision, revision('d')?);
    // The warnings are the proposal's: the findings of the tree that was accepted.
    assert!(!proposal.warnings.is_empty());
    assert_eq!(same(&result.warnings)?, same(&proposal.warnings)?);
    let issued = world.confirmations.issued()?;
    assert_eq!(
        some(issued.first(), "the confirmation")?.2,
        Some(mutation(ACCEPTING))
    );
    let promotions = world.versions.promotions()?;
    let (promotion, landed) = some(promotions.first(), "the promotion")?;
    assert_eq!(promotions.len(), 1);
    assert_eq!(*landed, revision('d')?);
    assert_eq!(promotion.mutation_id, mutation(ACCEPTING));
    assert_eq!(promotion.proposal_id, proposal.id);
    assert_eq!(promotion.expected_head, revision('a')?);
    assert_eq!(promotion.candidate, revision('c')?);
    assert_eq!(promotion.approver.subject, ALICE);
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Accepted);
    assert_eq!(world.world.search.indexed()?, [revision('d')?]);
    let accepted: Vec<NewEvent> = events(&world)?
        .into_iter()
        .filter(|(mutation_id, _)| *mutation_id == Some(mutation(ACCEPTING)))
        .map(|(_, event)| event)
        .collect();
    assert_eq!(accepted.len(), 2, "{accepted:?}");
    let changed = some(accepted.first(), "the changed event")?;
    assert_eq!(changed.kind, EventKind::Changed);
    assert_eq!(changed.revision, Some(revision('d')?));
    assert_eq!(changed.item_id, None);
    let updated = some(accepted.get(1), "the proposal event")?;
    assert_eq!(updated.kind, EventKind::ProposalUpdated);
    assert_eq!(updated.proposal_id, Some(proposal.id));
    let receipts = world.world.records.receipts()?;
    let (under, receipt) = some(receipts.first(), "the receipt")?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(*under, Some(mutation(ACCEPTING)));
    assert_eq!(receipt.id, result.receipt_id);
    assert_eq!(receipt.operation_id, OperationName::AcceptProposal);
    Ok(())
}

#[tokio::test]
async fn accepting_needs_a_browser_session_before_anything_is_read() -> TestResult {
    let world = proposal_world()?;
    let (proposal, confirmation) = opened_and_confirmed(&world).await?;
    let error = err_of(
        world
            .service
            .accept_proposal(
                &alice(OperationName::AcceptProposal, Some(mutation(ACCEPTING)))?,
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(world.confirmations.consumes(), 0);
    assert!(world.versions.promotions()?.is_empty());
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Open);
    Ok(())
}

#[tokio::test]
async fn accepting_refuses_a_proposal_that_is_not_open_or_not_the_one_shown() -> TestResult {
    for (stored, expected) in [
        (ProposalStatus::Declined, ErrorCode::Conflict),
        (ProposalStatus::Conflict, ErrorCode::Conflict),
        (ProposalStatus::Accepted, ErrorCode::Conflict),
    ] {
        let world = proposal_world()?;
        let (proposal, confirmation) = opened_and_confirmed(&world).await?;
        world
            .proposals
            .update(
                &scope()?,
                mutation(SETTING),
                Proposal {
                    status: stored.clone(),
                    ..proposal.clone()
                },
                ProposalStatus::Open,
            )
            .await?;
        for context in [accepting(ACCEPTING)?, resumed(accepting(ACCEPTING)?)] {
            let error = err_of(
                world
                    .service
                    .accept_proposal(&context, accept_request(&proposal, &confirmation, 'a')?)
                    .await,
            )?;
            assert_eq!(error.code, expected, "{stored:?}: {error}");
        }
        assert_eq!(world.confirmations.consumes(), 0, "{stored:?}");
        assert!(world.versions.promotions()?.is_empty(), "{stored:?}");
        assert_eq!(status(&world, proposal.id)?, stored);
    }
    // A proposal revision that is not the proposal's.
    let world = proposal_world()?;
    let (proposal, confirmation) = opened_and_confirmed(&world).await?;
    let mut request = accept_request(&proposal, &confirmation, 'a')?;
    request.proposal_revision = revision('f')?;
    let error = err_of(
        world
            .service
            .accept_proposal(&accepting(ACCEPTING)?, request)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(error.field.as_deref(), Some("/proposal_revision"));
    assert_eq!(world.confirmations.consumes(), 0);
    assert!(world.versions.promotions()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn the_confirmation_is_consumed_before_anything_is_promoted() -> TestResult {
    let world = proposal_world()?;
    let (proposal, confirmation) = opened_and_confirmed(&world).await?;
    // Another session presents the confirmation: the store refuses it, and nothing is promoted.
    let mut other = accepting(ACCEPTING)?;
    other.session_id = Some("session-2".to_owned());
    let error = err_of(
        world
            .service
            .accept_proposal(&other, accept_request(&proposal, &confirmation, 'a')?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert!(world.versions.promotions()?.is_empty());
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Open);
    // The same mutation, in the person's own session, then accepts with that confirmation.
    world
        .service
        .accept_proposal(
            &accepting(ACCEPTING)?,
            accept_request(&proposal, &confirmation, 'a')?,
        )
        .await?;
    // In a fresh world: a confirmation consumed by an attempt whose promotion then failed is
    // refused to another mutation, and nothing is promoted.
    let world = proposal_world()?;
    let (proposal, confirmation) = opened_and_confirmed(&world).await?;
    world.versions.fail_next_promote();
    err_of(
        world
            .service
            .accept_proposal(
                &accepting(ACCEPTING)?,
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    // The failed attempt consumed it; another mutation cannot.
    let error = err_of(
        world
            .service
            .accept_proposal(
                &accepting(ACCEPTING.saturating_add(1))?,
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict);
    assert!(world.versions.promotions()?.is_empty());
    Ok(())
}

/// The refusal of a stale acceptance: `Conflict` with `StaleBase { head }` on `field`.
fn assert_stale(error: &ApiError, head: &Revision, field: Option<&str>) {
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert_eq!(error.field.as_deref(), field);
    assert_eq!(
        error.detail.as_deref(),
        Some(&ErrorDetail::StaleBase { head: head.clone() })
    );
}

#[tokio::test]
async fn a_head_that_moved_makes_the_proposal_conflict_with_the_head() -> TestResult {
    let world = proposal_world()?;
    let (proposal, confirmation) = opened_and_confirmed(&world).await?;
    world.world.versions.set_head(revision('e')?)?;
    let error = err_of(
        world
            .service
            .accept_proposal(
                &accepting(ACCEPTING)?,
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    // The head moved: the request's `expected_head` is the stale field.
    assert_stale(&error, &revision('e')?, Some("/expected_head"));
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Conflict);
    assert!(world.versions.promotions()?.is_empty());
    assert_eq!(world.world.search.indexed()?, Vec::<Revision>::new());
    Ok(())
}

#[tokio::test]
async fn a_candidate_drafted_on_an_earlier_head_makes_the_proposal_conflict() -> TestResult {
    let world = proposal_world()?;
    let (proposal, confirmation) = opened_and_confirmed(&world).await?;
    // The person was shown the current head, `e`, but the candidate sits on `a`.
    world.world.versions.set_head(revision('e')?)?;
    let error = err_of(
        world
            .service
            .accept_proposal(
                &accepting(ACCEPTING)?,
                accept_request(&proposal, &confirmation, 'e')?,
            )
            .await,
    )?;
    // The store refuses it with the head (contract ruling 5: a base that is not the head is
    // stale); the request named the head, so no request field is stale.
    assert_stale(&error, &revision('e')?, None);
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Conflict);
    // The promotion was asked for onto the head shown, and the store refused it: nothing landed.
    let attempts = world.versions.attempts()?;
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        some(attempts.first(), "the promotion asked for")?.expected_head,
        revision('e')?
    );
    assert!(world.versions.promotions()?.is_empty());
    Ok(())
}

/// Review probe P4, up to the racing acceptance: the first acceptance's promotion landed and its
/// status change failed, so the proposal is still `open`; the store refuses a second acceptance
/// with the head the first moved, the proposal is marked `conflict`, and the mark is announced
/// with a `proposal_updated` event under the second acceptance's mutation (review N1).
async fn racing_acceptance(world: &ProposalWorld) -> Built<(Proposal, Confirmation)> {
    let (proposal, confirmation) = opened_and_confirmed(world).await?;
    let racing = confirm(world, &proposal, 601).await?;
    fail_at(world, Step::Status)?;
    err_of(
        world
            .service
            .accept_proposal(
                &accepting(ACCEPTING)?,
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    assert_eq!(status(world, proposal.id)?, ProposalStatus::Open);
    let error = err_of(
        world
            .service
            .accept_proposal(
                &accepting(ACCEPTING.saturating_add(1))?,
                accept_request(&proposal, &racing, 'a')?,
            )
            .await,
    )?;
    // The head moved under the second acceptance: `d`, the first one's promotion.
    assert_stale(&error, &revision('d')?, Some("/expected_head"));
    assert_eq!(status(world, proposal.id)?, ProposalStatus::Conflict);
    Ok((proposal, confirmation))
}

#[tokio::test]
async fn a_stale_acceptance_announces_the_conflict_it_marks() -> TestResult {
    let world = proposal_world()?;
    let (proposal, _) = racing_acceptance(&world).await?;
    assert_eq!(
        proposal_events(&world, proposal.id)?,
        [
            Some(mutation(OPENED)),
            Some(mutation(ACCEPTING.saturating_add(1)))
        ]
    );
    Ok(())
}

/// Review probe P4 whole (N2): after a racing acceptance marked the proposal `conflict`, the
/// first acceptance's retry finds its promotion by its mutation and writes `accepted` over the
/// false mark (a compare-and-set from `conflict`), then finishes; a repeat answers the same.
#[tokio::test]
async fn a_landed_acceptance_is_finished_over_the_conflict_a_racing_one_marked() -> TestResult {
    let world = proposal_world()?;
    let (proposal, confirmation) = racing_acceptance(&world).await?;
    let mut answers = Vec::new();
    for _ in 0..2 {
        answers.push(
            world
                .service
                .accept_proposal(
                    &resumed(accepting(ACCEPTING)?),
                    accept_request(&proposal, &confirmation, 'a')?,
                )
                .await?,
        );
    }
    let result = some(answers.first(), "the retry's answer")?;
    let again = some(answers.get(1), "the repeat's answer")?;
    assert_eq!(result.revision, revision('d')?);
    assert_eq!(again.revision, result.revision);
    assert_eq!(again.receipt_id, result.receipt_id);
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Accepted);
    assert_eq!(
        world.proposals.writes()?,
        [ProposalStatus::Conflict, ProposalStatus::Accepted]
    );
    assert_eq!(world.versions.promotions()?.len(), 1);
    assert_eq!(world.world.search.indexed()?, [revision('d')?]);
    assert_eq!(
        proposal_events(&world, proposal.id)?,
        [
            Some(mutation(OPENED)),
            Some(mutation(ACCEPTING.saturating_add(1))),
            Some(mutation(ACCEPTING))
        ]
    );
    assert_eq!(world.world.records.receipts()?.len(), 1);
    Ok(())
}

/// A stale acceptance whose event failed after it marked the proposal `conflict` is finished by
/// its retry: the store takes the same mark under the same mutation as a retry, so the event is
/// appended once and the stale refusal answered again; nothing is consumed or promoted again.
/// Another mutation's attempt on that proposal is refused as any `conflict` one is.
#[tokio::test]
async fn a_resumed_stale_acceptance_announces_its_mark_after_a_failure_at_the_event() -> TestResult
{
    let world = proposal_world()?;
    let (proposal, confirmation) = opened_and_confirmed(&world).await?;
    world.world.versions.set_head(revision('e')?)?;
    world.events.fail_next(EventKind::ProposalUpdated)?;
    let error = err_of(
        world
            .service
            .accept_proposal(
                &accepting(ACCEPTING)?,
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Unavailable, "{error}");
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Conflict);
    assert_eq!(
        proposal_events(&world, proposal.id)?,
        [Some(mutation(OPENED))]
    );
    let consumes = world.confirmations.consumes();
    for _ in 0..2 {
        let error = err_of(
            world
                .service
                .accept_proposal(
                    &resumed(accepting(ACCEPTING)?),
                    accept_request(&proposal, &confirmation, 'a')?,
                )
                .await,
        )?;
        assert_stale(&error, &revision('e')?, Some("/expected_head"));
    }
    assert_eq!(
        proposal_events(&world, proposal.id)?,
        [Some(mutation(OPENED)), Some(mutation(ACCEPTING))]
    );
    assert_eq!(
        world.confirmations.consumes(),
        consumes,
        "not consumed again"
    );
    assert_eq!(world.versions.attempts()?.len(), 1, "not promoted again");
    assert_eq!(world.proposals.writes()?, [ProposalStatus::Conflict]);
    // A resumed attempt of a mutation that did not mark it gets the plain refusal.
    let error = err_of(
        world
            .service
            .accept_proposal(
                &resumed(accepting(ACCEPTING.saturating_add(1))?),
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert_eq!(error.detail, None, "{error}");
    assert_eq!(error.field, None, "{error}");
    assert_eq!(
        proposal_events(&world, proposal.id)?,
        [Some(mutation(OPENED)), Some(mutation(ACCEPTING))]
    );
    Ok(())
}

/// Review R1-N5: of two stale acceptances that both read the proposal `open`, the second's mark
/// is refused because the first marked it in between; the second still answers the store's
/// stale refusal (`StaleBase { head }` on `/expected_head`), not the store's plain refusal of
/// its mark, and it marks and announces nothing of its own.
#[tokio::test]
async fn a_stale_acceptance_whose_mark_another_won_still_answers_stale() -> TestResult {
    let world = proposal_world()?;
    let (proposal, confirmation) = opened_and_confirmed(&world).await?;
    world.world.versions.set_head(revision('e')?)?;
    // The other stale acceptance's mark lands after this one read the proposal `open`.
    world.proposals.land_before_next_update(
        &scope()?,
        mutation(ACCEPTING.saturating_add(1)),
        Proposal {
            status: ProposalStatus::Conflict,
            ..proposal.clone()
        },
        ProposalStatus::Open,
    )?;
    let error = err_of(
        world
            .service
            .accept_proposal(
                &accepting(ACCEPTING)?,
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    assert_stale(&error, &revision('e')?, Some("/expected_head"));
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Conflict);
    // The only status written is the other acceptance's mark, and this one announced nothing.
    assert_eq!(world.proposals.writes()?, [ProposalStatus::Conflict]);
    assert_eq!(
        proposal_events(&world, proposal.id)?,
        [Some(mutation(OPENED))]
    );
    Ok(())
}

/// A store fault at the stale mark is answered as given: only the mark's `Conflict` becomes the
/// stale refusal. The proposal stays `open`, and the retry marks it, announces it and answers
/// the stale refusal.
#[tokio::test]
async fn a_store_fault_at_the_stale_mark_is_answered_as_given_and_the_retry_marks_it() -> TestResult
{
    let world = proposal_world()?;
    let (proposal, confirmation) = opened_and_confirmed(&world).await?;
    world.world.versions.set_head(revision('e')?)?;
    world.proposals.fail_next(ProposalCall::Update)?;
    let error = err_of(
        world
            .service
            .accept_proposal(
                &accepting(ACCEPTING)?,
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Unavailable, "{error}");
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Open);
    let error = err_of(
        world
            .service
            .accept_proposal(
                &resumed(accepting(ACCEPTING)?),
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    assert_stale(&error, &revision('e')?, Some("/expected_head"));
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Conflict);
    assert_eq!(
        proposal_events(&world, proposal.id)?,
        [Some(mutation(OPENED)), Some(mutation(ACCEPTING))]
    );
    Ok(())
}

#[tokio::test]
async fn a_plain_conflict_of_the_promotion_leaves_the_proposal_open() -> TestResult {
    let world = proposal_world()?;
    // A proposal whose row names a candidate its reference does not retain.
    let id = ProposalId(Uuid::from_u128(88));
    let proposal: Proposal = serde_json::from_value(json!({
        "id": id,
        "workspace_id": scope()?.workspace_id,
        "base_revision": revision('a')?,
        "proposal_revision": revision('f')?,
        "content_digest": digest('d')?,
        "title": "t",
        "description": "d",
        "changes": [],
        "kind": "content",
        "status": "open",
        "created_by": "agent",
        "created_via": "mcp_delegation",
        "created_at": "2026-10-09T12:00:00.000Z"
    }))?;
    world
        .proposals
        .put(&scope()?, mutation(80), proposal.clone())?;
    world
        .versions
        .retain(id, revision('c')?, revision('a')?, mutation(80))?;
    let confirmation = confirm(&world, &proposal, 600).await?;
    let error = err_of(
        world
            .service
            .accept_proposal(
                &accepting(ACCEPTING)?,
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(error.detail, None, "{error}");
    assert_eq!(error.field, None);
    assert_eq!(status(&world, id)?, ProposalStatus::Open);
    // Any other failure of the promotion also leaves the status.
    world.versions.fail_next_promote();
    let error = err_of(
        world
            .service
            .accept_proposal(
                &resumed(accepting(ACCEPTING)?),
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert_eq!(status(&world, id)?, ProposalStatus::Open);
    Ok(())
}

fn fail_at(world: &ProposalWorld, step: Step) -> TestResult {
    match step {
        Step::Promote => world.versions.fail_next_promote(),
        Step::Status => world.proposals.fail_next(ProposalCall::Update)?,
        Step::Index => world.world.search.fail_next(),
        Step::Changed => world.events.fail_next(EventKind::Changed)?,
        Step::Announced => world.events.fail_next(EventKind::ProposalUpdated)?,
        Step::Receipt => world.world.records.fail_next_receipt(),
    }
    Ok(())
}

#[tokio::test]
async fn a_resumed_acceptance_finishes_after_a_failure_at_each_step() -> TestResult {
    for step in [
        Step::Promote,
        Step::Status,
        Step::Index,
        Step::Changed,
        Step::Announced,
        Step::Receipt,
    ] {
        let world = proposal_world()?;
        let (proposal, confirmation) = opened_and_confirmed(&world).await?;
        fail_at(&world, step)?;
        err_of(
            world
                .service
                .accept_proposal(
                    &accepting(ACCEPTING)?,
                    accept_request(&proposal, &confirmation, 'a')?,
                )
                .await,
        )?;
        let result = world
            .service
            .accept_proposal(
                &resumed(accepting(ACCEPTING)?),
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await?;
        // A completed acceptance repeated under its mutation answers the same.
        let again = world
            .service
            .accept_proposal(
                &resumed(accepting(ACCEPTING)?),
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await?;
        assert_eq!(result.revision, revision('d')?, "{step:?}");
        assert_eq!(again.revision, result.revision, "{step:?}");
        assert_eq!(again.receipt_id, result.receipt_id, "{step:?}");
        assert_eq!(
            same(&again.warnings)?,
            same(&proposal.warnings)?,
            "{step:?}"
        );
        assert_eq!(world.versions.promotions()?.len(), 1, "{step:?}");
        assert_eq!(status(&world, proposal.id)?, ProposalStatus::Accepted);
        assert_eq!(
            world.proposals.writes()?,
            [ProposalStatus::Accepted],
            "{step:?}"
        );
        assert_eq!(world.world.search.indexed()?, [revision('d')?], "{step:?}");
        let mine: Vec<EventKind> = events(&world)?
            .into_iter()
            .filter(|(under, _)| *under == Some(mutation(ACCEPTING)))
            .map(|(_, event)| event.kind)
            .collect();
        assert_eq!(
            mine,
            [EventKind::Changed, EventKind::ProposalUpdated],
            "{step:?}"
        );
        assert_eq!(world.world.records.receipts()?.len(), 1, "{step:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_landed_promotion_is_finished_after_its_confirmation_expired() -> TestResult {
    let world = proposal_world()?;
    let (proposal, confirmation) = opened_and_confirmed(&world).await?;
    fail_at(&world, Step::Status)?;
    err_of(
        world
            .service
            .accept_proposal(
                &accepting(ACCEPTING)?,
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    // The retry comes after the confirmation expired; its promotion landed, so it finishes.
    world
        .confirmations
        .set_now(Timestamp::try_from("2026-10-09T13:00:00.000Z".to_owned())?)?;
    let consumes = world.confirmations.consumes();
    let result = world
        .service
        .accept_proposal(
            &resumed(accepting(ACCEPTING)?),
            accept_request(&proposal, &confirmation, 'a')?,
        )
        .await?;
    assert_eq!(result.revision, revision('d')?);
    assert_eq!(
        world.confirmations.consumes(),
        consumes,
        "not consumed again"
    );
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Accepted);
    // A first attempt of another mutation is refused: the proposal is accepted.
    let error = err_of(
        world
            .service
            .accept_proposal(
                &accepting(ACCEPTING.saturating_add(1))?,
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict);
    Ok(())
}

#[tokio::test]
async fn an_acceptance_whose_status_change_loses_to_a_decline_answers_conflict() -> TestResult {
    let world = proposal_world()?;
    let (proposal, confirmation) = opened_and_confirmed(&world).await?;
    fail_at(&world, Step::Status)?;
    err_of(
        world
            .service
            .accept_proposal(
                &accepting(ACCEPTING)?,
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    // Someone declines the still-open proposal before the retry (the module header's known
    // limit): the retry's compare-and-set from `open` is refused.
    decline(&world, 800).await?;
    let error = err_of(
        world
            .service
            .accept_proposal(
                &resumed(accepting(ACCEPTING)?),
                accept_request(&proposal, &confirmation, 'a')?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Declined);
    Ok(())
}

/// Decline the fixture proposal (the only one) as Alice under `number`.
async fn decline(world: &ProposalWorld, number: u128) -> Built<Proposal> {
    let proposal = some(world.proposals.rows()?.first().cloned(), "a proposal")?;
    Ok(world
        .service
        .decline_proposal(
            &alice(OperationName::DeclineProposal, Some(mutation(number)))?,
            DeclineProposalRequest {
                workspace_id: scope()?.workspace_id,
                proposal_id: proposal.id,
                reason: "Not what we need.".to_owned(),
                idempotency_key: key(),
            },
        )
        .await?)
}

fn decline_request(proposal: &Proposal) -> Built<DeclineProposalRequest> {
    Ok(DeclineProposalRequest {
        workspace_id: scope()?.workspace_id,
        proposal_id: proposal.id,
        reason: "Not what we need.".to_owned(),
        idempotency_key: key(),
    })
}

#[tokio::test]
async fn declining_records_its_reason_as_a_comment_and_closes_the_proposal() -> TestResult {
    let world = proposal_world()?;
    let proposal = open(&world, OPENED, vec![create("notes/new.md")?]).await?;
    let declined = decline(&world, DECLINING).await?;
    assert_eq!(declined.status, ProposalStatus::Declined);
    assert_eq!(declined.id, proposal.id);
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Declined);
    // The reason is a comment by the person who declined, under the decline's mutation.
    let listed = world
        .service
        .list_comments(
            &alice(OperationName::ListComments, None)?,
            ListCommentsRequest {
                workspace_id: scope()?.workspace_id,
                proposal_id: proposal.id,
                page: PageRequest {
                    cursor: None,
                    limit: 10,
                },
            },
        )
        .await?;
    let comment = some(listed.items.first(), "the reason")?;
    assert_eq!(listed.items.len(), 1);
    assert_eq!(comment.id, derive_comment_id(mutation(DECLINING)));
    assert_eq!(comment.author, ALICE);
    assert_eq!(comment.text, "Not what we need.");
    assert!(comment.source.is_none());
    assert_eq!(
        proposal_events(&world, proposal.id)?,
        [Some(mutation(OPENED)), Some(mutation(DECLINING))]
    );
    // The candidate reference stays: it is history.
    assert_eq!(world.versions.reference(proposal.id)?, Some(revision('c')?));
    Ok(())
}

#[tokio::test]
async fn declining_refuses_a_proposal_that_is_not_open() -> TestResult {
    for stored in [
        ProposalStatus::Accepted,
        ProposalStatus::Conflict,
        ProposalStatus::Declined,
    ] {
        let world = proposal_world()?;
        let proposal = open(&world, OPENED, vec![create("notes/new.md")?]).await?;
        world
            .proposals
            .update(
                &scope()?,
                mutation(SETTING),
                Proposal {
                    status: stored.clone(),
                    ..proposal.clone()
                },
                ProposalStatus::Open,
            )
            .await?;
        // A resumed attempt that wrote no comment did not decline it either.
        for context in [
            alice(OperationName::DeclineProposal, Some(mutation(DECLINING)))?,
            resumed(alice(
                OperationName::DeclineProposal,
                Some(mutation(DECLINING)),
            )?),
        ] {
            let error = err_of(
                world
                    .service
                    .decline_proposal(&context, decline_request(&proposal)?)
                    .await,
            )?;
            assert_eq!(error.code, ErrorCode::Conflict, "{stored:?}");
        }
        assert!(world.proposals.comments()?.is_empty(), "{stored:?}");
        assert_eq!(status(&world, proposal.id)?, stored);
    }
    Ok(())
}

#[tokio::test]
async fn a_resumed_decline_finishes_after_a_failure_at_each_step() -> TestResult {
    for failing in [
        Some(ProposalCall::AddComment),
        Some(ProposalCall::Update),
        None,
    ] {
        let world = proposal_world()?;
        let proposal = open(&world, OPENED, vec![create("notes/new.md")?]).await?;
        match failing {
            Some(call) => world.proposals.fail_next(call)?,
            None => world.events.fail_next(EventKind::ProposalUpdated)?,
        }
        let context = alice(OperationName::DeclineProposal, Some(mutation(DECLINING)))?;
        err_of(
            world
                .service
                .decline_proposal(&context, decline_request(&proposal)?)
                .await,
        )?;
        let declined = world
            .service
            .decline_proposal(&resumed(context.clone()), decline_request(&proposal)?)
            .await?;
        let again = world
            .service
            .decline_proposal(&resumed(context), decline_request(&proposal)?)
            .await?;
        assert_eq!(declined.status, ProposalStatus::Declined, "{failing:?}");
        assert_eq!(again.status, ProposalStatus::Declined, "{failing:?}");
        assert_eq!(world.proposals.comments()?.len(), 1, "{failing:?}");
        assert_eq!(world.proposals.writes()?, [ProposalStatus::Declined]);
        assert_eq!(
            proposal_events(&world, proposal.id)?,
            [Some(mutation(OPENED)), Some(mutation(DECLINING))],
            "{failing:?}"
        );
    }
    Ok(())
}

/// A resumed decline with 200 earlier comments on the proposal answers its own decline: it is
/// told by the store's writer rule, not by a search of the discussion (review N6 pinned the
/// search this replaced, `has_comment`, past its first page of 200).
#[tokio::test]
async fn a_resumed_decline_answers_its_own_decline_after_many_earlier_comments() -> TestResult {
    let world = proposal_world()?;
    let proposal = open(&world, OPENED, vec![create("notes/new.md")?]).await?;
    for number in 0..200_u128 {
        let comment = serde_json::from_value(json!({
            "id": format!("earlier-{number}"),
            "author": ALICE,
            "text": "An earlier remark.",
            "created_at": "2026-10-09T12:00:00.000Z"
        }))?;
        world
            .proposals
            .add_comment(
                &scope()?,
                mutation(COMMENTING.saturating_add(number)),
                proposal.id,
                comment,
            )
            .await?;
    }
    // The decline wrote its comment and its status; its event failed.
    world.events.fail_next(EventKind::ProposalUpdated)?;
    let context = alice(OperationName::DeclineProposal, Some(mutation(DECLINING)))?;
    err_of(
        world
            .service
            .decline_proposal(&context, decline_request(&proposal)?)
            .await,
    )?;
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Declined);
    let declined = world
        .service
        .decline_proposal(&resumed(context), decline_request(&proposal)?)
        .await?;
    assert_eq!(declined.status, ProposalStatus::Declined);
    assert_eq!(world.proposals.comments()?.len(), 201);
    assert_eq!(
        proposal_events(&world, proposal.id)?,
        [Some(mutation(OPENED)), Some(mutation(DECLINING))]
    );
    Ok(())
}

/// Review probe P8 (R1-N1): a decline whose first attempt wrote its reason comment and failed
/// before its status change, retried after another person's decline landed, answers `conflict`
/// and appends no event: the store refuses its declined record, which another mutation wrote.
/// One `declined` write is recorded, the other decline's.
#[tokio::test]
async fn a_resumed_decline_after_another_persons_decline_answers_conflict() -> TestResult {
    let world = proposal_world()?;
    let proposal = open(&world, OPENED, vec![create("notes/new.md")?]).await?;
    world.proposals.fail_next(ProposalCall::Update)?;
    let late = alice(
        OperationName::DeclineProposal,
        Some(mutation(DECLINING.saturating_add(1))),
    )?;
    err_of(
        world
            .service
            .decline_proposal(&late, decline_request(&proposal)?)
            .await,
    )?;
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Open);
    decline(&world, DECLINING).await?;
    let error = err_of(
        world
            .service
            .decline_proposal(&resumed(late), decline_request(&proposal)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert_eq!(world.proposals.writes()?, [ProposalStatus::Declined]);
    assert_eq!(
        proposal_events(&world, proposal.id)?,
        [Some(mutation(OPENED)), Some(mutation(DECLINING))]
    );
    // Both reasons stay: the late decline's comment is the stated limit.
    assert_eq!(world.proposals.comments()?.len(), 2);
    Ok(())
}

/// Review R1-N2: of two declines by different mutations the second is refused, and a retry of
/// the first answers its result again (the declined proposal, its one event), because the store
/// takes the declined record as a retry only under the mutation that wrote it.
#[tokio::test]
async fn of_two_declines_the_second_is_refused_and_a_retry_of_the_first_answers_it() -> TestResult {
    let world = proposal_world()?;
    let proposal = open(&world, OPENED, vec![create("notes/new.md")?]).await?;
    let first = decline(&world, DECLINING).await?;
    let second = alice(
        OperationName::DeclineProposal,
        Some(mutation(DECLINING.saturating_add(1))),
    )?;
    for context in [second.clone(), resumed(second)] {
        let error = err_of(
            world
                .service
                .decline_proposal(&context, decline_request(&proposal)?)
                .await,
        )?;
        assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    }
    let again = world
        .service
        .decline_proposal(
            &resumed(alice(
                OperationName::DeclineProposal,
                Some(mutation(DECLINING)),
            )?),
            decline_request(&proposal)?,
        )
        .await?;
    assert_eq!(same(&again)?, same(&first)?);
    assert_eq!(world.proposals.writes()?, [ProposalStatus::Declined]);
    assert_eq!(
        proposal_events(&world, proposal.id)?,
        [Some(mutation(OPENED)), Some(mutation(DECLINING))]
    );
    Ok(())
}

/// The decline's stated limit (review probe P5, N4): a decline that failed after its reason
/// comment and before its compare-and-set, retried after an acceptance landed, answers
/// `conflict`, and its reason stays on the accepted proposal; no decline event is appended.
#[tokio::test]
async fn a_decline_that_loses_to_an_acceptance_leaves_its_reason_on_the_accepted_proposal()
-> TestResult {
    let world = proposal_world()?;
    let (proposal, confirmation) = opened_and_confirmed(&world).await?;
    world.proposals.fail_next(ProposalCall::Update)?;
    let context = alice(OperationName::DeclineProposal, Some(mutation(DECLINING)))?;
    err_of(
        world
            .service
            .decline_proposal(&context, decline_request(&proposal)?)
            .await,
    )?;
    world
        .service
        .accept_proposal(
            &accepting(ACCEPTING)?,
            accept_request(&proposal, &confirmation, 'a')?,
        )
        .await?;
    let error = err_of(
        world
            .service
            .decline_proposal(&resumed(context), decline_request(&proposal)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert_eq!(status(&world, proposal.id)?, ProposalStatus::Accepted);
    let comments = world.proposals.comments()?;
    let (under, reason) = some(comments.first(), "the reason")?;
    assert_eq!(comments.len(), 1);
    assert_eq!(*under, mutation(DECLINING));
    assert_eq!(reason.text, "Not what we need.");
    assert_eq!(
        proposal_events(&world, proposal.id)?,
        [Some(mutation(OPENED)), Some(mutation(ACCEPTING))]
    );
    Ok(())
}

/// A citation of the whole plan note at `a` in workspace `workspace`.
fn plan_citation(workspace: u128) -> Built<SourceReference> {
    Ok(SourceReference {
        workspace_id: WorkspaceId(Uuid::from_u128(workspace)),
        item_id: ItemId(Uuid::from_u128(NOTE)),
        path: path("notes/plan.md")?,
        revision: revision('a')?,
        digest: None,
        selection: Selection::All,
        locations: Vec::new(),
    })
}

fn comment_request(
    proposal: &Proposal,
    source: Option<SourceReference>,
) -> Built<AddCommentRequest> {
    Ok(AddCommentRequest {
        workspace_id: scope()?.workspace_id,
        proposal_id: proposal.id,
        text: "Line 3 is wrong.".to_owned(),
        source,
        idempotency_key: key(),
    })
}

fn commenting(number: u128) -> Built<OperationContext> {
    alice(OperationName::AddComment, Some(mutation(number)))
}

#[tokio::test]
async fn a_comment_keeps_its_checked_citation_on_a_proposal_in_any_state() -> TestResult {
    let world = proposal_world()?;
    let proposal = open(&world, OPENED, vec![create("notes/new.md")?]).await?;
    decline(&world, DECLINING).await?;
    let comment = world
        .service
        .add_comment(
            &commenting(COMMENTING)?,
            comment_request(&proposal, Some(plan_citation(1)?))?,
        )
        .await?;
    assert_eq!(comment.id, derive_comment_id(mutation(COMMENTING)));
    assert_eq!(comment.author, ALICE);
    assert_eq!(comment.text, "Line 3 is wrong.");
    assert_eq!(comment.created_at.as_str(), "2026-10-09T12:00:00.000Z");
    assert_eq!(same(&comment.source)?, same(&Some(plan_citation(1)?))?);
    let held = world.proposals.comments()?;
    assert_eq!(
        held.last().map(|(under, kept)| (*under, kept.id.clone())),
        Some((mutation(COMMENTING), comment.id.clone()))
    );
    assert_eq!(
        proposal_events(&world, proposal.id)?.last(),
        Some(&Some(mutation(COMMENTING)))
    );
    // A retry answers the same comment and appends nothing more.
    let again = world
        .service
        .add_comment(
            &resumed(commenting(COMMENTING)?),
            comment_request(&proposal, Some(plan_citation(1)?))?,
        )
        .await?;
    assert_eq!(again.id, comment.id);
    assert_eq!(world.proposals.comments()?.len(), 2);
    assert_eq!(proposal_events(&world, proposal.id)?.len(), 3);
    Ok(())
}

#[tokio::test]
async fn a_comment_on_a_proposal_the_workspace_does_not_hold_is_not_found() -> TestResult {
    let world = proposal_world()?;
    let proposal = open(&world, OPENED, vec![create("notes/new.md")?]).await?;
    let mut missing = comment_request(&proposal, None)?;
    missing.proposal_id = ProposalId(Uuid::from_u128(99));
    let error = err_of(
        world
            .service
            .add_comment(&commenting(COMMENTING)?, missing)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound);
    assert!(world.proposals.comments()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_comments_citation_is_checked_as_every_citation_is() -> TestResult {
    let world = proposal_world()?;
    let proposal = open(&world, OPENED, vec![create("notes/new.md")?]).await?;
    let purge_id = PurgeId(Uuid::from_u128(9));
    world.world.records.put_mapping(
        revision('b')?,
        RevisionMapping {
            purge_id,
            replacement: None,
        },
    )?;
    let mut elsewhere = plan_citation(1)?;
    elsewhere.path = path("notes/other.md")?;
    // The note is at `b` too: only the purge check refuses its citation there.
    world
        .world
        .versions
        .put_document(&revision('b')?, plan(&revision('b')?)?)?;
    let mut purged = plan_citation(1)?;
    purged.revision = revision('b')?;
    let mut located = plan_citation(1)?;
    located.locations = vec![SourceLocation::Unresolved {
        reason: UnresolvedReason::SuppliedText,
    }];
    let mut digested = plan_citation(1)?;
    digested.digest = Some(digest('9')?);
    for (source, code, field) in [
        (elsewhere, ErrorCode::InvalidInput, Some("/source/path")),
        (purged, ErrorCode::NotFound, None),
        (located, ErrorCode::InvalidInput, Some("/source/locations")),
        (digested, ErrorCode::NotFound, Some("/source/digest")),
    ] {
        let error = err_of(
            world
                .service
                .add_comment(
                    &commenting(COMMENTING)?,
                    comment_request(&proposal, Some(source))?,
                )
                .await,
        )?;
        assert_eq!(error.code, code, "{error}");
        assert_eq!(error.field.as_deref(), field, "{error}");
        if field.is_none() {
            assert_eq!(
                error.detail.as_deref(),
                Some(&ErrorDetail::Invalidated {
                    purge_id,
                    replacement: None
                })
            );
        }
    }
    assert!(world.proposals.comments()?.is_empty());
    Ok(())
}

/// A context of Alice that dispatch authorized on the fixture workspace and, for reading, on
/// `ELSEWHERE`.
fn with_elsewhere(operation: OperationName, number: Option<u128>) -> Built<OperationContext> {
    let mut context = alice(operation, number.map(mutation))?;
    context
        .grants
        .push(grant_on(ELSEWHERE, vec![Permission::Read])?);
    Ok(context)
}

/// List the fixture proposal's comments as `context`'s reader.
async fn comments_as(
    world: &ProposalWorld,
    context: &OperationContext,
    proposal: &Proposal,
) -> Built<Vec<okf_jawn_contract::proposal::Comment>> {
    Ok(world
        .service
        .list_comments(
            context,
            ListCommentsRequest {
                workspace_id: scope()?.workspace_id,
                proposal_id: proposal.id,
                page: PageRequest {
                    cursor: None,
                    limit: 10,
                },
            },
        )
        .await?
        .items)
}

#[tokio::test]
async fn comments_are_listed_oldest_first_page_by_page() -> TestResult {
    let world = proposal_world()?;
    let proposal = open(&world, OPENED, vec![create("notes/new.md")?]).await?;
    for number in [COMMENTING, COMMENTING.saturating_add(1)] {
        world
            .service
            .add_comment(&commenting(number)?, comment_request(&proposal, None)?)
            .await?;
    }
    let reader = alice(OperationName::ListComments, None)?;
    let first = world
        .service
        .list_comments(
            &reader,
            ListCommentsRequest {
                workspace_id: scope()?.workspace_id,
                proposal_id: proposal.id,
                page: PageRequest {
                    cursor: None,
                    limit: 1,
                },
            },
        )
        .await?;
    let rest = world
        .service
        .list_comments(
            &reader,
            ListCommentsRequest {
                workspace_id: scope()?.workspace_id,
                proposal_id: proposal.id,
                page: PageRequest {
                    cursor: first.next_cursor,
                    limit: 10,
                },
            },
        )
        .await?;
    let ids: Vec<String> = first
        .items
        .iter()
        .chain(&rest.items)
        .map(|comment| comment.id.clone())
        .collect();
    assert_eq!(
        ids,
        [
            derive_comment_id(mutation(COMMENTING)),
            derive_comment_id(mutation(COMMENTING.saturating_add(1)))
        ]
    );
    assert_eq!(rest.next_cursor, None);
    Ok(())
}

#[tokio::test]
async fn a_citation_in_a_workspace_the_reader_may_not_read_is_left_out() -> TestResult {
    let world = proposal_world()?;
    let proposal = open(&world, OPENED, vec![create("notes/new.md")?]).await?;
    // Two comments cite the other workspace, one cites the proposal's own.
    for (number, workspace) in [
        (COMMENTING, ELSEWHERE),
        (COMMENTING.saturating_add(1), ELSEWHERE),
        (COMMENTING.saturating_add(2), 1),
    ] {
        world
            .service
            .add_comment(
                &with_elsewhere(OperationName::AddComment, Some(number))?,
                comment_request(&proposal, Some(plan_citation(workspace)?))?,
            )
            .await?;
    }
    let elsewhere = WorkspaceId(Uuid::from_u128(ELSEWHERE));
    let reader = alice(OperationName::ListComments, None)?;
    let shown = |listed: &[okf_jawn_contract::proposal::Comment]| -> Vec<bool> {
        listed
            .iter()
            .map(|comment| comment.source.is_some())
            .collect()
    };
    // No grant on the other workspace: those comments, without their citation.
    let listed = comments_as(&world, &reader, &proposal).await?;
    assert_eq!(shown(&listed), [false, false, true]);
    // A grant without `read` is no better.
    world.access.grant(elsewhere, vec![Permission::Write])?;
    let listed = comments_as(&world, &reader, &proposal).await?;
    assert_eq!(shown(&listed), [false, false, true]);
    // With `read`, the citations are there.
    world.access.grant(elsewhere, vec![Permission::Read])?;
    let listed = comments_as(&world, &reader, &proposal).await?;
    assert_eq!(shown(&listed), [true, true, true]);
    assert_eq!(
        same(&some(listed.first(), "the comment")?.source)?,
        same(&Some(plan_citation(ELSEWHERE)?))?
    );
    // Each listing looks the grant up again, once for the workspace however many cite it, and
    // never for the proposal's own workspace.
    assert_eq!(world.access.lookups()?, [elsewhere, elsewhere, elsewhere]);
    // A lookup that fails otherwise than by refusing fails the listing.
    world.access.fail(elsewhere, ErrorCode::Unavailable)?;
    let error = err_of(
        world
            .service
            .list_comments(
                &reader,
                ListCommentsRequest {
                    workspace_id: scope()?.workspace_id,
                    proposal_id: proposal.id,
                    page: PageRequest {
                        cursor: None,
                        limit: 10,
                    },
                },
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Unavailable);
    // An access adapter that does not know the workspace answers as no grant.
    world.access.fail(elsewhere, ErrorCode::NotFound)?;
    let listed = comments_as(&world, &reader, &proposal).await?;
    assert_eq!(shown(&listed), [false, false, true]);
    Ok(())
}

#[tokio::test]
async fn a_citation_of_a_workspace_dispatch_did_not_authorize_is_refused() -> TestResult {
    let world = proposal_world()?;
    let proposal = open(&world, OPENED, vec![create("notes/new.md")?]).await?;
    let error = err_of(
        world
            .service
            .add_comment(
                &commenting(COMMENTING)?,
                comment_request(&proposal, Some(plan_citation(ELSEWHERE)?))?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(world.proposals.comments()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_kept_citation_is_remapped_or_invalidated_when_it_is_read() -> TestResult {
    for (replacement, item_there, remapped) in [
        (Some('b'), true, true),
        (Some('b'), false, false),
        (None, false, false),
    ] {
        let world = proposal_world()?;
        let proposal = open(&world, OPENED, vec![create("notes/new.md")?]).await?;
        world
            .service
            .add_comment(
                &commenting(COMMENTING)?,
                comment_request(&proposal, Some(plan_citation(1)?))?,
            )
            .await?;
        if item_there {
            world
                .world
                .versions
                .put_document(&revision('b')?, plan(&revision('b')?)?)?;
        }
        let purge_id = PurgeId(Uuid::from_u128(9));
        world.world.records.put_mapping(
            revision('a')?,
            RevisionMapping {
                purge_id,
                replacement: replacement.map(revision).transpose()?,
            },
        )?;
        let listed = comments_as(
            &world,
            &alice(OperationName::ListComments, None)?,
            &proposal,
        )
        .await?;
        let comment = some(listed.first(), "the comment")?;
        let source = some(comment.source.as_ref(), "the citation")?;
        if remapped {
            assert_eq!(source.revision, revision('b')?);
            assert_eq!(comment.invalidated_by, None);
        } else {
            assert_eq!(source.revision, revision('a')?);
            assert_eq!(comment.invalidated_by, Some(purge_id), "{replacement:?}");
        }
        // A store fault reading the cited item at the rewritten revision is no "not there".
        if replacement.is_some() {
            world
                .world
                .versions
                .fail_next_show(ErrorCode::Unavailable)?;
            let error = err_of(
                comments_as(
                    &world,
                    &alice(OperationName::ListComments, None)?,
                    &proposal,
                )
                .await,
            )?;
            let error = some(error.downcast_ref::<ApiError>(), "an ApiError")?;
            assert_eq!(error.code, ErrorCode::Unavailable);
        }
    }
    Ok(())
}
