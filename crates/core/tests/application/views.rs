//! Persistent Views through `ApplicationService` (core M1.6 and M1b): reading a saved View
//! (`get_view`), the catalog, a View's sources, the saved-View writes that validate it first,
//! queueing its export, and presenting and resolving it with its datasets.
//!
//! The fixture: at the head `revision('a')` a source card S (`reports/revenue-pdf.md`, its
//! conversion record `digest('b')` holding one table of quarters on page 2), a note N
//! (`notes/venue.md`) and a View V (`views/revenue.md`) whose two bindings cite S and N at
//! `revision('c')`, where both are held too.

use std::collections::BTreeMap;

use okf_jawn_contract::common::TextRange;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::history::{CommitRequest, FileChange, FileChangeKind, RestoreRequest};
use okf_jawn_contract::identity::{At, IdempotencyKey, ItemId, PurgeId, Revision};
use okf_jawn_contract::item::{
    CreateItemRequest, DraftContent, ItemDocument, ItemKind, SaveDraftRequest,
};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::source::{GetSourcesRequest, SourceLocation, SourceLocator};
use okf_jawn_contract::views::{GetCatalogRequest, GetViewRequest, ViewDocument};
use okf_jawn_core::conversion::{ConversionRecord, ConvertedCell, ConvertedTable};
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::portable::draft_digest;
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::{TreeEdit, derive_item_id};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    ALICE, Built, CARD_BODY, NOTE, World, alice, converted, digest, draft_of, mutation, note, path,
    record, revision, scope, source_card,
};

/// The source card S.
const SOURCE: u128 = 30;
/// The View V.
const VIEW: u128 = 40;
/// S's path.
const SOURCE_PATH: &str = "reports/revenue-pdf.md";
/// N's path.
const NOTE_PATH: &str = "notes/venue.md";
/// V's path.
const VIEW_PATH: &str = "views/revenue.md";

fn id(number: u128) -> ItemId {
    ItemId(Uuid::from_u128(number))
}

fn page(page_no: u32) -> SourceLocation {
    SourceLocation::Direct {
        locator: SourceLocator::Page { page_no },
    }
}

fn cell(row: u32, column: u32, text: &str) -> ConvertedCell {
    ConvertedCell {
        row,
        column,
        row_span: 1,
        column_span: 1,
        text: text.to_owned(),
        column_header: row == 0,
        row_header: false,
    }
}

/// The fixture record of `CARD_BODY` with one table of quarters on page 2 (lines 5 to 8).
fn table_record() -> Built<ConversionRecord> {
    let mut held = record(Vec::new())?;
    held.tables = vec![ConvertedTable {
        lines: TextRange { start: 5, end: 8 },
        location: page(2),
        num_rows: 3,
        num_cols: 2,
        cells: vec![
            cell(0, 0, "Quarter"),
            cell(0, 1, "Revenue"),
            cell(1, 0, "Q1"),
            cell(1, 1, "1200.5"),
            cell(2, 0, "Q2"),
            cell(2, 1, "980"),
        ],
    }];
    Ok(held)
}

/// One binding of a View block, citing `revision('c')`.
fn stored_binding(
    name: &str,
    item: u128,
    location: &str,
    selection: &Value,
    cites: bool,
) -> Built<Value> {
    let mut source = json!({
        "item_id": id(item),
        "path": location,
        "revision": revision('c')?,
        "selection": selection
    });
    if cites && let Some(object) = source.as_object_mut() {
        object.insert("digest".to_owned(), json!(digest('b')?));
    }
    Ok(json!({ "name": name, "source": source, "units": {}, "transforms": [] }))
}

/// The block of V: a chart of S's revenue, S's table and an excerpt of N.
fn view_block() -> Built<Value> {
    Ok(json!({
        "schema_version": 1,
        "mode": "pinned",
        "grammar": "json_render",
        "bindings": [
            stored_binding("revenue", SOURCE, SOURCE_PATH, &json!({ "kind": "pages", "range": { "start": 2, "end": 2 } }), true)?,
            stored_binding("venue", NOTE, NOTE_PATH, &json!({ "kind": "all" }), false)?
        ],
        "spec": {
            "root": "root",
            "elements": {
                "root": { "type": "Stack", "props": { "title": "Revenue" }, "children": ["chart", "table", "excerpt", "sources"] },
                "chart": { "type": "Chart", "props": { "binding": "revenue", "chart": "quarters", "title": "Revenue per quarter" }, "children": [] },
                "table": { "type": "DataTable", "props": { "binding": "revenue" }, "children": [] },
                "excerpt": { "type": "SourceExcerpt", "props": { "binding": "venue" }, "children": [] },
                "sources": { "type": "SourceList", "props": { "bindings": ["revenue", "venue"] }, "children": [] }
            }
        },
        "charts": {
            "quarters": {
                "data": { "name": "revenue" },
                "mark": "bar",
                "encoding": {
                    "x": { "field": "Quarter", "type": "nominal" },
                    "y": { "field": "Revenue", "type": "quantitative" }
                }
            }
        }
    }))
}

/// A View body: the description, then the block.
fn view_body(description: &str, block: &Value) -> Built<String> {
    Ok(format!(
        "{description}\n\n```json okf-jawn-view\n{}\n```\n",
        serde_json::to_string_pretty(block)?
    ))
}

/// The View item at `at` with this body.
fn view_item(at: &Revision, body: &str) -> Built<ItemDocument> {
    let mut document = note(
        VIEW,
        at,
        VIEW_PATH,
        body,
        json!({ "type": "Dashboard", "title": "Revenue" }),
    )?;
    document.summary.kind = ItemKind::View;
    "Dashboard".clone_into(&mut document.summary.type_name);
    Ok(document)
}

/// The world of the module header, V holding `body`.
fn view_world(body: &str) -> Built<World> {
    let world = World::new()?;
    for fill in ['a', 'c'] {
        let at = revision(fill)?;
        world.versions.put_document(
            &at,
            source_card(
                SOURCE,
                &at,
                SOURCE_PATH,
                CARD_BODY,
                converted(&digest('b')?),
            )?,
        )?;
        world.versions.put_document(
            &at,
            note(
                NOTE,
                &at,
                NOTE_PATH,
                "# Venue\nThe hall.\n",
                json!({ "type": "Note", "title": "Venue" }),
            )?,
        )?;
    }
    world
        .versions
        .put_document(&revision('a')?, view_item(&revision('a')?, body)?)?;
    world
        .blobs
        .put_object(digest('b')?, serde_json::to_vec(&table_record()?)?)?;
    Ok(world)
}

fn standard_world() -> Built<World> {
    view_world(&view_body(
        "Quarterly revenue, from the report.",
        &view_block()?,
    )?)
}

fn get_view_request(item: u128, at: At) -> Built<GetViewRequest> {
    Ok(GetViewRequest {
        workspace_id: scope()?.workspace_id,
        item_id: id(item),
        at,
    })
}

async fn opened(world: &World, item: u128) -> Built<Result<ViewDocument, ApiError>> {
    Ok(world
        .service
        .get_view(
            &alice(OperationName::GetView, None)?,
            get_view_request(item, At::Latest)?,
        )
        .await)
}

#[tokio::test]
async fn get_view_reads_the_saved_view_with_its_workspace_and_locations() -> TestResult {
    let world = standard_world()?;
    let view = opened(&world, VIEW).await??;
    assert_eq!(view.title, "Revenue");
    assert_eq!(view.description, "Quarterly revenue, from the report.");
    assert_eq!(view.bindings.len(), 2);
    let revenue = some(view.bindings.first(), "the revenue binding")?;
    let venue = some(view.bindings.get(1), "the venue binding")?;
    for binding in [revenue, venue] {
        assert_eq!(binding.source.workspace_id, scope()?.workspace_id);
        assert_eq!(binding.source.revision, revision('c')?);
        assert_eq!(binding.materialized, None);
    }
    // The card's page 2 is where the cited lines lie; a note has no place in an original.
    assert_eq!(revenue.source.locations, vec![page(2)]);
    assert_eq!(venue.source.locations, Vec::new());
    assert_eq!(view.charts.len(), 1);
    Ok(())
}

#[tokio::test]
async fn get_view_reads_an_earlier_revision_of_the_view() -> TestResult {
    let world = standard_world()?;
    let earlier = view_body("An earlier explanation.", &view_block()?)?;
    world
        .versions
        .put_document(&revision('c')?, view_item(&revision('c')?, &earlier)?)?;
    let view = world
        .service
        .get_view(
            &alice(OperationName::GetView, None)?,
            get_view_request(
                VIEW,
                At::Revision {
                    revision: revision('c')?,
                },
            )?,
        )
        .await?;
    assert_eq!(view.description, "An earlier explanation.");
    Ok(())
}

#[tokio::test]
async fn get_view_refuses_an_item_that_is_not_a_view_or_does_not_read_as_one() -> TestResult {
    let world = standard_world()?;
    let refused = err_of(opened(&world, NOTE).await?)?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/item_id"));
    let mut later = view_block()?;
    *some(later.get_mut("schema_version"), "the version")? = json!(2);
    let newer = view_world(&view_body("Newer.", &later)?)?;
    let unsupported = err_of(opened(&newer, VIEW).await?)?;
    assert_eq!(unsupported.code, ErrorCode::Unsupported);
    assert_eq!(unsupported.field.as_deref(), Some("/item_id"));
    assert!(
        unsupported.message.contains("/schema_version"),
        "{}",
        unsupported.message
    );
    let broken = view_world("Only prose, no block.\n")?;
    assert_eq!(
        err_of(opened(&broken, VIEW).await?)?.code,
        ErrorCode::Unsupported
    );
    Ok(())
}

#[tokio::test]
async fn get_view_answers_a_view_that_no_longer_validates_so_it_can_be_fixed() -> TestResult {
    // A table naming a binding the View does not declare fails `validate`, not the read.
    let mut block = view_block()?;
    *some(
        block.pointer_mut("/spec/elements/table/props/binding"),
        "the table's binding",
    )? = json!("gone");
    let world = view_world(&view_body("Broken.", &block)?)?;
    let view = opened(&world, VIEW).await??;
    assert_eq!(view.description, "Broken.");
    Ok(())
}

#[tokio::test]
async fn get_view_follows_a_rewritten_revision_and_keeps_a_removed_one_unlocated() -> TestResult {
    let world = standard_world()?;
    let at = revision('d')?;
    world.versions.put_document(
        &at,
        source_card(
            SOURCE,
            &at,
            SOURCE_PATH,
            CARD_BODY,
            converted(&digest('b')?),
        )?,
    )?;
    world.records.put_mapping(
        revision('c')?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(9)),
            replacement: Some(at.clone()),
        },
    )?;
    let remapped = opened(&world, VIEW).await??;
    let revenue = some(remapped.bindings.first(), "the revenue binding")?;
    assert_eq!(revenue.source.revision, at);
    assert_eq!(revenue.source.locations, vec![page(2)]);

    let removed = standard_world()?;
    removed.records.put_mapping(
        revision('c')?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(9)),
            replacement: None,
        },
    )?;
    let kept = opened(&removed, VIEW).await??;
    let revenue = some(kept.bindings.first(), "the revenue binding")?;
    assert_eq!(revenue.source.revision, revision('c')?);
    assert_eq!(revenue.source.locations, Vec::<SourceLocation>::new());
    Ok(())
}

#[tokio::test]
async fn get_catalog_answers_the_generated_catalog() -> TestResult {
    let world = World::new()?;
    let catalog = world
        .service
        .get_catalog(
            &alice(OperationName::GetCatalog, None)?,
            GetCatalogRequest {},
        )
        .await?;
    let committed: Value =
        serde_json::from_str(include_str!("../../../../api/presentation/catalog.json"))?;
    assert_eq!(serde_json::to_value(&catalog)?, committed);
    let names: Vec<&str> = catalog
        .components
        .iter()
        .map(|component| component.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Stack",
            "Columns",
            "SourceExcerpt",
            "DataTable",
            "Chart",
            "SourceList"
        ]
    );
    assert!(
        catalog
            .components
            .iter()
            .all(|component| component.actions.is_empty())
    );
    let view_schema: Value =
        serde_json::from_str(include_str!("../../../../api/forms/view.schema.json"))?;
    assert_eq!(catalog.view_schema, view_schema);
    Ok(())
}

#[tokio::test]
async fn get_sources_of_a_view_answers_its_bindings_citations() -> TestResult {
    let world = standard_world()?;
    let response = world
        .service
        .get_sources(
            &alice(OperationName::GetSources, None)?,
            GetSourcesRequest {
                workspace_id: scope()?.workspace_id,
                item_id: id(VIEW),
                at: At::Latest,
            },
        )
        .await?;
    assert_eq!(response.revision, revision('a')?);
    assert!(response.appearance.is_none());
    let cited: Vec<(ItemId, Revision, Vec<SourceLocation>)> = response
        .sources
        .iter()
        .map(|source| {
            (
                source.item_id,
                source.revision.clone(),
                source.locations.clone(),
            )
        })
        .collect();
    assert_eq!(
        cited,
        vec![
            (id(SOURCE), revision('c')?, vec![page(2)]),
            (id(NOTE), revision('c')?, Vec::new())
        ]
    );
    Ok(())
}

/// A `create_item` of a View at `views/new.md` titled `Revenue`.
fn create_view(body: &str) -> Built<CreateItemRequest> {
    Ok(CreateItemRequest {
        workspace_id: scope()?.workspace_id,
        base_revision: revision('a')?,
        path: path("views/new.md")?,
        title: "Revenue".to_owned(),
        type_name: "Dashboard".to_owned(),
        kind: ItemKind::View,
        body: body.to_owned(),
        properties: BTreeMap::new(),
        idempotency_key: IdempotencyKey(Uuid::from_u128(77)),
    })
}

/// The standard block with one edit at `pointer`.
fn block_with(pointer: &str, value: Value) -> Built<Value> {
    let mut block = view_block()?;
    *some(block.pointer_mut(pointer), pointer)? = value;
    Ok(block)
}

/// The standard block with `key` set in the object at `pointer`.
fn block_setting(pointer: &str, key: &str, value: Value) -> Built<Value> {
    let mut block = view_block()?;
    some(
        block.pointer_mut(pointer).and_then(Value::as_object_mut),
        pointer,
    )?
    .insert(key.to_owned(), value);
    Ok(block)
}

/// Record that a purge removed `revision(fill)`.
fn removed(world: &World, fill: char) -> Built<()> {
    world.records.put_mapping(
        revision(fill)?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(9)),
            replacement: None,
        },
    )?;
    Ok(())
}

#[tokio::test]
async fn create_item_writes_a_valid_view_as_the_client_wrote_it() -> TestResult {
    let world = standard_world()?;
    let body = view_body("Quarterly revenue.", &view_block()?)?;
    let item_id = derive_item_id(mutation(1), 0);
    let mut committed = view_item(&revision('e')?, &body)?;
    committed.summary.id = item_id;
    committed.summary.path = path("views/new.md")?;
    world.versions.queue_revision(revision('e')?)?;
    world.versions.put_document(&revision('e')?, committed)?;
    let created = world
        .service
        .create_item(
            &alice(OperationName::CreateItem, Some(mutation(1)))?,
            create_view(&body)?,
        )
        .await?;
    assert_eq!(created.item.summary.kind, ItemKind::View);
    let commits = world.versions.commits()?;
    assert_eq!(commits.len(), 1);
    assert!(matches!(
        some(commits.first(), "the commit")?.edits.as_slice(),
        [TreeEdit::CreateItem { kind: ItemKind::View, body: written, .. }] if *written == body
    ));
    // The file names no workspace and keeps no derived field.
    for derived in ["workspace_id", "locations", "materialized"] {
        assert!(!body.contains(derived), "{derived}");
    }
    Ok(())
}

#[tokio::test]
async fn create_item_refuses_an_invalid_view_on_its_field_and_commits_nothing() -> TestResult {
    let foreign = block_setting(
        "/bindings/0/source",
        "workspace_id",
        json!(scope()?.workspace_id),
    )?;
    let absent = block_with("/bindings/1/source/item_id", json!(id(99)))?;
    let bogus = block_with("/charts/quarters/mark", json!("bogus"))?;
    let cases = [
        (
            foreign,
            ErrorCode::InvalidInput,
            "/bindings/0/source/workspace_id",
        ),
        (absent, ErrorCode::NotFound, "/bindings/1/source"),
        (bogus, ErrorCode::InvalidInput, "/charts/quarters/mark"),
    ];
    for (block, code, pointer) in cases {
        let world = standard_world()?;
        let refused = err_of(
            world
                .service
                .create_item(
                    &alice(OperationName::CreateItem, Some(mutation(1)))?,
                    create_view(&view_body("Revenue.", &block)?)?,
                )
                .await,
        )?;
        assert_eq!(refused.code, code, "{pointer}: {}", refused.message);
        assert_eq!(refused.field.as_deref(), Some("/body"), "{pointer}");
        assert!(
            refused.message.starts_with(pointer),
            "{pointer}: {}",
            refused.message
        );
        assert!(world.versions.commits()?.is_empty(), "{pointer}");
    }
    // A cited revision a purge removed is the typed `NotFound`, still on the body.
    let world = standard_world()?;
    removed(&world, 'c')?;
    let invalidated = err_of(
        world
            .service
            .create_item(
                &alice(OperationName::CreateItem, Some(mutation(1)))?,
                create_view(&view_body("Revenue.", &view_block()?)?)?,
            )
            .await,
    )?;
    assert_eq!(invalidated.code, ErrorCode::NotFound);
    assert_eq!(invalidated.field.as_deref(), Some("/body"));
    assert!(matches!(
        invalidated.detail.as_deref(),
        Some(ErrorDetail::Invalidated { .. })
    ));
    assert!(world.versions.commits()?.is_empty());
    Ok(())
}

fn save_view(body: &str, properties: Value) -> Built<SaveDraftRequest> {
    Ok(SaveDraftRequest {
        workspace_id: scope()?.workspace_id,
        item_id: id(VIEW),
        base_revision: revision('a')?,
        body: body.to_owned(),
        properties: serde_json::from_value(properties)?,
        idempotency_key: IdempotencyKey(Uuid::from_u128(78)),
    })
}

#[tokio::test]
async fn a_view_draft_is_saved_only_as_a_valid_view() -> TestResult {
    let world = standard_world()?;
    let context = alice(OperationName::SaveDraft, Some(mutation(2)))?;
    let titled = json!({ "type": "Dashboard", "title": "Revenue" });
    let prose = err_of(
        world
            .service
            .save_draft(&context, save_view("# Not a View\n", titled.clone())?)
            .await,
    )?;
    assert_eq!(prose.code, ErrorCode::InvalidInput);
    assert_eq!(prose.field.as_deref(), Some("/body"));
    let valid = view_body("Revised.", &view_block()?)?;
    let untitled = err_of(
        world
            .service
            .save_draft(&context, save_view(&valid, json!({ "type": "Dashboard" }))?)
            .await,
    )?;
    assert_eq!(untitled.field.as_deref(), Some("/properties/title"));
    assert!(world.drafts.saves()?.is_empty());
    world
        .service
        .save_draft(&context, save_view(&valid, titled)?)
        .await?;
    assert_eq!(world.drafts.saves()?.len(), 1);
    Ok(())
}

/// Alice's draft of V at `a` holding `body`, titled.
fn view_draft(body: &str) -> Built<DraftContent> {
    let mut held = draft_of(VIEW, ALICE, &revision('a')?, body)?;
    held.properties
        .insert("type".to_owned(), json!("Dashboard"));
    held.properties.insert("title".to_owned(), json!("Revenue"));
    held.draft.content_digest = draft_digest(&held.body, &held.properties)?;
    Ok(held)
}

fn snapshot_of_the_view() -> Built<CommitRequest> {
    Ok(CommitRequest {
        workspace_id: scope()?.workspace_id,
        item_ids: vec![id(VIEW)],
        message: "Snapshot".to_owned(),
        idempotency_key: IdempotencyKey(Uuid::from_u128(79)),
    })
}

#[tokio::test]
async fn a_snapshot_checks_a_view_draft_again_and_commits_nothing_when_it_fails() -> TestResult {
    // The draft was valid when saved; its cited revision was removed since.
    let world = standard_world()?;
    world.versions.queue_revision(revision('e')?)?;
    world
        .drafts
        .put_draft(view_draft(&view_body("Revised.", &view_block()?)?)?)?;
    removed(&world, 'c')?;
    let refused = err_of(
        world
            .service
            .commit_items(
                &alice(OperationName::CommitItems, Some(mutation(3)))?,
                snapshot_of_the_view()?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(refused.field.as_deref(), Some("/body"));
    assert!(
        refused.message.contains(&Uuid::from_u128(VIEW).to_string()),
        "{}",
        refused.message
    );
    assert!(world.versions.commits()?.is_empty());
    assert_eq!(world.drafts.held()?.len(), 1);
    // Without the purge, the same draft is committed as its edit.
    let committing = standard_world()?;
    committing.versions.queue_revision(revision('e')?)?;
    let body = view_body("Revised.", &view_block()?)?;
    committing.drafts.put_draft(view_draft(&body)?)?;
    committing
        .service
        .commit_items(
            &alice(OperationName::CommitItems, Some(mutation(3)))?,
            snapshot_of_the_view()?,
        )
        .await?;
    let commits = committing.versions.commits()?;
    assert!(matches!(
        some(commits.first(), "the snapshot")?.edits.as_slice(),
        [TreeEdit::EditItem { body: written, .. }] if *written == body
    ));
    Ok(())
}

#[tokio::test]
async fn a_rewind_restores_a_view_as_its_history_holds_it_without_validating_it() -> TestResult {
    // At `c` the View's file was prose only, which no longer reads as a View.
    let world = standard_world()?;
    world.versions.put_document(
        &revision('c')?,
        view_item(&revision('c')?, "Prose only.\n")?,
    )?;
    world.versions.put_diff(
        &revision('a')?,
        &revision('c')?,
        Some(id(VIEW)),
        vec![FileChange {
            old_path: Some(VIEW_PATH.to_owned()),
            new_path: Some(VIEW_PATH.to_owned()),
            patch: "@@ -1 +1 @@\n-Quarterly revenue, from the report.\n+Prose only.\n".to_owned(),
            binary: false,
            kind: FileChangeKind::Modified,
        }],
    )?;
    world.versions.queue_revision(revision('e')?)?;
    world
        .service
        .restore_items(
            &alice(OperationName::RestoreItems, Some(mutation(4)))?,
            RestoreRequest {
                workspace_id: scope()?.workspace_id,
                base_revision: revision('a')?,
                target: revision('c')?,
                item_id: Some(id(VIEW)),
                message: "Rewind".to_owned(),
                idempotency_key: IdempotencyKey(Uuid::from_u128(80)),
            },
        )
        .await?;
    let commits = world.versions.commits()?;
    assert!(matches!(
        some(commits.first(), "the rewind")?.edits.as_slice(),
        [TreeEdit::RestoreItem { .. }]
    ));
    Ok(())
}
