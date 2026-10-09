//! Retained history: the log of a workspace or one item (following its moves), comparisons
//! and last-change attribution.

use git2::{BlameOptions, Commit, DiffFindOptions, DiffOptions, Oid, Patch, Repository, Sort};
use okf_jawn_contract::common::TextRange;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::history::{
    BlameLine, BlameResponse, Commit as WireCommit, DiffResponse, FileChange, FileChangeKind,
    LogResponse,
};
use okf_jawn_contract::identity::{ItemId, Revision, Timestamp};
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::source::SourceReference;
use okf_jawn_core::storage::{BlameQuery, DiffQuery, LogQuery, StorageScope};

use super::read::find;
use super::repo::{commit_of, git, internal, revision_of, split_trailer_block};
use crate::seam;

pub(crate) fn log(repository: &Repository, query: &LogQuery) -> Result<LogResponse, ApiError> {
    let tip = commit_of(repository, &query.tip)?;
    let mut walk = repository.revwalk().map_err(|error| git(&error))?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)
        .map_err(|error| git(&error))?;
    walk.push(tip.id()).map_err(|error| git(&error))?;
    let skip = query
        .page
        .cursor
        .as_deref()
        .map(str::parse::<usize>)
        .transpose()
        .map_err(|_| {
            ApiError::new(
                ErrorCode::InvalidInput,
                "the page cursor is not one this store issued",
            )
            .with_field("/page/cursor")
        })?
        .unwrap_or(0);
    let limit = usize::from(query.page.limit.clamp(1, 500));
    let mut matched = 0_usize;
    let mut commits = Vec::new();
    let mut more = false;
    for oid in walk {
        let commit = repository
            .find_commit(oid.map_err(|error| git(&error))?)
            .map_err(|error| git(&error))?;
        if let Some(item) = query.item_id
            && !changes_item(repository, &commit, item)?
        {
            continue;
        }
        if matched >= skip {
            if commits.len() == limit {
                more = true;
                break;
            }
            commits.push(wire_commit(&commit)?);
        }
        matched = matched.saturating_add(1);
    }
    Ok(LogResponse {
        commits,
        next_cursor: more.then(|| skip.saturating_add(limit).to_string()),
    })
}

pub(crate) fn diff(repository: &Repository, query: &DiffQuery) -> Result<DiffResponse, ApiError> {
    let from = commit_of(repository, &query.from)?;
    let to = commit_of(repository, &query.to)?;
    let old_tree = from.tree().map_err(|error| git(&error))?;
    let new_tree = to.tree().map_err(|error| git(&error))?;
    let mut paths = Vec::new();
    if let Some(item) = query.item_id {
        for tree in [&old_tree, &new_tree] {
            if let Some((path, _)) = find(repository, tree, item)? {
                paths.push(path.as_str().to_owned());
            }
        }
    }
    let mut options = DiffOptions::new();
    let mut diff = repository
        .diff_tree_to_tree(Some(&old_tree), Some(&new_tree), Some(&mut options))
        .map_err(|error| git(&error))?;
    diff.find_similar(Some(DiffFindOptions::new().renames(true)))
        .map_err(|error| git(&error))?;
    let mut changes = Vec::new();
    for (index, delta) in diff.deltas().enumerate() {
        let old_path = delta
            .old_file()
            .path()
            .and_then(|path| path.to_str())
            .map(str::to_owned);
        let new_path = delta
            .new_file()
            .path()
            .and_then(|path| path.to_str())
            .map(str::to_owned);
        if query.item_id.is_some()
            && !paths
                .iter()
                .any(|path| old_path.as_ref() == Some(path) || new_path.as_ref() == Some(path))
        {
            continue;
        }
        let kind = match delta.status() {
            git2::Delta::Added | git2::Delta::Copied => FileChangeKind::Added,
            git2::Delta::Deleted => FileChangeKind::Removed,
            git2::Delta::Renamed => FileChangeKind::Moved,
            _ => FileChangeKind::Modified,
        };
        let binary = delta.flags().is_binary();
        let patch = Patch::from_diff(&diff, index)
            .map_err(|error| git(&error))?
            .map(|mut patch| {
                patch
                    .to_buf()
                    .map(|buffer| String::from_utf8_lossy(&buffer).into_owned())
            })
            .transpose()
            .map_err(|error| git(&error))?
            .unwrap_or_default();
        changes.push(FileChange {
            old_path: (kind != FileChangeKind::Added)
                .then_some(old_path)
                .flatten(),
            new_path: (kind != FileChangeKind::Removed)
                .then_some(new_path)
                .flatten(),
            patch,
            binary,
            kind,
        });
    }
    Ok(DiffResponse {
        from: query.from.clone(),
        to: query.to.clone(),
        changes,
        warnings: Vec::new(),
    })
}

pub(crate) fn blame(
    repository: &Repository,
    scope: &StorageScope,
    query: &BlameQuery,
) -> Result<BlameResponse, ApiError> {
    let commit = commit_of(repository, &query.revision)?;
    let tree = commit.tree().map_err(|error| git(&error))?;
    let (path, file) = find(repository, &tree, query.item_id)?
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No item with that identity here"))?;
    let text = file.text();
    let body_lines: Vec<&str> = file.document.body.lines().collect();
    let offset = text.lines().count().saturating_sub(body_lines.len());
    let mut options = BlameOptions::new();
    options.newest_commit(commit.id());
    let blame = repository
        .blame_file(std::path::Path::new(path.as_str()), Some(&mut options))
        .map_err(|error| git(&error))?;
    let mut lines = Vec::new();
    for line in query.lines.start..=query.lines.end {
        let Some(text) = usize::try_from(line)
            .ok()
            .and_then(|line| line.checked_sub(1))
            .and_then(|index| body_lines.get(index))
        else {
            return Err(ApiError::new(
                ErrorCode::InvalidInput,
                "the selected lines lie outside the item body",
            )
            .with_field("/lines"));
        };
        let file_line = usize::try_from(line)
            .map_err(|_| internal("a line number does not fit"))?
            .saturating_add(offset);
        let hunk = blame
            .get_line(file_line)
            .ok_or_else(|| internal("a body line has no blame"))?;
        let attributed = repository
            .find_commit(hunk.final_commit_id())
            .map_err(|error| git(&error))?;
        lines.push(BlameLine {
            line,
            text: (*text).to_owned(),
            commit: wire_commit(&attributed)?,
        });
    }
    Ok(BlameResponse {
        source: SourceReference {
            workspace_id: scope.workspace_id,
            item_id: query.item_id,
            path,
            revision: query.revision.clone(),
            digest: None,
            selection: Selection::Lines {
                range: TextRange {
                    start: query.lines.start,
                    end: query.lines.end,
                },
            },
            locations: Vec::new(),
        },
        lines,
    })
}

/// The committer and time of the newest commit, at or before `revision`, that changed `path`.
pub(crate) fn last_change(
    repository: &Repository,
    revision: &Revision,
    path: &str,
) -> Result<Option<(String, Timestamp)>, ApiError> {
    let tip = commit_of(repository, revision)?;
    let mut walk = repository.revwalk().map_err(|error| git(&error))?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)
        .map_err(|error| git(&error))?;
    walk.push(tip.id()).map_err(|error| git(&error))?;
    for oid in walk {
        let commit = repository
            .find_commit(oid.map_err(|error| git(&error))?)
            .map_err(|error| git(&error))?;
        let mine = blob_at(&commit, path)?;
        let parent = commit
            .parents()
            .next()
            .map(|parent| blob_at(&parent, path))
            .transpose()?
            .flatten();
        if mine.is_some() && mine != parent {
            let committer = commit.committer();
            return Ok(Some((
                committer.name().unwrap_or_default().to_owned(),
                seam::unix_instant(committer.when().seconds())?,
            )));
        }
    }
    Ok(None)
}

/// Whether `commit` changed the item's file (its content or its path) against its first
/// parent; a root commit changes every item it holds.
fn changes_item(
    repository: &Repository,
    commit: &Commit<'_>,
    item: ItemId,
) -> Result<bool, ApiError> {
    let tree = commit.tree().map_err(|error| git(&error))?;
    let mine = find(repository, &tree, item)?.map(|(path, file)| (path, file.text()));
    let parent = match commit.parents().next() {
        Some(parent) => {
            let tree = parent.tree().map_err(|error| git(&error))?;
            find(repository, &tree, item)?.map(|(path, file)| (path, file.text()))
        }
        None => None,
    };
    Ok(mine != parent)
}

fn blob_at(commit: &Commit<'_>, path: &str) -> Result<Option<Oid>, ApiError> {
    let tree = commit.tree().map_err(|error| git(&error))?;
    match tree.get_path(std::path::Path::new(path)) {
        Ok(entry) => Ok(Some(entry.id())),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(git(&error)),
    }
}

/// The wire form of a commit; its message is shown without the mutation trailer.
pub(crate) fn wire_commit(commit: &Commit<'_>) -> Result<WireCommit, ApiError> {
    let (message, _) = split_trailer_block(commit.message().unwrap_or_default());
    let message = message.trim_end().to_owned();
    Ok(WireCommit {
        revision: revision_of(commit.id())?,
        parents: commit
            .parent_ids()
            .map(revision_of)
            .collect::<Result<_, _>>()?,
        message,
        author: commit.author().name().unwrap_or_default().to_owned(),
        committed_at: seam::unix_instant(commit.time().seconds())?,
    })
}
