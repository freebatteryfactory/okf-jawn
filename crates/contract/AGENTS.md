# Integration owner — contract

Read root AGENTS.md and SPEC.md.

**Directories:** `crates/contract/`

**Gate:** `bun scripts/dev.mjs gen-check` and `cargo test -p okf-jawn-contract`

**Receipt:** operations/schemas regenerate cleanly; semantic tests pass.

Own wire types, semantic descriptions, source and revision identities, operation declarations and generated schema agreement. Do not depend on storage or runtime startup. Regeneration must work before the application exists.

A lane may change its own authored generator inputs, run `gen`, and commit outputs; gen-check must pass on that branch. The integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
