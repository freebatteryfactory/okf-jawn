# Core / CLI lane — CLI

Read root AGENTS.md and SPEC.md.

**Directories:** `crates/cli/`, `generated/cli/` (generated; do not handwrite)

**Gate:** `cargo test -p okf-jawn-cli` and `cargo run -p okf-jawn-cli -- --help`

**Receipt:** canonical commands and aliases match the operation table; no credentials in argv.

The executable invokes HTTP operations; it does not open mutable storage directly. Keep canonical commands and operator aliases. Add ergonomic flags using existing Clap APIs without maintaining a separate schema. Never pass credentials as command arguments or permit redirects to leak tokens.

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
