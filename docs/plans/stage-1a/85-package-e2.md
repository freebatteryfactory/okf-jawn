## Package E2: Dispatch hardening

Added after verification of package E. The E verifier found no blocking defect, but its notes
include real gaps in a layer the storage lane is about to implement against (the ledger port)
and security rules that no test pins. This package closes them in one pass.

- **Branch:** `cure/dispatch-hardening`, cut from `integration/foundation-cure` after package H
  is merged.
- **Worktree:** `D:\okf\cure\dispatch-hardening`.
- **Runs in parallel with:** package T (scripts, hooks, CI, `tests/foundation`, `qualification/record.mjs`,
  `crates/mcp/tests`, `README.md`). No shared files.
- **Files allowed (exact):**
  - `crates/core/src/dispatch.rs`, `crates/core/src/mutations.rs`, `crates/core/src/context.rs`,
    `crates/core/src/access.rs`
  - `crates/core/src/lib.rs` (the one stale module doc line only)
  - `crates/core/tests/dispatch.rs`, `crates/core/tests/authorization.rs`,
    `crates/core/tests/support/**`
  - `crates/core/AGENTS.md`, `crates/storage/AGENTS.md` (only the sentences that describe the
    ledger port and the lane receipt)
  - `crates/server/src/lib.rs` (the dispatch call site only), `crates/server/tests/bindings.rs`
    (only what a changed fixture API forces)
  - `docs/plans/stage-1a/00-shared-interfaces.md` (the `MutationStore` block only)
- **Must not touch:** `crates/contract/**`, every other `crates/core/src/*.rs`, manifests,
  lockfiles, generated directories, scripts, CI.
- **SPEC sentences served:** §8 "Every non-read mutation carries a durable MutationId under an
  idempotency key scoped to tenant, subject and operation"; §11 "Principal identity is not
  authorization … effective permissions come from AccessControl as WorkspaceGrant and
  TenantGrant" and "Subject and client identity are retained separately"; §13 "Universal refusal
  is a product failure"; §14 lint policy.

All cargo commands run from PowerShell. Commits use the shared format with both trailers. Tests
use the shared idiom (`tests/support/check.rs`): no `unwrap`, `expect`, `panic!`, indexing, no
`#[allow]` / `#[expect]`. Every new test goes through `dispatch` unless it is a named fixture
self-test, and must fail when the rule it guards is removed; the acceptance section lists the
mutation for each.

### Task E2.1: The lease has a holder

Today `complete` and `release` take only a `MutationId`. A slow first attempt whose lease expired
and the resumed attempt that took it over can run at once under one id, and either can end the
other's lease.

**Interfaces — Produces** (update `00-shared-interfaces.md` to match):

```rust
// crates/core/src/mutations.rs
/// One granted lease on a mutation. `token` is different for every grant of the same mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MutationLease {
    /// The durable write identity.
    pub mutation_id: MutationId,
    /// Changes every time the lease is granted; a stale holder cannot complete or release.
    pub token: u64,
}

pub enum BeginOutcome {
    New(MutationLease),
    Replay(StoredResponse),
    Conflict { operation: OperationName },
    InProgress { mutation_id: MutationId, retry_after: u32 },
    Abandoned { lease: MutationLease },
}

pub trait MutationStore: Send + Sync {
    fn begin<'a>(&'a self, key: &'a MutationKey, digest: &'a Digest) -> PortFuture<'a, BeginOutcome>;
    /// Compare-and-set on the lease: fails with `ErrorCode::Conflict` when `lease` is no longer
    /// the current grant (it expired and another attempt holds the mutation).
    fn complete(&self, lease: MutationLease, response: Value) -> PortFuture<'_, ()>;
    /// Ends the lease only if `lease` is the current grant; a stale lease is a no-op.
    fn release(&self, lease: MutationLease) -> PortFuture<'_, ()>;
}
```

`OperationContext.mutation` stays `Option<MutationId>`: handlers need the identity, not the
lease. Dispatch keeps the lease locally.

- [ ] Write failing tests through `dispatch` with the fixture ledger extended to expire a lease
  on demand: (a) attempt 1 is parked in its handler, its lease is expired, attempt 2 begins and
  gets `Abandoned` with a new token and completes; when attempt 1's handler then returns, its
  `complete` is refused and the caller of attempt 1 gets `ErrorCode::Conflict`; the stored
  response is attempt 2's. (b) Attempt 1 fails after its lease was taken over: its `release`
  does not end attempt 2's lease (a third begin during attempt 2 is `InProgress`).
- [ ] Run; see them fail to compile (no `MutationLease`).
- [ ] Implement the trait change, the fixture, and dispatch.
- [ ] Run `cargo test --locked -p okf-jawn-contract -p okf-jawn-core`; see them pass.
- [ ] Commit.

### Task E2.2: A failure after a successful handler does not strand the lease

After the handler returns `Ok`, three things can still fail: response serialization, building the
ledger body, and `complete`. Today each returns its error and leaves the lease held, so retries
get `InProgress` until the lease expires.

- [ ] Give the fixture ledger a switch that makes `begin`, `complete` or `release` return an
  error once.
- [ ] Write failing tests through `dispatch`: (a) `begin` fails → the handler does not run and
  the caller gets that error; (b) `complete` fails with an error that is not a lost lease → the
  caller gets that error, and an immediate retry with the same key is `Abandoned` (runs as
  `Resumed`), not `InProgress`; (c) the handler fails and `release` also fails → the caller gets
  the HANDLER's error; (d) the ledger body cannot be built for an `AlreadyIssued` operation
  (pointer does not resolve) → `Internal`, lease released, and nothing secret is stored.
- [ ] Implement: on any post-handler failure other than a lost lease, call `release(lease)`
  best-effort and return the original error.
- [ ] Run, pass, commit.

### Task E2.3: Rules that no test pins

- [ ] Tenant-level authorization, through `dispatch`, with a fixture `AccessControl` that RETURNS
  a tenant grant (so the product, not the fixture, is the refuser): a tenant grant without
  `Admin` cannot `create_workspace`; a connector (`McpDelegation`) whose delegator holds tenant
  `Admin` cannot `create_workspace` (route rule); a delegation ceiling without `Admin` cannot
  either (ceiling rule); a principal with tenant `Admin` can (positive control).
- [ ] Ledger key scope: two principals with the same subject and key in different tenants are
  independent; the same subject and key on two different operations are independent.
- [ ] Add `client_id: Option<String>` to `MutationKey`, filled from `principal.client_id`, with a
  test that a connector sending the subject's key and body does not replay the subject's stored
  response and does not conflict with it.
- [ ] Digest is taken over the typed request: a request with an optional field omitted and the
  same request with that field explicitly `null` replay each other.
- [ ] `dispatch` refuses a request whose `targets()` is empty with `ErrorCode::Internal` before
  the handler runs. (No contract request returns an empty list today; test it with a request
  type defined in the test crate if the macro allows one, otherwise as a unit test of the
  target-authorization function in `dispatch.rs`.)
- [ ] Each of the above: failing first, then implement or confirm, run, commit (one commit per
  bullet is fine; group only what one reviewer would accept or reject together).

### Task E2.4: Smaller hardening

- [ ] Box the large inner future inside `dispatch` so the public `async fn dispatch` future is
  small and call sites need no `Box::pin`; remove the `Box::pin` at
  `crates/server/src/lib.rs`'s call site and confirm `clippy::large_futures` stays quiet there.
- [ ] Bound what an `InvalidInput` message echoes of the caller's value: at most 512 bytes of
  validator text, cut on a character boundary with `…`. Test with a 100 KB string value.
- [ ] Retention rule in the `mutations.rs` module doc and in `crates/storage/AGENTS.md`:
  completed rows and RELEASED rows are kept 7 days; only a row whose lease expired without
  `complete` or `release` stays until it is reconciled.
- [ ] Stale prose: `crates/core/src/lib.rs` (the `mutations` module line still says
  "abandoned-effect lookup"); `crates/core/AGENTS.md` (the lane receipt still lists
  "confirmation re-consume"; confirmation single-use is a storage-lane gate).
- [ ] Commit.

### Package E2 acceptance

Run in `D:\okf\cure\dispatch-hardening` at the tip, cargo from PowerShell.

| # | Command | Expected |
| --- | --- | --- |
| 1 | `git diff --name-only <base> HEAD` | only paths in "Files allowed" |
| 2 | `cargo fmt --check -p okf-jawn-core -p okf-jawn-server` | exit 0 |
| 3 | `cargo clippy --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server --all-targets -- -D warnings` | exit 0, no diagnostic |
| 4 | `cargo test --locked -p okf-jawn-contract -p okf-jawn-core -p okf-jawn-server` | 0 failed |
| 5 | `git grep -nE "\.unwrap\(\)|\.expect\(|panic!|#\s*!?\[\s*(allow|expect)" -- crates/core/src crates/core/tests crates/server/src crates/server/tests tests/support` | no output |
| 6 | `git grep -n "Box::pin(dispatch" -- crates` | no output |

Mutations the verifier repeats (each must turn a named test red, then be reverted): make
`complete` ignore the token; make `release` ignore the token; do not release after a failed
`complete`; let a `release` error replace the handler's error; remove the permission check in
`access::authorize_tenant`; remove the route check there; remove the delegation intersection
there; use a constant tenant in `MutationKey`; use a constant operation in `MutationKey`; drop
`client_id` from `MutationKey`; hash the raw input instead of the typed request; accept an empty
target list; remove the 512-byte bound.

### Deviations

To be filled by the implementer: anything above that cannot be done as written, with evidence.
A change to a shared interface beyond the `MutationStore` block is a stop-and-report.
