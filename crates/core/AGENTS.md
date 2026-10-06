# Core / CLI lane — core

Read root AGENTS.md and SPEC.md.

**Directories:** `crates/core/`

**Gate:** `cargo test -p okf-jawn-core`

**Receipt:** authorization and dispatch tests pass (grants, targets, mutation ledger); ApplicationService modules implement declared operations without fabricating success. Confirmation single-use is the storage lane's `confirmation-single-use` gate, not part of this receipt.

Implement `ports::Application` for `application::ApplicationService` in cohesive child modules of `crates/core/src/application/`, using only the injected `Ports`. Core never depends on the storage, ingest, server, mcp or cli crates. Capability checks and exact revision semantics remain shared. Dispatch authorizes every `RequestScope` target, builds `OperationContext`, and runs `MutationStore::begin` before handlers. Coordinate SearchIndex updates and rebuilds with content changes; rebuild never touches RecordStore. Connector operations are available to the local owner only. Add production behavior, not an in-memory stand-in. Coordinate CLI ergonomics with the complete operation contract.

OKF conformance and lint are application policy owned here. Call `okf_validator::validate_bundle` and `okf_validator::lint_bundle` at the appropriate boundaries (import, edit, refactor, proposal) and surface diagnostics through Attention and health. Do not duplicate that policy in storage or ingest. Use `okf-core` types only through the validator unless a core port or application type must name them directly.

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
