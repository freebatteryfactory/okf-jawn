//! Shared View fixtures must satisfy core's authored View rules, not only wire schemas.
//! The six-component fixture declares its cited bindings; materialized data belongs only to
//! a presentation's resolved bindings (SPEC section 10 and core M1.6 validation rules).

use okf_jawn_contract::identity::{Digest, WorkspaceId};
use okf_jawn_contract::views::{PresentResponse, ViewDocument};
use okf_jawn_core::views::validate::validate;
use uuid::Uuid;

use check::{TestResult, some};

fn workspace() -> Result<WorkspaceId, uuid::Error> {
    Ok(WorkspaceId(Uuid::parse_str(
        "aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff",
    )?))
}

#[test]
fn qualification_fixture_six_component_view_declares_its_sources_and_chart() -> TestResult {
    let view: ViewDocument = serde_json::from_str(include_str!(
        "../../../tests/fixtures/views/view-document-six-component.json"
    ))?;
    let checked = validate(&view, workspace()?)?;
    checked.require_drawable()?;
    assert_eq!(
        view.bindings
            .iter()
            .map(|binding| binding.name.as_str())
            .collect::<Vec<_>>(),
        ["venue", "metrics"]
    );
    assert_eq!(
        checked.needed().into_iter().collect::<Vec<_>>(),
        ["metrics"]
    );
    Ok(())
}

#[test]
fn qualification_fixture_present_keeps_materialized_data_only_in_resolved_bindings() -> TestResult {
    let present: PresentResponse = serde_json::from_str(include_str!(
        "../../../tests/fixtures/views/present-response.json"
    ))?;
    validate(&present.view, workspace()?)?.require_drawable()?;
    assert!(
        present
            .view
            .bindings
            .iter()
            .all(|binding| binding.materialized.is_none())
    );
    let metrics = some(
        present
            .resolved_bindings
            .iter()
            .find(|binding| binding.name == "metrics"),
        "the resolved metrics binding",
    )?;
    assert_eq!(
        metrics.materialized.as_ref().map(Digest::as_str),
        Some("aab513246005fcf5431e5452e4afe5b2620a41024b9c230fa50b10c3c655e74c")
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
