# Core / CLI lane

Read root AGENTS.md and SPEC.md.

Implement `ports::Application` for `application::ApplicationService` in cohesive child modules of `crates/core/src/application/`, using only the injected `Ports`. Core never depends on the storage, ingest, server, mcp or cli crates. Capability checks and exact revision semantics remain shared. Coordinate SearchIndex updates and rebuilds with content changes; rebuild never touches RecordStore. Connector operations are available to the local owner only. Add production behavior, not an in-memory stand-in. Coordinate CLI ergonomics with the complete operation contract.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
