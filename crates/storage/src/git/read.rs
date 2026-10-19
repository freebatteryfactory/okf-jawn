//! Reads of one resolved revision: folder listings, the whole tree, items, files, rules, types,
//! corrections.
//!
//! `show` finds an item by the identity in its application header, never by a path the caller
//! names, so a moved item is found at its new path. `item_at_path` answers what is at one path
//! of that revision, by direct tree lookup, and summarizes it exactly as `list` does.

use git2::{Oid, Repository, Sort, Tree};
use okf_jawn_contract::conventions::NamingRules;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{SuppliedText, TextOrigin};
use okf_jawn_contract::identity::{Digest, ItemId, ProposalId, Revision, Timestamp, WorkspacePath};
use okf_jawn_contract::item::{ItemDocument, ItemSummary, TypeDefinition};
use okf_jawn_contract::source::SourceAppearance;
use okf_jawn_core::portable::item_content_digest;
use okf_jawn_core::storage::{
    FolderListing, Page, Provenance, RulesFile, RulesParseError, TreeItem, WorkspaceTree,
};

use super::edit::{appearance_file, correction_adopts, correction_file, supplied_file};
use super::history::last_change;
use super::item::{ItemFile, RULES_FILE, TYPES_FILE, is_item_path};
use super::repo::{blob_bytes, commit_of, files, git, internal, revision_of};

/// Every item of a tree with its path, in path order.
pub(crate) fn items(
    repository: &Repository,
    tree: &Tree<'_>,
) -> Result<Vec<(WorkspacePath, ItemFile)>, ApiError> {
    items_of(repository, files(repository, tree)?)
}

/// The items among `files` (a tree's files, as `repo::files` lists them), in their order.
fn items_of(
    repository: &Repository,
    files: Vec<(String, Oid)>,
) -> Result<Vec<(WorkspacePath, ItemFile)>, ApiError> {
    let mut found = Vec::new();
    for (path, blob) in files {
        if let Some(file) = item_file(repository, &path, blob)? {
            let path = WorkspacePath::try_from(path.clone())
                .map_err(|error| internal(format!("{path} is not a workspace path: {error}")))?;
            found.push((path, file));
        }
    }
    Ok(found)
}

/// The item a file at `path` holds: `None` when `path` cannot hold an item (`is_item_path`)
/// or the text carries no application header. `items`, and so `list`, `item_at_path` and the
/// item-filtered log (`history::log`) decide what is an item here and nowhere else.
pub(crate) fn item_file(
    repository: &Repository,
    path: &str,
    blob: Oid,
) -> Result<Option<ItemFile>, ApiError> {
    if !is_item_path(path) {
        return Ok(None);
    }
    parses::record();
    let text = String::from_utf8(blob_bytes(repository, blob)?)
        .map_err(|_| internal(format!("{path} is not UTF-8")))?;
    ItemFile::parse(&text)
}

/// The navigation summary of one item at `revision`. `list` and `item_at_path` both summarize
/// through it, so the two cannot disagree.
fn summary_of(
    repository: &Repository,
    tree: &Tree<'_>,
    path: &WorkspacePath,
    file: &ItemFile,
    revision: &Revision,
) -> Result<ItemSummary, ApiError> {
    let appearance = appearance(repository, tree, file.header.item_id)?;
    Ok(file.summary(path, revision, appearance.as_ref()))
}

/// The item whose path is exactly `path` at `revision`, summarized as `list` of its folder
/// summarizes it; `NotFound` only when this repository holds no such commit.
///
/// The path is looked up in that commit's tree one entry per segment (`blob_at`): no folder is
/// listed and no other item file is read, so the cost is the depth of the path, not the size
/// of the folder or the workspace. A missing or non-folder segment, a folder, and a file that
/// is not an item (`item_file`: a folder `index.md`, anything under `.okf/`) answer `None`.
pub(crate) fn item_at_path(
    repository: &Repository,
    revision: &Revision,
    path: &WorkspacePath,
) -> Result<Option<ItemSummary>, ApiError> {
    let tree = commit_of(repository, revision)?
        .tree()
        .map_err(|error| git(&error))?;
    let Some(blob) = blob_at(&tree, path.as_str())? else {
        return Ok(None);
    };
    let Some(file) = item_file(repository, path.as_str(), blob)? else {
        return Ok(None);
    };
    summary_of(repository, &tree, path, &file, revision).map(Some)
}

/// The blob at `path` in `tree`, found one tree entry per segment; `None` when a segment is
/// missing or not a folder, or the entry is not a file.
pub(crate) fn blob_at(tree: &Tree<'_>, path: &str) -> Result<Option<Oid>, ApiError> {
    let entry = match tree.get_path(std::path::Path::new(path)) {
        Ok(entry) => entry,
        Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(error) => return Err(git(&error)),
    };
    Ok((entry.kind() == Some(git2::ObjectType::Blob)).then(|| entry.id()))
}

/// One item of a tree by identity.
pub(crate) fn find(
    repository: &Repository,
    tree: &Tree<'_>,
    item: ItemId,
) -> Result<Option<(WorkspacePath, ItemFile)>, ApiError> {
    Ok(items(repository, tree)?
        .into_iter()
        .find(|(_, file)| file.header.item_id == item))
}

pub(crate) fn list(
    repository: &Repository,
    revision: &Revision,
    folder: Option<&WorkspacePath>,
    page: &Page,
) -> Result<FolderListing, ApiError> {
    let tree = commit_of(repository, revision)?
        .tree()
        .map_err(|error| git(&error))?;
    let prefix = folder
        .map(|folder| format!("{}/", folder.as_str()))
        .unwrap_or_default();
    let mut folders = std::collections::BTreeSet::new();
    let mut listed = Vec::new();
    for (path, _) in files(repository, &tree)? {
        let Some(rest) = path.strip_prefix(&prefix) else {
            continue;
        };
        if let Some((child, _)) = rest.split_once('/')
            && !child.starts_with('.')
        {
            folders.insert(format!("{prefix}{child}"));
        }
    }
    for (path, file) in items(repository, &tree)? {
        let inside = path
            .as_str()
            .strip_prefix(&prefix)
            .is_some_and(|rest| !rest.contains('/'));
        if inside {
            listed.push(summary_of(repository, &tree, &path, &file, revision)?);
        }
    }
    let start = page
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
    let limit = usize::from(page.limit.clamp(1, 500));
    let folders: Vec<WorkspacePath> = folders
        .into_iter()
        .map(|folder| {
            WorkspacePath::try_from(folder.clone())
                .map_err(|error| internal(format!("{folder}: {error}")))
        })
        .collect::<Result<_, _>>()?;
    let total = folders.len().saturating_add(listed.len());
    let end = start.saturating_add(limit).min(total);
    let folder_count = folders.len();
    let page_folders = folders
        .into_iter()
        .enumerate()
        .filter(|(index, _)| (start..end).contains(index))
        .map(|(_, folder)| folder)
        .collect();
    let page_items = listed
        .into_iter()
        .enumerate()
        .filter(|(index, _)| (start..end).contains(&index.saturating_add(folder_count)))
        .map(|(_, item)| item)
        .collect();
    Ok(FolderListing {
        items: page_items,
        folders: page_folders,
        next_cursor: (end < total).then(|| end.to_string()),
    })
}

/// Every item of `revision` and every folder of its tree, in one walk (`VersionStore::tree`):
/// each item file is parsed once and summarized as `list` summarizes it (`summary_of`), with a
/// source card's original name from its header; the folders are every folder above a file, so
/// the dot-folders `list` leaves out, `.okf` included, are here.
pub(crate) fn tree(
    repository: &Repository,
    revision: &Revision,
) -> Result<WorkspaceTree, ApiError> {
    let tree = commit_of(repository, revision)?
        .tree()
        .map_err(|error| git(&error))?;
    let files = files(repository, &tree)?;
    let mut folders = std::collections::BTreeSet::new();
    for (path, _) in &files {
        folders.extend(
            path.match_indices('/')
                .filter_map(|(at, _)| path.get(..at))
                .map(str::to_owned),
        );
    }
    let mut items = Vec::new();
    for (path, file) in items_of(repository, files)? {
        items.push(TreeItem {
            summary: summary_of(repository, &tree, &path, &file, revision)?,
            original_name: file
                .header
                .source
                .as_ref()
                .map(|source| source.original_name.clone()),
        });
    }
    let folders = folders
        .into_iter()
        .map(|folder| {
            WorkspacePath::try_from(folder.clone())
                .map_err(|error| internal(format!("{folder}: {error}")))
        })
        .collect::<Result<_, _>>()?;
    Ok(WorkspaceTree { items, folders })
}

pub(crate) fn show(
    repository: &Repository,
    revision: &Revision,
    item: ItemId,
) -> Result<ItemDocument, ApiError> {
    let tree = commit_of(repository, revision)?
        .tree()
        .map_err(|error| git(&error))?;
    let (path, file) = find(repository, &tree, item)?
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No item with that identity here"))?;
    let mut source = appearance(repository, &tree, item)?;
    if let (Some(appearance), Some(extraction)) = (source.as_mut(), file.header.extraction.clone())
    {
        appearance.extraction = extraction;
        if appearance.extraction.text_origin == TextOrigin::SuppliedByAgent {
            appearance.extraction.supplied = supplied(
                repository,
                revision,
                &tree,
                item,
                appearance.extraction.digest.as_ref(),
            )?;
        }
    }
    let summary = file.summary(&path, revision, source.as_ref());
    // `item_content_digest` reads only the body and properties, so the document is built with
    // a placeholder digest first and then given the digest computed from it.
    let mut document = ItemDocument {
        summary,
        body: file.document.body.clone(),
        properties: file.properties(),
        content_digest: Digest::try_from("0".repeat(64))
            .map_err(|error| internal(error.to_string()))?,
        source,
        draft: None,
    };
    document.content_digest = item_content_digest(&document)?;
    Ok(document)
}

pub(crate) fn file(
    repository: &Repository,
    revision: &Revision,
    path: &str,
) -> Result<Option<Vec<u8>>, ApiError> {
    let tree = commit_of(repository, revision)?
        .tree()
        .map_err(|error| git(&error))?;
    blob_at(&tree, path)?
        .map(|blob| blob_bytes(repository, blob))
        .transpose()
}

/// The naming rules file at `revision` (`VersionStore::rules`): its text, and its bytes read as
/// `NamingRules`. The bytes are read, not the text, so a file that is not UTF-8 never reads as
/// rules (the text shows U+FFFD where its bad bytes were). A file that does not read is answered
/// with the reader's message and its 1-based line and column, never as an error.
pub(crate) fn rules(
    repository: &Repository,
    revision: &Revision,
) -> Result<Option<RulesFile>, ApiError> {
    let Some(bytes) = file(repository, revision, RULES_FILE)? else {
        return Ok(None);
    };
    let parsed = yaml_serde::from_slice::<NamingRules>(&bytes).map_err(|error| {
        let location = error.location();
        RulesParseError {
            message: error.to_string(),
            line: location
                .as_ref()
                .and_then(|at| u32::try_from(at.line()).ok()),
            column: location
                .as_ref()
                .and_then(|at| u32::try_from(at.column()).ok()),
        }
    });
    Ok(Some(RulesFile {
        text: String::from_utf8_lossy(&bytes).into_owned(),
        parsed,
    }))
}

pub(crate) fn types(
    repository: &Repository,
    revision: &Revision,
) -> Result<Vec<TypeDefinition>, ApiError> {
    file(repository, revision, TYPES_FILE)?
        .map(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|error| internal(format!("{TYPES_FILE} does not parse: {error}")))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(crate) fn correction(
    repository: &Repository,
    revision: &Revision,
    item: ItemId,
    digest: &Digest,
) -> Result<Option<String>, ApiError> {
    let Some(bytes) = file(repository, revision, &correction_file(item, digest))? else {
        return Ok(None);
    };
    let record: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| internal(format!("a correction does not parse: {error}")))?;
    Ok(record
        .get("corrected_markdown")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned))
}

/// A card's stored source appearance, if it is a card.
pub(crate) fn appearance(
    repository: &Repository,
    tree: &Tree<'_>,
    item: ItemId,
) -> Result<Option<SourceAppearance>, ApiError> {
    let entry = match tree.get_path(std::path::Path::new(&appearance_file(item))) {
        Ok(entry) => entry,
        Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(error) => return Err(git(&error)),
    };
    serde_json::from_slice(&blob_bytes(repository, entry.id())?)
        .map(Some)
        .map_err(|error| internal(format!("a source appearance does not parse: {error}")))
}

/// The accepted supplied text of a source, with its approval read from the commit that
/// promoted it (its committer is the approver).
fn supplied(
    repository: &Repository,
    revision: &Revision,
    tree: &Tree<'_>,
    item: ItemId,
    digest: Option<&Digest>,
) -> Result<Option<SuppliedText>, ApiError> {
    let path = supplied_file(item);
    let adopts = adopted_proposal(repository, tree, item, digest)?;
    let Some((supplied_at, record)) = supplied_record(repository, revision, tree, &path, adopts)?
    else {
        return Ok(None);
    };
    let supplier: Provenance =
        serde_json::from_value(record.get("supplier").cloned().unwrap_or_default())
            .map_err(|error| internal(format!("supplied text has no supplier: {error}")))?;
    let markdown = record
        .get("markdown")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let proposal_id = supplied_proposal(&record)?;
    let Some((approver, approved_at)) =
        supplied_approval(repository, &supplied_at, &path, proposal_id)?
    else {
        return Ok(None);
    };
    Ok(Some(SuppliedText {
        proposal_id,
        supplied_by: supplier.subject,
        client_id: supplier.client_id,
        approved_by: approver,
        approved_at,
        content_digest: Digest::try_from(crate::seam::hex(
            &<sha2::Sha256 as sha2::Digest>::digest(markdown.as_bytes()),
        ))
        .map_err(|error| internal(error.to_string()))?,
        pages: serde_json::from_value(record.get("pages").cloned().unwrap_or_default())
            .unwrap_or_default(),
        kept_by: keeper(repository, revision, tree, item, digest, proposal_id)?,
    }))
}

/// A later accepted supply replaces the companion file but not its retained Git history.
/// An explicit adoption selects that proposal's record and original approval revision.
fn supplied_record(
    repository: &Repository,
    revision: &Revision,
    tree: &Tree<'_>,
    path: &str,
    adopts: Option<ProposalId>,
) -> Result<Option<(Revision, serde_json::Value)>, ApiError> {
    let current = blob_at(tree, path)?;
    if let Some(blob) = current {
        let record = supplied_blob(repository, blob)?;
        if adopts.is_none() || Some(supplied_proposal(&record)?) == adopts {
            return Ok(Some((revision.clone(), record)));
        }
    } else if adopts.is_none() {
        return Ok(None);
    }
    let mut seen = std::collections::BTreeSet::new();
    seen.extend(current);
    let mut walk = repository.revwalk().map_err(|error| git(&error))?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)
        .map_err(|error| git(&error))?;
    walk.push(commit_of(repository, revision)?.id())
        .map_err(|error| git(&error))?;
    for oid in walk {
        let commit = repository
            .find_commit(oid.map_err(|error| git(&error))?)
            .map_err(|error| git(&error))?;
        let tree = commit.tree().map_err(|error| git(&error))?;
        let Some(blob) = blob_at(&tree, path)? else {
            continue;
        };
        if !seen.insert(blob) {
            continue;
        }
        let record = supplied_blob(repository, blob)?;
        if Some(supplied_proposal(&record)?) == adopts {
            return Ok(Some((revision_of(commit.id())?, record)));
        }
    }
    Err(internal(
        "the adopted supplied text is absent from retained history",
    ))
}

fn supplied_blob(repository: &Repository, blob: Oid) -> Result<serde_json::Value, ApiError> {
    serde_json::from_slice(&blob_bytes(repository, blob)?)
        .map_err(|error| internal(format!("supplied text does not parse: {error}")))
}

fn supplied_proposal(record: &serde_json::Value) -> Result<ProposalId, ApiError> {
    serde_json::from_value(record.get("proposal_id").cloned().unwrap_or_default())
        .map_err(|error| internal(format!("supplied text has no proposal: {error}")))
}

/// The original acceptance is the earliest introduction of this immutable proposal on the
/// resolved accepted line. Rewind can reintroduce the same blob, but its committer is not the
/// approver. First parents exclude draft refs and merge side branches; promotion makes a new
/// commit on that line, with the accepting human as committer and the candidate as no parent.
fn supplied_approval(
    repository: &Repository,
    revision: &Revision,
    path: &str,
    proposal: ProposalId,
) -> Result<Option<(String, Timestamp)>, ApiError> {
    let mut next = Some(commit_of(repository, revision)?.id());
    let mut approval = None;
    while let Some(oid) = next {
        let commit = repository.find_commit(oid).map_err(|error| git(&error))?;
        next = commit.parent_ids().next();
        let tree = commit.tree().map_err(|error| git(&error))?;
        let Some(blob) = blob_at(&tree, path)? else {
            continue;
        };
        let before = if let Some(parent) = next {
            let parent = repository
                .find_commit(parent)
                .map_err(|error| git(&error))?;
            let tree = parent.tree().map_err(|error| git(&error))?;
            blob_at(&tree, path)?
        } else {
            None
        };
        if before == Some(blob) || supplied_proposal(&supplied_blob(repository, blob)?)? != proposal
        {
            continue;
        }
        let committer = commit.committer();
        approval = Some((
            committer.name().unwrap_or_default().to_owned(),
            crate::seam::unix_instant(committer.when().seconds())?,
        ));
    }
    Ok(approval)
}

fn adopted_proposal(
    repository: &Repository,
    tree: &Tree<'_>,
    item: ItemId,
    digest: Option<&Digest>,
) -> Result<Option<ProposalId>, ApiError> {
    let Some(digest) = digest else {
        return Ok(None);
    };
    let Some(blob) = blob_at(tree, &correction_file(item, digest))? else {
        return Ok(None);
    };
    let record: serde_json::Value = serde_json::from_slice(&blob_bytes(repository, blob)?)
        .map_err(|error| internal(format!("a correction does not parse: {error}")))?;
    correction_adopts(&record)
}

/// Adoption changes the correction file, not the original approval. Read the keeper from
/// that choice's commit so a later conversion cannot relabel the original approver.
fn keeper(
    repository: &Repository,
    revision: &Revision,
    tree: &Tree<'_>,
    item: ItemId,
    digest: Option<&Digest>,
    proposal: ProposalId,
) -> Result<Option<String>, ApiError> {
    let Some(digest) = digest else {
        return Ok(None);
    };
    let path = correction_file(item, digest);
    if adopted_proposal(repository, tree, item, Some(digest))? != Some(proposal) {
        return Ok(None);
    }
    Ok(last_change(repository, revision, &path)?.map(|(subject, _)| subject))
}

/// How many item files `item_file` parsed on this thread, counted in unit tests so the cost of
/// a walk can be checked; nothing is counted outside tests.
pub(crate) mod parses {
    #[cfg(test)]
    thread_local! {
        static COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// Count one parse.
    #[cfg(test)]
    pub(crate) fn record() {
        COUNT.with(|count| count.set(count.get().saturating_add(1)));
    }

    /// Count nothing outside tests.
    #[cfg(not(test))]
    pub(crate) const fn record() {}

    /// The parses counted on this thread since the last call.
    #[cfg(test)]
    pub(crate) fn take() -> usize {
        COUNT.with(|count| count.replace(0))
    }
}
