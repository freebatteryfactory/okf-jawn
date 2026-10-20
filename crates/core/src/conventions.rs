//! The naming rules engine (SPEC section 6): which rule names an item, the name it builds, and
//! the rename plan of a selection. Pure functions over the rules and a tree; no port. The four
//! operations (`application::conventions`) and ingest's import name through it, so a preview
//! and an import name a path alike.
//!
//! A rule set is checked and compiled once (`CompiledRules::compile`): each glob, separator,
//! prefix, suffix, date format and destination is refused on its field, a JSON Pointer, before
//! anything is named. Then one item is named (`propose`) in this order:
//!
//! 1. The first rule in list order whose glob matches the item's whole workspace path and whose
//!    `type_name` is absent or the item's OKF type names it; no later rule is tried. A glob is
//!    the `glob` crate's shell pattern with `*`, `?` and `[...]` never crossing `/`, matched on
//!    collision keys (the glob's and the path's, `identity::collision_key_of`), so two paths
//!    that collide are matched alike, whatever their letter case or normalization.
//! 2. The file name is split into its folder, its stem before `.md`, and a source card's
//!    `-<ext>` or `-<ext>-<n>` tail (the shape `conventions::source_card_name` and
//!    `numbered_source_card_name` write); the base is what remains.
//! 3. An existing prefix and suffix come off the base (compared in lower case), so a rule never
//!    adds them twice; version suffixes are stripped when the rule says so (`_v2`, ` - Copy (2)`,
//!    never `draft` or a bare `-2`); dates are found: a year-first, year-last or compact date is
//!    one protected word, read by `date_order`, rewritten by `date_format` when it is read, and
//!    an ambiguous one refuses the item's name, never guessed (`DATE_AMBIGUOUS`).
//! 4. The other text is split into words on space, `-` and `_` only (`convert_case::split`; no
//!    camelCase or digit split), cased (`case`), joined with `separator`, and the prefix, the
//!    suffix, the card's tail (`extension_policy`) and `.md` are added.
//! 5. The folder is the item's own, or the rule's `destination` written with the item's first
//!    date. The new path must be a `WorkspacePath` holding no reserved segment (`.okf`,
//!    `index.md`, `log.md`).
//!
//! The plan of a selection (`plan`) resolves collisions against the final state of the tree:
//! `suffix` numbers later claimants (`numbered`), `keep_existing` leaves them as they are, and
//! `reject` gives a contested name to nobody; each contested key is one `DuplicateObservation`.
//! Its lists come out sorted, so the same rules and tree give a byte-identical plan. An import
//! names each new card with `name_new_item`, through the same `propose` and `numbered`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use convert_case::{Boundary, Pattern as Words};
use glob::{MatchOptions, Pattern};
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::conventions::{
    CollisionPolicy, DateOrder, DuplicateObservation, ExtensionPolicy, LetterCase, NamingRule,
    NamingRules, RenameEntry, fit_segment, numbered_source_card_name,
};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{
    IdentityError, ItemId, MAX_WORKSPACE_PATH_BYTES, WorkspacePath, collision_key_of,
};
use okf_jawn_contract::item::ItemSummary;
use regex::{Captures, Regex};
use time::format_description::FormatDescriptionV3;
use time::{Date, Month};

use crate::storage::TreeItem;

/// A rule set checked and compiled once: its globs, date formats and destinations.
#[derive(Debug)]
pub struct CompiledRules {
    rules: Vec<CompiledRule>,
    patterns: &'static Patterns,
}

/// One item as the rules see it.
#[derive(Debug, Clone, Copy)]
pub struct Subject<'a> {
    /// Its path now, or a new item's proposed path.
    pub path: &'a WorkspacePath,
    /// Its OKF type (`ItemSummary::type_name`).
    pub type_name: &'a str,
    /// A source card's original file name (`items::SourceHeader::original_name`); `None` for
    /// every other item.
    pub original_name: Option<&'a str>,
}

/// What the rules make of one path before collisions are resolved.
#[derive(Debug, Clone)]
pub enum Proposal {
    /// No rule matches the item.
    Unmatched,
    /// The rule that names the item leaves its path as it is.
    Unchanged {
        /// Findings about the item, located at its path.
        warnings: Vec<Warning>,
    },
    /// The rule gives a name that is refused (an ambiguous date, a name that is no workspace
    /// path or a reserved one, a name left empty, a file that is not `.md`); the item keeps
    /// its path.
    Refused {
        /// Why, located at the item's path.
        warnings: Vec<Warning>,
    },
    /// The rule gives the item a new path.
    Renamed {
        /// The new path, before collisions are resolved.
        path: WorkspacePath,
        /// The naming rule's collision policy.
        policy: CollisionPolicy,
        /// The naming rule's `alias_preservation`.
        keep_alias: bool,
        /// Findings about the item, located at its path.
        warnings: Vec<Warning>,
    },
}

/// A selection's rename plan, without the revision and the digest the operation adds.
#[derive(Debug, Clone)]
pub struct PlanBody {
    /// The items whose path changes, by their current path.
    pub entries: Vec<PlannedEntry>,
    /// One per contested collision key, by path.
    pub duplicate_observations: Vec<DuplicateObservation>,
    /// Findings about items that are not entries, by location, code and message.
    pub warnings: Vec<Warning>,
    /// Selected items that are not archived.
    pub considered: u32,
    /// Considered items whose rules leave their path as it is.
    pub unchanged: u32,
    /// Considered items whose rules give a refused name.
    pub refused: u32,
    /// Selected archived items, never renamed.
    pub archived_left_out: u32,
}

/// One entry of a plan, with what applying it needs beyond the wire entry.
#[derive(Debug, Clone)]
pub struct PlannedEntry {
    /// The wire entry.
    pub entry: RenameEntry,
    /// The naming rule's `alias_preservation`: record the old path when the item moves.
    pub keep_alias: bool,
}

/// The path chosen for a new item (an import's card) and what was found naming it.
#[derive(Debug, Clone)]
pub struct NewName {
    /// The chosen path; its collision key is taken.
    pub path: WorkspacePath,
    /// Findings, located at the chosen path.
    pub warnings: Vec<Warning>,
}

/// The paths an import must keep distinct: items cannot be ancestors, while folders retain
/// their stored spellings and may receive more cards. Includes the import's earlier cards.
#[derive(Debug, Default)]
pub struct TakenPaths {
    items: BTreeSet<String>,
    folders: BTreeMap<String, String>,
}

/// One compiled rule.
#[derive(Debug)]
struct CompiledRule {
    rule: NamingRule,
    glob: Pattern,
    date_format: Option<FormatDescriptionV3<'static>>,
    destination: Option<Destination>,
}

/// A compiled destination template.
#[derive(Debug)]
enum Destination {
    /// The same folder for every date (the probe dates write one folder).
    Fixed(String),
    /// A folder written with the item's date.
    Dated(FormatDescriptionV3<'static>),
}

/// The keys a round of `resolve` has assigned so far: item paths, and the folders above them.
#[derive(Default)]
struct Assigned {
    items: BTreeSet<String>,
    folders: BTreeSet<String>,
}

/// The part of a date a run of digits writes, judged by its value in two probe texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Year,
    ShortYear,
    Month,
    Day,
    Other,
}

/// The regular expressions that recognize dates and version suffixes in a name.
#[derive(Debug)]
struct Patterns {
    /// A date-shaped token between non-digits (group 1).
    candidate: Regex,
    /// Year, month, day.
    year_first: Regex,
    /// Two parts, then a four-digit year.
    year_last: Regex,
    /// Eight digits, `YYYYMMDD`.
    compact: Regex,
    /// One version suffix at the end of a base.
    version: Regex,
}

/// How one date token of a name reads under a rule's `date_order`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reading {
    /// Read without doubt.
    Read(Date),
    /// Date-shaped, but not a date under the order (or not a calendar date).
    NotRead,
    /// Both month-first and day-first are dates, and different ones.
    Ambiguous,
}

/// A run of a base: text, or a date token.
enum Piece<'b> {
    Text(&'b str),
    Date { text: &'b str, reading: Reading },
}

/// The words of a base and what finding its dates gave (`CompiledRule::words`).
struct Worded {
    words: Vec<String>,
    warnings: Vec<Warning>,
    /// One flag per ambiguous date token the date format would rewrite.
    ambiguous: Vec<Warning>,
    /// The reading of the base's first date token, for a destination that needs a date.
    first_date: Option<Reading>,
}

/// A source card's original-extension tail, as its name spells it.
struct Tail<'s> {
    extension: &'s str,
    number: Option<&'s str>,
}

/// A considered item the rules would move, with its candidate path.
struct Mover<'t> {
    summary: &'t ItemSummary,
    candidate: WorkspacePath,
    key: String,
    policy: CollisionPolicy,
    keep_alias: bool,
    warnings: Vec<Warning>,
}

/// What a plan counts (`PlanBody`), as it tallies the selection.
#[derive(Default)]
struct Counts {
    considered: usize,
    unchanged: usize,
    refused: usize,
    archived: usize,
}

/// What collision resolution decided for one mover.
#[derive(Debug, Clone)]
enum Outcome {
    /// It takes this path, with the warning numbering gave, if any.
    Takes(WorkspacePath, Option<Warning>),
    /// `keep_existing`: it keeps its path.
    Kept(Warning),
    /// `reject`, or no numbered name is a workspace path: it keeps its path, refused.
    Refused(Warning),
}

/// The only schema version of naming rules this build reads.
pub const RULES_SCHEMA_VERSION: u32 = 1;
/// The most rules one rule set holds.
pub const MAX_RULES: usize = 256;
/// The most bytes of one text field of a rule.
pub const MAX_RULE_TEXT_BYTES: usize = 1024;
/// The most entries one `apply_names` applies: each move reloads the staged bundle, so an apply
/// costs entries times items (until storage has a batch move).
pub const MAX_APPLY_ENTRIES: usize = 1000;
/// An ambiguous date in a name: the item is not renamed.
pub const DATE_AMBIGUOUS: &str = "date_ambiguous";
/// A date-shaped token the rule's order does not read: kept as written.
pub const DATE_NOT_READ: &str = "date_not_read";
/// A destination that needs a date the name does not give: the item keeps its folder.
pub const DESTINATION_NEEDS_DATE: &str = "destination_needs_date";
/// Nothing is left of the name once the rule is applied.
pub const NAME_LEFT_EMPTY: &str = "name_left_empty";
/// The new path is no workspace path.
pub const PATH_NOT_VALID: &str = "path_not_valid";
/// The new path has a segment the server keeps.
pub const PATH_RESERVED: &str = "path_reserved";
/// The item's file does not end in `.md`, so it cannot be renamed.
pub const ITEM_NOT_MOVABLE: &str = "item_not_movable";
/// A contested name was numbered.
pub const COLLISION_SUFFIXED: &str = "collision_suffixed";
/// A contested name was left to its holder or first claimant (`keep_existing`).
pub const COLLISION_KEPT: &str = "collision_kept";
/// A contested name was given to nobody (`reject`).
pub const COLLISION_REJECTED: &str = "collision_rejected";
/// A stored rules file is not YAML of naming rules (`get_rules`).
pub const RULES_YAML_INVALID: &str = "rules_yaml_invalid";
/// A stored rules file names a schema version this build does not read (`get_rules`).
pub const RULES_SCHEMA_VERSION_UNKNOWN: &str = "rules_schema_version_unknown";
/// A field of stored rules is refused (`get_rules`).
pub const RULES_INVALID: &str = "rules_invalid";
/// A field of stored rules is too large (`get_rules`).
pub const RULES_TOO_LARGE: &str = "rules_too_large";

/// The regular expressions, compiled at first use.
static PATTERNS: LazyLock<Result<Patterns, regex::Error>> = LazyLock::new(Patterns::build);

/// How a glob is matched: `*`, `?` and `[...]` never match `/`; a leading `.` is ordinary.
const GLOB_OPTIONS: MatchOptions = MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};
/// The word breaks of a name.
const BREAKS: [Boundary; 3] = [Boundary::Space, Boundary::Hyphen, Boundary::Underscore];
/// The segments the server keeps, besides okf-core's reserved files.
const APP_DIR: &str = ".okf";

impl CompiledRules {
    /// Check `rules` and compile them; `pointer` is where they sit in the request (`/rules`).
    ///
    /// # Errors
    /// Returns the first refusal, in field order (the schema version, the number of rules, then
    /// each rule's glob, type, separator, prefix, suffix, date format and destination):
    /// `InvalidInput` or `TooLarge` naming the field as a JSON Pointer under `pointer`, or
    /// `Internal` when a pattern of this module does not compile.
    pub fn compile(rules: &NamingRules, pointer: &str) -> Result<Self, ApiError> {
        let (compiled, mut problems) = check(rules, pointer)?;
        if problems.is_empty() {
            Ok(compiled)
        } else {
            Err(problems.remove(0))
        }
    }

    /// Every problem of `rules`, as warnings located at JSON Pointers into the rules file
    /// (`/schema_version`, `/rules/2/glob`): what `get_rules` answers for a stored file that
    /// reads as rules this build cannot apply.
    ///
    /// # Errors
    /// Returns `Internal` when a pattern of this module does not compile.
    pub fn problems(rules: &NamingRules) -> Result<Vec<Warning>, ApiError> {
        let (_, problems) = check(rules, "")?;
        Ok(problems
            .into_iter()
            .map(|problem| {
                let code = match (problem.field.as_deref(), problem.code) {
                    (Some("/schema_version"), _) => RULES_SCHEMA_VERSION_UNKNOWN,
                    (_, ErrorCode::TooLarge) => RULES_TOO_LARGE,
                    _ => RULES_INVALID,
                };
                Warning {
                    code: code.to_owned(),
                    message: problem.message,
                    location: problem.field,
                }
            })
            .collect())
    }
}

impl CompiledRule {
    /// Whether this rule names an item whose path has collision key `key` and type `type_name`.
    fn applies_to(&self, key: &str, type_name: &str) -> bool {
        self.glob.matches_with(key, GLOB_OPTIONS)
            && self
                .rule
                .type_name
                .as_deref()
                .is_none_or(|wanted| wanted == type_name)
    }

    /// The name this rule gives `subject` (`propose`, steps 2 to 5).
    fn name(&self, subject: &Subject<'_>, patterns: &Patterns) -> Proposal {
        let location = subject.path.as_str();
        let (folder, file) = match location.rsplit_once('/') {
            Some((folder, file)) => (Some(folder), file),
            None => (None, location),
        };
        let Some(stem) = file.strip_suffix(".md") else {
            return refused(
                location,
                ITEM_NOT_MOVABLE,
                "the item's file does not end in .md in lower case, so it cannot be renamed",
            );
        };
        let rule = &self.rule;
        let (base, tail) = card_tail(stem, subject.original_name);
        let base = without_affixes(base, &rule.prefix, rule.suffix.as_deref());
        let base = if rule.strip_version_suffix {
            without_versions(base, &patterns.version)
        } else {
            base
        };
        let Worded {
            words,
            mut warnings,
            mut ambiguous,
            first_date,
        } = self.words(base, location, patterns);
        let folder = match self.folder(folder, first_date, location) {
            Ok((folder, warning)) => {
                warnings.extend(warning);
                folder
            }
            Err(flag) => {
                // With a date format, every ambiguous token, the first included, is flagged.
                if self.date_format.is_none() {
                    ambiguous.push(flag);
                }
                None
            }
        };
        if !ambiguous.is_empty() {
            return Proposal::Refused {
                warnings: ambiguous,
            };
        }
        let joined = words.join(&rule.separator);
        if joined.is_empty() {
            return refused(
                location,
                NAME_LEFT_EMPTY,
                "nothing is left of the name once the rule is applied",
            );
        }
        let tail = tail.map_or_else(String::new, |tail| tail.written(&rule.extension_policy));
        let name = format!(
            "{}{joined}{}{tail}.md",
            rule.prefix,
            rule.suffix.as_deref().unwrap_or_default()
        );
        let full = match folder {
            Some(folder) => format!("{folder}/{name}"),
            None => name,
        };
        let path = match WorkspacePath::from_supplied(&full) {
            Ok(path) => path,
            Err(error) => {
                return refused(
                    location,
                    PATH_NOT_VALID,
                    &format!("the rule names the item {full}, which is no workspace path: {error}"),
                );
            }
        };
        if reserved(path.as_str()) {
            return refused(
                location,
                PATH_RESERVED,
                &format!(
                    "the rule names the item {}, a path the server keeps (.okf, index.md and \
                     log.md, as any path segment)",
                    path.as_str()
                ),
            );
        }
        if path == *subject.path {
            Proposal::Unchanged { warnings }
        } else {
            Proposal::Renamed {
                path,
                policy: rule.collision.clone(),
                keep_alias: rule.alias_preservation,
                warnings,
            }
        }
    }

    /// The words of `base` (the module header, steps 3 and 4): its text runs split and cased,
    /// its date tokens kept whole, each rewritten by the date format when one is set and the
    /// token reads; with the warnings and the flags that gave, and the first token's reading.
    fn words(&self, base: &str, location: &str, patterns: &Patterns) -> Worded {
        let rule = &self.rule;
        let mut worded = Worded {
            words: Vec::new(),
            warnings: Vec::new(),
            ambiguous: Vec::new(),
            first_date: None,
        };
        for piece in pieces(base, &rule.date_order, patterns) {
            let (text, reading) = match piece {
                Piece::Text(text) => {
                    worded.words.extend(words_of(text, &rule.case));
                    continue;
                }
                Piece::Date { text, reading } => (text, reading),
            };
            if worded.first_date.is_none() {
                worded.first_date = Some(reading);
            }
            let Some(format) = &self.date_format else {
                worded.words.push(text.to_owned());
                continue;
            };
            match reading {
                Reading::Read(date) => {
                    if let Ok(written) = date.format(format) {
                        worded.words.push(written);
                    } else {
                        worded.words.push(text.to_owned());
                        worded.warnings.push(not_read(location, text));
                    }
                }
                Reading::NotRead => {
                    worded.words.push(text.to_owned());
                    worded.warnings.push(not_read(location, text));
                }
                Reading::Ambiguous => {
                    worded
                        .ambiguous
                        .push(ambiguous_date(location, text, &rule.date_order));
                }
            }
        }
        worded
    }

    /// The folder of the new path, with the warning choosing it gave, or the flag of an
    /// ambiguous date the destination would read. `folder` is the item's own, `first` the
    /// reading of the base's first date token.
    fn folder(
        &self,
        folder: Option<&str>,
        first: Option<Reading>,
        location: &str,
    ) -> Result<(Option<String>, Option<Warning>), Warning> {
        let own = folder.map(str::to_owned);
        let format = match &self.destination {
            None => return Ok((own, None)),
            Some(Destination::Fixed(folder)) => return Ok((Some(folder.clone()), None)),
            Some(Destination::Dated(format)) => format,
        };
        let needs = || {
            warning(
                location,
                DESTINATION_NEEDS_DATE,
                "the destination needs a date the name does not give, so the item keeps its \
                 folder",
            )
        };
        match first {
            Some(Reading::Read(date)) => match date.format(format) {
                Ok(written) => Ok((Some(written), None)),
                Err(_) => Ok((own, Some(needs()))),
            },
            Some(Reading::Ambiguous) => Err(warning(
                location,
                DATE_AMBIGUOUS,
                "the destination needs the name's first date, which reads as two dates; name the \
                 order with date_order",
            )),
            Some(Reading::NotRead) | None => Ok((own, Some(needs()))),
        }
    }
}

impl Assigned {
    /// Whether `key` is free: held by no stayer, folder or assigned path, no folder of an
    /// assigned path, and with no stayer's or assigned item path as a folder above it.
    fn free(&self, key: &str, held: &BTreeSet<String>, folder_keys: &BTreeSet<String>) -> bool {
        !held.contains(key)
            && !folder_keys.contains(key)
            && !self.items.contains(key)
            && !self.folders.contains(key)
            && ancestors(key).all(|folder| !held.contains(folder) && !self.items.contains(folder))
    }

    /// Assign `key`, and the folders above it.
    fn take(&mut self, key: &str) {
        self.items.insert(key.to_owned());
        self.folders.extend(ancestors(key).map(str::to_owned));
    }
}

impl TakenPaths {
    /// Start with the complete tree at the import's base revision, keeping item and folder
    /// paths separate. Ancestors of each item are folders too.
    #[must_use]
    pub fn new(items: &[TreeItem], folders: &[WorkspacePath]) -> Self {
        let mut taken = Self {
            items: BTreeSet::new(),
            folders: folder_spellings(folders),
        };
        for item in items {
            taken.take(&item.summary.path);
        }
        taken
    }

    /// Whether an item or folder holds this collision key.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.items.contains(key) || self.folders.contains_key(key)
    }

    /// The first ancestor occupied by an item; numbering the leaf cannot make it a folder.
    fn item_ancestor<'p>(&self, path: &'p WorkspacePath) -> Option<&'p str> {
        ancestors(path.as_str()).find(|ancestor| self.items.contains(&collision_key_of(ancestor)))
    }

    /// Reserve a free item path and its folders, keeping any spelling already chosen.
    fn claim(&mut self, path: &WorkspacePath) -> bool {
        if self.contains(&path.collision_key()) || self.item_ancestor(path).is_some() {
            return false;
        }
        self.take(path);
        true
    }

    /// Record an item and its ancestor folders after its path has been checked.
    fn take(&mut self, path: &WorkspacePath) {
        self.items.insert(path.collision_key());
        for folder in ancestors(path.as_str()) {
            self.folders
                .entry(collision_key_of(folder))
                .or_insert_with(|| folder.to_owned());
        }
    }
}

impl Patterns {
    fn build() -> Result<Self, regex::Error> {
        Ok(Self {
            candidate: Regex::new(
                r"(?:^|[^0-9])([0-9]{4}[-_.][0-9]{1,2}[-_.][0-9]{1,2}|[0-9]{1,2}[-_.][0-9]{1,2}[-_.][0-9]{4}|[0-9]{8})(?:[^0-9]|$)",
            )?,
            year_first: Regex::new(r"^([0-9]{4})([-_.])([0-9]{1,2})([-_.])([0-9]{1,2})$")?,
            year_last: Regex::new(r"^([0-9]{1,2})([-_.])([0-9]{1,2})([-_.])([0-9]{4})$")?,
            compact: Regex::new(r"^([0-9]{4})([0-9]{2})([0-9]{2})$")?,
            version: Regex::new(
                r"(?i)(?:[ _-]+(?:v[0-9]+(?:\.[0-9]+)*|final|copy(?: [0-9]+)?)| *\([0-9]+\))$",
            )?,
        })
    }

    fn get() -> Result<&'static Self, ApiError> {
        PATTERNS.as_ref().map_err(|error| {
            ApiError::new(
                ErrorCode::Internal,
                format!("a naming rules pattern does not compile: {error}"),
            )
        })
    }
}

impl Tail<'_> {
    /// The tail as the extension policy writes it: `-<ext>` kept, lower-cased or left out, then
    /// `-<n>` when the card had a number.
    fn written(&self, policy: &ExtensionPolicy) -> String {
        let mut written = match policy {
            ExtensionPolicy::Preserve => format!("-{}", self.extension),
            ExtensionPolicy::Lowercase => format!("-{}", self.extension.to_lowercase()),
            ExtensionPolicy::Strip => String::new(),
        };
        if let Some(number) = self.number {
            written.push('-');
            written.push_str(number);
        }
        written
    }
}

/// What the rules make of one path (the module header, steps 1 to 5); collisions are not
/// resolved here (`plan`, `name_new_item`).
#[must_use]
pub fn propose(rules: &CompiledRules, subject: &Subject<'_>) -> Proposal {
    let key = subject.path.collision_key();
    rules
        .rules
        .iter()
        .find(|rule| rule.applies_to(&key, subject.type_name))
        .map_or(Proposal::Unmatched, |rule| {
            rule.name(subject, rules.patterns)
        })
}

/// `path` with `-<number>` before its `.md`: `notes/report.md` and 2 give `notes/report-2.md`.
///
/// The name always fits one path segment (`conventions::fit_segment`: the stem is shortened,
/// the tail kept whole) and the whole path at most `MAX_WORKSPACE_PATH_BYTES` (the stem is cut
/// further at a character boundary, and the dots and spaces a cut leaves are trimmed), so a name
/// at either limit is shortened, never refused.
///
/// # Errors
/// Returns the rule the numbered path breaks: when the folder leaves no room for a stem of one
/// character, or the result is no workspace path.
pub fn numbered(path: &WorkspacePath, number: u32) -> Result<WorkspacePath, IdentityError> {
    let text = path.as_str();
    let (folder, file) = match text.rsplit_once('/') {
        Some((folder, file)) => (Some(folder), file),
        None => (None, text),
    };
    let stem = file.strip_suffix(".md").unwrap_or(file);
    let tail = format!("-{number}.md");
    let joined = |segment: &str| match folder {
        Some(folder) => format!("{folder}/{segment}"),
        None => segment.to_owned(),
    };
    let mut full = joined(&fit_segment(stem, &tail));
    if full.len() > MAX_WORKSPACE_PATH_BYTES {
        let used = folder.map_or(0, |folder| folder.len().saturating_add(1));
        let room = MAX_WORKSPACE_PATH_BYTES
            .saturating_sub(used)
            .saturating_sub(tail.len());
        let mut cut = String::new();
        for character in stem.chars() {
            if cut.len().saturating_add(character.len_utf8()) > room {
                break;
            }
            cut.push(character);
        }
        let cut = cut.trim_end_matches(['.', ' ']);
        if cut.is_empty() {
            return Err(IdentityError(
                "the folder leaves no room for a numbered name",
            ));
        }
        full = joined(&fit_segment(cut, &tail));
    }
    WorkspacePath::try_from(full)
}

/// The rename plan of the `selected` items of `tree`, against the whole tree and its `folders`
/// (collisions are with any item or folder, since a destination can name any folder).
///
/// Archived selected items are counted in `archived_left_out` and never renamed. Every other
/// selected item is named (`propose`) and counted: renamed (an entry), `unchanged` (no rule, a
/// rule that leaves its path, or `keep_existing` losing a collision) or `refused` (a refused
/// name, or `reject` and a contested name), so `considered` is entries plus `unchanged` plus
/// `refused`. Collisions are compared by collision key against the final state of the tree, an
/// item that moves freeing its old key: stayers (unselected, archived, unmoved and refused
/// items) and folders hold their keys; movers are assigned in order, first those whose new path
/// differs only in letter case or normalization (each holds that key itself), then the rest by
/// current path. A mover whose name is not free is numbered (`suffix`), keeps its path
/// (`keep_existing`) or is refused with every other claimant of that name (`reject`); a mover
/// that so becomes a stayer frees nothing, and assignment starts again with it holding its key.
///
/// # Errors
/// Returns `Internal` when a count does not fit a `u32`.
pub fn plan(
    rules: &CompiledRules,
    tree: &[TreeItem],
    folders: &[WorkspacePath],
    selected: &BTreeSet<ItemId>,
) -> Result<PlanBody, ApiError> {
    let folder_spellings = folder_spellings(folders);
    let mut items: Vec<&TreeItem> = tree.iter().collect();
    items.sort_by(|left, right| left.summary.path.as_str().cmp(right.summary.path.as_str()));
    let mut counts = Counts::default();
    let mut warnings = Vec::new();
    let mut stayers: Vec<&ItemSummary> = Vec::new();
    let mut movers: Vec<Mover<'_>> = Vec::new();
    for item in items {
        let summary = &item.summary;
        if !selected.contains(&summary.id) {
            stayers.push(summary);
            continue;
        }
        if summary.archived {
            counts.archived = increment(counts.archived);
            stayers.push(summary);
            continue;
        }
        counts.considered = increment(counts.considered);
        let subject = Subject {
            path: &summary.path,
            type_name: &summary.type_name,
            original_name: item.original_name.as_deref(),
        };
        match propose_in_folders(rules, &subject, &folder_spellings) {
            Proposal::Unmatched => {
                counts.unchanged = increment(counts.unchanged);
                stayers.push(summary);
            }
            Proposal::Unchanged { warnings: found } => {
                counts.unchanged = increment(counts.unchanged);
                warnings.extend(found);
                stayers.push(summary);
            }
            Proposal::Refused { warnings: found } => {
                counts.refused = increment(counts.refused);
                warnings.extend(found);
                stayers.push(summary);
            }
            Proposal::Renamed {
                path,
                policy,
                keep_alias,
                warnings: found,
            } => movers.push(Mover {
                summary,
                key: path.collision_key(),
                candidate: path,
                policy,
                keep_alias,
                warnings: found,
            }),
        }
    }
    let order = assignment_order(&movers);
    let folder_keys: BTreeSet<String> = folders.iter().map(WorkspacePath::collision_key).collect();
    let outcomes = resolve(&movers, &order, &stayers, &folder_keys);
    let mut entries = settle(&movers, &outcomes, &mut counts, &mut warnings);
    entries.sort_by(|left, right| left.entry.from.as_str().cmp(right.entry.from.as_str()));
    warnings.sort_by(|left, right| {
        (left.location.as_deref(), &left.code, &left.message).cmp(&(
            right.location.as_deref(),
            &right.code,
            &right.message,
        ))
    });
    let duplicate_observations = observations(&movers, &order, &outcomes, &stayers, &folder_keys);
    Ok(PlanBody {
        entries,
        duplicate_observations,
        warnings,
        considered: count(counts.considered)?,
        unchanged: count(counts.unchanged)?,
        refused: count(counts.refused)?,
        archived_left_out: count(counts.archived)?,
    })
}

/// Folder keys retain their stored paths: a naming rule cannot respell an existing folder.
fn folder_spellings(folders: &[WorkspacePath]) -> BTreeMap<String, String> {
    folders
        .iter()
        .map(|folder| (folder.collision_key(), folder.as_str().to_owned()))
        .collect()
}

/// Name a subject using the deepest stored ancestor spelling, including any new folders
/// below it. An expanded stored spelling must still fit the workspace path bounds.
fn propose_in_folders(
    rules: &CompiledRules,
    subject: &Subject<'_>,
    folders: &BTreeMap<String, String>,
) -> Proposal {
    match propose(rules, subject) {
        Proposal::Renamed {
            path,
            policy,
            keep_alias,
            mut warnings,
        } => match in_stored_folders(&path, folders) {
            Ok(path) => Proposal::Renamed {
                path,
                policy,
                keep_alias,
                warnings,
            },
            Err(error) => {
                warnings.push(warning(subject.path.as_str(), PATH_NOT_VALID, error.0));
                Proposal::Refused { warnings }
            }
        },
        other => other,
    }
}

/// The candidate in its deepest existing folder, leaving the file name and new folders intact.
fn in_stored_folders(
    path: &WorkspacePath,
    folders: &BTreeMap<String, String>,
) -> Result<WorkspacePath, IdentityError> {
    let stored = ancestors(path.as_str()).filter_map(|ancestor| {
        folders
            .get(&collision_key_of(ancestor))
            .map(|spelling| (ancestor, spelling))
    });
    match stored.last() {
        Some((ancestor, spelling)) => {
            WorkspacePath::from_supplied(&path.as_str().replacen(ancestor, spelling, 1))
        }
        None => Ok(path.clone()),
    }
}

/// The entries of `movers` by their `outcomes` (`plan`): a mover that takes another path is an
/// entry, with the warning numbering gave; one that ends at its own path or keeps it
/// (`keep_existing`) is counted unchanged, and one refused is counted refused, its warnings and
/// the collision's added to the plan's.
fn settle(
    movers: &[Mover<'_>],
    outcomes: &BTreeMap<usize, Outcome>,
    counts: &mut Counts,
    warnings: &mut Vec<Warning>,
) -> Vec<PlannedEntry> {
    let mut entries = Vec::new();
    for (index, mover) in movers.iter().enumerate() {
        let mut found = mover.warnings.clone();
        match outcomes.get(&index) {
            Some(Outcome::Takes(path, numbering)) if *path != mover.summary.path => {
                found.extend(numbering.clone());
                entries.push(PlannedEntry {
                    entry: RenameEntry {
                        item_id: mover.summary.id,
                        from: mover.summary.path.clone(),
                        to: path.clone(),
                        warnings: found,
                    },
                    keep_alias: mover.keep_alias,
                });
            }
            Some(Outcome::Takes(..)) => {
                counts.unchanged = increment(counts.unchanged);
                warnings.extend(found);
            }
            Some(Outcome::Kept(collision)) => {
                counts.unchanged = increment(counts.unchanged);
                found.push(collision.clone());
                warnings.extend(found);
            }
            Some(Outcome::Refused(collision)) => {
                counts.refused = increment(counts.refused);
                found.push(collision.clone());
                warnings.extend(found);
            }
            // `resolve` decides every mover; one it left undecided keeps its path, refused.
            None => {
                counts.refused = increment(counts.refused);
                warnings.extend(found);
            }
        }
    }
    entries
}

/// Name one new item, an import's card: `subject.path` is its path without rules, and `taken`
/// holds every item and folder at the import's base and every item the same import named before;
/// the chosen item and its ancestor folders are added to it, preserving stored folder spelling.
///
/// The ruled path is taken when its key is free. Taken, a `suffix` rule numbers it
/// (`numbered`), so the rule's changes stay. Otherwise (no rule matches, the rule leaves the
/// path or gives a refused name, `keep_existing` or `reject`), the path without rules is taken,
/// numbered as the import numbers a card without rules (`numbered_source_card_name` for a
/// source card, `numbered` otherwise); a ruled name that lost its collision says so in the
/// warnings. A ruled name below an existing item falls back too: numbering its leaf could never
/// make that ancestor a folder. So a naming rule's collision never fails an import.
///
/// # Errors
/// Returns `InvalidInput` on `/destination` when the unruled path itself cannot use its stored
/// folder spelling or lies below an item; `Internal` when no numbered name is a workspace path.
pub fn name_new_item(
    rules: &CompiledRules,
    subject: &Subject<'_>,
    taken: &mut TakenPaths,
) -> Result<NewName, ApiError> {
    let (by_rule, mut warnings) = match propose_in_folders(rules, subject, &taken.folders) {
        Proposal::Unmatched => (None, Vec::new()),
        Proposal::Unchanged { warnings } | Proposal::Refused { warnings } => (None, warnings),
        Proposal::Renamed {
            path,
            policy,
            warnings,
            ..
        } => (Some((path, policy)), warnings),
    };
    let chosen = match by_rule {
        Some((path, policy)) => {
            if let Some(parent) = taken.item_ancestor(&path) {
                warnings.push(warning(
                    path.as_str(),
                    PATH_NOT_VALID,
                    &format!(
                        "{parent} is an item, not a folder, so the name is chosen without the rules"
                    ),
                ));
                None
            } else if taken.claim(&path) {
                Some(path)
            } else if policy == CollisionPolicy::Suffix {
                let numbered = first_numbered(taken, |number| numbered(&path, number).ok());
                if numbered.is_some() {
                    warnings.push(warning(
                        path.as_str(),
                        COLLISION_SUFFIXED,
                        &format!("{} is taken, so the name is numbered", path.as_str()),
                    ));
                }
                numbered
            } else {
                let (code, why) = if policy == CollisionPolicy::Reject {
                    (COLLISION_REJECTED, "the rule refuses a contested name")
                } else {
                    (COLLISION_KEPT, "the rule keeps the existing name")
                };
                warnings.push(warning(
                    path.as_str(),
                    code,
                    &format!(
                        "{} is taken and {why}, so the name is chosen without the rules",
                        path.as_str()
                    ),
                ));
                None
            }
        }
        None => None,
    };
    let path = match chosen {
        Some(path) => path,
        None => unruled(subject, taken)?,
    };
    for found in &mut warnings {
        found.location = Some(path.as_str().to_owned());
    }
    Ok(NewName { path, warnings })
}

/// The path without rules for `subject` (`name_new_item`): its own path when free, else the
/// first free numbered one, as an import names a card without rules.
fn unruled(subject: &Subject<'_>, taken: &mut TakenPaths) -> Result<WorkspacePath, ApiError> {
    let path = in_stored_folders(subject.path, &taken.folders)
        .map_err(|error| invalid(error.0, "/destination"))?;
    if let Some(parent) = taken.item_ancestor(&path) {
        return Err(invalid(
            format!("{parent} is an item, not a destination folder"),
            "/destination",
        ));
    }
    if taken.claim(&path) {
        return Ok(path);
    }
    let folder = path.as_str().rsplit_once('/').map(|(folder, _)| folder);
    first_numbered(taken, |number| match subject.original_name {
        Some(original) => {
            let name = numbered_source_card_name(original, number);
            let full = match folder {
                Some(folder) => format!("{folder}/{name}"),
                None => name,
            };
            WorkspacePath::from_supplied(&full).ok()
        }
        None => numbered(&path, number).ok(),
    })
    .ok_or_else(|| {
        ApiError::new(
            ErrorCode::Internal,
            format!(
                "no numbered name for {} is a free workspace path",
                subject.path.as_str()
            ),
        )
    })
}

/// The first of `numbered(2)`, `numbered(3)`, ... whose key is not in `taken`, now taken; `None`
/// once a number gives no path. At most one more number than there are taken keys is tried, since
/// each taken key blocks one number at most.
fn first_numbered(
    taken: &mut TakenPaths,
    numbered: impl Fn(u32) -> Option<WorkspacePath>,
) -> Option<WorkspacePath> {
    let occupied = taken.items.len().saturating_add(taken.folders.len());
    let last = u32::try_from(occupied).map_or(u32::MAX, |count| count.saturating_add(2));
    for number in 2..=last {
        let path = numbered(number)?;
        if taken.claim(&path) {
            return Some(path);
        }
    }
    None
}

/// Check every field of `rules` (`CompiledRules::compile`), collecting every problem in field
/// order, and compile the rules that pass.
fn check(rules: &NamingRules, pointer: &str) -> Result<(CompiledRules, Vec<ApiError>), ApiError> {
    let patterns = Patterns::get()?;
    let probes = probe_dates()?;
    let mut problems = Vec::new();
    if rules.schema_version != RULES_SCHEMA_VERSION {
        problems.push(invalid(
            format!(
                "this build reads naming rules of schema version {RULES_SCHEMA_VERSION}, not {}",
                rules.schema_version
            ),
            &format!("{pointer}/schema_version"),
        ));
    }
    if rules.rules.len() > MAX_RULES {
        problems.push(
            ApiError::new(
                ErrorCode::TooLarge,
                format!(
                    "a rule set holds at most {MAX_RULES} rules, not {}",
                    rules.rules.len()
                ),
            )
            .with_field(format!("{pointer}/rules")),
        );
    }
    let mut compiled = Vec::new();
    for (index, rule) in rules.rules.iter().enumerate() {
        let at = format!("{pointer}/rules/{index}");
        match check_rule(rule, &at, patterns, &probes) {
            Ok(rule) => compiled.push(rule),
            Err(found) => problems.extend(found),
        }
    }
    Ok((
        CompiledRules {
            rules: compiled,
            patterns,
        },
        problems,
    ))
}

/// Check one rule at `at` (`/rules/<i>` under the pointer), collecting every refused field.
fn check_rule(
    rule: &NamingRule,
    at: &str,
    patterns: &Patterns,
    probes: &[Date; 4],
) -> Result<CompiledRule, Vec<ApiError>> {
    let mut problems = Vec::new();
    let glob = compile_glob(&rule.glob, &format!("{at}/glob"))
        .map_err(|error| problems.push(error))
        .ok();
    let field = format!("{at}/type_name");
    if let Some(type_name) = &rule.type_name {
        if type_name.is_empty() {
            problems.push(invalid("leave type_name out to match every type", &field));
        } else if let Err(error) = bounded(type_name, "a type name", &field) {
            problems.push(error);
        }
    }
    if let Err(error) = check_separator(rule, &format!("{at}/separator")) {
        problems.push(error);
    }
    if let Err(error) = check_affix(&rule.prefix, "prefix", &format!("{at}/prefix")) {
        problems.push(error);
    }
    if let Some(suffix) = &rule.suffix
        && let Err(error) = check_affix(suffix, "suffix", &format!("{at}/suffix"))
    {
        problems.push(error);
    }
    let date_format = compile_date_format(rule, &format!("{at}/date_format"), patterns, probes)
        .map_err(|error| problems.push(error))
        .ok()
        .flatten();
    let destination = compile_destination(rule, &format!("{at}/destination"), probes)
        .map_err(|error| problems.push(error))
        .ok()
        .flatten();
    match glob {
        Some(glob) if problems.is_empty() => Ok(CompiledRule {
            rule: rule.clone(),
            glob,
            date_format,
            destination,
        }),
        _ => Err(problems),
    }
}

/// Refuse a glob that is empty, too long, rooted, ends in `/` or holds `\`, or that the `glob`
/// crate refuses; otherwise its pattern, compiled from its collision key.
fn compile_glob(glob: &str, field: &str) -> Result<Pattern, ApiError> {
    if glob.is_empty() {
        return Err(invalid("a glob must not be empty", field));
    }
    bounded(glob, "a glob", field)?;
    if glob.starts_with('/') {
        return Err(invalid(
            "a glob is relative to the workspace root; leave out the leading /",
            field,
        ));
    }
    if glob.ends_with('/') {
        let folder = glob.trim_end_matches('/');
        return Err(invalid(
            format!("a glob matches item paths; write {folder}/** for everything under {folder}"),
            field,
        ));
    }
    if glob.contains('\\') {
        return Err(invalid("a glob uses / between folders, never \\", field));
    }
    Pattern::new(&collision_key_of(glob))
        .map_err(|error| invalid(format!("{} (at {})", error.msg, error.pos), field))
}

/// Refuse a separator that is too long, holds `/`, cannot stand inside a path segment, or
/// contradicts `snake` (`_`) or `kebab` (`-`) case.
fn check_separator(rule: &NamingRule, field: &str) -> Result<(), ApiError> {
    let separator = &rule.separator;
    bounded(separator, "a separator", field)?;
    if separator.contains('/') {
        return Err(invalid("a separator cannot hold /", field));
    }
    WorkspacePath::try_from(format!("a{separator}b")).map_err(|error| invalid(error.0, field))?;
    match rule.case {
        LetterCase::Snake if separator != "_" => {
            Err(invalid("snake case joins words with _", field))
        }
        LetterCase::Kebab if separator != "-" => {
            Err(invalid("kebab case joins words with -", field))
        }
        LetterCase::Preserve
        | LetterCase::Lower
        | LetterCase::Upper
        | LetterCase::Title
        | LetterCase::Snake
        | LetterCase::Kebab => Ok(()),
    }
}

/// Refuse a prefix or suffix that is too long, holds `/`, or cannot begin (a prefix) or end (a
/// suffix) a file name.
fn check_affix(affix: &str, what: &str, field: &str) -> Result<(), ApiError> {
    bounded(affix, &format!("a {what}"), field)?;
    if affix.contains('/') {
        return Err(invalid(format!("a {what} cannot hold /"), field));
    }
    let probe = if what == "prefix" {
        format!("{affix}a.md")
    } else {
        format!("a{affix}.md")
    };
    WorkspacePath::try_from(probe).map_err(|error| invalid(error.0, field))?;
    Ok(())
}

/// The rule's date format, compiled; `None` when it is empty (dates kept as written). A format
/// is accepted only when every probe date it writes reads back, by the recognizer names are read
/// with (`classify`) under the rule's `date_order`, as that same date: so it writes `[year]`,
/// `[month]` and `[day]` as numbers, each once, year first or in the declared order, with one of
/// `-`, `_` or `.` between them (the same both times) or nothing (then zero-padded), and a
/// second preview leaves the names it wrote as they are. A refusal says which of those it breaks
/// (`date_format_fault`).
fn compile_date_format(
    rule: &NamingRule,
    field: &str,
    patterns: &Patterns,
    probes: &[Date; 4],
) -> Result<Option<FormatDescriptionV3<'static>>, ApiError> {
    if rule.date_format.is_empty() {
        return Ok(None);
    }
    bounded(&rule.date_format, "a date format", field)?;
    let format = time::format_description::parse_owned::<3>(&rule.date_format)
        .map_err(|error| invalid(format!("not a date format: {error}"), field))?;
    let mut written = Vec::new();
    for probe in probes {
        let text = probe
            .format(&format)
            .map_err(|error| written_fault(&error, field))?;
        if text.contains('/') {
            return Err(invalid("the date would hold /", field));
        }
        written.push(text);
    }
    let reads_back = probes.iter().zip(&written).all(|(probe, text)| {
        classify(text, &rule.date_order, patterns) == Some(Reading::Read(*probe))
    });
    if !reads_back {
        return Err(invalid(
            date_format_fault(&written, &rule.date_order),
            field,
        ));
    }
    for text in &written {
        WorkspacePath::try_from(format!("a{text}b")).map_err(|error| invalid(error.0, field))?;
    }
    Ok(Some(format))
}

/// Which rule of a date format the probe dates' texts break (`compile_date_format`); `written`
/// holds the texts of the probes, in `probe_dates` order.
fn date_format_fault(written: &[String], order: &DateOrder) -> String {
    const NUMBERS: &str = "a date format writes only numbers: [year], [month], [day]";
    const EACH_ONCE: &str = "a date format writes [year], [month] and [day], each once";
    const BETWEEN: &str =
        "between the parts of a date stands -, _ or ., the same both times, or nothing";
    const ORDER: &str = "a date format puts the year first, or follows date_order";
    let skeletons: BTreeSet<String> = written.iter().map(|text| skeleton(text)).collect();
    if skeletons.len() > 1 {
        let unspaced: BTreeSet<String> =
            skeletons.iter().map(|text| text.replace(' ', "")).collect();
        return if unspaced.len() == 1 {
            "a date format pads [month] and [day] with zeros or not at all, never with spaces"
                .to_owned()
        } else {
            NUMBERS.to_owned()
        };
    }
    // 2025-03-04 and 1999-12-31: every part differs, and the second's month and day have two
    // digits whatever the padding.
    let (Some(march), Some(december), Some(first)) =
        (written.first(), written.get(1), written.get(2))
    else {
        return EACH_ONCE.to_owned();
    };
    let runs = digit_runs(december);
    let between = literals(december);
    if between.first().is_some_and(|text| !text.is_empty())
        || between.last().is_some_and(|text| !text.is_empty())
    {
        return "a date format holds the date alone: no text before or after it".to_owned();
    }
    match runs.as_slice() {
        [compact] => {
            if compact.len() != 8 {
                EACH_ONCE.to_owned()
            } else if *compact == "19991231" {
                "a compact date writes every part zero-padded".to_owned()
            } else {
                "a date with nothing between its parts puts the year first".to_owned()
            }
        }
        [_, _] => {
            if runs.iter().map(|run| run.len()).sum::<usize>() == 8 {
                BETWEEN.to_owned()
            } else {
                EACH_ONCE.to_owned()
            }
        }
        [_, _, _] => {
            let (Some(one), Some(two)) = (between.get(1), between.get(2)) else {
                return BETWEEN.to_owned();
            };
            if one != two || !matches!(one.as_str(), "-" | "_" | ".") {
                return BETWEEN.to_owned();
            }
            let parts = parts_of(&digit_runs(march), &runs);
            if parts.contains(&Part::ShortYear) {
                return "the year keeps four digits ([year], never [year repr:last_two])"
                    .to_owned();
            }
            let wanted = [Part::Year, Part::Month, Part::Day];
            if parts.len() != 3 || !wanted.iter().all(|part| parts.contains(part)) {
                return EACH_ONCE.to_owned();
            }
            let declared = match order {
                DateOrder::Dmy => Some([Part::Day, Part::Month, Part::Year]),
                DateOrder::Mdy => Some([Part::Month, Part::Day, Part::Year]),
                DateOrder::Ymd | DateOrder::RejectAmbiguous => None,
            };
            if parts != wanted && !declared.is_some_and(|declared| parts == declared) {
                return ORDER.to_owned();
            }
            let year_at = parts.iter().position(|part| *part == Part::Year);
            let first_runs = digit_runs(first);
            if year_at
                .and_then(|at| first_runs.get(at))
                .is_some_and(|run| run.len() != 4)
            {
                return "the year keeps four digits ([year], never [year padding:none])".to_owned();
            }
            "a date format writes a date the names are read back from".to_owned()
        }
        _ => EACH_ONCE.to_owned(),
    }
}

/// Each run's part: `march` and `december` are the runs of 2025-03-04 and 1999-12-31.
fn parts_of(march: &[&str], december: &[&str]) -> Vec<Part> {
    let value = |run: &str| run.parse::<u32>().ok();
    march
        .iter()
        .zip(december)
        .map(|(first, second)| match (value(first), value(second)) {
            (Some(2025), Some(1999)) => Part::Year,
            (Some(25), Some(99)) => Part::ShortYear,
            (Some(3), Some(12)) => Part::Month,
            (Some(4), Some(31)) => Part::Day,
            _ => Part::Other,
        })
        .collect()
}

/// `text` with every run of ASCII digits written `#`.
fn skeleton(text: &str) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for character in text.chars() {
        if character.is_ascii_digit() {
            if !in_run {
                out.push('#');
            }
            in_run = true;
        } else {
            out.push(character);
            in_run = false;
        }
    }
    out
}

/// The runs of ASCII digits of `text`, in order.
fn digit_runs(text: &str) -> Vec<&str> {
    text.split(|character: char| !character.is_ascii_digit())
        .filter(|run| !run.is_empty())
        .collect()
}

/// The text before, between and after the digit runs of `text`: one more than there are runs
/// (`2025-03-04` gives `""`, `-`, `-`, `""`).
fn literals(text: &str) -> Vec<String> {
    let mut found = vec![String::new()];
    let mut in_run = false;
    for character in text.chars() {
        if character.is_ascii_digit() {
            in_run = true;
            continue;
        }
        if in_run {
            found.push(String::new());
            in_run = false;
        }
        if let Some(last) = found.last_mut() {
            last.push(character);
        }
    }
    if in_run {
        found.push(String::new());
    }
    found
}

/// The refusal of a format a date cannot write: one naming a part a date does not have, as the
/// `time` crate says (`InsufficientTypeInformation`), or its own words.
fn written_fault(error: &time::error::Format, field: &str) -> ApiError {
    match error {
        time::error::Format::InsufficientTypeInformation { .. } => {
            invalid("names a part a date does not have", field)
        }
        other => invalid(format!("cannot write a date: {other}"), field),
    }
}

/// The rule's destination, compiled; `None` when it is absent (each item keeps its folder).
/// It must not be empty, must parse as a format description (version 3), and must write every
/// probe date as a workspace folder path with no reserved segment.
fn compile_destination(
    rule: &NamingRule,
    field: &str,
    probes: &[Date; 4],
) -> Result<Option<Destination>, ApiError> {
    let Some(text) = &rule.destination else {
        return Ok(None);
    };
    if text.is_empty() {
        return Err(invalid(
            "leave destination out to keep each item's folder",
            field,
        ));
    }
    bounded(text, "a destination", field)?;
    let format = time::format_description::parse_owned::<3>(text)
        .map_err(|error| invalid(format!("not a folder template: {error}"), field))?;
    let mut written = BTreeSet::new();
    for probe in probes {
        let folder = probe
            .format(&format)
            .map_err(|error| written_fault(&error, field))?;
        let path =
            WorkspacePath::from_supplied(&folder).map_err(|error| invalid(error.0, field))?;
        if reserved(path.as_str()) {
            return Err(invalid(
                format!(
                    "{} is kept by the server: .okf, index.md and log.md (in any letter case, as \
                     any path segment) cannot hold an item",
                    path.as_str()
                ),
                field,
            ));
        }
        written.insert(String::from(path));
    }
    // The probes differ in year, month, day, weekday, ordinal and week, so a template that
    // names any part of a date writes more than one folder for them.
    let mut folders = written.into_iter();
    match (folders.next(), folders.next()) {
        (Some(folder), None) => Ok(Some(Destination::Fixed(folder))),
        _ => Ok(Some(Destination::Dated(format))),
    }
}

/// The probe dates a format is checked with: every part distinct (2025-03-04, with one-digit
/// month and day), two-digit month and day (1999-12-31), the smallest year (0001-01-01, for
/// the year's four digits), and a date whose ISO week belongs to the next year (2024-12-30).
fn probe_dates() -> Result<[Date; 4], ApiError> {
    let date = |year, month, day| {
        Date::from_calendar_date(year, month, day)
            .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))
    };
    Ok([
        date(2025, Month::March, 4)?,
        date(1999, Month::December, 31)?,
        date(1, Month::January, 1)?,
        date(2024, Month::December, 30)?,
    ])
}

/// The base and the source-card tail of a card's `stem` (the module header, step 2): a trailing
/// `-<ext>` or `-<ext>-<n>`, where `<ext>` is the original's extension (compared by collision
/// key) and `<n>` a number of at least 2 with no leading zero, after a non-empty base. A note,
/// and a card whose name does not end so (a person renamed it), has no tail.
fn card_tail<'s>(stem: &'s str, original: Option<&str>) -> (&'s str, Option<Tail<'s>>) {
    let Some(extension) = original.and_then(original_extension) else {
        return (stem, None);
    };
    if let Some((rest, number)) = stem.rsplit_once('-')
        && is_card_number(number)
        && let Some((base, written)) = split_extension(rest, extension)
    {
        return (
            base,
            Some(Tail {
                extension: written,
                number: Some(number),
            }),
        );
    }
    match split_extension(stem, extension) {
        Some((base, written)) => (
            base,
            Some(Tail {
                extension: written,
                number: None,
            }),
        ),
        None => (stem, None),
    }
}

/// The extension `conventions::source_card_name` splits off `original`: after its last `.`,
/// when something stands before that `.`.
fn original_extension(original: &str) -> Option<&str> {
    original
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, extension)| extension)
}

/// `text` as a base, `-` and `extension` as written there (compared by collision key), when
/// the base is not empty.
fn split_extension<'s>(text: &'s str, extension: &str) -> Option<(&'s str, &'s str)> {
    let wanted = collision_key_of(extension);
    text.match_indices('-').rev().find_map(|(at, _)| {
        let (base, rest) = text.split_at_checked(at)?;
        let written = rest.strip_prefix('-')?;
        (!base.is_empty() && collision_key_of(written) == wanted).then_some((base, written))
    })
}

/// Whether `text` is the number the import's numbering writes: decimal, at least 2, no leading
/// zero.
fn is_card_number(text: &str) -> bool {
    !text.starts_with('0')
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && text.parse::<u32>().is_ok_and(|number| number >= 2)
}

/// `base` without an existing `prefix` at its start and `suffix` at its end, each compared in
/// lower case over as many characters, and each taken off only when something is left.
fn without_affixes<'b>(base: &'b str, prefix: &str, suffix: Option<&str>) -> &'b str {
    let mut base = base;
    let wanted = prefix.chars().count();
    if wanted > 0
        && let Some((at, _)) = base.char_indices().nth(wanted)
        && let (Some(head), Some(rest)) = (base.get(..at), base.get(at..))
        && head.to_lowercase() == prefix.to_lowercase()
    {
        base = rest;
    }
    if let Some(suffix) = suffix {
        let wanted = suffix.chars().count();
        let total = base.chars().count();
        if wanted > 0
            && total > wanted
            && let Some((at, _)) = base.char_indices().nth(total.saturating_sub(wanted))
            && let (Some(head), Some(rest)) = (base.get(..at), base.get(at..))
            && rest.to_lowercase() == suffix.to_lowercase()
        {
            base = head;
        }
    }
    base
}

/// `base` without its version suffixes, repeatedly, never down to nothing.
fn without_versions<'b>(base: &'b str, version: &Regex) -> &'b str {
    let mut base = base;
    while let Some(found) = version.find(base) {
        match base.get(..found.start()) {
            Some(before) if !before.is_empty() => base = before,
            _ => break,
        }
    }
    base
}

/// `base` as text runs and date tokens, in order (the module header, step 3). A token is
/// searched from the end of the previous one, so two dates one character apart are both found.
fn pieces<'b>(base: &'b str, order: &DateOrder, patterns: &Patterns) -> Vec<Piece<'b>> {
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut text_from = 0;
    while start <= base.len() {
        let Some(token) = patterns
            .candidate
            .captures_at(base, start)
            .and_then(|found| found.get(1))
        else {
            break;
        };
        if let Some(reading) = classify(token.as_str(), order, patterns) {
            if let Some(text) = base.get(text_from..token.start())
                && !text.is_empty()
            {
                pieces.push(Piece::Text(text));
            }
            pieces.push(Piece::Date {
                text: token.as_str(),
                reading,
            });
            text_from = token.end();
        }
        start = token.end();
    }
    if let Some(text) = base.get(text_from..)
        && !text.is_empty()
    {
        pieces.push(Piece::Text(text));
    }
    pieces
}

/// How `token` reads under `order`, or `None` when it is ordinary text: two separators that
/// differ, or eight digits that are no calendar date. A year-first or compact token is read
/// year, month, day under every order; a year-last token as the order says, `ymd` reading none
/// and `reject_ambiguous` reading the one date it can be, or flagging two.
fn classify(token: &str, order: &DateOrder, patterns: &Patterns) -> Option<Reading> {
    if let Some(found) = patterns.year_first.captures(token) {
        let [year, first, month, second, day] = groups(&found)?;
        return (first == second).then(|| reading(date_of(year, month, day)));
    }
    if let Some(found) = patterns.year_last.captures(token) {
        let [one, first, two, second, year] = groups(&found)?;
        if first != second {
            return None;
        }
        let month_first = date_of(year, one, two);
        let day_first = date_of(year, two, one);
        return Some(match order {
            DateOrder::Ymd => Reading::NotRead,
            DateOrder::Mdy => reading(month_first),
            DateOrder::Dmy => reading(day_first),
            DateOrder::RejectAmbiguous => match (month_first, day_first) {
                (Some(month), Some(day)) if month != day => Reading::Ambiguous,
                (Some(date), _) | (None, Some(date)) => Reading::Read(date),
                (None, None) => Reading::NotRead,
            },
        });
    }
    let found = patterns.compact.captures(token)?;
    let mut parts = found
        .iter()
        .skip(1)
        .map(|part| part.map(|part| part.as_str()));
    let (Some(Some(year)), Some(Some(month)), Some(Some(day))) =
        (parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    date_of(year, month, day).map(Reading::Read)
}

/// The five groups of a year-first or year-last match.
fn groups<'t>(found: &Captures<'t>) -> Option<[&'t str; 5]> {
    let group = |index| found.get(index).map(|part| part.as_str());
    Some([group(1)?, group(2)?, group(3)?, group(4)?, group(5)?])
}

/// `Read` of a date, `NotRead` of none.
fn reading(date: Option<Date>) -> Reading {
    date.map_or(Reading::NotRead, Reading::Read)
}

/// The calendar date of decimal year, month and day, if it is one.
fn date_of(year: &str, month: &str, day: &str) -> Option<Date> {
    let month = Month::try_from(month.parse::<u8>().ok()?).ok()?;
    Date::from_calendar_date(year.parse().ok()?, month, day.parse().ok()?).ok()
}

/// The words of a text run, split on space, `-` and `_`, empty ones dropped, in `case`.
fn words_of(text: &str, case: &LetterCase) -> Vec<String> {
    let split = convert_case::split(&text, &BREAKS);
    let words = Words::RemoveEmpty.mutate(&split);
    match case {
        LetterCase::Preserve => words,
        LetterCase::Lower | LetterCase::Snake | LetterCase::Kebab => {
            Words::Lowercase.mutate(&words)
        }
        LetterCase::Upper => Words::Uppercase.mutate(&words),
        LetterCase::Title => Words::Custom(title_words).mutate(&words),
    }
}

/// Title case: a word that holds an upper-case letter is kept as written (`NASA`, `iPhone`);
/// any other gets its first character upper-cased and the rest kept.
fn title_words(words: &[&str]) -> Vec<String> {
    words
        .iter()
        .map(|word| {
            if word.chars().any(char::is_uppercase) {
                return (*word).to_owned();
            }
            let mut characters = word.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(characters).collect()
            })
        })
        .collect()
}

/// Whether a segment of `path` is one the server keeps: `.okf`, `index.md` or `log.md`,
/// compared by collision key. The folder operations refuse such a folder or destination with it
/// too (`application::folders`).
pub(crate) fn reserved(path: &str) -> bool {
    collision_key_of(path).split('/').any(|segment| {
        segment == APP_DIR
            || okf_core::RESERVED_FILENAMES
                .iter()
                .any(|name| collision_key_of(name) == segment)
    })
}

/// The order movers are assigned in: those whose candidate has their own current key first
/// (a rename that changes only letter case or normalization), then the rest, each group by
/// current path.
fn assignment_order(movers: &[Mover<'_>]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..movers.len()).collect();
    order.sort_by(|left, right| {
        let rank = |index: &usize| {
            movers.get(*index).map(|mover| {
                (
                    mover.key != mover.summary.path.collision_key(),
                    mover.summary.path.as_str(),
                )
            })
        };
        rank(left).cmp(&rank(right))
    });
    order
}

/// Resolve the movers' collisions (`plan`): each mover's outcome, by its index. A mover with no
/// outcome never occurs.
fn resolve(
    movers: &[Mover<'_>],
    order: &[usize],
    stayers: &[&ItemSummary],
    folder_keys: &BTreeSet<String>,
) -> BTreeMap<usize, Outcome> {
    let mut stayed: BTreeMap<usize, Outcome> = BTreeMap::new();
    // Each round either ends or turns at least one more mover into a stayer.
    for _ in 0..=movers.len() {
        let mut held: BTreeSet<String> = stayers
            .iter()
            .map(|stayer| stayer.path.collision_key())
            .collect();
        held.extend(
            stayed
                .keys()
                .filter_map(|index| movers.get(*index))
                .map(|mover| mover.summary.path.collision_key()),
        );
        let active: Vec<usize> = order
            .iter()
            .copied()
            .filter(|index| !stayed.contains_key(index))
            .collect();
        let mut claimants: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for index in &active {
            if let Some(mover) = movers.get(*index) {
                claimants
                    .entry(mover.key.as_str())
                    .or_default()
                    .push(*index);
            }
        }
        let mut assigned = Assigned::default();
        let mut decided: BTreeMap<usize, Outcome> = BTreeMap::new();
        let mut turned: BTreeMap<usize, Outcome> = BTreeMap::new();
        for index in active {
            let Some(mover) = movers.get(index) else {
                continue;
            };
            if turned.contains_key(&index) {
                continue;
            }
            let contested = mover.policy == CollisionPolicy::Reject
                && claimants
                    .get(mover.key.as_str())
                    .is_some_and(|claimed| claimed.len() > 1);
            if !contested && assigned.free(&mover.key, &held, folder_keys) {
                assigned.take(&mover.key);
                decided.insert(index, Outcome::Takes(mover.candidate.clone(), None));
                continue;
            }
            let location = mover.summary.path.as_str();
            match mover.policy {
                CollisionPolicy::Suffix => {
                    let outcome = numbered_outcome(mover, &mut assigned, &held, folder_keys);
                    match outcome {
                        Outcome::Takes(..) => {
                            decided.insert(index, outcome);
                        }
                        Outcome::Kept(_) | Outcome::Refused(_) => {
                            turned.insert(index, outcome);
                        }
                    }
                }
                CollisionPolicy::KeepExisting => {
                    turned.insert(index, kept(mover));
                }
                CollisionPolicy::Reject => {
                    turned.insert(index, rejected(location, &mover.candidate));
                    for other in claimants.get(mover.key.as_str()).into_iter().flatten() {
                        if let Some(claimant) = movers.get(*other) {
                            turned.entry(*other).or_insert_with(|| {
                                rejected(claimant.summary.path.as_str(), &mover.candidate)
                            });
                        }
                    }
                }
            }
        }
        if turned.is_empty() {
            stayed.extend(decided);
            return stayed;
        }
        stayed.extend(turned);
    }
    stayed
}

/// The folders above a key, outermost first.
fn ancestors(key: &str) -> impl Iterator<Item = &str> {
    key.match_indices('/').filter_map(|(at, _)| key.get(..at))
}

/// A `keep_existing` mover whose name is taken: it keeps its path.
fn kept(mover: &Mover<'_>) -> Outcome {
    Outcome::Kept(warning(
        mover.summary.path.as_str(),
        COLLISION_KEPT,
        &format!(
            "{} is taken, so the item keeps its name",
            mover.candidate.as_str()
        ),
    ))
}

/// The refusal of the item at `at` that claims the contested `candidate` under `reject`.
fn rejected(at: &str, candidate: &WorkspacePath) -> Outcome {
    Outcome::Refused(warning(
        at,
        COLLISION_REJECTED,
        &format!(
            "{} is contested, and the rule gives a contested name to nobody: decide which \
             item keeps it",
            candidate.as_str()
        ),
    ))
}

/// A `suffix` mover's numbered path: the first free `numbered` candidate, or its refusal when a
/// numbered path is no workspace path.
fn numbered_outcome(
    mover: &Mover<'_>,
    assigned: &mut Assigned,
    held: &BTreeSet<String>,
    folder_keys: &BTreeSet<String>,
) -> Outcome {
    let location = mover.summary.path.as_str();
    let blocked = held
        .len()
        .saturating_add(folder_keys.len())
        .saturating_add(assigned.items.len())
        .saturating_add(assigned.folders.len());
    let last = u32::try_from(blocked).map_or(u32::MAX, |count| count.saturating_add(2));
    for number in 2..=last {
        match numbered(&mover.candidate, number) {
            Ok(path) => {
                let key = path.collision_key();
                if assigned.free(&key, held, folder_keys) {
                    assigned.take(&key);
                    let why = format!(
                        "{} is taken, so the item is named {}",
                        mover.candidate.as_str(),
                        path.as_str()
                    );
                    return Outcome::Takes(path, Some(warning(location, COLLISION_SUFFIXED, &why)));
                }
            }
            Err(error) => {
                return Outcome::Refused(warning(
                    location,
                    PATH_NOT_VALID,
                    &format!(
                        "{} is taken, and its numbered name is no workspace path: {error}",
                        mover.candidate.as_str()
                    ),
                ));
            }
        }
    }
    Outcome::Refused(warning(
        location,
        PATH_NOT_VALID,
        &format!("no numbered name for {} is free", mover.candidate.as_str()),
    ))
}

/// One observation per contested key: a key more than one mover claimed, or one a mover claimed
/// and an item that keeps its path, or a folder, holds; by path.
fn observations(
    movers: &[Mover<'_>],
    order: &[usize],
    outcomes: &BTreeMap<usize, Outcome>,
    stayers: &[&ItemSummary],
    folder_keys: &BTreeSet<String>,
) -> Vec<DuplicateObservation> {
    let mut by_key: BTreeMap<&str, Vec<&Mover<'_>>> = BTreeMap::new();
    for index in order {
        if let Some(mover) = movers.get(*index) {
            by_key.entry(mover.key.as_str()).or_default().push(mover);
        }
    }
    // The items that end at their own path hold it: the stayers, and the movers that stayed.
    let mut holders: Vec<&ItemSummary> = stayers.to_vec();
    for (index, outcome) in outcomes {
        let Some(mover) = movers.get(*index) else {
            continue;
        };
        match outcome {
            Outcome::Takes(path, _) if *path != mover.summary.path => {}
            Outcome::Takes(..) | Outcome::Kept(_) | Outcome::Refused(_) => {
                holders.push(mover.summary);
            }
        }
    }
    let mut found = Vec::new();
    for (key, claimants) in by_key {
        let holder = holders.iter().find(|held| {
            held.path.collision_key() == key
                && claimants
                    .iter()
                    .all(|claimant| claimant.summary.id != held.id)
        });
        let folder = folder_keys.contains(key);
        if claimants.len() < 2 && holder.is_none() && !folder {
            continue;
        }
        let Some(first) = claimants.first() else {
            continue;
        };
        let path = holder.map_or_else(|| first.candidate.clone(), |held| held.path.clone());
        let mut item_ids: Vec<ItemId> = claimants
            .iter()
            .map(|claimant| claimant.summary.id)
            .collect();
        item_ids.extend(holder.map(|held| held.id));
        item_ids.sort();
        item_ids.dedup();
        let head = match (holder, folder) {
            (Some(held), _) => format!("{} holds {}", held.path.as_str(), path.as_str()),
            (None, true) => format!("{} is a folder", path.as_str()),
            (None, false) => format!("{} items would be named {}", claimants.len(), path.as_str()),
        };
        let told: Vec<String> = claimants
            .iter()
            .filter_map(|claimant| {
                let index = movers
                    .iter()
                    .position(|mover| mover.summary.id == claimant.summary.id)?;
                let from = claimant.summary.path.as_str();
                Some(match outcomes.get(&index) {
                    Some(Outcome::Takes(taken, _)) if taken.collision_key() == key => {
                        format!("{from} takes it")
                    }
                    Some(Outcome::Takes(taken, _)) => format!("{from} is named {}", taken.as_str()),
                    Some(Outcome::Kept(_)) => format!("{from} keeps its name"),
                    Some(Outcome::Refused(_)) | None => {
                        format!("{from} is not renamed until a person decides")
                    }
                })
            })
            .collect();
        found.push(DuplicateObservation {
            path,
            item_ids,
            message: format!("{head}: {}", told.join("; ")),
        });
    }
    found.sort_by(|left, right| left.path.as_str().cmp(right.path.as_str()));
    found
}

/// A refused proposal with one warning.
fn refused(location: &str, code: &str, message: &str) -> Proposal {
    Proposal::Refused {
        warnings: vec![warning(location, code, message)],
    }
}

/// A warning about the item at `location`.
fn warning(location: &str, code: &str, message: &str) -> Warning {
    Warning {
        code: code.to_owned(),
        message: message.to_owned(),
        location: Some(location.to_owned()),
    }
}

/// The warning of a date token the rule's order does not read.
fn not_read(location: &str, token: &str) -> Warning {
    warning(
        location,
        DATE_NOT_READ,
        &format!("{token} is not a date under the rule's date_order, so it is kept as written"),
    )
}

/// The warning of an ambiguous date token.
fn ambiguous_date(location: &str, token: &str, order: &DateOrder) -> Warning {
    let why = match order {
        DateOrder::RejectAmbiguous => "reads as month-first and as day-first",
        DateOrder::Ymd | DateOrder::Mdy | DateOrder::Dmy => "is ambiguous",
    };
    warning(
        location,
        DATE_AMBIGUOUS,
        &format!("{token} {why}, so the item is not renamed; name the order with date_order"),
    )
}

/// Refuse `text` over `MAX_RULE_TEXT_BYTES` as `TooLarge` on `field`.
fn bounded(text: &str, what: &str, field: &str) -> Result<(), ApiError> {
    if text.len() > MAX_RULE_TEXT_BYTES {
        return Err(ApiError::new(
            ErrorCode::TooLarge,
            format!("{what} is at most {MAX_RULE_TEXT_BYTES} bytes"),
        )
        .with_field(field));
    }
    Ok(())
}

/// An `InvalidInput` refusal of `field`.
fn invalid(message: impl Into<String>, field: &str) -> ApiError {
    ApiError::new(ErrorCode::InvalidInput, message).with_field(field)
}

/// `count + 1`, saturating.
const fn increment(count: usize) -> usize {
    count.saturating_add(1)
}

/// A count as the plan's `u32`.
fn count(value: usize) -> Result<u32, ApiError> {
    u32::try_from(value).map_err(|_| ApiError::new(ErrorCode::Internal, "a plan count overflows"))
}
