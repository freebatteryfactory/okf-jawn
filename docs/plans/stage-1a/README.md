# Stage 1a: Foundation Cure, Requalification and Record — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. The orchestrator runs the "Orchestrator" tasks itself and dispatches one implementer per package.

**Goal:** Turn `b205c4a` into a green, requalified, recorded foundation on `main`, with every confirmed defect and port gap cured, so the storage lane can start against ports that are implementable.

**Architecture:** Nine file-disjoint cure packages run in three waves on `cure/<name>` branches in worktrees on `D:`. The orchestrator merges each into `integration/foundation-cure` with a merge commit, regenerating outputs at each merge. One commit S is requalified from a clean tree; a record commit R follows; the integration branch then reaches `main` through a merge-commit pull request.

**Tech Stack:** Rust 1.99.0 (axum 0.8, schemars 1.2, jsonschema 0.58, rmcp 3.5, git2 0.21, rusqlite 0.40, object_store 0.14, okf-core 0.2.7, docling 1.93.5), Bun 1.4.2, TypeScript 7, Vite, Vitest, GitHub Actions.

**Spec:** `docs/plans/2026-10-05-stage-1-foundation-cure-design.md`. This plan covers its phases 1–4. Phases 5–6 (storage lane to completion, freeze and fan-out) get their own plan, written at commit R against the ports as cured, because their tasks depend on the final signatures.

## Global Constraints

- Merge only: `git merge --no-ff`. No rebase, no squash, no amend of a pushed commit, no force-push, no `--no-verify`.
- Commit at every green step, one concern per commit, in the commit format of the Shared interfaces section.
- Only the orchestrator merges. An implementer works only in its package's allowed files; if it needs anything else changed it stops and reports the symbol (file:line), the SPEC sentence, the proposed change and the failing output.
- Lints are absolute in product and test code: no `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, indexing or unchecked arithmetic; no `#[allow]`, `#[expect]` or `cfg_attr` suppression. Tests use the shared idiom.
- Generated directories `api/`, `generated/cli/`, `ui/src/api/generated/` are never hand-edited; change the authored input and run `bun scripts/dev.mjs gen`.
- No product operation is implemented in this plan. No success-returning stand-in exists anywhere.
- Toolchain: Rust 1.99.0, Bun 1.4.2. Docling stays at `docling` 1.93.5 with `docling-core` 1.93.6 (SPEC §5).
- On this machine cargo runs from PowerShell, never Git Bash.
- At most four agents build at once.
- A review finding blocks only if it cites a failing command or a named SPEC or AGENTS sentence. At most two review rounds per package; what is still open goes to the orchestrator, never back to the same implementer.
- An attempt is one edit-and-gate cycle on the same failing check. Three attempts without a new written diagnosis is a handoff.

## Review Focus

Conditions the design implies, most likely to bite a person using this software first. Each has a test in the owning package.

1. A client retries a write with the same idempotency key but its JSON keys in a different order → the stored response is replayed, not a "different request" conflict. Owner: package E (digest test with `preserve_order` enabled).
2. A write fails (stale base, validation inside the handler) and the user retries with the same key → the handler runs again; the user is not told "in progress" for the length of a lease. Owner: package E (handler-error-then-retry test).
3. A request arrives with no signed-in principal, or with a malformed or oversized body → a JSON `ApiError` with 401, 400 or 413, never a 500 text page. Owner: package H (router tests).
4. Someone runs `lanes-reset` after a lane branch was renamed or deleted while its worktree still holds uncommitted work → refused, nothing removed. Owner: package A (lanes-reset test).
5. A Docling fixture finishes before the memory sampler has started → the run completes and records a measurement or an explicit "not measured", never hangs. Owner: package B (instant-exit child test).

---
## How this plan is laid out

One file per package, so each implementer reads only `00-shared-interfaces.md` and its own file.

| File | What | Tasks | Wave | Run by |
| --- | --- | --- | --- | --- |
| `00-shared-interfaces.md` | Names, types, test idiom, commands, commit format every package uses | — | — | everyone |
| `05-orchestrator-setup.md` | Workspace on D:, lane removal, the dispatch → verify → merge loop | O.1–O.2 | — | orchestrator |
| `10-package-a.md` | Gates and tooling: offline tests from generated data, Docling pin, hooks, `lane` and `premerge`, CI, lane table, `lanes-reset`, stale prose | 19 | 1 | sub-agent |
| `20-package-b.md` | Harnesses: Docling runner and stages, receipt header, MCP Apps four-view check, provenance | 10 | 1 | sub-agent |
| `30-package-c.md` | Contract: boxed error detail, typed conflict, 13-field table, `workspaces` tool, `Target::Authenticated`, job kind, regeneration | 12 | 1 | sub-agent |
| `40-package-d.md` | Generator: no schema splits, example validation, table-driven hints, `mcp-apps.json` shape, utoipa removed, catalog output | 8 | 2 | sub-agent |
| `50-package-e.md` | Dispatch: release on error, order-independent digest, policy-driven ledger body, grant check, session and attempt in context, test idiom, rewritten tests | 8 | 2 | sub-agent |
| `60-package-f.md` | Store ports: job spec, tree edits, staged validation, catalog by tenant, artifacts, converter, search, lane briefs, signature guard | 19 | 2 | sub-agent |
| `70-package-g.md` | MCP helpers: text content within budget, error results | 3 | 2 | sub-agent |
| `80-package-h.md` | HTTP binding: 401/400/413/501 as `ApiError`, `Retry-After`, session extension | 4 | 3 | sub-agent |
| `90-orchestrator-close.md` | Integrate to S, protect main, requalify on D:, record R, merge-commit PR | O.3–O.5 | — | orchestrator |

Merge order: A, B, C; then G, F, E, D; then H. The integration branch may be red between
packages; `main` is untouched until Task O.5.

## Rulings on the deviations the section writers reported

Each package file ends with a `Deviations` list: places where the design or the brief could not
be followed as written, with evidence. The orchestrator accepts them as written, with these
rulings:

- **A.** jsdom was already gone; no lockfile change for it. Only Docling's three sub-crates are
  restored; `powerfmt` and `unicase` stay where the lock has them. The pre-push scope check
  covers lane branches; `cure/*` scope is checked by the orchestrator. UI lint errors present at
  `678f919` are cleared in the integration formatting sweep.
- **B.** Receipts are recorded by `qualification/record.mjs` after all three harnesses ran. The
  MCP Apps receipt's inputs are the harness and its fixtures, not product UI files. A must-fail
  PASS counts only if another PDF converted in the same run. `ConversionStatus::Failure` is
  never produced by docling 1.93.5, so the reachable must-fail PASS is a converter error.
- **C.** Seven operations are marked destructive as listed; the rest stay false. The SPEC §8
  sentence that contradicts the Snapshot decision is corrected in Task O.3.
- **D.** The agreement test checks that schemas compile, references resolve and examples
  validate; it does not synthesize instances. utoipa is removed. `view_schema` is the
  `ViewDocument` schema.
- **E.** `complete` and `release` use elided lifetimes. After `release`, the next `begin` returns
  `Abandoned`, so the retry is a resumed attempt. Gates are judged on E's own files until F
  merges.
- **F.** `find_commit` stays on `VersionStore` because `commit_items` needs it; `restore` becomes
  edits inside a commit; commits return `Committed`, not a wire `MutationResult`. Gaps F lists
  as unplanned are contract or product decisions for the storage-lane plan.
- **G.** Text-only hosts get serialized JSON for non-read tools; per-tool prose is lane work.
- **H.** `router` keeps its single-argument form. Wrong content type and malformed JSON are 400.

Nothing in the Rust of this plan was compiled while writing it (cargo was off limits to the
writers); JavaScript and TypeScript blocks were run in scratch copies where the files say so.
An implementer who finds a listing does not compile fixes it within the task's stated
interfaces and says so in the commit; a change to a shared interface is a stop-and-report.
