//! Search, links and the graph: one resolved, purge-checked revision of the derived index,
//! built on demand; drafts never reach a result; archived items only when asked, deprecated
//! items always.

use okf_jawn_contract::common::PageRequest;
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::extraction::{
    ExtractionFilter, ExtractionStatus, ExtractionSummary, TextOrigin,
};
use okf_jawn_contract::history::{Commit, LogResponse};
use okf_jawn_contract::identity::{At, ItemId, PurgeId, Revision};
use okf_jawn_contract::item::{ItemKind, ItemStatus, ItemSummary};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_contract::search::{
    GetGraphRequest, GetLinksRequest, Link, LinkDirection, SearchRequest,
};
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::ports::Application;
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{
    ALICE, Built, NOTE, World, alice, draft_of, instant, note, path, revision, scope,
};

const OTHER: u128 = 21;
const THIRD: u128 = 22;

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
async fn the_retained_window_is_the_first_thousand_commits_from_the_head() -> TestResult {
    let numbered = |number: usize| Revision::try_from(format!("{number:040x}"));
    let world = World::new()?;
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
    Ok(())
}

#[tokio::test]
async fn get_links_reads_one_direction_of_one_item_at_the_resolved_revision() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    world.search.put_link(&at, link(NOTE, OTHER, 3))?;
    world.search.put_link(&at, link(THIRD, NOTE, 7))?;
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
async fn get_graph_reads_a_bounded_graph_of_one_folder_and_refuses_no_room() -> TestResult {
    let world = World::new()?;
    let at = revision('a')?;
    world.search.mark_indexed(&at)?;
    world
        .search
        .put_entry(&at, summary(NOTE, "notes/a.md")?, "a")?;
    world
        .search
        .put_entry(&at, summary(OTHER, "notes/b.md")?, "b")?;
    world
        .search
        .put_entry(&at, summary(THIRD, "elsewhere/c.md")?, "c")?;
    world.search.put_link(&at, link(NOTE, OTHER, 1))?;
    let caller = alice(OperationName::GetGraph, None)?;
    let whole = world
        .service
        .get_graph(&caller, graph(Some("notes"), 10, At::Latest)?)
        .await?;
    assert_eq!(whole.revision, at);
    assert_eq!(whole.nodes.len(), 2);
    assert_eq!(whole.edges.len(), 1);
    assert!(!whole.truncated);
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
