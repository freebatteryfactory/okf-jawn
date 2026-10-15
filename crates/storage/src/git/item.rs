//! Item files: an OKF document whose frontmatter carries the application header.
//!
//! The property map of the API is the frontmatter, converted between JSON and okf-core's YAML
//! value. Side records live under `.okf/`, which is never an item path: the source appearance of
//! a card (`.okf/sources/<item>.json`), human corrections
//! (`.okf/corrections/<item>/<digest>.json`), accepted agent-supplied text
//! (`.okf/supplied/<item>.json`), type definitions (`.okf/types.json`) and naming rules
//! (`.okf/rules.yaml`).
//!
//! An item's rendering role is recorded in its header (`ApplicationHeader::kind`) when it is
//! created and read back from there, never inferred from the body: a Note that shows a fenced
//! vega-lite example stays a Note.
//!
//! An item's file ends in `.md`, in lower case (`is_item_path`), because the OKF format reads
//! only that: okf-core's bundle collects a file whose extension is exactly `md`, and
//! `ConceptId::to_path` writes `.md`. A file `X.MD` is a plain file, kept as it is; a write
//! that names one for an item is refused with the lower-case name (`check_item_path`).

use std::collections::{BTreeMap, BTreeSet};

use okf_core::yaml::{Mapping, Value as Yaml};
use okf_core::{Document, Frontmatter};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, ItemId, Revision, WorkspacePath};
use okf_jawn_contract::item::{APP_HEADER_KEY, ItemKind, ItemStatus, ItemSummary};
use okf_jawn_contract::source::SourceAppearance;
use okf_jawn_core::items::ApplicationHeader;
use serde_json::Value as Json;

/// One parsed item file.
#[derive(Debug, Clone)]
pub(crate) struct ItemFile {
    pub(crate) document: Document,
    pub(crate) header: ApplicationHeader,
}

/// Whose tree a refusal of a held respelling names (`Respelled::held`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Holder {
    /// The base a write stages: the head, or the base of a proposal's candidate.
    Head,
    /// The earlier revision a Rewind restores (`RestoreItem`, `RestoreWorkspace`).
    Restored,
}

/// What the store refuses to find in a tree (`respelled`): two spellings of one path, or a path
/// the server writes spelled another way.
#[derive(Debug)]
pub(crate) struct Respelled {
    /// The tree's spelling: a file, or a folder above one.
    path: String,
    /// The other spelling: another path of the tree with the same collision key, or, for
    /// `server`, the server's spelling, which the tree need not hold.
    other: String,
    /// Whether `other` is the spelling of a name the server writes (`SERVER_NAMES`).
    server: bool,
}

/// The application's private directory inside a workspace tree.
pub(crate) const APP_DIR: &str = ".okf";
/// Naming rules file.
pub(crate) const RULES_FILE: &str = ".okf/rules.yaml";
/// Type definitions file.
pub(crate) const TYPES_FILE: &str = ".okf/types.json";
/// The folder of the cards' source appearances, `<item id>.json` each.
pub(crate) const SOURCES_DIR: &str = ".okf/sources";
/// The folder of accepted agent-supplied text, `<item id>.json` each.
pub(crate) const SUPPLIED_DIR: &str = ".okf/supplied";
/// The folder of human corrections, `<item id>/<digest>.json` each.
pub(crate) const CORRECTIONS_DIR: &str = ".okf/corrections";
/// A folder's generated index, in every folder.
pub(crate) const INDEX_FILE: &str = "index.md";
/// The maintained change log at the root.
pub(crate) const LOG_FILE: &str = "log.md";
/// Every fixed name the server writes, spelled exactly as it writes it: the paths from the root
/// of `.okf/` and of what it keeps there (type definitions, naming rules, source appearances,
/// supplied text, corrections), the root's change log, and the index it writes in every folder.
/// Below `sources/`, `supplied/` and `corrections/` it also writes names made of an item's
/// identity and a digest, in lower case like these (`server_writes`). A tree that holds one of
/// these, or one of those, spelled another way is refused (`respelled`), so every path the
/// server writes has one spelling on every filesystem.
pub(crate) const SERVER_NAMES: [&str; 8] = [
    APP_DIR,
    TYPES_FILE,
    RULES_FILE,
    SOURCES_DIR,
    SUPPLIED_DIR,
    CORRECTIONS_DIR,
    LOG_FILE,
    INDEX_FILE,
];
/// The extension of an item's file, exactly: the only one okf-core reads as a concept.
const ITEM_EXTENSION: &str = "md";
/// The request field of the server-owned header.
const HEADER_FIELD: &str = "/properties/okf_jawn";

impl ItemFile {
    /// Parse an item file; `None` when the text has no application header (it is then not an
    /// item, such as a folder `index.md`).
    pub(crate) fn parse(text: &str) -> Result<Option<Self>, ApiError> {
        let document = Document::parse(text).map_err(|error| {
            ApiError::new(
                ErrorCode::Internal,
                format!("a committed item file does not parse: {error}"),
            )
        })?;
        let properties = properties_of(&document);
        let Some(header) = ApplicationHeader::from_properties(&properties)? else {
            return Ok(None);
        };
        Ok(Some(Self { document, header }))
    }

    /// The API's property map, header included.
    pub(crate) fn properties(&self) -> BTreeMap<String, Json> {
        properties_of(&self.document)
    }

    /// Replace the frontmatter with `properties`, keeping this file's header.
    pub(crate) fn set_properties(
        &mut self,
        properties: &BTreeMap<String, Json>,
    ) -> Result<(), ApiError> {
        self.document.frontmatter = frontmatter_of(properties, &self.header)?;
        Ok(())
    }

    /// Write this file's header back into its frontmatter.
    pub(crate) fn store_header(&mut self) -> Result<(), ApiError> {
        self.document.frontmatter.set(
            APP_HEADER_KEY,
            json_to_yaml(&self.header.to_property()?, HEADER_FIELD)?,
        );
        Ok(())
    }

    /// The file text.
    pub(crate) fn text(&self) -> String {
        self.document.serialize()
    }

    /// The navigation summary at `revision`.
    pub(crate) fn summary(
        &self,
        path: &WorkspacePath,
        revision: &Revision,
        appearance: Option<&SourceAppearance>,
    ) -> ItemSummary {
        let frontmatter = &self.document.frontmatter;
        let title = frontmatter.title().map_or_else(
            || {
                // One `.md` comes off, so `a.md.md` is titled `a.md`.
                let name = path.as_str().rsplit('/').next().unwrap_or_default();
                name.strip_suffix(".md").unwrap_or(name).to_owned()
            },
            std::borrow::Cow::into_owned,
        );
        ItemSummary {
            id: self.header.item_id,
            path: path.clone(),
            title,
            description: frontmatter
                .description()
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default(),
            type_name: frontmatter
                .type_()
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default(),
            kind: self.kind(),
            revision: revision.clone(),
            status: status_of(&frontmatter.status()),
            archived: self.header.archived,
            media_type: appearance.map(|appearance| appearance.media_type.clone()),
            extraction: self
                .header
                .extraction
                .as_ref()
                .map(okf_jawn_contract::extraction::Extraction::summary),
        }
    }

    /// The rendering role recorded in the header (see the module header).
    pub(crate) fn kind(&self) -> ItemKind {
        self.header.kind.clone()
    }
}

impl Respelled {
    /// The refusal of edits that would make a tree hold this (`Staged::refuse_collisions`):
    /// `Conflict`, naming both spellings.
    pub(crate) fn made(&self) -> ApiError {
        ApiError::new(ErrorCode::Conflict, self.clash())
    }

    /// The refusal of a revision that already holds this (`repo::materialize`): `Conflict`,
    /// naming both spellings, what is wrong with them (`what`), and the repair. For the base a
    /// write stages (`Holder::Head`) the repair is a whole-tree restore of a revision from before
    /// they appeared; for the revision a Rewind restores (`Holder::Restored`) the message says
    /// it is that revision that holds them, and the repair is to choose an earlier one.
    pub(crate) fn held(&self, holder: Holder) -> ApiError {
        let appeared = if self.server {
            shown(&self.path, self.encoded())
        } else {
            "they".to_owned()
        };
        let message = match holder {
            Holder::Head => format!(
                "{}: the revision {}; the workspace can be restored to a revision from before \
                 {appeared} appeared",
                self.clash(),
                self.what()
            ),
            Holder::Restored => format!(
                "{}: the revision being restored {}; choose a revision from before {appeared} \
                 appeared",
                self.clash(),
                self.what()
            ),
        };
        ApiError::new(ErrorCode::Conflict, message)
    }

    /// Both spellings, each escaped when they differ beyond letter case (`shown`).
    fn clash(&self) -> String {
        let encoded = self.encoded();
        let (path, other) = (shown(&self.path, encoded), shown(&self.other, encoded));
        if self.server {
            format!("{path} collides with the server's {other}")
        } else {
            format!("{path} collides with {other}")
        }
    }

    /// What is wrong with the two spellings, after "the revision": a path the server writes
    /// spelled another way, two spellings that differ only in letter case (Windows and macOS
    /// keep them as one), or two names that differ in how their characters are encoded, Unicode
    /// normalization (only macOS keeps those as one), alone or with letter case.
    fn what(&self) -> &'static str {
        if self.server {
            "spells a path the server writes another way, which Windows and macOS would merge with \
             it"
        } else if !self.encoded() {
            "holds two spellings of one path, which Windows and macOS keep as one"
        } else if nfc(&self.path) == nfc(&self.other) {
            "holds two names that differ only in how their characters are encoded, which macOS \
             keeps as one"
        } else {
            "holds two names that differ in letter case and in how their characters are encoded, \
             which macOS keeps as one"
        }
    }

    /// Whether the two spellings differ beyond letter case: in how their characters are
    /// encoded, which only normalization makes one (they share a collision key).
    fn encoded(&self) -> bool {
        self.path.to_lowercase() != self.other.to_lowercase()
    }
}

/// A new item file from its properties, body and header.
pub(crate) fn new_item(
    properties: &BTreeMap<String, Json>,
    body: &str,
    header: ApplicationHeader,
) -> Result<ItemFile, ApiError> {
    let frontmatter = frontmatter_of(properties, &header)?;
    Ok(ItemFile {
        document: Document::new(frontmatter, body),
        header,
    })
}

/// The OKF status word as the API names it.
pub(crate) fn status_of(status: &okf_core::Status) -> ItemStatus {
    match status {
        okf_core::Status::Draft => ItemStatus::Draft,
        okf_core::Status::Stable => ItemStatus::Stable,
        okf_core::Status::Deprecated => ItemStatus::Deprecated,
        okf_core::Status::Other(_) => ItemStatus::Other,
    }
}

/// Whether a tree path is an item file: its extension is exactly `md`, in lower case (a file
/// `X.MD` is a plain file, see the module header), and it is not reserved (`is_reserved`).
pub(crate) fn is_item_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|extension| extension == ITEM_EXTENSION)
        && !is_reserved(path)
}

/// `path` with its extension written `md`, when that extension is `md` in another letter case.
fn in_lower_case(path: &str) -> Option<String> {
    let extension = std::path::Path::new(path).extension()?;
    if extension == ITEM_EXTENSION || !extension.eq_ignore_ascii_case(ITEM_EXTENSION) {
        return None;
    }
    // The extension is the last two bytes, both ASCII, so the cut is a character boundary.
    let stem = path.get(..path.len().checked_sub(ITEM_EXTENSION.len())?)?;
    Some(format!("{stem}{ITEM_EXTENSION}"))
}

/// Whether a tree path is one the server keeps for itself or one that would collide with such
/// a path: any segment, at any depth, that is `.okf`, `index.md` or `log.md`. Compared by
/// collision key (NFC, then lower case) on every platform: on a case-insensitive filesystem
/// `notes/INDEX.md` is the folder index every commit regenerates, so an item there would be
/// overwritten, and a folder `a/INDEX.MD/` would merge with the file `a/index.md`.
pub(crate) fn is_reserved(path: &str) -> bool {
    collision_key(path).split('/').any(|segment| {
        segment == APP_DIR
            || okf_core::RESERVED_FILENAMES
                .iter()
                .any(|reserved| collision_key(reserved) == segment)
    })
}

/// What a tree of `files` (every file of the tree, as `/`-separated paths) holds that a
/// case-insensitive filesystem would merge, if anything. First two of the files, or of the
/// folders above them, that share a collision key but are spelled differently: files and
/// folders count alike, so `notes/a.md` and `Notes/b.md` collide on their folders, and `a.md`
/// and `A.md/x.md` collide too. Else a file, or a folder above one, whose key is that of a name
/// the server writes there but whose spelling is not the server's (`SERVER_NAMES`): the
/// server's next write of that name would land in it on such a filesystem and beside it on
/// another.
pub(crate) fn respelled<'a>(files: impl IntoIterator<Item = &'a str>) -> Option<Respelled> {
    let files: Vec<&str> = files.into_iter().collect();
    let mut paths = BTreeSet::new();
    for file in &files {
        paths.insert(*file);
        paths.extend(file.match_indices('/').filter_map(|(at, _)| file.get(..at)));
    }
    // Every spelling once, so two paths with one key are two spellings of it.
    let mut keys = BTreeMap::new();
    for path in paths {
        if let Some(other) = keys.insert(collision_key(path), path) {
            return Some(Respelled {
                path: path.to_owned(),
                other: other.to_owned(),
                server: false,
            });
        }
    }
    files.into_iter().find_map(respelled_server_name)
}

/// The first of `path` and the folders above it whose key is that of a name the server writes
/// there (`server_writes`) but whose spelling is not the server's, with the server's spelling:
/// `.OKF/x.json` gives `.OKF` and `.okf`; `notes/INDEX.md` gives itself and `notes/index.md`.
/// The server spells every name it writes in lower case, so its spelling is the name's key.
fn respelled_server_name(path: &str) -> Option<Respelled> {
    let mut tree = String::new();
    let mut server = String::new();
    for segment in path.split('/') {
        let key = collision_key(segment);
        let written = server_writes(&server, &key);
        if !tree.is_empty() {
            tree.push('/');
            server.push('/');
        }
        tree.push_str(segment);
        server.push_str(if written { &key } else { segment });
        if written && key != segment {
            return Some(Respelled {
                path: tree,
                other: server,
                server: true,
            });
        }
    }
    None
}

/// Whether the server writes a name whose collision key is `key` in `folder` (as the server
/// spells it; empty for the root): a name of `SERVER_NAMES` there, `index.md` in any folder,
/// `<item id>.json` in the source appearances and the supplied text, and `<item id>/` with
/// `<digest>.json` in it in the corrections.
fn server_writes(folder: &str, key: &str) -> bool {
    let path = if folder.is_empty() {
        key.to_owned()
    } else {
        format!("{folder}/{key}")
    };
    let item_file = || key.strip_suffix(".json").is_some_and(is_item_id);
    key == INDEX_FILE
        || SERVER_NAMES.contains(&path.as_str())
        || match folder {
            SOURCES_DIR | SUPPLIED_DIR => item_file(),
            CORRECTIONS_DIR => is_item_id(key),
            _ => {
                folder
                    .strip_prefix(CORRECTIONS_DIR)
                    .and_then(|below| below.strip_prefix('/'))
                    .is_some_and(is_item_id)
                    && key
                        .strip_suffix(".json")
                        .is_some_and(|digest| Digest::try_from(digest.to_owned()).is_ok())
            }
        }
}

/// Whether `text` is an item's identity as the server writes it in a path: the lower-case,
/// hyphenated UUID `ItemId` displays.
fn is_item_id(text: &str) -> bool {
    uuid::Uuid::parse_str(text).is_ok_and(|id| id.to_string() == text)
}

/// The key two paths collide on (`WorkspacePath::collision_key`: NFC, then lower case), for a
/// path of a tree whether or not it is a valid workspace path. A path that is not one (not in
/// NFC, or with a segment Windows refuses) is keyed segment by segment: a segment that is a
/// valid path once in NFC (`WorkspacePath::from_supplied`, contract's only NFC) gets its key,
/// so `café` and `cafe` with a combining accent collide; one that is not valid even in NFC (a
/// reserved character, a trailing dot or space, a device name) is lower-cased without NFC.
pub(crate) fn collision_key(path: &str) -> String {
    WorkspacePath::try_from(path.to_owned()).map_or_else(
        |_| {
            path.split('/')
                .map(|segment| {
                    WorkspacePath::from_supplied(segment)
                        .map_or_else(|_| segment.to_lowercase(), |valid| valid.collision_key())
                })
                .collect::<Vec<_>>()
                .join("/")
        },
        |valid| valid.collision_key(),
    )
}

/// `path` in NFC, segment by segment as `collision_key` normalizes it: a segment that is not a
/// valid path even in NFC is kept as it is.
fn nfc(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            WorkspacePath::from_supplied(segment)
                .map_or_else(|_| segment.to_owned(), |valid| valid.as_str().to_owned())
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// `path` as a refusal shows it: with `escaped`, every character that is not ASCII written as
/// its code point (`\u{e9}`), so two names that differ only in how their characters are
/// encoded do not print alike.
fn shown(path: &str, escaped: bool) -> String {
    if !escaped {
        return path.to_owned();
    }
    path.chars()
        .map(|character| {
            if character.is_ascii() {
                character.to_string()
            } else {
                character.escape_unicode().to_string()
            }
        })
        .collect()
}

fn properties_of(document: &Document) -> BTreeMap<String, Json> {
    document
        .frontmatter
        .iter()
        .filter_map(|(key, value)| {
            key.as_str()
                .map(|key| (key.to_owned(), yaml_to_json(value)))
        })
        .collect()
}

/// Frontmatter holding `properties` (any header in them replaced by `header`), in OKF's
/// preferred key order.
fn frontmatter_of(
    properties: &BTreeMap<String, Json>,
    header: &ApplicationHeader,
) -> Result<Frontmatter, ApiError> {
    let mut frontmatter = Frontmatter::new();
    for (key, value) in properties {
        if key != APP_HEADER_KEY {
            frontmatter.set(
                key.clone(),
                json_to_yaml(value, &pointer("/properties", key))?,
            );
        }
    }
    frontmatter.set(
        APP_HEADER_KEY,
        json_to_yaml(&header.to_property()?, HEADER_FIELD)?,
    );
    frontmatter.reorder_preferred();
    Ok(frontmatter)
}

/// A JSON value as okf-core's YAML value, exactly. `field` is the JSON pointer of the value
/// in the request.
///
/// okf-core's YAML integer is an `i64`, so an unsigned integer above `i64::MAX` has no exact
/// form in the item file. It is refused as `InvalidInput` on its field rather than rounded
/// through `f64`: a silently changed value would change the item's content digest.
pub(crate) fn json_to_yaml(value: &Json, field: &str) -> Result<Yaml, ApiError> {
    Ok(match value {
        Json::Null => Yaml::Null,
        Json::Bool(flag) => Yaml::Bool(*flag),
        Json::Number(number) => {
            if let Some(integer) = number.as_i64() {
                Yaml::Int(integer)
            } else if number.is_f64()
                && let Some(float) = number.as_f64()
            {
                Yaml::Float(float)
            } else {
                return Err(ApiError::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "the number {number} is an integer outside -2^63 to 2^63-1, which the                          item file cannot hold exactly; send it as a string"
                    ),
                )
                .with_field(field));
            }
        }
        Json::String(text) => Yaml::String(text.clone()),
        Json::Array(items) => Yaml::Sequence(
            items
                .iter()
                .enumerate()
                .map(|(index, item)| json_to_yaml(item, &format!("{field}/{index}")))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Json::Object(map) => {
            let mut mapping = Mapping::new();
            for (key, child) in map {
                mapping.insert(key.clone(), json_to_yaml(child, &pointer(field, key))?);
            }
            Yaml::Mapping(mapping)
        }
    })
}

/// `base` extended by the JSON pointer token `key` (RFC 6901 escaping).
fn pointer(base: &str, key: &str) -> String {
    format!("{base}/{}", key.replace('~', "~0").replace('/', "~1"))
}

/// okf-core's YAML value as JSON; a non-finite float reads as null.
pub(crate) fn yaml_to_json(value: &Yaml) -> Json {
    match value {
        Yaml::Null => Json::Null,
        Yaml::Bool(flag) => Json::Bool(*flag),
        Yaml::Int(integer) => Json::from(*integer),
        Yaml::Float(float) => serde_json::Number::from_f64(*float).map_or(Json::Null, Json::Number),
        Yaml::String(text) => Json::String(text.clone()),
        Yaml::Sequence(items) => Json::Array(items.iter().map(yaml_to_json).collect()),
        Yaml::Mapping(mapping) => Json::Object(
            mapping
                .iter()
                .filter_map(|(key, child)| {
                    key.as_display_string()
                        .map(|key| (key, yaml_to_json(child)))
                })
                .collect(),
        ),
    }
}

/// Refuse, naming it, a path that cannot hold an item: reserved in any letter case
/// (`is_reserved`), ending in `.md` in another letter case (refused naming the lower-case
/// name), or not Markdown. `field` is the request field that named the path.
pub(crate) fn check_item_path(path: &WorkspacePath, field: &str) -> Result<(), ApiError> {
    if is_reserved(path.as_str()) {
        return Err(ApiError::new(
            ErrorCode::InvalidInput,
            format!(
                "{} is reserved: .okf, index.md and log.md (in any letter case, as any path \
                 segment) are kept by the server and cannot hold an item",
                path.as_str()
            ),
        )
        .with_field(field));
    }
    if is_item_path(path.as_str()) {
        return Ok(());
    }
    let message = match in_lower_case(path.as_str()) {
        Some(named) => format!("a note's file ends in `.md` in lower case: name it {named}"),
        None => format!(
            "{} is not a Markdown file, so it cannot hold an item",
            path.as_str()
        ),
    };
    Err(ApiError::new(ErrorCode::InvalidInput, message).with_field(field))
}

/// The header a created item is written with: its identity and the rendering role its
/// creation names.
pub(crate) const fn bare_header(item_id: ItemId, kind: ItemKind) -> ApplicationHeader {
    ApplicationHeader {
        item_id,
        kind,
        archived: false,
        source: None,
        extraction: None,
    }
}
