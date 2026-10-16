# Archives written at records schema 3

Two archives the build before records schema 4 wrote. They are never written again: the point
of them is that a later build restores the bytes an earlier build wrote (SPEC section 12, "a
backup is independently recoverable"). `crates/storage/tests/archive_fixtures.rs` restores
them on the current build, by replacement and by import.

## What wrote them

| | |
| --- | --- |
| Commit | `c057d548edfe395b1f0cd2eb7f391022ee48f2a2` (main, the last commit at records schema 3) |
| Date | 2026-10-15 |
| Writer | `write_archive_fixtures` in `crates/storage/tests/archive_fixtures.rs`, as tracked at that commit, run in the core M1.4b worktree with that commit checked out and nothing changed. |
| Records schema | `user_version` 3, in the data directory and in the `records.sqlite` the workspace archive holds (with the table `commit_warnings`, empty: a workspace archive keeps no kept warnings) |
| Data directory format | `format.json`: `{"format":"okf-jawn-data","version":1}` |

## The files

| File | Bytes | SHA-256 | States |
| --- | --- | --- | --- |
| `workspace.zip` | 364695 | `0330f589e7b353195a4dde50a697f054613721aa9cfd2958bbdc27d217ff3c4a` | format `okf-jawn-workspace-archive`, version 1 |
| `installation.zip` | 948 | `a390b47f5dc330def9c211259cf650e6e76e1012a0ac8446f0a469ec44ce8916` | format `okf-jawn-installation-archive`, version 1 |

`workspace.zip` holds `manifest.json` (586 bytes), `records.sqlite` (356352 bytes: nearly all
of them the empty tables and indexes of the schema; an archive stores its entries without
compression), `history.pack` (4593 bytes: 5 commits, 13 trees, 19 blobs) and seven
`objects/<digest>` entries. `installation.zip` holds `installation.json` (776 bytes).

## What they hold

The same content as `records-schema-2` (its README lists it item by item): the writer is the
same and its constants are the tests' expectations. What differs from run to run:

- The commits carry the time they were written, so their identities differ. Here the head of
  `refs/heads/main` is `5b7ae0f12587ada59f2c133d1ad59fde94f18643` and the proposal's candidate
  (`refs/okf-jawn/proposals/00000000-0000-4000-8000-000000000020`) is
  `f0011c9f8276fc7555bec8114ae3a27aac34a17b`. The tests read both from the manifest.
- The installation identity is random: `local-owner-34e844f3cacb133e`. So is the connector's
  identity and the tenant event's permanent key.

The workspace is `b95b4aeb-972e-8632-98bb-10c1fe661a52` of the tenant `fixture-tenant`, as at
schema 2 (its identity derives from the writer's mutation). The manifest's `rows_left_out` is 0.

## Nothing personal

Before they were committed both archives were unpacked outside the repository, the pack into
loose Git objects (`git unpack-objects`, 37 objects, each printed with `git cat-file -p`), and
every file, the records database and the pack included as raw bytes, was searched for the user
name of the machine that wrote them, the repository owner's name and address, a `Users`
directory, `AppData`, any drive-letter path (a letter, a colon, a slash of either kind),
`/home/` and `/mnt/`, and for anything shaped like an e-mail address. Nothing was found but
`fixture-editor@fixtures.invalid`. A commit's e-mail field is the access route
(`local_owner`), as the store writes it; its time zone is the writing machine's offset.

## Fixtures for a later schema

The procedure is in `records-schema-2/README.md` and in the header of `archive_fixtures.rs`.
