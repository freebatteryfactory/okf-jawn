//! Applying `TreeEdit`s to a staged candidate tree with okf-core.
//!
//! Edits run in order on the staging directory. A caller edit may not change an application
//! header (`items::refuse_header_change`, `items::refuse_header_in_type`); the server's own
//! header updates (`SetStatus`, `WriteSourceCard`, `CorrectDigest`, `SupplyExtraction`) may.
//! Moves rewrite the links that point at the item (`okf_core::move_concept`). An item, a move
//! or a folder may not land on a reserved name (`.okf/`, `index.md`, `log.md`) in any letter
//! case: it is refused naming the path before anything is written. After the edits, no two
//! files may share a path collision key, the folder indexes are regenerated and the change is
//! appended to `log.md`. Conformance is the `CandidateCheck`'s decision, not this
//! module's.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use git2::Repository;
use okf_core::{Bundle, ConceptId, MoveOptions, RefactorError};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::TextOrigin;
use okf_jawn_contract::identity::{Digest, ItemId, WorkspacePath};
use okf_jawn_contract::item::{ItemStatus, TypeDefinition};
use okf_jawn_core::items::{ApplicationHeader, refuse_header_change, refuse_header_in_type};
use okf_jawn_core::storage::{SourceCard, TreeEdit};

use super::item::{
    APP_DIR, ItemFile, LOG_FILE, RULES_FILE, TYPES_FILE, bare_header, check_item_path,
    collision_key, is_item_path, is_reserved, new_item,
};
use super::repo::{commit_of, internal, materialize};
use crate::data::io_error;
use crate::seam;

/// The items of a staged tree by identity.
pub(crate) struct Staged {
    root: PathBuf,
    items: BTreeMap<ItemId, String>,
}

impl Staged {
    /// Index the items of a staging directory.
    pub(crate) fn scan(root: &Path) -> Result<Self, ApiError> {
        let mut staged = Self {
            root: root.to_path_buf(),
            items: BTreeMap::new(),
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

    /// Refuse two files of the staged tree whose paths share a collision key (Stage 1b design
    /// section 7), so a tree never holds two files a case-insensitive filesystem would merge.
    pub(crate) fn refuse_collisions(&self) -> Result<(), ApiError> {
        for path in self.items.values() {
            WorkspacePath::try_from(path.clone()).map_err(|error| {
                ApiError::new(ErrorCode::InvalidInput, format!("{path}: {error}"))
            })?;
        }
        let mut keys = BTreeMap::new();
        for path in files_under(&self.root, "")? {
            if let Some(other) = keys.insert(collision_key(&path), path.clone()) {
                return Err(ApiError::new(
                    ErrorCode::Conflict,
                    format!("{path} collides with {other}"),
                ));
            }
        }
        Ok(())
    }

    /// Whether a file or folder of the staged tree, or an item, already holds `path`'s
    /// collision key (or a folder above it holds a file's).
    fn occupied(&self, path: &WorkspacePath) -> Result<bool, ApiError> {
        let key = path.collision_key();
        if self.root.join(path.as_str()).exists() {
            return Ok(true);
        }
        let folder_prefix = format!("{key}/");
        // Every folder the path would need: a file already holding one of those keys blocks it.
        let folders: Vec<&str> = key
            .match_indices('/')
            .filter_map(|(at, _)| key.get(..at))
            .collect();
        Ok(files_under(&self.root, "")?.iter().any(|existing| {
            let existing = collision_key(existing);
            existing == key
                || existing.starts_with(&folder_prefix)
                || folders.contains(&existing.as_str())
        }) || self
            .items
            .values()
            .any(|existing| collision_key(existing) == key))
    }
}

/// Apply `edits` in order to the staged tree.
pub(crate) fn apply(
    repository: &Repository,
    staged: &mut Staged,
    edits: Vec<TreeEdit>,
) -> Result<(), ApiError> {
    for edit in edits {
        apply_one(repository, staged, edit)?;
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
        } => move_item(staged, item_id, &destination),
        TreeEdit::SetStatus {
            item_id,
            status,
            archived,
        } => set_status(staged, item_id, status, archived),
        TreeEdit::DeleteItem { item_id } => {
            let path = staged.path_of(item_id)?;
            remove(&staged.root.join(&path))?;
            staged.items.remove(&item_id);
            Ok(())
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
        TreeEdit::RestorePaths { from, paths } => restore_paths(repository, staged, &from, &paths),
        TreeEdit::RestoreWorkspace { from } => {
            let commit = commit_of(repository, &from)?;
            let tree = commit.tree().map_err(|error| super::repo::git(&error))?;
            clear_directory(&staged.root)?;
            materialize(repository, &tree, &staged.root)?;
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
    refuse_header_change(&BTreeMap::new(), &properties, "/properties")?;
    if staged.items.contains_key(&header.item_id) {
        return Err(conflict("an item with this identity already exists"));
    }
    if staged.occupied(path)? {
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
    let target = staged.root.join(folder.as_str());
    if target.exists() || staged.occupied(folder)? {
        return Err(conflict(format!("{} is already taken", folder.as_str())));
    }
    std::fs::create_dir_all(&target).map_err(|error| io_error("create", &target, &error))?;
    write_text(
        &target.join("index.md"),
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
        extraction.corrected = true;
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
    from: &okf_jawn_contract::identity::Revision,
    paths: &[WorkspacePath],
) -> Result<(), ApiError> {
    let commit = commit_of(repository, from)?;
    let tree = commit.tree().map_err(|error| super::repo::git(&error))?;
    for path in paths {
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
) -> Result<(), ApiError> {
    check_item_path(destination, "/destination")?;
    let source = staged.path_of(item)?;
    if source == destination.as_str() {
        return Ok(());
    }
    if staged.occupied(destination)? {
        return Err(conflict(format!(
            "{} is already taken",
            destination.as_str()
        )));
    }
    let bundle = Bundle::load(&staged.root)
        .map_err(|error| internal(format!("the staged tree did not load as a bundle: {error}")))?;
    let from = concept(&source)?;
    let to = concept(destination.as_str())?;
    let options = MoveOptions {
        update_index: false,
        update_log: false,
        ..MoveOptions::default()
    };
    okf_core::move_concept(&bundle, &from, &to, &options).map_err(|error| match error {
        RefactorError::ConceptAlreadyExists(_) => conflict(error.to_string()),
        other => {
            ApiError::new(ErrorCode::InvalidInput, other.to_string()).with_field("/destination")
        }
    })?;
    staged.rescan()
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
    if staged.occupied(&card.path)? {
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
                extraction.corrected = true;
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

/// Where a card's source appearance is kept.
pub(crate) fn appearance_path(root: &Path, item: ItemId) -> PathBuf {
    root.join(appearance_file(item))
}

/// The tree path of a card's source appearance.
pub(crate) fn appearance_file(item: ItemId) -> String {
    format!("{APP_DIR}/sources/{}.json", item.0)
}

/// The tree path of a correction of one digest.
pub(crate) fn correction_file(item: ItemId, digest: &Digest) -> String {
    format!("{APP_DIR}/corrections/{}/{}.json", item.0, digest.as_str())
}

/// The tree path of accepted agent-supplied text.
pub(crate) fn supplied_file(item: ItemId) -> String {
    format!("{APP_DIR}/supplied/{}.json", item.0)
}

fn correction_path(root: &Path, item: ItemId, digest: &Digest) -> PathBuf {
    root.join(correction_file(item, digest))
}

fn supplied_path(root: &Path, item: ItemId) -> PathBuf {
    root.join(supplied_file(item))
}

fn concept(path: &str) -> Result<ConceptId, ApiError> {
    ConceptId::parse(path.trim_end_matches(".md")).map_err(|error| {
        ApiError::new(ErrorCode::InvalidInput, format!("{path}: {error}"))
            .with_field("/destination")
    })
}

/// Every file under `root`, as `/`-separated paths relative to it.
pub(crate) fn files_under(root: &Path, prefix: &str) -> Result<Vec<String>, ApiError> {
    let mut found = Vec::new();
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
            found.extend(files_under(root, &relative)?);
        } else if kind.is_file() {
            found.push(relative);
        }
    }
    Ok(found)
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
