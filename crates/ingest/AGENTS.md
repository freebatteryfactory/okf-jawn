# Ingestion lane

Read root AGENTS.md and SPEC.md.

Implement core::conversion::Converter and import execution, not a second application API. Use the pinned Docling release through its actual API and structured ranges; no Python runtime. Docling is feature-sliced (`default-features = false`, `features = ["pdf"]`); do not enable `asr`, `fetch-images` or `vlm` without a deliberate shared change. Record its real native/model asset requirements with verified sources, versions and hashes. Use `okf-core` for concept and document construction when producing candidate bundle content; do not define OKF validity here—application policy in core validates before commit. Preserve originals, extracted locators and human corrections; pending or failed conversion keeps the bytes and reports that state. Execute jobs in the in-process Tokio worker from durable RecordStore claims; retries are idempotent and no crash durability is claimed before the restart check passes. Qualify native assets and failure limits.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
