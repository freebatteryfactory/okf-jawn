# Integration owner — xtask

Read root AGENTS.md and SPEC.md.

**Directories:** `xtask/`

**Gate:** `cargo test -p xtask` and `bun scripts/dev.mjs gen-check`

**Receipt:** API/CLI/UI generation deterministic; source-policy covers `crates/`, `xtask/src/`, and `qualification/`

Use the actual vendor generators. Keep generation independent, deterministic, and failure-transparent. Generated file deletion must be detected too. No handwritten Hey API substitutes. Schema tests compare semantics; snapshots only identify drift.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
