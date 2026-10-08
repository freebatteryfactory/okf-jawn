# MCP tool execution lane

Read root AGENTS.md and SPEC.md.

This lane's directories and gate command are in the root AGENTS.md table, and its construction gates are the `verification.json` entries whose `owner` is `mcp-execution` (each names its command).

The lane name `mcp-execution` means MCP tool execution (protocol binding and tool/resource handlers), not job runtime execution. Job runtime is ingest's Tokio + RecordStore adapter.

Use rmcp 3.5 constructors and selected protocol APIs. Implement the real ServerHandler and serve it over Streamable HTTP using the generated tools; the current library is adapters only. This crate serves no stdio transport: a local client that launches a process reaches the service through the CLI's `mcp --stdio` relay (SPEC §11), which the core-cli lane builds. Locally, authenticate with an owner-issued connector secret, never the browser session; hosted, with WorkOS Connect. Preserve text, images, structured output, UI metadata, app-only visibility and independent server permissions. Tokio + RecordStore is the selected job runtime underneath the application, never a substitute for MCP Apps support.

Tool results go through this crate's helpers. `read_result` puts the document Markdown in `content` within the request's byte budget and the whole response in `structuredContent`; `structured_result` does the same with the serialized response for every other tool. `error_result` returns the serialized `ApiError` as text with `isError` and no `structuredContent`, because each tool's generated `outputSchema` describes its success response only. Do not pass a hand-written summary as the text, and do not use `CallToolResult::structured_error`.

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
