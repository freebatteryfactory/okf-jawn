# Work in okf-jawn

Read README.md, SPEC.md and verification.json first. The delivered foundation is not a completed application and has not been Rust-compiled in its creation environment. Do not inherit a claim of green from file presence.

## Shared map

`contract` owns wire types and the canonical operation table. `core` owns application/IO ports and authorization semantics. `storage` implements persistence. `ingest` implements bounded conversion and recovery. `server` binds authenticated HTTP and static assets. `mcp` binds the external protocol and Apps. `cli` is an API client. `ui` composes the human workspace and reusable view components. `xtask` generates interfaces without runtime implementations.

The full scope is in SPEC.md. No built-in chat/model loop; no Cloudflare, desktop wrapper, S3 deployment, Forgejo or homegrown parser is smuggled back in. Do not substitute smaller demo functionality for the human UI or persistent visuals.

## First commands

```sh
node scripts/dev.mjs doctor
node scripts/dev.mjs check-offline
node scripts/dev.mjs vendor <library-or-concept>
```

Then, on the selected toolchain and an internet-capable machine, `bootstrap`, `gen-check`, `foundation`. Read failures. No fake lockfiles or generated-looking artifacts. Vendor notes include three executed Context7 lookups and other clearly marked references. Use the exact installed API, not memory of a similar version.

## Construction

Implement the complete assigned responsibility using real selected libraries and generated interfaces. Builders may run targeted checks and add unit/regression tests. They must not alter protected acceptance, shared contracts, dependencies, generator inputs or outputs without the integration owner's change.

Classify a failure before repair: your defect, unfinished neighboring work, or a wrong shared assumption. Fix your defect; do not invent a production substitute for a missing neighbor. Request a shared change rather than layering an adapter around a mistaken contract. Three attempts without a new diagnosis require a handoff, not more speculative edits.

Do not suppress lints, fake success, discard unsupported data, alter expected output to match implementation, or declare a partial test to be the full suite. Honest limitation comments are encouraged. Refactors for elegance and speculative optimizations wait until connected behavior exists.

The integration owner holds Cargo.toml/Cargo.lock, package manifests/lockfiles, contract, xtask, scripts, api, generated outputs, deployment and independent acceptance. Review routing in CODEOWNERS is not local filesystem enforcement. Never independently rewrite another lane's worktree.

## Conventions

Use `//!` purpose and non-obvious invariant headers. Imports, coherent types/traits, constants, implementations, functions/helpers, tests. Use canonical library terms, resolved Revision versus At selectors, and fallible APIs. No authored unsafe, allow, expect or conditional lint suppression. Do not force unrelated types into contract merely to avoid duplication.

Generated directories are `api/`, `generated/cli/`, `ui/src/api/generated/`, the router-generated tree and frontend build outputs. Change their authored inputs and run the generator. Source-policy AST checks and actual strict Clippy remain separate from dependency-free smoke checks.

## Handoff

State implemented responsibilities, genuinely connected journeys, remaining gaps, exact commands/results and the next required action. Store logs as build artifacts rather than growing permanent prose registries. Completion, blockage and budget exhaustion are different outcomes. Final acceptance is an independent check of the connected application, not the builder's summary.
