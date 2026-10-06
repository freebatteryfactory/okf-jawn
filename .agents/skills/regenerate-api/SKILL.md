---
name: regenerate-api
description: Regenerate real shared interfaces.
---

# Regenerate real shared interfaces

Run vendor lookup for schemars and hey-api. Run lock only when a lockfile is absent or a selected dependency deliberately changed; bootstrap never resolves. Run gen, inspect generated differences, then gen-check. No runtime service is required. Never manually replace generator output. Run semantic fixture tests, the TypeScript 7 type check, and the generated UI consumer build.

```sh
bun scripts/dev.mjs gen
bun scripts/dev.mjs gen-check
```
