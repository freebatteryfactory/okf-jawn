# Core / CLI lane

Read root AGENTS.md and SPEC.md.

The executable invokes HTTP operations; it does not open mutable storage directly. Keep canonical commands and operator aliases. Add ergonomic flags using existing Clap APIs without maintaining a separate schema. Never pass credentials as command arguments or permit redirects to leak tokens.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
