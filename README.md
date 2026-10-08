# okf-jawn

A document workspace for people and their AI tools.

## What this delivery is

This repository contains the **full-scope Phase 0 foundation source**: typed contracts for the intended application, real generator integrations, repository/task setup, shared application, storage and search ports, HTTP/CLI adapters, MCP result adapters, reusable presentation source, tests, and an offline vendor-reference index.

**It is not the finished application, and Phase 0 is qualified only when the committed qualification receipts say so.** Nobody types a result into `verification.json`: `bun qualification/record.mjs` writes the status of each receipt-backed gate and `phase_0_qualified` from the receipts under `qualification/receipts/`, `bun scripts/dev.mjs check-receipts` verifies them, and the gates that CI enforces on every push (real generation and the consumer checks among them) carry no status. The archive was first assembled in an environment without Rust, network access or the selected JavaScript tooling; that record is kept in `verification.json` as history, not as current readiness. No lockfile, generated OpenAPI, generated client or bundled MCP App is ever handwritten.

The application is Rust: the server, application core, storage, ingestion, MCP and CLI. The browser workspace is TypeScript/React built with Vite. **Bun** is the JavaScript package manager and tooling runtime for the workspace, scripts, frontend builds and offline tests; it is not an application server. Node and pnpm are not project prerequisites.

Runtime adapter dependencies are declared behind explicit `runtime` features so lockfile resolution sees the intended graph without making the generator link the heavy implementations. These feature declarations do not claim runtime behavior exists.

The complete intended product remains specified in [SPEC.md](SPEC.md). The production Application (`crates/core/src/application/`), storage and ingestion implementations, local-owner and WorkOS session middleware, raw transport bindings, server startup, the actual MCP server, the complete Explorer, and external qualification remain construction work. The source contains **no success-returning product stand-ins** for those responsibilities.

This is self-contained **source and handoff context**, not a vendored offline distribution. First dependency resolution needs network access. Subsequent runs use frozen Cargo and Bun resolution. WorkOS configuration and native converter/model assets are separate deployment requirements, never hidden credentials in the repository.

## Start here

Read `SPEC.md` for product meaning, `AGENTS.md` for construction rules, and `verification.json` for what each gate covers and what closes it: a committed receipt, a CI job, or a recorded decision. An agent does not need this conversation.

```sh
bun scripts/dev.mjs init
bun scripts/dev.mjs doctor
bun scripts/dev.mjs check-offline
bun scripts/dev.mjs vendor schemars
```

`init` creates a local Git repository, copies the deployment environment example only when absent, and points `core.hooksPath` at the tracked `scripts/hooks/`. It does not create a remote, set a Git identity, commit, connect WorkOS, or launch a server. It never overwrites an existing `.env`.

The toolchain baseline is Rust 1.99.0 (`rust-toolchain.toml`) and Bun 1.4.2 (`package.json` `packageManager` and `.bun-version`, which must agree). `doctor` reports the Bun version actually executing it. These are selected inputs, not an assertion that every package resolves and compiles together.

```sh
bun scripts/dev.mjs lock        # only when deliberately changing dependencies
bun scripts/dev.mjs bootstrap
bun scripts/dev.mjs gen-check
bun scripts/dev.mjs foundation
```

Equivalent `just` recipes are provided. `lock` is the only task that resolves dependencies: it runs `cargo update --workspace`, which rewrites only the workspace members' own entries and keeps every version already locked, and `bun install --lockfile-only`. To move one locked crate on purpose, run `cargo update <crate> --precise <version>` and review the diff. `bootstrap` refuses to run without both `Cargo.lock` and `bun.lock`, fetches with `--locked`/`--frozen-lockfile`, builds the actual Rust generator, executes both generation passes, and type checks and builds the real UI consumer. Install scripts run only for packages in `trustedDependencies` (currently empty: no dependency lifecycle scripts are trusted). Generators run from installed, pinned packages through `bun --bun run`; nothing is downloaded on demand. A resolution or API incompatibility fails visibly. Do not guess a replacement version, handwrite the expected generated files, disable a lint, or claim a failed step passed.

`gen` generates into two temporary directories, compares the entire output file sets and bytes, then publishes only generated directories. `gen-check` compares those results to the checkout without editing it. The generator does not require a database, converter, identity provider, or running application. Generator output is not an implementation-status claim.

`bootstrap` and `init` install the tracked hooks through `core.hooksPath`; nothing else needs installing. pre-commit runs `check-offline --fast` (the tests that create no repository and spawn no process, under a second). pre-push runs the full `check-offline`, `cargo fmt --all --check` and, for each pushed `build/*` ref (read from the refs git passes on stdin, at the commit being pushed), the scope check, and for every pushed ref the receipt check (`check-receipts --head <pushed sha>`): a hand-edited qualification receipt, or a typed gate status the receipts at that commit do not derive (a status still typed over a stale receipt is one), blocks a push to `main` or `integration/*` and is a `warning:` on any other ref, naming the harness to re-run or the command that rewrites the status. Hooks never regenerate files or touch another worktree.

## What gets generated

The authored source of operation meaning is `crates/contract/src/operations.rs`, together with the request/response types. The operation macro is consumed by metadata, the application trait, HTTP routing, CLI commands, and the Rust generator. It is a concrete Rust declaration table, not another workflow language.

| Output | Actual producer |
| --- | --- |
| `api/openapi.json` and `.yaml` | Schemars component schemas (the schema authority) and the typed operation declarations, assembled as plain JSON from them |
| `api/operations.json`, `transports.json` | Serde serialization of the same declarations |
| Per-operation input/output JSON Schemas | Schemars, explicit deserialize/serialize contracts |
| `api/forms/*.schema.json` | Schemars Draft 7 for RJSF's default AJV adapter |
| `api/mcp-tools.json` | Operation metadata plus generated input/output schemas |
| `api/examples/*.json` | Typed synthetic Rust values, not implementation-generated expected answers |
| `api/presentation/*` | json-render's actual catalog schema and prompt APIs |
| `ui/src/api/generated/*` | Hey API Fetch SDK, types, Zod, TanStack Query options |
| `generated/cli/*` | clap_complete and clap_mangen |
| `generated/converter/packages.json` | xtask, from the docling crates Cargo.lock pins |
| `ui/src/routeTree.gen.ts` | TanStack Router build plugin |
| `ui/dist-apps/*.html` | Actual frontend bundling of shared feature components |
| `ui/dist/docs/*` | Locally copied swagger-ui-dist assets and the canonical `api/openapi.yaml` |

There are 77 typed JSON application commands and 16 distinct transport declarations. Twelve tools are model-facing: `ls`, `grep`, `show`, `log`, `diff`, `blame`, `sources`, `links`, `propose`, `catalog`, `present` and `workspaces` (which lists the workspaces a connection may see, so an agent can pin its reads to the returned head revision); the scoped binary-read tool is app-only. Human approval, verification and connector management are never model tools. The raw transport declarations are documented schema obligations, not bound handlers yet.

A candidate `present` operation selects or composes approved views. Source components receive source bindings, not model-authored replacement evidence. The frontend includes source excerpts, Changes, Timeline, naming forms, a constrained composition catalog, and chart/table rendering source. The rest of the full UI is assigned in `SPEC.md`, not replaced by demo data.

## Authentication

Two explicit entry paths, chosen by `OKF_AUTH_MODE`, produce the same internal principal and pass through the same authorization rules (SPEC section 11). **Local** mode has one persistent installation identity that owns the local workspaces, with no WorkOS and no passwords; the browser gets a local session protected by Host/Origin checks, CSRF and strict cookies, and loopback alone grants nothing. Local MCP clients use separate, revocable connector credentials scoped to read (and propose only when enabled); they never review or approve, and never receive the owner's session. **Hosted** mode uses WorkOS AuthKit and Connect with GalaxyGate; missing or invalid hosted configuration fails startup and never falls back to local mode. The default local endpoint is `http://127.0.0.1:7711`.

## Vendor documentation without conversation recall

```sh
bun scripts/dev.mjs vendor schemars
bun scripts/dev.mjs vendor mcp
bun scripts/dev.mjs vendor bun
```

`vendors.json` records existing use sites, planned use sites, generated sites, official documentation locations, concrete symbols, short offline notes, the `verification.json` gates that record what has been proved about the library (its status is read there, never typed here), and, for each entry that records an executed Context7 lookup, its library ID and query (`context7_queries_executed` equals the number of such entries). Other entries are explicitly reference pointers rather than invented Context7 research receipts. Follow the installed package's exact version when a latest-page example disagrees. After packages are fetched, Cargo and installed package sources provide local source documentation too.

## Constructing the whole product

After the foundation is resolved and qualified, commit it and create isolated worktrees:

```sh
bun scripts/dev.mjs lanes            # all seven; or name them: lanes storage
bun scripts/dev.mjs lane storage     # that lane's gate
bun scripts/dev.mjs premerge         # the CI sequence, every step, every feature
```

`lanes` creates one worktree and `build/<lane>` branch per lane from the same clean commit, under `OKF_LANES_DIR` when set, else `D:\okf\lanes` on Windows when `D:` exists, else `../okf-jawn-lanes`. `scripts/lib/lanes.mjs` is the only lane table: worktrees, gates, the scope check, the AGENTS.md table and the CODEOWNERS lane rows are generated from it or tested against it. `lane <name>` runs that lane's fmt, Clippy, tests, source policy and scope check (Biome, route generation through `bun scripts/dev.mjs routes`, `tsc` and filtered Vitest for the UI lanes), writes `.artifacts/lane/<name>/<sha>.log` and ends with `PASS <name> <sha>` or `FAIL <name> <sha> <step>`; the sha carries `-dirty` when the tree had uncommitted changes. `premerge` is what CI runs; it never stops at the first failure. `lanes-reset` removes only clean lanes that hold no commits of their own, and never forces. None of this starts paid agents, spends API credits, or grants repository privileges. Each lane's files contain short local instructions. The integration owner controls contracts, manifests, lockfiles, generated outputs, deployment and independent acceptance. `CODEOWNERS` routes review on GitHub; it is not a local lock. A ruleset on `main` requires the two always-running CI jobs and blocks force-push and deletion; squash and rebase merging are disabled, so history reaches `main` only through merge commits. No required review exists until real owners replace the placeholder handles. Builders may test their work but may not redefine it from whichever unrelated check is red.

The intended release is the complete product in `SPEC.md`, not a succession of cut-down demos. Lanes work on full responsibilities in parallel rather than storage first and UI later; assemble them while context is fresh, then accept connected user journeys. Do not spend construction time on speculative optimization or adapters hiding a wrong shared assumption.

## Verification and limits

`check-offline` runs the dependency-free tooling tests with `bun test` (`check-offline --fast` only those that create no repository and spawn no process, as pre-commit does): positive/negative HTTP harness controls and static contract/policy/toolchain consistency checks. It is not Rust compilation, TypeScript 7 type checking (`tsc -b` in `ui/`, which imports the route tree that `bun scripts/dev.mjs routes` writes; the Hey API client runtime is a separate project in `ui/tsconfig.generated.json` because of hey-api/openapi-ts#3157, and `typecheck:generated-strict` reports when that exception can go), Schemars/Hey API execution, full semantic schema validation, WorkOS qualification, conversion, or product acceptance. `foundation` requests the broader checks and fails if their prerequisites are missing.

Each Phase 0 gate in `verification.json` has one kind. A `receipt` gate names the committed receipt that closes it and its harness's tracked `criteria.json`; its status is derived, never typed: `bun qualification/record.mjs <name>...` is the only way a receipt is recorded (a harness `run.mjs` takes no argument and records nothing): it copies finished receipts of any result into `qualification/receipts/` (a failed or unfinished qualification is evidence, and its gate then says `failed` or `incomplete`), refuses only a receipt whose header or envelope cannot be trusted, and writes each such status and `phase_0_qualified` from them, and with no name only rewrites those values. A receipt gate may list `accepted_failures`: failing criteria the owner has decided to accept, each with the decision, its date and the gate that tracks its cure. A FAIL receipt whose every failing required criterion is listed, from a run that judged everything else (no harness error, no required criterion left unjudged), derives `accepted_with_limitations`, which qualifies Phase 0 as `passed` does; a run that was cut short derives `incomplete` however its one failure is accepted, an INCOMPLETE receipt is never upgraded, and an acceptance naming a criterion its receipt no longer fails is itself a `check-receipts` failure until the entry is removed. A `ci` gate names the workflow job and step that enforce it on every push and carries no status. A `decision` gate records a choice, its date and the role that made it. `bun scripts/dev.mjs check-receipts` fails when a typed value differs from the derived one, when a receipt is not a JSON object with the shared header or cites a commit that is not an ancestor, when its result is not the fold of its criteria, when a criterion its harness pins is missing or not required, and when a receipt is for another gate or no gate names it. A receipt that fails any of these checks, or one of whose inputs changed after the commit it cites, is trusted for nothing, whatever result it types: its gate derives `incomplete`, in `check-receipts`, in its `--head` form and in `record.mjs` alike. So when an input of a recorded receipt changes, `check-receipts` fails until the harness is re-run and recorded, or `bun qualification/record.mjs` is run to write the gate back to `incomplete`; the stale receipt then stays in the tree and qualifies nothing. The limit of all of this: receipts are produced on a developer machine and are not signed, so a consistent hand edit of a receipt is not detected by `check-receipts`. A criterion flipped to `pass`, another commit typed into `git_sha` or a shortened `inputs` list passes it, and what catches such an edit today is a reviewer reading the diff. Producing the receipts in CI, where a contributor cannot edit them, is the planned cure. `bun scripts/dev.mjs clean-checkout` reruns the whole foundation in a detached worktree of HEAD and requires that tree to stay clean; it is a local tool, not a gate, because CI starts from a fresh clone on every run.

`bun scripts/dev.mjs qualify mcp-wire` checks a supplied running MCP endpoint. It does not certify that a host rendered the iframe. `qualify application` exercises a disposable test deployment; it refuses to run without explicit credentials and a test-environment opt-in. No secret or external endpoint is supplied by default.

Jobs run on an in-process Tokio worker over durable `RecordStore`. Acceptance is persisted with `create_job` before the job is enqueued, a job runs only under the lease `claim_job` grants, and handlers are idempotent on the job's `MutationId`; unfinished work is reconciled through `expire_leases` and `pending_jobs`. Queue delivery is only a wake-up, and the product is not claimed crash-durable until the construction check `job-crash-against-record-store` passes against the real RecordStore and import path.

Our source is dual licensed under MIT OR Apache-2.0. Dependencies retain their own licenses. This repository is not a license/security/compliance certification.
