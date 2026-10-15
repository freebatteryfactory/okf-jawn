//! Human-readable naming conventions with preview and collision semantics.
//!
//! The titles on the rule types are the labels of the schema-driven form; they are short and
//! neutral, and the descriptions say what each one does.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::identity::MAX_PATH_SEGMENT_UTF16_UNITS;

/// Supported case transformations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LetterCase {
    /// Preserve case.
    #[schemars(title = "Keep as written")]
    Preserve,
    /// Lower case.
    #[schemars(title = "Lower case")]
    Lower,
    /// Upper case.
    #[schemars(title = "Upper case")]
    Upper,
    /// Title case.
    #[schemars(title = "Title case")]
    Title,
    /// Snake case.
    #[schemars(title = "Snake case")]
    Snake,
    /// Kebab case.
    #[schemars(title = "Kebab case")]
    Kebab,
}

/// Declared date interpretation; ambiguous input is not guessed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DateOrder {
    /// Year, month, day.
    #[schemars(title = "Year, month, day")]
    Ymd,
    /// Month, day, year.
    #[schemars(title = "Month, day, year")]
    Mdy,
    /// Day, month, year.
    #[schemars(title = "Day, month, year")]
    Dmy,
    /// Require unambiguous dates or a human choice.
    #[schemars(title = "Ask when unclear")]
    RejectAmbiguous,
}

/// How proposed naming collisions are handled.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollisionPolicy {
    /// Require an explicit decision.
    #[schemars(title = "Ask for a decision")]
    Reject,
    /// Add a deterministic suffix.
    #[schemars(title = "Add a number")]
    Suffix,
    /// Keep the original name and report the collision.
    #[schemars(title = "Keep the existing name")]
    KeepExisting,
}

/// How filename extensions are handled when applying a naming rule.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionPolicy {
    /// Leave the extension unchanged.
    #[schemars(title = "Keep")]
    Preserve,
    /// Lower-case the extension.
    #[schemars(title = "Lower case")]
    Lowercase,
    /// Drop the extension from the normalized note path (never from originals).
    #[schemars(title = "Remove")]
    Strip,
}

/// A typed rule scoped by glob and optional OKF type.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamingRule {
    /// Match against the supplied relative path.
    #[schemars(title = "Applies to paths")]
    pub glob: String,
    /// Optional OKF type selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(title = "Applies to type")]
    pub type_name: Option<String>,
    /// Case transformation.
    #[schemars(title = "Letter case")]
    pub case: LetterCase,
    /// Word separator.
    #[schemars(title = "Word separator")]
    pub separator: String,
    /// Optional literal prefix.
    #[schemars(title = "Prefix")]
    pub prefix: String,
    /// Optional literal suffix applied to the normalized base name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(title = "Suffix")]
    pub suffix: Option<String>,
    /// Declared incoming date convention.
    #[schemars(title = "Date order")]
    pub date_order: DateOrder,
    /// Explicit output format.
    #[schemars(title = "Date format")]
    pub date_format: String,
    /// Remove filename suffixes only from normalized notes, never originals.
    #[schemars(title = "Remove version suffixes")]
    pub strip_version_suffix: bool,
    /// How the filename extension is transformed on normalized notes.
    #[schemars(title = "File extension")]
    pub extension_policy: ExtensionPolicy,
    /// When true, preserve existing path aliases while renaming the primary path.
    #[schemars(title = "Keep old paths as aliases")]
    pub alias_preservation: bool,
    /// Relative destination template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(title = "Destination folder")]
    pub destination: Option<String>,
    /// Collision behavior.
    #[schemars(title = "When names collide")]
    pub collision: CollisionPolicy,
}

/// Versioned naming rules rendered as a form and editable as YAML.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamingRules {
    /// Format version; this release emits 1.
    #[schemars(title = "Format version")]
    pub schema_version: u32,
    /// Rules applied in explicit list order.
    #[schemars(title = "Rules")]
    pub rules: Vec<NamingRule>,
}

/// Read the workspace conventions at one version.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetRulesRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
}

/// The naming rules at one revision, or the stored file when it cannot be read as rules.
///
/// Never `internal` for a file a person or an import wrote: a file this build cannot read is
/// answered as `unreadable`, so a person can repair it in the YAML editor and save typed rules
/// with `set_rules`, which replaces the file whatever it held.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetRulesResponse {
    /// The revision the rules were read at.
    pub revision: crate::identity::Revision,
    /// The rules; absent when no rules were saved at `revision`, and when the stored file
    /// cannot be read as this build's rules (then `unreadable` is present).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<NamingRules>,
    /// The stored file and what was wrong with it; present exactly when a file is stored at
    /// `revision` and it cannot be read as this build's rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreadable: Option<UnreadableRules>,
}

/// A stored rules file this build cannot read as rules.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnreadableRules {
    /// The file's text as stored, at most `item::MAX_ITEM_TEXT_BYTES`; a longer file is
    /// `too_large`.
    pub text: String,
    /// What was wrong; a `schema_version` this build does not know is one such problem.
    pub problems: Vec<crate::common::Warning>,
}

/// Persist naming conventions as a versioned workspace file.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetRulesRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this change is based; stale writes conflict.
    pub base_revision: crate::identity::Revision,
    /// Validated rules.
    pub rules: NamingRules,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Which items a preview covers.
///
/// Archived items are never renamed: whichever selection is made, they are counted in the
/// plan's `archived_left_out`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NameSelection {
    /// The items named; at least one, or the request is `invalid_input` on
    /// `/selection/item_ids`.
    Items {
        /// Selected items.
        #[schemars(length(min = 1))]
        item_ids: Vec<crate::identity::ItemId>,
    },
    /// The items of one folder.
    Folder {
        /// The folder.
        folder: crate::identity::WorkspacePath,
        /// Also the items of every folder under it.
        recursive: bool,
    },
    /// Every item of the workspace.
    Workspace,
}

/// Calculate a rename plan with no persistent mutation.
///
/// A plan too large for one request body is applied folder by folder: `apply_names` refuses an
/// over-large body as every request does (`transport::MAX_REQUEST_BODY_BYTES`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewNamesRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Requested revision selector; latest is resolved once before reading.
    pub at: crate::identity::At,
    /// The items to preview.
    pub selection: NameSelection,
    /// Candidate rules, not necessarily saved.
    pub rules: NamingRules,
}

/// A proposed path change with an explanation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameEntry {
    /// Stable application item identity; paths remain the portable OKF identity.
    pub item_id: crate::identity::ItemId,
    /// Current path.
    pub from: crate::identity::WorkspacePath,
    /// Proposed path.
    pub to: crate::identity::WorkspacePath,
    /// Ambiguity or collision details.
    pub warnings: Vec<crate::common::Warning>,
}

/// One path observed as a duplicate during rename preview, including paths that differ only in
/// case or Unicode normalization (`WorkspacePath::collision_key`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DuplicateObservation {
    /// Path observed more than once.
    pub path: crate::identity::WorkspacePath,
    /// Item identities that collide on that path.
    pub item_ids: Vec<crate::identity::ItemId>,
    /// Human-readable observation without mutating content.
    pub message: String,
}

/// A preview bound to source state and exact conventions.
///
/// The counts are for the person reading the preview; `apply_names` does not check them.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenamePlan {
    /// Workspace revision inspected.
    pub base_revision: crate::identity::Revision,
    /// Digest of the exact canonical rules.
    pub rules_digest: crate::identity::Digest,
    /// Proposed changes: only the items whose path would change.
    pub entries: Vec<RenameEntry>,
    /// Duplicate path observations recorded during preview.
    pub duplicate_observations: Vec<DuplicateObservation>,
    /// Plan-level warnings.
    pub warnings: Vec<crate::common::Warning>,
    /// Items the selection holds that are not archived.
    pub considered: u32,
    /// Considered items whose path would not change; they are not in `entries`.
    pub unchanged: u32,
    /// Archived items the selection holds; they are never renamed.
    pub archived_left_out: u32,
}

/// Apply a revision-bound preview and rewrite affected links.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyNamesRequest {
    /// Workspace whose permissions and storage scope apply.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Plan revalidated before any writes.
    pub plan: RenamePlan,
    /// Rules whose digest must match the preview.
    pub rules: NamingRules,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

impl NameSelection {
    /// Refuse an `items` selection that names no item.
    ///
    /// `pointer` is the selection's JSON Pointer inside its request, such as `/selection`.
    ///
    /// # Errors
    /// Returns `InvalidInput` on `<pointer>/item_ids` for an `items` selection with no id.
    pub fn check(&self, pointer: &str) -> Result<(), crate::error::ApiError> {
        match self {
            Self::Items { item_ids } if item_ids.is_empty() => Err(crate::error::ApiError::new(
                crate::error::ErrorCode::InvalidInput,
                "an items selection names at least one item",
            )
            .with_field(format!("{pointer}/item_ids"))),
            Self::Items { .. } | Self::Folder { .. } | Self::Workspace => Ok(()),
        }
    }
}

/// The file name of the source card for an original named `original`.
///
/// Split at the last `.`: the stem, `-`, the extension, then `.md` (`report.pdf` gives
/// `report-pdf.md`, `v1.2.notes.txt` gives `v1.2.notes-txt.md`). A name with no `.`, or whose
/// only `.` is its first character, gets `.md` alone (`README` gives `README.md`, `.env` gives
/// `.env.md`). Case is kept. A collision then takes the numbered-suffix rule
/// (`numbered_source_card_name`: `report-pdf-2.md`) and the naming rules apply; the original
/// name is kept exactly as supplied in the card's application header.
///
/// The card's name always fits one path segment (`fit_segment`): a long stem is shortened and
/// `-<extension>.md` is kept whole. When that tail cannot fit beside a stem of at least one
/// character, the whole name is the stem and the tail is `.md`. So an import never fails because
/// an original's name is long.
#[must_use]
pub fn source_card_name(original: &str) -> String {
    card_name(original, "")
}

/// The file name of a later source card for an original named `original` in one folder, taken
/// when the earlier names are: the numbered-suffix rule puts `-<number>` after the extension
/// marker, so the card still says what it came from (`report.pdf` gives `report-pdf-2.md`,
/// `README` gives `README-2.md`). That is the source-card tail `-<ext>-<n>` the naming rules
/// read back.
///
/// Like `source_card_name`'s, the name fits one path segment: the stem is shortened and
/// `-<extension>-<number>.md` is kept whole; when that tail cannot fit beside a stem of at least
/// one character, the whole name is the stem and the tail is `-<number>.md`. A `number` below 2
/// names the first card, which carries no number: `source_card_name`'s.
#[must_use]
pub fn numbered_source_card_name(original: &str, number: u32) -> String {
    if number < 2 {
        return source_card_name(original);
    }
    card_name(original, &format!("-{number}"))
}

/// `stem` and `tail` as one path segment of at most `identity::MAX_PATH_SEGMENT_UTF16_UNITS`
/// UTF-16 code units, measured on their NFC forms.
///
/// `tail` is kept whole (a card's `-pdf.md`, a numbered card's `-pdf-2.md`); `stem` is
/// shortened at a character boundary, so a surrogate pair is never split, and the trailing dots
/// and spaces a cut leaves are trimmed, since a segment may not end in either. A stem that fits
/// is kept as it is.
/// The result is in NFC. A tail longer than the bound is the caller's to avoid
/// (`source_card_name` and `numbered_source_card_name` do).
#[must_use]
pub fn fit_segment(stem: &str, tail: &str) -> String {
    let stem: String = stem.nfc().collect();
    let tail: String = tail.nfc().collect();
    let room = MAX_PATH_SEGMENT_UTF16_UNITS.saturating_sub(units(&tail));
    if units(&stem) <= room {
        return format!("{stem}{tail}");
    }
    let mut kept = String::new();
    let mut used = 0_usize;
    for character in stem.chars() {
        let width = character.len_utf16();
        if used.saturating_add(width) > room {
            break;
        }
        used = used.saturating_add(width);
        kept.push(character);
    }
    let kept = kept.trim_end_matches(['.', ' ']);
    format!("{kept}{tail}")
}

/// A source card's name with `count` (empty, or `-<n>`) after its extension marker, as
/// `source_card_name` and `numbered_source_card_name` describe it.
fn card_name(original: &str, count: &str) -> String {
    if let Some((stem, extension)) = original.rsplit_once('.')
        && let Some(first) = stem.chars().next()
    {
        let tail = format!("-{extension}{count}.md");
        let first: String = std::iter::once(first).nfc().collect();
        if units(&first).saturating_add(units_nfc(&tail)) <= MAX_PATH_SEGMENT_UTF16_UNITS {
            return fit_segment(stem, &tail);
        }
    }
    fit_segment(original, &format!("{count}.md"))
}

/// The UTF-16 code units of `text`.
fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The UTF-16 code units of `text`'s NFC form.
fn units_nfc(text: &str) -> usize {
    text.nfc().map(char::len_utf16).sum()
}
