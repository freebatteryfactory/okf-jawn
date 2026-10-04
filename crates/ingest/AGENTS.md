# Ingestion lane

Read root AGENTS.md and SPEC.md.

Implement core::conversion::Converter and import execution, not a second application API. Use the pinned Docling release through its actual API and structured ranges; no Python runtime. Record its real native/model asset requirements with verified sources, versions and hashes. Preserve originals, extracted locators and human corrections; pending or failed conversion keeps the bytes and reports that state. Execute jobs in the in-process Tokio worker from durable RecordStore claims; retries are idempotent and no crash durability is claimed before the restart check passes. Qualify native assets and failure limits.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
