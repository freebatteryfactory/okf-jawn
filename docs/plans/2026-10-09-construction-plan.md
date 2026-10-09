# Construction plan: the seven lanes (2026-10-09)

Temporary, like the rest of `docs/plans/`: removed at Stage 1 close. Base: `main` at `de3b5ec`.
The core-ports fixes R1 to R8 (`cure/core-ports-review`, `d94adec`..`d1b0e27`) are treated as
landed before any lane starts (Wave 0).

The integration owner runs construction from this document, locally, three or four lanes at a
time. It fixes execution order and boundaries. It does not redesign anything: the contract and
the core ports are fixed. A lane that finds a wrong shared assumption stops and requests a shared
change (root AGENTS.md: "Request a shared change rather than layering an adapter around a
mistaken contract"). It does not route around it. Gate ids, commands and paths are quoted
exactly. Anything marked [inferred] is this plan's reading, not a quoted rule.

Sources: root `AGENTS.md` (its lane table is generated from `scripts/lib/lanes.mjs`), `SPEC.md`,
`verification.json` (`current.gates.construction`: 26 gates; `current.gates.acceptance`: 10
gates), every lane `AGENTS.md`, `docs/plans/2026-10-08-stage-1b-contract-design.md` sections 9 and 13
(section 9.6 on the fix branch), the core port traits in `crates/core/src/`, and the core-ports
review and its rulings R1 to R8.

## 1. Rules every lane runs under

### 1.1 Worktrees and build environment

- `bun scripts/dev.mjs lanes <lane>...`, run on a clean `main`, creates branch `build/<lane>` at
  `D:\okf\lanes\<lane>` (`lanesParent` in `scripts/lib/lanes.mjs`). Each wave's lanes are created
  from `main` when that wave starts, not all seven at once, so a later lane starts from the
  merged work of the earlier ones. [inferred: `lanes` takes lane names]
- Cargo runs from PowerShell only, with `$env:CARGO_BUILD_JOBS='4'` and a target directory for each
  lane: `$env:CARGO_TARGET_DIR='D:\okf\target-<lane>'`. A shared target directory makes
  parallel builds wait on its lock. Test binaries built from another checkout also keep that
  checkout's `CARGO_MANIFEST_DIR` paths (observed: a qualification test failed with os error 3
  naming another worktree). The UI lanes need no Cargo target. D: has about 190 GB free.
- Ingest tests, and server tests once the server composes ingest, need `DOCLING_RS_MODELS_DIR`
  set to the owner's pinned model copy (2026-10-08 handoff: that local copy is the pinned
  reference).

### 1.2 The lane cycle

1. The lane works one task at a time and commits at every green step: subject `type(scope): what.`,
   body `Why:`, `What changed:`, `Verified:`, `Next:` or `Blocked:`. Three attempts without a
   new diagnosis mean a handoff (root AGENTS.md).
2. Gate: `bun scripts/dev.mjs lane <lane>` writes `.artifacts/lane/<lane>/<sha>.log`, which must end
   `PASS <lane> <sha>`. The lane also runs the receipt command of each construction gate it is
   closing (section 3) and quotes the output under `Verified:`.
3. Independent review, mutation-tested. For each rule a gate names, the reviewer undoes the rule,
   shows the named test going red, then reverts. A finding blocks only when it cites a failing
   command or a named SPEC or AGENTS sentence. At most two review rounds.
4. The integration owner merges with `git merge --no-ff`. The same merge carries the
   integration-owner edits this plan names for it. Where generated outputs disagree, the owner
   runs `bun scripts/dev.mjs gen`. The owner requalifies every receipt whose inputs the merge
   changed (1.5), runs `bun scripts/dev.mjs premerge`, and brings the merge to `main` through a PR
   merged with a merge commit, as #4 to #6 were. The `main` ruleset requires the CI jobs.
5. Lanes still running take `git merge main` at their next task boundary, on a clean tree. On a
   conflict in a generated directory or a lockfile they take `main`'s side, run `gen` and commit.

A lane may merge more than once. Section 2 lists the planned partial merges. After a merge the
lane continues on the same branch. No lane ever reads another lane's unmerged branch: it builds
only against what is on `main`.

### 1.3 Filtered receipts can pass on zero tests

A receipt of the form `cargo test ... -- <filter>` exits 0 when the filter matches no test. That
is not a pass. The log must show at least one test run whose name contains the filter, and the
reviewer checks that those are the tests the gate's meaning describes. There are two traps on
`de3b5ec`:

- `cargo test -p okf-jawn-core -- views::` matches no test today. The tests in
  `crates/core/tests/views.rs` are top-level functions with no `views::` path, and
  `crates/core/src/views.rs` has no test module. So core-cli puts its `view-server-validation`
  tests where `views::` selects them, for example a `mod views` in a test file or unit tests in
  `src/views.rs`.
- `cargo test -p okf-jawn-core -- candidate_check` already matches
  `a_candidate_check_rejects_a_tree_it_cannot_load` (`crates/core/tests/ports.rs:928`). That test
  exercises a test check, not the production `CandidateCheck`.

### 1.4 Gate status during construction

The status of a construction gate is limited to `blocked_on_lanes`
(`tests/foundation/records.test.mjs:272`), and nobody types a status. During construction, a
gate's evidence is its receipt command's output on the merge commit: the lane log plus the
`Verified:` line. After the merge, premerge's `test` step (`cargo test --locked --workspace
--all-features`) and its `ui-test` step re-run every cargo and Vitest receipt on every push. How
a construction gate is recorded as closed is an open owner decision (O4).

### 1.5 Receipts: what makes them stale, and how they are requalified

These are the exact `inputs` of the two committed receipts:

- Docling (`docling-library-qualification`): `qualification/docling`,
  `qualification/docling/criteria.json`, `qualification/lib`, `scripts/lib/provenance.mjs`,
  `scripts/lib/receipt-envelope.mjs`, `tests/fixtures/documents`, `Cargo.toml`, `Cargo.lock`,
  `rust-toolchain.toml`, `.cargo/config.toml`.
- MCP Apps (`mcp-apps-protocol-qualification`): `.bun-version`, `.cargo/config.toml`, `Cargo.lock`,
  `Cargo.toml`, `api/mcp-apps.json`, `api/mcp-tools.json`, `bun.lock`, `bunfig.toml`, `package.json`,
  `qualification/lib`, `qualification/mcp-apps`, `rust-toolchain.toml`, `scripts/lib/provenance.mjs`,
  `scripts/lib/receipt-envelope.mjs`, `tests/fixtures/views`, `ui/index.html`, `ui/package.json`,
  `ui/scripts`, `ui/src`, `ui/tsconfig.json`, `ui/vite.config.ts`.

Consequences:

- **Any Rust dependency change makes both receipts stale.** Both list `Cargo.lock` and `Cargo.toml`,
  and adding a dependency to a crate manifest changes `Cargo.lock`, even for a crate that is
  already locked, so it is never Docling alone.
- `ui/src` changes (including `ui/src/api/generated/`), changes to `api/mcp-apps.json` and
  `api/mcp-tools.json`, `tests/fixtures/views`, `ui/package.json` and `bun.lock` make the MCP Apps
  receipt stale. So does every merge of workspace-ui and of views.
- `tests/fixtures/documents` and `qualification/docling` make the Docling receipt stale.
- A merge of storage, ingest, core-cli, server or mcp-execution makes no receipt stale unless it
  carries a dependency change. [inferred: `crates/` is not a receipt input]

Requalification: on a clean commit, run `bun scripts/dev.mjs qualify docling` and/or
`bun scripts/dev.mjs qualify mcp-apps`. Then run `bun qualification/record.mjs docling mcp-apps`;
when both are stale, both harnesses run first and are recorded together, as `record.mjs`
requires. Commit the receipts and run `bun scripts/dev.mjs check-receipts`. A merge that changes a
receipt input is followed by requalification of that receipt before the next merge to `main`;
premerge ends with `check-receipts`.

Batching: all dependency requests of a wave go into one `bun scripts/dev.mjs lock` commit with one
requalification of both receipts (batches A and B, section 2). Integration-owner edits travel in
the merge they belong to, so that merge needs only one requalification.

### 1.6 Requesting a dependency

A lane never edits a manifest or a lockfile. Its request names the crate or package, the version,
where it is used, and a vendor note. The lane first runs `bun scripts/dev.mjs vendor <lib>`. Today
`vendors.json` has no entry for pulldown-cmark, for a memory-cap facility, or for session or
cookie crates. When there is no entry, the lane runs the Context7 lookup and hands the integration
owner the note's content: official sources, symbols, and the query it executed. The integration
owner records the `vendors.json` entry, changes the manifest, runs `bun scripts/dev.mjs lock`,
commits on `main` inside the wave's batch, and requalifies (1.5). Lanes pick the change up with
`git merge main`. `unsafe_code = "forbid"` applies to authored code, so any OS facility is reached
only through a crate with a safe API.

## 2. Waves

| Wave | Lanes running | Starts when |
| --- | --- | --- |
| 0 | integration owner only | now |
| 1 | storage, ingest, core-cli, views | Wave 0 is on `main` |
| 2 | workspace-ui, mcp-execution, server | each takes the slot of a Wave 1 lane as that lane finishes (at most four lanes at once) |
| 3 | no new lane: assembly | storage, ingest, core-cli and mcp-execution are merged |

### Wave 0: integration owner, before any lane

1. Land R1 to R8: review and merge `cure/core-ports-review`. Decide its two flagged items: the
   `PartialEq` derive added to contract `OutlineEntry` (no wire change), and the placeholder
   `SearchRequest` inside `SearchQuery::check`. Design 9.6 says a shared contract function for that
   rule "is the integration owner's to add". Recommended: accept the derive, and add the shared
   function now, before core-cli builds search.
2. Add the `Dataset` form of the views dataset fixture as a new file beside
   `tests/fixtures/views/present-metrics-dataset.json`. That file stays rows of records until the
   views merge (design section 12). This lets the views lane test against a shared `Dataset`
   fixture without editing `tests/fixtures/views` (decision I2). [inferred]
3. Requalify MCP Apps once for steps 1 and 2. R3's regeneration changed `api/mcp-tools.json` and
   `ui/src/api/generated/types.gen.ts`, per the fix report. Docling stays valid.
4. Record decisions I1, I3, I4 and I5 (section 6), so the lanes start from them.
5. Run `bun scripts/dev.mjs lanes storage ingest core-cli views`.

### Wave 1: storage, ingest, core-cli, views (three Rust lanes, one UI lane)

- **storage** is first because "storage's data-format version and migrations land before anything
  stores data", and every real-adapter test of another lane waits for it.
- **ingest** is a long pole: Docling page windows, the child-process memory cap and its
  measurement, location, glyph flagging, and every job handler. Its converter work needs no
  neighbour. Its RecordStore work waits for storage M1.
- **core-cli** is a long pole: 77 operations, built over its own ports with test fakes. Its
  present/resolve producer call waits for views M1.
- **views** is here because PresentView must switch to the generated `Dataset` before
  application-operations calls the dataset producer. It is small and light on CPU.

### Wave 2: workspace-ui, mcp-execution, server

- **workspace-ui** is independent, but four lanes is the cap. It takes views' slot when views
  finishes, and then consumes views' merged components instead of racing them.
- **mcp-execution** is an adapter over `ports::Application` and needs no neighbour. It runs as one
  short lane, and its wire receipt waits for Wave 3.
- **server** takes storage's slot. Its bindings need only core. Its composition is its last task
  by rule: "server composes the real storage and ingest adapters last, after both are merged".
  It also needs core-cli's `ApplicationService` and mcp-execution's handler on `main`. [inferred:
  startup injects into `core::application::ApplicationService`, and the server's `runtime`
  feature already depends on `okf-jawn-mcp`]

### Wave 3: assembly (no new lane)

The server composition merge comes first. After it, the receipts that need a running service:
`bun scripts/dev.mjs qualify mcp-wire` (mcp-execution), core-cli's tests of the CLI against a
running server and of the stdio relay (decision I5), the SPEC section 12 crash/restart check
(ingest, decision I1), and then the acceptance gates (section 5).

### Planned merges and requalifications

Each merge happens when its prerequisites are on `main`. Each step that changes a receipt input
is followed by the requalification named on that line before the next merge (1.5).

| Merge | Prerequisite on `main` | Integration-owner edits in the same merge | Requalify |
| --- | --- | --- | --- |
| views M1: PresentView reads `Dataset` | Wave 0 | `present-metrics-dataset.json` becomes the `Dataset` document. `qualification/mcp-apps/lib/views.mjs`, `criteria.mjs` and `tests/foundation/harness.test.mjs` follow it. The separate Wave 0 file goes. | MCP Apps |
| Batch A (`lock`): ingest's memory-cap crate(s) and conversion bin target (I4), core-cli's pulldown-cmark, any storage request | requests in hand, ideally by the first task boundary of each lane | the `vendors.json` entries, manifests, `Cargo.lock`, `gen` | Docling and MCP Apps |
| storage M1: format, migrations, lock, all stores, index | - | - | none (no dependency carried) |
| ingest M1: converter, handlers | Batch A | - | none |
| views M2: per-chart failure isolation, saved views | views M1 | `tests/foundation/records.test.mjs:227` and the `authored-typescript-seams` `covers` sentence (and its evidence list, if a test file changes) follow the changed `ui/tests/unit/layout.test.tsx` | MCP Apps |
| core-cli M1: operations; present/resolve once views M1 is on `main`; item outlines | views M1, Batch A | - | none |
| storage M2: Purger, Backups | storage M1 | - | none |
| Batch B (`lock`): server session/cookie crates and any token-validation crate; the cli dev-dependency of I5 | vendor notes recorded | as Batch A | Docling and MCP Apps |
| mcp-execution | - | - | none |
| server M1: bindings | Batch B | - | none |
| workspace-ui (one or more merges) | views M2 | - | MCP Apps, after each |
| server M2: composition and binary | storage M2, ingest M1, core-cli M1, mcp-execution, server M1 | `deploy/` follows the binaries (I4) | none, unless it carries a dependency |
| core-cli M2: CLI against a running server, stdio relay | server M2, Batch B | - | none |

## 3. The lanes

Each lane section gives (1) its directories, gate command and expected receipt, quoted from the
root `AGENTS.md` table; (2) its construction gates, each with its receipt command and what makes
it pass; (3) what it consumes and what it must not depend on; (4) the dependency changes it is
expected to need; (5) its own obligations and task order.

### 3.1 storage

1. Directories: `crates/storage/` except `crates/storage/Cargo.toml`. Gate:
   `bun scripts/dev.mjs lane storage`. Receipt: `.artifacts/lane/storage/<sha>.log` ending
   `PASS storage <sha>`: store and index tests pass; rebuild never touches jobs, reviews or receipts.
2. Gates (owner `storage`):
   - `storage-git-cas-sqlite`: `cargo test -p okf-jawn-storage --features runtime`. Passes when
     these are tested against real Git, SQLite and the local object store: VersionStore (trailer,
     `find_commit`, commit idempotent on MutationId, candidates), BlobStore, WorkspaceCatalog, the
     SQLite stores (Record, Mutation, Draft, Confirmation, SandboxCapability, Proposal, Upload,
     Event, Credential), SearchIndex (no drafts) and local AccessControl.
   - `mutation-crash-reconcile`: `cargo test -p okf-jawn-storage --features runtime -- mutation_crash_reconcile`.
     The commit succeeds and `complete` is never written. The retry gets the first revision back,
     and no second commit exists.
   - `mutation-lease-compare-and-set`: `cargo test -p okf-jawn-storage --features runtime -- mutation_lease_compare_and_set`.
     A superseded attempt can neither complete nor release the mutation, in both takeover orders.
   - `confirmation-single-use`: `cargo test -p okf-jawn-storage --features runtime -- confirmation_single_use`.
     A confirmation cannot be consumed twice, or for a different revision, digest, action or target.
   - `draft-per-editor`: `cargo test -p okf-jawn-storage --features runtime -- draft_per_editor`.
     Two editors get independent drafts, and the second Snapshot conflicts after the first.
   - `storage-single-writer-lock`: `cargo test -p okf-jawn-storage --features runtime -- single_writer`.
     An exclusive lock on a file in the data directory (std `File::try_lock`) is taken before any
     writable store opens and held for the process's life. A second process is refused with a
     clear error.
   - `storage-format-version-and-migrations`: `cargo test -p okf-jawn-storage --features runtime -- migrations`.
     `rusqlite_migration` (`to_latest`, `pending_migrations`) handles the schemas, and
     application-owned formats carry a version. Data newer than the build understands is refused
     and left unchanged. Before an upgrade: a recoverable backup, then the migration, then
     verification of the result.
   - `storage-purge-workspace`: `cargo test -p okf-jawn-storage --features runtime -- purge`.
     Workspace purge removes originals, derivatives, index entries, history and managed backups,
     and marks what referred to them as invalidated. It never claims to erase copies outside the
     application. Archive stays reversible. Item purge follows with the same guarantees.
3. Consumes: the port traits in the `crates/storage/AGENTS.md` table, plus `Purger` and `Backups`
   (design 9.3; `Backups::open_installation_archive`, design 9.6) and the `RecordStore` additions
   of design 9.2 (`JobScope`, the lease-renewing `update_progress`, purge records, derived
   objects, the revision map, `UploadStore::consume`). Also core's helpers
   (`core::credentials::secret_hash`, `core::sandbox::token_hash`,
   `JobSpec::to_stored`/`from_stored`, `stored::decode_stored`), contract types, and the
   `CandidateCheck` it is handed on each call.
   Must not: depend on the ingest, server, mcp or cli crates; take selectors, identities or wire
   requests; decide OKF conformance (that is the check's job, and the check belongs to core);
   store a plaintext secret or token; return the `JobSpec`, initiator or `MutationId` of a claim
   altered.
4. Dependencies: the runtime set is already declared (`git2`, `rusqlite`, `rusqlite_migration`,
   `object_store`, `okf-core`, `sha2`, `bytes`, `tokio`, `serde_json`, `yaml_serde`, `cap-std`,
   `directories`). `File::try_lock` is std (toolchain 1.99.0), and `VACUUM INTO` is plain SQL
   through `rusqlite`. [inferred] The workspace archive writer may need `zip` (already locked,
   for ingest), and FTS5 is expected from `rusqlite`'s `bundled` build. The lane confirms both.
   Anything missing goes into Batch A, or Batch B if found later.
5. Obligations, in order:
   - **First task, before any store writes:** the data-format version and migrations, then the
     single-writer lock, then removal at startup of whatever a crash left under `<data>/staging`.
   - `EventLog::append` to `EventScope::Workspace` for a workspace the tenant does not hold (never
     created, or purged) records the event at tenant level and never creates or revives rows for
     that workspace (`crates/core/src/events.rs`, R1). Test both cases.
   - A repeated `MutationId` is a no-op that returns the prior row, enforced with a unique column
     for every store `crates/storage/AGENTS.md` lists. The mutation lease is a compare-and-set on
     its token, and rows are kept 7 days. Record the chosen lease length in
     `crates/storage/AGENTS.md` (this is the lane's own file).
   - VersionStore: stage, check, commit, and replay through the `Okf-Jawn-Mutation:` trailer. It
     refuses path collisions and application-header changes made through Create, Edit,
     `set_type` or a draft; the server's own header updates are allowed (design 9.3, 13).
   - SearchIndex reads committed trees only. It stores each source's `ExtractionStatus` and each
     item's archived flag. Rebuilding it never touches RecordStore.
   - Purger: workspace purge first, then item purge, with the revision map. Backups: the
     per-workspace and installation archives, `restore_import`, and the offline, pre-migration
     and replacement-restore functions that the server command and startup call (design 9.3).
   - Merges: M1 is everything except Purger and Backups; M2 adds Purger and Backups.

### 3.2 ingest

1. Directories: `crates/ingest/` except `crates/ingest/Cargo.toml`. Gate:
   `bun scripts/dev.mjs lane ingest`. Receipt: `.artifacts/lane/ingest/<sha>.log` ending
   `PASS ingest <sha>`: converter and job-handler tests pass; crash/restart against the real
   RecordStore is a separate construction receipt.
2. Gates (owner `ingest`):
   - `converter-worker-memory-ceiling`: `cargo test -p okf-jawn-ingest --features runtime`.
     Conversion runs in a child process with a hard memory cap, in page windows. The cap and the
     window size are set from a measurement of whole-document against windowed conversion. The
     bound is not `DOCLING_RS_MAX_MEMORY_MB`.
   - `job-crash-against-record-store`: `.artifacts/qualification/ or construction receipt for import restart`.
     This is the only one of the 26 receipts that is not a command (decision I1). It passes when a
     kill and restart against the real RecordStore with `JobHandler(&ClaimedJob)` leaves no
     duplicate effect, expired leases are returned and pending jobs re-enqueued (`expire_leases`,
     `pending_jobs`), and an attempt that dies without a restart is retried after its lease expires.
   - `ingest-locates-unlocated-items`: `cargo test -p okf-jawn-ingest --features runtime -- locates_unlocated_items`.
     The rule of `qualification/docling/src/locate.rs` applies. An item it does not find stays
     unlocated, is never given a guessed box, and every location records its source. Decision O1
     comes first.
   - `ingest-flags-undecodable-text`: `cargo test -p okf-jawn-ingest --features runtime -- flags_undecodable_text`.
     The rule of `qualification/docling/src/glyphs.rs` applies. Flagged text is never indexed as
     words, and its page is reported as partly extracted.
3. Consumes: the `Converter`, `JobHandler` and `JobQueue` traits; `ClaimedJob` and every `JobSpec`
   kind; `ConversionRecord` (including R3's `outline`), `WindowCoverage` and `ConverterLimits`.
   Also the storage port traits named under "Ports to call" in `crates/ingest/AGENTS.md`, as
   trait objects, plus `Purger`, `Backups` and `SearchIndex::rebuild` for the jobs that are not
   imports (I3); `core::storage::derive_item_id`; the `CandidateCheck` that composition injects
   (core-cli's production check); and `generated/converter/packages.json` through `include_str!`.
   It reimplements the rules of `qualification/docling/src/locate.rs` and `glyphs.rs` in the
   crate. [inferred: depending on the qualification crate would be a manifest change]
   Must not: depend on the storage crate in production code (only through core's ports); build an
   OKF conformance policy of its own; add an external job engine or queue service; add Python; or
   enable `asr`, `fetch-images` or `vlm` without a shared change.
4. Dependencies:
   - The memory-cap facility, chosen after a vendor lookup (rlimit, or Windows job objects).
     [inferred] Candidates: on Unix the `rlimit` crate, where the child sets its own limit at start
     (this avoids the unsafe `pre_exec`); on Windows, job objects through a crate with a safe API.
     Container hosts may also bound the process with cgroups. macOS: decision O3.
   - The conversion child's executable (I4). Both go into Batch A.
   - pulldown-cmark is not ingest's. The converter outline comes from Docling's document. [inferred]
5. Obligations, in order:
   - Converter: `page_count`, then one window per child under `limits().memory_limit_bytes`.
     Reaching the cap is `Failure(MemoryLimit)`. A missing model or native library is a worker
     fault. Dropping the future kills the child. Build `DocumentConverter` from
     `ConversionSettings` and the timeout, and open the source with `SourceDocument::from_bytes`,
     using the format of `file_name`.
   - **Memory cap and window size come from a measurement.** It compares whole-document against
     windowed conversion (Markdown, tables and peak memory) and includes a PDF whose tables cross
     pages. [inferred] No fixture in `tests/fixtures/documents/SOURCES.json` is described as
     having a table that crosses pages. The lane adds one under `crates/ingest/` with its source
     and licence, or asks the integration owner to add it to `tests/fixtures/documents` (a Docling
     receipt input, so a requalification follows). Record the chosen `window_pages` and
     `memory_limit_bytes` with the measurement. The handoff reports a peak of about 2.2 GB for the
     18-page PDF with page images on.
   - **`ConversionRecord::outline`** is filled with heading entries that select their section as
     lines: from the heading line to the line before the next heading of the same or a higher
     level, or to the end of the text. Lines refer to the joined Markdown (R3).
     `reading::section_lines` trusts this.
   - The windowing loop in the import and redigest handlers follows design 9.1: windows are
     merged into one record and each `WindowCoverage` is checked. The joined `PageCoverage` is
     checked over `1..=page_count`. A window that hit the cap becomes `not_converted` pages, and
     `unconverted_only` joins the previous record's converted windows with the new ones.
   - The import handler commits each source card as `pending` before it converts, and replaces
     the card afterwards. The card shows the contract `Extraction` in every outcome.
   - Idempotency: every commit uses `CommitChanges { mutation_id: claimed.mutation_id, .. }` and new
     items use `derive_item_id`. `update_progress` is called as a heartbeat at most every third of
     the lease, and the handler stops when the job is cancelled. At startup the worker reconciles
     with `expire_leases()` and `pending_jobs()`.
   - Locating unlocated items and flagging undecodable text follow `crates/ingest/AGENTS.md` and
     both gates. The lane asks for O1 before it builds `ingest-locates-unlocated-items`.
   - Merges: M1 is the converter and the handlers (three gates), tested against fakes of the
     storage ports. The crash receipt follows in Wave 3 (I1).

### 3.3 core-cli

1. Directories: `crates/core/`, `crates/cli/` except `crates/core/Cargo.toml`,
   `crates/cli/Cargo.toml`. Gate: `bun scripts/dev.mjs lane core-cli`. Receipt:
   `.artifacts/lane/core-cli/<sha>.log` ending `PASS core-cli <sha>`: Application operations and the
   CLI command surface.
2. Gates (owner `core-cli`):
   - `core-ports-carry-extraction-and-views`: `cargo test -p okf-jawn-core`. This work landed with
     #6 and R1 to R8 (Wave 0). The lane keeps it green.
   - `application-operations`: `cargo test -p okf-jawn-core -p okf-jawn-cli`. Passes when
     `ApplicationService` implements all 77 operations over `OperationContext` and the handlers
     call the checks the core ports provide: `authorize_job_kind`, `authorize_object`,
     `check_revision`, `cited_locations`/`fill_locations`, `section_lines`,
     `materialize`/`retain_dataset` and the header refusals. Export writes `verified`, and import
     reads it as imported coverage. The CLI is exercised against a running server (I5).
   - `view-server-validation`: `cargo test -p okf-jawn-core -- views::`. `views::validate` checks
     against `catalog.schema.json` and `vega-lite.schema.json` plus referential checks, and
     rejects bindings outside the workspace. Present and saved-view writes call it first. The
     filter needs care (1.3).
   - `draft-visibility`: `cargo test -p okf-jawn-core -- draft_visibility`. `read_item` never
     returns a draft, and `get_item` returns only the caller's own.
   - `candidate-check-runs-validator`: `cargo test -p okf-jawn-core -- candidate_check`. The
     production `CandidateCheck` runs `okf-validator` on the staged candidate, and a refused
     candidate is never committed. The filter needs care (1.3).
   - `cli-mcp-stdio-relay`: `cargo test -p okf-jawn-cli -- mcp_stdio`. `mcp --stdio` forwards each
     message to the running service with a connector credential. It holds no store of its own,
     and it starts the service when none is running.
3. Consumes: the contract (operation table, `for_each_operation!`), `generated/cli/` (generator
   output), core's own ports, `okf-validator`/`okf-core`, pulldown-cmark (Batch A), and views M1
   on `main` before it wires the producer.
   Must not: let core depend on the storage, ingest, server, mcp or cli crates; ship an in-memory
   or fixture store (fakes live in `tests/support` only); let the CLI open storage or carry a
   credential in argv; scan Markdown in core itself ("Core parses no Markdown",
   `crates/core/src/reading.rs:344`).
4. Dependencies: pulldown-cmark, which is already in `Cargo.lock` through docling and is approved
   for item Markdown outlines. It is added once, in Batch A, with any other core-cli request,
   followed by one requalification. [inferred] core-cli is the lane that builds item outlines:
   `reading.rs:344-349` gives the outline of an item's own Markdown to "the selected parser's
   ... (`application-operations`)". No new crate is expected for the relay [inferred: tokio
   `io-std` and `process`, and `reqwest`, are present]. The cli dev-dependency of I5 is in Batch B.
5. Obligations, in order:
   - Handlers in cohesive child modules of `crates/core/src/application/`, using only the injected
     `Ports`. Dispatch is already in place. OKF validation and lint are applied at import, edit,
     refactor and proposal.
   - The production `CandidateCheck` is exported so that startup can hand it to ingest's handler.
     [inferred: ingest commits through `VersionStore::commit(check)`, and core injects no handler]
   - `present_view`/`resolve_view` call `views::materialize` and `retain_dataset` only once views
     M1 is on `main`. Until then the lane builds the other operations. A source whose shown text
     is not the converter's gets `DatasetUnavailable` (R5; decision O2).
   - Item outlines are built with pulldown-cmark and follow the same section-span rule as ingest's
     outline, so `section_lines` behaves the same for both producers.
   - Name tests so each filtered receipt selects the gate's own tests (1.3).
   - Last, after server M2: the CLI against a running server and the stdio relay (I5).
   - Merges: M1 is all operations (with present/resolve once views M1 has landed) and outlines.
     M2 is the CLI against the server and the relay.

### 3.4 server

1. Directories: `crates/server/` except `crates/server/Cargo.toml`. Gate:
   `bun scripts/dev.mjs lane server`. Receipt: `.artifacts/lane/server/<sha>.log` ending
   `PASS server <sha>`: auth, session, sandbox-origin and readiness bindings.
2. Gates (owner `server`):
   - `server-binary-and-startup`: `cargo test -p okf-jawn-server --features runtime`. WorkOS
     AccessControl, session/CSRF, TransportAuth, the sandbox capability origin route,
     ReadinessProbe and startup composition.
   - `http-fallback-errors`: `cargo test -p okf-jawn-server --features runtime -- http_fallback_errors`.
     An unknown path and a wrong method return `ApiError` JSON.
   - `grant-cache-ttl`: `cargo test -p okf-jawn-server --features runtime -- grant_cache_ttl`.
     A revoked WorkOS role is denied within 60 seconds. Approve, Review and Admin are always
     checked fresh, and the local adapter does not cache.
3. Consumes: core's `ApplicationService`, `Ports` and dispatch (core-cli); storage's adapters,
   `ReadinessProbe` and backup/restore functions; ingest's `Converter`, `JobHandler`, `JobQueue`
   and worker; mcp-execution's handler for the MCP route; the `workos` crate; and the UI build
   outputs as static assets. Before composition the lane builds against core with test doubles
   in `tests/`.
   Must not: hold operation behaviour; fall back to a guest owner; treat loopback as trust; fall
   back from hosted to local; echo unbounded client input; or select a session crate before its
   vendor note exists.
4. Dependencies: session/cookie crates, only after a recorded vendor note (the
   `crates/server/AGENTS.md` TODO). [inferred] A JWKS/JWT validation crate too, if the vendor
   lookup shows that `workos` 3.3.0 does not validate Connect tokens. All of this goes into
   Batch B.
5. Obligations, in order:
   - **Only a request authenticated by the browser session cookie may carry
     `AccessRoute::BrowserSession` or `AccessRoute::LocalOwner`.** The CLI, a connector secret and
     every other non-browser client get other routes. Test each entry path's route. The
     mutation to check: giving a connector bearer `LocalOwner` must turn a draft test red.
   - The identity middleware is the only place a `Principal` or `SessionId` is inserted, and it is
     layered outside the router.
   - Local: a single-use launch token at `/auth/local`, an HttpOnly SameSite=Strict cookie,
     Host/Origin/CSRF checks, constrained CORS, and connector-secret bearer auth for MCP. Hosted:
     AuthKit and Connect. A missing or invalid hosted configuration fails startup.
   - Fallback errors, the grant cache, the sandbox origin route, readiness separate from health,
     and graceful shutdown.
   - **Startup composes the real storage and ingest adapters last**, after both are merged, along
     with core-cli's `ApplicationService` and mcp-execution's handler. At that point storage takes
     the single-writer lock before its stores open, the pre-migration backup runs, and the worker
     reconciles.
   - Merges: M1 is the bindings (`http-fallback-errors`, `grant-cache-ttl` and the binding parts of
     `server-binary-and-startup`). M2 is the composition and the binary.

### 3.5 mcp-execution

1. Directories: `crates/mcp/` except `crates/mcp/Cargo.toml`. Gate:
   `bun scripts/dev.mjs lane mcp-execution`. Receipt: `.artifacts/lane/mcp-execution/<sha>.log` ending
   `PASS mcp-execution <sha>`: tool and resource adapters; `bun scripts/dev.mjs qualify mcp-wire`
   against a running endpoint is a separate receipt.
2. Gate (owner `mcp-execution`): `mcp-server-backed-by-application`: `bun scripts/dev.mjs qualify mcp-wire`.
   A production `ServerHandler` over `Application` serves `ui://okf-jawn/app.html` from
   `api/mcp-apps.json`, with no draft-bearing tool. The receipt needs a real endpoint
   (`OKF_TEST_MCP_URL`, optionally `OKF_TEST_AGENT_TOKEN`); `tests/integration/mcp-wire.mjs` uses
   no simulated server. It can run first in Wave 3.
3. Consumes: `ports::Application` (core), the generated `api/mcp-tools.json` and
   `api/mcp-apps.json`, the App bundle built from views' `ui/src/mcp-apps/`, and rmcp 3.5.
   Must not: serve stdio (the CLI relay does that); offer a draft-bearing tool; accept the browser
   session; return a hand-written summary as text; use `CallToolResult::structured_error`; or
   depend on the storage, ingest or server crates.
4. Dependencies: none expected. [inferred: the rmcp features `server`, `macros`, `schemars` and
   `transport-streamable-http-server` are declared]
5. Obligations: results go through `read_result`, `structured_result` and `error_result`, as
   `crates/mcp/AGENTS.md` states. Permissions apply independently of visibility hints. One merge.

### 3.6 workspace-ui

1. Directories: `ui/` except `ui/src/features/views/`, `ui/src/mcp-apps/`, the six views unit
   specs, `ui/package.json` and `ui/biome.json`. Gate: `bun scripts/dev.mjs lane workspace-ui`.
   Receipt: `.artifacts/lane/workspace-ui/<sha>.log` ending `PASS workspace-ui <sha>`: Explorer,
   WebMCP and sandbox-viewer unit results.
2. Gate (owner `workspace-ui`): `workspace-ui-drafts-and-confirmations`: `bun --bun run --cwd ui test`.
   Autosave through `save_draft`, Snapshot with the Conflict and rebase flow (keep mine or take
   theirs, per item, with a diff: design 13 D), and confirmation for review and accept.
3. Consumes: the generated client in `ui/src/api/generated/` (body/path/query groups), views
   components as merged on `main`, and the server's sandbox origin as configuration.
   Must not: edit views' directories or specs, `ui/package.json`, `ui/biome.json` or generated
   output; keep identity in a second store; offer Verify/Approve WebMCP tools.
4. Dependencies: none expected. [inferred: `ui/package.json` already carries the Explorer
   libraries] An addition is a request that changes `ui/package.json` and `bun.lock`, which are
   MCP Apps inputs.
5. Obligations: the complete Explorer (tree, search, tabs, split panes, viewers, editing,
   properties, naming form with YAML and preview, graph, history, proposals, review, Attention),
   WebMCP, and the HTML sandbox viewer on the separate origin. Every merge makes the MCP Apps
   receipt stale.

### 3.7 views

1. Directories: `ui/src/features/views/`, `ui/src/mcp-apps/`, `ui/tests/unit/catalog-schema.test.ts`,
   `ui/tests/unit/json-render-roundtrip.test.tsx`, `ui/tests/unit/layout.test.tsx`,
   `ui/tests/unit/mcp-apps-dispatch.test.tsx`, `ui/tests/unit/mcp-apps-mime.test.ts`,
   `ui/tests/unit/view-document-roundtrip.test.tsx`. Gate: `bun scripts/dev.mjs lane views`.
   Receipt: `.artifacts/lane/views/<sha>.log` ending `PASS views <sha>`: View and MCP Apps unit
   results; the Apps bundle build and host rendering are separate qualifications.
2. Gates (owner `views`):
   - `views-saved-persist-reopen`: `bun --bun run --cwd ui test`. A saved View persists and
     reopens, with server diagnostics as the source of truth.
   - `views-chart-failure-isolation`: `bun scripts/dev.mjs lane views`. A chart that fails to
     render shows its own alert, and the View's text and other charts still render.
3. Consumes: the generated Zod schemas `zDataset` and `zPresentResponse` (`charts`, `as_of`), the
   Wave 0 `Dataset` fixture, json-render, vega/vega-lite/vega-interpreter, and workspace-ui's
   feature components for the MCP Apps only as merged on `main`.
   Must not: depend on a running producer (present/resolve live in application-operations, so
   the lane builds against fixtures of the generated `Dataset`); take numbers an agent wrote;
   install `@json-render/mcp` or a Node MCP server; or edit `tests/fixtures/views` or
   `tests/foundation`.
4. Dependencies: none expected. [inferred]
5. Obligations, in order:
   - **M1:** PresentView and `ui/src/mcp-apps/main.tsx` parse the dataset with the generated
     `zDataset` instead of rows of records (`rowsSchema`, `PresentView.tsx:20-22`, `:108`). The
     integration owner's edits for this merge are in section 2.
   - **M2:** per-chart failure isolation, using `charts`. The lane changes
     `ui/tests/unit/layout.test.tsx` along with the behaviour: its test "refuses a chart
     specification that compile rejects" pinned the opposite. The gate text names that file and
     the records test, which the integration owner changes in the same merge. A
     `DatasetUnavailable` or `TooLarge` chart status is that chart's alert, not the View's. Saved
     views persist and reopen.

## 4. Integration owner

Gate (owner `integration-owner`): `contract-expresses-extraction-sources-views`:
`bun scripts/dev.mjs premerge`. It landed with #4, the Stage 1b contract, and premerge on `main`
keeps it.

Duties, by wave:

- Wave 0: section 2.
- Batches A and B: the vendor notes, manifest edits, `lock`, `gen` and one requalification each.
- The edits that travel in views M1 and M2 (section 2).
- Requalification after every merge that changes a receipt input (1.5).
- Wave 3: the crash/restart harness and its command (I1); updating `deploy/Dockerfile` and
  `deploy/compose.yaml` for the composed binaries; and the acceptance runs (section 5).
- Answering shared-change requests from lanes: symbol, SPEC sentence, proposed change, failing output.

## 5. Acceptance gates: when each becomes runnable

No acceptance gate becomes runnable from Wave 1 or Wave 2 alone. Each one needs either nothing
from the lanes or the composed server.

| Gate | Status | Runnable after | Needs the owner for |
| --- | --- | --- | --- |
| `release-model-inventory` | `blocked_on_product` | now (independent of the lanes); required before any installer or image ships models | where the pinned model bytes the project holds are published (the models-v1 tag was re-published, and the owner's local copy is the reference) |
| `qualify-application` | `blocked_on_product` | Wave 3: `bun scripts/dev.mjs qualify application` against a real disposable deployment | no |
| `index-rebuild` | `blocked_on_product` | Wave 3 | no |
| `saved-view-persist-reopen` | `blocked_on_product` | Wave 3 (views, core-cli, storage, server) | no |
| `explorer-playwright-journeys` | `blocked_on_product` | Wave 3 and the last workspace-ui merge; Chromium and WebKit as two projects of one Playwright config | no |
| `docker-volume-persistence` | `blocked_on_product` | Wave 3 and the integration owner's `deploy/` update | no |
| `scale-reference-workload` | `blocked_on_product` | Wave 3 | [inferred] the owner's acknowledgement of the reference machine, corpus and query set the first measurement fixes, since they become release criteria |
| `mcp-apps-web-hosts` | `not_run` | Wave 3, after `qualify mcp-wire` passes | yes: the account holder's claude.ai/ChatGPT session to register the custom connector, and a public HTTPS endpoint |
| `workos-end-to-end` | `blocked_on_product` | Wave 3 (server hosted mode) | yes: a WorkOS environment (AuthKit, Connect, role assignments) behind GalaxyGate |
| `desktop-launcher-trial` | `blocked_on_product` | after Wave 3 | yes: signing identities for Windows and macOS. Electrobun (or Tauri 2) arrives as an integration-owner dependency. Whether installers ship is decided after the trial |

## 6. Open decisions

### Owner (product behaviour)

- **O1. `ingest-locates-unlocated-items`: does the product bound the distance?** ingest asks before
  it builds the gate. Recommended default: no bound. Every location found through the text layer
  is recorded as `inferred`, and SPEC section 5 says the interface never presents an inferred
  location with the certainty of a direct one. The contract carries no distance, so a bound could
  only drop locations. Revisit if acceptance finds a wrong placement that readers rely on.
- **O2. Charts over approved agent-supplied text, or human-corrected text.** These are refused
  today: `views::materialize` returns `DatasetUnavailable` (R5). Recommended default: keep
  refusing, and show the refusal as that chart's own alert. A dataset is read only from the
  converter's tables, and supplied text is Markdown with no `ConvertedTable`. Allowing it would
  need a table source for that text and a shared change.
- **O3. A memory cap on a platform without a hard facility.** [inferred: on macOS the address-space
  limit is not enforced, which the ingest vendor lookup confirms or refutes] Recommended
  default: conversion refuses to start there. That is a worker fault, and readiness reports it,
  so no unbounded conversion runs. Revisit at `desktop-launcher-trial`. The alternative is to
  sample the child's memory and kill it over the cap, but that is not the hard bound the gate
  names.
- **O4. How a construction gate is recorded as closed.** The status vocabulary is closed
  (`tests/foundation/records.test.mjs:272`), and statuses are never typed. Recommended default: no
  status change during construction; the evidence is the lane log and the merge commit's
  `Verified:` line, and premerge re-runs every cargo and Vitest receipt. At Stage close, the
  integration owner proposes moving each gate whose command premerge runs to the existing `ci`
  kind. That is one change to `verification.json` and its records test.

### Integration owner (engineering, recorded before the lane that needs it)

- **I1. The `job-crash-against-record-store` command.** Its receipt is not a command today. SPEC
  section 12: "submits an import, kills execution immediately after acknowledgement, restarts and
  retries, then opens the same completed result ... nothing short of that proves product crash
  durability". Recommended default: a harness under `tests/integration/` that runs in Wave 3
  against the composed server binary (no second composition). The integration owner writes it and
  puts its exact command into the gate's `receipt`. The ingest lane owns its failures, and its own
  handler tests cover replay against fakes until then.
- **I2. A `Dataset` fixture beside the rows fixture in Wave 0.** Recommended: yes (section 2).
  `ui/tests/unit/mcp-apps-dispatch.test.tsx` and `view-document-roundtrip.test.tsx` (both views
  files) read `tests/fixtures/views`, which views cannot edit. Without the new file, views M1
  cannot test the switch.
- **I3. ingest's handler runs every `JobSpec` kind.** Recommended: yes. Core injects no
  `JobHandler` (`crates/core/src/application/mod.rs`, the `Ports` doc), and ingest owns the
  runtime adapter. So exports, backups, restore, index rebuild, View export and purge
  (`crates/core/src/jobs.rs`, `JobSpec`) run through ingest's handler, which calls the storage
  ports. Otherwise those jobs would never run.
- **I4. The conversion child's executable.** Recommended: a bin target of `okf-jawn-ingest` with
  `required-features = ["runtime"]`, declared in `crates/ingest/Cargo.toml` in Batch A. Tests reach
  it through `CARGO_BIN_EXE_<name>`, the server finds it beside its own executable, and `deploy/`
  and the launcher ship it. The alternative, a worker mode of the server binary, couples server's
  `main` to ingest.
- **I5. How the cli tests reach a running server.** `application-operations` ("CLI against a running
  server") and `cli-mcp-stdio-relay` ("starts the service when none is running") need the
  composed server, and `okf-jawn-cli` depends only on the contract. Recommended: in Batch B,
  `okf-jawn-cli` gets a dev-dependency on `okf-jawn-server` (features `runtime`). The tests start
  the composed service in-process on a loopback port with a temporary data directory, and core-cli
  M2 lands after server M2. This keeps CI's `cargo test --workspace` self-contained.

## 7. Risks by wave

| Wave | Risk | What detects it |
| --- | --- | --- |
| 0 | Landing R1 to R8 regresses a core test or the regenerated outputs | premerge on the merge (`test`, `gen-check`) |
| 0 | The new `Dataset` fixture breaks the MCP Apps harness | the MCP Apps receipt's criteria |
| 1 | Three parallel cargo builds slow a requalification past the Docling timeout probe | the Docling receipt's `timeout_case` and failed criteria; requalify with lane builds paused |
| 1 | A shared target directory leaks another worktree's paths into test binaries | a test failing with os error 3 that names another worktree |
| 1 | A filtered receipt passes on zero tests | the reviewer reads the test count in the log (1.3) |
| 1 | Real storage code shows a core port is wrong | the storage lane's compile or tests; the lane stops and reports the symbol, the SPEC sentence and the output |
| 1 | A window boundary changes what Docling assembles (a heading level, or a table across pages) | ingest's whole-against-windowed measurement test |
| 1 | The only Windows job-object crate needs authored unsafe | clippy (`unsafe_code = "forbid"`) and `xtask source-policy`; the vendor note must name a safe API |
| 1 | core-cli wires the producer before views M1 is on `main` | the merge order in section 2; the views specs and the MCP Apps receipt fail on the shape |
| 1 | views M2's `layout.test.tsx` change breaks `records.test.mjs:227` | premerge `check-offline`; the integration owner's edit travels in the same merge |
| 1 | Item outlines (pulldown-cmark) and converter outlines disagree on section spans | core-cli's `section_lines` tests over both producers |
| 2 | Every UI merge makes MCP Apps stale | `check-receipts` in premerge |
| 2 | A non-browser request carries `BrowserSession` or `LocalOwner` and reaches drafts | server route tests with the mutation in 3.4, and `qualify application` |
| 2 | A session crate lands without a vendor note | Batch B review against `vendors.json` and the `crates/server/AGENTS.md` TODO |
| 2 | workspace-ui needs a views component that is not merged yet | workspace-ui's `typecheck`; take `main` after views M2 |
| 2 | Composition shows an adapter that does not fit `Ports` | `cargo test -p okf-jawn-server --features runtime`; request a shared change, never an adapter shim |
| 3 | Tool shapes or Apps resources fail on the wire | `bun scripts/dev.mjs qualify mcp-wire` |
| 3 | Crash and restart duplicates an occurrence or a commit, or loses an original | the I1 harness |
| 3 | Authorization refuses everything ("Universal refusal is a product failure") | `qualify application`: a drafter proposes but cannot approve, and a reviewer accepts the exact revision |
| 3 | A journey fails only in WebKit | the WebKit project of `explorer-playwright-journeys` |

## Appendix: the 26 construction gates

| Gate | Owner | Receipt | Section | First runnable |
| --- | --- | --- | --- | --- |
| `contract-expresses-extraction-sources-views` | integration-owner | `bun scripts/dev.mjs premerge` | 4 | now (landed with #4) |
| `core-ports-carry-extraction-and-views` | core-cli | `cargo test -p okf-jawn-core` | 3.3 | Wave 0 (#6 and R1 to R8) |
| `storage-git-cas-sqlite` | storage | `cargo test -p okf-jawn-storage --features runtime` | 3.1 | Wave 1, storage M1 |
| `mutation-crash-reconcile` | storage | `cargo test -p okf-jawn-storage --features runtime -- mutation_crash_reconcile` | 3.1 | Wave 1, storage M1 |
| `mutation-lease-compare-and-set` | storage | `cargo test -p okf-jawn-storage --features runtime -- mutation_lease_compare_and_set` | 3.1 | Wave 1, storage M1 |
| `confirmation-single-use` | storage | `cargo test -p okf-jawn-storage --features runtime -- confirmation_single_use` | 3.1 | Wave 1, storage M1 |
| `draft-per-editor` | storage | `cargo test -p okf-jawn-storage --features runtime -- draft_per_editor` | 3.1 | Wave 1, storage M1 |
| `storage-single-writer-lock` | storage | `cargo test -p okf-jawn-storage --features runtime -- single_writer` | 3.1 | Wave 1, storage M1 |
| `storage-format-version-and-migrations` | storage | `cargo test -p okf-jawn-storage --features runtime -- migrations` | 3.1 | Wave 1, storage M1 |
| `storage-purge-workspace` | storage | `cargo test -p okf-jawn-storage --features runtime -- purge` | 3.1 | Wave 1, storage M2 |
| `converter-worker-memory-ceiling` | ingest | `cargo test -p okf-jawn-ingest --features runtime` | 3.2 | Wave 1, after Batch A |
| `job-crash-against-record-store` | ingest | `.artifacts/qualification/ or construction receipt for import restart` | 3.2, I1 | Wave 3 |
| `ingest-locates-unlocated-items` | ingest | `cargo test -p okf-jawn-ingest --features runtime -- locates_unlocated_items` | 3.2, O1 | Wave 1, after O1 |
| `ingest-flags-undecodable-text` | ingest | `cargo test -p okf-jawn-ingest --features runtime -- flags_undecodable_text` | 3.2 | Wave 1 |
| `application-operations` | core-cli | `cargo test -p okf-jawn-core -p okf-jawn-cli` | 3.3, I5 | core parts in Wave 1; complete in Wave 3 |
| `view-server-validation` | core-cli | `cargo test -p okf-jawn-core -- views::` | 3.3, 1.3 | Wave 1 |
| `draft-visibility` | core-cli | `cargo test -p okf-jawn-core -- draft_visibility` | 3.3 | Wave 1 |
| `candidate-check-runs-validator` | core-cli | `cargo test -p okf-jawn-core -- candidate_check` | 3.3, 1.3 | Wave 1 |
| `cli-mcp-stdio-relay` | core-cli | `cargo test -p okf-jawn-cli -- mcp_stdio` | 3.3, I5 | Wave 3 |
| `server-binary-and-startup` | server | `cargo test -p okf-jawn-server --features runtime` | 3.4 | bindings in Wave 2; complete at server M2 |
| `http-fallback-errors` | server | `cargo test -p okf-jawn-server --features runtime -- http_fallback_errors` | 3.4 | Wave 2, server M1 |
| `grant-cache-ttl` | server | `cargo test -p okf-jawn-server --features runtime -- grant_cache_ttl` | 3.4 | Wave 2, server M1 |
| `mcp-server-backed-by-application` | mcp-execution | `bun scripts/dev.mjs qualify mcp-wire` | 3.5 | Wave 3 |
| `workspace-ui-drafts-and-confirmations` | workspace-ui | `bun --bun run --cwd ui test` | 3.6 | Wave 2 |
| `views-saved-persist-reopen` | views | `bun --bun run --cwd ui test` | 3.7 | Wave 1, views M2 |
| `views-chart-failure-isolation` | views | `bun scripts/dev.mjs lane views` | 3.7 | Wave 1, views M2 |
