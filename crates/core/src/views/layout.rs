//! The file a View is stored in (core M1.6 design section 2; owner decision T1): an OKF concept
//! whose `title` property is the View's title and whose body is the View's description as
//! Markdown, then one fenced code block whose opening line is exactly ```` ```json okf-jawn-view ````
//! and which ends the body. The block is a JSON object holding the rest of the document:
//! `schema_version`, `mode`, `grammar`, `bindings`, `spec` and `charts` (optional, empty when
//! absent).
//!
//! A View file names no workspace (ruling E1). A binding in the block is its citation without
//! `workspace_id` (a saved View binds only to the workspace that holds it, SPEC section 10, so the
//! reader fills it in), without `locations` (derived; the server fills them when it returns the
//! View) and without `materialized` (produced when the View is resolved). The three are refused
//! by name (ruling E10), so the stored bytes are always the client's own and a restore into
//! another workspace leaves nothing in the file to rewrite.
//!
//! Every reader of the file sees the same block: the line rule here, pulldown-cmark (Markdown
//! renderers and outlines) and okf-core (`markdown::code_free_lines`, the fence rule its link
//! rewriting on a move uses). A body on which they would disagree is refused, so a move never
//! rewrites text inside the block and a Markdown viewer never shows the block as prose. A JSON
//! text never has a line that begins with a backtick or a tilde, so to okf-core every line of a
//! well-formed block lies inside one fence. Storage never parses the block; core is its only
//! reader.
//!
//! The block's JSON is read with a check that no object repeats a key: `serde_json`'s `Value`
//! keeps the last of two equal keys, so the file would show one value while the View used the
//! other. Its format version is read first, so a document of a later version is refused for its
//! version, not for a field this build does not know.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;

use okf_core::markdown::code_free_lines;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, ItemId, Revision, WorkspaceId, WorkspacePath};
use okf_jawn_contract::item::ItemDocument;
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::source::SourceReference;
use okf_jawn_contract::views::{RenderGrammar, ViewBinding, ViewDocument, ViewMode};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};
use serde::de::{
    DeserializeOwned, DeserializeSeed, Deserializer, Error as _, MapAccess, SeqAccess, Visitor,
};
use serde_json::{Map, Value};

use super::pointer_token;

/// A seed that reads one JSON value and refuses a key repeated in one object, noting where.
struct Unique<'a> {
    /// The JSON Pointer of the value read.
    at: String,
    /// Where a repeated key was met: the object's pointer and the key.
    repeated: &'a RefCell<Option<(String, String)>>,
}

/// The info string of a View's block.
pub const VIEW_BLOCK_INFO: &str = "json okf-jawn-view";
/// The format version of the View documents this build reads and writes.
pub const VIEW_SCHEMA_VERSION: u32 = 1;
/// The opening line of a View's block, exactly.
const OPEN: &str = "```json okf-jawn-view";
/// The closing line of a View's block, exactly.
const CLOSE: &str = "```";
/// The fields of the block, in no order.
const DOCUMENT_FIELDS: [&str; 6] = [
    "schema_version",
    "mode",
    "grammar",
    "bindings",
    "spec",
    "charts",
];
/// The fields of one binding in the block.
const BINDING_FIELDS: [&str; 4] = ["name", "source", "units", "transforms"];
/// The fields of one binding's source in the block.
const SOURCE_FIELDS: [&str; 5] = ["item_id", "path", "revision", "digest", "selection"];

impl<'de> DeserializeSeed<'de> for Unique<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Unique<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        loop {
            let at = format!("{}/{}", self.at, items.len());
            match sequence.next_element_seed(Unique {
                at,
                repeated: self.repeated,
            })? {
                Some(item) => items.push(item),
                None => return Ok(Value::Array(items)),
            }
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                *self.repeated.borrow_mut() = Some((self.at.clone(), key));
                return Err(A::Error::custom("a key is repeated in one object"));
            }
            let value = map.next_value_seed(Unique {
                at: format!("{}/{}", self.at, pointer_token(&key)),
                repeated: self.repeated,
            })?;
            object.insert(key, value);
        }
        Ok(Value::Object(object))
    }
}

/// The View a stored item holds, every binding in `workspace`.
///
/// # Errors
/// Returns `InvalidInput` on the document's pointer for a file that is not a View document of
/// this build: see `view_from`.
pub fn view_of(document: &ItemDocument, workspace: WorkspaceId) -> Result<ViewDocument, ApiError> {
    view_from(&document.properties, &document.body, workspace)
}

/// The View a file of these properties and this body holds, every binding in `workspace`: the
/// title from the `title` property, the description from the prose before the block, the rest
/// from the block; every binding's `locations` empty and `materialized` absent.
///
/// # Errors
/// Returns `InvalidInput` whose `field` is a JSON Pointer into the `ViewDocument` (`/title`,
/// `/schema_version`, `/bindings/0/source/workspace_id`), or none for a body that holds no
/// readable block: a title that is not a string; no block, several, one not closed, one that
/// does not end the body, or one the other readers would not see as the same block; JSON that
/// does not parse or repeats a key; a version other than `VIEW_SCHEMA_VERSION`; a missing,
/// mistyped or unknown field (`workspace_id`, `locations` and `materialized` named).
pub fn view_from(
    properties: &BTreeMap<String, Value>,
    body: &str,
    workspace: WorkspaceId,
) -> Result<ViewDocument, ApiError> {
    let Some(Value::String(title)) = properties.get("title") else {
        return Err(fault(
            "/title",
            "a View's title is its `title` property, a string",
        ));
    };
    let (description, block) = split(body)?;
    let object = parse_unique(&block)?;
    let schema_version: u32 = field(&object, "schema_version", "")?;
    if schema_version != VIEW_SCHEMA_VERSION {
        return Err(fault(
            "/schema_version",
            format!(
                "this build reads and writes View documents of version {VIEW_SCHEMA_VERSION}; \
                 this one is version {schema_version}"
            ),
        ));
    }
    refuse_unknown(&object, &DOCUMENT_FIELDS, "", "a View document")?;
    let mode: ViewMode = field(&object, "mode", "")?;
    let grammar: RenderGrammar = field(&object, "grammar", "")?;
    let spec: Value = field(&object, "spec", "")?;
    let charts: BTreeMap<String, Value> = if object.contains_key("charts") {
        field(&object, "charts", "")?
    } else {
        BTreeMap::new()
    };
    let Some(Value::Array(stored)) = object.get("bindings") else {
        return Err(fault("/bindings", "`bindings` is a list of bindings"));
    };
    let bindings = stored
        .iter()
        .enumerate()
        .map(|(index, binding)| self::binding(binding, &format!("/bindings/{index}"), workspace))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ViewDocument {
        schema_version,
        title: title.clone(),
        description,
        mode,
        grammar,
        bindings,
        spec,
        charts,
    })
}

/// The description and the block text of a body, after checking that every reader sees the
/// same block (the module header).
fn split(body: &str) -> Result<(String, String), ApiError> {
    let lines: Vec<&str> = body.lines().collect();
    let opens: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| **line == OPEN)
        .map(|(index, _)| index)
        .collect();
    let open = match opens.as_slice() {
        [one] => *one,
        [] => {
            return Err(fault(
                "",
                "a View's body ends with one ```json okf-jawn-view block, which holds its document",
            ));
        }
        several => {
            return Err(fault(
                "",
                format!(
                    "the body holds {} okf-jawn-view blocks; a View has one",
                    several.len()
                ),
            ));
        }
    };
    let first = open.saturating_add(1);
    let close = lines
        .iter()
        .enumerate()
        .skip(first)
        .find(|(_, line)| **line == CLOSE)
        .map(|(index, _)| index)
        .ok_or_else(|| fault("", "the okf-jawn-view block is not closed by a line ```"))?;
    if lines
        .iter()
        .skip(close.saturating_add(1))
        .any(|line| !line.trim().is_empty())
    {
        return Err(fault("", "the okf-jawn-view block must end the body"));
    }
    agree(body, open, close)?;
    let block = lines.get(first..close).unwrap_or_default().join("\n");
    let prose = lines.get(..open).unwrap_or_default();
    let shown = |line: &&str| !line.trim().is_empty();
    let description = match (prose.iter().position(shown), prose.iter().rposition(shown)) {
        (Some(start), Some(end)) => prose.get(start..=end).unwrap_or_default().join("\n"),
        _ => String::new(),
    };
    Ok((description, block))
}

/// Refuse a body on which pulldown-cmark or okf-core would not read lines `open..=close` as the
/// one View block.
fn agree(body: &str, open: usize, close: usize) -> Result<(), ApiError> {
    let offset: usize = body.split_inclusive('\n').take(open).map(str::len).sum();
    let starts: Vec<usize> = Parser::new(body)
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))
                if info.as_ref() == VIEW_BLOCK_INFO =>
            {
                Some(range.start)
            }
            _ => None,
        })
        .collect();
    if starts.as_slice() != [offset] {
        return Err(fault(
            "",
            "the description leaves a block open (a code fence, an HTML block) that holds the \
             View block, so a Markdown reader does not see it; close it before the block",
        ));
    }
    // `code_free_lines` numbers lines from one.
    let block = open.saturating_add(1)..=close.saturating_add(1);
    if code_free_lines(body)
        .iter()
        .any(|(number, _)| block.contains(number))
    {
        return Err(fault(
            "",
            "okf-core reads the View block as prose, so a move could rewrite text inside it: the \
             description opens a fence okf-core does not see closed (a line that begins with ``` \
             or ~~~, as in an indented code block); close it before the block",
        ));
    }
    Ok(())
}

/// The block's JSON object, read with the repeated-key check.
fn parse_unique(text: &str) -> Result<Map<String, Value>, ApiError> {
    let repeated = RefCell::new(None);
    let mut reader = serde_json::Deserializer::from_str(text);
    let parsed = Unique {
        at: String::new(),
        repeated: &repeated,
    }
    .deserialize(&mut reader)
    .and_then(|value| reader.end().map(|()| value));
    let value = parsed.map_err(|error| match repeated.take() {
        Some((at, key)) => fault(&at, format!("the key `{key}` is repeated in one object")),
        None => fault("", format!("the View block is not JSON: {error}")),
    })?;
    match value {
        Value::Object(object) => Ok(object),
        _ => Err(fault("", "the View block holds one JSON object")),
    }
}

/// One binding of the block, read into the wire's `ViewBinding` in `workspace`.
fn binding(value: &Value, at: &str, workspace: WorkspaceId) -> Result<ViewBinding, ApiError> {
    let Value::Object(object) = value else {
        return Err(fault(at, "a binding is an object"));
    };
    if object.contains_key("materialized") {
        return Err(fault(
            &format!("{at}/materialized"),
            "a binding's dataset is produced when the View is resolved; a saved View keeps none",
        ));
    }
    refuse_unknown(object, &BINDING_FIELDS, at, "a binding")?;
    let source_at = format!("{at}/source");
    let Some(Value::Object(source)) = object.get("source") else {
        return Err(fault(
            &source_at,
            "a binding's `source` is its citation, an object",
        ));
    };
    if source.contains_key("workspace_id") {
        return Err(fault(
            &format!("{source_at}/workspace_id"),
            "a saved View binds only to its own workspace and does not name it",
        ));
    }
    if source.contains_key("locations") {
        return Err(fault(
            &format!("{source_at}/locations"),
            "locations are derived from the citation; a saved View keeps none",
        ));
    }
    refuse_unknown(source, &SOURCE_FIELDS, &source_at, "a binding's source")?;
    let item_id: ItemId = field(source, "item_id", &source_at)?;
    let path: WorkspacePath = field(source, "path", &source_at)?;
    let revision: Revision = field(source, "revision", &source_at)?;
    let digest: Option<Digest> = if source.contains_key("digest") {
        field(source, "digest", &source_at)?
    } else {
        None
    };
    let selection: Selection = field(source, "selection", &source_at)?;
    Ok(ViewBinding {
        name: field(object, "name", at)?,
        source: SourceReference {
            workspace_id: workspace,
            item_id,
            path,
            revision,
            digest,
            selection,
            locations: Vec::new(),
        },
        units: field(object, "units", at)?,
        transforms: field(object, "transforms", at)?,
        materialized: None,
    })
}

/// Refuse the first key of `object` (in key order) that `known` does not hold.
fn refuse_unknown(
    object: &Map<String, Value>,
    known: &[&str],
    at: &str,
    what: &str,
) -> Result<(), ApiError> {
    let mut keys: Vec<&String> = object.keys().collect();
    keys.sort();
    match keys.into_iter().find(|key| !known.contains(&key.as_str())) {
        Some(key) => Err(fault(
            &format!("{at}/{}", pointer_token(key)),
            format!("`{key}` is not a field of {what}"),
        )),
        None => Ok(()),
    }
}

/// One field of an object, decoded; refused at its pointer when missing or mistyped.
fn field<T: DeserializeOwned>(
    object: &Map<String, Value>,
    key: &str,
    at: &str,
) -> Result<T, ApiError> {
    let pointer = format!("{at}/{}", pointer_token(key));
    let value = object
        .get(key)
        .ok_or_else(|| fault(&pointer, format!("`{key}` is missing")))?;
    T::deserialize(value).map_err(|error| fault(&pointer, error.to_string()))
}

/// The refusal of a file that is not a View document, at `pointer` into the document.
fn fault(pointer: &str, message: impl Into<String>) -> ApiError {
    let refused = ApiError::new(ErrorCode::InvalidInput, message);
    if pointer.is_empty() {
        refused
    } else {
        refused.with_field(pointer)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use okf_jawn_contract::error::{ApiError, ErrorCode};
    use okf_jawn_contract::identity::WorkspaceId;
    use okf_jawn_contract::views::ViewDocument;
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::view_from;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn err_of<T>(result: Result<T, ApiError>) -> Result<ApiError, Box<dyn std::error::Error>> {
        match result {
            Ok(_) => Err("expected an error".into()),
            Err(error) => Ok(error),
        }
    }

    fn workspace(id: u128) -> WorkspaceId {
        WorkspaceId(Uuid::from_u128(id))
    }

    fn titled() -> BTreeMap<String, Value> {
        BTreeMap::from([
            ("title".to_owned(), json!("Pipeline throughput")),
            ("type".to_owned(), json!("Dashboard")),
        ])
    }

    /// The block of a valid two-binding View, as JSON text.
    fn block() -> Result<String, Box<dyn std::error::Error>> {
        Ok(serde_json::to_string_pretty(&json!({
            "schema_version": 1,
            "mode": "pinned",
            "grammar": "json_render",
            "bindings": [
                {
                    "name": "report",
                    "source": {
                        "item_id": "22222222-3333-4444-8555-666666666666",
                        "path": "reports/pipeline-pdf.md",
                        "revision": "0123456789abcdef0123456789abcdef01234567",
                        "digest": "b".repeat(64),
                        "selection": { "kind": "pages", "range": { "start": 3, "end": 3 } }
                    },
                    "units": { "Documents": "count" },
                    "transforms": []
                },
                {
                    "name": "venue",
                    "source": {
                        "item_id": "11111111-2222-4333-8444-555555555555",
                        "path": "notes/venue.md",
                        "revision": "0123456789abcdef0123456789abcdef01234567",
                        "selection": { "kind": "all" }
                    },
                    "units": {},
                    "transforms": []
                }
            ],
            "spec": { "root": "root", "elements": {} },
            "charts": { "stages": { "data": { "name": "report" }, "mark": "bar" } }
        }))?)
    }

    fn body(prose: &str, block: &str) -> String {
        format!("{prose}```json okf-jawn-view\n{block}\n```\n")
    }

    fn read(body: &str) -> Result<ViewDocument, ApiError> {
        view_from(&titled(), body, workspace(1))
    }

    /// A block with one field of the first binding (or its source) replaced or removed.
    fn edited(
        edit: impl FnOnce(&mut serde_json::Map<String, Value>),
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut value: Value = serde_json::from_str(&block()?)?;
        let object = value.as_object_mut().ok_or("the block is an object")?;
        edit(object);
        Ok(serde_json::to_string_pretty(&value)?)
    }

    fn first_binding(
        object: &mut serde_json::Map<String, Value>,
    ) -> Option<&mut serde_json::Map<String, Value>> {
        object
            .get_mut("bindings")
            .and_then(Value::as_array_mut)
            .and_then(|bindings| bindings.get_mut(0))
            .and_then(Value::as_object_mut)
    }

    #[test]
    fn a_view_file_reads_as_its_title_description_and_block() -> TestResult {
        let view = read(&body("Two lines\nof prose\n\n\n", &block()?))?;
        assert_eq!(view.title, "Pipeline throughput");
        assert_eq!(view.description, "Two lines\nof prose");
        assert_eq!(view.bindings.len(), 2);
        assert!(view.bindings.iter().all(|binding| {
            binding.source.workspace_id == workspace(1)
                && binding.source.locations.is_empty()
                && binding.materialized.is_none()
        }));
        assert_eq!(view.charts.len(), 1);
        let bare = read(&body("", &block()?))?;
        assert_eq!(bare.description, "");
        // The same file in another workspace binds there and differs in nothing else.
        let elsewhere = view_from(
            &titled(),
            &body("Two lines\nof prose\n\n\n", &block()?),
            workspace(2),
        )?;
        assert!(
            elsewhere
                .bindings
                .iter()
                .all(|binding| binding.source.workspace_id == workspace(2))
        );
        assert_eq!(
            serde_json::to_value(&elsewhere.spec)?,
            serde_json::to_value(&view.spec)?
        );
        Ok(())
    }

    #[test]
    fn a_crlf_view_file_reads_as_its_lf_spelling() -> TestResult {
        let lf = body("Prose\n\n", &block()?);
        let crlf = lf.replace('\n', "\r\n");
        assert_eq!(
            serde_json::to_value(read(&crlf)?)?,
            serde_json::to_value(read(&lf)?)?
        );
        Ok(())
    }

    #[test]
    fn a_body_without_exactly_one_block_that_ends_it_is_refused() -> TestResult {
        let one = body("", &block()?);
        let none = err_of(read("Only prose.\n"))?;
        assert_eq!(none.code, ErrorCode::InvalidInput);
        assert!(none.message.contains("ends with one"), "{}", none.message);
        let twice = err_of(read(&format!("{one}\n{one}")))?;
        assert!(
            twice.message.contains("2 okf-jawn-view blocks"),
            "{}",
            twice.message
        );
        let trailing = err_of(read(&format!("{one}More prose.\n")))?;
        assert!(
            trailing.message.contains("must end the body"),
            "{}",
            trailing.message
        );
        let open = err_of(read(&format!("```json okf-jawn-view\n{}\n", block()?)))?;
        assert!(open.message.contains("not closed"), "{}", open.message);
        for marker in [
            "```JSON okf-jawn-view",
            " ```json okf-jawn-view",
            "````json okf-jawn-view",
            "~~~json okf-jawn-view",
        ] {
            let spelled = err_of(read(&format!("{marker}\n{}\n```\n", block()?)))?;
            assert!(
                spelled.message.contains("ends with one"),
                "{marker}: {}",
                spelled.message
            );
        }
        // The marker quoted in the description's own example is a second marker line.
        let quoted = err_of(read(&body(
            "~~~\n```json okf-jawn-view\n~~~\n\n",
            &block()?,
        )))?;
        assert!(
            quoted.message.contains("2 okf-jawn-view blocks"),
            "{}",
            quoted.message
        );
        Ok(())
    }

    #[test]
    fn a_description_that_hides_the_block_from_a_reader_is_refused() -> TestResult {
        // pulldown-cmark reads the marker as code of an unclosed fence.
        let unclosed = err_of(read(&body("```python\nprint(1)\n\n", &block()?)))?;
        assert!(
            unclosed.message.contains("leaves a block open"),
            "{}",
            unclosed.message
        );
        // An indented code block holding a fence line: okf-core opens a fence there.
        let indented = err_of(read(&body("Example:\n\n    ```\n    code\n\n", &block()?)))?;
        assert!(
            indented.message.contains("okf-core"),
            "{}",
            indented.message
        );
        Ok(())
    }

    #[test]
    fn a_block_that_repeats_a_key_is_refused_where_it_repeats() -> TestResult {
        let text = block()?.replacen(
            "\"mode\": \"pinned\",",
            "\"mode\": \"pinned\",\n  \"mode\": \"live\",",
            1,
        );
        let repeated = err_of(read(&body("", &text)))?;
        assert_eq!(repeated.field, None);
        assert!(
            repeated.message.contains("`mode` is repeated"),
            "{}",
            repeated.message
        );
        let inner = block()?.replacen(
            "\"units\": {}",
            "\"units\": { \"a\": \"x\", \"a\": \"y\" }",
            1,
        );
        let nested = err_of(read(&body("", &inner)))?;
        assert_eq!(nested.field.as_deref(), Some("/bindings/1/units"));
        Ok(())
    }

    #[test]
    fn a_block_naming_a_derived_field_is_refused_by_name() -> TestResult {
        let named = [
            (
                "workspace_id",
                true,
                "/bindings/0/source/workspace_id",
                "own workspace",
            ),
            ("locations", true, "/bindings/0/source/locations", "derived"),
            (
                "materialized",
                false,
                "/bindings/0/materialized",
                "resolved",
            ),
        ];
        for (key, in_source, pointer, gist) in named {
            let text = edited(|object| {
                if let Some(binding) = first_binding(object) {
                    let target = if in_source {
                        binding.get_mut("source").and_then(Value::as_object_mut)
                    } else {
                        Some(binding)
                    };
                    if let Some(target) = target {
                        target.insert(key.to_owned(), json!([]));
                    }
                }
            })?;
            let refused = err_of(read(&body("", &text)))?;
            assert_eq!(refused.code, ErrorCode::InvalidInput, "{key}");
            assert_eq!(refused.field.as_deref(), Some(pointer), "{key}");
            assert!(refused.message.contains(gist), "{key}: {}", refused.message);
        }
        let unknown = edited(|object| {
            object.insert("theme".to_owned(), json!("dark"));
        })?;
        let refused = err_of(read(&body("", &unknown)))?;
        assert_eq!(refused.field.as_deref(), Some("/theme"));
        Ok(())
    }

    #[test]
    fn a_title_that_is_absent_or_not_a_string_is_refused() -> TestResult {
        let text = body("", &block()?);
        let absent = err_of(view_from(&BTreeMap::new(), &text, workspace(1)))?;
        assert_eq!(absent.field.as_deref(), Some("/title"));
        let numbered = BTreeMap::from([("title".to_owned(), json!(2024))]);
        let number = err_of(view_from(&numbered, &text, workspace(1)))?;
        assert_eq!(number.field.as_deref(), Some("/title"));
        Ok(())
    }

    #[test]
    fn a_later_version_is_refused_for_its_version_before_its_fields() -> TestResult {
        let text = edited(|object| {
            object.insert("schema_version".to_owned(), json!(2));
            object.insert("theme".to_owned(), json!("dark"));
        })?;
        let refused = err_of(read(&body("", &text)))?;
        assert_eq!(refused.field.as_deref(), Some("/schema_version"));
        assert!(refused.message.contains("version 2"), "{}", refused.message);
        Ok(())
    }

    #[test]
    fn a_mistyped_or_missing_field_is_refused_at_its_pointer() -> TestResult {
        let missing = edited(|object| {
            object.remove("mode");
        })?;
        assert_eq!(
            err_of(read(&body("", &missing)))?.field.as_deref(),
            Some("/mode")
        );
        let path = edited(|object| {
            if let Some(source) = first_binding(object)
                .and_then(|binding| binding.get_mut("source"))
                .and_then(Value::as_object_mut)
            {
                source.insert("path".to_owned(), json!("../escape.md"));
            }
        })?;
        assert_eq!(
            err_of(read(&body("", &path)))?.field.as_deref(),
            Some("/bindings/0/source/path")
        );
        let not_json = err_of(read(&body("", "{ \"schema_version\": 1,")))?;
        assert!(
            not_json.message.contains("not JSON"),
            "{}",
            not_json.message
        );
        Ok(())
    }
}
