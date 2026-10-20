# Archives written at records schema 2

Two archives the build before records schema 3 wrote. They are never written again: the point
of them is that a later build restores the bytes an earlier build wrote (SPEC section 12, "a
backup is independently recoverable"). `crates/storage/tests/archive_fixtures.rs` restores
them on the current build, by replacement and by import.

## What wrote them

| | |
| --- | --- |
| Commit | `1b14926a11bff95f4cf375c321af55536437ef0a` (main, the last commit at records schema 2) |
| Date | 2026-10-13 |
| Writer | `write_archive_fixtures` in `crates/storage/tests/archive_fixtures.rs`, run in a detached worktree at that commit. The writer was the tracked one with one line different: that build's `CandidateCheck::check` took only the staged root, so the fixture's check had no `rewritten` argument. |
| Records schema | `user_version` 2, in the data directory and in the `records.sqlite` the workspace archive holds (30 tables, no `commit_warnings`) |
| Data directory format | `format.json`: `{"format":"okf-jawn-data","version":1}` |

## The files

| File | Bytes | SHA-256 | States |
| --- | --- | --- | --- |
| `workspace.zip` | 356461 | `211fa4d8f04efa68b724360fd9554ac0fde288892ccba88ec08db014e81bcef7` | format `okf-jawn-workspace-archive`, version 1 |
| `installation.zip` | 948 | `6f2ee4cd5814d0dc218f0c664504eae0a9e6f1fe5c976ee2620131ab450835c9` | format `okf-jawn-installation-archive`, version 1 |

`workspace.zip` holds `manifest.json` (586 bytes), `records.sqlite` (348160 bytes: 85 pages
of 4096, nearly all of them the empty tables and indexes of the schema; an archive stores its
entries without compression, so this is the size of any workspace archive of that build),
`history.pack` (4551 bytes: 5 commits, 13 trees, 19 blobs) and seven `objects/<digest>`
entries. `installation.zip` holds `installation.json` (776 bytes).

## What they hold

Everything is synthetic. The tenant is `fixture-tenant`; the two subjects are `fixture-owner`
and `fixture-editor@fixtures.invalid`. The identities the writer chose are the numbered
`00000000-0000-4000-8000-<n, twelve digits>`; "item 10" below is the one numbered 10.

The workspace `b95b4aeb-972e-8632-98bb-10c1fe661a52`, named `Fixture workspace`, described
`Synthetic content for restore tests.`:

- **History**, four commits on `refs/heads/main`, all by `fixture-owner`: the workspace's
  first, then `Seed` (mutation 3), `Revise` (mutation 4) and `Rules` (mutation 5). The head is
  `48f7bfab0a76a0941f32e49015d25e6357774349`.
- **Notes at the head**:
  - item 10, `notes/harbor.md`: `The harbor plan.`, a blank line, then
    `See [budget](budget.md) and the [schedule](../plans/schedule.md).` Its two links go to
    items 11 and 12.
  - item 11, `notes/budget.md`: `The budget for the harbor, revised.` (`The budget for the
    harbor.` at the `Seed` commit).
  - item 12, `plans/schedule.md`: `The schedule follows the harbor plan.`
- **A source card**, item 13, `sources/report-pdf.md`, titled `report.pdf`, body `Converted
  text of the fixture report.` Its original object is the 22 bytes `fixture original bytes`.
  Its extraction is completed and names a conversion record (765 bytes of JSON), which names a
  structured export (`{"fixture": "structured export"}`) and a page render (`fixture page
  render`). All four are objects of the archive.
- **Naming rules** in `.okf/rules.yaml`: schema version 1, one rule for `notes/**` (kebab
  case, separator `-`, dates year first as `%Y-%m-%d`, lower-case extension, aliases kept,
  collisions rejected).
- **A proposal**, number 20, `Fixture proposal`, open, created by
  `fixture-editor@fixtures.invalid`, based on the head. Its candidate
  (`refs/okf-jawn/proposals/<proposal>`, `0c601e8661b4d7af5be101002fe66fca61c08472`, mutation
  6) adds item 14, `notes/idea.md`: `A proposed idea.` It has one comment,
  `fixture-comment-1`, by `fixture-owner`: `A fixture comment.`
- **Two drafts** of item 10, based on the head: `fixture-owner`'s (`The harbor plan, drafted
  by the owner.`) and `fixture-editor@fixtures.invalid`'s (`The harbor plan, drafted by the
  editor.`).
- **A review**, number 60, of item 10 at the head, whole document, by `fixture-owner`,
  coverage `current`; and **a receipt**, number 61, of a `get_item` of the same source.
- **A finished job**: an `export_workspace` job, succeeded, whose artifact and output is a
  retained export (the 20 bytes `fixture export bytes`, an object of the archive).
- **A derived dataset** of item 13 at the head, `text/csv`: `harbor,budget`, then `1,2`.
- **A completed upload**, `fixture-upload.txt`: the 20 bytes `fixture upload bytes`.
- **A grant**: `fixture-editor@fixtures.invalid` holds every permission on the workspace.
- **One workspace event**, `changed`, of item 10 at the head.

The data directory also held a second workspace, `Second fixture workspace`
(`33c9b240-a089-81c2-85ce-ca065192eb3a`), with one note. No fixture archive holds it: the
workspace archive is filtered to its own workspace.

The installation archive is the tenant's:

- the installation identity, `local-owner-c8cbbecd1cbdac77` (random, made by the store);
- one connector, `Fixture connector`, permissions `read` and `propose`, allowed both
  workspaces, with no secret and no secret hash;
- one tenant event, `signed_in`, under its permanent key;
- no purge record.

## Nothing personal

Before they were committed both archives were unpacked (the pack into loose Git objects, so
the commit headers were read as text) and every file was searched for the user name of the
machine that wrote them, a `Users` directory with either slash, `AppData`, any drive-letter
path (a letter, a colon, a slash of either kind), `/home/`, `/mnt/`, and anything shaped like
an e-mail address that is not at `fixtures.invalid`. Nothing was found. The only addresses are
`fixture-editor@fixtures.invalid`; a commit's e-mail field is the access route
(`local_owner`), as the store writes it.

## Fixtures for a later schema

Before a change to the records schema, or to anything else an archive stores, write the
fixtures of the schema that is about to end, on the commit before the change. From the
repository root, in PowerShell:

```text
$env:OKF_JAWN_FIXTURE_OUT = "$PWD\crates\storage\tests\fixtures\archives"
cargo test --locked --all-features -p okf-jawn-storage --test archive_fixtures -- --ignored --nocapture write_archive_fixtures
```

The writer makes `records-schema-<n>/workspace.zip` and `installation.zip` beside this
directory, `n` being the records `user_version` of the build it ran on, and prints the
versions and sizes this file states. It refuses to replace a fixture that exists. Then:

1. Unpack both archives outside the repository and repeat the search above. A fixture is a
   tracked file.
2. Write the new directory's `README.md` from what the writer printed.
3. Add the new directory to the restore tests in `archive_fixtures.rs`. The commits differ in
   every run (they carry the time), so the tests read the workspace and its head from the
   archive's manifest.
4. Only then change the schema. The restore tests of every earlier directory must stay green.

The foundation guard (`tests/foundation/gates.test.mjs`) admits a `.zip` only at
`crates/storage/tests/fixtures/archives/records-schema-<n>/workspace.zip` and
`installation.zip`.
