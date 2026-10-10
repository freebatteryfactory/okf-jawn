//! Search, links and the graph (SPEC section 7) over the derived index behind `SearchIndex`.
//!
//! The application owns what the index does not: the caller's workspace scope (dispatch has
//! authorized it), resolving the selector once (a named revision is purge-checked first), the
//! request's folder as a workspace path, and coordinating the index with content. A write
//! indexes the revision it produced (`shared::announce`). A query at a revision the index has
//! not built yet (it answers `Unavailable`) builds that revision's index on demand, bounded:
//!
//! - A workspace builds at most one index at a time: the build holds the workspace's gate
//!   (`ApplicationService::index_gate`), and a request that waited on it asks the index again
//!   before building, so concurrent requests for one revision share one build.
//! - Only a revision inside the retained window is built: one of the first `INDEX_WINDOW`
//!   commits of the accepted line's history from the head, the head included
//!   (`VersionStore::log` from the head), after the purge check `shared::resolve` already
//!   made. Any other revision, such as a
//!   proposal candidate, an unknown commit or one older than the window, keeps the index's
//!   `Unavailable` and nothing is built.
//!
//! Indexing a revision is idempotent, so the query is asked once more after the build.
//!
//! The index never holds a draft, so nothing here reads one. Deprecated items are always
//! searchable: the query carries no status filter. Archived items are searched only when the
//! request includes them (`SearchQuery::include_archived`, which the index applies). A query may
//! be empty only with an extraction filter (`SearchQuery::check`, the contract's
//! `require_text_or_filter`). The hits a search returns are recorded in a receipt, as every read
//! of content is.

use std::future::Future;

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Revision, WorkspacePath};
use okf_jawn_contract::search::{
    GetGraphRequest, GetGraphResponse, GetLinksRequest, GetLinksResponse, SearchRequest,
    SearchResponse,
};

use super::ApplicationService;
use super::shared::{invalid, record_receipt, resolve, workspace_scope};
use crate::context::OperationContext;
use crate::search::{GraphQuery, LinkQuery, SearchQuery};
use crate::storage::{LogQuery, Page, StorageScope};

/// How many commits of the accepted line, from the head and the head included, an on-demand
/// index build may reach.
const INDEX_WINDOW: usize = 1000;
/// Commits asked per page of the history walk that decides whether a revision is in the window.
const WINDOW_PAGE: u16 = 250;

/// Search one resolved revision and return cited snippets.
///
/// # Errors
/// Returns `InvalidInput` on `/folder` for a folder that is not a workspace path, on `/query`
/// for a blank query without an extraction filter; the typed `NotFound` of an invalidated
/// revision; or any port error.
pub(super) async fn search_items(
    service: &ApplicationService,
    context: &OperationContext,
    request: SearchRequest,
) -> Result<SearchResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let folder = folder_of(request.folder)?;
    let revision = resolve(service, &scope, &request.at).await?;
    let query = SearchQuery {
        revision: revision.clone(),
        text: request.query,
        folder,
        include_archived: request.include_archived,
        extraction: request.extraction,
        page: request.page.into(),
    };
    query.check().map_err(|error| error.with_field("/query"))?;
    let index = &service.ports().search;
    let response = indexed(service, &scope, &revision, || {
        index.search(&scope, query.clone())
    })
    .await?;
    let returned = response.hits.iter().map(|hit| hit.source.clone()).collect();
    record_receipt(service, context, &scope, None, returned).await?;
    Ok(SearchResponse {
        revision,
        ..response
    })
}

/// One item's outgoing links, backlinks or both at one resolved revision.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated revision, or any port error.
pub(super) async fn get_links(
    service: &ApplicationService,
    context: &OperationContext,
    request: GetLinksRequest,
) -> Result<GetLinksResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let revision = resolve(service, &scope, &request.at).await?;
    let query = LinkQuery {
        revision: revision.clone(),
        item_id: request.item_id,
        direction: request.direction,
        page: request.page.into(),
    };
    let index = &service.ports().search;
    let response = indexed(service, &scope, &revision, || {
        index.links(&scope, query.clone())
    })
    .await?;
    Ok(GetLinksResponse {
        revision,
        ..response
    })
}

/// A bounded graph of one resolved revision, optionally of one folder.
///
/// # Errors
/// Returns `InvalidInput` on `/max_nodes` for a limit of zero, on `/folder` for a folder that is
/// not a workspace path; the typed `NotFound` of an invalidated revision; or any port error.
pub(super) async fn get_graph(
    service: &ApplicationService,
    context: &OperationContext,
    request: GetGraphRequest,
) -> Result<GetGraphResponse, ApiError> {
    if request.max_nodes == 0 {
        return Err(invalid(
            "a graph needs room for at least one node",
            "/max_nodes",
        ));
    }
    let scope = workspace_scope(context)?;
    let folder = folder_of(request.folder)?;
    let revision = resolve(service, &scope, &request.at).await?;
    let query = GraphQuery {
        revision: revision.clone(),
        folder,
        max_nodes: request.max_nodes,
    };
    let index = &service.ports().search;
    let response = indexed(service, &scope, &revision, || {
        index.graph(&scope, query.clone())
    })
    .await?;
    Ok(GetGraphResponse {
        revision,
        ..response
    })
}

/// The request's folder as a workspace path; absent or empty is the whole workspace.
fn folder_of(folder: Option<String>) -> Result<Option<WorkspacePath>, ApiError> {
    match folder {
        None => Ok(None),
        Some(folder) if folder.is_empty() => Ok(None),
        Some(folder) => WorkspacePath::try_from(folder)
            .map(Some)
            .map_err(|error| invalid(error.0, "/folder")),
    }
}

/// Answer `query` from the index; when the index has not built `revision` yet and it lies in
/// the retained window, build it under the workspace's gate and ask once more, as the module
/// header says.
async fn indexed<T, Answer>(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
    query: impl Fn() -> Answer,
) -> Result<T, ApiError>
where
    Answer: Future<Output = Result<T, ApiError>>,
{
    match query().await {
        Err(error) if error.code == ErrorCode::Unavailable => {
            let gate = service.index_gate(scope)?;
            let _building = gate.lock().await;
            // A build another request ran while this one waited may have built it.
            match query().await {
                Err(unavailable) if unavailable.code == ErrorCode::Unavailable => {
                    if !in_window(service, scope, revision).await? {
                        return Err(unavailable);
                    }
                    service
                        .ports()
                        .search
                        .index_revision(scope, revision.clone())
                        .await?;
                    query().await
                }
                answer => answer,
            }
        }
        answer => answer,
    }
}

/// Whether `revision` is one of the first `INDEX_WINDOW` commits of the accepted line's history
/// from the head, the head included. Costs at most `INDEX_WINDOW / WINDOW_PAGE` pages of history
/// from the head.
async fn in_window(
    service: &ApplicationService,
    scope: &StorageScope,
    revision: &Revision,
) -> Result<bool, ApiError> {
    let versions = &service.ports().versions;
    let head = versions.head(scope).await?;
    if &head == revision {
        return Ok(true);
    }
    let mut cursor = None;
    let mut walked = 0_usize;
    while walked < INDEX_WINDOW {
        let page = versions
            .log(
                scope,
                LogQuery {
                    tip: head.clone(),
                    item_id: None,
                    page: Page {
                        cursor,
                        limit: WINDOW_PAGE,
                    },
                },
            )
            .await?;
        if page.commits.is_empty() {
            return Ok(false);
        }
        let room = INDEX_WINDOW.saturating_sub(walked);
        if page
            .commits
            .iter()
            .take(room)
            .any(|commit| &commit.revision == revision)
        {
            return Ok(true);
        }
        walked = walked.saturating_add(page.commits.len());
        let Some(next) = page.next_cursor else {
            return Ok(false);
        };
        cursor = Some(next);
    }
    Ok(false)
}
