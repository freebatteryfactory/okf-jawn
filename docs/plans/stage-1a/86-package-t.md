## Package T: Tooling follow-ups from verification and the wave-1 checkpoint audit

Added after wave 1 and wave 2. Every item is a note from a package verifier or from the
independent checkpoint audit (`checkpoint-1-audit`), chosen because it would cost every lane
agent time, or because it weakens a gate.

- **Branch:** `cure/tooling`, cut from `integration/foundation-cure` after package H is merged.
- **Worktree:** `D:\okf\cure\tooling` (run `bun install --frozen-lockfile` first).
- **Runs in parallel with:** package E2 (`crates/core/**`, `crates/server/**`). No shared files.
- **Files allowed (exact):** `scripts/dev.mjs`, `scripts/lib/gates.mjs`, `scripts/lib/lanes.mjs`,
  `scripts/hooks/pre-commit`, `scripts/hooks/pre-push`, `.github/workflows/ci.yml`,
  `.github/workflows/qualify.yml`, `tests/foundation/*.test.mjs`, `tests/foundation/fixture-repo.mjs`,
  `qualification/record.mjs`, new `crates/mcp/tests/apps.rs`, `README.md`, `AGENTS.md` (only
  sentences describing the hooks and the lane gate).
- **Must not touch:** `crates/*/src`, `crates/core/**`, `crates/server/**`, `xtask/**`, manifests,
  lockfiles, generated directories, `verification.json`, `vendors.json`, `SPEC.md`.

Commits use the shared format with both trailers, one concern per commit. Offline tests are
dependency-free `bun test` files in the style of the existing ones; each new or changed test
must fail before its change and fail again if the rule is removed. cargo only from PowerShell.

### Task T.1: The lane gate checks only the lane's own formatting

`scripts/lib/gates.mjs` `laneSteps` runs `cargo fmt --all --check`, so any lane fails on another
lane's unformatted file.

- [ ] Failing test in `tests/foundation/gates.test.mjs`: the fmt step of every Rust lane is
  `cargo fmt --check -p <each crate of the lane>` and contains no `--all`.
- [ ] Implement; run; commit.

### Task T.2: A fast pre-commit, a full pre-push

`scripts/hooks/pre-commit` runs the whole offline suite (about 28 s, temporary git repositories)
on every commit, which works against "commit often".

- [ ] `dev.mjs check-offline --fast` runs the foundation tests that create no temporary
  repository and spawn no child process (select them by an explicit list exported from one
  place; a test fails if a `*.test.mjs` file is in neither the fast nor the slow list).
- [ ] `pre-commit` runs `check-offline --fast`; `pre-push` keeps the full `check-offline`,
  `cargo fmt --all --check` and the scope check.
- [ ] `pre-push` takes the branch from the refs git passes on stdin
  (`<local ref> <local sha> <remote ref> <remote sha>` per line), not from the current HEAD,
  and applies the scope check to each pushed `build/*` ref.
- [ ] Update `tests/foundation/hooks.test.mjs`; measure and state the `--fast` wall time in the
  commit body (target: under 5 s on this machine).
- [ ] Commit.

### Task T.3: Workflow gaps

- [ ] `.github/workflows/qualify.yml`: `if-no-files-found: error` on the artifact upload.
- [ ] `.github/workflows/ci.yml`: a pull request from a branch of this repository must not run
  CI twice. Keep `push` and `workflow_dispatch`; drop `pull_request` (the push run's checks
  attach to the same commit the pull request shows). State in the commit body that pull
  requests from forks then get no CI, which is acceptable for this repository today.
- [ ] `tests/foundation/ci.test.mjs` asserts both. Commit.

### Task T.4: Gate details

- [ ] `scripts/lib/gates.mjs` `revisionLabel`: a failed `git status --porcelain` counts as dirty
  (label ends `-dirty`). Test.
- [ ] `dev.mjs scope` on a detached HEAD prints one clear line telling the caller to pass a lane
  (`scope <lane>`), exit 1. Test.
- [ ] `scripts/lib/lanes.mjs`: remove the `cures` rows and the `cure/*` branch handling from the
  scope check and the pre-push hook. Cure branches are finished; their scope was checked by the
  orchestrator. Tests updated.
- [ ] `tests/foundation/harness.test.mjs`: the positive HTTP control near line 97 asserts the
  value it receives. The time-budgeted tests (4000 ms budgets) get budgets that hold on a loaded
  machine (they failed once under a parallel cargo build): raise the budget to 20000 ms and keep
  the assertion that the fast path finishes well inside it.
- [ ] `qualification/record.mjs` refuses a protocol-only MCP Apps receipt (one whose basic-host
  section was skipped) with a clear message. Test in `tests/foundation/harness.test.mjs`.
- [ ] `README.md`: name the `routes` task where the UI gates are described.
- [ ] Commit (group as a reviewer would accept or reject together).

### Task T.5: rmcp really decodes the App declaration

Package D asserts the shape of `api/mcp-apps.json` as JSON only.

- [ ] New `crates/mcp/tests/apps.rs`: read the committed `api/mcp-apps.json`, deserialize every
  entry of `resources` into the rmcp 3.5 resource type (read
  `rmcp-3.5.0/src/model/resource.rs` and quote the struct you use in the commit body), and
  assert `uri == "ui://okf-jawn/app.html"`, the MIME type, and that `_meta.ui.csp` survives the
  round trip. Shared test idiom; `mod check;` if the crate's tests include `tests/support`.
- [ ] Gate: `cargo clippy --locked -p okf-jawn-mcp --all-targets -- -D warnings` and
  `cargo test --locked -p okf-jawn-mcp`. Commit.

### Package T acceptance

| # | Command | Expected |
| --- | --- | --- |
| 1 | `git diff --name-only <base> HEAD` | only paths in "Files allowed" |
| 2 | `bun scripts/dev.mjs check-offline` | 0 fail |
| 3 | `bun scripts/dev.mjs check-offline --fast` | 0 fail, wall time stated |
| 4 | `sh scripts/hooks/pre-commit` | exit 0 |
| 5 | `bun scripts/dev.mjs lanes-table` then `git status --porcelain` | table current, no output |
| 6 | PowerShell: `cargo test --locked -p okf-jawn-mcp` | 0 failed, `apps` target present |
| 7 | PowerShell: `cargo clippy --locked -p okf-jawn-mcp --all-targets -- -D warnings` | exit 0 |

Mutations the verifier repeats: put `--all` back in the lane fmt step; make `pre-commit` run the
full suite; restore `pull_request` in ci.yml; set `if-no-files-found: warn`; make a failed
`git status` label clean; let `record.mjs` accept a protocol-only receipt; change the App
resource `uri` in a scratch copy of `api/mcp-apps.json`.

### Deviations

To be filled by the implementer: anything above that cannot be done as written, with evidence.
