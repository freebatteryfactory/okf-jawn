//! Folders, items and types: reads resolve once and carry the server's digest, `get_item` adds
//! only the caller's own draft, and every write commits on its base with the conformance
//! check, then indexes, announces and (for a `MutationResult`) records a receipt.

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::PageRequest;
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::events::{EventKind, ReceiptAudience};
use okf_jawn_contract::extraction::{
    ExtractionFilter, ExtractionStatus, ExtractionSummary, TextOrigin,
};
use okf_jawn_contract::identity::{At, IdempotencyKey, ItemId, PurgeId};
use okf_jawn_contract::item::{
    APP_HEADER_KEY, CreateFolderRequest, CreateItemRequest, DeleteItemRequest, GetItemRequest,
    ItemKind, ItemStatus, ListItemsRequest, ListTypesRequest, MoveItemRequest, SetLifecycleRequest,
    SetTypeRequest, TypeDefinition,
};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::events::EventScope;
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::{FolderListing, Provenance, TreeEdit, derive_item_id};
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    ALICE, Built, NOTE, World, alice, context, draft_of, mutation, note, path, revision, scope,
    served_digest,
};

/// Make one port call of a write fail.
type InjectFailure = fn(&World);

fn key() -> IdempotencyKey {
    IdempotencyKey(Uuid::from_u128(77))
}

fn listed(
    id: u128,
    status: Option<ExtractionStatus>,
) -> Built<okf_jawn_contract::item::ItemSummary> {
    let mut summary = note(
        id,
        &revision('a')?,
        &format!("notes/{id}.md"),
        "",
        json!({}),
    )?
    .summary;
    if let Some(status) = status {
        summary.kind = ItemKind::Source;
        summary.extraction = Some(ExtractionSummary {
            status,
            text_origin: TextOrigin::None,
            corrected: false,
        });
    }
    Ok(summary)
}

fn create_request(kind: ItemKind, properties: serde_json::Value) -> Built<CreateItemRequest> {
    Ok(CreateItemRequest {
        workspace_id: scope()?.workspace_id,
        base_revision: revision('a')?,
        path: path("notes/new.md")?,
        title: "New".to_owned(),
        type_name: "Note".to_owned(),
        kind,
        body: "# New\n".to_owned(),
        properties: serde_json::from_value(properties)?,
        idempotency_key: key(),
    })
}

fn move_request() -> Built<MoveItemRequest> {
    Ok(MoveItemRequest {
        workspace_id: scope()?.workspace_id,
        item_id: ItemId(Uuid::from_u128(NOTE)),
        base_revision: revision('a')?,
        destination: path("archive/plan.md")?,
        idempotency_key: key(),
    })
}

/// Put the fixture note at `notes/plan.md` at each revision named by `fills`, as an edit of it
/// reads its path at the base.
fn note_at(world: &World, fills: &[char]) -> Built<()> {
    for fill in fills {
        let at = revision(*fill)?;
        world.versions.put_document(
            &at,
            note(
                NOTE,
                &at,
                "notes/plan.md",
                "# Plan\n",
                json!({ "type": "Note" }),
            )?,
        )?;
    }
    Ok(())
}
#[tokio::test]
async fn list_items_lists_one_folder_at_the_head_resolved_once() -> TestResult {
    let world = World::new()?;
    world.versions.put_listing(
        &revision('a')?,
        Some(path("notes")?),
        FolderListing {
            items: vec![
                listed(1, None)?,
                listed(2, Some(ExtractionStatus::Completed))?,
            ],
            folders: vec![path("notes/old")?],
            next_cursor: Some("next".to_owned()),
        },
    )?;
    let response = world
        .service
        .list_items(
            &alice(OperationName::ListItems, None)?,
            ListItemsRequest {
                workspace_id: scope()?.workspace_id,
                at: At::Latest,
                folder: "notes".to_owned(),
                extraction: None,
                page: PageRequest {
                    cursor: None,
                    limit: 50,
                },
            },
        )
        .await?;
    assert_eq!(response.revision, revision('a')?);
    assert_eq!(response.items.len(), 2);
    assert_eq!(response.folders, vec!["notes/old".to_owned()]);
    assert_eq!(response.next_cursor.as_deref(), Some("next"));
    Ok(())
}

#[tokio::test]
async fn list_items_hides_archived_items_at_the_current_head_and_keeps_them_in_history()
-> TestResult {
    let world = World::new()?;
    let mut archived = listed(2, None)?;
    archived.archived = true;
    world.versions.put_listing(
        &revision('a')?,
        Some(path("notes")?),
        FolderListing {
            items: vec![listed(1, None)?, archived],
            folders: Vec::new(),
            next_cursor: None,
        },
    )?;
    let caller = alice(OperationName::ListItems, None)?;
    let request = |at: At| -> Built<ListItemsRequest> {
        Ok(ListItemsRequest {
            workspace_id: scope()?.workspace_id,
            at,
            folder: "notes".to_owned(),
            extraction: None,
            page: PageRequest {
                cursor: None,
                limit: 50,
            },
        })
    };
    let listed_ids = |items: &[okf_jawn_contract::item::ItemSummary]| -> Vec<(ItemId, bool)> {
        items.iter().map(|item| (item.id, item.archived)).collect()
    };
    let live = ItemId(Uuid::from_u128(1));
    let shelved = ItemId(Uuid::from_u128(2));
    // `a` is the head: latest, and `a` named, are current listings.
    for at in [
        At::Latest,
        At::Revision {
            revision: revision('a')?,
        },
    ] {
        let current = world.service.list_items(&caller, request(at)?).await?;
        assert_eq!(listed_ids(&current.items), vec![(live, false)]);
    }
    // Once the head has moved on, `a` is history and keeps the archived item, flagged.
    world.versions.set_head(revision('b')?)?;
    let history = world
        .service
        .list_items(
            &caller,
            request(At::Revision {
                revision: revision('a')?,
            })?,
        )
        .await?;
    assert_eq!(
        listed_ids(&history.items),
        vec![(live, false), (shelved, true)]
    );
    Ok(())
}

#[tokio::test]
async fn list_items_unprocessed_lists_only_partial_failed_or_unsupported_sources() -> TestResult {
    let world = World::new()?;
    world.versions.put_listing(
        &revision('a')?,
        None,
        FolderListing {
            items: vec![
                listed(1, None)?,
                listed(2, Some(ExtractionStatus::Completed))?,
                listed(3, Some(ExtractionStatus::Partial))?,
                listed(4, Some(ExtractionStatus::Failed))?,
                listed(5, Some(ExtractionStatus::Unsupported))?,
                listed(6, Some(ExtractionStatus::Pending))?,
            ],
            folders: vec![path("notes")?],
            next_cursor: None,
        },
    )?;
    let response = world
        .service
        .list_items(
            &alice(OperationName::ListItems, None)?,
            ListItemsRequest {
                workspace_id: scope()?.workspace_id,
                at: At::Latest,
                folder: String::new(),
                extraction: Some(ExtractionFilter::Unprocessed),
                page: PageRequest {
                    cursor: None,
                    limit: 50,
                },
            },
        )
        .await?;
    let ids: Vec<ItemId> = response.items.iter().map(|item| item.id).collect();
    assert_eq!(
        ids,
        vec![
            ItemId(Uuid::from_u128(3)),
            ItemId(Uuid::from_u128(4)),
            ItemId(Uuid::from_u128(5))
        ]
    );
    assert_eq!(response.folders, vec!["notes".to_owned()]);
    Ok(())
}

#[tokio::test]
async fn list_items_refuses_a_folder_that_is_not_a_workspace_path() -> TestResult {
    let world = World::new()?;
    let refused = err_of(
        world
            .service
            .list_items(
                &alice(OperationName::ListItems, None)?,
                ListItemsRequest {
                    workspace_id: scope()?.workspace_id,
                    at: At::Latest,
                    folder: "../escape".to_owned(),
                    extraction: None,
                    page: PageRequest {
                        cursor: None,
                        limit: 50,
                    },
                },
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/folder"));
    Ok(())
}

#[tokio::test]
async fn get_item_answers_the_servers_content_digest() -> TestResult {
    let world = World::new()?;
    let stored = note(
        NOTE,
        &revision('a')?,
        "notes/plan.md",
        "# Plan\n",
        json!({ "type": "Note" }),
    )?;
    world
        .versions
        .put_document(&revision('a')?, stored.clone())?;
    let document = world
        .service
        .get_item(
            &alice(OperationName::GetItem, None)?,
            GetItemRequest {
                workspace_id: scope()?.workspace_id,
                item_id: stored.summary.id,
                at: At::Latest,
            },
        )
        .await?;
    assert_eq!(document.content_digest, served_digest(&stored)?);
    assert_ne!(document.content_digest, stored.content_digest);
    Ok(())
}

#[tokio::test]
async fn draft_visibility_get_item_returns_only_the_callers_own_draft() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.versions.put_document(
        &at,
        note(
            NOTE,
            &at,
            "notes/plan.md",
            "# Plan\n",
            json!({ "type": "Note" }),
        )?,
    )?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &at, "alice's words")?)?;
    world
        .drafts
        .put_draft(draft_of(NOTE, "bob", &at, "bob's words")?)?;
    let request = || -> Built<GetItemRequest> {
        Ok(GetItemRequest {
            workspace_id: scope()?.workspace_id,
            item_id: ItemId(Uuid::from_u128(NOTE)),
            at: At::Latest,
        })
    };
    for (subject, expected) in [
        (ALICE, Some("alice's words")),
        ("bob", Some("bob's words")),
        ("carol", None),
    ] {
        let caller = context(
            OperationName::GetItem,
            subject,
            AccessRoute::BrowserSession,
            None,
        )?;
        let document = world.service.get_item(&caller, request()?).await?;
        assert_eq!(
            document.draft.as_ref().map(|draft| draft.body.as_str()),
            expected,
            "{subject}"
        );
        assert_eq!(document.body, "# Plan\n");
    }
    Ok(())
}

#[tokio::test]
async fn a_purged_revision_is_refused_before_anything_is_read_at_it() -> TestResult {
    let world = World::new()?;
    let purged = revision('c')?;
    let purge_id = PurgeId(Uuid::from_u128(5));
    world.records.put_mapping(
        purged.clone(),
        RevisionMapping {
            purge_id,
            replacement: Some(revision('d')?),
        },
    )?;
    let refused = err_of(
        world
            .service
            .get_item(
                &alice(OperationName::GetItem, None)?,
                GetItemRequest {
                    workspace_id: scope()?.workspace_id,
                    item_id: ItemId(Uuid::from_u128(NOTE)),
                    at: At::Revision { revision: purged },
                },
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(
        refused.detail.as_deref(),
        Some(&ErrorDetail::Invalidated {
            purge_id,
            replacement: Some(revision('d')?)
        })
    );
    assert_eq!(world.versions.shows(), 0);
    Ok(())
}

#[tokio::test]
async fn create_item_commits_a_note_on_its_base_and_returns_it_as_committed() -> TestResult {
    let world = World::new()?;
    let id = mutation(1);
    let item_id = derive_item_id(id, 0);
    let committed = note(
        1,
        &revision('b')?,
        "notes/new.md",
        "# New\n",
        json!({ "type": "Note" }),
    )?;
    let mut committed = committed;
    committed.summary.id = item_id;
    world.versions.queue_revision(revision('b')?)?;
    world
        .versions
        .put_document(&revision('b')?, committed.clone())?;
    let caller = alice(OperationName::CreateItem, Some(id))?;
    let document = world
        .service
        .create_item(
            &caller,
            create_request(ItemKind::Note, json!({ "type": "Note" }))?,
        )
        .await?;
    assert_eq!(document.summary.id, item_id);
    assert_eq!(document.content_digest, served_digest(&committed)?);
    let commits = world.versions.commits()?;
    let commit = some(commits.first(), "the commit")?;
    assert_eq!(commits.len(), 1);
    assert_eq!(commit.mutation_id, id);
    assert_eq!(commit.expected_head, revision('a')?);
    assert_eq!(commit.author, Provenance::from_principal(&caller.principal));
    assert!(matches!(
        commit.edits.as_slice(),
        [TreeEdit::CreateItem { item_id: created, path: at, title: Some(title), kind: ItemKind::Note, .. }]
            if *created == item_id && at.as_str() == "notes/new.md" && title == "New"
    ));
    assert_eq!(world.search.indexed()?, vec![revision('b')?]);
    let events = world.events.appended()?;
    let (log, under, event) = some(events.first(), "the changed event")?;
    assert_eq!(log, &EventScope::Workspace(scope()?));
    assert_eq!(*under, Some(id));
    assert_eq!(event.kind, EventKind::Changed);
    assert_eq!(event.revision, Some(revision('b')?));
    assert_eq!(event.item_id, Some(item_id));
    Ok(())
}

#[tokio::test]
async fn create_item_refuses_a_source_a_view_and_a_supplied_header_before_any_commit() -> TestResult
{
    let world = World::new()?;
    let caller = alice(OperationName::CreateItem, Some(mutation(1)))?;
    let source = err_of(
        world
            .service
            .create_item(&caller, create_request(ItemKind::Source, json!({}))?)
            .await,
    )?;
    assert_eq!(source.code, ErrorCode::InvalidInput);
    assert_eq!(source.field.as_deref(), Some("/kind"));
    let view = err_of(
        world
            .service
            .create_item(&caller, create_request(ItemKind::View, json!({}))?)
            .await,
    )?;
    assert_eq!(view.code, ErrorCode::NotImplemented);
    let header = err_of(
        world
            .service
            .create_item(
                &caller,
                create_request(
                    ItemKind::Note,
                    json!({ APP_HEADER_KEY: { "item_id": "x" } }),
                )?,
            )
            .await,
    )?;
    assert_eq!(header.code, ErrorCode::InvalidInput);
    assert_eq!(header.field.as_deref(), Some("/properties/okf_jawn"));
    assert!(world.versions.commits()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_write_is_judged_by_the_conformance_check_and_a_refusal_commits_nothing() -> TestResult {
    let world = World::new()?;
    note_at(&world, &['a'])?;
    world.versions.queue_revision(revision('b')?)?;
    world
        .versions
        .stage()
        .write("notes/untyped.md", "---\ntitle: No type\n---\nBody\n")?;
    let refused = err_of(
        world
            .service
            .move_item(
                &alice(OperationName::MoveItem, Some(mutation(1)))?,
                move_request()?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert!(
        refused.message.contains("notes/untyped.md"),
        "{}",
        refused.message
    );
    assert!(world.versions.commits()?.is_empty());
    assert_eq!(world.search.indexed()?.len(), 0);
    assert_eq!(world.events.appended()?.len(), 0);
    assert_eq!(world.records.receipts()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn an_edit_reports_lint_only_for_the_files_it_touched() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    // The moved item lives at `notes/bare.md`; `notes/other.md` is untouched. Both lack a
    // heading, which okf-validator's lint flags.
    world.versions.put_document(
        &at,
        note(
            NOTE,
            &at,
            "notes/bare.md",
            "No heading.\n",
            json!({ "type": "Note" }),
        )?,
    )?;
    world.versions.queue_revision(revision('b')?)?;
    let stage = world.versions.stage();
    stage.write("notes/bare.md", "---\ntype: Note\n---\n\nNo heading.\n")?;
    stage.write("notes/other.md", "---\ntype: Note\n---\n\nNo heading.\n")?;
    let moved = world
        .service
        .move_item(
            &alice(OperationName::MoveItem, Some(mutation(1)))?,
            move_request()?,
        )
        .await?;
    let lint: Vec<_> = moved
        .warnings
        .iter()
        .filter(|warning| warning.code.starts_with("okf_lint"))
        .collect();
    assert!(
        lint.iter()
            .any(|warning| warning.location.as_deref() == Some("notes/bare.md")),
        "{lint:?}"
    );
    assert!(
        lint.iter().all(|warning| matches!(
            warning.location.as_deref(),
            Some("notes/bare.md" | "archive/plan.md")
        )),
        "{lint:?}"
    );
    assert_eq!(world.versions.commits()?.len(), 1);
    Ok(())
}
#[tokio::test]
async fn a_write_on_a_moved_head_conflicts_and_one_on_a_purged_base_is_refused() -> TestResult {
    let world = World::new()?;
    note_at(&world, &['a'])?;
    world.versions.queue_revision(revision('b')?)?;
    world.versions.set_head(revision('c')?)?;
    let conflict = err_of(
        world
            .service
            .move_item(
                &alice(OperationName::MoveItem, Some(mutation(1)))?,
                move_request()?,
            )
            .await,
    )?;
    assert_eq!(conflict.code, ErrorCode::Conflict);
    world.records.put_mapping(
        revision('a')?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(5)),
            replacement: None,
        },
    )?;
    world.versions.set_head(revision('a')?)?;
    let purged = err_of(
        world
            .service
            .move_item(
                &alice(OperationName::MoveItem, Some(mutation(2)))?,
                move_request()?,
            )
            .await,
    )?;
    assert_eq!(purged.code, ErrorCode::NotFound);
    assert!(world.versions.commits()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn item_writes_commit_their_edit_and_record_a_receipt() -> TestResult {
    let world = World::new()?;
    note_at(&world, &['a', 'b', 'c'])?;
    let workspace_id = scope()?.workspace_id;
    let item_id = ItemId(Uuid::from_u128(NOTE));
    for fill in ['b', 'c', 'd', 'e'] {
        world.versions.queue_revision(revision(fill)?)?;
    }
    let moved = world
        .service
        .move_item(
            &alice(OperationName::MoveItem, Some(mutation(1)))?,
            move_request()?,
        )
        .await?;
    let lifecycle = world
        .service
        .set_lifecycle(
            &alice(OperationName::SetLifecycle, Some(mutation(2)))?,
            SetLifecycleRequest {
                workspace_id,
                item_id,
                base_revision: revision('b')?,
                status: Some(ItemStatus::Deprecated),
                archived: Some(true),
                idempotency_key: key(),
            },
        )
        .await?;
    let deleted = world
        .service
        .delete_item(
            &alice(OperationName::DeleteItem, Some(mutation(3)))?,
            DeleteItemRequest {
                workspace_id,
                item_id,
                base_revision: revision('c')?,
                idempotency_key: key(),
            },
        )
        .await?;
    let folder = world
        .service
        .create_folder(
            &alice(OperationName::CreateFolder, Some(mutation(4)))?,
            CreateFolderRequest {
                workspace_id,
                base_revision: revision('d')?,
                folder: path("archive")?,
                idempotency_key: key(),
            },
        )
        .await?;
    let revisions: Vec<_> = [&moved, &lifecycle, &deleted, &folder]
        .iter()
        .map(|result| result.revision.clone())
        .collect();
    assert_eq!(
        revisions,
        vec![
            revision('b')?,
            revision('c')?,
            revision('d')?,
            revision('e')?
        ]
    );
    let edits: Vec<TreeEdit> = world
        .versions
        .commits()?
        .into_iter()
        .flat_map(|commit| commit.edits)
        .collect();
    assert!(matches!(
        edits.as_slice(),
        [
            TreeEdit::MoveItem { destination, .. },
            TreeEdit::SetStatus { status: Some(ItemStatus::Deprecated), archived: Some(true), .. },
            TreeEdit::DeleteItem { .. },
            TreeEdit::CreateFolder { folder },
        ] if destination.as_str() == "archive/plan.md" && folder.as_str() == "archive"
    ));
    let receipts = world.records.receipts()?;
    assert_eq!(receipts.len(), 4);
    let (under, receipt) = some(receipts.first(), "the move's receipt")?;
    assert_eq!(*under, Some(mutation(1)));
    assert_eq!(receipt.id, moved.receipt_id);
    assert_eq!(receipt.operation_id, OperationName::MoveItem);
    assert_eq!(receipt.principal_subject, ALICE);
    assert_eq!(receipt.audience, ReceiptAudience::HumanDisplay);
    assert!(receipt.sources.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_resumed_write_answers_the_first_attempts_revision_and_receipt() -> TestResult {
    let world = World::new()?;
    note_at(&world, &['a'])?;
    world.versions.queue_revision(revision('b')?)?;
    let caller = alice(OperationName::MoveItem, Some(mutation(1)))?;
    let first = world.service.move_item(&caller, move_request()?).await?;
    let again = world.service.move_item(&caller, move_request()?).await?;
    assert_eq!(again.revision, first.revision);
    assert_eq!(again.receipt_id, first.receipt_id);
    assert_eq!(world.versions.commits()?.len(), 1);
    assert_eq!(world.records.receipts()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_write_that_fails_after_its_commit_completes_on_retry() -> TestResult {
    let failures: [(&str, InjectFailure); 3] = [
        ("indexing", |world| world.search.fail_next()),
        ("the event", |world| world.events.fail_next()),
        ("the receipt", |world| world.records.fail_next_receipt()),
    ];
    for (step, fail) in failures {
        let world = World::new()?;
        note_at(&world, &['a'])?;
        world.versions.queue_revision(revision('b')?)?;
        fail(&world);
        let caller = alice(OperationName::MoveItem, Some(mutation(1)))?;
        let failed = err_of(world.service.move_item(&caller, move_request()?).await)?;
        assert_eq!(failed.code, ErrorCode::Internal, "{step}");
        assert_eq!(world.versions.commits()?.len(), 1, "{step}: committed");
        // The retry under the same idempotency key re-runs the handler.
        let retried = world.service.move_item(&caller, move_request()?).await?;
        assert_eq!(retried.revision, revision('b')?, "{step}");
        assert_eq!(
            world.versions.commits()?.len(),
            1,
            "{step}: no second commit"
        );
        assert_eq!(world.search.indexed()?, vec![revision('b')?], "{step}");
        let events = world.events.appended()?;
        assert_eq!(events.len(), 1, "{step}: one event");
        let (_, under, event) = some(events.first(), "the event")?;
        assert_eq!(*under, Some(mutation(1)), "{step}");
        assert_eq!(event.kind, EventKind::Changed, "{step}");
        assert_eq!(event.revision, Some(revision('b')?), "{step}");
        let receipts = world.records.receipts()?;
        assert_eq!(receipts.len(), 1, "{step}: one receipt");
        let (under, receipt) = some(receipts.first(), "the receipt")?;
        assert_eq!(*under, Some(mutation(1)), "{step}");
        assert_eq!(receipt.id, retried.receipt_id, "{step}");
    }
    Ok(())
}

#[tokio::test]
async fn set_type_refuses_a_header_schema_and_a_schema_that_does_not_compile() -> TestResult {
    let world = World::new()?;
    let request = |schema: serde_json::Value| -> Built<SetTypeRequest> {
        Ok(SetTypeRequest {
            workspace_id: scope()?.workspace_id,
            base_revision: revision('a')?,
            definition: TypeDefinition {
                name: "Meeting".to_owned(),
                schema_version: 1,
                properties_schema: schema,
                ui_schema: json!({}),
            },
            idempotency_key: key(),
        })
    };
    let caller = alice(OperationName::SetType, Some(mutation(1)))?;
    let header = err_of(
        world
            .service
            .set_type(
                &caller,
                request(json!({ "properties": { APP_HEADER_KEY: {} } }))?,
            )
            .await,
    )?;
    assert_eq!(
        header.field.as_deref(),
        Some("/definition/properties_schema/properties/okf_jawn")
    );
    let broken = err_of(
        world
            .service
            .set_type(&caller, request(json!({ "type": 12 }))?)
            .await,
    )?;
    assert_eq!(broken.code, ErrorCode::InvalidInput);
    assert_eq!(
        broken.field.as_deref(),
        Some("/definition/properties_schema")
    );
    assert!(world.versions.commits()?.is_empty());
    world.versions.queue_revision(revision('b')?)?;
    let saved = world
        .service
        .set_type(
            &caller,
            request(json!({ "type": "object", "properties": { "venue": { "type": "string" } } }))?,
        )
        .await?;
    assert_eq!(saved.revision, revision('b')?);
    let commits = world.versions.commits()?;
    assert!(matches!(
        commits.first().map(|commit| commit.edits.as_slice()),
        Some([TreeEdit::SetType { definition }]) if definition.name == "Meeting"
    ));
    Ok(())
}

#[tokio::test]
async fn list_types_reads_the_definitions_at_the_head() -> TestResult {
    let world = World::new()?;
    world.versions.put_types(vec![TypeDefinition {
        name: "Meeting".to_owned(),
        schema_version: 1,
        properties_schema: json!({ "type": "object" }),
        ui_schema: json!({}),
    }])?;
    let listed = world
        .service
        .list_types(
            &alice(OperationName::ListTypes, None)?,
            ListTypesRequest {
                workspace_id: scope()?.workspace_id,
            },
        )
        .await?;
    let names: Vec<&str> = listed.items.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(names, vec!["Meeting"]);
    Ok(())
}
