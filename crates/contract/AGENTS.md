# Integration owner — contract

Read root AGENTS.md and SPEC.md.

This lane's directories and gate command are in the root AGENTS.md table, and its construction gates are the `verification.json` entries whose `owner` is `integration-owner` (each names its command). Operations and schemas regenerate cleanly and the semantic tests pass.

Own wire types, semantic descriptions, source and revision identities, operation declarations and generated schema agreement. Do not depend on storage or runtime startup. Regeneration must work before the application exists.

A lane may change its own authored generator inputs, run `gen`, and commit outputs; gen-check must pass on that branch. The integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. Return concrete boundary changes to the integration owner.
