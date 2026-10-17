# Sweep 2a, small part: storage defects and test gaps carried from Wave 1

You are a builder. Storage's reviews through Wave 1 left items that were ruled "carried": small defects, doc sentences that say less than the code does, and tests the reviewers proved missing. You build fourteen of them, in storage. Each one's definition, its source, and the current code (file:line on main c057d54) are in sweep-2a-facts.md. The integration owner's rulings below settle what the facts leave open.

The other items of sweep 2a (the purge rebuilt by reachability, `discard_upload`'s storage side, the ledger's "purged" state, the managed-backup refusal, and the purge and archive bookkeeping) are a separate package. Do not touch them.

## Read first (in this folder where the file is here; the other notes named are the integration owner's local notes, outside git)
1. The repository's README.md, SPEC.md and verification.json, as AGENTS.md asks.
2. sweep-2a-facts.md: the summary table and "Facts that cut across several items", then the section of each item you build. Each section cites where the item was defined and quotes it; those review files are the integration owner's local notes, outside git, so the quotes are what you work from. The r1 to r11 probe source is sweep-1-review-probes.rs.txt in this folder.
3. lane-shared.md and core-builder-checklist.md, both in this folder (read the checklist before you code; answer each class in one line in your report).
4. In the worktree: the root AGENTS.md (the Rust test idiom), crates/storage/AGENTS.md, and the code each item cites.

## Where
- **Location.** The integration owner names the worktree, branch and target in the message that sends you this brief.
- **Runners and messages:** a folder of your own outside the repository.
- **Cargo.**
  - Run it only from the PowerShell tool, `-p okf-jawn-storage`, one build at a time.
  - Other builders run cargo on this machine. Never stop a cargo, rustc or bun process you did not start.
  - Before a build, check that at least 6 GB of memory and 20 GB of disk on D: are free.
- **Linux.** Claims that differ by system run in WSL, invoked as `wsl.exe` from the PowerShell tool, in a clone of your own. Move commits with `git bundle`, and delete your WSL clone and its target at the end.
- **Report:** sweep-2a-small-report.md in the README directory, kept current.

## The items and the rulings
1. **N1, a Rewind counts companion-only differences** (facts section 1). As the reviewer proposed.
2. **MT, the restore trailer check** (section 2). Build the test the reviewer described. Its probe source was not kept, so write it from the description.
3. **MCLEAR, a landing clears only its own pending put** (section 3).
4. **MKEPTART, a kept export's artifact descriptor survives the scrub** (section 4).
5. **S7, an item listed twice is restored as its last listing says** (section 5): a test.
6. **The reviewer's probes r1 to r11** (section 7). Commit each probe that still applies on main as a test, from `sweep-1-review-probes.rs.txt`, adapted to the test idiom. Each one's expected outcome is what main does now if that matches the reviewer's ruling; if main does otherwise, say so and stop that probe.
7. **A backup taken during an import keeps the upload usable, N-6** (section 9).
8. **What a failed replacement restore leaves, N-3** (section 10).
   - Ruling: do not move the checks. Write down exactly what a run that fails at the copy leaves: installation rows, and the workspaces restored before the failure. Put that in `restore_replacement`'s doc and `restore.rs`'s header.
   - Extend `backup_a_constraint_failed_at_the_copy_is_invalid_input` to rerun with a good set and show it completes.
9. **R11A, the pack walk follows every manifest reference** (section 11).
10. **Git errors name no machine path** (section 13).
    - Ruling: an error mapped from git2 never carries an absolute path, a user name or a drive letter. Keep the git error's class and code, and name the path relative to the data directory when one is needed.
    - Test with a probe that makes git2 fail on a path, for example a missing or locked object, and assert the message holds no part of the data directory's absolute path.
11. **The item-filtered log's cost** (section 19).
    - Ruling: `log` with an item filter parses only what can matter. At each commit, take git2's tree-to-tree diff against the parent, and parse only the changed `.md` paths (plus the item's last known path) to learn whether the item changed. A commit whose diff touches no Markdown file costs no parse at all.
    - Measure before and after on the reviewer's shape (200 items, 301 commits, debug build), with the same workload both times. Report both numbers; aim for an order of magnitude.
    - Results must be identical: an equivalence test over a scripted history, plus the existing log tests.
12. **A 64-hex revision, N5** (section 20).
    - Ruling: this repository uses SHA-1, so a revision of 64 hex characters can never name a commit here. It answers `not_found` ("no such revision"), with its JSON Pointer field where the request names one (`/at/revision`, `/base_revision` and so on), never `Unavailable`, which a caller would retry. The same goes for a cursor's tip.
    - Test each place it can arrive: `commit_of`, `oid` (held head) and a cursor.
13. **A Rewind of two notes that traded paths** (section 25). Write the test the file-links reviewer described (`probe_rewind_traded_self_links`). Its source was not kept.
    - Two notes trade paths, each with a self-link, and both are rewound.
    - Assert the bytes come back exactly, and that the check is told exactly `["notes/ref.md"]` for the Rewind's commit (the probe records each call's list, so its whole recording reads `[["notes/ref.md"]]`).
    - Show the mutant F1-f (a Rewind's first hop ending at the destination, edit.rs:628) red.
14. **Storage's `collision_key` fallback uses `collision_key_of`** (section 26; contract ruling 33).
    - `git/item.rs`'s fallback for a string that is not a valid `WorkspacePath` uses the contract's `collision_key_of`, so NFC applies to it too.
    - Test Codex's pair from PR #34: `café?.txt` in NFC and in NFD, which `respelled` and the backstop now refuse.
    - Drop storage's own lower-case-only fallback.

**Closed, not built:** item 22 (a blocked ancestor path in a relink). Core's `taken` is documented narrower than storage's check, and storage refuses the case with `Conflict`; the reviewer allowed either cure. Items 6 (S9), 14 (one local tenant id) and 21 (the Windows case quirk) are already fixed on main (facts table).

## Tests and mutants
Every item with a rule has a test that fails without the rule. Show each mutant red, with file:line and the test. Run at least one known-red control per batch. Commit first, then mutate; `git checkout -- <file>` restores the committed file. Run new tests that touch file names or Git on both systems.

## Must not
- Edit outside `crates/storage`. Never touch a `Cargo.toml`, a lockfile, contract, core, ingest, `api/`, `generated/`, `ui/`, `scripts/`, `tests/foundation`, SPEC.md or the plan.
- Add a records schema step.
- Change an existing expectation except where a ruling says so. Name each one you do change.
- Suppress a lint, or author `unsafe`.
- Use Python at all, not even `python3 --version`.
- Start subagents.
- Push, rebase, amend or force.
- Run a shell command containing PowerShell's dollar-underscore variable anywhere but the PowerShell tool.

## Commits
- One concern each, made with `git commit -F <file>`.
- Subject `fix(storage): what.`, `test(storage): what.` or `docs(storage): what.`.
- Body: `Why:` (cite the item and its source), `What changed:`, `Verified:`, `Next:`.
- End with exactly the two trailer lines the integration owner's message gives.
- Check `git log -1 --format=%s` after every commit.
- Run strict clippy before each commit.

## Done when
- The storage suite passes on Windows and in WSL. Give the counts before and after.
- Strict clippy and `cargo fmt --all --check` pass.
- The schema-2 archive fixtures restore.
- `git status --short` is empty, and your WSL clone is removed.

## Return
- status and the commits;
- the counts on both systems;
- per item: the tests and mutants (file:line, red or survived);
- the measured log cost, before and after;
- each changed expectation;
- concerns;
- the checklist, one line per class.

If your context runs low, return BUDGET with the report current and the tree clean.
