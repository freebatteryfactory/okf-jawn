# Instruction-like text warnings, version 1

Owner decision, 2026-10-19. This resolves the unselected detector in core M1.5b's R4; it does not claim the detector is implemented. The owning product rule remains SPEC section 11: warnings are advisory, false positives are expected, and a clean scan is never a safety certificate.

## Rules

Use deterministic, local, versioned rules. No model or external service receives document text.

1. Impersonation of a system or developer role: examples include `system:`, `developer message`, `as your administrator`, and fake chat-role markers.
2. Requests to override or ignore prior instructions.
3. Requests to reveal secrets, credentials, system prompts, or hidden instructions.
4. Requests to take an action or send data out: call a tool, fetch or post to a URL, send email, or run a command.
5. Hidden text: invisible or zero-width characters and bidi controls around words. Text surfaced by the converter from comments, alternative text, or white-on-white content is matched like visible text. The detector does not claim to discover content the converter never surfaced.

The first version is English only. State that limit. Matching is case-insensitive on NFKC-normalized text and linear in text length. Preserve a mapping to the original text so normalization never moves a reported citation to a different passage. Detect hidden characters without discarding them during normalization.

## Behavior

- Scan digest text, corrected text, and supplied text.
- A text change or a new rule-set version scans again. Findings from older text or rules must never appear as current findings.
- Each finding names its rule id, rule-set version, matched passage, and location using the same locator system as citations.
- Bound findings per item and report the number omitted by that bound. The implementation must state its selected bound and test overflow; the owner did not select a numeric value.
- Use the wording “may contain instructions aimed at an AI assistant.” Never call a document or passage “malicious.”
- Never block import, hide, rewrite, or drop text, or change the converter outcome because of a finding.
- A clean scan shows nothing and is never labeled “safe.” Workspace reads and outbound actions retain their own authorization.

## Implementation boundary

Build this as a bounded follow-up to the existing Attention implementation, preserving its resolved revision, deterministic ordering, and pagination. Reuse existing text and citation representations. If the public or internal contract cannot carry the required evidence, the integration owner changes the declaration, generated consumers, and affected tests together. Do not replace missing location evidence with an invented locator.

Keep source text, normalized matching text, and displayed matched passages distinct. An implementation that scans on demand may avoid persisting findings; any cache must include the exact text identity and rule-set version. Do not retain stale findings while a new scan is pending.

## Verification and finish line

Write tests first and observe them fail before implementing each behavior. Each rule has a positive and negative corpus, including ordinary prose about instructions or passwords and near-misses. Cover Unicode normalization and case handling, hidden-character locations, corrected and supplied text, text and rule-version changes, the findings bound and omitted count, stable pagination, and preservation of the converter result and source text.

Use local focused checks and affected tests, followed by full CI at the final branch revision under the owner's approved CI policy. This work is complete only when the real Attention path emits current findings with the required evidence and advisory wording; helper-only tests do not establish that connection.
