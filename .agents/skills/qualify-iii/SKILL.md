---
name: qualify-iii
description: Qualify the selected durable execution path.
---

# Qualify the selected durable execution path

Read exact engine/worker/adapter documentation and licenses. iii (engine v0.24.4, iii-sdk 0.24.4, builtin `file_based` queue) was Phase 0 qualified and ended **REJECTED WITH FALLBACK**: engine+queue+worker kill before Ok, effect-ledger idempotency, blob-path payloads, startup/RSS and tagged ELv2 were observed, but `dlq_messages` browse stayed empty so inspected DLQ could not be claimed. The selected product runtime is Tokio + `RecordStore` (SPEC sections 5 and 12). Re-running `qualify iii` is for evidence refresh only unless a shared decision re-adopts iii after a terminal PASS.

The construction gate repeats the check against the product: submit an actual import; kill immediately after acknowledgment; restart and retry; verify original persistence, one occurrence, one completed commit and correct scope against the real RecordStore. Verify channel transfer and trace linkage. Capture artifact hashes/config, not a vague latest version.

```sh
bun scripts/dev.mjs vendor iii
```
