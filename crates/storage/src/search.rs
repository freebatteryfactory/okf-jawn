//! The search, link and graph index (`SearchIndex`) in its own SQLite file with FTS5.
//!
//! The index is derived from committed Git trees only: drafts live in the records database and
//! are never read here. Each indexed revision keeps one projection of its items (summary,
//! archived flag, extraction status, text) and links. `rebuild` discards a workspace's rows in
//! this file and indexes the head again; jobs, reviews and receipts are in the records file,
//! which this module never opens. A query's text is bound as an FTS5 phrase per word, never
//! spliced into SQL.

use std::collections::BTreeMap;

use okf_core::ConceptId;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::ExtractionFilter;
use okf_jawn_contract::identity::{ItemId, Revision, WorkspacePath};
use okf_jawn_contract::item::ItemSummary;
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::search::{
    GetGraphResponse, GetLinksResponse, Link, LinkDirection, SearchHit, SearchResponse,
};
use okf_jawn_contract::source::SourceReference;
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::search::{GraphQuery, LinkQuery, SearchIndex, SearchQuery};
use okf_jawn_core::storage::StorageScope;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::db::{Db, json, scope_key, sql};
use crate::git::repo::{Repositories, commit_of, git};
use crate::git::{ItemFile, appearance_of, items_of};

/// `SearchIndex` over the index database and the workspace repositories.
#[derive(Debug, Clone)]
pub struct SqliteSearch {
    index: Db,
    repositories: Repositories,
}

/// One item of a revision, ready to index.
struct Entry {
    summary: ItemSummary,
    body: String,
    links: Vec<Link>,
}

/// item id, summary JSON, snippet, rank: one search row.
type HitColumns = (String, String, String, f64);

impl SqliteSearch {
    pub(crate) const fn new(index: Db, repositories: Repositories) -> Self {
        Self {
            index,
            repositories,
        }
    }

    async fn ensure(&self, scope: &StorageScope, revision: Revision) -> Result<(), ApiError> {
        let (tenant, workspace) = scope_key(scope);
        let wanted = revision.clone();
        let indexed = self
            .index
            .call(move |connection| indexed(connection, &tenant, &workspace, &wanted))
            .await?;
        if indexed {
            return Ok(());
        }
        let repositories = self.repositories.clone();
        let owned = scope.clone();
        let read = revision.clone();
        let entries = tokio::task::spawn_blocking(move || entries(&repositories, &owned, &read))
            .await
            .map_err(|error| crate::db::internal(format!("an index task stopped: {error}")))??;
        let (tenant, workspace) = scope_key(scope);
        self.index
            .transaction(move |transaction| {
                store(transaction, &tenant, &workspace, &revision, &entries)
            })
            .await
    }
}

impl SearchIndex for SqliteSearch {
    fn index_revision<'a>(
        &'a self,
        scope: &'a StorageScope,
        revision: Revision,
    ) -> PortFuture<'a, ()> {
        Box::pin(self.ensure(scope, revision))
    }

    fn search<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: SearchQuery,
    ) -> PortFuture<'a, SearchResponse> {
        let (tenant, workspace) = scope_key(scope);
        let workspace_id = scope.workspace_id;
        Box::pin(self.index.call(move |connection| {
            query.check()?;
            require_indexed(connection, &tenant, &workspace, &query.revision)?;
            let rows = matching(connection, &tenant, &workspace, &query)?;
            let start = offset(query.page.cursor.as_deref())?;
            let limit = usize::from(query.page.limit.clamp(1, 500));
            let mut hits = Vec::new();
            let mut total = 0_usize;
            for (_, summary, snippet, rank) in rows {
                let summary: ItemSummary =
                    serde_json::from_str(&summary).map_err(|error| json(&error))?;
                if !query.include_archived && summary.archived {
                    continue;
                }
                if query.extraction == Some(ExtractionFilter::Unprocessed)
                    && !summary
                        .extraction
                        .as_ref()
                        .is_some_and(|extraction| extraction.status.is_unprocessed())
                {
                    continue;
                }
                if let Some(folder) = &query.folder
                    && !summary
                        .path
                        .as_str()
                        .starts_with(&format!("{}/", folder.as_str()))
                {
                    continue;
                }
                if total >= start && hits.len() < limit {
                    hits.push(hit(workspace_id, &query.revision, summary, snippet, rank));
                }
                total = total.saturating_add(1);
            }
            let end = start.saturating_add(hits.len());
            Ok(SearchResponse {
                revision: query.revision,
                hits,
                next_cursor: (end < total).then(|| end.to_string()),
            })
        }))
    }

    fn links<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: LinkQuery,
    ) -> PortFuture<'a, GetLinksResponse> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.index.call(move |connection| {
            require_indexed(connection, &tenant, &workspace, &query.revision)?;
            let item = query.item_id.0.to_string();
            let mut links = Vec::new();
            if matches!(
                query.direction,
                LinkDirection::Outgoing | LinkDirection::Both
            ) {
                links.extend(read_links(
                    connection,
                    &tenant,
                    &workspace,
                    &query.revision,
                    "source_item",
                    &item,
                )?);
            }
            if matches!(
                query.direction,
                LinkDirection::Incoming | LinkDirection::Both
            ) {
                links.extend(read_links(
                    connection,
                    &tenant,
                    &workspace,
                    &query.revision,
                    "target_item",
                    &item,
                )?);
            }
            let start = offset(query.page.cursor.as_deref())?;
            let limit = usize::from(query.page.limit.clamp(1, 500));
            let total = links.len();
            let page: Vec<Link> = links.into_iter().skip(start).take(limit).collect();
            let end = start.saturating_add(page.len());
            Ok(GetLinksResponse {
                revision: query.revision,
                links: page,
                next_cursor: (end < total).then(|| end.to_string()),
            })
        }))
    }

    fn graph<'a>(
        &'a self,
        scope: &'a StorageScope,
        query: GraphQuery,
    ) -> PortFuture<'a, GetGraphResponse> {
        let (tenant, workspace) = scope_key(scope);
        Box::pin(self.index.call(move |connection| {
            require_indexed(connection, &tenant, &workspace, &query.revision)?;
            let mut statement = connection
                .prepare(
                    "SELECT summary FROM index_items
                     WHERE tenant_id = ?1 AND workspace_id = ?2 AND revision = ?3 ORDER BY path",
                )
                .map_err(|error| sql(&error))?;
            let summaries = statement
                .query_map(params![tenant, workspace, query.revision.as_str()], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|error| sql(&error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| sql(&error))?;
            let limit = usize::try_from(query.max_nodes).unwrap_or(usize::MAX);
            let mut nodes = Vec::new();
            let mut truncated = false;
            for summary in summaries {
                let summary: ItemSummary =
                    serde_json::from_str(&summary).map_err(|error| json(&error))?;
                let inside = query.folder.as_ref().is_none_or(|folder| {
                    summary
                        .path
                        .as_str()
                        .starts_with(&format!("{}/", folder.as_str()))
                });
                if !inside {
                    continue;
                }
                if nodes.len() == limit {
                    truncated = true;
                    break;
                }
                nodes.push(summary);
            }
            let ids: Vec<ItemId> = nodes.iter().map(|node| node.id).collect();
            let mut edges = Vec::new();
            for id in &ids {
                edges.extend(
                    read_links(
                        connection,
                        &tenant,
                        &workspace,
                        &query.revision,
                        "source_item",
                        &id.0.to_string(),
                    )?
                    .into_iter()
                    .filter(|link| link.to.is_some_and(|to| ids.contains(&to))),
                );
            }
            Ok(GetGraphResponse {
                revision: query.revision,
                nodes,
                edges,
                truncated,
            })
        }))
    }

    fn rebuild<'a>(&'a self, scope: &'a StorageScope, head: Revision) -> PortFuture<'a, ()> {
        Box::pin(async move {
            let (tenant, workspace) = scope_key(scope);
            self.index
                .transaction(move |transaction| discard(transaction, &tenant, &workspace))
                .await?;
            self.ensure(scope, head).await
        })
    }
}

/// The projection of every item of a revision.
fn entries(
    repositories: &Repositories,
    scope: &StorageScope,
    revision: &Revision,
) -> Result<Vec<Entry>, ApiError> {
    let repository = repositories.open(scope)?;
    let tree = commit_of(&repository, revision)?
        .tree()
        .map_err(|error| git(&error))?;
    let files: Vec<(WorkspacePath, ItemFile)> = items_of(&repository, &tree)?;
    let by_path: BTreeMap<String, ItemId> = files
        .iter()
        .map(|(path, file)| (path.as_str().to_owned(), file.header.item_id))
        .collect();
    let mut found = Vec::new();
    for (path, file) in &files {
        let appearance = appearance_of(&repository, &tree, file.header.item_id)?;
        let summary = file.summary(path, revision, appearance.as_ref());
        let links = links_of(path, file, &by_path);
        found.push(Entry {
            summary,
            body: file.document.body.clone(),
            links,
        });
    }
    Ok(found)
}

/// The Markdown links of one item, each on its source line, resolved to items where they
/// name one.
fn links_of(
    path: &WorkspacePath,
    file: &ItemFile,
    by_path: &BTreeMap<String, ItemId>,
) -> Vec<Link> {
    let source = ConceptId::parse(path.as_str().trim_end_matches(".md")).ok();
    let mut links = Vec::new();
    for (index, line) in file.document.body.lines().enumerate() {
        for link in okf_core::links::extract_links(line) {
            let to = source.as_ref().and_then(|source| {
                link.resolve_all(source).into_iter().find_map(|concept| {
                    by_path
                        .get(&format!("{}.md", concept.segments().join("/")))
                        .copied()
                })
            });
            links.push(Link {
                from: file.header.item_id,
                to_path: link.target.clone(),
                to,
                label: link.text.clone(),
                line: u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX),
            });
        }
    }
    links
}

fn store(
    transaction: &Transaction<'_>,
    tenant: &str,
    workspace: &str,
    revision: &Revision,
    entries: &[Entry],
) -> Result<(), ApiError> {
    let inserted = transaction
        .execute(
            "INSERT INTO indexed_revisions (tenant_id, workspace_id, revision) VALUES (?1, ?2, ?3)
             ON CONFLICT DO NOTHING",
            params![tenant, workspace, revision.as_str()],
        )
        .map_err(|error| sql(&error))?;
    if inserted == 0 {
        return Ok(());
    }
    for entry in entries {
        let summary = serde_json::to_string(&entry.summary).map_err(|error| json(&error))?;
        let status = entry
            .summary
            .extraction
            .as_ref()
            .map(|extraction| variant_text!(&extraction.status))
            .transpose()?;
        transaction
            .execute(
                "INSERT INTO index_items (tenant_id, workspace_id, revision, item_id, path,
                     archived, extraction_status, summary)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    tenant,
                    workspace,
                    revision.as_str(),
                    entry.summary.id.0.to_string(),
                    entry.summary.path.as_str(),
                    entry.summary.archived,
                    status,
                    summary
                ],
            )
            .map_err(|error| sql(&error))?;
        let row = transaction.last_insert_rowid();
        transaction
            .execute(
                "INSERT INTO index_text (rowid, title, description, body) VALUES (?1, ?2, ?3, ?4)",
                params![
                    row,
                    entry.summary.title,
                    entry.summary.description,
                    entry.body
                ],
            )
            .map_err(|error| sql(&error))?;
        for link in &entry.links {
            transaction
                .execute(
                    "INSERT INTO index_links (tenant_id, workspace_id, revision, source_item,
                         source_path, target_path, target_item, label, line)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        tenant,
                        workspace,
                        revision.as_str(),
                        link.from.0.to_string(),
                        entry.summary.path.as_str(),
                        link.to_path,
                        link.to.map(|to| to.0.to_string()),
                        link.label,
                        i64::from(link.line)
                    ],
                )
                .map_err(|error| sql(&error))?;
        }
    }
    Ok(())
}

fn discard(transaction: &Transaction<'_>, tenant: &str, workspace: &str) -> Result<(), ApiError> {
    transaction
        .execute(
            "DELETE FROM index_text WHERE rowid IN
                 (SELECT entry FROM index_items WHERE tenant_id = ?1 AND workspace_id = ?2)",
            params![tenant, workspace],
        )
        .map_err(|error| sql(&error))?;
    for table in ["index_items", "index_links", "indexed_revisions"] {
        transaction
            .execute(
                &format!("DELETE FROM {table} WHERE tenant_id = ?1 AND workspace_id = ?2"),
                params![tenant, workspace],
            )
            .map_err(|error| sql(&error))?;
    }
    Ok(())
}

/// Rows of the revision the text matches, best first; every item when the text is blank.
fn matching(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
    query: &SearchQuery,
) -> Result<Vec<HitColumns>, ApiError> {
    let terms: Vec<String> = query
        .text
        .split_whitespace()
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect();
    let read = |row: &rusqlite::Row<'_>| -> rusqlite::Result<HitColumns> {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    };
    let rows = if terms.is_empty() {
        let mut statement = connection
            .prepare(
                "SELECT item_id, summary, '', 0.0 FROM index_items
                 WHERE tenant_id = ?1 AND workspace_id = ?2 AND revision = ?3 ORDER BY path",
            )
            .map_err(|error| sql(&error))?;
        statement
            .query_map(params![tenant, workspace, query.revision.as_str()], read)
            .map_err(|error| sql(&error))?
            .collect::<Result<Vec<_>, _>>()
    } else {
        let mut statement = connection
            .prepare(
                "SELECT items.item_id, items.summary,
                     snippet(index_text, 2, '', '', ' ... ', 24), -bm25(index_text)
                 FROM index_text JOIN index_items AS items ON items.entry = index_text.rowid
                 WHERE index_text MATCH ?4 AND items.tenant_id = ?1 AND items.workspace_id = ?2
                   AND items.revision = ?3
                 ORDER BY bm25(index_text), items.path",
            )
            .map_err(|error| sql(&error))?;
        statement
            .query_map(
                params![tenant, workspace, query.revision.as_str(), terms.join(" ")],
                read,
            )
            .map_err(|error| sql(&error))?
            .collect::<Result<Vec<_>, _>>()
    };
    rows.map_err(|error| sql(&error))
}

fn read_links(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
    revision: &Revision,
    column: &str,
    item: &str,
) -> Result<Vec<Link>, ApiError> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT source_item, target_path, target_item, label, line FROM index_links
             WHERE tenant_id = ?1 AND workspace_id = ?2 AND revision = ?3 AND {column} = ?4
             ORDER BY rowid"
        ))
        .map_err(|error| sql(&error))?;
    let rows = statement
        .query_map(params![tenant, workspace, revision.as_str(), item], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|error| sql(&error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql(&error))?;
    rows.into_iter()
        .map(|(from, to_path, to, label, line)| {
            Ok(Link {
                from: from_text!(from.as_str())?,
                to_path,
                to: to.map(|to| from_text!(to.as_str())).transpose()?,
                label,
                line: u32::try_from(line).unwrap_or(0),
            })
        })
        .collect()
}

fn indexed(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
    revision: &Revision,
) -> Result<bool, ApiError> {
    connection
        .query_row(
            "SELECT 1 FROM indexed_revisions
             WHERE tenant_id = ?1 AND workspace_id = ?2 AND revision = ?3",
            params![tenant, workspace, revision.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|found| found.is_some())
        .map_err(|error| sql(&error))
}

fn require_indexed(
    connection: &Connection,
    tenant: &str,
    workspace: &str,
    revision: &Revision,
) -> Result<(), ApiError> {
    if indexed(connection, tenant, workspace, revision)? {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::Unavailable,
            "that revision is not indexed yet",
        ))
    }
}

fn hit(
    workspace: okf_jawn_contract::identity::WorkspaceId,
    revision: &Revision,
    summary: ItemSummary,
    snippet: String,
    score: f64,
) -> SearchHit {
    SearchHit {
        source: SourceReference {
            workspace_id: workspace,
            item_id: summary.id,
            path: summary.path,
            revision: revision.clone(),
            digest: None,
            selection: Selection::All,
            locations: Vec::new(),
        },
        title: summary.title,
        snippet: if snippet.is_empty() {
            summary.description
        } else {
            snippet
        },
        score,
        extraction: summary.extraction,
    }
}

fn offset(cursor: Option<&str>) -> Result<usize, ApiError> {
    cursor
        .map(str::parse::<usize>)
        .transpose()
        .map_err(|_| {
            ApiError::new(
                ErrorCode::InvalidInput,
                "the page cursor is not one this store issued",
            )
            .with_field("/page/cursor")
        })
        .map(Option::unwrap_or_default)
}
