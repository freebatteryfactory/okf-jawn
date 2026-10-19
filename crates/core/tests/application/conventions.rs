//! The naming rules' four operations over fakes of their ports (core-m1-3b-design.md section
//! 7, vectors GR, SR, PN and AN), named `conventions_<operation>_...` so
//! `cargo test -p okf-jawn-core -- conventions` selects them. The fake version store answers
//! `tree` from the documents scripted at a revision, in path order, with each source card's
//! original name from its application header, and `rules` from what a test scripted; it does
//! not rewrite links or write aliases, so those are proven against real storage
//! (`crates/storage/tests/conventions.rs`).

use std::collections::BTreeSet;

use okf_jawn_contract::common::Warning;
use okf_jawn_contract::conventions::{
    ApplyNamesRequest, CollisionPolicy, DateOrder, ExtensionPolicy, GetRulesRequest, LetterCase,
    NameSelection, NamingRule, NamingRules, PreviewNamesRequest, RenamePlan, SetRulesRequest,
};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::events::EventKind;
use okf_jawn_contract::identity::{At, IdempotencyKey, ItemId, PurgeId, Revision};
use okf_jawn_contract::item::ItemDocument;
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::context::Attempt;
use okf_jawn_core::conventions::{
    COLLISION_SUFFIXED, DATE_AMBIGUOUS, RULES_INVALID, RULES_SCHEMA_VERSION_UNKNOWN,
    RULES_YAML_INVALID,
};
use okf_jawn_core::jobs::RevisionMapping;
use okf_jawn_core::mutations::request_digest;
use okf_jawn_core::ports::Application;
use okf_jawn_core::storage::{RulesFile, RulesParseError, TreeEdit, VersionStore as _};
use serde_json::json;
use uuid::Uuid;

use crate::check::{TestResult, err_of, some};
use crate::fixture::{Built, World, alice, mutation, note, path, revision, scope};

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

fn with(change: impl FnOnce(&mut NamingRule)) -> NamingRule {
    let mut rule = r0();
    change(&mut rule);
    rule
}

fn rules(rules: Vec<NamingRule>) -> NamingRules {
    NamingRules {
        schema_version: 1,
        rules,
    }
}

fn key(number: u128) -> IdempotencyKey {
    IdempotencyKey(Uuid::from_u128(number))
}

fn item(number: u128) -> ItemId {
    ItemId(Uuid::from_u128(number))
}

/// A note `number` at `at` in the head `a`.
fn put(world: &World, number: u128, at: &str) -> Built<()> {
    let head = revision('a')?;
    world.versions.put_document(
        &head,
        note(number, &head, at, "# Plan\n", json!({ "type": "Note" }))?,
    )
}

/// `document` archived.
fn archived(mut document: ItemDocument) -> ItemDocument {
    document.summary.archived = true;
    document
}

fn get_request(at: At) -> Built<GetRulesRequest> {
    Ok(GetRulesRequest {
        workspace_id: scope()?.workspace_id,
        at,
    })
}

fn preview_request(selection: NameSelection, rules: NamingRules) -> Built<PreviewNamesRequest> {
    Ok(PreviewNamesRequest {
        workspace_id: scope()?.workspace_id,
        at: At::Latest,
        selection,
        rules,
    })
}

async fn preview(world: &World, selection: NameSelection, set: NamingRules) -> Built<RenamePlan> {
    Ok(world
        .service
        .preview_names(
            &alice(OperationName::PreviewNames, None)?,
            preview_request(selection, set)?,
        )
        .await?)
}

fn apply_request(plan: RenamePlan, rules: NamingRules) -> Built<ApplyNamesRequest> {
    Ok(ApplyNamesRequest {
        workspace_id: scope()?.workspace_id,
        plan,
        rules,
        idempotency_key: key(9),
    })
}

async fn apply(
    world: &World,
    plan: RenamePlan,
    set: NamingRules,
) -> Result<okf_jawn_contract::common::MutationResult, ApiError> {
    let request = apply_request(plan, set)
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
    let context = alice(OperationName::ApplyNames, Some(mutation(1)))
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
    world.service.apply_names(&context, request).await
}

fn moved(edits: &[TreeEdit]) -> Vec<(ItemId, String, bool)> {
    edits
        .iter()
        .filter_map(|edit| match edit {
            TreeEdit::MoveItem {
                item_id,
                destination,
                keep_alias,
            } => Some((*item_id, destination.as_str().to_owned(), *keep_alias)),
            _ => None,
        })
        .collect()
}

fn field_of(error: &ApiError) -> &str {
    error.field.as_deref().unwrap_or_default()
}

fn located(problems: &[Warning]) -> Vec<(String, String)> {
    problems
        .iter()
        .map(|problem| {
            (
                problem.code.clone(),
                problem.location.clone().unwrap_or_default(),
            )
        })
        .collect()
}

// get_rules (design 7.1).

#[tokio::test]
async fn conventions_get_rules_gr1_no_rules_saved_answers_neither() -> TestResult {
    let world = World::new()?;
    let answer = world
        .service
        .get_rules(
            &alice(OperationName::GetRules, None)?,
            get_request(At::Latest)?,
        )
        .await?;
    assert_eq!(answer.revision, revision('a')?);
    assert!(answer.rules.is_none());
    assert!(answer.unreadable.is_none());
    // A read receipt with no sources, as `log_items` records.
    let receipts = world.records.receipts()?;
    let [(None, receipt)] = receipts.as_slice() else {
        return Err(format!("not one read receipt: {receipts:?}").into());
    };
    assert!(receipt.sources.is_empty());
    assert_eq!(receipt.operation_id, OperationName::GetRules);
    Ok(())
}

#[tokio::test]
async fn conventions_get_rules_gr2_saved_rules_are_answered() -> TestResult {
    let world = World::new()?;
    let saved = rules(vec![
        with(|rule| {
            "inbox/**".clone_into(&mut rule.glob);
            rule.type_name = Some("meeting".to_owned());
            rule.date_order = DateOrder::Dmy;
            "[year]-[month]-[day]".clone_into(&mut rule.date_format);
            rule.destination = Some("meetings/[year]".to_owned());
        }),
        r0(),
    ]);
    world.versions.put_rules(
        &revision('a')?,
        RulesFile {
            text: "schema_version: 1\n".to_owned(),
            parsed: Ok(saved.clone()),
        },
    )?;
    let answer = world
        .service
        .get_rules(
            &alice(OperationName::GetRules, None)?,
            get_request(At::Latest)?,
        )
        .await?;
    assert_eq!(
        serde_json::to_value(some(answer.rules, "the rules")?)?,
        serde_json::to_value(saved)?
    );
    assert!(answer.unreadable.is_none());
    Ok(())
}

#[tokio::test]
async fn conventions_get_rules_gr3_a_yaml_error_is_unreadable_at_its_line() -> TestResult {
    let world = World::new()?;
    let text = "schema_version: 1\ncolour: red\nrules: []\n";
    world.versions.put_rules(
        &revision('a')?,
        RulesFile {
            text: text.to_owned(),
            parsed: Err(RulesParseError {
                message: "unknown field `colour`".to_owned(),
                line: Some(2),
                column: Some(1),
            }),
        },
    )?;
    let answer = world
        .service
        .get_rules(
            &alice(OperationName::GetRules, None)?,
            get_request(At::Latest)?,
        )
        .await?;
    assert!(answer.rules.is_none());
    let unreadable = some(answer.unreadable, "the unreadable file")?;
    assert_eq!(unreadable.text, text);
    assert_eq!(
        located(&unreadable.problems),
        [(RULES_YAML_INVALID.to_owned(), "line 2, column 1".to_owned())]
    );
    Ok(())
}

#[tokio::test]
async fn conventions_get_rules_gr4_gr5_rules_this_build_cannot_apply_are_unreadable() -> TestResult
{
    let world = World::new()?;
    world.versions.put_rules(
        &revision('a')?,
        RulesFile {
            text: "schema_version: 2\n".to_owned(),
            parsed: Ok(NamingRules {
                schema_version: 2,
                rules: vec![with(|rule| "notes/**x".clone_into(&mut rule.glob))],
            }),
        },
    )?;
    let answer = world
        .service
        .get_rules(
            &alice(OperationName::GetRules, None)?,
            get_request(At::Latest)?,
        )
        .await?;
    assert!(answer.rules.is_none());
    let unreadable = some(answer.unreadable, "the unreadable file")?;
    assert_eq!(unreadable.text, "schema_version: 2\n");
    assert_eq!(
        located(&unreadable.problems),
        [
            (
                RULES_SCHEMA_VERSION_UNKNOWN.to_owned(),
                "/schema_version".to_owned()
            ),
            (RULES_INVALID.to_owned(), "/rules/0/glob".to_owned()),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn conventions_get_rules_gr6_a_file_over_the_text_bound_is_too_large() -> TestResult {
    let world = World::new()?;
    let limit = okf_jawn_contract::item::MAX_ITEM_TEXT_BYTES;
    for (size, refused) in [(limit, false), (limit.saturating_add(1), true)] {
        world.versions.put_rules(
            &revision('a')?,
            RulesFile {
                text: "#".repeat(size),
                parsed: Err(RulesParseError {
                    message: "no rules".to_owned(),
                    line: None,
                    column: None,
                }),
            },
        )?;
        let answer = world
            .service
            .get_rules(
                &alice(OperationName::GetRules, None)?,
                get_request(At::Latest)?,
            )
            .await;
        if refused {
            assert_eq!(err_of(answer)?.code, ErrorCode::TooLarge);
        } else {
            let unreadable = some(answer?.unreadable, "the unreadable file")?;
            assert_eq!(
                located(&unreadable.problems),
                [(RULES_YAML_INVALID.to_owned(), String::new())]
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn conventions_get_rules_gr7_a_purged_revision_is_not_found() -> TestResult {
    let world = World::new()?;
    world.records.put_mapping(
        revision('c')?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(5)),
            replacement: None,
        },
    )?;
    let refused = err_of(
        world
            .service
            .get_rules(
                &alice(OperationName::GetRules, None)?,
                get_request(At::Revision {
                    revision: revision('c')?,
                })?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    Ok(())
}

#[tokio::test]
async fn conventions_get_rules_gr8_a_file_that_is_not_utf8_is_unreadable() -> TestResult {
    let world = World::new()?;
    world.versions.put_rules(
        &revision('a')?,
        RulesFile {
            text: "\u{fffd}\u{fffd}schema_version: 1\n".to_owned(),
            parsed: Err(RulesParseError {
                message: "invalid UTF-8".to_owned(),
                line: None,
                column: None,
            }),
        },
    )?;
    let answer = world
        .service
        .get_rules(
            &alice(OperationName::GetRules, None)?,
            get_request(At::Latest)?,
        )
        .await?;
    let unreadable = some(answer.unreadable, "the unreadable file")?;
    assert!(unreadable.text.starts_with('\u{fffd}'));
    assert_eq!(unreadable.problems.len(), 1);
    Ok(())
}

// set_rules (design 7.2).

fn set_request(base: Revision, set: NamingRules) -> Built<SetRulesRequest> {
    Ok(SetRulesRequest {
        workspace_id: scope()?.workspace_id,
        base_revision: base,
        rules: set,
        idempotency_key: key(3),
    })
}

#[tokio::test]
async fn conventions_set_rules_sr1_saves_one_commit_with_a_receipt() -> TestResult {
    let world = World::new()?;
    world.versions.queue_revision(revision('b')?)?;
    let saved = rules(vec![r0()]);
    let result = world
        .service
        .set_rules(
            &alice(OperationName::SetRules, Some(mutation(1)))?,
            set_request(revision('a')?, saved.clone())?,
        )
        .await?;
    assert_eq!(result.revision, revision('b')?);
    let commits = world.versions.commits()?;
    let [commit] = commits.as_slice() else {
        return Err(format!("not one commit: {commits:?}").into());
    };
    let [TreeEdit::SetRules { rules: written }] = commit.edits.as_slice() else {
        return Err(format!("not one SetRules: {:?}", commit.edits).into());
    };
    assert_eq!(serde_json::to_value(written)?, serde_json::to_value(saved)?);
    assert_eq!(commit.message, "Save naming rules");
    let receipts = world.records.receipts()?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts.first().map(|(_, receipt)| receipt.id),
        Some(result.receipt_id)
    );
    Ok(())
}

#[tokio::test]
async fn conventions_set_rules_sr2_a_stale_base_conflicts_and_writes_nothing() -> TestResult {
    let world = World::new()?;
    world.versions.set_head(revision('c')?)?;
    let refused = err_of(
        world
            .service
            .set_rules(
                &alice(OperationName::SetRules, Some(mutation(1)))?,
                set_request(revision('a')?, rules(vec![r0()]))?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert_eq!(field_of(&refused), "/base_revision");
    assert!(matches!(
        refused.detail.as_deref(),
        Some(ErrorDetail::StaleBase { head }) if *head == revision('c')?
    ));
    assert_eq!(world.versions.commits()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn conventions_set_rules_sr3_a_retry_answers_the_first_commit() -> TestResult {
    let world = World::new()?;
    world.versions.queue_revision(revision('b')?)?;
    let context = alice(OperationName::SetRules, Some(mutation(1)))?;
    let first = world
        .service
        .set_rules(&context, set_request(revision('a')?, rules(vec![r0()]))?)
        .await?;
    let mut resumed = context.clone();
    resumed.attempt = Attempt::Resumed;
    let second = world
        .service
        .set_rules(&resumed, set_request(revision('a')?, rules(vec![r0()]))?)
        .await?;
    assert_eq!(first.revision, second.revision);
    assert_eq!(first.receipt_id, second.receipt_id);
    assert_eq!(world.versions.commits()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn conventions_set_rules_sr4_invalid_rules_are_refused_before_any_write() -> TestResult {
    let world = World::new()?;
    let refused = err_of(
        world
            .service
            .set_rules(
                &alice(OperationName::SetRules, Some(mutation(1)))?,
                set_request(
                    revision('a')?,
                    rules(vec![with(|rule| "notes/**x".clone_into(&mut rule.glob))]),
                )?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(field_of(&refused), "/rules/rules/0/glob");
    assert_eq!(world.versions.commits()?.len(), 0);
    assert_eq!(world.records.receipts()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn conventions_set_rules_sr5_saving_over_an_unreadable_file_is_accepted() -> TestResult {
    let world = World::new()?;
    world.versions.put_rules(
        &revision('a')?,
        RulesFile {
            text: "colour: red\n".to_owned(),
            parsed: Err(RulesParseError {
                message: "unknown field".to_owned(),
                line: Some(1),
                column: Some(1),
            }),
        },
    )?;
    world.versions.queue_revision(revision('b')?)?;
    world
        .service
        .set_rules(
            &alice(OperationName::SetRules, Some(mutation(1)))?,
            set_request(revision('a')?, rules(vec![r0()]))?,
        )
        .await?;
    assert_eq!(world.versions.commits()?.len(), 1);
    Ok(())
}

// preview_names (design 7.3).

#[tokio::test]
async fn conventions_preview_pn1_an_empty_items_selection_is_refused() -> TestResult {
    let world = World::new()?;
    let refused = err_of(
        world
            .service
            .preview_names(
                &alice(OperationName::PreviewNames, None)?,
                preview_request(
                    NameSelection::Items {
                        item_ids: Vec::new(),
                    },
                    rules(vec![r0()]),
                )?,
            )
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(field_of(&refused), "/selection/item_ids");
    Ok(())
}

#[tokio::test]
async fn conventions_preview_pn2_pn3_unknown_and_repeated_items_are_refused() -> TestResult {
    let world = World::new()?;
    put(&world, 1, "notes/Plan A.md")?;
    let unknown = err_of(
        world
            .service
            .preview_names(
                &alice(OperationName::PreviewNames, None)?,
                preview_request(
                    NameSelection::Items {
                        item_ids: vec![item(7)],
                    },
                    rules(vec![r0()]),
                )?,
            )
            .await,
    )?;
    assert_eq!(
        (unknown.code.clone(), field_of(&unknown)),
        (ErrorCode::NotFound, "/selection/item_ids/0")
    );
    let repeated = err_of(
        world
            .service
            .preview_names(
                &alice(OperationName::PreviewNames, None)?,
                preview_request(
                    NameSelection::Items {
                        item_ids: vec![item(1), item(1)],
                    },
                    rules(vec![r0()]),
                )?,
            )
            .await,
    )?;
    assert_eq!(
        (repeated.code.clone(), field_of(&repeated)),
        (ErrorCode::InvalidInput, "/selection/item_ids/1")
    );
    Ok(())
}

#[tokio::test]
async fn conventions_preview_pn4_a_folder_selection_and_its_depth() -> TestResult {
    let world = World::new()?;
    put(&world, 1, "notes/A one.md")?;
    put(&world, 2, "notes/sub/B two.md")?;
    put(&world, 3, "other/C three.md")?;
    for (recursive, considered) in [(false, 1), (true, 2)] {
        let plan = preview(
            &world,
            NameSelection::Folder {
                folder: path("notes")?,
                recursive,
            },
            rules(vec![r0()]),
        )
        .await?;
        assert_eq!(plan.considered, considered, "recursive {recursive}");
        assert_eq!(plan.entries.len(), usize::try_from(considered)?);
    }
    Ok(())
}

#[tokio::test]
async fn conventions_preview_pn5_a_folder_the_revision_does_not_hold_is_not_found() -> TestResult {
    let world = World::new()?;
    put(&world, 1, "notes/A one.md")?;
    let refused = err_of(
        world
            .service
            .preview_names(
                &alice(OperationName::PreviewNames, None)?,
                preview_request(
                    NameSelection::Folder {
                        folder: path("nowhere")?,
                        recursive: true,
                    },
                    rules(vec![r0()]),
                )?,
            )
            .await,
    )?;
    assert_eq!(
        (refused.code.clone(), field_of(&refused)),
        (ErrorCode::NotFound, "/selection/folder")
    );
    Ok(())
}

#[tokio::test]
async fn conventions_preview_pn6_archived_items_are_left_out_and_counted() -> TestResult {
    let world = World::new()?;
    let head = revision('a')?;
    put(&world, 1, "A one.md")?;
    put(&world, 2, "b-two.md")?;
    world.versions.put_document(
        &head,
        archived(note(3, &head, "C Three.md", "", json!({ "type": "Note" }))?),
    )?;
    let plan = preview(&world, NameSelection::Workspace, rules(vec![r0()])).await?;
    assert_eq!(
        (
            plan.considered,
            plan.unchanged,
            plan.refused,
            plan.archived_left_out
        ),
        (2, 1, 0, 1)
    );
    assert_eq!(plan.entries.len(), 1);
    let entries = u32::try_from(plan.entries.len())?;
    let sum = entries
        .checked_add(plan.unchanged)
        .and_then(|sum| sum.checked_add(plan.refused));
    assert_eq!(Some(plan.considered), sum);
    Ok(())
}

#[tokio::test]
async fn conventions_preview_pn7_invalid_rules_are_refused_before_any_read() -> TestResult {
    let world = World::new()?;
    let refused = err_of(
        world
            .service
            .preview_names(
                &alice(OperationName::PreviewNames, None)?,
                preview_request(
                    NameSelection::Workspace,
                    rules(vec![with(|rule| "notes/**x".clone_into(&mut rule.glob))]),
                )?,
            )
            .await,
    )?;
    assert_eq!(field_of(&refused), "/rules/rules/0/glob");
    assert_eq!(world.versions.trees(), 0);
    Ok(())
}

#[tokio::test]
async fn conventions_preview_pn8_a_preview_writes_and_records_nothing() -> TestResult {
    let world = World::new()?;
    put(&world, 1, "A one.md")?;
    let plan = preview(&world, NameSelection::Workspace, rules(vec![r0()])).await?;
    assert_eq!(plan.base_revision, revision('a')?);
    assert_eq!(plan.rules_digest, request_digest(&rules(vec![r0()]))?);
    assert_eq!(world.versions.commits()?.len(), 0);
    assert_eq!(world.events.appended()?.len(), 0);
    assert_eq!(world.records.receipts()?.len(), 0);
    assert_eq!(world.versions.trees(), 1);
    Ok(())
}

#[tokio::test]
async fn conventions_preview_pn9_a_purged_revision_is_not_found() -> TestResult {
    let world = World::new()?;
    world.records.put_mapping(
        revision('c')?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(5)),
            replacement: None,
        },
    )?;
    let mut request = preview_request(NameSelection::Workspace, rules(vec![r0()]))?;
    request.at = At::Revision {
        revision: revision('c')?,
    };
    let refused = err_of(
        world
            .service
            .preview_names(&alice(OperationName::PreviewNames, None)?, request)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(world.versions.trees(), 0);
    Ok(())
}

#[tokio::test]
async fn conventions_preview_refused_names_are_counted_apart_and_named() -> TestResult {
    let world = World::new()?;
    put(&world, 1, "Minutes 03-04-2025.md")?;
    put(&world, 2, "plan-a.md")?;
    put(&world, 3, "Plan B.md")?;
    let set = rules(vec![with(|rule| {
        rule.date_order = DateOrder::RejectAmbiguous;
        "[year]-[month]-[day]".clone_into(&mut rule.date_format);
    })]);
    let plan = preview(&world, NameSelection::Workspace, set).await?;
    assert_eq!(
        (plan.entries.len(), plan.unchanged, plan.refused),
        (1, 1, 1)
    );
    let codes: Vec<(&str, Option<&str>)> = plan
        .warnings
        .iter()
        .map(|warning| (warning.code.as_str(), warning.location.as_deref()))
        .collect();
    assert_eq!(codes, [(DATE_AMBIGUOUS, Some("Minutes 03-04-2025.md"))]);
    Ok(())
}

#[tokio::test]
async fn conventions_preview_a_source_cards_original_name_reaches_the_rules() -> TestResult {
    let world = World::new()?;
    let head = revision('a')?;
    let mut card = note(
        1,
        &head,
        "inbox/Q3 Report-PDF.md",
        "",
        json!({ "type": "source" }),
    )?;
    card.summary.type_name = "source".to_owned();
    card.properties.insert(
        "okf_jawn".to_owned(),
        json!({
            "item_id": Uuid::from_u128(1).to_string(),
            "kind": "source",
            "source": { "original_name": "Q3 Report.PDF", "digest": "a".repeat(64) }
        }),
    );
    world.versions.put_document(&head, card)?;
    let set = rules(vec![with(|rule| {
        rule.extension_policy = ExtensionPolicy::Strip;
    })]);
    let plan = preview(&world, NameSelection::Workspace, set).await?;
    let to: Vec<&str> = plan.entries.iter().map(|entry| entry.to.as_str()).collect();
    assert_eq!(to, ["inbox/q3-report.md"]);
    Ok(())
}

// apply_names (design 7.4).

/// A world holding `paths` (items numbered from 1) and the plan of all of them under `set`.
async fn previewed(paths: &[&str], set: &NamingRules) -> Built<(World, RenamePlan)> {
    let world = World::new()?;
    for (number, at) in (1_u128..).zip(paths) {
        put(&world, number, at)?;
    }
    let plan = preview(&world, NameSelection::Workspace, set.clone()).await?;
    world.versions.queue_revision(revision('b')?)?;
    Ok((world, plan))
}

#[tokio::test]
async fn conventions_apply_an1_a_moved_head_conflicts_on_the_plans_base() -> TestResult {
    let set = rules(vec![r0()]);
    let (world, plan) = previewed(&["Plan A.md"], &set).await?;
    world.versions.set_head(revision('c')?)?;
    let refused = err_of(apply(&world, plan, set).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert_eq!(field_of(&refused), "/plan/base_revision");
    assert!(matches!(
        refused.detail.as_deref(),
        Some(ErrorDetail::StaleBase { head }) if *head == revision('c')?
    ));
    assert_eq!(world.versions.commits()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn conventions_apply_a_head_that_moves_before_the_commit_names_the_plans_base() -> TestResult
{
    let set = rules(vec![r0()]);
    let (world, plan) = previewed(&["Plan A.md"], &set).await?;
    world
        .versions
        .land_before_next_commits(vec![revision('c')?])?;
    let refused = err_of(apply(&world, plan, set).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert_eq!(field_of(&refused), "/plan/base_revision");
    Ok(())
}

#[tokio::test]
async fn conventions_apply_an2_other_rules_than_the_previews_are_refused() -> TestResult {
    let set = rules(vec![r0()]);
    let (world, plan) = previewed(&["Plan A.md"], &set).await?;
    let other = rules(vec![with(|rule| {
        rule.case = LetterCase::Lower;
        "_".clone_into(&mut rule.separator);
    })]);
    let refused = err_of(apply(&world, plan, other).await)?;
    assert_eq!(
        (refused.code.clone(), field_of(&refused)),
        (ErrorCode::InvalidInput, "/rules")
    );
    assert_eq!(world.versions.commits()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn conventions_apply_an3_an_edited_entry_is_refused_naming_its_field() -> TestResult {
    let set = rules(vec![r0()]);
    for (field, edit) in [("to", 0_u8), ("from", 1), ("item_id", 2)] {
        let (world, mut plan) = previewed(&["Plan A.md"], &set).await?;
        let entry = some(plan.entries.first_mut(), "an entry")?;
        match edit {
            0 => entry.to = path("elsewhere.md")?,
            1 => entry.from = path("Other.md")?,
            _ => entry.item_id = item(9),
        }
        let refused = err_of(apply(&world, plan, set.clone()).await)?;
        assert_eq!(
            (refused.code.clone(), field_of(&refused)),
            (
                ErrorCode::InvalidInput,
                format!("/plan/entries/0/{field}").as_str()
            )
        );
        assert_eq!(world.versions.commits()?.len(), 0);
    }
    Ok(())
}

#[tokio::test]
async fn conventions_apply_an4_an5_an_empty_plan_and_an_item_twice_are_refused() -> TestResult {
    let set = rules(vec![r0()]);
    let (world, plan) = previewed(&["Plan A.md"], &set).await?;
    let mut empty = plan.clone();
    empty.entries.clear();
    let refused = err_of(apply(&world, empty, set.clone()).await)?;
    assert_eq!(
        (refused.code.clone(), field_of(&refused)),
        (ErrorCode::InvalidInput, "/plan/entries")
    );
    let mut twice = plan;
    let entry = some(twice.entries.first(), "an entry")?.clone();
    twice.entries.push(entry);
    let refused = err_of(apply(&world, twice, set).await)?;
    assert_eq!(
        (refused.code.clone(), field_of(&refused)),
        (ErrorCode::InvalidInput, "/plan/entries/1/item_id")
    );
    assert_eq!(world.versions.commits()?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn conventions_apply_an6_one_commit_its_referrers_told_and_announced() -> TestResult {
    let set = rules(vec![r0()]);
    let (world, plan) = previewed(&["notes/Plan_A.md", "notes/b.md"], &set).await?;
    world
        .versions
        .put_referrer(item(1), path("notes/referrer.md")?)?;
    world
        .versions
        .stage()
        .write("notes/referrer.md", "---\ntype: Note\n---\n\nNo heading.\n")?;
    let result = apply(&world, plan, set).await?;
    assert_eq!(result.revision, revision('b')?);
    let commits = world.versions.commits()?;
    let [commit] = commits.as_slice() else {
        return Err(format!("not one commit: {commits:?}").into());
    };
    assert_eq!(
        moved(&commit.edits),
        [(item(1), "notes/plan-a.md".to_owned(), false)]
    );
    assert_eq!(commit.message, "Apply naming rules to 1 items");
    // The store named the referrer to the check, which reports lint about it.
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.location.as_deref() == Some("notes/referrer.md")),
        "{:?}",
        result.warnings
    );
    let receipts = world.records.receipts()?;
    assert_eq!(
        receipts.first().map(|(_, receipt)| receipt.id),
        Some(result.receipt_id)
    );
    let events = world.events.appended()?;
    let [(_, Some(announced), event)] = events.as_slice() else {
        return Err(format!("not one event: {events:?}").into());
    };
    assert_eq!(*announced, mutation(1));
    assert_eq!(event.kind, EventKind::Changed);
    assert_eq!(event.item_id, None);
    // The touched items are not read again: the plan holds their paths.
    assert_eq!(world.versions.shows(), 0);
    Ok(())
}

#[tokio::test]
async fn conventions_apply_an7_a_case_only_rename_is_one_direct_move() -> TestResult {
    let set = rules(vec![r0()]);
    let (world, plan) = previewed(&["a/Report.md"], &set).await?;
    apply(&world, plan, set).await?;
    let commits = world.versions.commits()?;
    let commit = some(commits.first(), "the commit")?;
    assert_eq!(
        moved(&commit.edits),
        [(item(1), "a/report.md".to_owned(), false)]
    );
    let shown = world
        .versions
        .show(&scope()?, &revision('b')?, item(1))
        .await?;
    assert_eq!(shown.summary.path.as_str(), "a/report.md");
    Ok(())
}

fn routed(from: &str, to: &str, keep_alias: bool) -> NamingRule {
    with(|rule| {
        rule.glob = format!("{from}/*.md");
        rule.destination = Some(to.to_owned());
        rule.alias_preservation = keep_alias;
    })
}

#[tokio::test]
async fn conventions_apply_an8_a_chain_and_a_swap_go_through_temporary_paths() -> TestResult {
    // A chain: B frees `b/x.md` by moving first, straight to `c/x.md`; A waits for it.
    let set = rules(vec![routed("a", "b", true), routed("b", "c", true)]);
    let (world, plan) = previewed(&["a/x.md", "b/x.md"], &set).await?;
    apply(&world, plan, set).await?;
    let commits = world.versions.commits()?;
    let edits = moved(&some(commits.first(), "the chain's commit")?.edits);
    let temporary_a = format!("a/okf-jawn-renaming-{}.md", Uuid::from_u128(1));
    assert_eq!(
        edits,
        [
            (item(2), "c/x.md".to_owned(), true),
            (item(1), temporary_a.clone(), true),
            (item(1), "b/x.md".to_owned(), false),
        ]
    );
    for (number, at) in [(1, "b/x.md"), (2, "c/x.md")] {
        let shown = world
            .versions
            .show(&scope()?, &revision('b')?, item(number))
            .await?;
        assert_eq!(shown.summary.path.as_str(), at);
    }
    // A swap: both wait, so both go through their temporary paths.
    let set = rules(vec![routed("a", "b", false), routed("b", "a", false)]);
    let (world, plan) = previewed(&["a/x.md", "b/x.md"], &set).await?;
    apply(&world, plan, set).await?;
    let commits = world.versions.commits()?;
    let edits = moved(&some(commits.first(), "the swap's commit")?.edits);
    let temporary_b = format!("b/okf-jawn-renaming-{}.md", Uuid::from_u128(2));
    assert_eq!(
        edits,
        [
            (item(1), temporary_a, false),
            (item(2), temporary_b, false),
            (item(1), "b/x.md".to_owned(), false),
            (item(2), "a/x.md".to_owned(), false),
        ]
    );
    for (number, at) in [(1, "b/x.md"), (2, "a/x.md")] {
        let shown = world
            .versions
            .show(&scope()?, &revision('b')?, item(number))
            .await?;
        assert_eq!(shown.summary.path.as_str(), at);
    }
    Ok(())
}

#[tokio::test]
async fn conventions_apply_an9_a_resumed_attempt_answers_the_landed_commit() -> TestResult {
    let set = rules(vec![r0()]);
    let (world, plan) = previewed(&["Plan A.md"], &set).await?;
    let first = apply(&world, plan.clone(), set.clone()).await?;
    // The head is now the landed commit, not the plan's base: a resumed attempt skips the head
    // check and the store replays.
    let mut resumed = alice(OperationName::ApplyNames, Some(mutation(1)))?;
    resumed.attempt = Attempt::Resumed;
    let second = world
        .service
        .apply_names(&resumed, apply_request(plan.clone(), set.clone())?)
        .await?;
    assert_eq!(first.revision, second.revision);
    assert_eq!(first.receipt_id, second.receipt_id);
    assert_eq!(world.versions.commits()?.len(), 1);
    // A first attempt now finds the head moved.
    let refused = err_of(apply(&world, plan, set).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    Ok(())
}

#[tokio::test]
async fn conventions_apply_an10_more_than_1000_entries_is_too_large() -> TestResult {
    let set = rules(vec![r0()]);
    let (world, mut plan) = previewed(&["Plan A.md"], &set).await?;
    let entry = some(plan.entries.first(), "an entry")?.clone();
    plan.entries = (0..1001_u128)
        .map(|number| {
            let mut copy = entry.clone();
            copy.item_id = item(number.saturating_add(1000));
            copy
        })
        .collect();
    let refused = err_of(apply(&world, plan.clone(), set.clone()).await)?;
    assert_eq!(
        (refused.code.clone(), field_of(&refused)),
        (ErrorCode::TooLarge, "/plan/entries")
    );
    // At the bound the plan is checked further (and refused: these items are not the rules').
    plan.entries.truncate(1000);
    let checked = err_of(apply(&world, plan, set).await)?;
    assert_eq!(checked.code, ErrorCode::InvalidInput);
    Ok(())
}

#[tokio::test]
async fn conventions_apply_a_numbered_plan_applies_as_previewed() -> TestResult {
    let set = rules(vec![with(|rule| rule.strip_version_suffix = true)]);
    let (world, plan) = previewed(&["a/Report v2.md", "a/Report (1).md"], &set).await?;
    let suffixed = plan
        .entries
        .iter()
        .find(|entry| entry.to.as_str() == "a/report-2.md")
        .map(|entry| entry.warnings.iter().any(|w| w.code == COLLISION_SUFFIXED));
    assert_eq!(suffixed, Some(true));
    apply(&world, plan, set).await?;
    let commits = world.versions.commits()?;
    let edits = moved(&some(commits.first(), "the commit")?.edits);
    let targets: BTreeSet<String> = edits.into_iter().map(|(_, to, _)| to).collect();
    assert_eq!(
        targets,
        BTreeSet::from(["a/report.md".to_owned(), "a/report-2.md".to_owned()])
    );
    Ok(())
}

#[tokio::test]
async fn conventions_apply_an_alias_is_kept_only_from_the_base_path() -> TestResult {
    let set = rules(vec![with(|rule| rule.alias_preservation = true)]);
    let (world, plan) = previewed(&["notes/Old Name.md"], &set).await?;
    apply(&world, plan, set).await?;
    let commits = world.versions.commits()?;
    assert_eq!(
        moved(&some(commits.first(), "the commit")?.edits),
        [(item(1), "notes/old-name.md".to_owned(), true)]
    );
    Ok(())
}

#[tokio::test]
async fn conventions_apply_a_purged_base_is_not_found() -> TestResult {
    let set = rules(vec![r0()]);
    let (world, plan) = previewed(&["Plan A.md"], &set).await?;
    world.records.put_mapping(
        revision('a')?,
        RevisionMapping {
            purge_id: PurgeId(Uuid::from_u128(5)),
            replacement: None,
        },
    )?;
    let refused = err_of(apply(&world, plan, set).await)?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(world.versions.commits()?.len(), 0);
    Ok(())
}
