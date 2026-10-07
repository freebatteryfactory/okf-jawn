# Storage lane

Read root AGENTS.md and SPEC.md.

**Directories:** `crates/storage/`

**Gates** (construction gates in `verification.json` owned by this lane):

| Gate | Command |
| --- | --- |
| `storage-git-cas-sqlite` | `cargo test -p okf-jawn-storage --features runtime` |
| `mutation-crash-reconcile` | `cargo test -p okf-jawn-storage --features runtime -- mutation_crash_reconcile` |
| `confirmation-single-use` | `cargo test -p okf-jawn-storage --features runtime -- confirmation_single_use` |
| `draft-per-editor` | `cargo test -p okf-jawn-storage --features runtime -- draft_per_editor` |

**Receipt:** every port below passes its tests against real Git, SQLite and the local object store; rebuilding search never touches jobs, reviews or receipts.

## Ports to implement

| Port | Defined in | Built with |
| --- | --- | --- |
| `BlobStore` | `core::storage` | `object_store` (`fs`) |
| `VersionStore` | `core::storage` | `git2`, `okf-core` |
| `WorkspaceCatalog` | `core::storage` | `rusqlite`, `git2`, `okf-core` |
| `RecordStore` | `core::jobs` | `rusqlite` |
| `MutationStore` | `core::mutations` | `rusqlite` |
| `DraftStore` | `core::drafts` | `rusqlite` |
| `ConfirmationStore` | `core::confirmations` | `rusqlite` |
| `SandboxCapabilityStore` | `core::sandbox` | `rusqlite` |
| `ProposalStore` | `core::proposals` | `rusqlite` |
| `UploadStore` | `core::uploads` | `rusqlite`, `object_store` |
| `EventLog` | `core::events` | `rusqlite` |
| `CredentialStore` | `core::credentials` | `rusqlite` |
| `SearchIndex` | `core::search` | `rusqlite` FTS, `okf-core` |
| local `AccessControl` | `core::access` | `rusqlite` |
| `ReadinessProbe` | `core::readiness` | the stores above |

## Rules

- **A repeated `MutationId` is a no-op that returns the prior row.** This holds for `RecordStore::create_job`, `retry_job`, `cancel_job`, `record_artifact`, `insert_review` and `insert_receipt` (when it is given one), `WorkspaceCatalog::create`, `update` and `archive`, `ConfirmationStore::create`, `DraftStore::save` and `discard` (keyed with the item), `ProposalStore::insert` and `add_comment`, `UploadStore::create`, and `CredentialStore::create_connector`, which answers `ConnectorIssue::Existing` and never a second secret. Enforce it with a unique column, not a lookup. There are no `find_*_by_mutation` methods.
- **The mutation ledger leases by token and keeps finished rows 7 days.** The key is (tenant, subject, client, operation, idempotency key). `MutationStore::begin` grants a `MutationLease { mutation_id, token }`, and every grant of the same mutation carries a token no earlier grant carried. `complete` is a compare-and-set on that token: it fails with `Conflict` and stores nothing when the lease is no longer the current grant (it expired and another attempt holds the mutation). `release` ends the lease only for the current grant; under a stale lease it changes nothing. Completed rows and released rows are kept 7 days; only a row whose lease expired without `complete` or `release` stays until it is reconciled. `Conflict` is returned for a lost lease and for nothing else (other failures use `Unavailable` or `Internal`), and releasing a completed mutation is a no-op. Construction gates: `mutation-lease-compare-and-set` (a superseded attempt can neither complete nor release; both takeover orders) beside `mutation-crash-reconcile`. The lease length is the storage lane's choice; record it here when it is set.
- **`VersionStore::commit` is idempotent on `MutationId`.** Every commit carries an `Okf-Jawn-Mutation:` trailer. Before writing, scan `expected_head..head`; a commit with the trailer is returned with `replayed: true`. `mutation-crash-reconcile` proves it: the commit succeeds, `MutationStore::complete` is never written, the retry calls `commit` again with the same id and gets the first revision back; no second commit exists. `find_commit` exists only for Snapshot, whose expected head is not stable across attempts.
- **Stage, check, then commit.** `commit` and `create_candidate` materialize the base tree in `<data>/staging/<tenant>/<workspace>/<mutation>`, apply the `TreeEdit`s there with `okf-core` (moves, link rewrites, index regeneration), call the `CandidateCheck` they were given on that directory, and only then write the tree and the commit and move the reference. Remove the directory when the call returns, success or failure; remove anything a crash left under `<data>/staging` at startup. Never decide OKF conformance here: that is the check's job, and the check is core's.
- **No selectors, identities or wire requests.** Ports take a `StorageScope` or `TenantId`, resolved `Revision`s and core parameter types. `WorkspaceCatalog` never filters and returns `Workspace::permissions` empty.
- **Hashes are defined in core.** Store and look up connector secrets with `core::credentials::secret_hash`; sandbox tokens arrive already hashed with `core::sandbox::token_hash`. Never store a plaintext secret or token.
- **Bytes have no media type.** `BlobStore` keys objects by tenant and SHA-256; the media type lives on the source card, the asset or the sandbox capability that refers to the object.
- **An artifact is a record over a blob.** Export, backup and View-export bytes are retained in `BlobStore` like any other object; `RecordStore::record_artifact` maps an `ArtifactId` to `{ kind, digest, size, media type, producing job }` and `get_artifact` reads it back inside one workspace. `Job.artifact` is `ArtifactRecord::download` of that record. Artifact rows are application records: a backup includes them and rebuilding search never touches them.
- **Drafts are never indexed, exported or committed implicitly.** `SearchIndex` reads committed trees only.

ingest owns `JobHandler(&ClaimedJob)` and the Tokio runtime adapter; storage owns the durable stores those handlers write through, and returns the `JobSpec`, initiator and `MutationId` it was given, unchanged, on every claim.

Generator-input rule: change only this lane's authored inputs; run `gen` and commit outputs; gen-check must pass; integration owner regenerates at merge.

Do not alter shared manifests, operation declarations, generator output, or protected acceptance as a private workaround. When real code shows a port is wrong, stop and report the symbol, the SPEC sentence and the failing output to the integration owner.
