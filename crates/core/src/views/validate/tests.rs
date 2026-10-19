//! Unit tests of `views::validate`: the design's vectors V1 to V16 and C1 to C4 (core M1.6
//! design section 3), over a valid `json_render` View (F0) and a valid `vega_lite` View (G0).

use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::WorkspaceId;
use okf_jawn_contract::views::{ChartFailure, ViewDocument};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{ChartCheck, Checked, validate};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const WORKSPACE: u128 = 1;

fn err_of<T>(result: Result<T, ApiError>) -> Result<ApiError, Box<dyn std::error::Error>> {
    match result {
        Ok(_) => Err("expected an error".into()),
        Err(error) => Ok(error),
    }
}

fn some<T>(option: Option<T>, what: &str) -> Result<T, Box<dyn std::error::Error>> {
    option.ok_or_else(|| format!("missing: {what}").into())
}

fn workspace(id: u128) -> WorkspaceId {
    WorkspaceId(Uuid::from_u128(id))
}

fn source(item: &str, path: &str, selection: &Value) -> Value {
    json!({
        "workspace_id": workspace(WORKSPACE),
        "item_id": item,
        "path": path,
        "revision": "0123456789abcdef0123456789abcdef01234567",
        "selection": selection
    })
}

fn binding(name: &str, source: &Value) -> Value {
    json!({ "name": name, "source": source, "units": {}, "transforms": [] })
}

/// F0: a pinned `json_render` View with a chart over `report` and an excerpt of `venue`.
fn f0() -> Value {
    json!({
        "schema_version": 1,
        "title": "Pipeline throughput",
        "description": "How many documents each stage handled.",
        "mode": "pinned",
        "grammar": "json_render",
        "bindings": [
            binding("report", &source(
                "22222222-3333-4444-8555-666666666666",
                "reports/pipeline-pdf.md",
                &json!({ "kind": "pages", "range": { "start": 3, "end": 3 } }),
            )),
            binding("venue", &source(
                "11111111-2222-4333-8444-555555555555",
                "notes/venue.md",
                &json!({ "kind": "all" }),
            ))
        ],
        "spec": {
            "root": "root",
            "elements": {
                "root": {
                    "type": "Stack", "props": { "title": "October import" },
                    "children": ["columns", "chart", "sources"],
                    "slots": { "default": ["columns", "chart", "sources"] }
                },
                "columns": {
                    "type": "Columns", "props": {}, "children": ["excerpt", "table"]
                },
                "excerpt": { "type": "SourceExcerpt", "props": { "binding": "venue" }, "children": [] },
                "table": { "type": "DataTable", "props": { "binding": "report" }, "children": [] },
                "chart": {
                    "type": "Chart",
                    "props": { "binding": "report", "chart": "stages", "title": "Documents per stage" },
                    "children": []
                },
                "sources": { "type": "SourceList", "props": { "bindings": ["report", "venue"] }, "children": [] }
            }
        },
        "charts": {
            "stages": {
                "data": { "name": "report" },
                "mark": "bar",
                "encoding": {
                    "x": { "field": "Stage", "type": "nominal" },
                    "y": { "field": "Documents", "type": "quantitative" }
                }
            }
        }
    })
}

/// G0: a `vega_lite` View over one binding.
fn g0() -> Value {
    json!({
        "schema_version": 1,
        "title": "Metrics",
        "description": "",
        "mode": "pinned",
        "grammar": "vega_lite",
        "bindings": [binding("metrics", &source(
            "22222222-3333-4444-8555-666666666666",
            "reports/pipeline-pdf.md",
            &json!({ "kind": "all" }),
        ))],
        "spec": {
            "data": { "name": "metrics" },
            "mark": "bar",
            "encoding": { "x": { "field": "category", "type": "nominal" } }
        }
    })
}

fn check(value: Value) -> Result<Result<Checked, ApiError>, Box<dyn std::error::Error>> {
    let view: ViewDocument = serde_json::from_value(value)?;
    Ok(validate(&view, workspace(WORKSPACE)))
}

/// `value` with the value at `pointer` replaced.
fn with(
    mut value: Value,
    pointer: &str,
    replacement: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    *some(value.pointer_mut(pointer), pointer)? = replacement;
    Ok(value)
}

/// `value` with `key` set in the object at `pointer`.
fn set(
    mut value: Value,
    pointer: &str,
    key: &str,
    replacement: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    some(
        value.pointer_mut(pointer).and_then(Value::as_object_mut),
        pointer,
    )?
    .insert(key.to_owned(), replacement);
    Ok(value)
}

/// The View-level refusal of `value`, asserted `invalid_input` on `field`.
fn refused_at(value: Value, field: &str) -> Result<ApiError, Box<dyn std::error::Error>> {
    let refused = err_of(check(value)?)?;
    assert_eq!(refused.code, ErrorCode::InvalidInput, "{}", refused.message);
    assert_eq!(refused.field.as_deref(), Some(field), "{}", refused.message);
    Ok(refused)
}

/// The one chart verdict of a View that passed.
fn chart_of(value: Value, index: usize) -> Result<ChartCheck, Box<dyn std::error::Error>> {
    let checked = check(value)??;
    Ok(some(checked.charts.get(index), "the chart")?.clone())
}

fn failure(check: &ChartCheck) -> Option<(ChartFailure, Option<String>, String)> {
    match check {
        ChartCheck::Refused {
            reason,
            binding,
            pointer,
            ..
        } => Some((*reason, binding.clone(), pointer.clone())),
        ChartCheck::Drawable { .. } => None,
    }
}

#[test]
fn a_valid_json_render_view_and_a_valid_vega_lite_view_pass() -> TestResult {
    let checked = check(f0())??;
    assert_eq!(
        checked.charts,
        vec![ChartCheck::Drawable {
            bindings: vec!["report".to_owned()]
        }]
    );
    assert_eq!(
        checked.tables.into_iter().collect::<Vec<_>>(),
        vec!["report"]
    );
    let vega = check(g0())??;
    assert_eq!(
        vega.charts,
        vec![ChartCheck::Drawable {
            bindings: vec!["metrics".to_owned()]
        }]
    );
    vega.require_drawable()?;
    Ok(())
}

#[test]
fn the_version_and_the_number_of_bindings_are_bounded() -> TestResult {
    refused_at(with(f0(), "/schema_version", json!(2))?, "/schema_version")?;
    // 50 bindings, all listed by one SourceList, pass; 51 are refused.
    let many = |count: usize| -> Result<Value, Box<dyn std::error::Error>> {
        let bindings: Vec<Value> = (0..count)
            .map(|index| {
                binding(
                    &format!("b{index}"),
                    &source(
                        "11111111-2222-4333-8444-555555555555",
                        "notes/venue.md",
                        &json!({ "kind": "all" }),
                    ),
                )
            })
            .collect();
        let names: Vec<Value> = (0..count).map(|index| json!(format!("b{index}"))).collect();
        let mut value = with(f0(), "/bindings", Value::Array(bindings))?;
        value = with(
            value,
            "/spec",
            json!({
                "root": "list",
                "elements": { "list": { "type": "SourceList", "props": { "bindings": names }, "children": [] } }
            }),
        )?;
        with(value, "/charts", json!({}))
    };
    // The catalog's SourceList lists at most 50, so 51 bindings need two lists; the bound
    // refuses them first.
    check(many(50)?)??;
    refused_at(many(51)?, "/bindings")?;
    Ok(())
}

#[test]
fn binding_names_are_one_to_120_characters_and_unique() -> TestResult {
    refused_at(
        with(f0(), "/bindings/1/name", json!("report"))?,
        "/bindings/1/name",
    )?;
    refused_at(
        with(f0(), "/bindings/1/name", json!(""))?,
        "/bindings/1/name",
    )?;
    let long = "v".repeat(121);
    refused_at(
        with(f0(), "/bindings/1/name", json!(long))?,
        "/bindings/1/name",
    )?;
    // 120 characters pass, every reference renamed with it.
    let name = "é".repeat(120);
    let mut value = with(f0(), "/bindings/1/name", json!(name))?;
    value = with(value, "/spec/elements/excerpt/props/binding", json!(name))?;
    value = with(
        value,
        "/spec/elements/sources/props/bindings/1",
        json!(name),
    )?;
    check(value)??;
    Ok(())
}

#[test]
fn a_binding_in_another_workspace_or_with_a_supplied_dataset_is_refused() -> TestResult {
    let foreign = with(f0(), "/bindings/1/source/workspace_id", json!(workspace(2)))?;
    let refused = refused_at(foreign, "/bindings")?;
    assert!(
        refused.message.contains("own workspace"),
        "{}",
        refused.message
    );
    refused_at(
        set(f0(), "/bindings/1", "materialized", json!("a".repeat(64)))?,
        "/bindings/1/materialized",
    )?;
    Ok(())
}

#[test]
fn binding_transforms_are_vega_lite_transforms_that_load_nothing() -> TestResult {
    // E3: a valid transform is kept and passes.
    let filtered = with(
        f0(),
        "/bindings/0/transforms",
        json!([{ "filter": "datum.Documents > 1" }]),
    )?;
    check(filtered)??;
    refused_at(
        with(f0(), "/bindings/0/transforms", json!([{ "explode": true }]))?,
        "/bindings/0/transforms/0",
    )?;
    let from_url =
        json!([{ "lookup": "Stage", "from": { "data": { "url": "x.json" }, "key": "Stage" } }]);
    refused_at(
        with(f0(), "/bindings/0/transforms", from_url)?,
        "/bindings/0/transforms/0/from/data/url",
    )?;
    let unknown =
        json!([{ "lookup": "Stage", "from": { "data": { "name": "nope" }, "key": "Stage" } }]);
    refused_at(
        with(f0(), "/bindings/0/transforms", unknown)?,
        "/bindings/0/transforms/0/from/data/name",
    )?;
    // A lookup of another binding: the chart that reads `report` reads `venue` too.
    let lookup =
        json!([{ "lookup": "Stage", "from": { "data": { "name": "venue" }, "key": "Stage" } }]);
    let checked = check(with(f0(), "/bindings/0/transforms", lookup)?)??;
    assert_eq!(
        checked.charts,
        vec![ChartCheck::Drawable {
            bindings: vec!["report".to_owned(), "venue".to_owned()]
        }]
    );
    Ok(())
}

#[test]
fn a_vega_lite_view_has_no_named_charts_and_shows_every_binding() -> TestResult {
    refused_at(
        set(g0(), "", "charts", json!({ "x": { "mark": "bar" } }))?,
        "/charts",
    )?;
    let mut extra = g0();
    some(
        extra.pointer_mut("/bindings").and_then(Value::as_array_mut),
        "bindings",
    )?
    .push(binding(
        "other",
        &source(
            "11111111-2222-4333-8444-555555555555",
            "notes/venue.md",
            &json!({ "kind": "all" }),
        ),
    ));
    refused_at(extra, "/bindings/1")?;
    Ok(())
}

#[test]
fn the_layout_meets_the_catalog_component_by_component() -> TestResult {
    let script = err_of(check(set(
        f0(),
        "/spec/elements",
        "script",
        json!({ "type": "Script", "props": {}, "children": [] }),
    )?)?)?;
    assert_eq!(script.code, ErrorCode::InvalidInput);
    assert!(
        script
            .field
            .as_deref()
            .is_some_and(|field| field.starts_with("/spec/elements/script")),
        "{:?}",
        script.field
    );
    let untitled = with(
        f0(),
        "/spec/elements/chart/props",
        json!({ "binding": "report", "chart": "stages" }),
    )?;
    refused_at(untitled, "/spec/elements/chart/props")?;
    refused_at(
        set(
            f0(),
            "/spec/elements/chart",
            "on",
            json!({ "press": { "action": "x" } }),
        )?,
        "/spec/elements/chart",
    )?;
    Ok(())
}

#[test]
fn a_view_is_a_static_composition() -> TestResult {
    refused_at(set(f0(), "/spec", "state", json!({}))?, "/spec/state")?;
    refused_at(
        set(f0(), "/spec/elements/table", "visible", json!(true))?,
        "/spec/elements/table/visible",
    )?;
    refused_at(
        set(
            f0(),
            "/spec/elements/columns",
            "repeat",
            json!({ "statePath": "/rows" }),
        )?,
        "/spec/elements/columns/repeat",
    )?;
    Ok(())
}

#[test]
fn the_root_children_and_slots_name_elements_and_leaves_hold_none() -> TestResult {
    refused_at(with(f0(), "/spec/root", json!("missing"))?, "/spec/root")?;
    refused_at(
        with(
            f0(),
            "/spec/elements/root/children",
            json!(["columns", "ghost"]),
        )?,
        "/spec/elements/root/children/1",
    )?;
    let reordered = with(
        f0(),
        "/spec/elements/root/slots",
        json!({ "default": ["chart", "columns", "sources"] }),
    )?;
    refused_at(reordered, "/spec/elements/root/slots")?;
    let mut parent = with(f0(), "/spec/elements/table/children", json!(["excerpt"]))?;
    parent = with(parent, "/spec/elements/columns/children", json!(["table"]))?;
    refused_at(parent, "/spec/elements/table/children")?;
    Ok(())
}

#[test]
fn a_cycle_or_an_element_the_root_does_not_reach_is_refused() -> TestResult {
    let cycle = with(
        f0(),
        "/spec/elements/columns/children",
        json!(["excerpt", "table", "root"]),
    )?;
    let refused = refused_at(cycle, "/spec/elements/columns/children/2")?;
    assert!(refused.message.contains("cycle"), "{}", refused.message);
    let orphan = set(
        f0(),
        "/spec/elements",
        "loose",
        json!({ "type": "SourceList", "props": { "bindings": ["venue"] }, "children": [] }),
    )?;
    refused_at(orphan, "/spec/elements/loose")?;
    // One element under two parents is shown twice, and allowed.
    let twice = with(
        f0(),
        "/spec/elements/columns/children",
        json!(["excerpt", "table", "sources"]),
    )?;
    check(twice)??;
    Ok(())
}

#[test]
fn props_name_declared_bindings_and_charts() -> TestResult {
    refused_at(
        with(f0(), "/spec/elements/table/props/binding", json!("nope"))?,
        "/spec/elements/table/props/binding",
    )?;
    refused_at(
        with(f0(), "/spec/elements/chart/props/chart", json!("nope"))?,
        "/spec/elements/chart/props/chart",
    )?;
    refused_at(
        with(
            f0(),
            "/spec/elements/sources/props/bindings/1",
            json!("nope"),
        )?,
        "/spec/elements/sources/props/bindings/1",
    )?;
    // A key holding `/` is escaped in the pointer.
    let mut slashed = f0();
    let elements = some(
        slashed
            .pointer_mut("/spec/elements")
            .and_then(Value::as_object_mut),
        "elements",
    )?;
    let table = some(elements.remove("table"), "the table")?;
    elements.insert("a/b".to_owned(), table);
    slashed = with(slashed, "/spec/elements/columns/children/1", json!("a/b"))?;
    slashed = with(slashed, "/spec/elements/a~1b/props/binding", json!("nope"))?;
    refused_at(slashed, "/spec/elements/a~1b/props/binding")?;
    Ok(())
}

#[test]
fn every_chart_is_drawn_and_shows_a_binding_it_reads() -> TestResult {
    let spare = set(
        f0(),
        "/charts",
        "spare",
        json!({ "data": { "name": "report" }, "mark": "bar" }),
    )?;
    refused_at(spare, "/charts/spare")?;
    let other = with(f0(), "/spec/elements/chart/props/binding", json!("venue"))?;
    let refused = refused_at(other, "/spec/elements/chart/props/binding")?;
    assert!(
        refused.message.contains("does not read"),
        "{}",
        refused.message
    );
    // When the chart is refused, its own status says why, and the component is not judged.
    let bogus = with(
        with(f0(), "/charts/stages/mark", json!("bogus"))?,
        "/spec/elements/chart/props/binding",
        json!("venue"),
    )?;
    let checked = check(bogus)??;
    assert!(matches!(
        some(checked.charts.first(), "the chart")?,
        ChartCheck::Refused {
            reason: ChartFailure::InvalidSpec,
            ..
        }
    ));
    Ok(())
}

#[test]
fn every_binding_is_shown() -> TestResult {
    let mut value = f0();
    some(
        value.pointer_mut("/bindings").and_then(Value::as_array_mut),
        "bindings",
    )?
    .push(binding(
        "extra",
        &source(
            "11111111-2222-4333-8444-555555555555",
            "notes/venue.md",
            &json!({ "kind": "all" }),
        ),
    ));
    refused_at(value, "/bindings/2")?;
    Ok(())
}

#[test]
fn a_chart_that_fails_vega_lites_schema_is_refused_at_the_deepest_fault() -> TestResult {
    let bogus = chart_of(with(f0(), "/charts/stages/mark", json!("bogus"))?, 0)?;
    let (reason, binding, pointer) = some(failure(&bogus), "a refusal")?;
    assert_eq!(reason, ChartFailure::InvalidSpec);
    assert_eq!(binding, None);
    assert_eq!(pointer, "/charts/stages/mark");
    let layered =
        json!({ "data": { "name": "metrics" }, "layer": [{ "mark": "bar" }], "bogus": 1 });
    let check = chart_of(with(g0(), "/spec", layered)?, 0)?;
    assert_eq!(
        some(failure(&check), "a refusal")?.0,
        ChartFailure::InvalidSpec
    );
    // A saved View refuses it.
    let refused = err_of(check_saved(with(
        f0(),
        "/charts/stages/mark",
        json!("bogus"),
    )?)?)?;
    assert_eq!(refused.field.as_deref(), Some("/charts/stages/mark"));
    Ok(())
}

fn check_saved(value: Value) -> Result<Result<(), ApiError>, Box<dyn std::error::Error>> {
    Ok(check(value)?.and_then(|checked| checked.require_drawable()))
}

#[test]
fn a_chart_that_loads_or_runs_anything_is_refused_where_it_does() -> TestResult {
    let cases = [
        (
            json!({ "data": { "url": "https://x/y.csv" }, "mark": "bar" }),
            "/spec/data/url",
        ),
        (
            json!({ "data": { "values": [{ "a": 1 }] }, "mark": "bar" }),
            "/spec/data/values",
        ),
        (
            json!({ "data": { "sequence": { "start": 0, "stop": 3 } }, "mark": "bar" }),
            "/spec/data/sequence",
        ),
        (
            json!({ "data": { "name": "metrics", "format": { "type": "json" } }, "mark": "bar" }),
            "/spec/data/format",
        ),
        (
            json!({ "data": { "name": "metrics" }, "datasets": { "metrics": [] }, "mark": "bar" }),
            "/spec/datasets",
        ),
        (
            json!({ "data": { "name": "metrics" }, "mark": "image", "encoding": { "url": { "value": "https://x/a.png" } } }),
            "/spec/encoding/url",
        ),
        (
            json!({ "data": { "name": "metrics" }, "mark": "bar", "transform": [{ "lookup": "k", "from": { "data": { "url": "x.json" }, "key": "k" } }] }),
            "/spec/transform/0/from/data/url",
        ),
        (
            json!({ "data": { "name": "metrics" }, "mark": "bar", "encoding": { "href": { "field": "link" } } }),
            "/spec/encoding/href",
        ),
        (
            json!({ "data": { "name": "metrics" }, "mark": "text", "encoding": { "text": { "field": "v", "formatType": "money" } } }),
            "/spec/encoding/text/formatType",
        ),
        (
            json!({ "data": { "name": "metrics" }, "mark": "bar", "params": [{ "name": "p", "value": 1, "bind": { "element": "#host" } }] }),
            "/spec/params/0/bind/element",
        ),
        (
            json!({ "data": { "name": "metrics" }, "mark": "bar", "usermeta": { "url": "x" } }),
            "/spec/usermeta/url",
        ),
    ];
    for (spec, pointer) in cases {
        let verdict = chart_of(with(g0(), "/spec", spec)?, 0)?;
        let (reason, _, at) = some(failure(&verdict), pointer)?;
        assert_eq!(reason, ChartFailure::InvalidSpec, "{pointer}");
        assert_eq!(at, pointer);
    }
    // Allowed: a Vega expression, a built-in format type, an input binding.
    let allowed = json!({
        "data": { "name": "metrics" },
        "mark": "text",
        "transform": [{ "filter": "datum.value > 1" }],
        "params": [{ "name": "p", "value": 1, "bind": { "input": "range", "min": 0, "max": 9 } }],
        "encoding": { "text": { "field": "value", "type": "quantitative", "formatType": "number" } }
    });
    assert_eq!(
        chart_of(with(g0(), "/spec", allowed)?, 0)?,
        ChartCheck::Drawable {
            bindings: vec!["metrics".to_owned()]
        }
    );
    Ok(())
}

#[test]
fn a_chart_reads_named_datasets_that_are_bindings() -> TestResult {
    let unnamed = chart_of(with(g0(), "/spec", json!({ "mark": "bar" }))?, 0)?;
    let (reason, _, pointer) = some(failure(&unnamed), "a refusal")?;
    assert_eq!(reason, ChartFailure::InvalidSpec);
    assert_eq!(pointer, "/spec");
    let unknown = chart_of(with(g0(), "/spec/data/name", json!("nope"))?, 0)?;
    assert_eq!(
        failure(&unknown),
        Some((
            ChartFailure::UnknownBinding,
            Some("nope".to_owned()),
            "/spec/data/name".to_owned()
        ))
    );
    // A layer inherits the top-level data.
    let layered =
        json!({ "data": { "name": "metrics" }, "layer": [{ "mark": "bar" }, { "mark": "rule" }] });
    assert_eq!(
        chart_of(with(g0(), "/spec", layered)?, 0)?,
        ChartCheck::Drawable {
            bindings: vec!["metrics".to_owned()]
        }
    );
    // A layer over a second binding reads both.
    let mut two = g0();
    some(
        two.pointer_mut("/bindings").and_then(Value::as_array_mut),
        "bindings",
    )?
    .push(binding(
        "venue",
        &source(
            "11111111-2222-4333-8444-555555555555",
            "notes/venue.md",
            &json!({ "kind": "all" }),
        ),
    ));
    let both = json!({ "data": { "name": "metrics" }, "layer": [{ "mark": "bar" }, { "mark": "rule", "data": { "name": "venue" } }] });
    assert_eq!(
        chart_of(with(two, "/spec", both)?, 0)?,
        ChartCheck::Drawable {
            bindings: vec!["metrics".to_owned(), "venue".to_owned()]
        }
    );
    Ok(())
}

#[test]
fn needed_bindings_are_what_drawable_charts_and_tables_read() -> TestResult {
    let checked = check(f0())??;
    assert_eq!(
        checked.needed().into_iter().collect::<Vec<_>>(),
        vec!["report"]
    );
    Ok(())
}
