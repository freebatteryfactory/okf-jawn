## Orchestrator tasks: setup and the per-package loop

These tasks are run by the orchestrator in the main checkout
(`C:\Users\eayou\code_dir\okf-jawn`, branch `integration/foundation-cure`).

### Task O.1: Workspace on D: and removal of the empty lanes

**Files:** none in the repo.

- [ ] **Step 1: Confirm every lane is empty**

Run (Git Bash):

```bash
cd /c/Users/eayou/code_dir/okf-jawn
for l in storage ingest core-cli server mcp-execution workspace-ui views; do
  echo "$l dirty=$(git -C ../okf-jawn-lanes/$l status --porcelain | wc -l) ahead=$(git rev-list --count b13fcd0..build/$l)"
done
```

Expected: seven lines, each `dirty=0 ahead=0`. Any other value: stop and report to the owner.

- [ ] **Step 2: Remove the worktrees and branches without force**

```bash
for l in storage ingest core-cli server mcp-execution workspace-ui views; do
  git worktree remove ../okf-jawn-lanes/$l && git branch -d build/$l
done
git worktree list
```

Expected: only the main checkout is listed. `git worktree remove` without `--force` and
`git branch -d` both refuse if anything is uncommitted or unmerged.

- [ ] **Step 3: Create the D: layout**

Run (PowerShell):

```powershell
New-Item -ItemType Directory -Force D:\okf\cure, D:\okf\lanes, D:\okf\target, D:\okf\playwright | Out-Null
Get-PSDrive D | Select-Object Used, Free
```

Expected: `Free` above 200 GB.

- [ ] **Step 4: Publish the integration branch**

```bash
git push -u origin integration/foundation-cure
```

Expected: branch created on origin; CI starts and fails on the two known offline tests and
Clippy (that is the starting state this plan cures).

The orchestrator's own cargo runs in the main checkout set
`$env:CARGO_TARGET_DIR = 'D:\okf\target\main'` first.

### Task O.2: The loop the orchestrator runs for each package

Packages and waves:

| Wave | Package | Branch | Worktree | Needs `bun install` |
| --- | --- | --- | --- | --- |
| 1 | A Gates and tooling | `cure/gates` | `D:\okf\cure\gates` | yes |
| 1 | B Harnesses | `cure/harness` | `D:\okf\cure\harness` | yes |
| 1 | C Contract | `cure/contract` | `D:\okf\cure\contract` | yes |
| 2 | D Generator | `cure/generator` | `D:\okf\cure\generator` | yes |
| 2 | E Dispatch | `cure/dispatch` | `D:\okf\cure\dispatch` | no |
| 2 | F Store ports | `cure/ports` | `D:\okf\cure\ports` | no |
| 2 | G MCP helpers | `cure/mcp` | `D:\okf\cure\mcp` | no |
| 3 | H HTTP binding | `cure/http` | `D:\okf\cure\http` | no |

A wave's worktrees are created only after every package of the previous wave is merged.
Merge order inside wave 1 is A, then B, then C: A's offline tests read the generated operation
list, so `check-offline` is green again as soon as A lands, and C's regeneration then runs
against A's tests. Wave 2 merges in the order G, F, E, D (smallest blast radius first); E leaves
`crates/server` uncompilable until H, which is expected and stated in E's merge message.

Scope on `cure/*` branches is checked by the orchestrator in Step 4 below, not by the pre-push
hook (the lane table has rows for the seven lanes only; package A Deviation 10).

- [ ] **Step 1: Create the worktree from the integration branch**

```bash
git worktree add -b cure/<name> /d/okf/cure/<name> integration/foundation-cure
cd /d/okf/cure/<name> && bun install --frozen-lockfile   # only where the table says yes
```

- [ ] **Step 2: Dispatch one implementer**

Brief = the Shared interfaces section, the package's section of this plan, the Global
Constraints, and: "Work only in `D:\okf\cure\<name>`. Follow the tasks in order. Commit after
each task in the stated format. If a step cannot be done as written, stop and report; do not
adapt. Finish with: commits made (SHA and subject), the acceptance commands you ran with their
output tails, and anything left open."

- [ ] **Step 3: Dispatch one verifier that did not write the package**

Brief = the package's section, the Shared interfaces, and: "In `D:\okf\cure\<name>`, read-only.
Run every command under 'Package <X> acceptance' and report the result of each. Read
`git diff integration/foundation-cure...HEAD`. Report (a) any file changed outside the
package's allowed list, (b) any test that would still pass with its rule removed — name the
one-line mutation you tried in a scratch copy, (c) any contradiction with a named SPEC or
AGENTS sentence. A finding is blocking only with a failing command or a named sentence."

Blocking findings go back to the implementer once. Round two verifies only those findings.
Anything still open is the orchestrator's: merge with the gap recorded in the merge message,
or open a follow-up task.

- [ ] **Step 4: Orchestrator's own gate**

Run the package's acceptance commands in the worktree (cargo from PowerShell), then:

```bash
git -C /d/okf/cure/<name> diff --name-only integration/foundation-cure...HEAD
```

Expected: every path is inside the package's allowed list.

- [ ] **Step 5: Merge with a merge commit, regenerating outputs**

In the main checkout:

```bash
git merge --no-ff --no-commit cure/<name>
bun scripts/dev.mjs gen            # when crates/contract, xtask or ui/scripts changed
bun scripts/dev.mjs check-offline
```

Then cargo gates from PowerShell with `$env:CARGO_TARGET_DIR = 'D:\okf\target\main'`.
Conflicts in `api/`, `generated/cli/`, `ui/src/api/generated/`, `Cargo.lock` or `bun.lock`:
`git checkout --ours -- <path>`, re-run `gen` (or the lock command the package states), never
edit markers. Commit with this message shape:

```text
merge(cure): package <X> <name> into integration.

Head: <sha of cure/<name>>  Base: <sha of integration before merge>
Delivered: <one line per task>
Gates: <command -> result> ...
Regenerated: yes|no
Known gaps: <none | list>

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

- [ ] **Step 6: Push and read CI**

```bash
git push
gh run watch --repo freebatteryfactory/okf-jawn "$(gh run list --repo freebatteryfactory/okf-jawn --branch integration/foundation-cure --limit 1 --json databaseId --jq '.[0].databaseId')"
```

The integration branch may be red between packages (for example the server crate between E and
H). Record in the merge message which failures are expected and which package clears them.
`main` is not touched until Task O.5.

- [ ] **Step 7: Remove the worktree, keep the branch**

```bash
git worktree remove /d/okf/cure/<name>
```

---
