---
name: qualify-iii
description: Qualify the selected durable execution path.
---

# Qualify the selected durable execution path

Read exact engine/worker/adapter documentation and licenses. No iii implementation is adopted yet; the in-process Tokio worker over RecordStore is the execution path until iii passes this same check. Submit actual import; kill immediately after acknowledgment; restart and retry; verify original persistence, one occurrence, one completed commit and correct scope. Run the identical check against the in-process worker before claiming its crash durability. Verify channel transfer and trace linkage. Capture artifact hashes/config, not a vague latest version.

```sh
bun scripts/dev.mjs vendor iii
```
