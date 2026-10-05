---
name: Dependency qualification pass
overview: Feature-slice OKF and Docling to the narrowest official seams, replace Scalar with swagger-ui-dist for /docs, drop the direct esbuild and its install-script trust, align the MCP TS SDK family, scope Bun/Node ambient types per TS project, fix fixable JS advisories, and add exact-ID audit gates with product vs tooling findings labeled separately.
todos:
  - id: okf
    content: "Replace umbrella okf: okf-core in storage and ingest; okf-validator (default-features=false) in core only; record ownership in the AGENTS files and vendors.json"
    status: completed
  - id: docling
    content: Upgrade to newest published matching docling version with default-features=false, features=[pdf]; drop direct docling-core
    status: completed
  - id: cargo-audit
    content: cargo update, review lock diff; .cargo/audit.toml with deny warnings; exact-ID ignores only if the new graph still reports the font-stack pair
    status: completed
  - id: swagger
    content: Replace Scalar with swagger-ui-dist reading canonical api/openapi.yaml; root scarfSettings disabled; no CDN
    status: completed
  - id: esbuild
    content: "Move generate-catalog.mjs to Vite build() with fixed unhashed names; remove esbuild; trustedDependencies: []; bun pm untrusted recorded; update toolchain test and README"
    status: completed
  - id: ts-types
    content: Align MCP client/core 2.3.0; add @types/bun 1.4.2; tooling tsconfig with bun types; scope node types only where MCP declarations need Buffer; check Hey API version
    status: completed
  - id: js-audit
    content: bun audit fix --dry-run only; apply via exact bun add pins or targeted exact overrides; audit --prod, --audit-level=high, full
    status: completed
  - id: gates-docs
    content: Add audit task (cargo-audit 0.22.2, three bun levels, SCARF_ANALYTICS=false in CI); update vendors.json (fix okf upstream), README, verification.json current, tree
    status: completed
  - id: verify
    content: Run the full verification order and report versions, eliminated and remaining advisories with labels
    status: completed
  - id: commit
    content: Create themed self-describing commits (no push)
    status: completed
isProject: false
---

# Dependency qualification and cleanup pass

## Corrections before execution

1. **One OpenAPI artifact for docs.** Docs consume one committed, canonical OpenAPI artifact byte for byte, with no extra representation made for documentation. Both `api/openapi.json` and `api/openapi.yaml` are already written by the Rust generator and checked by `gen-check`, so neither is a new artifact. Swagger UI reads `api/openapi.yaml` (the file named in the correction).
2. **Exact pins survive audit fixes.** Keep `trustedDependencies: []` and `[install] exact = true`. Run `bun audit fix --dry-run` only to see what's available. Apply direct fixes with `bun add [-d] pkg@x.y.z --exact`, and use an `overrides` entry only for a transitive whose owning dependency can't move yet. Never run a bare `bun audit fix`, which can rewrite exact pins as ranges.
3. **cargo-audit 0.22.2, strict.** Pin `cargo-audit` 0.22.2 (`cargo install cargo-audit --version 0.22.2 --locked`) and keep warnings denied. Put `[output] deny = ["warnings"]` in `.cargo/audit.toml`. Add exact-ID ignores only after the sliced and upgraded graph still reports them.
4. **Three JS audit levels.**
   - `bun audit --prod` blocks on any shipped-runtime finding.
   - `bun audit --audit-level=high` blocks on high or critical findings across the whole graph, tooling included.
   - Full `bun audit` is recorded so lower-severity tooling findings can be triaged.
   - An unavoidable high build-tool advisory is excepted only by exact advisory, path, rationale and removal condition.
5. **The 8 authored TS errors aren't a requirement.** They're outside this pass: don't fix them opportunistically, but report the actual count afterwards. If a dependency correction legitimately removes one, keep that.
6. **Scarf.** Set `scarfSettings.enabled = false` once, in the root `package.json`, since every install runs from the root workspace. Also set `SCARF_ANALYTICS=false` in CI. Don't duplicate the setting in `ui/package.json`.
7. **No speculative Node island.** Create no MCP Apps Node-types project ahead of time. Compile the imported `ext-apps/react` surface first, and add a scoped Node-ambient project only if that demonstrably needs one.

## Final execution notes

1. **OKF placement follows responsibility, not the old umbrella's locations.**
   - `okf-core` goes into storage (bundle, link, index and refactor mechanics) and into ingest (concept and document construction).
   - `okf-validator` goes into core only. Conformance and lint are application policy: `core::application` decides when `validate_bundle` and `lint_bundle` run, covering import, edits, refactors and proposals, and turning diagnostics into Attention and health.
   - Storage enforces only storage invariants, never OKF policy. Ingest produces candidate content but doesn't define validity. Validation is never duplicated in storage or ingest.
   - Core takes `okf-core` directly only if its port or application types name OKF types; otherwise it gets it through the validator.
   - If depending on the validator from core ever creates a real cycle, stop and report the exact cycle rather than moving validation into storage.
   - This pass adds the dependency and records the ownership in [crates/core/AGENTS.md](crates/core/AGENTS.md), [crates/storage/AGENTS.md](crates/storage/AGENTS.md), [crates/ingest/AGENTS.md](crates/ingest/AGENTS.md) and `vendors.json`. It doesn't implement the validation flow itself, which is lane work.
2. **Deterministic catalog build.** The Vite catalog build uses a fixed ESM entry name (for example `entryFileNames: 'catalog.mjs'`), fixed chunk and asset names, and no content hashes. `gen-check` must show two catalog generations are byte-identical.
3. **Full `bun audit` is informational for low-severity tooling.** The audit task captures and records its exit code and output, but a lower-severity tooling finding alone never fails the task. `bun audit --prod` and `bun audit --audit-level=high` still block. No `&&` chain may turn the full audit into a third gate.
4. **Observable trust list.** After setting `"trustedDependencies": []` and reinstalling, run `bun pm untrusted` and record its output.
   - Nothing it reports is trusted automatically.
   - An exact package is added only if its lifecycle script is required for a feature we use, and only after the script has been inspected.
   - Everything else stays blocked.

Policy applied throughout: newest compatible direct release, narrowest useful features, existing library API before our own code, runtime and tooling findings labeled separately, exceptions only by exact advisory ID with a reason and a removal condition. No architecture changes made only to improve audit counts, no hand-edited generated output, and no `--latest` sweeps.

## Facts established (read-only)

- The installed `okf` 0.2.7 is `https://github.com/W4G1/okf`. Its default features are `validator, studio, python, javascript, rust, sql`, and `okf-validator`'s defaults are the four parsers (`python = ["dep:rustpython-parser"]`).
- `okf`, `docling` and `docling-core` are declared only as optional dependencies in [crates/ingest/Cargo.toml](crates/ingest/Cargo.toml) and [crates/storage/Cargo.toml](crates/storage/Cargo.toml). No source file imports them, so the direct `docling-core` dependency can simply be removed.
- [vendors.json](vendors.json) lists the wrong upstream for `okf` (`GoogleCloudPlatform/open-knowledge-format`), which is documentation drift.
- [tests/foundation/toolchain.test.mjs](tests/foundation/toolchain.test.mjs) line 42 asserts `trustedDependencies == ['esbuild']`.
- The only `@modelcontextprotocol` import in UI source is `@modelcontextprotocol/ext-apps/react` in `ui/src/mcp-apps/main.tsx`. `client` and `core` are present to satisfy ext-apps' peer dependencies.
- The server does not serve `/docs` yet; [ui/scripts/bundle-docs.mjs](ui/scripts/bundle-docs.mjs) only builds `ui/dist/docs`.

## 1. Rust: OKF and Docling

- In [Cargo.toml](Cargo.toml), replace `okf = "=0.2.7"` with:

```toml
okf-core = { version = "=0.2.7" }
okf-validator = { version = "=0.2.7", default-features = false }
```

- Storage and ingest: swap `dep:okf` for `dep:okf-core` only in their `runtime` features.
- Core: add `okf-validator` (plus `okf-core` directly only if core names OKF types), following final execution note 1.
- Before writing the vendor note, confirm in the installed crates' docs that `Bundle::load`, `validate_bundle`, `lint_bundle` and the link/index APIs exist.
- Docling:
  - Find the newest published pair in the registry (`cargo search docling`, `cargo info docling@<v>`). Use 1.93.6 if it's published, otherwise the newest version both crates share.
  - Set `docling = { version = "=<v>", default-features = false, features = ["pdf"] }`.
  - Delete the workspace `docling-core` entry and its pin comment, and remove `docling-core` from ingest.
  - Read the chosen version's `[features]` to confirm `pdf` and the formats that need no feature before accepting.
- Run `cargo update` once, review the `Cargo.lock` diff, then `cargo audit`.
  - Expected: the 7 `paste` and `unic-*` advisories disappear.
  - Create [.cargo/audit.toml](.cargo/audit.toml) with `[output] deny = ["warnings"]`.
  - If, after the upgrade, only `rustybuzz` (RUSTSEC-2026-0206) and `ttf-parser` (RUSTSEC-2026-0192) remain with no CVE, add `[advisories] ignore` for exactly those reported IDs. Beside each, comment the path (docling, resvg 0.47, usvg), the reason (upstream MSRV pin) and the removal condition (docling moves to resvg 0.48 or later).
- Leave duplicate transitive versions (`syn`, `getrandom`, `base64`, `windows-sys`) alone.

## 2. API docs: Scalar out, swagger-ui-dist in

- In [ui/package.json](ui/package.json), remove `@scalar/api-reference` and add `swagger-ui-dist` at the current stable version (5.33.0 if still latest), with `--exact`. Add `"scarfSettings": { "enabled": false }` to the root [package.json](package.json) only.
- Rewrite [ui/scripts/bundle-docs.mjs](ui/scripts/bundle-docs.mjs):
  - Locate the installed package the same way it does now.
  - Copy `swagger-ui-bundle.js`, `swagger-ui-standalone-preset.js` and `swagger-ui.css` into `dist/docs`.
  - Place the canonical `api/openapi.yaml` into the build output unchanged; no other OpenAPI copy is created.
  - Write `index.html` and `init.js` calling `SwaggerUIBundle({ url: './openapi.yaml', dom_id: '#swagger-ui', presets: [SwaggerUIBundle.presets.apis, SwaggerUIStandalonePreset], layout: 'StandaloneLayout', validatorUrl: null })`. Try-it-out stays enabled, with no CDN.
- Leave rmcp, MCP tools, MCP Apps, WebMCP and the Explorer untouched.

## 3. Drop the direct esbuild

- Rewrite [ui/scripts/generate-catalog.mjs](ui/scripts/generate-catalog.mjs) to use Vite's `build()`:
  - `configFile: false`, `build.ssr` pointing at `scripts/catalog-entry.ts`;
  - ESM output with fixed, unhashed names (`entryFileNames: 'catalog.mjs'`, fixed chunk and asset names), `write: true` into a temporary directory, then `import()` it as now;
  - the `@` alias carried over if `catalog.ts` needs it.
- Remove `esbuild` from `ui/package.json`, and set `"trustedDependencies": []` explicitly in the root `package.json`. An empty list rather than a deleted key, because deleting it would re-enable Bun's default trusted list.
- Reinstall, then run `bun pm untrusted` and record the output. Nothing it lists is trusted unless its script is inspected and required (final execution note 4).
- Update the toolchain test to assert `[]`, and fix the README line 41 wording.
- Prove the catalog output is deterministic: `gen-check` must still pass byte for byte.

## 4. TypeScript packages and ambient types

- Align `@modelcontextprotocol/client` and `@modelcontextprotocol/core` at 2.3.0, keep `@modelcontextprotocol/ext-apps` at 2.0.3, and add no TS server.
- Add `@types/bun` 1.4.2.
- Create a composite [ui/tsconfig.tooling.json](ui/tsconfig.tooling.json) for `vite.config.ts`, `vitest.config.ts`, `playwright.config.ts`, `openapi-ts.config.ts` and `scripts/catalog-entry.ts`, with `"types": ["bun"]`.
  - Remove those files from [ui/tsconfig.json](ui/tsconfig.json), which keeps `src` and `tests` with `"types": ["vite/client", "vitest/globals"]`, and add the new project to `references`.
  - Build info goes in `.tsbuild/tooling.tsbuildinfo` and output in `.tsbuild/tooling`.
- Node types:
  - Compile the imported `ext-apps/react` surface first, without `skipLibCheck` masking. One way is a one-off `tsc --noEmit --skipLibCheck false` on a file that only imports it; don't commit that probe.
  - Only if that shows a missing `Buffer` (or another Node global), put `src/mcp-apps` in its own referenced project with `"node"` added. Otherwise create nothing.
  - Keep a direct `@types/node` only if something needs it directly, pinned exactly to the current TS 7-capable release (26.6.4).
- Hey API: check whether a release newer than 0.99.0 exists and upgrade only if it does. Keep the TS6 API package, the exact-optional exception for the generated runtime, and both removal probes.

## 5. JavaScript advisories

- Run `bun audit fix --dry-run` and read the proposed changes. Don't run a bare `bun audit fix`.
- Apply each fix deliberately:
  - **Direct dependency:** `bun add [-d] pkg@<current stable> --exact`.
  - **Transitive dependency whose owner can't move:** a root `overrides` entry pinned exactly, with a compatibility check (the owning tool still runs: `gen-check` for Hey API, `shadcn --help` for shadcn).
- Re-resolve `bun.lock` with a normal `bun install` and review the lockfile and package diff.
- Run the three levels:
  - `bun audit --prod` (runtime, blocking);
  - `bun audit --audit-level=high` (whole graph, blocking);
  - full `bun audit` (recorded).
- Keep `shadcn` dev-only at current stable (4.21.1), pinned exactly.
- A high-severity tooling advisory that can't be fixed gets an exact exception: advisory, path, rationale and removal condition.

## 6. Gates and records

- Add an `audit` task to [scripts/dev.mjs](scripts/dev.mjs) and [justfile](justfile):
  - `cargo audit` (warnings denied through `.cargo/audit.toml`), `bun audit --prod` and `bun audit --audit-level=high` each block;
  - full `bun audit` runs separately, and its exit code and output are captured without propagating, so a lower-severity tooling finding never fails the task (final execution note 3).
  - Wire it into [.github/workflows/ci.yml](.github/workflows/ci.yml): install `cargo-audit` with `cargo install cargo-audit --version 0.22.2 --locked`, and set `SCARF_ANALYTICS=false`.
- Update [vendors.json](vendors.json):
  - `okf`: correct the upstream to W4G1/okf, note the crates and features, and add the symbols.
  - `docling`: new version and features, with its own note about the resvg pin.
  - Replace `scalar` with a `swagger-ui-dist` entry.
  - `bun`: note the empty `trustedDependencies` list.
  - Add an MCP TS SDK note.
- Also update [README.md](README.md) (docs row, trusted dependencies), the `current` record in [verification.json](verification.json) (historical archive untouched), and `REPOSITORY-TREE.txt`.

## 7. Verification order

1. One deliberate resolution: `cargo update` and `bun install` without `--frozen-lockfile`, then review both lockfile diffs.
2. `cargo fmt --check`.
3. Strict Clippy, default and runtime features.
4. `cargo test --workspace`.
5. `cargo audit` (strict through `.cargo/audit.toml`).
6. `bun install --frozen-lockfile`, then `bun audit --prod`, `bun audit --audit-level=high` and `bun audit`.
7. UI `typecheck` and `lint`.
8. `bun scripts/dev.mjs gen-check`, then `foundation`.
9. `check-offline`.

The existing 8 authored TS errors are outside this pass. Don't chase them here, and report the actual count afterwards.

**Report:** the new direct versions; advisories eliminated; each remaining advisory with its path, labeled product runtime, build-time tooling or upstream-only; and failed or blocked commands.

## 8. Commit (after verification passes as far as it can)

Themed commits, each self-describing:

1. OKF feature slicing.
2. Docling upgrade and feature slicing, plus the audit exceptions.
3. Docs renderer swap.
4. esbuild removal and the empty trust list.
5. TS types and MCP alignment.
6. JS audit fixes.
7. Audit gates and records.

Nothing is pushed.