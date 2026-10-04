# Integration owner

Read root AGENTS.md and SPEC.md.

Use the actual vendor generators. Keep generation independent, deterministic, and failure-transparent. Generated file deletion must be detected too. No handwritten Hey API substitutes. Schema tests compare semantics; snapshots only identify drift.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
