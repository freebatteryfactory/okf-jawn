//! Drafts, confirmations, sandbox capabilities and proposals against the real records database.
//!
//! `confirmation_single_use_*` tests are the receipt of `confirmation-single-use`; the
//! `draft_per_editor_*` tests here cover the store half of `draft-per-editor` (the Snapshot half
//! is in `versions.rs`).
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;

use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::identity::{Digest, Revision, TenantId, Timestamp};
use okf_jawn_contract::proposal::{Comment, Proposal, ProposalStatus};
use okf_jawn_contract::review::{ConfirmationAction, ConfirmationTarget};
use okf_jawn_core::confirmations::{ConfirmationConsume, ConfirmationCreate, ConfirmationStore};
use okf_jawn_core::drafts::{DraftStore, DraftWrite};
use okf_jawn_core::proposals::{ProposalFilter, ProposalStore};
use okf_jawn_core::sandbox::{SandboxCapabilityStore, SandboxMint, token_hash};
use okf_jawn_core::storage::{Page, StorageScope};
use okf_jawn_storage::Storage;
use serde_json::json;

use check::{TestResult, err_of, some};

/// The identity numbered `n`, of the type the context names.
macro_rules! uuid {
    ($n:expr) => {
        serde_json::from_value(json!(uuid_text($n)))
    };
}

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

const FUTURE: &str = "2999-01-01T00:00:00.000Z";

/// A fixed UUID spelling numbered `n`.
fn uuid_text(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

fn scope(workspace: u32) -> Fallible<StorageScope> {
    Ok(StorageScope {
        tenant_id: TenantId::try_from("local".to_owned())?,
        workspace_id: uuid!(workspace)?,
    })
}

fn digest(fill: char) -> Fallible<Digest> {
    Ok(Digest::try_from(fill.to_string().repeat(64))?)
}

fn revision(fill: char) -> Fallible<Revision> {
    Ok(Revision::try_from(fill.to_string().repeat(40))?)
}

fn draft(editor: &str, body: &str) -> Fallible<DraftWrite> {
    Ok(DraftWrite {
        item_id: uuid!(3)?,
        editor: editor.to_owned(),
        base_revision: revision('a')?,
        body: body.to_owned(),
        properties: BTreeMap::from([("title".to_owned(), json!(body))]),
        content_digest: digest('d')?,
    })
}

fn challenge(expires_at: &str) -> Fallible<ConfirmationCreate> {
    Ok(ConfirmationCreate {
        action: ConfirmationAction::Review,
        target: ConfirmationTarget::Item { item_id: uuid!(3)? },
        revision: revision('a')?,
        content_digest: digest('d')?,
        session_id: "session-1".to_owned(),
        subject: "owner".to_owned(),
        expires_at: Timestamp::try_from(expires_at.to_owned())?,
    })
}

fn presented(id: okf_jawn_contract::identity::ConfirmationId) -> Fallible<ConfirmationConsume> {
    Ok(ConfirmationConsume {
        confirmation_id: id,
        action: ConfirmationAction::Review,
        target: ConfirmationTarget::Item { item_id: uuid!(3)? },
        revision: revision('a')?,
        content_digest: digest('d')?,
        session_id: "session-1".to_owned(),
        subject: "owner".to_owned(),
    })
}

fn proposal(id: u32, status: &ProposalStatus) -> Fallible<Proposal> {
    Ok(serde_json::from_value(json!({
        "id": uuid_text(id),
        "workspace_id": uuid_text(1),
        "base_revision": revision('a')?,
        "proposal_revision": revision('b')?,
        "content_digest": digest('d')?,
        "title": format!("proposal {id}"),
        "description": "why",
        "changes": [],
        "kind": "content",
        "status": status,
        "created_by": "agent",
        "created_via": "mcp_delegation",
        "created_at": "2026-10-09T12:00:00.000Z"
    }))?)
}

fn comment(id: &str) -> Fallible<Comment> {
    Ok(serde_json::from_value(json!({
        "id": id,
        "author": "owner",
        "text": "looks right",
        "created_at": "2026-10-09T12:00:00.000Z"
    }))?)
}

#[tokio::test]
async fn draft_per_editor_two_editors_keep_independent_drafts() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let drafts = storage.drafts();
    let workspace = scope(1)?;
    drafts
        .save(&workspace, uuid!(10)?, draft("ana", "mine")?)
        .await?;
    drafts
        .save(&workspace, uuid!(11)?, draft("ben", "his")?)
        .await?;
    let ana = some(
        drafts.get(&workspace, uuid!(3)?, "ana").await?,
        "ana's draft",
    )?;
    let ben = some(
        drafts.get(&workspace, uuid!(3)?, "ben").await?,
        "ben's draft",
    )?;
    assert_eq!(ana.body, "mine");
    assert_eq!(ben.body, "his");
    assert_eq!(drafts.list(&workspace, "ana").await?.len(), 1);
    drafts
        .discard(&workspace, uuid!(12)?, uuid!(3)?, "ana")
        .await?;
    assert!(drafts.get(&workspace, uuid!(3)?, "ana").await?.is_none());
    let kept = some(
        drafts.get(&workspace, uuid!(3)?, "ben").await?,
        "ben's draft",
    )?;
    assert_eq!(
        kept.body, "his",
        "discarding one editor's draft leaves the other's"
    );
    assert!(drafts.get(&scope(2)?, uuid!(3)?, "ben").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn a_repeated_draft_save_or_discard_changes_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let drafts = storage.drafts();
    let workspace = scope(1)?;
    let first = drafts
        .save(&workspace, uuid!(10)?, draft("ana", "one")?)
        .await?;
    drafts
        .save(&workspace, uuid!(11)?, draft("ana", "two")?)
        .await?;
    let replayed = drafts
        .save(&workspace, uuid!(10)?, draft("ana", "three")?)
        .await?;
    assert_eq!(replayed.saved_at, first.saved_at);
    let current = some(drafts.get(&workspace, uuid!(3)?, "ana").await?, "the draft")?;
    assert_eq!(current.body, "two", "a replayed save wrote nothing");
    let removed = drafts
        .discard(&workspace, uuid!(12)?, uuid!(3)?, "ana")
        .await?;
    let again = drafts
        .discard(&workspace, uuid!(12)?, uuid!(3)?, "ana")
        .await?;
    assert_eq!(again.saved_at, removed.saved_at);
    let error = err_of(
        drafts
            .discard(&workspace, uuid!(13)?, uuid!(3)?, "ana")
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound);
    Ok(())
}

#[tokio::test]
async fn confirmation_single_use_is_consumed_by_one_mutation_only() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let confirmations = storage.confirmations();
    let workspace = scope(1)?;
    let issued = confirmations
        .create(&workspace, uuid!(20)?, challenge(FUTURE)?)
        .await?;
    let repeated = confirmations
        .create(&workspace, uuid!(20)?, challenge(FUTURE)?)
        .await?;
    assert_eq!(repeated.id, issued.id, "create is unique on its mutation");
    confirmations
        .consume(&workspace, uuid!(21)?, presented(issued.id)?)
        .await?;
    // The same mutation, resumed after a crash, consumes again.
    confirmations
        .consume(&workspace, uuid!(21)?, presented(issued.id)?)
        .await?;
    let error = err_of(
        confirmations
            .consume(&workspace, uuid!(22)?, presented(issued.id)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict);
    assert!(error.message.contains("already used"), "{}", error.message);
    Ok(())
}

#[tokio::test]
async fn confirmation_single_use_refuses_another_revision_digest_action_or_target() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let confirmations = storage.confirmations();
    let workspace = scope(1)?;
    let issued = confirmations
        .create(&workspace, uuid!(20)?, challenge(FUTURE)?)
        .await?;
    let mut other_revision = presented(issued.id)?;
    other_revision.revision = revision('b')?;
    let mut other_digest = presented(issued.id)?;
    other_digest.content_digest = digest('e')?;
    let mut other_action = presented(issued.id)?;
    other_action.action = ConfirmationAction::AcceptProposal;
    let mut other_target = presented(issued.id)?;
    other_target.target = ConfirmationTarget::Item { item_id: uuid!(4)? };
    for (mutation, wrong) in [other_revision, other_digest, other_action, other_target]
        .into_iter()
        .enumerate()
    {
        let mutation = u32::try_from(mutation)?.saturating_add(30);
        let error = err_of(
            confirmations
                .consume(&workspace, uuid!(mutation)?, wrong)
                .await,
        )?;
        assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    }
    // None of the refused attempts used it up.
    confirmations
        .consume(&workspace, uuid!(40)?, presented(issued.id)?)
        .await?;
    Ok(())
}

#[tokio::test]
async fn confirmation_single_use_is_bound_to_its_session_scope_and_expiry() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let confirmations = storage.confirmations();
    let workspace = scope(1)?;
    let issued = confirmations
        .create(&workspace, uuid!(20)?, challenge(FUTURE)?)
        .await?;
    let mut other_session = presented(issued.id)?;
    other_session.session_id = "session-2".to_owned();
    let error = err_of(
        confirmations
            .consume(&workspace, uuid!(21)?, other_session)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Forbidden);
    let error = err_of(
        confirmations
            .consume(&scope(2)?, uuid!(21)?, presented(issued.id)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound);
    let expired = confirmations
        .create(
            &workspace,
            uuid!(22)?,
            challenge("2020-01-01T00:00:00.000Z")?,
        )
        .await?;
    let error = err_of(
        confirmations
            .consume(&workspace, uuid!(23)?, presented(expired.id)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict);
    assert!(error.message.contains("expired"), "{}", error.message);
    Ok(())
}

#[tokio::test]
async fn a_sandbox_capability_resolves_by_hash_until_it_expires() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let sandbox = storage.sandbox();
    let mint = |expires_at: &str| -> Fallible<SandboxMint> {
        Ok(SandboxMint {
            item_id: uuid!(3)?,
            revision: revision('a')?,
            object: digest('f')?,
            media_type: "text/html".to_owned(),
            expires_at: Timestamp::try_from(expires_at.to_owned())?,
        })
    };
    sandbox
        .mint(&scope(1)?, token_hash("live"), mint(FUTURE)?)
        .await?;
    sandbox
        .mint(
            &scope(1)?,
            token_hash("old"),
            mint("2020-01-01T00:00:00.000Z")?,
        )
        .await?;
    let resolved = some(sandbox.resolve(token_hash("live")).await?, "the binding")?;
    assert_eq!(resolved.scope, scope(1)?);
    assert_eq!(resolved.media_type, "text/html");
    assert!(sandbox.resolve(token_hash("old")).await?.is_none());
    assert!(sandbox.resolve(token_hash("never")).await?.is_none());
    let error = err_of(
        sandbox
            .mint(&scope(1)?, token_hash("live"), mint(FUTURE)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict);
    Ok(())
}

#[tokio::test]
async fn proposals_and_comments_are_unique_on_their_mutation_and_page() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let proposals = storage.proposals();
    let workspace = scope(1)?;
    let first = proposals
        .insert(&workspace, uuid!(50)?, proposal(60, &ProposalStatus::Open)?)
        .await?;
    let again = proposals
        .insert(&workspace, uuid!(50)?, proposal(61, &ProposalStatus::Open)?)
        .await?;
    assert_eq!(again.id, first.id);
    proposals
        .insert(&workspace, uuid!(51)?, proposal(62, &ProposalStatus::Open)?)
        .await?;
    let page = proposals
        .list(
            &workspace,
            ProposalFilter {
                status: None,
                page: Page {
                    cursor: None,
                    limit: 1,
                },
            },
        )
        .await?;
    assert_eq!(page.items.len(), 1);
    assert_eq!(some(page.items.first(), "the newest")?.id, uuid!(62)?);
    let rest = proposals
        .list(
            &workspace,
            ProposalFilter {
                status: None,
                page: Page {
                    cursor: page.next_cursor,
                    limit: 10,
                },
            },
        )
        .await?;
    assert_eq!(rest.items.len(), 1);
    assert_eq!(rest.next_cursor, None);
    let mut declined = first.clone();
    declined.status = ProposalStatus::Declined;
    proposals.update(&workspace, declined).await?;
    let open = proposals
        .list(
            &workspace,
            ProposalFilter {
                status: Some(ProposalStatus::Open),
                page: Page {
                    cursor: None,
                    limit: 10,
                },
            },
        )
        .await?;
    assert_eq!(open.items.len(), 1);
    let said = proposals
        .add_comment(&workspace, uuid!(52)?, first.id, comment("c1")?)
        .await?;
    let replayed = proposals
        .add_comment(&workspace, uuid!(52)?, first.id, comment("c2")?)
        .await?;
    assert_eq!(replayed.id, said.id);
    let comments = proposals
        .list_comments(
            &workspace,
            first.id,
            Page {
                cursor: None,
                limit: 10,
            },
        )
        .await?;
    assert_eq!(comments.items.len(), 1);
    let error = err_of(proposals.get(&scope(2)?, first.id).await)?;
    assert_eq!(error.code, ErrorCode::NotFound);
    let error = err_of(
        proposals
            .add_comment(&workspace, uuid!(53)?, uuid!(99)?, comment("c3")?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
