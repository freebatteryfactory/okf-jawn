//! Retained history: the log of a workspace or one item (following its moves), comparisons
//! and last-change attribution.
//!
//! A comparison's patch numbers lines of the whole item file, frontmatter included; blame and
//! reads number lines of the body. `body_offset` is the one count between the two: blame adds
//! it to a body line, and `body_start` states it so a caller can map a patch's lines back. It
//! counts the file as stored, not as the serializer would write it again: an item restored
//! from an archive may hold frontmatter the serializer does not reproduce (a comment line, no
//! blank line after the closing delimiter, CRLF line endings).
//!
//! An item is its file and its companion files (`edit::is_companion`: its source appearance,
//! its accepted supplied text and its corrections), which `RestoreItem` restores with it. So the
//! item-limited comparison lists a companion file's change as the item's, and the item-filtered
//! log names a commit that changed only companions (an appearance-only card write, say), as
//! one that changed the item's file.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use git2::{
    BlameOptions, Commit, DiffFindOptions, DiffOptions, Oid, Patch, Repository, Sort, Tree,
};
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

use super::edit::is_companion;
use super::item::is_item_path;
use super::read::{find, item_file, items};
use super::repo::{commit_of, git, internal, revision_of, split_trailer_block};
use crate::seam;

/// One item followed down an item-filtered log (`log`), so that a commit costs what it changed
/// rather than what its tree holds (sweep-1 review N1).
///
/// A commit changed the item when the item's file at the commit (the first file in path order
/// whose header names the item, as `read::find` finds it) differs from its file at the commit's
/// first parent in path or text, or when one of its companion files changed (`is_companion`);
/// a root commit is compared with an empty tree. Each commit is compared with its first parent
/// by Git's tree-to-tree diff (none when the two trees are one), and only the item files that
/// diff names are parsed, at both commits, so a commit whose diff names no item file costs no
/// parse. Which files hold the item is learned by parsing a whole tree (`read::items`) once, at
/// the first commit walked whose diff names a file holding it at either end, and is carried
/// from each commit to its first parent from then on; a commit reached otherwise (a second
/// parent) learns it again when its diff names the item.
struct Tracked {
    item: ItemId,
    /// The files holding the item at each commit not walked yet whose first-parent child was.
    holders: HashMap<Oid, BTreeSet<String>>,
}

/// The line that opens and closes an item file's frontmatter, as `okf_core` reads it.
const FRONTMATTER_DELIMITER: &str = "---";

impl Tracked {
    fn new(item: ItemId) -> Self {
        Self {
            item,
            holders: HashMap::new(),
        }
    }

    /// Whether `commit` changed the item against its first parent (see `Tracked`).
    fn changed_by(
        &mut self,
        repository: &Repository,
        commit: &Commit<'_>,
    ) -> Result<bool, ApiError> {
        let tree = commit.tree().map_err(|error| git(&error))?;
        let parent = commit.parents().next();
        let parent_tree = parent
            .as_ref()
            .map(Commit::tree)
            .transpose()
            .map_err(|error| git(&error))?;
        let known = self.holders.remove(&commit.id());
        let (changed, companion) =
            changed_paths(repository, parent_tree.as_ref(), &tree, self.item)?;
        let ours = self.held(repository, Some(&tree), &changed)?;
        let theirs = self.held(repository, parent_tree.as_ref(), &changed)?;
        let mine = match known {
            Some(known) => known,
            // Neither end holds the item in a file the diff names: the files that hold it are
            // the same at both, so its first one is the same file with the same bytes.
            None if ours.is_empty() && theirs.is_empty() => return Ok(companion),
            None => holders_in(repository, &tree, self.item)?,
        };
        let before: BTreeSet<String> = mine
            .iter()
            .filter(|path| !changed.contains(*path))
            .cloned()
            .chain(theirs.keys().cloned())
            .collect();
        let differs = match (mine.first(), before.first()) {
            (None, None) => false,
            (Some(now), Some(was)) if now == was => {
                changed.contains(now) && ours.get(now) != theirs.get(now)
            }
            _ => true,
        };
        if let Some(parent) = parent {
            self.holders.entry(parent.id()).or_insert(before);
        }
        Ok(companion || differs)
    }

    /// The text of the item in each of `paths` that holds it in `tree`; none without a tree.
    fn held(
        &self,
        repository: &Repository,
        tree: Option<&Tree<'_>>,
        paths: &BTreeSet<String>,
    ) -> Result<BTreeMap<String, String>, ApiError> {
        let mut found = BTreeMap::new();
        let Some(tree) = tree else {
            return Ok(found);
        };
        for path in paths {
            if let Some(blob) = super::read::blob_at(tree, path)?
                && let Some(file) = item_file(repository, path, blob)?
                && file.header.item_id == self.item
            {
                found.insert(path.clone(), file.text());
            }
        }
        Ok(found)
    }
}

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
    let mut tracked = query.item_id.map(Tracked::new);
    for oid in walk {
        let commit = repository
            .find_commit(oid.map_err(|error| git(&error))?)
            .map_err(|error| git(&error))?;
        if let Some(tracked) = tracked.as_mut()
            && !tracked.changed_by(repository, &commit)?
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
        if let Some(item) = query.item_id
            && !paths
                .iter()
                .any(|path| old_path.as_ref() == Some(path) || new_path.as_ref() == Some(path))
            && ![&old_path, &new_path]
                .into_iter()
                .any(|path| path.as_deref().is_some_and(|path| is_companion(path, item)))
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
    let body_lines: Vec<&str> = file.document.body.lines().collect();
    let offset = body_offset(&stored_text(repository, &tree, path.as_str())?);
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

/// The 1-based file line on which the item's body starts at `revision` (`body_offset` plus
/// one); `NotFound` when no item with that identity is there.
pub(crate) fn body_start(
    repository: &Repository,
    revision: &Revision,
    item: ItemId,
) -> Result<u32, ApiError> {
    let tree = commit_of(repository, revision)?
        .tree()
        .map_err(|error| git(&error))?;
    let (path, _) = find(repository, &tree, item)?
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No item with that identity here"))?;
    u32::try_from(body_offset(&stored_text(repository, &tree, path.as_str())?).saturating_add(1))
        .map_err(|_| internal("a line number does not fit"))
}

/// How many lines of an item's file, as stored, come before its body; blame and `body_start`
/// both count with it, so a body line one of them names is the file line the other names.
///
/// It follows `okf_core::Document::parse` line for line over the stored text: frontmatter
/// exists only when the first line trims to `---`; it ends at the next line that trims to
/// `---`; and parse strips exactly one empty line right after that delimiter. So the count is
/// 0 without frontmatter, otherwise the lines up to and including the closing delimiter, plus
/// that one empty line when it is there. Lines are split as `str::lines` splits them (on `\n`,
/// a trailing `\r` dropped), as parse and libgit2's blame both count them. For an empty body
/// the count still names the line where a body would begin.
pub(crate) fn body_offset(stored: &str) -> usize {
    let mut lines = stored.lines();
    if lines
        .next()
        .is_none_or(|first| first.trim() != FRONTMATTER_DELIMITER)
    {
        return 0;
    }
    let mut before = 1_usize;
    for line in lines.by_ref() {
        before = before.saturating_add(1);
        if line.trim() == FRONTMATTER_DELIMITER {
            return if lines.next() == Some("") {
                before.saturating_add(1)
            } else {
                before
            };
        }
    }
    // Unterminated frontmatter does not parse, so no item file reaches here; such a text is
    // all body to the reader that keeps it.
    0
}

/// The stored text of the file at `path` in `tree`.
fn stored_text(repository: &Repository, tree: &Tree<'_>, path: &str) -> Result<String, ApiError> {
    let entry = tree
        .get_path(std::path::Path::new(path))
        .map_err(|error| git(&error))?;
    let blob = repository
        .find_blob(entry.id())
        .map_err(|error| git(&error))?;
    String::from_utf8(blob.content().to_vec())
        .map_err(|_| internal("an item file is not UTF-8 text"))
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

/// The item files a commit's tree-to-tree diff against `parent` names (`is_item_path`), at
/// either end, and whether it names a companion file of `item`; nothing when the trees are one.
fn changed_paths(
    repository: &Repository,
    parent: Option<&Tree<'_>>,
    tree: &Tree<'_>,
    item: ItemId,
) -> Result<(BTreeSet<String>, bool), ApiError> {
    let mut changed = BTreeSet::new();
    let mut companion = false;
    if parent.is_some_and(|parent| parent.id() == tree.id()) {
        return Ok((changed, companion));
    }
    let diff = repository
        .diff_tree_to_tree(parent, Some(tree), None)
        .map_err(|error| git(&error))?;
    for delta in diff.deltas() {
        for file in [delta.old_file(), delta.new_file()] {
            let Some(path) = file.path().and_then(std::path::Path::to_str) else {
                continue;
            };
            if is_item_path(path) {
                changed.insert(path.to_owned());
            }
            companion = companion || is_companion(path, item);
        }
    }
    Ok((changed, companion))
}

/// Every file of `tree` whose header names `item` (`read::items`).
fn holders_in(
    repository: &Repository,
    tree: &Tree<'_>,
    item: ItemId,
) -> Result<BTreeSet<String>, ApiError> {
    Ok(items(repository, tree)?
        .into_iter()
        .filter(|(_, file)| file.header.item_id == item)
        .map(|(path, _)| path.as_str().to_owned())
        .collect())
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

#[cfg(test)]
mod tests {
    use git2::Sort;
    use okf_core::Document;
    use okf_jawn_contract::error::ApiError;
    use okf_jawn_contract::identity::{ItemId, MutationId, Revision, TenantId, WorkspacePath};
    use okf_jawn_core::storage::{
        CommitChanges, LogQuery, Page, StorageScope, TreeEdit, VersionStore, WorkspaceCatalog,
    };

    use super::{body_offset, find, git};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// What one tree holds of an item (`changed_plainly`): its first file's path and text, and
    /// each companion file with its blob.
    type Side = (Option<(WorkspacePath, String)>, Vec<(String, git2::Oid)>);

    /// The pages of one log (`pages`): each page's commits, and whether it said more follow.
    type Pages = Vec<(Vec<Revision>, bool)>;

    /// A data directory with one workspace, written through the store and through Git.
    struct Scripted {
        /// Declared before `directory`, so it drops first and the directory is removed whole.
        storage: crate::storage::Storage,
        scope: StorageScope,
        head: Revision,
        directory: tempfile::TempDir,
    }

    /// Accepts every candidate.
    struct Accept;

    impl Scripted {
        async fn new() -> Result<Self, Box<dyn std::error::Error>> {
            let directory = tempfile::tempdir()?;
            let storage = crate::storage::Storage::open(directory.path())?;
            let tenant = TenantId::try_from("local".to_owned())?;
            let created = storage
                .catalog()
                .create(&tenant, MutationId(uuid::Uuid::from_u128(1)), workspace())
                .await?;
            Ok(Self {
                storage,
                scope: StorageScope {
                    tenant_id: tenant,
                    workspace_id: created.id,
                },
                head: created.head,
                directory,
            })
        }

        /// Commit `edits` through the store on the head.
        async fn commit(
            &mut self,
            mutation: u128,
            edits: Vec<TreeEdit>,
        ) -> Result<Revision, Box<dyn std::error::Error>> {
            let committed = self
                .storage
                .versions()
                .commit(
                    &self.scope,
                    CommitChanges {
                        mutation_id: MutationId(uuid::Uuid::from_u128(mutation)),
                        expected_head: self.head.clone(),
                        author: person(),
                        message: format!("change {mutation}"),
                        edits,
                    },
                    std::sync::Arc::new(Accept),
                )
                .await?;
            self.head = committed.revision.clone();
            Ok(committed.revision)
        }

        fn repository(&self) -> Result<git2::Repository, Box<dyn std::error::Error>> {
            Ok(git2::Repository::open_bare(
                self.directory
                    .path()
                    .join("repositories")
                    .join("local")
                    .join(format!("{}.git", self.scope.workspace_id.0)),
            )?)
        }

        /// Commit through Git on `parents` (the head when empty), with the tree of the first
        /// parent changed by `change`, and move `refs/heads/main` to it unless `side`.
        fn raw(
            &mut self,
            parents: &[Revision],
            side: bool,
            change: &dyn Fn(
                &mut git2::build::TreeUpdateBuilder,
                &git2::Repository,
            ) -> Result<(), git2::Error>,
        ) -> Result<Revision, Box<dyn std::error::Error>> {
            let repository = self.repository()?;
            let named: Vec<Revision> = if parents.is_empty() {
                vec![self.head.clone()]
            } else {
                parents.to_vec()
            };
            let commits = named
                .iter()
                .map(|revision| repository.find_commit(git2::Oid::from_str(revision.as_str())?))
                .collect::<Result<Vec<_>, git2::Error>>()?;
            let first = commits.first().ok_or("no parent")?;
            let mut update = git2::build::TreeUpdateBuilder::new();
            change(&mut update, &repository)?;
            let tree = repository.find_tree(update.create_updated(&repository, &first.tree()?)?)?;
            let who = git2::Signature::now("raw", "raw@example.invalid")?;
            let parents: Vec<&git2::Commit<'_>> = commits.iter().collect();
            let commit = repository.commit(
                (!side).then_some("refs/heads/main"),
                &who,
                &who,
                "raw",
                &tree,
                &parents,
            )?;
            let revision = Revision::try_from(commit.to_string())?;
            if !side {
                self.head = revision.clone();
            }
            Ok(revision)
        }

        /// The bytes of the file at `at` on the head.
        fn bytes(&self, at: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
            let repository = self.repository()?;
            let tree = repository
                .find_commit(git2::Oid::from_str(self.head.as_str())?)?
                .tree()?;
            let entry = tree.get_path(std::path::Path::new(at))?;
            Ok(repository.find_blob(entry.id())?.content().to_vec())
        }
    }

    impl okf_jawn_core::storage::CandidateCheck for Accept {
        fn check(
            &self,
            _root: &std::path::Path,
            _rewritten: &[WorkspacePath],
        ) -> Result<Vec<okf_jawn_contract::common::Warning>, ApiError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn a_file_without_frontmatter_has_its_body_on_line_one() -> TestResult {
        let text = "First.\nSecond.\n";
        assert!(Document::parse(text)?.frontmatter.is_empty());
        assert_eq!(body_offset(text), 0, "{text:?}");
        Ok(())
    }

    #[test]
    fn the_body_starts_after_the_frontmatter_and_its_blank_line() -> TestResult {
        for keys in [1_usize, 2, 5] {
            let properties = (0..keys).fold(String::new(), |mut text, key| {
                text.push_str("key");
                text.push_str(&key.to_string());
                text.push_str(": value\n");
                text
            });
            let text = format!("---\n{properties}---\n\nFirst.\nSecond.\n");
            let offset = body_offset(&text);
            // `---`, one line per key, `---`, the blank line.
            assert_eq!(offset, keys + 3, "{text:?}");
            assert_eq!(text.lines().nth(offset), Some("First."), "{text:?}");
            assert_eq!(
                Document::parse(&text)?.body.lines().next(),
                Some("First."),
                "{text:?}"
            );
        }
        Ok(())
    }

    /// Stored shapes the serializer does not write: the line `body_offset` names is the first
    /// line of the body `Document::parse` reads from the same text.
    #[test]
    fn the_offset_names_the_first_body_line_parse_reads_from_the_stored_text() -> TestResult {
        let shapes = [
            "---\ntype: Note\n---\n\nFirst.\nSecond.\n",
            "---\ntype: Note\n---\nFirst.\nSecond.\n",
            "---\n# a comment\ntype: Note\n---\n\nFirst.\nSecond.\n",
            "---\r\ntype: Note\r\n---\r\n\r\nFirst.\r\nSecond.\r\n",
            "---\ntype: Note\n---\n\n\nFirst.\n",
            "  ---  \ntype: Note\n ---\n\nFirst.\n",
            // A whitespace-only line is not the empty line parse strips: it is body.
            "---\ntype: Note\n---\n \nFirst.\n",
        ];
        for text in shapes {
            let body = Document::parse(text)?.body;
            let first = body.lines().next();
            assert_eq!(text.lines().nth(body_offset(text)), first, "{text:?}");
        }
        Ok(())
    }

    fn workspace() -> okf_jawn_core::storage::NewWorkspace {
        okf_jawn_core::storage::NewWorkspace {
            name: "History".to_owned(),
            description: String::new(),
            creator: person(),
        }
    }

    fn person() -> okf_jawn_core::storage::Provenance {
        okf_jawn_core::storage::Provenance {
            subject: "ana".to_owned(),
            route: okf_jawn_contract::access::AccessRoute::LocalOwner,
            client_id: None,
        }
    }

    fn id(n: u128) -> ItemId {
        ItemId(uuid::Uuid::from_u128(n))
    }

    fn note(n: u128, at: &str, body: &str) -> Result<TreeEdit, Box<dyn std::error::Error>> {
        Ok(TreeEdit::CreateItem {
            item_id: id(n),
            path: WorkspacePath::try_from(at.to_owned())?,
            title: Some(format!("Note {n}")),
            type_name: "Note".to_owned(),
            kind: okf_jawn_contract::item::ItemKind::Note,
            body: body.to_owned(),
            properties: std::collections::BTreeMap::new(),
        })
    }

    fn moved(n: u128, to: &str) -> Result<TreeEdit, Box<dyn std::error::Error>> {
        Ok(TreeEdit::MoveItem {
            item_id: id(n),
            destination: WorkspacePath::try_from(to.to_owned())?,
        })
    }

    fn edited(n: u128, body: &str) -> TreeEdit {
        TreeEdit::EditItem {
            item_id: id(n),
            body: body.to_owned(),
            properties: std::collections::BTreeMap::new(),
        }
    }

    /// A source card of item `n` at `at` whose appearance names `media_type`.
    fn card(n: u128, at: &str, media_type: &str) -> Result<TreeEdit, Box<dyn std::error::Error>> {
        Ok(TreeEdit::WriteSourceCard(Box::new(
            okf_jawn_core::storage::SourceCard {
                item_id: id(n),
                path: WorkspacePath::try_from(at.to_owned())?,
                title: "scan.pdf".to_owned(),
                type_name: "Source".to_owned(),
                body: "Converted.".to_owned(),
                properties: std::collections::BTreeMap::new(),
                appearance: serde_json::from_value(serde_json::json!({
                    "object": "d".repeat(64),
                    "names": [{
                        "filename": "scan.pdf",
                        "folder": "sources",
                        "observed_at": "2026-10-09T12:00:00.000Z",
                        "supplied_by": "ana"
                    }],
                    "media_type": media_type,
                    "size": "10",
                    "metadata": {},
                    "extraction": {
                        "outcome": { "status": "unsupported" },
                        "digest": "a".repeat(64),
                        "text_origin": "none",
                        "corrected": false,
                        "warnings": []
                    }
                }))?,
            },
        )))
    }

    /// Whether `commit` changed `item`, read the plain way: the item's file (its first file in
    /// path order, as `read::find` finds it), path and text, and every companion file of it,
    /// path and bytes, read from the whole tree of the commit and of its first parent.
    fn changed_plainly(
        repository: &git2::Repository,
        commit: &git2::Commit<'_>,
        item: ItemId,
    ) -> Result<bool, ApiError> {
        let companion = |path: &str| {
            let id = item.0;
            path == format!(".okf/sources/{id}.json")
                || path == format!(".okf/supplied/{id}.json")
                || path.starts_with(&format!(".okf/corrections/{id}/"))
        };
        let side = |tree: Option<&git2::Tree<'_>>| -> Result<Side, ApiError> {
            let Some(tree) = tree else {
                return Ok((None, Vec::new()));
            };
            let found = find(repository, tree, item)?.map(|(path, file)| (path, file.text()));
            let companions = crate::git::repo::files(repository, tree)?
                .into_iter()
                .filter(|(path, _)| companion(path))
                .collect();
            Ok((found, companions))
        };
        let tree = commit.tree().map_err(|error| git(&error))?;
        let parent = commit
            .parents()
            .next()
            .map(|parent| parent.tree())
            .transpose()
            .map_err(|error| git(&error))?;
        Ok(side(Some(&tree))? != side(parent.as_ref())?)
    }

    /// The commits from `tip` that changed `item` (`changed_plainly`), in the log's walk order.
    fn changed_commits(
        repository: &git2::Repository,
        tip: &Revision,
        item: ItemId,
    ) -> Result<Vec<Revision>, Box<dyn std::error::Error>> {
        let mut walk = repository.revwalk()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
        walk.push(git2::Oid::from_str(tip.as_str())?)?;
        let mut found = Vec::new();
        for oid in walk {
            let commit = repository.find_commit(oid?)?;
            if changed_plainly(repository, &commit, item)? {
                found.push(Revision::try_from(commit.id().to_string())?);
            }
        }
        Ok(found)
    }

    /// Every page of the item-filtered log of `item` from `tip` at page size `limit`, each
    /// page's commits and whether it said more follow.
    fn pages(
        repository: &git2::Repository,
        tip: &Revision,
        item: ItemId,
        limit: u16,
    ) -> Result<Pages, Box<dyn std::error::Error>> {
        let mut found = Vec::new();
        let mut cursor = None;
        loop {
            let page = super::log(
                repository,
                &LogQuery {
                    tip: tip.clone(),
                    item_id: Some(item),
                    page: Page {
                        cursor: cursor.take(),
                        limit,
                    },
                },
            )?;
            let commits = page
                .commits
                .into_iter()
                .map(|commit| commit.revision)
                .collect();
            let more = page.next_cursor.is_some();
            found.push((commits, more));
            match page.next_cursor {
                Some(next) if found.len() < 100 => cursor = Some(next),
                _ => return Ok(found),
            }
        }
    }

    /// A history of every kind of change an item-filtered log must tell apart: notes created,
    /// edited, moved, renamed by letter case, deleted and rewound; a card corrected, then shown
    /// another way (its appearance alone); commits through Git with the same tree, with a
    /// second file holding an item's identity earlier in path order (added, changed, removed),
    /// with only a file that is no item, with only a correction file of the card, and a merge
    /// whose second parent changed a note. Answers the workspace and the items it holds or held.
    async fn scripted_history() -> Result<(Scripted, Vec<ItemId>), Box<dyn std::error::Error>> {
        let mut space = Scripted::new().await?;
        let early = space
            .commit(
                2,
                vec![
                    note(10, "notes/a.md", "A.")?,
                    note(11, "notes/b.md", "B.")?,
                    card(12, "sources/c.md", "application/pdf")?,
                    note(13, "x.md", "X.")?,
                ],
            )
            .await?;
        space.commit(3, vec![edited(10, "A, edited.")]).await?;
        let moved_on = space.commit(4, vec![moved(10, "archive/a.md")?]).await?;
        space
            .commit(
                5,
                vec![TreeEdit::CorrectDigest {
                    item_id: id(12),
                    digest: okf_jawn_contract::identity::Digest::try_from("a".repeat(64))?,
                    corrected_markdown: "Corrected.".to_owned(),
                    adopts: None,
                }],
            )
            .await?;
        space
            .commit(6, vec![card(12, "sources/c.md", "application/x-pdf")?])
            .await?;
        space
            .commit(
                7,
                vec![moved(11, "notes/sub/b.md")?, edited(13, "X, edited.")],
            )
            .await?;
        space.commit(8, vec![moved(11, "notes/sub/B.md")?]).await?;
        space
            .commit(9, vec![TreeEdit::DeleteItem { item_id: id(13) }])
            .await?;
        space.raw(&[], false, &|_, _| Ok(()))?;
        let copy = space.bytes("archive/a.md")?;
        space.raw(&[], false, &|update, repository| {
            update.upsert("aaa.md", repository.blob(&copy)?, git2::FileMode::Blob);
            Ok(())
        })?;
        let changed = String::from_utf8(copy.clone())?.replace("A, edited.", "A, copied.");
        space.raw(&[], false, &|update, repository| {
            update.upsert(
                "aaa.md",
                repository.blob(changed.as_bytes())?,
                git2::FileMode::Blob,
            );
            Ok(())
        })?;
        space.raw(&[], false, &|update, _| {
            update.remove("aaa.md");
            Ok(())
        })?;
        space.raw(&[], false, &|update, repository| {
            update.upsert(
                "files/x.pdf",
                repository.blob(b"%PDF")?,
                git2::FileMode::Blob,
            );
            Ok(())
        })?;
        let correction = format!(".okf/corrections/{}/{}.json", id(12).0, "b".repeat(64));
        space.raw(&[], false, &|update, repository| {
            update.upsert(&correction, repository.blob(b"{}")?, git2::FileMode::Blob);
            Ok(())
        })?;
        space
            .commit(
                10,
                vec![TreeEdit::RestoreItem {
                    item_id: id(13),
                    from: early.clone(),
                }],
            )
            .await?;
        let side_note =
            String::from_utf8(space.bytes("notes/sub/B.md")?)?.replace("B.", "B, on a side line.");
        let side = space.raw(&[moved_on], true, &|update, repository| {
            update.upsert(
                "notes/b.md",
                repository.blob(side_note.as_bytes())?,
                git2::FileMode::Blob,
            );
            Ok(())
        })?;
        let head = space.head.clone();
        space.raw(&[head, side], false, &|_, _| Ok(()))?;
        Ok((space, vec![id(10), id(11), id(12), id(13), id(99)]))
    }

    /// Sweep-1 review N1, the item-filtered log's cost: the log that parses only the item files
    /// a commit's diff names (`Tracked`) names exactly the commits the plain reading of every
    /// tree does (`changed_plainly`), page by page, for every item of a scripted history and
    /// one it never held.
    #[tokio::test]
    async fn the_item_filtered_log_names_what_reading_every_tree_names() -> TestResult {
        let (space, items) = scripted_history().await?;
        let repository = space.repository()?;
        for item in items {
            let expected = changed_commits(&repository, &space.head, item)?;
            for limit in [1_u16, 2, 3, 500] {
                let limit_items = usize::from(limit);
                let mut wanted: Vec<(Vec<Revision>, bool)> = expected
                    .chunks(limit_items)
                    .enumerate()
                    .map(|(index, chunk)| {
                        let seen = index.saturating_add(1).saturating_mul(limit_items);
                        (chunk.to_vec(), seen < expected.len())
                    })
                    .collect();
                if wanted.is_empty() {
                    wanted.push((Vec::new(), false));
                }
                assert_eq!(
                    pages(&repository, &space.head, item, limit)?,
                    wanted,
                    "item {} at limit {limit}",
                    item.0
                );
            }
        }
        Ok(())
    }

    /// Sweep-1 review N1, the item-filtered log's cost: a commit whose diff names no item file
    /// costs no parse. Over 120 commits that change only a file that is no item, the log of an
    /// item changed below them parses each item file of the workspace about once, not once per
    /// commit walked.
    #[tokio::test]
    async fn a_commit_whose_diff_names_no_item_file_costs_no_parse() -> TestResult {
        const ITEMS: u128 = 30;
        let mut space = Scripted::new().await?;
        let mut edits = vec![note(10, "victim.md", "The victim.")?];
        for n in 0..ITEMS {
            edits.push(note(1000 + n, &format!("n/i{n:04}.md"), "A note.")?);
        }
        space.commit(2, edits).await?;
        space.commit(3, vec![moved(10, "victim2.md")?]).await?;
        let removed = space
            .commit(4, vec![TreeEdit::DeleteItem { item_id: id(10) }])
            .await?;
        for n in 0..120_u32 {
            let bytes = format!("%PDF {n}");
            space.raw(&[], false, &|update, repository| {
                update.upsert(
                    "files/x.pdf",
                    repository.blob(bytes.as_bytes())?,
                    git2::FileMode::Blob,
                );
                Ok(())
            })?;
        }
        let repository = space.repository()?;
        crate::git::read::parses::take();
        let found = super::log(
            &repository,
            &LogQuery {
                tip: space.head.clone(),
                item_id: Some(id(10)),
                page: Page {
                    cursor: None,
                    limit: 1,
                },
            },
        )?;
        let parsed = crate::git::read::parses::take();
        let first = found.commits.first().map(|commit| commit.revision.clone());
        assert_eq!(first, Some(removed));
        let bound = usize::try_from(ITEMS)?.saturating_add(10);
        assert!(
            parsed <= bound,
            "{parsed} item files parsed (at most {bound})"
        );
        Ok(())
    }
}
