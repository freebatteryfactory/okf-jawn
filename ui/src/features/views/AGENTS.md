# Views lane

Read root AGENTS.md and SPEC.md.

This lane's directories and gate command are in the root AGENTS.md table, and its construction gates are the `verification.json` entries whose `owner` is `views` (each names its command). Rendering in a host is the MCP Apps qualification, not this lane's unit results.

Own cited saved Views, json-render catalog/renderer, Vega-Lite interpreter/chart/table, source resolution and export. No agent-written source text or retyped authoritative numbers. Pinned views do not silently refresh; live results show resolved revisions. Complete catalog chart bindings and ranges, not just a chart-shaped mock.

MCP Apps host bundles live under `ui/src/mcp-apps/`. WebMCP and the HTML sandbox viewer are workspace-ui; sandbox origin serving is server.

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
