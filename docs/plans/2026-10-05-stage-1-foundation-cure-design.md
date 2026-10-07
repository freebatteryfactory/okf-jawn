# Stage 1: foundation cure, requalification, storage proof

Status: design approved in conversation by the owner on 2026-10-05. Temporary working document.
It is removed by an ordinary commit when Stage 1 closes; SPEC.md, README.md and AGENTS.md stay
the canonical prose. Base commit: `b205c4a` on `main`. Branch: `integration/foundation-cure`.

## 1. Why this exists

Phase 0 was reopened. A repair landed in `50113b9..b205c4a`, but:

- CI is red on `b205c4a` (run 37352882339): two stale offline tests, 28 Clippy errors in
  `okf-jawn-core`, `cargo fmt --check` failing in 11 files.
- No qualification ran on the repaired code.
- Two code reviews and four planning audits found defects in the new dispatch layer, tests that
  pass with their rule removed, and ports that still cannot express what SPEC.md promises.

Three rounds of reviewing the ports on paper each found about ten new gaps. Stage 1 therefore
ends with real code proving the persistence ports before the other six lanes start.

## 2. Outcome

Stage 1 is done when all of these hold:

1. `main` is green in CI, reached only through the integration branch.
2. Qualification receipts were produced from one clean commit and pass `check-receipts`.
3. `verification.json` records `phase_0_qualified: true` with every Phase 0 gate terminal.
4. The storage lane's gates pass against real Git, SQLite and blob storage, including
   `mutation-crash-reconcile`, `confirmation-single-use` and `draft-per-editor`.
5. The other six lanes sit on a commit whose ports storage has proven, behind a generated
   "not implemented" application skeleton, with briefs that match the code.

Out of scope: any product operation other than what the storage lane needs to prove its ports;
WorkOS end to end; the Explorer; real MCP host rendering (an acceptance gate, `not_run`).

## 3. Decisions

### Owner decisions (product, policy)

| Topic | Decision |
| --- | --- |
| Plan scope | Two stages. Stage 1 in detail now; Stage 2 outlined in section 10. |
| Writers | Only the orchestrator's sub-agents commit. Cursor is stopped for this repo. |
| Reaching main | Sub-agent branches merge into `integration/foundation-cure`; that branch is pushed for CI and merged to `main` only when green. |
| Git | Merge only. No rebase, no squash, no amend of pushed commits, no force-push. Commit often; a commit is context for the next agent. |
| Proving the ports | Paper cure of every planner gap, then the storage lane alone to completion, then fan out. |
| Codegen | Lean on deterministic generation wherever it does not make the build brittle (section 6). |
| Disk | Lane worktrees and Rust build output live on `D:`; the main checkout stays on `C:`. |
| Test lints | Stay absolute: no unwrap, expect, panic or indexing in tests, no suppression. Provide one blessed test idiom and shared helpers. |
| Unbuilt operations | A generated skeleton returns a typed "not implemented" error (HTTP 501), never a success. Acceptance requires the unbuilt list to be empty. |
| Snapshot conflict | Blocked only when an item being snapshotted was itself changed or deleted since the draft's base. Unrelated head movement never blocks. The error lists every conflicting item with its diff. |
| Agent discovery | A read-only model tool `workspaces` lists the workspaces the connection may see (model tools go from 11 to 12). Its description tells the agent to pin reads to the returned head revision. |
| Saved Views | A saved View binds only to sources in its own workspace. |
| Tool names | Unix style stays: `ls`, `grep`, `show`, `log`, `diff`, `blame`, `sources`, `links`, `propose`, `catalog`, `present`, `workspaces`. |
| Earlier, still in force | Drafts: one per (item, editor); saving never creates a revision; `get_item` returns only the caller's own draft; agents, search, export and MCP never see drafts. Several tenants per deployment. Receipts committed under `qualification/receipts/`. Grant cache at most 60 s for Read/Write/Propose; Approve, Review and Admin always fresh. Completed ledger rows kept 7 days; abandoned rows kept until reconciled. |

### Engineering decisions (orchestrator)

| Topic | Decision |
| --- | --- |
| Docling | Restore `docling-core` and siblings to 1.93.6 as SPEC §5 pins. `lock` stops regenerating the whole lockfile. |
| Crash retry | A resumed attempt re-runs the handler under the same `MutationId` against stores that treat a repeated id as a no-op returning the prior row. `VersionStore::commit` is idempotent on `MutationId`. `AbandonedEffects`, `record_effect` and `MutationStore::find` are removed. |
| Failed write | A handler error releases the lease; the same key may be retried. |
| Connector after a crash | A resumed `create_connector` rotates the secret for the existing connector and returns it. A completed one replays `already_issued`. |
| Digest | Hash the re-serialized typed request with sorted keys, independent of `serde_json` `preserve_order`. |
| Tenant-level reads | `Target::Authenticated` for session, `list_workspaces`, catalog. Any signed-in principal may call them; results are filtered by grants. |
| Health | Readiness detail needs sign-in. `/healthz` stays public. |
| Hooks | Tracked hooks under `scripts/hooks/`, installed by `bootstrap` through `core.hooksPath`. `lefthook.yml` goes. |
| MCP text fallback | Tool `content` carries the document text within budget, not a summary. |
| Main protection | Ruleset on `main` (required checks, no force-push) is applied in setup, matched to how the integration branch reaches `main`. Squash and rebase merging are already disabled on GitHub. |

## 4. Phases

1. **Setup** (orchestrator). Remove the seven empty lane worktrees and branches, recreate later
   on `D:`. Point Rust build output at `D:`. Apply the `main` ruleset.
2. **Paper cure** (section 5). Ends at one commit **S** on the integration branch with CI green.
3. **Requalify on S** from a clean tree: `qualify docling`, `qualify mcp-apps`
   and the clean-checkout run. An agent that did none of
   the cure work checks each receipt against what it claims. The orchestrator checks CI.
4. **Record R.** One commit adding receipts under `qualification/receipts/` and updating
   `verification.json` and `vendors.json` only. `check-receipts` passes. Then the integration
   branch merges to `main`.
5. **Storage lane to completion** on `build/storage`, recreated from R. When real code shows a
   port is wrong, the storage agent stops and reports the symbol, the SPEC sentence and the
   failing output. The orchestrator changes the port on the integration branch, regenerates,
   merges to `main`, and merges `main` into the lane.
6. **Freeze and fan out.** Land the generated application skeleton and the unbuilt-operations
   report. Refresh every lane `AGENTS.md`. Recreate the other six lanes from that commit.

## 5. Cure packages

One sub-agent per package, each on branch `cure/<name>` in its own worktree on `D:`. Packages in
the same wave touch disjoint files. The orchestrator regenerates outputs at each merge; packages
do not commit `api/`, `generated/cli/` or `ui/src/api/generated/` except where stated.

### Wave 1 (parallel)

**A. Gates and tooling.** Files: `tests/foundation/{policy,vendor}.test.mjs`, `scripts/dev.mjs`,
`scripts/lib/{lanes,process}.mjs`, `scripts/hooks/*`, `lefthook.yml`, `.github/workflows/ci.yml`,
`.github/CODEOWNERS`, `.gitignore`, `Cargo.lock`, per-crate `Cargo.toml`, `ui/vitest.config.ts`,
`ui/package.json` + `bun.lock` (jsdom), `README.md`, `AGENTS.md`, `REPOSITORY-TREE.txt`,
`verification.json` and `vendors.json` (stale text only), `tests/integration/acceptance.mjs`,
`deploy/.env.example`.

- Offline tests derive operation and tool counts from `api/operations.json` and vendor counts
  from the entries, not literals.
- `lock` performs a minimal update. Docling pin restored.
- `dev.mjs lane <name>`: fmt check, Clippy and tests for that lane's crates with its features
  and `--all-targets`, source-policy, and a scope check that the diff against the merge-base
  stays inside the lane's directories. UI lanes: biome, route generation, `tsc`, filtered
  Vitest. Logs to `.artifacts/lane/<name>/<sha>.log`, ends with one PASS/FAIL line.
- `dev.mjs premerge`: exactly what CI runs, with `--all-features`, not fail-fast, including
  `check-receipts`.
- One lane table in `scripts/lib/lanes.mjs` (directories, crates, features); `lanes`,
  `lanes-reset`, the scope check and the AGENTS.md table read it. Lane parent directory is
  configurable.
- `lanes-reset` checks every existing worktree, treats a failed `git status` as dirty, and never
  uses `--force`.
- On Windows, `process.mjs` removes Git's `usr\bin` from `PATH` for cargo and uses a longer
  cargo timeout.
- CI: later steps run with `if: !cancelled()`; tests and Clippy use `--all-features`; cargo and
  bun caches; concurrency cancel; a `lane` job on `build/**`; audit in its own job.
- Pre-land manifest needs: `time` and `base64` in core, dev-dependencies for storage,
  `tower-http` and `tokio-util` features.
- `clean-checkout` becomes a tracked task. `.cursor/plans/*` is untracked.
- Stale prose corrected: 69 operations, 12 model tools, Schemars as schema authority, vendor
  lookup count, `save_draft` in the acceptance script, gates still marked passed on old commits.
- Fails without the cure: the two red offline tests; a scope-check test with an out-of-lane
  file; a `lanes-reset` test with a dirty worktree whose branch was deleted.

**B. Harnesses.** Files: `qualification/**`, `scripts/lib/provenance.mjs`,
`tests/foundation/harness.test.mjs`, `tests/fixtures/documents/SOURCES.json`.

- Docling orchestrator: attach the exit listener before any await; single-flight memory
  sampling that reads the peak before the child exits; never overwrite a sample with null.
- An `Err` from `converter.convert` is a converter-stage failure and is recorded as such.
  Whatever Docling returns for `must_fail_truncated.pdf` is recorded as observed; a Success or
  PartialSuccess is a FAIL with a finding for the owner.
- Both receipts share one top-level header: `git_sha`, `inputs`, `produced_at`.
- MCP Apps harness: basic-host renders all four tools and asserts each view's text; any failure
  fails the run; process-group kill on POSIX; ngrok lifecycle recorded as closed.
- Lint errors in `qualification/docling/src/main.rs` fixed.
- Fails without the cure: an instant-exit child resolves; a Docling receipt passes
  `check-receipts`; the harness run fails when one view fails.

**C. Contract.** Files: `crates/contract/**`, plus the minimum edits elsewhere needed to keep the
workspace compiling. Runs `gen` and commits generated outputs, because wave 2 branches from it.

- `ApiError.detail` is boxed (cures `result_large_err` everywhere).
- `ErrorDetail::DraftConflict` carries `items: Vec<DraftConflictItem>`, each with `item_id`,
  `draft_base`, `current_revision` and a typed diff (`Vec<FileChange>`). `CommitRequest` no
  longer rejects on head movement alone; each draft's own base revision is the precondition.
- The three `Option` fields that cause Input/Output splits become symmetric.
- `ErrorCode::NotImplemented`; `Target::Authenticated`.
- `list_workspaces` gains alias `workspaces` and model visibility, with the pinning guidance in
  its description.
- Operation table gains columns for what the generator now decides by name: replay policy,
  destructive hint, operator alias.
- `Job` gains a `kind`. `Principal` stays identity only.
- Same-workspace rule for saved Views documented on `ViewBinding`; `PresentRequest` targets
  stay.
- `WorkspacePath` schema carries its full constraint.
- Fails without the cure: Clippy `result_large_err`; a "serialize and deserialize schemas are
  identical for every type" test; a test that every table column is consumed.

### Wave 2 (parallel, after C merges)

**D. Generator.** Files: `xtask/**`, `ui/scripts/**`, `ui/openapi-ts.config.ts`,
`scripts/lib/generation.mjs`.

- Generation fails on any schema split; the rename machinery and hard-coded refs go.
- A synthesized instance of every request and response is validated against its per-operation
  schema and its OpenAPI component (replaces three hand-written fixtures).
- `api/mcp-apps.json` in the shape `rmcp` 3.5 resources need (`name`, `_meta.ui.csp`); one
  source for the bundle manifest.
- Name-matching special cases read the new table columns.
- `CatalogResponse` has a generated output; one definition of the json-render spec shape.
- `create_sandbox_capability` is not generated as a cached query.
- Fails without the cure: the no-split check; the all-operations instance test.

**E. Dispatch.** Files: `crates/core/src/{dispatch,mutations,context,access}.rs`,
`crates/core/tests/{dispatch,authorization}.rs`, `crates/core/tests/support/**`,
`tests/support/**`.

- `MutationStore::release` on handler error. Digest of the typed request with sorted keys.
- Ledger body stripped by `ReplayPolicy` on every path. A returned grant must match the
  workspace and tenant asked for.
- `OperationContext` gains `session_id` and `attempt: First | Resumed`.
  `AbandonedEffects`, `record_effect` and `find` are removed.
- Item-ordering and lifetime lints fixed. `ApiError.field` set from the validator's instance
  path.
- Shared test helpers and a documented idiom: tests return `Result`, use `?` and assertion
  macros, no unwrap/expect/panic/indexing. All 21 violations removed.
- Every dispatch test goes through `dispatch` and fails when its rule is removed. New cases:
  error then retry is not `InProgress`; reordered keys give the same digest with
  `preserve_order` enabled; mismatched grant refused; delegation ceiling tested with Propose;
  resumed attempt re-runs the handler exactly once more against an idempotent fixture store.

**F. Store ports.** Files: `crates/core/src/{storage,jobs,credentials,sandbox,confirmations,
drafts,proposals,uploads,events,conversion,search,readiness}.rs`,
`crates/core/src/application/mod.rs`, new `crates/core/tests/ports.rs`,
`crates/{storage,ingest}/AGENTS.md`.

- Jobs: a core-owned job specification (kind, inputs, initiator, `MutationId`) separate from
  the wire `Job`; progress update; completion can set produced items and artifact.
- `VersionStore`: a core-owned tree-edit type covering create, edit, move, archive, delete,
  folders, types, rules and import cards; `read_file(revision, path)`; author and message on
  candidates; restore keeps its message; `commit` documented idempotent on `MutationId`.
- Ports take resolved `Revision`s and core types, not wire requests carrying `At`.
- `WorkspaceCatalog` keyed by `TenantId`, no `Principal`, no filtering.
- A stage → validate → commit path so `okf-validator` can see a candidate tree.
- `MutationId` on `insert_review`, `insert_receipt` and catalog `create`.
- `create_connector` returns `Issued` or `Existing` with a rotate call.
- Sandbox: the store never sees the plaintext token; one shared hash function.
- `Converter`: keeps structured output and status, takes a timeout and typed settings.
- `BlobStore`: media type stored beside the object, not as an object attribute.
- Fails without the cure: `ports.rs` builds a commit from a claimed job alone; a compile-time
  check that no port signature mentions `At` or a `*Request` type.

**G. MCP helpers.** Files: `crates/mcp/**`.

- `content` carries the text; error results match the tool's output contract.

### Wave 3

**H. HTTP binding** (after E). Files: `crates/server/src/lib.rs`, `crates/server/tests/**`.
Missing principal returns 401 with an `ApiError` body; JSON and body-limit rejections return
`ApiError`; new error codes mapped; binding follows the new dispatch signature.

**I. Integrate** (orchestrator, serial). Regenerate, fix UI type names that moved, run
`premerge`, push, wait for CI. The resulting commit is S.

## 6. Codegen

The operation table in `crates/contract/src/operations.rs` stays the single source.

| Generated from it | Consumer |
| --- | --- |
| Operation and tool counts, policy assertions | offline tests read `api/operations.json` |
| One synthesized request and response per operation | schema agreement test |
| "Not implemented" `Application` skeleton | a new operation fails to compile until routed |
| Unbuilt-operations report | acceptance requires it empty |
| MCP tool list and CLI subcommands | `mcp`, `cli` |
| Lane table | AGENTS.md table, CODEOWNERS, scope check |
| Catalog schema with per-component props | Rust view validation, UI |

Not generated: handler bodies, SQL, UI components, authorization targets. Rule of thumb: generate
what is a projection of the table; hand-write what carries behaviour.

## 7. Operating rules

1. The orchestrator is the integration owner. Only it merges, always `git merge --no-ff`.
2. A task brief names: base commit, SPEC sentences served, files allowed, ports and operation
   ids involved, neighbours to treat as fixed, acceptance command, what is out of scope.
3. An agent works only in its allowed files. It never changes shared contracts, tests it did not
   write, manifests, lockfiles, lint config or records. If it needs one changed it stops and
   reports the symbol (file:line), the SPEC sentence, the proposed change and the failing output.
4. Commit at every green step, one concern each. Subject `type(scope): what.` Body: Why, What
   changed, Verified (command and result), Next or Blocked.
5. Lanes take `main` by `git merge main` at task boundaries with a clean tree.
6. Conflicts in generated directories or lockfiles: take `main`'s side, run `gen`, commit. Never
   edit conflict markers in generated files.
7. To merge a branch: `git merge --no-ff --no-commit`, `gen`, `premerge`, commit, push, wait for
   CI. One at a time. If CI goes red, fix forward or `git revert -m 1`.
8. A merge message gives head and base SHAs, what was delivered, gate results and CI run,
   whether outputs were regenerated, and known gaps.
9. Classify a failure before repair: my defect, an unfinished neighbour, or a wrong shared
   assumption. The last two stop work and go to the orchestrator.
10. A review finding blocks only if it cites a failing command or a named SPEC or AGENTS
    sentence. Everything else is a note.
11. At most two review rounds per task. Round two only checks round-one findings and
    regressions. Anything still open goes to the orchestrator, who merges with a recorded gap,
    opens a new task or changes the contract. It never returns to the same implementer.
12. An attempt is one edit-and-gate cycle on the same failing check. Three attempts without a
    new written diagnosis is a handoff.
13. Each package is verified by an agent that did not write it, and by the orchestrator running
    the gate and reading CI, before it merges.
14. At most four agents build at once.
15. On this machine cargo runs from PowerShell, never Git Bash.

## 8. Risks

- **Storage finds many port changes.** Expected; that is its purpose. Each change is a small
  integration commit, regenerated and merged into the lane. If changes exceed a handful per day
  the orchestrator pauses the lane and batches them.
- **Docling accepts the truncated PDF.** Recorded as a finding; the owner decides. It does not
  block the cure, only the Docling gate's terminal state.
- **Wave 2 packages disagree at compile time.** The integrate step owns cross-package breaks.
- **Schema split check blocks generation.** If a type legitimately differs between input and
  output, the contract package gives it two named types rather than relaxing the check.
- **Requalification prerequisites:** Playwright Chromium, network for basic-host,
  model assets for Docling. Checked before phase 3 starts.

## 9. Evidence

Findings this design answers, by source: two `/code-review` runs (`b13fcd0..b205c4a`), and four
planning audits (cures, library and architecture, plan delivery, devops) on 2026-10-05. The
implementation plan cites file and line for each task.

## 10. Stage 2 outline

After storage is proven: (1) core application, views and the ingest converter in parallel;
(2) ingest jobs and MCP execution; (3) server; (4) workspace UI alongside throughout. Each lane
merges small and often behind the skeleton.

Questions for the owner before Stage 2 (none block Stage 1):

- Is the first release local-first, or hosted multi-user as well?
- Must local desktop hosts connect over stdio, or is HTTP enough?
- Is Windows a supported place to run the product, or only the development machine?
- On a Snapshot conflict, does the editor get a merge view or "reload and reapply"?
- Do exported files use the product's lifecycle words or OKF's, and are app reviews exported as
  OKF `verified`?

## 11. Amendments made while writing the implementation plan

- §5 D: the generator does not synthesize an instance of every request and response. Schemars is
  the single source for both outputs, so that check would compare a thing with itself. It checks
  that every per-operation schema compiles, every `$ref` resolves, and every shipped example
  validates.
- §5 A: jsdom was already removed; `tower-http` and `tokio-util` features are left to the server
  lane; `REPOSITORY-TREE.txt` is deleted in favour of `dev.mjs tree`.
- §5 E/F: after `release`, a retry is a resumed attempt. `VersionStore::find_commit` stays for
  `commit_items`. `ReplayPolicy::AlreadyIssued` carries a JSON pointer to the created id.
- §5 H: `router` keeps its current single-argument signature.
- §4 phase 3: requalification runs in a dedicated worktree on `D:`; receipts recorded are
  docling and mcp-apps, and the clean-checkout receipt stays under `.artifacts/`.
- The implementation plan is `docs/plans/stage-1a/` (one file per package). Phases 5–6 get their
  own plan at commit R.
- Packages E2, T and P were added during execution: E2 hardened dispatch, T the test follow-ups,
  P the UI lint fix and the prose pass.
- The `main` ruleset (the two always-running CI jobs required, force-push and deletion blocked,
  merge commits only) was applied before commit S.
- The 27 guesses from the store-ports verification open the storage-lane plan.
- The rejected external job engine, its harness, workspace member and receipt gate were removed at the owner's instruction (package J); SPEC §5 and §12 now state the Tokio-over-RecordStore design directly.
