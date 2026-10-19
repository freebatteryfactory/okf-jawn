//! The naming rules engine (`okf_jawn_core::conventions`): every test vector of the design
//! (core-m1-3b-design.md sections 4 to 6 and 8), one test each, named
//! `conventions_<group>_<vector>_...` so `cargo test -p okf-jawn-core -- conventions` selects
//! them. Each vector starts from the design's baseline rule R0 (`r0`): every path, kebab case
//! joined by `-`, no prefix or suffix, `ymd`, dates kept as written, no version stripping,
//! extensions kept, no aliases, collisions numbered.

use std::collections::BTreeSet;

use okf_jawn_contract::common::Warning;
use okf_jawn_contract::conventions::{
    CollisionPolicy, DateOrder, ExtensionPolicy, LetterCase, NamingRule, NamingRules,
};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{ItemId, Revision, WorkspacePath};
use okf_jawn_contract::item::{ItemKind, ItemStatus, ItemSummary};
use okf_jawn_core::conventions::{
    COLLISION_KEPT, COLLISION_REJECTED, COLLISION_SUFFIXED, CompiledRules, DATE_AMBIGUOUS,
    DATE_NOT_READ, DESTINATION_NEEDS_DATE, ITEM_NOT_MOVABLE, NAME_LEFT_EMPTY, PATH_NOT_VALID,
    PATH_RESERVED, PlanBody, Proposal, RULES_INVALID, RULES_SCHEMA_VERSION_UNKNOWN,
    RULES_TOO_LARGE, Subject, name_new_item, numbered, plan, propose,
};
use okf_jawn_core::mutations::request_digest;
use okf_jawn_core::storage::TreeItem;
use uuid::Uuid;

use check::{TestResult, err_of, some};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// The design's `DF`.
const DF: &str = "[year]-[month]-[day]";

/// The design's baseline rule R0.
fn r0() -> NamingRule {
    NamingRule {
        glob: "**".to_owned(),
        type_name: None,
        case: LetterCase::Kebab,
        separator: "-".to_owned(),
        prefix: String::new(),
        suffix: None,
        date_order: DateOrder::Ymd,
        date_format: String::new(),
        strip_version_suffix: false,
        extension_policy: ExtensionPolicy::Preserve,
        alias_preservation: false,
        destination: None,
        collision: CollisionPolicy::Suffix,
    }
}

/// R0 with `change` applied.
fn with(change: impl FnOnce(&mut NamingRule)) -> NamingRule {
    let mut rule = r0();
    change(&mut rule);
    rule
}

fn rule_set(rules: Vec<NamingRule>) -> NamingRules {
    NamingRules {
        schema_version: 1,
        rules,
    }
}

fn compiled(rules: Vec<NamingRule>) -> Result<CompiledRules, ApiError> {
    CompiledRules::compile(&rule_set(rules), "/rules")
}

fn path(text: &str) -> Fallible<WorkspacePath> {
    Ok(WorkspacePath::try_from(text.to_owned())?)
}

/// What `rules` make of a note at `at`.
fn named(rules: Vec<NamingRule>, at: &str) -> Fallible<Proposal> {
    named_as(rules, at, "note", None)
}

/// What `rules` make of an item at `at` of `type_name`, a source card of `original` when given.
fn named_as(
    rules: Vec<NamingRule>,
    at: &str,
    type_name: &str,
    original: Option<&str>,
) -> Fallible<Proposal> {
    let rules = compiled(rules)?;
    let at = path(at)?;
    Ok(propose(
        &rules,
        &Subject {
            path: &at,
            type_name,
            original_name: original,
        },
    ))
}

/// The new path of a renamed proposal, with its warnings' codes.
fn renamed(proposal: &Proposal) -> Fallible<(String, Vec<String>)> {
    match proposal {
        Proposal::Renamed { path, warnings, .. } => Ok((path.as_str().to_owned(), codes(warnings))),
        other => Err(format!("not renamed: {other:?}").into()),
    }
}

/// Assert `rule` renames a note at `at` to `to`, with no warning.
fn renames(rule: NamingRule, at: &str, to: &str) -> TestResult {
    let (got, warnings) = renamed(&named(vec![rule], at)?)?;
    assert_eq!(got, to, "{at}");
    assert_eq!(warnings, Vec::<String>::new(), "{at}");
    Ok(())
}

/// Assert `rule` leaves a note at `at` as it is, with no warning.
fn keeps(rule: NamingRule, at: &str) -> TestResult {
    match named(vec![rule], at)? {
        Proposal::Unchanged { warnings } if warnings.is_empty() => Ok(()),
        other => Err(format!("{at} is not unchanged: {other:?}").into()),
    }
}

/// The codes of a refused proposal's warnings.
fn refused_codes(proposal: &Proposal) -> Fallible<Vec<String>> {
    match proposal {
        Proposal::Refused { warnings } => Ok(codes(warnings)),
        other => Err(format!("not refused: {other:?}").into()),
    }
}

fn codes(warnings: &[Warning]) -> Vec<String> {
    warnings
        .iter()
        .map(|warning| warning.code.clone())
        .collect()
}

/// The refusal of rules with `rule` alone: its code, field and message.
fn refusal(rule: NamingRule) -> Fallible<(ErrorCode, String, String)> {
    let error = err_of(compiled(vec![rule]))?;
    Ok((error.code, error.field.unwrap_or_default(), error.message))
}

/// Assert `rule` is refused as `InvalidInput` on `field` of rule 0, its message holding `gist`.
fn refuses(rule: NamingRule, field: &str, gist: &str) -> TestResult {
    let (code, at, message) = refusal(rule)?;
    assert_eq!(code, ErrorCode::InvalidInput, "{message}");
    assert_eq!(at, format!("/rules/rules/0/{field}"), "{message}");
    assert!(message.contains(gist), "{message:?} does not say {gist:?}");
    Ok(())
}

fn revision() -> Fallible<Revision> {
    Ok(Revision::try_from("a".repeat(40))?)
}

fn id(number: u128) -> ItemId {
    ItemId(Uuid::from_u128(number))
}

/// A tree item at `at`.
fn item(number: u128, at: &str) -> Fallible<TreeItem> {
    item_of(number, at, "note", None)
}

fn item_of(number: u128, at: &str, type_name: &str, original: Option<&str>) -> Fallible<TreeItem> {
    Ok(TreeItem {
        summary: ItemSummary {
            id: id(number),
            path: path(at)?,
            title: String::new(),
            description: String::new(),
            type_name: type_name.to_owned(),
            kind: if original.is_some() {
                ItemKind::Source
            } else {
                ItemKind::Note
            },
            revision: revision()?,
            status: ItemStatus::Stable,
            archived: false,
            media_type: None,
            extraction: None,
        },
        original_name: original.map(str::to_owned),
    })
}

/// The folders of `items`' paths.
fn folders_of(items: &[TreeItem]) -> Fallible<Vec<WorkspacePath>> {
    let mut folders = BTreeSet::new();
    for item in items {
        let text = item.summary.path.as_str();
        for (at, _) in text.match_indices('/') {
            folders.insert(some(text.get(..at), "a folder")?.to_owned());
        }
    }
    folders.into_iter().map(|folder| path(&folder)).collect()
}

/// The plan of every item of `items` under `rules`.
fn plan_all(rules: Vec<NamingRule>, items: &[TreeItem]) -> Fallible<PlanBody> {
    let selected = items.iter().map(|item| item.summary.id).collect();
    plan_of(rules, items, &selected)
}

fn plan_of(
    rules: Vec<NamingRule>,
    items: &[TreeItem],
    selected: &BTreeSet<ItemId>,
) -> Fallible<PlanBody> {
    Ok(plan(
        &compiled(rules)?,
        items,
        &folders_of(items)?,
        selected,
    )?)
}

/// The plan's entries as (item, to).
fn moves(body: &PlanBody) -> Vec<(ItemId, String)> {
    body.entries
        .iter()
        .map(|planned| (planned.entry.item_id, planned.entry.to.as_str().to_owned()))
        .collect()
}

/// The plan's warnings as (location, code).
fn plan_warnings(body: &PlanBody) -> Vec<(String, String)> {
    body.warnings
        .iter()
        .map(|warning| {
            (
                warning.location.clone().unwrap_or_default(),
                warning.code.clone(),
            )
        })
        .collect()
}

/// `considered` is entries plus `unchanged` plus `refused` (contract `RenamePlan.refused`).
fn counts_add_up(body: &PlanBody) -> TestResult {
    let entries = u32::try_from(body.entries.len())?;
    let sum = entries
        .checked_add(body.unchanged)
        .and_then(|sum| sum.checked_add(body.refused));
    assert_eq!(
        Some(body.considered),
        sum,
        "considered {} = entries {entries} + unchanged {} + refused {}",
        body.considered,
        body.unchanged,
        body.refused
    );
    Ok(())
}

// Which items a rule applies to: `glob` (design 4.1).

#[test]
fn conventions_glob_g1_star_matches_a_name_in_its_folder() -> TestResult {
    renames(
        with(|rule| "notes/*.md".clone_into(&mut rule.glob)),
        "notes/Meeting Notes.md",
        "notes/meeting-notes.md",
    )
}

#[test]
fn conventions_glob_g2_star_does_not_cross_a_slash() -> TestResult {
    let proposal = named(
        vec![with(|rule| "notes/*.md".clone_into(&mut rule.glob))],
        "notes/2025/Plan A.md",
    )?;
    assert!(matches!(proposal, Proposal::Unmatched), "{proposal:?}");
    Ok(())
}

#[test]
fn conventions_glob_g3_matches_ignoring_case_and_never_renames_folders() -> TestResult {
    renames(
        with(|rule| "NOTES/**".clone_into(&mut rule.glob)),
        "notes/Sub/Plan A.md",
        "notes/Sub/plan-a.md",
    )
}

#[test]
fn conventions_glob_g4_star_alone_names_root_items_only() -> TestResult {
    let rule = || with(|rule| "*.md".clone_into(&mut rule.glob));
    let proposal = named(vec![rule()], "notes/Plan A.md")?;
    assert!(matches!(proposal, Proposal::Unmatched), "{proposal:?}");
    renames(rule(), "Plan A.md", "plan-a.md")
}

#[test]
fn conventions_glob_g5_a_negated_class_excludes_its_letters() -> TestResult {
    let rule = || with(|rule| "notes/[!a-c]*.md".clone_into(&mut rule.glob));
    renames(rule(), "notes/Draft X.md", "notes/draft-x.md")?;
    let proposal = named(vec![rule()], "notes/Alpha.md")?;
    assert!(matches!(proposal, Proposal::Unmatched), "{proposal:?}");
    Ok(())
}

#[test]
fn conventions_glob_g6_matches_non_ascii_letters_ignoring_case() -> TestResult {
    renames(
        with(|rule| "**/Über*.md".clone_into(&mut rule.glob)),
        "Archiv/über uns.md",
        "Archiv/über-uns.md",
    )
}

#[test]
fn conventions_glob_g7_question_mark_is_one_character() -> TestResult {
    renames(
        with(|rule| "?.md".clone_into(&mut rule.glob)),
        "\u{c9}.md",
        "\u{e9}.md",
    )
}

#[test]
fn conventions_glob_g8_braces_are_literal() -> TestResult {
    let rule = || with(|rule| "notes/{a,b}.md".clone_into(&mut rule.glob));
    keeps(rule(), "notes/{a,b}.md")?;
    let proposal = named(vec![rule()], "notes/a.md")?;
    assert!(matches!(proposal, Proposal::Unmatched), "{proposal:?}");
    Ok(())
}

#[test]
fn conventions_glob_g9_an_empty_glob_is_refused() -> TestResult {
    refuses(
        with(|rule| rule.glob = String::new()),
        "glob",
        "must not be empty",
    )
}

#[test]
fn conventions_glob_g10_a_leading_slash_is_refused() -> TestResult {
    refuses(
        with(|rule| "/notes/*.md".clone_into(&mut rule.glob)),
        "glob",
        "leave out the leading /",
    )
}

#[test]
fn conventions_glob_g11_a_trailing_slash_is_refused_naming_the_glob_to_write() -> TestResult {
    refuses(
        with(|rule| "notes/".clone_into(&mut rule.glob)),
        "glob",
        "write notes/** for everything under notes",
    )
}

#[test]
fn conventions_glob_g12_a_recursive_wildcard_inside_a_component_is_refused() -> TestResult {
    refuses(
        with(|rule| "notes/**x".clone_into(&mut rule.glob)),
        "glob",
        "recursive wildcards must form a single path component (at ",
    )
}

#[test]
fn conventions_glob_g13_an_unclosed_class_is_refused() -> TestResult {
    refuses(
        with(|rule| "notes/[ab".clone_into(&mut rule.glob)),
        "glob",
        "invalid range pattern (at ",
    )
}

#[test]
fn conventions_glob_g14_a_backslash_is_refused() -> TestResult {
    refuses(
        with(|rule| "notes\\*.md".clone_into(&mut rule.glob)),
        "glob",
        "never \\",
    )
}

#[test]
fn conventions_glob_over_1024_bytes_is_too_large() -> TestResult {
    let (code, field, _) = refusal(with(|rule| rule.glob = "a".repeat(1025)))?;
    assert_eq!(
        (code, field.as_str()),
        (ErrorCode::TooLarge, "/rules/rules/0/glob")
    );
    let at_limit = compiled(vec![with(|rule| rule.glob = "a".repeat(1024))]);
    assert!(at_limit.is_ok());
    Ok(())
}

// `type_name`.

#[test]
fn conventions_type_t1_a_type_names_its_items() -> TestResult {
    let (to, _) = renamed(&named_as(
        vec![with(|rule| rule.type_name = Some("meeting".to_owned()))],
        "Weekly Sync.md",
        "meeting",
        None,
    )?)?;
    assert_eq!(to, "weekly-sync.md");
    Ok(())
}

#[test]
fn conventions_type_t2_a_type_is_compared_exactly() -> TestResult {
    let proposal = named_as(
        vec![with(|rule| rule.type_name = Some("meeting".to_owned()))],
        "Weekly Sync.md",
        "Meeting",
        None,
    )?;
    assert!(matches!(proposal, Proposal::Unmatched), "{proposal:?}");
    Ok(())
}

#[test]
fn conventions_type_t3_no_type_names_every_type() -> TestResult {
    let (to, _) = renamed(&named_as(vec![r0()], "Weekly Sync.md", "anything", None)?)?;
    assert_eq!(to, "weekly-sync.md");
    Ok(())
}

#[test]
fn conventions_type_t4_a_source_card_is_matched_by_its_source_type() -> TestResult {
    let (to, _) = renamed(&named_as(
        vec![with(|rule| rule.type_name = Some("source".to_owned()))],
        "Report-pdf.md",
        "source",
        Some("Report.pdf"),
    )?)?;
    assert_eq!(to, "report-pdf.md");
    Ok(())
}

#[test]
fn conventions_type_t5_an_empty_type_is_refused() -> TestResult {
    refuses(
        with(|rule| rule.type_name = Some(String::new())),
        "type_name",
        "leave type_name out to match every type",
    )
}

// List order: the first matching rule names an item.

#[test]
fn conventions_order_o1_the_first_matching_rule_names_each_item() -> TestResult {
    let rules = || {
        vec![
            with(|rule| {
                "notes/**".clone_into(&mut rule.glob);
                rule.case = LetterCase::Upper;
                "_".clone_into(&mut rule.separator);
            }),
            r0(),
        ]
    };
    assert_eq!(
        renamed(&named(rules(), "notes/Plan A.md")?)?.0,
        "notes/PLAN_A.md"
    );
    assert_eq!(renamed(&named(rules(), "Plan A.md")?)?.0, "plan-a.md");
    Ok(())
}

#[test]
fn conventions_order_o2_a_rule_whose_type_does_not_match_is_passed_over() -> TestResult {
    let rules = vec![
        with(|rule| {
            rule.type_name = Some("meeting".to_owned());
            rule.case = LetterCase::Upper;
            "_".clone_into(&mut rule.separator);
        }),
        r0(),
    ];
    assert_eq!(renamed(&named(rules, "Plan A.md")?)?.0, "plan-a.md");
    Ok(())
}

#[test]
fn conventions_order_o3_a_rule_that_keeps_a_name_stops_the_search() -> TestResult {
    let rules = vec![
        with(|rule| {
            "keep/**".clone_into(&mut rule.glob);
            rule.case = LetterCase::Preserve;
            " ".clone_into(&mut rule.separator);
        }),
        r0(),
    ];
    let proposal = named(rules, "keep/My File.md")?;
    assert!(
        matches!(proposal, Proposal::Unchanged { ref warnings } if warnings.is_empty()),
        "{proposal:?}"
    );
    Ok(())
}

#[test]
fn conventions_order_o4_no_rules_match_nothing() -> TestResult {
    let proposal = named(Vec::new(), "Plan A.md")?;
    assert!(matches!(proposal, Proposal::Unmatched), "{proposal:?}");
    let body = plan_all(Vec::new(), &[item(1, "Plan A.md")?])?;
    assert_eq!((body.entries.len(), body.unchanged), (0, 1));
    Ok(())
}

// `strip_version_suffix` (design 4.3).

fn stripping() -> NamingRule {
    with(|rule| rule.strip_version_suffix = true)
}

#[test]
fn conventions_version_v1_a_version_number_comes_off() -> TestResult {
    renames(stripping(), "Budget_v2.md", "budget.md")
}

#[test]
fn conventions_version_v2_a_windows_copy_comes_off_in_two_steps() -> TestResult {
    renames(stripping(), "Budget - Copy (2).md", "budget.md")
}

#[test]
fn conventions_version_v3_several_suffixes_come_off() -> TestResult {
    renames(stripping(), "Budget final v3.md", "budget.md")?;
    renames(stripping(), "report_v2_final (1).md", "report.md")?;
    renames(stripping(), "Plan V3.1.md", "plan.md")?;
    renames(stripping(), "Plan copy 2.md", "plan.md")
}

#[test]
fn conventions_version_v4_a_version_with_no_break_before_it_stays() -> TestResult {
    renames(stripping(), "Budgetv2.md", "budgetv2.md")
}

#[test]
fn conventions_version_v5_a_suffix_that_would_leave_nothing_stays() -> TestResult {
    keeps(stripping(), "final.md")?;
    keeps(stripping(), "(1).md")
}

#[test]
fn conventions_version_v6_nothing_comes_off_unless_the_rule_says_so() -> TestResult {
    renames(r0(), "Budget_v2.md", "budget-v2.md")
}

#[test]
fn conventions_version_v7_a_cards_version_comes_off_before_its_tail() -> TestResult {
    let (to, _) = renamed(&named_as(
        vec![stripping()],
        "Budget (1)-pdf.md",
        "source",
        Some("Budget (1).pdf"),
    )?)?;
    assert_eq!(to, "budget-pdf.md");
    Ok(())
}

#[test]
fn conventions_version_v8_copy_as_a_word_comes_off_too() -> TestResult {
    renames(stripping(), "Ad copy.md", "ad.md")
}

#[test]
fn conventions_version_words_that_carry_meaning_stay() -> TestResult {
    renames(stripping(), "Plan draft.md", "plan-draft.md")?;
    renames(stripping(), "Plan old.md", "plan-old.md")?;
    renames(stripping(), "Plan rev2.md", "plan-rev2.md")?;
    keeps(stripping(), "plan-2.md")
}

// Dates: `date_order` (design 4.4).

fn dated(order: DateOrder) -> NamingRule {
    with(|rule| {
        rule.date_order = order;
        DF.clone_into(&mut rule.date_format);
    })
}

#[test]
fn conventions_date_d1_a_year_first_date_is_rewritten() -> TestResult {
    renames(
        dated(DateOrder::Ymd),
        "Minutes 2025.3.4.md",
        "minutes-2025-03-04.md",
    )
}

#[test]
fn conventions_date_d2_a_year_last_date_reads_by_the_declared_order() -> TestResult {
    renames(
        dated(DateOrder::Mdy),
        "Minutes 03-04-2025.md",
        "minutes-2025-03-04.md",
    )?;
    renames(
        dated(DateOrder::Dmy),
        "Minutes 03-04-2025.md",
        "minutes-2025-04-03.md",
    )
}

#[test]
fn conventions_date_d3_an_ambiguous_date_refuses_the_name_never_guessed() -> TestResult {
    let proposal = named(
        vec![dated(DateOrder::RejectAmbiguous)],
        "Minutes 03-04-2025.md",
    )?;
    assert_eq!(refused_codes(&proposal)?, [DATE_AMBIGUOUS]);
    let body = plan_all(
        vec![dated(DateOrder::RejectAmbiguous)],
        &[item(1, "Minutes 03-04-2025.md")?],
    )?;
    assert_eq!(body.entries.len(), 0);
    assert_eq!(body.refused, 1);
    assert_eq!(
        plan_warnings(&body),
        [(
            "Minutes 03-04-2025.md".to_owned(),
            DATE_AMBIGUOUS.to_owned()
        )]
    );
    counts_add_up(&body)
}

#[test]
fn conventions_date_d4_a_date_only_one_order_reads_is_read() -> TestResult {
    renames(
        dated(DateOrder::RejectAmbiguous),
        "Minutes 13-04-2025.md",
        "minutes-2025-04-13.md",
    )?;
    renames(
        dated(DateOrder::RejectAmbiguous),
        "Minutes 04-13-2025.md",
        "minutes-2025-04-13.md",
    )
}

#[test]
fn conventions_date_d5_both_readings_one_date_is_read() -> TestResult {
    renames(
        dated(DateOrder::RejectAmbiguous),
        "Minutes 04-04-2025.md",
        "minutes-2025-04-04.md",
    )
}

#[test]
fn conventions_date_d6_ymd_reads_no_year_last_date() -> TestResult {
    let (to, warnings) = renamed(&named(
        vec![dated(DateOrder::Ymd)],
        "Minutes 03-04-2025.md",
    )?)?;
    assert_eq!(to, "minutes-03-04-2025.md");
    assert_eq!(warnings, [DATE_NOT_READ]);
    Ok(())
}

#[test]
fn conventions_date_d7_a_reading_that_is_no_date_is_kept_with_a_warning() -> TestResult {
    let (to, warnings) = renamed(&named(
        vec![dated(DateOrder::Mdy)],
        "Minutes 13-04-2025.md",
    )?)?;
    assert_eq!(to, "minutes-13-04-2025.md");
    assert_eq!(warnings, [DATE_NOT_READ]);
    Ok(())
}

#[test]
fn conventions_date_d8_a_compact_date_is_one_protected_word() -> TestResult {
    renames(
        dated(DateOrder::Ymd),
        "IMG_20250304_1234.md",
        "img-2025-03-04-1234.md",
    )
}

#[test]
fn conventions_date_d9_a_year_first_token_that_is_no_date_is_kept() -> TestResult {
    let (to, warnings) = renamed(&named(vec![dated(DateOrder::Ymd)], "scan 2025-02-30.md")?)?;
    assert_eq!(to, "scan-2025-02-30.md");
    assert_eq!(warnings, [DATE_NOT_READ]);
    Ok(())
}

#[test]
fn conventions_date_d10_a_two_digit_year_is_not_a_date() -> TestResult {
    renames(dated(DateOrder::Ymd), "Q3 03-04-25.md", "q3-03-04-25.md")
}

#[test]
fn conventions_date_d11_with_no_format_nothing_is_read_or_flagged() -> TestResult {
    renames(
        with(|rule| rule.date_order = DateOrder::RejectAmbiguous),
        "Minutes 03-04-2025.md",
        "minutes-03-04-2025.md",
    )
}

#[test]
fn conventions_date_two_dates_one_character_apart_are_both_found() -> TestResult {
    renames(
        dated(DateOrder::Ymd),
        "From 2025.3.4_2025.3.5.md",
        "from-2025-03-04-2025-03-05.md",
    )
}

#[test]
fn conventions_date_mixed_separators_and_invalid_compact_dates_are_text() -> TestResult {
    renames(
        dated(DateOrder::Ymd),
        "Run 2025-03_04.md",
        "run-2025-03-04.md",
    )?;
    renames(
        dated(DateOrder::Ymd),
        "Ticket 20251304.md",
        "ticket-20251304.md",
    )
}

// `date_format`.

fn formatted(format: &str) -> NamingRule {
    with(|rule| format.clone_into(&mut rule.date_format))
}

#[test]
fn conventions_format_f1_an_empty_format_keeps_dates_as_written() -> TestResult {
    renames(r0(), "Minutes 2025.3.4.md", "minutes-2025.3.4.md")
}

#[test]
fn conventions_format_f2_a_day_first_format_under_dmy_is_stable() -> TestResult {
    let rule = || {
        with(|rule| {
            rule.date_order = DateOrder::Dmy;
            "[day].[month].[year]".clone_into(&mut rule.date_format);
        })
    };
    renames(rule(), "Minutes 2025-03-04.md", "minutes-04.03.2025.md")?;
    keeps(rule(), "minutes-04.03.2025.md")
}

#[test]
fn conventions_format_f3_a_compact_format_is_stable() -> TestResult {
    let rule = || formatted("[year][month][day]");
    renames(
        rule(),
        "Report 2025-3-4 final.md",
        "report-20250304-final.md",
    )?;
    keeps(rule(), "report-20250304-final.md")
}

#[test]
fn conventions_format_f4_a_month_name_is_refused() -> TestResult {
    refuses(
        formatted("[month repr:short] [year]"),
        "date_format",
        "a date format writes only numbers: [year], [month], [day]",
    )
}

#[test]
fn conventions_format_f5_a_format_that_does_not_parse_is_refused() -> TestResult {
    refuses(
        formatted("[year]-[month"),
        "date_format",
        "not a date format: ",
    )
}

#[test]
fn conventions_format_f6_a_part_a_date_does_not_have_is_refused() -> TestResult {
    refuses(
        formatted("[hour]"),
        "date_format",
        "names a part a date does not have",
    )
}

#[test]
fn conventions_format_f7_a_slash_is_refused() -> TestResult {
    refuses(
        formatted("[year]/[month]"),
        "date_format",
        "the date would hold /",
    )
}

#[test]
fn conventions_format_f8_an_unknown_component_is_refused() -> TestResult {
    refuses(formatted("[invalid]"), "date_format", "not a date format: ")
}

#[test]
fn conventions_format_f9_a_formatted_date_is_left_on_a_second_preview() -> TestResult {
    renames(
        formatted(DF),
        "Report 2025-03-04.md",
        "report-2025-03-04.md",
    )?;
    keeps(formatted(DF), "report-2025-03-04.md")
}

#[test]
fn conventions_format_f10_each_part_once() -> TestResult {
    refuses(
        formatted("[year]"),
        "date_format",
        "a date format writes [year], [month] and [day], each once",
    )?;
    refuses(
        formatted("[year]-[month]-[day]-[day]"),
        "date_format",
        "each once",
    )
}

#[test]
fn conventions_format_f11_year_first_or_the_declared_order() -> TestResult {
    refuses(
        with(|rule| {
            rule.date_order = DateOrder::Ymd;
            "[day].[month].[year]".clone_into(&mut rule.date_format);
        }),
        "date_format",
        "a date format puts the year first, or follows date_order",
    )?;
    refuses(
        with(|rule| {
            rule.date_order = DateOrder::RejectAmbiguous;
            "[day].[month].[year]".clone_into(&mut rule.date_format);
        }),
        "date_format",
        "puts the year first, or follows date_order",
    )?;
    refuses(
        with(|rule| {
            rule.date_order = DateOrder::Dmy;
            "[month].[day].[year]".clone_into(&mut rule.date_format);
        }),
        "date_format",
        "puts the year first, or follows date_order",
    )?;
    refuses(
        formatted("[year]-[day]-[month]"),
        "date_format",
        "puts the year first, or follows date_order",
    )?;
    let month_first = compiled(vec![with(|rule| {
        rule.date_order = DateOrder::Mdy;
        "[month]_[day]_[year]".clone_into(&mut rule.date_format);
    })]);
    assert!(month_first.is_ok());
    Ok(())
}

#[test]
fn conventions_format_f12_a_compact_date_is_zero_padded() -> TestResult {
    refuses(
        formatted("[year padding:none][month padding:none][day padding:none]"),
        "date_format",
        "a compact date writes every part zero-padded",
    )
}

#[test]
fn conventions_format_f13_one_separator_the_same_both_times() -> TestResult {
    refuses(
        formatted("[year]x[month]x[day]"),
        "date_format",
        "between the parts of a date stands -, _ or ., the same both times, or nothing",
    )?;
    refuses(
        formatted("[year]-[month].[day]"),
        "date_format",
        "the same both times",
    )
}

#[test]
fn conventions_format_the_year_keeps_four_digits() -> TestResult {
    refuses(
        formatted("[year repr:last_two]-[month]-[day]"),
        "date_format",
        "the year keeps four digits",
    )?;
    refuses(
        formatted("[year padding:none]-[month]-[day]"),
        "date_format",
        "the year keeps four digits",
    )
}

#[test]
fn conventions_format_unpadded_parts_with_separators_are_accepted() -> TestResult {
    let rule = || formatted("[year]-[month padding:none]-[day padding:none]");
    renames(rule(), "Minutes 2025-03-04.md", "minutes-2025-3-4.md")?;
    keeps(rule(), "minutes-2025-3-4.md")
}

#[test]
fn conventions_format_space_padding_and_text_around_the_date_are_refused() -> TestResult {
    refuses(
        formatted("[year]-[month]-[day padding:space]"),
        "date_format",
        "never with spaces",
    )?;
    refuses(
        formatted("on [year]-[month]-[day]"),
        "date_format",
        "no text before or after it",
    )?;
    refuses(
        formatted("[year base:iso_week]-[month]-[day]"),
        "date_format",
        "read back",
    )
}

// Words: `case` (design 4.5).

fn cased(case: LetterCase, separator: &str) -> NamingRule {
    with(|rule| {
        rule.case = case;
        separator.clone_into(&mut rule.separator);
    })
}

#[test]
fn conventions_case_c1_kebab() -> TestResult {
    renames(r0(), "Meeting Notes.md", "meeting-notes.md")
}

#[test]
fn conventions_case_c2_snake_drops_empty_words() -> TestResult {
    renames(
        cased(LetterCase::Snake, "_"),
        "Meeting  Notes - Q3.md",
        "meeting_notes_q3.md",
    )
}

#[test]
fn conventions_case_c3_upper_uses_full_unicode_mapping() -> TestResult {
    renames(
        cased(LetterCase::Upper, "_"),
        "stra\u{df}e plan.md",
        "STRASSE_PLAN.md",
    )
}

#[test]
fn conventions_case_c4_lower_writes_a_final_sigma() -> TestResult {
    renames(
        cased(LetterCase::Lower, " "),
        "\u{39f}\u{394}\u{3a5}\u{3a3}\u{3a3}\u{395}\u{3a5}\u{3a3} Notes.md",
        "\u{3bf}\u{3b4}\u{3c5}\u{3c3}\u{3c3}\u{3b5}\u{3c5}\u{3c2} notes.md",
    )
}

#[test]
fn conventions_case_c5_title_keeps_a_word_that_holds_a_capital() -> TestResult {
    renames(
        cased(LetterCase::Title, " "),
        "the NASA iPhone report.md",
        "The NASA iPhone Report.md",
    )
}

#[test]
fn conventions_case_c6_title_capitalizes_a_non_ascii_letter() -> TestResult {
    renames(cased(LetterCase::Title, " "), "über uns.md", "Über Uns.md")
}

#[test]
fn conventions_case_c7_preserve_rejoins_only() -> TestResult {
    renames(
        cased(LetterCase::Preserve, "_"),
        "Meeting Notes.md",
        "Meeting_Notes.md",
    )
}

#[test]
fn conventions_case_c8_no_camel_case_split() -> TestResult {
    renames(r0(), "XMLHttpRequest notes.md", "xmlhttprequest-notes.md")
}

#[test]
fn conventions_case_c9_no_digit_split() -> TestResult {
    renames(r0(), "Q3 v2 plan.md", "q3-v2-plan.md")
}

#[test]
fn conventions_case_c10_other_symbols_stay_in_their_word() -> TestResult {
    renames(r0(), "Tom's notes (draft).md", "tom's-notes-(draft).md")
}

#[test]
fn conventions_case_c11_snake_and_kebab_name_their_separator() -> TestResult {
    refuses(
        cased(LetterCase::Snake, "-"),
        "separator",
        "snake case joins words with _",
    )?;
    refuses(
        cased(LetterCase::Kebab, "_"),
        "separator",
        "kebab case joins words with -",
    )
}

// `separator`.

#[test]
fn conventions_separator_s1_empty_joins_words_and_is_stable() -> TestResult {
    renames(
        cased(LetterCase::Title, ""),
        "meeting notes.md",
        "MeetingNotes.md",
    )?;
    keeps(cased(LetterCase::Title, ""), "MeetingNotes.md")
}

#[test]
fn conventions_separator_s2_a_dot_is_stable() -> TestResult {
    renames(
        cased(LetterCase::Lower, "."),
        "Meeting Notes.md",
        "meeting.notes.md",
    )?;
    keeps(cased(LetterCase::Lower, "."), "meeting.notes.md")
}

#[test]
fn conventions_separator_s3_every_break_is_one_word_break() -> TestResult {
    renames(
        cased(LetterCase::Preserve, " "),
        "meeting_notes-final.md",
        "meeting notes final.md",
    )
}

#[test]
fn conventions_separator_s4_a_slash_is_refused() -> TestResult {
    refuses(
        cased(LetterCase::Lower, "/"),
        "separator",
        "a separator cannot hold /",
    )
}

#[test]
fn conventions_separator_s5_a_character_windows_refuses_is_refused() -> TestResult {
    refuses(
        cased(LetterCase::Lower, ":"),
        "separator",
        "path segment holds a character Windows refuses",
    )
}

#[test]
fn conventions_separator_s6_a_control_character_is_refused() -> TestResult {
    let (code, field, _) = refusal(cased(LetterCase::Lower, "\t"))?;
    assert_eq!(
        (code, field.as_str()),
        (ErrorCode::InvalidInput, "/rules/rules/0/separator")
    );
    Ok(())
}

#[test]
fn conventions_separator_s7_a_combining_mark_is_refused() -> TestResult {
    refuses(
        cased(LetterCase::Lower, "\u{301}"),
        "separator",
        "path must be in Unicode NFC",
    )
}

// `prefix` and `suffix` (design 4.2 and 4.6).

fn prefixed(prefix: &str) -> NamingRule {
    with(|rule| prefix.clone_into(&mut rule.prefix))
}

#[test]
fn conventions_prefix_p1_a_prefix_is_added() -> TestResult {
    renames(prefixed("MTG-"), "Weekly sync.md", "MTG-weekly-sync.md")
}

#[test]
fn conventions_prefix_p2_a_prefix_in_another_case_is_not_doubled() -> TestResult {
    renames(prefixed("MTG-"), "mtg-weekly-sync.md", "MTG-weekly-sync.md")
}

#[test]
fn conventions_prefix_p3_a_present_prefix_is_kept() -> TestResult {
    keeps(prefixed("MTG-"), "MTG-weekly-sync.md")
}

#[test]
fn conventions_prefix_p4_a_prefix_is_never_reshaped_by_the_words() -> TestResult {
    keeps(prefixed("2025 "), "2025 plan.md")
}

#[test]
fn conventions_prefix_p5_a_slash_is_refused() -> TestResult {
    refuses(prefixed("a/b"), "prefix", "a prefix cannot hold /")
}

#[test]
fn conventions_prefix_p6_a_device_name_is_refused() -> TestResult {
    refuses(
        prefixed("CON."),
        "prefix",
        "path segment is a name Windows reserves for a device",
    )
}

fn suffixed(suffix: &str) -> NamingRule {
    with(|rule| {
        rule.case = LetterCase::Snake;
        "_".clone_into(&mut rule.separator);
        rule.suffix = Some(suffix.to_owned());
    })
}

#[test]
fn conventions_suffix_x1_a_suffix_is_added() -> TestResult {
    renames(
        suffixed("_notes"),
        "Board meeting.md",
        "board_meeting_notes.md",
    )
}

#[test]
fn conventions_suffix_x2_a_present_suffix_is_kept() -> TestResult {
    keeps(suffixed("_notes"), "board_meeting_notes.md")
}

#[test]
fn conventions_suffix_x3_a_suffix_belongs_to_the_base_before_the_card_tail() -> TestResult {
    let (to, _) = renamed(&named_as(
        vec![with(|rule| rule.suffix = Some("-ed".to_owned()))],
        "report-pdf.md",
        "source",
        Some("report.pdf"),
    )?)?;
    assert_eq!(to, "report-ed-pdf.md");
    Ok(())
}

#[test]
fn conventions_suffix_x4_a_character_windows_refuses_is_refused() -> TestResult {
    refuses(
        with(|rule| rule.suffix = Some("x|".to_owned())),
        "suffix",
        "path segment holds a character Windows refuses",
    )
}

// `extension_policy` (design 4.7).

fn card(policy: ExtensionPolicy, at: &str, original: &str) -> Fallible<Proposal> {
    named_as(
        vec![with(|rule| rule.extension_policy = policy)],
        at,
        "source",
        Some(original),
    )
}

#[test]
fn conventions_extension_e1_preserve_keeps_the_tail_as_written() -> TestResult {
    let proposal = card(
        ExtensionPolicy::Preserve,
        "Q3 Report-PDF.md",
        "Q3 Report.PDF",
    )?;
    assert_eq!(renamed(&proposal)?.0, "q3-report-PDF.md");
    Ok(())
}

#[test]
fn conventions_extension_e2_lowercase_lowers_the_tail() -> TestResult {
    let proposal = card(
        ExtensionPolicy::Lowercase,
        "Q3 Report-PDF.md",
        "Q3 Report.PDF",
    )?;
    assert_eq!(renamed(&proposal)?.0, "q3-report-pdf.md");
    Ok(())
}

#[test]
fn conventions_extension_e3_strip_drops_the_tail() -> TestResult {
    let proposal = card(ExtensionPolicy::Strip, "Q3 Report-PDF.md", "Q3 Report.PDF")?;
    assert_eq!(renamed(&proposal)?.0, "q3-report.md");
    Ok(())
}

#[test]
fn conventions_extension_e4_a_note_has_no_tail() -> TestResult {
    renames(
        with(|rule| rule.extension_policy = ExtensionPolicy::Strip),
        "Plan.md",
        "plan.md",
    )
}

#[test]
fn conventions_extension_e5_the_imports_number_is_kept() -> TestResult {
    let proposal = card(ExtensionPolicy::Strip, "report-pdf-2.md", "report.pdf")?;
    assert_eq!(renamed(&proposal)?.0, "report-2.md");
    Ok(())
}

#[test]
fn conventions_extension_e6_a_card_a_person_renamed_has_no_tail() -> TestResult {
    let proposal = card(ExtensionPolicy::Strip, "Board deck.md", "deck.pptx")?;
    assert_eq!(renamed(&proposal)?.0, "board-deck.md");
    Ok(())
}

#[test]
fn conventions_extension_e7_an_original_with_no_extension_has_no_tail() -> TestResult {
    let proposal = card(ExtensionPolicy::Lowercase, "README.md", "README")?;
    assert_eq!(renamed(&proposal)?.0, "readme.md");
    Ok(())
}

#[test]
fn conventions_extension_e8_a_dot_file_has_no_tail() -> TestResult {
    let proposal = card(ExtensionPolicy::Strip, ".env.md", ".env")?;
    assert!(
        matches!(proposal, Proposal::Unchanged { ref warnings } if warnings.is_empty()),
        "{proposal:?}"
    );
    Ok(())
}

#[test]
fn conventions_extension_e9_an_item_not_ending_in_md_cannot_be_renamed() -> TestResult {
    let proposal = named(vec![r0()], "Plan.MD")?;
    assert_eq!(refused_codes(&proposal)?, [ITEM_NOT_MOVABLE]);
    Ok(())
}

#[test]
fn conventions_extension_a_number_with_a_leading_zero_or_below_two_is_no_tail() -> TestResult {
    // `-02` and `-1` are not the import's numbers, so no tail is read and `strip` drops nothing:
    // read as tails, these would become `report-02.md` and `report-1.md`.
    for at in ["report-pdf-02.md", "report-pdf-1.md"] {
        let proposal = card(ExtensionPolicy::Strip, at, "report.pdf")?;
        assert!(
            matches!(proposal, Proposal::Unchanged { ref warnings } if warnings.is_empty()),
            "{at}: {proposal:?}"
        );
    }
    // And a number of 2 or more is: `-pdf-12` is the tail.
    let proposal = card(ExtensionPolicy::Strip, "report-pdf-12.md", "report.pdf")?;
    assert_eq!(renamed(&proposal)?.0, "report-12.md");
    Ok(())
}

// `destination` (design 4.8).

fn destined(destination: &str) -> NamingRule {
    with(|rule| rule.destination = Some(destination.to_owned()))
}

#[test]
fn conventions_destination_ds1_a_type_moves_into_its_folder() -> TestResult {
    let rule = || {
        with(|rule| {
            "inbox/*.md".clone_into(&mut rule.glob);
            rule.type_name = Some("meeting".to_owned());
            rule.destination = Some("meetings".to_owned());
        })
    };
    let proposal = named_as(vec![rule()], "inbox/Weekly sync.md", "meeting", None)?;
    assert_eq!(renamed(&proposal)?.0, "meetings/weekly-sync.md");
    let proposal = named_as(vec![rule()], "inbox/Weekly sync.md", "note", None)?;
    assert!(matches!(proposal, Proposal::Unmatched), "{proposal:?}");
    Ok(())
}

#[test]
fn conventions_destination_ds2_a_dated_folder_takes_the_names_date() -> TestResult {
    renames(
        with(|rule| {
            DF.clone_into(&mut rule.date_format);
            rule.destination = Some("meetings/[year]".to_owned());
        }),
        "inbox/Sync 2025-03-04.md",
        "meetings/2025/sync-2025-03-04.md",
    )
}

#[test]
fn conventions_destination_ds3_no_date_keeps_the_folder_with_a_warning() -> TestResult {
    let proposal = named(
        vec![with(|rule| {
            DF.clone_into(&mut rule.date_format);
            rule.destination = Some("meetings/[year]".to_owned());
        })],
        "inbox/Sync.md",
    )?;
    let (to, warnings) = renamed(&proposal)?;
    assert_eq!(to, "inbox/sync.md");
    assert_eq!(warnings, [DESTINATION_NEEDS_DATE]);
    Ok(())
}

#[test]
fn conventions_destination_ds4_the_date_is_read_for_the_folder_only() -> TestResult {
    renames(
        with(|rule| {
            rule.date_order = DateOrder::Mdy;
            rule.destination = Some("meetings/[year]".to_owned());
        }),
        "inbox/Sync 03-04-2025.md",
        "meetings/2025/sync-03-04-2025.md",
    )
}

#[test]
fn conventions_destination_ds5_an_ambiguous_date_refuses_the_name() -> TestResult {
    let proposal = named(
        vec![with(|rule| {
            rule.date_order = DateOrder::RejectAmbiguous;
            rule.destination = Some("meetings/[year]".to_owned());
        })],
        "inbox/Sync 03-04-2025.md",
    )?;
    assert_eq!(refused_codes(&proposal)?, [DATE_AMBIGUOUS]);
    Ok(())
}

#[test]
fn conventions_destination_ds6_an_empty_destination_is_refused() -> TestResult {
    refuses(
        destined(""),
        "destination",
        "leave destination out to keep each item's folder",
    )
}

#[test]
fn conventions_destination_ds7_a_reserved_folder_is_refused() -> TestResult {
    refuses(destined(".okf/x"), "destination", "is kept by the server")?;
    refuses(
        destined("notes/index.md"),
        "destination",
        "is kept by the server",
    )?;
    refuses(
        destined("Notes/LOG.md/x"),
        "destination",
        "is kept by the server",
    )
}

#[test]
fn conventions_destination_ds8_a_folder_that_is_no_workspace_path_is_refused() -> TestResult {
    refuses(
        destined("/meetings"),
        "destination",
        "path must be normalized, relative, and outside .git",
    )?;
    refuses(
        destined("meetings/"),
        "destination",
        "path must be normalized, relative, and outside .git",
    )?;
    refuses(
        destined("CON/x"),
        "destination",
        "path segment is a name Windows reserves for a device",
    )
}

#[test]
fn conventions_destination_ds9_a_part_a_date_does_not_have_is_refused() -> TestResult {
    refuses(
        destined("[hour]"),
        "destination",
        "names a part a date does not have",
    )
}

#[test]
fn conventions_destination_a_month_name_folder_is_allowed() -> TestResult {
    renames(
        with(|rule| rule.destination = Some("archive/[year]/[month repr:long]".to_owned())),
        "Plan 2025-03-04.md",
        "archive/2025/March/plan-2025-03-04.md",
    )
}

// What stays (design 4.9).

#[test]
fn conventions_stays_w1_an_archived_item_is_left_out_and_holds_its_name() -> TestResult {
    let mut archived = item(1, "a/Report.md")?;
    archived.summary.archived = true;
    let items = [archived, item(2, "a/REPORT v2.md")?];
    let body = plan_all(vec![stripping()], &items)?;
    assert_eq!(body.archived_left_out, 1);
    assert_eq!(body.considered, 1);
    assert_eq!(moves(&body), [(id(2), "a/report-2.md".to_owned())]);
    counts_add_up(&body)
}

#[test]
fn conventions_stays_w2_a_reserved_result_is_refused() -> TestResult {
    let proposal = named(
        vec![with(|rule| {
            rule.strip_version_suffix = true;
            rule.case = LetterCase::Lower;
        })],
        "LOG v2.md",
    )?;
    assert_eq!(refused_codes(&proposal)?, [PATH_RESERVED]);
    Ok(())
}

#[test]
fn conventions_stays_w3_a_name_already_right_is_unchanged_not_an_entry() -> TestResult {
    keeps(r0(), "notes/plan-a.md")?;
    let body = plan_all(vec![r0()], &[item(1, "notes/plan-a.md")?])?;
    assert_eq!((body.entries.len(), body.unchanged), (0, 1));
    Ok(())
}

#[test]
fn conventions_stays_w4_a_device_name_result_is_refused() -> TestResult {
    let proposal = named(vec![stripping()], "CON v2.md")?;
    assert_eq!(refused_codes(&proposal)?, [PATH_NOT_VALID]);
    Ok(())
}

#[test]
fn conventions_stays_w5_a_name_left_empty_is_refused() -> TestResult {
    let proposal = named(vec![r0()], "___.md")?;
    assert_eq!(refused_codes(&proposal)?, [NAME_LEFT_EMPTY]);
    Ok(())
}

#[test]
fn conventions_stays_w6_a_result_over_the_path_bound_is_refused() -> TestResult {
    let mut segments: Vec<String> = (0..15).map(|_| "f".repeat(255)).collect();
    segments.push("g".repeat(240));
    let folder = segments.join("/");
    assert_eq!(folder.len(), 4080);
    let proposal = named(vec![prefixed("abcdefghij-")], &format!("{folder}/Plan.md"))?;
    assert_eq!(refused_codes(&proposal)?, [PATH_NOT_VALID]);
    Ok(())
}

#[test]
fn conventions_stays_a_refused_name_counts_as_refused_and_is_named() -> TestResult {
    let items = [
        item(1, "CON v2.md")?,
        item(2, "___.md")?,
        item(3, "notes/plan-a.md")?,
        item(4, "notes/Plan B.md")?,
    ];
    let body = plan_all(vec![stripping()], &items)?;
    assert_eq!(
        (body.entries.len(), body.unchanged, body.refused),
        (1, 1, 2)
    );
    assert_eq!(
        plan_warnings(&body),
        [
            ("CON v2.md".to_owned(), PATH_NOT_VALID.to_owned()),
            ("___.md".to_owned(), NAME_LEFT_EMPTY.to_owned())
        ]
    );
    counts_add_up(&body)
}

// Collisions (design 4.10).

#[test]
fn conventions_collision_co1_suffix_numbers_the_later_claimant() -> TestResult {
    let items = [item(1, "a/Report v2.md")?, item(2, "a/Report (1).md")?];
    let body = plan_all(vec![stripping()], &items)?;
    assert_eq!(
        moves(&body),
        [
            (id(2), "a/report.md".to_owned()),
            (id(1), "a/report-2.md".to_owned())
        ]
    );
    let numbered_warnings = body
        .entries
        .iter()
        .find(|planned| planned.entry.item_id == id(1))
        .map(|planned| codes(&planned.entry.warnings));
    assert_eq!(numbered_warnings, Some(vec![COLLISION_SUFFIXED.to_owned()]));
    let [observation] = body.duplicate_observations.as_slice() else {
        return Err(format!("not one observation: {:?}", body.duplicate_observations).into());
    };
    assert_eq!(observation.path.as_str(), "a/report.md");
    assert_eq!(observation.item_ids, [id(1), id(2)]);
    counts_add_up(&body)
}

#[test]
fn conventions_collision_co2_a_stayer_holds_its_name() -> TestResult {
    let items = [item(3, "a/report.md")?, item(1, "a/Report v2.md")?];
    let selected = BTreeSet::from([id(1)]);
    let body = plan_of(vec![stripping()], &items, &selected)?;
    assert_eq!(moves(&body), [(id(1), "a/report-2.md".to_owned())]);
    let [observation] = body.duplicate_observations.as_slice() else {
        return Err(format!("not one observation: {:?}", body.duplicate_observations).into());
    };
    assert_eq!(observation.path.as_str(), "a/report.md");
    assert_eq!(observation.item_ids, [id(1), id(3)]);
    Ok(())
}

#[test]
fn conventions_collision_co3_keep_existing_leaves_the_later_claimant() -> TestResult {
    let items = [item(1, "a/Report v2.md")?, item(2, "a/Report (1).md")?];
    let rule = with(|rule| {
        rule.strip_version_suffix = true;
        rule.collision = CollisionPolicy::KeepExisting;
    });
    let body = plan_all(vec![rule], &items)?;
    assert_eq!(moves(&body), [(id(2), "a/report.md".to_owned())]);
    assert_eq!((body.unchanged, body.refused), (1, 0));
    assert_eq!(
        plan_warnings(&body),
        [("a/Report v2.md".to_owned(), COLLISION_KEPT.to_owned())]
    );
    assert_eq!(body.duplicate_observations.len(), 1);
    counts_add_up(&body)
}

#[test]
fn conventions_collision_co4_reject_gives_a_contested_name_to_nobody() -> TestResult {
    let items = [item(1, "a/Report v2.md")?, item(2, "a/Report (1).md")?];
    let rule = with(|rule| {
        rule.strip_version_suffix = true;
        rule.collision = CollisionPolicy::Reject;
    });
    let body = plan_all(vec![rule], &items)?;
    assert_eq!(body.entries.len(), 0);
    assert_eq!((body.unchanged, body.refused), (0, 2));
    assert_eq!(
        plan_warnings(&body),
        [
            ("a/Report (1).md".to_owned(), COLLISION_REJECTED.to_owned()),
            ("a/Report v2.md".to_owned(), COLLISION_REJECTED.to_owned())
        ]
    );
    let [observation] = body.duplicate_observations.as_slice() else {
        return Err(format!("not one observation: {:?}", body.duplicate_observations).into());
    };
    assert_eq!(observation.item_ids, [id(1), id(2)]);
    counts_add_up(&body)
}

#[test]
fn conventions_collision_co5_an_items_own_key_is_no_collision() -> TestResult {
    let body = plan_all(vec![r0()], &[item(1, "a/Report.md")?])?;
    assert_eq!(moves(&body), [(id(1), "a/report.md".to_owned())]);
    assert_eq!(body.duplicate_observations.len(), 0);
    Ok(())
}

#[test]
fn conventions_collision_co6_a_case_only_rename_goes_first() -> TestResult {
    // By path order `a/REPORT v2.md` would claim `a/report.md` first.
    let items = [item(1, "a/Report.md")?, item(2, "a/REPORT v2.md")?];
    let body = plan_all(vec![stripping()], &items)?;
    assert_eq!(
        moves(&body),
        [
            (id(2), "a/report-2.md".to_owned()),
            (id(1), "a/report.md".to_owned())
        ]
    );
    counts_add_up(&body)
}

fn routed(from: &str, to: &str) -> NamingRule {
    with(|rule| {
        rule.glob = format!("{from}/*.md");
        rule.destination = Some(to.to_owned());
    })
}

#[test]
fn conventions_collision_co7_a_chain_frees_each_name() -> TestResult {
    let items = [item(1, "a/x.md")?, item(2, "b/x.md")?];
    let body = plan_all(vec![routed("a", "b"), routed("b", "c")], &items)?;
    assert_eq!(
        moves(&body),
        [(id(1), "b/x.md".to_owned()), (id(2), "c/x.md".to_owned())]
    );
    assert_eq!(body.duplicate_observations.len(), 0);
    Ok(())
}

#[test]
fn conventions_collision_co8_a_swap_frees_both_names() -> TestResult {
    let items = [item(1, "a/x.md")?, item(2, "b/x.md")?];
    let body = plan_all(vec![routed("a", "b"), routed("b", "a")], &items)?;
    assert_eq!(
        moves(&body),
        [(id(1), "b/x.md".to_owned()), (id(2), "a/x.md".to_owned())]
    );
    Ok(())
}

#[test]
fn conventions_collision_co9_an_unselected_item_is_a_stayer() -> TestResult {
    let items = [item(1, "a/Report v2.md")?, item(3, "a/report.md")?];
    let body = plan_of(vec![stripping()], &items, &BTreeSet::from([id(1)]))?;
    assert_eq!(moves(&body), [(id(1), "a/report-2.md".to_owned())]);
    assert_eq!(body.considered, 1);
    Ok(())
}

#[test]
fn conventions_collision_a_folder_holds_its_name() -> TestResult {
    let items = [item(1, "a/Report.md/inner.md")?, item(2, "a/Report v2.md")?];
    let body = plan_of(vec![stripping()], &items, &BTreeSet::from([id(2)]))?;
    // `a/report.md` is a folder (of `a/Report.md/inner.md`), whatever its letter case.
    assert_eq!(moves(&body), [(id(2), "a/report-2.md".to_owned())]);
    Ok(())
}

/// SPEC section 3 compares folders by collision key; section 6's accepted preview must
/// retain the folder spelling storage accepts, including empty and nested folders.
#[test]
fn conventions_destination_reuses_the_stored_folder_spelling() -> TestResult {
    let items = [item(1, "inbox/Report.md")?, item(2, "notes/existing.md")?];
    let selected = BTreeSet::from([id(1)]);
    for (destination, folder, expected) in [
        ("Notes", "notes", "notes/report.md"),
        ("ARCHIVE", "Archive", "Archive/report.md"),
        ("Notes/New", "notes", "notes/New/report.md"),
        ("Notes/PAST/New", "notes/Past", "notes/Past/New/report.md"),
    ] {
        let body = plan(
            &compiled(vec![routed("inbox", destination)])?,
            &items,
            &[path("notes")?, path(folder)?],
            &selected,
        )?;
        assert_eq!(
            moves(&body),
            [(id(1), expected.to_owned())],
            "{destination}"
        );
        assert!(body.warnings.is_empty(), "{destination}");
        assert_eq!(body.refused, 0, "{destination}");
        counts_add_up(&body)?;
    }
    Ok(())
}

/// A rule's respelling of a folder alone is not an item rename (SPEC sections 3 and 6).
#[test]
fn conventions_destination_a_stored_folder_respelling_alone_is_unchanged() -> TestResult {
    let items = [item(1, "notes/report.md")?];
    let body = plan_all(vec![routed("notes", "Notes")], &items)?;
    assert!(body.entries.is_empty());
    assert_eq!((body.unchanged, body.refused), (1, 0));
    counts_add_up(&body)
}

#[test]
fn conventions_collision_a_kept_mover_holds_its_name_in_a_second_round() -> TestResult {
    // B would free `b/x.md` for A, but B's own name is taken by C under keep_existing, so B
    // stays and A is numbered.
    let items = [item(1, "a/x.md")?, item(2, "b/x.md")?, item(3, "c/x.md")?];
    let rules = vec![
        routed("a", "b"),
        with(|rule| {
            "b/*.md".clone_into(&mut rule.glob);
            rule.destination = Some("c".to_owned());
            rule.collision = CollisionPolicy::KeepExisting;
        }),
    ];
    let body = plan_of(rules, &items, &BTreeSet::from([id(1), id(2)]))?;
    assert_eq!(moves(&body), [(id(1), "b/x-2.md".to_owned())]);
    assert_eq!((body.unchanged, body.refused), (1, 0));
    counts_add_up(&body)
}

// Determinism and the digest (design 5).

#[test]
fn conventions_determinism_dt1_the_same_preview_twice_is_identical() -> TestResult {
    let items = [
        item(1, "a/Report v2.md")?,
        item(2, "a/Report (1).md")?,
        item(3, "CON v2.md")?,
    ];
    let first = plan_all(vec![stripping()], &items)?;
    let second = plan_all(vec![stripping()], &items)?;
    assert_eq!(format!("{first:?}"), format!("{second:?}"));
    Ok(())
}

#[test]
fn conventions_determinism_dt2_the_selection_order_changes_nothing() -> TestResult {
    let forward = [item(1, "b/B.md")?, item(2, "a/A.md")?, item(3, "c/C.md")?];
    let backward = [item(3, "c/C.md")?, item(2, "a/A.md")?, item(1, "b/B.md")?];
    let first = plan_all(vec![r0()], &forward)?;
    let second = plan_all(vec![r0()], &backward)?;
    assert_eq!(format!("{first:?}"), format!("{second:?}"));
    Ok(())
}

#[test]
fn conventions_determinism_dt3_the_digest_ignores_key_order_and_absent_options() -> TestResult {
    let typed = request_digest(&rule_set(vec![r0()]))?;
    let reordered: NamingRules = serde_json::from_value(serde_json::json!({
        "rules": [{
            "collision": "suffix", "destination": null, "alias_preservation": false,
            "extension_policy": "preserve", "strip_version_suffix": false, "date_format": "",
            "date_order": "ymd", "suffix": null, "prefix": "", "separator": "-",
            "case": "kebab", "type_name": null, "glob": "**"
        }],
        "schema_version": 1
    }))?;
    assert_eq!(request_digest(&reordered)?, typed);
    let other = request_digest(&rule_set(vec![cased(LetterCase::Lower, "_")]))?;
    assert_ne!(other, typed);
    Ok(())
}

#[test]
fn conventions_determinism_dt4_every_list_is_sorted() -> TestResult {
    let items = [
        item(1, "z/Report v2.md")?,
        item(2, "z/Report (1).md")?,
        item(3, "a/Report v2.md")?,
        item(4, "a/Report (1).md")?,
        item(5, "m/Plan B.md")?,
        item(6, "CON v2.md")?,
        item(7, "___.md")?,
    ];
    let body = plan_all(vec![stripping()], &items)?;
    let froms: Vec<&str> = body
        .entries
        .iter()
        .map(|planned| planned.entry.from.as_str())
        .collect();
    let mut sorted = froms.clone();
    sorted.sort_unstable();
    assert_eq!(froms, sorted);
    assert_eq!(froms.len(), 5);
    let paths: Vec<&str> = body
        .duplicate_observations
        .iter()
        .map(|observation| observation.path.as_str())
        .collect();
    assert_eq!(paths, ["a/report.md", "z/report.md"]);
    let locations: Vec<(String, String)> = plan_warnings(&body);
    let mut ordered = locations.clone();
    ordered.sort();
    assert_eq!(locations, ordered);
    assert_eq!(locations.len(), 2);
    Ok(())
}

// Validation (design 6).

#[test]
fn conventions_validation_sv1_schema_version_1_is_accepted() -> TestResult {
    compiled(vec![r0()])?;
    Ok(())
}

#[test]
fn conventions_validation_sv2_sv3_another_schema_version_is_refused() -> TestResult {
    for version in [0, 2] {
        let error = err_of(CompiledRules::compile(
            &NamingRules {
                schema_version: version,
                rules: vec![r0()],
            },
            "/rules",
        ))?;
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("/rules/schema_version"));
    }
    Ok(())
}

#[test]
fn conventions_validation_sv4_problems_locate_every_field_in_the_file() -> TestResult {
    let problems = CompiledRules::problems(&NamingRules {
        schema_version: 2,
        rules: vec![
            r0(),
            with(|rule| "notes/**x".clone_into(&mut rule.glob)),
            with(|rule| {
                "/".clone_into(&mut rule.separator);
                rule.prefix = "a".repeat(1025);
            }),
        ],
    })?;
    let located: Vec<(String, String)> = problems
        .iter()
        .map(|problem| {
            (
                problem.location.clone().unwrap_or_default(),
                problem.code.clone(),
            )
        })
        .collect();
    assert_eq!(
        located,
        [
            (
                "/schema_version".to_owned(),
                RULES_SCHEMA_VERSION_UNKNOWN.to_owned()
            ),
            ("/rules/1/glob".to_owned(), RULES_INVALID.to_owned()),
            ("/rules/2/separator".to_owned(), RULES_INVALID.to_owned()),
            ("/rules/2/prefix".to_owned(), RULES_TOO_LARGE.to_owned()),
        ]
    );
    Ok(())
}

#[test]
fn conventions_validation_sv5_more_than_256_rules_is_too_large() -> TestResult {
    let error = err_of(compiled(vec![r0(); 257]))?;
    assert_eq!(error.code, ErrorCode::TooLarge);
    assert_eq!(error.field.as_deref(), Some("/rules/rules"));
    assert!(compiled(vec![r0(); 256]).is_ok());
    Ok(())
}

#[test]
fn conventions_validation_the_first_refusal_is_the_first_field() -> TestResult {
    let error = err_of(compiled(vec![
        r0(),
        with(|rule| {
            "a/b".clone_into(&mut rule.prefix);
            rule.glob = String::new();
        }),
    ]))?;
    assert_eq!(error.field.as_deref(), Some("/rules/rules/1/glob"));
    Ok(())
}

#[test]
fn conventions_validation_text_fields_are_bounded_at_1024_bytes() -> TestResult {
    let long = "a".repeat(1025);
    for (field, rule) in [
        (
            "type_name",
            with(|rule| rule.type_name = Some(long.clone())),
        ),
        ("prefix", with(|rule| long.clone_into(&mut rule.prefix))),
        ("suffix", with(|rule| rule.suffix = Some(long.clone()))),
        (
            "date_format",
            with(|rule| long.clone_into(&mut rule.date_format)),
        ),
        (
            "destination",
            with(|rule| rule.destination = Some(long.clone())),
        ),
        (
            "separator",
            with(|rule| {
                rule.case = LetterCase::Lower;
                long.clone_into(&mut rule.separator);
            }),
        ),
    ] {
        let (code, at, _) = refusal(rule)?;
        assert_eq!(code, ErrorCode::TooLarge, "{field}");
        assert_eq!(at, format!("/rules/rules/0/{field}"));
    }
    Ok(())
}

// `numbered` and the shared function for the import (design 8).

#[test]
fn conventions_numbered_inserts_the_number_before_md() -> TestResult {
    assert_eq!(
        numbered(&path("notes/report.md")?, 2)?.as_str(),
        "notes/report-2.md"
    );
    assert_eq!(
        numbered(&path("report-pdf.md")?, 3)?.as_str(),
        "report-pdf-3.md"
    );
    Ok(())
}

#[test]
fn conventions_numbered_shortens_a_name_at_the_segment_bound() -> TestResult {
    let long = format!("notes/{}.md", "r".repeat(252));
    let shortened = numbered(&path(&long)?, 2)?;
    let name = some(shortened.as_str().rsplit('/').next(), "a name")?;
    assert_eq!(name.encode_utf16().count(), 255);
    assert!(name.ends_with("-2.md"));
    Ok(())
}

#[test]
fn conventions_numbered_shortens_a_name_at_the_path_bound() -> TestResult {
    let mut segments: Vec<String> = (0..15).map(|_| "f".repeat(255)).collect();
    segments.push("g".repeat(240));
    let folder = segments.join("/");
    let at = format!("{folder}/{}.md", "p".repeat(12));
    assert_eq!(at.len(), 4096);
    let shortened = numbered(&path(&at)?, 2)?;
    assert_eq!(shortened.as_str().len(), 4096);
    assert!(shortened.as_str().ends_with("-2.md"));
    Ok(())
}

fn card_subject<'a>(at: &'a WorkspacePath, original: &'a str) -> Subject<'a> {
    Subject {
        path: at,
        type_name: "source",
        original_name: Some(original),
    }
}

#[test]
fn conventions_import_im2_a_card_is_named_by_the_rules() -> TestResult {
    let at = path("inbox/Q3 Report-PDF.md")?;
    let mut taken = BTreeSet::new();
    let name = name_new_item(
        &compiled(vec![r0()])?,
        &card_subject(&at, "Q3 Report.PDF"),
        &mut taken,
    )?;
    assert_eq!(name.path.as_str(), "inbox/q3-report-PDF.md");
    assert!(taken.contains("inbox/q3-report-pdf.md"));
    Ok(())
}

#[test]
fn conventions_import_im3_the_jobs_own_cards_are_taken() -> TestResult {
    let rules = compiled(vec![r0()])?;
    let mut taken = BTreeSet::new();
    let first = path("inbox/a-pdf.md")?;
    let second = path("inbox/A-pdf.md")?;
    let one = name_new_item(&rules, &card_subject(&first, "a.pdf"), &mut taken)?;
    let two = name_new_item(&rules, &card_subject(&second, "A.pdf"), &mut taken)?;
    assert_eq!(
        (one.path.as_str(), two.path.as_str()),
        ("inbox/a-pdf.md", "inbox/a-pdf-2.md")
    );
    assert_eq!(codes(&two.warnings), [COLLISION_SUFFIXED]);
    assert_eq!(
        two.warnings
            .first()
            .and_then(|warning| warning.location.as_deref()),
        Some("inbox/a-pdf-2.md")
    );
    Ok(())
}

#[test]
fn conventions_import_im4_keep_existing_falls_back_to_the_name_without_rules() -> TestResult {
    let at = path("inbox/Q3 Report-pdf.md")?;
    let mut taken = BTreeSet::from(["inbox/q3-report-pdf.md".to_owned()]);
    let rules = compiled(vec![with(|rule| {
        rule.collision = CollisionPolicy::KeepExisting;
    })])?;
    let name = name_new_item(&rules, &card_subject(&at, "Q3 Report.pdf"), &mut taken)?;
    assert_eq!(name.path.as_str(), "inbox/Q3 Report-pdf.md");
    assert_eq!(codes(&name.warnings), [COLLISION_KEPT]);
    Ok(())
}

#[test]
fn conventions_import_im4_reject_falls_back_and_then_numbers_without_rules() -> TestResult {
    let at = path("inbox/Q3 Report-pdf.md")?;
    let mut taken = BTreeSet::from([
        "inbox/q3-report-pdf.md".to_owned(),
        "inbox/q3 report-pdf.md".to_owned(),
    ]);
    let rules = compiled(vec![with(|rule| rule.collision = CollisionPolicy::Reject)])?;
    let name = name_new_item(&rules, &card_subject(&at, "Q3 Report.pdf"), &mut taken)?;
    assert_eq!(name.path.as_str(), "inbox/Q3 Report-pdf-2.md");
    assert_eq!(codes(&name.warnings), [COLLISION_REJECTED]);
    Ok(())
}

#[test]
fn conventions_import_im7_suffix_numbers_the_ruled_name() -> TestResult {
    let at = path("inbox/Q3 Report-pdf.md")?;
    let mut taken = BTreeSet::from(["inbox/q3-report-pdf.md".to_owned()]);
    let name = name_new_item(
        &compiled(vec![r0()])?,
        &card_subject(&at, "Q3 Report.pdf"),
        &mut taken,
    )?;
    assert_eq!(name.path.as_str(), "inbox/q3-report-pdf-2.md");
    Ok(())
}

#[test]
fn conventions_import_im5_a_card_the_import_named_is_unchanged_in_a_preview() -> TestResult {
    let rules = compiled(vec![r0()])?;
    let mut taken = BTreeSet::new();
    let first = path("inbox/a-pdf.md")?;
    let second = path("inbox/A-pdf.md")?;
    let one = name_new_item(&rules, &card_subject(&first, "a.pdf"), &mut taken)?;
    let two = name_new_item(&rules, &card_subject(&second, "A.pdf"), &mut taken)?;
    let items = [
        item_of(1, one.path.as_str(), "source", Some("a.pdf"))?,
        item_of(2, two.path.as_str(), "source", Some("A.pdf"))?,
    ];
    let body = plan_all(vec![r0()], &items)?;
    assert_eq!((body.entries.len(), body.unchanged), (0, 2));
    Ok(())
}

#[test]
fn conventions_import_an_unmatched_card_keeps_the_numbering_without_rules() -> TestResult {
    let at = path("inbox/report-pdf.md")?;
    let mut taken = BTreeSet::from(["inbox/report-pdf.md".to_owned()]);
    let rules = compiled(vec![with(|rule| "elsewhere/**".clone_into(&mut rule.glob))])?;
    let name = name_new_item(&rules, &card_subject(&at, "report.pdf"), &mut taken)?;
    assert_eq!(name.path.as_str(), "inbox/report-pdf-2.md");
    assert!(name.warnings.is_empty());
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
