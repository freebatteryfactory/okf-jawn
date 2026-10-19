//! Versioned, local English warning rules. Matches are advisory, never an execution policy.
//!
//! Normalize each original line with NFKC and Unicode lowercase, retaining its byte boundaries.
//! One fixed-size regex visits matches in source order; monotone line cursors locate them in
//! the original text. At most 32 findings are retained, and further matches are only counted.
//! No result is cached: callers scan the shown text again for every requested revision/rule set.

use okf_jawn_contract::attention::InstructionRule;
use okf_jawn_contract::common::TextRange;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use regex::Regex;
use unicode_normalization::UnicodeNormalization;

/// Changes whenever the rule behavior changes; this is carried on every emitted finding.
pub const RULE_SET_VERSION: &str = "instruction-like-en-v1";
/// Findings retained per item; omitted matches are counted separately.
pub const MAX_FINDINGS: usize = 32;

/// Fixed English forms, with no backreferences, lookaround or unbounded wildcard searches.
const RULES: &str = r"(?mx)
    (?P<role>
        ^[\t\x20]*(?:system|developer)[\t\x20]*:
        |\bdeveloper\s+message\b
        |\bas\s+your\s+administrator\b
        |(?:<\|(?:im_start|start_header_id)\|>|\[im_start\])\s*(?:system|developer)\b
        |(?:\[|<)(?:system|developer)(?:\]|>)
    )
    |(?P<override>\b(?:ignore|disregard|override|forget)\s+
        (?:(?:all|the|any|your)\s+){0,2}
        (?:(?:previous|prior|earlier|above)\s+)?(?:instructions?|rules?|prompts?)\b)
    |(?P<secret>\b(?:reveal|show|print|output|share|disclose|leak|expose)\s+
        (?:(?:me|us|your|the|all|any|hidden|internal|secret|original)\s+){0,5}
        (?:system\s+prompts?|developer\s+prompts?|hidden\s+instructions?|instructions?
            |secrets?|credentials?|passwords?|tokens?|api\s+keys?)\b)
    |(?P<action>\b(?:
        (?:call|invoke|use)\s+(?:(?:a|an|the)\s+)?tool\b
        |(?:run|execute)\s+(?:(?:a|an|the|this|following)\s+){0,2}(?:command|script|code)\b
        |(?:fetch|visit|open|post|upload|send)\s+(?:(?:to|from|data|at)\s+){0,2}https?://
        |(?:send|write)\s+(?:(?:a|an|the|this)\s+)?e-?mail\b
        |e-?mail\s+(?:this|the|your)\s+(?:document|data|file|contents?)\b
    ))
    |(?P<hidden>[\u{00ad}\u{034f}\u{061c}\u{180e}\u{200b}-\u{200f}\u{202a}-\u{202e}\u{2060}-\u{2064}\u{2066}-\u{206f}\u{feff}])
";

/// Compiled once for one workspace scan, then shared across its documents.
pub struct Detector {
    rules: Regex,
}

/// A match whose lines still refer to the untouched body.
#[derive(Debug)]
pub struct Finding {
    /// Explainable rule that matched.
    pub rule: InstructionRule,
    /// Complete original lines containing the match.
    pub passage: String,
    /// One-based inclusive body lines, ready for the application's citation path.
    pub lines: TextRange,
}

/// Bounded evidence; an empty list means only that these rules found no match.
#[derive(Debug)]
pub struct Scan {
    /// First matches in source order.
    pub findings: Vec<Finding>,
    /// Matches beyond the retained bound.
    pub omitted_findings: u64,
}

/// A normalized line's boundaries and the untouched original line it represents.
struct Line {
    normalized_start: usize,
    original_start: usize,
    original_end: usize,
}

impl Detector {
    /// Compile the fixed rule set without a panic or external dependency.
    ///
    /// # Errors
    /// An invalid built-in expression is an internal programming error.
    pub fn new() -> Result<Self, ApiError> {
        Ok(Self {
            rules: Regex::new(RULES).map_err(|error| {
                ApiError::new(ErrorCode::Internal, format!("instruction rules: {error}"))
            })?,
        })
    }

    /// Scan unchanged document text; normalization is used only for matching.
    ///
    /// # Errors
    /// Returns an internal error if the original-line mapping cannot represent a match.
    pub fn scan(&self, text: &str) -> Result<Scan, ApiError> {
        let (normalized, lines) = normalize(text);
        let mut output = Scan {
            findings: Vec::new(),
            omitted_findings: 0,
        };
        let mut first = 0;
        let mut last = 0;
        for matched in self.rules.captures_iter(&normalized) {
            let (rule, span) = [
                ("role", InstructionRule::RoleImpersonation),
                ("override", InstructionRule::OverrideInstructions),
                ("secret", InstructionRule::RevealSecrets),
                ("action", InstructionRule::ExternalAction),
                ("hidden", InstructionRule::HiddenText),
            ]
            .into_iter()
            .find_map(|(name, rule)| matched.name(name).map(|span| (rule, span)))
            .ok_or_else(mapping_error)?;
            if matches!(
                rule,
                InstructionRule::OverrideInstructions
                    | InstructionRule::RevealSecrets
                    | InstructionRule::ExternalAction
            ) && negated(normalized.get(..span.start()).ok_or_else(mapping_error)?)
            {
                continue;
            }
            if output.findings.len() == MAX_FINDINGS {
                output.omitted_findings += 1;
                continue;
            }
            while lines
                .get(first + 1)
                .is_some_and(|line| line.normalized_start <= span.start())
            {
                first += 1;
            }
            last = last.max(first);
            while lines
                .get(last + 1)
                .is_some_and(|line| line.normalized_start < span.end())
            {
                last += 1;
            }
            let begin = lines.get(first).ok_or_else(mapping_error)?;
            let end = lines.get(last).ok_or_else(mapping_error)?;
            let passage = text
                .get(begin.original_start..end.original_end)
                .ok_or_else(mapping_error)?;
            output.findings.push(Finding {
                rule,
                passage: passage.to_owned(),
                lines: TextRange {
                    start: u32::try_from(first + 1).map_err(|_| mapping_error())?,
                    end: u32::try_from(last + 1).map_err(|_| mapping_error())?,
                },
            });
        }
        Ok(output)
    }
}

/// Keep line boundaries even when compatibility forms expand to several characters.
fn normalize(text: &str) -> (String, Vec<Line>) {
    let mut normalized = String::with_capacity(text.len());
    let mut lines = Vec::new();
    let mut original_start = 0;
    for raw in text.split_inclusive('\n') {
        let body = raw.strip_suffix('\n').unwrap_or(raw);
        let body = body.strip_suffix('\r').unwrap_or(body);
        lines.push(Line {
            normalized_start: normalized.len(),
            original_start,
            original_end: original_start + body.len(),
        });
        normalized.extend(body.nfkc().flat_map(char::to_lowercase));
        if raw.ends_with('\n') {
            normalized.push('\n');
        }
        original_start += raw.len();
    }
    (normalized, lines)
}

/// Ordinary warnings such as "never share passwords" are negative controls, not requests.
/// Only the immediate word is consulted; scanning it backwards is bounded to 32 characters.
fn negated(before: &str) -> bool {
    let reversed: String = before.chars().rev().take(32).collect();
    let word: String = reversed
        .split_whitespace()
        .next()
        .map_or_else(String::new, |word| word.chars().rev().collect());
    matches!(
        word.as_str(),
        "never" | "not" | "don't" | "avoid" | "without"
    )
}

fn mapping_error() -> ApiError {
    ApiError::new(
        ErrorCode::Internal,
        "instruction finding has no original line",
    )
}
