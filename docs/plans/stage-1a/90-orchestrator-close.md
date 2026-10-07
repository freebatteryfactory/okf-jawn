## Orchestrator tasks: integrate, requalify, record

Run by the orchestrator in the main checkout (`C:\Users\eayou\code_dir\okf-jawn`, branch
`integration/foundation-cure`) after package H is merged. Cargo from PowerShell with
`$env:CARGO_TARGET_DIR = 'D:\okf\target\main'`.

### Task O.3: Integrate to commit S

**Files:** whatever the cross-package breaks touch; `SPEC.md`, `README.md`, `vendors.json`,
`verification.json`, `crates/core/AGENTS.md`, `.agents/skills/regenerate-api/SKILL.md`,
`docs/plans/2026-10-05-stage-1-foundation-cure-design.md`; formatting-only changes anywhere.

- [ ] **Step 1: Regenerate and find the cross-package breaks**

```bash
bun install --frozen-lockfile
bun scripts/dev.mjs gen
bun scripts/dev.mjs premerge
```

`premerge` runs every step and lists all failures. Expected failures at this point, each fixed
in its own commit with a message naming the packages involved:

- Package E's fixtures against traits package F changed (E Deviation 2, F Deviations 7–8).
- `crates/core/tests/ports.rs` linted for the first time (F Deviation 7).
- `tests/support/check.rs` now exists: replace the local `TestResult` declarations that packages
  B, C, D, F and G made in wave 1–2 with `mod check;` only where the crate already includes
  `tests/support` (core, server, mcp); leave the qualification crates and xtask as they are.

- [ ] **Step 2: Formatting sweep (one commit, no behaviour)**

PowerShell: `cargo fmt --all`. Git Bash: `bun --bun run --cwd ui biome format --write .`
Then `git diff --stat` must show formatting only. Commit as
`style: format the workspace with rustfmt and Biome.` with `Why:` citing the 11 unformatted
Rust files and 4 Biome errors present at `678f919`.

- [ ] **Step 3: Prose that no package owned (one commit)**

- `SPEC.md` line 91 area (§8): replace the sentence that says a Snapshot is rejected when the
  head moved under a draft with the owner's rule: "A Snapshot is blocked only when an item being
  snapshotted was itself changed or deleted since its draft's base; unrelated head movement
  never blocks; the conflict lists every affected item with its diff."
- `SPEC.md` §9: model tools now include `workspaces`; an agent pins reads to the returned head.
- `SPEC.md` §10: a saved or presented View binds only to sources in its own workspace.
- `SPEC.md` §8 / §11: a failed write releases its key and the retry runs as a resumed attempt;
  a resumed `create_connector` rotates the secret; `AbandonedEffects` is gone.
- `vendors.json` utoipa entry, `README.md` and `.agents/skills/regenerate-api/SKILL.md`: utoipa
  is no longer a dependency (package D).
- `crates/core/AGENTS.md`: remove "confirmation re-consume" wording; name `CandidateCheck` as
  core-lane work (F ruling).
- `verification.json`: gates `storage-git-cas-sqlite` and `mutation-crash-reconcile` name
  `find_commit` and "commit is idempotent on MutationId" instead of `find_mutation`; gate
  `view-server-validation` gains "a binding outside the View's workspace is rejected
  (`ViewDocument::bindings_outside`)".
- Design doc: note the accepted deviations (no synthesized instances; `find_commit` kept;
  router signature kept; receipts for docling and mcp-apps only).

- [ ] **Step 4: Full gate, push, CI**

```bash
bun scripts/dev.mjs gen-check
bun scripts/dev.mjs premerge
git push
```

Expected: `premerge` ends with its PASS line; CI on the pushed commit is green in every job.
That commit is **S**. Record its SHA and the CI run id.

- [ ] **Step 5: Protect main and correct the sentences that say it is unprotected**

```bash
gh run view --repo freebatteryfactory/okf-jawn <run-id-of-S> --json jobs --jq '.jobs[].name'
gh api -X POST repos/freebatteryfactory/okf-jawn/rulesets --input - <<'EOF'
{
  "name": "main",
  "target": "branch",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["refs/heads/main"], "exclude": [] } },
  "rules": [
    { "type": "non_fast_forward" },
    { "type": "deletion" },
    { "type": "required_status_checks",
      "parameters": { "strict_required_status_checks_policy": false,
                      "required_status_checks": [] } }
  ]
}
EOF
```

Before sending, fill `required_status_checks` with one `{ "context": "<name>" }` per job name
the first command printed. Expected: HTTP 201. Then, in one commit, update the "no branch
protection is configured yet" sentences in `README.md`, `AGENTS.md`, `.github/CODEOWNERS` and
`SPEC.md` §14 to say what the ruleset enforces. Push; that commit replaces S only if CI is
green on it (a prose-only commit after S is acceptable; requalify on the later one).

### Task O.4: Requalify on S

**Files:** none tracked. Runs in a dedicated worktree so build output and receipts are on `D:`.

- [ ] **Step 1: Worktree and prerequisites**

```powershell
git -C C:\Users\eayou\code_dir\okf-jawn worktree add --detach D:\okf\requalify <S>
Set-Location D:\okf\requalify
bun install --frozen-lockfile
New-Item -ItemType Directory -Force .artifacts\qualification\docling | Out-Null
Copy-Item C:\Users\eayou\code_dir\okf-jawn\.artifacts\qualification\docling\assets.json .artifacts\qualification\docling\
$env:PLAYWRIGHT_BROWSERS_PATH = 'D:\okf\playwright'
bun x --cwd ui playwright install chromium
wsl -l -v
git status --porcelain
```

Expected: `assets.json` copied (it is git-ignored, so the tree stays clean); Chromium installed;
a WSL distribution listed; `git status --porcelain` prints nothing. Anything missing: stop and
report which prerequisite and which gate it blocks.

- [ ] **Step 2: Run the harnesses in the order package B's acceptance section B gives**

```powershell
bun scripts/dev.mjs bootstrap
bun qualification/docling/run.mjs      # both harnesses: exit 0 PASS, 2 FAIL, 3 INCOMPLETE;
bun qualification/mcp-apps/run.mjs     # 1 = refused before it started, no receipt (dirty tree)
bun scripts/dev.mjs clean-checkout     # all exit codes 0, git_status_empty true
bun qualification/record.mjs docling mcp-apps   # copies both receipts, whatever their result, and
                                                # writes each gate's status and phase_0_qualified
```

- [ ] **Step 3: Independent receipt audit**

Dispatch one agent that wrote none of the cure, read-only in `D:\okf\requalify`: "For each
receipt under `qualification/receipts/` and the clean-checkout receipt under
`.artifacts/qualification/clean-checkout/`: confirm `git_sha` equals S. Apply the checklist in
'What an independent reader checks in each receipt' from package B's acceptance section. For
every claim the matching Phase 0 gate in `verification.json` makes, quote the receipt field
that supports it or write 'unsupported'. Report anything a receipt asserts that its harness
could not have observed."

- [ ] **Step 4: Read the derived states**

Nobody decides or types a gate's state. `record.mjs` printed one line per receipt gate:
`passed`, `failed`, `incomplete` or `accepted_with_limitations`, as the criteria of its receipt
fold, and one line for `phase_0_qualified`. A FAIL or INCOMPLETE receipt is recorded like a
PASS, because a failed qualification is evidence. A gate says `accepted_with_limitations` only
when its receipt is a FAIL and every failing required criterion is listed in the gate's
`accepted_failures` by the owner, each tied to the gate that tracks its cure; that qualifies
Phase 0 as `passed` does, and an acceptance whose criterion no longer fails must be removed
before `check-receipts` passes again. If Docling accepted the truncated PDF, or any criterion
the owner did not accept fails, the Docling gate says `failed`, the finding is reported to the
owner, and `phase_0_qualified` stays false. The owner decides what is done about the finding,
not what the record says.

### Task O.5: Record R and reach main

**Files:** Create `qualification/receipts/{docling,mcp-apps}.json`. Modify
`verification.json`, `vendors.json`. Nothing else: R must not touch any receipt input.

- [ ] **Step 1: Bring the receipts into the main checkout and update the records**

```powershell
Copy-Item D:\okf\requalify\qualification\receipts\*.json C:\Users\eayou\code_dir\okf-jawn\qualification\receipts\
bun qualification/record.mjs    # no name: copies nothing, rewrites the derived values
```

The orchestrator types no status in `verification.json`. `record.mjs` writes each receipt
gate's status and `phase_0_qualified` from the receipts just copied, and the orchestrator
commits what it wrote; `check-receipts` fails on any other value. The gates of kind `ci` carry
no status and need no edit, and `clean-checkout` is a tool whose receipt stays under
`.artifacts/`. In `vendors.json`: each library's `qualification` text is rewritten from its
receipt, not from memory.

- [ ] **Step 2: Check, commit, push**

```bash
bun scripts/dev.mjs check-receipts     # check-receipts: 2 receipt(s) valid against HEAD; <one status per gate>; verification.json agrees.
bun scripts/dev.mjs check-offline      # 0 fail
git add qualification/receipts verification.json vendors.json
git commit -F <message file>
git push
```

Message: `chore(record): record Phase 0 gates from receipts produced on <S short>.` with the
shared body (Why / What changed / Verified / Next) and both trailers. Expected: CI green on R.

- [ ] **Step 3: Merge to main through a merge-commit pull request**

```bash
gh pr create --repo freebatteryfactory/okf-jawn --base main --head integration/foundation-cure \
  --title "Stage 1a: foundation cure, requalification and record" --body-file <summary file>
gh pr merge --repo freebatteryfactory/okf-jawn --merge <number>
git fetch origin && git switch main && git merge --ff-only origin/main
git worktree remove D:\okf\requalify
```

The summary file lists S and R, the CI runs, each gate's state, and the gaps recorded in merge
messages, and ends with:

```text
🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

Expected: `main` contains R through a merge commit made by GitHub (squash and rebase merging
are disabled on the repository); CI on `main` is green.

- [ ] **Step 4: Hand back**

Report to the owner: S and R, CI runs, each Phase 0 gate's state with its receipt, gaps recorded
in merge messages, and the storage-lane questions (the "For the storage-lane plan" list from
package F, plus: a random source and `uuid` for the storage `runtime` feature; where citations
persist; the Stage 2 questions in design §10). Then write the storage-lane plan against the
ports as merged.
