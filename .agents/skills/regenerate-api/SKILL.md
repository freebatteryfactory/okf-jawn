---
name: regenerate-api
description: Regenerate real shared interfaces.
---

# Regenerate real shared interfaces

Run vendor lookup for utoipa, schemars and hey-api. Run bootstrap only to resolve absent lockfiles. Run gen, inspect generated differences, then gen-check. No runtime service is required. Never manually replace generator output. Run semantic fixture tests plus the generated UI consumer build.

```sh
node scripts/dev.mjs gen
node scripts/dev.mjs gen-check
```
