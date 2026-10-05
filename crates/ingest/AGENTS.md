# Ingestion lane

Read root AGENTS.md and SPEC.md.

**Directories:** `crates/ingest/`

**Gate:** `cargo test -p okf-jawn-ingest --features runtime`, then the construction crash/restart check against the real RecordStore and import path

**Receipt:** conversion + JobHandler tests; restart/idempotency evidence under `.artifacts/` (not a claim until the SPEC §12 check passes)

Implement `core::conversion::Converter` and `core::jobs::JobHandler`, not a second application API. Own the selected Tokio + RecordStore runtime adapter (in-process worker over durable job truth). iii Phase 0 qualification ended REJECTED_WITH_FALLBACK; do not re-adopt iii or add iii-sdk as a product dependency. Use the pinned Docling release through its actual API and structured ranges; no Python runtime. Docling is feature-sliced (`default-features = false`, `features = ["pdf"]`); do not enable `asr`, `fetch-images` or `vlm` without a deliberate shared change. Record its real native/model asset requirements with verified sources, versions and hashes. Use `okf-core` for concept and document construction when producing candidate bundle content; do not define OKF validity here—application policy in core validates before commit. Preserve originals, extracted locators and human corrections; pending or failed conversion keeps the bytes and reports that state. Durable job truth remains in RecordStore (storage lane); queue delivery is not application completion. Handlers must be idempotent against the durable job identity.

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
