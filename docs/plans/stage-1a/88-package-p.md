## Package P: UI lint and the prose that no package owned

The last package before commit S. After the eight planned packages and the follow-ups E2 and T,
CI passes every pre-merge step except `ui-lint`, and several canonical documents still describe
behaviour the owner changed. This package makes both true.

- **Branch:** `cure/prose`, cut from `integration/foundation-cure` after package E2 is merged.
- **Worktree:** `D:\okf\cure\prose` (run `bun install --frozen-lockfile` first).
- **Files allowed (exact):** `SPEC.md`, `README.md`, `AGENTS.md`, `.github/CODEOWNERS`,
  `.github/workflows/ci.yml` (one line), `vendors.json`, `verification.json`,
  `.agents/skills/regenerate-api/SKILL.md`, `crates/core/AGENTS.md`, `crates/server/AGENTS.md`,
  `docs/plans/2026-10-05-stage-1-foundation-cure-design.md`, the UI files Biome reports
  (`ui/scripts/bundle-app.mjs` and files under `ui/tests/unit/`), `tests/foundation/*.test.mjs`
  (only where a corrected sentence or count is asserted).
- **Must not touch:** any `crates/*/src`, `crates/*/tests`, `xtask/**`, `scripts/**`,
  `qualification/**`, manifests, lockfiles, generated directories, `ui/src/**`.

Commits use the shared format with both trailers, one concern per commit. `bun scripts/dev.mjs
check-offline` must be green after every commit. No cargo is needed.

### Task P.1: `ui-lint` green

- [ ] Run `bun --bun run --cwd ui lint` and record every error and warning with its file.
- [ ] Fix each ERROR in the file where it is reported: formatting through
  `bun --bun run --cwd ui biome format --write <file>`; a lint error (for example a non-null
  assertion in a test) by changing the code so the rule is satisfied — never with
  `biome-ignore`, and never by weakening an assertion (a test that asserted a value is present
  must still fail when it is absent). Leave warnings unless fixing one is a one-line change in a
  file you already touch; list the remaining warnings in the commit body.
- [ ] `bun --bun run --cwd ui lint` exits 0; `bun scripts/dev.mjs routes`, then
  `bun --bun run --cwd ui typecheck` and `bun --bun run --cwd ui test` still pass (47 tests).
- [ ] Commit.

### Task P.2: SPEC.md states the owner's decisions

Replace or add exactly these sentences (find each place by the quoted text, not a line number):

- [ ] §8, in the paragraph starting "Drafts may autosave": replace

  > Snapshot (`commit_items`) commits the caller's selected drafts against an unchanged base; if the head moved under a draft, the whole commit is rejected with a typed conflict (item, draft base, current revision, diff) and the editor rebases by saving against the new head.

  with

  > Snapshot (`commit_items`) commits the caller's selected drafts in one commit. It is blocked only when an item being snapshotted was itself changed or deleted since that draft's base; unrelated changes elsewhere in the workspace never block it. The typed conflict lists every affected item with its draft base, the current revision and a diff, and the editor rebases each by saving against the new head.

- [ ] §8, in the paragraph starting "Every non-read mutation": replace

  > Completed mutations are retained for a bounded period (seven days). Abandoned mutations stay until reconciled.

  with

  > Completed mutations, and mutations released after a failed attempt, are retained for a bounded period (seven days). A mutation whose lease expired without completion or release stays until it is reconciled. A failed write releases its lease, and a retry with the same key and the same request runs as a resumed attempt under the same MutationId; the same key with a different request is a conflict. Each attempt holds a lease token, so a superseded attempt can neither complete nor release the mutation.

  and replace

  > so a retry after an abandoned lease never duplicates an effect or issues a second connector secret.

  with

  > so a resumed attempt, which re-runs the handler, never duplicates an effect. A resumed `create_connector` rotates the secret of the connector it already created and returns it; replaying a completed one returns AlreadyIssued without the secret.

- [ ] §9, first paragraph: replace "Model tools cover listing, search, reading, links, sources,
  log, diff, attribution, optional propose, catalog and present." with "Model tools cover
  workspace discovery, listing, search, reading, links, sources, log, diff, attribution,
  optional propose, catalog and present. The `workspaces` tool lists the workspaces the
  connection may see, each with its head revision; an agent pins later reads to that revision."
- [ ] §10, first paragraph: append "A saved or presented View binds only to sources in its own
  workspace."
- [ ] §14, last paragraph: replace "and no required review exists until owners and branch
  protection are actually configured; local work does not wait for those settings." with "A
  ruleset on `main` requires the two always-running CI jobs and blocks force-push and deletion;
  squash and rebase merging are disabled, so history reaches `main` only through merge commits.
  No required review exists until real owners replace the placeholder handles."
- [ ] Commit. Then check that no other sentence of SPEC.md now contradicts these (search for
  "head moved", "abandoned", "branch protection"); report any you find instead of rewriting it.

### Task P.3: README, AGENTS, CODEOWNERS and the lane briefs

- [ ] `README.md`: the table row for `api/openapi.json` no longer mentions Utoipa (the document
  is assembled as plain JSON from Schemars schemas); the vendor-lookup example uses `schemars`
  instead of `utoipa`; any sentence saying branch protection is not configured is replaced by
  the §14 wording above in one line.
- [ ] `AGENTS.md`: "no branch protection is configured yet" becomes "a ruleset on `main` requires
  the CI jobs and blocks force-push; merges are merge commits only".
- [ ] `.github/CODEOWNERS`: the first comment line says the same; the placeholder-handles line
  stays.
- [ ] `.agents/skills/regenerate-api/SKILL.md`: the vendor lookup names schemars and hey-api,
  not utoipa.
- [ ] `crates/core/AGENTS.md`: name `CandidateCheck` (the production check that runs
  `okf-validator` on a staged candidate directory before commit) as core-lane work, with the
  construction gate `view-server-validation` beside it.
- [ ] `crates/server/AGENTS.md`: add to the lane's responsibilities — the identity middleware is
  the only place a `Principal` or `SessionId` extension is inserted, layered outside the router;
  an unknown path and a wrong method must return `ApiError` JSON (today they are axum defaults
  with an empty body); body-rejection messages must not echo unbounded client input.
- [ ] `.github/workflows/ci.yml`: the `lane` job's artifact upload uses
  `if-no-files-found: error`; extend the existing assertion in `tests/foundation/ci.test.mjs` to
  cover every upload step in ci.yml.
- [ ] Commit (records and briefs may be one commit; the workflow line with its test is its own).

### Task P.4: Records

- [ ] `vendors.json`: remove the `utoipa` entry (it is no longer a dependency); make
  `context7_queries_executed` and any other derived count agree with the entries, as
  `tests/foundation/vendor.test.mjs` requires.
- [ ] `verification.json`: gates `storage-git-cas-sqlite` and `mutation-crash-reconcile` name
  `find_commit` and "commit is idempotent on MutationId" instead of `find_mutation`; gate
  `view-server-validation` gains "a binding outside the View's workspace is rejected
  (`ViewDocument::bindings_outside`)"; replace remaining Utoipa wording in CURRENT entries (leave
  the historical archive record unchanged); add construction gates, each `blocked_on_lanes`
  with an owner lane: `mutation-lease-compare-and-set` (storage: a superseded attempt can
  neither complete nor release) and `http-fallback-errors` (server: unknown path and wrong
  method return `ApiError`). `phase_0_qualified` stays false.
- [ ] `docs/plans/2026-10-05-stage-1-foundation-cure-design.md` §11: add one line each for —
  packages E2, T and P were added during execution; the `main` ruleset was applied before
  commit S; the 27 guesses from the store-ports verification open the storage-lane plan.
- [ ] `bun scripts/dev.mjs check-offline` → 0 fail. Commit.

### Package P acceptance

| # | Command | Expected |
| --- | --- | --- |
| 1 | `git diff --name-only <base> HEAD` | only paths in "Files allowed" |
| 2 | `bun --bun run --cwd ui lint` | exit 0 |
| 3 | `bun scripts/dev.mjs routes`, `bun --bun run --cwd ui typecheck`, `bun --bun run --cwd ui test` | exit 0; 47 tests pass |
| 4 | `bun scripts/dev.mjs check-offline` | 0 fail |
| 5 | `git grep -n -i "utoipa" -- README.md AGENTS.md vendors.json .agents crates/*/AGENTS.md` | no output |
| 6 | `git grep -n "find_mutation\|no branch protection\|head moved under a draft" -- SPEC.md README.md AGENTS.md verification.json .github crates/*/AGENTS.md` | no output |
| 7 | `git grep -n "biome-ignore\|@ts-ignore\|@ts-expect-error" -- ui/scripts ui/tests` | no output |

### Deviations

To be filled by the implementer: any sentence above that cannot be placed as written (the
quoted text is not found, or another sentence contradicts it), with the file and the text found.
Do not rewrite other SPEC sentences on your own; report them.
