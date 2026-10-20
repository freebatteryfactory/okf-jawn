//! `views::validate`: a View document against the approved component catalog and Vega-Lite's
//! schema, plus the referential rules (core M1.6 design section 3, as ruled). Pure: no port is
//! read; a citation is checked by its caller (`reading::check_citation`).
//!
//! A fault of the document, its layout or its bindings is the View's: `validate` answers
//! `InvalidInput` whose `field` is a JSON Pointer into the `ViewDocument`, and every caller
//! refuses. A fault inside one chart's own specification (Vega-Lite's schema, a denied load, a
//! dataset name no binding declares) is that chart's (`ChartCheck::Refused`): `present_view` and
//! `resolve_view` report it as the chart's status, and every saved-View write refuses it
//! (`Checked::require_drawable`; ruling E7).
//!
//! The View-level rules, in the order they are checked: the format version; at most
//! `MAX_VIEW_BINDINGS` bindings; binding names of 1 to 120 characters, each once; every binding
//! in the View's own workspace (`ViewDocument::require_own_workspace`); no `materialized` (the
//! server produces it); each binding transform a Vega-Lite `Transform` that loads nothing and
//! reads only bindings (ruling E3), with no cycle among binding transforms; a `vega_lite`
//! View has no named charts. For `json_render`:
//! the catalog schema, each component's props against its own schema first; a static
//! composition, so no `state`, `repeat` or `visible` (ruling E2); a root that names an element;
//! children and slots that name elements, a container's slots exactly `{"default": children}`,
//! and a leaf with no children; no cycle and no element the root does not reach; props that name
//! declared bindings and charts. Then every chart is checked, and then: every named chart is drawn
//! by a `Chart` component, each `Chart` component shows a binding its chart reads, and every
//! binding is shown by something (ruling E4: a review covers exactly what is shown).
//!
//! A chart is refused (`InvalidSpec`) when it fails Vega-Lite's schema, when it loads or runs
//! anything (a `data` that is not exactly `{"name": ...}`, a `url`, an `href`, inline
//! `datasets`, a custom `formatType`, `customFormatTypes`, a `bind` to an outside `element`),
//! or when it names no dataset; it is `UnknownBinding` when it reads a name no binding declares.
//! What a chart reads includes what the transforms of the bindings it reads look up, closed
//! transitively. Vega expressions are allowed: SPEC section 10 selects Vega's expression
//! interpreter. Objects are walked in key order, so the first fault reported does not depend on
//! the map order `serde_json` was built with.

use std::collections::{BTreeMap, BTreeSet};

use jsonschema::ValidationError;
use jsonschema::error::ValidationErrorKind;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::WorkspaceId;
use okf_jawn_contract::views::{ChartFailure, MAX_VIEW_BINDINGS, RenderGrammar, ViewDocument};
use serde_json::{Map, Value};

use super::layout::VIEW_SCHEMA_VERSION;
use super::{pointer_token, schemas};
use crate::echo::bounded;

/// What a valid View's charts and tables read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// One per `ViewDocument::chart_refs()`, in that order.
    pub charts: Vec<ChartCheck>,
    /// Binding names a `DataTable` component shows (`json_render`).
    pub tables: BTreeSet<String>,
}

/// One chart's static verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChartCheck {
    /// It can be drawn once its datasets are produced.
    Drawable {
        /// The binding names it reads, sorted, each once.
        bindings: Vec<String>,
    },
    /// Its own specification is at fault.
    Refused {
        /// `InvalidSpec` or `UnknownBinding`.
        reason: ChartFailure,
        /// The unknown dataset name, for `UnknownBinding`.
        binding: Option<String>,
        /// Where in the `ViewDocument`.
        pointer: String,
        /// Safe explanation for the chart's own alert.
        message: String,
    },
}

/// The longest binding name a component prop can name (the catalog's `maxLength`).
const MAX_BINDING_NAME_CHARS: usize = 120;
/// The `formatType` values Vega-Lite formats with itself.
const FORMAT_TYPES: [&str; 3] = ["number", "time", "utc"];
/// The components that hold children.
const CONTAINERS: [&str; 2] = ["Stack", "Columns"];
/// The components whose `binding` prop names one binding.
const BOUND: [&str; 3] = ["SourceExcerpt", "DataTable", "Chart"];
/// Why a layout with state, repetition or conditions is refused (ruling E2).
const STATIC: &str = "a View is a static composition: no state, repetition or conditions";

/// Why a custom format type is refused.
const EXECUTABLE: &str =
    "a custom format type calls a registered function, an executable extension";

impl Checked {
    /// Refuse the first chart that cannot be drawn: a saved View has none (ruling E7).
    ///
    /// # Errors
    /// Returns `InvalidInput` at the chart's pointer, with its message.
    pub fn require_drawable(&self) -> Result<(), ApiError> {
        match self.charts.iter().find_map(|chart| match chart {
            ChartCheck::Refused {
                pointer, message, ..
            } => Some((pointer, message)),
            ChartCheck::Drawable { .. } => None,
        }) {
            Some((pointer, message)) => Err(fault(pointer, message.clone())),
            None => Ok(()),
        }
    }

    /// Every binding name a drawable chart or a table reads, sorted: the bindings whose
    /// datasets a present or a resolve produces.
    #[must_use]
    pub fn needed(&self) -> BTreeSet<String> {
        let mut needed = self.tables.clone();
        for chart in &self.charts {
            if let ChartCheck::Drawable { bindings } = chart {
                needed.extend(bindings.iter().cloned());
            }
        }
        needed
    }
}

/// Check a View document saved or presented in `workspace` (the module header).
///
/// # Errors
/// Returns `InvalidInput` on a pointer into the `ViewDocument` for a View-level fault, and
/// `Internal` when an embedded schema does not compile.
pub fn validate(view: &ViewDocument, workspace: WorkspaceId) -> Result<Checked, ApiError> {
    if view.schema_version != VIEW_SCHEMA_VERSION {
        return Err(fault(
            "/schema_version",
            format!("this build reads and writes View documents of version {VIEW_SCHEMA_VERSION}"),
        ));
    }
    if view.bindings.len() > MAX_VIEW_BINDINGS {
        return Err(fault(
            "/bindings",
            format!(
                "a View holds at most {MAX_VIEW_BINDINGS} bindings; this one holds {}",
                view.bindings.len()
            ),
        ));
    }
    let names = binding_names(view)?;
    view.require_own_workspace(workspace, "")?;
    if let Some(index) = view
        .bindings
        .iter()
        .position(|binding| binding.materialized.is_some())
    {
        return Err(fault(
            &format!("/bindings/{index}/materialized"),
            "a binding's dataset is produced by the server when the View is resolved; send the \
             binding without it",
        ));
    }
    let looked_up = transforms(view, &names)?;
    match view.grammar {
        RenderGrammar::VegaLite => {
            if !view.charts.is_empty() {
                return Err(fault(
                    "/charts",
                    "a vega_lite View draws its own spec; named charts belong to json_render",
                ));
            }
            let chart = check_chart(&view.spec, "/spec", &names, &looked_up)?;
            if let ChartCheck::Drawable { bindings } = &chart {
                unshown(view, &BTreeSet::new(), bindings)?;
            }
            Ok(Checked {
                charts: vec![chart],
                tables: BTreeSet::new(),
            })
        }
        RenderGrammar::JsonRender => layout(view, &names, &looked_up),
    }
}

/// The binding names, each 1 to 120 characters and given once.
fn binding_names(view: &ViewDocument) -> Result<BTreeSet<&str>, ApiError> {
    let mut names = BTreeSet::new();
    for (index, binding) in view.bindings.iter().enumerate() {
        let length = binding.name.chars().count();
        if length == 0 || length > MAX_BINDING_NAME_CHARS {
            return Err(fault(
                &format!("/bindings/{index}/name"),
                format!("a binding's name has 1 to {MAX_BINDING_NAME_CHARS} characters"),
            ));
        }
        if !names.insert(binding.name.as_str()) {
            return Err(fault(
                &format!("/bindings/{index}/name"),
                format!("another binding is already named `{}`", binding.name),
            ));
        }
    }
    Ok(names)
}

/// Check every binding's transforms (ruling E3): each a Vega-Lite `Transform` that loads nothing
/// and looks up only bindings. Validate all edges before refusing a dependency cycle, retaining
/// the existing schema/load/unknown-name precedence. Answers the names each binding looks up.
fn transforms(
    view: &ViewDocument,
    names: &BTreeSet<&str>,
) -> Result<BTreeMap<String, BTreeSet<String>>, ApiError> {
    let validator = schemas::vega_lite_transform()?;
    let mut looked_up = BTreeMap::new();
    let mut dependencies = BTreeMap::new();
    for (index, binding) in view.bindings.iter().enumerate() {
        let mut reads = BTreeSet::new();
        let mut edges = Vec::new();
        for (position, transform) in binding.transforms.iter().enumerate() {
            let at = format!("/bindings/{index}/transforms/{position}");
            if let Err(error) = validator.validate(transform) {
                let (path, message) = diagnose(&error);
                return Err(fault(
                    &format!("{at}{path}"),
                    format!("the transform is not a Vega-Lite transform: {message}"),
                ));
            }
            let mut found = BTreeMap::new();
            walk(transform, &at, &mut found)
                .map_err(|(pointer, message)| fault(&pointer, message))?;
            for (name, pointer) in found {
                if !names.contains(name.as_str()) {
                    return Err(fault(
                        &pointer,
                        format!("the transform looks up `{name}`, which no binding declares"),
                    ));
                }
                edges.push((name.clone(), pointer));
                reads.insert(name);
            }
        }
        dependencies.insert(binding.name.clone(), edges);
        looked_up.insert(binding.name.clone(), reads);
    }
    acyclic_transforms(view, &dependencies)?;
    Ok(looked_up)
}

/// Visit input bindings and transform edges in their validated order. Only an edge into the
/// active path closes a cycle: a completed dependency may be shared by any number of bindings.
/// The caller's existing `MAX_VIEW_BINDINGS` check bounds the active path to 50 entries.
fn acyclic_transforms(
    view: &ViewDocument,
    dependencies: &BTreeMap<String, Vec<(String, String)>>,
) -> Result<(), ApiError> {
    let mut completed = BTreeSet::new();
    for binding in &view.bindings {
        let root = binding.name.as_str();
        if completed.contains(root) {
            continue;
        }
        let mut active = BTreeSet::from([root]);
        let mut path = vec![(root, 0_usize)];
        while let Some((name, next)) = path.last_mut() {
            let parent = *name;
            let edge = dependencies.get(parent).and_then(|edges| edges.get(*next));
            *next = next.saturating_add(1);
            match edge {
                None => {
                    active.remove(parent);
                    completed.insert(parent);
                    path.pop();
                }
                Some((target, pointer)) => {
                    if active.contains(target.as_str()) {
                        return Err(fault(
                            pointer,
                            format!(
                                "makes a cycle in binding transforms: `{parent}` looks up `{target}`"
                            ),
                        ));
                    }
                    if !completed.contains(target.as_str()) {
                        active.insert(target.as_str());
                        path.push((target.as_str(), 0));
                    }
                }
            }
        }
    }
    Ok(())
}

/// The static verdict of one Vega-Lite chart at `at`.
fn check_chart(
    chart: &Value,
    at: &str,
    names: &BTreeSet<&str>,
    looked_up: &BTreeMap<String, BTreeSet<String>>,
) -> Result<ChartCheck, ApiError> {
    let refused = |reason, binding, pointer: String, message: String| ChartCheck::Refused {
        reason,
        binding,
        pointer,
        message,
    };
    if let Err(error) = schemas::vega_lite_spec()?.validate(chart) {
        let (path, message) = diagnose(&error);
        return Ok(refused(
            ChartFailure::InvalidSpec,
            None,
            format!("{at}{path}"),
            format!("the chart fails Vega-Lite's schema at {at}{path}: {message}"),
        ));
    }
    let mut found = BTreeMap::new();
    if let Err((pointer, message)) = walk(chart, at, &mut found) {
        return Ok(refused(ChartFailure::InvalidSpec, None, pointer, message));
    }
    if found.is_empty() {
        return Ok(refused(
            ChartFailure::InvalidSpec,
            None,
            at.to_owned(),
            "a chart reads its data by `data.name`; this one names no dataset".to_owned(),
        ));
    }
    if let Some((name, pointer)) = found
        .iter()
        .find(|(name, _)| !names.contains(name.as_str()))
    {
        return Ok(refused(
            ChartFailure::UnknownBinding,
            Some(name.clone()),
            pointer.clone(),
            format!("the chart reads `{name}`, which no binding of this View declares"),
        ));
    }
    // What the transforms of the bindings it reads look up, closed transitively.
    let mut reads: BTreeSet<String> = found.into_keys().collect();
    let mut pending: Vec<String> = reads.iter().cloned().collect();
    while let Some(name) = pending.pop() {
        for more in looked_up.get(&name).into_iter().flatten() {
            if reads.insert(more.clone()) {
                pending.push(more.clone());
            }
        }
    }
    Ok(ChartCheck::Drawable {
        bindings: reads.into_iter().collect(),
    })
}

/// Walk a chart or a transform (keys in order) for the dataset names it reads, refusing the
/// first thing it may not do (the module header).
fn walk(
    value: &Value,
    at: &str,
    found: &mut BTreeMap<String, String>,
) -> Result<(), (String, String)> {
    match value {
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                walk(item, &format!("{at}/{index}"), found)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            for (key, child) in sorted(map) {
                let here = format!("{at}/{}", pointer_token(key));
                let refusal = match key.as_str() {
                    "url" => Some(
                        "loads an external resource; a chart reads only datasets the server resolves",
                    ),
                    "href" => Some("a chart does not navigate"),
                    "datasets" => Some(
                        "holds inline datasets; a chart reads only datasets the server resolves",
                    ),
                    "customFormatTypes" => Some(EXECUTABLE),
                    "formatType"
                        if child
                            .as_str()
                            .is_none_or(|kind| !FORMAT_TYPES.contains(&kind)) =>
                    {
                        Some(EXECUTABLE)
                    }
                    _ => None,
                };
                if let Some(message) = refusal {
                    return Err((here, message.to_owned()));
                }
                if key == "bind"
                    && child
                        .as_object()
                        .is_some_and(|bind| bind.contains_key("element"))
                {
                    return Err((
                        format!("{here}/element"),
                        "a chart binds no element outside itself".to_owned(),
                    ));
                }
                if key == "data" && child.is_object() {
                    named_data(child, &here, found)?;
                    continue;
                }
                walk(child, &here, found)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// A `data` object: exactly `{"name": <string>}`.
fn named_data(
    value: &Value,
    at: &str,
    found: &mut BTreeMap<String, String>,
) -> Result<(), (String, String)> {
    let Some(map) = value.as_object() else {
        return Ok(());
    };
    for (key, _) in sorted(map) {
        let message = match key.as_str() {
            "name" => continue,
            "url" => "loads data from a URL; a chart reads only datasets the server resolves",
            "values" => "holds inline values; a chart reads only datasets the server resolves",
            "sequence" | "sphere" | "graticule" => {
                "generates data; a chart reads only datasets the server resolves"
            }
            "format" => "a named dataset takes no format",
            _ => "is not part of a named dataset",
        };
        return Err((format!("{at}/{}", pointer_token(key)), message.to_owned()));
    }
    match map.get("name") {
        Some(Value::String(name)) => {
            found
                .entry(name.clone())
                .or_insert_with(|| format!("{at}/name"));
            Ok(())
        }
        _ => Err((
            at.to_owned(),
            "a chart reads a dataset by its `name`".to_owned(),
        )),
    }
}

/// Where a validation fault lies, and what it is: through each `anyOf` or `oneOf`, the branch
/// that came closest (the fewest errors, then the deepest; the first of equals) and its deepest
/// error. Answers its instance path and its text, bounded.
fn diagnose(error: &ValidationError<'_>) -> (String, String) {
    fn depth(error: &ValidationError<'_>) -> usize {
        error.instance_path().as_str().matches('/').count()
    }
    fn deepest<'a>(branch: &'a [ValidationError<'static>]) -> Option<&'a ValidationError<'static>> {
        branch.iter().fold(None, |best, candidate| match best {
            Some(best) if depth(best) >= depth(candidate) => Some(best),
            _ => Some(candidate),
        })
    }
    let mut current = error;
    loop {
        let context = match current.kind() {
            ValidationErrorKind::AnyOf { context }
            | ValidationErrorKind::OneOfNotValid { context } => Some(context),
            _ => None,
        };
        let closest = context
            .and_then(|context| {
                context
                    .iter()
                    .filter(|branch| !branch.is_empty())
                    .min_by(|left, right| {
                        left.len().cmp(&right.len()).then_with(|| {
                            let depth_of = |branch: &Vec<ValidationError<'static>>| {
                                deepest(branch).map_or(0, depth)
                            };
                            depth_of(right).cmp(&depth_of(left))
                        })
                    })
            })
            .and_then(|branch| deepest(branch.as_slice()));
        match closest {
            Some(next) => current = next,
            None => {
                return (
                    current.instance_path().as_str().to_owned(),
                    bounded(current.to_string()),
                );
            }
        }
    }
}

/// The checks of a `json_render` View (V8 to V16 of the design).
fn layout(
    view: &ViewDocument,
    names: &BTreeSet<&str>,
    looked_up: &BTreeMap<String, BTreeSet<String>>,
) -> Result<Checked, ApiError> {
    let spec = &view.spec;
    if let Some(elements) = spec.get("elements").and_then(Value::as_object) {
        for (key, element) in sorted(elements) {
            let (Some(kind), Some(props)) = (
                element.get("type").and_then(Value::as_str),
                element.get("props"),
            ) else {
                continue;
            };
            if let Some(validator) = schemas::component_props(kind)?
                && let Err(error) = validator.validate(props)
            {
                let (path, message) = diagnose(&error);
                return Err(fault(
                    &format!("/spec/elements/{}/props{path}", pointer_token(key)),
                    format!("the props of a {kind} do not meet its schema: {message}"),
                ));
            }
        }
    }
    if let Err(error) = schemas::catalog_schema()?.validate(spec) {
        let (path, message) = diagnose(&error);
        return Err(fault(
            &format!("/spec{path}"),
            format!("the layout does not meet the approved catalog: {message}"),
        ));
    }
    let elements = spec
        .get("elements")
        .and_then(Value::as_object)
        .ok_or_else(|| fault("/spec/elements", "the layout's elements are an object"))?;
    if spec.get("state").is_some() {
        return Err(fault("/spec/state", STATIC));
    }
    for (key, element) in sorted(elements) {
        for dynamic in ["repeat", "visible"] {
            if element.get(dynamic).is_some() {
                return Err(fault(
                    &format!("/spec/elements/{}/{dynamic}", pointer_token(key)),
                    STATIC,
                ));
            }
        }
    }
    let root = spec.get("root").and_then(Value::as_str).unwrap_or_default();
    if !elements.contains_key(root) {
        return Err(fault(
            "/spec/root",
            "the root names no element of the layout",
        ));
    }
    structure(elements)?;
    reach(elements, root)?;
    let shown = references(view, elements, names)?;
    let mut charts = Vec::with_capacity(view.charts.len());
    for (name, chart) in &view.charts {
        charts.push(check_chart(
            chart,
            &format!("/charts/{}", pointer_token(name)),
            names,
            looked_up,
        )?);
    }
    drawn(view, elements, &charts)?;
    let mut read = BTreeSet::new();
    for chart in &charts {
        if let ChartCheck::Drawable { bindings } = chart {
            read.extend(bindings.iter().cloned());
        }
    }
    let read: Vec<String> = read.into_iter().collect();
    unshown(view, &shown, &read)?;
    let tables = sorted(elements)
        .into_iter()
        .filter(|(_, element)| element.get("type").and_then(Value::as_str) == Some("DataTable"))
        .filter_map(|(_, element)| prop_str(element, "binding").map(str::to_owned))
        .collect();
    Ok(Checked { charts, tables })
}

/// Children and slots name elements; a container's slots are exactly `{"default": children}`;
/// a leaf holds no children and no slots.
fn structure(elements: &Map<String, Value>) -> Result<(), ApiError> {
    for (key, element) in sorted(elements) {
        let at = format!("/spec/elements/{}", pointer_token(key));
        let children: Vec<&str> = children(element).collect();
        for (index, child) in children.iter().enumerate() {
            if !elements.contains_key(*child) {
                return Err(fault(
                    &format!("{at}/children/{index}"),
                    format!("names `{child}`, which is no element of the layout"),
                ));
            }
        }
        let kind = element
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let slots = element.get("slots");
        if CONTAINERS.contains(&kind) {
            if let Some(slots) = slots {
                let mut expected = Map::new();
                expected.insert(
                    "default".to_owned(),
                    Value::Array(
                        children
                            .iter()
                            .map(|child| Value::String((*child).to_owned()))
                            .collect(),
                    ),
                );
                let expected = Value::Object(expected);
                if *slots != expected {
                    return Err(fault(
                        &format!("{at}/slots"),
                        "a container's slots are exactly `{\"default\": <its children>}`",
                    ));
                }
            }
        } else if !children.is_empty() || slots.is_some() {
            return Err(fault(
                &format!("{at}/children"),
                format!("a {kind} shows no children, so it holds none and no slots"),
            ));
        }
    }
    Ok(())
}

/// No cycle from the root, and every element reached: an element the root does not reach is
/// never shown.
fn reach(elements: &Map<String, Value>, root: &str) -> Result<(), ApiError> {
    let mut seen: BTreeSet<&str> = BTreeSet::from([root]);
    let mut on_path: BTreeSet<&str> = BTreeSet::from([root]);
    let mut path: Vec<(&str, usize)> = vec![(root, 0)];
    while let Some((key, next)) = path.last_mut() {
        let parent = *key;
        let child = elements
            .get(parent)
            .and_then(|element| element.get("children"))
            .and_then(Value::as_array)
            .and_then(|children| children.get(*next))
            .and_then(Value::as_str);
        let index = *next;
        *next = next.saturating_add(1);
        match child {
            None => {
                on_path.remove(parent);
                path.pop();
            }
            Some(child) => {
                if on_path.contains(child) {
                    return Err(fault(
                        &format!("/spec/elements/{}/children/{index}", pointer_token(parent)),
                        format!("makes a cycle: `{child}` holds `{parent}`"),
                    ));
                }
                if elements.contains_key(child) && seen.insert(child) {
                    on_path.insert(child);
                    path.push((child, 0));
                }
            }
        }
    }
    match sorted(elements)
        .into_iter()
        .find(|(key, _)| !seen.contains(key.as_str()))
    {
        Some((key, _)) => Err(fault(
            &format!("/spec/elements/{}", pointer_token(key)),
            "the root does not reach this element, so it is never shown",
        )),
        None => Ok(()),
    }
}

/// Props name declared bindings and charts; answers every binding name a component shows.
fn references<'a>(
    view: &ViewDocument,
    elements: &'a Map<String, Value>,
    names: &BTreeSet<&str>,
) -> Result<BTreeSet<&'a str>, ApiError> {
    let mut shown = BTreeSet::new();
    for (key, element) in sorted(elements) {
        let at = format!("/spec/elements/{}/props", pointer_token(key));
        let kind = element
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if BOUND.contains(&kind)
            && let Some(binding) = prop_str(element, "binding")
        {
            if !names.contains(binding) {
                return Err(fault(
                    &format!("{at}/binding"),
                    format!("names `{binding}`, which no binding of this View declares"),
                ));
            }
            shown.insert(binding);
        }
        if kind == "Chart"
            && let Some(chart) = prop_str(element, "chart")
            && !view.charts.contains_key(chart)
        {
            return Err(fault(
                &format!("{at}/chart"),
                format!("names the chart `{chart}`, which this View does not hold"),
            ));
        }
        if kind == "SourceList" {
            let listed = element
                .get("props")
                .and_then(|props| props.get("bindings"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str);
            for (index, binding) in listed.enumerate() {
                if !names.contains(binding) {
                    return Err(fault(
                        &format!("{at}/bindings/{index}"),
                        format!("names `{binding}`, which no binding of this View declares"),
                    ));
                }
                shown.insert(binding);
            }
        }
    }
    Ok(shown)
}

/// Every named chart is drawn by a `Chart` component, and each `Chart` component shows a binding
/// its chart reads (when the chart is drawable).
fn drawn(
    view: &ViewDocument,
    elements: &Map<String, Value>,
    charts: &[ChartCheck],
) -> Result<(), ApiError> {
    let checks: BTreeMap<&str, &ChartCheck> =
        view.charts.keys().map(String::as_str).zip(charts).collect();
    let mut drawn = BTreeSet::new();
    for (key, element) in sorted(elements) {
        if element.get("type").and_then(Value::as_str) != Some("Chart") {
            continue;
        }
        let (Some(chart), Some(binding)) =
            (prop_str(element, "chart"), prop_str(element, "binding"))
        else {
            continue;
        };
        drawn.insert(chart);
        if let Some(ChartCheck::Drawable { bindings }) = checks.get(chart).copied()
            && !bindings.iter().any(|read| read == binding)
        {
            return Err(fault(
                &format!("/spec/elements/{}/props/binding", pointer_token(key)),
                format!("the chart `{chart}` does not read `{binding}`"),
            ));
        }
    }
    match view
        .charts
        .keys()
        .find(|name| !drawn.contains(name.as_str()))
    {
        Some(name) => Err(fault(
            &format!("/charts/{}", pointer_token(name)),
            "no Chart component draws this chart, so it is never shown",
        )),
        None => Ok(()),
    }
}

/// Every binding is named by a component or read by a drawable chart.
fn unshown(view: &ViewDocument, shown: &BTreeSet<&str>, read: &[String]) -> Result<(), ApiError> {
    match view
        .bindings
        .iter()
        .position(|binding| !shown.contains(binding.name.as_str()) && !read.contains(&binding.name))
    {
        Some(index) => Err(fault(
            &format!("/bindings/{index}"),
            "nothing in the View shows this binding",
        )),
        None => Ok(()),
    }
}

/// The child keys an element names.
fn children(element: &Value) -> impl Iterator<Item = &str> {
    element
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

/// One string prop of an element.
fn prop_str<'a>(element: &'a Value, prop: &str) -> Option<&'a str> {
    element
        .get("props")
        .and_then(|props| props.get(prop))
        .and_then(Value::as_str)
}

/// The entries of an object in key order.
fn sorted(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut entries: Vec<(&String, &Value)> = map.iter().collect();
    entries.sort_by_key(|(left, _)| *left);
    entries
}

/// A View-level refusal at `pointer` into the `ViewDocument`.
fn fault(pointer: &str, message: impl Into<String>) -> ApiError {
    let refused = ApiError::new(ErrorCode::InvalidInput, message);
    if pointer.is_empty() {
        refused
    } else {
        refused.with_field(pointer)
    }
}

#[cfg(test)]
mod tests;
