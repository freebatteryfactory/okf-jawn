//! Persistent Views through `ApplicationService` (core M1.6 and M1b): reading a saved View
//! (`get_view`), the catalog, a View's sources, the saved-View writes that validate it first,
//! queueing its export, and presenting and resolving it with its datasets.
//!
//! The fixture: at the head `revision('a')` a source card S (`reports/revenue-pdf.md`, its
//! conversion record `digest('b')` holding one table of quarters on page 2), a note N
//! (`notes/venue.md`) and a View V (`views/revenue.md`) whose two bindings cite S and N at
//! `revision('c')`, where both are held too.

use std::collections::BTreeMap;

use okf_jawn_contract::common::{PageRange, TextRange};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::extraction::{ConversionOutcome, Extraction, TextOrigin};
use okf_jawn_contract::history::{CommitRequest, FileChange, FileChangeKind, RestoreRequest};
use okf_jawn_contract::identity::{At, IdempotencyKey, ItemId, PurgeId, Revision};
use okf_jawn_contract::import::JobKind;
use okf_jawn_contract::item::{
    CreateItemRequest, DraftContent, ItemDocument, ItemKind, SaveDraftRequest,
};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::read::{AssetRole, Selection};
use okf_jawn_contract::source::{
    DeclaredOutcome, GetObjectRequest, GetSourcesRequest, SourceLocation, SourceLocator,
};
use okf_jawn_contract::views::{
    ChartFailure, ChartRef, ChartResult, ChartStatus, Dataset, ExportViewRequest,
    GetCatalogRequest, GetViewRequest, PresentRequest, PresentResponse, ResolveViewRequest,
    ViewDocument,
};
use okf_jawn_core::context::Attempt;
use okf_jawn_core::conversion::{
    ConversionRecord, ConvertedCell, ConvertedTable, PixelSize, RetainedAsset,
};
use okf_jawn_core::jobs::{JobScope, JobSpec, RevisionMapping};
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
use crate::proposal_store::noon;
use crate::tenancy::Tenancy;

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
/// The fixture clock's instant, which `as_of` carries.
const NOW: &str = "2026-10-09T12:00:00.000Z";

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

/// The world of the module header, V holding `body`, its service reading the fixture's noon.
fn view_world(body: &str) -> Built<World> {
    let mut world = World::new()?;
    world.service = world.service.with_clock(noon);
    stock(&world, body)?;
    Ok(world)
}

/// Put the module header's tree into `world`, V holding `body`.
fn stock(world: &World, body: &str) -> Built<()> {
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
    Ok(())
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

/// SPEC section 7 and ruling E11 keep a binding's exact selection, while an OKF note source
/// cites the whole item. The same item and revision cannot make those selections equivalent.
#[tokio::test]
async fn a_views_declared_source_keeps_a_whole_citation_beside_its_partial_binding() -> TestResult {
    let block = block_with("/bindings/0/source/revision", json!(revision('a')?))?;
    let body = view_body(
        "Revenue, with the whole report declared as a source.",
        &block,
    )?;
    let world = view_world(&body)?;
    let entry = json!({ "id": "whole-report", "resource": "../reports/revenue-pdf.md" });
    let mut document = view_item(&revision('a')?, &body)?;
    document
        .properties
        .insert("sources".to_owned(), json!([entry.clone()]));
    world.versions.replace_document(&revision('a')?, document)?;

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
    let binding = some(response.sources.first(), "the partial binding citation")?;
    assert_eq!(binding.item_id, id(SOURCE));
    assert_eq!(binding.revision, revision('a')?);
    assert_eq!(
        binding.selection,
        Selection::Pages {
            range: PageRange { start: 2, end: 2 }
        }
    );
    let declared = some(response.declared.first(), "the declared whole report")?;
    assert_eq!(declared.entry, entry);
    // Two binding citations precede the separate whole-item citation required by the entry.
    assert_eq!(declared.outcome, DeclaredOutcome::Cited { index: 2 });
    assert_eq!(response.sources.len(), 3);
    let whole = some(response.sources.get(2), "the whole report citation")?;
    assert_eq!(whole.item_id, id(SOURCE));
    assert_eq!(whole.revision, revision('a')?);
    assert_eq!(whole.digest, Some(digest('b')?));
    assert_eq!(whole.selection, Selection::All);
    assert_eq!(whole.locations, vec![page(1), page(2)]);
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

/// The wire form of a block: a `ViewDocument` in the fixture workspace, titled.
fn wire(block: &Value) -> Built<ViewDocument> {
    let mut value = block.clone();
    let object = some(value.as_object_mut(), "the block")?;
    object.insert("title".to_owned(), json!("Revenue"));
    object.insert("description".to_owned(), json!("Quarterly revenue."));
    for binding in some(
        object.get_mut("bindings").and_then(Value::as_array_mut),
        "bindings",
    )? {
        some(
            binding
                .pointer_mut("/source")
                .and_then(Value::as_object_mut),
            "a source",
        )?
        .insert("workspace_id".to_owned(), json!(scope()?.workspace_id));
    }
    Ok(serde_json::from_value(value)?)
}

fn present_request(view: ViewDocument) -> Built<PresentRequest> {
    Ok(PresentRequest {
        workspace_id: scope()?.workspace_id,
        view,
    })
}

async fn presented(world: &World, view: ViewDocument) -> Built<Result<PresentResponse, ApiError>> {
    Ok(world
        .service
        .present_view(
            &alice(OperationName::PresentView, None)?,
            present_request(view)?,
        )
        .await)
}

fn resolve_request(refresh_live: bool) -> Built<ResolveViewRequest> {
    Ok(ResolveViewRequest {
        workspace_id: scope()?.workspace_id,
        item_id: id(VIEW),
        at: At::Latest,
        refresh_live,
    })
}

async fn resolved(world: &World, refresh_live: bool) -> Built<Result<PresentResponse, ApiError>> {
    Ok(world
        .service
        .resolve_view(
            &alice(OperationName::ResolveView, None)?,
            resolve_request(refresh_live)?,
        )
        .await)
}

fn ready(bindings: &[&str]) -> ChartStatus {
    ChartStatus::Ready {
        bindings: bindings.iter().map(|name| (*name).to_owned()).collect(),
    }
}

/// The failure reason and binding of the first chart.
fn first_failure(response: &PresentResponse) -> Built<(ChartFailure, Option<String>, String)> {
    match &some(response.charts.first(), "the chart")?.status {
        ChartStatus::Failed {
            reason,
            binding,
            message,
        } => Ok((*reason, binding.clone(), message.clone())),
        ChartStatus::Ready { .. } => Err("the chart is ready".into()),
    }
}

fn codes(response: &PresentResponse) -> Vec<(String, Option<String>)> {
    response
        .warnings
        .iter()
        .map(|warning| (warning.code.clone(), warning.location.clone()))
        .collect()
}

/// S at `c` with this extraction instead of the converter's.
fn with_source_text(world: &World, extraction: Extraction) -> Built<()> {
    let at = revision('c')?;
    world.versions.replace_document(
        &at,
        source_card(SOURCE, &at, SOURCE_PATH, CARD_BODY, extraction)?,
    )?;
    Ok(())
}

#[tokio::test]
async fn present_view_materializes_records_and_cites_each_binding() -> TestResult {
    let world = standard_world()?;
    let response = presented(&world, wire(&view_block()?)?).await??;
    assert_eq!(
        response.charts,
        vec![ChartResult {
            chart: ChartRef::Named {
                name: "quarters".to_owned()
            },
            status: ready(&["revenue"]),
        }]
    );
    assert_eq!(response.as_of.as_str(), NOW);
    let cited = revision('c')?;
    assert!(response.warnings.is_empty(), "{:?}", response.warnings);
    let revenue = some(response.resolved_bindings.first(), "revenue")?;
    let venue = some(response.resolved_bindings.get(1), "venue")?;
    let digest_of = some(revenue.materialized.clone(), "the dataset")?;
    assert_eq!(venue.materialized, None);
    assert_eq!(revenue.source.revision, cited);
    assert_eq!(revenue.source.locations, vec![page(2)]);
    // The view answered is the candidate as validated: no dataset named in its bindings.
    assert!(
        response
            .view
            .bindings
            .iter()
            .all(|binding| binding.materialized.is_none())
    );
    // The dataset's bytes are retained and recorded against S at `c`.
    let dataset: Dataset =
        serde_json::from_slice(&some(world.blobs.bytes(&digest_of)?, "the bytes")?)?;
    let names: Vec<&str> = dataset
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect();
    assert_eq!(names, ["Quarter", "Revenue"]);
    assert_eq!(dataset.rows.len(), 2);
    let derived = world.records.derived()?;
    let object = some(derived.first(), "the derived object")?;
    assert_eq!(derived.len(), 1);
    assert_eq!(world.blobs.puts()?, vec![digest_of.clone()]);
    assert_eq!(
        (object.item_id, &object.revision),
        (id(SOURCE), &revision('c')?)
    );
    assert_eq!(object.digest, digest_of);
    // The receipt holds exactly the citations answered.
    let receipts = world.records.receipts()?;
    let (under, receipt) = some(receipts.first(), "the receipt")?;
    assert_eq!(*under, None);
    assert_eq!(receipt.id, response.receipt_id);
    assert_eq!(
        serde_json::to_value(&receipt.sources)?,
        serde_json::to_value(
            response
                .resolved_bindings
                .iter()
                .map(|binding| &binding.source)
                .collect::<Vec<_>>()
        )?
    );
    Ok(())
}

#[tokio::test]
async fn a_materialized_dataset_is_an_object_of_the_bound_revision() -> TestResult {
    let world = standard_world()?;
    let response = presented(&world, wire(&view_block()?)?).await??;
    let revenue = some(response.resolved_bindings.first(), "revenue")?;
    let object = some(revenue.materialized.clone(), "the dataset")?;
    let context = alice(OperationName::GetObject, None)?;
    let served = world
        .service
        .get_object(
            &context,
            GetObjectRequest {
                source: revenue.source.clone(),
                object: object.clone(),
                offset: None,
                length: None,
            },
        )
        .await?;
    assert_eq!(served.sha256, object);
    let mut elsewhere = revenue.source.clone();
    elsewhere.revision = revision('a')?;
    elsewhere.locations = Vec::new();
    let refused = err_of(
        world
            .service
            .get_object(
                &context,
                GetObjectRequest {
                    source: elsewhere,
                    object,
                    offset: None,
                    length: None,
                },
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    Ok(())
}

#[tokio::test]
async fn a_chart_over_text_that_is_not_the_converters_fails_alone_and_says_why() -> TestResult {
    let corrected = Extraction {
        corrected: true,
        ..converted(&digest('b')?)
    };
    let supplied = Extraction {
        text_origin: TextOrigin::SuppliedByAgent,
        ..converted(&digest('b')?)
    };
    for (extraction, gist) in [
        (corrected, "correction"),
        (supplied, "supplied by an agent"),
    ] {
        let world = standard_world()?;
        with_source_text(&world, extraction)?;
        let response = presented(&world, wire(&view_block()?)?).await??;
        let (reason, binding, message) = first_failure(&response)?;
        assert_eq!(reason, ChartFailure::DatasetUnavailable, "{gist}");
        assert_eq!(binding.as_deref(), Some("revenue"), "{gist}");
        assert!(message.contains(gist), "{gist}: {message}");
        assert!(codes(&response).contains(&(
            "dataset_unavailable".to_owned(),
            Some("/bindings/0".to_owned())
        )));
        assert_eq!(
            some(response.resolved_bindings.first(), "revenue")?.materialized,
            None
        );
        // The excerpt's binding is still read and cited.
        assert_eq!(response.resolved_bindings.len(), 2);
        assert!(world.records.derived()?.is_empty(), "{gist}");
    }
    Ok(())
}

#[tokio::test]
async fn a_chart_whose_own_spec_is_at_fault_fails_alone() -> TestResult {
    // A second chart with a mark Vega-Lite does not know, drawn beside the first.
    let mut block = view_block()?;
    some(
        block.pointer_mut("/charts").and_then(Value::as_object_mut),
        "charts",
    )?
    .insert(
        "broken".to_owned(),
        json!({ "data": { "name": "revenue" }, "mark": "bogus" }),
    );
    let elements = some(
        block
            .pointer_mut("/spec/elements")
            .and_then(Value::as_object_mut),
        "elements",
    )?;
    elements.insert(
        "other".to_owned(),
        json!({ "type": "Chart", "props": { "binding": "revenue", "chart": "broken", "title": "Broken" }, "children": [] }),
    );
    some(
        block
            .pointer_mut("/spec/elements/root/children")
            .and_then(Value::as_array_mut),
        "children",
    )?
    .push(json!("other"));
    let world = standard_world()?;
    let response = presented(&world, wire(&block)?).await??;
    let statuses: Vec<(&ChartRef, bool)> = response
        .charts
        .iter()
        .map(|result| {
            (
                &result.chart,
                matches!(result.status, ChartStatus::Ready { .. }),
            )
        })
        .collect();
    assert_eq!(
        statuses,
        vec![
            (
                &ChartRef::Named {
                    name: "broken".to_owned()
                },
                false
            ),
            (
                &ChartRef::Named {
                    name: "quarters".to_owned()
                },
                true
            )
        ]
    );
    assert_eq!(first_failure(&response)?.0, ChartFailure::InvalidSpec);
    // A chart that reads a name no binding declares is its own `UnknownBinding`.
    let mut unknown = view_block()?;
    *some(
        unknown.pointer_mut("/charts/quarters/data/name"),
        "the data name",
    )? = json!("nope");
    let refused = presented(&standard_world()?, wire(&unknown)?).await??;
    let (reason, binding, _) = first_failure(&refused)?;
    assert_eq!(reason, ChartFailure::UnknownBinding);
    assert_eq!(binding.as_deref(), Some("nope"));
    Ok(())
}

#[tokio::test]
async fn present_view_refuses_a_fault_of_the_view_and_writes_nothing() -> TestResult {
    let mut supplied = wire(&view_block()?)?;
    some(supplied.bindings.get_mut(0), "revenue")?.materialized = Some(digest('d')?);
    let mut located = wire(&view_block()?)?;
    some(located.bindings.get_mut(0), "revenue")?
        .source
        .locations = vec![page(1)];
    let mut moved = wire(&view_block()?)?;
    some(moved.bindings.get_mut(0), "revenue")?.source.path = path("reports/other-pdf.md")?;
    let mut unbound = view_block()?;
    *some(
        unbound.pointer_mut("/spec/elements/table/props/binding"),
        "the table",
    )? = json!("nope");
    let cases = [
        (
            wire(&unbound)?,
            ErrorCode::InvalidInput,
            "/view/spec/elements/table/props/binding",
        ),
        (
            supplied,
            ErrorCode::InvalidInput,
            "/view/bindings/0/materialized",
        ),
        (
            located,
            ErrorCode::InvalidInput,
            "/view/bindings/0/source/locations",
        ),
        (
            moved,
            ErrorCode::InvalidInput,
            "/view/bindings/0/source/path",
        ),
    ];
    for (view, code, field) in cases {
        let world = standard_world()?;
        let refused = err_of(presented(&world, view).await?)?;
        assert_eq!(refused.code, code, "{field}: {}", refused.message);
        assert_eq!(refused.field.as_deref(), Some(field));
        assert!(world.records.derived()?.is_empty(), "{field}");
        assert!(world.records.receipts()?.is_empty(), "{field}");
    }
    // A cited revision a purge removed: the typed `NotFound`, at that binding.
    let world = standard_world()?;
    removed(&world, 'c')?;
    let refused = err_of(presented(&world, wire(&view_block()?)?).await?)?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(refused.field.as_deref(), Some("/view/bindings/0/source"));
    assert!(matches!(
        refused.detail.as_deref(),
        Some(ErrorDetail::Invalidated { .. })
    ));
    assert!(world.records.receipts()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn presenting_twice_records_one_dataset_and_two_receipts() -> TestResult {
    let world = standard_world()?;
    let first = presented(&world, wire(&view_block()?)?).await??;
    let second = presented(&world, wire(&view_block()?)?).await??;
    assert_eq!(
        some(first.resolved_bindings.first(), "revenue")?.materialized,
        some(second.resolved_bindings.first(), "revenue")?.materialized
    );
    assert_eq!(world.records.derived()?.len(), 1);
    assert_eq!(world.records.receipts()?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn newer_source_data_at_the_head_is_reported_and_not_used() -> TestResult {
    let world = standard_world()?;
    let at = revision('a')?;
    world.versions.replace_document(
        &at,
        source_card(
            SOURCE,
            &at,
            SOURCE_PATH,
            CARD_BODY,
            converted(&digest('f')?),
        )?,
    )?;
    let response = presented(&world, wire(&view_block()?)?).await??;
    assert_eq!(
        codes(&response),
        vec![("newer_source".to_owned(), Some("/bindings/0".to_owned()))]
    );
    assert_eq!(
        some(response.resolved_bindings.first(), "revenue")?
            .source
            .revision,
        revision('c')?
    );
    assert_eq!(first_failure(&response).ok(), None);
    Ok(())
}

/// SPEC section 10: changing who supplies the shown text is observable at an unchanged digest.
#[tokio::test]
async fn source_freshness_observes_correction_and_supplied_text_state_at_the_same_digest()
-> TestResult {
    for (corrected, text_origin) in [
        (true, TextOrigin::Converter),
        (false, TextOrigin::SuppliedByAgent),
    ] {
        let world = standard_world()?;
        let extraction = Extraction {
            corrected,
            text_origin,
            ..converted(&digest('b')?)
        };
        world.versions.replace_document(
            &revision('a')?,
            source_card(SOURCE, &revision('a')?, SOURCE_PATH, CARD_BODY, extraction)?,
        )?;
        let response = resolved(&world, false).await??;
        assert!(
            codes(&response).contains(&("newer_source".to_owned(), Some("/bindings/0".to_owned())))
        );
        let bound = some(response.resolved_bindings.first(), "the pinned binding")?;
        assert_eq!(bound.source.revision, revision('c')?);
        assert_eq!(bound.source.digest, Some(digest('b')?));
        assert_eq!(first_failure(&response).ok(), None);
    }
    Ok(())
}

#[tokio::test]
async fn source_freshness_observes_a_further_correction_without_a_digest_or_state_change()
-> TestResult {
    let world = standard_world()?;
    let corrected = Extraction {
        corrected: true,
        ..converted(&digest('b')?)
    };
    with_source_text(&world, corrected.clone())?;
    world.versions.replace_document(
        &revision('a')?,
        source_card(
            SOURCE,
            &revision('a')?,
            SOURCE_PATH,
            "# Revenue\nA further correction: revenue is 1300.\n",
            corrected,
        )?,
    )?;
    let response = resolved(&world, false).await??;
    assert!(
        codes(&response).contains(&("newer_source".to_owned(), Some("/bindings/0".to_owned())))
    );
    let bound = some(
        response.resolved_bindings.first(),
        "the pinned corrected text",
    )?;
    assert_eq!(bound.source.revision, revision('c')?);
    assert_eq!(bound.source.digest, Some(digest('b')?));
    assert_eq!(
        first_failure(&response)?.0,
        ChartFailure::DatasetUnavailable
    );
    Ok(())
}

#[tokio::test]
async fn resolve_view_resolves_a_pinned_view_at_its_saved_revisions() -> TestResult {
    let world = standard_world()?;
    let response = resolved(&world, false).await??;
    assert_eq!(response.as_of.as_str(), NOW);
    assert_eq!(
        some(response.charts.first(), "the chart")?.status,
        ready(&["revenue"])
    );
    let saved = revision('c')?;
    assert!(
        response
            .resolved_bindings
            .iter()
            .all(|binding| binding.source.revision == saved)
    );
    let receipts = world.records.receipts()?;
    let (_, receipt) = some(receipts.first(), "the receipt")?;
    assert_eq!(receipt.operation_id, OperationName::ResolveView);
    assert_eq!(
        serde_json::to_value(&receipt.sources)?,
        serde_json::to_value(
            response
                .resolved_bindings
                .iter()
                .map(|binding| &binding.source)
                .collect::<Vec<_>>()
        )?
    );
    Ok(())
}

#[tokio::test]
async fn a_pinned_view_does_not_refresh() -> TestResult {
    let world = standard_world()?;
    let refused = err_of(resolved(&world, true).await?)?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/refresh_live"));
    assert!(world.records.receipts()?.is_empty());
    assert_eq!(world.records.derived()?, Vec::new());
    assert_eq!(world.blobs.puts()?, Vec::new());
    Ok(())
}

/// The world with V saved as a live View.
fn live_world() -> Built<World> {
    view_world(&view_body(
        "Quarterly revenue, live.",
        &block_with("/mode", json!("live"))?,
    )?)
}

#[tokio::test]
async fn a_live_view_not_refreshed_reads_its_saved_revisions() -> TestResult {
    let world = live_world()?;
    let at = revision('a')?;
    world.versions.replace_document(
        &at,
        source_card(
            SOURCE,
            &at,
            SOURCE_PATH,
            CARD_BODY,
            converted(&digest('f')?),
        )?,
    )?;
    let response = resolved(&world, false).await??;
    assert_eq!(
        some(response.resolved_bindings.first(), "revenue")?
            .source
            .revision,
        revision('c')?
    );
    assert_eq!(
        codes(&response),
        vec![("newer_source".to_owned(), Some("/bindings/0".to_owned()))]
    );
    Ok(())
}

#[tokio::test]
async fn a_live_view_refreshed_reads_each_binding_at_the_head() -> TestResult {
    // At the head S was converted again (record `f`) and moved.
    let world = live_world()?;
    let at = revision('a')?;
    world.versions.replace_document(
        &at,
        source_card(
            SOURCE,
            &at,
            "archive/revenue-pdf.md",
            CARD_BODY,
            converted(&digest('f')?),
        )?,
    )?;
    world
        .blobs
        .put_object(digest('f')?, serde_json::to_vec(&table_record()?)?)?;
    let response = resolved(&world, true).await??;
    let revenue = some(response.resolved_bindings.first(), "revenue")?;
    assert_eq!(revenue.source.revision, at);
    assert_eq!(revenue.source.path.as_str(), "archive/revenue-pdf.md");
    assert_eq!(revenue.source.digest, Some(digest('f')?));
    assert!(revenue.materialized.is_some());
    let derived = world.records.derived()?;
    assert_eq!(
        some(derived.first(), "the dataset")?.revision,
        at,
        "recorded against the revision it was read at"
    );
    // The saved View is not rewritten: its binding still cites `c`.
    assert_eq!(
        some(response.view.bindings.first(), "saved")?
            .source
            .revision,
        revision('c')?
    );
    assert_eq!(codes(&response), vec![("live_resolution".to_owned(), None)]);
    assert!(
        some(response.warnings.first(), "the warning")?
            .message
            .contains(at.as_str())
    );
    Ok(())
}

/// A live binding of a precise representation, with the same table/chart layout as the fixture.
fn live_representation(cited: Option<char>) -> Built<World> {
    let mut block = block_with("/mode", json!("live"))?;
    let source = some(
        block
            .pointer_mut("/bindings/0/source")
            .and_then(Value::as_object_mut),
        "revenue source",
    )?;
    source.insert("selection".to_owned(), json!({ "kind": "all" }));
    match cited {
        Some(fill) => {
            source.insert("digest".to_owned(), json!(digest(fill)?));
        }
        None => {
            source.remove("digest");
        }
    }
    view_world(&view_body("Live representations.", &block)?)
}

/// Re-conversion changes the record to f; the retained original is named explicitly by the test.
fn converted_head(world: &World, original: char, record: &ConversionRecord) -> Built<()> {
    let at = revision('a')?;
    let mut document = source_card(
        SOURCE,
        &at,
        SOURCE_PATH,
        CARD_BODY,
        converted(&digest('f')?),
    )?;
    some(document.source.as_mut(), "the source appearance")?.object = digest(original)?;
    world.versions.replace_document(&at, document)?;
    world
        .blobs
        .put_object(digest('f')?, serde_json::to_vec(record)?)
}

fn picture(fill: char) -> Built<RetainedAsset> {
    Ok(RetainedAsset {
        digest: digest(fill)?,
        media_type: "image/png".to_owned(),
        role: AssetRole::Picture,
        location: page(2),
        pixel_size: Some(PixelSize {
            width: 1,
            height: 1,
        }),
        caption: None,
    })
}

/// SPEC section 10: refreshing current inputs preserves which representation a binding cites.
#[tokio::test]
async fn live_refresh_preserves_the_original_representation() -> TestResult {
    let world = live_representation(Some('a'))?;
    converted_head(&world, 'd', &table_record()?)?;
    let response = resolved(&world, true).await??;
    let source = &some(response.resolved_bindings.first(), "the refreshed binding")?.source;
    assert_eq!(source.revision, revision('a')?);
    assert_eq!(source.digest, Some(digest('d')?));
    assert_eq!(source.selection, Selection::All);
    assert!(
        source.locations.is_empty(),
        "the original acquires no conversion locators"
    );
    assert_eq!(
        first_failure(&response)?.0,
        ChartFailure::DatasetUnavailable
    );
    Ok(())
}

#[tokio::test]
async fn live_refresh_preserves_a_retained_image_representation() -> TestResult {
    let world = live_representation(Some('d'))?;
    let mut record = table_record()?;
    record.assets = vec![picture('d')?];
    world
        .blobs
        .put_object(digest('b')?, serde_json::to_vec(&record)?)?;
    converted_head(&world, 'a', &record)?;
    let response = resolved(&world, true).await??;
    let source = &some(response.resolved_bindings.first(), "the refreshed image")?.source;
    assert_eq!(source.revision, revision('a')?);
    assert_eq!(source.digest, Some(digest('d')?));
    assert_eq!(source.locations, vec![page(2)]);
    assert_eq!(
        first_failure(&response)?.0,
        ChartFailure::DatasetUnavailable
    );
    Ok(())
}

#[tokio::test]
async fn live_refresh_marks_a_vanished_image_unavailable_without_guessing_its_replacement()
-> TestResult {
    let world = live_representation(Some('d'))?;
    let mut cited = table_record()?;
    cited.assets = vec![picture('d')?];
    world
        .blobs
        .put_object(digest('b')?, serde_json::to_vec(&cited)?)?;
    let mut current = table_record()?;
    // Same role and location are insufficient evidence that a different image is its successor.
    current.assets = vec![picture('e')?];
    converted_head(&world, 'a', &current)?;
    let response = resolved(&world, true).await??;
    let binding = some(response.resolved_bindings.first(), "the unavailable image")?;
    assert_eq!(
        binding.source.revision,
        revision('c')?,
        "no fabricated current citation"
    );
    assert_eq!(binding.source.digest, Some(digest('d')?));
    assert!(binding.materialized.is_none());
    assert_eq!(
        first_failure(&response)?.0,
        ChartFailure::DatasetUnavailable
    );
    assert!(codes(&response).contains(&(
        "source_unavailable".to_owned(),
        Some("/bindings/0".to_owned())
    )));
    Ok(())
}

#[tokio::test]
async fn live_refresh_adopts_a_conversion_for_a_previously_pending_text_binding() -> TestResult {
    let world = live_representation(None)?;
    let pending = Extraction {
        outcome: ConversionOutcome::Pending,
        converter: None,
        digest: None,
        text_origin: TextOrigin::None,
        page_count: None,
        ..converted(&digest('b')?)
    };
    world.versions.replace_document(
        &revision('c')?,
        source_card(SOURCE, &revision('c')?, SOURCE_PATH, "", pending)?,
    )?;
    converted_head(&world, 'a', &table_record()?)?;
    let response = resolved(&world, true).await??;
    let binding = some(
        response.resolved_bindings.first(),
        "the newly converted text",
    )?;
    assert_eq!(binding.source.revision, revision('a')?);
    assert_eq!(binding.source.digest, Some(digest('f')?));
    assert!(binding.materialized.is_some());
    assert_eq!(
        some(response.charts.first(), "the ready chart")?.status,
        ready(&["revenue"])
    );
    Ok(())
}

#[tokio::test]
async fn a_live_binding_whose_item_left_the_head_fails_its_chart_alone() -> TestResult {
    let world = live_world()?;
    world.versions.set_head(revision('d')?)?;
    // Only N and V are at the new head.
    let head = revision('d')?;
    world.versions.put_document(
        &head,
        note(
            NOTE,
            &head,
            NOTE_PATH,
            "# Venue\nThe hall.\n",
            json!({ "type": "Note", "title": "Venue" }),
        )?,
    )?;
    world.versions.put_document(
        &head,
        view_item(
            &head,
            &view_body("Live.", &block_with("/mode", json!("live"))?)?,
        )?,
    )?;
    let response = resolved(&world, true).await??;
    let (reason, binding, _) = first_failure(&response)?;
    assert_eq!(reason, ChartFailure::DatasetUnavailable);
    assert_eq!(binding.as_deref(), Some("revenue"));
    let found = codes(&response);
    assert!(found.contains(&("source_removed".to_owned(), Some("/bindings/0".to_owned()))));
    assert!(found.contains(&(
        "dataset_unavailable".to_owned(),
        Some("/bindings/0".to_owned())
    )));
    assert_eq!(
        some(response.resolved_bindings.get(1), "venue")?
            .source
            .revision,
        head
    );
    Ok(())
}

#[tokio::test]
async fn a_saved_binding_whose_revision_was_purged_is_invalidated_not_refused() -> TestResult {
    let world = standard_world()?;
    removed(&world, 'c')?;
    let response = resolved(&world, false).await??;
    let (reason, binding, _) = first_failure(&response)?;
    assert_eq!(reason, ChartFailure::Invalidated);
    assert_eq!(binding.as_deref(), Some("revenue"));
    assert_eq!(world.records.derived()?, Vec::new());
    assert_eq!(world.blobs.puts()?, Vec::new());
    Ok(())
}

#[tokio::test]
async fn resolve_view_refuses_a_note_and_a_view_this_build_cannot_present() -> TestResult {
    let world = standard_world()?;
    let note_refused = err_of(
        world
            .service
            .resolve_view(
                &alice(OperationName::ResolveView, None)?,
                ResolveViewRequest {
                    item_id: id(NOTE),
                    ..resolve_request(false)?
                },
            )
            .await,
    )?;
    assert_eq!(note_refused.code, ErrorCode::InvalidInput);
    assert_eq!(note_refused.field.as_deref(), Some("/item_id"));
    let broken = view_world(&view_body(
        "Broken.",
        &block_with("/spec/elements/table/props/binding", json!("gone"))?,
    )?)?;
    let unsupported = err_of(resolved(&broken, false).await?)?;
    assert_eq!(unsupported.code, ErrorCode::Unsupported);
    assert_eq!(unsupported.field.as_deref(), Some("/item_id"));
    assert!(broken.records.receipts()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_unit_naming_no_column_is_reported() -> TestResult {
    let world = view_world(&view_body(
        "Units.",
        &block_with(
            "/bindings/0/units",
            json!({ "Revenue": "EUR", "Pages": "count" }),
        )?,
    )?)?;
    let response = resolved(&world, false).await??;
    assert_eq!(
        codes(&response),
        vec![("unknown_unit".to_owned(), Some("/bindings/0".to_owned()))]
    );
    let bytes = some(
        world.blobs.bytes(&some(
            some(response.resolved_bindings.first(), "revenue")?
                .materialized
                .clone(),
            "the dataset",
        )?)?,
        "the bytes",
    )?;
    let dataset: Dataset = serde_json::from_slice(&bytes)?;
    let revenue = some(dataset.columns.get(1), "the revenue column")?;
    assert_eq!(revenue.unit.as_deref(), Some("EUR"));
    Ok(())
}

fn export_request(item: u128) -> Built<ExportViewRequest> {
    Ok(ExportViewRequest {
        workspace_id: scope()?.workspace_id,
        item_id: id(item),
        at: At::Latest,
        idempotency_key: IdempotencyKey(Uuid::from_u128(81)),
    })
}

/// The tenancy fixture (jobs and wake-ups) over the module header's tree, V holding `body`.
fn export_tenancy(body: &str) -> Built<Tenancy> {
    let tenancy = Tenancy::new()?;
    stock(&tenancy.world, body)?;
    Ok(tenancy)
}

#[tokio::test]
async fn export_view_queues_the_export_of_the_view_at_the_revision_it_resolved() -> TestResult {
    let tenancy = export_tenancy(&view_body("Revenue.", &view_block()?)?)?;
    let caller = alice(OperationName::ExportView, Some(mutation(5)))?;
    let job = tenancy
        .service
        .export_view(&caller, export_request(VIEW)?)
        .await?;
    assert_eq!(job.kind, JobKind::ExportView);
    let created = tenancy.jobs.created()?;
    let (job_scope, new_job, _) = some(created.first(), "the job")?;
    assert_eq!(*job_scope, JobScope::Workspace(scope()?));
    assert_eq!(new_job.mutation_id, mutation(5));
    assert_eq!(
        new_job.spec,
        JobSpec::ExportView {
            item_id: id(VIEW),
            revision: revision('a')?,
        }
    );
    assert_eq!(
        tenancy.queue.woken()?,
        vec![(JobScope::Workspace(scope()?), job.id)]
    );
    // A retry under the same mutation answers the job its first attempt created.
    let mut resumed = caller.clone();
    resumed.attempt = Attempt::Resumed;
    let again = tenancy
        .service
        .export_view(&resumed, export_request(VIEW)?)
        .await?;
    assert_eq!(again.id, job.id);
    assert_eq!(tenancy.jobs.created()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn export_view_queues_nothing_for_a_note_or_a_view_it_cannot_read() -> TestResult {
    let tenancy = export_tenancy(&view_body("Revenue.", &view_block()?)?)?;
    let caller = alice(OperationName::ExportView, Some(mutation(5)))?;
    let note_refused = err_of(
        tenancy
            .service
            .export_view(&caller, export_request(NOTE)?)
            .await,
    )?;
    assert_eq!(note_refused.code, ErrorCode::InvalidInput);
    assert_eq!(note_refused.field.as_deref(), Some("/item_id"));
    let broken = export_tenancy("Only prose.\n")?;
    let unsupported = err_of(
        broken
            .service
            .export_view(&caller, export_request(VIEW)?)
            .await,
    )?;
    assert_eq!(unsupported.code, ErrorCode::Unsupported);
    assert!(tenancy.jobs.created()?.is_empty());
    assert!(broken.jobs.created()?.is_empty());
    assert_eq!(broken.queue.woken()?, Vec::new());
    Ok(())
}

#[tokio::test]
async fn a_view_whose_chart_fails_its_own_check_is_still_exported() -> TestResult {
    let tenancy = export_tenancy(&view_body(
        "Revenue.",
        &block_with("/charts/quarters/mark", json!("bogus"))?,
    )?)?;
    tenancy
        .service
        .export_view(
            &alice(OperationName::ExportView, Some(mutation(5)))?,
            export_request(VIEW)?,
        )
        .await?;
    assert_eq!(tenancy.jobs.created()?.len(), 1);
    Ok(())
}
