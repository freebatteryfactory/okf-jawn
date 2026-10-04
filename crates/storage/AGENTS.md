# Storage lane

Read root AGENTS.md and SPEC.md.

Implement core::storage, core::jobs and core::search traits using git2, rusqlite and object_store. The SearchIndex implementation owns SQLite FTS tables, index updates and rebuild mechanics; it never shares a rebuild path with jobs, reviews or receipts. Read `bun scripts/dev.mjs vendor okf` before reimplementing relocation or link/index behavior. Preserve bytes/occurrences/digests and historical references. Complete recovery and backup behavior; no telemetry-only receipts.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
