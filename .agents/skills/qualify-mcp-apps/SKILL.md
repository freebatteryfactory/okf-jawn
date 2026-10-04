---
name: qualify-mcp-apps
description: Qualify connector and host visuals.
---

# Qualify connector and host visuals

Run the wire probe against the actual endpoint, then use an actual MCP Apps-compatible host to call read/diff/log/present. Verify returned data, source click/range, accessible table, theme/sizing and text fallback. A served HTML resource alone is not visual acceptance. Do not claim a screenshot from a local harness is ChatGPT or Claude.

```sh
bun scripts/dev.mjs qualify mcp-wire
```
