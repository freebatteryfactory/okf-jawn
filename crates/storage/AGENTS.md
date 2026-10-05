# Storage lane

Read root AGENTS.md and SPEC.md.

**Directories:** `crates/storage/`

**Gate:** `cargo test -p okf-jawn-storage --features runtime`

**Receipt:** BlobStore, VersionStore, WorkspaceCatalog, RecordStore, ProposalStore, UploadStore, EventLog, CredentialStore, SearchIndex and ReadinessProbe implementations pass their tests; rebuild never touches jobs/reviews/receipts.

Implement the store ports in `core::storage`, `core::jobs`, `core::search`, `core::proposals`, `core::uploads`, `core::events`, `core::credentials` and `core::readiness` using git2, rusqlite and object_store (`fs`). The SearchIndex implementation owns SQLite FTS tables, index updates and rebuild mechanics; it never shares a rebuild path with jobs, reviews or receipts. Use `okf-core` for bundle load/model, relocation, link graph and index regeneration; do not reimplement those. Enforce storage invariants only; OKF conformance and lint are application policy in core and must not be decided here. Preserve bytes/occurrences/digests and historical references. Complete recovery and backup/restore behavior; no telemetry-only receipts.

ingest owns `JobHandler` and the Tokio runtime adapter; storage owns the durable stores those handlers write through.

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
