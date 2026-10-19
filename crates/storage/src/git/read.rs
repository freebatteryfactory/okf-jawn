//! Reads of one resolved revision: folder listings, items, files, rules, types, corrections.
//!
//! `show` finds an item by the identity in its application header, never by a path the caller
//! names, so a moved item is found at its new path. `item_at_path` answers what is at one path
//! of that revision, by direct tree lookup, and summarizes it exactly as `list` does.

use git2::{ObjectType, Oid, Repository, Tree};
use okf_jawn_contract::conventions::NamingRules;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{SuppliedText, TextOrigin};
use okf_jawn_contract::identity::{Digest, ItemId, Revision, WorkspacePath};
use okf_jawn_contract::item::{ItemDocument, ItemSummary, TypeDefinition};
use okf_jawn_contract::source::SourceAppearance;
use okf_jawn_core::portable::{content_digest, item_content_digest};
use okf_jawn_core::storage::{
    CommittedFile, CommittedFiles, CommittedItem, FilePage, FolderListing, Page, Provenance,
};

use super::edit::{appearance_file, correction_file, supplied_file};
use super::export::refuse_off_the_line;
use super::history::last_change;
use super::item::{ItemFile, RULES_FILE, TYPES_FILE, is_item_path};
use super::repo::{blob_bytes, commit_of, files, git, internal};

/// The Git file mode of a regular file.
const REGULAR_FILE: i32 = 0o100_644;
/// The Git file mode of an executable file, also a regular file.
const EXECUTABLE_FILE: i32 = 0o100_755;

/// Every item of a tree with its path, in path order.
pub(crate) fn items(
    repository: &Repository,
    tree: &Tree<'_>,
) -> Result<Vec<(WorkspacePath, ItemFile)>, ApiError> {
    let mut found = Vec::new();
    for (path, blob) in files(repository, tree)? {
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
/// item-filtered log (`history::log`), and `committed_files` decide what is an item here and
/// nowhere else (`item_in`).
pub(crate) fn item_file(
    repository: &Repository,
    path: &str,
    blob: Oid,
) -> Result<Option<ItemFile>, ApiError> {
    if !is_item_path(path) {
        return Ok(None);
    }
    item_in(path, blob_bytes(repository, blob)?)
}

/// The item the bytes of a file at an item path hold (`item_file`).
fn item_in(path: &str, bytes: Vec<u8>) -> Result<Option<ItemFile>, ApiError> {
    parses::record();
    let text = String::from_utf8(bytes).map_err(|_| internal(format!("{path} is not UTF-8")))?;
    ItemFile::parse(&text)
}

/// One page of every file of `revision`'s tree, in byte order of path
/// (`VersionStore::committed_files`).
///
/// The tree's paths are listed first without reading a blob; only the files of the page are
/// read and, at an item path, parsed. So a whole walk reads each file once and visits the
/// tree's paths once a page.
pub(crate) fn committed_files(
    repository: &Repository,
    revision: &Revision,
    page: &FilePage,
) -> Result<CommittedFiles, ApiError> {
    let commit = commit_of(repository, revision)?;
    refuse_off_the_line(repository, commit.id())?;
    let tree = commit.tree().map_err(|error| git(&error))?;
    let mut paths = Vec::new();
    regular_files(repository, &tree, "", &mut paths)?;
    paths.sort();
    let limit = usize::from(page.limit.clamp(1, 500));
    let mut remaining = paths
        .into_iter()
        .filter(|(path, _)| {
            page.after
                .as_deref()
                .is_none_or(|after| path.as_str() > after)
        })
        .peekable();
    let mut files = Vec::new();
    let mut held: u64 = 0;
    for (path, blob) in remaining.by_ref() {
        let bytes = blob_bytes(repository, blob)?;
        held = held.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        let item = if is_item_path(&path) {
            item_in(&path, bytes.clone())?
                .map(|file| committed_item(&file))
                .transpose()?
        } else {
            None
        };
        files.push(CommittedFile { path, bytes, item });
        if files.len() >= limit || held > page.max_bytes {
            break;
        }
    }
    let next_after = remaining
        .peek()
        .is_some()
        .then(|| files.last().map(|file| file.path.clone()))
        .flatten();
    Ok(CommittedFiles { files, next_after })
}

/// What `show` would answer of an item file that a portable export needs: its identity, role,
/// content digest (computed from the same body and properties `show` builds its document from)
/// and the digests its header names.
fn committed_item(file: &ItemFile) -> Result<CommittedItem, ApiError> {
    Ok(CommittedItem {
        item_id: file.header.item_id,
        kind: file.kind(),
        content_digest: content_digest(&file.document.body, &file.properties())?,
        source: file.header.source.clone(),
        record: file
            .header
            .extraction
            .as_ref()
            .and_then(|extraction| extraction.digest.clone()),
    })
}

/// Every regular file under `tree`, as `/`-separated paths below `prefix` with their blobs.
///
/// A link or a submodule is `Internal`: no store edit writes one, and a portable export must not
/// write a link's target text as if it were a file.
fn regular_files(
    repository: &Repository,
    tree: &Tree<'_>,
    prefix: &str,
    found: &mut Vec<(String, Oid)>,
) -> Result<(), ApiError> {
    for entry in tree {
        let name = entry
            .name()
            .ok()
            .ok_or_else(|| internal("a tree entry name is not UTF-8"))?;
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        match (entry.kind(), entry.filemode()) {
            (Some(ObjectType::Tree), _) => {
                let child = repository
                    .find_tree(entry.id())
                    .map_err(|error| git(&error))?;
                regular_files(repository, &child, &path, found)?;
            }
            (Some(ObjectType::Blob), REGULAR_FILE | EXECUTABLE_FILE) => {
                found.push((path, entry.id()));
            }
            _ => {
                return Err(internal(format!(
                    "{path} is not a regular file of the tree (a link or a submodule)"
                )));
            }
        }
    }
    Ok(())
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
            appearance.extraction.supplied = supplied(repository, revision, &tree, item)?;
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

pub(crate) fn rules(
    repository: &Repository,
    revision: &Revision,
) -> Result<Option<NamingRules>, ApiError> {
    file(repository, revision, RULES_FILE)?
        .map(|bytes| {
            yaml_serde::from_slice(&bytes)
                .map_err(|error| internal(format!("{RULES_FILE} does not parse: {error}")))
        })
        .transpose()
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
) -> Result<Option<SuppliedText>, ApiError> {
    let path = supplied_file(item);
    let entry = match tree.get_path(std::path::Path::new(&path)) {
        Ok(entry) => entry,
        Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(error) => return Err(git(&error)),
    };
    let record: serde_json::Value = serde_json::from_slice(&blob_bytes(repository, entry.id())?)
        .map_err(|error| internal(format!("supplied text does not parse: {error}")))?;
    let supplier: Provenance =
        serde_json::from_value(record.get("supplier").cloned().unwrap_or_default())
            .map_err(|error| internal(format!("supplied text has no supplier: {error}")))?;
    let markdown = record
        .get("markdown")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let Some((approver, approved_at)) = last_change(repository, revision, &path)? else {
        return Ok(None);
    };
    Ok(Some(SuppliedText {
        proposal_id: serde_json::from_value(record.get("proposal_id").cloned().unwrap_or_default())
            .map_err(|error| internal(format!("supplied text has no proposal: {error}")))?,
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
        kept_by: None,
    }))
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
