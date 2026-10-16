# Builder self-check before you return a package

Run this after your lane gate is green and before you write the hand-off. It lists the defect classes that cost the most review fix rounds in core-cli, storage and mcp-execution. Each class had at least two findings in different rounds or packages; 122 findings from 48 review, re-review and brief files are matched (non-blocking notes that the owner later ruled are included). Classes are ordered by count. For every class, put one line in your report: the command you ran or the answer, and its result. "N/A: no such code in this package" is a valid answer. lane-shared.md already requires a failing test per rule; class 1 stays because that rule was broken 20 times, so your report must carry the mutation table (file:line, test that went red).

## 1. Rule with no test that fails without it (20 findings)
Went wrong: a guard, filter, constant, sort or branch was built, and the suite stays green when it is removed or replaced.
Example: lane-storage-m2-review.md, B7: "13 mutants: 1 red, 12 survived." (the fixture had no dataset, job output or survivor record, so the rule never ran).
Self-check: delete or invert each guard you added, one at a time (the condition, the call, the constant, the sort/dedup), run the target tests, and list every one that stays green. Check the fixture holds a row of every kind the rule must handle.

## 2. Rule covers the cases you thought of, not every place or spelling (13 findings)
Went wrong: a refusal, scrub, copy or validator was applied to some entry points, tables, copies or spellings and missed one.
Example: lane-storage-m2-review.md, B1: "A workspace archive known only to `backup_archives` survives the purge."
Self-check: list every entry point that writes or names the guarded type (grep the port methods, tables, operations) and put the guard's file:line beside each; an empty cell is a defect. For a purge, scrub, archive or redaction, plant a unique string in every field of the input, run it, and grep every file and table (the Q5 marker probe in lane-storage-m2-rereview.md). For a string filter, try upper case, percent-encoding, aliases and alternate keywords.

## 3. Retry or resume does not repeat the first attempt (11 findings)
Went wrong: a second call under the same mutation id removed newer work, wrote a second effect, lost the first answer, or keyed itself on something that changes.
Example: lane-core-cli-m1-2-review.md, B1: "A resumed Snapshot can remove a newer draft that was never committed".
Self-check: for each write, inject a failure after each step (commit, index, event, receipt, record) and call again with the same id. Assert one commit, the first attempt's revision and warnings, and that nothing the retry adds on its own (a newer draft, a different object) is removed or landed. Answer in the report: what is each idempotence key derived from, and can that change? (lane-storage-m2-backup2-rereview.md B2: "So the key names a file, not its events.") Confirm the replay path runs the same refusals as the first call.

## 4. Test pins one case, one side or one caller of the rule (10 findings)
Went wrong: the test also passes a different wrong implementation: only canonical names, only sequential calls, only the upper bound, only one of three paths.
Example: lane-mcp-execution-m1-review.md, B2: "Tool-name resolution is pinned only against canonical ids."
Self-check: write the nearest wrong implementation of each rule and run your suite on it: first for last, all for any, the check on one path or caller type only, a wider column or filter, state shared between callers. Test a bound on both sides (limit-1, limit, limit+1; lane-core-cli-m1-2-rereview.md B2 had 3 attempts pinned only from above). For "every path, route or caller", write one test per member, by name.

## 5. The check inspects something other than what the next step uses (13 findings)
Went wrong: a validator, reader or pre-check looks at a different thing from the code that consumes the input (other spelling, other column list, other walk, wrong bytes), or it runs after a write.
Example: lane-storage-m2-pass4-rereview.md, B1: "So the check and the copy read different things." (upper-case column, view, generated column).
Self-check: name the function that consumes the input and make the check call that same function or column list, or run the consumer's walk on a scratch copy. Feed forged input (alternate spelling, `%2F`, unknown field, UTF-16, a blob only history needs) and assert `InvalidInput` and nothing written: zero rows, zero files, head unchanged. Read stored data by named field, never by scanning text for hex (lane-storage-m2-b1r-rereview.md B-1), and never through a `deny_unknown_fields` contract type to read two fields (R3-1).

## 6. A port or contract is missing what the next consumer needs (9 findings)
Went wrong: the package was right alone, but a journey through the real neighbour could not complete: no port method, no wire field, or a doc silent on empty versus NotFound, companion files or replay.
Example: lane-core-cli-m0-review.md, B2: "A port the wave needs is not in M0 (plan 1.7)."
Self-check: for each port method, field or doc you add or touch, grep the other crates for its callers (or the SPEC journey that will call it) and write one line: caller, what it assumes, the doc sentence that says so. Walk each SPEC journey you serve through the real adapters on paper (export with history; restore, then index, then announce; import, then read). A step with no field or method is a shared-change request now, not after review (lane-storage-m2-takemain-rereview.md F1: "Composed, a real restore completes with `revision: None`.").

## 7. A test passes for the wrong reason (8 findings)
Went wrong: the test reads back what it set, holds the condition constant, is decided by a sleep, or no longer reaches the case its name claims.
Example: lane-storage-m2-rereview.md, window 2: the test "holds the pin across the entire purge, so it never exercises a release during step 6".
Self-check: for each new test, name the one line whose removal turns it red, and show it red. A race test waits on a counter or channel, never on a sleep that decides the result, and asserts that the interleaving it names happened. A test named for 25 hours moves the clock. A test of a rule that returns its own input proves nothing.

## 8. A doc or header says more than the code does (8 findings)
Went wrong: a port doc, `//!` header or report states all, never, exactly, atomic or idempotent, and the code or a probe disproves it; or a known limit is left out.
Example: lane-storage-m2-rereview.md, "The proposed `Purger` port doc is not accurate": "'a purge never deletes bytes another workspace is about to reference'. False (Q1, Q2)."
Self-check: list each sentence you wrote or changed in a port doc, header or report that contains all, every, never, only, exactly, atomic, idempotent, at any moment or nothing. Put the test that fails if it is false beside it, or cut the word. State known limits in the port doc the caller reads, not only in a private header (lane-storage-m2-review.md B6).

## 9. An error is swallowed, folded into another, or the wrong code (7 findings)
Went wrong: `Conflict` read as "the head moved", `NotFound` as "no item", a store fault tolerated, or a forged input answered `Conflict`, `Internal` or `NotFound`.
Example: lane-core-cli-m1-2-rereview.md, K1: "A store fault on removal is swallowed, and the Snapshot reports success."
Self-check: grep your diff for `.ok()`, `unwrap_or`, `if let Ok`, `matches!` on error codes, `let _ =`, `map_err(|_|`. For each, name the one error you tolerate and add a test where the port answers a different one. In the report, give the code a caller sees for each refusal: `InvalidInput` for bad or forged input (a newer schema included), `Conflict` naming the cause, `Internal` only for our fault.

## 10. A limit, bound or cost is not checked against the envelope (7 findings)
Went wrong: a cap silently dropped data, a writer accepted what the reader refuses, or work grew with history or item count.
Example: lane-storage-m2-pass4-rereview.md, B3: "A conversion record over 64 MiB silently loses its structured exports and assets".
Self-check: for every limit, loop or read you add, write the bound and compute it at the SPEC.md line 89 envelope (10,000 items per workspace, about 93 pages each). A limit refuses loudly on the writer and the reader and never skips; test limit+1. Say which code is O(items x revisions) or quadratic.

## 11. A check-then-act window, or a guard that dies with the request (6 findings)
Went wrong: another writer, a second call or a dropped future slips between the check and the act.
Example: lane-core-cli-m1-2-rereview.md, B1: "A cancelled build gives up the workspace's gate while storage keeps building".
Self-check: for each check followed by an act, write in one line who else can write between them. Run a loop of concurrent tasks (reviewers used 300 rounds), cancel the future mid-step (`tokio::time::timeout`) and assert the invariant. A guard that must outlive the request lives in a spawned task. Put the check in the same transaction or lock as the act.

## 12. A fake is more forgiving than the real adapter (6 findings)
Went wrong: the fake answered empty, sorted, deduplicated, unpaged, or "no change" when unscripted, so the defect appeared only against real storage or ingest.
Example: lane-core-cli-m1-2-review.md, fakes note: `FakeVersions::diff` "answers an unscripted comparison with no change", which hid B2.
Self-check: for each fake method a rule relies on, copy the port-doc sentence it encodes above it, then make it give the awkward real answer: frontmatter in files, several pages, unsorted or repeated results, NotFound versus empty, a failure on the second call. List in the report each rule tested only against a fake, and the test or storage obligation that covers the real adapter.

## 13. A citation or receipt does not name exactly what was returned (4 findings)
Went wrong: a clamped range was cited as requested, file lines as body lines, or an operation that returns content recorded no receipt.
Example: lane-core-cli-m1-2-fix3-rereview.md, Blocking 1: "Diff citations use file line numbers; blame and reads use body line numbers."
Self-check: for each content-returning operation, one test compares the receipt's sources with the response JSON, using a file with real frontmatter, a clamped range (Lines 1..999 on a 3-line note), the last block of a continued read, and a moved item. List the operations that return content and the receipt test for each (diff, log, links and graph were each missing one).

## Tests that reviewers kept asking for
- One mutant per guard, with the red test named (classes 1 and 4: 30 findings).
- A same-mutation retry after a failure injected at each step, comparing the replay with the first answer.
- A concurrent loop plus a mid-step cancellation.
- Forged input to a restore, import, validator or schema: refused as `InvalidInput`, nothing written.
- A unique-marker scan of every stored byte after a purge, scrub, archive or redaction.
- One test per path, route, caller, table or revision-naming operation, by name.
- Limit-1, limit and limit+1; a bound pinned from both sides.
- Fixtures with real shapes: frontmatter, several pages, non-hex text, a rich workspace round trip with job outputs.
- A port that answers a different error than the one the code tolerates, with the code asserted.
- Receipt sources equal to the response JSON.
