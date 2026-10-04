# okf-jawn

A document workspace for people and their AI tools.

## What this delivery is

This repository contains the **full-scope Phase 0 foundation source**: typed contracts for the intended application, real generator integrations, repository/task setup, shared application and storage ports, HTTP/CLI adapters, MCP result adapters, reusable presentation source, tests, and an offline vendor-reference index.

**It is not the finished application, and Phase 0 is not yet qualified.** The creation environment had Node and Git but no Rust toolchain, pnpm, Docker, package-registry DNS, or online authorized development device. Consequently the Rust generator and vendor frontend generators could not be executed. No Cargo.lock, pnpm-lock.yaml, generated OpenAPI, generated client, or bundled MCP App is fabricated in this delivery. Source inspection and executable dependency-free tooling checks are recorded in `verification.json`.

Runtime adapter dependencies are declared behind explicit `runtime` features so lockfile resolution sees the intended graph without making the generator link the heavy implementations. These feature declarations do not claim runtime behavior exists.

The complete intended product remains specified in [SPEC.md](SPEC.md). Storage and ingestion implementations, WorkOS/session middleware, raw transport bindings, an executable application server, the actual MCP server, the complete Explorer, and external qualification remain construction work. The source contains **no success-returning product stand-ins** for those responsibilities. The blank storage/ingest libraries reserve real package boundaries rather than pretend to implement them.

This is self-contained **source and handoff context**, not a vendored offline distribution. First dependency resolution needs network access. Subsequent generator runs use frozen Cargo resolution and installed frontend packages. WorkOS configuration and native converter/model assets are separate deployment requirements, never hidden credentials in the archive.

## Start here

Read `SPEC.md` for product meaning, `AGENTS.md` for construction rules, and `verification.json` for what was actually checked. An agent does not need this conversation.

```sh
node scripts/dev.mjs init
node scripts/dev.mjs doctor
node scripts/dev.mjs check-offline
node scripts/dev.mjs vendor schemars
```

`init` creates a local Git repository and copies the deployment environment example only when absent. It does not create a remote, set a Git identity, commit, connect WorkOS, or launch a server. It never overwrites an existing `.env`.

The dependency baseline is Rust 1.99.0, Node 24.21.0, and pnpm 12.8.1. These are selected inputs, not an assertion that all packages resolve together. Install the exact toolchain, then run:

```sh
node scripts/dev.mjs bootstrap
node scripts/dev.mjs gen-check
node scripts/dev.mjs foundation
```

Equivalent `just` recipes are provided. `bootstrap` resolves absent lockfiles, otherwise preserves them; fetches the selected dependencies; builds the actual Rust generator; executes both generation passes; and attempts to build the real UI consumer. A resolution or API incompatibility fails visibly. Do not guess a replacement version, handwrite the expected generated files, disable a lint, or claim a failed step passed.

`gen` generates into two temporary directories, compares the entire output file sets and bytes, then publishes only generated directories. `gen-check` compares those results to the checkout without editing it. The generator does not require a database, converter, identity provider, or running application. Generator output is not an implementation-status claim.

## What gets generated

The authored source of operation meaning is `crates/contract/src/operations.rs`, together with the request/response types. The operation macro is consumed by metadata, the application trait, HTTP routing, CLI commands, and the Rust generator. It is a concrete Rust declaration table, not another workflow language.

| Output | Actual producer |
| --- | --- |
| `api/openapi.json` and `.yaml` | Utoipa schema types and the typed operation declarations |
| `api/operations.json`, `transports.json` | Serde serialization of the same declarations |
| Per-operation input/output JSON Schemas | Schemars, explicit deserialize/serialize contracts |
| `api/forms/*.schema.json` | Schemars Draft 7 for RJSF's default AJV adapter |
| `api/mcp-tools.json` | Operation metadata plus generated input/output schemas |
| `api/examples/*.json` | Typed Rust fixture values, not implementation-generated expected answers |
| `api/presentation/*` | json-render's actual catalog schema and prompt APIs |
| `ui/src/api/generated/*` | Hey API Fetch SDK, types, Zod, TanStack Query options |
| `generated/cli/*` | clap_complete and clap_mangen |
| `ui/src/routeTree.gen.ts` | TanStack Router build plugin |
| `ui/dist-apps/*.html` | Actual frontend bundling of shared feature components |
| `ui/dist/docs/*` | Locally copied Scalar browser bundle and generated OpenAPI |

There are 62 typed JSON application commands and 13 distinct transport declarations. Eleven tools are model-facing; the scoped binary-read tool is app-only. Human approval and verification are never model tools. The raw transport declarations are documented schema obligations, not bound handlers yet.

A candidate `present` operation selects or composes approved views. Source components receive source bindings, not model-authored replacement evidence. The frontend includes source excerpts, Changes, Timeline, naming forms, a constrained composition catalog, and chart/table rendering source. The rest of the full UI is assigned in `SPEC.md`, not replaced by demo data.

## Vendor documentation without conversation recall

```sh
node scripts/dev.mjs vendor utoipa
node scripts/dev.mjs vendor mcp
node scripts/dev.mjs vendor workos
```

`vendors.json` records use sites, official documentation locations, concrete symbols, short offline notes, and the three Context7 library IDs/queries actually used in this build. Other entries are explicitly reference pointers rather than invented Context7 research receipts. Follow the installed package's exact version when a latest-page example disagrees. After packages are fetched, Cargo and installed npm sources provide local source documentation too.

## Constructing the whole product

After the foundation is resolved and qualified, commit it locally and create isolated worktrees:

```sh
node scripts/dev.mjs lanes
```

This creates seven lanes from the same clean commit. It does not start paid agents, spend API credits, or grant repository privileges. Each lane's files contain short local instructions. The integration owner controls contracts, manifests, generated outputs, deployment and independent acceptance. Builders may test their work but may not redefine it from whichever unrelated check is red.

The intended release is the complete product in `SPEC.md`, not a succession of cut-down demos. Complete responsibilities in parallel, assemble them while context is fresh, then accept connected user journeys. Do not spend construction time on speculative optimization or adapters hiding a wrong shared assumption.

## Verification and limits

`check-offline` executes the delivered Node tooling, positive/negative HTTP harness controls, and static contract/policy consistency checks. These are not Rust compilation, Utoipa/Hey API execution, full semantic schema validation, WorkOS qualification, conversion, or product acceptance. `foundation` requests the broader checks and fails if their prerequisites are missing.

`node scripts/dev.mjs qualify mcp-wire` checks a supplied running MCP endpoint. It does not certify that a host rendered the iframe. `qualify application` exercises a disposable test deployment; it refuses to run without explicit credentials and a test-environment opt-in. No secret or external endpoint is supplied by default.

The iii runtime has not been adopted. No daemon, mutable image tag, fake worker lockfile, or fallback executor is included. Its exact queue/adapter crash behavior must be qualified before adoption. The core job and conversion interfaces retain the intended behavior either way.

Our source is dual licensed under MIT OR Apache-2.0. Dependencies retain their own licenses. This archive is not a license/security/compliance certification.
