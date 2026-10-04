# Work in okf-jawn

Read README.md, SPEC.md and verification.json first. The delivered foundation is not a completed application. Do not inherit a claim of green from file presence; only the recorded results of the selected tools count.

## Shared map

`contract` owns wire types and the canonical operation table. `core` owns application/IO ports, authorization semantics and the production Application in `core::application`. `storage` implements persistence and the search index. `ingest` implements bounded conversion and recovery. `server` binds authenticated HTTP and static assets, and its startup composes the concrete adapters into the Application. `mcp` binds the external protocol and Apps. `cli` is an API client. `ui` composes the human workspace and reusable view components. `xtask` generates interfaces without runtime implementations.

The application is Rust. Bun is the JavaScript package manager and tooling runtime; Node and pnpm are not project prerequisites. The full scope is in SPEC.md. No built-in chat/model loop; no Cloudflare, desktop wrapper, S3 deployment, Forgejo or homegrown parser is smuggled back in. Do not substitute smaller demo functionality for the human UI or persistent visuals.

## First commands

```sh
bun scripts/dev.mjs doctor
bun scripts/dev.mjs check-offline
bun scripts/dev.mjs vendor <library-or-concept>
```

Then, on the selected toolchain, `bootstrap`, `gen-check`, `foundation`. These require committed `Cargo.lock` and `bun.lock`; a missing lockfile is an error. Only `lock` resolves dependencies, and only when deliberately run. Read failures. No fake lockfiles or generated-looking artifacts. Vendor notes include three executed Context7 lookups and other clearly marked references. Use the exact installed API, not memory of a similar version.

## Construction

Implement the complete assigned responsibility using real selected libraries and generated interfaces. Builders may run targeted checks and add unit/regression tests. They must not alter protected acceptance, shared contracts, dependencies, generator inputs or outputs without the integration owner's change.

Classify a failure before repair: your defect, unfinished neighboring work, or a wrong shared assumption. Fix your defect; do not invent a production substitute for a missing neighbor. Request a shared change rather than layering an adapter around a mistaken contract. Three attempts without a new diagnosis require a handoff, not more speculative edits.

Do not suppress lints, fake success, discard unsupported data, alter expected output to match implementation, or declare a partial test to be the full suite. Honest limitation comments are encouraged. Refactors for elegance and speculative optimizations wait until connected behavior exists.

The integration owner holds Cargo.toml/Cargo.lock, package.json/bun.lock and other package manifests, contract, xtask, scripts, api, generated outputs, deployment and independent acceptance. Ownership is enforced locally by isolated worktrees; CODEOWNERS is review routing, not a local editing lock, and no branch protection is configured yet. Never independently rewrite another lane's worktree.

## Conventions

Use `//!` purpose and non-obvious invariant headers. Imports, coherent types/traits, constants, implementations, functions/helpers, tests. Use canonical library terms, resolved Revision versus At selectors, and fallible APIs. No authored unsafe, allow, expect or conditional lint suppression. Do not force unrelated types into contract merely to avoid duplication.

Generated directories are `api/`, `generated/cli/`, `ui/src/api/generated/`, the router-generated tree and frontend build outputs. Change their authored inputs and run the generator. Source-policy AST checks, actual strict Clippy and TypeScript 7 type checking remain separate from dependency-free smoke checks; Bun executing or transpiling TypeScript is not type checking.

## Handoff

State implemented responsibilities, genuinely connected journeys, remaining gaps, exact commands/results and the next required action. Store logs as build artifacts rather than growing permanent prose registries. Completion, blockage and budget exhaustion are different outcomes. Final acceptance is an independent check of the connected application, not the builder's summary.
