# core M1.3b design: the naming rules

Phase 1 of core-m1-3b-brief.md: a written design, no production code, no build. Read on main at
c795015 (read-only). The contract is designed as contract-batch-2-rulings.md entries 10, 11, 12
and 28 will leave it on main: `PreviewNamesRequest.selection: NameSelection` (`Items`,
`Folder { folder, recursive }`, `Workspace`), `RenamePlan.{considered, unchanged,
archived_left_out}`, `GetRulesResponse { revision, rules, unreadable }`, schemars titles, and
`get_rules` as the model tool `rules`. Where today's contract types differ, the rulings win.

Nothing in this file is built. Every rule below names the tests phase 2 writes for it (section
12). Choices a person could reasonably disagree with are collected in section 11, each with a
proposal; the body of the design uses the proposal.

---

## 1. Decisions in one page

1. **First match.** Rules are tried in list order; the first rule whose `glob` and `type_name`
   both match an item names it. No other rule is tried for that item (section 4.1).
2. **Glob** is the `glob` crate's shell pattern, matched against the item's whole workspace path
   with `*`, `?` and `[...]` never crossing `/`, and compared case- and normalization-
   insensitively: the glob text is put in NFC and lower-cased exactly as
   `WorkspacePath::collision_key` does, and matched against the path's `collision_key()`. Two
   paths that collide are therefore always matched alike.
3. **A name is built in a fixed order**: split the file name (and a source card's
   original-extension tail), strip an existing prefix and suffix, strip version suffixes, find
   dates, split into words, apply the case, join with the separator, add prefix and suffix, add
   the extension tail, choose the folder, validate the path, resolve collisions.
4. **Word breaks** are runs of space, `-` and `_` only (convert_case's `Space`, `Hyphen`,
   `Underscore` boundaries). No camelCase or letter/digit splitting. Other symbols stay inside
   their word.
5. **A date in a name is one protected word**: it is never split, re-cased or re-joined by the
   word rules. It is rewritten only when it is read without doubt and `date_format` is not empty.
6. **An ambiguous date leaves the whole item unchanged**, flagged in the plan's warnings; it is
   never guessed.
7. **`date_format` and `destination` use the `time` crate's format-description syntax, version
   3** (`[year]-[month]-[day]`, `meetings/[year]`). An empty `date_format` keeps dates as
   written.
8. **`extension_policy` acts only on a source card's original-extension tail** (`-pdf` in
   `report-pdf.md`). The item file extension is always written `.md`. The original file is never
   touched.
9. **Collisions** are compared by `collision_key` against the final state of the tree.
   `suffix` adds `-2`, `-3`, ... before `.md`; `keep_existing` lets the current holder or the
   first claimant keep the name; `reject` gives a contested name to nobody. Every collision is a
   `DuplicateObservation`.
10. **`apply_names` recomputes the plan** for the plan's own items at its base with the request's
    rules and refuses the whole apply unless every entry is identical; then it writes one commit
    of `MoveItem`s, through a temporary path where a move must wait for another (a case-only
    rename, a chain, a swap).
11. **The shared function** `conventions::propose` names one path; `preview_names` and the
    import's `name_new_item` both call it and the same `numbered`, so they cannot differ.
12. **Libraries**: `glob` 0.3.4 (patterns), `convert_case` 0.12.0 (words and letter case),
    `regex` 1.13.1 (recognizing dates and version suffixes), `time` 0.3.55 (date validity and
    formats; already core's), `unicode-normalization` 0.1.25 (NFC of the glob text). No
    homegrown matcher, case mapper or date parser.

---

## 2. Terms and notation

- **key(p)**: `WorkspacePath::collision_key(p)`: the NFC value lower-cased by `str::to_lowercase`.
- **item path**: an item's workspace path at the revision read (always NFC).
- **stem**: the file name without its final `.md`.
- **mover / stayer**: an item the plan would give a new path / an item that keeps its path
  (unselected, archived, unmatched, unchanged, flagged, refused).
- **holder of key K**: an item whose current path has key K.
- **R0**, the baseline rule every vector starts from unless it says otherwise:

```yaml
glob: "**"
case: kebab
separator: "-"
prefix: ""
date_order: ymd
date_format: ""
strip_version_suffix: false
extension_policy: preserve
alias_preservation: false
collision: suffix
```

(`type_name`, `suffix` and `destination` absent.) `DF` below means
`date_format: "[year]-[month]-[day]"`.

- A vector is written `path [type; original] | rule change from R0 | result`. Type is `note`
  unless given; `original` is a source card's `SourceHeader.original_name`. A result is the new
  path, `unchanged`, `unmatched` (unchanged, no rule matched), or a refusal
  `code field "message gist"`. `W(code)` is a warning with that code on the item.

A complete rules file, as `set_rules` stores it in `.okf/rules.yaml`:

```yaml
schema_version: 1
rules:
  - glob: "inbox/**"
    type_name: meeting
    case: kebab
    separator: "-"
    prefix: ""
    date_order: dmy
    date_format: "[year]-[month]-[day]"
    strip_version_suffix: true
    extension_policy: lowercase
    alias_preservation: true
    destination: "meetings/[year]"
    collision: suffix
  - glob: "**"
    case: title
    separator: " "
    prefix: ""
    date_order: reject_ambiguous
    date_format: ""
    strip_version_suffix: false
    extension_policy: preserve
    alias_preservation: false
    collision: keep_existing
```

`date_format` and `destination` start with `[`, so in hand-written YAML they must be quoted (a
plain scalar starting with `[` is a flow sequence). The UI's YAML writer quotes them already.

---

## 3. What a name is made of

An item path `P` is split as:

1. **folder F**: everything before the last `/` (empty at the root);
2. **file name N**: everything after it;
3. **stem S**: N without a final `.md`. If N does not end with exactly `.md` (lower case) the
   item gets `W(item_not_movable)` and is unchanged: storage accepts `X.MD` as an item, but
   okf-core loads only files whose extension is exactly `md` and `ConceptId::to_path` writes
   `.md`, so such an item cannot be moved (section 10, concern K3).
4. **source-card tail** (source cards only, section 4.7): a trailing `-<ext>` or `-<ext>-<n>`
   that `source_card_name` and the import's numbering wrote.
5. **base B**: what remains; the rules transform B.

The new file name is `prefix + B' + suffix + tail' + ".md"` in the folder the rule chooses.

---

## 4. The algorithm, field by field, in the order a name is built

### 4.1 Which items a rule applies to: `glob`, `type_name`, list order

Algorithm (`select(rules, P, type)`):

1. For each rule `i` in list order:
   1. `glob_key = nfc(rule.glob).to_lowercase()`; `pattern = glob::Pattern::new(glob_key)`
      (compiled once at validation);
   2. `globbed = pattern.matches_with(key(P), MatchOptions { case_sensitive: true,
      require_literal_separator: true, require_literal_leading_dot: false })`;
   3. `typed = rule.type_name is None || rule.type_name == type` (exact, case-sensitive string
      comparison of the item's OKF `type`, `ItemSummary::type_name`);
   4. if `globbed && typed`, rule `i` names the item; stop.
2. No rule matched: the item is `unmatched` (counted in `unchanged`).

Glob syntax (the `glob` crate's, section 9.1):
- `?` one character; `*` any run of characters except `/`; `**` as a whole path component
  matches any number of components, zero included (`**/x.md` matches `x.md`; `notes/**`
  matches everything under `notes/`; `**` alone matches everything); `[abc]`, `[a-z]`,
  `[!a-z]` one character in / not in the set; `[?]`, `[*]`, `[[]` match the metacharacter.
- The glob is matched against the **whole** path from the workspace root. `*.md` matches only
  root items; write `**/*.md` for every depth.
- `{a,b}` has no meaning: braces are literal characters (the crate has no alternation).
- Refused at validation (section 6): empty; a leading `/` (globs are already relative to the
  root); a trailing `/` (a glob matches item paths; write `notes/**`); any `\` (the crate treats
  `\` as a separator on Windows only, so the same glob would match differently by platform);
  longer than 1,024 bytes; and whatever `Pattern::new` refuses (`**` not a whole component,
  `***`, an unclosed `[`), with the crate's message and position.
- A glob that matches nothing is not an error: the rule names nothing. (A rule may be meant for
  items outside the selection.)

Why first match: every field of `NamingRule` is required, so a rule cannot leave a field
"unset" for a later rule to fill; applying every matching rule would chain one rule's output
into the next rule's input, which no person reads as the meaning of a rule list. First match
makes the rule that named an item the rule the person reads (contract: "Rules applied in
explicit list order").

Vectors (`glob`):

| # | path | rule change | result |
|---|---|---|---|
| G1 | `notes/Meeting Notes.md` | glob `notes/*.md` | `notes/meeting-notes.md` |
| G2 | `notes/2025/Plan A.md` | glob `notes/*.md` | unmatched (`*` does not cross `/`) |
| G3 | `notes/Sub/Plan A.md` | glob `NOTES/**` | `notes/Sub/plan-a.md` (case-insensitive; folders are not renamed) |
| G4 | `notes/Plan A.md`; `Plan A.md` | glob `*.md` | unmatched; `plan-a.md` |
| G5 | `notes/Draft X.md`; `notes/Alpha.md` | glob `notes/[!a-c]*.md` | `notes/draft-x.md`; unmatched |
| G6 | `Archiv/über uns.md` | glob `**/Über*.md` | `Archiv/über-uns.md` (non-ASCII case-insensitive, which the crate alone cannot do) |
| G7 | `É.md` | glob `?.md` | `é.md` (`?` is one character; a case-only rename) |
| G8 | `notes/{a,b}.md`; `notes/a.md` | glob `notes/{a,b}.md` | `notes/{a,b}.md` matched (unchanged by kebab); `notes/a.md` unmatched |
| G9 | any | glob `` | `invalid_input /rules/rules/0/glob "a glob must not be empty"` |
| G10 | any | glob `/notes/*.md` | `invalid_input /rules/rules/0/glob "a glob is relative to the workspace root; leave out the leading /"` |
| G11 | any | glob `notes/` | `invalid_input /rules/rules/0/glob "a glob matches item paths; write notes/** for everything under notes"` |
| G12 | any | glob `notes/**x` | `invalid_input /rules/rules/0/glob "recursive wildcards must form a single path component (at 8)"` |
| G13 | any | glob `notes/[ab` | `invalid_input /rules/rules/0/glob "invalid range pattern (at 6)"` |
| G14 | any | glob `notes\*.md` | `invalid_input /rules/rules/0/glob "a glob uses / between folders, never \\"` |

Vectors (`type_name`):

| # | path [type] | rule change | result |
|---|---|---|---|
| T1 | `Weekly Sync.md` [meeting] | type_name `meeting` | `weekly-sync.md` |
| T2 | `Weekly Sync.md` [Meeting] | type_name `meeting` | unmatched (exact comparison) |
| T3 | `Weekly Sync.md` [anything] | type_name absent | `weekly-sync.md` |
| T4 | `Report-pdf.md` [source; `Report.pdf`] | type_name `source` | `report-pdf.md` (cards are matched by their `source` type, at import too) |
| T5 | any | type_name `""` | `invalid_input /rules/rules/0/type_name "leave type_name out to match every type"` |

Vectors (list order):

| # | path | rules | result |
|---|---|---|---|
| O1 | `notes/Plan A.md`; `Plan A.md` | `[ {R0, glob notes/**, case upper, separator _}, R0 ]` | `notes/PLAN_A.md` (rule 0); `plan-a.md` (rule 1) |
| O2 | `Plan A.md` [note] | `[ {R0, type_name meeting, case upper, separator _}, R0 ]` | `plan-a.md` (rule 0 does not match the type) |
| O3 | `keep/My File.md` | `[ {R0, glob keep/**, case preserve, separator " "}, R0 ]` | unchanged: rule 0 names it and leaves it as it is; rule 1 is never tried |
| O4 | any | `rules: []` | unmatched; the plan has no entry |

### 4.2 The base name: existing `prefix` and `suffix` are taken off first

So that a rule is applied once and never twice (brief: "a name that already has them is not
prefixed twice"), and so that the word rules never re-shape the literal prefix or suffix:

1. If `prefix` is not empty and B begins with it, compared as `to_lowercase()` of the first
   `prefix.chars().count()` characters of B against `prefix.to_lowercase()`, and something is
   left after it, B loses those characters.
2. Then the same for `suffix` at the end of B (last `suffix.chars().count()` characters).

They are added back, literally, in section 4.6.

### 4.3 `strip_version_suffix`

When true, with this regular expression (regex crate, case-insensitive, anchored at the end):

```
(?i)(?:[ _-]+(?:v[0-9]+(?:\.[0-9]+)*|final|copy(?: [0-9]+)?)| *\([0-9]+\))$
```

1. Find the leftmost match in B.
2. If there is one and the text before it is not empty, B becomes that text; repeat from 1.
3. Otherwise stop.

So the version suffixes are exactly: a run of spaces, `-` or `_` followed by `v<digits>` with
optional `.<digits>` groups (`_v2`, `-V3.1`, ` v10.0.1`), by `final`, by `copy` or `copy <n>`;
and an optional run of spaces followed by `(<n>)` (` (1)`, `(12)`). Any letter case. Several in a
row are all removed (`report_v2_final (1)` gives `report`). Never removed: a suffix with no break
before it (`reportv2`), one that would leave nothing (`final`, `(1)`), and words that carry
meaning (`draft`, `old`, `new`, `latest`, `rev2`, a bare `-2`). The Windows form ` - Copy (2)`
is covered by the two alternatives in turn.

"Never originals" (contract doc): this changes the item's name only; a source's original name is
in its header and its appearance, which no rename touches.

Vectors (R0 + `strip_version_suffix: true` unless stated):

| # | path | rule change | result |
|---|---|---|---|
| V1 | `Budget_v2.md` | | `budget.md` |
| V2 | `Budget - Copy (2).md` | | `budget.md` (` (2)` then ` - Copy`) |
| V3 | `Budget final v3.md` | | `budget.md` (` v3` then ` final`) |
| V4 | `Budgetv2.md` | | `budgetv2.md` (no break before `v2`) |
| V5 | `final.md` | | unchanged (nothing would be left) |
| V6 | `Budget_v2.md` | `strip_version_suffix: false` | `budget-v2.md` |
| V7 | `Budget (1)-pdf.md` [source; `Budget (1).pdf`] | | `budget-pdf.md` |
| V8 | `Ad copy.md` | | `ad.md` (a known cost of `copy`; the preview shows it) |

### 4.4 Dates: `date_order` and `date_format`

**Recognition** (always, whatever `date_format` is). Find date-shaped tokens in B with

```
(?:^|[^0-9])([0-9]{4}[-_.][0-9]{1,2}[-_.][0-9]{1,2}|[0-9]{1,2}[-_.][0-9]{1,2}[-_.][0-9]{4}|[0-9]{8})(?:[^0-9]|$)
```

searched with `Regex::captures_at(B, start)`, starting at 0 and continuing at the end of group 1
of the previous match (so two dates separated by one character are both found; the regex crate
has no look-around). Group 1 is a candidate, then classified by three anchored patterns:

- **Y** (year first): `^([0-9]{4})([-_.])([0-9]{1,2})([-_.])([0-9]{1,2})$`, both separators
  equal;
- **L** (year last): `^([0-9]{1,2})([-_.])([0-9]{1,2})([-_.])([0-9]{4})$`, both separators equal;
- **C** (compact): `^([0-9]{4})([0-9]{2})([0-9]{2})$`.

A Y or L token is **date-shaped** whether or not it is a valid date. A C token is date-shaped
only when it is a valid `YYYYMMDD` date (an eight-digit run is usually an identifier). A token
with mixed separators (`2025-03_04`) is ordinary text. Two-digit years (`03-04-25`), month names
and spaces as separators are not recognized at all (a two-digit year's century would be a
guess). Only ASCII digits count.

**Reading** (`time::Date::from_calendar_date(year, Month::try_from(month)?, day)`):

| shape | `ymd` | `mdy` | `dmy` | `reject_ambiguous` |
|---|---|---|---|---|
| Y `y-m-d` | read Y-M-D | read Y-M-D | read Y-M-D | read Y-M-D |
| C `yyyymmdd` | read | read | read | read |
| L `a-b-y` | not read | read as M-D-Y (month a) | read as D-M-Y (day a) | mdy and dmy readings: both valid and different: **ambiguous**; exactly one valid, or both the same date (a = b): read it; neither: not read |

A Y or L token whose reading is not a calendar date (`2025-02-30`, `13-04-2025` under `mdy`) is
**not read**. Year-first and compact tokens are read Y-M-D under every order: ISO 8601 writes
dates that way and no declared order changes it.

**Use**:

1. If `date_format` is not empty: every **read** token is replaced by
   `date.format(&date_format)`; every **not read** token is kept exactly as written and the item
   gets `W(date_not_read)`; an **ambiguous** token flags the item.
2. If `date_format` is empty: every date-shaped token is kept exactly as written, with no
   warning. Only a destination that needs a date reads one (the first token, section 4.8), and
   an ambiguous first token then flags the item.
3. A flagged item is not renamed at all: no entry; one `W(date_ambiguous)` per ambiguous token
   in the plan's `warnings`, located at the item's path (SPEC section 6: "Ambiguous dates are
   flagged instead of guessed"; SPEC section 3: "stays flagged, not converted"). So ambiguity
   flags only a token something would convert: with an empty `date_format` and no date-needing
   destination nothing is converted, so nothing is flagged (D11).
4. Every date-shaped token, rewritten or kept, becomes one **protected word** for section 4.5.

**`date_format` syntax**: the `time` crate's format description, version 3
(`time::format_description::parse_owned::<3>`). A person writes components in brackets and any
other text literally: `[year]`, `[month]`, `[day]`, with modifiers such as `[month repr:short]`,
`[day padding:none]`, `[year repr:last_two]`; `\[`, `\]` and `\\` escape. Examples:
`"[year]-[month]-[day]"` gives `2025-03-04`; `"[day].[month].[year]"` gives `04.03.2025`;
`"[year][month][day]"` gives `20250304`. Empty means "keep dates as written" (never "delete
them"). Validation (section 6): it must parse; formatting the probe dates 2025-03-04,
1999-12-31 and 0001-01-01 with it must succeed (a time-of-day or offset component cannot format
a `Date`: `InsufficientTypeInformation`); each output must be non-empty, contain no `/`, and be
valid inside a path segment (`WorkspacePath::try_from(format!("a{out}b"))`).

Vectors (`date_order`; DF unless stated):

| # | path | rule change | result |
|---|---|---|---|
| D1 | `Minutes 2025.3.4.md` | DF | `minutes-2025-03-04.md` |
| D2 | `Minutes 03-04-2025.md` | DF, `mdy`; then `dmy` | `minutes-2025-03-04.md`; `minutes-2025-04-03.md` |
| D3 | `Minutes 03-04-2025.md` | DF, `reject_ambiguous` | unchanged; plan warning `W(date_ambiguous)` at `Minutes 03-04-2025.md` |
| D4 | `Minutes 13-04-2025.md` | DF, `reject_ambiguous` | `minutes-2025-04-13.md` (only D-M-Y is a date) |
| D5 | `Minutes 04-04-2025.md` | DF, `reject_ambiguous` | `minutes-2025-04-04.md` (both readings are one date) |
| D6 | `Minutes 03-04-2025.md` | DF, `ymd` | `minutes-03-04-2025.md` + `W(date_not_read)` (kept, protected) |
| D7 | `Minutes 13-04-2025.md` | DF, `mdy` | `minutes-13-04-2025.md` + `W(date_not_read)` |
| D8 | `IMG_20250304_1234.md` | DF | `img-2025-03-04-1234.md` |
| D9 | `scan 2025-02-30.md` | DF | `scan-2025-02-30.md` + `W(date_not_read)` |
| D10 | `Q3 03-04-25.md` | DF | `q3-03-04-25.md` (two-digit year: not a date, no warning) |
| D11 | `Minutes 03-04-2025.md` | `date_format ""`, `reject_ambiguous` | `minutes-03-04-2025.md` (nothing used, nothing flagged) |

Vectors (`date_format`):

| # | path | rule change | result |
|---|---|---|---|
| F1 | `Minutes 2025.3.4.md` | `date_format ""` | `minutes-2025.3.4.md` (kept as written, not split by kebab) |
| F2 | `Minutes 2025-03-04.md` | `"[day].[month].[year]"` | `minutes-04.03.2025.md`; previewed again under `ymd`: unchanged + plan `W(date_not_read)` |
| F3 | `Report 2025-3-4 final.md` | `"[year][month][day]"` | `report-20250304-final.md`; previewed again: unchanged |
| F4 | any | `"[month repr:short] [year]"` | `invalid_input /rules/rules/0/date_format "a date format writes only numbers: [year], [month], [day]"` (ruling: a formatted date must read again as the same date, so a second preview leaves it as it is) |
| F7 | `Report 2025-03-04.md`, previewed twice | `"[year]-[month]-[day]"` | first preview `report-2025-03-04.md`; second preview: unchanged |
| F5 | any | `"[year]-[month"` | `invalid_input /rules/rules/0/date_format "not a date format: unclosed [ (at 7)"` |
| F6 | any | `"[hour]"` | `invalid_input /rules/rules/0/date_format "names a part a date does not have"` |
| F7 | any | `"[year]/[month]"` | `invalid_input /rules/rules/0/date_format "the date would hold /"` |
| F8 | any | `"[invalid]"` | `invalid_input /rules/rules/0/date_format "not a date format: invalid component name invalid (at 1)"` |

### 4.5 Words: `case` and `separator`

1. B is now a sequence of text runs and protected words (dates).
2. Each text run is split with `convert_case::split(run, &[Boundary::Space, Boundary::Hyphen,
   Boundary::Underscore])`; empty words are dropped (`Pattern::RemoveEmpty`). Every other
   character (`.`, `'`, `(`, `&`, digits, letters of any script) stays inside its word; there is
   no camelCase and no letter/digit split.
3. The text words are mapped by `case`, with convert_case's patterns (`Pattern::mutate`):
   - `preserve`: unchanged;
   - `lower`, `snake`, `kebab`: `Pattern::Lowercase` (`str::to_lowercase`: full Unicode,
     final sigma: `ΟΔΥΣΣΕΥΣ` gives `οδυσσευς`);
   - `upper`: `Pattern::Uppercase` (`str::to_uppercase`: `straße` gives `STRASSE`);
   - `title`: `Pattern::Custom(title_words)`: a word that has at least one uppercase character
     (`char::is_uppercase`) is kept as written; any other word gets its first character
     upper-cased (`char::to_uppercase`) and the rest unchanged. So `NASA`, `iPhone` and `macOS`
     stay, `report` becomes `Report`, `über` becomes `Über`. (Known limit: `char::to_uppercase`
     is not title-case mapping, so a word starting `ǆ` becomes `Ǆ...`, not `ǅ...`; and a word
     starting with a symbol is unchanged: `(draft)` stays `(draft)`.)
4. All words, text and protected, in their order, are joined with `separator`.
5. An empty result gives `W(name_left_empty)` and the item is unchanged.

`separator` is any string (empty included) such that `a{separator}b` is a valid NFC path
segment: no `/`, no `\ : < > " | ? *`, no control character, and not starting with a combining
mark. `snake` requires `_` and `kebab` requires `-` (section 11, Q5): a snake rule joined by `-`
would not be snake case, and refusing the contradiction is better than silently choosing one
field. At most 1,024 bytes.

Idempotence: lower/upper/title applied twice give the same result; a separator that is itself a
break (` `, `-`, `_`) re-splits into the same words; any other separator makes the joined result
one word next time, which the same case leaves as it is.

Vectors (`case`):

| # | path | rule change | result |
|---|---|---|---|
| C1 | `Meeting Notes.md` | (R0 kebab) | `meeting-notes.md` |
| C2 | `Meeting  Notes - Q3.md` | `snake`, `_` | `meeting_notes_q3.md` |
| C3 | `straße plan.md` | `upper`, `_` | `STRASSE_PLAN.md` |
| C4 | `ΟΔΥΣΣΕΥΣ Notes.md` | `lower`, `" "` | `οδυσσευς notes.md` |
| C5 | `the NASA iPhone report.md` | `title`, `" "` | `The NASA iPhone Report.md` |
| C6 | `über uns.md` | `title`, `" "` | `Über Uns.md` |
| C7 | `Meeting Notes.md` | `preserve`, `_` | `Meeting_Notes.md` |
| C8 | `XMLHttpRequest notes.md` | (R0) | `xmlhttprequest-notes.md` (no camelCase split) |
| C9 | `Q3 v2 plan.md` | (R0) | `q3-v2-plan.md` (no digit split) |
| C10 | `Tom's notes (draft).md` | (R0) | `tom's-notes-(draft).md` |
| C11 | any | `snake`, `-` | `invalid_input /rules/rules/0/separator "snake case joins words with _"` |

Vectors (`separator`):

| # | path | rule change | result |
|---|---|---|---|
| S1 | `meeting notes.md` | `title`, `""` | `MeetingNotes.md`; previewed again: unchanged |
| S2 | `Meeting Notes.md` | `lower`, `.` | `meeting.notes.md`; previewed again: unchanged |
| S3 | `meeting_notes-final.md` | `preserve`, `" "` | `meeting notes final.md` |
| S4 | any | `lower`, `/` | `invalid_input /rules/rules/0/separator "a separator cannot hold /"` |
| S5 | any | `lower`, `:` | `invalid_input /rules/rules/0/separator "path segment holds a character Windows refuses ..."` (the contract's own message) |
| S6 | any | `lower`, `"\t"` | `invalid_input /rules/rules/0/separator` (control character) |
| S7 | any | `lower`, `"́"` | `invalid_input /rules/rules/0/separator "path must be in Unicode NFC"` |

### 4.6 `prefix` and `suffix` (added)

1. `B' = prefix + joined words + (suffix or "")`, both literal: never split, re-cased or
   re-joined.
2. `prefix` must make `{prefix}a.md` a valid path segment and hold no `/`; `suffix` must make
   `a{suffix}.md` one. Both at most 1,024 bytes. (A prefix `CON.` is refused: the segment's stem
   would be the device name `CON`.)
3. Section 4.2 took any existing prefix or suffix off first, so applying a rule twice changes
   nothing.

Vectors (`prefix`):

| # | path | rule change | result |
|---|---|---|---|
| P1 | `Weekly sync.md` | prefix `MTG-` | `MTG-weekly-sync.md` |
| P2 | `mtg-weekly-sync.md` | prefix `MTG-` | `MTG-weekly-sync.md` (prefix found case-insensitively, not doubled; a case-only rename) |
| P3 | `MTG-weekly-sync.md` | prefix `MTG-` | unchanged |
| P4 | `2025 plan.md` | prefix `"2025 "` | unchanged (the prefix keeps its space; without step 4.2 it would become `2025 2025-plan.md`) |
| P5 | any | prefix `a/b` | `invalid_input /rules/rules/0/prefix "a prefix cannot hold /"` |
| P6 | any | prefix `CON.` | `invalid_input /rules/rules/0/prefix "path segment is a name Windows reserves for a device"` |

Vectors (`suffix`):

| # | path | rule change | result |
|---|---|---|---|
| X1 | `Board meeting.md` | `snake`, `_`, suffix `_notes` | `board_meeting_notes.md` |
| X2 | `board_meeting_notes.md` | `snake`, `_`, suffix `_notes` | unchanged |
| X3 | `report-pdf.md` [source; `report.pdf`] | suffix `-ed` | `report-ed-pdf.md` (the suffix belongs to the base, before the card tail) |
| X4 | any | suffix `x\|` | `invalid_input /rules/rules/0/suffix "path segment holds a character Windows refuses ..."` |

### 4.7 The extension: `extension_policy`, and how a source card's name is read

A source card's name is `conventions::source_card_name(original)`: the original's stem, `-`, the
original's extension, `.md` (`report.pdf` gives `report-pdf.md`); the import adds `-<n>` before
`.md` on a collision (`report-pdf-2.md`). The rules apply on top of that name, reading it back
the same way:

1. `E` = the original's extension as `source_card_name` splits it: `original.rsplit_once('.')`
   with a non-empty stem; none for `README` or `.env`.
2. If E exists and S ends with `-T` or `-T-n`, where `T.to_lowercase() == E.to_lowercase()`, `n`
   is a decimal number of at least 2 with no leading zero, and what precedes is not empty: the
   tail is (T as written, n). B = S without the tail. Otherwise the card has no tail (a person
   renamed it) and the extension policy has nothing to act on.
3. After the base is built, `tail'` is:
   - `preserve`: `-T` (as written now);
   - `lowercase`: `-` + `T.to_lowercase()`;
   - `strip`: nothing;
   - then `-n` if the card had a number.
4. Notes and Views have no tail; `extension_policy` does not affect them.
5. The item's own extension is always written `.md` (lower case): it is what okf-core reads and
   writes for a concept (section 3).
6. "Never applied to an original" (SPEC section 6, "Preserve originals"): the original's bytes,
   its `SourceHeader.original_name` and its `SourceAppearance.names` are never touched by a
   rename; only the card's workspace path changes.

So a card that was never renamed by hand reads and writes exactly `source_card_name`'s shape,
and a card a person renamed keeps that person's name as the input (section 11, Q13).

Vectors (`extension_policy`):

| # | path [type; original] | rule change | result |
|---|---|---|---|
| E1 | `Q3 Report-PDF.md` [source; `Q3 Report.PDF`] | `preserve` | `q3-report-PDF.md` |
| E2 | same | `lowercase` | `q3-report-pdf.md` |
| E3 | same | `strip` | `q3-report.md` |
| E4 | `Plan.md` [note] | `strip` | `plan.md` (no effect on a note) |
| E5 | `report-pdf-2.md` [source; `report.pdf`] | `strip` | `report-2.md` (the import's number is kept) |
| E6 | `Board deck.md` [source; `deck.pptx`] | `strip` | `board-deck.md` (no `-pptx` tail: renamed by a person) |
| E7 | `README.md` [source; `README`] | `lowercase` | `readme.md` |
| E8 | `.env.md` [source; `.env`] | `strip` | unchanged |
| E9 | `Plan.MD` [note] | any | unchanged + `W(item_not_movable)` |

### 4.8 The folder: `destination`

1. Absent: the item stays in its folder F.
2. Present: a `time` format description, version 3 (same syntax as `date_format`), whose literal
   text is a folder path relative to the workspace root and whose components are parts of a
   date: `meetings`, `meetings/[year]`, `archive/[year]/[month]`.
3. The destination **needs a date** when formatting it with the probe dates 2025-03-04 and
   1999-12-31 gives different text (any date component does: they differ in year, month, day,
   weekday, ordinal and week). This is decided once, at validation.
4. If it needs a date: the date is that of the **first** date-shaped token in B (leftmost):
   read: format the destination with it; ambiguous: the item is flagged (section 4.4); not read
   or none: the item stays in F with `W(destination_needs_date)` and the rest of its rename
   applies.
5. If it does not need a date: the destination's text is the folder.
6. The result D must pass `WorkspacePath::from_supplied(D)` (NFC, Windows-reserved segments, at
   most 4,096 bytes) and have no segment whose key is `.okf`, `index.md` or `log.md`. A result
   that fails gives `W(path_not_valid)` / `W(path_reserved)` and the item is unchanged.

No `{type}` placeholder: a type-dependent destination is a rule with that `type_name` and its own
destination (SPEC section 6 "type-dependent destinations"). No "original folder" placeholder:
leaving `destination` out keeps the folder. An empty destination is refused, because a form may
send an empty field for "none" and an empty destination meaning "the root" would move everything
there; so the root is not a destination (section 11, Q11).

Validation (section 6): not empty; parses; formats the probe dates; each output is a valid,
non-reserved folder path; a date-needing destination is allowed with an empty `date_format`
(the date is then read for the folder only).

Vectors (`destination`):

| # | path [type] | rule change | result |
|---|---|---|---|
| DS1 | `inbox/Weekly sync.md` [meeting]; same [note] | glob `inbox/*.md`, type_name `meeting`, destination `meetings` | `meetings/weekly-sync.md`; unmatched |
| DS2 | `inbox/Sync 2025-03-04.md` | DF, destination `meetings/[year]` | `meetings/2025/sync-2025-03-04.md` |
| DS3 | `inbox/Sync.md` | DF, destination `meetings/[year]` | `inbox/sync.md` + `W(destination_needs_date)` |
| DS4 | `inbox/Sync 03-04-2025.md` | `date_format ""`, `mdy`, destination `meetings/[year]` | `meetings/2025/sync-03-04-2025.md` |
| DS5 | `inbox/Sync 03-04-2025.md` | `reject_ambiguous`, destination `meetings/[year]` | unchanged + plan `W(date_ambiguous)` |
| DS6 | any | destination `""` | `invalid_input /rules/rules/0/destination "leave destination out to keep each item's folder"` |
| DS7 | any | destination `.okf/x`; `notes/index.md` | `invalid_input /rules/rules/0/destination "... is kept by the server"` |
| DS8 | any | destination `/meetings`; `meetings/`; `CON/x` | `invalid_input /rules/rules/0/destination` with the contract's path message |
| DS9 | any | destination `[hour]` | `invalid_input /rules/rules/0/destination "names a part a date does not have"` |

### 4.9 Validating the new path, and what stays

1. `P' = D + "/" + name` (or `F + "/" + name`, or `name` at the root).
2. `WorkspacePath::from_supplied(P')` must succeed: otherwise `W(path_not_valid)` with the
   contract's message, unchanged.
3. No segment of P' may have the key `.okf`, `index.md` or `log.md`: otherwise
   `W(path_reserved)`, unchanged. (Reserved files are not items, so they are never renamed; this
   step stops an item from being renamed onto one.)
4. If `P' == P` byte for byte: unchanged, not an entry.
5. Archived items (`ItemSummary::archived`) are never renamed: they are counted in
   `archived_left_out` and are stayers (their keys stay taken).

Vectors (what stays):

| # | path | rule change | result |
|---|---|---|---|
| W1 | `a/Report.md` (archived); `a/REPORT v2.md` | strip true | archived: left out (`archived_left_out` 1); `a/REPORT v2.md` gives `a/report-2.md` (the archived item holds `a/report.md`) |
| W2 | `LOG v2.md` | strip true, `lower` | unchanged + `W(path_reserved)` (would be `log.md`) |
| W3 | `notes/plan-a.md` | (R0) | unchanged, not an entry |
| W4 | `CON v2.md` | strip true | unchanged + `W(path_not_valid)` (`con.md` is a device name) |
| W5 | `___.md` | (R0) | unchanged + `W(name_left_empty)` |
| W6 | `Plan.md` in a folder whose path is 4,080 bytes (the item's path: 4,088 bytes) | prefix `abcdefghij-` | unchanged + `W(path_not_valid)` (the new path would be 4,099 bytes) |

### 4.10 Collisions: `collision`, `DuplicateObservation`

Compared by key against the **final** state: an item that is renamed frees its old key.

1. **Candidates**: run sections 4.1 to 4.9 for every selected item. Stayers: unselected,
   archived, unmatched, unchanged, flagged, refused. Movers: the rest.
2. **Taken** = keys of every stayer's path and of every folder at the base.
3. **Assign**, movers in this order: first every mover whose candidate key equals its own
   current key (a case-only rename: it holds that key, no other item can, so it is always
   assigned and no later claimant can take it), then the others by `from` (Rust `String` order
   of the current path). For mover m with candidate c and rule policy p:
   1. free = `key(c)` is not taken and not already assigned, and no ancestor folder of c has the
      key of a taken or assigned item path, and (for `reject`) no other mover's candidate has
      `key(c)`;
   2. free: assign c;
   3. not free:
      - `suffix`: for n = 2, 3, ...: `numbered(c, n)` (`-n` inserted before `.md`); assign the
        first whose key is free and which is a valid `WorkspacePath`
        (`W(collision_suffixed)`); if a numbered path is not valid, m becomes a stayer
        (`W(path_not_valid)`);
      - `keep_existing`: m becomes a stayer (`W(collision_kept)`);
      - `reject`: m becomes a stayer (`W(collision_rejected)`); every other mover whose
        candidate has the same key does too.
4. If step 3 made any new stayer, add its key to Taken and repeat step 3 from the start for the
   remaining movers. Stayers only grow, so this ends after at most one round per mover;
   typically one or two rounds.
5. A mover whose assigned path equals its current path is unchanged (this happens when the
   number it is given is the one it already has).
6. **Observations**: one `DuplicateObservation` per contested key (a key claimed by more than one
   item, or claimed by a mover and held by a stayer), resolved or not:
   `path` = the holder's path if a stayer holds it, else the first claimant's candidate in the
   order of step 3; `item_ids` = every item involved, sorted; `message` says how it was resolved.
   "Every pair that collides" is covered: a key's observation lists all its items.

So: `suffix` numbers later claimants deterministically; `keep_existing` lets the item that holds
the name now (or the first claimant) keep it and leaves the rest as they are; `reject` gives a
contested name to nobody, so a person decides. The difference between the last two is the
first claimant.

Vectors (`collision`; A, B, C are items; strip true unless stated):

| # | items | rule change | result |
|---|---|---|---|
| CO1 | A `a/Report v2.md`, B `a/Report (1).md` | `suffix` | B `a/report.md` (B's `from` sorts first: `(` < `v`); A `a/report-2.md` + `W(collision_suffixed)`; observation `a/report.md` {A, B} |
| CO2 | C `a/report.md` (stayer), A `a/Report v2.md` | `suffix` | A `a/report-2.md`; observation `a/report.md` {A, C} |
| CO3 | A, B as CO1 | `keep_existing` | B `a/report.md`; A unchanged + plan `W(collision_kept)`; observation {A, B}; entries 1, unchanged 1 |
| CO4 | A, B as CO1 | `reject` | both unchanged + plan `W(collision_rejected)` each; observation {A, B}; entries 0 |
| CO5 | A `a/Report.md` | (R0, strip false) | `a/report.md`: its own key is not a collision |
| CO6 | A `a/Report.md`, B `a/REPORT v2.md` | `suffix` | A `a/report.md` (a case-only rename holds its key and goes first); B `a/report-2.md` |
| CO7 | A `a/x.md`, B `b/x.md` | rules `[ {R0, glob a/*.md, destination b}, {R0, glob b/*.md, destination c} ]` | A `b/x.md`, B `c/x.md` (a chain: B frees `b/x.md`) |
| CO8 | A `a/x.md`, B `b/x.md` | rules `[ {R0, glob a/*.md, destination b}, {R0, glob b/*.md, destination a} ]` | A `b/x.md`, B `a/x.md` (a swap) |
| CO9 | A `a/Report v2.md` (selected), C `a/report.md` (not selected) | `suffix`, selection Items [A] | A `a/report-2.md` (unselected items are stayers) |

### 4.11 Aliases: `alias_preservation`

What SPEC section 6 "Preserve ... aliases" already guarantees, whatever this field says: a
source's observed names (`SourceAppearance.names`, documented in the contract as "Observed
aliases associated with this occurrence") and its original name are never changed by a rename;
the appearance file is keyed by item identity, not path, so a move does not touch it.

What the field adds, proposed (section 11, Q12): when true, the item's path before the rename is
kept as a **path alias** in the item's frontmatter key `aliases`, a YAML list of workspace path
strings, oldest first, each once. OKF defines no alias key (okf-core 0.2.7
`KNOWN_FRONTMATTER_KEYS` has 17 keys and none is an alias), so `aliases` is a producer
extension, which OKF preserves. When false, nothing is written and an existing `aliases` value is
left as it is.

1. An absent `aliases` becomes `[old path]`; a string `s` becomes `[s, old path]`; a list of
   strings gets `old path` appended unless it already holds it; any other value refuses the apply
   (`invalid_input` naming the item: a value the person wrote is not overwritten).
2. Only the move from the item's path at the base records an alias; a move from a temporary path
   (section 7.4) never does.
3. What keeps references working is the move itself: `okf_core::move_concept` rewrites every
   incoming link and rebases the moved item's own relative links, and storage tells the check the
   referrers it rewrote (`CandidateCheck::check`, `rewritten`). The alias is for references
   outside the bundle (bookmarks, other tools).
4. Writing `aliases` does not change the review fingerprint: `ItemDocument.content_digest` leaves
   `aliases` out (contract ruling 31, the owner's decision), so a pure rename never turns a
   reviewed item to `changed`. A move that rebases relative links in the item's body still
   changes it, as any content change does.
5. Where it is written: by storage, during the move (request P2, section 10). Core cannot add it
   cheaply or safely itself: an `EditItem` needs the item's whole body and properties (one
   `show` each, and `show` scans every item in storage today), and it must run before every move
   of the commit, or it would undo a link another move rewrote in that item.

Vectors (`alias_preservation`; need P2):

| # | item | rule change | after apply |
|---|---|---|---|
| A1 | `notes/Old Name.md`, no `aliases` | true | `notes/old-name.md`, `aliases: ["notes/Old Name.md"]` |
| A2 | same, `aliases: ["a.md"]` | true | `aliases: ["a.md", "notes/Old Name.md"]` |
| A3 | same, `aliases: "a.md"` | true | `aliases: ["a.md", "notes/Old Name.md"]` |
| A4 | same, `aliases: 5` | true | apply refused `invalid_input`, naming the item; nothing written |
| A5 | same, `aliases: ["a.md"]` | false | `aliases: ["a.md"]` unchanged |
| A6 | `Plan.md` (case-only, through a temporary path) | true | `plan.md`, `aliases: ["Plan.md"]` (never the temporary path) |
| A7 | `notes/Plan_A.md`, and `notes/B.md` holding `[p](Plan_A.md)` | false | `notes/plan-a.md`; `B.md` holds `[p](plan-a.md)`; the check is told `notes/B.md` |

---

## 5. Determinism and the digest

1. The same rules and the same tree give a byte-identical `RenamePlan`:
   - `entries` sorted by `from` (Rust `String` order);
   - `duplicate_observations` sorted by `path`, each `item_ids` sorted;
   - plan `warnings` sorted by (`location`, `code`, `message`);
   - each entry's `warnings` in the order of the steps that raised them;
   - no map with an unspecified order is iterated; the selection's order (an `Items` list in
     another order) changes nothing.
2. Per-item warnings carry `location` = the item's current path, on an entry or in the plan.
3. `rules_digest = mutations::request_digest(&rules)`: SHA-256 of the rules re-serialized with
   every object's keys sorted. Absent options are left out by serde
   (`skip_serializing_if`), so `type_name: null` and no `type_name` give one digest; key order
   and whitespace of the request do not matter; any change of a value does.

Vectors (determinism):

| # | case | result |
|---|---|---|
| DT1 | the same preview twice | identical JSON |
| DT2 | `Items [A, B, C]` and `Items [C, A, B]` | identical plans |
| DT3 | rules with keys in another order; with `type_name: null` | same `rules_digest`; `separator` `-` vs `_` gives another |
| DT4 | a tree where three entries, two observations and two plan warnings arise | each list in the order of step 1 |

---

## 6. Validation of rules

One validator, `CompiledRules::compile(rules, pointer)`, used by `set_rules`, `preview_names`,
`apply_names` (pointer `/rules`), the import (the stored file) and `get_rules` (pointer `` and
every problem collected instead of the first). Checks in this order; the first failure refuses
(`get_rules` collects all):

| # | field (pointer under `/rules`) | refusal |
|---|---|---|
| 1 | `/schema_version` | `invalid_input`: not 1 ("this build reads naming rules of schema version 1") |
| 2 | `/rules` | `too_large`: more than 256 rules |
| 3 | `/rules/{i}/glob` | `invalid_input`: empty; leading `/`; trailing `/`; holds `\`; `too_large` over 1,024 bytes; `glob::Pattern::new` error (its `msg`, at its `pos`) |
| 4 | `/rules/{i}/type_name` | `invalid_input`: empty string; `too_large` over 1,024 bytes |
| 5 | `/rules/{i}/separator` | `invalid_input`: holds `/`; `a{sep}b` not a `WorkspacePath` (the contract's message); `snake` without `_`, `kebab` without `-`; `too_large` over 1,024 bytes |
| 6 | `/rules/{i}/prefix` | `invalid_input`: holds `/`; `{prefix}a.md` not a `WorkspacePath`; `too_large` over 1,024 bytes |
| 7 | `/rules/{i}/suffix` | same as prefix, probe `a{suffix}.md` |
| 8 | `/rules/{i}/date_format` | `invalid_input` (when not empty): not a format description (`InvalidFormatDescription`, with its index); cannot format a date; an output is empty, holds `/`, or makes `a{out}b` no `WorkspacePath`; `too_large` over 1,024 bytes |
| 9 | `/rules/{i}/destination` | `invalid_input` (when present): empty; not a format description; cannot format a date; an output is no `WorkspacePath` or has a reserved segment; `too_large` over 1,024 bytes |

`case`, `date_order`, `collision`, `extension_policy` and the booleans are enums and booleans the
deserializer already checked.

Vectors (`schema_version`):

| # | input | result |
|---|---|---|
| SV1 | `schema_version: 1` | accepted |
| SV2 | `set_rules` with `schema_version: 2` | `invalid_input /rules/schema_version` |
| SV3 | `set_rules` with `schema_version: 0` | `invalid_input /rules/schema_version` |
| SV4 | stored file with `schema_version: 2` | `get_rules` answers `unreadable` with a problem at `/schema_version` |
| SV5 | `set_rules` with 257 rules | `too_large /rules/rules` |

---

## 7. The operations

### 7.1 `get_rules`

1. Resolve `at` once (`shared::resolve`: the purge map first).
2. Read the stored file with the changed port (request P0): `VersionStore::rules(scope,
   revision) -> Option<RulesFile>`, `RulesFile { text: String, parsed: Result<NamingRules,
   RulesParseError> }`, `RulesParseError { message, line: Option<u32>, column: Option<u32> }`.
3. No file: `GetRulesResponse { revision, rules: None, unreadable: None }`.
4. `text` over `MAX_ITEM_TEXT_BYTES` (8 MiB): `too_large` (ruling 11).
5. `parsed` is an error: `unreadable: { text, problems: [Warning { code: rules_yaml_invalid,
   message, location: "line L, column C" }] }` (yaml_serde's `Error::location()`, 1-based; no
   location when the reader gives none). The reader stops at its first error, so one problem.
6. `parsed` is rules: run the validator (section 6) collecting every problem, with locations as
   JSON Pointers into the file (`/schema_version`, `/rules/2/glob`). Any problem:
   `unreadable: { text, problems }`, `rules` absent. None: `rules: Some(rules)`.
7. Never `internal` for a file a person or an import wrote. A text that is not UTF-8 reaches
   core with U+FFFD in place of bad bytes (`String::from_utf8_lossy`) and a parse problem.
8. A read receipt with no sources is recorded, as `log_items` does: `get_rules` is now the agent
   tool `rules`, whose results are tool-result records (SPEC section 2).

Why a rule-check failure is `unreadable` and not `rules`: "rules this build cannot apply" are not
"this build's rules" (ruling 11), and the YAML editor is where the text and its problems can be
shown together for repair; `set_rules` replaces the file whatever it held.

Vectors (`get_rules`):

| # | stored `.okf/rules.yaml` at `at` | answer |
|---|---|---|
| GR1 | absent | `{ revision }` (no `rules`, no `unreadable`) |
| GR2 | the file of section 2 | `{ revision, rules }` equal to what was saved |
| GR3 | `schema_version: 1` / `colour: red` / `rules: []` | `unreadable { text, problems: [rules_yaml_invalid at line 2] }` (unknown key) |
| GR4 | `schema_version: 2` and valid rules | `unreadable`, problem `rules_schema_version_unknown` at `/schema_version` |
| GR5 | one rule with glob `notes/**x` | `unreadable`, problem at `/rules/0/glob` |
| GR6 | 9 MiB of text | `too_large` |
| GR7 | `at` names a revision a purge invalidated | the typed `not_found` of `resolve` |
| GR8 | bytes `0xff 0xfe ...` | `unreadable`, `text` with U+FFFD, one parse problem |

### 7.2 `set_rules`

1. Validate (section 6, pointer `/rules`): `invalid_input`/`too_large` on the field; nothing
   written.
2. `shared::commit_with_receipt(Write { base: request.base_revision, message: "Save naming
   rules", edits: [TreeEdit::SetRules { rules }], item: None, touched: Touched::default() })`.
   Storage serializes with `yaml_serde::to_string` into `.okf/rules.yaml`; comments are not kept
   (the UI says so).
3. A base that is not the head is the commit path's `conflict` (with `StaleBase` once batch 2
   lands); a retry under the same key answers the first result.

Vectors: SR1 valid rules: one commit; `get_rules` at the new revision answers them. SR2 stale
base: `conflict`, head unchanged. SR3 same idempotency key twice: one commit, same revision and
receipt. SR4 invalid glob: `invalid_input /rules/rules/0/glob`, head unchanged. SR5 saving over an
unreadable file (GR3): accepted; `get_rules` then answers `rules`.

### 7.3 `preview_names`

1. Validate the request's rules (section 6, `/rules`).
2. Resolve `at` once; this is `base_revision`.
3. Read the tree at the base: every item (with a source card's original name) and every folder,
   with request P1 (`VersionStore::tree`); without P1, the fallback of section 10.
4. Resolve the selection:
   - `Items { item_ids }`: every id must be an item at the base, else `not_found
     /selection/item_ids/{i}`; a repeated id is `invalid_input /selection/item_ids/{i}` (if the
     contract's `check_rules` has not refused it already);
   - `Folder { folder, recursive }`: the folder must exist at the base, else `not_found
     /selection/folder`; its items, directly inside (`recursive: false`) or at any depth;
   - `Workspace`: every item.
5. Run sections 4.1 to 4.10 over the selected items, against the whole tree.
6. Answer `RenamePlan { base_revision, rules_digest, entries, duplicate_observations, warnings,
   considered, unchanged, refused, archived_left_out }` with `considered` = the selected items that
   are not archived, `archived_left_out` = the archived ones (never renamed), `refused` = the
   considered items whose rules give a refused name (each named in `warnings`), and
   `considered = entries.len() + unchanged + refused`.
7. Nothing is written: no commit, no event, no receipt (it answers paths, as `list_items` does).

Why the whole tree even for a small selection: a destination can move an item into any folder,
and a collision is with any item there.

Cost at the SPEC envelope (10,000 items, about 100 folders): with P1, one pass over the tree
(storage parses each item once) plus, per item, one glob match per rule tried (at most 256) and
one regex scan; collisions in O(M log M) per round. Without P1: about 150 `list` calls, each of
which parses every item in storage today (`read::items`), so about 1.5 million item parses, plus
one `show` (another full parse) per selected source card for its original name; a whole-workspace
preview at the envelope would take minutes. That is why P1 is requested.

Vectors (`preview_names`):

| # | case | result |
|---|---|---|
| PN1 | `Items []` | `invalid_input /selection/item_ids` (contract) |
| PN2 | `Items [unknown id]` | `not_found /selection/item_ids/0` |
| PN3 | `Items [A, A]` | `invalid_input /selection/item_ids/1` |
| PN4 | `Folder notes` with `notes/a.md`, `notes/sub/b.md`: `recursive` false; true | `considered` 1; 2 |
| PN5 | `Folder nowhere` | `not_found /selection/folder` |
| PN6 | `Workspace` with 3 items, one archived and matching | `considered 3, archived_left_out 1` |
| PN7 | rules with glob `notes/**x` | `invalid_input /rules/rules/0/glob`; no port read |
| PN8 | any preview | head, events and receipts unchanged |
| PN9 | `at` a purged revision | `not_found` of `resolve` |

### 7.4 `apply_names`

1. Validate the request's rules (section 6, `/rules`).
2. `rules_digest(request.rules) == plan.rules_digest`, else `invalid_input /rules` ("these are not
   the rules the plan was previewed with").
3. `plan.entries` is not empty, else `invalid_input /plan/entries`; no item id twice, else
   `invalid_input /plan/entries/{i}/item_id`; at most 1,000 entries, else `too_large
   /plan/entries` (section 11, Q15).
4. `check_named(plan.base_revision)` (purge map).
5. On the first attempt only (`OperationContext.attempt == First`): the head must be
   `plan.base_revision`, else `conflict` with `ErrorDetail::StaleBase { head }` on
   `/plan/base_revision`, nothing written (SPEC section 6: "A stale preview must not silently
   apply"). A resumed attempt skips this: its commit may have landed already, and the store's
   replay answers it (step 9).
6. **Recompute**: run `preview_names`' steps 3 and 5 at `plan.base_revision` with
   `selection = Items(the entries' item ids)` and the request's rules. The recomputed entries must
   equal the plan's entries exactly, in order (`item_id`, `from`, `to`); the first difference
   refuses the whole apply with `invalid_input /plan/entries/{i}/{field}`. Warnings,
   observations and the three counts of the request's plan are not compared (ruling 10: "apply
   does not check them"). The recompute with only the entries' items gives the preview's own
   answer: every item the preview left unchanged or did not select is a stayer either way, and
   suffix numbers depend only on the claimants, which are all entries.
7. **Edits**, in three phases, each in entry order:
   1. *direct*: every entry whose `to` key is held at the base by no item: `MoveItem { item_id,
      destination: to, keep_alias }`;
   2. *to temporary*: every other entry (its `to` key is held at the base by itself, a case-only
      rename, or by another entry, a chain or a swap): `MoveItem` to
      `<folder of from>/okf-jawn-renaming-<item id>.md` (at the root if that is not a
      `WorkspacePath`; numbered with `numbered` in the unlikely case its key is held by an item
      or folder at the base or by another entry's `to`), with `keep_alias`;
   3. *from temporary*: those entries again, `MoveItem` to `to`, `keep_alias: false`.
   After phases 1 and 2 every entry has left its `from`, and section 4.10 guaranteed no `to`
   collides with a stayer or another `to`, so every phase-3 destination is free. A temporary
   path is never in the committed tree. `keep_alias` is the naming rule's `alias_preservation`
   (request P2).
8. One commit: `shared::commit_with_receipt(Write { base: plan.base_revision, message: "Apply
   naming rules to N items", edits, item: None, touched: Touched { paths: every from and every to,
   items: [], of_type: None } })`. `Touched.items` stays empty on purpose: `store_commit` reads
   each touched item's path with `show`, which scans every item in storage today, and the plan
   already holds the paths. Storage tells the check the referrers each move rewrote.
9. Idempotence: a retry under the same key answers the stored result (dispatch); a resumed
   attempt replays the landed commit (`VersionStore::commit` finds the trailer after
   `expected_head`) and answers its kept warnings.
10. Too large: a plan whose request body exceeds `transport::MAX_REQUEST_BODY_BYTES` (16 MiB) is
    refused `too_large` by the transport before the handler (ruling 10: apply folder by folder).
    At about 300 bytes an entry that is some 50,000 entries, more than the envelope; the 1,000-entry
    bound of step 3 is the binding one (cost, section 10, K5).

Vectors (`apply_names`):

| # | case | result |
|---|---|---|
| AN1 | first attempt, head moved since the preview | `conflict` + `StaleBase { head }` on `/plan/base_revision`; nothing written |
| AN2 | rules changed after the preview (separator `_`) | `invalid_input /rules` |
| AN3 | an entry's `to` edited by the client | `invalid_input /plan/entries/0/to` |
| AN4 | `entries: []` | `invalid_input /plan/entries` |
| AN5 | an item id twice | `invalid_input /plan/entries/1/item_id` |
| AN6 | A7's tree | one commit; the referrer's link rewritten; the check told `notes/B.md`; `MutationResult` with receipt; one `changed` event with no item |
| AN7 | CO5 (`a/Report.md` to `a/report.md`) | one commit; tree holds `a/report.md`, no `okf-jawn-renaming-` file |
| AN8 | CO7 and CO8 | one commit each; final paths as previewed |
| AN9 | resumed attempt after the commit landed | same revision, one commit in history |
| AN10 | 1,001 entries | `too_large /plan/entries` |

### 7.5 The agent tool `rules`

`get_rules` with alias `rules` and visibility `model` (ruling 28) answers through dispatch like
every model tool: no code of its own. Phase 2 adds one MCP test that the tool answers what
`get_rules` answers (GR1, GR3). The MCP server's text budget for a result applies to an 8 MiB
unreadable text; that is the MCP lane's bound, noted as concern K7.

---

## 8. The shared function (R-I4) and the import

In `crates/core/src/conventions.rs` (pure, no port):

```rust
/// Rules checked and compiled once: globs, date formats, destinations.
pub struct CompiledRules { /* private */ }
impl CompiledRules {
    /// Check `rules` (section 6); `pointer` is where they sit in the request (`/rules`) or ``.
    pub fn compile(rules: &NamingRules, pointer: &str) -> Result<Self, ApiError>;
    /// Every problem, for `get_rules`.
    pub fn problems(rules: &NamingRules) -> Vec<Warning>;
}

/// One item as the rules see it.
pub struct Subject<'a> {
    pub path: &'a WorkspacePath,       // its path now, or a new item's proposed path
    pub type_name: &'a str,            // its OKF type
    pub original_name: Option<&'a str>,// a source card's original file name
}

/// What the rules make of one path, before collisions (sections 4.1 to 4.9).
pub enum Proposal {
    Unmatched,
    Unchanged { warnings: Vec<Warning> },
    Flagged { warnings: Vec<Warning> },
    Renamed { path: WorkspacePath, policy: CollisionPolicy, keep_alias: bool, warnings: Vec<Warning> },
}
pub fn propose(rules: &CompiledRules, subject: &Subject<'_>) -> Proposal;

/// `path` with `-n` before its `.md` (n >= 2).
pub fn numbered(path: &WorkspacePath, n: u32) -> Result<WorkspacePath, IdentityError>;

/// The rename plan of `selected` in `tree` (sections 4.10 and 5).
pub fn plan(rules: &CompiledRules, tree: &[TreeItem], folders: &[WorkspacePath],
            selected: &BTreeSet<ItemId>) -> PlanBody;

/// Name one new item (an import's card).
pub fn name_new_item(rules: &CompiledRules, subject: &Subject<'_>,
                     taken: &mut BTreeSet<String>) -> Result<NewName, ApiError>;
pub struct NewName { pub path: WorkspacePath, pub warnings: Vec<Warning> }
```

`name_new_item(rules, subject, taken)`: `subject.path` is the card's path without rules
(`destination/source_card_name(filename)`), `type_name` is `source`, `original_name` the upload's
file name; `taken` holds the keys of every item and folder at the import's base and of every
card the job named before (the function adds the key it chooses).

1. `propose`. `Unmatched`, `Unchanged`, `Flagged`: the candidate is `subject.path` (with the
   proposal's warnings). `Renamed`: the candidate is its path.
2. Candidate key free: take it.
3. Taken, `Renamed` with `suffix`: the first free `numbered(candidate, n)`, n >= 2.
4. Taken otherwise (`keep_existing`, `reject`, or no rule): fall back to `subject.path`, then
   `numbered(subject.path, n)` for n >= 2: exactly today's `free_path` rule; a ruled candidate that
   lost adds `W(collision_kept)` / `W(collision_rejected)` (section 11, Q14).
5. A path that is no `WorkspacePath` is `invalid_input /upload_ids/{i}` as today.

Ingest's import with `apply_naming_rules: true`:
1. Read the rules at the job's base (P0 shape). None saved: name as without rules and add one
   completion warning `naming_rules_absent`. Unreadable or refused by the validator: the job
   fails, not retried, `invalid_input /apply_naming_rules` with the first problem.
2. `taken` = keys of every item and folder at the base (the whole tree, because a destination can
   point anywhere), then `name_new_item` per card in `upload_ids` order. Names are chosen at the
   base (ruling 26, R-I8); a path taken since fails the job as today.
3. The naming warnings join the job's completion warnings, located at the card's path.
4. `apply_naming_rules: false` is unchanged (`free_path`, which may call `numbered`).

Why the two cannot differ: `preview_names` and the import both name a path with `propose` and
number with `numbered`; a card the import named is `unchanged` in a later preview with the same
rules (IM5), because the card tail recognizes the import's number.

Vectors (the shared function and the import; rules R0 unless stated; destination `inbox`):

| # | case | result |
|---|---|---|
| IM1 | no rules saved, upload `Q3 Report.PDF` | `inbox/Q3 Report-PDF.md` + completion `W(naming_rules_absent)` |
| IM2 | upload `Q3 Report.PDF` | `inbox/q3-report-PDF.md` |
| IM3 | uploads `a.pdf` and `A.pdf` in one import | `inbox/a-pdf.md`, `inbox/a-pdf-2.md` (the job's own cards are taken) |
| IM4 | `inbox/q3-report-pdf.md` exists; upload `Q3 Report.pdf`; `keep_existing` | `inbox/Q3 Report-pdf.md` + `W(collision_kept)` |
| IM5 | after IM3, `preview_names` of `inbox` with the same rules | both cards `unchanged` |
| IM6 | stored rules unreadable at the base | job fails, not retried, `invalid_input /apply_naming_rules` |
| IM7 | `inbox/q3-report-pdf.md` exists; upload `Q3 Report.pdf`; `suffix` | `inbox/q3-report-pdf-2.md` |

---

## 9. Libraries (read from the installed sources)

Registry: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. Already direct dependencies
of core: `time` 0.3.55 (no features of its own; `parsing` and `formatting` reach it only through
the contract's features), `okf-core`, `sha2`, `serde_json`. In the workspace's
`[workspace.dependencies]` and the lock but not core's: `convert_case` 0.12.0 and `regex` 1.13.1
(both declared by ingest, unused there), `unicode-normalization` 0.1.25 (contract, ingest). In
the lock only as other crates' dependencies: `glob` 0.3.4 (`clang-sys`), `heck` 0.5.0
(`clap_derive`, `strum_macros`), `jiff` 0.2.37 (`lopdf`), `chrono` 0.4.45 (`calamine`, `rmcp`,
...). Not in the lock: `globset`.

### 9.1 Glob matching

**glob 0.3.4** (`src/lib.rs`):
- `Pattern::new(pattern: &str) -> Result<Self, PatternError>`; `PatternError { pos, msg }` with
  the messages `"wildcards are either regular `*` or recursive `**`"`, `"recursive wildcards
  must form a single path component"`, `"invalid range pattern"`.
- Doc of `Pattern`: "`**` matches the current directory and arbitrary subdirectories ... This
  sequence **must** form a single path component, so both `**a` and `b**` are invalid"; "`[!...]`
  is the negation of `[...]`"; "The metacharacters `?`, `*`, `[`, `]` can be matched by using
  brackets (e.g. `[?]`)". No alternation: `{` is an ordinary character.
- `pub fn matches_with(&self, str: &str, options: MatchOptions) -> bool`, walking `char`s, so
  `?` is one Unicode scalar value (`?.md` matches `é.md`).
- `MatchOptions { case_sensitive, require_literal_separator, require_literal_leading_dot }`;
  with `require_literal_separator` "`*` or `?` or `[...]`" never match `/`.
- Case folding is ASCII only: `fn chars_eq` "`} else if !case_sensitive && a.is_ascii() &&
  b.is_ascii() { // FIXME: work with non-ascii chars properly (issue #9084)`", and the
  `case_sensitive` doc: "This currently only considers upper/lower case relationships between
  ASCII characters". Hence this design lower-cases both sides itself and matches
  case-sensitively (G6).
- Separators are `std::path::is_separator`, which is `/` or `\` on Windows: hence `\` is refused
  in a glob (G14).

**globset 0.4.18** (not in the lock; would add `globset` and `bstr`):
- `GlobBuilder::case_insensitive`, `literal_separator`, `backslash_escape`; `{a,b}` alternation;
  "`foo/**` matches `foo/a` and `foo/a/b`, but not `foo`".
- `Tokens::to_regex_with` starts every pattern with `re.push_str("(?-u)")` (bytes, not Unicode)
  then `(?i)`: case folding is ASCII only, `Token::Any` is `.`, one **byte**, so `?.md` does not
  match `é.md`, and a class holding a non-ASCII character becomes a class of its bytes
  (`char_to_escaped_literal` writes `\xc3\xa9`).

**Proposal: glob 0.3.4.** It is character-based, its errors carry positions for the form, and it
adds nothing to the lock. Its ASCII-only folding is answered by matching collision keys.

### 9.2 Words and letter case

**heck 0.5.0** (`src/lib.rs`):
- `transform` splits with `s.split(|c: char| !c.is_alphanumeric())`: every non-alphanumeric is a
  break and is dropped ("Characters not within words (such as spaces, punctuations, and
  underscores) are not included in the output"): `Tom's notes` gives `tom-s-notes`, `v1.2`
  gives `v1-2`, `C++` gives `c`.
- camelCase is always split: "`XMLHttpRequest` is segmented `XML|Http|Request`"; `iPhone` gives
  `i|Phone`; title test10: `"XMLHttpRequest" => "Xml Http Request"`.
- Fixed joiners (`write!(f, "-")` in kebab, `" "` in title); no way to choose the separator or
  turn a boundary off. `capitalize` upper-cases the first character and lower-cases the rest
  (`NASA` gives `Nasa`).

**convert_case 0.12.0** (`src/boundary.rs`, `pattern.rs`, `converter.rs`):
- `pub use boundary::{split, Boundary}`; `pub fn split<'s, T>(s: &'s T, boundaries: &[Boundary])
  -> Vec<&'s str>`. "`Hyphen`, `Underscore`, and `Space` consume the character they split on";
  with only those three it takes a byte fast path. "All symbols that are not part of the default
  boundary conditions are ignored" (kept in the word: `"dots.arent.default"`).
- The defaults also split digits and camelCase (`ccase!(title, "word2vec") == "Word 2 Vec"`,
  `LowerUpper`, `Acronym`); this design passes only the three separators. "we only split on
  ASCII digits" (`GraphemeFlags::classify`).
- `Pattern::Lowercase` is `word.to_lowercase()`, `Uppercase` `word.to_uppercase()`, `Capital`
  first character upper and the rest `to_lowercase()`, `RemoveEmpty` drops empty words,
  `Custom(fn(&[&str]) -> Vec<String>)`; `pub fn mutate<S: AsRef<str>>(&self, words: &[S]) ->
  Vec<String>`.
- "Leading, trailing, and duplicate delimiters create empty words" (hence `RemoveEmpty`).
- `Converter::convert`: split, apply the patterns in order, `join(&self.delimiter)`.

**Proposal: convert_case 0.12.0**, through `split` and `Pattern::mutate` (not `Converter`,
because dates are protected words that must not be split or re-cased), with `Pattern::Custom`
for title (section 4.5). It is already the workspace's pinned version and depends only on
`unicode-segmentation` 1.13.3 (in the lock).

### 9.3 Dates and recognition

**time 0.3.55** (core's own):
- `format_description::parse_owned::<3>(s)` "**It is recommended to use version 3**"; it returns
  `FormatDescriptionV3<'static>`, "deliberately opaque". The version-1 `parse` is
  `#[deprecated(since = "0.3.48", ...)]` (and workspace warnings are errors), so it is not used.
- Version 3 lexing: "all escape sequences begin with `\`"; an unknown component is
  `InvalidFormatDescription::InvalidComponentName { name, index }`.
- `impl Formattable for FormatDescriptionV3<'_>`; `Date::format(self, format: &(impl Formattable
  + ?Sized)) -> Result<String, error::Format>`; a component a date lacks gives
  `Format::InsufficientTypeInformation` ("The type being formatted does not contain sufficient
  information to format a component"), which is how `[hour]` is refused (F6, DS9).
- `Date::from_calendar_date(year: i32, month: Month, day: u8) -> Result<Self,
  error::ComponentRange>` ("`assert!(Date::from_calendar_date(2019, Month::February,
  29).is_err())`") and `impl TryFrom<u8> for Month` decide what is a date (D4, D9).
- `pub mod format_description` exists only with the `formatting` or `parsing` feature; the parse
  functions need `alloc`. Core needs `formatting` stated in its own manifest.

**jiff 0.2.37**: `civil::Date::strftime`: "If the formatting string is malformed in some way, then
it is silently ignored" (`"%Y %"` gives `"2024 %"`); errors only through
`jiff::fmt::strtime::format`. A second date library for core. **chrono 0.4.45**:
`StrftimeItems::new(..).parse() -> Result<Vec<Item>, ParseError>`, `NaiveDate::from_ymd_opt`;
also a second date library. Neither is needed.

**regex 1.13.1**: "This includes, but is not limited to, look-around and backreferences" (not
supported), hence the boundary group and `captures_at`, whose doc says "the `\A` anchor can only
match when `start == 0`" (the context is respected) and which panics when `start` is past the
end (never called so). Patterns are compiled once (`Regex::new` at first use, an error is
`Internal`).

**Proposal: time 0.3.55** (version-3 descriptions, `from_calendar_date`) **and regex 1.13.1**.

### 9.4 Others

- **unicode-normalization 0.1.25**: `UnicodeNormalization::nfc` for the glob text, exactly as
  `collision_key` does it.
- **yaml_serde 0.10.7** (storage, for P0): `Error::location() -> Option<Location>`; `Location::
  line()`, `column()` are 1-based ("`line` and `column` returned from libyaml are 0-indexed but
  all error messages add +1").
- **okf-core 0.2.7**: `move_concept` rewrites incoming links and rebases outgoing ones; no alias
  key in `KNOWN_FRONTMATTER_KEYS`.

### 9.5 Manifest request (integration owner; nothing changed by this builder)

`crates/core/Cargo.toml`:
```toml
glob = { workspace = true }                      # new [workspace.dependencies] entry: glob = { version = "=0.3.4" }
convert_case = { workspace = true }              # =0.12.0
regex = { workspace = true }                     # =1.13.1
unicode-normalization = { workspace = true }     # =0.1.25
time = { workspace = true, features = ["formatting"] }
```
No new lock entry (all four already resolve at these versions). Vendor notes for `glob`,
`convert_case` and `regex` (none exist in `vendors.json`).

---

## 10. Requests and concerns

Requests (each blocks the part named; phase 2 builds the rest):

- **P0 (core port, storage implements; blocks `get_rules` beyond today).** `VersionStore::rules`
  answers `Option<RulesFile>` (section 7.1): the text and the parse result with the reader's
  1-based line and column, instead of folding a parse failure into `internal`
  (`storage/src/git/read.rs:232-242`). Today's callers: none outside storage and the fakes.
- **P1 (core port, storage implements; performance at the envelope).** `VersionStore::tree(scope,
  revision) -> WorkspaceTree { items: Vec<TreeItem { summary: ItemSummary, original_name:
  Option<String> }>, folders: Vec<WorkspacePath> }`: every item and folder in one pass, path
  order, including items under folders whose name starts with `.` (which `list` does not show,
  `read.rs:127-131`). No fallback: P1 is built in phase 2 (rulings), because a walk that misses items
  under dot-folders would give an incomplete selection and collision set.
- **P2 (core port, storage implements; only if Q12 is accepted).** `TreeEdit::MoveItem` gains
  `keep_alias: bool`: before the move, append the item's current path to its frontmatter
  `aliases` as section 4.11 says, in the staged tree. Until then, an apply whose entries need an
  alias answers `not_implemented` naming P2.

Concerns (found while designing; not this package's to fix):

- **K1. A case-only `move_item` is refused today.** Storage's `move_item` checks `occupied`, which
  counts the moved item's own path (`edit.rs:157-160`: `self.items.values().any(|existing|
  collision_key(existing) == key)`), so renaming `Report.md` to `report.md` answers "is already
  taken". `apply_names` works around it with a temporary path; a person's own rename in the UI
  does not.
- **K2. Dropping that check would lose data on Windows and macOS.** `okf_core::move_concept` writes
  the target, then `if source_path.exists() && source_path != target_path {
  fs::remove_file(&source_path)?; }`; on a case-insensitive filesystem `Report.md` and
  `report.md` are one file, so the document just written is deleted. A storage fix for K1 must
  move through a temporary name (and okf-core should be told).
- **K3. `.MD` items cannot be moved.** Storage's `is_item_path` accepts any case of `md`; okf-core's
  `collect_markdown` takes only `e == "md"`, `ConceptId::to_path` appends `.md`, and storage's
  `concept()` trims `.md` case-sensitively. Such an item is invisible to okf-core (moves, links,
  validation).
- **K4. Segment length.** NTFS refuses a name over 255 UTF-16 units; at design time `WorkspacePath`
  bounded only the whole path. Contract ruling 30 now refuses such a segment, so an entry whose
  result breaks it gets a warning and no rename (rulings, K4).
- **K5. Apply cost.** Each `MoveItem` re-loads the whole staged bundle (`relink` calls
  `Bundle::load`), so an apply of M entries reads O(M x N) files: at 1,000 entries and 10,000
  items, ten million file reads. A batch move in storage (one load for consecutive moves) would
  remove the bound of section 7.4 step 3.
- **K6. `show` and `list` scan every item.** `read::find` and `read::list` each parse every item of
  the revision (`read.rs:99-107`, `read.rs:133`); anything per-item through these ports is
  quadratic.
- **K7. The `rules` tool's text.** An unreadable file of up to 8 MiB is returned as `text`; the
  MCP server's result budget decides what an agent sees.
- **K8. Brief wording.** The brief's phase 2 says the import names cards "with the collision keys
  of the head"; ruling 26 and R-I8 say names are chosen at the base. This design follows the
  ruling (Q16).

---

## 11. Open questions (each with a proposal)

| # | question | proposal |
|---|---|---|
| Q1 | First matching rule names an item, or every matching rule applies in turn? | First match (4.1). |
| Q2 | Glob case- and normalization-sensitive? | Insensitive: glob and path compared as collision keys, so colliding paths are matched alike. |
| Q3 | Glob shape: `*` never crosses `/` (`*.md` is root only); leading `/`, trailing `/`, `\` refused; `{a,b}` literal. | As stated. |
| Q4 | Word breaks: only space, `-`, `_` (no camelCase, no letter/digit, no `.`)? | Yes. The alternative (convert_case `LowerUpper` + `Acronym`) turns `iPhone` into `i-phone` and `PowerPoint` into `power-point`. |
| Q5 | `snake`/`kebab` with another separator: refuse, or let one field win? | Refuse at validation (`snake` needs `_`, `kebab` needs `-`). |
| Q6 | What does `title` do to `iPhone`, `NASA`, `über`? | Words that already have a capital are kept (`iPhone`, `NASA`, `QUARTERLY`); others get a capital first letter (`Über`). The alternative, convert_case `Capital`, gives `Iphone`, `Nasa`. |
| Q7 | The version-suffix list. | Trailing `v<n>[.<n>...]`, `final`, `copy`, `copy <n>` after a space/`-`/`_` run, and ` (n)`; any case; repeated; never leaving nothing. Not `draft`, `old`, `new`, `latest`, `rev2`, a bare `-2`. |
| Q8 | Which date spellings are read, and how a declared order applies. | `YYYY-M-D` and `M-D-YYYY`/`D-M-YYYY` with one of `- _ .`, and `YYYYMMDD`; no two-digit years, month names or spaces. Year-first and compact are always Y-M-D; `ymd` does not read year-last dates. |
| Q9 | An ambiguous date: leave the whole item unchanged, or rename the rest and keep the date text? | The whole item unchanged, flagged in the plan's warnings. |
| Q10 | `date_format` syntax and meaning of empty; are dates protected from the word rules? | `time` format descriptions v3 (`[year]-[month]-[day]`), not strftime; empty keeps dates as written; a date is always one protected word. |
| Q11 | Destination: placeholders, empty, the root. | Date parts only (time v3 syntax); no `{type}` (a rule per type) and no original-folder placeholder; empty refused, so the root is not a destination; a missing date keeps the folder with a warning. |
| Q12 | `alias_preservation`: where is the old path kept? | Frontmatter `aliases` (list of former paths), written by storage during the move (P2); it changes the content digest, so reviews of renamed items show `changed`. Alternatives: the application header (no digest change, not portable OKF) or no record (links and history only). |
| Q13 | `extension_policy` and source cards: what is the extension, and is a card's input its current name? | Only the card's original-extension tail (`-pdf`, `-pdf-2`) is affected; `.md` is always lower case; a card's input is its current name (a person's rename is respected), read in `source_card_name`'s shape. |
| Q14 | Collision meanings; and at import, what `keep_existing`/`reject` do. | `suffix`: `-2, -3` before `.md`, case-only renames first then `from` order; `keep_existing`: the holder or first claimant keeps the name; `reject`: nobody gets a contested name. At import both fall back to the un-ruled numbered name with a warning; an import never fails on a naming collision. |
| Q15 | Limits. | At most 256 rules, 1,024 bytes per text field, 1,000 entries per apply (until a batch move, K5). |
| Q16 | Import collision keys at the head (brief) or the base (ruling 26, R-I8)? | The base, as the ruling says. |
| Q17 | `get_rules`: a file that parses but fails a rule check, and the line format. | `unreadable`, every problem listed (JSON Pointer locations); a YAML error located `line L, column C`. A read receipt with no sources is recorded; `preview_names` records none. |

---

## 12. Phase 2 test plan (for the reviewer)

- `crates/core/src/conventions.rs`: the engine (pure). Tests in `crates/core/tests/conventions.rs`
  named `conventions_<group>_<vector>` (for example `conventions_glob_g6_matches_non_ascii_case`),
  so `cargo test -p okf-jawn-core -- conventions` selects every vector of sections 4 to 6 and 8.
- `crates/core/src/application/conventions.rs`: the four handlers and their `route!` arms; tests
  `conventions_get_rules_*`, `conventions_set_rules_*`, `conventions_preview_*`,
  `conventions_apply_*` against the fakes (GR, SR, PN, AN), each fake method the rules rely on
  answering the awkward real answer (unsorted listings, several pages, a parse failure).
- Ingest: an import with rules (IM2, IM3, IM6) and the preview agreement (IM5).
- MCP: one test that the `rules` tool answers what `get_rules` answers.
- Mutants, at least one per rule, each shown red: first match (try last match), each glob refusal,
  case-insensitive glob (match the raw path), `require_literal_separator`, type equality
  (case-insensitive), prefix/suffix strip (remove it), version regex alternatives one by one and
  the repeat, date shapes one by one, separator equality, each order's reading, ambiguity (guess
  `mdy`), protected words (split them), each case pattern, title keep-rule, snake/kebab separator
  check, extension tail (`-n` recognition, each policy), destination needs-date probe, reserved
  check, archived exclusion, collision order (case-only first; `from` order), each policy, the
  stayer repeat, observation listing, sorting of each list, digest key order, apply's digest
  check, head check on first attempt only, recompute comparison, phase assignment, temporary path
  never committed, `Touched.items` empty.
