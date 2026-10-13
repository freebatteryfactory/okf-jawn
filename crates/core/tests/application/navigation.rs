//! Search, links and the graph: one resolved, purge-checked revision of the derived index,
//! built on demand; drafts never reach a result; archived items only when asked, deprecated
//! items always.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use okf_jawn_contract::common::{PageRequest, TextRange};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::events::ReceiptAudience;
use okf_jawn_contract::extraction::{
    ExtractionFilter, ExtractionStatus, ExtractionSummary, TextOrigin,
};
use okf_jawn_contract::history::{Commit, LogResponse};
use okf_jawn_contract::identity::{At, ItemId, PurgeId, Revision};
use okf_jawn_contract::item::{ItemKind, ItemStatus, ItemSummary};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::search::{
    GetGraphRequest, GetGraphResponse, GetLinksRequest, GetLinksResponse, Link, LinkDirection,
    SearchRequest, SearchResponse,
};
use okf_jawn_contract::source::{SourceLocation, SourceLocator};
use okf_jawn_core::application::{ApplicationService, Ports};
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::ports::{Application, PortFuture};
use okf_jawn_core::search::{GraphQuery, LinkQuery, SearchIndex, SearchQuery};
use okf_jawn_core::storage::StorageScope;
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    ALICE, Built, CARD_BODY, FakeSearch, NOTE, Unused, World, alice, config, converted, digest,
    draft_of, instant, note, path, record, revision, scope, source_card,
};

/// How many index builds started, are running, and ran at most at once.
#[derive(Default)]
struct Counters {
    started: AtomicUsize,
    running: AtomicUsize,
    most: AtomicUsize,
}

/// A search index whose build runs on the blocking pool, as storage's does
/// (`crates/storage/src/search.rs:69`, `tokio::task::spawn_blocking`): dropping the future
/// that awaits it does not stop it.
struct DetachedSearch {
    inner: Arc<FakeSearch>,
    counters: Arc<Counters>,
    fail_first: AtomicBool,
    millis: u64,
}

const OTHER: u128 = 21;
const THIRD: u128 = 22;
const CARD: u128 = 10;

impl SearchIndex for DetachedSearch {
    fn index_revision<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: Revision,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            let counters = Arc::clone(&self.counters);
            let millis = self.millis;
            let fail = self.fail_first.swap(false, Ordering::SeqCst);
            tokio::task::spawn_blocking(move || {
                counters.started.fetch_add(1, Ordering::SeqCst);
                let now = counters
                    .running
                    .fetch_add(1, Ordering::SeqCst)
                    .saturating_add(1);
                counters.most.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(millis));
                counters.running.fetch_sub(1, Ordering::SeqCst);
            })
            .await
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
            if fail {
                return Err(ApiError::new(ErrorCode::Internal, "the build failed"));
            }
            self.inner.index_revision(scope, revision).await
        })
    }
    fn search<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: SearchQuery,
    ) -> PortFuture<'a, SearchResponse> {
        self.inner.search(scope, query)
    }
    fn links<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: LinkQuery,
    ) -> PortFuture<'a, GetLinksResponse> {
        self.inner.links(scope, query)
    }
    fn graph<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: GraphQuery,
    ) -> PortFuture<'a, GetGraphResponse> {
        self.inner.graph(scope, query)
    }
    fn rebuild<'a>(&'a self, scope: &'a StorageScope, head: Revision) -> PortFuture<'a, ()> {
        self.inner.rebuild(scope, head)
    }
}

fn item(id: u128) -> ItemId {
    ItemId(Uuid::from_u128(id))
}

fn summary(id: u128, location: &str) -> Built<ItemSummary> {
    Ok(note(id, &revision('a')?, location, "", json!({}))?.summary)
}

fn search(query: &str, include_archived: bool) -> Built<SearchRequest> {
    Ok(SearchRequest {
        workspace_id: scope()?.workspace_id,
        at: At::Latest,
        query: query.to_owned(),
        folder: None,
        include_archived,
        extraction: None,
        page: PageRequest {
            cursor: None,
            limit: 50,
        },
    })
}

fn links(direction: LinkDirection, at: At) -> Built<GetLinksRequest> {
    Ok(GetLinksRequest {
        workspace_id: scope()?.workspace_id,
        item_id: item(NOTE),
        at,
        direction,
        page: PageRequest {
            cursor: None,
            limit: 50,
        },
    })
}

fn graph(folder: Option<&str>, max_nodes: u32, at: At) -> Built<GetGraphRequest> {
    Ok(GetGraphRequest {
        workspace_id: scope()?.workspace_id,
        at,
        folder: folder.map(str::to_owned),
        max_nodes,
    })
}

fn link(from: u128, to: u128, line: u32) -> Link {
    Link {
        from: item(from),
        to_path: format!("notes/{to}.md"),
        to: Some(item(to)),
        label: format!("to {to}"),
        line,
    }
}

fn hit_items(response: &okf_jawn_contract::search::SearchResponse) -> Vec<ItemId> {
    response.hits.iter().map(|hit| hit.source.item_id).collect()
}

/// The accepted line back from the head `a`: the head, then each revision of `older`.
fn accepted_line(world: &World, older: &[char]) -> Built<()> {
    let commits = std::iter::once('a')
        .chain(older.iter().copied())
        .map(|fill| {
            Ok(Commit {
                revision: revision(fill)?,
                parents: Vec::new(),
                message: "Edit".to_owned(),
                author: ALICE.to_owned(),
                committed_at: instant()?,
            })
        })
        .collect::<Built<Vec<_>>>()?;
    world.versions.put_log(
        &revision('a')?,
        None,
        LogResponse {
            commits,
            next_cursor: None,
        },
    )?;
    Ok(())
}

fn search_at(fill: char) -> Built<SearchRequest> {
    let mut request = search("plan", false)?;
    request.at = At::Revision {
        revision: revision(fill)?,
    };
    Ok(request)
}

#[tokio::test]
async fn search_items_searches_the_resolved_revision_and_records_a_receipt() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    world
        .search
        .put_entry(&at, summary(NOTE, "notes/plan.md")?, "the launch plan")?;
    world
        .search
        .put_entry(&at, summary(OTHER, "drafts/plan.md")?, "another plan")?;
    let mut request = search("plan", false)?;
    request.folder = Some("notes".to_owned());
    let response = world
        .service
        .search_items(&alice(OperationName::SearchItems, None)?, request)
        .await?;
    assert_eq!(response.revision, at);
    assert_eq!(hit_items(&response), vec![item(NOTE)]);
    let queries = world.search.queries()?;
    let query = some(queries.first(), "the query")?;
    assert_eq!(query.revision, at);
    assert_eq!(query.text, "plan");
    assert_eq!(query.folder, Some(path("notes")?));
    let receipts = world.records.receipts()?;
    let (under, receipt) = some(receipts.first(), "the search's receipt")?;
    assert_eq!(*under, None);
    assert_eq!(receipt.operation_id, OperationName::SearchItems);
    assert_eq!(
        serde_json::to_value(&receipt.sources)?,
        serde_json::to_value(
            response
                .hits
                .iter()
                .map(|hit| hit.source.clone())
                .collect::<Vec<_>>()
        )?
    );
    Ok(())
}

#[tokio::test]
async fn search_items_searches_archived_items_only_when_asked_and_deprecated_items_always()
-> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    let mut deprecated = summary(NOTE, "notes/old.md")?;
    deprecated.status = ItemStatus::Deprecated;
    let mut archived = summary(OTHER, "notes/shelved.md")?;
    archived.archived = true;
    world
        .search
        .put_entry(&at, deprecated, "plan, deprecated")?;
    world.search.put_entry(&at, archived, "plan, archived")?;
    world
        .search
        .put_entry(&at, summary(THIRD, "notes/plan.md")?, "plan")?;
    let caller = alice(OperationName::SearchItems, None)?;
    let ordinary = world
        .service
        .search_items(&caller, search("plan", false)?)
        .await?;
    assert_eq!(hit_items(&ordinary), vec![item(NOTE), item(THIRD)]);
    let everything = world
        .service
        .search_items(&caller, search("plan", true)?)
        .await?;
    assert_eq!(
        hit_items(&everything),
        vec![item(NOTE), item(OTHER), item(THIRD)]
    );
    let asked: Vec<bool> = world
        .search
        .queries()?
        .iter()
        .map(|query| query.include_archived)
        .collect();
    assert_eq!(asked, vec![false, true]);
    Ok(())
}

#[tokio::test]
async fn draft_visibility_search_items_never_returns_a_draft() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    world
        .search
        .put_entry(&at, summary(NOTE, "notes/plan.md")?, "the committed plan")?;
    world
        .drafts
        .put_draft(draft_of(NOTE, ALICE, &at, "the secret drafted plan")?)?;
    let caller = alice(OperationName::SearchItems, None)?;
    let drafted = world
        .service
        .search_items(&caller, search("secret", false)?)
        .await?;
    assert_eq!(drafted.hits.len(), 0);
    let committed = world
        .service
        .search_items(&caller, search("plan", false)?)
        .await?;
    let hit = some(committed.hits.first(), "the committed hit")?;
    assert_eq!(hit.snippet, "the committed plan");
    assert_eq!(world.drafts.gets(), 0);
    Ok(())
}

#[tokio::test]
async fn search_items_refuses_an_empty_query_without_an_extraction_filter() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    let mut unprocessed = summary(NOTE, "sources/scan.md")?;
    unprocessed.kind = ItemKind::Source;
    unprocessed.extraction = Some(ExtractionSummary {
        status: ExtractionStatus::Failed,
        text_origin: TextOrigin::None,
        corrected: false,
    });
    world.search.put_entry(&at, unprocessed, "")?;
    world
        .search
        .put_entry(&at, summary(OTHER, "notes/plan.md")?, "plan")?;
    let caller = alice(OperationName::SearchItems, None)?;
    let refused = err_of(
        world
            .service
            .search_items(&caller, search("  ", false)?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/query"));
    assert_eq!(world.search.queries()?.len(), 0);
    let mut filtered = search("", false)?;
    filtered.extraction = Some(ExtractionFilter::Unprocessed);
    let listed = world.service.search_items(&caller, filtered).await?;
    assert_eq!(hit_items(&listed), vec![item(NOTE)]);
    Ok(())
}

#[tokio::test]
async fn search_items_refuses_a_folder_that_is_not_a_workspace_path() -> TestResult {
    let world = World::new()?;
    let mut request = search("plan", false)?;
    request.folder = Some("../outside".to_owned());
    let refused = err_of(
        world
            .service
            .search_items(&alice(OperationName::SearchItems, None)?, request)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/folder"));
    assert_eq!(world.search.queries()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn a_query_at_a_revision_the_index_has_not_built_indexes_it_and_asks_again() -> TestResult {
    let world = World::new()?;
    accepted_line(&world, &['b', 'c'])?;
    let at = revision('a')?;
    world
        .search
        .put_entry(&at, summary(NOTE, "notes/plan.md")?, "plan")?;
    world.search.put_link(&at, link(NOTE, OTHER, 3))?;
    let response = world
        .service
        .search_items(
            &alice(OperationName::SearchItems, None)?,
            search("plan", false)?,
        )
        .await?;
    assert_eq!(hit_items(&response), vec![item(NOTE)]);
    assert_eq!(world.search.indexed()?, vec![at.clone()]);
    // Asked, asked again once the workspace's build gate is held, and asked after the build.
    assert_eq!(world.search.queries()?.len(), 3);
    // Links and the graph of another unbuilt revision build it the same way.
    let older = revision('b')?;
    world.search.put_link(&older, link(NOTE, OTHER, 4))?;
    // The receipt cites the linking item, read at that revision.
    world.versions.put_document(
        &older,
        note(NOTE, &older, "notes/plan.md", "", json!({ "type": "Note" }))?,
    )?;
    let linked = world
        .service
        .get_links(
            &alice(OperationName::GetLinks, None)?,
            links(
                LinkDirection::Outgoing,
                At::Revision {
                    revision: older.clone(),
                },
            )?,
        )
        .await?;
    assert_eq!(linked.links.len(), 1);
    let oldest = revision('c')?;
    world
        .search
        .put_entry(&oldest, summary(NOTE, "notes/plan.md")?, "plan")?;
    let graphed = world
        .service
        .get_graph(
            &alice(OperationName::GetGraph, None)?,
            graph(
                None,
                10,
                At::Revision {
                    revision: oldest.clone(),
                },
            )?,
        )
        .await?;
    assert_eq!(graphed.nodes.len(), 1);
    assert_eq!(world.search.indexed()?, vec![at, older, oldest]);
    Ok(())
}

#[tokio::test]
async fn concurrent_queries_at_one_unbuilt_revision_share_one_build() -> TestResult {
    let world = World::new()?;
    accepted_line(&world, &['b'])?;
    world
        .search
        .put_entry(&revision('b')?, summary(NOTE, "notes/plan.md")?, "plan")?;
    let caller = alice(OperationName::SearchItems, None)?;
    let (first, second) = tokio::join!(
        world.service.search_items(&caller, search_at('b')?),
        world.service.search_items(&caller, search_at('b')?),
    );
    assert_eq!(hit_items(&first?), vec![item(NOTE)]);
    assert_eq!(hit_items(&second?), vec![item(NOTE)]);
    assert_eq!(
        world.search.builds(),
        1,
        "the waiting request found the build done"
    );
    Ok(())
}

#[tokio::test]
async fn a_workspace_builds_one_index_at_a_time() -> TestResult {
    let world = World::new()?;
    accepted_line(&world, &['b', 'c'])?;
    let caller = alice(OperationName::SearchItems, None)?;
    let (first, second) = tokio::join!(
        world.service.search_items(&caller, search_at('b')?),
        world.service.search_items(&caller, search_at('c')?),
    );
    first?;
    second?;
    assert_eq!(
        world.search.indexed()?,
        vec![revision('b')?, revision('c')?]
    );
    assert_eq!(world.search.most_at_once(), 1);
    Ok(())
}

#[tokio::test]
async fn a_revision_outside_the_retained_window_is_never_built() -> TestResult {
    let world = World::new()?;
    // `e` is not on the accepted line: a proposal candidate or an unknown commit.
    accepted_line(&world, &['b'])?;
    let refused = err_of(
        world
            .service
            .search_items(&alice(OperationName::SearchItems, None)?, search_at('e')?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(world.search.builds(), 0);
    assert_eq!(world.search.indexed()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn the_window_walk_follows_the_history_across_pages() -> TestResult {
    let world = World::new()?;
    accepted_line(&world, &['b', 'c', 'd', 'e'])?;
    // Two commits a page: `e` is on the third page.
    world.versions.page_log(2)?;
    world
        .service
        .search_items(&alice(OperationName::SearchItems, None)?, search_at('e')?)
        .await?;
    assert_eq!(world.search.indexed()?, vec![revision('e')?]);
    let cursors: Vec<Option<String>> = world
        .versions
        .log_queries()?
        .into_iter()
        .map(|query| query.page.cursor)
        .collect();
    assert_eq!(
        cursors,
        vec![None, Some("2".to_owned()), Some("4".to_owned())]
    );
    // `f` is not on the line: every page is read, and nothing is built.
    let refused = err_of(
        world
            .service
            .search_items(&alice(OperationName::SearchItems, None)?, search_at('f')?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(world.search.builds(), 1);
    Ok(())
}

#[tokio::test]
async fn the_retained_window_is_the_first_thousand_commits_from_the_head() -> TestResult {
    let numbered = |number: usize| Revision::try_from(format!("{number:040x}"));
    let world = World::new()?;
    // Pages of 7 commits, so the window's edge falls inside a page.
    world.versions.page_log(7)?;
    // The head `a`, then 1000 older commits numbered 1 to 1000.
    let mut commits = vec![revision('a')?];
    for number in 1..=1000 {
        commits.push(numbered(number)?);
    }
    let commits = commits
        .into_iter()
        .map(|revision| {
            Ok(Commit {
                revision,
                parents: Vec::new(),
                message: "Edit".to_owned(),
                author: ALICE.to_owned(),
                committed_at: instant()?,
            })
        })
        .collect::<Built<Vec<_>>>()?;
    world.versions.put_log(
        &revision('a')?,
        None,
        LogResponse {
            commits,
            next_cursor: None,
        },
    )?;
    let caller = alice(OperationName::SearchItems, None)?;
    let at = |revision| -> Built<SearchRequest> {
        let mut request = search("plan", false)?;
        request.at = At::Revision { revision };
        Ok(request)
    };
    // The 1000th commit from the head, the head included, is built.
    world
        .service
        .search_items(&caller, at(numbered(999)?)?)
        .await?;
    assert_eq!(world.search.indexed()?, vec![numbered(999)?]);
    // The 1001st is not.
    let refused = err_of(
        world
            .service
            .search_items(&caller, at(numbered(1000)?)?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(world.search.builds(), 1);
    Ok(())
}

// The reviewer's probes of the build gate under a failed build and under cancellation, kept as
// regression tests, over an index whose build runs on the blocking pool as storage's does.
fn detached(
    world: &World,
    millis: u64,
    fail_first: bool,
) -> Built<(ApplicationService, Arc<Counters>)> {
    let counters = Arc::new(Counters::default());
    let search = Arc::new(DetachedSearch {
        inner: world.search.clone(),
        counters: Arc::clone(&counters),
        fail_first: AtomicBool::new(fail_first),
        millis,
    });
    let unused = Arc::new(Unused);
    let ports = Ports {
        access: unused.clone(),
        mutations: unused.clone(),
        catalog: unused.clone(),
        blobs: world.blobs.clone(),
        versions: world.versions.clone(),
        records: world.records.clone(),
        queue: unused.clone(),
        search,
        converter: unused.clone(),
        proposals: unused.clone(),
        uploads: unused.clone(),
        events: world.events.clone(),
        credentials: unused.clone(),
        drafts: world.drafts.clone(),
        confirmations: unused.clone(),
        sandbox: world.sandbox.clone(),
        readiness: world.readiness.clone(),
    };
    Ok((ApplicationService::new(ports, config())?, counters))
}

/// A request waiting on a build that fails then builds the revision itself, never two at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rr_a_waiter_on_a_failed_build_builds_it_once_the_gate_is_free() -> TestResult {
    let world = World::new()?;
    accepted_line(&world, &['b'])?;
    let (service, counters) = detached(&world, 200, true)?;
    let caller = alice(OperationName::SearchItems, None)?;
    let (first, second) = tokio::join!(
        service.search_items(&caller, search_at('b')?),
        service.search_items(&caller, search_at('b')?),
    );
    let failed = err_of(first)?;
    assert_eq!(failed.code, ErrorCode::Internal);
    second?;
    assert_eq!(counters.started.load(Ordering::SeqCst), 2);
    assert_eq!(counters.most.load(Ordering::SeqCst), 1);
    Ok(())
}

/// A request cancelled while it waits on the gate leaves the build and the queue intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rr_cancelling_a_waiting_request_leaves_one_build() -> TestResult {
    let world = World::new()?;
    accepted_line(&world, &['b'])?;
    let (service, counters) = detached(&world, 300, false)?;
    let caller = alice(OperationName::SearchItems, None)?;
    let (first, waiting) = tokio::join!(
        service.search_items(&caller, search_at('b')?),
        tokio::time::timeout(
            Duration::from_millis(50),
            service.search_items(&caller, search_at('b')?)
        ),
    );
    first?;
    assert!(waiting.is_err(), "the waiting request was cancelled");
    service.search_items(&caller, search_at('b')?).await?;
    assert_eq!(counters.started.load(Ordering::SeqCst), 1);
    assert_eq!(counters.most.load(Ordering::SeqCst), 1);
    Ok(())
}

/// A request cancelled while it builds must keep the workspace's gate until the build it
/// started ends: storage's build keeps running after the request is dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rr_cancelling_a_building_request_keeps_one_build_at_a_time() -> TestResult {
    let world = World::new()?;
    accepted_line(&world, &['b', 'c'])?;
    let (service, counters) = detached(&world, 400, false)?;
    let caller = alice(OperationName::SearchItems, None)?;
    let cancelled = tokio::time::timeout(
        Duration::from_millis(100),
        service.search_items(&caller, search_at('b')?),
    )
    .await;
    assert!(
        cancelled.is_err(),
        "the first request was cancelled mid-build"
    );
    service.search_items(&caller, search_at('c')?).await?;
    assert_eq!(
        counters.most.load(Ordering::SeqCst),
        1,
        "two builds of one workspace ran at once after a cancelled request"
    );
    Ok(())
}

#[tokio::test]
async fn navigation_reads_are_purge_checked_before_the_index_is_asked() -> TestResult {
    let world = World::new()?;
    let purged = revision('c')?;
    let purge_id = PurgeId(Uuid::from_u128(5));
    world.records.put_mapping(
        purged.clone(),
        RevisionMapping {
            purge_id,
            replacement: None,
        },
    )?;
    let at = At::Revision { revision: purged };
    let mut request = search("plan", false)?;
    request.at = at.clone();
    let refusals = [
        err_of(
            world
                .service
                .search_items(&alice(OperationName::SearchItems, None)?, request)
                .await,
        )?,
        err_of(
            world
                .service
                .get_links(
                    &alice(OperationName::GetLinks, None)?,
                    links(LinkDirection::Both, at.clone())?,
                )
                .await,
        )?,
        err_of(
            world
                .service
                .get_graph(&alice(OperationName::GetGraph, None)?, graph(None, 10, at)?)
                .await,
        )?,
    ];
    for refused in refusals {
        assert_eq!(refused.code, ErrorCode::NotFound);
        assert_eq!(
            refused.detail.as_deref(),
            Some(&ErrorDetail::Invalidated {
                purge_id,
                replacement: None
            })
        );
    }
    assert_eq!(world.search.queries()?.len(), 0);
    assert_eq!(world.search.link_queries()?.len(), 0);
    assert_eq!(world.search.graph_queries()?.len(), 0);
    assert_eq!(world.search.indexed()?.len(), 0);
    assert_eq!(
        world.records.receipts()?.len(),
        0,
        "a refused read records none"
    );
    Ok(())
}

#[tokio::test]
async fn get_links_reads_one_direction_of_one_item_at_the_resolved_revision() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    world.search.put_link(&at, link(NOTE, OTHER, 3))?;
    world.search.put_link(&at, link(THIRD, NOTE, 7))?;
    linking_items(&world, &at)?;
    let caller = alice(OperationName::GetLinks, None)?;
    let mut lines = Vec::new();
    for direction in [
        LinkDirection::Outgoing,
        LinkDirection::Incoming,
        LinkDirection::Both,
    ] {
        let response = world
            .service
            .get_links(&caller, links(direction, At::Latest)?)
            .await?;
        assert_eq!(response.revision, at);
        lines.push(
            response
                .links
                .iter()
                .map(|link| link.line)
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(lines, vec![vec![3], vec![7], vec![3, 7]]);
    let queries = world.search.link_queries()?;
    let query = some(queries.first(), "the link query")?;
    assert_eq!(query.revision, at);
    assert_eq!(query.item_id, item(NOTE));
    Ok(())
}

#[tokio::test]
async fn a_search_continues_at_the_revision_its_first_page_resolved() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    for (id, location) in [
        (NOTE, "notes/a.md"),
        (OTHER, "notes/b.md"),
        (THIRD, "notes/c.md"),
    ] {
        world
            .search
            .put_entry(&at, summary(id, location)?, "the plan")?;
    }
    let page = |at: At, cursor: Option<String>| -> Built<SearchRequest> {
        let mut request = search("plan", false)?;
        request.at = at;
        request.page = PageRequest { cursor, limit: 2 };
        Ok(request)
    };
    let caller = alice(OperationName::SearchItems, None)?;
    let first = world
        .service
        .search_items(&caller, page(At::Latest, None)?)
        .await?;
    let cursor = some(first.next_cursor.clone(), "a continuation")?;
    // A write lands: `latest` is `b` now, which the index has not built.
    world.versions.set_head(revision('b')?)?;
    let second = world
        .service
        .search_items(&caller, page(At::Latest, Some(cursor.clone()))?)
        .await?;
    assert_eq!(second.revision, at);
    assert_eq!(second.next_cursor, None);
    let mut hits = hit_items(&first);
    hits.extend(hit_items(&second));
    assert_eq!(
        hits,
        vec![item(NOTE), item(OTHER), item(THIRD)],
        "each hit once"
    );
    let asked: Vec<_> = world
        .search
        .queries()?
        .into_iter()
        .map(|query| (query.revision, query.page.cursor, query.page.limit))
        .collect();
    assert_eq!(
        asked,
        vec![(at.clone(), None, 2), (at, Some("2".to_owned()), 2)]
    );
    assert_eq!(world.search.builds(), 0, "`b` was never asked for");
    // A search cursor is no cursor of an item's links.
    let mut request = links(LinkDirection::Both, At::Latest)?;
    request.page.cursor = Some(cursor.clone());
    let as_links = alice(OperationName::GetLinks, None)?;
    let elsewhere = err_of(world.service.get_links(&as_links, request).await)?;
    assert_eq!(elsewhere.code, ErrorCode::InvalidInput);
    assert_eq!(elsewhere.field.as_deref(), Some("/page/cursor"));
    assert_eq!(world.search.link_queries()?.len(), 0);
    // A cursor this operation did not issue, and one that continues another revision's search.
    let named_b = At::Revision {
        revision: revision('b')?,
    };
    for (at, text) in [(At::Latest, "2".to_owned()), (named_b, cursor)] {
        let refused = err_of(
            world
                .service
                .search_items(&caller, page(at, Some(text))?)
                .await,
        )?;
        assert_eq!(refused.code, ErrorCode::InvalidInput);
        assert_eq!(refused.field.as_deref(), Some("/page/cursor"));
    }
    assert_eq!(world.search.queries()?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_search_of_three_pages_reads_every_page_at_the_revision_the_first_resolved() -> TestResult
{
    // The cursor a continuation returns carries the first page's revision too: a write lands
    // after each page, and the index has built no later head.
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    for id in 20..25_u128 {
        world
            .search
            .put_entry(&at, summary(id, &format!("notes/{id}.md"))?, "the plan")?;
    }
    let mut hits = Vec::new();
    let mut cursor = None;
    for landing in ['b', 'c', 'd'] {
        let mut request = search("plan", false)?;
        request.page = PageRequest {
            cursor: cursor.take(),
            limit: 2,
        };
        let found = world
            .service
            .search_items(&alice(OperationName::SearchItems, None)?, request)
            .await?;
        assert_eq!(found.revision, at);
        hits.extend(hit_items(&found));
        cursor = found.next_cursor;
        world.versions.set_head(revision(landing)?)?;
    }
    assert_eq!(cursor, None, "three pages");
    let expected: Vec<ItemId> = (20..25_u128).map(item).collect();
    assert_eq!(hits, expected, "each hit once, over three pages");
    assert_eq!(world.search.builds(), 0, "no later head was asked for");
    Ok(())
}

#[tokio::test]
async fn an_items_links_of_three_pages_read_every_page_at_the_revision_the_first_resolved()
-> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    for line in 1..=5_u32 {
        world.search.put_link(&at, link(NOTE, OTHER, line))?;
    }
    linking_items(&world, &at)?;
    let mut lines = Vec::new();
    let mut cursor = None;
    for landing in ['b', 'c', 'd'] {
        let mut request = links(LinkDirection::Outgoing, At::Latest)?;
        request.page = PageRequest {
            cursor: cursor.take(),
            limit: 2,
        };
        let linked = world
            .service
            .get_links(&alice(OperationName::GetLinks, None)?, request)
            .await?;
        assert_eq!(linked.revision, at);
        lines.extend(linked.links.iter().map(|link| link.line));
        cursor = linked.next_cursor;
        world.versions.set_head(revision(landing)?)?;
    }
    assert_eq!(cursor, None, "three pages");
    assert_eq!(
        lines,
        vec![1, 2, 3, 4, 5],
        "each link once, over three pages"
    );
    assert_eq!(world.search.builds(), 0, "no later head was asked for");
    Ok(())
}

#[tokio::test]
async fn an_items_links_continue_at_the_revision_their_first_page_resolved() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    world.search.put_link(&at, link(NOTE, OTHER, 3))?;
    world.search.put_link(&at, link(THIRD, NOTE, 7))?;
    world.search.put_link(&at, link(NOTE, THIRD, 2))?;
    linking_items(&world, &at)?;
    let page = |at: At, cursor: Option<String>| -> Built<GetLinksRequest> {
        let mut request = links(LinkDirection::Both, at)?;
        request.page = PageRequest { cursor, limit: 2 };
        Ok(request)
    };
    let caller = alice(OperationName::GetLinks, None)?;
    let first = world
        .service
        .get_links(&caller, page(At::Latest, None)?)
        .await?;
    let cursor = some(first.next_cursor, "a continuation")?;
    // A write lands: `latest` is `b` now, which the index has not built.
    world.versions.set_head(revision('b')?)?;
    let second = world
        .service
        .get_links(&caller, page(At::Latest, Some(cursor.clone()))?)
        .await?;
    assert_eq!(second.revision, at);
    assert_eq!(second.next_cursor, None);
    let lines: Vec<u32> = first
        .links
        .iter()
        .chain(&second.links)
        .map(|link| link.line)
        .collect();
    assert_eq!(lines, vec![3, 7, 2], "each link once");
    let asked: Vec<_> = world
        .search
        .link_queries()?
        .into_iter()
        .map(|query| (query.revision, query.page.cursor, query.page.limit))
        .collect();
    assert_eq!(
        asked,
        vec![(at.clone(), None, 2), (at, Some("2".to_owned()), 2)]
    );
    // A links cursor is no search cursor.
    let mut request = search("plan", false)?;
    request.page.cursor = Some(cursor.clone());
    let as_search = alice(OperationName::SearchItems, None)?;
    let elsewhere = err_of(world.service.search_items(&as_search, request).await)?;
    assert_eq!(elsewhere.code, ErrorCode::InvalidInput);
    assert_eq!(elsewhere.field.as_deref(), Some("/page/cursor"));
    assert_eq!(world.search.queries()?.len(), 0);
    // A cursor this operation did not issue, and one that continues another revision's links.
    let named_b = At::Revision {
        revision: revision('b')?,
    };
    for (at, text) in [(At::Latest, "2".to_owned()), (named_b, cursor)] {
        let refused = err_of(
            world
                .service
                .get_links(&caller, page(at, Some(text))?)
                .await,
        )?;
        assert_eq!(refused.code, ErrorCode::InvalidInput);
        assert_eq!(refused.field.as_deref(), Some("/page/cursor"));
    }
    assert_eq!(world.search.link_queries()?.len(), 2);
    Ok(())
}

/// NOTE at `notes/plan.md`, with frontmatter of its type and two more properties, and THIRD at
/// `notes/third.md`, both readable at `at`: the items the fixture links come from.
fn linking_items(world: &World, at: &Revision) -> Built<()> {
    world.versions.put_document(
        at,
        note(
            NOTE,
            at,
            "notes/plan.md",
            "# Plan\nfirst\nsee [other](21.md)\n",
            json!({ "type": "Note", "author": "Ana", "tags": "plans" }),
        )?,
    )?;
    world.versions.put_document(
        at,
        note(THIRD, at, "notes/third.md", "", json!({ "type": "Note" }))?,
    )?;
    Ok(())
}

/// The item, revision and selection of each source of the only receipt.
fn cited(world: &World) -> Built<Vec<(ItemId, Revision, Selection)>> {
    let receipts = world.records.receipts()?;
    assert_eq!(receipts.len(), 1, "one receipt");
    let (under, receipt) = some(receipts.first(), "the receipt")?;
    assert_eq!(*under, None, "a read records under no mutation");
    Ok(receipt
        .sources
        .iter()
        .map(|source| {
            (
                source.item_id,
                source.revision.clone(),
                source.selection.clone(),
            )
        })
        .collect())
}

fn line(at: u32) -> Selection {
    Selection::Lines {
        range: TextRange { start: at, end: at },
    }
}

#[tokio::test]
async fn get_links_records_one_reference_per_link_by_its_body_line() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    world.search.put_link(&at, link(NOTE, OTHER, 3))?;
    world.search.put_link(&at, link(THIRD, NOTE, 7))?;
    // A broken link names no item; it still cites the item it is written in.
    world.search.put_link(
        &at,
        Link {
            from: item(NOTE),
            to_path: "notes/missing.md".to_owned(),
            to: None,
            label: "gone".to_owned(),
            line: 2,
        },
    )?;
    linking_items(&world, &at)?;
    let response = world
        .service
        .get_links(
            &alice(OperationName::GetLinks, None)?,
            links(LinkDirection::Both, At::Latest)?,
        )
        .await?;
    assert_eq!(response.links.len(), 3);
    // NOTE has frontmatter; the index numbers its body, so line 3 is cited as body line 3.
    assert_eq!(
        cited(&world)?,
        vec![
            (item(NOTE), at.clone(), line(3)),
            (item(THIRD), at.clone(), line(7)),
            (item(NOTE), at, line(2)),
        ]
    );
    let receipts = world.records.receipts()?;
    let (_, receipt) = some(receipts.first(), "the receipt")?;
    assert_eq!(receipt.operation_id, OperationName::GetLinks);
    assert_eq!(receipt.audience, ReceiptAudience::HumanDisplay);
    Ok(())
}

#[tokio::test]
async fn a_link_cited_from_a_source_card_carries_the_cards_computed_locations() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    // The linking item is a source card showing its converter's text (`CARD_BODY`), whose
    // record puts lines 1 to 3 on page 1 and lines 5 to 8 on page 2.
    world.versions.put_document(
        &at,
        source_card(
            CARD,
            &at,
            "sources/report-pdf.md",
            CARD_BODY,
            converted(&digest('b')?),
        )?,
    )?;
    world
        .blobs
        .put_object(digest('b')?, serde_json::to_vec(&record(Vec::new())?)?)?;
    world.search.put_link(&at, link(CARD, NOTE, 6))?;
    world.search.put_link(&at, link(CARD, NOTE, 2))?;
    // Line 4 is the blank line between the two pages: the record locates nothing there.
    world.search.put_link(&at, link(CARD, NOTE, 4))?;
    let response = world
        .service
        .get_links(
            &alice(OperationName::GetLinks, None)?,
            links(LinkDirection::Incoming, At::Latest)?,
        )
        .await?;
    assert_eq!(response.links.len(), 3);
    let on_page = |page_no: u32| SourceLocation::Direct {
        locator: SourceLocator::Page { page_no },
    };
    let receipts = world.records.receipts()?;
    let (_, receipt) = some(receipts.first(), "the receipt")?;
    let cited: Vec<_> = receipt
        .sources
        .iter()
        .map(|source| {
            (
                source.item_id,
                source.digest.clone(),
                source.selection.clone(),
                source.locations.clone(),
            )
        })
        .collect();
    // Each link cites the card by its conversion record, located as a read of that line is.
    let record_digest = Some(digest('b')?);
    assert_eq!(
        cited,
        vec![
            (item(CARD), record_digest.clone(), line(6), vec![on_page(2)]),
            (item(CARD), record_digest.clone(), line(2), vec![on_page(1)]),
            (item(CARD), record_digest, line(4), Vec::new()),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn get_links_whose_linking_item_cannot_be_read_fails_and_records_no_receipt() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    world.search.put_link(&at, link(NOTE, OTHER, 3))?;
    linking_items(&world, &at)?;
    world.versions.fail_next_show(ErrorCode::Unavailable)?;
    let failed = err_of(
        world
            .service
            .get_links(
                &alice(OperationName::GetLinks, None)?,
                links(LinkDirection::Outgoing, At::Latest)?,
            )
            .await,
    )?;
    assert_eq!(failed.code, ErrorCode::Unavailable);
    assert_eq!(world.records.receipts()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn get_graph_reads_a_bounded_graph_of_one_folder_and_refuses_no_room() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    world
        .search
        .put_entry(&at, summary(NOTE, "notes/plan.md")?, "a")?;
    world
        .search
        .put_entry(&at, summary(OTHER, "notes/b.md")?, "b")?;
    world
        .search
        .put_entry(&at, summary(THIRD, "elsewhere/c.md")?, "c")?;
    world.search.put_link(&at, link(NOTE, OTHER, 1))?;
    linking_items(&world, &at)?;
    let caller = alice(OperationName::GetGraph, None)?;
    let whole = world
        .service
        .get_graph(&caller, graph(Some("notes"), 10, At::Latest)?)
        .await?;
    assert_eq!(whole.revision, at);
    assert_eq!(whole.nodes.len(), 2);
    assert_eq!(whole.edges.len(), 1);
    assert!(!whole.truncated);
    // Its receipt cites each node whole, as a search hit cites an item, and the edge, which
    // carries a label and a source line, as a link: its linking item by that body line.
    assert_eq!(
        cited(&world)?,
        vec![
            (item(NOTE), at.clone(), Selection::All),
            (item(OTHER), at.clone(), Selection::All),
            (item(NOTE), at.clone(), line(1)),
        ]
    );
    let receipts = world.records.receipts()?;
    let (_, receipt) = some(receipts.first(), "the graph's receipt")?;
    assert_eq!(receipt.operation_id, OperationName::GetGraph);
    let paths: Vec<&str> = receipt
        .sources
        .iter()
        .map(|source| source.path.as_str())
        .collect();
    assert_eq!(paths, vec!["notes/plan.md", "notes/b.md", "notes/plan.md"]);
    let cut = world
        .service
        .get_graph(&caller, graph(Some("notes"), 1, At::Latest)?)
        .await?;
    assert_eq!(cut.nodes.len(), 1);
    assert!(cut.truncated);
    let queries = world.search.graph_queries()?;
    let query = some(queries.first(), "the graph query")?;
    assert_eq!(query.folder, Some(path("notes")?));
    assert_eq!(query.max_nodes, 10);
    let refused = err_of(
        world
            .service
            .get_graph(&caller, graph(None, 0, At::Latest)?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/max_nodes"));
    assert_eq!(world.search.graph_queries()?.len(), 2);
    Ok(())
}
