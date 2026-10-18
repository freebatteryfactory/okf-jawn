//! The naming rules' four operations (SPEC section 6) over the rules engine
//! (`crate::conventions`), which names every path; nothing here names one.
//!
//! - `get_rules` reads `.okf/rules.yaml` at the resolved revision (`VersionStore::rules`):
//!   none saved, the rules, or, for a file that does not read as rules this build can apply, the
//!   file's text and every problem (a YAML error at its line and column, a refused field at its
//!   JSON Pointer), never an error, so a person can repair it. It records a read receipt with no
//!   sources: it is the agent tool `rules`, whose results are recorded as every tool's are.
//! - `set_rules` checks the rules (`CompiledRules::compile`) and saves them in one commit
//!   (`TreeEdit::SetRules`), through the commit path every write takes.
//! - `preview_names` writes nothing and records nothing: the selection is resolved and named
//!   against the whole tree at the resolved revision (`VersionStore::tree`, one pass), since a
//!   destination can name any folder and a collision is with any item there.
//! - `apply_names` revalidates the plan before any write (SPEC section 6: "A stale preview must
//!   not silently apply"): the rules must be the ones the plan was previewed with (their
//!   digest), the base must be the head on a first attempt, and the plan's entries are computed
//!   again at its base with the request's rules for the entries' items, and must be the same,
//!   in order. Then one commit of `MoveItem`s, with the links each move rewrites (the store
//!   tells the check the referrers). An entry whose new path another entry holds at the base (a
//!   chain, a swap) goes through a temporary path, `okf-jawn-renaming-<item id>.md` beside it,
//!   which no committed tree holds; a rename that changes only letter case is one direct move
//!   (storage takes it through a parked name of its own). A resumed attempt skips the head
//!   check: its commit may have landed, and the store replays it.

use std::collections::{BTreeMap, BTreeSet};

use okf_jawn_contract::common::{MutationResult, Warning};
use okf_jawn_contract::conventions::{
    ApplyNamesRequest, GetRulesRequest, GetRulesResponse, NameSelection, PreviewNamesRequest,
    RenamePlan, SetRulesRequest, UnreadableRules,
};
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{ItemId, WorkspacePath};
use okf_jawn_contract::item::MAX_ITEM_TEXT_BYTES;

use super::ApplicationService;
use super::shared::{
    Touched, Write, check_named, commit_with_receipt, invalid, record_receipt, resolve,
    workspace_scope,
};
use crate::context::{Attempt, OperationContext};
use crate::conventions::{
    CompiledRules, MAX_APPLY_ENTRIES, PlannedEntry, RULES_YAML_INVALID, numbered, plan,
};
use crate::mutations::request_digest;
use crate::storage::{RulesFile, TreeEdit, WorkspaceTree};

/// The request field of a plan's base.
const PLAN_BASE: &str = "/plan/base_revision";
/// The request field of a plan's entries.
const PLAN_ENTRIES: &str = "/plan/entries";

/// The naming rules at one resolved revision, or the stored file that cannot be read as them.
///
/// # Errors
/// Returns the typed `NotFound` of an invalidated revision, `TooLarge` for a stored file over
/// `item::MAX_ITEM_TEXT_BYTES`, or any port error.
pub(super) async fn get_rules(
    service: &ApplicationService,
    context: &OperationContext,
    request: GetRulesRequest,
) -> Result<GetRulesResponse, ApiError> {
    let scope = workspace_scope(context)?;
    let revision = resolve(service, &scope, &request.at).await?;
    let stored = service.ports().versions.rules(&scope, &revision).await?;
    let (rules, unreadable) = match stored {
        None => (None, None),
        Some(file) => read_rules(file)?,
    };
    record_receipt(service, context, &scope, None, Vec::new()).await?;
    Ok(GetRulesResponse {
        revision,
        rules,
        unreadable,
    })
}

/// Save rules as `.okf/rules.yaml` in one commit on the request's base.
///
/// # Errors
/// Returns the rules' refusal (`InvalidInput` or `TooLarge` on its field under `/rules`) before
/// anything is read or written, or any error of the commit (`Conflict` with `StaleBase` on
/// `/base_revision` for a base that is not the head).
pub(super) async fn set_rules(
    service: &ApplicationService,
    context: &OperationContext,
    request: SetRulesRequest,
) -> Result<MutationResult, ApiError> {
    CompiledRules::compile(&request.rules, "/rules")?;
    commit_with_receipt(
        service,
        context,
        Write {
            base: request.base_revision,
            message: "Save naming rules".to_owned(),
            edits: vec![TreeEdit::SetRules {
                rules: request.rules,
            }],
            item: None,
            touched: Touched::default(),
        },
    )
    .await
}

/// The rename plan of a selection at one resolved revision; nothing is written.
///
/// # Errors
/// Returns the rules' refusal before any read; `InvalidInput` on `/selection/item_ids` for an
/// empty selection and on `/selection/item_ids/<i>` for an item listed twice; `NotFound` on
/// `/selection/item_ids/<i>` for an item the revision does not hold and on `/selection/folder`
/// for a folder it does not hold; the typed `NotFound` of an invalidated revision; or any port
/// error.
pub(super) async fn preview_names(
    service: &ApplicationService,
    context: &OperationContext,
    request: PreviewNamesRequest,
) -> Result<RenamePlan, ApiError> {
    let rules = CompiledRules::compile(&request.rules, "/rules")?;
    request.selection.check("/selection")?;
    let scope = workspace_scope(context)?;
    let base = resolve(service, &scope, &request.at).await?;
    let tree = service.ports().versions.tree(&scope, &base).await?;
    let selected = selected(&tree, &request.selection)?;
    let body = plan(&rules, &tree.items, &tree.folders, &selected)?;
    Ok(RenamePlan {
        base_revision: base,
        rules_digest: request_digest(&request.rules)?,
        entries: body
            .entries
            .into_iter()
            .map(|planned| planned.entry)
            .collect(),
        duplicate_observations: body.duplicate_observations,
        warnings: body.warnings,
        considered: body.considered,
        unchanged: body.unchanged,
        refused: body.refused,
        archived_left_out: body.archived_left_out,
    })
}

/// Apply a previewed plan in one commit of moves, after revalidating it (the module header).
///
/// # Errors
/// Returns the rules' refusal; `InvalidInput` on `/rules` for rules whose digest is not the
/// plan's, on `/plan/entries` for a plan with no entries, on `/plan/entries/<i>/item_id` for an
/// item listed twice, and on `/plan/entries/<i>/<field>` for the first entry that is not what
/// the rules give at the plan's base; `TooLarge` on `/plan/entries` past `MAX_APPLY_ENTRIES`;
/// the typed `NotFound` of an invalidated base; on a first attempt, `Conflict` with `StaleBase`
/// on `/plan/base_revision` when the base is not the head (also when the head moves before the
/// commit lands); or any error of the commit.
pub(super) async fn apply_names(
    service: &ApplicationService,
    context: &OperationContext,
    request: ApplyNamesRequest,
) -> Result<MutationResult, ApiError> {
    let rules = CompiledRules::compile(&request.rules, "/rules")?;
    let plan_body = request.plan;
    if request_digest(&request.rules)? != plan_body.rules_digest {
        return Err(invalid(
            "these are not the rules the plan was previewed with; preview again",
            "/rules",
        ));
    }
    let entries = plan_body.entries;
    if entries.is_empty() {
        return Err(invalid(
            "a plan with no entries renames nothing",
            PLAN_ENTRIES,
        ));
    }
    if entries.len() > MAX_APPLY_ENTRIES {
        return Err(ApiError::new(
            ErrorCode::TooLarge,
            format!(
                "one apply renames at most {MAX_APPLY_ENTRIES} items; apply the plan folder by \
                 folder"
            ),
        )
        .with_field(PLAN_ENTRIES));
    }
    let mut listed = BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        if !listed.insert(entry.item_id) {
            return Err(invalid(
                "the plan lists this item twice",
                &format!("{PLAN_ENTRIES}/{index}/item_id"),
            ));
        }
    }
    let scope = workspace_scope(context)?;
    let base = plan_body.base_revision;
    check_named(service, &scope, &base).await?;
    let versions = &service.ports().versions;
    if context.attempt == Attempt::First {
        let head = versions.head(&scope).await?;
        if head != base {
            return Err(stale(head));
        }
    }
    let tree = versions.tree(&scope, &base).await?;
    let again = plan(&rules, &tree.items, &tree.folders, &listed)?;
    for (index, entry) in entries.iter().enumerate() {
        let Some(computed) = again.entries.get(index).map(|planned| &planned.entry) else {
            return Err(not_previewed(index, "item_id"));
        };
        if computed.item_id != entry.item_id {
            return Err(not_previewed(index, "item_id"));
        }
        if computed.from != entry.from {
            return Err(not_previewed(index, "from"));
        }
        if computed.to != entry.to {
            return Err(not_previewed(index, "to"));
        }
    }
    let mut paths: Vec<WorkspacePath> = Vec::new();
    for entry in &entries {
        paths.push(entry.from.clone());
        paths.push(entry.to.clone());
    }
    let edits = moves(&again.entries, &tree)?;
    commit_with_receipt(
        service,
        context,
        Write {
            base,
            message: format!("Apply naming rules to {} items", entries.len()),
            edits,
            item: None,
            // Every old and new path. The items stay unlisted: the plan holds their paths, and
            // listing them would read each one again (`shared::store_commit`).
            touched: Touched {
                paths,
                ..Touched::default()
            },
        },
    )
    .await
    .map_err(on_the_plan)
}

/// `get_rules`' answer for a stored file: the rules, or the file's text and its problems.
///
/// # Errors
/// Returns `TooLarge` for a file over `item::MAX_ITEM_TEXT_BYTES`, or `Internal` when a pattern
/// of the rules engine does not compile.
fn read_rules(
    file: RulesFile,
) -> Result<
    (
        Option<okf_jawn_contract::conventions::NamingRules>,
        Option<UnreadableRules>,
    ),
    ApiError,
> {
    if file.text.len() > MAX_ITEM_TEXT_BYTES {
        return Err(ApiError::new(
            ErrorCode::TooLarge,
            format!(
                "the naming rules file is larger than {MAX_ITEM_TEXT_BYTES} bytes; save rules to \
                 replace it"
            ),
        ));
    }
    let problems = match file.parsed {
        Err(error) => vec![Warning {
            code: RULES_YAML_INVALID.to_owned(),
            message: error.message,
            location: match (error.line, error.column) {
                (Some(line), Some(column)) => Some(format!("line {line}, column {column}")),
                (Some(line), None) => Some(format!("line {line}")),
                (None, _) => None,
            },
        }],
        Ok(rules) => {
            let problems = CompiledRules::problems(&rules)?;
            if problems.is_empty() {
                return Ok((Some(rules), None));
            }
            problems
        }
    };
    Ok((
        None,
        Some(UnreadableRules {
            text: file.text,
            problems,
        }),
    ))
}

/// The items a selection names in `tree`.
///
/// # Errors
/// As `preview_names` says of the selection.
fn selected(tree: &WorkspaceTree, selection: &NameSelection) -> Result<BTreeSet<ItemId>, ApiError> {
    match selection {
        NameSelection::Items { item_ids } => {
            let held: BTreeSet<ItemId> = tree.items.iter().map(|item| item.summary.id).collect();
            let mut chosen = BTreeSet::new();
            for (index, item) in item_ids.iter().enumerate() {
                let field = format!("/selection/item_ids/{index}");
                if !held.contains(item) {
                    return Err(ApiError::new(
                        ErrorCode::NotFound,
                        "no item with that identity at this revision",
                    )
                    .with_field(field));
                }
                if !chosen.insert(*item) {
                    return Err(invalid("the selection lists this item twice", &field));
                }
            }
            Ok(chosen)
        }
        NameSelection::Folder { folder, recursive } => {
            if !tree.folders.contains(folder) {
                return Err(ApiError::new(
                    ErrorCode::NotFound,
                    "no folder at that path at this revision",
                )
                .with_field("/selection/folder"));
            }
            let prefix = format!("{}/", folder.as_str());
            Ok(tree
                .items
                .iter()
                .filter(|item| {
                    item.summary
                        .path
                        .as_str()
                        .strip_prefix(&prefix)
                        .is_some_and(|rest| *recursive || !rest.contains('/'))
                })
                .map(|item| item.summary.id)
                .collect())
        }
        NameSelection::Workspace => Ok(tree.items.iter().map(|item| item.summary.id).collect()),
    }
}

/// The moves that apply `entries` at `tree`'s revision (the module header): first every entry
/// whose new path no other item holds at the base, straight to it; then every other entry to
/// its temporary path; then those from there to their new paths. Only a move from the item's
/// path at the base records an alias.
///
/// # Errors
/// Returns `Internal` when no temporary path is a free workspace path.
fn moves(entries: &[PlannedEntry], tree: &WorkspaceTree) -> Result<Vec<TreeEdit>, ApiError> {
    let holders: BTreeMap<String, ItemId> = tree
        .items
        .iter()
        .map(|item| (item.summary.path.collision_key(), item.summary.id))
        .collect();
    let mut taken: BTreeSet<String> = holders.keys().cloned().collect();
    taken.extend(tree.folders.iter().map(WorkspacePath::collision_key));
    taken.extend(
        entries
            .iter()
            .map(|planned| planned.entry.to.collision_key()),
    );
    let mut direct = Vec::new();
    let mut away = Vec::new();
    let mut back = Vec::new();
    for planned in entries {
        let entry = &planned.entry;
        let held_by_another = holders
            .get(&entry.to.collision_key())
            .is_some_and(|holder| *holder != entry.item_id);
        if held_by_another {
            away.push(TreeEdit::MoveItem {
                item_id: entry.item_id,
                destination: temporary(&entry.from, entry.item_id, &mut taken)?,
                keep_alias: planned.keep_alias,
            });
            back.push(TreeEdit::MoveItem {
                item_id: entry.item_id,
                destination: entry.to.clone(),
                keep_alias: false,
            });
        } else {
            direct.push(TreeEdit::MoveItem {
                item_id: entry.item_id,
                destination: entry.to.clone(),
                keep_alias: planned.keep_alias,
            });
        }
    }
    direct.extend(away);
    direct.extend(back);
    Ok(direct)
}

/// The temporary path of an entry's item: `okf-jawn-renaming-<item id>.md` beside `from` (at
/// the root when that is no workspace path), numbered when an item, a folder or an entry's new
/// path holds its key; its key is then taken.
///
/// # Errors
/// Returns `Internal` when no such path is free.
fn temporary(
    from: &WorkspacePath,
    item: ItemId,
    taken: &mut BTreeSet<String>,
) -> Result<WorkspacePath, ApiError> {
    let name = format!("okf-jawn-renaming-{}.md", item.0);
    let beside = match from.as_str().rsplit_once('/') {
        Some((folder, _)) => format!("{folder}/{name}"),
        None => name.clone(),
    };
    let path = WorkspacePath::try_from(beside)
        .or_else(|_| WorkspacePath::try_from(name))
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.0))?;
    if taken.insert(path.collision_key()) {
        return Ok(path);
    }
    let last = u32::try_from(taken.len()).map_or(u32::MAX, |count| count.saturating_add(2));
    for number in 2..=last {
        if let Ok(candidate) = numbered(&path, number)
            && taken.insert(candidate.collision_key())
        {
            return Ok(candidate);
        }
    }
    Err(ApiError::new(
        ErrorCode::Internal,
        format!("no temporary path beside {} is free", from.as_str()),
    ))
}

/// The refusal of a plan whose base is not the head.
fn stale(head: okf_jawn_contract::identity::Revision) -> ApiError {
    ApiError::new(
        ErrorCode::Conflict,
        "the workspace changed since the preview; preview again",
    )
    .with_detail(ErrorDetail::StaleBase { head })
    .with_field(PLAN_BASE)
}

/// The refusal of the plan's entry `index`, whose `field` is not what the rules give.
fn not_previewed(index: usize, field: &str) -> ApiError {
    invalid(
        "the plan is not what these rules give at its base; preview again",
        &format!("{PLAN_ENTRIES}/{index}/{field}"),
    )
}

/// Name the plan's base on the commit's refusal of a base that is not the head: this request's
/// base is `/plan/base_revision`, not the `/base_revision` the commit path names.
fn on_the_plan(error: ApiError) -> ApiError {
    if matches!(error.detail.as_deref(), Some(ErrorDetail::StaleBase { .. })) {
        error.with_field(PLAN_BASE)
    } else {
        error
    }
}
