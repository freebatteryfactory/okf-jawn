## Package A: Gates and tooling

- **Branch:** `cure/gates`, cut from `integration/foundation-cure` at `678f919`.
- **Worktree:** `D:\okf\cure\gates` (its own `target\`).
- **Wave:** 1, in parallel with B (harnesses) and C (contract). Neighbours are fixed: do not read
  their branches, do not wait for them.
- **Line numbers** in this section are those of `678f919`. Earlier tasks shift them; locate each
  edit by the quoted anchor text.
- **Shells:** `cargo` only from PowerShell. `bun` and `git` from either. Every command below is
  run from the worktree root.
- **Commits:** the shared format, one commit per task, made only when
  `bun scripts/dev.mjs check-offline` is green. Never amend, rebase or squash.

**Files allowed (exact list)**

| Action | Paths |
| --- | --- |
| Create | `scripts/lib/lanes.mjs`, `scripts/lib/receipts.mjs`, `scripts/lib/gates.mjs`, `scripts/hooks/pre-commit`, `scripts/hooks/pre-push`, `tests/foundation/fixture-repo.mjs`, `tests/foundation/lockfile.test.mjs`, `tests/foundation/lanes.test.mjs`, `tests/foundation/receipts.test.mjs`, `tests/foundation/gates.test.mjs`, `tests/foundation/hooks.test.mjs`, `tests/foundation/ci.test.mjs`, `tests/foundation/records.test.mjs` |
| Modify | `scripts/dev.mjs`, `scripts/lib/process.mjs`, `scripts/lib/init.mjs`, `tests/foundation/policy.test.mjs`, `tests/foundation/vendor.test.mjs`, `tests/foundation/process.test.mjs`, `tests/foundation/init.test.mjs`, `tests/foundation/toolchain.test.mjs`, `tests/integration/acceptance.mjs`, `.github/workflows/ci.yml`, `.github/workflows/qualify.yml`, `.github/workflows/contract.yml`, `.github/CODEOWNERS`, `Cargo.lock`, `crates/core/Cargo.toml`, `crates/storage/Cargo.toml`, `ui/vitest.config.ts`, `README.md`, `AGENTS.md`, `justfile`, `verification.json`, `vendors.json`, `deploy/.env.example` |
| Delete | `lefthook.yml`, `REPOSITORY-TREE.txt` |
| Untrack | `.cursor/plans/dependency_qualification_pass_e1c8326b.plan.md` (`git rm --cached`; the file stays on disk) |

**Must not touch:** anything under `crates/*/src`, `crates/*/tests`, `xtask/src`, `qualification/`,
`ui/src`, `ui/scripts`, `tests/support/`, `tests/fixtures/`, `api/`, `generated/`,
`ui/src/api/generated/`; `scripts/lib/provenance.mjs`, `scripts/lib/generation.mjs`,
`tests/foundation/harness.test.mjs`; root `Cargo.toml`, `package.json`, `ui/package.json`,
`bun.lock`, `SPEC.md`, `deny.toml`, `clippy.toml`, `rustfmt.toml`, `ui/biome.json`,
`docs/plans/**`. Temporary edits to `api/*.json` made by a mutation-proof step are reverted with
`git checkout --` in the same step and never committed.

**SPEC / AGENTS sentences served**

- SPEC §13: "Phase 0 … is complete only after clean-checkout generation, repeat generation with no
  file-set or byte drift, compilation/exercise of representative consumers".
- SPEC §13: "The integration owner controls shared types, operation declarations,
  manifests/lockfiles, generation, deployment and independent acceptance. Seven lanes implement
  complete responsibilities in isolated worktrees".
- SPEC §14: "Ownership is explicit in these documents and enforced locally by isolated worktrees."
- SPEC §14: "Report what was executed separately from what was authored."
- SPEC §5: "Use the pinned Docling Rust implementation through its actual supported API; do not
  replace it merely because a newer release exists."
- AGENTS.md: "Do not inherit a claim of green from file presence; only the recorded results of the
  selected tools count."
- AGENTS.md: "Only `lock` resolves dependencies, and only when deliberately run."
- AGENTS.md: "Do not suppress lints, fake success, … or declare a partial test to be the full
  suite."

**Interfaces this package produces for everyone else**

| Command | Result |
| --- | --- |
| `bun scripts/dev.mjs lanes [name ...]` | creates `build/<lane>` worktrees under the lanes parent (all seven when no name is given) |
| `bun scripts/dev.mjs lanes-reset` | removes clean, empty lane worktrees and branches; refuses otherwise; never forces |
| `bun scripts/dev.mjs lanes-table` | rewrites the generated table in `AGENTS.md` |
| `bun scripts/dev.mjs scope [lane] [--base <ref>]` | exit 1 and a list when a changed path lies outside the lane |
| `bun scripts/dev.mjs lane <name>` | log `.artifacts/lane/<name>/<label>.log`; last line `PASS <name> <label>` or `FAIL <name> <label> <step>` |
| `bun scripts/dev.mjs premerge [--step <id>]` | log `.artifacts/premerge/<label>.log`; last line `PASS premerge <label>` or `FAIL premerge <label> <id,id,...>` |
| `bun scripts/dev.mjs routes` | writes `ui/src/routeTree.gen.ts` without building |
| `bun scripts/dev.mjs check-receipts` | validates `qualification/receipts/*.json` against HEAD and `verification.json` |
| `bun scripts/dev.mjs clean-checkout` | writes `.artifacts/qualification/clean-checkout/receipt.json` |

`<label>` is the full 40-hex HEAD, with the suffix `-dirty` when `git status --porcelain` is not
empty. A verifier accepts only a label without the suffix.

Receipt header this package requires of every file under `qualification/receipts/` (package B
produces them): `{ "git_sha": "<40 lowercase hex>", "inputs": ["<repo-relative path>", ...]`
(at least one)`, "produced_at": "<RFC 3339>" }`.

Clean-checkout receipt: `{ "git_sha", "inputs": ["."], "produced_at", "exit_codes": { "bootstrap",
"gen-check-1", "gen-check-2", "foundation", "check", "test", "check-offline", "audit" },
"git_status_empty" }`.

---

### Task A.1: Policy test reads the generated operation and tool lists

**Files:**
- Modify: `tests/foundation/policy.test.mjs:1-40` (header, parsing, first five tests). Lines 41-69
  are unchanged.
- Test: the same file.

**Interfaces:**
- Consumes: `api/operations.json` — an array of `{ id, path, label, alias, visibility,
  permission, ui, success_status, description }`, `permission` lowercase. `api/mcp-tools.json` —
  `{ tools: [{ name, annotations: { readOnlyHint, destructiveHint }, _meta: { ui: { visibility:
  string[] } } }] }`. `api/schemas/<id>.input.json` and `<id>.output.json`.
- Produces: nothing new.

- [ ] Run the red test and confirm the defect.
  `bun test ./tests/foundation/policy.test.mjs`
  Expected: `(fail) complete operation surface has unique canonical identifiers and paths` with
  `actual: 69, expected: 66`.

- [ ] Replace `tests/foundation/policy.test.mjs` lines 1-40 with:

```js
/** Inspect authored source policy; Rust AST enforcement runs separately through xtask. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { files } from '../../scripts/lib/files.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
// Both files are generator output of crates/contract/src/operations.rs; gen-check proves they match it.
const operations=JSON.parse(await readFile(join(root,'api/operations.json'),'utf8'));
const tools=JSON.parse(await readFile(join(root,'api/mcp-tools.json'),'utf8')).tools;
const byId=new Map(operations.map(o=>[o.id,o]));
const exposed=operations.filter(o=>o.alias);
// Owner decision: a model tool whose name is not in this list needs a new decision, not a new row.
const approvedModelTools=['ls','grep','show','log','diff','blame','sources','links','propose','catalog','present','workspaces'];
const humanOnly=['approve','review','admin'];

test('complete operation surface has unique canonical identifiers and paths',()=>{
 assert.ok(operations.length>0,'api/operations.json is empty');
 assert.equal(new Set(operations.map(o=>o.id)).size,operations.length);
 assert.equal(new Set(operations.map(o=>o.path)).size,operations.length);
 for(const o of operations){
  assert.match(o.id,/^[a-z]+(?:_[a-z]+)*$/);
  assert.match(o.path,/^\/api\/[a-z]+\/[a-z]+(?:-[a-z]+)*$/,o.id);
  assert.ok(o.path.endsWith(`/${o.id.replaceAll('_','-')}`),`${o.id} is served at ${o.path}`);
  assert.ok(o.description.length>0,o.id);
 }
});
test('every operation has generated input and output schemas and no schema is orphaned',async()=>{
 const expected=operations.flatMap(o=>[`${o.id}.input.json`,`${o.id}.output.json`]).sort();
 assert.deepEqual((await readdir(join(root,'api/schemas'))).sort(),expected);
});
test('agent exposure cannot include human approval or verification',()=>{
 assert.equal(new Set(exposed.map(o=>o.alias)).size,exposed.length);
 for(const o of operations){
  if(o.alias)assert.ok(['model','app'].includes(o.visibility),`${o.id} has a tool alias but visibility "${o.visibility}"`);
  else assert.equal(o.visibility,'',`${o.id} has a visibility but no tool alias`);
 }
 for(const o of exposed)assert.ok(['read','propose'].includes(o.permission),`${o.id} is a tool but requires ${o.permission}`);
 for(const o of operations.filter(o=>humanOnly.includes(o.permission)))assert.equal(o.alias,'',`${o.id} requires ${o.permission} and must never be a tool`);
 for(const id of ['create_review','create_confirmation','accept_proposal','decline_proposal'])assert.equal(byId.get(id)?.alias,'',id);
 const model=exposed.filter(o=>o.visibility==='model').map(o=>o.alias);
 assert.ok(model.length>0,'no model tool is declared');
 for(const alias of model)assert.ok(approvedModelTools.includes(alias),`model tool ${alias} is not an approved name`);
 assert.deepEqual(exposed.filter(o=>o.visibility==='app').map(o=>o.id),['get_object']);
});
test('the generated MCP tool list is exactly the exposed operations',()=>{
 assert.deepEqual(tools.map(t=>t.name).sort(),exposed.map(o=>o.alias).sort());
 for(const tool of tools){
  const operation=exposed.find(o=>o.alias===tool.name);
  assert.deepEqual(tool._meta.ui.visibility,operation.visibility==='model'?['model','app']:['app'],tool.name);
  assert.equal(tool.annotations.readOnlyHint,operation.permission==='read',tool.name);
  assert.equal(tool.annotations.destructiveHint,false,tool.name);
 }
});
test('saved views, human naming UX, and full reading surfaces remain declared',()=>{
 for(const id of ['read_item','preview_names','apply_names','get_graph','get_view','present_view','resolve_view','export_view','create_review','accept_proposal','backup_workspace','restore_workspace','get_object'])assert.ok(byId.has(id),id);
});
test('connector credentials are owner-administered and never agent tools',()=>{
 for(const id of ['create_connector','list_connectors','revoke_connector']){
  const o=byId.get(id);assert.ok(o,id);assert.equal(o.permission,'admin',id);assert.equal(o.alias,'',id);assert.equal(o.visibility,'',id);
 }
});
```

- [ ] Run it.
  `bun test ./tests/foundation/policy.test.mjs`
  Expected: ` 11 pass`, ` 0 fail`.

- [ ] Mutation proof (the guard still bites). Expose a review operation, run, restore:

```sh
bun -e "const f='api/operations.json';const o=JSON.parse(await Bun.file(f).text());const r=o.find(x=>x.id==='create_review');r.alias='verify';r.visibility='model';await Bun.write(f,JSON.stringify(o,null,2))"
bun test ./tests/foundation/policy.test.mjs
git checkout -- api/operations.json
git status --porcelain api/
```
  Expected from the test run: `(fail) agent exposure cannot include human approval or verification`
  with `create_review is a tool but requires review`. Expected from the last command: no output.

- [ ] Commit.
  `git add tests/foundation/policy.test.mjs`

```text
fix(gates): derive the policy test from the generated operation list.

Why: CI run 37352882339 is red because policy.test.mjs asserted 66 operations (there are 69) and regex-parsed operations.rs, which package C is about to widen to 13 columns.
What changed: the test reads api/operations.json, api/mcp-tools.json and api/schemas/; counts are derived. Kept: unique ids and paths, no tool above read/propose, review/approve/admin operations never tools, connector operations admin-only. New: tool list equals exposed operations, model tool names limited to the approved twelve, schema pair per operation.
Verified: bun test ./tests/foundation/policy.test.mjs -> 11 pass, 0 fail; exposing create_review as a model tool fails the test.
Next: Task A.2, vendor lookup counts.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.2: Vendor index counts its recorded Context7 lookups from the entries

**Files:**
- Modify: `tests/foundation/vendor.test.mjs:17-29`; `vendors.json:5`; `vendors.json:9-12`.
- Test: `tests/foundation/vendor.test.mjs`.

**Interfaces:**
- Consumes: `vendors.json` — `{ context7_queries_executed: number, vendors: [{ name,
  context7_library_id: string|null, context7_query: string|null, knowledge_source, use_sites,
  planned_use_sites?, generated_sites?, official_sources, offline_notes }] }`.
- Produces: the invariant `context7_queries_executed === vendors.filter(v =>
  v.context7_library_id !== null).length`.

- [ ] Replace `tests/foundation/vendor.test.mjs` lines 17-29 (the first `test(...)`) with:

```js
test('recorded Context7 lookups are counted from the entries, never from memory',async()=>{
 const data=JSON.parse(await readFile(new URL('vendors.json',root),'utf8'));
 const recorded=data.vendors.filter(v=>v.context7_library_id!==null);
 assert.equal(data.context7_queries_executed,recorded.length,'context7_queries_executed must equal the entries that record a library id and its query');
 assert.equal(new Set(data.vendors.map(v=>v.name)).size,data.vendors.length,'vendor names must be unique');
 for(const v of data.vendors){
  if(v.context7_library_id===null)assert.equal(v.context7_query,null,`${v.name} records a query without a library id`);
  else{
   assert.match(v.context7_library_id,/^\/[^/\s]+\/\S+$/,`${v.name} library id`);
   assert.ok(typeof v.context7_query==='string'&&v.context7_query.length>0,`${v.name} records a library id without its query`);
   assert.match(v.knowledge_source,/Context7/,`${v.name} knowledge_source must name the lookup`);
  }
  assert.ok(v.official_sources.length);assert.ok(v.offline_notes.length);
  const planned=v.planned_use_sites??[],generated=v.generated_sites??[];
  assert.ok(v.use_sites.length+planned.length+generated.length,`${v.name} has no site`);
  for(const site of v.use_sites)assert.ok(await containsFile(site),`${v.name} existing use site ${site} has no file; list it as planned`);
  for(const site of [...planned,...generated])assert.ok(!v.use_sites.includes(site),`${v.name} lists ${site} as both existing and not`);
  for(const site of generated)assert.ok(generatedRoots.some(g=>site.startsWith(g)),`${v.name} generated site ${site} is not a generated directory`);
 }
});
```

- [ ] Run it.
  `bun test ./tests/foundation/vendor.test.mjs`
  Expected: `(fail) recorded Context7 lookups are counted from the entries, never from memory`
  with `7 !== 6`.

- [ ] Correct the record (mechanical edits):
  - `vendors.json:5` → `  "context7_queries_executed": 6,`
  - `vendors.json:9-12` → 
    ```json
          "use_sites": [
            "xtask/src/api.rs"
          ],
    ```
  Check the second edit against the source: `git grep -n utoipa -- crates` prints nothing.

- [ ] Run it.
  `bun test ./tests/foundation/vendor.test.mjs` → ` 3 pass`, ` 0 fail`.
  `bun scripts/dev.mjs check-offline` → ` 0 fail` (both tests that were red at `678f919` are now
  green).

- [ ] Commit.
  `git add tests/foundation/vendor.test.mjs vendors.json`

```text
fix(gates): count recorded Context7 lookups from the vendor entries.

Why: CI run 37352882339 is red because vendor.test.mjs asserted exactly 3 lookups; the file said 7 while 6 entries record a library id and query, against its own scope sentence ("counts the lookups whose library and query are recorded here"). The seventh was the earlier schemars query that 50113b9 overwrote.
What changed: the test derives the count from the entries and requires id and query to be recorded together; context7_queries_executed is 6; utoipa no longer lists crates/contract/src as a use site (no crate under crates/ depends on it).
Verified: bun test ./tests/foundation/vendor.test.mjs -> 3 pass, 0 fail; bun scripts/dev.mjs check-offline -> 0 fail.
Next: Task A.3, restore the Docling pin.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.3: Restore the Docling sibling crates to 1.93.6

**Files:**
- Create: `tests/foundation/lockfile.test.mjs`.
- Modify: `Cargo.lock` (the `docling-core`, `docling-onnx`, `docling-pdf` blocks at :1254-1291).
- Test: `tests/foundation/lockfile.test.mjs`.

**Interfaces:**
- Consumes: `Cargo.lock`; `Cargo.toml:34` (`docling = { version = "=1.93.5", ... }`);
  `vendors.json` docling `offline_notes` ("currently resolves docling-core 1.93.6");
  `verification.json` `current.direct_versions.docling` ("owns docling-core 1.93.6").
- Produces: `packages(lock) -> [{ name, version, dependencies: string[] }]` (test-local helper,
  reused by Task A.15).

- [ ] Write `tests/foundation/lockfile.test.mjs`:

```js
/** Cargo.lock agrees with the pins the records state; resolution is a deliberate act. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');

/** Every `[[package]]` of a Cargo.lock as {name, version, dependencies}. */
function packages(lock){
 return lock.split(/\r?\n\[\[package\]\]\r?\n/).slice(1).map(block=>({
  name:/^name = "([^"]+)"/m.exec(block)?.[1],
  version:/^version = "([^"]+)"/m.exec(block)?.[1],
  dependencies:[...(/^dependencies = \[\r?\n([\s\S]*?)^\]/m.exec(block)?.[1].matchAll(/"([^"]+)"/g)??[])].map(m=>m[1]),
 }));
}

test('Docling sibling crates stay on the release the records name',async()=>{
 const locked=packages(await read('Cargo.lock'));
 const versions=['docling-core','docling-onnx','docling-pdf'].map(name=>{
  const found=locked.filter(p=>p.name===name);
  assert.equal(found.length,1,`${name} must be locked exactly once`);
  return found[0].version;
 });
 assert.equal(new Set(versions).size,1,`docling-core, docling-onnx and docling-pdf disagree: ${versions.join(', ')}`);
 const core=versions[0];
 const docling=JSON.parse(await read('vendors.json')).vendors.find(v=>v.name==='docling');
 assert.ok(docling.offline_notes.some(note=>note.includes(`docling-core ${core}`)),`Cargo.lock has docling-core ${core}; vendors.json does not name it`);
 assert.ok(JSON.parse(await read('verification.json')).current.direct_versions.docling.includes(`docling-core ${core}`),`Cargo.lock has docling-core ${core}; verification.json does not name it`);
 const pinned=/^docling = \{ version = "=([^"]+)"/m.exec(await read('Cargo.toml'))?.[1];
 assert.equal(locked.find(p=>p.name==='docling')?.version,pinned,'Cargo.lock docling differs from the Cargo.toml pin');
});
```

- [ ] Run it.
  `bun test ./tests/foundation/lockfile.test.mjs`
  Expected: `(fail)` with `Cargo.lock has docling-core 1.96.1; vendors.json does not name it`.

- [ ] Restore the pin. PowerShell, in `D:\okf\cure\gates`:

```powershell
cargo update docling-core docling-onnx docling-pdf --precise 1.93.6
```
  Expected output names three downgrades (`Downgrading docling-core v1.96.1 -> v1.93.6`, and the
  same for `docling-onnx` and `docling-pdf`) and nothing else. If cargo refuses the command, stop
  and report its output to the orchestrator; do not run `cargo update` without a package name and
  do not hand-edit `Cargo.lock`.

- [ ] Prove only the three Docling packages changed. PowerShell:

```powershell
git diff --stat -- Cargo.lock
git diff -U1 -- Cargo.lock | Select-String -Pattern '^ name = '
git diff -U0 -- Cargo.lock | Select-String -Pattern '^\+(version|checksum)'
cargo metadata --locked --format-version 1 | Out-Null; $LASTEXITCODE
```
  Expected, in order:
  - ` Cargo.lock | 12 ++++++------` and ` 1 file changed, 6 insertions(+), 6 deletions(-)`;
  - exactly three lines: ` name = "docling-core"`, ` name = "docling-onnx"`,
    ` name = "docling-pdf"`;
  - exactly these six lines (the checksums recorded at `b13fcd0`):
    ```text
    +version = "1.93.6"
    +checksum = "7a9612b6016fa242f6b9c0075ff6f74688a9190bd62f7870ee62181f7f0280f1"
    +version = "1.93.6"
    +checksum = "e05e93f30284836ff159d7870bf8d45b1ef26ea4df5f7869bcb07fe22d15de13"
    +version = "1.93.6"
    +checksum = "61cfc0d963a53cc989cdd5c427161d2d8eaa5e809cdeb82d8524c003428ead9a"
    ```
  - `0`.

- [ ] Run it.
  `bun test ./tests/foundation/lockfile.test.mjs` → ` 1 pass`, ` 0 fail`.

- [ ] Commit.
  `git add Cargo.lock tests/foundation/lockfile.test.mjs`

```text
fix(lock): restore docling-core, docling-onnx and docling-pdf to 1.93.6.

Why: 50113b9 ran `cargo generate-lockfile`, which moved the three Docling sibling crates from 1.93.6 to 1.96.1 under an unchanged docling =1.93.5 pin. SPEC section 5: "do not replace it merely because a newer release exists."
What changed: Cargo.lock holds the three crates at 1.93.6 with the checksums recorded at b13fcd0. A new offline test fails when the lock, vendors.json and verification.json disagree about the docling-core release or when the siblings diverge.
Verified: git diff --stat -- Cargo.lock -> 6 insertions, 6 deletions, all in the three docling blocks; cargo metadata --locked -> exit 0; bun test ./tests/foundation/lockfile.test.mjs -> 1 pass.
Next: Task A.4, make `lock` a minimal update so this cannot recur.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.4: `lock` performs a minimal update

**Files:**
- Modify: `scripts/dev.mjs:37-42` (`lock`); `README.md:41`.
- Test: `tests/foundation/lockfile.test.mjs` (append).

**Interfaces:**
- Produces: `bun scripts/dev.mjs lock` runs `cargo update --workspace` then
  `bun install --lockfile-only`. (`cargo update --workspace`: "Attempt to update only packages
  defined in the workspace. Other packages are updated only if they don't already exist in the
  lockfile.")

- [ ] Append to `tests/foundation/lockfile.test.mjs`:

```js
test('the lock task updates minimally and never re-resolves the whole graph',async()=>{
 const source=await read('scripts/dev.mjs');
 assert.doesNotMatch(source,/generate-lockfile/);
 assert.match(source,/run\('cargo', \['update', '--workspace'\]/);
 for(const file of ['README.md','AGENTS.md','justfile'])assert.doesNotMatch(await read(file),/generate-lockfile/,file);
});
```

- [ ] Run it.
  `bun test ./tests/foundation/lockfile.test.mjs`
  Expected: `(fail) the lock task updates minimally and never re-resolves the whole graph`.

- [ ] Replace `scripts/dev.mjs:37-42` with:

```js
async function lock() {
  await prerequisites();
  // Minimal update: workspace members' own entries follow their manifests; every package already
  // in Cargo.lock keeps its version unless a manifest requirement no longer admits it.
  await run('cargo', ['update', '--workspace'], { cwd: root });
  await run(bun(), ['install', '--lockfile-only'], { cwd: root });
  process.stdout.write('Updated Cargo.lock and bun.lock for manifest changes only; already locked versions were kept. Review `git diff Cargo.lock bun.lock` and commit; nothing was installed.\n');
}
```

- [ ] `README.md:41`: replace the sentence
  ``` `lock` is the only task that resolves dependencies: it runs `cargo generate-lockfile` and `bun install --lockfile-only`. ```
  with
  ``` `lock` is the only task that resolves dependencies: it runs `cargo update --workspace`, which rewrites only the workspace members' own entries and keeps every version already locked, and `bun install --lockfile-only`. To move one locked crate on purpose, run `cargo update <crate> --precise <version>` and review the diff. ```

- [ ] Run it.
  `bun test ./tests/foundation/lockfile.test.mjs` → ` 2 pass`, ` 0 fail`.

- [ ] Prove the task is a no-op on a consistent tree. PowerShell:
  `bun scripts/dev.mjs lock; git status --porcelain Cargo.lock bun.lock`
  Expected: the "Updated Cargo.lock and bun.lock" line, then no output from `git status`. If
  either lockfile changed, run `git checkout -- Cargo.lock bun.lock`, stop, and report the diff to
  the orchestrator: a minimal update that moves an unrelated package is a wrong shared assumption,
  not something to commit.

- [ ] Commit.
  `git add scripts/dev.mjs README.md tests/foundation/lockfile.test.mjs`

```text
fix(gates): make the lock task a minimal update.

Why: `lock` ran `cargo generate-lockfile`, which re-resolves every dependency; that is how the Docling siblings drifted. AGENTS.md: "Only lock resolves dependencies, and only when deliberately run."
What changed: `lock` runs `cargo update --workspace` and `bun install --lockfile-only`; README states the behaviour and the explicit `--precise` route for a deliberate move.
Verified: bun test ./tests/foundation/lockfile.test.mjs -> 2 pass; bun scripts/dev.mjs lock leaves Cargo.lock and bun.lock unchanged.
Next: Task A.5, the process runner.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.5: Process runner — cargo PATH and timeout on Windows, tree kill, live tee, hook environment

**Files:**
- Modify: `scripts/lib/process.mjs` (whole file); `scripts/dev.mjs:6` and after `:15`.
- Test: `tests/foundation/process.test.mjs` (append).

**Interfaces — produces (all exported from `scripts/lib/process.mjs`):**

```js
export const DEFAULT_TIMEOUT_MS = 600_000;
export const CARGO_TIMEOUT_MS = 2_700_000;
export const gitLocalEnvironment;                       // readonly string[]
export function defaultTimeout(command);                // number
export function cargoEnvironment(env = process.env, platform = process.platform);          // {} or { <PATH key>: string }
export function childEnvironment(command, env = {}, base = process.env, platform = process.platform);
export function treeKill(pid, platform = process.platform);   // ['taskkill', string[]] | null
export async function run(command, args, options = {});
// options: { cwd, env, capture, tee, timeout, allowFailure }
// tee: (chunk: string, stream: 'stdout' | 'stderr') => void — live output; the child's stdio is piped.
```

- [ ] Append to `tests/foundation/process.test.mjs` (and change its import line 4 to the one
  shown):

```js
import { CARGO_TIMEOUT_MS, DEFAULT_TIMEOUT_MS, cargoEnvironment, childEnvironment, defaultTimeout, gitLocalEnvironment, run, treeKill, version } from '../../scripts/lib/process.mjs';
```

```js
test('cargo gets 45 minutes; every other command keeps ten', () => {
 assert.equal(CARGO_TIMEOUT_MS, 45 * 60 * 1000);
 assert.equal(defaultTimeout('cargo'), CARGO_TIMEOUT_MS);
 assert.equal(defaultTimeout('git'), 10 * 60 * 1000);
 assert.equal(defaultTimeout('C:\\Users\\x\\.bun\\bin\\bun.exe'), DEFAULT_TIMEOUT_MS);
});
test('cargo on Windows never sees a usr\\bin directory, whose link.exe shadows the MSVC linker', () => {
 const path = ['C:\\Program Files\\Git\\usr\\bin', 'C:\\Users\\x\\.cargo\\bin', 'C:\\Program Files\\Git\\mingw64\\bin', 'C:/msys64/usr/bin/', 'C:\\Windows\\System32'].join(';');
 assert.deepEqual(cargoEnvironment({ Path: path, HOME: 'x' }, 'win32'), { Path: 'C:\\Users\\x\\.cargo\\bin;C:\\Program Files\\Git\\mingw64\\bin;C:\\Windows\\System32' });
 assert.deepEqual(cargoEnvironment({ PATH: 'C:\\Git\\usr\\bin;C:\\x' }, 'win32'), { PATH: 'C:\\x' });
 assert.deepEqual(cargoEnvironment({ Path: 'C:\\Users\\x\\.cargo\\bin' }, 'win32'), {});
 assert.deepEqual(cargoEnvironment({ PATH: '/usr/bin:/usr/local/bin' }, 'linux'), {});
 const base = { Path: 'C:\\Git\\usr\\bin;C:\\x' };
 assert.equal(childEnvironment('cargo', { EXTRA: '1' }, base, 'win32').Path, 'C:\\x');
 assert.equal(childEnvironment('cargo', { EXTRA: '1' }, base, 'win32').EXTRA, '1');
 assert.equal(childEnvironment('git', {}, base, 'win32').Path, base.Path);
});
test('a timed-out command ends its whole process tree on Windows', () => {
 assert.deepEqual(treeKill(4242, 'win32'), ['taskkill', ['/PID', '4242', '/T', '/F']]);
 assert.equal(treeKill(4242, 'linux'), null);
});
test('tee receives live output from both streams without capture', async () => {
 const seen = { stdout: '', stderr: '' };
 const result = await run(process.execPath, ['-e', 'process.stdout.write("out"); process.stderr.write("err")'], { tee: (chunk, stream) => { seen[stream] += chunk; } });
 assert.deepEqual(seen, { stdout: 'out', stderr: 'err' });
 assert.equal(result.stdout, '');
});
test('the task entrypoint drops the repository variables Git exports to hooks', async () => {
 for (const name of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'GIT_COMMON_DIR']) assert.ok(gitLocalEnvironment.includes(name), name);
 const { readFile } = await import('node:fs/promises');
 const source = await readFile(new URL('../../scripts/dev.mjs', import.meta.url), 'utf8');
 assert.match(source, /for \(const name of gitLocalEnvironment\) delete process\.env\[name\];/);
 assert.ok(source.indexOf('delete process.env[name]') < source.indexOf('async function doctor'), 'the variables must be dropped before any task runs');
});
```

- [ ] Run it.
  `bun test ./tests/foundation/process.test.mjs`
  Expected: the file fails to load with `Export named 'CARGO_TIMEOUT_MS' not found in module`.

- [ ] Replace `scripts/lib/process.mjs` with:

```js
/** Run argument-vector commands without shell interpolation or swallowed failures. */
import { spawn } from 'node:child_process';

/** Limit for one command unless the caller passes `timeout`. */
export const DEFAULT_TIMEOUT_MS = 600_000;
/** A cold build of the runtime feature graph (Docling, libgit2, SQLite) outlasts ten minutes. */
export const CARGO_TIMEOUT_MS = 2_700_000;
/**
 * Repository-location variables Git exports to hooks. A task that inherited them would aim its
 * own git commands, and those of the disposable repositories the offline tests create, at the
 * repository whose hook is running.
 */
export const gitLocalEnvironment = Object.freeze(['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'GIT_PREFIX',
  'GIT_COMMON_DIR', 'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES', 'GIT_IMPLICIT_WORK_TREE']);

export function defaultTimeout(command) {
  return command === 'cargo' ? CARGO_TIMEOUT_MS : DEFAULT_TIMEOUT_MS;
}

/**
 * PATH override for a cargo child on Windows. Git for Windows and MSYS2 keep a coreutils
 * `link.exe` in `usr\bin`; ahead of the MSVC linker it makes every build script fail to link.
 * Returns `{}` when nothing has to change, so other platforms and clean PATHs are untouched.
 */
export function cargoEnvironment(env = process.env, platform = process.platform) {
  if (platform !== 'win32') return {};
  const key = Object.keys(env).find(name => name.toUpperCase() === 'PATH');
  if (!key) return {};
  const entries = String(env[key]).split(';');
  const kept = entries.filter(entry => !/[\\/]usr[\\/]bin[\\/]?$/i.test(entry.trim()));
  return kept.length === entries.length ? {} : { [key]: kept.join(';') };
}

export function childEnvironment(command, env = {}, base = process.env, platform = process.platform) {
  const merged = { ...base, ...env };
  return command === 'cargo' ? { ...merged, ...cargoEnvironment(merged, platform) } : merged;
}

/**
 * The command that ends a whole process tree, or `null` where the child alone is signalled.
 * On Windows SIGTERM ends only cargo.exe and leaves rustc and link.exe holding `target\` open.
 * POSIX callers that need a group kill own their process group (see qualification harnesses).
 */
export function treeKill(pid, platform = process.platform) {
  return platform === 'win32' ? ['taskkill', ['/PID', String(pid), '/T', '/F']] : null;
}

export async function run(command, args, options = {}) {
  const { cwd, env = {}, capture = false, tee, timeout = defaultTimeout(command), allowFailure = false } = options;
  const piped = capture || typeof tee === 'function';
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, env: childEnvironment(command, env), shell: false,
      stdio: piped ? ['ignore', 'pipe', 'pipe'] : 'inherit' });
    let stdout = ''; let stderr = ''; let timer;
    if (piped) {
      child.stdout.setEncoding('utf8'); child.stderr.setEncoding('utf8');
      child.stdout.on('data', chunk => { if (capture) stdout += chunk; if (tee) tee(chunk, 'stdout'); });
      child.stderr.on('data', chunk => { if (capture) stderr += chunk; if (tee) tee(chunk, 'stderr'); });
    }
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('close', (code, signal) => {
      clearTimeout(timer);
      if (code === 0 || allowFailure) resolve({ code: code ?? 1, stdout, stderr });
      else reject(new Error(`${command} ${args.join(' ')} exited ${code ?? signal}\n${stderr}`));
    });
    timer = setTimeout(() => {
      const kill = child.pid === undefined ? null : treeKill(child.pid);
      if (kill) spawn(kill[0], kill[1], { stdio: 'ignore', windowsHide: true }).once('error', () => child.kill('SIGTERM'));
      else child.kill('SIGTERM');
      reject(new Error(`${command} timed out after ${timeout} ms; no success recorded`));
    }, timeout);
  });
}

export async function version(command, args = ['--version']) {
  try { return (await run(command, args, { capture: true, timeout: 20_000 })).stdout.trim(); }
  catch (error) { return { unavailable: String(error.message) }; }
}
```

- [ ] Mechanical edits in `scripts/dev.mjs`:
  - `:6` → `import { gitLocalEnvironment, run, version } from './lib/process.mjs';`
  - insert after `:15` (`const [task = 'help', ...args] = process.argv.slice(2);`):
    ```js
    // Git exports these to hooks. Every task addresses the checkout that contains this file, and
    // the offline tests create disposable repositories; an inherited GIT_DIR or GIT_INDEX_FILE
    // would aim their git commands at this repository.
    for (const name of gitLocalEnvironment) delete process.env[name];
    ```

- [ ] Run it.
  `bun test ./tests/foundation/process.test.mjs` → ` 9 pass`, ` 0 fail` (the existing
  `timeout rejects the command` still matches `/timed out/`).
  `bun scripts/dev.mjs check-offline` → ` 0 fail`.

- [ ] Commit.
  `git add scripts/lib/process.mjs scripts/dev.mjs tests/foundation/process.test.mjs`

```text
fix(gates): give cargo a clean PATH and 45 minutes, and kill timed-out trees on Windows.

Why: every command had a ten-minute limit and only the direct child was signalled; from Git Bash, Git's usr\bin\link.exe shadowed the MSVC linker (design section 7 rule 15). Git also exports GIT_DIR and GIT_INDEX_FILE to hooks, which would point test fixture repositories at the real one.
What changed: run() strips `...\usr\bin` from PATH for cargo on win32, defaults cargo to 2,700,000 ms, ends the process tree with taskkill on a Windows timeout, and accepts `tee` for live output. dev.mjs deletes Git's repository-location variables before any task. POSIX timeout behaviour is unchanged.
Verified: bun test ./tests/foundation/process.test.mjs -> 9 pass, 0 fail; bun scripts/dev.mjs check-offline -> 0 fail.
Next: Task A.6, the lane table.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.6: One lane table; `lanes`, the AGENTS.md table and CODEOWNERS read it

**Files:**
- Create: `scripts/lib/lanes.mjs`, `tests/foundation/lanes.test.mjs`.
- Modify: `scripts/dev.mjs:3`, `:6-11` (imports), `:80-88` (`lanes`), `:270` (switch);
  `AGENTS.md:13-22`; `.github/CODEOWNERS` (whole file).
- Test: `tests/foundation/lanes.test.mjs`.

**Interfaces — produces (exported from `scripts/lib/lanes.mjs` in this task):**

```js
export const lanes;            // readonly Lane[]
// Lane = { name, branch: `build/${name}`, kind: 'rust' | 'ui',
//          directories: string[]  (repo-relative, each ends with '/'),
//          files: string[]        (single owned files outside `directories`),
//          exclude: string[]      (paths inside the lane it must not change),
//          crates: string[], features: string[] ('<package>/<feature>'),
//          tests: string[]        (Vitest file filters, relative to ui/),
//          testExclude: string[]  (Vitest --exclude globs, relative to ui/),
//          proves: string }
export const LANE_TABLE_BEGIN; export const LANE_TABLE_END;   // marker lines in AGENTS.md
export function laneNamed(name);                               // Lane, or throws `Unknown lane: ...`
export function lanesParent(root, env = process.env, platform = process.platform, present = existsSync);  // string
export function gitIn(cwd);    // (args, { cwd }?) => Promise<{ code, stdout, stderr }>, never rejects on a non-zero exit
export function renderLaneTable();                             // string, no trailing newline
export async function syncLaneTable(root);                     // true when AGENTS.md was rewritten
export async function createLanes(root, { names = [], parent, git } = {});   // string[] of worktree paths
```

Lane parent: `OKF_LANES_DIR` when set and non-empty; else `D:\okf\lanes` on win32 when `D:\`
exists; else `<root>/../okf-jawn-lanes`.

- [ ] Write `tests/foundation/lanes.test.mjs`:

```js
/** One lane table drives worktrees, gates, scope, the AGENTS.md table and CODEOWNERS. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { LANE_TABLE_BEGIN, LANE_TABLE_END, laneNamed, lanes, lanesParent, renderLaneTable } from '../../scripts/lib/lanes.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');

/** Case-sensitive existence, so `/Justfile` cannot stand in for `justfile` on Windows or macOS. */
async function existsExact(relative){
 let directory=root;
 for(const segment of relative.split('/').filter(Boolean)){
  let names;try{names=await readdir(directory);}catch{return false;}
  if(!names.includes(segment))return false;
  directory=join(directory,segment);
 }
 return true;
}

test('the lane table is well formed and matches the crates it gates',async()=>{
 assert.equal(new Set(lanes.map(lane=>lane.name)).size,lanes.length);
 for(const lane of lanes){
  assert.equal(lane.branch,`build/${lane.name}`);
  assert.ok(['rust','ui'].includes(lane.kind),lane.name);
  assert.ok(lane.directories.length>0,lane.name);
  for(const directory of lane.directories){assert.ok(directory.endsWith('/'),directory);assert.ok(await existsExact(directory),`${lane.name}: ${directory} does not exist`);}
  for(const path of [...lane.files,...lane.exclude])assert.ok(await existsExact(path),`${lane.name}: ${path} does not exist`);
  assert.ok(lane.proves.length>0,lane.name);
  if(lane.kind==='ui'){assert.deepEqual(lane.crates,[]);assert.deepEqual(lane.features,[]);continue;}
  assert.ok(lane.crates.length>0,lane.name);
  const manifests=await Promise.all(lane.directories.map(directory=>read(`${directory}Cargo.toml`)));
  for(const crate of lane.crates){
   const manifest=manifests.find(text=>text.includes(`name = "${crate}"`));
   assert.ok(manifest,`${lane.name}: no manifest under its directories declares ${crate}`);
   for(const feature of lane.features.filter(entry=>entry.startsWith(`${crate}/`)))assert.match(manifest,new RegExp(`^${feature.slice(crate.length+1)} = \\[`,'m'),`${crate} declares no feature ${feature}`);
  }
  for(const feature of lane.features)assert.ok(lane.crates.includes(feature.split('/')[0]),`${lane.name}: ${feature} names a crate the lane does not gate`);
  for(const directory of lane.directories)assert.ok(lane.exclude.includes(`${directory}Cargo.toml`),`${lane.name}: ${directory}Cargo.toml is a manifest and must be excluded`);
 }
});
test('what the views lane owns is excluded from workspace-ui, and every unit spec runs in exactly one UI lane',async()=>{
 const views=laneNamed('views'),workspace=laneNamed('workspace-ui');
 for(const path of [...views.directories,...views.files])assert.ok(workspace.exclude.includes(path),`workspace-ui does not exclude ${path}`);
 const specs=(await readdir(join(root,'ui/tests/unit'))).filter(name=>/\.test\.tsx?$/.test(name)).map(name=>`tests/unit/${name}`);
 for(const spec of specs)assert.equal(views.tests.includes(spec),workspace.testExclude.includes(spec),`${spec} must run in exactly one UI lane`);
 for(const filter of views.tests.filter(entry=>entry.startsWith('tests/unit/')))assert.ok(specs.includes(filter),`views filter ${filter} matches no spec`);
 assert.throws(()=>laneNamed('no-such-lane'),/Unknown lane: no-such-lane/);
});
test('the lane parent directory is configurable',()=>{
 const configured=resolve(root,'..','elsewhere');
 assert.equal(lanesParent(root,{OKF_LANES_DIR:configured},'win32',()=>true),configured);
 assert.equal(lanesParent(root,{OKF_LANES_DIR:'  '},'win32',()=>true),'D:\\okf\\lanes');
 assert.equal(lanesParent(root,{},'win32',()=>false),resolve(root,'..','okf-jawn-lanes'));
 assert.equal(lanesParent(root,{},'linux',()=>true),resolve(root,'..','okf-jawn-lanes'));
});
test('AGENTS.md carries the table rendered from the lane table',async()=>{
 const agents=await read('AGENTS.md');
 const begin=agents.indexOf(LANE_TABLE_BEGIN),end=agents.indexOf(LANE_TABLE_END);
 assert.ok(begin>=0&&end>begin,'AGENTS.md is missing the lane-table markers');
 assert.equal(agents.slice(begin+LANE_TABLE_BEGIN.length,end).trim(),renderLaneTable());
 for(const lane of lanes)assert.ok(agents.includes(`\`bun scripts/dev.mjs lane ${lane.name}\``),lane.name);
});
test('CODEOWNERS names real paths and its lane rows agree with the lane table',async()=>{
 const rows=(await read('.github/CODEOWNERS')).split(/\r?\n/).filter(line=>line.trim()&&!line.startsWith('#'))
  .map(line=>({path:line.split(/\s+/)[0],lane:/# lane: (\S+)/.exec(line)?.[1]??null}));
 for(const row of rows){
  assert.ok(row.path.startsWith('/'),row.path);
  assert.ok(await existsExact(row.path),`CODEOWNERS path ${row.path} does not exist with that exact spelling`);
 }
 for(const lane of lanes)assert.deepEqual(rows.filter(row=>row.lane===lane.name).map(row=>row.path).sort(),[...lane.directories,...lane.files].map(path=>`/${path}`).sort(),lane.name);
 for(const row of rows.filter(entry=>entry.lane))assert.ok(lanes.some(lane=>lane.name===row.lane),`CODEOWNERS names unknown lane ${row.lane}`);
});
```

- [ ] Run it.
  `bun test ./tests/foundation/lanes.test.mjs`
  Expected: fails to load with `Cannot find module '../../scripts/lib/lanes.mjs'`.

- [ ] Write `scripts/lib/lanes.mjs`:

```js
/**
 * The one lane table. Worktree creation and reset, each lane's gate, the scope check, the
 * AGENTS.md table and the CODEOWNERS lane rows read it; nothing else lists lanes.
 */
import { existsSync } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { run } from './process.mjs';

/** Unit specs of the views lane that sit beside the workspace-ui specs, relative to `ui/`. */
const viewsUnitSpecs = ['catalog-schema.test.ts', 'json-render-roundtrip.test.tsx', 'layout.test.tsx',
  'mcp-apps-dispatch.test.tsx', 'mcp-apps-mime.test.ts', 'view-document-roundtrip.test.tsx'].map(name => `tests/unit/${name}`);

export const lanes = Object.freeze([
  { name: 'storage', branch: 'build/storage', kind: 'rust',
    directories: ['crates/storage/'], files: [], exclude: ['crates/storage/Cargo.toml'],
    crates: ['okf-jawn-storage'], features: ['okf-jawn-storage/runtime'], tests: [], testExclude: [],
    proves: 'store and index tests pass; rebuild never touches jobs, reviews or receipts' },
  { name: 'ingest', branch: 'build/ingest', kind: 'rust',
    directories: ['crates/ingest/'], files: [], exclude: ['crates/ingest/Cargo.toml'],
    crates: ['okf-jawn-ingest'], features: ['okf-jawn-ingest/runtime'], tests: [], testExclude: [],
    proves: 'converter and job-handler tests pass; crash/restart against the real RecordStore is a separate construction receipt' },
  { name: 'core-cli', branch: 'build/core-cli', kind: 'rust',
    directories: ['crates/core/', 'crates/cli/'], files: [], exclude: ['crates/core/Cargo.toml', 'crates/cli/Cargo.toml'],
    crates: ['okf-jawn-core', 'okf-jawn-cli'], features: [], tests: [], testExclude: [],
    proves: 'Application operations and the CLI command surface' },
  { name: 'server', branch: 'build/server', kind: 'rust',
    directories: ['crates/server/'], files: [], exclude: ['crates/server/Cargo.toml'],
    crates: ['okf-jawn-server'], features: ['okf-jawn-server/runtime'], tests: [], testExclude: [],
    proves: 'auth, session, sandbox-origin and readiness bindings' },
  { name: 'mcp-execution', branch: 'build/mcp-execution', kind: 'rust',
    directories: ['crates/mcp/'], files: [], exclude: ['crates/mcp/Cargo.toml'],
    crates: ['okf-jawn-mcp'], features: [], tests: [], testExclude: [],
    proves: 'tool and resource adapters; `bun scripts/dev.mjs qualify mcp-wire` against a running endpoint is a separate receipt' },
  { name: 'workspace-ui', branch: 'build/workspace-ui', kind: 'ui',
    directories: ['ui/'], files: [],
    exclude: ['ui/src/features/views/', 'ui/src/mcp-apps/', ...viewsUnitSpecs.map(spec => `ui/${spec}`), 'ui/package.json', 'ui/biome.json'],
    crates: [], features: [], tests: [], testExclude: ['src/features/views/**', 'src/mcp-apps/**', ...viewsUnitSpecs],
    proves: 'Explorer, WebMCP and sandbox-viewer unit results' },
  { name: 'views', branch: 'build/views', kind: 'ui',
    directories: ['ui/src/features/views/', 'ui/src/mcp-apps/'], files: viewsUnitSpecs.map(spec => `ui/${spec}`), exclude: [],
    crates: [], features: [], tests: ['src/features/views/', 'src/mcp-apps/', ...viewsUnitSpecs], testExclude: [],
    proves: 'View and MCP Apps unit results; the Apps bundle build and host rendering are separate qualifications' },
]);

export const LANE_TABLE_BEGIN = '<!-- lane-table:begin generated by `bun scripts/dev.mjs lanes-table` from scripts/lib/lanes.mjs; do not edit -->';
export const LANE_TABLE_END = '<!-- lane-table:end -->';

export function laneNamed(name) {
  const lane = lanes.find(entry => entry.name === name);
  if (!lane) throw new Error(`Unknown lane: ${name ?? '(none given)'}. Lanes: ${lanes.map(entry => entry.name).join(', ')}.`);
  return lane;
}

/** Where lane worktrees live: OKF_LANES_DIR, else D:\okf\lanes on Windows when D: exists, else ../okf-jawn-lanes. */
export function lanesParent(root, env = process.env, platform = process.platform, present = existsSync) {
  const configured = env.OKF_LANES_DIR?.trim();
  if (configured) return resolve(configured);
  if (platform === 'win32' && present('D:\\')) return 'D:\\okf\\lanes';
  return resolve(root, '..', 'okf-jawn-lanes');
}

/** Git in `cwd` with captured output. A non-zero exit is a result to inspect, never an exception. */
export function gitIn(cwd) {
  return (args, options = {}) => run('git', args, { cwd: options.cwd ?? cwd, capture: true, allowFailure: true });
}

/** The stdout of a git call that had to succeed. */
function must(result, what) {
  if (result.code !== 0) throw new Error(`${what} failed (git exit ${result.code}): ${result.stderr.trim()}`);
  return result.stdout;
}

const code = text => `\`${text}\``;

function owned(lane) {
  const paths = [...lane.directories, ...lane.files].map(code).join(', ');
  return lane.exclude.length ? `${paths} except ${lane.exclude.map(code).join(', ')}` : paths;
}

export function renderLaneTable() {
  return [
    '| Lane | Directories | Gate command | Expected receipt |',
    '| --- | --- | --- | --- |',
    '| integration-owner | `crates/contract`, `xtask`, `api/`, `generated/`, manifests, lockfiles, `scripts/`, `deploy/`, `tests/integration/` | `bun scripts/dev.mjs premerge` | `.artifacts/premerge/<sha>.log` ending `PASS premerge <sha>`: every CI step with all features, plus `check-receipts` |',
    ...lanes.map(lane => `| ${lane.name} | ${owned(lane)} | \`bun scripts/dev.mjs lane ${lane.name}\` | \`.artifacts/lane/${lane.name}/<sha>.log\` ending \`PASS ${lane.name} <sha>\`: ${lane.proves} |`),
  ].join('\n');
}

/** Rewrite the generated table between the markers in AGENTS.md. Returns whether the file changed. */
export async function syncLaneTable(root) {
  const path = join(root, 'AGENTS.md');
  const current = await readFile(path, 'utf8');
  const begin = current.indexOf(LANE_TABLE_BEGIN); const end = current.indexOf(LANE_TABLE_END);
  if (begin < 0 || end < begin) throw new Error('AGENTS.md has no lane-table markers; restore them before regenerating.');
  const next = `${current.slice(0, begin + LANE_TABLE_BEGIN.length)}\n${renderLaneTable()}\n${current.slice(end)}`;
  if (next === current) return false;
  await writeFile(path, next);
  return true;
}

/** Create `build/<lane>` worktrees from HEAD for the named lanes, or for all of them. */
export async function createLanes(root, { names = [], parent = lanesParent(root), git = gitIn(root) } = {}) {
  const selected = names.length ? names.map(laneNamed) : lanes;
  const status = await git(['status', '--porcelain']);
  if (status.code !== 0 || status.stdout.trim()) throw new Error('Commit the foundation before creating worktrees; no dirty-state fan-out.');
  const base = must(await git(['rev-parse', '--verify', 'HEAD']), 'git rev-parse HEAD').trim();
  await mkdir(parent, { recursive: true });
  const created = [];
  for (const lane of selected) {
    must(await git(['worktree', 'add', '-b', lane.branch, join(parent, lane.name), base]), `git worktree add ${lane.name}`);
    created.push(join(parent, lane.name));
  }
  return created;
}
```

- [ ] Mechanical edits in `scripts/dev.mjs`:
  - add after the `./lib/toolchain.mjs` import (`:11`):
    `import { createLanes, syncLaneTable } from './lib/lanes.mjs';`
  - delete `:80-88` (`async function lanes() { ... }`).
  - `:270` → 
    ```js
        case 'lanes': process.stdout.write(`${(await createLanes(root, { names: args })).join('\n')}\n`); break;
        case 'lanes-table': process.stdout.write(await syncLaneTable(root) ? 'AGENTS.md lane table regenerated.\n' : 'AGENTS.md lane table is current.\n'); break;
    ```
  (`lanesReset` at `:96-149` still carries its own list; Task A.8 replaces it.)

- [ ] `AGENTS.md`: replace lines 13-22 (the whole table) with exactly these two lines, then
  generate:

```text
<!-- lane-table:begin generated by `bun scripts/dev.mjs lanes-table` from scripts/lib/lanes.mjs; do not edit -->
<!-- lane-table:end -->
```
  `bun scripts/dev.mjs lanes-table` → `AGENTS.md lane table regenerated.`
  Run it again → `AGENTS.md lane table is current.`

- [ ] Replace `.github/CODEOWNERS` with:

```text
# Review routing only — not a local filesystem lock. No branch protection is configured yet.
# Placeholder handles: replace with real GitHub teams/users before enabling required reviews.
# Lane rows are checked against scripts/lib/lanes.mjs by tests/foundation/lanes.test.mjs.

# Integration owner
/AGENTS.md @okf-jawn-integration-owner
/README.md @okf-jawn-integration-owner
/SPEC.md @okf-jawn-integration-owner
/verification.json @okf-jawn-integration-owner
/vendors.json @okf-jawn-integration-owner
/Cargo.toml @okf-jawn-integration-owner
/Cargo.lock @okf-jawn-integration-owner
/deny.toml @okf-jawn-integration-owner
/package.json @okf-jawn-integration-owner
/bun.lock @okf-jawn-integration-owner
/bunfig.toml @okf-jawn-integration-owner
/.bun-version @okf-jawn-integration-owner
/rust-toolchain.toml @okf-jawn-integration-owner
/justfile @okf-jawn-integration-owner
/REPOSITORY-TREE.txt @okf-jawn-integration-owner
/scripts/ @okf-jawn-integration-owner
/crates/contract/ @okf-jawn-integration-owner
/xtask/ @okf-jawn-integration-owner
/api/ @okf-jawn-integration-owner
/generated/ @okf-jawn-integration-owner
/ui/src/api/generated/ @okf-jawn-integration-owner
/ui/package.json @okf-jawn-integration-owner
/deploy/ @okf-jawn-integration-owner
/tests/integration/ @okf-jawn-integration-owner
/tests/foundation/ @okf-jawn-integration-owner
/.github/ @okf-jawn-integration-owner
/.agents/ @okf-jawn-integration-owner
/qualification/ @okf-jawn-integration-owner

# storage lane
/crates/storage/ @okf-jawn-integration-owner # lane: storage

# ingest lane (JobHandler + Tokio runtime adapter)
/crates/ingest/ @okf-jawn-integration-owner # lane: ingest

# core-cli lane
/crates/core/ @okf-jawn-integration-owner # lane: core-cli
/crates/cli/ @okf-jawn-integration-owner # lane: core-cli

# server lane (HTTP/auth + sandbox origin serving)
/crates/server/ @okf-jawn-integration-owner # lane: server

# mcp-execution lane (MCP tool execution)
/crates/mcp/ @okf-jawn-integration-owner # lane: mcp-execution

# workspace-ui lane (Explorer, WebMCP, HTML sandbox viewer)
/ui/ @okf-jawn-integration-owner # lane: workspace-ui

# views lane (saved Views + MCP Apps bundles) — more specific paths win
/ui/src/features/views/ @okf-jawn-integration-owner # lane: views
/ui/src/mcp-apps/ @okf-jawn-integration-owner # lane: views
/ui/tests/unit/catalog-schema.test.ts @okf-jawn-integration-owner # lane: views
/ui/tests/unit/json-render-roundtrip.test.tsx @okf-jawn-integration-owner # lane: views
/ui/tests/unit/layout.test.tsx @okf-jawn-integration-owner # lane: views
/ui/tests/unit/mcp-apps-dispatch.test.tsx @okf-jawn-integration-owner # lane: views
/ui/tests/unit/mcp-apps-mime.test.ts @okf-jawn-integration-owner # lane: views
/ui/tests/unit/view-document-roundtrip.test.tsx @okf-jawn-integration-owner # lane: views
```
  (The `/REPOSITORY-TREE.txt` row stays until Task A.16 deletes the file; the exact-path test
  makes that task remove the row.)

- [ ] Run it.
  `bun test ./tests/foundation/lanes.test.mjs` → ` 5 pass`, ` 0 fail`.
  Mutation proof: change `/justfile` back to `/Justfile` in CODEOWNERS and run the test →
  `CODEOWNERS path /Justfile does not exist with that exact spelling`; restore.

- [ ] Commit.
  `git add scripts/lib/lanes.mjs scripts/dev.mjs tests/foundation/lanes.test.mjs AGENTS.md .github/CODEOWNERS`

```text
refactor(gates): keep one lane table and read it everywhere.

Why: the lane list lived in dev.mjs twice, in the AGENTS.md table and in CODEOWNERS, and the lane parent directory was hard-coded beside the checkout although lane worktrees live on D: (design section 3, Disk). CODEOWNERS named /Justfile; the file is justfile.
What changed: scripts/lib/lanes.mjs holds name, branch, kind, directories, owned files, exclusions, crates, features and Vitest filters. `lanes [name ...]` creates worktrees under OKF_LANES_DIR, else D:\okf\lanes, else ../okf-jawn-lanes. The AGENTS.md table is generated by `lanes-table`; an offline test holds AGENTS.md and CODEOWNERS to the table and checks every CODEOWNERS path with exact case.
Verified: bun test ./tests/foundation/lanes.test.mjs -> 5 pass, 0 fail; bun scripts/dev.mjs lanes-table -> "AGENTS.md lane table is current."
Next: Task A.7, the scope check.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.7: Scope check — a lane branch may change only its own paths

**Files:**
- Create: `tests/foundation/fixture-repo.mjs`.
- Modify: `scripts/lib/lanes.mjs` (append); `scripts/dev.mjs` (import, two helpers, one case).
- Test: `tests/foundation/lanes.test.mjs` (append).

**Interfaces — produces:**

```js
// scripts/lib/lanes.mjs
export const generatedRoots;   // ['api/', 'generated/cli/', 'ui/src/api/generated/']
export const cures;            // [{ name, branch, base, paths: string[] }]  temporary Stage 1 rows
export function scopeFor(branch);            // { name, base, allowed: string[], exclude: string[] } | null
export function outOfScope(paths, scope);    // string[]: the paths that violate the scope, in input order
export async function checkScope(root, { lane, base, git } = {});
// resolves { name, base: <merge-base sha>, changed: string[] (sorted) }; rejects with
// `scope check failed for <name>: <n> path(s) outside its scope:\n  <path>...` or
// `No scope row for branch <branch>; ...`

// tests/foundation/fixture-repo.mjs
export async function git(cwd, ...args);                       // trimmed stdout; rejects on non-zero
export async function fixtureRepo(t, files?);                  // { base, root }; branch `main`, one commit
export async function commit(root, files, message);            // full sha of the new commit
```

A path is in scope when it is under an `allowed` entry (an entry ending in `/` is a directory
prefix, any other entry is an exact file) and under no `exclude` entry. For a lane, `allowed` is
its `directories`, its `files` and `generatedRoots` (AGENTS.md generator-input rule: "run
`bun scripts/dev.mjs gen`, and commit the generated outputs with the lane change"). Changed paths
are the tracked differences between `git merge-base HEAD <base>` and the working tree, plus
untracked files that are not ignored. `<base>` is tried as given, then as `origin/<base>`.

- [ ] Write `tests/foundation/fixture-repo.mjs`:

```js
/** Disposable Git repositories for tooling tests; never the real checkout. */
import { mkdtemp, mkdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { gitLocalEnvironment, run } from '../../scripts/lib/process.mjs';

// A second guard beside dev.mjs: no fixture command may inherit a hook's repository.
for (const name of gitLocalEnvironment) delete process.env[name];

const identity = ['-c', 'user.name=okf-jawn test', '-c', 'user.email=test@example.invalid', '-c', 'commit.gpgsign=false'];

export async function git(cwd, ...args) {
  return (await run('git', [...identity, ...args], { cwd, capture: true })).stdout.trim();
}

export async function commit(root, files, message) {
  for (const [path, content] of Object.entries(files)) {
    await mkdir(dirname(join(root, path)), { recursive: true });
    await writeFile(join(root, path), content);
  }
  await git(root, 'add', '--all');
  await git(root, 'commit', '--quiet', '-m', message);
  return git(root, 'rev-parse', 'HEAD');
}

export async function fixtureRepo(t, files = { 'README.md': 'fixture\n' }) {
  const base = await realpath(await mkdtemp(join(tmpdir(), 'okf-repo-')));
  // Cleanup of a scratch directory is best effort; a locked file there is not a test failure.
  t.after(() => rm(base, { recursive: true, force: true, maxRetries: 3 }).catch(() => {}));
  const root = join(base, 'repo');
  await mkdir(root);
  await git(root, 'init', '--quiet', '--initial-branch=main');
  await commit(root, files, 'initial');
  return { base, root };
}
```

- [ ] Append to `tests/foundation/lanes.test.mjs` (extend the two import lines as shown):

```js
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { LANE_TABLE_BEGIN, LANE_TABLE_END, checkScope, laneNamed, lanes, lanesParent, outOfScope, renderLaneTable, scopeFor } from '../../scripts/lib/lanes.mjs';
import { commit, fixtureRepo, git } from './fixture-repo.mjs';
```

```js
test('paths outside a lane, and manifests inside it, are out of scope',()=>{
 assert.deepEqual(outOfScope(['crates/storage/src/lib.rs','api/operations.json','crates/storage/Cargo.toml','crates/core/src/lib.rs','Cargo.lock'],scopeFor('build/storage')),['crates/storage/Cargo.toml','crates/core/src/lib.rs','Cargo.lock']);
 assert.deepEqual(outOfScope(['ui/src/routes/index.tsx','ui/src/features/views/Chart.tsx','ui/package.json','ui/tests/unit/layout.test.tsx'],scopeFor('build/workspace-ui')),['ui/src/features/views/Chart.tsx','ui/package.json','ui/tests/unit/layout.test.tsx']);
 assert.deepEqual(outOfScope(['ui/src/features/views/Chart.tsx','ui/tests/unit/layout.test.tsx','ui/tests/unit/wire.test.ts'],scopeFor('build/views')),['ui/tests/unit/wire.test.ts']);
 assert.equal(scopeFor('feature/unknown'),null);
});
test('the scope check fails a lane branch that changed an out-of-lane file',async t=>{
 const {root}=await fixtureRepo(t,{'crates/storage/src/lib.rs':'//! storage\n','crates/core/src/lib.rs':'//! core\n'});
 await git(root,'checkout','--quiet','-b','build/storage');
 await commit(root,{'crates/storage/src/lib.rs':'//! storage, changed\n','api/operations.json':'[]\n'},'in lane, with generated output');
 assert.deepEqual((await checkScope(root)).changed,['api/operations.json','crates/storage/src/lib.rs']);
 await commit(root,{'crates/core/src/lib.rs':'//! core, changed from the storage lane\n'},'out of lane');
 await assert.rejects(checkScope(root),/scope check failed for storage: 1 path\(s\) outside its scope:\n  crates\/core\/src\/lib\.rs/);
 await assert.rejects(checkScope(root,{lane:'core-cli'}),/crates\/storage\/src\/lib\.rs/);
});
test('uncommitted and untracked files count, and a branch without a row has no scope',async t=>{
 const {root}=await fixtureRepo(t);
 await git(root,'checkout','--quiet','-b','build/ingest');
 await writeFile(join(root,'stray.txt'),'x');
 await assert.rejects(checkScope(root),/scope check failed for ingest[\s\S]*stray\.txt/);
 await git(root,'checkout','--quiet','-b','cure/unknown');
 await assert.rejects(checkScope(root),/No scope row for branch cure\/unknown/);
});
test('this package has a scope row against the integration branch',()=>{
 const scope=scopeFor('cure/gates');
 assert.equal(scope.base,'integration/foundation-cure');
 assert.deepEqual(outOfScope(['scripts/dev.mjs','scripts/hooks/pre-push','Cargo.lock','crates/core/Cargo.toml','crates/core/src/dispatch.rs','qualification/docling/run.mjs','scripts/lib/provenance.mjs','tests/foundation/harness.test.mjs'],scope),['crates/core/src/dispatch.rs','qualification/docling/run.mjs','scripts/lib/provenance.mjs','tests/foundation/harness.test.mjs']);
});
```

- [ ] Run it.
  `bun test ./tests/foundation/lanes.test.mjs`
  Expected: fails to load with `Export named 'checkScope' not found in module`.

- [ ] Append to `scripts/lib/lanes.mjs`:

```js
/** Generated outputs any lane may commit after running `gen` (AGENTS.md, generator-input rule). */
export const generatedRoots = Object.freeze(['api/', 'generated/cli/', 'ui/src/api/generated/']);

/**
 * Stage 1 cure packages: temporary rows the integration owner adds per package and deletes when
 * Stage 1 closes. A `cure/*` branch without a row has no scope and fails the check.
 */
export const cures = Object.freeze([
  { name: 'gates', branch: 'cure/gates', base: 'integration/foundation-cure', paths: [
    'scripts/dev.mjs', 'scripts/lib/lanes.mjs', 'scripts/lib/receipts.mjs', 'scripts/lib/gates.mjs',
    'scripts/lib/process.mjs', 'scripts/lib/init.mjs', 'scripts/hooks/',
    'tests/foundation/fixture-repo.mjs', 'tests/foundation/lockfile.test.mjs', 'tests/foundation/lanes.test.mjs',
    'tests/foundation/receipts.test.mjs', 'tests/foundation/gates.test.mjs', 'tests/foundation/hooks.test.mjs',
    'tests/foundation/ci.test.mjs', 'tests/foundation/records.test.mjs', 'tests/foundation/policy.test.mjs',
    'tests/foundation/vendor.test.mjs', 'tests/foundation/process.test.mjs', 'tests/foundation/init.test.mjs',
    'tests/foundation/toolchain.test.mjs', 'tests/integration/acceptance.mjs',
    '.github/workflows/ci.yml', '.github/workflows/qualify.yml', '.github/workflows/contract.yml', '.github/CODEOWNERS',
    'Cargo.lock', 'crates/core/Cargo.toml', 'crates/storage/Cargo.toml', 'ui/vitest.config.ts',
    'README.md', 'AGENTS.md', 'justfile', 'verification.json', 'vendors.json', 'deploy/.env.example',
    'lefthook.yml', 'REPOSITORY-TREE.txt', '.cursor/plans/dependency_qualification_pass_e1c8326b.plan.md',
  ] },
]);

const inside = (path, entry) => (entry.endsWith('/') ? path.startsWith(entry) : path === entry);

export function scopeFor(branch) {
  const lane = lanes.find(entry => entry.branch === branch);
  if (lane) return { name: lane.name, base: 'main', allowed: [...lane.directories, ...lane.files, ...generatedRoots], exclude: lane.exclude };
  const cure = cures.find(entry => entry.branch === branch);
  return cure ? { name: cure.name, base: cure.base, allowed: cure.paths, exclude: [] } : null;
}

export function outOfScope(paths, scope) {
  return paths.filter(path => scope.exclude.some(entry => inside(path, entry)) || !scope.allowed.some(entry => inside(path, entry)));
}

/**
 * Fail when anything changed since the merge-base with the scope's base lies outside the scope.
 * Committed, staged, unstaged and untracked-but-not-ignored paths all count.
 */
export async function checkScope(root, { lane, base, git = gitIn(root) } = {}) {
  const branch = lane ? laneNamed(lane).branch : must(await git(['rev-parse', '--abbrev-ref', 'HEAD']), 'git rev-parse --abbrev-ref HEAD').trim();
  const scope = scopeFor(branch);
  if (!scope) throw new Error(`No scope row for branch ${branch}; add one to scripts/lib/lanes.mjs before pushing or gating it.`);
  const wanted = base ?? scope.base;
  let reference = null;
  for (const candidate of [wanted, `origin/${wanted}`]) {
    if ((await git(['rev-parse', '--verify', '--quiet', `${candidate}^{commit}`])).code === 0) { reference = candidate; break; }
  }
  if (!reference) throw new Error(`scope check needs ${wanted} or origin/${wanted} to compare against; neither exists here.`);
  const mergeBase = must(await git(['merge-base', 'HEAD', reference]), `git merge-base HEAD ${reference}`).trim();
  const tracked = must(await git(['diff', '--name-only', '--no-renames', '-z', mergeBase]), 'git diff --name-only');
  const untracked = must(await git(['ls-files', '--others', '--exclude-standard', '-z']), 'git ls-files --others');
  const changed = [...new Set([...tracked.split('\0'), ...untracked.split('\0')].filter(Boolean))].sort();
  const outside = outOfScope(changed, scope);
  if (outside.length) throw new Error(`scope check failed for ${scope.name}: ${outside.length} path(s) outside its scope:\n  ${outside.join('\n  ')}`);
  return { name: scope.name, base: mergeBase, changed };
}
```

- [ ] Mechanical edits in `scripts/dev.mjs`:
  - the lanes import → `import { checkScope, createLanes, syncLaneTable } from './lib/lanes.mjs';`
  - add after the `gitLocalEnvironment` loop:
    ```js
    /** The value following `--name`, or undefined. */
    const option = name => (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);
    /** Arguments that are neither a `--flag` nor the value of one. */
    const positional = args.filter((value, index) => !value.startsWith('--') && !args[index - 1]?.startsWith('--'));
    ```
  - add to the switch, after `case 'lanes-table'`:
    ```js
        case 'scope': {
          const result = await checkScope(root, { lane: positional[0], base: option('--base') });
          process.stdout.write(`scope: ${result.changed.length} changed path(s) since ${result.base}, all inside ${result.name}.\n`);
          break;
        }
    ```

- [ ] Run it.
  `bun test ./tests/foundation/lanes.test.mjs` → ` 9 pass`, ` 0 fail`.
  `bun scripts/dev.mjs scope` (on `cure/gates`) → `scope: <n> changed path(s) since 678f919…, all
  inside gates.`

- [ ] Commit.
  `git add scripts/lib/lanes.mjs scripts/dev.mjs tests/foundation/lanes.test.mjs tests/foundation/fixture-repo.mjs`

```text
feat(gates): add the scope check for lane and cure branches.

Why: SPEC section 14 says ownership is "enforced locally by isolated worktrees", but nothing stopped a lane branch from changing another lane's files, a manifest or a lockfile (design section 7 rule 3).
What changed: `bun scripts/dev.mjs scope [lane] [--base ref]` compares the merge-base with main (the integration branch for cure rows) against the working tree and fails on any path outside the lane's directories, owned files and generated roots, or inside its exclusions. Unknown branches have no scope and fail.
Verified: bun test ./tests/foundation/lanes.test.mjs -> 9 pass, 0 fail (a build/storage branch that edits crates/core/src/lib.rs is rejected); bun scripts/dev.mjs scope on cure/gates -> all inside gates.
Next: Task A.8, lanes-reset.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.8: `lanes-reset` checks every worktree, treats any git failure as dirty, never forces

**Files:**
- Modify: `scripts/lib/lanes.mjs` (imports, append); `scripts/dev.mjs:90-149` (delete
  `lanesReset` and `existsPath`), `:271` (switch), imports.
- Test: `tests/foundation/lanes.test.mjs` (append).

**Interfaces — produces:**

```js
export async function resetLanes(root, { parent = lanesParent(root), git = gitIn(root) } = {});
// resolves 'lanes-reset: removed <n> worktree(s) and <m> branch(es); nothing was forced.'
// rejects  'lanes-reset refused; nothing was removed:\n  <reason>...' listing every reason.
```

Targets: for every lane, each registered worktree whose branch is `refs/heads/build/<lane>`
(wherever it lives, so the legacy `..\okf-jawn-lanes` worktrees are found), plus
`<parent>/<lane>` when that directory exists, whatever it has checked out. Refusal reasons: the
directory exists but is not a worktree of this repository; a registered worktree's directory is
missing; `git status --porcelain` there exits non-zero; its output is not empty; the worktree's
HEAD or the lane branch has commits that are not in the main checkout's HEAD. Removal uses
`git worktree remove <path>` and `git branch -d <branch>` only.

- [ ] Append to `tests/foundation/lanes.test.mjs` (extend imports as shown):

```js
import { existsSync } from 'node:fs';
import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { LANE_TABLE_BEGIN, LANE_TABLE_END, checkScope, createLanes, laneNamed, lanes, lanesParent, outOfScope, renderLaneTable, resetLanes, scopeFor } from '../../scripts/lib/lanes.mjs';
import { run } from '../../scripts/lib/process.mjs';
```

```js
test('lanes-reset removes clean, empty lanes without forcing anything',async t=>{
 const {base,root}=await fixtureRepo(t);const parent=join(base,'lanes');const calls=[];
 await createLanes(root,{names:['storage','views'],parent});
 const recording=(args,options={})=>{calls.push(args);return run('git',args,{cwd:options.cwd??root,capture:true,allowFailure:true});};
 assert.match(await resetLanes(root,{parent,git:recording}),/removed 2 worktree\(s\) and 2 branch\(es\); nothing was forced/);
 assert.equal(existsSync(join(parent,'storage')),false);assert.equal(existsSync(join(parent,'views')),false);
 assert.equal(await git(root,'branch','--list','build/*'),'');
 for(const args of calls)for(const flag of ['--force','-f','-D'])assert.ok(!args.includes(flag),`git ${args.join(' ')}`);
});
test('lanes-reset refuses a dirty worktree even when its branch was deleted',async t=>{
 const {base,root}=await fixtureRepo(t);const parent=join(base,'lanes');
 await createLanes(root,{names:['storage','ingest'],parent});
 const storage=join(parent,'storage');
 await git(storage,'checkout','--quiet','--detach');
 await git(root,'branch','-D','build/storage');
 await writeFile(join(storage,'unsaved-work.txt'),'not committed anywhere');
 await assert.rejects(resetLanes(root,{parent}),/lanes-reset refused; nothing was removed:[\s\S]*storage is dirty/);
 assert.equal(await readFile(join(storage,'unsaved-work.txt'),'utf8'),'not committed anywhere');
 assert.equal(existsSync(join(parent,'ingest')),true,'a refusal leaves every lane in place');
});
test('lanes-reset treats a failed git status as dirty, and lane commits as work',async t=>{
 const {base,root}=await fixtureRepo(t);const parent=join(base,'lanes');
 await createLanes(root,{names:['server'],parent});
 const failing=(args,options={})=>args[0]==='status'&&options.cwd?Promise.resolve({code:128,stdout:'',stderr:'fatal: simulated'}):run('git',args,{cwd:options.cwd??root,capture:true,allowFailure:true});
 await assert.rejects(resetLanes(root,{parent,git:failing}),/could not read its status \(git exit 128\); treated as dirty/);
 await commit(join(parent,'server'),{'crates/server/src/new.rs':'//! lane work\n'},'lane work');
 await assert.rejects(resetLanes(root,{parent}),/1 commit\(s\) that are not in HEAD/);
 assert.equal(existsSync(join(parent,'server')),true);
});
test('a directory that is not a worktree of this repository is never removed',async t=>{
 const {base,root}=await fixtureRepo(t);const parent=join(base,'lanes');
 await mkdir(join(parent,'views'),{recursive:true});await writeFile(join(parent,'views','notes.txt'),'someone else');
 await assert.rejects(resetLanes(root,{parent}),/views exists but is not a worktree of this repository/);
 assert.equal(existsSync(join(parent,'views','notes.txt')),true);
});
test('the entrypoint lists no lane and no lane directory of its own, and nothing forces git',async()=>{
 const entry=await read('scripts/dev.mjs');
 for(const lane of lanes)assert.doesNotMatch(entry,new RegExp(`'${lane.name}'`),lane.name);
 assert.doesNotMatch(entry,/okf-jawn-lanes|--force/);
 assert.doesNotMatch(await read('scripts/lib/lanes.mjs'),/'--force'|'-f'|'-D'/);
});
```

- [ ] Run it.
  `bun test ./tests/foundation/lanes.test.mjs`
  Expected: fails to load with `Export named 'resetLanes' not found in module`.

- [ ] In `scripts/lib/lanes.mjs` change the two import lines to

```js
import { existsSync, realpathSync } from 'node:fs';
import { exists } from './files.mjs';
```
  (the second is new, placed after the `./process.mjs` import) and append:

```js
/** A path as the filesystem spells it, so git's and Node's spellings of one directory compare equal. */
function canonical(path) {
  let real;
  try { real = realpathSync.native(path); } catch { real = resolve(path); }
  return process.platform === 'win32' ? real.toLowerCase() : real;
}

/** Registered worktrees as {path, head, branch}; `branch` is null when detached. */
async function worktrees(git) {
  return must(await git(['worktree', 'list', '--porcelain']), 'git worktree list').split(/\r?\n\r?\n/)
    .map(block => block.trim()).filter(Boolean).map(block => ({
      path: /^worktree (.+)$/m.exec(block)?.[1] ?? '',
      head: /^HEAD ([0-9a-f]{40})$/m.exec(block)?.[1] ?? null,
      branch: /^branch (.+)$/m.exec(block)?.[1] ?? null,
    }));
}

/**
 * Remove lane worktrees and branches that hold nothing. Every lane directory that exists is
 * inspected whatever it has checked out; any doubt is a refusal, and a refusal removes nothing.
 */
export async function resetLanes(root, { parent = lanesParent(root), git = gitIn(root) } = {}) {
  const registered = await worktrees(git);
  const head = must(await git(['rev-parse', '--verify', 'HEAD']), 'git rev-parse HEAD').trim();
  const main = canonical(root);
  const targets = new Map();
  const branches = [];
  for (const lane of lanes) {
    for (const entry of registered) {
      if (entry.branch === `refs/heads/${lane.branch}` && canonical(entry.path) !== main) targets.set(canonical(entry.path), { path: entry.path, entry });
    }
    const expected = join(parent, lane.name);
    if (await exists(expected) && !targets.has(canonical(expected))) {
      targets.set(canonical(expected), { path: expected, entry: registered.find(entry => canonical(entry.path) === canonical(expected)) ?? null });
    }
    if ((await git(['rev-parse', '--verify', '--quiet', `refs/heads/${lane.branch}`])).code === 0) branches.push(lane.branch);
  }
  const refusals = [];
  const holdsWork = async (tip, label) => {
    const count = await git(['rev-list', '--count', `${head}..${tip}`]);
    if (count.code !== 0) return `${label}: could not compare it with HEAD (git exit ${count.code}); treated as holding work`;
    return Number(count.stdout.trim()) > 0 ? `${label} has ${count.stdout.trim()} commit(s) that are not in HEAD` : null;
  };
  for (const { path, entry } of targets.values()) {
    if (!entry) { refusals.push(`${path} exists but is not a worktree of this repository`); continue; }
    if (!await exists(path)) { refusals.push(`${path} is a registered worktree whose directory is missing; run \`git worktree prune\` and retry`); continue; }
    const status = await git(['status', '--porcelain'], { cwd: path });
    if (status.code !== 0) { refusals.push(`${path}: could not read its status (git exit ${status.code}); treated as dirty`); continue; }
    if (status.stdout.trim()) { refusals.push(`${path} is dirty`); continue; }
    const problem = entry.head ? await holdsWork(entry.head, path) : `${path} has no readable HEAD; treated as holding work`;
    if (problem) refusals.push(problem);
  }
  for (const branch of branches) {
    const problem = await holdsWork(`refs/heads/${branch}`, branch);
    if (problem) refusals.push(problem);
  }
  if (refusals.length) throw new Error(`lanes-reset refused; nothing was removed:\n  ${[...new Set(refusals)].join('\n  ')}`);
  for (const { path } of targets.values()) must(await git(['worktree', 'remove', path]), `git worktree remove ${path}`);
  for (const branch of branches) must(await git(['branch', '-d', branch]), `git branch -d ${branch}`);
  return `lanes-reset: removed ${targets.size} worktree(s) and ${branches.length} branch(es); nothing was forced.`;
}
```

- [ ] Mechanical edits in `scripts/dev.mjs`:
  - delete `:90-149` (the doc comment, `async function lanesReset()` and
    `async function existsPath(path)`).
  - the lanes import → `import { checkScope, createLanes, resetLanes, syncLaneTable } from './lib/lanes.mjs';`
  - `case 'lanes-reset'` → `    case 'lanes-reset': process.stdout.write(`${await resetLanes(root)}\n`); break;`

- [ ] Run it.
  `bun test ./tests/foundation/lanes.test.mjs` → ` 14 pass`, ` 0 fail`.

- [ ] Commit.
  `git add scripts/lib/lanes.mjs scripts/dev.mjs tests/foundation/lanes.test.mjs`

```text
fix(gates): make lanes-reset refuse on any doubt and never force.

Why: when build/<lane> did not exist the safety loop skipped the worktree, and the removal loop still ran `git worktree remove --force` on it; a failed `git status` counted as clean. A dirty worktree whose branch had been deleted lost its files.
What changed: resetLanes inspects every registered lane worktree and every existing <parent>/<lane> directory whatever it has checked out, collects all refusals (unregistered directory, missing directory, non-zero status, dirty, commits not in HEAD), removes nothing if there is one, and uses only `git worktree remove` and `git branch -d`. dev.mjs no longer lists lanes or their directory.
Verified: bun test ./tests/foundation/lanes.test.mjs -> 14 pass, 0 fail, including the deleted-branch dirty worktree and the simulated status failure.
Next: Task A.9, check-receipts.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.9: `check-receipts` requires receipts once Phase 0 is recorded as qualified

**Files:**
- Create: `scripts/lib/receipts.mjs`, `tests/foundation/receipts.test.mjs`.
- Modify: `scripts/dev.mjs:3` (import), `:175-240` (delete `checkReceipts`), `:273` (switch).
- Test: `tests/foundation/receipts.test.mjs`.

**Interfaces:**
- Consumes: `verification.json` — `phase_0_qualified: boolean`,
  `current.gates.phase_0: [{ id, status, receipt: string|null }]`. Receipt header from package B
  (see the package header).
- Produces:

```js
// scripts/lib/receipts.mjs
export const libraryGates;   // ['docling-library-qualification', 'iii-library-qualification', 'mcp-apps-protocol-qualification']
export async function checkReceipts(root);   // resolves a one-line summary; rejects 'check-receipts failed:\n<one line per failure>'
```

Rules: every `qualification/receipts/*.json` must have the header, a `git_sha` that is an
ancestor of HEAD, and no input changed between `git_sha` and HEAD. While `phase_0_qualified` is
`false`, an empty directory is accepted. When it is `true`, each gate in `libraryGates` must
exist in `verification.json` and its `receipt` must be `qualification/receipts/<file>.json`
naming a file that exists. The directory is never created by the check.

- [ ] Write `tests/foundation/receipts.test.mjs`:

```js
/** check-receipts: receipts describe HEAD, and a qualified Phase 0 has one per library gate. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { checkReceipts, libraryGates } from '../../scripts/lib/receipts.mjs';
import { commit, fixtureRepo, git } from './fixture-repo.mjs';

const paths=Object.fromEntries(libraryGates.map(id=>[id,`qualification/receipts/${id}.json`]));
const verification=(qualified,receipts={})=>JSON.stringify({phase_0_qualified:qualified,current:{gates:{phase_0:libraryGates.map(id=>({id,status:qualified?'passed':'incomplete',receipt:receipts[id]??null}))}}});
const receipt=(sha,inputs=['harness/run.mjs'])=>JSON.stringify({git_sha:sha,inputs,produced_at:'2026-10-05T18:00:00Z'});

test('an empty receipts directory is accepted only while Phase 0 is unqualified',async t=>{
 const {root}=await fixtureRepo(t,{'verification.json':verification(false),'qualification/receipts/.gitkeep':''});
 assert.match(await checkReceipts(root),/accepted only because verification\.json records phase_0_qualified false/);
 await commit(root,{'verification.json':verification(true,paths)},'claim qualification without receipts');
 await assert.rejects(checkReceipts(root),error=>libraryGates.every(id=>error.message.includes(`${id}: phase_0_qualified is true but its receipt`)));
});
test('a qualified Phase 0 passes with one current receipt per library gate',async t=>{
 const {root}=await fixtureRepo(t,{'verification.json':verification(false),'harness/run.mjs':'// harness\n'});
 const sha=await git(root,'rev-parse','HEAD');
 await commit(root,{'verification.json':verification(true,paths),...Object.fromEntries(Object.values(paths).map(path=>[path,receipt(sha)]))},'record R');
 assert.match(await checkReceipts(root),/3 receipt\(s\) valid against HEAD; all 3 Phase 0 library gates have one/);
 await commit(root,{'verification.json':verification(true,{...paths,'iii-library-qualification':'.artifacts/qualification/iii/receipt.json'})},'point a gate outside the committed receipts');
 await assert.rejects(checkReceipts(root),/iii-library-qualification: phase_0_qualified is true but its receipt \(\.artifacts\/qualification\/iii\/receipt\.json\)/);
});
test('a receipt is rejected when an input changed, its commit is foreign, or its header is incomplete',async t=>{
 const {root}=await fixtureRepo(t,{'verification.json':verification(false),'harness/run.mjs':'// v1\n'});
 const sha=await git(root,'rev-parse','HEAD');
 await commit(root,{'qualification/receipts/a.json':receipt(sha)},'receipt');
 assert.match(await checkReceipts(root),/1 receipt\(s\) valid against HEAD\./);
 await commit(root,{'harness/run.mjs':'// v2\n'},'change an input');
 await assert.rejects(checkReceipts(root),/a\.json: inputs changed after [0-9a-f]{40}:\n  harness\/run\.mjs/);
 await commit(root,{'qualification/receipts/a.json':receipt('0'.repeat(40))},'foreign commit');
 await assert.rejects(checkReceipts(root),/a\.json: git_sha 0{40} is not an ancestor of HEAD/);
 await commit(root,{'qualification/receipts/a.json':JSON.stringify({git_sha:sha,inputs:[],produced_at:'2026-10-05T18:00:00Z'})},'no inputs');
 await assert.rejects(checkReceipts(root),/a\.json: inputs must be a non-empty string\[\]/);
 await commit(root,{'qualification/receipts/a.json':JSON.stringify({git_sha:sha,inputs:['harness/run.mjs']})},'no time');
 await assert.rejects(checkReceipts(root),/a\.json: produced_at must be an RFC 3339 time/);
 await commit(root,{'qualification/receipts/a.json':'{not json'},'not json');
 await assert.rejects(checkReceipts(root),/a\.json: not valid JSON/);
});
```

- [ ] Run it.
  `bun test ./tests/foundation/receipts.test.mjs`
  Expected: fails to load with `Cannot find module '../../scripts/lib/receipts.mjs'`.

- [ ] Write `scripts/lib/receipts.mjs`:

```js
/** Committed qualification receipts must describe HEAD, and a qualified Phase 0 must have them. */
import { readdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { run } from './process.mjs';
import { exists } from './files.mjs';

/** Phase 0 gates that are closed by an executed library qualification and its receipt. */
export const libraryGates = Object.freeze(['docling-library-qualification', 'iii-library-qualification', 'mcp-apps-protocol-qualification']);

const receiptsPath = 'qualification/receipts';

/** Reasons one receipt file cannot be trusted against HEAD; empty when it can. */
async function receiptFailures(root, directory, name) {
  let receipt;
  try { receipt = JSON.parse(await readFile(join(directory, name), 'utf8')); }
  catch (error) { return [`${name}: not valid JSON (${error.message})`]; }
  const sha = receipt.git_sha;
  if (typeof sha !== 'string' || !/^[0-9a-f]{40}$/.test(sha)) return [`${name}: git_sha must be a full 40-hex commit`];
  if (!Array.isArray(receipt.inputs) || !receipt.inputs.length || !receipt.inputs.every(item => typeof item === 'string' && item.length > 0)) {
    return [`${name}: inputs must be a non-empty string[] of repo-relative paths`];
  }
  if (typeof receipt.produced_at !== 'string' || Number.isNaN(Date.parse(receipt.produced_at))) return [`${name}: produced_at must be an RFC 3339 time`];
  const git = args => run('git', args, { cwd: root, capture: true, allowFailure: true });
  if ((await git(['merge-base', '--is-ancestor', sha, 'HEAD'])).code !== 0) return [`${name}: git_sha ${sha} is not an ancestor of HEAD`];
  const changed = await git(['diff', '--name-only', sha, 'HEAD', '--', ...receipt.inputs]);
  if (changed.code !== 0) return [`${name}: could not compare its inputs with HEAD (git exit ${changed.code})`];
  const names = changed.stdout.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
  return names.length ? [`${name}: inputs changed after ${sha}:\n  ${names.join('\n  ')}`] : [];
}

export async function checkReceipts(root) {
  const verification = JSON.parse(await readFile(join(root, 'verification.json'), 'utf8'));
  const qualified = verification.phase_0_qualified === true;
  const directory = join(root, ...receiptsPath.split('/'));
  const entries = await exists(directory) ? (await readdir(directory)).filter(name => name.endsWith('.json')).sort() : [];
  const failures = [];
  for (const name of entries) failures.push(...await receiptFailures(root, directory, name));
  if (qualified) {
    const gates = verification.current?.gates?.phase_0 ?? [];
    for (const id of libraryGates) {
      const gate = gates.find(entry => entry.id === id);
      if (!gate) { failures.push(`${id}: phase_0_qualified is true but verification.json has no such Phase 0 gate`); continue; }
      const file = typeof gate.receipt === 'string' && gate.receipt.startsWith(`${receiptsPath}/`) ? gate.receipt.slice(receiptsPath.length + 1) : null;
      if (!file || file.includes('/') || !entries.includes(file)) {
        failures.push(`${id}: phase_0_qualified is true but its receipt (${gate.receipt ?? 'null'}) is not a file under ${receiptsPath}/`);
      }
    }
  }
  if (failures.length) throw new Error(`check-receipts failed:\n${failures.join('\n')}`);
  if (!entries.length) return `check-receipts: no receipts under ${receiptsPath}/; accepted only because verification.json records phase_0_qualified false.`;
  return `check-receipts: ${entries.length} receipt(s) valid against HEAD${qualified ? `; all ${libraryGates.length} Phase 0 library gates have one` : ''}.`;
}
```

- [ ] Mechanical edits in `scripts/dev.mjs`:
  - `:3` → `import { readFile } from 'node:fs/promises';`
  - delete `:175-240` (the doc comment and `async function checkReceipts()`).
  - add `import { checkReceipts } from './lib/receipts.mjs';` after the lanes import.
  - `case 'check-receipts'` → `    case 'check-receipts': process.stdout.write(`${await checkReceipts(root)}\n`); break;`

- [ ] Run it.
  `bun test ./tests/foundation/receipts.test.mjs` → ` 3 pass`, ` 0 fail`.
  `bun scripts/dev.mjs check-receipts` → `check-receipts: no receipts under
  qualification/receipts/; accepted only because verification.json records phase_0_qualified
  false.` and `git status --porcelain` prints nothing.

- [ ] Commit.
  `git add scripts/lib/receipts.mjs scripts/dev.mjs tests/foundation/receipts.test.mjs`

```text
fix(gates): require library receipts once Phase 0 is recorded as qualified.

Why: check-receipts passed on an empty directory unconditionally, so verification.json could say phase_0_qualified true with no evidence. Design section 2 outcome 2: receipts "pass check-receipts".
What changed: checkReceipts lives in scripts/lib/receipts.mjs. Empty is accepted only while phase_0_qualified is false; when true, the docling, iii and mcp-apps gates must each name an existing file under qualification/receipts/. Receipts need git_sha (40 lowercase hex), non-empty inputs and produced_at; the check no longer creates the directory.
Verified: bun test ./tests/foundation/receipts.test.mjs -> 3 pass, 0 fail; bun scripts/dev.mjs check-receipts -> accepted, tree unchanged.
Next: Task A.10, the lane gate.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.10: `dev.mjs lane <name>` — one gate per lane, logged, with a verdict line

**Files:**
- Create: `scripts/lib/gates.mjs`, `tests/foundation/gates.test.mjs`.
- Modify: `scripts/dev.mjs` (imports, `routes`, two cases); `ui/vitest.config.ts:9`.
- Test: `tests/foundation/gates.test.mjs`.

**Interfaces — produces:**

```js
// scripts/lib/gates.mjs
// Step = { id: string, command: string, args: string[], cwd: string }
export function laneSteps(root, lane);                 // Step[]
export async function executeStep(step, write);        // exit code; never rejects
export async function runSteps(steps, { logPath, failFast, execute = executeStep, echo });
//   -> { results: [{ id, code }], failed: string[], write }
export async function revisionLabel(root);             // '<40 hex>' or '<40 hex>-dirty'
export async function runLane(root, name, { execute, echo } = {});
//   -> { passed: boolean, line: string, logPath: string }
```

Rust lane steps, in order (fail-fast): `fmt` (`cargo fmt --all --check`); `clippy`
(`cargo clippy --locked -p <crate>... [--features <pkg/feature,...>] --all-targets -- -D
warnings`); `test` (`cargo test --locked -p <crate>... [--features ...]`); `source-policy`
(`cargo xtask source-policy --root <root>`); `scope` (`bun scripts/dev.mjs scope <lane>`).
UI lane steps: `biome` (`bun --bun run lint` in `ui/`); `routes` (`bun scripts/dev.mjs routes`);
`typecheck` (`bun --bun run typecheck` in `ui/`, which is `tsc -b && tsc -p tsconfig.tests.json
--noEmit`); `test` (`bun --bun run test <filters> [--exclude <glob>]...` in `ui/`); `scope`.

Log format: first line `log: <path>`; per step `=== <id>: <command> <args>`, its output,
`=== <id> exit <code>`; last line the verdict. `bun scripts/dev.mjs lane <name>` exits 1 on FAIL.

`bun scripts/dev.mjs routes` resolves the Vite config in `ui/`; the TanStack Router plugin
writes `src/routeTree.gen.ts` from its `configResolved` hook, so nothing is bundled. Prerequisite
of both tasks: `bun install --frozen-lockfile` has been run in this worktree.

- [ ] Write `tests/foundation/gates.test.mjs`:

```js
/** Gate sequences are data; the runner logs every step and ends with one verdict line. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { laneSteps, runLane } from '../../scripts/lib/gates.mjs';
import { laneNamed, lanes } from '../../scripts/lib/lanes.mjs';
import { fixtureRepo, git } from './fixture-repo.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');
const fixture={'.gitignore':'/.artifacts/\n','README.md':'fixture\n'};
const passing=async(step,write)=>{write(`ran ${step.id}\n`);return 0;};
const quiet={echo:()=>{}};
const lastLine=async path=>(await readFile(path,'utf8')).trimEnd().split('\n').at(-1);

test('a Rust lane gate is fmt, Clippy and tests for its crates and features, source policy, then scope',()=>{
 const steps=laneSteps('/repo',laneNamed('storage'));
 assert.deepEqual(steps.map(step=>step.id),['fmt','clippy','test','source-policy','scope']);
 assert.deepEqual(steps.slice(0,4).map(step=>step.command),['cargo','cargo','cargo','cargo']);
 assert.deepEqual(steps[0].args,['fmt','--all','--check']);
 assert.deepEqual(steps[1].args,['clippy','--locked','-p','okf-jawn-storage','--features','okf-jawn-storage/runtime','--all-targets','--','-D','warnings']);
 assert.deepEqual(steps[2].args,['test','--locked','-p','okf-jawn-storage','--features','okf-jawn-storage/runtime']);
 assert.deepEqual(steps[3].args,['xtask','source-policy','--root','/repo']);
 assert.deepEqual(steps[4].args,['scripts/dev.mjs','scope','storage']);
 const core=laneSteps('/repo',laneNamed('core-cli'));
 assert.deepEqual(core[1].args,['clippy','--locked','-p','okf-jawn-core','-p','okf-jawn-cli','--all-targets','--','-D','warnings']);
 assert.deepEqual(core[2].args,['test','--locked','-p','okf-jawn-core','-p','okf-jawn-cli']);
});
test('a UI lane gate is Biome, route generation, tsc, Vitest filtered to the lane, then scope',async()=>{
 const views=laneSteps('/repo',laneNamed('views'));
 assert.deepEqual(views.map(step=>step.id),['biome','routes','typecheck','test','scope']);
 assert.deepEqual(views[0].args,['--bun','run','lint']);
 assert.deepEqual(views[1].args,['scripts/dev.mjs','routes']);
 assert.deepEqual(views[2].args,['--bun','run','typecheck']);
 assert.deepEqual(views[3].args,['--bun','run','test',...laneNamed('views').tests]);
 assert.ok(views[3].cwd.endsWith('ui'));
 const workspace=laneSteps('/repo',laneNamed('workspace-ui'))[3].args;
 assert.deepEqual(workspace.slice(0,5),['--bun','run','test','--exclude','src/features/views/**']);
 assert.equal(workspace.filter(argument=>argument==='--exclude').length,laneNamed('workspace-ui').testExclude.length);
 const config=await read('ui/vitest.config.ts');
 for(const glob of ['tests/unit/**/*.test.{ts,tsx}','src/**/*.test.{ts,tsx}'])assert.ok(config.includes(`'${glob}'`),`ui/vitest.config.ts does not include ${glob}`);
 const entry=await read('scripts/dev.mjs');
 assert.match(entry,/resolveConfig\(\{\}, 'build'\)/);assert.match(entry,/case 'routes':/);
});
test('every lane has a gate, and the lane task ends its log with one verdict line',async t=>{
 for(const lane of lanes)assert.equal(laneSteps('/repo',lane).at(-1).id,'scope',lane.name);
 const {root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const result=await runLane(repo,'storage',{execute:passing,...quiet});
 assert.equal(result.passed,true);assert.equal(result.line,`PASS storage ${sha}`);
 assert.ok(result.logPath.endsWith(join('.artifacts','lane','storage',`${sha}.log`)));
 const log=await readFile(result.logPath,'utf8');
 assert.equal(await lastLine(result.logPath),`PASS storage ${sha}`);
 for(const id of ['fmt','clippy','test','source-policy','scope'])assert.ok(log.includes(`=== ${id}: `)&&log.includes(`ran ${id}\n=== ${id} exit 0\n`),id);
});
test('a failing step stops the lane gate and is named; an uncommitted tree is never labelled as its commit',async t=>{
 const {root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const ran=[];
 const failing=async(step,write)=>{ran.push(step.id);write(`ran ${step.id}\n`);return step.id==='clippy'?101:0;};
 const result=await runLane(repo,'storage',{execute:failing,...quiet});
 assert.equal(result.passed,false);assert.equal(result.line,`FAIL storage ${sha} clippy`);
 assert.deepEqual(ran,['fmt','clippy']);
 assert.equal(await lastLine(result.logPath),`FAIL storage ${sha} clippy`);
 await writeFile(join(repo,'README.md'),'edited, not committed\n');
 assert.equal((await runLane(repo,'storage',{execute:passing,...quiet})).line,`PASS storage ${sha}-dirty`);
 await assert.rejects(runLane(repo,'no-such-lane',{execute:passing,...quiet}),/Unknown lane: no-such-lane/);
});
```

- [ ] Run it.
  `bun test ./tests/foundation/gates.test.mjs`
  Expected: fails to load with `Cannot find module '../../scripts/lib/gates.mjs'`.

- [ ] Write `scripts/lib/gates.mjs`:

```js
/** Gate sequences as data (lane, premerge, clean checkout) and the one runner that logs them. */
import { appendFileSync, writeFileSync } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { run } from './process.mjs';
import { bun } from './toolchain.mjs';
import { laneNamed } from './lanes.mjs';

const dev = (root, id, ...task) => ({ id, command: bun(), args: ['scripts/dev.mjs', ...task], cwd: root });
const uiScript = (root, id, script, ...extra) => ({ id, command: bun(), args: ['--bun', 'run', script, ...extra], cwd: join(root, 'ui') });
const cargo = (root, id, ...args) => ({ id, command: 'cargo', args, cwd: root });

/** The gate of one lane: its own crates and features, or its own UI specs, then the scope check. */
export function laneSteps(root, lane) {
  if (lane.kind === 'ui') {
    return [
      uiScript(root, 'biome', 'lint'),
      dev(root, 'routes', 'routes'),
      uiScript(root, 'typecheck', 'typecheck'),
      uiScript(root, 'test', 'test', ...lane.tests, ...lane.testExclude.flatMap(glob => ['--exclude', glob])),
      dev(root, 'scope', 'scope', lane.name),
    ];
  }
  const packages = lane.crates.flatMap(name => ['-p', name]);
  const features = lane.features.length ? ['--features', lane.features.join(',')] : [];
  return [
    cargo(root, 'fmt', 'fmt', '--all', '--check'),
    cargo(root, 'clippy', 'clippy', '--locked', ...packages, ...features, '--all-targets', '--', '-D', 'warnings'),
    cargo(root, 'test', 'test', '--locked', ...packages, ...features),
    cargo(root, 'source-policy', 'xtask', 'source-policy', '--root', root),
    dev(root, 'scope', 'scope', lane.name),
  ];
}

/** Run one step with its live output sent to `write`. The exit code is the result, never an exception. */
export async function executeStep(step, write) {
  try {
    return (await run(step.command, step.args, { cwd: step.cwd, allowFailure: true, tee: chunk => write(chunk) })).code;
  } catch (error) {
    write(`${error.message}\n`);
    return 1;
  }
}

/**
 * Run steps in order and log them to `logPath`. With `failFast` the first failure ends the run;
 * without it every step runs, so one failure cannot hide the rest.
 */
export async function runSteps(steps, { logPath, failFast, execute = executeStep, echo = chunk => process.stdout.write(chunk) }) {
  await mkdir(dirname(logPath), { recursive: true });
  writeFileSync(logPath, '');
  const write = chunk => { appendFileSync(logPath, chunk); echo(chunk); };
  write(`log: ${logPath}\n`);
  const results = [];
  for (const step of steps) {
    write(`\n=== ${step.id}: ${step.command} ${step.args.join(' ')}\n`);
    const code = await execute(step, write);
    write(`=== ${step.id} exit ${code}\n`);
    results.push({ id: step.id, code });
    if (code !== 0 && failFast) break;
  }
  return { results, failed: results.filter(result => result.code !== 0).map(result => result.id), write };
}

/** HEAD, suffixed `-dirty` when the tree has uncommitted changes, so a log never claims a commit it did not test. */
export async function revisionLabel(root) {
  const head = (await run('git', ['rev-parse', 'HEAD'], { cwd: root, capture: true })).stdout.trim();
  const status = (await run('git', ['status', '--porcelain'], { cwd: root, capture: true })).stdout.trim();
  return status ? `${head}-dirty` : head;
}

export async function runLane(root, name, { execute, echo } = {}) {
  const lane = laneNamed(name);
  const label = await revisionLabel(root);
  const logPath = join(root, '.artifacts', 'lane', lane.name, `${label}.log`);
  const { failed, write } = await runSteps(laneSteps(root, lane), { logPath, failFast: true, execute, echo });
  const line = failed.length ? `FAIL ${lane.name} ${label} ${failed[0]}` : `PASS ${lane.name} ${label}`;
  write(`\n${line}\n`);
  return { passed: failed.length === 0, line, logPath };
}
```

- [ ] Mechanical edits:
  - `ui/vitest.config.ts:9` → `    include: ['tests/unit/**/*.test.{ts,tsx}', 'src/**/*.test.{ts,tsx}'],`
  - `scripts/dev.mjs:3` → `import { readFile, rm } from 'node:fs/promises';`
  - `scripts/dev.mjs`: add `import { runLane } from './lib/gates.mjs';` after the receipts import.
  - `scripts/dev.mjs`: add after `generatedStrictProbe()`:
    ```js
    /**
     * Write ui/src/routeTree.gen.ts without bundling. The TanStack Router Vite plugin generates
     * the tree from its configResolved hook, so resolving the Vite config is enough. The plugin
     * logs and swallows generator errors, so the file is removed first and required afterwards.
     */
    async function routes() {
      const target = join(ui, 'src', 'routeTree.gen.ts');
      await rm(target, { force: true });
      await run(bun(), ['-e', "const { resolveConfig } = await import('vite'); await resolveConfig({}, 'build');"], { cwd: ui });
      if (!await exists(target)) throw new Error('Route tree was not generated: ui/src/routeTree.gen.ts is missing after resolving the Vite config.');
      process.stdout.write('Generated ui/src/routeTree.gen.ts.\n');
    }
    ```
  - `scripts/dev.mjs` switch: add after `case 'gen-check'`:
    `    case 'routes': await routes(); break;`
    and after the `scope` case:
    ```js
        case 'lane': {
          const result = await runLane(root, positional[0]);
          if (!result.passed) process.exitCode = 1;
          break;
        }
    ```

- [ ] Run it.
  `bun test ./tests/foundation/gates.test.mjs` → ` 4 pass`, ` 0 fail`.

- [ ] Exercise the real route step (needs `bun install --frozen-lockfile` once in this worktree):
  `bun install --frozen-lockfile`
  `bun scripts/dev.mjs routes` → `Generated ui/src/routeTree.gen.ts.`
  `bun --bun run --cwd ui typecheck` → exit 0 with no prior `vite build`.
  `bun --bun run --cwd ui test` → the same number of passing test files as before the include
  change (8), exit 0.
  `git status --porcelain` → only the files of this task (the route tree is ignored).

- [ ] Commit.
  `git add scripts/lib/gates.mjs scripts/dev.mjs tests/foundation/gates.test.mjs ui/vitest.config.ts`

```text
feat(gates): add the per-lane gate with a logged verdict.

Why: each lane's gate was a bare `cargo test` or `vitest` line in AGENTS.md: no fmt, no Clippy, no --all-targets, no source policy, no scope check, and UI lanes could not type check without a full build. Design section 5, package A.
What changed: `bun scripts/dev.mjs lane <name>` runs fmt, Clippy and tests for the lane's crates and features, source policy and the scope check (UI lanes: Biome, `routes`, tsc, Vitest filtered to the lane), stops at the first failure, writes .artifacts/lane/<name>/<label>.log and ends with PASS or FAIL naming the step; the label is HEAD, with -dirty when the tree is not committed. `routes` writes the route tree by resolving the Vite config. Vitest also collects src/**/*.test.{ts,tsx}.
Verified: bun test ./tests/foundation/gates.test.mjs -> 4 pass, 0 fail; bun scripts/dev.mjs routes then bun --bun run --cwd ui typecheck -> exit 0 without a build; bun --bun run --cwd ui test -> 8 files pass.
Next: Task A.11, premerge.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.11: `dev.mjs premerge` — the CI sequence, every step, all features

**Files:**
- Modify: `scripts/lib/gates.mjs` (append); `scripts/dev.mjs:275-284` (`check`, `test` cases),
  import, one helper, one case.
- Test: `tests/foundation/gates.test.mjs` (append).

**Interfaces — produces:**

```js
export function premergeSteps(root);      // Step[], ids in this order:
// check-offline, gen-check, fmt, clippy, source-policy, test, ui-lint, ui-build,
// ui-typecheck, ui-test, check-receipts
export async function runPremerge(root, { only, execute, echo } = {});
//   only undefined -> every step, never fail-fast; { passed, line, logPath }
//   only '<id>'    -> that step alone with inherited stdio; rejects on a non-zero exit
```

`clippy` is `cargo clippy --locked --workspace --all-features --all-targets -- -D warnings`;
`test` is `cargo test --locked --workspace --all-features`. Bootstrap prerequisites are assumed
(`bun scripts/dev.mjs bootstrap` has run in this checkout). The `check` and `test` tasks become
subsets of these steps, so the flags are defined once.

- [ ] Append to `tests/foundation/gates.test.mjs` (extend the gates import as shown):

```js
import { laneSteps, premergeSteps, runLane, runPremerge } from '../../scripts/lib/gates.mjs';
```

```js
test('premerge is the whole CI sequence with every feature, and includes check-receipts',()=>{
 const steps=premergeSteps('/repo');
 assert.deepEqual(steps.map(step=>step.id),['check-offline','gen-check','fmt','clippy','source-policy','test','ui-lint','ui-build','ui-typecheck','ui-test','check-receipts']);
 const args=id=>steps.find(step=>step.id===id).args;
 assert.deepEqual(args('clippy'),['clippy','--locked','--workspace','--all-features','--all-targets','--','-D','warnings']);
 assert.deepEqual(args('test'),['test','--locked','--workspace','--all-features']);
 assert.deepEqual(args('gen-check'),['scripts/dev.mjs','gen-check']);
 assert.deepEqual(args('check-receipts'),['scripts/dev.mjs','check-receipts']);
});
test('premerge runs every step and reports all failures, not the first',async t=>{
 const {root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const ran=[];
 const execute=async(step,write)=>{ran.push(step.id);write(`ran ${step.id}\n`);return ['fmt','ui-test'].includes(step.id)?1:0;};
 const result=await runPremerge(repo,{execute,...quiet});
 assert.deepEqual(ran,premergeSteps(repo).map(step=>step.id));
 assert.equal(result.passed,false);assert.equal(result.line,`FAIL premerge ${sha} fmt,ui-test`);
 assert.equal(await lastLine(result.logPath),`FAIL premerge ${sha} fmt,ui-test`);
 assert.equal((await runPremerge(repo,{execute:passing,...quiet})).line,`PASS premerge ${sha}`);
 await assert.rejects(runPremerge(repo,{only:'no-such-step'}),/Unknown premerge step: no-such-step/);
});
test('check and test are subsets of premerge, so the cargo flags are written once',async()=>{
 const entry=await read('scripts/dev.mjs');
 assert.match(entry,/case 'check': await runNamed\(\['fmt', 'clippy', 'source-policy', 'ui-lint', 'ui-typecheck', 'gen-check'\]\); break;/);
 assert.match(entry,/case 'test': await runNamed\(\['test', 'ui-test'\]\); break;/);
 assert.doesNotMatch(entry,/'--all-targets'|'--all-features'/);
});
```

- [ ] Run it.
  `bun test ./tests/foundation/gates.test.mjs`
  Expected: fails to load with `Export named 'premergeSteps' not found in module`.

- [ ] Append to `scripts/lib/gates.mjs`:

```js
/**
 * Everything CI runs after bootstrap, in CI's order, with every feature. CI executes these one
 * step at a time (`premerge --step <id>`); tests/foundation/ci.test.mjs keeps the two identical.
 */
export function premergeSteps(root) {
  return [
    dev(root, 'check-offline', 'check-offline'),
    dev(root, 'gen-check', 'gen-check'),
    cargo(root, 'fmt', 'fmt', '--all', '--check'),
    cargo(root, 'clippy', 'clippy', '--locked', '--workspace', '--all-features', '--all-targets', '--', '-D', 'warnings'),
    cargo(root, 'source-policy', 'xtask', 'source-policy', '--root', root),
    cargo(root, 'test', 'test', '--locked', '--workspace', '--all-features'),
    uiScript(root, 'ui-lint', 'lint'),
    uiScript(root, 'ui-build', 'build'),
    uiScript(root, 'ui-typecheck', 'typecheck'),
    uiScript(root, 'ui-test', 'test'),
    dev(root, 'check-receipts', 'check-receipts'),
  ];
}

export async function runPremerge(root, { only, execute, echo } = {}) {
  const steps = premergeSteps(root);
  if (only !== undefined) {
    const step = steps.find(entry => entry.id === only);
    if (!step) throw new Error(`Unknown premerge step: ${only}. Steps: ${steps.map(entry => entry.id).join(', ')}.`);
    await run(step.command, step.args, { cwd: step.cwd });
    return { passed: true, line: `PASS premerge ${only}`, logPath: null };
  }
  const label = await revisionLabel(root);
  const logPath = join(root, '.artifacts', 'premerge', `${label}.log`);
  const { failed, write } = await runSteps(steps, { logPath, failFast: false, execute, echo });
  const line = failed.length ? `FAIL premerge ${label} ${failed.join(',')}` : `PASS premerge ${label}`;
  write(`\n${line}\n`);
  return { passed: failed.length === 0, line, logPath };
}
```

- [ ] Mechanical edits in `scripts/dev.mjs`:
  - the gates import → `import { premergeSteps, runLane, runPremerge } from './lib/gates.mjs';`
  - add before `async function main()`:
    ```js
    /** Run named premerge steps in order, stopping at the first failure. */
    async function runNamed(ids) {
      const steps = premergeSteps(root);
      for (const id of ids) {
        const step = steps.find(entry => entry.id === id);
        if (!step) throw new Error(`No premerge step named ${id}.`);
        await run(step.command, step.args, { cwd: step.cwd });
      }
    }
    ```
  - replace `:275-284` (the `check` and `test` cases, ten lines) with:
    ```js
        case 'check': await runNamed(['fmt', 'clippy', 'source-policy', 'ui-lint', 'ui-typecheck', 'gen-check']); break;
        case 'test': await runNamed(['test', 'ui-test']); break;
    ```
  - add after the `lane` case:
    ```js
        case 'premerge': {
          const result = await runPremerge(root, { only: option('--step') });
          if (!result.passed) process.exitCode = 1;
          break;
        }
    ```

- [ ] Run it.
  `bun test ./tests/foundation/gates.test.mjs` → ` 7 pass`, ` 0 fail`.
  `bun scripts/dev.mjs premerge --step check-offline` → the offline suite runs, exit 0.
  `bun scripts/dev.mjs premerge --step nope` → `Unknown premerge step: nope. Steps: …`, exit 1.

- [ ] Commit.
  `git add scripts/lib/gates.mjs scripts/dev.mjs tests/foundation/gates.test.mjs`

```text
feat(gates): add premerge, the CI sequence run locally with every feature.

Why: nothing local reproduced CI; `check` and `test` never enabled the runtime features, so storage, ingest and server code was linted only by a separate CI job and never tested; check-receipts was called by nothing.
What changed: `bun scripts/dev.mjs premerge` runs check-offline, gen-check, fmt, Clippy and tests with --workspace --all-features, source policy, UI lint, build, typecheck and tests, and check-receipts; it never stops early, logs to .artifacts/premerge/<label>.log and ends with PASS or FAIL listing every failed step. `--step <id>` runs one step for CI. `check` and `test` are now subsets of the same step list.
Verified: bun test ./tests/foundation/gates.test.mjs -> 7 pass, 0 fail; bun scripts/dev.mjs premerge --step check-offline -> exit 0.
Next: Task A.12, the workflow.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.12: CI runs premerge step by step, caches, cancels, gates lanes, audits separately

**Files:**
- Create: `tests/foundation/ci.test.mjs`.
- Modify: `.github/workflows/ci.yml` (whole file); `.github/workflows/qualify.yml:15,26`;
  `.github/workflows/contract.yml:16`; `vendors.json:342`.
- Test: `tests/foundation/ci.test.mjs`.

**Interfaces:**
- Consumes: `premergeSteps(root)` ids; `bun scripts/dev.mjs premerge --step <id>`;
  `bun scripts/dev.mjs lane <name>`; `Bun.YAML.parse` (present in the pinned Bun 1.4.2).
- Produces: jobs `source-tooling`, `foundation`, `lane`, `audit`; no `runtime-clippy`.

Action pins. `actions/checkout@11bd7190…` (v4.2.2) and `actions/upload-artifact@ea165f8d…`
(v4.6.2) declare `using: node20`. Pins observed on 2026-10-05:

| Action | Tag | Commit | `using` |
| --- | --- | --- | --- |
| `actions/checkout` | v7.0.1 | `3d3c42e5aac5ba805825da76410c181273ba90b1` | node24 |
| `actions/upload-artifact` | v7.0.1 | `043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` | node24 |
| `actions/cache` | v6.1.0 | `55cc8345863c7cc4c66a329aec7e433d2d1c52a9` | node24 |

- [ ] Re-derive each pin before using it (either shell). For each of `actions/checkout`,
  `actions/upload-artifact`, `actions/cache`:

```sh
git ls-remote --tags https://github.com/actions/checkout | grep -E 'refs/tags/v[0-9]+\.[0-9]+\.[0-9]+(\^\{\})?$' | sort -t/ -k3 -V | tail -3
gh api "repos/actions/checkout/contents/action.yml?ref=<commit>" --jq .content | base64 -d | grep 'using:'
```
  Take the highest `vX.Y.Z`; when a `^{}` line exists for it, that line's commit is the one to
  pin. The second command must print a runtime other than `node20`. If a newer tag than the table
  exists, use it and write the tag you used in the trailing `# vX.Y.Z` comment and in the commit
  message.

- [ ] Write `tests/foundation/ci.test.mjs`:

```js
/** The workflow runs the premerge sequence step by step; this file keeps the two from drifting. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { premergeSteps } from '../../scripts/lib/gates.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');
const workflow=Bun.YAML.parse(await read('.github/workflows/ci.yml'));
const always='${{ !cancelled() }}';
const prefix='bun scripts/dev.mjs premerge --step ';
// Commits whose action.yml declares `using: node20`: checkout v4.2.2, upload-artifact v4.6.2.
const node20Pins=['11bd71901bbe5b1630ceea73d27597364c9af683','ea165f8d65b6e75b540449e92b4886f43607fa02'];
const checkout=job=>workflow.jobs[job].steps.find(step=>step.uses?.startsWith('actions/checkout@'));

test('CI runs exactly the premerge sequence, one step each, and a failure does not hide later steps',()=>{
 const steps=workflow.jobs.foundation.steps;
 const gated=steps.filter(step=>typeof step.run==='string'&&step.run.startsWith(prefix));
 assert.deepEqual(gated.map(step=>step.run.slice(prefix.length)),premergeSteps(root).map(step=>step.id));
 for(const step of gated)assert.equal(step.if,always,step.run);
 const bootstrap=steps.findIndex(step=>step.run==='bun scripts/dev.mjs bootstrap');
 assert.ok(bootstrap>=0&&bootstrap<steps.indexOf(gated[0]),'bootstrap must precede the gated steps');
 assert.equal(steps.find(step=>step.uses?.startsWith('actions/upload-artifact@')).if,always);
 assert.equal(checkout('foundation').with['fetch-depth'],0,'check-receipts compares against history');
});
test('Clippy and tests cover every feature, so no separate runtime job exists',()=>{
 for(const id of ['clippy','test']){
  const args=premergeSteps(root).find(step=>step.id===id).args;
  assert.ok(args.includes('--workspace')&&args.includes('--all-features'),id);
 }
 assert.equal(workflow.jobs['runtime-clippy'],undefined);
});
test('superseded runs are cancelled, cargo and bun are cached, and no action runs on Node 20',async()=>{
 assert.match(workflow.concurrency.group,/github\.ref/);
 assert.ok('cancel-in-progress' in workflow.concurrency);
 for(const job of ['foundation','lane']){
  const caches=workflow.jobs[job].steps.filter(step=>step.uses?.startsWith('actions/cache@')).map(step=>step.with.path);
  assert.ok(caches.some(path=>path.includes('~/.cargo/registry')&&/^target$/m.test(path)),`${job}: no cargo cache`);
  assert.ok(caches.some(path=>path.includes('~/.bun/install/cache')),`${job}: no bun cache`);
 }
 for(const file of (await readdir(join(root,'.github/workflows'))).filter(name=>name.endsWith('.yml'))){
  const uses=[...(await read(`.github/workflows/${file}`)).matchAll(/^\s*-?\s*uses:\s*(\S+)/gm)].map(match=>match[1]);
  assert.ok(uses.length>0,file);
  for(const action of uses){
   assert.match(action,/^[\w.-]+\/[\w.-]+@[0-9a-f]{40}$/,`${file}: ${action} is not pinned to a commit`);
   assert.ok(!node20Pins.includes(action.split('@')[1]),`${file}: ${action} runs on Node 20`);
  }
 }
});
test('lane branches run their own gate; audit runs on main and on a schedule, not in the foundation job',()=>{
 const lane=workflow.jobs.lane;
 assert.match(lane.if,/startsWith\(github\.ref, 'refs\/heads\/build\/'\)/);
 assert.ok(lane.steps.some(step=>step.run?.includes('bun scripts/dev.mjs lane "${GITHUB_REF_NAME#build/}"')));
 assert.equal(checkout('lane').with['fetch-depth'],0,'the scope check compares against origin/main');
 const audit=workflow.jobs.audit;
 assert.match(audit.if,/schedule/);assert.match(audit.if,/refs\/heads\/main/);
 assert.ok(audit.steps.some(step=>step.run==='bun scripts/dev.mjs audit'));
 assert.ok(!workflow.jobs.foundation.steps.some(step=>step.run?.includes('audit')));
 assert.ok(Array.isArray(workflow.on.schedule)&&workflow.on.schedule.length===1);
 for(const job of ['source-tooling','foundation'])assert.match(workflow.jobs[job].if,/github\.event_name != 'schedule'/,job);
});
```

- [ ] Run it.
  `bun test ./tests/foundation/ci.test.mjs`
  Expected: 4 fail; the first with an empty actual list against the eleven premerge ids.

- [ ] Replace `.github/workflows/ci.yml` with:

```yaml
name: Source and foundation diagnostics
on:
  push:
  pull_request:
  workflow_dispatch:
  schedule:
    - cron: "23 5 * * *"
permissions:
  contents: read
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: ${{ github.ref != 'refs/heads/main' }}
jobs:
  source-tooling:
    name: Dependency-free source tooling, not product acceptance
    if: github.event_name != 'schedule'
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - uses: oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6 # v2.2.0
        with:
          bun-version-file: .bun-version
      - run: bun scripts/dev.mjs check-offline
  foundation:
    name: Actual pinned generators and consumers, every feature
    if: github.event_name != 'schedule'
    runs-on: ubuntu-24.04
    timeout-minutes: 90
    env:
      SCARF_ANALYTICS: "false"
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
          fetch-depth: 0
      - uses: oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6 # v2.2.0
        with:
          bun-version-file: .bun-version
      - name: Install the toolchain selected by rust-toolchain.toml
        run: rustup toolchain install
      - uses: actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0
        with:
          path: |
            ~/.cargo/registry/index
            ~/.cargo/registry/cache
            ~/.cargo/git/db
            target
          key: cargo-${{ runner.os }}-${{ hashFiles('rust-toolchain.toml', 'Cargo.lock') }}
          restore-keys: |
            cargo-${{ runner.os }}-
      - uses: actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0
        with:
          path: ~/.bun/install/cache
          key: bun-${{ runner.os }}-${{ hashFiles('.bun-version', 'bun.lock') }}
      - run: bun scripts/dev.mjs bootstrap
      # One step per premerge step, in premerge order (tests/foundation/ci.test.mjs holds them
      # equal). Each runs unless the run was cancelled, so one failure cannot hide the others.
      - if: ${{ !cancelled() }}
        run: bun scripts/dev.mjs premerge --step check-offline
      - if: ${{ !cancelled() }}
        run: bun scripts/dev.mjs premerge --step gen-check
      - if: ${{ !cancelled() }}
        run: bun scripts/dev.mjs premerge --step fmt
      - if: ${{ !cancelled() }}
        run: bun scripts/dev.mjs premerge --step clippy
      - if: ${{ !cancelled() }}
        run: bun scripts/dev.mjs premerge --step source-policy
      - if: ${{ !cancelled() }}
        run: bun scripts/dev.mjs premerge --step test
      - if: ${{ !cancelled() }}
        run: bun scripts/dev.mjs premerge --step ui-lint
      - if: ${{ !cancelled() }}
        run: bun scripts/dev.mjs premerge --step ui-build
      - if: ${{ !cancelled() }}
        run: bun scripts/dev.mjs premerge --step ui-typecheck
      - if: ${{ !cancelled() }}
        run: bun scripts/dev.mjs premerge --step ui-test
      - if: ${{ !cancelled() }}
        run: bun scripts/dev.mjs premerge --step check-receipts
      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        if: ${{ !cancelled() }}
        with:
          name: resolved-foundation
          if-no-files-found: error
          path: |
            Cargo.lock
            bun.lock
            api/
            generated/cli/
            ui/src/api/generated/
            ui/dist/
            ui/dist-apps/
  lane:
    name: Lane gate for build/* branches
    if: github.event_name == 'push' && startsWith(github.ref, 'refs/heads/build/')
    runs-on: ubuntu-24.04
    timeout-minutes: 90
    env:
      SCARF_ANALYTICS: "false"
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
          fetch-depth: 0
      - uses: oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6 # v2.2.0
        with:
          bun-version-file: .bun-version
      - name: Install the toolchain selected by rust-toolchain.toml
        run: rustup toolchain install
      - uses: actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0
        with:
          path: |
            ~/.cargo/registry/index
            ~/.cargo/registry/cache
            ~/.cargo/git/db
            target
          key: cargo-lane-${{ github.ref_name }}-${{ hashFiles('rust-toolchain.toml', 'Cargo.lock') }}
          restore-keys: |
            cargo-lane-${{ github.ref_name }}-
      - uses: actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0
        with:
          path: ~/.bun/install/cache
          key: bun-${{ runner.os }}-${{ hashFiles('.bun-version', 'bun.lock') }}
      - run: bun install --frozen-lockfile
      - name: Lane gate
        run: bun scripts/dev.mjs lane "${GITHUB_REF_NAME#build/}"
      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        if: ${{ !cancelled() }}
        with:
          name: lane-log
          if-no-files-found: warn
          include-hidden-files: true
          path: .artifacts/lane/
  audit:
    name: Dependency advisories and licences
    if: github.event_name == 'schedule' || github.event_name == 'workflow_dispatch' || github.ref == 'refs/heads/main'
    runs-on: ubuntu-24.04
    timeout-minutes: 20
    env:
      SCARF_ANALYTICS: "false"
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - uses: oven-sh/setup-bun@0c5077e51419868618aeaa5fe8019c62421857d6 # v2.2.0
        with:
          bun-version-file: .bun-version
      - name: Install the toolchain selected by rust-toolchain.toml
        run: rustup toolchain install
      - uses: taiki-e/install-action@183e4297cca2404691e9380e1307288dced5c82a # v2.87.25
        with:
          tool: cargo-audit@0.22.2,cargo-deny@0.20.2
      - run: bun install --frozen-lockfile
      - run: bun scripts/dev.mjs audit
```

- [ ] Mechanical edits:
  - `.github/workflows/qualify.yml:15` →
    `      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1`
  - `.github/workflows/qualify.yml:26` →
    `      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1`
    and add `          include-hidden-files: true` under its `with:` (the path is `.artifacts/`).
  - `.github/workflows/contract.yml:16` →
    `      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1`
  - `vendors.json:342` →
    `        "actions/checkout pin: v7.0.1 = 3d3c42e5aac5ba805825da76410c181273ba90b1 (node24). v4.2.2 (11bd7190) and actions/upload-artifact v4.6.2 (ea165f8d) declare the deprecated node20 runtime; tests/foundation/ci.test.mjs rejects both."`

- [ ] Run it.
  `bun test ./tests/foundation/ci.test.mjs` → ` 4 pass`, ` 0 fail`.
  `bun test ./tests/foundation/toolchain.test.mjs` → ` 0 fail` (it still finds
  `oven-sh/setup-bun@<40 hex>` and `bun-version-file: .bun-version`).
  Mutation proof: delete the `premerge --step fmt` step from ci.yml, run the first command →
  `(fail) CI runs exactly the premerge sequence…`; restore.

- [ ] Commit.
  `git add .github/workflows/ci.yml .github/workflows/qualify.yml .github/workflows/contract.yml vendors.json tests/foundation/ci.test.mjs`

```text
ci(gates): run premerge step by step with caches, cancellation, a lane job and a separate audit.

Why: in run 37352882339 the failing foundation step hid fmt, Clippy and the tests behind it; tests and Clippy never enabled runtime features; nothing was cached; superseded runs kept running; build/* branches had no gate; audit ran on every push; checkout v4.2.2 and upload-artifact v4.6.2 run on the deprecated Node 20 runtime.
What changed: the foundation job runs bootstrap, then one `premerge --step <id>` per premerge step, each with `if: !cancelled()`; cargo and bun caches; a concurrency group that cancels superseded runs except on main; a `lane` job on build/** that runs `dev.mjs lane <name>`; `audit` in its own job on main, on a daily schedule and on dispatch; runtime-clippy removed because --all-features covers it. checkout v7.0.1, upload-artifact v7.0.1 and cache v6.1.0 are pinned by commit in all three workflows.
Verified: bun test ./tests/foundation/ci.test.mjs -> 4 pass, 0 fail; removing one premerge step from ci.yml fails it. The workflow itself is verified by the first push of the integration branch.
Next: Task A.13, tracked hooks.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.13: Tracked hooks installed through `core.hooksPath`

**Files:**
- Create: `scripts/hooks/pre-commit`, `scripts/hooks/pre-push`, `tests/foundation/hooks.test.mjs`.
- Modify: `scripts/lib/init.mjs` (whole file); `scripts/dev.mjs:9`, `:59-61` (`bootstrap`);
  `tests/foundation/init.test.mjs` (append); `tests/foundation/toolchain.test.mjs:35`;
  `README.md:30`, `README.md:43`.
- Delete: `lefthook.yml`.
- Test: `tests/foundation/hooks.test.mjs`, `tests/foundation/init.test.mjs`.

**Interfaces — produces:**

```js
// scripts/lib/init.mjs
export async function installHooks(root);   // true when core.hooksPath was set to 'scripts/hooks'; false without .git or without scripts/hooks
export async function initialize(root);     // { repository, remoteCreated: false, committed: false, hooksInstalled: boolean }
```

`pre-commit`: `bun scripts/dev.mjs check-offline`. `pre-push`: `check-offline`,
`cargo fmt --all --check`, and `bun scripts/dev.mjs scope` when the current branch is `build/*`
or `cure/*`. Both first run `unset $(git rev-parse --local-env-vars)`. `core.hooksPath` lives in
the shared repository configuration, so one `bootstrap` installs the hooks for every worktree
whose checkout contains `scripts/hooks/`; `cargo fmt` does not link, so it is safe from Git's
`sh` on Windows.

- [ ] Write `tests/foundation/hooks.test.mjs`:

```js
/** Tracked hooks: POSIX sh, installed through core.hooksPath, never a tool that is not installed. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run } from '../../scripts/lib/process.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');

test('tracked hooks are POSIX sh, drop the hook environment, and run the agreed checks',async()=>{
 const commitHook=await read('scripts/hooks/pre-commit'),pushHook=await read('scripts/hooks/pre-push');
 for(const [name,source] of [['pre-commit',commitHook],['pre-push',pushHook]]){
  assert.ok(source.startsWith('#!/bin/sh\n'),`${name} must start with #!/bin/sh`);
  assert.ok(!source.includes('\r'),`${name} must use LF line endings`);
  assert.match(source,/^set -eu$/m,name);
  assert.match(source,/^unset \$\(git rev-parse --local-env-vars\)$/m,`${name} must drop GIT_DIR and GIT_INDEX_FILE before running tests`);
  assert.match(source,/^bun scripts\/dev\.mjs check-offline$/m,name);
  assert.doesNotMatch(source,/\[\[|\bfunction\b|<<<|\blefthook\b/,`${name} uses a non-POSIX construct or an uninstalled tool`);
 }
 assert.doesNotMatch(commitHook,/cargo/,'pre-commit stays dependency-free');
 assert.match(pushHook,/^cargo fmt --all --check$/m);
 assert.match(pushHook,/build\/\*\|cure\/\*\) bun scripts\/dev\.mjs scope ;;/);
 await assert.rejects(read('lefthook.yml'),{code:'ENOENT'});
 // A source copy without history (the build container) has no index to inspect.
 if(!existsSync(join(root,'.git')))return;
 const modes=(await run('git',['ls-files','-s','scripts/hooks'],{cwd:root,capture:true})).stdout.trim().split(/\r?\n/);
 assert.equal(modes.length,2);
 for(const line of modes)assert.ok(line.startsWith('100755 '),`not executable in the index: ${line}`);
});
test('bootstrap and init install the hooks',async()=>{
 const entry=await read('scripts/dev.mjs');
 const bootstrap=entry.slice(entry.indexOf('async function bootstrap()'),entry.indexOf('async function vendor()'));
 assert.match(bootstrap,/await installHooks\(root\);/);
 assert.match(await read('scripts/lib/init.mjs'),/hooksInstalled: await installHooks\(root\)/);
});
```

- [ ] Append to `tests/foundation/init.test.mjs` (line 7 becomes
  `import { initialize, installHooks } from '../../scripts/lib/init.mjs';`):

```js
test('init points the repository at the tracked hooks and leaves a copy without history alone',async t=>{
 const root=await mkdtemp(join(tmpdir(),'okf-hooks-'));t.after(()=>rm(root,{recursive:true,force:true}));
 await mkdir(join(root,'scripts','hooks'),{recursive:true});
 assert.equal(await installHooks(root),false,'no .git: nothing to configure, and no error');
 assert.equal((await initialize(root)).hooksInstalled,true);
 assert.equal((await run('git',['config','--get','core.hooksPath'],{cwd:root,capture:true})).stdout.trim(),'scripts/hooks');
});
```

- [ ] Run them.
  `bun test ./tests/foundation/hooks.test.mjs ./tests/foundation/init.test.mjs`
  Expected: `init.test.mjs` fails to load with `Export named 'installHooks' not found in
  module`; `hooks.test.mjs` fails with `ENOENT … scripts/hooks/pre-commit`.

- [ ] Write `scripts/hooks/pre-commit` (LF line endings):

```sh
#!/bin/sh
# Tracked hook; `bun scripts/dev.mjs bootstrap` or `init` installs it through core.hooksPath.
# It runs only the dependency-free checks, never regenerates files, never touches another worktree.
set -eu
# Git exports GIT_DIR and GIT_INDEX_FILE to hooks. The offline tests create disposable
# repositories; with those variables set, their git commands would act on this repository.
unset $(git rev-parse --local-env-vars)
bun scripts/dev.mjs check-offline
```

- [ ] Write `scripts/hooks/pre-push` (LF line endings):

```sh
#!/bin/sh
# Tracked hook; `bun scripts/dev.mjs bootstrap` or `init` installs it through core.hooksPath.
# It never regenerates files and never touches another worktree.
set -eu
# Git exports GIT_DIR and GIT_INDEX_FILE to hooks. The offline tests create disposable
# repositories; with those variables set, their git commands would act on this repository.
unset $(git rev-parse --local-env-vars)
bun scripts/dev.mjs check-offline
cargo fmt --all --check
case "$(git rev-parse --abbrev-ref HEAD)" in
  build/*|cure/*) bun scripts/dev.mjs scope ;;
esac
```

- [ ] Replace `scripts/lib/init.mjs` with:

```js
/** Initialize a local repository without a remote, credentials, commit, or overwritten settings. */
import { copyFile } from 'node:fs/promises';
import { constants } from 'node:fs';
import { join } from 'node:path';
import { exists } from './files.mjs';
import { run } from './process.mjs';

/**
 * Point this repository at the tracked hooks under scripts/hooks. A source copy without `.git`
 * (the build container) or without the hooks has nothing to configure; that is not an error.
 */
export async function installHooks(root) {
  if (!await exists(join(root, '.git')) || !await exists(join(root, 'scripts', 'hooks'))) return false;
  await run('git', ['config', 'core.hooksPath', 'scripts/hooks'], { cwd: root, capture: true });
  return true;
}

export async function initialize(root) {
  if (!await exists(join(root, '.git'))) await run('git', ['init', '--initial-branch=main', root], { cwd: root });
  const sample = join(root, 'deploy', '.env.example');
  if (await exists(sample) && !await exists(join(root, '.env'))) {
    await copyFile(sample, join(root, '.env'), constants.COPYFILE_EXCL);
  }
  return { repository: root, remoteCreated: false, committed: false, hooksInstalled: await installHooks(root) };
}
```

- [ ] Mechanical edits:
  - `scripts/dev.mjs:9` → `import { initialize, installHooks } from './lib/init.mjs';`
  - `scripts/dev.mjs` `bootstrap()`: insert `  await installHooks(root);` after
    `  await requireLockfiles(root);`.
  - `tests/foundation/toolchain.test.mjs:35`: in the file list replace `'lefthook.yml'` with
    `'scripts/hooks/pre-commit','scripts/hooks/pre-push'`.
  - `README.md:30`: replace the first sentence with
    ``` `init` creates a local Git repository, copies the deployment environment example only when absent, and points `core.hooksPath` at the tracked `scripts/hooks/`. ```
  - `README.md`: after line 43 (the paragraph beginning `` `gen` generates into two temporary
    directories``) insert a blank line and this paragraph:
    ``` `bootstrap` and `init` install the tracked hooks through `core.hooksPath`; nothing else needs installing. pre-commit runs `check-offline`. pre-push runs `check-offline`, `cargo fmt --all --check` and, on a `build/*` or `cure/*` branch, the scope check. Hooks never regenerate files or touch another worktree. ```
  - `git rm lefthook.yml`

- [ ] Stage the hooks as executable before running the test (the test reads the index):
  `git add --chmod=+x scripts/hooks/pre-commit scripts/hooks/pre-push`
  `git ls-files -s scripts/hooks` → two lines, each starting `100755 `.

- [ ] Run them.
  `bun test ./tests/foundation/hooks.test.mjs ./tests/foundation/init.test.mjs ./tests/foundation/toolchain.test.mjs`
  → ` 0 fail`.
  `bun scripts/dev.mjs check-offline` → ` 0 fail`.

- [ ] Exercise the real hook once, from the worktree root:
  `sh scripts/hooks/pre-commit` → the offline suite runs, exit 0. (Do not run `bootstrap` only to
  install hooks; `core.hooksPath` is shared repository configuration and the orchestrator sets it
  when this package merges.)

- [ ] Commit.
  `git add scripts/lib/init.mjs scripts/dev.mjs tests/foundation/hooks.test.mjs tests/foundation/init.test.mjs tests/foundation/toolchain.test.mjs README.md`
  (`scripts/hooks/*` and the `lefthook.yml` deletion are already staged.)

```text
chore(gates): replace the uninstalled lefthook config with tracked hooks.

Why: lefthook.yml named a tool that is installed nowhere; .git/hooks held only samples and core.hooksPath was unset, so no hook ever ran. Design section 3, Hooks.
What changed: scripts/hooks/pre-commit runs check-offline; scripts/hooks/pre-push runs check-offline, cargo fmt --all --check and, on build/* or cure/* branches, the scope check. Both are POSIX sh, executable in the index, and drop Git's hook environment first. bootstrap and init run `git config core.hooksPath scripts/hooks`, and skip it in a copy without .git. lefthook.yml is deleted.
Verified: bun test hooks, init and toolchain tests -> 0 fail; sh scripts/hooks/pre-commit -> exit 0; git ls-files -s scripts/hooks -> 100755 twice.
Next: Task A.14, clean-checkout.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.14: `dev.mjs clean-checkout` — the whole foundation in a temporary worktree of HEAD

**Files:**
- Modify: `scripts/lib/gates.mjs` (imports, append); `scripts/dev.mjs` (import, one case).
- Test: `tests/foundation/gates.test.mjs` (append).

**Interfaces — produces:**

```js
export const cleanCheckoutTasks;   // [[id, task], ...]: bootstrap, gen-check-1, gen-check-2, foundation, check, test, check-offline, audit
export async function cleanCheckout(root, { parent = lanesParent(root), execute = executeStep, echo, now = () => new Date() } = {});
//   -> { passed, receipt, receiptPath, worktree, removed }
```

Behaviour: `git worktree add --detach <parent>/clean-checkout-<first 12 of sha> <sha>`; each
task runs as `bun scripts/dev.mjs <task>` inside it, all of them even after a failure, each
logged to `.artifacts/qualification/clean-checkout/<id>.log` in the checkout that started the
run; then `git status --porcelain` in the worktree; then the receipt. The worktree is removed
with `git worktree remove <path>` only when its status is empty; a drifted worktree is left for
inspection. `passed` means every exit code is 0 and the status is empty. The receipt is written
under `.artifacts/` (ignored) and is not a committed qualification receipt: its `inputs` is
`["."]`, the whole tree at `git_sha`.

- [ ] Append to `tests/foundation/gates.test.mjs` (extend imports as shown):

```js
import { existsSync } from 'node:fs';
import { cleanCheckout, cleanCheckoutTasks, laneSteps, premergeSteps, runLane, runPremerge } from '../../scripts/lib/gates.mjs';
```

```js
test('clean-checkout runs the whole foundation in a detached worktree of HEAD and removes it',async t=>{
 const {base,root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const parent=join(base,'lanes');const worktree=join(parent,`clean-checkout-${sha.slice(0,12)}`);const ran=[];
 const execute=async(step,write)=>{ran.push([step.id,step.cwd,step.args.join(' ')]);write(`ran ${step.id}\n`);return 0;};
 const result=await cleanCheckout(repo,{parent,execute,...quiet,now:()=>new Date('2026-10-05T18:00:00Z')});
 assert.deepEqual(ran.map(([id])=>id),['bootstrap','gen-check-1','gen-check-2','foundation','check','test','check-offline','audit']);
 assert.deepEqual(ran.map(([,,args])=>args),['bootstrap','gen-check','gen-check','foundation','check','test','check-offline','audit'].map(name=>`scripts/dev.mjs ${name}`));
 for(const [,cwd] of ran)assert.equal(cwd,worktree);
 assert.deepEqual(result.receipt,{git_sha:sha,inputs:['.'],produced_at:'2026-10-05T18:00:00.000Z',exit_codes:Object.fromEntries(ran.map(([id])=>[id,0])),git_status_empty:true});
 assert.deepEqual(Object.keys(result.receipt),['git_sha','inputs','produced_at','exit_codes','git_status_empty']);
 assert.deepEqual(JSON.parse(await readFile(join(repo,'.artifacts','qualification','clean-checkout','receipt.json'),'utf8')),result.receipt);
 assert.equal(result.passed,true);assert.equal(result.removed,true);
 assert.equal(existsSync(worktree),false);
 assert.equal((await git(repo,'worktree','list','--porcelain')).split(/\r?\n/).filter(line=>line.startsWith('worktree ')).length,1);
});
test('clean-checkout records a failing step and a drifted tree, keeps the evidence, and never forces',async t=>{
 const {base,root:repo}=await fixtureRepo(t,fixture);const sha=await git(repo,'rev-parse','HEAD');
 const parent=join(base,'lanes');const ran=[];
 const execute=async(step,write)=>{ran.push(step.id);write(`ran ${step.id}\n`);if(step.id==='gen-check-2')await writeFile(join(step.cwd,'drift.json'),'{}');return step.id==='check'?3:0;};
 const result=await cleanCheckout(repo,{parent,execute,...quiet});
 assert.equal(ran.length,cleanCheckoutTasks.length,'a failing step does not stop the run');
 assert.equal(result.receipt.exit_codes.check,3);assert.equal(result.receipt.git_status_empty,false);
 assert.equal(result.passed,false);assert.equal(result.removed,false);
 assert.equal(existsSync(join(parent,`clean-checkout-${sha.slice(0,12)}`,'drift.json')),true,'a drifted worktree is left for inspection');
 await assert.rejects(cleanCheckout(repo,{parent,execute,...quiet}),/clean-checkout refused: .* already exists/);
 // `-D` is Clippy's deny flag here, so only the force spellings are forbidden in this file.
 assert.doesNotMatch(await read('scripts/lib/gates.mjs'),/'--force'|'-f'/,'gates.mjs must never force git');
});
```

- [ ] Run it.
  `bun test ./tests/foundation/gates.test.mjs`
  Expected: fails to load with `Export named 'cleanCheckout' not found in module`.

- [ ] In `scripts/lib/gates.mjs` change three import lines to

```js
import { mkdir, writeFile } from 'node:fs/promises';
import { exists } from './files.mjs';
import { laneNamed, lanesParent } from './lanes.mjs';
```
  (the `./files.mjs` import is new, placed after the `./process.mjs` import) and append:

```js
/** The clean-checkout sequence as [log id, dev.mjs task]; generation is checked twice for drift. */
export const cleanCheckoutTasks = Object.freeze([['bootstrap', 'bootstrap'], ['gen-check-1', 'gen-check'], ['gen-check-2', 'gen-check'],
  ['foundation', 'foundation'], ['check', 'check'], ['test', 'test'], ['check-offline', 'check-offline'], ['audit', 'audit']]);

/**
 * Run the whole foundation in a temporary detached worktree of HEAD and write a receipt.
 * Every task runs even after a failure. The worktree is removed only when it is still clean;
 * otherwise it stays as evidence. Nothing here forces git.
 */
export async function cleanCheckout(root, { parent = lanesParent(root), execute = executeStep, echo = chunk => process.stdout.write(chunk), now = () => new Date() } = {}) {
  const git = (args, cwd = root) => run('git', args, { cwd, capture: true, allowFailure: true });
  const sha = (await run('git', ['rev-parse', 'HEAD'], { cwd: root, capture: true })).stdout.trim();
  const worktree = join(parent, `clean-checkout-${sha.slice(0, 12)}`);
  if (await exists(worktree)) throw new Error(`clean-checkout refused: ${worktree} already exists. Inspect it, then remove it with \`git worktree remove\`.`);
  await mkdir(parent, { recursive: true });
  const added = await git(['worktree', 'add', '--detach', worktree, sha]);
  if (added.code !== 0) throw new Error(`git worktree add failed (git exit ${added.code}): ${added.stderr.trim()}`);
  const out = join(root, '.artifacts', 'qualification', 'clean-checkout');
  const exit_codes = {};
  for (const [id, task] of cleanCheckoutTasks) {
    const step = { id, command: bun(), args: ['scripts/dev.mjs', task], cwd: worktree };
    const { results } = await runSteps([step], { logPath: join(out, `${id}.log`), failFast: true, execute, echo });
    exit_codes[id] = results[0].code;
  }
  const status = await git(['status', '--porcelain'], worktree);
  const git_status_empty = status.code === 0 && status.stdout.trim() === '';
  const receipt = { git_sha: sha, inputs: ['.'], produced_at: now().toISOString(), exit_codes, git_status_empty };
  const receiptPath = join(out, 'receipt.json');
  await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
  let removed = false;
  if (git_status_empty) {
    const removal = await git(['worktree', 'remove', worktree]);
    removed = removal.code === 0;
    if (!removed) echo(`clean-checkout: could not remove ${worktree} (${removal.stderr.trim()}); remove it with \`git worktree remove\` once nothing holds it open.\n`);
  } else {
    echo(`clean-checkout: ${worktree} is not clean after the run and was left in place:\n${status.stdout}${status.stderr}`);
  }
  return { passed: git_status_empty && Object.values(exit_codes).every(value => value === 0), receipt, receiptPath, worktree, removed };
}
```

- [ ] Mechanical edits in `scripts/dev.mjs`:
  - the gates import → `import { cleanCheckout, premergeSteps, runLane, runPremerge } from './lib/gates.mjs';`
  - add after the `premerge` case:
    ```js
        case 'clean-checkout': {
          const result = await cleanCheckout(root);
          process.stdout.write(`${result.passed ? 'CLEAN_CHECKOUT_PASS' : 'CLEAN_CHECKOUT_FAIL'} ${result.receipt.git_sha}\nreceipt: ${result.receiptPath}\n`);
          if (!result.passed) process.exitCode = 1;
          break;
        }
    ```

- [ ] Run it.
  `bun test ./tests/foundation/gates.test.mjs` → ` 9 pass`, ` 0 fail`.
  Do not run the real `bun scripts/dev.mjs clean-checkout` in this package: it is a cold build of
  every feature and belongs to phase 3 on commit S.

- [ ] Commit.
  `git add scripts/lib/gates.mjs scripts/dev.mjs tests/foundation/gates.test.mjs`

```text
feat(gates): track the clean-checkout run as a task.

Why: SPEC section 13 makes Phase 0 "complete only after clean-checkout generation, repeat generation with no file-set or byte drift", but the script existed only as an untracked .artifacts/run-clean-checkout.ps1 with a hard-coded clone path and a different receipt header.
What changed: `bun scripts/dev.mjs clean-checkout` adds a detached worktree of HEAD under the lanes parent, runs bootstrap, gen-check twice, foundation, check, test, check-offline and audit inside it without stopping at a failure, writes {git_sha, inputs, produced_at, exit_codes, git_status_empty} to .artifacts/qualification/clean-checkout/receipt.json, and removes the worktree with plain `git worktree remove` only when it is still clean.
Verified: bun test ./tests/foundation/gates.test.mjs -> 9 pass, 0 fail (fixture repository, injected step runner). The real run is phase 3.
Next: Task A.15, manifests.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.15: Pre-land the manifest needs of wave 2

**Files:**
- Modify: `crates/core/Cargo.toml:20`; `crates/storage/Cargo.toml:31`; `Cargo.lock`
  (`okf-jawn-core` at :3217-3230, `okf-jawn-storage` at :3295-3312).
- Test: `tests/foundation/lockfile.test.mjs` (append).

**Interfaces:**
- Consumes: root `Cargo.toml` `[workspace.dependencies]`: `time = "=0.3.55"`,
  `base64 = "=0.22.1"`, `tempfile = "=3.27.0"`, `tokio = "=1.53.2"` (unchanged).
- Produces: `okf-jawn-core` may `use time` and `use base64`; `okf-jawn-storage` tests may use
  `tempfile` and `tokio` without the `runtime` feature. Nothing else in any manifest changes.

- [ ] Append to `tests/foundation/lockfile.test.mjs`:

```js
test('pre-landed manifest needs are declared from the workspace and locked',async()=>{
 const section=(manifest,name)=>manifest.split(/^\[/m).find(part=>part.startsWith(`${name}]`))??'';
 const core=await read('crates/core/Cargo.toml'),storage=await read('crates/storage/Cargo.toml');
 for(const dependency of ['time','base64'])assert.match(section(core,'dependencies'),new RegExp(`^${dependency}\\.workspace = true$`,'m'),`okf-jawn-core must take ${dependency} from the workspace`);
 assert.match(section(storage,'dev-dependencies'),/^tempfile\.workspace = true$/m);
 assert.match(section(storage,'dev-dependencies'),/^tokio = \{ workspace = true \}$/m);
 const locked=packages(await read('Cargo.lock'));
 const dependencies=name=>locked.find(entry=>entry.name===name).dependencies;
 for(const dependency of ['base64 0.22.1','time'])assert.ok(dependencies('okf-jawn-core').includes(dependency),`Cargo.lock: okf-jawn-core lacks ${dependency}`);
 assert.ok(dependencies('okf-jawn-storage').includes('tempfile'),'Cargo.lock: okf-jawn-storage lacks tempfile');
});
```

- [ ] Run it.
  `bun test ./tests/foundation/lockfile.test.mjs`
  Expected: `(fail)` with `okf-jawn-core must take time from the workspace`.

- [ ] Mechanical edits:
  - `crates/core/Cargo.toml`: after line 20 (`uuid.workspace = true`) insert
    ```toml
    time.workspace = true
    base64.workspace = true
    ```
  - `crates/storage/Cargo.toml`: before line 31 (`[features]`) insert
    ```toml
    [dev-dependencies]
    tempfile.workspace = true
    tokio = { workspace = true }

    ```

- [ ] Update the lock minimally and prove what changed. PowerShell:

```powershell
cargo update --workspace
git diff --stat -- Cargo.lock
git diff -U0 -- Cargo.lock | Select-String -Pattern '^[+-] '
cargo metadata --locked --format-version 1 | Out-Null; $LASTEXITCODE
```
  Expected: ` 1 file changed, 3 insertions(+)`; exactly `+ "base64 0.22.1",`, `+ "time",` (both
  in `okf-jawn-core`) and `+ "tempfile",` (in `okf-jawn-storage`); `0`. Any other changed line:
  `git checkout -- Cargo.lock`, stop and report.

- [ ] Run it.
  `bun test ./tests/foundation/lockfile.test.mjs` → ` 3 pass`, ` 0 fail`.

- [ ] Commit.
  `git add crates/core/Cargo.toml crates/storage/Cargo.toml Cargo.lock tests/foundation/lockfile.test.mjs`

```text
chore(manifests): pre-land time and base64 for core and test dependencies for storage.

Why: wave 2 packages may not edit manifests or lockfiles (design section 7 rule 3); the dispatch and store-port work needs time and base64 in okf-jawn-core, and the storage lane needs tempfile and tokio for tests without the runtime feature.
What changed: crates/core depends on workspace time and base64; crates/storage has [dev-dependencies] tempfile and tokio from the workspace. Cargo.lock gains exactly three dependency lines; no version moved.
Verified: cargo update --workspace then git diff --stat -- Cargo.lock -> 3 insertions; cargo metadata --locked -> exit 0; bun test ./tests/foundation/lockfile.test.mjs -> 3 pass.
Next: Task A.16, repository hygiene.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.16: Untrack the ignored Cursor plan and delete the stale tree snapshot

**Files:**
- Create: `tests/foundation/records.test.mjs`.
- Modify: `.github/CODEOWNERS` (remove the `/REPOSITORY-TREE.txt` row).
- Delete: `REPOSITORY-TREE.txt`.
- Untrack: `.cursor/plans/dependency_qualification_pass_e1c8326b.plan.md`.
- Test: `tests/foundation/records.test.mjs`.

**Interfaces:**
- Produces: `bun scripts/dev.mjs tree` is the only tree view. References to
  `REPOSITORY-TREE.txt` at `678f919`: `.github/CODEOWNERS:20`, the file's own line 37, the Cursor
  plan being untracked here, and the design document (which describes this task; not edited).

- [ ] Write `tests/foundation/records.test.mjs`:

```js
/** Records, prose and scripts agree with the generated surface and with what was actually run. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run } from '../../scripts/lib/process.mjs';
const root=fileURLToPath(new URL('../../',import.meta.url));
const read=file=>readFile(join(root,file),'utf8');

test('nothing tracked is also ignored, and no snapshot of the tree is kept',async()=>{
 await assert.rejects(read('REPOSITORY-TREE.txt'),{code:'ENOENT'});
 for(const file of ['README.md','AGENTS.md','.github/CODEOWNERS','justfile','scripts/dev.mjs'])assert.doesNotMatch(await read(file),/REPOSITORY-TREE/,file);
 assert.match(await read('scripts/dev.mjs'),/case 'tree':/,'the on-demand view stays');
 // A source copy without history (the build container) has no index to inspect.
 if(!existsSync(join(root,'.git')))return;
 const ignored=await run('git',['ls-files','-ci','--exclude-standard'],{cwd:root,capture:true});
 assert.equal(ignored.stdout.trim(),'','these tracked files are ignored by .gitignore');
});
```

- [ ] Run it.
  `bun test ./tests/foundation/records.test.mjs`
  Expected: `(fail)` — `Missing expected rejection` (the snapshot still exists).

- [ ] Apply:

```sh
git rm --cached .cursor/plans/dependency_qualification_pass_e1c8326b.plan.md
git rm REPOSITORY-TREE.txt
```
  and delete the line `/REPOSITORY-TREE.txt @okf-jawn-integration-owner` from
  `.github/CODEOWNERS`.

- [ ] Run it.
  `bun test ./tests/foundation/records.test.mjs ./tests/foundation/lanes.test.mjs` → ` 0 fail`.
  `git ls-files -ci --exclude-standard` → no output.
  `git grep -n REPOSITORY-TREE -- . ':!docs/plans'` → no output.
  `bun scripts/dev.mjs tree` → prints the current tree, exit 0.

- [ ] Commit.
  `git add .github/CODEOWNERS tests/foundation/records.test.mjs`
  (the two `git rm` changes are already staged.)

```text
chore(gates): untrack the ignored Cursor plan and delete the stale tree snapshot.

Why: .cursor/ is ignored but one plan under it was tracked, so `git ls-files -ci` was never empty; REPOSITORY-TREE.txt listed files that no longer exist. SPEC section 14: "Report what was executed separately from what was authored."
What changed: the Cursor plan is removed from the index (the file stays on disk); REPOSITORY-TREE.txt and its CODEOWNERS row are gone; `bun scripts/dev.mjs tree` remains the on-demand view. An offline test fails if a tracked file is ignored or the snapshot returns.
Verified: bun test records and lanes tests -> 0 fail; git ls-files -ci --exclude-standard -> empty; git grep REPOSITORY-TREE outside docs/plans -> empty.
Next: Task A.17, verification.json.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.17: `verification.json` stops recording passes that predate the repair

**Files:**
- Modify: `verification.json:47`, `:69-134` (`checks`), `:168-173`, `:174-179`, `:198-203`,
  `:209-214`, `:215-220` (five Phase 0 gates).
- Test: `tests/foundation/records.test.mjs` (append).

**Interfaces — produces (the invariant every later record edit must keep):**
- `phase_0_qualified === false` ⇒ `current.deterministic_foundation_green === false`, and no
  entry of `current.checks` or `current.gates.phase_0` has `status: "passed"`.
- `phase_0_qualified === true` ⇒ `current.deterministic_foundation_green === true`, and every
  `current.gates.phase_0` status is one of `passed`, `rejected_with_fallback`,
  `bounded_upstream_exception`.
- The three ids in `libraryGates` exist in `current.gates.phase_0`.

Evidence for the edits: CI run 37352882339 on `b205c4a` — `check-offline` failed, `foundation`
failed, and `test`, `check` and `audit` were skipped. Every `passed` below was recorded on
`e3dd8bf`, before `50113b9..b205c4a`.

- [ ] Append to `tests/foundation/records.test.mjs` (add the import beside the others):

```js
import { libraryGates } from '../../scripts/lib/receipts.mjs';
```

```js
test('verification.json records no pass while Phase 0 is reopened, and only terminal gates once qualified',async()=>{
 const record=JSON.parse(await read('verification.json'));
 const gates=record.current.gates.phase_0;
 assert.equal(new Set(gates.map(gate=>gate.id)).size,gates.length);
 for(const id of libraryGates)assert.ok(gates.some(gate=>gate.id===id),`Phase 0 gate ${id} is missing`);
 if(record.phase_0_qualified===true){
  assert.equal(record.current.deterministic_foundation_green,true);
  for(const gate of gates)assert.ok(['passed','rejected_with_fallback','bounded_upstream_exception'].includes(gate.status),`${gate.id} is ${gate.status} although phase_0_qualified is true`);
  return;
 }
 assert.equal(record.current.deterministic_foundation_green,false,'deterministic_foundation_green cannot be true while Phase 0 is unqualified');
 for(const gate of gates)assert.notEqual(gate.status,'passed',`${gate.id} is recorded as passed while Phase 0 is reopened`);
 for(const check of record.current.checks)assert.notEqual(check.status,'passed',`${check.command.join(' ')} is recorded as passed while Phase 0 is reopened`);
});
```

- [ ] Run it.
  `bun test ./tests/foundation/records.test.mjs`
  Expected: `(fail)` with `deterministic_foundation_green cannot be true while Phase 0 is
  unqualified`.

- [ ] Edit `verification.json`:
  - `:47` → `    "deterministic_foundation_green": false,`
  - replace `:69-134` (the whole `"checks": [ ... ],` member) with:

```json
    "checks": [
      {
        "command": [
          "bun",
          "scripts/dev.mjs",
          "check-offline"
        ],
        "status": "incomplete",
        "last_passed_on": "e3dd8bf8d9bcca0856ad5da1c98fcc8b88547807",
        "meaning": "Failed on b205c4a (CI 37352882339): policy.test.mjs and vendor.test.mjs asserted stale literal counts. Record again from a run on S."
      },
      {
        "command": [
          "bun",
          "scripts/dev.mjs",
          "gen-check"
        ],
        "status": "incomplete",
        "last_passed_on": "e3dd8bf8d9bcca0856ad5da1c98fcc8b88547807",
        "meaning": "Last pass was CI 37322344115, before the repair in 50113b9..b205c4a. Record again from a run on S."
      },
      {
        "command": [
          "bun",
          "scripts/dev.mjs",
          "foundation"
        ],
        "status": "incomplete",
        "last_passed_on": "e3dd8bf8d9bcca0856ad5da1c98fcc8b88547807",
        "meaning": "Failed on b205c4a (CI 37352882339). Covers gen-check, cargo test for contract/core/xtask only, UI build, typecheck and offline. Record again from a run on S."
      },
      {
        "command": [
          "bun",
          "--bun",
          "run",
          "lint"
        ],
        "cwd": "ui",
        "status": "incomplete",
        "last_passed_on": "e3dd8bf8d9bcca0856ad5da1c98fcc8b88547807",
        "meaning": "Skipped in CI 37352882339 on b205c4a after the foundation step failed. Record again from a run on S."
      },
      {
        "command": [
          "bun",
          "--bun",
          "run",
          "typecheck"
        ],
        "cwd": "ui",
        "status": "incomplete",
        "last_passed_on": "e3dd8bf8d9bcca0856ad5da1c98fcc8b88547807",
        "meaning": "tsc -b via ui/tsconfig.json referencing ui/tsconfig.tests.json. Skipped in CI 37352882339 on b205c4a after the foundation step failed. Record again from a run on S."
      },
      {
        "command": [
          "bun",
          "scripts/dev.mjs",
          "audit"
        ],
        "status": "incomplete",
        "last_passed_on": "e3dd8bf8d9bcca0856ad5da1c98fcc8b88547807",
        "meaning": "Skipped in CI 37352882339 on b205c4a. GHSA-vfj7-8cjw-p6xm remains, ignored as build-time tooling. Record again from a run on S."
      }
    ],
```
  - gate `authored-typescript-seams` (`:168-173`): `"status": "incomplete"`, `"receipt": null`,
    and prefix its `meaning` with
    `Reopened: last passed on e3dd8bf (CI 37322344115); Vitest was skipped in CI 37352882339 on b205c4a. Re-run on S. Covers: `
    (keep the existing sentence after `Covers: `).
  - gate `deterministic-foundation` (`:174-179`): `"status": "incomplete"`, `"receipt": null`,
    `"meaning": "Reopened: CI 37352882339 on b205c4a is red (check-offline and foundation failed; test, check and audit were skipped). Last green was CI 37322344115 and the clean checkout at e3dd8bf8d9bcca0856ad5da1c98fcc8b88547807, before the repair. Re-run on S."`
  - gate `json-render-catalog-round-trip` (`:198-203`): `"status": "incomplete"`,
    `"receipt": null`, and prefix its `meaning` with
    `Reopened: last passed on e3dd8bf; neither cargo test nor Vitest ran in CI 37352882339 on b205c4a. Re-run on S. Covers: `
  - gate `clean-checkout-rerun` (`:209-214`): `"status": "incomplete"`, `"receipt": null`,
    `"meaning": "Reopened: last run was on e3dd8bf8d9bcca0856ad5da1c98fcc8b88547807, before the repair. Re-run with `bun scripts/dev.mjs clean-checkout` on S; it writes .artifacts/qualification/clean-checkout/receipt.json."`
  - gate `github-actions-ci-receipt` (`:215-220`): `"status": "incomplete"`, `"receipt": null`,
    `"meaning": "Reopened: push CI 37352882339 on b205c4a8027b56b02f717af0c94d5dc59f5760b3 failed in all three jobs. Last green was CI 37322344115 on e3dd8bf8d9bcca0856ad5da1c98fcc8b88547807. Record the run on S."`

- [ ] Run it.
  `bun -e "JSON.parse(await Bun.file('verification.json').text()); console.log('valid')"` → `valid`.
  `bun test ./tests/foundation/records.test.mjs ./tests/foundation/lockfile.test.mjs` →
  ` 0 fail` (the lockfile test still finds `docling-core 1.93.6` in `direct_versions`).

- [ ] Commit.
  `git add verification.json tests/foundation/records.test.mjs`

```text
docs(records): stop recording passes that predate the foundation repair.

Why: verification.json said phase_0_qualified false yet kept deterministic_foundation_green true, six checks and five Phase 0 gates as passed on e3dd8bf, while CI 37352882339 on b205c4a is red. AGENTS.md: "only the recorded results of the selected tools count."
What changed: deterministic_foundation_green is false; the six checks and the gates authored-typescript-seams, deterministic-foundation, json-render-catalog-round-trip, clean-checkout-rerun and github-actions-ci-receipt are incomplete, each naming the last green commit and what must be re-run on S. An offline test forbids `passed` while Phase 0 is unqualified and requires terminal gates once it is.
Verified: bun test ./tests/foundation/records.test.mjs -> 0 fail; the file parses.
Next: Task A.18, the acceptance script and the deployment example.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.18: Acceptance script follows drafts and Snapshot; deployment example describes its images

**Files:**
- Modify: `tests/integration/acceptance.mjs:28`; `deploy/.env.example:15`.
- Test: `tests/foundation/records.test.mjs` (append).

**Interfaces:**
- Consumes: `api/operations.json` paths. `save_draft` request (`crates/contract/src/item.rs`):
  `{ workspace_id, item_id, base_revision, body, properties, idempotency_key }`. `commit_items`
  request as fixed by the shared interfaces (package C removes `expected_head`):
  `{ workspace_id, item_ids (min 1), message, idempotency_key }`, response
  `{ revision, receipt_id, warnings }`. `deploy/compose.yaml:9-11` (`${NAME:?...}`),
  `deploy/Dockerfile:5-7` (`ARG NAME`).
- Produces: nothing new. The script is not executed in Stage 1 (it needs a running server).

- [ ] Append to `tests/foundation/records.test.mjs`:

```js
test('the acceptance script calls only declared operations, and edits through a draft and a Snapshot',async()=>{
 const paths=new Set(JSON.parse(await read('api/operations.json')).map(operation=>operation.path));
 const source=await read('tests/integration/acceptance.mjs');
 const calls=[...source.matchAll(/call\('([a-z]+)','([a-z_]+)'/g)];
 assert.ok(calls.length>0);
 for(const [,domain,id] of calls)assert.ok(paths.has(`/api/${domain}/${id.replaceAll('_','-')}`),`acceptance.mjs calls ${domain}/${id}, which is not a declared operation`);
 assert.match(source,/call\('items','save_draft'/);
 assert.match(source,/call\('history','commit_items',\{workspace_id,item_ids:\[item_id\],message:/);
});
test('the deployment example declares and describes every image the compose file and Dockerfile require',async()=>{
 const example=await read('deploy/.env.example');
 const required=[...(await read('deploy/compose.yaml')).matchAll(/\$\{([A-Z_]+):\?/g)].map(match=>match[1]);
 const images=[...(await read('deploy/Dockerfile')).matchAll(/^ARG ([A-Z_]+)$/gm)].map(match=>match[1]);
 assert.ok(required.length>0&&images.length>0);
 for(const name of new Set([...required,...images]))assert.match(example,new RegExp(`^${name}=`,'m'),`deploy/.env.example does not declare ${name}`);
 const described=name=>example.split(/\r?\n/).find(line=>line.startsWith(`# ${name}:`))??'';
 for(const name of images)assert.ok(described(name).length>0,`${name} has no description`);
 assert.match(described('BUN_IMAGE'),/\.bun-version/,'BUN_IMAGE must be described the way compose.yaml and the Dockerfile require it');
});
```

- [ ] Run it.
  `bun test ./tests/foundation/records.test.mjs`
  Expected: 2 fail — `acceptance.mjs calls items/update_item, which is not a declared operation`
  and `BUN_IMAGE has no description`.

- [ ] Mechanical edits:
  - `tests/integration/acceptance.mjs:28` (the `const update=await call('items','update_item',…)`
    line) → these three lines:
    ```js
    await call('items','save_draft',{workspace_id,item_id,base_revision:revision,body:'The proposal is now recorded as changed.\n',properties,idempotency_key:randomUUID()});
    const snapshot=await call('history','commit_items',{workspace_id,item_ids:[item_id],message:'Record the change',idempotency_key:randomUUID()});
    assert.notEqual(snapshot.revision,revision,'a Snapshot must create a new revision; saving a draft must not');
    ```
  - `deploy/.env.example:15` (`# Set digest-pinned OCI image references for reproducible
    deployment builds.`) → these four lines:
    ```text
    # Build images for deploy/compose.yaml, which refuses to build while any of them is empty.
    # BUN_IMAGE: digest-pinned oven/bun image of the release in .bun-version; doctor rejects any other.
    # RUST_IMAGE: digest-pinned Rust image for the toolchain in rust-toolchain.toml.
    # RUNTIME_IMAGE: qualified digest-pinned runtime image for the application target.
    ```

- [ ] Run it.
  `bun test ./tests/foundation/records.test.mjs ./tests/foundation/policy.test.mjs ./tests/foundation/toolchain.test.mjs ./tests/foundation/init.test.mjs`
  → ` 0 fail`.

- [ ] Commit.
  `git add tests/integration/acceptance.mjs deploy/.env.example tests/foundation/records.test.mjs`

```text
fix(acceptance): edit through save_draft and commit_items, and describe the build images.

Why: the acceptance script still called update_item, which 50113b9 replaced with drafts (owner decision: "saving never creates a revision"); deploy/.env.example described its three image variables with one generic line while compose.yaml and the Dockerfile require BUN_IMAGE to match .bun-version.
What changed: the script saves a draft, commits it with commit_items (no expected_head, per the shared interfaces) and asserts the new revision. The example describes BUN_IMAGE, RUST_IMAGE and RUNTIME_IMAGE as compose requires them. Offline tests hold the script to declared operation paths and the example to the variables compose and the Dockerfile name.
Verified: bun test ./tests/foundation/records.test.mjs -> 0 fail. The script itself runs only against a disposable deployment, which does not exist yet.
Next: Task A.19, prose and task list.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Task A.19: README, AGENTS.md, justfile and `help` state the current surface

**Files:**
- Modify: `README.md:51`, `:64`, `:80`, `:84-90`; `AGENTS.md:32`, `:46`, `:50`; `justfile:48-50`
  and append; `scripts/dev.mjs:292` (help).
- Test: `tests/foundation/records.test.mjs` (append).

**Interfaces:**
- Consumes: `api/operations.json`, `api/transports.json`, `api/mcp-tools.json`; the test idiom
  and commit format from the shared interfaces; design §7.
- Produces: the final task list of `scripts/dev.mjs`:
  `init doctor lock bootstrap gen gen-check routes check-offline check test foundation premerge
  lane scope vendor tree lanes lanes-table lanes-reset clean-checkout qualify check-receipts
  audit`.

- [ ] Append to `tests/foundation/records.test.mjs`:

```js
test('README states the generated counts and tool names, and neither prose file remembers a number',async()=>{
 const readme=await read('README.md'),agents=await read('AGENTS.md');
 const operations=JSON.parse(await read('api/operations.json')),transports=JSON.parse(await read('api/transports.json'));
 const tools=JSON.parse(await read('api/mcp-tools.json')).tools;
 assert.ok(readme.includes(`There are ${operations.length} typed JSON application commands and ${transports.length} distinct transport declarations.`),'README operation and transport counts differ from api/');
 for(const tool of tools.filter(entry=>entry._meta.ui.visibility.includes('model')))assert.ok(readme.includes(`\`${tool.name}\``),`README does not name the model tool ${tool.name}`);
 assert.doesNotMatch(readme,/Utoipa schema types/);
 for(const [name,text] of [['README.md',readme],['AGENTS.md',agents]])assert.doesNotMatch(text,/\b(?:three|3)\b[^.\n]*Context7/i,name);
 for(const term of ['TestResult','err_of','git merge --no-ff','Why:','PowerShell'])assert.ok(agents.includes(term),`AGENTS.md does not state ${term}`);
});
test('help names every task the entrypoint accepts, and just mirrors the gates',async()=>{
 const entry=await read('scripts/dev.mjs');
 const cases=[...entry.matchAll(/^\s+case '([a-z-]+)':/gm)].map(match=>match[1]).filter(name=>name!=='help');
 const help=/case 'help': process\.stdout\.write\('Tasks: ([^\\]+)\\n'\)/.exec(entry)?.[1].split(' ')??[];
 assert.deepEqual([...help].sort(),[...cases].sort());
 const just=await read('justfile');
 for(const recipe of ['lane','premerge','clean-checkout','lanes-reset','check-receipts'])assert.match(just,new RegExp(`^${recipe}\\b`,'m'),`justfile has no ${recipe} recipe`);
});
```

- [ ] Run it.
  `bun test ./tests/foundation/records.test.mjs`
  Expected: 2 fail — `README operation and transport counts differ from api/` and the help list
  missing `routes`, `premerge`, `lane`, `scope`, `lanes-table`, `clean-checkout`.

- [ ] `README.md` edits:
  - `:51` → ``| `api/openapi.json` and `.yaml` | Schemars component schemas (the schema authority) and the typed operation declarations, assembled with Utoipa's OpenAPI builder |``
  - `:64` → `There are 69 typed JSON application commands and 15 distinct transport declarations. Twelve tools are model-facing: ` + `` `ls`, `grep`, `show`, `log`, `diff`, `blame`, `sources`, `links`, `propose`, `catalog`, `present` and `workspaces` `` + ` (which lists the workspaces a connection may see, so an agent can pin its reads to the returned head revision); the scoped binary-read tool is app-only. Human approval, verification and connector management are never model tools. The raw transport declarations are documented schema obligations, not bound handlers yet.`
  - `:80`: replace `and the three Context7 library IDs/queries recorded for the foundation.`
    with ``and, for each entry that records an executed Context7 lookup, its library ID and query (`context7_queries_executed` equals the number of such entries).``
  - `:84-90` (from `After the foundation is resolved` through the paragraph that ends
    `…whichever unrelated check is red.`) → 

````markdown
After the foundation is resolved and qualified, commit it and create isolated worktrees:

```sh
bun scripts/dev.mjs lanes            # all seven; or name them: lanes storage
bun scripts/dev.mjs lane storage     # that lane's gate
bun scripts/dev.mjs premerge         # the CI sequence, every step, every feature
```

`lanes` creates one worktree and `build/<lane>` branch per lane from the same clean commit, under `OKF_LANES_DIR` when set, else `D:\okf\lanes` on Windows when `D:` exists, else `../okf-jawn-lanes`. `scripts/lib/lanes.mjs` is the only lane table: worktrees, gates, the scope check, the AGENTS.md table and the CODEOWNERS lane rows are generated from it or tested against it. `lane <name>` runs that lane's fmt, Clippy, tests, source policy and scope check (Biome, route generation, `tsc` and filtered Vitest for the UI lanes), writes `.artifacts/lane/<name>/<sha>.log` and ends with `PASS <name> <sha>` or `FAIL <name> <sha> <step>`; the sha carries `-dirty` when the tree had uncommitted changes. `premerge` is what CI runs; it never stops at the first failure. `lanes-reset` removes only clean lanes that hold no commits of their own, and never forces. None of this starts paid agents, spends API credits, or grants repository privileges. Each lane's files contain short local instructions. The integration owner controls contracts, manifests, lockfiles, generated outputs, deployment and independent acceptance. `CODEOWNERS` routes review on GitHub; it is not a local lock, and no branch protection is configured yet. Builders may test their work but may not redefine it from whichever unrelated check is red.
````

- [ ] `AGENTS.md` edits:
  - `:32`: replace `Vendor notes include three executed Context7 lookups and other clearly marked references.`
    with `Vendor notes mark which entries record an executed Context7 lookup; the rest are clearly marked references.`
  - `:46`: after `Ownership is enforced locally by isolated worktrees;` the sentence continues
    unchanged; append to the end of the paragraph:
    `` `bun scripts/dev.mjs scope`, part of every lane gate and of pre-push, fails when a lane branch changes a path outside its directories, a manifest, or a lockfile.``
  - after `:50` (the first Conventions paragraph) insert these two paragraphs:

````markdown
Rust tests use one idiom. Include `tests/support/check.rs` with `#[path]`; a test returns `TestResult` (`Result<(), Box<dyn std::error::Error>>`) and uses `?`, `assert!`/`assert_eq!`, `err_of(result)` for a result that must have failed and `some(option, "what")` for a value that must be present. No `unwrap`, `expect`, `expect_err`, `panic!`, `unreachable!` or `[]` indexing (use `.get(..)` with `some`), and no `#[allow]` or `#[expect]` anywhere.

Operating rules. Only the integration owner merges, always `git merge --no-ff`; never rebase, squash, amend a pushed commit or force-push. Work only in the files your brief allows: never change shared contracts, tests you did not write, manifests, lockfiles, lint config or records; stop and report the symbol (file:line), the SPEC sentence, the proposed change and the failing output. Commit at every green step, one concern each: subject `type(scope): what.`, body `Why:`, `What changed:`, `Verified:` (command and result), then `Next:` or `Blocked:`. Take `main` with `git merge main` at task boundaries on a clean tree; on a conflict in a generated directory or a lockfile take `main`'s side, run `gen` and commit, never editing conflict markers there. An attempt is one edit-and-gate cycle on the same failing check. A review finding blocks only if it cites a failing command or a named SPEC or AGENTS sentence; at most two review rounds per task. On Windows run cargo from PowerShell, never Git Bash.
````

- [ ] `justfile` edits:
  - `:48-50` → 
    ```just
    # Create isolated Git worktrees from an already committed foundation (all lanes, or the named ones).
    lanes *names:
        bun scripts/dev.mjs lanes {{names}}
    ```
  - append at the end of the file:
    ```just

    # Remove clean lane worktrees and branches that hold no commits. Never forces.
    lanes-reset:
        bun scripts/dev.mjs lanes-reset

    # One lane's gate: fmt, Clippy, tests, source policy, scope (UI lanes: Biome, routes, tsc, Vitest).
    lane name:
        bun scripts/dev.mjs lane "{{name}}"

    # The CI sequence with every feature; runs every step and reports all failures.
    premerge:
        bun scripts/dev.mjs premerge

    check-receipts:
        bun scripts/dev.mjs check-receipts

    # The whole foundation in a temporary detached worktree of HEAD; writes a receipt under .artifacts/.
    clean-checkout:
        bun scripts/dev.mjs clean-checkout
    ```

- [ ] `scripts/dev.mjs` help line (`:292` at `678f919`) →
  `    case 'help': process.stdout.write('Tasks: init doctor lock bootstrap gen gen-check routes check-offline check test foundation premerge lane scope vendor tree lanes lanes-table lanes-reset clean-checkout qualify check-receipts audit\n'); break;`

- [ ] Confirm the end state of `main()` in `scripts/dev.mjs` is exactly:

```js
async function main() {
  switch (task) {
    case 'doctor': await doctor(); break;
    case 'init': process.stdout.write(`${JSON.stringify(await initialize(root), null, 2)}\n`); break;
    case 'check-offline': await offlineChecks(); break;
    case 'lock': await lock(); break;
    case 'bootstrap': await bootstrap(); break;
    case 'gen': await generate(root); break;
    case 'gen-check': await generate(root, true); break;
    case 'routes': await routes(); break;
    case 'vendor': await vendor(); break;
    case 'tree': process.stdout.write(await tree(root)); break;
    case 'lanes': process.stdout.write(`${(await createLanes(root, { names: args })).join('\n')}\n`); break;
    case 'lanes-table': process.stdout.write(await syncLaneTable(root) ? 'AGENTS.md lane table regenerated.\n' : 'AGENTS.md lane table is current.\n'); break;
    case 'scope': {
      const result = await checkScope(root, { lane: positional[0], base: option('--base') });
      process.stdout.write(`scope: ${result.changed.length} changed path(s) since ${result.base}, all inside ${result.name}.\n`);
      break;
    }
    case 'lane': {
      const result = await runLane(root, positional[0]);
      if (!result.passed) process.exitCode = 1;
      break;
    }
    case 'premerge': {
      const result = await runPremerge(root, { only: option('--step') });
      if (!result.passed) process.exitCode = 1;
      break;
    }
    case 'clean-checkout': {
      const result = await cleanCheckout(root);
      process.stdout.write(`${result.passed ? 'CLEAN_CHECKOUT_PASS' : 'CLEAN_CHECKOUT_FAIL'} ${result.receipt.git_sha}\nreceipt: ${result.receiptPath}\n`);
      if (!result.passed) process.exitCode = 1;
      break;
    }
    case 'lanes-reset': process.stdout.write(`${await resetLanes(root)}\n`); break;
    case 'qualify': await qualify(); break;
    case 'check-receipts': process.stdout.write(`${await checkReceipts(root)}\n`); break;
    case 'audit': await audit(); break;
    case 'check': await runNamed(['fmt', 'clippy', 'source-policy', 'ui-lint', 'ui-typecheck', 'gen-check']); break;
    case 'test': await runNamed(['test', 'ui-test']); break;
    case 'foundation':
      await generate(root, true);
      await run('cargo', ['test', '--locked', '-p', 'okf-jawn-contract', '-p', 'okf-jawn-core', '-p', 'xtask'], { cwd: root });
      await uiScript('build');
      await generatedStrictProbe();
      await uiScript('typecheck');
      await offlineChecks(); break;
    case 'help': process.stdout.write('Tasks: init doctor lock bootstrap gen gen-check routes check-offline check test foundation premerge lane scope vendor tree lanes lanes-table lanes-reset clean-checkout qualify check-receipts audit\n'); break;
    default: throw new Error(`Unknown task: ${task}. Use help.`);
  }
}
```
  and that its imports are exactly:

```js
import { readFile, rm } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gitLocalEnvironment, run, version } from './lib/process.mjs';
import { files, exists } from './lib/files.mjs';
import { generate, requireLockfiles } from './lib/generation.mjs';
import { initialize, installHooks } from './lib/init.mjs';
import { tree } from './lib/tree.mjs';
import { bun, pins } from './lib/toolchain.mjs';
import { checkScope, createLanes, resetLanes, syncLaneTable } from './lib/lanes.mjs';
import { checkReceipts } from './lib/receipts.mjs';
import { cleanCheckout, premergeSteps, runLane, runPremerge } from './lib/gates.mjs';
```
  The `lanes` case passes `args` unfiltered, so `bun scripts/dev.mjs lanes storage` creates one
  lane and an unknown name fails with `Unknown lane: …`.

- [ ] Run it.
  `bun scripts/dev.mjs lanes-table` → `AGENTS.md lane table is current.`
  `bun test ./tests/foundation/records.test.mjs` → ` 6 pass`, ` 0 fail`.
  `bun scripts/dev.mjs help` → the task line above.
  `bun scripts/dev.mjs check-offline` → ` 85 pass`, ` 0 fail`, `Ran 85 tests across 15 files`.

- [ ] Commit.
  `git add README.md AGENTS.md justfile scripts/dev.mjs tests/foundation/records.test.mjs`

```text
docs(gates): bring README, AGENTS.md, justfile and help in line with the tooling.

Why: README said 66 operations, eleven model tools, Utoipa as the schema source and three Context7 lookups; AGENTS.md repeated the lookup count and had no test idiom or operating rules; help and justfile omitted every new task. SPEC section 14: "Root README, AGENTS and this SPEC are the canonical prose."
What changed: README states 69 operations, the twelve model tools by name including workspaces, Schemars as schema authority, and the lanes, lane, premerge and lanes-reset behaviour. AGENTS.md states the Rust test idiom and the operating rules of design section 7 in compact form. justfile gains lane, premerge, lanes-reset, check-receipts and clean-checkout. An offline test derives the README counts from api/ and holds help to the switch.
Verified: bun scripts/dev.mjs check-offline -> 85 pass, 0 fail across 15 files; bun scripts/dev.mjs lanes-table -> current.
Next: package A acceptance by an agent that did not write it, then merge into integration/foundation-cure.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
```

---

### Package A acceptance

Run by an agent that did not write the package, in `D:\okf\cure\gates` at the tip of
`cure/gates`. `bun`/`git` from either shell; `cargo` lines from PowerShell.

| # | Command | Expected |
| --- | --- | --- |
| 1 | `git status --porcelain` | no output |
| 2 | `bun scripts/dev.mjs check-offline` | ` 85 pass`, ` 0 fail`, `Ran 85 tests across 15 files`; the test names `complete operation surface has unique canonical identifiers and paths` and `recorded Context7 lookups are counted from the entries, never from memory` are `(pass)` |
| 3 | `bun scripts/dev.mjs scope` | `scope: <n> changed path(s) since 678f919…, all inside gates.` (`<n>` is 39 when every task was applied as written), exit 0 |
| 4 | `git diff --name-only 678f919 HEAD -- crates/*/src crates/*/tests xtask/src qualification ui/src scripts/lib/provenance.mjs api generated tests/support` | no output |
| 5 | `git diff --stat 678f919 HEAD -- Cargo.lock` | ` 1 file changed, 9 insertions(+), 6 deletions(-)` |
| 6 | PowerShell: `git diff -U0 678f919 HEAD -- Cargo.lock \| Select-String -Pattern '^[+-][^+-]'` | fifteen lines: three times `-version = "1.96.1"` / `+version = "1.93.6"` with a `-checksum`/`+checksum` pair each (the `+` checksums are those listed in Task A.3), then `+ "base64 0.22.1",`, `+ "time",`, `+ "tempfile",` |
| 7 | PowerShell: `cargo metadata --locked --format-version 1 \| Out-Null; $LASTEXITCODE` | `0` |
| 8 | `git diff --stat 678f919 HEAD -- bun.lock package.json ui/package.json Cargo.toml` | no output |
| 9 | `bun scripts/dev.mjs lanes-table` then `git status --porcelain` | `AGENTS.md lane table is current.`; no output |
| 10 | `bun scripts/dev.mjs check-receipts` | `check-receipts: no receipts under qualification/receipts/; accepted only because verification.json records phase_0_qualified false.` |
| 11 | `git ls-files -ci --exclude-standard` | no output |
| 12 | `git ls-files -s scripts/hooks` | two lines starting `100755 ` |
| 13 | `git ls-files lefthook.yml REPOSITORY-TREE.txt .cursor` | no output |
| 14 | `sh scripts/hooks/pre-commit` | offline suite, exit 0 |
| 15 | `bun install --frozen-lockfile`, then `bun scripts/dev.mjs routes`, then `bun --bun run --cwd ui typecheck`, then `bun --bun run --cwd ui test` | each exit 0; `Generated ui/src/routeTree.gen.ts.`; Vitest reports 8 passed files |
| 16 | `bun scripts/dev.mjs premerge --step nope` | `Unknown premerge step: nope. Steps: check-offline, gen-check, fmt, clippy, source-policy, test, ui-lint, ui-build, ui-typecheck, ui-test, check-receipts.`, exit 1 |
| 17 | `bun scripts/dev.mjs lane no-such-lane` | `Unknown lane: no-such-lane. Lanes: storage, ingest, core-cli, server, mcp-execution, workspace-ui, views.`, exit 1 |
| 18 | `git log --format=%s 678f919..HEAD` | 19 subjects, each `type(scope): … .`; `git log --format=%b 678f919..HEAD \| grep -c '^Verified:'` prints `19` |

Mutation checks the verifier repeats (each must fail, then be reverted with `git checkout --`):

1. In `api/operations.json` give `create_review` the alias `verify` and visibility `model` →
   `bun test ./tests/foundation/policy.test.mjs` fails `agent exposure cannot include human
   approval or verification`.
2. In `vendors.json` set `context7_queries_executed` to `7` →
   `bun test ./tests/foundation/vendor.test.mjs` fails with `7 !== 6`.
3. In `.github/workflows/ci.yml` delete the `premerge --step clippy` step →
   `bun test ./tests/foundation/ci.test.mjs` fails `CI runs exactly the premerge sequence…`.
4. In `verification.json` set `deterministic_foundation_green` to `true` →
   `bun test ./tests/foundation/records.test.mjs` fails.

Not verified by this package, and not to be reported as verified: `lane <name>` and `premerge`
end to end (Clippy is red on `678f919` until packages C and E land, and Biome is red in four
files this package may not touch — Deviation 20; the orchestrator runs `premerge` at integrate),
the GitHub workflow itself (first push of the integration branch), the real `clean-checkout`
(phase 3 on S), and `tests/integration/acceptance.mjs` against a server.

How this section was checked while writing it: every code block above was extracted and applied
to a scratch copy of `678f919` (`git archive`, its own repository; the real checkout was not
modified). There, `check-offline` ran 85 pass / 0 fail across 15 files; a real `git commit`
through the installed pre-commit hook passed and left the repository intact; `routes`, then
`typecheck` and Vitest (8 files, 35 tests) passed without a build; the views and workspace-ui
Vitest filters selected 6 and 2 files; the five mutations listed here and in Task A.6 failed as
stated; and the policy and records tests also passed against a simulated package C output
(13 tools, `workspaces` exposed). Not exercised there: every cargo command (the lock changes
were simulated by editing the three Docling blocks and the three dependency lines by hand), so
Tasks A.3 and A.15 are the first real cargo runs.

Merge notes for the orchestrator: package C regenerates `api/`; after it merges, row 2 must still
pass unchanged (the policy test admits `workspaces` and the 13-tool list). `core.hooksPath` is
set for the shared repository the first time `bootstrap` or `init` runs in any worktree that has
`scripts/hooks/`. The seven legacy worktrees under `..\okf-jawn-lanes` are found by
`lanes-reset` through their `build/*` branches even though the default parent is now
`D:\okf\lanes`. Delete the `cure/gates` row from `cures` in `scripts/lib/lanes.mjs` (or replace
it with rows for later packages) once this branch is merged.

### Deviations

1. **jsdom is not in `ui/package.json`.** The brief says it is still listed. At `678f919`,
   `git grep -n -i jsdom -- ui/package.json` prints nothing; `devDependencies` has
   `happy-dom` only, and the single `jsdom` in `bun.lock` (:1563) is Vitest's optional peer. No
   task edits `ui/package.json` or `bun.lock`; acceptance row 8 proves they are untouched.
2. **`context7_queries_executed` becomes 6, not a derived 7.** 7 and 6 cannot both hold under the
   file's own scope sentence ("counts the lookups whose library and query are recorded here").
   `git diff b13fcd0 50113b9 -- vendors.json` shows the seventh lookup was the earlier schemars
   query, overwritten by the new one. Task A.2 sets the number to 6 and asserts equality. If the
   owner wants the superseded query kept, it needs a second recorded query on the schemars entry.
3. **"Adopted iii worker" wording was not found.** `git grep -n -i adopt -- verification.json
   vendors.json README.md AGENTS.md` matches only `vendors.json:317` ("Foundation-repair adopts
   these settings", about cargo-deny). SPEC.md:55 says "iii was not adopted". Nothing was changed
   for this item.
4. **`vendors.json` Docling version text needs no edit.** It already says `docling-core 1.93.6`;
   only `Cargo.lock` was wrong. Task A.3 restores the lock and adds a test that holds the three
   places equal.
5. **Two more packages drifted in `50113b9` and are left as they are:** `powerfmt` 0.2.0→0.2.1
   and `unicase` 2.9.0→2.10.0 (`git diff b13fcd0 50113b9 -- Cargo.lock`). Neither is pinned by
   SPEC or a manifest and the brief asks for the Docling crates only.
6. **`cargo update … --precise` with three package names is taken from the cargo-update man page,
   not from a run** (this section was written without running cargo). Task A.3 states the exact
   expected diff and tells the implementer to stop and report if cargo refuses.
7. **Manifests: `tower-http` and `tokio-util` features are not added.** The design (§5, A) lists
   them; the brief says "Nothing else". Root `Cargo.toml` is untouched.
8. **More files than the design lists for A.** New: `scripts/lib/receipts.mjs`,
   `scripts/lib/gates.mjs` (so receipts and gates can be tested against temporary repositories;
   `dev.mjs` binds `root` to its own location), `tests/foundation/fixture-repo.mjs` and six test
   files. Modified beyond the list: `scripts/lib/init.mjs` (hook install),
   `tests/foundation/{process,init,toolchain}.test.mjs` (`toolchain.test.mjs:35` reads
   `lefthook.yml`, which is deleted), `justfile`, `.github/workflows/{qualify,contract}.yml`.
   None overlaps package B or C. `.gitignore` is in the design's list but needs no change.
9. **`actions/upload-artifact@ea165f8d…` (v4.6.2) also declares `using: node20`;** the brief
   flags only checkout. Both are bumped, in all three workflows (`contract.yml` is in neither the
   design's nor the brief's file list). `upload-artifact` ≥ 4.4 skips hidden files by default, so
   the two uploads of `.artifacts/…` get `include-hidden-files: true`; without it the existing
   `qualify.yml` upload would be empty.
10. **Scope check on `cure/*` branches has one row.** The lane table holds the seven construction
    lanes; cure packages are a separate temporary list (`cures`) with a row for `cure/gates`
    only, because this section cannot see the branch names or file lists of packages B–H. A
    `cure/*` branch without a row fails `scope` (and so pre-push) with `No scope row for branch
    …`. Cure rows compare against `integration/foundation-cure`, not `main`, because
    `git diff main 678f919` already contains the design document. Orchestrator to add rows or
    drop `cure/*` from the hook.
11. **Lane scope is narrower than CODEOWNERS and wider than "directories" in two ways.** Every
    lane excludes its own manifests (`crates/<lane>/Cargo.toml`, `ui/package.json`,
    `ui/biome.json`) per design §7 rule 3, and may change `api/`, `generated/cli/` and
    `ui/src/api/generated/` per the AGENTS.md generator-input rule. The six views unit specs in
    `ui/tests/unit/` are owned by the views lane (they are what its Vitest filter runs) and
    excluded from workspace-ui; CODEOWNERS gains matching rows.
12. **A gate label carries `-dirty`.** The brief gives `PASS <name> <sha>`; with uncommitted
    changes the label is `<sha>-dirty`, so a log cannot claim a commit it did not test.
13. **The lane gate is fail-fast; premerge is not.** The brief's `FAIL <name> <sha> <step>` names
    one step; premerge's FAIL line lists every failed step, comma-separated.
14. **Receipt header is stricter than today's check.** `git_sha` must be 40 lowercase hex (the
    `commit_sha` alias is no longer accepted), `inputs` must be non-empty and `produced_at` must
    parse. This follows the design's shared header for package B; B's receipts must satisfy it.
    "Library gate" is taken to mean the docling, iii and mcp-apps gates; `clean-checkout-rerun`
    keeps its receipt under `.artifacts/` and is not required by `check-receipts`.
15. **Timeout tree kill is Windows only.** `taskkill /T /F` ends the tree there; on POSIX the
    direct child is still the only process signalled, because a group kill needs
    `detached: true`, which changes Ctrl-C delivery for every task. Package B does its own POSIX
    group kill in the harness.
16. **`deploy/.env.example` versus `compose.yaml`: the brief does not say what the mismatch is.**
    What the files show: the example described three required image variables with one generic
    line, while `compose.yaml:9` and `Dockerfile:4` require `BUN_IMAGE` to match `.bun-version`.
    Task A.18 fixes that and tests that every `${NAME:?}` and `ARG` is declared. Not addressed,
    because it would change `compose.yaml` or `init` and could not be verified offline: `init`
    copies the example to `<root>/.env`, while `/deploy/.env` is what `.gitignore` anticipates.
17. **CI concurrency does not cancel runs on `main`** (`cancel-in-progress` is false there), so
    every commit that reaches `main` keeps its result; all other refs cancel superseded runs.
18. **`routes` deletes `ui/src/routeTree.gen.ts` before regenerating it.** The TanStack plugin
    logs and swallows generator errors (`router-generator-plugin.js`: `catch (e) {
    console.error(e) }`), so a stale file would otherwise hide a failed generation.
19. **Branch-protection sentences are unchanged.** `gh api repos/freebatteryfactory/okf-jawn/rulesets`
    returned `[]` and `branches/main/protection` returned 404 on 2026-10-05, so "no branch
    protection is configured yet" (README, AGENTS.md, CODEOWNERS) is still true. Whoever applies
    the ruleset in setup must update those three sentences and SPEC §14.
20. **Finding outside this package: `bun --bun run --cwd ui lint` fails at `678f919`.** In the
    scratch copy Biome 2.5.15 reports `Found 4 errors`, all `format`, in
    `ui/scripts/bundle-app.mjs` (package D's directory) and
    `ui/tests/unit/{catalog-schema.test.ts,layout.test.tsx,mcp-apps-dispatch.test.tsx}` (no
    package's files). CI never showed it because the `check` step was skipped in run
    37352882339. Until someone runs `biome format --write` on those four files, the premerge
    step `ui-lint` and the `biome` step of both UI lane gates fail. Package A does not fix it
    (rule 3: tests it did not write, another package's directory); the orchestrator needs to
    assign it before integrate.
21. **The commit trailer in this section is the shared-interfaces one** (`Co-Authored-By` only).
    The session that wrote this section was later told by its harness to add a `Claude-Session:`
    line to commits it creates; it creates none, so the shared format stands. An implementer
    whose own harness asks for an extra trailer line should follow that instruction for its own
    commits.
