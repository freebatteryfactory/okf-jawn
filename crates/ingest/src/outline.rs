//! The outline of converted Markdown, with each heading selecting its section as lines.
//!
//! A heading entry selects its section: from the heading line to the line before the next
//! heading of the same or a higher level, or to the end of the text (`OutlineEntry::selection`;
//! `reading::section_lines` trusts it). Lines are one-based and count the lines of the
//! Markdown as `str::lines` splits them, so a final newline does not add a line.
//!
//! A heading is an ATX heading, which is the only form docling's Markdown export writes: one to
//! six `#` at the start of a line, then a space or the end of the line. A `#` line inside a
//! fenced code block is code, not a heading. The outline of a windowed conversion is computed
//! again over the joined Markdown, so a section that runs over a window boundary ends where the
//! whole document says it ends.

use okf_jawn_contract::common::TextRange;
use okf_jawn_contract::read::{OutlineEntry, OutlineEntryKind, Selection};

/// The deepest heading level Markdown has.
const MAX_LEVEL: usize = 6;

/// The heading entries of `markdown`, in order, each selecting its section as lines.
#[must_use]
pub fn outline_of(markdown: &str) -> Vec<OutlineEntry> {
    let headings = headings(markdown);
    let last_line = line_count(markdown);
    headings
        .iter()
        .enumerate()
        .map(|(index, (line, level, label))| {
            let end = headings
                .iter()
                .skip(index.saturating_add(1))
                .find(|(_, other, _)| other <= level)
                .map_or(last_line, |(next, _, _)| next.saturating_sub(1));
            OutlineEntry {
                label: label.clone(),
                level: u16::from(*level),
                selection: Selection::Lines {
                    range: TextRange {
                        start: *line,
                        end: end.max(*line),
                    },
                },
                kind: OutlineEntryKind::Heading,
            }
        })
        .collect()
}

/// How many lines `markdown` has, as `str::lines` counts them.
#[must_use]
pub fn line_count(markdown: &str) -> u32 {
    u32::try_from(markdown.lines().count()).unwrap_or(u32::MAX)
}

/// The headings of `markdown`: one-based line, level and label.
fn headings(markdown: &str) -> Vec<(u32, u8, String)> {
    let mut found = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for (index, line) in markdown.lines().enumerate() {
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len().saturating_sub(trimmed.len());
        if indent <= 3
            && let Some(marker) = fence_marker(trimmed)
        {
            fence = match fence {
                None => Some(marker),
                Some((character, length))
                    if marker.0 == character
                        && marker.1 >= length
                        && trimmed.trim_start_matches(character).trim().is_empty() =>
                {
                    None
                }
                open => open,
            };
            continue;
        }
        if fence.is_some() || indent > 3 {
            continue;
        }
        let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
        if level == 0 || level > MAX_LEVEL {
            continue;
        }
        let Some(rest) = trimmed.get(level..) else {
            continue;
        };
        if !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
            continue;
        }
        let label = rest.trim().trim_end_matches('#').trim_end().to_owned();
        let (Ok(line), Ok(level)) = (u32::try_from(index.saturating_add(1)), u8::try_from(level))
        else {
            continue;
        };
        found.push((line, level, label));
    }
    found
}

/// The fence a line opens or closes: its character and how many it repeats.
fn fence_marker(line: &str) -> Option<(char, usize)> {
    let character = line
        .chars()
        .next()
        .filter(|first| *first == '`' || *first == '~')?;
    let length = line.chars().take_while(|next| *next == character).count();
    (length >= 3).then_some((character, length))
}
