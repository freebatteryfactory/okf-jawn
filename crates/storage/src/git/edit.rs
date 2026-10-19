//! Applying `TreeEdit`s to a staged candidate tree with okf-core.
//!
//! Edits run in order on the staging directory. A caller edit may not change an application
//! header (`items::refuse_header_change`, `items::refuse_header_in_type`); the server's own
//! header updates (`SetStatus`, `WriteSourceCard`, `CorrectDigest`, `SupplyExtraction`) may,
//! and the server's restores (`RestoreItem`, `RestorePaths`, `RestoreWorkspace`) write the
//! files an earlier revision holds without that refusal. An item's companion files (its source
//! appearance, supplied text and corrections folder under `.okf/`) go with it: `DeleteItem`
//! removes them and `RestoreItem` sets them to `from`'s state. Moves rewrite the links that
//! point at the item (`okf_core::move_concept`), and so does a `RestoreItem` that puts an item
//! back at another path than the one it holds now. An item, a move or a folder may not land on a
//! reserved name (`.okf/`, `index.md`, `log.md`) in any letter case: it is refused naming the
//! path before anything is written. After the edits, no two paths, files or folders, may share
//! a collision key, and no path may spell a name the server writes (`item::SERVER_NAMES`)
//! another way (`Staged::refuse_collisions`; a tree that already holds either is refused when it
//! is staged, `repo::materialize`, naming the repair); then the folder indexes are regenerated
//! and the change is appended to `log.md`. The refusal does not run again after those: they
//! write only `index.md` in the tree's folders and `log.md` at the root, and a tree that spells
//! either another way is already refused, so nothing they write can make a collision the
//! refusal passed (a second run after them was reached by no tree, and was dropped). The files
//! a move rewrote besides the moved item's own (the referrers) are remembered, and the
//! candidate check is told them (`Staged::rewritten`, `CandidateCheck::check`).
//! Conformance is the `CandidateCheck`'s decision, not this module's.
//!
//! The staged tree is on the server's own filesystem, where Windows and macOS hold `Report.md`
//! and `report.md` as one file and Linux and Git hold two. A move whose destination differs
//! from the item's path only that way (the two share a collision key) is a rename that changes
//! only letter case, and it goes in two hops, each with its links (`relink`): first to a free
//! private path beside the item (`park`), then to the destination. In one hop okf-core would
//! write the destination and then remove the source, which on such a filesystem is the file it
//! just wrote. okf-core also rewrites the item's own links on every hop, even within one folder
//! (`./plan.md` becomes `plan.md`), and not idempotently, so the item's file is written at the
//! destination as the first hop left it, which is what a direct move writes (`move_item`). The
//! private path is in no commit's tree, in no `rewritten` list and in no log entry: the person
//! sees one move in one commit. For that move only, the item's own file does not count as
//! taking the destination (`Staged::occupied`); a move onto any other item's or file's key is
//! refused as taken.
//!
//! A move with `keep_alias` first records the item's path in its frontmatter `aliases`
//! (`record_alias`), after the move's refusals and before its first hop, so a rename by letter
//! case records the item's own path, never the private one, and a move to the path the item holds
//! records nothing.
//!
//! okf-core 0.2.7's `move_concept` appends `.md` to a moved concept's relative links to files
//! that are not Markdown, on every move (`![p](pics/p.png)` becomes `![p](pics/p.png.md)`):
//! `Link::resolve` gives an id for any relative target inside the bundle, and the outgoing-link
//! rewrite takes every such link for a link to a concept, which `compute_relative_path` ends in
//! `.md`. As a workaround, every move (`move_in`, which every hop of `MoveItem`, `park` and a
//! Rewind's `move_back` takes) writes the moved item's body again from its body before the
//! move, rewritten with okf-core's own public functions as `move_concept` rewrites it but taking
//! a relative link for a concept's only when the bundle holds that concept, the one condition
//! of the upstream fix (`moved_body`); okf-core's frontmatter is kept as it wrote it. The
//! workaround is removed when an okf-core release carries the fix. The same rewrite makes the
//! item's links to itself follow it, which `move_concept` does not (it writes a relative one as
//! a link to the old name and keeps an absolute one): a link that resolves to the moved concept
//! names the path the move ends at, a relative one as `compute_relative_path` gives it and an
//! absolute one as `/<path>.md`, the form okf-core gives another note's absolute link to the
//! moved item, each with its anchor; a pure anchor (`#sec`) is kept. A `sources` entry of the
//! frontmatter that names the item itself is not covered: okf-core rebases it as a path, so it
//! keeps naming the old one. Reference-style links and their definitions (`[t][ref]`,
//! `[ref]: pics/p.png`) and raw HTML (`<img src="pics/p.png">`) are not links that
//! `rewrite_markdown_links` sees, so a move to another folder leaves them as written, and they
//! then resolve from the new folder, as with okf-core 0.2.7 and the upstream fix.
//!
//! For the same reason, every write that names a new path checks the folders it names against
//! the staged tree's own spellings before that path is written there
//! (`Staged::refuse_respelled_folder`): `notes/new.md` when the tree holds `Notes/` would land in
//! `Notes/` on Windows and macOS and make a second folder on Linux, so it is refused on every
//! system as `InvalidInput`, naming the tree's spelling and the request field. The writes are
//! `CreateItem`, `MoveItem`, `CreateFolder` (whose own last segment counts too),
//! `WriteSourceCard`, and the restores that write an earlier path: `RestoreItem` (each move back
//! and each restored file) and `RestorePaths`, which also refuses a listed file the tree spells
//! another way (`refuse_respelled_file`) and a listed path below a file
//! (`refuse_file_as_folder`). A refusal leaves only the staged copy changed, and the
//! staged copy is discarded, so nothing is committed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use git2::Repository;
use okf_core::yaml::Value as Yaml;
use okf_core::{
    Bundle, ConceptId, Document, Link, LinkKind, LinkRewriteAction, MoveOptions, RefactorError,
    compute_relative_path, rebase_relative_path, rewrite_markdown_links,
};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::TextOrigin;
use okf_jawn_contract::identity::{
    Digest, ItemId, ProposalId, Revision, WorkspacePath, collision_key_of,
};
use okf_jawn_contract::item::{ItemStatus, TypeDefinition};
use okf_jawn_core::items::{ApplicationHeader, refuse_header_change, refuse_header_in_type};
use okf_jawn_core::portable::ALIASES_KEY;
use okf_jawn_core::storage::{SourceCard, TreeEdit};

use super::item::{
    CORRECTIONS_DIR, Holder, INDEX_FILE, ItemFile, LOG_FILE, RULES_FILE, SOURCES_DIR, SUPPLIED_DIR,
    TYPES_FILE, bare_header, check_item_path, is_item_path, is_reserved, new_item, respelled,
};
use super::repo::{commit_of, internal, materialize, materialize_at};
use crate::data::io_error;
use crate::seam;

/// The items of a staged tree by identity.
pub(crate) struct Staged {
    root: PathBuf,
    items: BTreeMap<ItemId, String>,
    /// Every file a move rewrote besides the moved item's own old and new file, as a
    /// `/`-separated path relative to the root: the referrers whose links followed the item.
    rewritten: BTreeSet<String>,
}

impl Staged {
    /// Index the items of a staging directory.
    pub(crate) fn scan(root: &Path) -> Result<Self, ApiError> {
        let mut staged = Self {
            root: root.to_path_buf(),
            items: BTreeMap::new(),
            rewritten: BTreeSet::new(),
        };
        staged.rescan()?;
        Ok(staged)
    }

    fn rescan(&mut self) -> Result<(), ApiError> {
        let mut items = BTreeMap::new();
        for path in files_under(&self.root, "")? {
            if !is_item_path(&path) {
                continue;
            }
            let text = read_text(&self.root.join(&path))?;
            if let Some(file) = ItemFile::parse(&text)?
                && items.insert(file.header.item_id, path.clone()).is_some()
            {
                return Err(ApiError::new(
                    ErrorCode::Conflict,
                    format!(
                        "two files carry the identity of item {}",
                        file.header.item_id.0
                    ),
                ));
            }
        }
        self.items = items;
        Ok(())
    }

    fn path_of(&self, item: ItemId) -> Result<String, ApiError> {
        self.items.get(&item).cloned().ok_or_else(|| {
            ApiError::new(
                ErrorCode::NotFound,
                "No item with that identity in this tree",
            )
        })
    }

    fn read(&self, item: ItemId) -> Result<(String, ItemFile), ApiError> {
        let path = self.path_of(item)?;
        let file = ItemFile::parse(&read_text(&self.root.join(&path))?)?
            .ok_or_else(|| internal("an indexed item lost its header"))?;
        Ok((path, file))
    }

    fn write(&mut self, path: &str, file: &ItemFile) -> Result<(), ApiError> {
        write_text(&self.root.join(path), &file.text())?;
        self.items.insert(file.header.item_id, path.to_owned());
        Ok(())
    }

    /// The files the edits made this store rewrite on its own, for the candidate check
    /// (`CandidateCheck::check`, `rewritten`): the referrers every move so far rewrote that
    /// the staged tree still holds in that exact spelling, in path order, each once. A
    /// referrer a later edit removed, moved away or renamed by letter case is left out (its
    /// new path, if any, is one the caller's own edit names), and so is one whose path is no
    /// `WorkspacePath`: no item has such a path (`refuse_collisions`), so only a file that is
    /// no item can be left out for it. The spelling is compared with the staged tree's own
    /// listing (`files_under`), because on a filesystem that holds `Ref.md` and `ref.md` as one
    /// file, a path lookup finds either spelling.
    pub(crate) fn rewritten(&self) -> Result<Vec<WorkspacePath>, ApiError> {
        let files: BTreeSet<String> = files_under(&self.root, "")?.into_iter().collect();
        Ok(self
            .rewritten
            .iter()
            .filter(|path| files.contains(*path))
            .filter_map(|path| WorkspacePath::try_from(path.clone()).ok())
            .collect())
    }

    /// Refuse two paths of the staged tree that share a collision key (Stage 1b design section
    /// 7), files and the folders above them alike, and a path the server writes spelled another
    /// way (`respelled`), so a tree never holds two files, or two spellings of one folder, that
    /// a case-insensitive filesystem would merge, whatever the edits did. On such a filesystem
    /// the staged copy cannot hold such a pair; the tree it was staged from is checked for one
    /// when it is staged (`repo::materialize`). It runs once, after the edits and before the
    /// indexes and the log are written (`stage_and_commit`), and the rule still holds on what is
    /// committed: those writes are `index.md` in folders the tree holds and `log.md` at the root,
    /// and a tree that spells either another way, as a file or a folder, is refused here as a
    /// path the server writes spelled another way.
    pub(crate) fn refuse_collisions(&self) -> Result<(), ApiError> {
        for path in self.items.values() {
            WorkspacePath::try_from(path.clone()).map_err(|error| {
                ApiError::new(ErrorCode::InvalidInput, format!("{path}: {error}"))
            })?;
        }
        let files = files_under(&self.root, "")?;
        match respelled(files.iter().map(String::as_str)) {
            Some(found) => Err(found.made()),
            None => Ok(()),
        }
    }

    /// Refuse `path` when a folder it names is in the staged tree already, spelled another way
    /// (one collision key, other letters): `InvalidInput` on `field`, naming the spelling the
    /// tree has (see the module header). `with_last` counts the last segment as a folder too,
    /// for a folder being created; otherwise only the folders above it are checked. A folder
    /// the tree does not hold yet, under any spelling, is new and passes.
    fn refuse_respelled_folder(
        &self,
        path: &WorkspacePath,
        with_last: bool,
        field: &str,
    ) -> Result<(), ApiError> {
        let mut folders: Vec<&str> = path.as_str().split('/').collect();
        if !with_last {
            folders.pop();
        }
        let mut directory = self.root.clone();
        let mut spelled = Vec::with_capacity(folders.len());
        for folder in folders {
            let Some(name) = spelled_in(&directory, folder, true)? else {
                return Ok(());
            };
            directory.push(&name);
            let same = name == folder;
            spelled.push(name);
            if !same {
                return Err(ApiError::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "{}: the folder is spelled {}/",
                        path.as_str(),
                        spelled.join("/")
                    ),
                )
                .with_field(field));
            }
        }
        Ok(())
    }

    /// Whether a file or folder of the staged tree (an empty folder an earlier edit left
    /// included), or an item, already holds `path`'s collision key, or a path below it (whose
    /// folder at that key is listed too), or whether a file holds the key of a folder above it.
    /// The staged directory is listed and compared by key (`entries_under`), so a folder spelled
    /// another way counts on a filesystem that holds `report.md/` and `Report.md` apart as on
    /// one that holds them as one.
    ///
    /// `own` is the moved item's own file, for a move. Only when it holds `path`'s own key (a
    /// rename that changes only letter case, see the module header) is it not counted; then,
    /// on a filesystem that holds both spellings as one file, `path` exists because of it, so
    /// only the listing is asked. Otherwise the filesystem is asked too whether anything is at
    /// `path`. Every other file, folder and item counts.
    fn occupied(&self, path: &WorkspacePath, own: Option<&str>) -> Result<bool, ApiError> {
        let key = path.collision_key();
        let own = own.filter(|own| collision_key_of(own) == key);
        if own.is_none() && self.root.join(path.as_str()).exists() {
            return Ok(true);
        }
        // Every folder the path would need: a file already holding one of those keys blocks it.
        let folders: Vec<&str> = key
            .match_indices('/')
            .filter_map(|(at, _)| key.get(..at))
            .collect();
        Ok(entries_under(&self.root)?.iter().any(|(existing, folder)| {
            if own == Some(existing.as_str()) {
                return false;
            }
            let existing = collision_key_of(existing);
            // Anything below `path` is under a folder at its key, which is an entry itself.
            existing == key || (!folder && folders.contains(&existing.as_str()))
        }) || self
            .items
            .values()
            .any(|existing| own != Some(existing.as_str()) && collision_key_of(existing) == key))
    }
}

/// Apply `edits` in order to the staged tree.
///
/// A run of consecutive `RestoreItem` edits is applied as one step (`restore_items`), so a
/// Rewind of items that traded paths since `from` restores them all.
pub(crate) fn apply(
    repository: &Repository,
    staged: &mut Staged,
    edits: Vec<TreeEdit>,
) -> Result<(), ApiError> {
    let mut edits = edits.into_iter().peekable();
    while let Some(edit) = edits.next() {
        let TreeEdit::RestoreItem { item_id, from } = edit else {
            apply_one(repository, staged, edit)?;
            continue;
        };
        let mut run = vec![(item_id, from)];
        while let Some(TreeEdit::RestoreItem { item_id, from }) =
            edits.next_if(|next| matches!(next, TreeEdit::RestoreItem { .. }))
        {
            run.push((item_id, from));
        }
        restore_items(repository, staged, run)?;
    }
    Ok(())
}

fn apply_one(repository: &Repository, staged: &mut Staged, edit: TreeEdit) -> Result<(), ApiError> {
    match edit {
        TreeEdit::CreateItem {
            item_id,
            path,
            title,
            type_name,
            kind,
            body,
            properties,
        } => create_item(
            staged,
            bare_header(item_id, kind),
            &path,
            title,
            type_name,
            &body,
            properties,
        ),
        TreeEdit::EditItem {
            item_id,
            body,
            properties,
        } => {
            let (path, mut file) = staged.read(item_id)?;
            refuse_header_change(&file.properties(), &properties, "/properties")?;
            file.set_properties(&properties)?;
            file.document.body = body;
            staged.write(&path, &file)
        }
        TreeEdit::MoveItem {
            item_id,
            destination,
            keep_alias,
        } => move_item(staged, item_id, &destination, keep_alias),
        TreeEdit::SetStatus {
            item_id,
            status,
            archived,
        } => set_status(staged, item_id, status, archived),
        TreeEdit::DeleteItem { item_id } => {
            let path = staged.path_of(item_id)?;
            remove(&staged.root.join(&path))?;
            staged.items.remove(&item_id);
            remove(&appearance_path(&staged.root, item_id))?;
            remove(&supplied_path(&staged.root, item_id))?;
            remove_folder(&staged.root.join(corrections_folder(item_id)))
        }
        TreeEdit::CreateFolder { folder } => create_folder(staged, &folder),
        TreeEdit::SetType { definition } => set_type(&staged.root, definition),
        TreeEdit::SetRules { rules } => {
            let text = yaml_serde::to_string(&rules)
                .map_err(|error| internal(format!("naming rules did not serialize: {error}")))?;
            write_text(&staged.root.join(RULES_FILE), &text)
        }
        TreeEdit::WriteSourceCard(card) => write_card(staged, &card),
        TreeEdit::SupplyExtraction {
            item_id,
            based_on,
            pages,
            markdown,
            proposal_id,
            supplier,
        } => {
            let record = serde_json::json!({
                "based_on": based_on,
                "pages": pages,
                "markdown": markdown,
                "proposal_id": proposal_id,
                "supplier": supplier,
            });
            supply(staged, item_id, &record, markdown)
        }
        TreeEdit::CorrectDigest {
            item_id,
            digest,
            corrected_markdown,
            adopts,
        } => correct(staged, item_id, &digest, corrected_markdown, adopts),
        TreeEdit::RestoreItem { item_id, from } => {
            restore_items(repository, staged, vec![(item_id, from)])
        }
        TreeEdit::RestorePaths { from, paths } => restore_paths(repository, staged, &from, &paths),
        TreeEdit::RestoreWorkspace { from } => {
            let commit = commit_of(repository, &from)?;
            let tree = commit.tree().map_err(|error| super::repo::git(&error))?;
            clear_directory(&staged.root)?;
            materialize(repository, &tree, &staged.root, Holder::Restored)?;
            staged.rescan()
        }
    }
}

fn create_item(
    staged: &mut Staged,
    header: ApplicationHeader,
    path: &WorkspacePath,
    title: Option<String>,
    type_name: String,
    body: &str,
    mut properties: BTreeMap<String, serde_json::Value>,
) -> Result<(), ApiError> {
    check_item_path(path, "/path")?;
    staged.refuse_respelled_folder(path, false, "/path")?;
    refuse_header_change(&BTreeMap::new(), &properties, "/properties")?;
    if staged.items.contains_key(&header.item_id) {
        return Err(conflict("an item with this identity already exists"));
    }
    if staged.occupied(path, None)? {
        return Err(conflict(format!("{} is already taken", path.as_str())));
    }
    properties.insert("type".to_owned(), serde_json::Value::String(type_name));
    if let Some(title) = title {
        properties.insert("title".to_owned(), serde_json::Value::String(title));
    }
    let file = new_item(&properties, body, header)?;
    staged.write(path.as_str(), &file)
}

/// Create an empty folder with its index, refusing a reserved name in any letter case.
fn create_folder(staged: &Staged, folder: &WorkspacePath) -> Result<(), ApiError> {
    if is_reserved(folder.as_str()) {
        return Err(ApiError::new(
            ErrorCode::InvalidInput,
            format!(
                "{} is reserved: .okf, index.md and log.md (in any letter case, as any path \
                 segment) are kept by the server",
                folder.as_str()
            ),
        )
        .with_field("/folder"));
    }
    staged.refuse_respelled_folder(folder, true, "/folder")?;
    let target = staged.root.join(folder.as_str());
    if target.exists() || staged.occupied(folder, None)? {
        return Err(conflict(format!("{} is already taken", folder.as_str())));
    }
    std::fs::create_dir_all(&target).map_err(|error| io_error("create", &target, &error))?;
    write_text(
        &target.join(INDEX_FILE),
        &okf_core::index::build_index_text(&[]),
    )
}

/// Keep agent-supplied text beside the card; the card shows it unless a correction is shown.
fn supply(
    staged: &mut Staged,
    item_id: ItemId,
    record: &serde_json::Value,
    markdown: String,
) -> Result<(), ApiError> {
    let (path, mut file) = staged.read(item_id)?;
    let extraction = file.header.extraction.as_mut().ok_or_else(|| {
        ApiError::new(ErrorCode::InvalidInput, "only a source has extracted text")
    })?;
    extraction.text_origin = TextOrigin::SuppliedByAgent;
    extraction.supplied = None;
    if !extraction.corrected {
        file.document.body = markdown;
    }
    write_text(&supplied_path(&staged.root, item_id), &record.to_string())?;
    file.store_header()?;
    staged.write(&path, &file)
}

/// Keep a correction of one digest beside the card; the card shows it when that digest is
/// the current one.
fn correct(
    staged: &mut Staged,
    item_id: ItemId,
    digest: &Digest,
    corrected_markdown: String,
    adopts: Option<okf_jawn_contract::identity::ProposalId>,
) -> Result<(), ApiError> {
    let (path, mut file) = staged.read(item_id)?;
    let record = serde_json::json!({
        "corrected_markdown": corrected_markdown,
        "adopts": adopts,
    });
    let extraction = file.header.extraction.as_mut().ok_or_else(|| {
        ApiError::new(
            ErrorCode::InvalidInput,
            "only a source has a digest to correct",
        )
    })?;
    if extraction.digest.as_ref() == Some(digest) {
        extraction.corrected = adopts.is_none();
        if adopts.is_some() {
            extraction.text_origin = TextOrigin::SuppliedByAgent;
        }
        file.document.body = corrected_markdown;
    }
    write_text(
        &correction_path(&staged.root, item_id, digest),
        &record.to_string(),
    )?;
    file.store_header()?;
    staged.write(&path, &file)
}

fn restore_paths(
    repository: &Repository,
    staged: &mut Staged,
    from: &Revision,
    paths: &[WorkspacePath],
) -> Result<(), ApiError> {
    let commit = commit_of(repository, from)?;
    let tree = commit.tree().map_err(|error| super::repo::git(&error))?;
    for (index, path) in paths.iter().enumerate() {
        let field = format!("/paths/{index}");
        staged.refuse_respelled_folder(path, false, &field)?;
        refuse_file_as_folder(&staged.root, path, &field)?;
        refuse_respelled_file(&staged.root, path, &field)?;
        let target = staged.root.join(path.as_str());
        match tree.get_path(Path::new(path.as_str())) {
            Ok(entry) => {
                let bytes = super::repo::blob_bytes(repository, entry.id())?;
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|error| io_error("create", parent, &error))?;
                }
                std::fs::write(&target, bytes)
                    .map_err(|error| io_error("write", &target, &error))?;
            }
            Err(error) if error.code() == git2::ErrorCode::NotFound => remove(&target)?,
            Err(error) => return Err(super::repo::git(&error)),
        }
    }
    staged.rescan()
}

/// Restore each listed item and its companions to their state at its `from`.
///
/// Each item is found by identity at its `from` and in the staged tree; absent at both is
/// `NotFound`. An item the staged tree holds at another path than `from`'s is first moved
/// back there with its links (`move_back`). Then every listed item's file is removed, so items that
/// traded paths since `from` do not block each other. Then, in order, an item `from` holds is
/// written at its path there with the exact bytes `from` holds, after that path is checked free
/// (`Staged::occupied`): landing on another item's path, or on any other file or folder, is
/// `Conflict`. Each companion (the source appearance, the supplied text and the whole
/// corrections folder of the item) is set to `from`'s state: copied when present there,
/// removed when not, so an item absent at `from` is removed with its companions. An item
/// listed twice ends as its last listing restores it. The header refusal does not apply: this
/// is the server's own restore, as `RestorePaths` is.
fn restore_items(
    repository: &Repository,
    staged: &mut Staged,
    run: Vec<(ItemId, Revision)>,
) -> Result<(), ApiError> {
    let mut found = Vec::with_capacity(run.len());
    for (item, from) in run {
        let tree = commit_of(repository, &from)?
            .tree()
            .map_err(|error| super::repo::git(&error))?;
        let earlier = super::read::find(repository, &tree, item)?.map(|(path, _)| path);
        if earlier.is_none() && !staged.items.contains_key(&item) {
            return Err(ApiError::new(
                ErrorCode::NotFound,
                "No item with that identity at either revision",
            ));
        }
        found.push((item, tree, earlier));
    }
    move_back(staged, &found)?;
    for (item, _, _) in &found {
        if let Some(current) = staged.items.remove(item) {
            remove(&staged.root.join(current))?;
        }
    }
    for (item, tree, earlier) in &found {
        // Only an item listed twice was already restored by this run.
        if let Some(restored) = staged.items.remove(item) {
            remove(&staged.root.join(restored))?;
        }
        if let Some(earlier) = earlier {
            staged.refuse_respelled_folder(earlier, false, "/target")?;
            if staged.occupied(earlier, None)? {
                return Err(conflict(format!("{} is already taken", earlier.as_str())));
            }
            restore_file(repository, tree, &staged.root, earlier.as_str())?;
            staged.items.insert(*item, earlier.as_str().to_owned());
        }
        restore_file(repository, tree, &staged.root, &appearance_file(*item))?;
        restore_file(repository, tree, &staged.root, &supplied_file(*item))?;
        restore_folder(repository, tree, &staged.root, &corrections_folder(*item))?;
    }
    staged.rescan()
}

/// Move each item of a restore run that the staged tree holds at a path other than its path at
/// its `from` back to that path, as `MoveItem` moves an item, so every link that points at it
/// follows it. An item the staged tree does not hold (deleted since) or holds at its `from`
/// path has nothing to move; an item listed twice moves to its last listing's path.
///
/// Items that traded paths since `from` would block each other, so the movers go first to a
/// private path each (`park`, beside its current path), then the other listed items' files are
/// removed (they are restored from `from` after this), then each mover goes to its `from` path,
/// refused as `Conflict` when that path is taken (`Staged::occupied`). Each step rewrites the
/// links, so a link that pointed at a mover ends pointing at its restored path. A mover whose
/// `from` path differs from its current one only in letter case is parked like any other, so
/// it never moves straight between the two spellings.
fn move_back(
    staged: &mut Staged,
    found: &[(ItemId, git2::Tree<'_>, Option<WorkspacePath>)],
) -> Result<(), ApiError> {
    let mut destinations: BTreeMap<ItemId, Option<&WorkspacePath>> = BTreeMap::new();
    for (item, _, earlier) in found {
        destinations.insert(*item, earlier.as_ref());
    }
    let mut movers = Vec::new();
    for (item, destination) in &destinations {
        if let (Some(destination), Some(current)) = (destination, staged.items.get(item))
            && current != destination.as_str()
        {
            movers.push((*item, current.clone(), *destination));
        }
    }
    if movers.is_empty() {
        return Ok(());
    }
    let mut parked = Vec::with_capacity(movers.len());
    for (item, current, destination) in movers {
        // `ends` is the hop's own target here: the next hop reads these bytes and takes a link to
        // the private path for one to the item itself, and `restore_items` then writes the earlier
        // revision's bytes over them. Named for the destination, a link of the item to itself
        // would be taken for a link to another mover that is parked from that path after it.
        let private = park(staged, item, &current, "rewind", None, None)?;
        parked.push((private, destination));
    }
    for item in destinations.keys() {
        let Some(current) = staged.items.get(item) else {
            continue;
        };
        if parked
            .iter()
            .any(|(private, _)| private.as_str() == current)
        {
            continue;
        }
        let current = current.clone();
        staged.items.remove(item);
        remove(&staged.root.join(current))?;
    }
    for (private, destination) in parked {
        staged.refuse_respelled_folder(destination, false, "/target")?;
        if staged.occupied(destination, None)? {
            return Err(conflict(format!(
                "{} is already taken",
                destination.as_str()
            )));
        }
        relink(staged, private.as_str(), destination.as_str(), None)?;
    }
    Ok(())
}

/// Set the file at `path` under `root` to `tree`'s file there, or remove it when `tree` holds
/// no file there.
fn restore_file(
    repository: &Repository,
    tree: &git2::Tree<'_>,
    root: &Path,
    path: &str,
) -> Result<(), ApiError> {
    let target = root.join(path);
    match tree.get_path(Path::new(path)) {
        Ok(entry) if entry.kind() == Some(git2::ObjectType::Blob) => {
            let bytes = super::repo::blob_bytes(repository, entry.id())?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| io_error("create", parent, &error))?;
            }
            std::fs::write(&target, bytes).map_err(|error| io_error("write", &target, &error))
        }
        Ok(_) => remove(&target),
        Err(error) if error.code() == git2::ErrorCode::NotFound => remove(&target),
        Err(error) => Err(super::repo::git(&error)),
    }
}

/// Replace the folder at `path` under `root` with `tree`'s folder there, or remove it when
/// `tree` holds no folder there.
fn restore_folder(
    repository: &Repository,
    tree: &git2::Tree<'_>,
    root: &Path,
    path: &str,
) -> Result<(), ApiError> {
    let target = root.join(path);
    remove_folder(&target)?;
    match tree.get_path(Path::new(path)) {
        Ok(entry) if entry.kind() == Some(git2::ObjectType::Tree) => {
            let folder = repository
                .find_tree(entry.id())
                .map_err(|error| super::repo::git(&error))?;
            std::fs::create_dir_all(&target)
                .map_err(|error| io_error("create", &target, &error))?;
            materialize_at(repository, &folder, &target, path, Holder::Restored)
        }
        Ok(_) => Ok(()),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(()),
        Err(error) => Err(super::repo::git(&error)),
    }
}

/// Regenerate the folder indexes and append the change to `log.md`.
pub(crate) fn maintain(root: &Path, message: &str) -> Result<(), ApiError> {
    okf_core::index::regenerate_indexes(root)
        .map_err(|error| io_error("regenerate the indexes of", root, &error))?;
    let now = seam::now()?;
    let date = okf_core::Date::parse(now.as_str().get(..10).unwrap_or_default())
        .ok_or_else(|| internal("today's date did not parse"))?;
    let summary = message.lines().next().unwrap_or_default();
    okf_core::append_log_entry(root, date, "change", summary)
        .map_err(|error| io_error("append to", &root.join(LOG_FILE), &error))?;
    Ok(())
}

fn move_item(
    staged: &mut Staged,
    item: ItemId,
    destination: &WorkspacePath,
    keep_alias: bool,
) -> Result<(), ApiError> {
    check_item_path(destination, "/destination")?;
    staged.refuse_respelled_folder(destination, false, "/destination")?;
    let source = staged.path_of(item)?;
    if source == destination.as_str() {
        return Ok(());
    }
    if staged.occupied(destination, Some(&source))? {
        return Err(conflict(format!(
            "{} is already taken",
            destination.as_str()
        )));
    }
    if keep_alias {
        record_alias(staged, item, &source)?;
    }
    if collision_key_of(&source) != destination.collision_key() {
        return relink(staged, &source, destination.as_str(), Some("/destination"));
    }
    // A rename that changes only letter case goes by a private path (the module header). The
    // first hop is a move within the item's own folder, which is the destination's (S2 refuses
    // another spelling of it), so the item's file is then what a direct move writes. The second
    // hop would rewrite the item's own links once more, and okf-core's rewrite is not
    // idempotent (`../../a.pdf` becomes `../a.pdf`, then `../a.pdf.md`), so the first hop's
    // bytes are written at the destination after it.
    // Those bytes are what lands, so the first hop's `ends` is the move's final destination,
    // not the hop's own target: the item's links to itself name the destination. Named for the
    // private path, they would carry the private name into the committed tree.
    let private = park(
        staged,
        item,
        &source,
        "recase",
        Some(destination),
        Some("/destination"),
    )?;
    let parked = staged.root.join(private.as_str());
    let bytes = std::fs::read(&parked).map_err(|error| io_error("read", &parked, &error))?;
    relink(
        staged,
        private.as_str(),
        destination.as_str(),
        Some("/destination"),
    )?;
    let landed = staged.root.join(destination.as_str());
    std::fs::write(&landed, bytes).map_err(|error| io_error("write", &landed, &error))
}

/// Record `source`, the item's path before the move it is about to take, in its frontmatter
/// `aliases` (`TreeEdit::MoveItem`'s `keep_alias`): an absent value becomes a list of it, a
/// string a list of the string and it, and a list of strings gains it unless it holds it. Any
/// other value refuses the move with `InvalidInput` naming the item, and is not written over.
fn record_alias(staged: &mut Staged, item: ItemId, source: &str) -> Result<(), ApiError> {
    let (path, mut file) = staged.read(item)?;
    let old = Yaml::String(source.to_owned());
    let aliases = match file.document.frontmatter.get(ALIASES_KEY) {
        None => vec![old],
        Some(Yaml::String(text)) if text == source => vec![old],
        Some(Yaml::String(text)) => vec![Yaml::String(text.clone()), old],
        Some(Yaml::Sequence(listed))
            if listed.iter().all(|alias| matches!(alias, Yaml::String(_))) =>
        {
            let mut listed = listed.clone();
            if !listed
                .iter()
                .any(|alias| matches!(alias, Yaml::String(text) if text == source))
            {
                listed.push(old);
            }
            listed
        }
        Some(_) => {
            return Err(ApiError::new(
                ErrorCode::InvalidInput,
                format!(
                    "{path} holds `{ALIASES_KEY}` that is not a list of paths, so its old path is \
                     not recorded over it; make it a list, or leave it out"
                ),
            ));
        }
    };
    file.document
        .frontmatter
        .set(ALIASES_KEY, Yaml::Sequence(aliases));
    staged.write(&path, &file)
}

/// Move the item file at `current` to a private path beside it, with every link that points at
/// it (`move_in`), and answer that path. The caller moves the item on from there within the
/// same commit, so the private path reaches no commit's tree. `ends` is where the bytes this
/// hop writes end up, for the item's links to itself (`moved_body`): the destination of a
/// rename by letter case, whose first hop's bytes are written there (`move_item`), or, when it
/// is `None`, the private path, for a hop whose bytes the next hop moves on. `field` is passed
/// to `move_in`.
///
/// The private path is the first free one of `okf-jawn-<purpose>-<item id>.md`, then
/// `okf-jawn-<purpose>-<item id>-1.md`, `-2` and so on, in `current`'s folder. Free means that
/// no file, folder or item holds its collision key in any spelling (`Staged::occupied`), and
/// that no link or `sources` entry of the staged tree resolves to it (`linked`): the next hop
/// rewrites every reference that resolves to the private path, so it must find only the ones
/// this hop wrote. Each entry of the folder takes at most one of these names (the one with its
/// key), and so does each linked concept, so one of the first `entries + linked + 1` is free.
fn park(
    staged: &mut Staged,
    item: ItemId,
    current: &str,
    purpose: &str,
    ends: Option<&WorkspacePath>,
    field: Option<&str>,
) -> Result<WorkspacePath, ApiError> {
    let bundle = load(&staged.root)?;
    let linked = linked(&bundle);
    let folder = current.rsplit_once('/').map(|(folder, _)| folder);
    let directory = folder.map_or_else(|| staged.root.clone(), |folder| staged.root.join(folder));
    let entries = std::fs::read_dir(&directory)
        .map_err(|error| io_error("read", &directory, &error))?
        .count();
    for attempt in 0..=entries.saturating_add(linked.len()) {
        let name = match attempt {
            0 => format!("okf-jawn-{purpose}-{}.md", item.0),
            _ => format!("okf-jawn-{purpose}-{}-{attempt}.md", item.0),
        };
        let private = folder.map_or_else(|| name.clone(), |folder| format!("{folder}/{name}"));
        let private = WorkspacePath::try_from(private)
            .map_err(|error| internal(format!("a private path is invalid: {error}")))?;
        if linked.contains(&concept(private.as_str())?) || staged.occupied(&private, None)? {
            continue;
        }
        let ends = ends.unwrap_or(&private);
        move_in(
            staged,
            &bundle,
            current,
            private.as_str(),
            ends.as_str(),
            field,
        )?;
        return Ok(private);
    }
    Err(internal(format!(
        "no private path beside {current} was free"
    )))
}

/// Every concept a reference of `bundle` resolves to, found as `okf_core::move_concept` finds
/// the references it rewrites: each Markdown link of a body (`rewrite_markdown_links`, then
/// `Link::resolve` from the document's own concept) and each `sources[].resource` of a
/// frontmatter. A reference to a concept that does not exist counts: a move to that concept
/// would rewrite it.
fn linked(bundle: &Bundle) -> BTreeSet<ConceptId> {
    let mut found = BTreeSet::new();
    for concept in bundle.concepts() {
        rewrite_markdown_links(&concept.document.body, |link, _| {
            found.extend(link.resolve(&concept.id));
            LinkRewriteAction::Keep
        });
        let Some(okf_core::Value::Sequence(sources)) = concept.document.frontmatter.get("sources")
        else {
            continue;
        };
        for source in sources {
            if let okf_core::Value::Mapping(entry) = source
                && let Some(okf_core::Value::String(resource)) = entry.get("resource")
            {
                let link = Link {
                    text: String::new(),
                    kind: Link::classify(resource),
                    target: resource.clone(),
                };
                found.extend(link.resolve(&concept.id));
            }
        }
    }
    found
}

/// The staged tree as okf-core reads it.
fn load(root: &Path) -> Result<Bundle, ApiError> {
    Bundle::load(root)
        .map_err(|error| internal(format!("the staged tree did not load as a bundle: {error}")))
}

/// Move the item file at `source` to `destination` with `okf_core::move_concept`, which rewrites
/// every link that points at it, and index the staged tree again (`move_in`). The caller has
/// checked `destination` free.
fn relink(
    staged: &mut Staged,
    source: &str,
    destination: &str,
    field: Option<&str>,
) -> Result<(), ApiError> {
    let bundle = load(&staged.root)?;
    // The hop is the whole move: its target is where it ends.
    move_in(staged, &bundle, source, destination, destination, field)
}

/// `relink` on `bundle`, the staged tree as it is now. `ends` is the path the item's links to
/// itself name after this hop (`moved_body`): `destination` for a single move and for each hop of
/// a Rewind's `move_back` (whose first hop ends at its private path), and the final destination
/// for the first hop of a rename by letter case (`park`), whose bytes are the ones kept. A refusal other than an
/// existing target is `InvalidInput` on `field` for a caller's move, and `Internal` for the
/// server's own (`field` is `None`).
fn move_in(
    staged: &mut Staged,
    bundle: &Bundle,
    source: &str,
    destination: &str,
    ends: &str,
    field: Option<&str>,
) -> Result<(), ApiError> {
    let from = concept(source)?;
    let to = concept(destination)?;
    let ends = concept(ends)?;
    let options = MoveOptions {
        update_index: false,
        update_log: false,
        ..MoveOptions::default()
    };
    let moved =
        okf_core::move_concept(bundle, &from, &to, &options).map_err(|error| match error {
            RefactorError::ConceptAlreadyExists(_) => conflict(error.to_string()),
            other => match field {
                Some(field) => {
                    ApiError::new(ErrorCode::InvalidInput, other.to_string()).with_field(field)
                }
                None => internal(format!("moving {source} to {destination} failed: {other}")),
            },
        })?;
    keep_file_links(bundle, &from, &to, &ends, &moved.target_path)?;
    // Every file the move wrote besides the item's own two: the referrers it rewrote.
    for path in &moved.affected_files {
        if *path != moved.source_path
            && *path != moved.target_path
            && let Some(relative) = relative(bundle.root(), path)
        {
            staged.rewritten.insert(relative);
        }
    }
    staged.rescan()
}

/// Write the body of the item okf-core just moved from `from` to `to` again (the module
/// header's workaround for okf-core 0.2.7): its body in `bundle`, the staged tree before the
/// move, rewritten for the move, its links to itself naming `ends` (`moved_body`), into the
/// file okf-core wrote at `written`. The frontmatter is the one okf-core wrote there (its
/// relative paths rebased), read back and written with okf-core's own parser and serializer:
/// its emitter is built so that `parse(emit(v)) == v` (okf-core's `yaml::emitter`), so emitting
/// the value read back writes the frontmatter okf-core wrote (pinned for a rebased `sources`
/// entry by the file-links tests).
fn keep_file_links(
    bundle: &Bundle,
    from: &ConceptId,
    to: &ConceptId,
    ends: &ConceptId,
    written: &Path,
) -> Result<(), ApiError> {
    let before = bundle
        .get(from)
        .ok_or_else(|| internal(format!("the moved concept {from} is not in the bundle")))?;
    let mut document = Document::parse(&read_text(written)?).map_err(|error| {
        internal(format!(
            "the item okf-core moved to {to} does not parse: {error}"
        ))
    })?;
    document.body = moved_body(bundle, &before.document.body, from, to, ends);
    write_text(written, &document.serialize())
}

/// `body`, the body of the concept `from`, with its links rewritten for a move to `to` as
/// `okf_core::move_concept` rewrites them, with two conditions more. First, a link to the
/// concept itself (`link.resolve(from)` is `from`: the literal reading of a percent-encoded
/// target, which okf-core's rewrite of other notes' links takes too) names `ends`, the concept
/// the whole move ends at: a relative one as `compute_relative_path(to, ends)`, an absolute one
/// as `/<ends>.md`, each followed by the link's anchor. A pure anchor (`#sec`) resolves to
/// nothing and is kept. Then the upstream fix's: a relative link that resolves to a concept
/// `bundle` holds becomes a link to that concept (`compute_relative_path`, then the link's
/// anchor); any other relative link, such as one to an image or a file that is not Markdown,
/// whether that file exists or not, is rebased as a path (`rebase_relative_path`), which keeps
/// its name. Every other link is kept.
fn moved_body(
    bundle: &Bundle,
    body: &str,
    from: &ConceptId,
    to: &ConceptId,
    ends: &ConceptId,
) -> String {
    let folder = |id: &ConceptId| {
        id.parent()
            .map(|parent| parent.segments().to_vec())
            .unwrap_or_default()
    };
    let (from_folder, to_folder) = (folder(from), folder(to));
    let (rewritten, _) = rewrite_markdown_links(body, |link, _| {
        let anchor = link
            .target
            .find('#')
            .and_then(|at| link.target.get(at..))
            .unwrap_or_default();
        let resolved = link.resolve(from);
        if resolved.as_ref() == Some(from) {
            return LinkRewriteAction::Rewrite(match link.kind {
                LinkKind::Absolute => format!("/{ends}.md{anchor}"),
                _ => format!("{}{anchor}", compute_relative_path(to, ends)),
            });
        }
        if link.kind != LinkKind::Relative {
            return LinkRewriteAction::Keep;
        }
        LinkRewriteAction::Rewrite(match resolved.filter(|id| bundle.contains(id)) {
            Some(concept) => format!("{}{anchor}", compute_relative_path(to, &concept)),
            None => rebase_relative_path(&from_folder, &to_folder, &link.target),
        })
    });
    rewritten
}

fn set_status(
    staged: &mut Staged,
    item: ItemId,
    status: Option<ItemStatus>,
    archived: Option<bool>,
) -> Result<(), ApiError> {
    let (path, mut file) = staged.read(item)?;
    if let Some(status) = status {
        let word = match status {
            ItemStatus::Draft => "draft",
            ItemStatus::Stable => "stable",
            ItemStatus::Deprecated => "deprecated",
            ItemStatus::Other => {
                return Err(ApiError::new(
                    ErrorCode::InvalidInput,
                    "`other` is kept as a file has it and cannot be set",
                )
                .with_field("/status"));
            }
        };
        file.document
            .frontmatter
            .set("status", okf_core::Value::String(word.to_owned()));
    }
    if let Some(archived) = archived {
        file.header.archived = archived;
        file.store_header()?;
    }
    staged.write(&path, &file)
}

fn set_type(root: &Path, definition: TypeDefinition) -> Result<(), ApiError> {
    refuse_header_in_type(&definition)?;
    let path = root.join(TYPES_FILE);
    let mut types: Vec<TypeDefinition> = if path.exists() {
        serde_json::from_str(&read_text(&path)?)
            .map_err(|error| internal(format!("{TYPES_FILE} does not parse: {error}")))?
    } else {
        Vec::new()
    };
    types.retain(|existing| existing.name != definition.name);
    types.push(definition);
    types.sort_by(|left, right| left.name.cmp(&right.name));
    let text = serde_json::to_string_pretty(&types)
        .map_err(|error| internal(format!("type definitions did not serialize: {error}")))?;
    write_text(&path, &text)
}

/// Create a card, or replace the card with the same identity, keeping its archived flag and
/// any correction recorded for the digest it now shows.
fn write_card(staged: &mut Staged, card: &SourceCard) -> Result<(), ApiError> {
    check_item_path(&card.path, "/path")?;
    staged.refuse_respelled_folder(&card.path, false, "/path")?;
    refuse_header_change(&BTreeMap::new(), &card.properties, "/properties")?;
    let existing = staged.items.get(&card.item_id).cloned();
    let archived = match &existing {
        Some(path) => {
            let (_, file) = staged.read(card.item_id)?;
            remove(&staged.root.join(path))?;
            staged.items.remove(&card.item_id);
            file.header.archived
        }
        None => false,
    };
    if staged.occupied(&card.path, None)? {
        return Err(conflict(format!("{} is already taken", card.path.as_str())));
    }
    let mut header = card.header(archived)?;
    let mut body = card.body.clone();
    if let Some(extraction) = header.extraction.as_mut()
        && let Some(digest) = extraction.digest.clone()
    {
        let correction = correction_path(&staged.root, card.item_id, &digest);
        if correction.exists() {
            let record: serde_json::Value = serde_json::from_str(&read_text(&correction)?)
                .map_err(|error| internal(format!("a correction does not parse: {error}")))?;
            if let Some(text) = record
                .get("corrected_markdown")
                .and_then(serde_json::Value::as_str)
            {
                let adopts = correction_adopts(&record)?;
                extraction.corrected = adopts.is_none();
                if adopts.is_some() {
                    extraction.text_origin = TextOrigin::SuppliedByAgent;
                }
                text.clone_into(&mut body);
            }
        }
    }
    let mut properties = card.properties.clone();
    properties.insert(
        "type".to_owned(),
        serde_json::Value::String(card.type_name.clone()),
    );
    properties.insert(
        "title".to_owned(),
        serde_json::Value::String(card.title.clone()),
    );
    let file = new_item(&properties, &body, header)?;
    let appearance = serde_json::to_string_pretty(&card.appearance)
        .map_err(|error| internal(format!("a source appearance did not serialize: {error}")))?;
    write_text(&appearance_path(&staged.root, card.item_id), &appearance)?;
    staged.write(card.path.as_str(), &file)
}

/// The proposal a stored correction adopts, absent for a human-authored correction.
pub(crate) fn correction_adopts(
    record: &serde_json::Value,
) -> Result<Option<ProposalId>, ApiError> {
    match record.get("adopts") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| {
                internal(format!(
                    "a correction's adopted proposal does not parse: {error}"
                ))
            }),
    }
}

/// Where a card's source appearance is kept.
pub(crate) fn appearance_path(root: &Path, item: ItemId) -> PathBuf {
    root.join(appearance_file(item))
}

/// The tree path of a card's source appearance.
pub(crate) fn appearance_file(item: ItemId) -> String {
    format!("{SOURCES_DIR}/{}.json", item.0)
}

/// The tree path of a correction of one digest.
pub(crate) fn correction_file(item: ItemId, digest: &Digest) -> String {
    format!("{}/{}.json", corrections_folder(item), digest.as_str())
}

/// Whether the tree path `path` is one of the item's companion files, which `RestoreItem` sets
/// back with it (`restore_items`): its source appearance, its accepted supplied text, or a file
/// anywhere in its corrections folder.
pub(crate) fn is_companion(path: &str, item: ItemId) -> bool {
    path == appearance_file(item)
        || path == supplied_file(item)
        || path
            .strip_prefix(&corrections_folder(item))
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The tree path of the folder holding every correction of one item.
fn corrections_folder(item: ItemId) -> String {
    format!("{CORRECTIONS_DIR}/{}", item.0)
}

/// The tree path of accepted agent-supplied text.
pub(crate) fn supplied_file(item: ItemId) -> String {
    format!("{SUPPLIED_DIR}/{}.json", item.0)
}

fn correction_path(root: &Path, item: ItemId, digest: &Digest) -> PathBuf {
    root.join(correction_file(item, digest))
}

fn supplied_path(root: &Path, item: ItemId) -> PathBuf {
    root.join(supplied_file(item))
}

/// The okf-core concept of the item file at `path`: the path with one `.md` taken off, as
/// `ConceptId::from_path` takes it, so the file `a.md.md` is the concept `a.md`.
fn concept(path: &str) -> Result<ConceptId, ApiError> {
    ConceptId::parse(path.strip_suffix(".md").unwrap_or(path)).map_err(|error| {
        ApiError::new(ErrorCode::InvalidInput, format!("{path}: {error}"))
            .with_field("/destination")
    })
}

/// Refuse a path whose last segment the staged tree under `root` holds spelled another way
/// (one collision key, other letters), as a file or a folder: `Conflict` on `field`, naming
/// both. Writing there would replace that file on a filesystem that holds both spellings as
/// one, and add a second spelling on one that does not; removing it would remove that file on
/// the first and nothing on the second.
fn refuse_respelled_file(root: &Path, path: &WorkspacePath, field: &str) -> Result<(), ApiError> {
    let (folder, name) = match path.as_str().rsplit_once('/') {
        Some((folder, name)) => (Some(folder), name),
        None => (None, path.as_str()),
    };
    let directory = folder.map_or_else(|| root.to_path_buf(), |folder| root.join(folder));
    match spelled_in(&directory, name, false)? {
        Some(spelled) if spelled != name => {
            let other =
                folder.map_or_else(|| spelled.clone(), |folder| format!("{folder}/{spelled}"));
            Err(conflict(format!("{} collides with {other}", path.as_str())).with_field(field))
        }
        _ => Ok(()),
    }
}

/// Refuse a path one of whose folders the staged tree under `root` holds as a file, in any
/// spelling: `Conflict` on `field`, naming the path and the file. Writing or removing below a
/// file fails differently by system (Windows reads `notes/a.md` through a file `NOTES` and
/// fails, Linux finds nothing there), so it is refused alike, before anything below it is read.
fn refuse_file_as_folder(root: &Path, path: &WorkspacePath, field: &str) -> Result<(), ApiError> {
    let mut folders: Vec<&str> = path.as_str().split('/').collect();
    folders.pop();
    let mut directory = root.to_path_buf();
    let mut walked = Vec::with_capacity(folders.len());
    for folder in folders {
        let Some(name) = spelled_in(&directory, folder, false)? else {
            return Ok(());
        };
        directory.push(&name);
        walked.push(name);
        if !directory.is_dir() {
            return Err(conflict(format!(
                "{}: {} is a file, not a folder",
                path.as_str(),
                walked.join("/")
            ))
            .with_field(field));
        }
    }
    Ok(())
}

/// How `directory` spells `segment`: the name of its entry named exactly `segment`, or else of
/// one whose collision key is `segment`'s; `None` when it has neither, or does not exist.
/// `folders` counts only its folders.
fn spelled_in(directory: &Path, segment: &str, folders: bool) -> Result<Option<String>, ApiError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("read", directory, &error)),
    };
    let key = collision_key_of(segment);
    let mut other = None;
    for entry in entries {
        let entry = entry.map_err(|error| io_error("read", directory, &error))?;
        let kind = entry
            .file_type()
            .map_err(|error| io_error("inspect", &entry.path(), &error))?;
        if folders && !kind.is_dir() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| internal("a staged file name is not UTF-8"))?;
        if name == segment {
            return Ok(Some(name));
        }
        if collision_key_of(&name) == key {
            other = Some(name);
        }
    }
    Ok(other)
}

/// `path` as a `/`-separated path relative to `root`; `None` when it is not under `root` or a
/// part of it is not UTF-8.
fn relative(root: &Path, path: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|part| match part {
            std::path::Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    parts.map(|parts| parts.join("/"))
}

/// Every file under `root`, as `/`-separated paths relative to it.
pub(crate) fn files_under(root: &Path, prefix: &str) -> Result<Vec<String>, ApiError> {
    let mut found = Vec::new();
    walk(root, prefix, false, &mut found)?;
    Ok(found.into_iter().map(|(path, _)| path).collect())
}

/// Every file and every folder under `root`, empty folders included, as `/`-separated paths
/// relative to it, each with whether it is a folder.
fn entries_under(root: &Path) -> Result<Vec<(String, bool)>, ApiError> {
    let mut found = Vec::new();
    walk(root, "", true, &mut found)?;
    Ok(found)
}

/// Add every file under `prefix` of `root` to `found`, and with `folders` every folder too.
fn walk(
    root: &Path,
    prefix: &str,
    folders: bool,
    found: &mut Vec<(String, bool)>,
) -> Result<(), ApiError> {
    let directory = if prefix.is_empty() {
        root.to_path_buf()
    } else {
        root.join(prefix)
    };
    for entry in
        std::fs::read_dir(&directory).map_err(|error| io_error("read", &directory, &error))?
    {
        let entry = entry.map_err(|error| io_error("read", &directory, &error))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| internal("a staged file name is not UTF-8"))?;
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let kind = entry
            .file_type()
            .map_err(|error| io_error("inspect", &entry.path(), &error))?;
        if kind.is_dir() {
            walk(root, &relative, folders, found)?;
            if folders {
                found.push((relative, true));
            }
        } else if kind.is_file() {
            found.push((relative, false));
        }
    }
    Ok(())
}

fn read_text(path: &Path) -> Result<String, ApiError> {
    std::fs::read_to_string(path).map_err(|error| io_error("read", path, &error))
}

fn write_text(path: &Path, text: &str) -> Result<(), ApiError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| io_error("create", parent, &error))?;
    }
    std::fs::write(path, text).map_err(|error| io_error("write", path, &error))
}

fn remove(path: &Path) -> Result<(), ApiError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error("remove", path, &error)),
    }
}

/// Remove the folder at `path` with everything under it; an absent folder is no error.
fn remove_folder(path: &Path) -> Result<(), ApiError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error("remove", path, &error)),
    }
}

fn clear_directory(root: &Path) -> Result<(), ApiError> {
    for entry in std::fs::read_dir(root).map_err(|error| io_error("read", root, &error))? {
        let path = entry
            .map_err(|error| io_error("read", root, &error))?
            .path();
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        removed.map_err(|error| io_error("remove", &path, &error))?;
    }
    Ok(())
}

fn conflict(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Conflict, message)
}
