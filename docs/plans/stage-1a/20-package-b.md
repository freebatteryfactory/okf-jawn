## Package B: Harnesses

- **Branch:** `cure/harness` (from `integration/foundation-cure` at `678f919`; code identical to `b205c4a`).
- **Worktree:** `D:\okf\cure\harness`. Every path below is relative to it.
- **Wave:** 1, in parallel with package A (`scripts/dev.mjs`, `tests/foundation/{policy,vendor}.test.mjs`, CI, manifests, docs) and package C (`crates/contract/**`).
- **Files allowed (exact):**
  - `qualification/docling/run.mjs`, `qualification/docling/src/main.rs`
  - `qualification/docling/lib/runner.mjs`, `qualification/docling/lib/receipt.mjs` (new)
  - `qualification/mcp-apps/run.mjs`, `qualification/mcp-apps/src/main.rs`
  - `qualification/mcp-apps/lib/process.mjs`, `qualification/mcp-apps/lib/views.mjs` (new)
  - `qualification/iii/run.mjs`
  - `qualification/lib/cargo.mjs`, `qualification/record.mjs` (new)
  - `scripts/lib/provenance.mjs`
  - `tests/foundation/harness.test.mjs`
  - `tests/fixtures/documents/SOURCES.json`
- **Must not touch:** `scripts/dev.mjs`, any `Cargo.toml`, `Cargo.lock`, `bun.lock`, `package.json`, `clippy.toml`, `tests/fixtures/views/**`, `ui/**`, `crates/**`, `xtask/**`, `verification.json`, `vendors.json`, `qualification/receipts/**`, and every byte of the fixture files under `tests/fixtures/documents/` (only `SOURCES.json` changes). No `#[allow]`, `#[expect]`, `unwrap`, `expect`, `panic!`, `unreachable!` or `[]` indexing in Rust, product or test.
- **SPEC / AGENTS sentences served:**
  - SPEC §5: "Use the pinned Docling Rust implementation through its actual supported API"; "No fake success, empty digest or text-only substitute may stand in for the converter."; "Heavy conversion runs in bounded workers with time, memory and cancellation controls."
  - SPEC §9: "MCP Apps supplies source, Changes, Timeline and constrained presentation views."; "Keep tools usable with a textual fallback when a host cannot display an App."
  - SPEC §12: "the gate ended REJECTED WITH FALLBACK and the selected runtime is Tokio + RecordStore".
  - SPEC §13: Phase 0 "is complete only after … architecture-relevant external library/host qualification."
  - AGENTS.md: "Do not inherit a claim of green from file presence; only the recorded results of the selected tools count."; "Do not suppress lints, fake success, discard unsupported data, alter expected output to match implementation, or declare a partial test to be the full suite."
  - `verification.json` gates: `docling-library-qualification` ("Requalify on a clean SHA with per-fixture peak memory and must_fail_truncated.pdf."), `mcp-apps-protocol-qualification` ("Requalify on a clean SHA with provenance guard and ngrok closed_at."), `iii-library-qualification` ("Decision stands; provenance guard will be added before any re-run.").

**Working rules for every task**

- `bun` and `git` run from either shell. **cargo runs from PowerShell only.** `bun qualification/*/run.mjs` spawns cargo, so launch it from PowerShell too.
- JS test command for this package: `bun test ./tests/foundation/harness.test.mjs`. The whole offline suite (`bun scripts/dev.mjs check-offline`) has two known failures in `policy.test.mjs` and `vendor.test.mjs` that package A cures; they are not yours.
- The first failing-test step of a JS task adds imports of modules or exports that do not exist yet. Bun then fails to load the file and reports every test in it as failed. That is the expected red.
- Do not commit when an observed result differs from the "expected" text. Stop and report (operating rule 12: three attempts on one failing check without a new written diagnosis is a handoff).
- Commit messages use the shared format and are given in full. Their `Verified:` lines state the expected results; commit only when you observed exactly those.

**Facts read from source that the tasks rely on**

- Docling 1.93.5 `src/source.rs:41`: `pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConversionError>`. It returns `Err(ConversionError::UnknownFormat { hint })` for a missing or unknown extension and `Err(ConversionError::Io(_))` when `std::fs::read` fails. It never parses the bytes.
- Docling 1.93.5 `src/converter.rs:910`: `pub fn convert(&self, source: SourceDocument) -> Result<ConversionResult, ConversionError>`. A PDF goes through `self.ml_pipeline()…convert_outcome(…).map_err(|e| ConversionError::with_source("pdf", e))?` (`:1077-1085`). The returned status is computed at `:1218-1222` as `if errors.is_empty() { ConversionStatus::Success } else { ConversionStatus::PartialSuccess }`.
- `ConversionStatus::Failure` is declared (`src/result.rs:10-14`) but **never constructed anywhere in docling 1.93.5** (`grep Failure` over `src/` matches only the declaration). For `must_fail_truncated.pdf` the reachable PASS is therefore `convert` returning `Err` (stage `converter_error`). The `converter_status` + `Failure` branch stays in the judge because the enum allows it.
- A spent document budget is reported by buffered `convert` as `PartialSuccess` with `ErrorItem::timeout(message)` (`src/result.rs:33-39`: `component_type: "document_backend"`, `module_name: "pipeline"`; message text from docling-pdf `lib.rs:197`: "document timeout of {:.3}s exceeded after …"). `src/error.rs:30-36`: "Buffered conversions never raise" `ConversionError::Timeout`. An `Err` is therefore never a honoured timeout.
- `ml_pipeline()` loads the models before it reads the PDF. With missing model assets **every** PDF returns `Err`, including the must-fail fixture. Task B.5 makes a must-fail PASS count only when another PDF fixture converted successfully in the same run.
- Docling exposes no version constant: `src/lib.rs:41-72` re-exports types and `docling_core`/`docling_pdf` items only. The converter version is read from `Cargo.lock` by the orchestrator and handed to the harness as `OKF_DOCLING_CRATE_VERSION` (no `build.rs`, no manifest change).
- rmcp 3.5.0 `src/model.rs:4247-4264`: `CallToolRequestParams { pub name: Cow<'static, str>, pub arguments: Option<JsonObject>, … }`; `:45` `pub type JsonObject<F = Value> = serde_json::Map<String, F>`; `:711` `McpError::invalid_params(message: impl Into<Cow<'static, str>>, data: Option<Value>)`; `:2891` `ContentBlock::text(text: impl Into<String>)`; `:4075` `CallToolResult::success(content: Vec<ContentBlock>)`.
- `@modelcontextprotocol/ext-apps` 2.0.3 `dist/src/spec.types.d.ts:621-640`: `type McpUiToolVisibility = "model" | "app"`; `McpUiToolMeta.visibility?: McpUiToolVisibility[]` ("app": Tool callable by the app from this server only).
- `@axe-core/playwright` 4.13.0 `dist/index.d.ts`: `new AxeBuilder({ page })`, `.analyze(): Promise<AxeResults>`. A node inside a frame has one `target` entry per frame hop plus one for the element.
- basic-host v2.0.3 (cached copy under `.artifacts/qualification/mcp-apps/basic-host/`): `src/index.tsx:60-65` reads `server`, `tool`, `call=true`, `theme=hide` from the query string; the App runs in host page → sandbox proxy iframe on `:8081` → inner iframe (`src/sandbox.ts:40-44`), so the App document is at frame depth 2.
- `ui/src/features/views/PresentView.tsx:117-129`: the present view calls host tool `show` once per resolved binding with `{ workspace_id, item_id, at: { kind: 'revision', revision }, view: 'text', selection, max_bytes: 65536, max_images: 0 }` and checks only that the returned `source.revision` equals the binding's.
- `scripts/dev.mjs:184-240` (`check-receipts`): requires top-level `git_sha` (or `commit_sha`) matching `/^[0-9a-f]{40}$/i` and `inputs` as `string[]`; then `git merge-base --is-ancestor` and `git diff --name-only <sha> HEAD -- <inputs>`.
- Prototype evidence (Bun 1.4.2, Windows, inline scripts, no files written): the runner and process helpers in B.3 and B.7 were executed as written. Observed: instant-exit child resolved in 76 ms; held child sampled once while alive; timeout kill at 320 ms with `signal: 'SIGTERM'`; missing binary resolved with `spawnError: 'Executable not found in $PATH: …'`; real `PeakWorkingSet64` sample in 399 ms; early-exit rejection in 86 ms carrying stderr and `exit 1`; grandchild process gone after `killProcessTree`.

---

### Task B.1: One receipt header and an absolute clean-tree guard

**Files:**
- Modify: `scripts/lib/provenance.mjs:1-58` (whole file replaced)
- Create: `qualification/record.mjs`
- Test: `tests/foundation/harness.test.mjs:1-5` (header and imports), appended tests

**Interfaces:**
- Consumes: `git status --porcelain --untracked-files=all`, `git rev-parse HEAD`, `git cat-file -e <sha>:<path>`.
- Produces (all exported from `scripts/lib/provenance.mjs`):
  - `requireCleanTree(cwd?: string): Promise<string>` — unchanged contract; rejects on any porcelain output or any non-zero git exit; returns the 40-hex HEAD.
  - `receiptHeader(root: string, inputs: string[]): Promise<{ git_sha: string, inputs: string[], produced_at: string }>` — calls `requireCleanTree`; rejects inputs that are empty, absolute, contain `\`, `.`/`..`/empty segments, or do not exist at HEAD. `produced_at` is the ISO-8601 UTC instant the clean tree was observed (before the harness ran).
  - `receiptHeaderProblems(receipt: unknown): string[]` — empty when the top level carries the header.
  - `recordReceipt(root: string, name: string, receipt: object): Promise<string>` — writes `qualification/receipts/<name>.json` (2-space JSON + newline) and returns the path; rejects a bad name, a missing header, or `receipt.git_sha !== HEAD`.
- Receipt header shape (top level of every receipt): `{ "git_sha": "<40 hex>", "inputs": ["repo/relative/path", …], "produced_at": "2026-10-05T18:00:00.000Z" }`.
- CLI: `bun qualification/record.mjs <name>…` with names from `docling`, `mcp-apps`, `iii`; copies `.artifacts/qualification/<name>/receipt.json` through `recordReceipt`.

- [ ] **Step 1: write the failing tests.** Replace lines 1-5 of `tests/foundation/harness.test.mjs` with the block below (the three existing HTTP tests on lines 7-24 stay untouched), then append the tests after line 24.

```js
/**
 * Harness controls. They are not application acceptance.
 *
 * HTTP controls prove the assertion helpers can fail. Qualification controls prove the
 * orchestrators' own rules (provenance, process handling, judgement) without cargo or network.
 */
import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { reviewCoversRevision, readResolvedRevision } from '../support/assertions.mjs';
import {
  receiptHeader,
  receiptHeaderProblems,
  recordReceipt,
  requireCleanTree,
} from '../../scripts/lib/provenance.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));

/** Reject when `promise` has not settled after `ms`; a hang must read as a named failure. */
function within(promise, ms, label) {
  let timer;
  const expired = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${label} did not settle within ${ms} ms`)), ms);
  });
  return Promise.race([promise, expired]).finally(() => clearTimeout(timer));
}

/** A throwaway git repository outside this checkout, isolated from user hooks and signing. */
async function tempRepo({ commit = true } = {}) {
  const base = await mkdtemp(join(tmpdir(), 'okf-provenance-'));
  const repo = join(base, 'repo');
  const hooks = join(base, 'hooks');
  await mkdir(join(repo, 'dir'), { recursive: true });
  await mkdir(hooks);
  const git = (...args) =>
    execFileSync(
      'git',
      [
        '-c', `core.hooksPath=${hooks}`,
        '-c', 'commit.gpgsign=false',
        '-c', 'user.name=okf-test',
        '-c', 'user.email=okf-test@example.invalid',
        ...args,
      ],
      { cwd: repo, stdio: 'pipe' },
    ).toString();
  git('init');
  await writeFile(join(repo, 'a.txt'), 'a\n');
  await writeFile(join(repo, 'dir', 'b.txt'), 'b\n');
  if (commit) {
    git('add', '.');
    git('commit', '-m', 'init');
  }
  return { base, repo, git, dispose: () => rm(base, { recursive: true, force: true }) };
}
```

Appended tests:

```js
test('requireCleanTree returns HEAD when clean and rejects untracked, modified and staged paths', async () => {
  const fixture = await tempRepo();
  try {
    assert.match(await requireCleanTree(fixture.repo), /^[0-9a-f]{40}$/);
    await writeFile(join(fixture.repo, 'new.txt'), 'x\n');
    await assert.rejects(requireCleanTree(fixture.repo), /clean git tree[\s\S]*new\.txt/);
    await rm(join(fixture.repo, 'new.txt'));
    await writeFile(join(fixture.repo, 'a.txt'), 'changed\n');
    await assert.rejects(requireCleanTree(fixture.repo), /clean git tree[\s\S]*a\.txt/);
    fixture.git('add', 'a.txt');
    await assert.rejects(requireCleanTree(fixture.repo), /clean git tree[\s\S]*a\.txt/);
  } finally {
    await fixture.dispose();
  }
});

test('requireCleanTree sees untracked files even when user configuration hides them', async () => {
  const fixture = await tempRepo();
  try {
    fixture.git('config', 'status.showUntrackedFiles', 'no');
    await writeFile(join(fixture.repo, 'new.txt'), 'x\n');
    await assert.rejects(requireCleanTree(fixture.repo), /clean git tree[\s\S]*new\.txt/);
  } finally {
    await fixture.dispose();
  }
});

test('requireCleanTree rejects when git exits non-zero', async () => {
  const fixture = await tempRepo({ commit: false });
  try {
    await rm(join(fixture.repo, 'a.txt'));
    await rm(join(fixture.repo, 'dir'), { recursive: true });
    // Empty work tree, no commit: status prints nothing and exits 0, rev-parse HEAD exits 128.
    await assert.rejects(requireCleanTree(fixture.repo), /git rev-parse HEAD exited \d+/);
  } finally {
    await fixture.dispose();
  }
});

test('receiptHeader output is the top-level shape check-receipts validates', async () => {
  const fixture = await tempRepo();
  try {
    const before = Date.now();
    const header = await receiptHeader(fixture.repo, ['a.txt', 'dir']);
    assert.deepEqual(Object.keys(header), ['git_sha', 'inputs', 'produced_at']);
    // The two predicates of scripts/dev.mjs check-receipts (lines 202-210).
    assert.match(header.git_sha, /^[0-9a-f]{40}$/i);
    assert.ok(Array.isArray(header.inputs) && header.inputs.every((item) => typeof item === 'string'));
    assert.deepEqual(header.inputs, ['a.txt', 'dir']);
    assert.ok(Date.parse(header.produced_at) >= before - 1000);
    assert.deepEqual(receiptHeaderProblems({ ...header, anything: 1 }), []);
    // The old Docling shape nested the header and failed check-receipts.
    assert.equal(receiptHeaderProblems({ orchestrator: header }).length, 3);
  } finally {
    await fixture.dispose();
  }
});

test('receiptHeader refuses a dirty tree and inputs check-receipts could never see change', async () => {
  const fixture = await tempRepo();
  try {
    await assert.rejects(receiptHeader(fixture.repo, ['missing.txt']), /does not exist at [0-9a-f]{40}/);
    await assert.rejects(receiptHeader(fixture.repo, ['../a.txt']), /segments/);
    await assert.rejects(receiptHeader(fixture.repo, ['dir/']), /segments/);
    await assert.rejects(receiptHeader(fixture.repo, ['C:/a.txt']), /relative to the repository root/);
    await assert.rejects(receiptHeader(fixture.repo, ['/a.txt']), /relative to the repository root/);
    await assert.rejects(receiptHeader(fixture.repo, ['dir\\b.txt']), /forward slashes/);
    await assert.rejects(receiptHeader(fixture.repo, []), /non-empty array/);
    await writeFile(join(fixture.repo, 'new.txt'), 'x\n');
    await assert.rejects(receiptHeader(fixture.repo, ['a.txt']), /clean git tree/);
  } finally {
    await fixture.dispose();
  }
});

test('recordReceipt writes qualification/receipts/<name>.json only for a headed receipt at HEAD', async () => {
  const fixture = await tempRepo();
  try {
    const header = await receiptHeader(fixture.repo, ['a.txt']);
    const target = await recordReceipt(fixture.repo, 'docling', { ...header, result: 'PASS' });
    assert.equal(target, join(fixture.repo, 'qualification', 'receipts', 'docling.json'));
    assert.deepEqual(JSON.parse(await readFile(target, 'utf8')), { ...header, result: 'PASS' });
    await assert.rejects(recordReceipt(fixture.repo, '../escape', header), /lower-case words/);
    await assert.rejects(recordReceipt(fixture.repo, 'nested', { orchestrator: header }), /git_sha/);
    await assert.rejects(
      recordReceipt(fixture.repo, 'stale', { ...header, git_sha: 'a'.repeat(40) }),
      /HEAD is [0-9a-f]{40}/,
    );
  } finally {
    await fixture.dispose();
  }
});
```

Notes on honesty of these tests: tests 1 and 3 characterise behaviour `requireCleanTree` already has at HEAD (`provenance.mjs:46-57` and `:33-36`); they go green as soon as the file loads and go red if the `status.stdout.trim()` check or the non-zero-exit rejection is removed. Test 2 is red at HEAD after the file loads (plain `git status --porcelain` honours `status.showUntrackedFiles=no`). Tests 4-6 are red until Step 3.

- [ ] **Step 2: run and see the failure.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: exit 1; Bun reports `SyntaxError: Export named 'receiptHeader' not found in module '…scripts/lib/provenance.mjs'` (wording may be "export 'receiptHeader' not found"); 0 pass.

- [ ] **Step 3: implement.** Replace `scripts/lib/provenance.mjs` with:

```js
/**
 * Qualification harness provenance.
 *
 * Receipts must cite a clean committed tree. Dirty working trees fail before
 * any receipt is written so a SHA cannot be claimed for uncommitted work.
 *
 * Every receipt starts with the same top-level header, produced only by receiptHeader:
 *   { "git_sha": "<40-hex>", "inputs": ["path/relative/to/repo", ...], "produced_at": "<ISO-8601 UTC>" }
 * produced_at is the moment the clean tree was observed, before the harness ran.
 * `bun scripts/dev.mjs check-receipts` validates git_sha and inputs of every file
 * under qualification/receipts/; recordReceipt is the only writer of that directory.
 */

import { spawn } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const defaultRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const FULL_SHA = /^[0-9a-f]{40}$/i;
const RECEIPT_NAME = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

function git(args, cwd) {
  return new Promise((resolveGit, reject) => {
    const child = spawn('git', args, {
      cwd,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('error', reject);
    child.on('close', (code) => {
      if (code === 0) resolveGit({ stdout, stderr });
      else reject(new Error(`git ${args.join(' ')} exited ${code}: ${stderr.trim() || stdout.trim()}`));
    });
  });
}

/**
 * Fail if the working tree is dirty. Return the HEAD SHA when clean.
 * Any porcelain output is dirty; any non-zero git exit rejects.
 * @param {string} [cwd] repository root
 * @returns {Promise<string>}
 */
export async function requireCleanTree(cwd = defaultRoot) {
  // --untracked-files=all: a user-level status.showUntrackedFiles=no must not hide new files.
  const status = await git(['status', '--porcelain', '--untracked-files=all'], cwd);
  if (status.stdout.trim()) {
    throw new Error(
      `Qualification requires a clean git tree before writing a receipt. Dirty paths:\n${status.stdout.trim()}`,
    );
  }
  const head = await git(['rev-parse', 'HEAD'], cwd);
  const sha = head.stdout.trim();
  if (!FULL_SHA.test(sha)) {
    throw new Error(`git rev-parse HEAD did not return a full SHA: ${sha}`);
  }
  return sha;
}

function inputProblem(input) {
  if (typeof input !== 'string' || input.length === 0) return 'must be a non-empty string';
  if (input.includes('\\')) return 'must use forward slashes';
  if (input.startsWith('/') || /^[A-Za-z]:/.test(input)) {
    return 'must be relative to the repository root';
  }
  if (input.split('/').some((part) => part === '' || part === '.' || part === '..')) {
    return 'must not contain empty, "." or ".." segments';
  }
  return null;
}

/**
 * The header every qualification receipt carries at its top level.
 * @param {string} root repository root
 * @param {string[]} inputs repo-relative paths whose later change invalidates the receipt
 * @returns {Promise<{ git_sha: string, inputs: string[], produced_at: string }>}
 */
export async function receiptHeader(root, inputs) {
  if (!Array.isArray(inputs) || inputs.length === 0) {
    throw new Error('receiptHeader: inputs must be a non-empty array of repo-relative paths');
  }
  for (const input of inputs) {
    const problem = inputProblem(input);
    if (problem) throw new Error(`receiptHeader: input ${JSON.stringify(input)} ${problem}`);
  }
  const git_sha = await requireCleanTree(root);
  for (const input of inputs) {
    try {
      await git(['cat-file', '-e', `${git_sha}:${input}`], root);
    } catch {
      throw new Error(
        `receiptHeader: input ${input} does not exist at ${git_sha}; check-receipts could never see it change`,
      );
    }
  }
  return { git_sha, inputs: [...inputs], produced_at: new Date().toISOString() };
}

/**
 * Why `receipt` does not carry the shared header at its top level; empty when it does.
 * @param {unknown} receipt
 * @returns {string[]}
 */
export function receiptHeaderProblems(receipt) {
  if (receipt === null || typeof receipt !== 'object' || Array.isArray(receipt)) {
    return ['receipt must be a JSON object'];
  }
  const problems = [];
  if (typeof receipt.git_sha !== 'string' || !FULL_SHA.test(receipt.git_sha)) {
    problems.push('git_sha must be a 40-hex commit SHA at the top level');
  }
  if (
    !Array.isArray(receipt.inputs) ||
    receipt.inputs.length === 0 ||
    !receipt.inputs.every((item) => typeof item === 'string')
  ) {
    problems.push('inputs must be a non-empty string[] of repo-relative paths at the top level');
  }
  if (typeof receipt.produced_at !== 'string' || Number.isNaN(Date.parse(receipt.produced_at))) {
    problems.push('produced_at must be an ISO-8601 timestamp at the top level');
  }
  return problems;
}

/**
 * Copy a finished receipt to qualification/receipts/<name>.json.
 * The tree need not be clean here (an earlier recorded receipt is itself untracked),
 * but HEAD must still be the commit the receipt cites.
 * @param {string} root repository root
 * @param {string} name receipt name, e.g. "docling"
 * @param {object} receipt finished receipt with the shared header
 * @returns {Promise<string>} the written path
 */
export async function recordReceipt(root, name, receipt) {
  if (typeof name !== 'string' || !RECEIPT_NAME.test(name)) {
    throw new Error(`recordReceipt: name ${JSON.stringify(name)} must be lower-case words joined by hyphens`);
  }
  const problems = receiptHeaderProblems(receipt);
  if (problems.length) throw new Error(`recordReceipt(${name}): ${problems.join('; ')}`);
  const head = (await git(['rev-parse', 'HEAD'], root)).stdout.trim();
  if (head !== receipt.git_sha) {
    throw new Error(
      `recordReceipt(${name}): receipt cites ${receipt.git_sha} but HEAD is ${head}; requalify on the current commit`,
    );
  }
  const target = join(root, 'qualification', 'receipts', `${name}.json`);
  await mkdir(dirname(target), { recursive: true });
  await writeFile(target, `${JSON.stringify(receipt, null, 2)}\n`);
  return target;
}
```

Create `qualification/record.mjs`:

```js
/**
 * Copy finished qualification receipts into qualification/receipts/.
 *
 * Usage: bun qualification/record.mjs <name>...   (docling | mcp-apps | iii)
 *
 * Each harness needs a clean tree and a recorded receipt is an untracked file, so three
 * harnesses cannot each record on the same commit. Run all three first (their receipts
 * land in the ignored .artifacts/qualification/<name>/receipt.json), then record them
 * together. recordReceipt refuses a receipt without the shared header or one that cites
 * a commit other than HEAD.
 */

import { readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { recordReceipt } from '../scripts/lib/provenance.mjs';

const KNOWN = ['docling', 'mcp-apps', 'iii'];
const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const names = process.argv.slice(2);

if (names.length === 0 || names.some((name) => !KNOWN.includes(name))) {
  throw new Error(`Usage: bun qualification/record.mjs <name>... where name is one of ${KNOWN.join(', ')}`);
}
for (const name of names) {
  const source = join(root, '.artifacts', 'qualification', name, 'receipt.json');
  const receipt = JSON.parse((await readFile(source, 'utf8')).replace(/^\uFEFF/, ''));
  const target = await recordReceipt(root, name, receipt);
  process.stdout.write(`recorded ${source} -> ${target}\n`);
}
```

- [ ] **Step 4: run and see it pass.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: `9 pass`, `0 fail`.
  Run: `bun qualification/record.mjs nonsense`
  Expected: exit 1, `Usage: bun qualification/record.mjs <name>... where name is one of docling, mcp-apps, iii`.

- [ ] **Step 5: commit.**

```sh
git add scripts/lib/provenance.mjs qualification/record.mjs tests/foundation/harness.test.mjs
git commit -F - <<'MSG'
feat(qualify): add one receipt header and a recorder behind the clean-tree guard.

Why: check-receipts (scripts/dev.mjs:202-210) needs top-level git_sha and
inputs, but each orchestrator built its own header and Docling nested it under
receipt.orchestrator. requireCleanTree had no tests, and plain
`git status --porcelain` hides untracked files when the user sets
status.showUntrackedFiles=no. AGENTS.md: "only the recorded results of the
selected tools count."
What changed: provenance.mjs exports receiptHeader(root, inputs),
receiptHeaderProblems(receipt) and recordReceipt(root, name, receipt);
requireCleanTree passes --untracked-files=all. qualification/record.mjs copies
finished receipts into qualification/receipts/. receiptHeader rejects inputs
that do not exist at HEAD.
Verified: bun test ./tests/foundation/harness.test.mjs -> 9 pass, 0 fail.
Next: Task B.2 adds the shared cargo helpers the orchestrators import.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
MSG
```

---

### Task B.2: Cargo facts for the orchestrators

**Files:**
- Create: `qualification/lib/cargo.mjs`
- Test: `tests/foundation/harness.test.mjs` (imports, appended tests)

**Interfaces:**
- Consumes: `cargo metadata --format-version 1 --no-deps --locked` (JSON field `target_directory`); `Cargo.lock` text.
- Produces (exported from `qualification/lib/cargo.mjs`):
  - `exec(command: string, args: string[], options?: { cwd?: string, env?: object, inherit?: boolean }): Promise<{ code: number, stdout: string, stderr: string }>`
  - `lockedPackages(lockText: string, name: string): { name: string, version: string | null, checksum: string | null }[]`
  - `lockedPackage(lockText: string, name: string): { name, version: string, checksum: string | null }` — throws unless exactly one.
  - `releaseBinary(targetDir: string, name: string, platform?: string): string`
  - `cargoTargetDir(root: string): Promise<string>`
  - `buildRelease(root: string, pkg: string): Promise<string>` — runs `cargo build --locked --release -p <pkg>` and returns the binary path.

- [ ] **Step 1: write the failing tests.** Add to the import block of `tests/foundation/harness.test.mjs`:

```js
import { lockedPackage, lockedPackages, releaseBinary } from '../../qualification/lib/cargo.mjs';
```

Append:

```js
test('lockedPackage reads exactly one pinned package from Cargo.lock text', () => {
  const lock = [
    '# This file is automatically @generated by Cargo.',
    'version = 4',
    '',
    '[[package]]',
    'name = "docling"',
    'version = "1.93.5"',
    'source = "registry+https://github.com/rust-lang/crates.io-index"',
    'checksum = "abc123"',
    '',
    '[[package]]',
    'name = "docling-core"',
    'version = "1.93.6"',
    '',
    '[[package]]',
    'name = "sha2"',
    'version = "0.10.9"',
    '',
    '[[package]]',
    'name = "sha2"',
    'version = "0.11.0"',
    '',
  ];
  for (const text of [lock.join('\n'), lock.join('\r\n')]) {
    assert.deepEqual(lockedPackage(text, 'docling'), { name: 'docling', version: '1.93.5', checksum: 'abc123' });
    assert.deepEqual(lockedPackages(text, 'sha2').map((entry) => entry.version), ['0.10.9', '0.11.0']);
    assert.throws(() => lockedPackage(text, 'sha2'), /exactly one sha2 package.*found 2/);
    assert.throws(() => lockedPackage(text, 'absent'), /exactly one absent package.*found 0/);
  }
});

test('the locked docling version is the one the workspace manifest pins', async () => {
  const manifest = await readFile(join(root, 'Cargo.toml'), 'utf8');
  const pinned = /^docling = \{ version = "=([^"]+)"/m.exec(manifest)?.[1];
  assert.ok(pinned, 'workspace Cargo.toml must pin docling with an exact version');
  assert.equal(lockedPackage(await readFile(join(root, 'Cargo.lock'), 'utf8'), 'docling').version, pinned);
});

test('releaseBinary names the platform executable under the cargo target directory', () => {
  assert.equal(releaseBinary('t', 'okf-qualify-docling', 'win32'), join('t', 'release', 'okf-qualify-docling.exe'));
  assert.equal(releaseBinary('t', 'okf-qualify-docling', 'linux'), join('t', 'release', 'okf-qualify-docling'));
});
```

- [ ] **Step 2: run and see the failure.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: exit 1, `Cannot find module '../../qualification/lib/cargo.mjs'`; 0 pass.

- [ ] **Step 3: implement.** Create `qualification/lib/cargo.mjs`:

```js
/**
 * Cargo facts the qualification orchestrators need: where release binaries land
 * (honouring CARGO_TARGET_DIR and build.target-dir) and which versions Cargo.lock pins.
 * Dependency-free; the pure functions are covered by tests/foundation/harness.test.mjs.
 */

import { spawn } from 'node:child_process';
import { join } from 'node:path';

/** Run a command; resolve with its exit code and captured output (inherit: stream instead). */
export function exec(command, args, options = {}) {
  return new Promise((resolveExec, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: options.env ?? process.env,
      stdio: options.inherit ? 'inherit' : ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });
    let stdout = '';
    let stderr = '';
    child.stdout?.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr?.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('error', reject);
    child.on('close', (code) => resolveExec({ code: code ?? 1, stdout, stderr }));
  });
}

/** Every [[package]] entry of Cargo.lock named `name`. */
export function lockedPackages(lockText, name) {
  const field = (block, key) => new RegExp(`^${key} = "([^"]+)"\\r?$`, 'm').exec(block)?.[1] ?? null;
  return lockText
    .split(/\r?\n\[\[package\]\]\r?\n/)
    .filter((block) => field(block, 'name') === name)
    .map((block) => ({ name, version: field(block, 'version'), checksum: field(block, 'checksum') }));
}

/** The single [[package]] entry named `name`; throws when Cargo.lock has none or several. */
export function lockedPackage(lockText, name) {
  const matches = lockedPackages(lockText, name);
  if (matches.length !== 1 || !matches[0].version) {
    throw new Error(`Cargo.lock must pin exactly one ${name} package with a version; found ${matches.length}`);
  }
  return matches[0];
}

/** Path of a release binary under a cargo target directory. */
export function releaseBinary(targetDir, name, platform = process.platform) {
  return join(targetDir, 'release', platform === 'win32' ? `${name}.exe` : name);
}

/** The workspace target directory as cargo itself resolves it. */
export async function cargoTargetDir(root) {
  const result = await exec('cargo', ['metadata', '--format-version', '1', '--no-deps', '--locked'], { cwd: root });
  if (result.code !== 0) throw new Error(`cargo metadata exited ${result.code}\n${result.stderr}`);
  const directory = JSON.parse(result.stdout).target_directory;
  if (typeof directory !== 'string' || directory.length === 0) {
    throw new Error('cargo metadata did not report target_directory');
  }
  return directory;
}

/** Build one workspace package in release mode from the lockfile; return its binary path. */
export async function buildRelease(root, pkg) {
  const result = await exec('cargo', ['build', '--locked', '--release', '-p', pkg], { cwd: root, inherit: true });
  if (result.code !== 0) throw new Error(`cargo build --locked --release -p ${pkg} exited ${result.code}`);
  return releaseBinary(await cargoTargetDir(root), pkg);
}
```

- [ ] **Step 4: run and see it pass.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: `12 pass`, `0 fail`.

- [ ] **Step 5: commit.**

```sh
git add qualification/lib/cargo.mjs tests/foundation/harness.test.mjs
git commit -F - <<'MSG'
feat(qualify): read the locked converter version and the cargo target directory.

Why: the Docling harness printed a hard-coded "1.93.5" (main.rs:81-87) and both
orchestrators assumed <root>/target, which is wrong once build output moves to
D:. SPEC 5: "Use the pinned Docling Rust implementation".
What changed: qualification/lib/cargo.mjs exports exec, lockedPackages,
lockedPackage, releaseBinary, cargoTargetDir and buildRelease.
Verified: bun test ./tests/foundation/harness.test.mjs -> 12 pass, 0 fail.
Next: Task B.3 adds the single-sample Docling fixture runner.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
MSG
```

---

### Task B.3: Docling fixture runner — listeners first, one sample, no polling

**Files:**
- Create: `qualification/docling/lib/runner.mjs`
- Test: `tests/foundation/harness.test.mjs` (imports, appended tests)

**Interfaces:**
- Consumes: Node `child_process.spawn` (`'error'` and `'close'` events, `child.stdin.end()`, `child.kill()`), `fs/promises.readFile('/proc/<pid>/status')`, Windows `powershell -NoProfile -NonInteractive -Command "(Get-Process -Id <pid> -ErrorAction SilentlyContinue).PeakWorkingSet64"`.
- **Stdout marker protocol** (produced by the Rust harness in B.4 when `OKF_DOCLING_HOLD=1`): after its receipt is on disk the process prints exactly one line
  `{"okf_docling":"done","only":"<fixture>","receipt":"<path>"}`
  and then blocks reading stdin until EOF. Other stdout lines are ignored. The runner samples the peak once on the first marker, then ends the child's stdin.
- Produces (exported from `qualification/docling/lib/runner.mjs`):
  - `DONE_MARKER: 'done'`
  - `parseDoneMarker(line: string): object | null`
  - `mergePeak(current: number | null, next: number | null): number | null`
  - `parseVmHwm(statusText: string): number | null` (bytes)
  - `samplePeakRss(pid: number, platform?: string): Promise<{ bytes: number | null, note: string }>`
  - `runFixtureProcess(options: { command: string, args?: string[], cwd?: string, env?: object, sample?: (pid: number) => { bytes, note } | Promise<{ bytes, note }>, timeoutMs?: number, onStdout?: (text: string) => void, onStderr?: (text: string) => void }): Promise<{ exitCode: number | null, signal: string | null, spawnError: string | null, timedOut: boolean, done: object | null, peakRssBytes: number | null, peakRssNote: string, stdout: string, stderr: string }>` — never rejects.

- [ ] **Step 1: write the failing tests.** Add to the import block:

```js
import {
  mergePeak,
  parseDoneMarker,
  parseVmHwm,
  runFixtureProcess,
  samplePeakRss,
} from '../../qualification/docling/lib/runner.mjs';
```

Append:

```js
const MARKER_LINE = "JSON.stringify({okf_docling:'done',only:'x.pdf',receipt:'r.json'})+'\\n'";
const HOLD_CHILD = `process.stdout.write(${MARKER_LINE});process.stdin.on('data',()=>{});process.stdin.on('end',()=>process.exit(0));`;

test('an instant-exit converter process resolves promptly and is recorded as unmeasured', async () => {
  const result = await within(runFixtureProcess({ command: process.execPath, args: ['-e', ''] }), 4000, 'instant exit');
  assert.equal(result.exitCode, 0);
  assert.equal(result.done, null);
  assert.equal(result.peakRssBytes, null);
  assert.match(result.peakRssNote, /exited before printing the done marker/);
});

test('the peak is sampled exactly once, while the process still waits on stdin', async () => {
  let calls = 0;
  let aliveAtSample = null;
  const sample = (pid) => {
    calls += 1;
    try {
      process.kill(pid, 0);
      aliveAtSample = true;
    } catch {
      aliveAtSample = false;
    }
    return { bytes: 4096, note: 'stub' };
  };
  const result = await within(
    runFixtureProcess({ command: process.execPath, args: ['-e', HOLD_CHILD], sample }),
    4000,
    'held child',
  );
  assert.equal(result.exitCode, 0);
  assert.deepEqual(result.done, { okf_docling: 'done', only: 'x.pdf', receipt: 'r.json' });
  assert.equal(calls, 1);
  assert.equal(aliveAtSample, true);
  assert.equal(result.peakRssBytes, 4096);
  assert.equal(result.peakRssNote, 'stub');
});

test('a null sample never overwrites a number', async () => {
  assert.equal(mergePeak(5, null), 5);
  assert.equal(mergePeak(null, 5), 5);
  assert.equal(mergePeak(5, 3), 5);
  assert.equal(mergePeak(3, 5), 5);
  assert.equal(mergePeak(null, null), null);
  assert.equal(mergePeak(5, Number.NaN), 5);
  let calls = 0;
  const sample = () => {
    calls += 1;
    return calls === 1 ? { bytes: 100, note: 'first' } : { bytes: null, note: 'late null' };
  };
  const twice = `const l=${MARKER_LINE};process.stdout.write(l+l);process.stdin.on('data',()=>{});process.stdin.on('end',()=>process.exit(0));`;
  const result = await within(
    runFixtureProcess({ command: process.execPath, args: ['-e', twice], sample }),
    4000,
    'double marker',
  );
  assert.equal(calls, 1);
  assert.equal(result.peakRssBytes, 100);
  assert.equal(result.peakRssNote, 'first');
});

test('a process that exits while the sample is in flight still resolves, with the reason recorded', async () => {
  const sample = () => new Promise((done) => setTimeout(() => done({ bytes: null, note: 'process gone' }), 300));
  const result = await within(
    runFixtureProcess({ command: process.execPath, args: ['-e', `process.stdout.write(${MARKER_LINE});`], sample }),
    4000,
    'exit during sample',
  );
  assert.equal(result.exitCode, 0);
  assert.equal(result.peakRssBytes, null);
  assert.equal(result.peakRssNote, 'process gone');
});

test('a converter that never finishes is killed at the fixture timeout', async () => {
  const result = await within(
    runFixtureProcess({ command: process.execPath, args: ['-e', 'setInterval(()=>{},1000)'], timeoutMs: 300 }),
    4000,
    'timeout',
  );
  assert.equal(result.timedOut, true);
  assert.notEqual(result.exitCode, 0);
  assert.equal(result.peakRssBytes, null);
  assert.match(result.peakRssNote, /killed after 300 ms/);
});

test('a missing converter binary is a result, not a hang or a throw', async () => {
  const result = await within(
    runFixtureProcess({ command: join(tmpdir(), 'okf-no-such-binary') }),
    4000,
    'missing binary',
  );
  assert.notEqual(result.exitCode, 0);
  assert.equal(result.peakRssBytes, null);
});

test('the platform sampler reads a real high-water mark and reports why when it cannot', async () => {
  assert.equal(parseVmHwm('Name:\tx\nVmHWM:\t    1234 kB\nVmRSS:\t     900 kB\n'), 1234 * 1024);
  assert.equal(parseVmHwm('Name:\tx\nVmRSS:\t 900 kB\n'), null);
  assert.equal(parseDoneMarker('Wrote C:\\out\\receipt.json'), null);
  assert.equal(parseDoneMarker('{"okf_docling":"other"}'), null);
  assert.deepEqual(parseDoneMarker(' {"okf_docling":"done","only":"a"} \r'), { okf_docling: 'done', only: 'a' });
  const own = await samplePeakRss(process.pid);
  if (process.platform === 'win32' || process.platform === 'linux') {
    assert.ok(own.bytes > 0, own.note);
  } else {
    assert.equal(own.bytes, null);
    assert.match(own.note, /no peak RSS source on platform/);
  }
  assert.deepEqual(await samplePeakRss(process.pid, 'sunos'), {
    bytes: null,
    note: 'no peak RSS source on platform sunos',
  });
});
```

- [ ] **Step 2: run and see the failure.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: exit 1, `Cannot find module '../../qualification/docling/lib/runner.mjs'`; 0 pass.

- [ ] **Step 3: implement.** Create `qualification/docling/lib/runner.mjs`:

```js
/**
 * Run one Docling fixture process and read its peak resident memory once.
 *
 * Protocol with qualification/docling/src/main.rs when OKF_DOCLING_HOLD=1:
 *   1. the converter process writes its per-fixture receipt, then prints one stdout line
 *      {"okf_docling":"done","only":"<fixture>","receipt":"<path>"};
 *   2. it blocks reading stdin until EOF;
 *   3. this runner samples the process peak exactly once (PeakWorkingSet64 on Windows,
 *      VmHWM on Linux: both are high-water marks the OS keeps, so one late read is the
 *      true peak), then closes stdin and the process exits.
 *
 * The 'error' and 'close' listeners are attached synchronously after spawn, before
 * anything is awaited, so a process that exits at once is always observed. There is no
 * polling loop and no second sample. runFixtureProcess never rejects: a missing peak is
 * a recorded reason, judged by the caller.
 */

import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';

export const DONE_MARKER = 'done';

const NOT_SAMPLED = 'converter exited before printing the done marker; peak RSS was not sampled';
const SAMPLER_TIMEOUT_MS = 30_000;
const WINDOWS_NOTE = 'PeakWorkingSet64 of the converter process, read once via Get-Process while it waited on stdin';
const LINUX_NOTE = 'VmHWM of the converter process, read once from /proc/<pid>/status while it waited on stdin';

/** The parsed done marker when `line` is one; null for every other stdout line. */
export function parseDoneMarker(line) {
  const text = line.trim();
  if (!text.startsWith('{')) return null;
  try {
    const value = JSON.parse(text);
    return value && value.okf_docling === DONE_MARKER ? value : null;
  } catch {
    return null;
  }
}

/** Largest valid sample; a null, NaN or non-positive value never replaces a number. */
export function mergePeak(current, next) {
  const valid = (value) => typeof value === 'number' && Number.isFinite(value) && value > 0;
  if (!valid(next)) return valid(current) ? current : null;
  return valid(current) ? Math.max(current, next) : next;
}

/** VmHWM in bytes from the text of /proc/<pid>/status, or null when the line is absent. */
export function parseVmHwm(statusText) {
  const match = /^VmHWM:\s+(\d+)\s+kB\r?$/m.exec(statusText);
  return match ? Number(match[1]) * 1024 : null;
}

function sampleWindows(pid) {
  return new Promise((resolveSample) => {
    let settled = false;
    let timer = null;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolveSample(value);
    };
    const ps = spawn(
      'powershell',
      [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        `(Get-Process -Id ${pid} -ErrorAction SilentlyContinue).PeakWorkingSet64`,
      ],
      { stdio: ['ignore', 'pipe', 'ignore'], windowsHide: true },
    );
    let buffer = '';
    timer = setTimeout(() => {
      ps.kill();
      finish({ bytes: null, note: `Get-Process did not answer within ${SAMPLER_TIMEOUT_MS} ms` });
    }, SAMPLER_TIMEOUT_MS);
    ps.stdout.on('data', (chunk) => {
      buffer += chunk;
    });
    ps.on('error', (error) => finish({ bytes: null, note: `powershell could not start: ${error.message}` }));
    ps.on('close', () => {
      const bytes = Number(buffer.trim());
      finish(
        Number.isFinite(bytes) && bytes > 0
          ? { bytes, note: WINDOWS_NOTE }
          : {
              bytes: null,
              note: `Get-Process reported no PeakWorkingSet64 for pid ${pid} (output ${JSON.stringify(buffer.trim())})`,
            },
      );
    });
  });
}

/** One reading of a live process's peak resident memory, with the source or the reason it is missing. */
export async function samplePeakRss(pid, platform = process.platform) {
  if (!Number.isInteger(pid) || pid <= 0) return { bytes: null, note: 'no process id to sample' };
  if (platform === 'win32') return sampleWindows(pid);
  if (platform === 'linux') {
    try {
      const bytes = parseVmHwm(await readFile(`/proc/${pid}/status`, 'utf8'));
      return bytes === null
        ? { bytes: null, note: `/proc/${pid}/status has no VmHWM line` }
        : { bytes, note: LINUX_NOTE };
    } catch (error) {
      return { bytes: null, note: `/proc/${pid}/status unreadable: ${error.message}` };
    }
  }
  return { bytes: null, note: `no peak RSS source on platform ${platform}` };
}

/** Spawn one fixture process, sample its peak on the done marker, release it, report what happened. */
export function runFixtureProcess({
  command,
  args = [],
  cwd,
  env,
  sample = samplePeakRss,
  timeoutMs = 900_000,
  onStdout,
  onStderr,
}) {
  return new Promise((resolveRun) => {
    const state = {
      stdout: '',
      stderr: '',
      pending: '',
      done: null,
      peak: null,
      note: NOT_SAMPLED,
      timedOut: false,
      sampling: null,
    };
    const report = (exit) => ({
      exitCode: exit.exitCode,
      signal: exit.signal,
      spawnError: exit.spawnError,
      timedOut: state.timedOut,
      done: state.done,
      peakRssBytes: state.peak,
      peakRssNote:
        state.timedOut && state.peak === null
          ? `converter was killed after ${timeoutMs} ms; peak RSS was not sampled`
          : state.note,
      stdout: state.stdout,
      stderr: state.stderr,
    });

    let child;
    try {
      child = spawn(command, args, { cwd, env, stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
    } catch (error) {
      resolveRun(report({ exitCode: null, signal: null, spawnError: error.message }));
      return;
    }

    let settled = false;
    let timer = null;
    const settle = (exit) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      // An in-flight sample finishes first so its result (or its reason) is in the report.
      Promise.resolve(state.sampling).then(() => resolveRun(report(exit)));
    };

    // Listeners first. Nothing in this function awaits before they exist.
    child.on('error', (error) => settle({ exitCode: null, signal: null, spawnError: error.message }));
    child.on('close', (code, signal) => settle({ exitCode: code, signal: signal ?? null, spawnError: null }));
    child.stdin?.on('error', () => {});

    timer = setTimeout(() => {
      state.timedOut = true;
      child.kill();
    }, timeoutMs);

    const release = () => {
      try {
        child.stdin?.end();
      } catch {
        // stdin already closed: the process is gone.
      }
    };

    const onLine = (line) => {
      const marker = parseDoneMarker(line);
      if (!marker || state.sampling) return; // single flight: only the first marker samples
      state.done = marker;
      state.sampling = Promise.resolve()
        .then(() => sample(child.pid))
        .then(
          (sampled) => {
            state.peak = mergePeak(state.peak, sampled?.bytes ?? null);
            state.note = sampled?.note ?? 'sampler returned no note';
          },
          (error) => {
            state.note = `peak RSS sampler failed: ${error.message}`;
          },
        )
        .finally(release);
    };

    child.stdout?.on('data', (chunk) => {
      const text = String(chunk);
      state.stdout += text;
      onStdout?.(text);
      state.pending += text;
      let newline = state.pending.indexOf('\n');
      while (newline >= 0) {
        onLine(state.pending.slice(0, newline));
        state.pending = state.pending.slice(newline + 1);
        newline = state.pending.indexOf('\n');
      }
    });
    child.stderr?.on('data', (chunk) => {
      const text = String(chunk);
      state.stderr += text;
      onStderr?.(text);
    });
  });
}
```

- [ ] **Step 4: run and see it pass.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: `19 pass`, `0 fail`.
  Removal check (do it, then undo it): delete the `child.on('close', …)` line and re-run — "an instant-exit converter process resolves promptly" must fail with `instant exit did not settle within 4000 ms`. Restore the line. (The executor is not `async`, so nothing can be awaited before the listeners by construction.)

- [ ] **Step 5: commit.**

```sh
git add qualification/docling/lib/runner.mjs tests/foundation/harness.test.mjs
git commit -F - <<'MSG'
feat(qualify): run Docling fixtures with listeners first and one peak sample.

Why: run.mjs:142-158 awaited a memory sample before attaching the child's
close listener, so a converter that exited during that await was never seen to
exit and the run hung until the 30-minute timeout; line 153 assigned instead of
max-merging, so a late null could erase a valid sample; a new powershell.exe
was spawned every 50 ms with no in-flight guard.
What changed: qualification/docling/lib/runner.mjs exports runFixtureProcess,
samplePeakRss, mergePeak, parseVmHwm and parseDoneMarker. Listeners attach
synchronously after spawn; the peak is read once when the child prints its
done marker and waits on stdin; Linux reads /proc/<pid>/status with readFile;
a missing sample is a recorded reason, never a throw; a per-fixture timeout
kills a stuck converter.
Verified: bun test ./tests/foundation/harness.test.mjs -> 19 pass, 0 fail.
Next: Task B.4 makes the Rust harness print the done marker and record stages.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
MSG
```

---

### Task B.4: Docling Rust harness — truthful stages, done marker, lint errors

**Files:**
- Modify: `qualification/docling/src/main.rs:1-614` (whole file replaced)
- Test: unit tests in the same file (`#[cfg(test)] mod tests`, last item)

**Interfaces:**
- Consumes: `docling::{ConversionStatus, DocumentConverter, SourceDocument}` as quoted in the facts section. Environment: `DOCLING_RS_MODELS_DIR` (required), `OKF_DOCLING_ONLY` (required: one catalog fixture name, `must_fail_truncated.pdf`, `must_fail`, or `timeout_probe`), `OKF_DOCLING_CRATE_VERSION` (required: version string read from `Cargo.lock` by the orchestrator), `OKF_DOCLING_FIXTURES`, `OKF_DOCLING_OUT`, `OKF_DOCLING_HOLD` (`1` = print the done marker and wait for stdin EOF).
- Produces:
  - Per-fixture `<OKF_DOCLING_OUT>/receipt.json`: `{ converter_crate, fixtures_dir, models_dir, receipts: FixtureReceipt[0..1], summary, timeout_case: FixtureReceipt | null }`.
  - `FixtureReceipt` gains `stage: "source" | "converter_error" | "converter_status"` and `finding: string | null`; `status` becomes `"Success" | "PartialSuccess" | "Failure" | null` (null unless the stage is `converter_status`). The value `pre_converter_error` no longer exists.
  - Outcomes: must-fail → `FAIL_before_converter` (stage `source`), `PASS_explicit_failure` (stage `converter_error`, or `converter_status` with `Failure`), `FAIL_expected_failure` + `finding: "converter accepts truncated PDF"` (Success or PartialSuccess). Supported fixtures → `FAIL_source_error`, `FAIL_converter_error`, or the existing status-based outcomes. Timeout probe → `PASS` only for stage `converter_status`, status `PartialSuccess` and a `pipeline` error whose message contains "timeout"; otherwise `FAIL_timeout_not_honoured`. Any stage with a changed fixture hash → `FAIL_original_mutated`.
  - Stdout done marker, one line, only when `OKF_DOCLING_HOLD=1`: `{"okf_docling":"done","only":"<OKF_DOCLING_ONLY>","receipt":"<path>"}`; then the process reads stdin to EOF and exits 0.
- Removed: the all-fixtures-in-one-process mode (`run_all`, `convert_catalog`); see Deviations.

- [ ] **Step 1: write the failing tests.** Append this module to the end of the current `qualification/docling/src/main.rs` (after line 614):

```rust
#[cfg(test)]
mod tests {
    use super::{
        ConversionStatus, DONE, DoneMarker, ErrorReceipt, Expected, MUST_FAIL_FINDING,
        MUST_FAIL_NAME, Reached, Stage, TIMEOUT_PROBE, fixture_catalog, judge, lookup_only,
        timeout_honoured,
    };
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The value of an option that must be present; `what` names it in the failure.
    fn some<T>(value: Option<T>, what: &str) -> Result<T, String> {
        value.ok_or_else(|| format!("missing {what}"))
    }

    #[test]
    fn must_fail_is_judged_by_the_stage_it_reached() -> TestResult {
        let never_reached = judge(Expected::ExplicitFailure, Reached::Source, true);
        assert_eq!(never_reached.outcome, "FAIL_before_converter");
        assert_eq!(never_reached.finding, None);

        let refused = judge(Expected::ExplicitFailure, Reached::ConverterError, true);
        assert_eq!(refused.outcome, "PASS_explicit_failure");
        assert_eq!(refused.finding, None);

        let failure = Reached::ConverterStatus {
            status: ConversionStatus::Failure,
            markdown: "",
        };
        assert_eq!(
            judge(Expected::ExplicitFailure, failure, true).outcome,
            "PASS_explicit_failure"
        );

        for status in [ConversionStatus::Success, ConversionStatus::PartialSuccess] {
            let reached = Reached::ConverterStatus {
                status,
                markdown: "text",
            };
            let accepted = judge(Expected::ExplicitFailure, reached, true);
            assert_eq!(accepted.outcome, "FAIL_expected_failure");
            assert_eq!(some(accepted.finding, "finding")?, MUST_FAIL_FINDING);
        }

        let mutated = judge(Expected::ExplicitFailure, Reached::ConverterError, false);
        assert_eq!(mutated.outcome, "FAIL_original_mutated");
        Ok(())
    }

    #[test]
    fn a_converter_error_never_passes_a_supported_fixture_or_the_timeout_probe() -> TestResult {
        let (_, role, expected) = lookup_only(TIMEOUT_PROBE)?;
        assert_eq!(role, TIMEOUT_PROBE);
        assert_eq!(
            judge(expected, Reached::ConverterError, true).outcome,
            "FAIL_converter_error"
        );
        assert_eq!(
            judge(expected, Reached::Source, true).outcome,
            "FAIL_source_error"
        );

        let errors = [ErrorReceipt {
            component_type: "document_backend".to_owned(),
            error_message: "document timeout of 0.001s exceeded after 0 of 1 pages".to_owned(),
            module_name: "pipeline".to_owned(),
        }];
        assert!(!timeout_honoured(Stage::ConverterError, None, &errors));
        assert!(!timeout_honoured(Stage::ConverterStatus, Some("Success"), &errors));
        assert!(!timeout_honoured(Stage::ConverterStatus, Some("PartialSuccess"), &[]));
        assert!(timeout_honoured(Stage::ConverterStatus, Some("PartialSuccess"), &errors));
        Ok(())
    }

    #[test]
    fn stage_names_and_the_done_marker_match_the_orchestrator_protocol() -> TestResult {
        assert_eq!(serde_json::to_value(Stage::Source)?, "source");
        assert_eq!(serde_json::to_value(Stage::ConverterError)?, "converter_error");
        assert_eq!(serde_json::to_value(Stage::ConverterStatus)?, "converter_status");
        let marker = DoneMarker {
            okf_docling: DONE,
            only: "sample_sheet.xlsx",
            receipt: Path::new("out/receipt.json"),
        };
        assert_eq!(
            serde_json::to_string(&marker)?,
            r#"{"okf_docling":"done","only":"sample_sheet.xlsx","receipt":"out/receipt.json"}"#
        );
        Ok(())
    }

    #[test]
    fn catalog_and_must_fail_are_exactly_the_recorded_sources() -> TestResult {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/documents/SOURCES.json");
        let sources: serde_json::Value = serde_json::from_str(&fs::read_to_string(path)?)?;
        let files = sources.get("files").and_then(serde_json::Value::as_object);
        let recorded: BTreeSet<String> = some(files, "files object")?.keys().cloned().collect();
        let harness: BTreeSet<String> = fixture_catalog()
            .into_iter()
            .map(|(name, _, _)| name.to_owned())
            .chain(std::iter::once(MUST_FAIL_NAME.to_owned()))
            .collect();
        assert_eq!(harness, recorded);
        Ok(())
    }
}
```

`TestResult` and `some` are declared locally because `tests/support/check.rs` is produced by package E in wave 2 and cannot be included from this crate yet; the names and signatures match the shared idiom.

- [ ] **Step 2: run and see the failure (PowerShell).**
  Run: `cargo test --locked -p okf-qualify-docling`
  Expected: compile errors, among them ``error[E0432]: unresolved imports `super::DONE`, `super::DoneMarker`, `super::Reached`, `super::Stage`, …`` and ``no `judge` in the root``. (The first build of the docling tree in the test profile takes several minutes.)

- [ ] **Step 3: implement.** Replace everything in `qualification/docling/src/main.rs` **above** the `#[cfg(test)] mod tests` block with the code below; the tests module from Step 1 stays as the last item.

```rust
//! Direct-library Docling qualification harness.
//!
//! Instantiates `docling::DocumentConverter` against real fixtures and assets.
//! Does not implement `okf_jawn_core::conversion::Converter` or any product port.
//!
//! The orchestrator (`qualification/docling/run.mjs`) runs one process per fixture,
//! named by `OKF_DOCLING_ONLY`, so peak resident memory is per fixture. With
//! `OKF_DOCLING_HOLD=1` the process prints one JSON "done" line on stdout once its
//! receipt is on disk and then blocks on stdin until EOF; the orchestrator reads this
//! process's peak memory exactly once during that wait.
//!
//! Every receipt records the stage the fixture reached: `source` (the file never
//! reached the converter), `converter_error` (`convert` returned `Err`) or
//! `converter_status` (`convert` returned `Ok`). A must-fail fixture passes only at
//! the last two, and only when the converter refused it.

use docling::{ConversionStatus, DocumentConverter, SourceDocument};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Printed once on stdout when the receipt is on disk and the process is about to wait.
#[derive(Serialize)]
struct DoneMarker<'a> {
    okf_docling: &'static str,
    only: &'a str,
    receipt: &'a Path,
}

/// Process inputs read once from the environment.
struct Environment {
    converter_version: String,
    fixtures_dir: String,
    hold: bool,
    models_dir: String,
    out_dir: String,
}

#[derive(Clone, Serialize)]
struct ErrorReceipt {
    component_type: String,
    error_message: String,
    module_name: String,
}

/// What a fixture must produce for the gate to pass.
#[derive(Clone, Copy)]
enum Expected {
    /// Supported extraction: `Success`, non-empty Markdown, optional substring.
    SuccessNonEmpty { must_contain: Option<&'static str> },
    /// The converter itself must refuse the input.
    ExplicitFailure,
}

#[derive(Clone, Serialize)]
struct FixtureReceipt {
    converter_version: String,
    elapsed_ms: u128,
    errors: Vec<ErrorReceipt>,
    expected: String,
    /// Set when a must-fail fixture was accepted; a decision for the owner.
    finding: Option<String>,
    fixture: String,
    markdown_chars: usize,
    markdown_nonempty: bool,
    must_contain_ok: Option<bool>,
    original_unchanged: bool,
    outcome: String,
    page_image_count: usize,
    page_provenance: Vec<PageProvenance>,
    path: PathBuf,
    /// Always null in-process: authored Rust forbids the OS FFI needed to sample RSS.
    /// The orchestrator attaches the peak it read for this process.
    peak_rss_bytes: Option<u64>,
    role: String,
    settings: BTreeMap<String, serde_json::Value>,
    sha256_after: String,
    sha256_before: String,
    stage: Stage,
    /// The `ConversionStatus` label; null unless `convert` returned `Ok`.
    status: Option<String>,
}

/// One fixture about to be handed to the converter.
struct FixtureRun<'a> {
    expected: Expected,
    path: &'a Path,
    role: &'a str,
    session: &'a Session,
    sha_before: String,
    started: Instant,
}

/// The gate's verdict on one observation.
struct Judgement {
    finding: Option<String>,
    must_contain_ok: Option<bool>,
    outcome: &'static str,
}

/// What came back from the converter call, with the evidence to record.
struct Observed<'a> {
    elapsed_ms: u128,
    errors: Vec<ErrorReceipt>,
    page_provenance: Vec<PageProvenance>,
    reached: Reached<'a>,
}

#[derive(Clone, Serialize)]
struct PageProvenance {
    page_no: usize,
    has_image: bool,
}

#[derive(Serialize)]
struct QualificationReport {
    converter_crate: String,
    fixtures_dir: PathBuf,
    models_dir: PathBuf,
    receipts: Vec<FixtureReceipt>,
    summary: BTreeMap<String, String>,
    timeout_case: Option<FixtureReceipt>,
}

/// How far a fixture travelled, with what the gate judges at that point.
#[derive(Clone, Copy)]
enum Reached<'a> {
    /// `SourceDocument::from_file` failed; the converter never saw the fixture.
    Source,
    /// `DocumentConverter::convert` returned `Err`.
    ConverterError,
    /// `DocumentConverter::convert` returned `Ok` with this status and Markdown.
    ConverterStatus {
        status: ConversionStatus,
        markdown: &'a str,
    },
}

/// Converter identity and settings recorded on every receipt of this process.
struct Session {
    converter_version: String,
    settings: BTreeMap<String, serde_json::Value>,
}

/// The stage name written to the receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Stage {
    Source,
    ConverterError,
    ConverterStatus,
}

const DONE: &str = "done";
const MUST_FAIL_FINDING: &str = "converter accepts truncated PDF";
const MUST_FAIL_NAME: &str = "must_fail_truncated.pdf";
const TIMEOUT_BUDGET_MS: u64 = 1;
const TIMEOUT_PROBE: &str = "timeout_probe";

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn status_label(status: ConversionStatus) -> &'static str {
    match status {
        ConversionStatus::Failure => "Failure",
        ConversionStatus::PartialSuccess => "PartialSuccess",
        ConversionStatus::Success => "Success",
    }
}

fn expected_label(expected: Expected) -> &'static str {
    match expected {
        Expected::SuccessNonEmpty { .. } => "SuccessNonEmpty",
        Expected::ExplicitFailure => "ExplicitFailure",
    }
}

fn stage_of(reached: Reached<'_>) -> Stage {
    match reached {
        Reached::Source => Stage::Source,
        Reached::ConverterError => Stage::ConverterError,
        Reached::ConverterStatus { .. } => Stage::ConverterStatus,
    }
}

fn page_provenance(result: &docling::ConversionResult) -> Vec<PageProvenance> {
    let mut pages: Vec<PageProvenance> = result
        .document
        .page_images
        .iter()
        .map(|(page_no, image)| PageProvenance {
            page_no: *page_no,
            has_image: !image.mimetype.is_empty(),
        })
        .collect();
    pages.sort_by_key(|page| page.page_no);
    pages
}

fn verdict(outcome: &'static str) -> Judgement {
    Judgement {
        finding: None,
        must_contain_ok: None,
        outcome,
    }
}

/// Judge one observation. A mutated original fails whatever else happened.
fn judge(expected: Expected, reached: Reached<'_>, original_unchanged: bool) -> Judgement {
    if !original_unchanged {
        return verdict("FAIL_original_mutated");
    }
    match expected {
        Expected::ExplicitFailure => judge_must_fail(reached),
        Expected::SuccessNonEmpty { must_contain } => judge_success(must_contain, reached),
    }
}

/// PASS only when the converter itself refused the input. A source-stage error never
/// reached the converter; an accepted input is a finding, and the fixture stays as it is.
fn judge_must_fail(reached: Reached<'_>) -> Judgement {
    match reached {
        Reached::Source => verdict("FAIL_before_converter"),
        Reached::ConverterError
        | Reached::ConverterStatus {
            status: ConversionStatus::Failure,
            ..
        } => verdict("PASS_explicit_failure"),
        Reached::ConverterStatus {
            status: ConversionStatus::Success | ConversionStatus::PartialSuccess,
            ..
        } => Judgement {
            finding: Some(MUST_FAIL_FINDING.to_owned()),
            must_contain_ok: None,
            outcome: "FAIL_expected_failure",
        },
    }
}

fn judge_success(must_contain: Option<&str>, reached: Reached<'_>) -> Judgement {
    let (status, markdown) = match reached {
        Reached::Source => return verdict("FAIL_source_error"),
        Reached::ConverterError => return verdict("FAIL_converter_error"),
        Reached::ConverterStatus { status, markdown } => (status, markdown),
    };
    let must_contain_ok = must_contain.map(|needle| markdown.contains(needle));
    let outcome = if matches!(status, ConversionStatus::Failure) {
        "FAIL_unexpected_failure"
    } else if markdown.trim().is_empty() {
        "FAIL_empty_markdown"
    } else if must_contain_ok == Some(false) {
        "FAIL_missing_expected_text"
    } else if matches!(status, ConversionStatus::Success) {
        "PASS"
    } else {
        // `PartialSuccess` with content is recorded but is not a pass for a supported fixture.
        "FAIL_partial_not_success"
    };
    Judgement {
        finding: None,
        must_contain_ok,
        outcome,
    }
}

/// A buffered `convert` reports a spent document budget as `PartialSuccess` with one
/// `pipeline` error item. An `Err` from the converter is never a honoured timeout.
fn timeout_honoured(stage: Stage, status: Option<&str>, errors: &[ErrorReceipt]) -> bool {
    stage == Stage::ConverterStatus
        && status == Some("PartialSuccess")
        && errors.iter().any(|item| {
            item.module_name == "pipeline" && item.error_message.to_lowercase().contains("timeout")
        })
}

fn build_receipt(run: &FixtureRun<'_>, observed: Observed<'_>) -> Result<FixtureReceipt, String> {
    let sha_after = sha256_file(run.path)?;
    let original_unchanged = run.sha_before == sha_after;
    let judgement = judge(run.expected, observed.reached, original_unchanged);
    let (status, markdown) = match observed.reached {
        Reached::ConverterStatus { status, markdown } => {
            (Some(status_label(status).to_owned()), markdown)
        }
        Reached::Source | Reached::ConverterError => (None, ""),
    };
    Ok(FixtureReceipt {
        converter_version: run.session.converter_version.clone(),
        elapsed_ms: observed.elapsed_ms,
        errors: observed.errors,
        expected: expected_label(run.expected).to_owned(),
        finding: judgement.finding,
        fixture: run
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        markdown_chars: markdown.chars().count(),
        markdown_nonempty: !markdown.trim().is_empty(),
        must_contain_ok: judgement.must_contain_ok,
        original_unchanged,
        outcome: judgement.outcome.to_owned(),
        page_image_count: observed.page_provenance.len(),
        page_provenance: observed.page_provenance,
        path: run.path.to_path_buf(),
        peak_rss_bytes: None,
        role: run.role.to_owned(),
        settings: run.session.settings.clone(),
        sha256_after: sha_after,
        sha256_before: run.sha_before.clone(),
        stage: stage_of(observed.reached),
        status,
    })
}

/// A receipt for a fixture that produced no `ConversionResult`.
fn refused(
    run: &FixtureRun<'_>,
    reached: Reached<'_>,
    module_name: &str,
    message: String,
) -> Result<FixtureReceipt, String> {
    let component_type = match reached {
        Reached::Source => "source",
        Reached::ConverterError | Reached::ConverterStatus { .. } => "converter",
    };
    build_receipt(
        run,
        Observed {
            elapsed_ms: run.started.elapsed().as_millis(),
            errors: vec![ErrorReceipt {
                component_type: component_type.to_owned(),
                error_message: message,
                module_name: module_name.to_owned(),
            }],
            page_provenance: Vec::new(),
            reached,
        },
    )
}

fn convert_fixture(
    converter: &DocumentConverter,
    run: &FixtureRun<'_>,
) -> Result<FixtureReceipt, String> {
    let source = match SourceDocument::from_file(run.path) {
        Ok(source) => source,
        Err(error) => {
            return refused(
                run,
                Reached::Source,
                "SourceDocument::from_file",
                error.to_string(),
            );
        }
    };
    let result = match converter.convert(source) {
        Ok(result) => result,
        Err(error) => {
            return refused(
                run,
                Reached::ConverterError,
                "DocumentConverter::convert",
                error.to_string(),
            );
        }
    };
    let elapsed_ms = run.started.elapsed().as_millis();
    let markdown = result.document.export_to_markdown();
    let errors = result
        .errors
        .iter()
        .map(|item| ErrorReceipt {
            component_type: item.component_type.clone(),
            error_message: item.error_message.clone(),
            module_name: item.module_name.clone(),
        })
        .collect();
    build_receipt(
        run,
        Observed {
            elapsed_ms,
            errors,
            page_provenance: page_provenance(&result),
            reached: Reached::ConverterStatus {
                status: result.status,
                markdown: &markdown,
            },
        },
    )
}

fn fixture_catalog() -> [(&'static str, &'static str, Expected); 10] {
    [
        (
            "sample_with_image.docx",
            "docx_with_images",
            Expected::SuccessNonEmpty {
                must_contain: Some("OKF"),
            },
        ),
        (
            "sample_sheet.xlsx",
            "xlsx",
            Expected::SuccessNonEmpty {
                must_contain: Some("Widget"),
            },
        ),
        (
            "born_digital_text.pdf",
            "born_digital_pdf",
            Expected::SuccessNonEmpty {
                must_contain: Some("Born-digital"),
            },
        ),
        (
            "scanned_image_only.pdf",
            "scanned_or_image_pdf",
            Expected::SuccessNonEmpty { must_contain: None },
        ),
        (
            "table_heavy.pdf",
            "table_heavy_pdf",
            Expected::SuccessNonEmpty {
                must_contain: Some("Name"),
            },
        ),
        (
            "sample_image.png",
            "image",
            Expected::SuccessNonEmpty { must_contain: None },
        ),
        (
            "corpus/word_sample.docx",
            "corpus_docx",
            Expected::SuccessNonEmpty {
                must_contain: Some("Summer"),
            },
        ),
        (
            "corpus/xlsx_01.xlsx",
            "corpus_xlsx",
            Expected::SuccessNonEmpty {
                must_contain: Some("col-1"),
            },
        ),
        (
            "corpus/powerpoint_sample.pptx",
            "corpus_pptx",
            Expected::SuccessNonEmpty {
                must_contain: Some("Test Table"),
            },
        ),
        (
            "corpus/redp5110_sampled.pdf",
            "corpus_pdf",
            Expected::SuccessNonEmpty {
                must_contain: Some("IBM"),
            },
        ),
    ]
}

fn lookup_only(only: &str) -> Result<(&'static str, &'static str, Expected), String> {
    if only == MUST_FAIL_NAME || only == "must_fail" {
        return Ok((MUST_FAIL_NAME, "must_fail", Expected::ExplicitFailure));
    }
    if only == TIMEOUT_PROBE {
        return Ok((
            "scanned_image_only.pdf",
            TIMEOUT_PROBE,
            Expected::SuccessNonEmpty { must_contain: None },
        ));
    }
    for (name, role, expected) in fixture_catalog() {
        if name == only {
            return Ok((name, role, expected));
        }
    }
    Err(format!(
        "OKF_DOCLING_ONLY={only} is not a known fixture, must_fail, or timeout_probe"
    ))
}

fn write_report(out_dir: &str, report: &QualificationReport) -> Result<PathBuf, String> {
    fs::create_dir_all(out_dir).map_err(|error| error.to_string())?;
    let report_path = PathBuf::from(out_dir).join("receipt.json");
    let json = serde_json::to_string_pretty(report).map_err(|error| error.to_string())?;
    fs::write(&report_path, format!("{json}\n")).map_err(|error| error.to_string())?;
    writeln!(io::stdout(), "Wrote {}", report_path.display()).map_err(|error| error.to_string())?;
    Ok(report_path)
}

/// Print the done marker, then wait until the orchestrator closes stdin.
fn hold_for_sample(only: &str, receipt: &Path) -> Result<(), String> {
    let marker = DoneMarker {
        okf_docling: DONE,
        only,
        receipt,
    };
    let line = serde_json::to_string(&marker).map_err(|error| error.to_string())?;
    {
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{line}").map_err(|error| error.to_string())?;
        stdout.flush().map_err(|error| error.to_string())?;
    }
    let _bytes = io::copy(&mut io::stdin().lock(), &mut io::sink())
        .map_err(|error| format!("wait for stdin EOF: {error}"))?;
    Ok(())
}

fn build_session(environment: &Environment, artifacts: &Path, budget_ms: Option<u64>) -> Session {
    let mut settings = BTreeMap::new();
    settings.insert(
        "artifacts_dir".to_owned(),
        serde_json::Value::String(artifacts.display().to_string()),
    );
    settings.insert(
        "models_dir".to_owned(),
        serde_json::Value::String(environment.models_dir.clone()),
    );
    settings.insert(
        "ocr_lang".to_owned(),
        serde_json::Value::String("en".to_owned()),
    );
    if let Some(ms) = budget_ms {
        settings.insert(
            "document_timeout_ms".to_owned(),
            serde_json::Value::from(ms),
        );
    }
    Session {
        converter_version: environment.converter_version.clone(),
        settings,
    }
}

fn build_converter(artifacts: &Path, budget_ms: Option<u64>) -> DocumentConverter {
    DocumentConverter::new()
        .ocr_lang("en")
        .artifacts_dir(artifacts.display().to_string())
        .document_timeout(budget_ms.map(Duration::from_millis))
}

fn run_one(only: &str, environment: &Environment) -> Result<(), String> {
    let (name, role, expected) = lookup_only(only)?;
    let is_timeout = role == TIMEOUT_PROBE;
    let budget_ms = is_timeout.then_some(TIMEOUT_BUDGET_MS);
    let artifacts = tempfile::tempdir().map_err(|error| error.to_string())?;
    let session = build_session(environment, artifacts.path(), budget_ms);
    let path = PathBuf::from(&environment.fixtures_dir).join(name);
    if !path.is_file() {
        return Err(format!("missing fixture {}", path.display()));
    }
    let converter = build_converter(artifacts.path(), budget_ms);
    let run = FixtureRun {
        expected,
        path: &path,
        role,
        session: &session,
        sha_before: sha256_file(&path)?,
        started: Instant::now(),
    };
    let mut receipt = convert_fixture(&converter, &run)?;
    if is_timeout {
        let honoured = timeout_honoured(receipt.stage, receipt.status.as_deref(), &receipt.errors);
        receipt.outcome = if honoured {
            "PASS".to_owned()
        } else {
            "FAIL_timeout_not_honoured".to_owned()
        };
    }

    let key = if is_timeout { TIMEOUT_PROBE } else { name };
    let mut summary = BTreeMap::new();
    summary.insert(key.to_owned(), receipt.outcome.clone());
    let (receipts, timeout_case) = if is_timeout {
        (Vec::new(), Some(receipt))
    } else {
        (vec![receipt], None)
    };
    let report = QualificationReport {
        converter_crate: environment.converter_version.clone(),
        fixtures_dir: PathBuf::from(&environment.fixtures_dir),
        models_dir: PathBuf::from(&environment.models_dir),
        receipts,
        summary,
        timeout_case,
    };
    let report_path = write_report(&environment.out_dir, &report)?;
    // The process exits 0 once its receipt is written; the orchestrator judges outcomes.
    if environment.hold {
        hold_for_sample(only, &report_path)?;
    }
    Ok(())
}

fn read_environment() -> Result<Environment, String> {
    let models_dir = env::var("DOCLING_RS_MODELS_DIR")
        .map_err(|_| "DOCLING_RS_MODELS_DIR must be set to the verified models cache".to_owned())?;
    let converter_version = env::var("OKF_DOCLING_CRATE_VERSION")
        .ok()
        .map(|version| version.trim().to_owned())
        .filter(|version| !version.is_empty())
        .map(|version| format!("docling {version}"))
        .ok_or_else(|| {
            "OKF_DOCLING_CRATE_VERSION must be the docling version pinned in Cargo.lock; \
             run.mjs reads it there"
                .to_owned()
        })?;
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixtures_dir = env::var("OKF_DOCLING_FIXTURES").unwrap_or_else(|_| {
        manifest_dir
            .join("../../tests/fixtures/documents")
            .to_string_lossy()
            .into_owned()
    });
    let out_dir = env::var("OKF_DOCLING_OUT").unwrap_or_else(|_| {
        manifest_dir
            .join("../../.artifacts/qualification/docling")
            .to_string_lossy()
            .into_owned()
    });
    let hold = env::var("OKF_DOCLING_HOLD").is_ok_and(|value| value == "1");
    Ok(Environment {
        converter_version,
        fixtures_dir,
        hold,
        models_dir,
        out_dir,
    })
}

fn run() -> Result<(), String> {
    let environment = read_environment()?;
    let only = env::var("OKF_DOCLING_ONLY")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "OKF_DOCLING_ONLY must name one catalog fixture, must_fail_truncated.pdf or \
             timeout_probe: one process per fixture keeps peak memory per fixture"
                .to_owned()
        })?;
    run_one(&only, &environment)
}

fn main() -> Result<(), String> {
    run().inspect_err(|error| {
        let _ = writeln!(io::stderr(), "okf-qualify-docling: {error}");
    })
}
```

What this fixes against the workspace lint policy: `before_converter_receipt` with 9 parameters (old `:160-170`, `clippy::too_many_arguments`) is replaced by `refused(run, reached, module_name, message)` over the `FixtureRun` struct; the `const` after functions (old `:374`, `clippy::arbitrary_source_item_ordering` with `clippy.toml` grouping types → constants → impls → functions → `mod`) moves into the constants block after the types; the hard-coded `"1.93.5"` (old `:81-87`) is gone.

- [ ] **Step 4: run the Rust gate (PowerShell).**
  Run: `cargo fmt -p okf-qualify-docling` then `cargo fmt --check -p okf-qualify-docling`
  Expected: second command exits 0 with no output.
  Run: `cargo clippy --locked -p okf-qualify-docling --all-targets -- -D warnings`
  Expected: `Finished` with no warnings or errors. If Clippy names a lint in this file, fix the code it points at; never add an attribute. This file could not be compiled while the plan was written, so a residual pedantic finding is possible and is your defect to fix within three attempts.
  Run: `cargo test --locked -p okf-qualify-docling`
  Expected: `test result: ok. 4 passed; 0 failed`.
  Run: `cargo build --locked --release -p okf-qualify-docling`
  Expected: `Finished \`release\` profile`.

- [ ] **Step 5: smoke the hold protocol end to end without model assets (PowerShell, from the worktree root).** XLSX uses a declarative backend (`converter.rs:982-985`) and needs no models.

```powershell
bun -e "import { runFixtureProcess } from './qualification/docling/lib/runner.mjs'; import { cargoTargetDir, releaseBinary } from './qualification/lib/cargo.mjs'; const bin = releaseBinary(await cargoTargetDir('.'), 'okf-qualify-docling'); const r = await runFixtureProcess({ command: bin, env: { ...process.env, DOCLING_RS_MODELS_DIR: 'unused-for-xlsx', OKF_DOCLING_CRATE_VERSION: '1.93.5', OKF_DOCLING_ONLY: 'sample_sheet.xlsx', OKF_DOCLING_OUT: '.artifacts/qualification/docling/smoke', OKF_DOCLING_HOLD: '1' } }); console.log(JSON.stringify({ exit: r.exitCode, done: r.done, peak: r.peakRssBytes, note: r.peakRssNote }));"
```

  Expected: one JSON line with `"exit":0`, `"done":{"okf_docling":"done","only":"sample_sheet.xlsx","receipt":"…smoke…receipt.json"}`, `"peak"` a positive integer, and the `PeakWorkingSet64` note. `.artifacts/qualification/docling/smoke/receipt.json` has `receipts[0].stage == "converter_status"`, `status == "Success"`, `outcome == "PASS"`, `converter_version == "docling 1.93.5"`.

- [ ] **Step 6: commit.**

```sh
git add qualification/docling/src/main.rs
git commit -F - <<'MSG'
fix(qualify): record the stage a Docling fixture reached and hold for one sample.

Why: main.rs:233-247 routed an Err from converter.convert through
before_converter_receipt and recorded pre_converter_error /
FAIL_before_converter although the converter ran; the same Err path counted as
PASS for the timeout probe (:293-296). before_converter_receipt took 9
parameters, a const followed functions (:374) and the converter version was a
hard-coded string (:81-87); all three fail the workspace lint policy. SPEC 5:
"No fake success ... may stand in for the converter."
What changed: receipts carry stage (source | converter_error |
converter_status), a nullable status and a finding. must_fail passes only when
the converter refused the input; a source-stage error is FAIL_before_converter;
Success or PartialSuccess is FAIL_expected_failure with the finding "converter
accepts truncated PDF". The timeout probe passes only on PartialSuccess with a
pipeline timeout item. With OKF_DOCLING_HOLD=1 the process prints one JSON done
line and waits for stdin EOF. The version comes from OKF_DOCLING_CRATE_VERSION.
The all-fixtures-in-one-process mode is removed: it could not give per-fixture
peak memory.
Verified: cargo fmt --check -p okf-qualify-docling -> clean; cargo clippy
--locked -p okf-qualify-docling --all-targets -- -D warnings -> clean; cargo
test --locked -p okf-qualify-docling -> 4 passed; cargo build --locked
--release -p okf-qualify-docling -> Finished; xlsx hold smoke -> exit 0, done
marker, positive peak, stage converter_status.
Next: Task B.5 composes the receipt and makes run.mjs a thin entry point.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
MSG
```

---

### Task B.5: Docling receipt composition and thin `run.mjs`

**Files:**
- Create: `qualification/docling/lib/receipt.mjs`
- Modify: `qualification/docling/run.mjs:1-311` (whole file replaced)
- Modify: `tests/fixtures/documents/SOURCES.json:64` (one `notes` line replaced, one `pass_when` line added)
- Test: `tests/foundation/harness.test.mjs` (imports, appended tests)

**Interfaces:**
- Consumes: `receiptHeader`, `recordReceipt` (B.1); `buildRelease`, `lockedPackage`, `lockedPackages` (B.2); `runFixtureProcess` (B.3); per-fixture receipts and the done marker (B.4).
- Produces (exported from `qualification/docling/lib/receipt.mjs`, pure, no imports):
  - `MUST_FAIL: 'must_fail_truncated.pdf'`, `TIMEOUT_PROBE: 'timeout_probe'`
  - `FIXTURE_RUNS: string[]` (12 entries, order of execution)
  - `DOCLING_INPUTS: string[]`
  - `fixtureEntry({ only, run, report }): object`
  - `buildDoclingReceipt({ header, converter, platform, modelsDir, runs, finishedAt }): object`
- CLI: `bun qualification/docling/run.mjs [--record]`. Exit 0 only when `receipt.result === 'PASS'`; the receipt is written (and recorded with `--record`) before a FAIL exits non-zero.
- Docling receipt shape (`.artifacts/qualification/docling/receipt.json`):

```json
{
  "git_sha": "<40 hex>", "inputs": ["qualification/docling", "qualification/lib", "tests/fixtures/documents", "Cargo.toml", "Cargo.lock"], "produced_at": "<ISO>",
  "component": "docling-library-qualification",
  "finished_at": "<ISO>",
  "result": "PASS | FAIL",
  "finding": null,
  "converter": { "crate": "docling", "version": "1.93.5", "checksum": "<hex>", "docling_core_versions": ["1.93.6"], "source": "Cargo.lock" },
  "build": "cargo build --locked --release -p okf-qualify-docling",
  "platform": "win32",
  "peak_memory_source": "PeakWorkingSet64 … | VmHWM …",
  "fixtures_dir": "tests/fixtures/documents",
  "models_dir": "<path>",
  "assets_manifest": ".artifacts/qualification/docling/assets.json",
  "summary": { "<fixture>": "<outcome>", "timeout_probe": "<outcome>" },
  "memory_summary": { "<fixture>": "PASS | FAIL_not_measured" },
  "receipts": [ { "…FixtureReceipt…": "", "peak_rss_bytes": 123456789, "peak_rss_note": "<source or reason>", "memory": "PASS" } ],
  "timeout_case": { "…FixtureReceipt…": "", "peak_rss_bytes": null, "peak_rss_note": "<why>", "memory": "FAIL_not_measured" },
  "per_fixture": [ { "only": "<fixture>", "exit_code": 0, "signal": null, "timed_out": false, "done": true, "peak_rss_bytes": 123, "peak_rss_note": "…", "stdout_sha256": "<hex>" } ]
}
```

- [ ] **Step 1: write the failing tests.** Add to the import block:

```js
import {
  DOCLING_INPUTS,
  FIXTURE_RUNS,
  MUST_FAIL,
  TIMEOUT_PROBE,
  buildDoclingReceipt,
} from '../../qualification/docling/lib/receipt.mjs';
```

Append:

```js
const DOCLING_HEADER = {
  git_sha: 'a'.repeat(40),
  inputs: DOCLING_INPUTS,
  produced_at: '2026-10-05T18:00:00.000Z',
};
const DOCLING_CONVERTER = {
  crate: 'docling',
  version: '1.93.5',
  checksum: 'abc',
  docling_core_versions: ['1.93.6'],
  source: 'Cargo.lock',
};

/** A fixture process that behaved: exit 0, measured, receipt on disk. `over` replaces parts. */
function doclingRun(only, over = {}) {
  const fixture = {
    fixture: only,
    outcome: only === MUST_FAIL ? 'PASS_explicit_failure' : 'PASS',
    stage: only === MUST_FAIL ? 'converter_error' : 'converter_status',
    status: only === MUST_FAIL ? null : only === TIMEOUT_PROBE ? 'PartialSuccess' : 'Success',
    finding: null,
    peak_rss_bytes: null,
    ...over.fixture,
  };
  return {
    only,
    run: {
      exitCode: 0,
      signal: null,
      spawnError: null,
      timedOut: false,
      done: { okf_docling: 'done', only },
      peakRssBytes: 1_048_576,
      peakRssNote: 'stub source',
      stderr: '',
      stdoutSha256: 'f'.repeat(64),
      ...over.run,
    },
    report:
      'report' in over
        ? over.report
        : only === TIMEOUT_PROBE
          ? { receipts: [], timeout_case: fixture }
          : { receipts: [fixture], timeout_case: null },
  };
}

function doclingReceipt(overrides = {}) {
  const runs = FIXTURE_RUNS.map((only) => doclingRun(only, overrides[only]));
  return buildDoclingReceipt({
    header: DOCLING_HEADER,
    converter: DOCLING_CONVERTER,
    platform: 'win32',
    modelsDir: 'models',
    runs,
    finishedAt: '2026-10-05T18:10:00.000Z',
  });
}

test('a Docling receipt carries the shared header at its top level and passes when every rule holds', () => {
  const receipt = doclingReceipt();
  assert.deepEqual(receiptHeaderProblems(receipt), []);
  assert.deepEqual(Object.keys(receipt).slice(0, 3), ['git_sha', 'inputs', 'produced_at']);
  assert.equal(receipt.orchestrator, undefined);
  assert.equal(receipt.result, 'PASS');
  assert.equal(receipt.receipts.length, FIXTURE_RUNS.length - 1);
  assert.equal(receipt.timeout_case.outcome, 'PASS');
  assert.deepEqual(Object.keys(receipt.summary), FIXTURE_RUNS);
});

test('a fixture whose peak was not measured fails the memory criterion without throwing', () => {
  const receipt = doclingReceipt({
    'sample_sheet.xlsx': { run: { peakRssBytes: null, peakRssNote: 'Get-Process reported no PeakWorkingSet64' } },
  });
  const entry = receipt.receipts.find((item) => item.fixture === 'sample_sheet.xlsx');
  assert.equal(entry.peak_rss_bytes, null);
  assert.equal(entry.peak_rss_note, 'Get-Process reported no PeakWorkingSet64');
  assert.equal(entry.memory, 'FAIL_not_measured');
  assert.equal(entry.outcome, 'PASS');
  assert.equal(receipt.memory_summary['sample_sheet.xlsx'], 'FAIL_not_measured');
  assert.equal(receipt.result, 'FAIL');
});

test('a truncated PDF the converter accepts is a finding and a FAIL, recorded as observed', () => {
  const receipt = doclingReceipt({
    [MUST_FAIL]: {
      fixture: {
        outcome: 'FAIL_expected_failure',
        stage: 'converter_status',
        status: 'Success',
        finding: 'converter accepts truncated PDF',
      },
    },
  });
  assert.equal(receipt.finding, 'converter accepts truncated PDF');
  assert.equal(receipt.summary[MUST_FAIL], 'FAIL_expected_failure');
  assert.equal(receipt.result, 'FAIL');
});

test('a refusal counts only when another PDF converted in the same run', () => {
  const pdfs = FIXTURE_RUNS.filter((only) => only.endsWith('.pdf') && only !== MUST_FAIL);
  const overrides = Object.fromEntries(
    pdfs.map((only) => [only, { fixture: { outcome: 'FAIL_converter_error', stage: 'converter_error', status: null } }]),
  );
  const receipt = doclingReceipt(overrides);
  assert.equal(receipt.summary[MUST_FAIL], 'FAIL_refusal_unproven');
  assert.match(receipt.receipts.find((item) => item.fixture === MUST_FAIL).refusal_note, /no other PDF fixture converted/);
  assert.equal(receipt.result, 'FAIL');
});

test('a fixture process that wrote no receipt, or never ran, is a harness FAIL', () => {
  const crashed = doclingReceipt({
    'table_heavy.pdf': { report: null, run: { exitCode: 1, stderr: 'okf-qualify-docling: missing fixture' } },
  });
  const entry = crashed.receipts.find((item) => item.fixture === 'table_heavy.pdf');
  assert.equal(entry.outcome, 'FAIL_harness');
  assert.equal(entry.harness_exit_code, 1);
  assert.match(entry.harness_error, /missing fixture/);
  assert.equal(crashed.result, 'FAIL');

  const partial = buildDoclingReceipt({
    header: DOCLING_HEADER,
    converter: DOCLING_CONVERTER,
    platform: 'win32',
    modelsDir: 'models',
    runs: [doclingRun('sample_sheet.xlsx')],
    finishedAt: '2026-10-05T18:10:00.000Z',
  });
  assert.equal(partial.summary[TIMEOUT_PROBE], 'FAIL_not_run');
  assert.equal(partial.result, 'FAIL');
});

test('every fixture run is a recorded source whose bytes are unchanged, and must_fail states its stage rule', async () => {
  const dir = join(root, 'tests/fixtures/documents');
  const sources = JSON.parse(await readFile(join(dir, 'SOURCES.json'), 'utf8'));
  assert.deepEqual(
    FIXTURE_RUNS.filter((only) => only !== TIMEOUT_PROBE).sort(),
    Object.keys(sources.files).sort(),
  );
  for (const [name, entry] of Object.entries(sources.files)) {
    const bytes = await readFile(join(dir, name));
    assert.equal(bytes.length, entry.bytes, name);
    assert.equal(createHash('sha256').update(bytes).digest('hex'), entry.sha256, name);
  }
  assert.deepEqual(sources.files[MUST_FAIL].pass_when, ['converter_error', 'converter_status:Failure']);
});
```

- [ ] **Step 2: run and see the failure.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: exit 1, `Cannot find module '../../qualification/docling/lib/receipt.mjs'`; 0 pass.

- [ ] **Step 3: implement the composition.** Create `qualification/docling/lib/receipt.mjs`:

```js
/**
 * Compose the Docling qualification receipt from per-fixture process results.
 * Pure: no I/O and no imports, so the rules are testable without cargo or model assets.
 *
 * The Rust harness judges each conversion (stage, outcome, finding). This module adds
 * what only the orchestrator knows: whether the peak was measured, whether the process
 * behaved, and whether a must-fail refusal is attributable to the truncated input.
 */

export const MUST_FAIL = 'must_fail_truncated.pdf';
export const TIMEOUT_PROBE = 'timeout_probe';

/** Execution order. Kept equal to fixture_catalog() + must_fail in src/main.rs and to SOURCES.json by tests on both sides. */
export const FIXTURE_RUNS = [
  'sample_with_image.docx',
  'sample_sheet.xlsx',
  'born_digital_text.pdf',
  'scanned_image_only.pdf',
  'table_heavy.pdf',
  'sample_image.png',
  'corpus/word_sample.docx',
  'corpus/xlsx_01.xlsx',
  'corpus/powerpoint_sample.pptx',
  'corpus/redp5110_sampled.pdf',
  MUST_FAIL,
  TIMEOUT_PROBE,
];

export const DOCLING_INPUTS = [
  'qualification/docling',
  'qualification/lib',
  'tests/fixtures/documents',
  'Cargo.toml',
  'Cargo.lock',
];

const passed = (outcome) => String(outcome).startsWith('PASS');

/** One fixture's receipt entry: the harness receipt plus memory, or a harness FAIL. */
export function fixtureEntry({ only, run, report }) {
  const measured = typeof run.peakRssBytes === 'number' && run.peakRssBytes > 0;
  const memory = {
    peak_rss_bytes: measured ? run.peakRssBytes : null,
    peak_rss_note: run.peakRssNote,
    memory: measured ? 'PASS' : 'FAIL_not_measured',
  };
  const base = only === TIMEOUT_PROBE ? report?.timeout_case : report?.receipts?.[0];
  if (run.exitCode !== 0 || base === null || typeof base !== 'object') {
    return {
      fixture: only,
      outcome: 'FAIL_harness',
      stage: null,
      status: null,
      finding: null,
      harness_exit_code: run.exitCode,
      harness_signal: run.signal,
      harness_timed_out: run.timedOut,
      harness_error: String(run.spawnError ?? run.stderr ?? '').slice(-2000),
      ...memory,
    };
  }
  return { ...base, ...memory };
}

/** The finished receipt. `result` is PASS only when every outcome and every memory entry passes. */
export function buildDoclingReceipt({ header, converter, platform, modelsDir, runs, finishedAt }) {
  const entries = runs.map((item) => ({ only: item.only, entry: fixtureEntry(item) }));

  // A PDF refusal proves nothing when the PDF pipeline itself is not working:
  // docling loads its models before it reads the file, so every PDF then errors.
  const pdfConverted = entries.some(
    ({ only, entry }) => only.endsWith('.pdf') && only !== MUST_FAIL && passed(entry.outcome),
  );
  for (const { only, entry } of entries) {
    if (only === MUST_FAIL && passed(entry.outcome) && !pdfConverted) {
      entry.outcome = 'FAIL_refusal_unproven';
      entry.refusal_note =
        'no other PDF fixture converted successfully in this run, so the refusal cannot be attributed to the truncated input';
    }
  }

  const summary = {};
  const memory_summary = {};
  for (const only of FIXTURE_RUNS) {
    const found = entries.find((item) => item.only === only);
    summary[only] = found ? found.entry.outcome : 'FAIL_not_run';
    memory_summary[only] = found ? found.entry.memory : 'FAIL_not_measured';
  }
  const mustFail = entries.find((item) => item.only === MUST_FAIL)?.entry;
  const ok = Object.values(summary).every(passed) && Object.values(memory_summary).every((value) => value === 'PASS');

  return {
    ...header,
    component: 'docling-library-qualification',
    finished_at: finishedAt,
    result: ok ? 'PASS' : 'FAIL',
    finding: mustFail?.finding ?? null,
    converter,
    build: 'cargo build --locked --release -p okf-qualify-docling',
    platform,
    peak_memory_source:
      platform === 'win32'
        ? 'PeakWorkingSet64 of each converter process, read once before it exits'
        : 'VmHWM of each converter process, read once before it exits',
    fixtures_dir: 'tests/fixtures/documents',
    models_dir: modelsDir,
    assets_manifest: '.artifacts/qualification/docling/assets.json',
    summary,
    memory_summary,
    receipts: entries.filter((item) => item.only !== TIMEOUT_PROBE).map((item) => item.entry),
    timeout_case: entries.find((item) => item.only === TIMEOUT_PROBE)?.entry ?? null,
    per_fixture: runs.map((item) => ({
      only: item.only,
      exit_code: item.run.exitCode,
      signal: item.run.signal,
      timed_out: item.run.timedOut,
      done: item.run.done !== null,
      peak_rss_bytes: item.run.peakRssBytes,
      peak_rss_note: item.run.peakRssNote,
      stdout_sha256: item.run.stdoutSha256,
    })),
  };
}
```

- [ ] **Step 4: replace the entry point.** Replace `qualification/docling/run.mjs` with:

```js
/**
 * Orchestrate Docling direct-library qualification. Thin entry point:
 *   lib/runner.mjs  runs one process per fixture and samples its peak memory once;
 *   lib/receipt.mjs composes and judges the receipt;
 *   scripts/lib/provenance.mjs supplies the header behind requireCleanTree.
 *
 * Usage (from PowerShell, cargo on PATH): bun qualification/docling/run.mjs [--record]
 * Needs .artifacts/qualification/docling/assets.json naming the verified model assets.
 * Writes .artifacts/qualification/docling/receipt.json; --record also copies it to
 * qualification/receipts/docling.json. Exits non-zero unless the receipt result is PASS.
 */

import { createHash } from 'node:crypto';
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { receiptHeader, recordReceipt } from '../../scripts/lib/provenance.mjs';
import { buildRelease, lockedPackage, lockedPackages } from '../lib/cargo.mjs';
import { DOCLING_INPUTS, FIXTURE_RUNS, buildDoclingReceipt } from './lib/receipt.mjs';
import { runFixtureProcess } from './lib/runner.mjs';

const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const outDir = join(root, '.artifacts/qualification/docling');
const assetsPath = join(outDir, 'assets.json');
const partialDir = join(outDir, 'partial');
const fixturesDir = join(root, 'tests/fixtures/documents');
const record = process.argv.includes('--record');

const header = await receiptHeader(root, DOCLING_INPUTS);

let assets;
try {
  assets = JSON.parse((await readFile(assetsPath, 'utf8')).replace(/^\uFEFF/, ''));
} catch (error) {
  throw new Error(
    `Docling model assets manifest not readable at ${assetsPath}: ${error.message}. Qualification needs the verified model assets; a missing asset is never qualified.`,
  );
}

const lockText = await readFile(join(root, 'Cargo.lock'), 'utf8');
const docling = lockedPackage(lockText, 'docling');
const converter = {
  crate: 'docling',
  version: docling.version,
  checksum: docling.checksum,
  docling_core_versions: lockedPackages(lockText, 'docling-core').map((entry) => entry.version),
  source: 'Cargo.lock',
};

const env = {
  ...process.env,
  DOCLING_RS_MODELS_DIR: assets.DOCLING_RS_MODELS_DIR,
  OKF_DOCLING_FIXTURES: fixturesDir,
  ...assets.recommended_env,
  OKF_DOCLING_CRATE_VERSION: docling.version,
  OKF_DOCLING_HOLD: '1',
};

await mkdir(outDir, { recursive: true });
await rm(partialDir, { recursive: true, force: true });
await mkdir(partialDir, { recursive: true });

const binPath = await buildRelease(root, 'okf-qualify-docling');

const runs = [];
for (const only of FIXTURE_RUNS) {
  process.stdout.write(`\n=== Docling fixture: ${only} ===\n`);
  const fixtureOut = join(partialDir, only.replaceAll(/[\\/]/g, '__'));
  await mkdir(fixtureOut, { recursive: true });
  const run = await runFixtureProcess({
    command: binPath,
    cwd: root,
    env: { ...env, OKF_DOCLING_ONLY: only, OKF_DOCLING_OUT: fixtureOut },
    onStdout: (text) => process.stdout.write(text),
    onStderr: (text) => process.stderr.write(text),
  });
  let report = null;
  try {
    report = JSON.parse(await readFile(join(fixtureOut, 'receipt.json'), 'utf8'));
  } catch {
    report = null; // judged as FAIL_harness by lib/receipt.mjs
  }
  runs.push({
    only,
    run: { ...run, stdoutSha256: createHash('sha256').update(run.stdout).digest('hex') },
    report,
  });
}

const receipt = buildDoclingReceipt({
  header,
  converter,
  platform: process.platform,
  modelsDir: assets.DOCLING_RS_MODELS_DIR,
  runs,
  finishedAt: new Date().toISOString(),
});

const receiptPath = join(outDir, 'receipt.json');
await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
process.stdout.write(`Docling qualification receipt: ${receiptPath}\n`);
if (record) {
  process.stdout.write(`Docling receipt recorded: ${await recordReceipt(root, 'docling', receipt)}\n`);
}

if (receipt.result !== 'PASS') {
  const finding = receipt.finding
    ? ` Finding for the owner: ${receipt.finding}. Fixture must_fail_truncated.pdf was not altered.`
    : '';
  throw new Error(
    `Docling qualification FAIL.${finding} ${JSON.stringify({ summary: receipt.summary, memory: receipt.memory_summary })}`,
  );
}
```

- [ ] **Step 5: record the stage rule beside the fixture.** In `tests/fixtures/documents/SOURCES.json`, replace line 64 (the `"notes"` line of `must_fail_truncated.pdf`) with these two lines; nothing else in the file changes and no fixture file is touched:

```json
      "notes": "Valid PDF-1.4 header truncated mid-stream. PASS only when the converter itself refuses it: DocumentConverter::convert returns Err (stage converter_error) or Ok with ConversionStatus::Failure (stage converter_status). A SourceDocument::from_file error (stage source) is FAIL: the fixture never reached the converter. Success or PartialSuccess is FAIL with the finding \"converter accepts truncated PDF\". The fixture is never altered to force a pass.",
      "pass_when": ["converter_error", "converter_status:Failure"],
```

- [ ] **Step 6: run and see it pass.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: `25 pass`, `0 fail`.
  Run: `bun build --no-bundle qualification/docling/run.mjs --outfile .artifacts/syntax/docling-run.js`
  Expected: exit 0 (syntax only; `.artifacts/` is ignored).
  Run (PowerShell): `cargo test --locked -p okf-qualify-docling`
  Expected: `4 passed` (the catalog test still equals `SOURCES.json` keys).

- [ ] **Step 7: commit.**

```sh
git add qualification/docling/lib/receipt.mjs qualification/docling/run.mjs tests/fixtures/documents/SOURCES.json tests/foundation/harness.test.mjs
git commit -F - <<'MSG'
fix(qualify): compose the Docling receipt with a top-level header and memory verdicts.

Why: run.mjs:271-279 wrote git_sha and inputs under receipt.orchestrator, so the
receipt could never pass check-receipts; a missing memory sample threw (:179-183)
instead of being recorded; and a must-fail "pass" could come from a PDF pipeline
that was not working at all, because docling loads its models before it reads
the file.
What changed: run.mjs is a thin entry point over lib/runner.mjs and the new pure
lib/receipt.mjs. The receipt starts with { git_sha, inputs, produced_at } from
receiptHeader. A fixture without a measured peak records peak_rss_bytes: null,
its peak_rss_note and memory: FAIL_not_measured, and the run fails. A must-fail
refusal counts only when another PDF converted in the same run. A process that
wrote no receipt is FAIL_harness. --record copies the receipt to
qualification/receipts/docling.json. SOURCES.json states the stage rule for
must_fail_truncated.pdf; no fixture byte changed.
Verified: bun test ./tests/foundation/harness.test.mjs -> 25 pass, 0 fail;
bun build --no-bundle qualification/docling/run.mjs -> exit 0; cargo test
--locked -p okf-qualify-docling -> 4 passed.
Next: Task B.6 fixes the MCP Apps Rust harness.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
MSG
```

---

### Task B.6: MCP Apps Rust harness — host protection needs `1`, and the present view can resolve its bindings

**Files:**
- Modify: `qualification/mcp-apps/src/main.rs` — `:1-5` (module doc), `:61-75` (types), `:77` (constants before the first `impl`), `:160-175` (`load`), `:208-218` (`tool_definitions`), `:220-230` (new method after `call_render_tool`), `:271-277` (`check_report`), `:328-334` (`call_tool`), `:360-369` (new functions after `tool_ui_meta`), `:447` (tunnel check), end of file (tests)
- Test: unit tests in the same file

**Interfaces:**
- Consumes: rmcp 3.5.0 items quoted in the facts section; fixtures `tests/fixtures/views/present-response.json` and `source-read-item.json` (read only).
- Produces:
  - `OKF_MCP_APPS_NGROK`: only the exact value `1` disables allowed-hosts protection.
  - A fifth tool `show`, app-only (`_meta.ui.visibility: ["app"]`, no `resourceUri`), read-only. Input: `{ item_id: string, at: { revision: string, … }, … }`. Result: `structuredContent` = the `source-read-item.json` object with `source` replaced by the `source` of the present-fixture binding whose `item_id` matches; text fallback `"Qualification show fixture (text fallback)."`. Unknown `item_id`, a revision other than the bound one, or missing arguments → `invalid_params`.
  - `--check` JSON gains `"app_tools": ["show"]`; `tool_count` and `tools` still describe the four render tools.

- [ ] **Step 1: write the failing tests.** Append to the end of `qualification/mcp-apps/src/main.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{ShowFixtures, fixtures_dir, load_fixture, show_fixture, tunnel_hosts_allowed};
    use serde_json::{Value, json};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn fixtures() -> Result<ShowFixtures, String> {
        let dir = fixtures_dir();
        Ok(ShowFixtures {
            present: load_fixture(&dir.join("present-response.json"))?,
            read_item: load_fixture(&dir.join("source-read-item.json"))?,
        })
    }

    #[test]
    fn show_answers_each_present_binding_with_its_own_source() -> TestResult {
        let fixtures = fixtures()?;
        let bindings = fixtures
            .present
            .get("resolved_bindings")
            .and_then(Value::as_array)
            .ok_or("present fixture has no resolved_bindings")?;
        assert_eq!(bindings.len(), 2);
        for binding in bindings {
            let source = binding.get("source").ok_or("binding has no source")?;
            let arguments = json!({
                "item_id": source.get("item_id"),
                "at": { "kind": "revision", "revision": source.get("revision") },
            });
            let shown = show_fixture(&fixtures, arguments.as_object())?;
            assert_eq!(shown.get("source"), Some(source));
            assert_eq!(shown.get("markdown"), fixtures.read_item.get("markdown"));
        }
        Ok(())
    }

    #[test]
    fn show_refuses_items_and_revisions_the_present_fixture_does_not_bind() -> TestResult {
        let fixtures = fixtures()?;
        let unknown = json!({
            "item_id": "99999999-9999-4999-8999-999999999999",
            "at": { "revision": "0123456789abcdef0123456789abcdef01234567" },
        });
        assert!(show_fixture(&fixtures, unknown.as_object()).is_err());
        let moved = json!({
            "item_id": "11111111-2222-4333-8444-555555555555",
            "at": { "revision": "89abcdef0123456789abcdef0123456789abcdef" },
        });
        assert!(show_fixture(&fixtures, moved.as_object()).is_err());
        assert!(show_fixture(&fixtures, None).is_err());
        Ok(())
    }

    #[test]
    fn only_the_value_one_lifts_host_protection() {
        assert!(tunnel_hosts_allowed(Some("1")));
        for value in [Some("0"), Some("true"), Some(""), None] {
            assert!(!tunnel_hosts_allowed(value));
        }
    }
}
```

The third test returns `()`: it has nothing fallible, and a function that can only return `Ok` trips `clippy::unnecessary_wraps` (pedantic, denied). It uses no forbidden construct. See Deviations.

- [ ] **Step 2: run and see the failure (PowerShell).**
  Run: `cargo test --locked -p okf-qualify-mcp-apps`
  Expected: compile errors, ``error[E0432]: unresolved imports `super::ShowFixtures`, `super::show_fixture`, `super::tunnel_hosts_allowed` ``.

- [ ] **Step 3: implement.** Apply these edits to `qualification/mcp-apps/src/main.rs` from the bottom of the file upward so the line numbers stay valid.

`:447` — replace the line `    if env::var_os("OKF_MCP_APPS_NGROK").is_some() {` with:

```rust
    if tunnel_hosts_allowed(env::var(TUNNEL_ENV).ok().as_deref()) {
```

After `:369` (end of `tool_ui_meta`) insert:

```rust

fn app_only_meta() -> MetaObject {
    let mut meta = MetaObject::new();
    meta.insert("ui".to_owned(), json!({ "visibility": ["app"] }));
    meta
}

fn show_schema() -> Arc<JsonObject> {
    let mut schema = Map::new();
    schema.insert("type".to_owned(), json!("object"));
    schema.insert(
        "properties".to_owned(),
        json!({ "item_id": { "type": "string" }, "at": { "type": "object" } }),
    );
    schema.insert("required".to_owned(), json!(["item_id", "at"]));
    Arc::new(schema)
}

/// The read result the present fixture's binding for the requested item resolves to.
fn show_fixture(fixtures: &ShowFixtures, arguments: Option<&JsonObject>) -> Result<Value, String> {
    let item_id = arguments
        .and_then(|input| input.get("item_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| "show requires a string item_id".to_owned())?;
    let revision = arguments
        .and_then(|input| input.get("at"))
        .and_then(|at| at.get("revision"))
        .and_then(Value::as_str)
        .ok_or_else(|| "show requires at.revision; qualification reads are pinned".to_owned())?;
    let source = fixtures
        .present
        .get("resolved_bindings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|binding| binding.get("source"))
        .find(|source| source.get("item_id").and_then(Value::as_str) == Some(item_id))
        .ok_or_else(|| format!("show: no present-fixture binding cites item {item_id}"))?;
    if source.get("revision").and_then(Value::as_str) != Some(revision) {
        return Err(format!(
            "show: {revision} is not the revision bound for item {item_id}"
        ));
    }
    let mut result = fixtures.read_item.clone();
    let fields = result
        .as_object_mut()
        .ok_or_else(|| "source-read-item.json must be a JSON object".to_owned())?;
    fields.insert("source".to_owned(), source.clone());
    Ok(result)
}

/// Only the exact value `1` lifts Host protection; `0`, `true` or an empty value keep it.
fn tunnel_hosts_allowed(value: Option<&str>) -> bool {
    value == Some("1")
}
```

`:328-334` — replace the body of `call_tool` (the single `std::future::ready(...)` line at `:333`) with:

```rust
        let result = if request.name == SHOW_TOOL {
            self.call_show(request.arguments.as_ref())
        } else {
            self.call_render_tool(&request.name)
        };
        std::future::ready(result.map(Into::into))
```

`:271-277` — in the final `json!` of `check_report`, add one line after `"tools": tools,`:

```rust
            "app_tools": [SHOW_TOOL],
```

After `:230` (end of `call_render_tool`) insert the method:

```rust

    fn call_show(&self, arguments: Option<&JsonObject>) -> Result<CallToolResult, McpError> {
        let source = show_fixture(&self.show, arguments)
            .map_err(|message| McpError::invalid_params(message, None))?;
        let mut result = CallToolResult::success(vec![ContentBlock::text(
            "Qualification show fixture (text fallback).".to_owned(),
        )]);
        result.structured_content = Some(source);
        Ok(result)
    }
```

`:208-218` — replace `tool_definitions` with:

```rust
    fn tool_definitions(&self) -> Vec<Tool> {
        let schema = empty_object_schema();
        let mut tools: Vec<Tool> = self
            .tools
            .iter()
            .map(|tool| {
                Tool::new(tool.name, tool.description, schema.clone())
                    .with_annotations(ToolAnnotations::new().read_only(true))
                    .with_meta(tool_ui_meta(&tool.resource_uri))
            })
            .collect();
        tools.push(
            Tool::new(SHOW_TOOL, SHOW_DESCRIPTION, show_schema())
                .with_annotations(ToolAnnotations::new().read_only(true))
                .with_meta(app_only_meta()),
        );
        tools
    }
```

`:171-175` — replace the `Ok(Self { … })` at the end of `load` with:

```rust
        let show = ShowFixtures {
            present: load_fixture(&fixtures.join("present-response.json"))?,
            read_item: load_fixture(&fixtures.join("source-read-item.json"))?,
        };

        Ok(Self {
            by_uri,
            resources,
            show,
            tools,
        })
```

`:70-75` — replace the struct and add the new type and the constants directly below it (types, then constants, then the `impl` at old `:77`):

```rust
#[derive(Clone)]
struct QualifyAppsServer {
    by_uri: BTreeMap<String, BundledApp>,
    resources: Vec<BundledApp>,
    show: ShowFixtures,
    tools: Vec<RenderTool>,
}

/// Fixtures the app-only `show` tool answers from.
#[derive(Clone)]
struct ShowFixtures {
    present: Value,
    read_item: Value,
}

const SHOW_DESCRIPTION: &str = "App-only fixture read: returns the source fixture text under the source reference of the present-fixture binding that cites the requested item.";
const SHOW_TOOL: &str = "show";
const TUNNEL_ENV: &str = "OKF_MCP_APPS_NGROK";
```

`:3-5` — replace the three doc lines with:

```rust
//! Serves built `ui/dist-apps` HTML bundles as MCP UI resources, four read-only
//! render tools and one app-only `show` fixture tool (the present view resolves its
//! bindings through it) over stdio or Streamable HTTP via `rmcp`. This is not the
//! product `ServerHandler` and must not be linked from product crates.
```

- [ ] **Step 4: run the Rust gate (PowerShell).**
  Run: `cargo fmt -p okf-qualify-mcp-apps` then `cargo fmt --check -p okf-qualify-docling -p okf-qualify-mcp-apps -p okf-qualify-iii`
  Expected: exit 0 with no output. If the check names `qualification/iii/src/main.rs`, run `cargo fmt -p okf-qualify-iii` and include that file in this commit; it is inside your allowed files.
  Run: `cargo clippy --locked -p okf-qualify-docling -p okf-qualify-mcp-apps --all-targets -- -D warnings`
  Expected: `Finished` with no warnings or errors (fix code, never suppress).
  Run: `cargo test --locked -p okf-qualify-docling -p okf-qualify-mcp-apps`
  Expected: `4 passed` for `okf-qualify-docling` and `3 passed` for `okf-qualify-mcp-apps`.

- [ ] **Step 5: commit.**

```sh
git add qualification/mcp-apps/src/main.rs
git commit -F - <<'MSG'
fix(qualify): require OKF_MCP_APPS_NGROK=1 and let the present view resolve its bindings.

Why: main.rs:447 used env::var_os(..).is_some(), so OKF_MCP_APPS_NGROK=0 still
disabled allowed-hosts protection. PresentView.tsx:117-129 calls host tool
`show` for every resolved binding, and the harness answered only render_*
(main.rs:220-226), so the present view could never render in a host. SPEC 9:
"MCP Apps supplies source, Changes, Timeline and constrained presentation
views."
What changed: only the exact value 1 lifts Host protection. The harness lists a
fifth, app-only, read-only tool `show` (_meta.ui.visibility ["app"]) that
returns the source fixture under the bound source reference and refuses items
or revisions the present fixture does not bind. --check reports app_tools.
Verified: cargo fmt --check -p okf-qualify-docling -p okf-qualify-mcp-apps -p
okf-qualify-iii -> clean; cargo clippy --locked -p okf-qualify-docling -p
okf-qualify-mcp-apps --all-targets -- -D warnings -> clean; cargo test --locked
-p okf-qualify-docling -p okf-qualify-mcp-apps -> 4 passed, 3 passed.
Next: Task B.7 adds process-group handling for the orchestrator.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
MSG
```

---

### Task B.7: MCP Apps process handling — whole-tree kill and immediate failure reporting

**Files:**
- Create: `qualification/mcp-apps/lib/process.mjs`
- Test: `tests/foundation/harness.test.mjs` (imports, appended tests)

**Interfaces:**
- Consumes: Node `child_process.spawn` with `detached` (POSIX: the child leads a new process group), `process.kill(-pid, signal)`, Windows `taskkill /pid <pid> /T /F`.
- Produces (exported from `qualification/mcp-apps/lib/process.mjs`):
  - `spawnGroup(command: string, args: string[], options?: { cwd?: string, env?: object }): { child: ChildProcess, stdout: () => string, stderr: () => string, exited: Promise<{ code: number | null, signal: string | null, error: string | null }> }` — `exited` never rejects.
  - `killProcessTree(proc, options?: { graceMs?: number }): Promise<{ code, signal, error }>` — resolves after the tree was signalled and the leader's exit was observed (or `graceMs` passed).
  - `waitForListening(proc, options: { pattern: RegExp, timeoutMs?: number, label?: string }): Promise<RegExpExecArray>` — resolves on the first match in the process's stderr or stdout; rejects at once with exit code and stderr tail when the process exits first; rejects after `timeoutMs` (default 60 000).

- [ ] **Step 1: write the failing tests.** Add to the import block:

```js
import { killProcessTree, spawnGroup, waitForListening } from '../../qualification/mcp-apps/lib/process.mjs';
```

Append:

```js
const pidAlive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};
const pause = (ms) => new Promise((done) => setTimeout(done, ms));

test('waitForListening resolves on the listening line and the process can then be stopped', async () => {
  const proc = spawnGroup(process.execPath, [
    '-e',
    "process.stderr.write('okf-qualify-mcp-apps listening on http://127.0.0.1:1/mcp\\n');setInterval(()=>{},1000)",
  ]);
  try {
    const match = await within(
      waitForListening(proc, { pattern: /okf-qualify-mcp-apps listening on (\S+)/, label: 'harness' }),
      4000,
      'listening',
    );
    assert.equal(match[1], 'http://127.0.0.1:1/mcp');
  } finally {
    await killProcessTree(proc);
  }
  assert.equal(pidAlive(proc.child.pid), false);
});

test('a harness that fails to bind is reported at once with its exit code and stderr', async () => {
  const proc = spawnGroup(process.execPath, [
    '-e',
    "process.stderr.write('okf-qualify-mcp-apps: bind 127.0.0.1:1: access denied\\n');process.exit(1)",
  ]);
  const started = Date.now();
  await assert.rejects(
    within(waitForListening(proc, { pattern: /listening on (\S+)/, timeoutMs: 60_000, label: 'harness' }), 4000, 'bind failure'),
    (error) => {
      assert.match(error.message, /harness exited before listening \(exit 1\)/);
      assert.match(error.message, /bind 127\.0\.0\.1:1: access denied/);
      return true;
    },
  );
  assert.ok(Date.now() - started < 4000);
});

test('killProcessTree stops grandchildren, not only the process it was given', async () => {
  const parent =
    "const {spawn}=require('node:child_process');const g=spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:'ignore'});process.stdout.write(String(g.pid)+'\\n');setInterval(()=>{},1000)";
  const proc = spawnGroup(process.execPath, ['-e', parent]);
  let grandchild = null;
  try {
    for (let attempt = 0; attempt < 80 && grandchild === null; attempt += 1) {
      const match = /^(\d+)\r?\n/.exec(proc.stdout());
      if (match) grandchild = Number(match[1]);
      else await pause(50);
    }
    assert.ok(grandchild !== null, 'parent did not report its child pid');
    assert.equal(pidAlive(grandchild), true);
    await killProcessTree(proc);
    let gone = false;
    for (let attempt = 0; attempt < 60 && !gone; attempt += 1) {
      gone = !pidAlive(grandchild);
      if (!gone) await pause(50);
    }
    assert.equal(gone, true, `grandchild ${grandchild} survived killProcessTree`);
  } finally {
    if (grandchild !== null && pidAlive(grandchild)) process.kill(grandchild);
    await killProcessTree(proc);
  }
});
```

- [ ] **Step 2: run and see the failure.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: exit 1, `Cannot find module '../../qualification/mcp-apps/lib/process.mjs'`; 0 pass.

- [ ] **Step 3: implement.** Create `qualification/mcp-apps/lib/process.mjs`:

```js
/**
 * Long-running helper processes for the MCP Apps orchestrator.
 *
 * spawnGroup starts a process as the leader of its own process group on POSIX
 * (detached: true) so killProcessTree can signal the whole group; on Windows
 * taskkill /T walks the tree. waitForListening reports a process that dies before it
 * is ready at once, with its exit code and stderr, instead of a later TCP timeout.
 */

import { spawn } from 'node:child_process';

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
const tail = (text, max = 2000) => (text.length > max ? text.slice(-max) : text);

/** Start a process whose output is kept and whose exit is always observable. */
export function spawnGroup(command, args, options = {}) {
  const child = spawn(command, args, {
    cwd: options.cwd,
    env: options.env ?? process.env,
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: true,
    detached: process.platform !== 'win32',
  });
  let stdout = '';
  let stderr = '';
  child.stdout?.on('data', (chunk) => {
    stdout += chunk;
  });
  child.stderr?.on('data', (chunk) => {
    stderr += chunk;
  });
  const exited = new Promise((resolveExit) => {
    child.on('error', (error) => resolveExit({ code: null, signal: null, error: error.message }));
    child.on('close', (code, signal) => resolveExit({ code, signal: signal ?? null, error: null }));
  });
  return { child, stdout: () => stdout, stderr: () => stderr, exited };
}

/** Signal a POSIX process group; true while the group still has members. */
function signalGroup(pid, signal) {
  try {
    process.kill(-pid, signal);
    return true;
  } catch (error) {
    return error.code === 'EPERM';
  }
}

/** Stop a process started by spawnGroup together with everything it started. */
export async function killProcessTree(proc, { graceMs = 5_000 } = {}) {
  const pid = proc?.child?.pid;
  if (!pid) return proc ? proc.exited : { code: null, signal: null, error: 'never started' };
  if (process.platform === 'win32') {
    await new Promise((done) => {
      const killer = spawn('taskkill', ['/pid', String(pid), '/T', '/F'], {
        stdio: 'ignore',
        windowsHide: true,
      });
      killer.on('error', done);
      killer.on('close', done);
    });
  } else {
    signalGroup(pid, 'SIGTERM');
    const deadline = Date.now() + graceMs;
    while (Date.now() < deadline && signalGroup(pid, 0)) await sleep(50);
    signalGroup(pid, 'SIGKILL');
  }
  return Promise.race([
    proc.exited,
    sleep(graceMs).then(() => ({ code: null, signal: null, error: `still running ${graceMs} ms after kill` })),
  ]);
}

/** Wait for a readiness line; fail at once when the process exits first. */
export function waitForListening(proc, { pattern, timeoutMs = 60_000, label = 'process' }) {
  return new Promise((resolveReady, reject) => {
    let settled = false;
    let timer = null;
    const check = () => {
      const match = pattern.exec(proc.stderr()) ?? pattern.exec(proc.stdout());
      if (match) finish(resolveReady, match);
    };
    const finish = (settleWith, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      proc.child.stderr?.off('data', check);
      proc.child.stdout?.off('data', check);
      settleWith(value);
    };
    timer = setTimeout(
      () =>
        finish(
          reject,
          new Error(`${label} did not report listening within ${timeoutMs} ms\nstderr:\n${tail(proc.stderr())}`),
        ),
      timeoutMs,
    );
    // spawnGroup's own listeners were registered first, so the buffers are current when check runs.
    proc.child.stderr?.on('data', check);
    proc.child.stdout?.on('data', check);
    proc.exited.then(({ code, signal, error }) =>
      finish(
        reject,
        new Error(
          `${label} exited before listening (exit ${code ?? 'none'}${signal ? `, signal ${signal}` : ''}${error ? `, ${error}` : ''})\nstderr:\n${tail(proc.stderr())}`,
        ),
      ),
    );
    check();
  });
}
```

- [ ] **Step 4: run and see it pass.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: `28 pass`, `0 fail`.
  Removal check (do it, then undo it): in `killProcessTree` replace `'/T', ` with nothing on Windows (or replace `signalGroup(pid, 'SIGTERM')` and `signalGroup(pid, 'SIGKILL')` with `proc.child.kill()` on POSIX) and re-run — "killProcessTree stops grandchildren" must fail with `grandchild <pid> survived killProcessTree`. Restore.

- [ ] **Step 5: commit.**

```sh
git add qualification/mcp-apps/lib/process.mjs tests/foundation/harness.test.mjs
git commit -F - <<'MSG'
feat(qualify): stop helper process trees and report a dead harness at once.

Why: run.mjs:69-88 fired taskkill/SIGTERM without waiting or escalating and fell
back to child.kill, which reaches only `cargo run`; a harness that failed to
bind surfaced a minute later as "timed out waiting for tcp" (:106-124) with its
stderr and exit code discarded.
What changed: qualification/mcp-apps/lib/process.mjs exports spawnGroup (own
process group on POSIX, observable exit, kept output), killProcessTree (awaited;
group SIGTERM then SIGKILL, or taskkill /T /F) and waitForListening (rejects
immediately with exit code and stderr when the process exits first).
Verified: bun test ./tests/foundation/harness.test.mjs -> 28 pass, 0 fail.
Next: Task B.8 adds the per-view judgement and scoped axe rules.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
MSG
```

---

### Task B.8: MCP Apps judgement — four views, scoped axe exclusions, closed tunnel

**Files:**
- Create: `qualification/mcp-apps/lib/views.mjs`
- Test: `tests/foundation/harness.test.mjs` (imports, appended tests)

**Interfaces:**
- Consumes: text rendered by `ui/src/features/{documents/SourceExcerpt,history/Changes,history/Timeline,views/Layout}.tsx` from `tests/fixtures/views/*.json` (strings quoted in the code below come from those files); axe `AxeResults` (`violations`, `passes`, `incomplete`, each rule with `id`, `impact`, `nodes[].target`).
- Produces (exported from `qualification/mcp-apps/lib/views.mjs`, pure, no imports):
  - `APP_RESOURCE_URI: 'ui://okf-jawn/app.html'`, `APP_ONLY_TOOLS: ['show']`, `UPSTREAM_HOST_RULES: ['color-contrast', 'frame-title']`
  - `VIEWS: { tool, view, mustContain: string[], mustNotContain: string[], alerts: string[] }[]` (four entries)
  - `basicHostUrl(tool: string, options?: { server?: string, port?: number }): string`
  - `judgeView(view, observed: { text: string, alerts: string[] } | null): { ok: boolean, missing: string[], foreign: string[], blocking: string[], alerts: string[], expected_alerts: string[] }`
  - `partitionAxe(results, appDepth: number): { app_frame_analysed: boolean, app_frame: object[], host_blocking: object[], host_tolerated: object[] }`
  - `basicHostVerdict(results: { tool, status, error? }[]): { status: 'passed' | 'failed', failed: { tool, error }[] }`
  - `runProblems({ protocol, basicHost, protocolOnly }): string[]`
  - `ngrokRecord({ enabled, opened_at, closed_at, public_url, local_port, note }): { status: 'not_run' | 'not_opened' | 'closed', opened_at, closed_at, public_url, local_port, note }` — throws when a tunnel was opened and not closed.

- [ ] **Step 1: write the failing tests.** Add to the import block:

```js
import {
  UPSTREAM_HOST_RULES,
  VIEWS,
  basicHostUrl,
  basicHostVerdict,
  judgeView,
  ngrokRecord,
  partitionAxe,
  runProblems,
} from '../../qualification/mcp-apps/lib/views.mjs';
```

Append:

```js
const REVISION = '0123456789abcdef0123456789abcdef01234567';
/** What each view's App frame shows for the committed fixtures (innerText, abbreviated). */
const VIEW_TEXT = {
  render_source: `fixtures/qualification-source.md ${REVISION}\nQualification source\nContract-valid ReadItemResponse fixture for the MCP Apps harness.`,
  render_changes: `Changes\n${REVISION} → 89abcdef0123456789abcdef0123456789abcdef\nfixtures/qualification-source.md\n@@ -1,3 +1,4 @@\n # Qualification source\n+\n+Harness fixture line.`,
  render_timeline: `Timeline\nQualification timeline fixture\nokf-qualify-mcp-apps · 2026-10-05T12:00:00Z\n89abcdef0123456789abcdef0123456789abcdef`,
  render_present: `Six-component catalog\nfixtures/qualification-source.md ${REVISION}\nQualification source\nContract-valid ReadItemResponse fixture for the MCP Apps harness.\nDataset unavailable: metrics\nMetrics chart\nResolved chart data or specification unavailable.\nfixtures/qualification-source.md @ ${REVISION}\nfixtures/qualification-metrics.json @ ${REVISION}`,
};

test('each of the four views is recognised by its own text and by no other view', () => {
  assert.deepEqual(VIEWS.map((view) => view.tool), ['render_source', 'render_changes', 'render_timeline', 'render_present']);
  for (const view of VIEWS) {
    assert.equal(
      basicHostUrl(view.tool),
      `http://127.0.0.1:8080/?server=okf-qualify-mcp-apps&tool=${view.tool}&call=true&theme=hide`,
    );
    for (const other of VIEWS) {
      const verdict = judgeView(view, { text: VIEW_TEXT[other.tool], alerts: other.alerts });
      assert.equal(verdict.ok, view === other, `${view.tool} judged against ${other.tool}: ${JSON.stringify(verdict)}`);
    }
  }
});

test('a view that is waiting, disconnected, unparsed or showing an unexpected alert is not rendered', () => {
  const source = VIEWS[0];
  assert.equal(judgeView(source, null).ok, false);
  assert.equal(judgeView(source, { text: 'Waiting for a tool result from the connected host.', alerts: [] }).ok, false);
  assert.equal(
    judgeView(source, { text: `${VIEW_TEXT.render_source}\nHost connection failed: x`, alerts: ['Host connection failed: x'] }).ok,
    false,
  );
  const present = VIEWS[3];
  const broken = judgeView(present, { text: VIEW_TEXT.render_present, alerts: ['MCP error -32602: unknown tool: show'] });
  assert.equal(broken.ok, false);
  assert.deepEqual(broken.alerts, ['MCP error -32602: unknown tool: show']);
  assert.equal(judgeView(present, { text: VIEW_TEXT.render_present, alerts: [] }).ok, false);
});

test('axe exclusions cover host chrome only; the App frame is judged by every rule', () => {
  const rule = (id, impact, ...targets) => ({ id, impact, nodes: targets.map((target) => ({ target })) });
  const results = {
    passes: [rule('document-title', null, ['iframe', 'iframe', 'html'])],
    incomplete: [],
    violations: [
      rule('color-contrast', 'serious', ['.collapsibleSize'], ['iframe', 'iframe', 'code']),
      rule('frame-title', 'serious', ['iframe'], ['iframe', 'iframe']),
      rule('button-name', 'critical', ['.closeButton']),
      rule('region', 'moderate', ['iframe', 'iframe', 'p']),
    ],
  };
  const axe = partitionAxe(results, 2);
  assert.equal(axe.app_frame_analysed, true);
  assert.deepEqual(axe.app_frame.map((item) => item.id), ['color-contrast']);
  assert.deepEqual(axe.app_frame[0].nodes, [['iframe', 'iframe', 'code']]);
  assert.deepEqual(axe.host_tolerated.map((item) => item.id), UPSTREAM_HOST_RULES);
  assert.deepEqual(axe.host_blocking.map((item) => item.id), ['button-name']);
  // Zero App-frame violations is a claim only when axe actually reached that frame.
  assert.equal(partitionAxe({ passes: [rule('x', null, ['body'])], violations: [], incomplete: [] }, 2).app_frame_analysed, false);
});

test('the run fails when one view fails, when a view is missing, or when the protocol check fails', () => {
  const passedViews = VIEWS.map((view) => ({ tool: view.tool, status: 'passed' }));
  assert.deepEqual(basicHostVerdict(passedViews), { status: 'passed', failed: [] });
  const oneFailed = passedViews.map((item) =>
    item.tool === 'render_timeline' ? { tool: item.tool, status: 'failed', error: 'missing=["Timeline"]' } : item,
  );
  const verdict = basicHostVerdict(oneFailed);
  assert.equal(verdict.status, 'failed');
  assert.deepEqual(verdict.failed, [{ tool: 'render_timeline', error: 'missing=["Timeline"]' }]);
  assert.equal(basicHostVerdict(passedViews.slice(0, 3)).failed[0].tool, 'render_present');

  const protocol = { status: 'passed' };
  assert.deepEqual(runProblems({ protocol, basicHost: { ...basicHostVerdict(passedViews) }, protocolOnly: false }), []);
  assert.match(runProblems({ protocol, basicHost: verdict, protocolOnly: false })[0], /basic_host failed: render_timeline/);
  assert.match(
    runProblems({ protocol, basicHost: { status: 'failed', error: 'chromium missing' }, protocolOnly: false })[0],
    /basic_host failed: chromium missing/,
  );
  assert.deepEqual(runProblems({ protocol, basicHost: { status: 'not_run' }, protocolOnly: true }), []);
  assert.match(runProblems({ protocol: { status: 'failed', error: 'x' }, basicHost: verdict, protocolOnly: true })[0], /protocol_check failed: x/);
});

test('a receipt never records an open tunnel', () => {
  const base = { public_url: 'https://x.ngrok.app/mcp', local_port: 18765, note: 'n' };
  assert.deepEqual(ngrokRecord({ ...base, enabled: false, opened_at: 't1', closed_at: 't2' }), {
    status: 'not_run', opened_at: null, closed_at: null, public_url: null, local_port: 18765, note: 'n',
  });
  assert.deepEqual(ngrokRecord({ ...base, enabled: true, opened_at: 't1', closed_at: 't2' }), {
    status: 'closed', opened_at: 't1', closed_at: 't2', public_url: 'https://x.ngrok.app/mcp', local_port: 18765, note: 'n',
  });
  assert.equal(ngrokRecord({ ...base, enabled: true, opened_at: null, closed_at: null }).status, 'not_opened');
  assert.throws(() => ngrokRecord({ ...base, enabled: true, opened_at: 't1', closed_at: null }), /opened but not closed/);
});
```

- [ ] **Step 2: run and see the failure.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: exit 1, `Cannot find module '../../qualification/mcp-apps/lib/views.mjs'`; 0 pass.

- [ ] **Step 3: implement.** Create `qualification/mcp-apps/lib/views.mjs`:

```js
/**
 * What the MCP Apps qualification expects to see, and how it judges what it saw.
 * Pure: no I/O and no imports, so every rule is testable without a browser.
 *
 * Expected strings come from tests/fixtures/views/*.json as rendered by
 * ui/src/features/{documents/SourceExcerpt,history/Changes,history/Timeline,views/Layout}.tsx.
 * The present fixture has no materialized dataset, so its DataTable and Chart show the
 * two honest "unavailable" alerts listed below; any other alert fails the view.
 */

export const APP_RESOURCE_URI = 'ui://okf-jawn/app.html';
export const APP_ONLY_TOOLS = ['show'];
/** Rules that fire on the upstream basic-host chrome; tolerated there, never in the App frame. */
export const UPSTREAM_HOST_RULES = ['color-contrast', 'frame-title'];

const REVISION = '0123456789abcdef0123456789abcdef01234567';
const SOURCE_ONLY = 'Contract-valid ReadItemResponse fixture for the MCP Apps harness.';
const CHANGES_ONLY = 'Harness fixture line.';
const TIMELINE_ONLY = 'Qualification timeline fixture';
const PRESENT_ONLY = 'Six-component catalog';

export const VIEWS = [
  {
    tool: 'render_source',
    view: 'source',
    mustContain: ['fixtures/qualification-source.md', REVISION, 'Qualification source', SOURCE_ONLY],
    mustNotContain: [CHANGES_ONLY, TIMELINE_ONLY, PRESENT_ONLY],
    alerts: [],
  },
  {
    tool: 'render_changes',
    view: 'changes',
    mustContain: ['Changes', '89abcdef0123456789abcdef0123456789abcdef', 'fixtures/qualification-source.md', CHANGES_ONLY],
    mustNotContain: [SOURCE_ONLY, TIMELINE_ONLY, PRESENT_ONLY],
    alerts: [],
  },
  {
    tool: 'render_timeline',
    view: 'timeline',
    mustContain: ['Timeline', TIMELINE_ONLY, 'okf-qualify-mcp-apps', '89abcdef0123456789abcdef0123456789abcdef'],
    mustNotContain: [SOURCE_ONLY, CHANGES_ONLY, PRESENT_ONLY],
    alerts: [],
  },
  {
    tool: 'render_present',
    view: 'present',
    mustContain: [
      PRESENT_ONLY,
      'Metrics chart',
      SOURCE_ONLY,
      `fixtures/qualification-source.md @ ${REVISION}`,
      `fixtures/qualification-metrics.json @ ${REVISION}`,
    ],
    mustNotContain: [CHANGES_ONLY, TIMELINE_ONLY],
    alerts: ['Dataset unavailable: metrics', 'Resolved chart data or specification unavailable.'],
  },
];

const BLOCKING_TEXT = [
  /Host connection failed/i,
  /Waiting for a tool result from the connected host\./,
  /does not match a supported Source, Changes, Timeline, or Present schema/,
];

/** The official basic-host URL that auto-calls one tool (query keys from its src/index.tsx). */
export function basicHostUrl(tool, { server = 'okf-qualify-mcp-apps', port = 8080 } = {}) {
  const params = new URLSearchParams({ server, tool, call: 'true', theme: 'hide' });
  return `http://127.0.0.1:${port}/?${params}`;
}

/** Whether the App frame shows exactly this view: its text, no other view's, and only its expected alerts. */
export function judgeView(view, observed) {
  const text = observed?.text ?? '';
  const alerts = [...(observed?.alerts ?? [])].map((alert) => alert.trim()).sort();
  const expected_alerts = [...view.alerts].sort();
  const missing = view.mustContain.filter((needle) => !text.includes(needle));
  const foreign = view.mustNotContain.filter((needle) => text.includes(needle));
  const blocking = BLOCKING_TEXT.filter((pattern) => pattern.test(text)).map(String);
  const ok =
    observed !== null &&
    observed !== undefined &&
    missing.length === 0 &&
    foreign.length === 0 &&
    blocking.length === 0 &&
    JSON.stringify(alerts) === JSON.stringify(expected_alerts);
  return { ok, missing, foreign, blocking, alerts, expected_alerts };
}

const SERIOUS = new Set(['serious', 'critical']);
/** Frame hops to a node: axe gives one target entry per frame boundary plus one for the element. */
const depthOf = (node) => (Array.isArray(node?.target) ? node.target.length - 1 : 0);

/**
 * Split serious/critical axe violations by where their nodes live.
 * `appDepth` is the App document's frame depth (2 in basic-host: host -> sandbox -> App).
 * Host chrome may violate only UPSTREAM_HOST_RULES; the App frame may violate nothing.
 */
export function partitionAxe(results, appDepth) {
  const inApp = (node) => depthOf(node) >= appDepth;
  const everyNode = ['violations', 'passes', 'incomplete'].flatMap((key) =>
    (results?.[key] ?? []).flatMap((rule) => rule.nodes ?? []),
  );
  const app_frame = [];
  const host_blocking = [];
  const host_tolerated = [];
  for (const rule of results?.violations ?? []) {
    if (!SERIOUS.has(rule.impact)) continue;
    const appNodes = (rule.nodes ?? []).filter(inApp);
    const hostNodes = (rule.nodes ?? []).filter((node) => !inApp(node));
    const entry = (nodes) => ({ id: rule.id, impact: rule.impact, nodes: nodes.map((node) => node.target) });
    if (appNodes.length) app_frame.push(entry(appNodes));
    if (hostNodes.length) {
      (UPSTREAM_HOST_RULES.includes(rule.id) ? host_tolerated : host_blocking).push(entry(hostNodes));
    }
  }
  return {
    app_frame_analysed: appDepth > 0 && everyNode.some(inApp),
    app_frame,
    host_blocking,
    host_tolerated,
  };
}

/** Passed only when every view in VIEWS has a passed result. */
export function basicHostVerdict(results) {
  const failed = [];
  for (const view of VIEWS) {
    const result = results.find((item) => item.tool === view.tool);
    if (!result) failed.push({ tool: view.tool, error: 'not rendered' });
    else if (result.status !== 'passed') {
      failed.push({ tool: view.tool, error: result.error ?? 'failed without an error message' });
    }
  }
  return { status: failed.length === 0 ? 'passed' : 'failed', failed };
}

/** Reasons the run must exit non-zero; empty when it may exit 0. */
export function runProblems({ protocol, basicHost, protocolOnly }) {
  const problems = [];
  if (protocol?.status !== 'passed') {
    problems.push(`protocol_check ${protocol?.status ?? 'missing'}: ${protocol?.error ?? 'no detail'}`);
  }
  if (!protocolOnly && basicHost?.status !== 'passed') {
    const detail =
      basicHost?.error ??
      (basicHost?.failed ?? []).map((item) => `${item.tool}: ${item.error}`).join('; ') ??
      'no detail';
    problems.push(`basic_host ${basicHost?.status ?? 'missing'}: ${detail}`);
  }
  return problems;
}

/** The tunnel lifecycle as a receipt may state it. An opened tunnel must have been closed. */
export function ngrokRecord({ enabled, opened_at, closed_at, public_url, local_port, note }) {
  if (!enabled) {
    return { status: 'not_run', opened_at: null, closed_at: null, public_url: null, local_port, note };
  }
  if (opened_at === null || opened_at === undefined) {
    return { status: 'not_opened', opened_at: null, closed_at: closed_at ?? null, public_url: null, local_port, note };
  }
  if (typeof closed_at !== 'string') {
    throw new Error('ngrok was opened but not closed; a receipt never records an open tunnel');
  }
  return { status: 'closed', opened_at, closed_at, public_url: public_url ?? null, local_port, note };
}
```

- [ ] **Step 4: run and see it pass.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: `33 pass`, `0 fail`.

- [ ] **Step 5: commit.**

```sh
git add qualification/mcp-apps/lib/views.mjs tests/foundation/harness.test.mjs
git commit -F - <<'MSG'
feat(qualify): judge all four MCP App views and scope axe exclusions to host chrome.

Why: the basic-host check rendered render_source only (run.mjs:342) and its
failure did not fail the run (:603-609); color-contrast and frame-title were
disabled for the whole page (:396-398), so the App frame was never checked for
them; the tunnel state lived in a mutated object that passed through
"session_open" (:459).
What changed: qualification/mcp-apps/lib/views.mjs defines the four views with
the text each must and must not show and the alerts each may show, judgeView,
partitionAxe (App frame judged by every rule; only the two upstream rules
tolerated on host chrome; "no violations" requires that axe reached the App
frame), basicHostVerdict, runProblems and ngrokRecord (throws on an open
tunnel).
Verified: bun test ./tests/foundation/harness.test.mjs -> 33 pass, 0 fail.
Next: Task B.9 wires these into qualification/mcp-apps/run.mjs.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
MSG
```

---

### Task B.9: Wire the MCP Apps orchestrator

**Files:**
- Modify: `qualification/mcp-apps/run.mjs` — `:1-7` (header comment), `:15` (import), `:31-32` (`NGROK_ENABLED`), `:69-124` (delete `killProcessTree`, `spawnDetached`, `waitForTcp`), `:142-233` (`protocolCheck`), `:314-422` (`runBasicHostCheck`), `:424-466` (`maybeNgrok`), `:498-683` (main flow). Kept as they are: `run` (`:37-67`), `waitForHttp` (`:126-140`), `ensureBasicHost` (`:235-294`), `buildBasicHost` (`:296-312`), `hostRenderFromEvidence` (`:468-496`).
- Test: `tests/foundation/harness.test.mjs` (one appended test)

**Interfaces:**
- Consumes: B.1 (`receiptHeader`, `recordReceipt`), B.2 (`buildRelease`), B.6 (the `show` tool, `app_tools`), B.7, B.8.
- Produces:
  - CLI: `bun qualification/mcp-apps/run.mjs [--record]`; environment `OKF_MCP_APPS_PROTOCOL_ONLY=1`, `OKF_MCP_APPS_NGROK=1`, `OKF_MCP_APPS_PORT`. Exit 0 only when `runProblems` is empty. `--record` with protocol-only is refused before anything runs.
  - MCP Apps receipt shape (`.artifacts/qualification/mcp-apps/receipt.json`), top level:

```json
{
  "git_sha": "<40 hex>", "inputs": ["qualification/mcp-apps", "qualification/lib", "tests/fixtures/views", "ui/src/mcp-apps", "ui/scripts/bundle-app.mjs", "Cargo.toml", "Cargo.lock"], "produced_at": "<ISO>",
  "component": "mcp-apps-protocol-qualification", "gate": "mcp-apps-protocol-qualification",
  "finished_at": "<ISO>", "result": "PASS | FAIL", "problems": [],
  "harness": "okf-qualify-mcp-apps", "protocol_only": false,
  "transport": { "stdio": "default", "http": "http://127.0.0.1:18765/mcp", "build_command": "cargo build --locked --release -p okf-qualify-mcp-apps", "serve_command": "okf-qualify-mcp-apps --http 127.0.0.1:18765" },
  "harness_process": { "exited_before_teardown": false, "exit": { "code": 1, "signal": null, "error": null }, "stderr_tail": "okf-qualify-mcp-apps listening on …" },
  "mime_check": {}, "bundle_sizes": {}, "resources_readable": {}, "check": {},
  "protocol_check": { "status": "passed", "tools": [], "app_only_tools": ["show"], "resources": [], "tool_calls": [ { "name": "render_source", "has_structured_content": true, "text_fallback": "…" } ], "show_calls": [ { "binding": "venue", "item_id": "…", "revision": "…" } ] },
  "basic_host": { "status": "passed | failed", "failed": [], "source": "cached | fetched_v2.0.3", "views": [ { "tool": "render_source", "view": "source", "status": "passed", "url": "…", "screenshot": "…", "app_frame_depth": 2, "feature_text": "…", "alerts": [], "expected_alerts": [], "axe": { "app_frame_analysed": true, "app_frame": [], "host_blocking": [], "host_tolerated": [] } } ], "axe_tolerated_upstream_host_rules": ["color-contrast", "frame-title"], "present_dataset": "not_exercised: …", "note": "…" },
  "ngrok": { "status": "not_run | not_opened | closed", "opened_at": null, "closed_at": null, "public_url": null, "local_port": 18765, "note": "…" },
  "host_render": {}, "static_bundle_smoke": "…", "how_to_http_ngrok": {}, "note": "…"
}
```

- [ ] **Step 1: write the failing test.** Append to `tests/foundation/harness.test.mjs`:

```js
test('the MCP Apps orchestrator renders every view, fails on any failure and never tolerates App-frame rules', async () => {
  const source = await readFile(join(root, 'qualification/mcp-apps/run.mjs'), 'utf8');
  assert.match(source, /for \(const view of VIEWS\)/);
  assert.match(source, /runProblems\(\{ protocol, basicHost, protocolOnly: PROTOCOL_ONLY \}\)/);
  assert.match(source, /waitForListening\(harness,/);
  assert.match(source, /partitionAxe\(/);
  assert.match(source, /ngrokRecord\(/);
  assert.match(source, /receiptHeader\(root, MCP_APPS_INPUTS\)/);
  assert.match(source, /recordReceipt\(root, 'mcp-apps', receipt\)/);
  assert.doesNotMatch(source, /disableRules\(|session_open|spawnDetached|waitForTcp|requireCleanTree|commit_sha/);
  assert.doesNotMatch(source, /'cargo',\s*\[\s*'run'/);
});
```

This test pins wiring only; the rules themselves are proven by the unit tests of B.1, B.7 and B.8.

- [ ] **Step 2: run and see the failure.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: `33 pass`, `1 fail`; the failing assertion is `assert.match(source, /for \(const view of VIEWS\)/)`.

- [ ] **Step 3: implement.** Edit `qualification/mcp-apps/run.mjs` from the bottom upward.

**`:498-683`** — replace everything from `const commitSha = await requireCleanTree(root);` to the end of the file with:

```js
const record = process.argv.includes('--record');
if (record && PROTOCOL_ONLY) {
  throw new Error('--record refused: a protocol-only run is not the gate receipt');
}
const header = await receiptHeader(root, MCP_APPS_INPUTS);

await mkdir(outDir, { recursive: true });
await mkdir(hostsDir, { recursive: true });

const build = await run('bun', ['--bun', 'run', 'build'], { cwd: uiDir });
if (build.code !== 0) {
  throw new Error(`ui build exited ${build.code}`);
}

const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
const resources = Array.isArray(manifest.resources) ? manifest.resources : [];
if (resources.length !== 1) {
  throw new Error(`manifest must list 1 shared App resource, found ${resources.length}`);
}
if (resources[0]?.uri !== APP_RESOURCE_URI) {
  throw new Error(`manifest uri must be ${APP_RESOURCE_URI}, got ${resources[0]?.uri}`);
}

const mimeMismatches = resources.filter((resource) => resource.mimeType !== RESOURCE_MIME_TYPE);
const mimeCheck = {
  sdk_constant: RESOURCE_MIME_TYPE,
  manifest_mime_types: resources.map((resource) => ({
    name: resource.name,
    mimeType: resource.mimeType,
  })),
  equal: mimeMismatches.length === 0,
};
if (!mimeCheck.equal) {
  throw new Error(
    `manifest mimeType must equal SDK RESOURCE_MIME_TYPE (${RESOURCE_MIME_TYPE}); mismatches: ${JSON.stringify(mimeMismatches)}`,
  );
}

// Build once, then run the binary itself: its stderr and exit code are ours to read,
// and the process group holds the harness, not `cargo run`.
const harnessBin = await buildRelease(root, 'okf-qualify-mcp-apps');

const checkRun = await run(harnessBin, ['--check'], {
  env: { ...process.env, OKF_MCP_APPS_DIST: distApps },
  stdio: ['ignore', 'pipe', 'pipe'],
});
if (checkRun.code !== 0) {
  throw new Error(`okf-qualify-mcp-apps --check exited ${checkRun.code}\n${checkRun.stderr}`);
}

let check;
try {
  const trimmed = checkRun.stdout.trim();
  const start = trimmed.indexOf('{');
  const end = trimmed.lastIndexOf('}');
  if (start < 0 || end < start) throw new Error('no JSON object in stdout');
  check = JSON.parse(trimmed.slice(start, end + 1));
} catch (error) {
  throw new Error(`could not parse harness check JSON: ${error.message}\n${checkRun.stdout}`);
}

const readable = Object.fromEntries(
  (check.resources ?? []).map((resource) => [resource.name, Boolean(resource.readable)]),
);
if (!readable.app) {
  throw new Error('shared App resource was not reported readable by the harness');
}

const harness = spawnGroup(harnessBin, ['--http', HTTP_BIND], {
  env: {
    ...process.env,
    OKF_MCP_APPS_DIST: distApps,
    // Explicit either way: an inherited value must not lift Host protection.
    OKF_MCP_APPS_NGROK: NGROK_ENABLED && !PROTOCOL_ONLY ? '1' : '0',
  },
});

let protocol = { status: 'not_run' };
let basicHost = { status: 'not_run', reason: PROTOCOL_ONLY ? 'skipped: protocol-only' : undefined };
let tunnel = null;
let fatal = null;
let harnessExit = null;
let harnessDiedEarly = false;

try {
  await waitForListening(harness, {
    pattern: /okf-qualify-mcp-apps listening on (\S+)/,
    label: 'okf-qualify-mcp-apps',
  });
  try {
    protocol = await protocolCheck(MCP_URL, manifest);
  } catch (error) {
    protocol = { status: 'failed', error: String(error.message || error) };
  }

  if (!PROTOCOL_ONLY) {
    if (NGROK_ENABLED) {
      try {
        tunnel = await openNgrok(HTTP_PORT);
      } catch (error) {
        tunnel = { proc: null, opened_at: null, public_url: null, note: String(error.message || error) };
      }
    }
    try {
      basicHost = await runBasicHostCheck(MCP_URL);
    } catch (error) {
      basicHost = {
        status: 'failed',
        error: String(error.message || error),
        note: '@modelcontextprotocol/ext-apps npm package does not ship basic-host; attempted official v2.0.3 GitHub example.',
      };
    }
  }
} catch (error) {
  fatal = error;
  protocol = { status: 'failed', error: String(error.message || error) };
} finally {
  if (tunnel) {
    if (tunnel.proc) await killProcessTree(tunnel.proc);
    tunnel.closed_at = new Date().toISOString();
  }
  harnessDiedEarly = harness.child.exitCode !== null;
  harnessExit = await killProcessTree(harness);
}

const ngrok = ngrokRecord({
  enabled: NGROK_ENABLED && !PROTOCOL_ONLY,
  opened_at: tunnel?.opened_at ?? null,
  closed_at: tunnel?.closed_at ?? null,
  public_url: tunnel?.public_url ?? null,
  local_port: HTTP_PORT,
  note: tunnel?.note ?? (PROTOCOL_ONLY ? 'skipped: protocol-only' : 'OKF_MCP_APPS_NGROK is not 1'),
});

const host_render = PROTOCOL_ONLY
  ? { status: 'not_run', reason: 'skipped: protocol-only' }
  : await hostRenderFromEvidence();

const problems = runProblems({ protocol, basicHost, protocolOnly: PROTOCOL_ONLY });
if (fatal) problems.unshift(`harness: ${String(fatal.message || fatal)}`);

const receipt = {
  ...header,
  component: 'mcp-apps-protocol-qualification',
  gate: 'mcp-apps-protocol-qualification',
  finished_at: new Date().toISOString(),
  result: problems.length === 0 ? 'PASS' : 'FAIL',
  problems,
  harness: 'okf-qualify-mcp-apps',
  protocol_only: PROTOCOL_ONLY,
  transport: {
    stdio: 'default',
    http: MCP_URL,
    build_command: 'cargo build --locked --release -p okf-qualify-mcp-apps',
    serve_command: `okf-qualify-mcp-apps --http ${HTTP_BIND}`,
  },
  harness_process: {
    exited_before_teardown: harnessDiedEarly,
    exit: harnessExit,
    stderr_tail: harness.stderr().slice(-2000),
  },
  mime_check: mimeCheck,
  bundle_sizes: Object.fromEntries(
    resources.map((resource) => [resource.name, resource.byteLength]),
  ),
  resources_readable: readable,
  check,
  protocol_check: protocol,
  basic_host: basicHost,
  ngrok,
  host_render,
  static_bundle_smoke:
    'ui/tests/e2e/mcp-apps-static-bundle-smoke.spec.ts is smoke only; not a host-render check.',
  how_to_http_ngrok: {
    harness: `cargo run --locked -p okf-qualify-mcp-apps --release -- --http ${HTTP_BIND}`,
    or_env: `OKF_MCP_APPS_HTTP=${HTTP_BIND}`,
    ngrok: `OKF_MCP_APPS_NGROK=1 ngrok http ${HTTP_PORT}`,
    connector_url: `https://<ngrok-host>/mcp`,
  },
  note: 'Phase 0 MCP Apps protocol qualification against the official basic-host example. host_render (claude.ai/ChatGPT) belongs to the acceptance gate mcp-apps-web-hosts and is never invented here.',
};

await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
process.stdout.write(`MCP Apps qualification receipt: ${receiptPath}\n`);
if (record) {
  process.stdout.write(`MCP Apps receipt recorded: ${await recordReceipt(root, 'mcp-apps', receipt)}\n`);
}

if (problems.length) {
  throw new Error(`MCP Apps qualification FAIL:\n${problems.join('\n')}`);
}
```

**`:424-466`** — replace `maybeNgrok` with:

```js
async function openNgrok(port) {
  const opened_at = new Date().toISOString();
  const proc = spawnGroup('ngrok', ['http', String(port), '--log=stdout']);
  let gone = false;
  proc.exited.then(() => {
    gone = true;
  });
  const deadline = Date.now() + 30_000;
  let publicUrl = null;
  while (!gone && Date.now() < deadline && !publicUrl) {
    try {
      const response = await fetch('http://127.0.0.1:4040/api/tunnels');
      if (response.ok) {
        const payload = await response.json();
        const entry = (payload.tunnels ?? []).find((item) => item.public_url?.startsWith('https://'));
        if (entry) publicUrl = entry.public_url;
      }
    } catch {
      // retry
    }
    if (!publicUrl) await new Promise((r) => setTimeout(r, 500));
  }
  return {
    proc,
    opened_at,
    public_url: publicUrl ? `${publicUrl}/mcp` : null,
    note: publicUrl
      ? 'Short-lived public URL for manual claude.ai / ChatGPT connector testing; closed before this receipt was written. Not Cloudflare.'
      : `ngrok started but no public https URL appeared on 127.0.0.1:4040; stderr: ${proc.stderr().slice(0, 1000)}`,
  };
}
```

**`:314-422`** — replace `runBasicHostCheck` with:

```js
function frameDepth(frame) {
  let depth = 0;
  for (let parent = frame.parentFrame(); parent; parent = parent.parentFrame()) depth += 1;
  return depth;
}

/** The App document: host page (0) -> sandbox proxy on :8081 (1) -> App (2). */
function appFrameOf(page) {
  return (
    page
      .frames()
      .map((frame) => ({ frame, depth: frameDepth(frame) }))
      .filter((item) => item.depth >= 2)
      .sort((a, b) => b.depth - a.depth)[0] ?? null
  );
}

async function readFrame(frame) {
  try {
    const text = await frame.locator('body').innerText({ timeout: 1_000 });
    const alerts = await frame.locator('[role="alert"]').allInnerTexts();
    return { text, alerts };
  } catch {
    return null;
  }
}

async function renderView(browser, AxeBuilder, view) {
  const url = basicHostUrl(view.tool);
  const screenshot = join(outDir, `basic-host-${view.tool}.png`);
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 60_000 });

    let located = null;
    let observed = null;
    let verdict = judgeView(view, null);
    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline && !verdict.ok) {
      const hostText = await page.locator('body').innerText();
      if (/Failed to connect to any servers/i.test(hostText)) {
        throw new Error(`basic-host could not reach the harness: ${hostText.slice(0, 200)}`);
      }
      located = appFrameOf(page);
      observed = located ? await readFrame(located.frame) : null;
      verdict = judgeView(view, observed);
      if (!verdict.ok) await page.waitForTimeout(500);
    }
    await page.screenshot({ path: screenshot, fullPage: true });

    const base = {
      tool: view.tool,
      view: view.view,
      url,
      screenshot,
      app_frame_depth: located?.depth ?? null,
      feature_text: (observed?.text ?? '').slice(0, 400),
      alerts: verdict.alerts,
      expected_alerts: verdict.expected_alerts,
    };
    if (!verdict.ok) {
      return {
        ...base,
        status: 'failed',
        error: `App frame did not show the ${view.view} view: missing=${JSON.stringify(verdict.missing)} foreign=${JSON.stringify(verdict.foreign)} blocking=${JSON.stringify(verdict.blocking)} alerts=${JSON.stringify(verdict.alerts)}`,
      };
    }

    // Every rule runs on the whole page; partitionAxe tolerates the two upstream rules
    // on host chrome only and requires proof that axe reached the App frame.
    const axe = partitionAxe(await new AxeBuilder({ page }).analyze(), located.depth);
    const axeProblems = [];
    if (!axe.app_frame_analysed) axeProblems.push('axe did not analyse the App frame');
    if (axe.app_frame.length) {
      axeProblems.push(`App frame serious/critical: ${JSON.stringify(axe.app_frame)}`);
    }
    if (axe.host_blocking.length) {
      axeProblems.push(
        `host chrome serious/critical outside the tolerated upstream rules: ${JSON.stringify(axe.host_blocking)}`,
      );
    }
    return axeProblems.length
      ? { ...base, axe, status: 'failed', error: axeProblems.join('; ') }
      : { ...base, axe, status: 'passed' };
  } finally {
    await context.close().catch(() => {});
  }
}

async function runBasicHostCheck(mcpUrl) {
  // Package does not ship basic-host HTML; use the tagged GitHub example.
  const ensured = await ensureBasicHost();
  await buildBasicHost();
  const host = spawnGroup('bun', ['serve.ts'], {
    cwd: basicHostDir,
    env: {
      ...process.env,
      SERVERS: JSON.stringify([mcpUrl]),
      // Must match hardcoded SANDBOX_PROXY_BASE_URL in basic-host src/implementation.ts.
      HOST_PORT: '8080',
      SANDBOX_PORT: '8081',
    },
  });
  try {
    let serving = false;
    const exitedEarly = host.exited.then(({ code, error }) => {
      if (serving) return;
      throw new Error(
        `basic-host server exited before serving (exit ${code ?? 'none'}${error ? `, ${error}` : ''})\n${host.stderr().slice(-2000)}`,
      );
    });
    try {
      await Promise.race([waitForHttp('http://127.0.0.1:8080/api/servers'), exitedEarly]);
    } finally {
      serving = true; // from here an exit is teardown, not a rejection nobody awaits
    }

    const { chromium } = await import(
      pathToFileURL(join(uiDir, 'node_modules/@playwright/test/index.mjs')).href
    );
    const AxeBuilder = (
      await import(pathToFileURL(join(uiDir, 'node_modules/@axe-core/playwright/dist/index.js')).href)
    ).default;

    const browser = await chromium.launch({ headless: true });
    const views = [];
    try {
      for (const view of VIEWS) {
        try {
          views.push(await renderView(browser, AxeBuilder, view));
        } catch (error) {
          views.push({
            tool: view.tool,
            view: view.view,
            status: 'failed',
            error: String(error.message || error),
          });
        }
      }
    } finally {
      await browser.close().catch(() => {});
    }
    return {
      ...basicHostVerdict(views),
      source: ensured.source,
      views,
      axe_tolerated_upstream_host_rules: UPSTREAM_HOST_RULES,
      present_dataset:
        'not_exercised: tests/fixtures/views/present-response.json has no materialized dataset, so the DataTable and Chart show their "unavailable" alerts; binding resolution through the app-only show tool is exercised',
      note: 'Official ext-apps basic-host example (v2.0.3) against the Streamable HTTP harness; not a claude.ai/ChatGPT claim.',
    };
  } finally {
    await killProcessTree(host);
  }
}
```

**`:142-233`** — replace `protocolCheck` with:

```js
async function protocolCheck(mcpUrl, manifest) {
  const clientMod = await import(
    pathToFileURL(join(uiDir, 'node_modules/@modelcontextprotocol/client/dist/index.mjs')).href
  );
  const { Client, StreamableHTTPClientTransport } = clientMod;
  const client = new Client({ name: 'okf-qualify-mcp-apps-protocol', version: '0.1.0' });
  const transport = new StreamableHTTPClientTransport(new URL(mcpUrl));
  await client.connect(transport);
  try {
    const tools = await client.listTools();
    const listed = Array.isArray(tools.tools) ? tools.tools : [];
    const names = (items) => JSON.stringify(items.map((tool) => tool.name).sort());
    const renderTools = listed.filter((tool) => typeof tool._meta?.ui?.resourceUri === 'string');
    const appOnly = listed.filter(
      (tool) => JSON.stringify(tool._meta?.ui?.visibility ?? null) === '["app"]',
    );
    const expectedRender = JSON.stringify(VIEWS.map((view) => view.tool).sort());
    if (names(renderTools) !== expectedRender) {
      throw new Error(`tools/list render tools ${names(renderTools)} !== ${expectedRender}`);
    }
    for (const tool of renderTools) {
      if (tool._meta.ui.resourceUri !== APP_RESOURCE_URI) {
        throw new Error(`tool ${tool.name} resourceUri ${tool._meta.ui.resourceUri} !== ${APP_RESOURCE_URI}`);
      }
    }
    if (names(appOnly) !== JSON.stringify([...APP_ONLY_TOOLS].sort())) {
      throw new Error(`tools/list app-only tools ${names(appOnly)} !== ${JSON.stringify(APP_ONLY_TOOLS)}`);
    }
    if (listed.length !== renderTools.length + appOnly.length) {
      throw new Error(`tools/list has tools that are neither render tools nor app-only: ${names(listed)}`);
    }

    const resources = await client.listResources();
    if (!Array.isArray(resources.resources) || resources.resources.length !== 1) {
      throw new Error(
        `resources/list expected 1 shared App resource, got ${resources.resources?.length}`,
      );
    }
    if (resources.resources[0]?.uri !== APP_RESOURCE_URI) {
      throw new Error(
        `resources/list uri must be ${APP_RESOURCE_URI}, got ${resources.resources[0]?.uri}`,
      );
    }
    const reads = [];
    for (const resource of resources.resources) {
      const read = await client.readResource({ uri: resource.uri });
      const content = read.contents?.[0];
      if (!content || typeof content.text !== 'string') {
        throw new Error(`resources/read ${resource.uri} missing text`);
      }
      if (content.mimeType !== RESOURCE_MIME_TYPE) {
        throw new Error(
          `resources/read ${resource.uri} mimeType ${content.mimeType} !== ${RESOURCE_MIME_TYPE}`,
        );
      }
      const csp = content._meta?.ui?.csp;
      if (!csp || typeof csp !== 'object' || Array.isArray(csp)) {
        throw new Error(`resources/read ${resource.uri} _meta.ui.csp must be an object`);
      }
      if (!Array.isArray(csp.connectDomains) || !Array.isArray(csp.resourceDomains)) {
        throw new Error(
          `resources/read ${resource.uri} _meta.ui.csp needs connectDomains and resourceDomains arrays`,
        );
      }
      const hash = createHash('sha256').update(content.text, 'utf8').digest('hex');
      const expected = manifest.resources.find((entry) => entry.uri === resource.uri);
      if (!expected) throw new Error(`manifest missing ${resource.uri}`);
      if (hash !== expected.sha256) {
        throw new Error(
          `resources/read ${resource.uri} sha256 ${hash} !== manifest ${expected.sha256}`,
        );
      }
      reads.push({
        uri: resource.uri,
        mimeType: content.mimeType,
        sha256: hash,
        cspObject: true,
      });
    }

    // Every render tool: structuredContent for the App, text for hosts without Apps.
    const toolCalls = [];
    let present = null;
    for (const view of VIEWS) {
      const call = await client.callTool({ name: view.tool, arguments: {} });
      if (!call.structuredContent) throw new Error(`${view.tool} missing structuredContent`);
      const textBlock = (call.content ?? []).find((block) => block.type === 'text');
      if (!textBlock || typeof textBlock.text !== 'string' || textBlock.text.length === 0) {
        throw new Error(`${view.tool} missing text fallback`);
      }
      if (view.tool === 'render_present') present = call.structuredContent;
      toolCalls.push({
        name: view.tool,
        has_structured_content: true,
        text_fallback: textBlock.text,
      });
    }

    // The present view resolves each binding through the app-only show tool
    // (arguments as ui/src/features/views/PresentView.tsx sends them).
    const showCalls = [];
    for (const binding of present?.resolved_bindings ?? []) {
      const shown = await client.callTool({
        name: 'show',
        arguments: {
          workspace_id: binding.source.workspace_id,
          item_id: binding.source.item_id,
          at: { kind: 'revision', revision: binding.source.revision },
          view: 'text',
          selection: binding.source.selection,
          max_bytes: 65536,
          max_images: 0,
        },
      });
      const source = shown.structuredContent?.source;
      if (source?.item_id !== binding.source.item_id || source?.revision !== binding.source.revision) {
        throw new Error(`show did not return the bound source for binding ${binding.name}`);
      }
      showCalls.push({ binding: binding.name, item_id: source.item_id, revision: source.revision });
    }
    if (showCalls.length === 0) throw new Error('render_present has no resolved_bindings to show');

    return {
      status: 'passed',
      tools: renderTools.map((tool) => ({
        name: tool.name,
        resourceUri: tool._meta.ui.resourceUri,
      })),
      app_only_tools: appOnly.map((tool) => tool.name),
      resources: reads,
      tool_calls: toolCalls,
      show_calls: showCalls,
    };
  } finally {
    await client.close().catch(() => {});
  }
}
```

**`:69-124`** — delete `killProcessTree`, `spawnDetached` and `waitForTcp` entirely (the functions between the end of `run` and the start of `waitForHttp`).

**`:31-32`** — replace the two `NGROK_ENABLED` lines with, and add the inputs constant below them:

```js
const NGROK_ENABLED = process.env.OKF_MCP_APPS_NGROK === '1';
const MCP_APPS_INPUTS = [
  'qualification/mcp-apps',
  'qualification/lib',
  'tests/fixtures/views',
  'ui/src/mcp-apps',
  'ui/scripts/bundle-app.mjs',
  'Cargo.toml',
  'Cargo.lock',
];
```

**`:15`** — replace `import { requireCleanTree } from '../../scripts/lib/provenance.mjs';` with:

```js
import { receiptHeader, recordReceipt } from '../../scripts/lib/provenance.mjs';
import { buildRelease } from '../lib/cargo.mjs';
import { killProcessTree, spawnGroup, waitForListening } from './lib/process.mjs';
import {
  APP_ONLY_TOOLS,
  APP_RESOURCE_URI,
  UPSTREAM_HOST_RULES,
  VIEWS,
  basicHostUrl,
  basicHostVerdict,
  judgeView,
  ngrokRecord,
  partitionAxe,
  runProblems,
} from './lib/views.mjs';
```

**`:1-7`** — replace the header comment with:

```js
/**
 * Orchestrate Phase 0 MCP Apps protocol qualification.
 *
 * Steps: rebuild dist-apps, build the harness binary, harness --check, Streamable HTTP
 * protocol check over all four render tools and the app-only show tool, then (unless
 * OKF_MCP_APPS_PROTOCOL_ONLY=1) the official basic-host example rendering each of the
 * four views under Playwright with axe, and an optional ngrok URL (OKF_MCP_APPS_NGROK=1)
 * that is always closed before the receipt is written.
 *
 * Usage (from PowerShell, cargo on PATH): bun qualification/mcp-apps/run.mjs [--record]
 * Exits non-zero when the protocol check or any view fails; the receipt is written first.
 * host_render reports only evidence found under .artifacts/.../hosts/ and never a PASS.
 */
```

- [ ] **Step 4: run and see it pass.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: `34 pass`, `0 fail`.
  Run: `bun build --no-bundle qualification/mcp-apps/run.mjs --outfile .artifacts/syntax/mcp-apps-run.js`
  Expected: exit 0.
  Run: `git grep -n "killProcessTree(host.child)\|spawnDetached\|waitForTcp\|commitSha" -- qualification/mcp-apps/run.mjs`
  Expected: no output (exit 1).

- [ ] **Step 5: commit, then prove the wiring with a protocol-only run** (needs `ui/node_modules` from `bun scripts/dev.mjs bootstrap` and cargo; no network, no Playwright, no model assets). Commit first, because the run needs a clean tree:

```sh
git add qualification/mcp-apps/run.mjs tests/foundation/harness.test.mjs
git commit -F - <<'MSG'
fix(qualify): render all four MCP App views and fail the run when any check fails.

Why: the basic-host check rendered one view (run.mjs:342) and its failure was
swallowed (:603-609), so a run could exit 0 with basic_host failed; axe disabled
color-contrast and frame-title for the whole page (:396-398); the harness ran
under `cargo run`, so a failed bind appeared as a TCP timeout with stderr and
exit code lost; the receipt was labelled with the acceptance gate
mcp-apps-web-hosts. SPEC 9: "MCP Apps supplies source, Changes, Timeline and
constrained presentation views."
What changed: run.mjs takes its header from receiptHeader, builds the harness
once and spawns the binary in its own process group, waits for its listening
line (or reports its exit code and stderr at once), checks all four render
tools plus the app-only show tool over Streamable HTTP, renders each view in
basic-host and judges its text, alerts and axe results with the App frame held
to every rule, records the tunnel only as not_run / not_opened / closed, writes
result and problems, supports --record, and exits non-zero on any problem.
OKF_MCP_APPS_NGROK must be exactly 1. component/gate are
mcp-apps-protocol-qualification.
Verified: bun test ./tests/foundation/harness.test.mjs -> 34 pass, 0 fail;
bun build --no-bundle qualification/mcp-apps/run.mjs -> exit 0.
Next: protocol-only run on this commit, then Task B.10 (iii header).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
MSG
```

  Then run (PowerShell): `$env:OKF_MCP_APPS_PROTOCOL_ONLY = '1'; bun qualification/mcp-apps/run.mjs; $LASTEXITCODE; Remove-Item Env:OKF_MCP_APPS_PROTOCOL_ONLY`
  Expected: exit 0; `.artifacts/qualification/mcp-apps/receipt.json` has `result: "PASS"`, `protocol_only: true`, `protocol_check.status: "passed"`, `protocol_check.app_only_tools: ["show"]`, four `tool_calls`, two `show_calls` (`venue`, `metrics`), `ngrok.status: "not_run"`, `harness_process.exited_before_teardown: false`, and top-level `git_sha` equal to `git rev-parse HEAD`. `git status --porcelain` is empty afterwards.
  If `ui/node_modules` is absent, record that this run was not possible in your handoff instead of running `bootstrap` in a way that touches lockfiles. If the run fails on a defect in your files, fix it in a new commit (`fix(qualify): …`, same format) and re-run.

---

### Task B.10: iii orchestrator — shared header only

**Files:**
- Modify: `qualification/iii/run.mjs:17` (import), `:488` (header), `:800-808` (receipt head), `:837-838` (record)
- Test: `tests/foundation/harness.test.mjs` (one appended test)

**Interfaces:**
- Consumes: `receiptHeader`, `recordReceipt` (B.1).
- Produces: `.artifacts/qualification/iii/receipt.json` starts with `{ git_sha, inputs: ["qualification/iii", "Cargo.toml", "Cargo.lock"], produced_at }`; `commit_sha` is gone. CLI: `bun qualification/iii/run.mjs [--record]`. **Unchanged and expected:** `decision: "REJECTED_WITH_FALLBACK"` and exit code 1. The orchestrator-failure receipt written by the `.catch` handler (`:843-856`) has no header and therefore can never be recorded.

- [ ] **Step 1: write the failing test.** Append to `tests/foundation/harness.test.mjs`:

```js
test('all three orchestrators take their header from receiptHeader and record only through recordReceipt', async () => {
  for (const name of ['docling', 'mcp-apps', 'iii']) {
    const source = await readFile(join(root, 'qualification', name, 'run.mjs'), 'utf8');
    assert.match(source, /= await receiptHeader\(root, /, name);
    assert.match(source, new RegExp(`recordReceipt\\(root, '${name}', receipt\\)`), name);
    assert.match(source, /process\.argv\.includes\('--record'\)/, name);
    assert.doesNotMatch(source, /requireCleanTree|commit_sha|writeFile\([^)]*qualification\/receipts/, name);
  }
});
```

- [ ] **Step 2: run and see the failure.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: `34 pass`, `1 fail`; the message names `iii`.

- [ ] **Step 3: implement.** In `qualification/iii/run.mjs`, bottom upward:

`:837-838` — after `await writeReceipt(receipt);` and before the `process.stdout.write(\`iii qualification receipt: …` line, insert:

```js
  if (process.argv.includes('--record')) {
    process.stdout.write(`iii receipt recorded: ${await recordReceipt(root, 'iii', receipt)}\n`);
  }
```

`:800-808` — replace

```js
  const receipt = {
    component: 'iii-phase0',
    commit_sha: commitSha,
    git_sha: commitSha,
    inputs: [
      'qualification/iii',
      'Cargo.toml',
      'Cargo.lock',
    ],
```

with

```js
  const receipt = {
    ...header,
    component: 'iii-phase0',
```

`:488` — replace `  const commitSha = await requireCleanTree(root);` with:

```js
  const header = await receiptHeader(root, ['qualification/iii', 'Cargo.toml', 'Cargo.lock']);
```

`:17` — replace the import with:

```js
import { receiptHeader, recordReceipt } from '../../scripts/lib/provenance.mjs';
```

Nothing else in this file changes.

- [ ] **Step 4: run and see it pass.**
  Run: `bun test ./tests/foundation/harness.test.mjs`
  Expected: `35 pass`, `0 fail`.
  Run: `bun build --no-bundle qualification/iii/run.mjs --outfile .artifacts/syntax/iii-run.js`
  Expected: exit 0.
  Run: `git diff --stat -- qualification/iii/run.mjs`
  Expected: one file, about 6 insertions and 9 deletions.

- [ ] **Step 5: commit.**

```sh
git add qualification/iii/run.mjs tests/foundation/harness.test.mjs
git commit -F - <<'MSG'
fix(qualify): give the iii receipt the shared header.

Why: the iii orchestrator built its own commit_sha/git_sha/inputs block
(run.mjs:802-808). verification.json: "Decision stands; provenance guard will be
added before any re-run."
What changed: the receipt starts with receiptHeader's { git_sha, inputs,
produced_at } and --record copies it to qualification/receipts/iii.json. The
REJECTED_WITH_FALLBACK decision logic and exit code 1 are untouched.
Verified: bun test ./tests/foundation/harness.test.mjs -> 35 pass, 0 fail;
bun build --no-bundle qualification/iii/run.mjs -> exit 0.
Next: package B is ready for independent verification (Package B acceptance).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01LoZr4ME3crLYjcTNn5GcAC
MSG
```

---

### Package B acceptance

**A. Re-run by the verifying agent — no model assets, no network.** From `D:\okf\cure\harness`, tree clean, branch `cure/harness`.

| # | Shell | Command | Expected |
| --- | --- | --- | --- |
| 1 | any | `git diff --name-only 678f919 HEAD` | Only paths from "Files allowed". No `scripts/dev.mjs`, manifest, lockfile, `tests/fixtures/views/**` or fixture document other than `SOURCES.json`. |
| 2 | any | `git diff 678f919 HEAD --stat -- tests/fixtures/documents` | Exactly one file: `SOURCES.json` (2 insertions, 1 deletion). |
| 3 | any | `bun test ./tests/foundation/harness.test.mjs` | `35 pass`, `0 fail`. |
| 4 | any | `bun scripts/dev.mjs check-offline` | No failure in `harness.test.mjs`. On a branch without package A, exactly the two known failures in `policy.test.mjs` (operation count) and `vendor.test.mjs` (lookup count) remain. |
| 5 | PowerShell | `cargo fmt --check -p okf-qualify-docling -p okf-qualify-mcp-apps -p okf-qualify-iii` | Exit 0, no output. |
| 6 | PowerShell | `cargo clippy --locked -p okf-qualify-docling -p okf-qualify-mcp-apps --all-targets -- -D warnings` | `Finished`, no warnings, no errors. |
| 7 | PowerShell | `cargo test --locked -p okf-qualify-docling -p okf-qualify-mcp-apps` | `4 passed` and `3 passed`, `0 failed`. |
| 8 | PowerShell | `cargo build --locked --release -p okf-qualify-docling` | `Finished \`release\` profile`. |
| 9 | any | The `git grep` command in the code block below this table | No output (exit 1). |
| 10 | PowerShell | The `bun -e` hold smoke from Task B.4 Step 5 | `"exit":0`, the done marker for `sample_sheet.xlsx`, a positive `peak`; smoke receipt has `stage: "converter_status"`, `status: "Success"`, `outcome: "PASS"`. |
| 11 | any | `bun build --no-bundle qualification/docling/run.mjs --outfile .artifacts/syntax/a.js` and the same for `qualification/mcp-apps/run.mjs`, `qualification/iii/run.mjs`, `qualification/record.mjs` | Each exits 0. |
| 12 | PowerShell | `$env:OKF_MCP_APPS_PROTOCOL_ONLY='1'; bun qualification/mcp-apps/run.mjs; $LASTEXITCODE` (needs `ui/node_modules`; no network) | Exit 0; receipt as described in Task B.9 Step 5; `git status --porcelain` empty afterwards. |
| 13 | any | `bun qualification/record.mjs mcp-apps` after row 12, then `bun scripts/dev.mjs check-receipts`, then delete `qualification/receipts/mcp-apps.json` | `recorded … -> …qualification\receipts\mcp-apps.json`; `check-receipts: 1 receipt(s) valid against HEAD.`; after the delete `git status --porcelain` is empty. This proves the header shape against the real checker. A protocol-only receipt is never committed. |

Row 9 command:

```sh
git grep -nE '#\s*!?\[\s*(allow|expect)\s*\(|\.unwrap\(\)|\.expect\(|panic!|unreachable!' -- qualification/docling/src qualification/mcp-apps/src
```

Removal checks the verifier may repeat (each must turn the named test red, then be reverted): delete `child.on('close', …)` in `runner.mjs` → "an instant-exit converter process resolves promptly"; change `mergePeak` to `return next` → "a null sample never overwrites a number"; drop `--untracked-files=all` in `provenance.mjs` → "requireCleanTree sees untracked files even when user configuration hides them"; remove `/T` (Windows) or use `proc.child.kill()` (POSIX) in `process.mjs` → "killProcessTree stops grandchildren"; make `basicHostVerdict` return `passed` unconditionally → "the run fails when one view fails".

**B. Run later by the orchestrator for real requalification (phase 3, on commit S).** All from PowerShell at the main checkout, tree clean (`git status --porcelain` empty), cargo on PATH.

Prerequisites, checked before starting:
- **Docling:** model assets downloaded and `.artifacts/qualification/docling/assets.json` present with `DOCLING_RS_MODELS_DIR` and `recommended_env` pointing at them (the file is ignored and machine-local; the existing one names `C:\Users\eayou\.cache\okf-jawn\docling\models`). Package A has restored the `docling-core` pin, so the receipt's `converter.docling_core_versions` reads `["1.93.6"]`.
- **MCP Apps:** `bun scripts/dev.mjs bootstrap` done; Playwright Chromium installed for the pinned `@playwright/test` 1.63.0 (`bun x --cwd ui playwright install chromium`; the last recorded attempt failed with "Executable doesn't exist … chromium_headless_shell-1243"); network access to `raw.githubusercontent.com` and the npm registry unless `.artifacts/qualification/mcp-apps/basic-host/` is already populated; ports 8080, 8081 and 18765 free. Optional: `ngrok` on PATH with `OKF_MCP_APPS_NGROK=1`.
- **iii:** WSL with a Linux distribution; network access to `github.com` releases; `CARGO_TARGET_DIR` unset or the build output under `<root>\target` (see Deviations, item 8).

Commands, in this order, without `--record` (a recorded receipt is an untracked file and the next harness needs a clean tree):

```powershell
bun qualification/docling/run.mjs      # expect exit 0 and result PASS, or exit 1 with a recorded finding
bun qualification/mcp-apps/run.mjs     # expect exit 0 and result PASS
bun qualification/iii/run.mjs          # expect exit 1 and decision REJECTED_WITH_FALLBACK
bun qualification/record.mjs docling mcp-apps iii
bun scripts/dev.mjs check-receipts     # expect: check-receipts: 3 receipt(s) valid against HEAD.
```

What an independent reader checks in each receipt before commit R:
- **docling:** all three header fields at the top level with `git_sha` = S; `summary` has 12 keys; every `memory_summary` value is `PASS` and every `peak_rss_bytes` is a positive integer; for `must_fail_truncated.pdf` the `stage` is `converter_error` (or `converter_status` with `Failure`) **and** `errors[0].error_message` names a PDF read/parse failure, not a missing model or library; `timeout_case.status` is `PartialSuccess` with a `pipeline` error containing "timeout". If the must-fail fixture was accepted, `result` is `FAIL`, `finding` is "converter accepts truncated PDF", the gate stays non-terminal and the owner decides (design §8); the fixture is not changed.
- **mcp-apps:** `basic_host.views` has four entries, each `status: "passed"`, `app_frame_depth: 2`, `axe.app_frame_analysed: true`, `axe.app_frame: []`, `axe.host_blocking: []`; `render_present` lists exactly the two expected alerts; four screenshots `basic-host-render_*.png` exist and show the views; `ngrok.status` is `not_run` or `closed`; `harness_process.exited_before_teardown` is `false`; `host_render` is not a PASS.
- **iii:** `decision: "REJECTED_WITH_FALLBACK"` with a `reason` about DLQ browse or crash tests, not about a missing binary, WSL or network.

---

### Deviations

1. **Process-group kill already existed at HEAD.** The brief says `killProcessTree` on POSIX signals only `cargo run` (~:75). At `b205c4a` `run.mjs:78-84` already calls `process.kill(-child.pid, 'SIGTERM')` and `spawnDetached` (`:97`) already sets `detached: process.platform !== 'win32'`. What was actually wrong: the kill was not awaited, never escalated to SIGKILL, fell back to `child.kill` on error, and the harness ran under `cargo run` so its stderr and exit code were behind cargo. B.7 and B.9 cure those and add a test that a grandchild dies.
2. **ngrok `session_open` never reached a receipt at HEAD.** `run.mjs:459` sets `status: 'session_open'` in memory, but the `finally` at `:615-620` overwrites it with `closed` and `closed_at` before the receipt is built. The remaining defects were that the close was not awaited and nothing prevented an open status from being written. B.8 `ngrokRecord` throws on an opened-but-not-closed tunnel.
3. **Line numbers in the brief are a few lines off** for `qualification/mcp-apps`: allowed-hosts check is `main.rs:447` (not ~445); the axe `disableRules` call is `run.mjs:396-398` (not ~381); `session_open` is `run.mjs:459` (not ~440). The plan uses the real numbers.
4. **The present view cannot render against the HEAD harness, so a fifth tool was added.** `PresentView.tsx:117-129` calls host tool `show` for each resolved binding; the harness answered only `render_*` (`main.rs:220-226` → `invalid_params "unknown tool"`), so the App shows an error alert. Asserting that alert as "distinguishing text" would be a fake pass. B.6 adds an app-only, read-only `show` fixture tool; `tools/list` now returns 5 tools (4 render + 1 app-only) and the protocol check asserts exactly that split.
5. **The present view's DataTable and Chart are not exercised.** `tests/fixtures/views/present-response.json` has no `materialized` dataset and no `read_object` tool exists in the harness; that fixture is outside package B. The view therefore shows two honest alerts ("Dataset unavailable: metrics", "Resolved chart data or specification unavailable."), which the plan pins as the only alerts allowed and records as `basic_host.present_dataset: "not_exercised: …"`. Chart rendering under the sandbox CSP stays unproven by this gate; a fixture with a materialized dataset plus a `read_object` fixture tool would be needed, and the owner of `tests/fixtures/views` decides.
6. **`--record` cannot be used on all three harnesses in sequence.** `requireCleanTree` stays absolute (any porcelain output fails), and `qualification/receipts/<name>.json` written by one harness makes the tree dirty for the next. Rather than add an exception to the guard, B.1 adds `qualification/record.mjs`, which records finished receipts after all three ran and refuses a receipt whose `git_sha` is not HEAD. `--record` on each `run.mjs` is implemented as briefed and works for a single harness.
7. **`scripts/dev.mjs qualify <name>` does not forward `--record`** (`dev.mjs:249-253` passes no arguments) and package B may not edit that file. Acceptance uses `bun qualification/<name>/run.mjs` directly. If the orchestrator wants `qualify <name> --record`, package A must forward `args.slice(1)`.
8. **`qualification/iii/run.mjs:385` hard-codes `<root>/target/release`.** The brief limits iii to "the shared receipt header only", so this is untouched. If Rust build output is pointed at `D:` through `CARGO_TARGET_DIR`, `buildWorker` throws "missing worker binary" and the receipt says `REJECTED_WITH_FALLBACK` for the wrong reason. Docling and MCP Apps resolve the directory through `cargo metadata` (B.2). Either run iii with the default target directory or allow a one-line change to use `buildRelease`.
9. **`cargo fmt --all --check` is not this package's gate.** The design (§1) records `cargo fmt --check` failing in 11 files at HEAD, most outside package B. The plan uses `cargo fmt --check -p okf-qualify-docling -p okf-qualify-mcp-apps -p okf-qualify-iii`; `--all` goes green only after packages A, C and E merge.
10. **`ConversionStatus::Failure` is unreachable in docling 1.93.5** (declared at `result.rs:13`, never constructed; `converter.rs:1218-1222` yields only `Success` or `PartialSuccess`). The brief's "`converter_status` with Failure" branch is kept because the enum allows it, but the only PASS the must-fail fixture can actually produce is `converter_error`.
11. **A must-fail PASS is additionally conditioned on another PDF converting in the same run** (`FAIL_refusal_unproven` otherwise). Not in the brief. Reason: `ml_pipeline()` loads models before reading the file, so with missing assets every PDF errors and the truncated fixture would "pass" for the wrong reason.
12. **The single-process all-fixtures mode of the Docling harness is removed** (`run_all`, `convert_catalog`, old `main.rs:485-584`). It could not provide per-fixture peak memory, which the gate requires, and keeping it meant maintaining a second judgement path. `OKF_DOCLING_ONLY` is now required.
13. **Converter version comes from the environment, not a build-time constant.** Docling exposes no version constant; a `build.rs` reading `Cargo.lock` was rejected because it adds lint surface to a file that could not be compiled while planning. `run.mjs` reads the `docling` entry of `Cargo.lock` and passes `OKF_DOCLING_CRATE_VERSION`; the receipt also records the lock checksum and the `docling-core` version(s). Until package A restores the pin, `Cargo.lock` at HEAD has `docling-core 1.96.1`, and the receipt would say so.
14. **Rust test idiom is declared locally.** `tests/support/check.rs` (`TestResult`, `some`) is produced by package E in wave 2 and cannot be included from the qualification crates in wave 1. The two test modules declare `type TestResult` and (Docling) `fn some` with the shared signatures. One MCP Apps test (`only_the_value_one_lifts_host_protection`) returns `()` because it has nothing fallible and a function that can only return `Ok` would trip `clippy::unnecessary_wraps`; it uses no forbidden construct.
15. **Receipt labels and inputs changed beyond the header.** The MCP Apps receipt's `component`/`gate` change from `mcp-apps-web-hosts` (the acceptance gate, `not_run`) to `mcp-apps-protocol-qualification` (the Phase 0 gate id in `verification.json`). `inputs` gain `qualification/lib` (both) and `tests/fixtures/views` (MCP Apps), which the harnesses now read. Not listed, although bundled into the App: `ui/src/features/**`, `ui/src/api/generated/**`, `ui/src/lib/wire.ts`, `ui/src/styles.css`, `ui/package.json`, `bun.lock`. Listing them would make `check-receipts` fail on every later UI change; leaving them out under-declares what the rendered views depend on. That trade is the integration owner's call.
16. **Two tests characterise existing behaviour rather than failing before a change** (`requireCleanTree` clean/dirty and non-zero git exit in B.1). The brief asks to "confirm … add tests"; they go red if the rule is removed and are marked as such in the task. Two more tests (B.9, B.10) assert orchestrator wiring by reading source text, because the orchestrators execute on import and cannot be loaded offline; the rules they wire are covered by behavioural unit tests.
17. **Nothing in this plan's Rust was compiled** (cargo was out of bounds for the planner). The JS runner and process helpers were executed inline under Bun 1.4.2 on this machine with the results quoted in the facts section; the provenance functions and both `main.rs` files were not executed. Clippy pedantic findings in B.4 or B.6 are possible and are fixed in code, never suppressed.
18. **`OKF_MCP_APPS_NGROK=true` no longer enables the tunnel** in `run.mjs` (HEAD accepted `1` or `true` at `:31-32`); it now matches the Rust rule of exactly `1`.
