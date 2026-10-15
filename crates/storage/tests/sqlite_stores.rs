//! Drafts, confirmations, sandbox capabilities and proposals against the real records database.
//!
//! `confirmation_single_use_*` tests are the receipt of `confirmation-single-use`; the
//! `draft_per_editor_*` tests here cover the store half of `draft-per-editor` (the Snapshot half
//! is in `versions.rs`).
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;

use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::identity::{Digest, ItemId, Revision, TenantId, Timestamp};
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
        .discard(&workspace, uuid!(12)?, uuid!(3)?, "ana", None)
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
        .discard(&workspace, uuid!(12)?, uuid!(3)?, "ana", None)
        .await?;
    let again = drafts
        .discard(&workspace, uuid!(12)?, uuid!(3)?, "ana", None)
        .await?;
    assert_eq!(again.saved_at, removed.saved_at);
    let error = err_of(
        drafts
            .discard(&workspace, uuid!(13)?, uuid!(3)?, "ana", None)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound);
    Ok(())
}

/// How many rows the records database holds under `mutation` in `draft_mutations`.
fn draft_mutation_rows(storage: &Storage, mutation: u32) -> Fallible<i64> {
    let connection = rusqlite::Connection::open(storage.data().records_path())?;
    Ok(connection.query_row(
        "SELECT COUNT(*) FROM draft_mutations WHERE mutation_id = ?1",
        [uuid_text(mutation)],
        |row| row.get(0),
    )?)
}

#[tokio::test]
async fn a_discard_expecting_the_held_digest_removes_the_draft() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let drafts = storage.drafts();
    let workspace = scope(1)?;
    drafts
        .save(&workspace, uuid!(10)?, draft("ana", "mine")?)
        .await?;
    let removed = drafts
        .discard(&workspace, uuid!(12)?, uuid!(3)?, "ana", Some(digest('d')?))
        .await?;
    assert_eq!(removed.content_digest, digest('d')?);
    assert!(drafts.get(&workspace, uuid!(3)?, "ana").await?.is_none());
    assert_eq!(draft_mutation_rows(&storage, 12)?, 1);
    Ok(())
}

#[tokio::test]
async fn a_discard_expecting_another_digest_conflicts_keeps_the_draft_and_records_nothing()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let drafts = storage.drafts();
    let workspace = scope(1)?;
    drafts
        .save(&workspace, uuid!(10)?, draft("ana", "saved again")?)
        .await?;
    let error = err_of(
        drafts
            .discard(&workspace, uuid!(12)?, uuid!(3)?, "ana", Some(digest('e')?))
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    let kept = some(drafts.get(&workspace, uuid!(3)?, "ana").await?, "the draft")?;
    assert_eq!(
        kept.body, "saved again",
        "a refused removal deletes nothing"
    );
    assert_eq!(
        draft_mutation_rows(&storage, 12)?,
        0,
        "a refused removal records nothing under its mutation"
    );
    let removed = drafts
        .discard(&workspace, uuid!(12)?, uuid!(3)?, "ana", None)
        .await?;
    assert_eq!(removed.content_digest, digest('d')?);
    assert!(
        drafts.get(&workspace, uuid!(3)?, "ana").await?.is_none(),
        "the same mutation may still remove the draft once nothing refuses it"
    );
    Ok(())
}

#[tokio::test]
async fn a_replayed_discard_ignores_the_expected_digest() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let drafts = storage.drafts();
    let workspace = scope(1)?;
    drafts
        .save(&workspace, uuid!(10)?, draft("ana", "first")?)
        .await?;
    let removed = drafts
        .discard(&workspace, uuid!(12)?, uuid!(3)?, "ana", Some(digest('d')?))
        .await?;
    let gone = drafts
        .discard(&workspace, uuid!(12)?, uuid!(3)?, "ana", Some(digest('f')?))
        .await?;
    assert_eq!(
        gone.saved_at, removed.saved_at,
        "a replay with no draft held"
    );
    drafts
        .save(
            &workspace,
            uuid!(11)?,
            DraftWrite {
                content_digest: digest('e')?,
                ..draft("ana", "saved after the removal")?
            },
        )
        .await?;
    let held = drafts
        .discard(&workspace, uuid!(12)?, uuid!(3)?, "ana", Some(digest('f')?))
        .await?;
    assert_eq!(
        held.saved_at, removed.saved_at,
        "a replay with a draft held"
    );
    assert_eq!(held.content_digest, digest('d')?);
    let kept = some(
        drafts.get(&workspace, uuid!(3)?, "ana").await?,
        "the newer draft",
    )?;
    assert_eq!(
        kept.body, "saved after the removal",
        "a replay removes nothing"
    );
    assert_eq!(draft_mutation_rows(&storage, 12)?, 1);
    Ok(())
}

/// A save racing a conditional discard never loses the newer draft. Either the discard ran
/// first (it removes the old draft and the save lands after it) or the save ran first (the
/// discard answers `Conflict`); either way the newer draft is held afterwards. The spawn order
/// alternates so both interleavings occur over the rounds. (Reviewer probe, adopted.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_save_racing_a_conditional_discard_is_never_lost() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let workspace = scope(1)?;
    let item: ItemId = uuid!(3)?;
    for round in 0..300_u32 {
        let base = round.saturating_mul(3).saturating_add(1000);
        storage
            .drafts()
            .save(&workspace, uuid!(base)?, draft("ana", "old")?)
            .await?;
        let saving = storage.drafts();
        let discarder = storage.drafts();
        let (save_scope, discard_scope) = (workspace.clone(), workspace.clone());
        let save_id = uuid!(base.saturating_add(1))?;
        let discard_id = uuid!(base.saturating_add(2))?;
        let newer = DraftWrite {
            content_digest: digest('e')?,
            ..draft("ana", "new")?
        };
        let expected = digest('d')?;
        let save_task = async move { saving.save(&save_scope, save_id, newer).await };
        let discard_task = async move {
            discarder
                .discard(&discard_scope, discard_id, item, "ana", Some(expected))
                .await
        };
        let (save, discard) = if round % 2 == 0 {
            let discard = tokio::spawn(discard_task);
            (tokio::spawn(save_task), discard)
        } else {
            let save = tokio::spawn(save_task);
            (save, tokio::spawn(discard_task))
        };
        let (save_outcome, removed) = tokio::join!(save, discard);
        save_outcome??;
        match removed? {
            Ok(removed) => assert_eq!(removed.content_digest, digest('d')?, "round {round}"),
            Err(error) => assert_eq!(error.code, ErrorCode::Conflict, "round {round}: {error}"),
        }
        let held = some(
            storage.drafts().get(&workspace, item, "ana").await?,
            "the newer draft",
        )?;
        assert_eq!(held.draft.content_digest, digest('e')?, "round {round}");
        assert_eq!(held.body, "new", "round {round}");
        storage
            .drafts()
            .discard(
                &workspace,
                uuid!(base.saturating_add(900_000))?,
                item,
                "ana",
                None,
            )
            .await?;
    }
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

/// An expired confirmation is refused even to the mutation that consumed it, as
/// `ConfirmationStore::consume`'s doc says (review probe P3): before its expiry that mutation
/// consumes it again; after it, the same mutation is refused `Conflict` "expired", and another
/// mutation `Conflict` "already used".
#[tokio::test]
async fn confirmation_single_use_expires_even_for_the_mutation_that_consumed_it() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let confirmations = storage.confirmations();
    let workspace = scope(1)?;
    let issued = confirmations
        .create(&workspace, uuid!(20)?, challenge(FUTURE)?)
        .await?;
    for _ in 0..2 {
        confirmations
            .consume(&workspace, uuid!(21)?, presented(issued.id)?)
            .await?;
    }
    // Its expiry passes: the stored expiry is written directly, as only time would move it.
    let changed = rusqlite::Connection::open(storage.data().records_path())?.execute(
        "UPDATE confirmations SET expires_at = ?1 WHERE confirmation_id = ?2",
        [
            "2020-01-01T00:00:00.000Z".to_owned(),
            issued.id.0.to_string(),
        ],
    )?;
    assert_eq!(changed, 1);
    let error = err_of(
        confirmations
            .consume(&workspace, uuid!(21)?, presented(issued.id)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
    assert!(error.message.contains("expired"), "{}", error.message);
    let error = err_of(
        confirmations
            .consume(&workspace, uuid!(22)?, presented(issued.id)?)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{}", error.message);
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
    proposals
        .update(&workspace, uuid!(54)?, declined, ProposalStatus::Open)
        .await?;
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

/// The stored status of proposal `id` in `workspace`.
async fn stored_status(
    storage: &Storage,
    workspace: &StorageScope,
    id: u32,
) -> Fallible<ProposalStatus> {
    Ok(storage.proposals().get(workspace, uuid!(id)?).await?.status)
}

/// `ProposalStore::update` is a compare-and-set on the stored status: written only from the
/// expected status, refused with `Conflict` and nothing written otherwise, a no-op when the
/// mutation that wrote the stored record writes it again, and `NotFound` for a proposal the
/// workspace does not hold.
#[tokio::test]
async fn a_proposal_status_change_is_written_only_from_the_expected_status() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let proposals = storage.proposals();
    let workspace = scope(1)?;
    proposals
        .insert(&workspace, uuid!(70)?, proposal(71, &ProposalStatus::Open)?)
        .await?;
    let declined = proposals
        .update(
            &workspace,
            uuid!(72)?,
            proposal(71, &ProposalStatus::Declined)?,
            ProposalStatus::Open,
        )
        .await?;
    assert_eq!(declined.status, ProposalStatus::Declined);
    // From `open` again: the stored status is `declined`, so nothing is written.
    let error = err_of(
        proposals
            .update(
                &workspace,
                uuid!(73)?,
                proposal(71, &ProposalStatus::Accepted)?,
                ProposalStatus::Open,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert_eq!(
        stored_status(&storage, &workspace, 71).await?,
        ProposalStatus::Declined
    );
    // A retry of the decline: the stored record again under its mutation, accepted whatever
    // was expected.
    let again = proposals
        .update(
            &workspace,
            uuid!(72)?,
            proposal(71, &ProposalStatus::Declined)?,
            ProposalStatus::Open,
        )
        .await?;
    assert_eq!(again.status, ProposalStatus::Declined);
    // The same status with another field is a change, and the status is not the expected one.
    let mut retitled = proposal(71, &ProposalStatus::Declined)?;
    retitled.title = "another title".to_owned();
    let error = err_of(
        proposals
            .update(
                &workspace,
                uuid!(72)?,
                retitled.clone(),
                ProposalStatus::Open,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert_eq!(
        proposals.get(&workspace, uuid!(71)?).await?.title,
        "proposal 71"
    );
    // From the stored status it is written.
    let written = proposals
        .update(&workspace, uuid!(74)?, retitled, ProposalStatus::Declined)
        .await?;
    assert_eq!(written.title, "another title");
    assert_eq!(
        proposals.get(&workspace, uuid!(71)?).await?.title,
        "another title"
    );
    let error = err_of(
        proposals
            .update(
                &scope(2)?,
                uuid!(75)?,
                proposal(71, &ProposalStatus::Accepted)?,
                ProposalStatus::Declined,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound, "{error}");
    Ok(())
}

/// A proposal record shaped as `open_proposal` stores one (review probe P2): an edit whose
/// properties nest unsorted keys, non-ASCII and escaped text, floats, `-0.0`, nulls and `1e21`;
/// a create; and kept warnings.
fn rich_proposal(id: u32, status: &ProposalStatus) -> Fallible<Proposal> {
    let mut rich = proposal(id, status)?;
    rich.changes = serde_json::from_value(json!([
        {
            "kind": "edit",
            "item_id": uuid_text(3),
            "body": "Zürich: \"quoted\"\n\ttab \\ backslash \u{1F642}",
            "properties": {
                "zeta": { "b": 1.5, "a": [null, -0.0, 1e21, "ü"] },
                "alpha": null,
                "title": "Ünïcode \u{2028} line"
            }
        },
        {
            "kind": "create",
            "path": "notes/new.md",
            "type_name": "Note",
            "body": "# New\n",
            "properties": { "title": "New", "description": "A new note." }
        }
    ]))?;
    rich.warnings = serde_json::from_value(json!([
        {
            "code": "okf_validator_info",
            "message": "link target does not resolve to a concept in the bundle: gone.md",
            "location": "notes/new.md"
        }
    ]))?;
    Ok(rich)
}

/// Review N3 (fix round 1, F4): two declines of one open proposal by different mutations write
/// the same record, and the second is refused, because only the mutation that wrote a change
/// may write it again as a retry; a retry of the first answers its result. The record is shaped
/// as `open_proposal` stores one, so the retry is told by the stored text of a real record.
#[tokio::test]
async fn of_two_identical_status_changes_by_different_mutations_the_second_is_refused() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let proposals = storage.proposals();
    let workspace = scope(1)?;
    let open = proposals
        .insert(
            &workspace,
            uuid!(80)?,
            rich_proposal(81, &ProposalStatus::Open)?,
        )
        .await?;
    // Both declines read the proposal open, and each writes it declined.
    let declined = Proposal {
        status: ProposalStatus::Declined,
        ..open
    };
    let first = proposals
        .update(
            &workspace,
            uuid!(82)?,
            declined.clone(),
            ProposalStatus::Open,
        )
        .await?;
    let error = err_of(
        proposals
            .update(
                &workspace,
                uuid!(83)?,
                declined.clone(),
                ProposalStatus::Open,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    // The first decline, retried under its mutation, answers its result.
    let again = proposals
        .update(
            &workspace,
            uuid!(82)?,
            declined.clone(),
            ProposalStatus::Open,
        )
        .await?;
    assert_eq!(serde_json::to_value(&again)?, serde_json::to_value(&first)?);
    // The second, retried, is refused again; the stored record is the first decline's.
    let error = err_of(
        proposals
            .update(
                &workspace,
                uuid!(83)?,
                declined.clone(),
                ProposalStatus::Open,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert_eq!(
        serde_json::to_value(proposals.get(&workspace, uuid!(81)?).await?)?,
        serde_json::to_value(&declined)?
    );
    Ok(())
}

/// An accept and a decline of one open proposal: whichever is written first, the other is
/// refused with `Conflict` and the stored status is the first one's.
///
/// Storage's records database is one connection behind a mutex, and each update runs in its own
/// IMMEDIATE transaction, so two updates are serialized, never truly concurrent: what this pins
/// is a compare-and-set that holds in either order, not a race inside a transaction. Each order
/// is driven explicitly first. Then 300 rounds spawn the two updates at once and assert that
/// each round wrote exactly one; which one comes first there is the store's choice, which the
/// test neither controls nor counts on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_status_changes_racing_from_open_write_one_and_refuse_the_other() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let workspace = scope(1)?;
    for (id, first, second) in [
        (1990_u32, ProposalStatus::Accepted, ProposalStatus::Declined),
        (1991_u32, ProposalStatus::Declined, ProposalStatus::Accepted),
    ] {
        let proposals = storage.proposals();
        proposals
            .insert(&workspace, uuid!(id)?, proposal(id, &ProposalStatus::Open)?)
            .await?;
        proposals
            .update(
                &workspace,
                uuid!(id.saturating_add(1000))?,
                proposal(id, &first)?,
                ProposalStatus::Open,
            )
            .await?;
        let error = err_of(
            proposals
                .update(
                    &workspace,
                    uuid!(id.saturating_add(2000))?,
                    proposal(id, &second)?,
                    ProposalStatus::Open,
                )
                .await,
        )?;
        assert_eq!(error.code, ErrorCode::Conflict, "{first:?} first: {error}");
        assert_eq!(stored_status(&storage, &workspace, id).await?, first);
    }
    let (mut accepted_wins, mut declined_wins) = (0_u32, 0_u32);
    for round in 0..300_u32 {
        let id = round.saturating_add(2000);
        storage
            .proposals()
            .insert(&workspace, uuid!(id)?, proposal(id, &ProposalStatus::Open)?)
            .await?;
        let (to_accept, to_decline) = (storage.proposals(), storage.proposals());
        let (accept_scope, decline_scope) = (workspace.clone(), workspace.clone());
        let accepted = proposal(id, &ProposalStatus::Accepted)?;
        let declined = proposal(id, &ProposalStatus::Declined)?;
        let (accepting, declining) = (
            uuid!(id.saturating_add(1000))?,
            uuid!(id.saturating_add(2000))?,
        );
        let accept = async move {
            to_accept
                .update(&accept_scope, accepting, accepted, ProposalStatus::Open)
                .await
        };
        let decline = async move {
            to_decline
                .update(&decline_scope, declining, declined, ProposalStatus::Open)
                .await
        };
        let (accept, decline) = if round % 2 == 0 {
            let decline = tokio::spawn(decline);
            (tokio::spawn(accept), decline)
        } else {
            let accept = tokio::spawn(accept);
            (accept, tokio::spawn(decline))
        };
        let (accept, decline) = tokio::join!(accept, decline);
        let winner = match (accept?, decline?) {
            (Ok(_), Err(error)) => {
                assert_eq!(error.code, ErrorCode::Conflict, "round {round}: {error}");
                accepted_wins = accepted_wins.saturating_add(1);
                ProposalStatus::Accepted
            }
            (Err(error), Ok(_)) => {
                assert_eq!(error.code, ErrorCode::Conflict, "round {round}: {error}");
                declined_wins = declined_wins.saturating_add(1);
                ProposalStatus::Declined
            }
            (accept, decline) => {
                return Err(format!(
                    "round {round}: exactly one change must win: {accept:?} and {decline:?}"
                )
                .into());
            }
        };
        assert_eq!(
            stored_status(&storage, &workspace, id).await?,
            winner,
            "round {round}"
        );
    }
    assert_eq!(accepted_wins.saturating_add(declined_wins), 300);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
