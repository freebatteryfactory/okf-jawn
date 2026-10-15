# Sweep 2a: facts for the brief

Collected read-only on main c057d54 (clean tree). Nothing was built or run: every "still applies"
below is a reading of the code and of the tests that exist, not a test result. File paths in the
code columns are under `crates/storage/src/` unless another crate is named. Review and report
files are in this directory. "PR #34" is the storage paths merge 30e9b8e, "PR #35" the file links
merge 3f57ba6, "PR #31" sweep 2b 09ed729, "PR #36" contract batch 2 c057d54.

A naming trap: "N1" means two different things in the ledger entry. In "N1 Rewind counts" it is the
core-cli M1.2 re-review's note N1 (section 1). In "the item-filtered log's cost" it is the sweep-1
review's note N1 (section 19). "N5" and "N11" of the same entry are also the sweep-1 review's
numbering (`sweep-1-review.md:208`, `:214`). Every label was found; none is missing.

## Summary table

| # | Item | Defined at | Still applies | Main code location |
| --- | --- | --- | --- | --- |
| 1 | N1 Rewind counts companion-only differences | `lane-core-cli-m1-2-rereview.md:112`; plan `:851` | yes | `git/history.rs:80-112` (`diff`), `:315` (`changes_item`) |
| 2 | MT: restore trailer check | `lane-storage-m2-f1-rereview.md:11,18,27`; plan `:859` | yes | `backup/restore.rs:1064-1078` (`is_restore_commit`) |
| 3 | MCLEAR: a landing clears only its own pending put | `lane-storage-m2-p1s-rereview.md:24,28`; plan `:861` | yes | `puts.rs:61-77` (`land`), `backup/mod.rs:527` |
| 4 | MKEPTART: a kept export's descriptor survives the scrub | `lane-storage-m2-pass3-rereview.md:12,14`; plan `:862` | yes | `backup/snapshot.rs:217-220` |
| 5 | S7: an item listed twice restores as its last listing | `lane-core-cli-m1-2-fix5-rereview.md:25,27`; `sweep-1-report.md:101-115` | yes | `git/edit.rs:603-612` (`move_back`) |
| 6 | S9: the occupied private path | `lane-core-cli-m1-2-fix5-rereview.md:12`; `sweep-1-report.md:106` | no (fixed by PR #34, 834532d) | `git/edit.rs:784-826` (`park`), test `tests/paths.rs:1334` |
| 7 | Reviewer's probes r1 to r11 (and the R-I14 probe) | `sweep-1-review.md:20,101-112`; probes `sweep-1-review-probes.rs.txt:399-932`; ledger `progress.md:1239` | partly | `tests/restore_item.rs:1364` is the one composed-edits test |
| 8 | A managed backup is never imported as a source | `progress.md:989-990`; `lane-storage-m2-b1r3-brief.md:62-64`; plan `:864` | yes | writer side `backup/archive.rs:637-700`; ingest side absent (`ingest/src/import.rs:435-468`) |
| 9 | A backup during an import keeps the upload usable (N-6) | `lane-storage-m2-b1r-rereview.md:153`; plan `:865` | yes | `backup/snapshot.rs:83,151,170-176`; `uploads.rs:481-570` |
| 10 | What a failed replacement restore leaves (N-3) | `lane-storage-m2-b1r-rereview.md:138`; plan `:866` | partly | `backup/mod.rs:108-193` (`restore_replacement` doc); test `tests/backup.rs:5426` |
| 11 | R11A: the pack walk follows every reference | `lane-storage-m2-b1r4-rereview.md:472-478,512` | yes | `backup/restore.rs:398-414`, called `:334` |
| 12 | N-24: an unopenable repository cannot purge itself | `lane-storage-m2-b1r4-rereview.md:484-487` | yes | `purge/mod.rs:304` |
| 13 | Git errors name no machine path | `lane-storage-report.md:3809-3813`; `lane-storage-m2-b1r4-rereview.md:488-490`; plan `:869` | yes | `git/repo.rs:571-576` (`git`) |
| 14 | One local tenant id | `lane-server-report.md:339-343`; `progress.md:1096`; plan `:867` | no (done in production code) | `contract/src/identity.rs:157,273`; `server/src/identity.rs:331`, `sessions.rs:230` |
| 15 | Outcomes reach the caller (N-17) | `lane-storage-m2-b1r4-rereview.md:148-153`; plan `:868`; ruling 14 | partly | `rows_left_out` built (154c6d7); `unread` has no wire field; `backup/archive.rs:452` carries `purges` only |
| 16 | The ledger's "purged" state and the six ledger limits | `pr20-ledger-r2-brief.md:21-22`; `progress.md:1162`; plan `:904-911` | yes | `schema.rs:34`, `mutations.rs`, `purge/rows.rs:39-52` |
| 17 | The installation archive is tracked before it is kept | `progress.md:1329`; plan `:913` | yes | `backup/mod.rs:292-339` (`put_for` at `:335`) |
| 18 | A resumed purge's counts | `progress.md:1329`; plan `:914` | partly (see section) | `purge/mod.rs:658-684` |
| 19 | The item-filtered log's cost (sweep-1 N1) | `sweep-1-review.md:118-124,204` | yes | `git/history.rs:60,315-330`, `git/read.rs:23-37,87-93` |
| 20 | A 64-hex revision answered Unavailable (sweep-1 N5) | `sweep-1-review.md:72,208` | yes | `git/repo.rs:268`, `git/mod.rs:707` |
| 21 | The Windows case quirk (sweep-1 N11) | `sweep-1-review.md:214`; probe `sweep-1-review-probes.rs.txt:932` | no (fixed by PR #34, 7b01ed7) | `git/edit.rs:209-236`, test `tests/paths.rs:1690` |
| 22 | A blocked ancestor path in a relink (sweep-1 N6) | `sweep-1-review.md:209`; `sweep-1-report.md:193`; `progress.md:1367` | yes | core `application/history.rs:519-560` (`taken`) vs `git/edit.rs:255-280` (`occupied`) |
| 23 | The purge rebuilt by reachability | plan `:895`; `contract-batch-2-rulings.md:58-59` | yes (not built) | `purge/mod.rs:268-350` (plan), `:566-690` (objects), `purge/references.rs` |
| 24 | `discard_upload`'s storage side | `contract-batch-2-report.md:134,514-518,534-541` | yes (not built) | no port method in `core/src/uploads.rs`; `uploads.rs`; `schema.rs:226` |
| 25 | A Rewind of two notes that traded paths, asserting what the check is told (F1-f) | `storage-file-links-review.md:281-284,326` | yes | probe path `git/edit.rs:628`; nearest test `tests/restore_item.rs:1091` |
| 26 | `collision_key` fallback uses `collision_key_of` | `progress.md:1656-1657`; `contract-batch-2-rulings.md` ruling 33 | yes | `git/item.rs:426-439`, `:443-452`; `contract/src/identity.rs:512` |

## Facts that cut across several items

- Records schema is at 3 (`schema.rs:15-18`, `RECORDS_V3` at `:401`, landed in PR #31 as db0753c).
  Core M1.4b is to own schema 4 (the seal), and the `discard_upload` tombstone takes the next step
  (`progress.md:1666`, `:1748`). The ledger's "purged" state (section 16) would be a third step: the
  `mutations.state` CHECK lists three states (`schema.rs:34`). The plan's standing rule (plan
  `:876-878`) is that a records schema change first writes its own archive fixtures with the kept
  writer (`tests/archive_fixtures.rs:791`, fixtures in `tests/fixtures/archives/records-schema-2/`).
- Only one thing deletes an object today: a purge (`blobs.rs:266-287`, `remove_for`, "Only a purge
  calls this"). There is no other reclamation of unreferenced bytes.
- The sweep-a-brief (`sweep-a-brief.md`) is the old sweep-A brief. It predates the split; several
  of its rulings were done in sweep 1 (R-I14 relink, `FormatHasNoLocator`, export-only picture
  fallback) and are not in the 2a list. It states S7 backwards (`sweep-1-report.md:189`).
- The plan's carried entry "Purge and backup outcomes reach the caller" (plan `:868`) still asks
  for `unread` and `purge_progress` on the wire. Contract ruling 14 (`contract-batch-2-rulings.md:59`)
  says "No `unread` on the wire", and `contract-batch-2-brief.md:19` says "there is no
  `PurgeReport.unread`". The two documents disagree; the plan entry has not been edited.

---

## 1. N1: a Rewind counts companion-only differences

1. Defined: `lane-core-cli-m1-2-rereview.md:112-121` (note N1); parked as a storage obligation at
   `progress.md:630`, moved into the sweep at `:804`; plan `:851`; brief text `sweep-a-brief.md:29-33`.
2. Definition: core refuses a one-item Rewind when the item-limited diff lists nothing, but storage's
   item-limited diff lists only the item's own `.md`. A change that touches only `.okf/sources/<id>.json`,
   `.okf/supplied/<id>.json` or `.okf/corrections/<id>/**` (a `CorrectDigest`, an appearance-only
   `WriteSourceCard`) is refused as "nothing differs" although `RestoreItem` would restore it. The
   brief adds that the item-filtered log shows companion changes too, and that the diff receipt
   cites nothing for companion paths (check that it still does).
3. Current code: still applies.
   - `git/history.rs:86-92` collects only the item's Markdown path at each of the two trees
     (`find(repository, tree, item)`), and `:108-112` drops every delta not on those paths.
   - `git/history.rs:315-330` (`changes_item`) compares the item file text only, so a companion-only
     commit does not appear in an item's Timeline either.
   - Core still refuses: `core/src/application/history.rs:352-356` ("nothing differs between the base
     and the target").
   - The port doc says only "Limit the comparison to this item, following its moves"
     (`core/src/storage.rs:408`); it does not say whether companions count.
   - The companion path helpers already exist in storage (`appearance_file`, `supplied_file`,
     `corrections_folder`, used at `git/edit.rs:580-582`).
   - No storage test diffs a companion-only change (`tests/versions.rs:281` and `:2166` are the only
     `diff` calls; neither is companion-only). Core's `restored` path list (`application/history.rs:360-371`)
     would receive `.okf/...` paths once the diff lists them; they parse as `WorkspacePath`.

## 2. MT: the restore trailer check

1. Defined: `lane-storage-m2-f1-rereview.md:11` (finding 4), `:18` (mutant table), `:27` (suggestion);
   ruling at `progress.md:833-834`; plan `:859`; brief `sweep-a-brief.md:48`.
2. Definition: import A stops after its restore commit; import B of the same archive under another
   mutation then finds the head's only parent equals the archive head. Without the trailer test B would
   skip its commit and succeed with no commit carrying B; with it B is refused ("a restore fills only a
   blank workspace"). The reviewer's probe `rr_other_mutation_after_stop` passes on the real code and
   fails on mutant MT, so the committed suite has a gap. The probe source is not kept in this directory.
3. Current code: still applies. The check is `backup/restore.rs:1064-1078` (`is_restore_commit`:
   `parent_ids == [main]` and `trailer_of(message) == Some(mutation)`), used at `:1012`. In
   `tests/backup.rs` the only stop-after-commit test is `backup_an_import_stopped_after_its_commit_writes_no_second_one_on_retry`
   (`:2354`), which retries under the same mutation `uuid!(84)`; no test imports under a second
   mutation (`grep` for "other_mutation", "another mutation", "different mutation" finds nothing).

## 3. MCLEAR: a landing clears only its own pending put

1. Defined: `lane-storage-m2-p1s-rereview.md:24` (mutant table), `:28` (note); `progress.md:930`;
   plan `:861`; brief `sweep-a-brief.md:49`.
2. Definition: mutant MCLEAR makes an archive record clear every pending put of the workspace instead
   of landing the one digest; it survives (lib 37, purge 47, backup 46 pass). Suggested test: another
   pending put in the same workspace survives an archive record.
3. Current code: still applies.
   - `puts.rs:61-77` (`land`) deletes by (tenant, workspace, digest); `clear` (`:109-120`) deletes all
     of a workspace's and is the purge's.
   - The archive record's call is `backup/mod.rs:527`. Other call sites: `git/export.rs:72`,
     `git/mod.rs:165`, `records.rs:287,483,795`, `sandbox.rs:64`, `uploads.rs:340`, `backup/restore.rs:1275`.
   - `tests/purge.rs:3544` (`a_workspace_archive_and_a_written_history_land_their_pending_puts`) and
     `:3038` (`every_record_that_references_a_put_object_lands_its_pending_put`) each assert the count
     is 0 after the record, from a state with only that put, so a clear-all mutant passes them.
     `tests/backup.rs:7086` (B-1) pins "a card lands only the puts of the objects it names" for the
     commit path, not the archive path.

## 4. MKEPTART: a kept export's artifact descriptor survives the scrub

1. Defined: `lane-storage-m2-pass3-rereview.md:12` (mutant), `:14` (non-blocking gap); `progress.md:939`;
   plan `:862`; brief `sweep-a-brief.md:50`.
2. Definition: the job-artifact scrub removes the descriptor of a backup artifact the archive leaves
   out; mutant MKEPTART drops the `NOT IN` clause so every descriptor is removed, and survives. No test
   keeps a job with a non-backup artifact (an export) and asserts its descriptor survives.
3. Current code: still applies. The scrub is `backup/snapshot.rs:217-220` (`UPDATE jobs SET record =
   json_remove(record, '$.artifact') WHERE ... artifact_id NOT IN (SELECT artifact_id FROM artifacts)`).
   The two tests that touch it, `tests/backup.rs:3268` and `:8870`, both use a workspace backup artifact
   only; neither keeps an export.

## 5. S7: an item listed twice is restored as its last listing says

1. Defined: `lane-core-cli-m1-2-fix5-rereview.md:25` (mutant S7 survived, "no test pins last-listing-wins
   for a mover"), `:27`; `progress.md:790-791`; moved to storage at `sweep-1-report.md:101-115`
   and `progress.md:1198`.
2. Definition: the port rule is "an item listed twice is restored as its last listing says"
   (`core/src/storage.rs:256`). Needed test (`sweep-1-report.md:113-115`): one commit with
   `[RestoreItem{item, R1}, RestoreItem{item, R2}]`, the item at a third path now and at different
   paths at R1 and R2; expect the item at its path at R2 with R2's bytes and a referrer's link pointing
   there. Mutant S7 is first-listing-wins. Note `sweep-a-brief.md:42` words it backwards.
3. Current code: still applies. `git/edit.rs:603-612` (`move_back`) builds `destinations` with
   `BTreeMap::insert` in listing order, so the last listing wins. The only duplicate-listing test,
   `tests/restore_item.rs:976` (`an_item_listed_twice_ends_as_its_last_listing`), uses a card whose path
   never differs between the listings (it compares companions, `card(12, ..)` has a fixed path), so a
   first-wins mutant in `move_back` is not exercised. No test in `tests/` lists a mover twice.

## 6. S9: the occupied private path

1. Defined: `lane-core-cli-m1-2-fix5-rereview.md:12` ("Collides with a real file only as a Conflict
   refusal (occupied check); that refusal is untested"), `:27`; `sweep-1-report.md:106,115`.
2. Definition: a Rewind needing the private path `okf-jawn-rewind-<item uuid>.md` at the root, with a
   file already there, was to answer `Conflict` naming the path (mutant S9: the `occupied(&private)` check
   removed).
3. Current code: no, fixed by PR #34 (834532d, "an item is parked at a free private name"). The premise
   changed: the private path is no longer at the root and no longer a refusal. `git/edit.rs:784-826`
   (`park`) puts it beside the item's current path and takes the first free name of
   `okf-jawn-<purpose>-<id>.md`, `-1`, `-2`, ... (free = `staged.occupied` is false and no link resolves
   to it, `:807`). Test: `tests/paths.rs:1334` (`a_rewind_parks_past_a_taken_private_name_and_leaves_links_to_it`,
   helper `rewound_past` `:1287-1326`) runs a Rewind past a plain file holding the first private name and
   past a link that points at it; the case-only rename equivalent is `:1155`. Residual: the old expected
   `Conflict` no longer exists, so the S9 sentence in the ledger is obsolete rather than open.

## 7. The reviewer's probes r1 to r11

1. Defined: `progress.md:1239` ("the reviewer's probes r1 to r11 as committed storage tests"); the
   probes are the sweep-1 review's, `sweep-1-review.md:20,101-112` and `sweep-1-review-probes.rs.txt`
   (section 1 of that file, a scratch `crates/storage/tests/zz_review_probe.rs`, 16 tests). Related
   ruling: the builder's R-I14 probe `sweep-1-r-i14-probe.rs.txt` is "adopted as a committed storage
   test in part 2" (`progress.md:1198`).
2. Definition (the storage probes, line in the probes file): `r1` `:399` moved several times, deleted,
   unrelated commits since, plus target = last holder, plus deleted-rewound-deleted (its `r3` lines are
   inside); `r2` `:494` deleted in a commit shared with other changes, folder emptied; `r4` `:557`
   referrers moved, edited or deleted since; `r5` `:604` an item that links to itself; `r6` `:629` link
   forms (inline, fragment, title, image, angle bracket, reference style); `r7` `:679` a source card with
   a correction and supplied text; `r8` `:763` a failure at each step writes nothing; `r9` `:855`
   an item renamed in letter case only and then deleted (the failing one, core's B1); `r10` `:895` a file
   where a folder of the last path is needed; `r11` `:932` a folder of another letter case holds the last
   name. (`r8` also covers the replay: the same mutation lands once, a third call replays.) The
   probes drive core's composed edits through a copy of core's `core_edits` (`:340-380`), because
   storage cannot depend on core's application.
3. Current code: partly.
   - One committed storage test composes core's three edits: `tests/restore_item.rs:1364`
     (`a_rewind_of_a_deleted_item_tells_the_check_the_referrers_its_move_rewrote`, moved once, deleted,
     Rewind; asserts the referrer's bytes and what the check is told). It is the R-I12 test, not r1.
   - `tests/restore_item.rs:1138` (`a_deleted_item_is_restored_without_rewriting_any_link`) pins the
     plain restore. No committed test covers r1, r2, r4, r5, r7, r8, r10 as such.
   - r9 is fixed in core (collision-key compare, `core/src/application/history.rs:478`) and tested
     against the fake; no storage test runs it. `tests/restore_item.rs:1027` is a different scenario.
   - r11 describes behavior PR #34 changed: creating `notes/new.md` under a tree holding `Notes/` is now
     refused (see section 21). The probe's "not reachable this way" branch (`:942-947`) is now the path
     taken.
   - r5 and r6 overlap with PR #35's self-link and file-link tests (`tests/paths.rs:1565-1650`,
     `file_links_*`), which cover moves and a Rewind of a move, not the deleted-item three-edit flow.
   - The R-I14 builder probe has three tests (`sweep-1-r-i14-probe.rs.txt:209,239,317`): the last holder
     is the first parent of the newest commit that changed the item; the composed edits relink and one
     restore does not; the composed edits conflict when the last path was taken since. None is committed
     under those names.

## 8. A managed backup is never imported as a source document

1. Defined: ruling at `progress.md:989` (B-2) and `:990`; `lane-storage-m2-b1r3-brief.md:62-64`; plan
   `:864`; brief `sweep-a-brief.md:57-61`; the three Codex threads that repeated it, `progress.md:1333`.
2. Definition: the workspace archive writer refuses a card whose original is a backup or written-history
   object, so such a card makes every later backup of its workspace fail. Ingest's import should refuse,
   with `InvalidInput` on the upload ("restore it instead"), to convert an upload whose digest is one of
   the tenant's managed workspace or installation archives or a written history. A restore's own upload
   stays allowed. The brief says a storage query for a digest's kind is a port change.
3. Current code: still applies.
   - The writer side exists: `backup/archive.rs:637-700` (`refuse_missing`) fails with "a backup or
     written history, which an archive never holds" (`:672`); `backup/snapshot.rs:77-110`
     (`archive_objects`) is how storage knows the excluded digests (artifacts of kind workspace or
     installation backup, all `history_exports`, all `backup_archives`).
   - The ingest side does not exist. `ingest/src/import.rs` has no "backup" check (the only match for
     "backup" is an unrelated line); `import.rs:435-468` reads each upload and calls
     `ports.uploads.consume_batch`.
   - No core port tells a digest's kind (no match for a managed-object query in `core/src`).
   - The restore job consumes its archive upload through a different path: `ingest/src/handler.rs:396`
     calls single `consume`, import calls `consume_batch` (`import.rs:468`). Storage's `consume_batch`
     (`uploads.rs:524`) already refuses under a purge refusal in its own transaction.

## 9. A backup taken during an import keeps the upload usable (N-6)

1. Defined: `lane-storage-m2-b1r-rereview.md:153` (N-6); carried at `progress.md:990`; plan `:865`;
   brief `sweep-a-brief.md:63-66`.
2. Definition: running jobs are not archived (`snapshot.rs`), so a backup between an import's upload
   consume and its card commit restores the upload as consumed by a job the archive dropped, with no
   job and no card. Rule: in the archive an upload consumed only by a job the archive drops is written
   back as completed and unconsumed; the import can then run again. Test: backup in that window,
   restore, then start an import from the upload; it lands.
3. Current code: still applies. `backup/snapshot.rs:83` keeps only `succeeded`, `failed`, `cancelled`
   jobs (`FINISHED_JOBS`, used at `:151,173`), and `:174` keeps upload rows with an object as they are;
   nothing under `backup/` mentions `consumed_by`. `uploads.rs:481-570` sets `consumed_by` at consume.
   `restore.rs:35-37` says uploads belong to the installation the archive came from for an import, so
   the case matters for a replacement restore (every archived row is copied).

## 10. What a failed replacement restore leaves (N-3)

1. Defined: `lane-storage-m2-b1r-rereview.md:138-143` (N-3); carried at `progress.md:990`; plan `:866`;
   brief `sweep-a-brief.md:68-70`.
2. Definition: a read-only pre-check covers most failures, but the copy finds some (an encoding the
   check missed, a key collision between two archives of one set). They leave the installation rows and
   the earlier workspaces written; a rerun with a good set completes. Either move each such check into
   the pre-check, or state in the `restore` doc exactly what is left and test that a rerun completes.
3. Current code: partly.
   - Not done: the doc of `restore_replacement` (`backup/mod.rs:108-122`) says a run again "accepts
     and leaves as it is a workspace this same archive file already restored in full"; it does not say
     what a failed run leaves. `backup/restore.rs:1-40` is silent too.
   - Done in part: rerun after a stop is tested (`tests/backup.rs:1768`
     `..._stopped_inside_a_workspace_completes_on_retry`, `:1993` `..._run_again_with_every_archive_completes`).
     The copy-time failure test (`:5426`, `backup_a_constraint_failed_at_the_copy_is_invalid_input`: the
     second archive repeats the first's creation mutation) asserts only the error and does not rerun
     with a good set. `restore_replacement` checks each archive first (`:131-147`) then installs the
     installation rows (`:149-153`) then restores each workspace (`:155-186`).

## 11. R11A: the pack walk follows every manifest reference

1. Defined: `lane-storage-m2-b1r4-rereview.md:472-478` and `:512` (mutant table, "R11A TIPSONE");
   adjudicated `progress.md:1099-1100`; brief `sweep-a-brief.md:73`. Probe:
   `lane-storage-m2-b1r4-rereview-probes.rs.txt:552` (`rr11_a_pack_lacking_a_blob_only_the_candidate_needs_is_refused`);
   its companion `:598` (`rr11_an_imported_real_archive_is_archived_again`).
2. Definition: no committed test has a pack that lacks an object only a second reference (here a
   proposal candidate) needs. Mutant R11A (the walk follows only the first manifest reference) survives
   every committed test and dies only to the `rr11_` probe. The code is right; adopt the probe.
3. Current code: still applies. The walk is `backup/restore.rs:398-414` (`refuse_unpackable(scratch,
   tips)`, pushes every tip, `PackBuilder::insert_walk`); `tips` is built from all
   `manifest.references` at `:327-332`, called at `:334`. `grep -c "rr11_" tests/backup.rs` is 0;
   `rr9_`/`rr10_` tests at `tests/backup.rs:8220,8921,8934` cover a pack lacking a blob on the head's
   or history's own line.

## 12. N-24: a survivor whose repository does not open cannot purge itself

1. Defined: `lane-storage-m2-b1r4-rereview.md:484-487`; adjudicated `progress.md:1100`; brief
   `sweep-a-brief.md:74`.
2. Definition: the workspace's own plan opens its repository, so an open failure stops it, as it stops
   every other purge of the tenant (N-22). Reach: store damage only. Wanted: like a damaged file, it must
   not stop the workspace's own purge; the purge removes its rows and every object no survivor references
   and records that the repository could not be read.
3. Current code: still applies. `purge/mod.rs:303-306`: `if repositories.path(&scope).exists() {
   references::history_recording(&repositories.open(&scope)?)? }`. `open` maps a libgit2 failure through
   `git()` (`git/repo.rs:67-70`). The open-failure counterpart for a survivor is tested
   (`tests/backup.rs:9209`, `rr10_a_survivor_whose_repository_does_not_open_fails_the_purge_naming_it`);
   the target's own is not. This item and section 23 touch the same lines.

## 13. Git errors name no machine path

1. Defined: `lane-storage-report.md:3809-3813` (the implementer's concern);
   `lane-storage-m2-b1r4-rereview.md:488-490`; `progress.md:1087,1097`; plan `:869`.
2. Definition: libgit2 messages passed up through `git()` carry the data-directory path into
   `Unavailable` errors an administrator reads. Remove the data-directory prefix from surfaced Git errors,
   keeping the workspace id.
3. Current code: still applies. `git/repo.rs:571-576`:
   `format!("the version store failed: {}", error.message())` with no filtering. Opens that surface it:
   `git/repo.rs:69,213,224`, `catalog.rs:265`, `purge/mod.rs:611`. For scope: `data.rs:246-251`
   (`io_error`) also formats `path.display()` into its `Unavailable`; the carried item names only `git()`.

## 14. One local tenant id

1. Defined: `lane-server-report.md:339-343` (S7); ruling `progress.md:1096` and `:1102` (Batch C
   fc8e52b); plan `:867`; brief `sweep-a-brief.md:76`.
2. Definition: storage and server use the contract's `LOCAL_TENANT_ID` (`"local"`) wherever production
   code names the local tenant; no other literal remains.
3. Current code: no, done for production code. `contract/src/identity.rs:157` (`LOCAL_TENANT_ID`) and
   `:273-276` (`TenantId::local`); `server/src/identity.rs:331` and `server/src/sessions.rs:230` use
   `TenantId::local()`. A search of `crates/*/src` for a `"local"` tenant literal outside test modules
   finds none (the storage matches `backup/mod.rs:848,938,1101,1126`, `backup/restore.rs:1478-1570`,
   `git/mod.rs:770,919,944` and `purge/rows.rs:489-564` are all after each file's `#[cfg(test)]`;
   `server/src/config.rs:115` is the auth mode, not the tenant). Test code in `crates/storage/tests/*.rs`
   and storage unit tests still spells `"local"` (for example `create(&storage, "local", ..)`); the
   item's wording ("production storage code") does not cover those. `crates/core/src/dispatch.rs:845`
   (`"tenant-local"`) is inside the unit tests.

## 15. Purge and backup outcomes reach the caller (N-17)

1. Defined: `lane-storage-m2-b1r4-rereview.md:148-153` (N-17); requests `lane-storage-report.md:3668-3673`;
   `progress.md:1078`; plan `:868`; brief `sweep-a-brief.md:77`; contract ruling 14
   (`contract-batch-2-rulings.md:58-62`).
2. Definition: `rows_left_out` (manifest only) and `unread` (the purge plan only) are storage-side;
   carry both through the core port and the contract's reports, and carry `purge_progress` in the
   installation archive so a replacement restore keeps `unread`.
3. Current code: partly.
   - Built: `rows_left_out`. `contract/src/workspace.rs:214-227` (`RestoreReport.rows_left_out`,
     `BackupReport`), `core/src/storage.rs:870-890` (the port answers it with the object); commits
     19a2c52 and 154c6d7 are on main through PR #36.
   - Superseded: `unread`. Ruling 14 says "No `unread` on the wire" and rebuilds the purge so nothing
     needs it (section 23). No `unread` field exists in `contract/src`. Today `WorkspacePlan.unread`
     (`purge/mod.rs:132-136`) is stored in `purge_progress.plan`, tested at `tests/backup.rs:8523`.
   - Open or moot: `purge_progress` in the installation archive. `backup/archive.rs:452` writes only
     the tenant's `purges` rows, and `backup/snapshot.rs:73` lists `purge_progress` as not archived. If
     `unread` goes, what `purge_progress.plan` would still hold is objects, archives, uploads, the
     copy lists and the report (`purge/mod.rs:110-139`).
   - The plan entry `plan:868` has not been brought in line with ruling 14 (see the cross-cutting note).

## 16. The ledger's terminal "purged" state and the six ledger limits

1. Defined: the state is first named in `pr20-ledger-r2-brief.md:21-22` ("Not in this round ... carried
   to the next storage package") and `progress.md:1162` ("a terminal 'purged' ledger state that `begin`
   refuses with Conflict naming the purge ... and `complete` checking for a recorded purge in its own
   transaction"). The limits are N1 to N4 of `pr20-ledger-review.md:33-36` plus the two carried at
   `progress.md:1162`; the six are listed in plan `:904-911`.
2. The six limits (plan wording, short): (1) a live row completed before records schema 2 records no
   workspace and keeps its response until it expires; (2) a row whose `complete` lands after the purge's
   last pass keeps its response; (3) an emptied row keeps its idempotency key and the request's digest,
   from which a guessed text could be confirmed; (4) a refused retry of an emptied row restarts that row's
   seven days; (5) a pre-migration copy taken later carries the first two with it, and a copy does not
   expire; (6) a `mutations` table missing a column the purge reads is skipped without a report.
3. Current code: still applies, none of it built.
   - `schema.rs:34`: `state ... CHECK (state IN ('leased', 'completed', 'released'))`. No `purged`
     state anywhere in `mutations.rs` (`:137` expires `completed`/`released` rows only).
   - The limits are stated in `purge/rows.rs:39-52` (five bullets; limit 5 is described in the
     pre-migration bullet above them, not as its own bullet), `core/src/mutations.rs:54` ("Two known
     limits") and `storage/AGENTS.md:31` ("Two known limits in the live ledger"). Commit 49bc2ac
     already changed the storage header's count word to five bullets; the other two still say two.
   - N4 (`pr20-ledger-review.md:36`) said no such copy could exist "before a records schema 3". Records
     schema 3 is on main (PR #31, `schema.rs:18,401`), so an upgrade from schema 2 takes a pre-migration
     copy that can carry the rows of limits 1 and 2.
   - Tests: `tests/purge_ledger.rs` (12 tests) pin the current behavior, including the six-day and
     eight-day expiry (49bc2ac).

## 17. The installation archive is tracked before it is kept

1. Defined: `progress.md:1329` (PR #20 last bot pass, thread 1: `backup/mod.rs:326-330` at that time);
   plan `:913`.
2. Definition: `backup_installation` publishes the archive to the object store and then records it, so a
   stop between leaves a blob no `backup_archives`, artifact or pending-put row names, and a retry
   publishes another. The workspace path records a pending put first; the tenant path must do the same,
   or reconcile the staged digest on retry. Test: stop after the publish, retry, find one archive and no
   unnamed blob.
3. Current code: still applies. `backup/mod.rs:292-339` (`installation_archive`): `put_for` at `:335`
   (no pin, no pending put), the staging directory is removed, then `record(...)` at `:338`. The
   workspace path uses `put_pinned` (`backup/mod.rs:265`), which writes the pending put in the same
   transaction as the purge refusal (`blobs.rs:112-127`, `puts::record`). `puts.rs` keys pending puts by
   (tenant, workspace), and `pending_puts.workspace_id` is `NOT NULL` (`schema.rs:381`), so the tenant
   level has no row shape for it today.

## 18. A resumed purge's counts

1. Defined: `progress.md:1329` (thread 2: `purge/mod.rs:664-675` at that time); plan `:914`.
2. Definition (the thread's words): if a purge stops after deleting objects and before it records the
   objects step, the retry finds them absent and counts each as kept because another workspace shares
   it, so the durable record can claim no removals. Keep the counts with the step's progress, or tell
   "already removed by this purge" from "kept because shared". Test: stop mid-step, resume, compare the
   record with what was removed.
3. Current code: partly. The mechanism as worded does not match the code I read.
   - `purge/mod.rs:658-684`: `planned` = `plan.objects` plus the target's pending-put objects that are
     files now; each digest is `removed` unless `surviving.contains(digest)` or the sweep touched it.
     `pins.rs:250-265` (`delete_unless_touched`) ends `delete().map(|_| true)`: it returns true even when
     the file was already absent (`blobs.rs:272-277`, `remove_for` answers `Ok(false)` for NotFound, and
     the caller drops that). So an object in `plan.objects` that attempt 1 deleted is counted removed on
     the resume, not kept.
   - What does under-report: objects only the target's pending puts name (added to `planned` only while
     `is_file()`, `:658-663`) that attempt 1 deleted are not in the resume's `planned`, so they drop out of
     the count; and the counts are written only after the whole loop (`:227-229`, `advance(.., Some(&plan))`),
     so an interrupted attempt keeps none. The code cannot tell "already removed" from "removed now".
   - No test stops inside the objects step and compares counts (`purge_workspace_resumes_by_repeating_after_a_failure`,
     `tests/purge.rs:2119`, stops at the rows step, before any object is deleted).

## 19. The item-filtered log's cost (sweep-1 review N1)

1. Defined: `sweep-1-review.md:118-124` (measurement) and `:204` (N1); carried at `progress.md:1238-1239`
   ("a load-bearing item").
2. Definition: `log` with an item filter walks to the item's second-newest change or the root and, at each
   commit, `changes_item` reads and parses every item file of two trees. Measured 34 s for 200 items and
   301 commits in a debug build, linear in items times commits. The cure is storage's: compare tree
   entries before parsing, stop when it can, or give core the last path. Every item Timeline pays it.
3. Current code: still applies. `git/history.rs:60` calls `changes_item` for each commit;
   `:315-330` calls `read::find` on the commit's tree and its parent's tree; `git/read.rs:87-93` (`find`)
   is `items(repository, tree)?.into_iter().find(..)`, and `items` (`:23-37`) reads and parses every
   item blob. PR #35 added `item_at_path` (`git/read.rs:62-80`), a direct path lookup, but the item filter
   does not use it. `log` also keeps walking after the page fills to learn if more exist
   (`git/history.rs:62-66`, the `more` flag). The consumer that pays this is core's `item_restore`
   (`core/src/application/history.rs:435` (`item_restore`)).

## 20. A 64-hex revision answered Unavailable (sweep-1 N5)

1. Defined: `sweep-1-review.md:72` (probe output) and `:208` (N5); carried at `progress.md:1239`.
2. Definition: `Revision` accepts 40 or 64 lower-case hex (`contract/src/identity.rs:387-391`), but
   storage answers a 64-hex revision `Unavailable` ("unable to parse OID - too long"), which a caller
   would retry. Same for `at` and for a cursor's tip.
3. Current code: still applies. `git/repo.rs:268` (`commit_of`:
   `Oid::from_str(..).map_err(|error| git(&error))`) and `git/mod.rs:707` (`oid`, used by `held_head`)
   map the parse failure through `git()` (`Unavailable`). `backup/archive.rs:553` maps the same parse to
   `invalid`. `Oid::from_str` on 64 hex is rejected by this libgit2 build, per the review's output.

## 21. The Windows case quirk (sweep-1 N11)

1. Defined: `sweep-1-review.md:214` (N11); probe `sweep-1-review-probes.rs.txt:932` (`r11`);
   carried at `progress.md:1239`.
2. Definition: on Windows, creating `notes/new.md` while the tree holds `Notes/index.md` succeeded and
   the file landed at `Notes/new.md`, a quirk of the case-insensitive staging directory.
3. Current code: no, fixed by PR #34 (7b01ed7, "a folder spelled two ways is refused before anything is
   written"; also 4b1e6d4, b511006). `git/edit.rs:209-236` (`refuse_respelled_folder`) refuses a write whose
   folders spell differently from the tree's, as `InvalidInput` naming the tree's spelling; it is called at
   `:408,437,512,577,646,726,1050`. The module header says so (`edit.rs:60-73`). Test:
   `tests/paths.rs:1690` (`a_note_written_into_a_folder_spelled_another_way_is_refused`: `notes/new.md`
   against `Notes/` is refused with "Notes/" and `/path`), and `:1738` for folders.

## 22. A blocked ancestor path in a relink (sweep-1 N6)

1. Defined: `sweep-1-review.md:209` (N6); `sweep-1-report.md:193`; the Codex P2 thread on PR #26
   "answered as the review's known N6 limit" at `progress.md:1367`. The thread text itself is not kept.
2. Definition: core's `taken` (the check that decides whether to compose the three edits of a Rewind
   of a deleted item) lists only the last path's own folder by exact spelling. A collision storage also
   refuses, a file standing where a parent folder of the last path is needed, is not seen, so the store
   answers `Conflict` naming the last path where one `RestoreItem` would have succeeded. The review
   noted plain edits cannot reach it (probe `r10`: a folder outlives its items through `index.md`);
   a whole-workspace Rewind that removes the folder first could. Proposed: the cure belongs where the
   whole tree is known (storage), or `taken` is documented narrower (it is).
3. Current code: still applies, and the code is core's. `core/src/application/history.rs:519-560`
   (`taken`) doc: "narrower than the store's own check of a destination ... also counts a file standing
   where a folder of the path is needed". Storage's check does see it: `git/edit.rs:255-280`
   (`Staged::occupied`, the `folders` arm: "a file holds the key of a folder above it"). No test pins the
   Conflict outcome (r10 is not committed). The relink itself is storage's `relink`/`move_in`
   (`git/edit.rs:867-900`).

## 23. The purge rebuilt by reachability

1. Defined: plan `docs/plans/2026-10-09-construction-plan.md:895` ("The purge removes by reachability");
   `contract-batch-2-rulings.md:58-59` (ruling 14); `contract-batch-2-report.md:145`; owner decision
   2026-10-14. SPEC section 8 is not changed to match.
2. Definition: today a purge reads the target's own history to learn which objects it named, so a file
   of it that cannot be read leaves the objects only that file named stored, and the purge completes.
   The rebuild: remove everything the target owns except what a surviving item still references, never
   reading the target; if a survivor's own records cannot be read, the purge does not complete
   (`PurgeState::Failed`, naming the survivor by identity, never a path or content) and resumes once
   repaired. The builder first states how objects and their references are laid out. Test: a target with
   an unreadable history file whose objects are all gone after the purge.
3. Current code: still applies, not started. `contract/src/purge.rs:70-80` already has
   `PurgeState::Failed` ("repeating the request resumes it"). The purge today is
   `purge/mod.rs` (seven steps, header `:1-70`), and the details follow.

### 23a. Where objects live

- Object store: `<data>/objects/<tenant>/<first two digest characters>/<digest>`, one file per digest,
  through `object_store`'s local filesystem store with fsync (`blobs.rs:1-9`, opened at `:71-80`,
  `data.rs` `objects_path`). Shared inside a tenant, never across tenants, never keyed by workspace.
  There is no per-object owner, reference count or workspace marker; `local_path` (`blobs.rs:256`) and
  `remove_for` (`:272`) are the only path/delete accessors, and "only a purge calls" the delete.
- Not in the object store: Git repositories `<data>/repositories/<tenant>/<workspace>.git`
  (`git/repo.rs:59-63`); upload bytes `<data>/uploads/<upload id>.part` and `.incoming` (`uploads.rs:1`,
  removed by the purge at `purge/mod.rs` `records()`); `<data>/records.sqlite` and `index.sqlite`
  (`data.rs:60-70`); pre-migration backups under `<data>/backups/pre-migration/` (`data.rs:140`);
  staging under `<data>/staging`.
- Archives are objects too: a workspace archive, an installation archive and a written history are zip
  or history files stored in the same tenant store and named by rows (below).

### 23b. How each thing references an object

| Thing | How it references an object | Where |
| --- | --- | --- |
| Note (non-source item) | a Markdown file in Git; names no stored object | `git/item.rs`, `git/read.rs:23-37` |
| Source card | `.okf/sources/<item id>.json` (contract `SourceAppearance`): `object` (the original) and `extraction.digest` (the conversion record), read by JSON pointer, not through the contract type. The item file's header repeats both digests but is not read | `purge/references.rs:1-30,724-806`; `contract/src/source.rs:150-152` |
| Conversion record | itself an object a card names; its `structured[].digest` and `assets[].digest` name more objects. Opened as a record only when a card names it; streamed | `purge/references.rs:34-45,542-600`; `core/src/conversion.rs:257,271` |
| Correction, supplied text | `.okf/corrections/<item>/<digest>.json`, `.okf/supplied/<item>.json` hold their own Markdown; the digest is a key to a record some card names, never an object only they keep | `purge/references.rs:23-28` |
| Draft | a row in `drafts` (`record`, `body`, `properties` text); no object digest (its `content_digest` is a text hash) | `schema.rs:159-169` |
| Upload | `uploads.object_digest` set when `complete` retains the bytes; `consumed_by` job; before completion only the `.part` file | `schema.rs:226-241`; `uploads.rs:340` |
| Workspace archive | `backup_archives.digest` (workspace_id NULL for the installation archive), an `artifacts` row of kind workspace_backup / installation_backup; the zip also holds copies as `objects/<digest>` entries | `schema.rs:360-368`; `backup/archive.rs` |
| Written history | `history_exports.digest` | `schema.rs:252-259` |
| Other rows | `artifacts.digest`, `derived_objects.digest`, `sandbox_capabilities.object`, `job_outputs.digest` (workspace through `jobs`) | `purge/references.rs:819-873` |
| In-flight write | `pending_puts(tenant, workspace, digest)` row (durable) and an in-process pin (`pins.rs`) | `puts.rs`, `blobs.rs:112-127`, `schema.rs:376-384` |

### 23c. What reads a target's history today

- The plan: `purge/mod.rs:296-306` under the workspace lock, `references::history_recording(&repositories.open(&scope)?)`
  (`purge/references.rs:440`), which walks every commit on every reference (`walk` `:618`, `tips` `:603`),
  reading each distinct `.okf/sources` tree once and each appearance blob (`scan` `:724`); damage goes
  to `History::unread` instead of stopping. Then `planned_objects` (`purge/mod.rs:745-775`) adds the
  target's rows (`references::records(.., Rows::Target, ..)`, `:819`) and follows the conversion
  records the cards named (`follow_records` `:542`).
- The revision map: `invalidate_revisions` (`purge/mod.rs:778-805`) writes every commit of the target to
  `revision_map` as invalidated, from `History::revisions`. That list is the commit graph of the target's
  repository, taken during the same walk.
- Survivors, in the objects step: `repositories.others(..)` lists the other repository directories
  (`git/repo.rs:115`); each is opened and read with `history_naming` (`purge/mod.rs:611-618`,
  `references.rs:425`), then its records followed once (`follow_records` `:625`), plus
  `references::records(.., Rows::Survivors, ..)` (`workspace_id IS NOT ?2`, so tenant-level rows with a
  NULL workspace count as survivors), other workspaces' pending puts (`puts::others`) and the sweep's
  pinned digests (`blobs.pins().sweep`, `pins.rs`). A survivor's damage or open failure stops the step
  with `Internal` naming the workspace (`references::refusing_in`, `naming`).
- The same reader is used outside the purge: the archive writer (`backup/archive.rs:687`,
  `references::history`), restore (`backup/restore.rs:333`, `history_from`), and a commit's landing of
  pending puts (`git/mod.rs:522`, `references::appearances`).
- Other target-reading steps: `copies::plan` / `copies::rewrite_archive` read pre-migration copies and
  other workspaces' archives that cite the target (`purge/copies.rs`), and `rows::remove` / `holds`
  read the records schema itself (`purge/rows.rs`).

### 23d. Tests and text that pin today's behavior

- `purge/mod.rs:1-70` (header) and `:15-27`, `:42-52` describe the "plan reads past damage" and
  "survivor damage stops the step" behavior.
- `tests/backup.rs:8454` (`rr9_a_damaged_workspace_purges_itself_and_frees_the_tenants_purges`, asserts
  the plan's `unread` list at `:8523`), `:8310`, `:8376`; `:9182-9230` (`rr10_` survivor damage and
  open-failure tests); `purge/tests.rs` and `tests/purge.rs` (counts at `:663,1753,2174,2689,3433`).

## 24. `discard_upload`'s storage side

1. Defined: `contract-batch-2-report.md:134` (ruling 3 row), `:514-518` ("Stopped rulings"),
   `:534-541` ("Requests" 2), `:551` (concern 2); ruling `progress.md:1666`.
2. Definition: the row `discard_upload` and its request type are built and answer `not_implemented`.
   Storage side to build: remove the slot's row and its part file at once; for a completed slot only
   unpin the object (tenant-shared, content-addressed bytes cannot be deleted per workspace), leaving
   its removal to the reachability sweep; keep a tombstone row (mutation id, `discarded_at`) so a repeat
   under the same mutation answers the stored `Upload` and `get_upload` answers `not_found`. That is a
   records schema step with archive fixtures, and needs an owner sentence on when unreferenced bytes are
   reclaimed. Schema numbering: whichever of M1.4b's seal and this lands second takes the next number.
3. Current code: not built. `contract/src/operations.rs:77` has the row; `grep` for `discard` in
   `core/src/uploads.rs` finds only a doc mention (`:88`), so `UploadStore` has no `discard` method, and
   storage `uploads.rs` has none. `uploads` is `STRICT` with `mutation_id TEXT NOT NULL UNIQUE`
   (`schema.rs:226-241`). The purge removes `.part`/`.incoming` files by upload id
   (`purge/mod.rs` `records()`), a pattern to reuse. Nothing but a purge deletes an object
   (`blobs.rs:266`). Archive paths that read `uploads`: `backup/snapshot.rs:174`,
   `backup/archive.rs:120-125` (`ROW_OBJECTS`), `purge/references.rs:857-870`.

## 25. A test for a Rewind of two notes that traded paths, asserting what the check is told

1. Defined: `storage-file-links-review.md:281-284` (concern 1, the probe `probe_rewind_traded_self_links`
   in `review_probe2.rs`) and `:326` (R1, non-blocking), mutant F1-f at `:317`.
2. Definition: items 10 at `notes/y.md` and 11 at `notes/x.md` each link to themselves (relative, and
   absolute with an anchor), `notes/ref.md` links to both; one commit trades their paths (three
   `MoveItem`s), then one commit rewinds both (`RestoreItem` for each from the first revision) under a
   recording check. At 2884a97 the files return byte for byte and the check is told `[["notes/ref.md"]]`.
   With F1-f (`park(.., "rewind", Some(destination), None)`) the bytes are the same but the check is also
   told `notes/y.md`, so the suite cannot see F1-f. R1: add it as a paths test asserting the bytes and
   the told list `[["notes/ref.md"]]`; correct the Rewind tests' docs (the hops' self-links are invisible
   in the bytes, not in what the check is told).
3. Current code: still applies. The park call is `git/edit.rs:628`
   (`park(staged, item, &current, "rewind", None, None)`), the `None` being the `ends` argument.
   Nearest tests: `tests/restore_item.rs:1091` (`links_to_items_that_traded_paths_follow_each_item_back`:
   two items trade paths, no self-links, no recording check) and `:1529` (two moved, not traded, no
   self-links); `tests/paths.rs:1611` (`self_links_follow_a_rewind_of_a_move`: one mover). Helpers for
   the new test already exist: `Recording` and `edit_check` (`restore_item.rs:1192-1203`), `linking`
   (`:1175`), `self_links_moved` (`paths.rs:1530`). The probe source was not kept in this directory (only
   its description at `:281`).

## 26. Storage's `collision_key` fallback uses `collision_key_of`

1. Defined: ruling 33, `progress.md:1657` (and `contract-batch-2-report.md:164`,
   `contract-batch-2-review.md:76`, which names "git/item.rs:370-383"); the Codex P2 on PR #34 at
   `progress.md:1656`: an NFC/NFD pair inside a segment that is non-portable even in NFC
   (`cafe?.txt` with a precomposed or combining accent) escapes `respelled`'s fallback. Earlier notes:
   `storage-paths-review.md:111` (N10), `storage-paths-report.md:153,275,370` (concern 6).
2. Definition: the contract now has `identity::collision_key_of(raw)`, NFC then lower case with no
   validation, and `WorkspacePath::collision_key` calls it. Storage adopts it in its next package, with a
   test of Codex's pair.
3. Current code: still applies. `contract/src/identity.rs:512-514` (`collision_key_of`, merged in PR #36,
   a4be757). Storage keeps its own two fallbacks: `git/item.rs:426-439` (`collision_key`: a segment that is
   not valid even in NFC is `segment.to_lowercase()` without NFC) and `git/item.rs:443-452` (`nfc`, used by
   `encoded()` at `:268-269` and the wording at `:254`: such a segment is kept as it is). The existing
   NFC/NFD test, `tests/paths.rs:2051-2070`, uses `café.txt`/`cafe\u{301}.txt`, which are valid segments;
   no test uses a segment invalid in NFC. `crates/storage/Cargo.toml` needs no change (the contract already
   depends on `unicode-normalization`).
