//! A source card's file name is derived from the original name, keeping its extension visible;
//! the rules carry their form labels, and a preview and a read of the rules have their shapes.

use check::{TestResult, err_of, some};
use okf_jawn_contract::conventions::{
    GetRulesResponse, NamingRules, PreviewNamesRequest, source_card_name,
};
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::scope::RequestScope;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde_json::{Value, json};
#[test]
fn source_card_name_joins_the_extension() {
    for (original, card) in [
        ("report.pdf", "report-pdf.md"),
        ("v1.2.notes.txt", "v1.2.notes-txt.md"),
        ("README", "README.md"),
        (".env", ".env.md"),
        ("Q3 Results.XLSX", "Q3 Results-XLSX.md"),
    ] {
        assert_eq!(source_card_name(original), card, "{original}");
    }
}

fn draft07<T: JsonSchema>() -> Result<Value, serde_json::Error> {
    serde_json::to_value(
        SchemaSettings::draft07()
            .into_generator()
            .into_root_schema_for::<T>(),
    )
}

/// Every field of a naming rule and of the rules, and every word of the rule's enums, has a
/// title in the form schema: the label the schema-driven form shows.
#[test]
fn every_rule_field_and_choice_has_a_title_in_the_form_schema() -> TestResult {
    let form = draft07::<NamingRules>()?;
    let definitions = some(form.get("definitions"), "the form's definitions")?;
    let mut fields = Vec::new();
    for (owner, properties) in [
        (
            "NamingRules",
            some(form.get("properties"), "the rules' properties")?,
        ),
        (
            "NamingRule",
            some(
                definitions.pointer("/NamingRule/properties"),
                "a rule's properties",
            )?,
        ),
    ] {
        for (name, property) in some(properties.as_object(), "properties")? {
            let title = property.get("title").and_then(Value::as_str);
            assert!(
                title.is_some_and(|title| !title.is_empty()),
                "{owner}.{name} has no title"
            );
            fields.push(format!("{owner}.{name}"));
        }
    }
    assert_eq!(
        fields.len(),
        15,
        "two fields of the rules, thirteen of a rule: {fields:?}"
    );
    let mut words = 0_usize;
    for choice in [
        "LetterCase",
        "DateOrder",
        "CollisionPolicy",
        "ExtensionPolicy",
    ] {
        let variants = some(
            definitions
                .pointer(&format!("/{choice}/oneOf"))
                .and_then(Value::as_array),
            "the choice's words",
        )?;
        for variant in variants {
            let word = variant.get("const").and_then(Value::as_str);
            let title = variant.get("title").and_then(Value::as_str);
            assert!(
                title.is_some_and(|title| !title.is_empty()),
                "{choice} {word:?} has no title"
            );
            words = words.saturating_add(1);
        }
    }
    assert_eq!(
        words, 16,
        "6 cases, 4 date orders, 3 collision and 3 extension policies"
    );
    assert_eq!(
        definitions.pointer("/NamingRule/properties/case/title"),
        Some(&json!("Letter case"))
    );
    assert_eq!(
        definitions.pointer("/NamingRule/properties/collision/title"),
        Some(&json!("When names collide"))
    );
    Ok(())
}

#[test]
fn rules_are_answered_with_their_revision_or_as_an_unreadable_file() -> TestResult {
    let none = json!({"revision": "a".repeat(40)});
    let answer: GetRulesResponse = serde_json::from_value(none.clone())?;
    assert!(answer.rules.is_none() && answer.unreadable.is_none());
    assert_eq!(serde_json::to_value(&answer)?, none, "no rules saved");
    let unreadable = json!({
        "revision": "a".repeat(40),
        "unreadable": {
            "text": "schema_version: 9\nrules: []\n",
            "problems": [{"code": "unknown_schema_version", "message": "version 9"}]
        }
    });
    let answer: GetRulesResponse = serde_json::from_value(unreadable.clone())?;
    let file = some(answer.unreadable.as_ref(), "the unreadable file")?;
    assert_eq!(file.problems.len(), 1);
    assert_eq!(serde_json::to_value(&answer)?, unreadable);
    Ok(())
}

#[test]
fn a_preview_selects_items_a_folder_or_the_workspace_and_names_at_least_one_item() -> TestResult {
    let request = |selection: Value| {
        json!({
            "workspace_id": "11111111-1111-4111-8111-111111111111",
            "at": {"kind": "latest"},
            "selection": selection,
            "rules": {"schema_version": 1, "rules": []}
        })
    };
    for selection in [
        json!({"kind": "items", "item_ids": ["22222222-2222-4222-8222-222222222222"]}),
        json!({"kind": "folder", "folder": "Clients/2026", "recursive": true}),
        json!({"kind": "workspace"}),
    ] {
        let decoded: PreviewNamesRequest = serde_json::from_value(request(selection.clone()))?;
        assert!(decoded.check_rules().is_ok(), "{selection}");
    }
    let empty: PreviewNamesRequest =
        serde_json::from_value(request(json!({"kind": "items", "item_ids": []})))?;
    let refused = err_of(empty.check_rules())?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(refused.field.as_deref(), Some("/selection/item_ids"));
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
