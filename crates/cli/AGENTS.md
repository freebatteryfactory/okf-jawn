# Core / CLI lane — CLI

Read root AGENTS.md and SPEC.md.

This lane's directories and gate command are in the root AGENTS.md table, and its construction gates are the `verification.json` entries whose `owner` is `core-cli` (each names its command). `generated/cli/` is generator output; never handwrite it. Canonical commands and aliases match the operation table, and no credential appears in argv.

The executable invokes HTTP operations; it does not open mutable storage directly. `mcp --stdio` is the stdio relay of SPEC §11, and this lane builds it: it forwards each MCP message to the running local service with a connector credential and holds no store of its own; it starts the service when none is running and otherwise connects to the running one, so it never becomes a second writer. Keep canonical commands and operator aliases. Add ergonomic flags using existing Clap APIs without maintaining a separate schema. Never pass credentials as command arguments or permit redirects to leak tokens.

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
