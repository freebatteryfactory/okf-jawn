# Views lane — MCP Apps bundles

Read root AGENTS.md and SPEC.md.

This lane's directories and gate command are in the root AGENTS.md table, and its construction gates are the `verification.json` entries whose `owner` is `views` (each names its command). `ui/dist-apps/` is build output.

Import the actual feature components. Keep the official App bridge and generated response validators. Do not install @json-render/mcp or a Node MCP server. Bundling and wire checks are not proof that Claude or ChatGPT rendered the UI. Capture actual host results, theme/sizing/accessibility and page/selection context behavior.

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
