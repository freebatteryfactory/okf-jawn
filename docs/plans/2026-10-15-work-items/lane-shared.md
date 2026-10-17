# Construction lanes — shared rules (read first)

Repository: freebatteryfactory/okf-jawn (Rust service + TypeScript UI document workspace). You are the builder of ONE construction lane. The integration owner (the orchestrator) merges, requalifies receipts and owns shared files.

## Read, in order
1. Root `AGENTS.md`: the lane table, Construction, Generator-input rule, Conventions, Operating rules. Every sentence binds you.
2. `SPEC.md`: the product. The sections your lane serves are named in your brief.
3. `docs/plans/2026-10-09-construction-plan.md`: section 1 (rules every lane runs under), your lane's section 3.x, section 6 (decisions), section 7 (risks). This is your plan. Its decisions are made; build them.
4. Your lane's `AGENTS.md` (crates/<lane>/AGENTS.md, ui/AGENTS.md or ui/src/features/views/AGENTS.md).
5. `verification.json`: your construction gates (owner = your lane). Each names its receipt command.
6. `docs/plans/2026-10-08-stage-1b-contract-design.md`: the sections your brief names.

## Hard rules
- Work only in your worktree and only in your lane's directories (the AGENTS.md table). `bun scripts/dev.mjs scope` fails otherwise.
- Never change: shared contracts (`crates/contract`), core port traits (core-cli lanes excepted, per plan 1.7), manifests (`Cargo.toml`, `package.json`), lockfiles, lint config, `verification.json`, `SPEC.md`, root `AGENTS.md`, `qualification/`, `tests/foundation`, `tests/fixtures`, generated directories, or another lane's files.
- When you need one of those changed, STOP that part. Write a request in your report with: the file and symbol (file:line), the SPEC or plan sentence, the exact proposed change, and the failing output or the reason. Then continue with work that does not need it. Do not build an adapter around a wrong shared assumption, and do not invent a production substitute for a missing neighbour (AGENTS.md "Classify a failure before repair").
- A dependency (crate or npm package) is requested the same way, with what `bun scripts/dev.mjs vendor <lib>` reports and why (plan 1.6). Do not add it yourself.
- Real selected libraries, exact installed APIs (read the vendored source under `~/.cargo/registry` or `node_modules`; do not code from memory of another version).
- No `unwrap`, `expect`, `panic!`, `unreachable!`, `[]` indexing, `#[allow]`/`#[expect]`, authored `unsafe`, or lint suppression. Rust tests use the `tests/support/check.rs` idiom (`TestResult`, `err_of`, `some`). `//!` headers on every file.
- Do not fake success. Do not alter expected output to match an implementation. Do not declare a partial suite to be the full one. A filtered receipt command must run at least one test (plan 1.3): check the count.
- Every rule you build gets a test that FAILS when the rule is removed. Before you report a task, apply that mutation yourself, show the red test, and revert it.
- Commit at every green step, one concern each. Subject `type(scope): what.`. Body `Why:`, `What changed:`, `Verified:` (command and result), then `Next:` or `Blocked:`. End with this line:
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
  Write the message to a file under %TEMP% and use `git commit -F`.
- Never push, merge, rebase, amend or force. Lane branches stay local (plan 1.2, N10). The integration owner merges.
- Never dispatch subagents.
- Do not put any personal e-mail address or machine path in a tracked file.
- No Python anywhere.

## Environment (Windows)
- Cargo ONLY from PowerShell, never Git Bash. Always set `$env:CARGO_BUILD_JOBS='2'; $env:CARGO_TARGET_DIR='<a target directory of your own, outside the repository>'` (your own target dir; plan 1.1). The first build is cold and slow. That is expected.
- Several lanes share this machine. Before a heavy build, a mutation batch or a browser run, check free memory (`Get-CimInstance Win32_OperatingSystem` FreePhysicalMemory) and wait if it is under 6 GB. NEVER stop a cargo, rustc, bun or browser process you did not start.
- In PowerShell, `[IO.File]` APIs resolve relative paths against the process cwd, not `Set-Location`, so use absolute paths.
- Bun runs from either shell. If you need browsers: `$env:PLAYWRIGHT_BROWSERS_PATH='<your Playwright browsers folder>'`.
- Your lane gate: `bun scripts/dev.mjs lane <lane>`. It writes `.artifacts/lane/<lane>/<sha>.log` ending `PASS <lane> <sha>`. Run it on a clean tree before you report a milestone.
- Take main with `git merge main` only when the orchestrator tells you to, on a clean tree. On a conflict in a generated directory or a lockfile, take main's side and run `gen`.

## Budget and handoff
Lanes are long. Your context may run out before the milestone is done. So keep `<report path>` current as you go, not only at the end: what is done (commits), what is in progress, what is next, open requests, and the exact state of the gate. Then a fresh builder can continue from your report and `git log`. Three attempts on the same failing check without a new diagnosis means: stop and write a handoff, not more speculative edits.

## Self-check before you return
Past packages each cost four or five review fix rounds, mostly for the same few kinds of defect. They are listed, with a check for each, in core-builder-checklist.md (in this folder). Run it after your gate is green and before your hand-off, and put one line per class in your report: what you ran or answered, and the result. "N/A: no such code in this package" is a valid answer. A UI package answers the classes that apply to it (1, 4, 7, 8 and 9 at least).

## Report
The full report goes to the path in your brief: what you built, every gate command with its exact result, the mutation evidence, every judgement call, and every request to the integration owner. Return only: status (DONE / DONE_WITH_CONCERNS / BLOCKED / BUDGET — partial, report current), the commit list, a one-line test summary, and concerns.
