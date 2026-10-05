# MCP tool execution lane

Read root AGENTS.md and SPEC.md.

**Directories:** `crates/mcp/`

**Gate:** `cargo test -p okf-jawn-mcp`; against a running endpoint, `bun scripts/dev.mjs qualify mcp-wire`

**Receipt:** ServerHandler + stdio/Streamable HTTP use generated tools; wire check logs under `.artifacts/` when run

The lane name `mcp-execution` means MCP tool execution (protocol binding and tool/resource handlers), not job runtime execution. Job runtime is ingest's Tokio + RecordStore adapter.

Use rmcp 3.5 constructors and selected protocol APIs. Implement real ServerHandler and stdio/Streamable HTTP service using the generated tools; the current library is adapters only. Locally, authenticate with an owner-issued connector secret, never the browser session; hosted, with WorkOS Connect. Preserve text, images, structured output, UI metadata, app-only visibility and independent server permissions. Tokio + RecordStore is the selected job runtime underneath the application, never a substitute for MCP Apps support.

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
