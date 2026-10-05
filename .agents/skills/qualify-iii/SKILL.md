---
name: qualify-iii
description: Qualify the selected durable execution path.
---

# Qualify the selected durable execution path

Read exact engine/worker/adapter documentation and licenses. iii (engine v0.24.4, iii-sdk 0.24.4, builtin `file_based` queue) passed Phase 0 library qualification via `qualify iii` and is the selected runtime. The construction gate repeats the check against the product: submit an actual import; kill immediately after acknowledgment; restart and retry; verify original persistence, one occurrence, one completed commit and correct scope against the real RecordStore. Verify channel transfer and trace linkage. Capture artifact hashes/config, not a vague latest version.

```sh
bun scripts/dev.mjs vendor iii
```
