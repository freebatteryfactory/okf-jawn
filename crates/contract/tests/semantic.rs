//! Semantic controls for serialization and validation; not whole-product acceptance.

use okf_jawn_contract::access::{AccessRoute, Permission};
use okf_jawn_contract::common::CellRange;
use okf_jawn_contract::events::{Event, EventKind};
use okf_jawn_contract::extraction::{
    ConversionOutcome, ConversionSettings, ExtractionStatus, FailureReason, PageCoverage,
    TextOrigin,
};
use okf_jawn_contract::history::CommitRequest;
use okf_jawn_contract::identity::{At, Digest, Revision, Timestamp, WorkspaceId, WorkspacePath};
use okf_jawn_contract::import::{Job, JobKind, RedigestRequest, StartImportRequest};
use okf_jawn_contract::item::{APP_HEADER_KEY, ItemStatus, ListItemsRequest};
use okf_jawn_contract::metadata::{OperationName, operations};
use okf_jawn_contract::proposal::{Change, ProposalKind};
use okf_jawn_contract::read::{AssetRole, ReadItemRequest};
use okf_jawn_contract::review::ReviewCoverage;
use okf_jawn_contract::search::SearchRequest;
use okf_jawn_contract::source::{SourceLocation, SourceReference};
use okf_jawn_contract::views::{ChartRef, ChartResult, ChartStatus, Dataset, ViewDocument};
use okf_jawn_contract::workspace::{ArtifactKind, ArtifactScope, DownloadArtifact};
use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

macro_rules! operation_schemas {
    ($(($id:ident, $request:ty, $response:ty, $path:literal, $label:literal, $alias:literal,
        $operator:literal, $visibility:literal, $permission:ident, $ui:literal, $status:literal,
        $destructive:literal, $description:literal)),* $(,)?) => {
        /// Every operation's input and output schema, labelled by operation and side.
        fn operation_schemas() -> Result<Vec<(String, Value)>, serde_json::Error> {
            Ok(vec![$(
                (concat!(stringify!($id), " input").to_owned(), schema_of::<$request>()?),
                (concat!(stringify!($id), " output").to_owned(), schema_of::<$response>()?),
            )*])
        }

        /// Operations whose response is a `Job`.
        fn job_returning() -> Vec<&'static str> {
            let mut found = Vec::new();
            $(if stringify!($response).replace(' ', "").ends_with("::import::Job") {
                found.push(stringify!($id));
            })*
            found
        }
    };
}

/// Wire names of every job kind, one per operation that starts a job.
const JOB_KINDS: &[&str] = &[
    "import",
    "redigest",
    "export_workspace",
    "backup_workspace",
    "restore_workspace",
    "rebuild_index",
    "export_view",
    "backup_installation",
    "purge_workspace",
    "purge_item",
];
/// Job kinds that run at the tenant level, in `JOB_KINDS` order.
const TENANT_JOB_KINDS: &[&str] = &["backup_installation", "purge_workspace", "purge_item"];
const ITEM: &str = "22222222-2222-4222-8222-222222222222";
const OTHER_ITEM: &str = "33333333-3333-4333-8333-333333333333";

fn schema_of<T: JsonSchema>() -> Result<Value, serde_json::Error> {
    serde_json::to_value(
        SchemaSettings::draft2020_12()
            .into_generator()
            .into_root_schema_for::<T>(),
    )
}

/// The wire words of a plain enum's schema.
fn enum_words<T: JsonSchema>() -> Result<Vec<String>, Box<dyn Error>> {
    let schema = schema_of::<T>()?;
    let mut words = Vec::new();
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        words.extend(values.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    for branch in schema
        .get("oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(word) = branch.get("const").and_then(Value::as_str) {
            words.push(word.to_owned());
        }
        if let Some(values) = branch.get("enum").and_then(Value::as_array) {
            words.extend(values.iter().filter_map(Value::as_str).map(str::to_owned));
        }
    }
    Ok(words)
}

fn citation() -> Value {
    json!({
        "workspace_id": "11111111-1111-4111-8111-111111111111",
        "item_id": ITEM,
        "path": "data/metrics.xlsx",
        "revision": "a".repeat(40),
        "selection": {"kind": "all"}
    })
}

fn coverage(
    page_count: u32,
    converted: &[(u32, u32)],
    partly_extracted: &[(u32, u32)],
    not_converted: &[(u32, u32)],
) -> Result<PageCoverage, serde_json::Error> {
    let ranges = |list: &[(u32, u32)]| -> Value {
        list.iter()
            .map(|(start, end)| json!({"start": start, "end": end}))
            .collect()
    };
    serde_json::from_value(json!({
        "page_count": page_count,
        "converted": ranges(converted),
        "partly_extracted": ranges(partly_extracted),
        "not_converted": ranges(not_converted)
    }))
}

fn supply(item: &str) -> Value {
    json!({"kind": "supply_extraction", "item_id": item, "markdown": "# Supplied"})
}

fn job(kind: &str, workspace: bool) -> Value {
    let mut job = json!({
        "id": "66666666-6666-4666-8666-666666666666",
        "kind": kind,
        "state": "queued",
        "progress": 0,
        "attempt": 1,
        "warnings": [],
        "item_ids": []
    });
    if workspace && let Some(object) = job.as_object_mut() {
        object.insert(
            "workspace_id".to_owned(),
            json!("11111111-1111-4111-8111-111111111111"),
        );
    }
    job
}

fn binding(name: &str, workspace: &str) -> serde_json::Value {
    json!({
        "name": name,
        "source": {
            "workspace_id": workspace,
            "item_id": "22222222-2222-4222-8222-222222222222",
            "path": "data/metrics.csv",
            "revision": "a".repeat(40),
            "selection": {"kind": "all"}
        },
        "units": {},
        "transforms": []
    })
}

#[test]
fn revision_does_not_accept_a_selector() -> Result<(), Box<dyn Error>> {
    assert!(Revision::try_from("latest".to_owned()).is_err());
    assert!(Revision::try_from("A".repeat(40)).is_err());
    let revision = Revision::try_from("a".repeat(40))?;
    assert_eq!(revision.as_str(), "a".repeat(40));
    let selector: At = serde_json::from_value(json!({"kind":"latest"}))?;
    assert_eq!(selector, At::Latest);
    Ok(())
}

#[test]
fn workspace_paths_reject_parent_and_git_traversal() -> Result<(), Box<dyn Error>> {
    for path in [
        "../client",
        "/client",
        "a/../b",
        "a//b",
        "C:/docs",
        ".git/config",
        "a\\b",
    ] {
        assert!(WorkspacePath::try_from(path.to_owned()).is_err(), "{path}");
    }
    assert_eq!(
        WorkspacePath::try_from("Clients/one.md".to_owned())?.as_str(),
        "Clients/one.md"
    );
    Ok(())
}

#[test]
fn optional_cursor_is_omitted_or_null_without_changing_semantics() -> Result<(), Box<dyn Error>> {
    let raw = json!({"workspace_id":"11111111-1111-4111-8111-111111111111",
        "item_id":"22222222-2222-4222-8222-222222222222", "at":{"kind":"latest"},
        "view":"text", "selection":{"kind":"all"}, "max_bytes":4096,"max_images":0});
    let missing: ReadItemRequest = serde_json::from_value(raw.clone())?;
    let mut explicit = raw;
    explicit
        .as_object_mut()
        .ok_or("fixture must be object")?
        .insert("cursor".to_owned(), serde_json::Value::Null);
    let nullable: ReadItemRequest = serde_json::from_value(explicit)?;
    assert_eq!(missing.cursor, nullable.cursor);
    assert!(serde_json::to_value(missing)?.get("cursor").is_none());
    Ok(())
}

#[test]
fn content_identity_cannot_be_a_filename() -> Result<(), Box<dyn Error>> {
    assert!(Digest::try_from("FINAL.pdf".to_owned()).is_err());
    assert_eq!(Digest::try_from("b".repeat(64))?.as_str(), "b".repeat(64));
    Ok(())
}

#[test]
fn the_present_dataset_fixture_is_a_dataset_of_the_metrics_binding() -> Result<(), Box<dyn Error>> {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/views");
    let raw = fs::read_to_string(fixtures.join("present-metrics.dataset.json"))?;
    let dataset: Dataset = serde_json::from_str(&raw)?;
    dataset.check()?;
    assert_eq!(dataset.schema_version, 1);
    assert_eq!(dataset.text_origin, TextOrigin::Converter);
    assert_eq!(dataset.columns.len(), 2);
    assert_eq!(dataset.rows.len(), 5);
    // The citation is the `metrics` binding's source in the present fixture, so a View that
    // binds it reads this dataset.
    let present: Value =
        serde_json::from_str(&fs::read_to_string(fixtures.join("present-response.json"))?)?;
    let bindings = present
        .get("resolved_bindings")
        .and_then(Value::as_array)
        .ok_or("present-response.json has resolved_bindings")?;
    let metrics = bindings
        .iter()
        .find(|binding| binding.get("name") == Some(&json!("metrics")))
        .and_then(|binding| binding.get("source"))
        .ok_or("present-response.json binds metrics")?;
    let cited = serde_json::from_value::<SourceReference>(metrics.clone())?;
    assert_eq!(
        serde_json::to_value(&cited)?,
        serde_json::to_value(&dataset.source)?
    );
    Ok(())
}

#[test]
fn view_document_six_component_round_trips_with_deny_unknown_fields() -> Result<(), Box<dyn Error>>
{
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/views/view-document-six-component.json");
    let raw = fs::read_to_string(&path)?;
    let parsed: ViewDocument = serde_json::from_str(&raw)?;
    assert_eq!(parsed.schema_version, 1);
    assert_eq!(
        parsed.grammar,
        okf_jawn_contract::views::RenderGrammar::JsonRender
    );
    assert_eq!(parsed.spec.get("root"), Some(&json!("root")));
    assert_eq!(
        parsed.spec.pointer("/elements/root/type").cloned(),
        Some(json!("Stack"))
    );
    let reserialized = serde_json::to_value(&parsed)?;
    let again: ViewDocument = serde_json::from_value(reserialized.clone())?;
    assert_eq!(serde_json::to_value(&again)?, reserialized);
    let mut unknown = serde_json::from_str::<serde_json::Value>(&raw)?;
    unknown
        .as_object_mut()
        .ok_or("fixture must be object")?
        .insert("unexpected_field".to_owned(), json!(true));
    assert!(serde_json::from_value::<ViewDocument>(unknown).is_err());
    Ok(())
}

#[test]
fn a_snapshot_request_carries_no_expected_head() -> Result<(), Box<dyn Error>> {
    let request = json!({
        "workspace_id": "11111111-1111-4111-8111-111111111111",
        "item_ids": ["22222222-2222-4222-8222-222222222222"],
        "message": "Quarterly numbers",
        "idempotency_key": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    });
    let decoded: CommitRequest = serde_json::from_value(request.clone())?;
    assert_eq!(decoded.item_ids.len(), 1);
    let mut with_head = request;
    with_head
        .as_object_mut()
        .ok_or("request must be an object")?
        .insert("expected_head".to_owned(), json!("a".repeat(40)));
    assert!(
        serde_json::from_value::<CommitRequest>(with_head).is_err(),
        "head movement alone is not a Snapshot precondition"
    );
    Ok(())
}

#[test]
fn every_job_states_its_kind() -> Result<(), Box<dyn Error>> {
    let decoded: Job = serde_json::from_value(job("import", true))?;
    assert_eq!(decoded.kind, JobKind::Import);
    for kind in JOB_KINDS {
        let tenant = TENANT_JOB_KINDS.contains(kind);
        let decoded: Job = serde_json::from_value(job(kind, !tenant))?;
        assert_eq!(serde_json::to_value(decoded.kind)?, json!(kind));
        assert_eq!(decoded.kind.is_tenant(), tenant, "{kind}");
    }
    let mut job = job("import", true);
    job.as_object_mut()
        .ok_or("job must be an object")?
        .remove("kind");
    assert!(
        serde_json::from_value::<Job>(job).is_err(),
        "a job without a kind must not decode"
    );
    Ok(())
}

#[test]
fn every_operation_that_starts_a_job_has_a_kind() {
    let starters: Vec<&str> = operations()
        .iter()
        .filter(|operation| operation.success_status == 202 && operation.id != "retry_job")
        .map(|operation| operation.id)
        .collect();
    assert_eq!(
        starters.len(),
        JOB_KINDS.len(),
        "202 operations: {starters:?}"
    );
}

#[test]
fn a_view_reports_the_bindings_outside_its_workspace() -> Result<(), Box<dyn Error>> {
    let home = "11111111-1111-4111-8111-111111111111";
    let elsewhere = "33333333-3333-4333-8333-333333333333";
    let view: ViewDocument = serde_json::from_value(json!({
        "schema_version": 1,
        "title": "Metrics",
        "description": "Quarterly metrics",
        "mode": "pinned",
        "grammar": "vega_lite",
        "bindings": [binding("local", home), binding("foreign", elsewhere)],
        "spec": {}
    }))?;
    let workspace: WorkspaceId = serde_json::from_value(json!(home))?;
    let outside: Vec<&str> = view
        .bindings_outside(workspace)
        .into_iter()
        .map(|found| found.name.as_str())
        .collect();
    assert_eq!(outside, ["foreign"]);
    let other: WorkspaceId = serde_json::from_value(json!(elsewhere))?;
    let outside_other: Vec<&str> = view
        .bindings_outside(other)
        .into_iter()
        .map(|found| found.name.as_str())
        .collect();
    assert_eq!(outside_other, ["local"]);
    Ok(())
}

#[test]
fn workspace_path_schema_states_the_expressible_part_of_its_rule() -> Result<(), Box<dyn Error>> {
    let schema = serde_json::to_value(
        SchemaSettings::draft2020_12()
            .into_generator()
            .into_root_schema_for::<WorkspacePath>(),
    )?;
    assert_eq!(
        schema.get("pattern"),
        Some(&json!(
            r#"^[^/\\:<>"|?*\x00-\x1f]+(/[^/\\:<>"|?*\x00-\x1f]+)*$"#
        ))
    );
    assert_eq!(schema.get("minLength"), Some(&json!(1)));
    assert_eq!(schema.get("maxLength"), Some(&json!(4096)));
    // What a regular expression cannot state stays in `TryFrom`.
    for path in [
        ".",
        "..",
        "a/./b",
        "a/../b",
        ".git/config",
        "notes/.GIT/x",
        "a\u{7f}b",
    ] {
        assert!(
            WorkspacePath::try_from(path.to_owned()).is_err(),
            "{path:?}"
        );
    }
    Ok(())
}

#[test]
fn conversion_outcomes_are_tagged_by_status_and_closed() -> Result<(), Box<dyn Error>> {
    for document in [
        json!({"status": "pending"}),
        json!({"status": "completed"}),
        json!({
            "status": "partial",
            "coverage": {
                "page_count": 3,
                "converted": [{"start": 1, "end": 2}],
                "partly_extracted": [],
                "not_converted": [{"start": 3, "end": 3}]
            },
            "issues": [{"component_type": "model", "module_name": "pipeline", "error_message": "budget"}]
        }),
        json!({"status": "partial", "issues": []}),
        json!({"status": "failed", "reason": {"kind": "damaged"}, "issues": []}),
        json!({"status": "failed", "reason": {"kind": "memory_limit", "limit_bytes": "2147483648"}, "issues": []}),
        json!({"status": "failed", "reason": {"kind": "time_limit", "limit_seconds": 600}, "issues": []}),
        json!({"status": "unsupported"}),
    ] {
        let outcome: ConversionOutcome = serde_json::from_value(document.clone())?;
        assert_eq!(serde_json::to_value(&outcome)?, document);
    }
    for refused in [
        json!({"status": "succeeded"}),
        json!({"status": "partial", "issues": [], "message": "half"}),
        json!({"status": "failed", "reason": {"kind": "damaged"}, "issues": [], "text": "x"}),
        json!({"status": "failed", "reason": {"kind": "out_of_disk"}, "issues": []}),
    ] {
        assert!(
            serde_json::from_value::<ConversionOutcome>(refused.clone()).is_err(),
            "{refused}"
        );
    }
    Ok(())
}

#[test]
fn page_coverage_covers_every_page_once() -> Result<(), Box<dyn Error>> {
    for accepted in [
        coverage(5, &[(1, 2)], &[(3, 3)], &[(4, 5)])?,
        coverage(5, &[(1, 1), (3, 5)], &[], &[(2, 2)])?,
        coverage(1, &[(1, 1)], &[], &[])?,
        coverage(4, &[], &[], &[(1, 4)])?,
    ] {
        assert!(accepted.check().is_ok(), "{accepted:?}");
    }
    for (refused, field) in [
        (coverage(5, &[(1, 3)], &[], &[(3, 5)])?, "/not_converted/0"),
        (coverage(5, &[(1, 2)], &[], &[(4, 5)])?, "/not_converted/0"),
        (coverage(5, &[(1, 6)], &[], &[])?, "/converted/0"),
        (coverage(5, &[(3, 1)], &[], &[])?, "/converted/0"),
        (coverage(5, &[(4, 5), (1, 3)], &[], &[])?, "/converted/1"),
        (coverage(5, &[(1, 4)], &[], &[])?, "/page_count"),
        (coverage(3, &[(0, 3)], &[], &[])?, "/converted/0"),
        (coverage(0, &[], &[], &[])?, "/page_count"),
        (coverage(0, &[(1, 1)], &[], &[])?, "/page_count"),
    ] {
        let error = refused.check().err().ok_or("refused coverage")?;
        assert_eq!(error.field.as_deref(), Some(field), "{refused:?}");
    }
    Ok(())
}

#[test]
fn unprocessed_means_partial_failed_or_unsupported() {
    for (status, unprocessed) in [
        (ExtractionStatus::Pending, false),
        (ExtractionStatus::Completed, false),
        (ExtractionStatus::Partial, true),
        (ExtractionStatus::Failed, true),
        (ExtractionStatus::Unsupported, true),
    ] {
        assert_eq!(status.is_unprocessed(), unprocessed, "{status:?}");
    }
}

#[test]
fn a_failure_reason_is_typed() -> Result<(), Box<dyn Error>> {
    let schema = schema_of::<FailureReason>()?;
    let branches = schema
        .get("oneOf")
        .and_then(Value::as_array)
        .ok_or("FailureReason is a union")?;
    let mut kinds = Vec::new();
    for branch in branches {
        let properties = branch
            .get("properties")
            .and_then(Value::as_object)
            .ok_or("each reason is an object")?;
        kinds.push(
            properties
                .get("kind")
                .and_then(|kind| kind.get("const"))
                .and_then(Value::as_str)
                .ok_or("each reason states its kind")?
                .to_owned(),
        );
        for name in properties.keys() {
            assert!(
                matches!(name.as_str(), "kind" | "limit_bytes" | "limit_seconds"),
                "a failure reason holds no free text: {name}"
            );
        }
        assert!(
            branch.get("additionalProperties") == Some(&json!(false))
                || branch.get("unevaluatedProperties") == Some(&json!(false)),
            "a failure reason is closed: {branch}"
        );
    }
    assert_eq!(
        kinds,
        [
            "damaged",
            "memory_limit",
            "time_limit",
            "converter_crashed",
            "converter_error"
        ]
    );
    // serde ignores extra fields beside a unit variant's tag; the schema, which dispatch
    // validates first, is what closes it. A data variant is closed by serde as well.
    assert!(
        serde_json::from_value::<FailureReason>(
            json!({"kind": "time_limit", "limit_seconds": 60, "message": "x"})
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn location_provenance_is_carried_by_the_tag() -> Result<(), Box<dyn Error>> {
    let page = json!({"kind": "page", "page_no": 2});
    let direct = json!({"provenance": "direct", "locator": page});
    let inferred = json!({"provenance": "inferred", "locator": page});
    let unresolved = json!({"provenance": "unresolved", "reason": {"kind": "no_match"}});
    assert!(matches!(
        serde_json::from_value(direct.clone())?,
        SourceLocation::Direct { .. }
    ));
    assert!(matches!(
        serde_json::from_value(inferred.clone())?,
        SourceLocation::Inferred { .. }
    ));
    assert!(matches!(
        serde_json::from_value(unresolved.clone())?,
        SourceLocation::Unresolved { .. }
    ));
    for document in [direct, inferred, unresolved] {
        let location: SourceLocation = serde_json::from_value(document.clone())?;
        assert_eq!(serde_json::to_value(&location)?, document);
    }
    assert!(
        serde_json::from_value::<SourceLocation>(json!({"provenance": "exact", "locator": page}))
            .is_err()
    );
    Ok(())
}

#[test]
fn an_unresolved_location_has_no_locator() {
    let guessed = json!({
        "provenance": "unresolved",
        "reason": {"kind": "ambiguous_match", "occurrences": 2},
        "locator": {"kind": "page", "page_no": 1}
    });
    assert!(serde_json::from_value::<SourceLocation>(guessed).is_err());
}

#[test]
fn a_region_keeps_docling_coordinates() -> Result<(), Box<dyn Error>> {
    let pdf = json!({
        "provenance": "direct",
        "locator": {"kind": "region", "region": {
            "page_no": 1,
            "bbox": {"l": 72.0, "t": 720.5, "r": 540.25, "b": 700.0, "coord_origin": "bottom_left"},
            "page_size": {"width": 612.0, "height": 792.0}
        }}
    });
    let pptx = json!({
        "provenance": "direct",
        "locator": {"kind": "region", "region": {
            "page_no": 3,
            "bbox": {"l": 457_200.0, "t": 274_638.0, "r": 8_229_600.0, "b": 1_143_000.0, "coord_origin": "top_left"},
            "page_size": {"width": 9_144_000.0, "height": 6_858_000.0}
        }}
    });
    for document in [pdf, pptx] {
        let location: SourceLocation = serde_json::from_value(document.clone())?;
        assert_eq!(serde_json::to_value(&location)?, document);
    }
    Ok(())
}

#[test]
fn cell_ranges_are_one_based() -> Result<(), Box<dyn Error>> {
    let schema = schema_of::<CellRange>()?;
    for bound in ["row_start", "row_end", "column_start", "column_end"] {
        assert_eq!(
            schema.pointer(&format!("/properties/{bound}/minimum")),
            Some(&json!(1)),
            "{bound}"
        );
    }
    Ok(())
}

#[test]
fn citation_locations_are_omitted_when_empty_without_changing_semantics()
-> Result<(), Box<dyn Error>> {
    let absent: SourceReference = serde_json::from_value(citation())?;
    let mut explicit = citation();
    explicit
        .as_object_mut()
        .ok_or("citation is an object")?
        .insert("locations".to_owned(), json!([]));
    let empty: SourceReference = serde_json::from_value(explicit)?;
    assert_eq!(absent.locations, Vec::<SourceLocation>::new());
    assert_eq!(empty.locations, Vec::<SourceLocation>::new());
    assert!(serde_json::to_value(&empty)?.get("locations").is_none());
    Ok(())
}

#[test]
fn asset_roles_are_page_image_and_picture() -> Result<(), Box<dyn Error>> {
    assert_eq!(enum_words::<AssetRole>()?, ["page_image", "picture"]);
    Ok(())
}

#[test]
fn import_settings_are_optional_and_redigest_settings_typed() -> Result<(), Box<dyn Error>> {
    let mut start = json!({
        "workspace_id": "11111111-1111-4111-8111-111111111111",
        "base_revision": "a".repeat(40),
        "upload_ids": ["55555555-5555-4555-8555-555555555555"],
        "destination": "",
        "idempotency_key": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "apply_naming_rules": false
    });
    let defaults: StartImportRequest = serde_json::from_value(start.clone())?;
    assert!(defaults.settings.is_none());
    let settings = serde_json::to_value(ConversionSettings::default())?;
    assert_eq!(
        settings,
        json!({"ocr": "auto", "table_structure": true, "page_images": true, "page_image_dpi": 144})
    );
    start
        .as_object_mut()
        .ok_or("request is an object")?
        .insert("settings".to_owned(), settings.clone());
    let chosen: StartImportRequest = serde_json::from_value(start)?;
    assert_eq!(chosen.settings, Some(ConversionSettings::default()));
    let redigest = |settings: Value| {
        json!({
            "workspace_id": "11111111-1111-4111-8111-111111111111",
            "item_id": ITEM,
            "base_revision": "a".repeat(40),
            "settings": settings,
            "unconverted_only": true,
            "idempotency_key": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        })
    };
    let typed: RedigestRequest = serde_json::from_value(redigest(settings.clone()))?;
    assert!(typed.unconverted_only);
    let mut unknown = settings;
    unknown
        .as_object_mut()
        .ok_or("settings are an object")?
        .insert("quality".to_owned(), json!("high"));
    assert!(serde_json::from_value::<RedigestRequest>(redigest(unknown)).is_err());
    let too_fine = ConversionSettings {
        page_image_dpi: 300,
        ..ConversionSettings::default()
    };
    assert_eq!(
        too_fine.check().err().and_then(|error| error.field),
        Some("/settings/page_image_dpi".to_owned())
    );
    Ok(())
}

#[test]
fn a_dataset_row_has_one_value_per_column() -> Result<(), Box<dyn Error>> {
    let dataset = |rows: Value| -> Result<Dataset, serde_json::Error> {
        serde_json::from_value(json!({
            "schema_version": 1,
            "source": citation(),
            "text_origin": "converter",
            "columns": [
                {"name": "region", "kind": "string"},
                {"name": "revenue", "kind": "number", "unit": "EUR"},
                {"name": "orders", "kind": "integer"},
                {"name": "audited", "kind": "boolean"},
                {"name": "closed", "kind": "date_time"}
            ],
            "rows": rows,
            "warnings": [{"kind": "cell_values_only", "sheets": ["Q3"]}]
        }))
    };
    let accepted = dataset(json!([
        ["North", 1250.5, 12, true, "2026-09-30T00:00:00.000Z"],
        ["South", null, null, null, null]
    ]))?;
    assert!(accepted.check().is_ok());
    for (rows, field) in [
        (json!([["North", 1.0, 1, true]]), "/rows/0"),
        (json!([["North", "1250", 1, true, null]]), "/rows/0/1"),
        (json!([["North", 1.0, 1.5, true, null]]), "/rows/0/2"),
        (json!([["North", 1.0, 1, "yes", null]]), "/rows/0/3"),
        (json!([["North", 1.0, 1, true, "30/09/2026"]]), "/rows/0/4"),
    ] {
        let error = dataset(rows.clone())?
            .check()
            .err()
            .ok_or("refused dataset")?;
        assert_eq!(error.field.as_deref(), Some(field), "{rows}");
    }
    Ok(())
}

#[test]
fn every_chart_of_a_view_has_one_ref() -> Result<(), Box<dyn Error>> {
    let vega: ViewDocument = serde_json::from_value(json!({
        "schema_version": 1, "title": "Revenue", "description": "By region",
        "mode": "pinned", "grammar": "vega_lite", "bindings": [], "spec": {}
    }))?;
    assert_eq!(vega.chart_refs(), [ChartRef::Spec]);
    let composed: ViewDocument = serde_json::from_value(json!({
        "schema_version": 1, "title": "Revenue", "description": "By region",
        "mode": "pinned", "grammar": "json_render", "bindings": [], "spec": {},
        "charts": {"trend": {}, "mix": {}}
    }))?;
    assert_eq!(
        composed.chart_refs(),
        [
            ChartRef::Named {
                name: "mix".to_owned()
            },
            ChartRef::Named {
                name: "trend".to_owned()
            }
        ]
    );
    Ok(())
}

#[test]
fn a_failed_chart_states_a_typed_reason() -> Result<(), Box<dyn Error>> {
    let failed = json!({
        "chart": {"kind": "named", "name": "trend"},
        "status": {"status": "failed", "reason": "invalidated", "binding": "q3", "message": "The source was purged"}
    });
    let result: ChartResult = serde_json::from_value(failed.clone())?;
    assert!(matches!(result.status, ChartStatus::Failed { .. }));
    assert_eq!(serde_json::to_value(&result)?, failed);
    let ready = json!({"status": "ready", "bindings": ["q3"]});
    let status: ChartStatus = serde_json::from_value(ready.clone())?;
    assert_eq!(serde_json::to_value(&status)?, ready);
    assert!(
        serde_json::from_value::<ChartStatus>(
            json!({"status": "failed", "reason": "renderer_crashed", "message": "x"})
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn the_unprocessed_filter_is_optional_on_listing_and_search() -> Result<(), Box<dyn Error>> {
    let listing = |filter: Option<&str>| {
        let mut request = json!({
            "workspace_id": "11111111-1111-4111-8111-111111111111",
            "at": {"kind": "latest"},
            "folder": "",
            "page": {"limit": 50}
        });
        if let (Some(filter), Some(object)) = (filter, request.as_object_mut()) {
            object.insert("extraction".to_owned(), json!(filter));
        }
        request
    };
    let search = |filter: Option<&str>| {
        let mut request = json!({
            "workspace_id": "11111111-1111-4111-8111-111111111111",
            "at": {"kind": "latest"},
            "query": "revenue",
            "include_archived": false,
            "page": {"limit": 50}
        });
        if let (Some(filter), Some(object)) = (filter, request.as_object_mut()) {
            object.insert("extraction".to_owned(), json!(filter));
        }
        request
    };
    let plain: ListItemsRequest = serde_json::from_value(listing(None))?;
    assert!(plain.extraction.is_none());
    let filtered: ListItemsRequest = serde_json::from_value(listing(Some("unprocessed")))?;
    assert!(filtered.extraction.is_some());
    assert!(serde_json::from_value::<ListItemsRequest>(listing(Some("failed"))).is_err());
    let plain: SearchRequest = serde_json::from_value(search(None))?;
    assert!(plain.extraction.is_none());
    let filtered: SearchRequest = serde_json::from_value(search(Some("unprocessed")))?;
    assert!(filtered.extraction.is_some());
    assert!(serde_json::from_value::<SearchRequest>(search(Some("pending"))).is_err());
    Ok(())
}

#[test]
fn proposal_kind_is_derived_from_changes() -> Result<(), Box<dyn Error>> {
    let changes = |values: Value| -> Result<Vec<Change>, serde_json::Error> {
        serde_json::from_value(values)
    };
    let archive = json!({"kind": "archive", "item_id": ITEM});
    assert_eq!(ProposalKind::of(&[])?, ProposalKind::Content);
    assert_eq!(
        ProposalKind::of(&changes(json!([archive]))?)?,
        ProposalKind::Content
    );
    assert_eq!(
        ProposalKind::of(&changes(json!([supply(ITEM)]))?)?,
        ProposalKind::SupplyExtraction
    );
    assert_eq!(
        ProposalKind::of(&changes(json!([supply(ITEM), supply(OTHER_ITEM)]))?)?,
        ProposalKind::SupplyExtraction
    );
    for refused in [
        json!([supply(ITEM), archive]),
        json!([supply(ITEM), supply(ITEM)]),
    ] {
        let error = ProposalKind::of(&changes(refused.clone())?)
            .err()
            .ok_or("refused proposal")?;
        assert_eq!(error.field.as_deref(), Some("/changes"), "{refused}");
    }
    Ok(())
}

#[test]
fn text_origin_words_are_converter_supplied_by_agent_and_none() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        enum_words::<TextOrigin>()?,
        ["converter", "supplied_by_agent", "none"]
    );
    Ok(())
}

#[test]
fn job_kinds_name_the_operation_that_starts_them() -> Result<(), Box<dyn Error>> {
    let table = operations();
    let returning = job_returning();
    let creators: BTreeSet<&str> = table
        .iter()
        .filter(|operation| {
            operation.success_status == 202
                && operation.id != "retry_job"
                && returning.contains(&operation.id)
        })
        .map(|operation| operation.id)
        .collect();
    let mut workspace = BTreeSet::new();
    let mut tenant = Vec::new();
    for word in JOB_KINDS {
        let kind: JobKind = serde_json::from_value(json!(word))?;
        let started_by = kind.started_by().as_str();
        if kind.is_tenant() {
            tenant.push(started_by);
        } else {
            assert!(
                workspace.insert(started_by),
                "{started_by} starts two kinds"
            );
        }
    }
    // A bijection: every workspace kind has its own creator, and every creator but the
    // installation backup starts a workspace kind.
    let workspace_creators: BTreeSet<&str> = creators
        .iter()
        .copied()
        .filter(|id| *id != "backup_installation")
        .collect();
    assert_eq!(workspace, workspace_creators);
    assert_eq!(
        tenant,
        ["backup_installation", "purge_workspace", "purge_item"]
    );
    assert_eq!(JobKind::Import.started_by(), OperationName::StartImport);
    Ok(())
}

#[test]
fn review_coverage_can_be_invalidated() -> Result<(), Box<dyn Error>> {
    let coverage: ReviewCoverage = serde_json::from_value(json!("invalidated"))?;
    assert_eq!(coverage, ReviewCoverage::Invalidated);
    assert_eq!(
        enum_words::<ReviewCoverage>()?,
        [
            "current",
            "changed",
            "imported",
            "unreviewed",
            "invalidated"
        ]
    );
    Ok(())
}

#[test]
fn timestamps_have_one_canonical_spelling() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        Timestamp::try_from("2026-10-08T14:03:07.250Z".to_owned())?.as_str(),
        "2026-10-08T14:03:07.250Z"
    );
    for refused in [
        "2026-10-08T14:03:07Z",
        "2026-10-08T14:03:07.5Z",
        "2026-10-08T14:03:07.250000Z",
        "2026-10-08T14:03:07.250+00:00",
        "2026-10-08T14:03:07.250z",
        "2026-10-08t14:03:07.250Z",
        "2026-02-30T00:00:00.000Z",
        "2026-10-08T24:00:00.000Z",
        "2026-12-31T23:59:60.000Z",
        "2026-10-08",
        " 2026-10-08T14:03:07.250Z",
    ] {
        assert!(
            Timestamp::try_from(refused.to_owned()).is_err(),
            "{refused:?}"
        );
        assert!(
            serde_json::from_value::<Timestamp>(json!(refused)).is_err(),
            "{refused:?}"
        );
    }
    let schema = schema_of::<Timestamp>()?;
    assert_eq!(schema.get("type"), Some(&json!("string")));
    assert_eq!(schema.get("format"), Some(&json!("date-time")));
    assert_eq!(
        schema.get("pattern"),
        Some(&json!(
            r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-5][0-9]:[0-5][0-9]\.[0-9]{3}Z$"
        ))
    );
    Ok(())
}

#[test]
fn timestamp_order_is_time_order() -> Result<(), Box<dyn Error>> {
    for (earlier, later) in [
        ("2026-10-08T14:03:07.999Z", "2026-10-08T14:03:08.000Z"),
        ("2026-10-08T14:03:59.999Z", "2026-10-08T14:04:00.000Z"),
        ("2026-12-31T23:59:59.999Z", "2027-01-01T00:00:00.000Z"),
        ("2026-10-08T14:03:07.250Z", "2026-10-08T14:03:07.500Z"),
    ] {
        let earlier = Timestamp::try_from(earlier.to_owned())?;
        let later = Timestamp::try_from(later.to_owned())?;
        assert!(earlier < later, "{earlier} sorts before {later}");
        assert!(earlier.instant()? < later.instant()?);
    }
    for (instant, written) in [
        ("2026-10-08T14:03:07.2509Z", "2026-10-08T14:03:07.250Z"),
        ("2026-10-08T14:03:07.9999Z", "2026-10-08T14:03:07.999Z"),
        ("2026-10-08T16:03:07.2509+02:00", "2026-10-08T14:03:07.250Z"),
        ("2026-10-08T14:03:07Z", "2026-10-08T14:03:07.000Z"),
    ] {
        let parsed = OffsetDateTime::parse(instant, &Rfc3339)?;
        assert_eq!(Timestamp::from_utc(parsed)?.as_str(), written, "{instant}");
    }
    Ok(())
}

#[test]
fn every_instant_is_a_timestamp() -> Result<(), Box<dyn Error>> {
    fn visit(label: &str, schema: &Value, found: &mut usize) {
        match schema {
            Value::Object(object) => {
                if let Some(properties) = object.get("properties").and_then(Value::as_object) {
                    for (name, property) in properties {
                        if name.ends_with("_at") {
                            assert!(
                                property.to_string().contains("\"#/$defs/Timestamp\""),
                                "{label}: {name} is not a Timestamp: {property}"
                            );
                            *found = found.saturating_add(1);
                        }
                    }
                }
                for child in object.values() {
                    visit(label, child, found);
                }
            }
            Value::Array(items) => {
                for item in items {
                    visit(label, item, found);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    let mut found = 0_usize;
    for (label, schema) in operation_schemas()? {
        visit(&label, &schema, &mut found);
    }
    assert!(found > 20, "the scan must see the instants, found {found}");
    Ok(())
}

#[test]
fn workspace_paths_refuse_windows_reserved_names() -> Result<(), Box<dyn Error>> {
    for path in [
        "CON",
        "con.txt",
        "docs/LPT1.md",
        "COM9",
        "CONIN$",
        "conout$.log",
        "Aux.tar.gz",
        "nul",
        "notes/COM\u{b9}.md",
    ] {
        assert!(WorkspacePath::try_from(path.to_owned()).is_err(), "{path}");
    }
    for path in ["console.md", "null-results.md", "COM10.md", "notes/lpt.md"] {
        assert_eq!(WorkspacePath::try_from(path.to_owned())?.as_str(), path);
    }
    Ok(())
}

#[test]
fn workspace_paths_refuse_trailing_dots_spaces_and_reserved_characters() {
    for path in [
        "notes.",
        "notes/draft ",
        "a./b.md",
        "a<b.md",
        "a>b.md",
        "say \"hi\".md",
        "a|b.md",
        "why?.md",
        "all*.md",
    ] {
        assert!(WorkspacePath::try_from(path.to_owned()).is_err(), "{path}");
    }
}

#[test]
fn workspace_paths_must_be_nfc() -> Result<(), Box<dyn Error>> {
    let decomposed = "notes/re\u{301}sume\u{301}.md";
    assert!(WorkspacePath::try_from(decomposed.to_owned()).is_err());
    let supplied = WorkspacePath::from_supplied(decomposed)?;
    assert_eq!(supplied.as_str(), "notes/r\u{e9}sum\u{e9}.md");
    assert!(WorkspacePath::from_supplied("notes/con.md").is_err());
    Ok(())
}

#[test]
fn collision_keys_fold_case_and_normalization() -> Result<(), Box<dyn Error>> {
    let upper = WorkspacePath::try_from("Notes/R\u{e9}sum\u{e9}.md".to_owned())?;
    let lower = WorkspacePath::try_from("notes/r\u{e9}sum\u{e9}.md".to_owned())?;
    let decomposed = WorkspacePath::from_supplied("NOTES/RE\u{301}SUME\u{301}.MD")?;
    assert_eq!(upper.collision_key(), lower.collision_key());
    assert_eq!(upper.collision_key(), decomposed.collision_key());
    let first = WorkspacePath::try_from("a/b.md".to_owned())?;
    let second = WorkspacePath::try_from("a/c.md".to_owned())?;
    assert_ne!(first.collision_key(), second.collision_key());
    Ok(())
}

#[test]
fn a_tenant_event_has_no_workspace() -> Result<(), Box<dyn Error>> {
    let signed_in = json!({
        "id": "e-1",
        "kind": "signed_in",
        "at": "2026-10-08T14:03:07.250Z",
        "actor": {"subject": "alice", "route": "browser_session"}
    });
    let event: Event = serde_json::from_value(signed_in.clone())?;
    assert!(event.workspace_id.is_none());
    assert_eq!(event.kind, EventKind::SignedIn);
    assert_eq!(serde_json::to_value(&event)?, signed_in);
    let denied = json!({
        "id": "e-2",
        "workspace_id": "11111111-1111-4111-8111-111111111111",
        "kind": "permission_denied",
        "at": "2026-10-08T14:03:08.000Z",
        "actor": {"subject": "agent", "route": "mcp_delegation", "client_id": "claude"},
        "operation": "backup_workspace"
    });
    let event: Event = serde_json::from_value(denied.clone())?;
    assert_eq!(event.operation, Some(OperationName::BackupWorkspace));
    assert_eq!(serde_json::to_value(&event)?, denied);
    Ok(())
}

#[test]
fn artifact_kind_decides_the_download_rule() {
    let browser_and_owner = [AccessRoute::BrowserSession, AccessRoute::LocalOwner];
    let with_service = [
        AccessRoute::BrowserSession,
        AccessRoute::LocalOwner,
        AccessRoute::Service,
    ];
    for (kind, scope, permission, routes) in [
        (
            ArtifactKind::Export,
            ArtifactScope::Workspace,
            Permission::Read,
            &with_service[..],
        ),
        (
            ArtifactKind::WorkspaceBackup,
            ArtifactScope::Workspace,
            Permission::Admin,
            &browser_and_owner[..],
        ),
        (
            ArtifactKind::InstallationBackup,
            ArtifactScope::Tenant,
            Permission::Admin,
            &browser_and_owner[..],
        ),
        (
            ArtifactKind::ViewExport,
            ArtifactScope::Workspace,
            Permission::Read,
            &with_service[..],
        ),
    ] {
        assert_eq!(kind.scope(), scope, "{kind:?}");
        assert_eq!(kind.download_permission(), permission, "{kind:?}");
        assert_eq!(kind.download_routes(), routes, "{kind:?}");
        assert!(
            !kind.download_routes().contains(&AccessRoute::McpDelegation),
            "{kind:?} never reaches an agent"
        );
    }
}

#[test]
fn a_download_artifact_states_its_kind() -> Result<(), Box<dyn Error>> {
    let mut artifact = json!({
        "artifact_id": "88888888-8888-4888-8888-888888888888",
        "kind": "installation_backup",
        "sha256": "c".repeat(64),
        "size": "2048",
        "download_path": "/api/artifacts/88888888-8888-4888-8888-888888888888"
    });
    let decoded: DownloadArtifact = serde_json::from_value(artifact.clone())?;
    assert_eq!(decoded.kind, ArtifactKind::InstallationBackup);
    assert_eq!(serde_json::to_value(&decoded)?, artifact);
    artifact
        .as_object_mut()
        .ok_or("artifact is an object")?
        .remove("kind");
    assert!(serde_json::from_value::<DownloadArtifact>(artifact).is_err());
    Ok(())
}

#[test]
fn a_tenant_job_has_no_workspace() -> Result<(), Box<dyn Error>> {
    for kind in JOB_KINDS {
        let tenant = TENANT_JOB_KINDS.contains(kind);
        // Present exactly for a workspace kind, absent exactly for a tenant kind.
        let consistent: Job = serde_json::from_value(job(kind, !tenant))?;
        assert!(consistent.check().is_ok(), "{kind}");
        assert_eq!(consistent.workspace_id.is_none(), tenant, "{kind}");
        let inconsistent: Job = serde_json::from_value(job(kind, tenant))?;
        let error = inconsistent.check().err().ok_or("inconsistent job")?;
        assert_eq!(error.field.as_deref(), Some("/workspace_id"), "{kind}");
    }
    let purge: Job = serde_json::from_value(job("purge_workspace", false))?;
    assert!(serde_json::to_value(&purge)?.get("workspace_id").is_none());
    Ok(())
}

#[test]
fn item_status_words_are_okf_status_words() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        enum_words::<ItemStatus>()?,
        ["draft", "stable", "deprecated", "other"]
    );
    Ok(())
}

#[test]
fn the_application_header_key_is_one_constant() {
    assert_eq!(APP_HEADER_KEY, "okf_jawn");
}

okf_jawn_contract::for_each_operation!(operation_schemas);
