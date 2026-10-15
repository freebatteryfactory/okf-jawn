//! A source card's file name is derived from the original name, keeping its extension visible;
//! the rules carry their form labels, and a preview and a read of the rules have their shapes.

use check::{TestResult, err_of, some};
use okf_jawn_contract::conventions::{
    GetRulesResponse, NamingRules, PreviewNamesRequest, fit_segment, numbered_source_card_name,
    source_card_name,
};
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::identity::{MAX_PATH_SEGMENT_UTF16_UNITS, WorkspacePath};
use okf_jawn_contract::scope::RequestScope;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;
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

/// A card's name is one path segment that `WorkspacePath::from_supplied` takes unchanged, of at
/// most `MAX_PATH_SEGMENT_UTF16_UNITS` UTF-16 units.
fn assert_one_segment(name: &str) -> TestResult {
    assert!(
        name.encode_utf16().count() <= MAX_PATH_SEGMENT_UTF16_UNITS,
        "{} units",
        name.encode_utf16().count()
    );
    assert!(!name.contains('/'), "{name}");
    assert_eq!(WorkspacePath::from_supplied(name)?.as_str(), name);
    Ok(())
}

#[test]
fn a_long_original_gets_a_card_name_that_fits_one_segment() -> TestResult {
    // 300 units: the stem is shortened and the tail kept whole.
    let long = format!("{}.pdf", "a".repeat(296));
    let card = source_card_name(&long);
    assert_one_segment(&card)?;
    assert_eq!(card, format!("{}-pdf.md", "a".repeat(248)));
    // A 254-unit name whose own length fits: only the tail it gains overflows.
    let near = format!("{}.pdf", "x".repeat(250));
    assert_eq!(near.encode_utf16().count(), 254);
    let card = source_card_name(&near);
    assert_one_segment(&card)?;
    assert_eq!(card, format!("{}-pdf.md", "x".repeat(248)));
    // A name that fits is unchanged.
    assert_eq!(
        source_card_name(&format!("{}.pdf", "x".repeat(248))),
        format!("{}-pdf.md", "x".repeat(248))
    );
    Ok(())
}

#[test]
fn a_cut_never_splits_a_character_and_never_ends_in_a_dot_or_a_space() -> TestResult {
    // The emoji's two units would be units 248 and 249: the cut falls inside it, so it goes.
    let astral = format!("{}\u{1f600}b.pdf", "a".repeat(247));
    let card = source_card_name(&astral);
    assert_one_segment(&card)?;
    assert_eq!(card, format!("{}-pdf.md", "a".repeat(247)));
    // A space, or a dot, left at the cut is trimmed.
    for left in [" ", "."] {
        let original = format!("{}{left}{}.pdf", "a".repeat(247), "b".repeat(20));
        let card = source_card_name(&original);
        assert_one_segment(&card)?;
        assert_eq!(card, format!("{}-pdf.md", "a".repeat(247)), "{left:?}");
    }
    Ok(())
}

#[test]
fn an_extension_too_long_for_any_stem_makes_the_whole_name_the_stem() -> TestResult {
    let original = format!("a.{}", "e".repeat(260));
    let card = source_card_name(&original);
    assert_one_segment(&card)?;
    assert_eq!(card, format!("a.{}.md", "e".repeat(250)));
    Ok(())
}

#[test]
fn a_name_is_measured_on_its_nfc_form_which_may_be_longer() -> TestResult {
    // U+0958 is written by NFC as two code points (U+0915 U+093C): 200 input units, 400 in NFC.
    let original = format!("{}.pdf", "\u{958}".repeat(200));
    assert_eq!(
        "\u{958}".nfc().collect::<String>(),
        "\u{915}\u{93c}",
        "NFC decomposes U+0958"
    );
    let card = source_card_name(&original);
    assert_one_segment(&card)?;
    assert!(card.ends_with("-pdf.md"), "{card}");
    Ok(())
}

#[test]
fn a_fitted_segment_keeps_its_tail_whole() -> TestResult {
    let stem = "s".repeat(300);
    for number in [2_u32, 10, 4_000_000_000] {
        let tail = format!("-{number}.md");
        let name = fit_segment(&stem, &tail);
        assert_one_segment(&name)?;
        assert!(name.ends_with(&tail), "{name}");
        assert_eq!(name.encode_utf16().count(), MAX_PATH_SEGMENT_UTF16_UNITS);
    }
    assert_eq!(fit_segment("report-pdf", "-2.md"), "report-pdf-2.md");
    Ok(())
}

/// Review N6: the extension's tail is kept while it fits beside one stem character, exactly up
/// to one segment, and one unit more makes the whole name the stem.
#[test]
fn an_extension_tail_is_kept_while_it_fits_beside_one_stem_character() -> TestResult {
    // One stem character and `-<250 e>.md` are 255 units: the tail is kept.
    let fits = format!("q.{}", "e".repeat(250));
    let card = source_card_name(&fits);
    assert_one_segment(&card)?;
    assert_eq!(card, format!("q-{}.md", "e".repeat(250)));
    assert_eq!(card.encode_utf16().count(), MAX_PATH_SEGMENT_UTF16_UNITS);
    // One more unit of extension: the whole name is the stem and `.md` the tail.
    let over = format!("q.{}", "e".repeat(251));
    let card = source_card_name(&over);
    assert_one_segment(&card)?;
    assert_eq!(card, format!("q.{}.md", "e".repeat(250)));
    Ok(())
}

/// Review N8: a numbered card keeps the source-card tail `-<ext>-<n>` whole, so a long name
/// stays `…-pdf-2.md`, never `…-p-2.md`; a name with no extension keeps `-<n>.md`.
#[test]
fn a_numbered_card_keeps_its_extension_marker_and_number_whole() -> TestResult {
    let long = format!("{}.pdf", "q".repeat(296));
    assert_eq!(long.chars().count(), 300);
    assert_eq!(
        numbered_source_card_name(&long, 1),
        format!("{}-pdf.md", "q".repeat(248))
    );
    let second = numbered_source_card_name(&long, 2);
    assert_one_segment(&second)?;
    assert_eq!(second, format!("{}-pdf-2.md", "q".repeat(246)));
    let bare = "q".repeat(300);
    assert_eq!(
        numbered_source_card_name(&bare, 1),
        format!("{}.md", "q".repeat(252))
    );
    let second = numbered_source_card_name(&bare, 2);
    assert_one_segment(&second)?;
    assert_eq!(second, format!("{}-2.md", "q".repeat(250)));
    for (original, number, card) in [
        ("report.pdf", 2, "report-pdf-2.md"),
        ("report.pdf", 10, "report-pdf-10.md"),
        ("Q3 Results.XLSX", 3, "Q3 Results-XLSX-3.md"),
        ("README", 2, "README-2.md"),
        (".env", 2, ".env-2.md"),
        ("report.pdf", 0, "report-pdf.md"),
    ] {
        assert_eq!(
            numbered_source_card_name(original, number),
            card,
            "{original} {number}"
        );
    }
    // An extension too long to keep beside a numbered tail: the whole name is the stem.
    let wide = format!("q.{}", "e".repeat(250));
    let card = numbered_source_card_name(&wide, 2);
    assert_one_segment(&card)?;
    assert_eq!(card, format!("q.{}-2.md", "e".repeat(248)));
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
