# Human workspace lane (workspace-ui)

Read root AGENTS.md and SPEC.md.

**Directories:** `ui/` except `ui/src/features/views` and `ui/src/mcp-apps` (those are the views lane)

**Gate:** `bun --bun run --cwd ui test` and `bun --bun run --cwd ui typecheck`

**Receipt:** Explorer unit/e2e results; WebMCP and HTML sandbox viewer behave under the configured sandbox origin

Keep the complete Explorer, tree/search/tabs/split panes, rich viewers/editing/properties, naming form+YAML+preview, graph, history/proposals/review and Attention. Own WebMCP integration and the HTML sandbox viewer that loads hostile HTML from the server's configured separate sandbox origin (server owns origin serving). Use existing packages. Generated SDK inputs are body/path/query groups, not guessed flat signatures. Router owns URL, Query owns server cache, Zustand ephemeral state. API/source identity is not replicated as an alternate store.

Generator-input rule: a lane may change its own authored generator inputs (for example openapi-ts config owned here), run `bun scripts/dev.mjs gen`, and commit outputs; gen-check must pass on the lane branch; the integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
