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

use std::collections::BTreeMap;

use okf_core::yaml::{Mapping, Value as Yaml};
use okf_core::{Document, Frontmatter};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{ItemId, Revision, WorkspacePath};
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

/// The application's private directory inside a workspace tree.
pub(crate) const APP_DIR: &str = ".okf";
/// Naming rules file.
pub(crate) const RULES_FILE: &str = ".okf/rules.yaml";
/// Type definitions file.
pub(crate) const TYPES_FILE: &str = ".okf/types.json";
/// The maintained change log at the root.
pub(crate) const LOG_FILE: &str = "log.md";

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
        self.document
            .frontmatter
            .set(APP_HEADER_KEY, json_to_yaml(&self.header.to_property()?)?);
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
                path.as_str()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches(".md")
                    .to_owned()
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

/// Whether a tree path is an item file: Markdown and not reserved (`is_reserved`).
pub(crate) fn is_item_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        && !is_reserved(path)
}

/// Whether a tree path is one the server keeps for itself: anything under `.okf/`, or a file
/// named `index.md` or `log.md` at any depth. Compared by collision key (NFC, then lower case),
/// on every platform: on a case-insensitive filesystem `notes/INDEX.md` is the folder index
/// that every commit regenerates, so an item there would be overwritten.
pub(crate) fn is_reserved(path: &str) -> bool {
    let key = collision_key(path);
    let first = key.split('/').next().unwrap_or_default();
    let name = key.rsplit('/').next().unwrap_or_default();
    first == APP_DIR
        || okf_core::RESERVED_FILENAMES
            .iter()
            .any(|reserved| collision_key(reserved) == name)
}

/// The key two paths collide on (`WorkspacePath::collision_key`); a path that is not a valid
/// workspace path is lower-cased as it is.
pub(crate) fn collision_key(path: &str) -> String {
    WorkspacePath::try_from(path.to_owned())
        .map_or_else(|_| path.to_lowercase(), |valid| valid.collision_key())
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
            frontmatter.set(key.clone(), json_to_yaml(value)?);
        }
    }
    frontmatter.set(APP_HEADER_KEY, json_to_yaml(&header.to_property()?)?);
    frontmatter.reorder_preferred();
    Ok(frontmatter)
}

/// A JSON value as okf-core's YAML value; a number outside `i64` and `f64` is refused.
pub(crate) fn json_to_yaml(value: &Json) -> Result<Yaml, ApiError> {
    Ok(match value {
        Json::Null => Yaml::Null,
        Json::Bool(flag) => Yaml::Bool(*flag),
        Json::Number(number) => match (number.as_i64(), number.as_f64()) {
            (Some(integer), _) => Yaml::Int(integer),
            (None, Some(float)) => Yaml::Float(float),
            (None, None) => {
                return Err(ApiError::new(
                    ErrorCode::InvalidInput,
                    "a property number is outside what the item file can hold",
                ));
            }
        },
        Json::String(text) => Yaml::String(text.clone()),
        Json::Array(items) => Yaml::Sequence(
            items
                .iter()
                .map(json_to_yaml)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Json::Object(map) => {
            let mut mapping = Mapping::new();
            for (key, child) in map {
                mapping.insert(key.clone(), json_to_yaml(child)?);
            }
            Yaml::Mapping(mapping)
        }
    })
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

/// Refuse, naming it, a path that cannot hold an item: not Markdown, or reserved in any letter
/// case (`is_reserved`). `field` is the request field that named the path.
pub(crate) fn check_item_path(path: &WorkspacePath, field: &str) -> Result<(), ApiError> {
    if is_reserved(path.as_str()) {
        return Err(ApiError::new(
            ErrorCode::InvalidInput,
            format!(
                "{} is reserved: .okf/, index.md and log.md (in any letter case) are kept by \
                 the server and cannot hold an item",
                path.as_str()
            ),
        )
        .with_field(field));
    }
    if is_item_path(path.as_str()) {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::InvalidInput,
            format!(
                "{} is not a Markdown file, so it cannot hold an item",
                path.as_str()
            ),
        )
        .with_field(field))
    }
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
